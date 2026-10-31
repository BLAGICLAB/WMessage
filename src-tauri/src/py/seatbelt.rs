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
    let out = match Command::new("sandbox-exec")
        .arg("-p")
        .arg(SEATBELT_PROFILE)
        .arg(py)
        .arg("-c")
        .arg(SEATBELT_PROBE_SCRIPT)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            return SeatbeltState::Unavailable(format!("sandbox-exec 无法启动：{e}"));
        }
    };
    if !out.status.success() {
        return SeatbeltState::Unavailable(format!(
            "sandbox-exec 探测失败（退出码 {:?}；profile 语法被拒或二进制不可用）",
            out.status.code()
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.contains("SB_ERR 1") {
        // EPERM = 闸生效的唯一证明形态
        SeatbeltState::Available
    } else if stdout.contains("SB_CONNECT_OK") {
        SeatbeltState::Unavailable("沙箱内 socket 连接竟然成功——网络闸未生效".to_string())
    } else if stdout.contains("SB_ERR 61") {
        SeatbeltState::Unavailable(
            "沙箱内 socket 到了内核（ECONNREFUSED 而非 EPERM）——deny network* 未拦截".to_string(),
        )
    } else {
        SeatbeltState::Unavailable(format!(
            "沙箱探测输出未识别（闸状态无法证明）：{}",
            stdout.trim()
        ))
    }
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
