//! py 执行的 setrlimit 可用性探测族（自 runtime.rs 按节拆出）：S20 逐资源探测
//! + 进程内缓存。整族 #[cfg(unix)]——Windows 无 setrlimit（Job Object 限额另路）。
//! 「探测失败 ≠ 可用」：ProbeError 项一律按未生效处理并 audit 留痕，不许静默放行。

#[cfg(unix)]
use super::procs::silent_cmd;
#[cfg(unix)]
use super::runtime::{cpu_limit_secs, mem_limit_bytes, nproc_limit};
#[cfg(unix)]
use std::process::Stdio;
#[cfg(unix)]
use std::sync::OnceLock;
#[cfg(unix)]
use std::time::{Duration, Instant};

// ── S20：setrlimit 可用性探测（子进程一次探测 + 进程内缓存，逐资源结论）──
//
// 背景：pre_exec 里的 setrlimit 在部分环境被拒（实测：裸 macOS 内核即拒
// setrlimit(RLIMIT_AS)，测试 sandbox 连 CPU 一起拒），旧实现静默吞掉
// →「限额看似生效实则没生效」；硬失败方案此前实测打断 py_exec
// （PHASE3-MEDIUM-TRIAGE P3S-20 FIX→回退登记）。折中：首次 py 执行前用同一解释器
// 子进程逐资源探测一次并缓存；未生效项不设限额但必须 audit 留痕，可用项照设。
// 硬约束：**探测失败 ≠ 可用**——ProbeError 项一律按「未探测到」处理，不许静默放行。

/// 单个资源的 setrlimit 探测结论。ProbeError 表示探测自身失败（spawn/超时/输出
/// 缺项/不可解析），**不许当可用**。
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetrlimitProbe {
    Available,
    Unavailable(String),
    ProbeError(String),
}

/// RLIMIT_AS / RLIMIT_CPU 逐资源探测结论。
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetrlimitSupport {
    pub rlimit_as: SetrlimitProbe,
    pub rlimit_cpu: SetrlimitProbe,
    pub rlimit_nproc: SetrlimitProbe,
}

#[cfg(unix)]
impl SetrlimitSupport {
    #[cfg(unix)]
    fn all_available(&self) -> bool {
        self.rlimit_as == SetrlimitProbe::Available
            && self.rlimit_cpu == SetrlimitProbe::Available
            && self.rlimit_nproc == SetrlimitProbe::Available
    }
}

#[cfg(unix)]
static SETRLIMIT_PROBE: OnceLock<SetrlimitSupport> = OnceLock::new();

#[cfg(unix)]
const SETRLIMIT_PROBE_SCRIPT: &str = concat!(
    "import resource as _r, sys\n",
    "def _t(res, v):\n",
    "    try:\n",
    "        _r.setrlimit(res, (v, v))\n",
    "        return 'OK'\n",
    "    except (OSError, ValueError) as e:\n",
    "        # CPython 把 setrlimit 的 EINVAL 映射成 ValueError（文案固定），一并捕获\n",
    "        return '%s: %s' % (type(e).__name__, e)\n",
    "print('WM_RLIMIT_AS=' + _t(_r.RLIMIT_AS, int(sys.argv[1])))\n",
    "print('WM_RLIMIT_CPU=' + _t(_r.RLIMIT_CPU, int(sys.argv[2])))\n",
    // NPROC 上限按生产值（argv[3]）：脚本进程自身 + 看门狗 + 用户脚本，
    // 太紧会挡掉 multiprocessing 的正常并发，取值见 nproc_limit()
    "print('WM_RLIMIT_NPROC=' + _t(_r.RLIMIT_NPROC, int(sys.argv[3])))\n",
);
// 探测值取生产上限，且**由 argv 从 Rust 常量传入**（不在脚本文本里写死数字）：
// AS 2GB = mem_limit_bytes 封顶 / CPU = timeout 上限 + 10 宽限。脚本与生产共用
// 同一组常量，天花板一改两边同时跟着变，不会出现「探针按旧值判可用、生产按新值
// 设置」的错位（那会让限额静默失效）。
// 上限通过 → 单调 ceiling 下全部生产值可用；上限被拒 → 从严判 Unavailable 留痕。
// 方向性硬约束：宁可误判「不可用」留痕（丢限额但诚实），不可误判「可用」静默（S20 原罪）。
// 已知残差：仅拒中间值、放行两端的非单调沙箱不在覆盖；pre_exec 内仍
// best-effort 静默，不做二次探测。

/// 从探测 stdout 逐资源提结论；缺项/空值按探测失败算（绝不猜成可用）。
#[cfg(unix)]
pub fn parse_setrlimit_probe_output(out: &str) -> SetrlimitSupport {
    fn pick(tag: &str, out: &str) -> SetrlimitProbe {
        let line = out
            .lines()
            .map(str::trim)
            .find(|l| l.starts_with(tag))
            .map(|l| &l[tag.len()..]);
        match line {
            Some("OK") => SetrlimitProbe::Available,
            Some("") | None => SetrlimitProbe::ProbeError(format!("探测输出缺 {tag} 项")),
            Some(err) => SetrlimitProbe::Unavailable(err.to_string()),
        }
    }
    SetrlimitSupport {
        rlimit_as: pick("WM_RLIMIT_AS=", out),
        rlimit_cpu: pick("WM_RLIMIT_CPU=", out),
        rlimit_nproc: pick("WM_RLIMIT_NPROC=", out),
    }
}

