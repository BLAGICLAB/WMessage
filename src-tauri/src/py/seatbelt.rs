//! macOS seatbelt 出网硬闸（run_python / apps 同一沙箱，一处包装两端生效）。
//!
//! 设计要点（方案 docs/RUN-PYTHON-SECURITY-REFIT-PLAN-2026-10-31.md §5.2）：
//! - profile 用 `(deny network*)` 前缀通配——socket 全家（outbound/inbound/bind）
//!   一并拒，比只 deny remote 更严：脚本摸本机 loopback 服务同样是外传暗道。
//! - **自证式探测**（沿 SETRLIMIT_PROBE_SCRIPT 的诚实哲学）：探测脚本在 profile 内
//!   尝试连回环上肯定无人监听的端口——闸生效 = EPERM（即时拒绝）；返回
//!   ECONNREFUSED / 竟然连上 / 其他任何形态 = profile 未生效，按闸失效处置。
//! - **失败关闭（fail-closed），无放行开关**：sandbox-exec 缺失、profile 被拒、
//!   探测输出未识别，一律拒跑 run_python 并审计。探测结果进程内缓存。
//! - Windows 无廉价对等物，维持提示词层软约束（跨平台口径分裂是认下的代价）。
//! - 限制随 fork/exec 继承：python 起的子进程（含 os.system("curl …")）同断网，
//!   这是它比 rlimit / 提示词都硬的点。

use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// 最小 profile：其余操作全放行（沙箱里跑 python 要 fork/exec/file 等），
/// 网络操作前缀通配全拒。
pub(crate) const SEATBELT_PROFILE: &str = "(version 1)\n(allow default)\n(deny network*)\n";

/// 自证探测脚本：连 127.0.0.1:1（loopback 上肯定没人监听的端口）。
/// 生效形态唯一：errno 1（EPERM，macOS）——内核直接拒 socket 连接。
/// ECONNREFUSED(61) = profile 没拦（内核都到得了端口）= 闸未生效。
const SEATBELT_PROBE_SCRIPT: &str = r#"import socket
s = socket.socket()
try:
    s.connect(("127.0.0.1", 1))
    print("SB_CONNECT_OK")
except OSError as e:
    print("SB_ERR", e.errno)
finally:
    s.close()
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SeatbeltState {
    /// 探测证明闸生效（sandbox 内 socket 被内核以 EPERM 拒）
    Available,
    /// 闸失效或无法证明生效——一律拒跑（fail-closed）
    Unavailable(String),
}

static SEATBELT_PROBE: OnceLock<SeatbeltState> = OnceLock::new();

/// 探测主体（无缓存，测试直打）：sandbox-exec 跑探测脚本，判定输出。
pub(crate) fn seatbelt_probe_uncached(py: &str) -> SeatbeltState {
    let mut cmd = Command::new("sandbox-exec");
    cmd.arg("-p")
        .arg(SEATBELT_PROFILE)
        .arg(py)
        .arg("-c")
        .arg(SEATBELT_PROBE_SCRIPT)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // 探测面与执行面同口径：环境走白名单裁剪——宿主环境若被注入
    // PYTHONPATH/sitecustomize，探测 python 会先执行注入代码，可把
    // 「闸失效」伪造成「生效」
    crate::py::runtime::apply_child_env(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return SeatbeltState::Unavailable(format!("sandbox-exec 无法启动：{e}"));
        }
    };
    // deadline 与 rlimit 探测同口径（10s 强杀）：探测在 gate 许可持有期内
    // 调用且不接受 stop 令牌，无界阻塞会占死许可并使本次 run_python 永不返回。
    // 超时按 Unavailable（fail-closed 方向）。
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return SeatbeltState::Unavailable(
                    "seatbelt 探测子进程超时（10s）已强杀，闸状态无法证明".to_string(),
                );
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => {
                return SeatbeltState::Unavailable(format!("探测子进程 wait 失败：{e}"));
            }
        }
    };
    let Some(status) = status else {
        return SeatbeltState::Unavailable("探测子进程状态未知，闸状态无法证明".to_string());
    };
    if !status.success() {
        return SeatbeltState::Unavailable(format!(
            "sandbox-exec 探测失败（退出码 {:?}；profile 语法被拒或二进制不可用）",
            status.code()
        ));
    }
    let stdout_bytes = child_stdout_or_empty(&mut child);
    let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
    judge_probe_output(&stdout)
}

fn child_stdout_or_empty(child: &mut std::process::Child) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = std::io::Read::read_to_end(&mut s, &mut buf);
    }
    buf
}

/// 探测输出判定（纯函数，测试直打）。
///
/// errno 精确解析：EPERM=1 是闸生效的唯一证明形态。子串
/// `contains("SB_ERR 1")` 会把 errno 10-19/100-199 全吞进 Available 分支
/// （闸失效被误判成生效 = fail-open），必须 == 1 才算生效。
fn judge_probe_output(stdout: &str) -> SeatbeltState {
    if let Some(errno) = parse_sb_errno(stdout) {
        return match errno {
            1 => SeatbeltState::Available,
            61 => SeatbeltState::Unavailable(
                "沙箱内 socket 到了内核（ECONNREFUSED 而非 EPERM）——deny network* 未拦截"
                    .to_string(),
            ),
            other => SeatbeltState::Unavailable(format!(
                "沙箱探测输出未识别（闸状态无法证明）：SB_ERR {other}"
            )),
        };
    }
    if stdout.contains("SB_CONNECT_OK") {
        return SeatbeltState::Unavailable("沙箱内 socket 连接竟然成功——网络闸未生效".to_string());
    }
    SeatbeltState::Unavailable(format!(
        "沙箱探测输出未识别（闸状态无法证明）：{}",
        stdout.trim()
    ))
}

/// 逐行找 `SB_ERR <整数>`（取第一条可解析的）。
fn parse_sb_errno(stdout: &str) -> Option<i32> {
    stdout.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("SB_ERR ")?.trim();
        rest.parse::<i32>().ok()
    })
}

/// 带缓存探测。首次结果 audit 留痕；Unavailable 由调用方拒跑。
///
/// 探测解释器固定用 cached_python——闸生效性是**系统属性**，与本次 run 传进来
/// 的 python 路径无关（spawn_fail 测试传假路径也能走到真正的 spawn 失败分支，
/// 而不是被探测误判成闸失效）。
pub(crate) fn seatbelt_probe_cached(audit: &mut dyn FnMut(&str)) -> Result<(), String> {
    let state = SEATBELT_PROBE.get_or_init(|| {
        let Some(py) = crate::py::env::cached_python() else {
            return SeatbeltState::Unavailable("本机未检测到 Python，无法自证探测".to_string());
        };
        let s = seatbelt_probe_uncached(&py);
        match &s {
            SeatbeltState::Available => {
                audit(
                    "run_python | seatbelt_probe | 出网闸生效（sandbox 内 socket 被拒，探测证明）",
                );
            }
            SeatbeltState::Unavailable(why) => {
                audit(&format!(
                    "run_python err | kind=seatbelt_unavailable | {why}"
                ));
            }
        }
        s
    });
    match state {
        SeatbeltState::Available => Ok(()),
        SeatbeltState::Unavailable(why) => Err(why.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// profile 语义锚点：default allow 兜底 + network 前缀通配 deny。
    /// deny 规则若被写成枚举式（只 deny outbound），inbound/bind 会漏——
    /// 这里钉死前缀通配的写法。
    #[test]
    fn seatbelt_profile_denies_all_network() {
        assert!(SEATBELT_PROFILE.contains("(allow default)"));
        assert!(SEATBELT_PROFILE.contains("(deny network*)"));
        assert!(
            !SEATBELT_PROFILE.contains("allow network"),
            "不得给任何网络放行条款"
        );
    }

    /// errno 精确判：只有 EPERM(1) 算闸生效——子串 contains("SB_ERR 1")
    /// 会把 errno 12/13/100 全吞进 Available 分支（fail-open）。
    #[test]
    fn judge_requires_exactly_eperm_errno() {
        assert_eq!(judge_probe_output("SB_ERR 1\n"), SeatbeltState::Available);
        assert!(matches!(
            judge_probe_output("SB_ERR 12\n"),
            SeatbeltState::Unavailable(_)
        ));
        assert!(matches!(
            judge_probe_output("SB_ERR 13\n"),
            SeatbeltState::Unavailable(_)
        ));
        assert!(matches!(
            judge_probe_output("SB_ERR 100\n"),
            SeatbeltState::Unavailable(_)
        ));
        assert!(matches!(
            judge_probe_output("SB_CONNECT_OK"),
            SeatbeltState::Unavailable(_)
        ));
        assert!(matches!(
            judge_probe_output("SB_ERR 61\n"),
            SeatbeltState::Unavailable(_)
        ));
        assert!(matches!(
            judge_probe_output("完全没识别的输出"),
            SeatbeltState::Unavailable(_)
        ));
        assert!(matches!(
            judge_probe_output("SB_ERR 不是数字\n"),
            SeatbeltState::Unavailable(_)
        ));
        assert!(matches!(
            judge_probe_output(""),
            SeatbeltState::Unavailable(_)
        ));
    }

    /// 锚点：真实跑一遍探测——闸内 socket 必须被内核拒（EPERM）才算闸生效。
    /// 本测试只在 macOS 有意义（无 cfg 门：CI 若在别的平台跑会编译失败，
    /// 但本仓测试本来就在 macOS 主力机跑）；测试机需装 python。
    #[cfg(target_os = "macos")]
    #[test]
    fn seatbelt_probe_proves_gate_denies_socket() {
        let Some(py) = crate::py::env::cached_python() else {
            panic!("测试机未检测到 python，无法自证探测");
        };
        let state = seatbelt_probe_uncached(&py);
        assert_eq!(
            state,
            SeatbeltState::Available,
            "seatbelt 必须证明网络闸生效（sandbox 内 socket 被拒）"
        );
    }

    /// 锚点：被 deny 的 socket 在沙箱内报 EPERM 而不是 ECONNREFUSED——
    /// 直接断言 errno 语义，防有人把判定改成「任何错误都算生效」。
    #[cfg(target_os = "macos")]
    #[test]
    fn sandboxed_socket_error_is_eperm_not_refused() {
        let Some(py) = crate::py::env::cached_python() else {
            panic!("测试机未检测到 python");
        };
        let out = Command::new("sandbox-exec")
            .arg("-p")
            .arg(SEATBELT_PROFILE)
            .arg(&py)
            .arg("-c")
            .arg("import socket\ns=socket.socket()\ntry:\n    s.connect((\"127.0.0.1\",1))\n    print(\"OK\")\nexcept OSError as e:\n    import errno\n    assert e.errno == errno.EPERM, f\"errno={e.errno}\"\n    print(\"EPERM\")\n")
            .output()
            .expect("sandbox-exec 可执行");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("EPERM"),
            "沙箱内 socket 错误必须是 EPERM：{stdout}"
        );
    }
}