/// 未生效/探测失败项的每运行 audit 行（全部可用时返回 None，不另扰）。
/// 硬约束文案：「限额未生效」/「未探测到 setrlimit 可用性」。
#[cfg(unix)]
pub fn rlimit_warn_line(s: &SetrlimitSupport) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for (name, p) in [
        ("RLIMIT_AS", &s.rlimit_as),
        ("RLIMIT_CPU", &s.rlimit_cpu),
        ("RLIMIT_NPROC", &s.rlimit_nproc),
    ] {
        match p {
            SetrlimitProbe::Available => {}
            SetrlimitProbe::Unavailable(w) => {
                parts.push(format!("{name}: 限额未生效({w})"));
            }
            SetrlimitProbe::ProbeError(w) => {
                parts.push(format!("{name}: 未探测到 setrlimit 可用性，限额未设({w})"));
            }
        }
    }
    (!parts.is_empty()).then(|| format!("run_python warn | rlimit_off | {}", parts.join("; ")))
}

#[cfg(unix)]
pub fn run_setrlimit_probe(py: &str) -> SetrlimitSupport {
    // 与 run_python_at 同一构造器（参数列表传参，无 shell 参与，无注入面）。
    use crate::bot::constants::APPS_MAX_TIMEOUT_SECS;
    let mut cmd = silent_cmd(py);
    cmd.arg("-c")
        .arg(SETRLIMIT_PROBE_SCRIPT)
        // 探测值来自生产常量（见 SETRLIMIT_PROBE_SCRIPT 注），脚本自己只读 argv。
        // 取两条路径（工具 600s / apps 1800s）中更宽的限额探测：能设宽才代表真可用。
        .arg(mem_limit_bytes(APPS_MAX_TIMEOUT_SECS).to_string())
        .arg(cpu_limit_secs(APPS_MAX_TIMEOUT_SECS).to_string())
        .arg(nproc_limit().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // 探测面与执行面同口径：环境走白名单裁剪——宿主环境若被注入
    // PYTHONPATH/sitecustomize，探测 python 会先执行注入代码，可把
    // 「探测失败」伪造成「可用」
    crate::py::runtime::apply_child_env(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return SetrlimitSupport {
                rlimit_as: SetrlimitProbe::ProbeError(format!("探测子进程 spawn 失败: {e}")),
                rlimit_cpu: SetrlimitProbe::ProbeError("同上（探测子进程未运行）".to_string()),
                rlimit_nproc: SetrlimitProbe::ProbeError("同上（探测子进程未运行）".to_string()),
            };
        }
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => {
                return SetrlimitSupport {
                    rlimit_as: SetrlimitProbe::ProbeError(format!("探测子进程 wait 失败: {e}")),
                    rlimit_cpu: SetrlimitProbe::ProbeError("同上（探测子进程未运行）".to_string()),
                    rlimit_nproc: SetrlimitProbe::ProbeError(
                        "同上（探测子进程未运行）".to_string(),
                    ),
                };
            }
        }
    };
    let Some(status) = status else {
        let msg = "探测子进程超时（10s）已强杀".to_string();
        return SetrlimitSupport {
            rlimit_as: SetrlimitProbe::ProbeError(msg.clone()),
            rlimit_cpu: SetrlimitProbe::ProbeError(msg),
            rlimit_nproc: SetrlimitProbe::ProbeError("同上（探测子进程未运行）".to_string()),
        };
    };
    if !status.success() {
        let msg = format!("探测子进程退出码异常: {status}");
        return SetrlimitSupport {
            rlimit_as: SetrlimitProbe::ProbeError(msg.clone()),
            rlimit_cpu: SetrlimitProbe::ProbeError(msg),
            rlimit_nproc: SetrlimitProbe::ProbeError("同上（探测子进程未运行）".to_string()),
        };
    }
    let mut out = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut s, &mut out);
    }
    parse_setrlimit_probe_output(&out)
}

/// 进程内一次缓存：rlimit 可用性是 OS/沙箱属性，与具体解释器实例无关，
/// py 路径中途失效不重探。全可用时 audit 一条确认；未生效/探测失败留给每运行 warn。
#[cfg(unix)]
pub(super) fn setrlimit_probe_cached(py: &str, audit: &mut dyn FnMut(&str)) -> SetrlimitSupport {
    SETRLIMIT_PROBE
        .get_or_init(|| {
            let r = run_setrlimit_probe(py);
            if r.all_available() {
                audit("run_python | rlimit_probe | setrlimit 全可用，限额照设");
            }
            r
        })
        .clone()
}
