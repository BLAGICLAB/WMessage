//! 受管长驻服务层：宿主托管的长驻进程（node MCP server / dev server / watcher）。
//!
//! 与 `py/runtime.rs` 是**两层**，不合并：
//! - 沙箱层（`run_python`）= 有界作业，跑完即弃，连坐杀进程组是对的；
//! - 服务层（本模块）= 长驻进程，宿主知道它、能停它、退出时收干净。
//!
//! 三条硬约束（改动本模块时不要破坏）：
//!
//! 1. **不继承 run_python 的 rlimit**。`RLIMIT_CPU = timeout + 10` 会让正在构建的
//!    dev server 收到 SIGXCPU（node 默认不装 handler，直接死）；`RLIMIT_AS` 卡的是
//!    虚拟地址空间，而 node 的 VSZ 是 400GB+ 量级，设了 node 启动即失败。
//!    所以本层的进程由宿主**直接 spawn**，不经过任何已设 rlimit 的子进程。
//! 2. **stdout/stderr 落文件，不占管道**。管道会被孙进程一直占着，触发宿主侧
//!    的 drain 超时连坐杀组（`py/runtime.rs` 的死法之二）。
//! 3. **停止走降级阶梯**（SIGINT → SIGTERM → SIGKILL），并递归回收**脱离进程组**
//!    的子孙——`kill` 进程组管不到 `setsid` 逃逸者。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use tauri::AppHandle;

use crate::audit::AuditLevel;
use crate::bot::mcp::config::STDIO_LAUNCH_ALLOWLIST;
use crate::bot::procs::{
    listening_sockets, merge_sockets, take_snapshot, tree_pids, tree_sample, ListeningSocket,
    ResourceSample,
};

mod watchdog;

// ── 常量 ──

/// 同时存活的受管服务数上限。超限直接拒绝启动。
pub(crate) const MAX_CONCURRENT_SERVICES: usize = 4;

/// 单流（stdout / stderr 各算）日志文件上限。
pub(crate) const OUTPUT_CAP_BYTES: u64 = 8 * 1024 * 1024;

/// 就绪瞬间固化的 stdout 尾部字节数。日志会被截断，启动横幅 / 监听地址不能跟着丢。
pub(crate) const READY_SNAPSHOT_BYTES: usize = 2 * 1024;

/// 就绪等待默认上限。
pub(crate) const DEFAULT_READY_TIMEOUT_SECS: u64 = 30;

/// 就绪等待硬上限。
pub(crate) const MAX_READY_TIMEOUT_SECS: u64 = 120;

/// `list_services` 返回的输出尾部行数。
pub(crate) const TAIL_LINES: usize = 20;

/// 输出上限巡检周期（秒）。
const OUTPUT_SWEEP_SECS: u64 = 30;

/// 停止之后等进程真正退出的窗口（毫秒）。
///
/// 过了这个窗口还没退 = 判定「停不掉」：状态放回原样并报失败，
/// 不写 `Exited`（写了等于把它从退出清理里摘掉，而它还在跑）。
const STOP_EXIT_WAIT_MS: u64 = 1000;

// ── 资源观测 ──

/// 单服务 RSS 告警阈值（字节）。正常 vite dev server 是 145MB，阈值在它上面 3.5 倍——
/// 抓住真正失控的进程，又不会对正常 dev server 误报。
pub(crate) const RSS_WARN_BYTES: u64 = 512 * 1024 * 1024;

/// 全部服务合计 RSS 告警阈值（字节）。4 × 145MB = 580MB 属正常；打到 1GB
/// 说明开的太多或某个跑飞了，该出声。
pub(crate) const RSS_TOTAL_WARN_BYTES: u64 = 1024 * 1024 * 1024;

/// CPU 占用告警阈值（百分比）：一个核的 95% 以上视为「跑满」。
///
/// 不是 100%：占用率是两次采样之差算出来的，采样时刻与进程记账有误差，
/// 真跑满时算出来是 99.9x%——阈值卡 100% 就永远差一点，永远不告警。
pub(crate) const CPU_WARN_PCT: f64 = 95.0;

/// CPU 高占用需持续的时长（毫秒）。瞬时跑满是正常的（首次构建、全量编译），
/// 持续 5 分钟才是问题。
pub(crate) const CPU_WARN_SUSTAIN_MS: i64 = 5 * 60 * 1000;

/// 告警项：单服务 RSS 超阈值。
pub(crate) const WARN_RSS: &str = "rss_over_limit";
/// 告警项：CPU 持续高占用。
pub(crate) const WARN_CPU: &str = "cpu_sustained_high";
/// 告警项：全部服务合计 RSS 超阈值。
pub(crate) const WARN_TOTAL_RSS: &str = "total_rss_over_limit";

/// 通知中心里的服务告警类型（payload 的 alertType）。
///
/// 一类 kind 两种类型：资源告警与端口违规对用户都是「看一眼」的告知，
/// 不需要分别造 kind——分类靠 payload 里的 alertType，撤收时也靠它。
const ALERT_RESOURCE: &str = "resource_warning";
const ALERT_VIOLATION: &str = "port_violation";

// ── P2-③：端口 / 监听地址闸 ──

/// 启动期加密采样点（毫秒，从 spawn 算起）。
///
/// 为什么加密：服务起来到 bind 落定之间可能隔好几秒（dev server 尤其慢），
/// 30s 一轮的巡检在这个窗口里会漏掉刚绑的端口。**最坏检测延迟上界**因此是
/// 启动期 10s、稳态 30s。
///
/// 这些采样点**不是容忍期**：一旦某次采样发现违规立即终止，不等窗口结束。
const STARTUP_SAMPLE_OFFSETS_MS: [u64; 6] = [0, 500, 1000, 2000, 5000, 10000];

/// 违规档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationRule {
    /// L1：监听地址不是 loopback（暴露到局域网/公网）
    NonLoopback,
    /// L2：占了宿主保留端口
    ReservedPort,
    /// L3：特权端口（<1024）
    PrivilegedPort,
}

impl std::fmt::Display for ViolationRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl ViolationRule {
    /// 面板与审计里的档位名。
    pub fn as_str(self) -> &'static str {
        match self {
            ViolationRule::NonLoopback => "L1",
            ViolationRule::ReservedPort => "L2",
            ViolationRule::PrivilegedPort => "L3",
        }
    }

    /// 违规的人话描述（给模型与面板看）。
    pub fn explain(self) -> &'static str {
        match self {
            ViolationRule::NonLoopback => "只允许监听 loopback（127.0.0.0/8 或 ::1）",
            ViolationRule::ReservedPort => "该端口是宿主保留端口，服务不能占",
            ViolationRule::PrivilegedPort => {
                "特权端口（<1024）需 root/CAP_NET_BIND_SERVICE，请改用 >=1024"
            }
        }
    }
}

/// 违规的处置动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationAction {
    /// 运行期采样发现 → 杀进程组
    Killed,
    /// spawn 前检查发现 → 拒绝启动
    Rejected,
}

impl ViolationAction {
    pub fn as_str(self) -> &'static str {
        match self {
            ViolationAction::Killed => "killed",
            ViolationAction::Rejected => "rejected",
        }
    }
}

/// 一次违规。
///
/// **正交于生命周期状态**：状态机讲的是进程还在不在（running/stopped/down），
/// 违规讲的是安全事件。一个服务可以 running + 有过违规，也可以 stopped +
/// 违规致死——混进状态机会污染所有状态机消费者。
#[derive(Debug, Clone)]
pub struct Violation {
    /// 触发的档位
    pub rule: ViolationRule,
    /// 违规的监听地址
    pub address: String,
    /// 违规的端口
    pub port: u16,
    /// 发生时刻（毫秒）
    pub at_ms: i64,
    /// 处置动作
    pub action: ViolationAction,
}

/// 判定结果（三态）。
///
/// **Unsupported 不等于 Ok**：「没判」与「判了没事」是两回事——安全闸在某个
/// 平台静默不生效是缺陷，所以语义上必须分开。
#[derive(Debug, Clone, PartialEq)]
pub enum GateVerdict {
    /// 没违规
    Ok,
    /// 违规
    Violated {
        rule: ViolationRule,
        address: String,
        port: u16,
    },
    /// **没判**：本平台采不到监听 socket（见 `procs::listening_supported`）
    Unsupported,
}

/// 命令行里粗解析出的监听意图。
///
/// **仅诊断**：模型可以写配置文件、设环境变量、在代码里硬编码端口，静态解析
/// 不可靠。真正兜底的是运行期采样。
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct FlagListen {
    /// 绑定地址（`--host` / `--listen` / `--bind`）
    pub host: Option<String>,
    /// 端口（`--port` / `-p` / `--listen host:port`）
    pub port: Option<u16>,
}

/// 端口 / 地址判据的保留集（静态 ∪ 宿主自身在监听的端口）。
///
/// 静态项是保底：宿主 API server 是可选启停的，4763 不在监听时静态项仍拒绑
/// ——保守正确（宿主「可能」用就该保留，不给模型抢绑窗口）。
/// 动态项每轮巡检刷新一次（宿主中途启停下一轮才反映；L2 是防抢占不是实时精确）。
static RESERVED_DYNAMIC: LazyLock<Mutex<HashSet<u16>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// 上一次监听采样是否成功（true = 采到了，可以判）。
///
/// 采集失败时端口闸**不判**——安全边界静默失效必须留痕：面板据此说明
/// 「本轮没判」，审计记一条 `service.listening_sample_failed`。
static LAST_SAMPLE_OK: AtomicBool = AtomicBool::new(true);

/// 记一次采样结果；只在「成功 → 失败」时写审计（每 30s 一轮，失败时逐轮记会刷屏）。
fn note_sample_status<R: tauri::Runtime>(app: &AppHandle<R>, complete: bool) {
    let prev = LAST_SAMPLE_OK.swap(complete, Ordering::Relaxed);
    if prev || complete {
        return;
    }
    crate::audit::write_event(
        app,
        AuditLevel::Warn,
        "service.listening_sample_failed",
        &[
            (
                "detail",
                "监听端口采集失败（lsof 起不来 / /proc/net/* 不可读）——端口闸本轮未判".to_string(),
            ),
            (
                "impact",
                "L1/L2 端口闸本轮不生效；动态保留集未刷新".to_string(),
            ),
        ],
    );
}

/// 静态保留端口（宿主自己的 API 端口）。
fn static_reserved_ports() -> HashSet<u16> {
    let mut s = HashSet::new();
    s.insert(crate::api::API_PORT);
    s
}

/// 当前保留集 = 静态 ∪ 动态。
pub(crate) fn reserved_ports() -> HashSet<u16> {
    let mut out = static_reserved_ports();
    out.extend(
        RESERVED_DYNAMIC
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .copied(),
    );
    out
}

/// 刷新保留集的动态部分（每轮巡检一次）。
///
/// **采样失败时不清空**：采空当成「宿主没在监听」会把动态保留集整个清掉，
/// L2（防抢占宿主端口）随之静默失效——两个闸一起废比一个闸废更糟。
fn refresh_reserved_ports(host_ports: &[u16], sample_complete: bool) {
    if !sample_complete {
        return;
    }
    let mut g = RESERVED_DYNAMIC.lock().unwrap_or_else(|e| e.into_inner());
    g.clear();
    g.extend(host_ports.iter().copied());
}

/// 地址是不是 loopback。
///
/// IPv4 的 `127.0.0.0/8` 与 IPv6 的 `::1` 都算；`0.0.0.0` / `::`（通配）不算。
pub(crate) fn is_loopback(address: &str) -> bool {
    if let Ok(ip) = address.parse::<std::net::Ipv4Addr>() {
        return ip.is_loopback();
    }
    if let Ok(ip) = address.parse::<std::net::Ipv6Addr>() {
        return ip.is_loopback();
    }
    false
}

/// 判一条监听 socket 触发了哪一档（没触发返回 None）。
///
/// 顺序 L1 → L2 → L3：一条 socket 可能同时踩多条（`0.0.0.0:80` 既是 L1 也是
/// L3），L1 是真安全边界，优先报它。
fn rule_of(
    sock: &ListeningSocket,
    reserved: &HashSet<u16>,
    allow_non_loopback: bool,
) -> Option<ViolationRule> {
    if !allow_non_loopback && !is_loopback(&sock.address) {
        return Some(ViolationRule::NonLoopback);
    }
    if reserved.contains(&sock.port) {
        return Some(ViolationRule::ReservedPort);
    }
    if sock.port < 1024 {
        return Some(ViolationRule::PrivilegedPort);
    }
    None
}

/// 运行期判定：看采到的监听 socket。
///
/// 本平台采不到 socket 时返回 **Unsupported**——不是 Ok。
/// **采样失败**（采集路径存在但本次采不到：lsof 起不来 / `/proc/net/*` 不可读）
/// 同样返回 Unsupported：那是「没判」，不是「判了没事」。两者混在 Ok 里，
/// L1 这条真安全边界会在采集器失灵时静默失效。
pub(crate) fn judge_sockets(
    socks: &[ListeningSocket],
    sample_complete: bool,
    reserved: &HashSet<u16>,
    allow_non_loopback: bool,
) -> GateVerdict {
    if !crate::bot::procs::listening_supported() || !sample_complete {
        return GateVerdict::Unsupported;
    }
    for sock in socks {
        if let Some(rule) = rule_of(sock, reserved, allow_non_loopback) {
            return GateVerdict::Violated {
                rule,
                address: sock.address.clone(),
                port: sock.port,
            };
        }
    }
    GateVerdict::Ok
}

/// spawn 前的 flag 早退判定（**弱于运行期判定**）。
///
/// 只覆盖命令行里写明的意图——模型能写配置文件、设环境变量、硬编码端口。
/// 所以 Windows 上（本平台采不到运行期监听）**不能拿它当边界**：调用方必须
/// 看到「本平台兜不住」才降级成诊断而不是拒绝。
pub(crate) fn judge_flags(
    flags: &FlagListen,
    reserved: &HashSet<u16>,
    allow_non_loopback: bool,
) -> Option<(ViolationRule, String, u16)> {
    if let Some(host) = &flags.host {
        if !allow_non_loopback && !is_loopback(host) {
            return Some((
                ViolationRule::NonLoopback,
                host.clone(),
                flags.port.unwrap_or(0),
            ));
        }
    }
    let port = flags.port?;
    if reserved.contains(&port) {
        return Some((
            ViolationRule::ReservedPort,
            flags.host.clone().unwrap_or_else(|| "0.0.0.0".to_string()),
            port,
        ));
    }
    if port < 1024 {
        return Some((
            ViolationRule::PrivilegedPort,
            flags.host.clone().unwrap_or_else(|| "0.0.0.0".to_string()),
            port,
        ));
    }
    None
}

/// 从命令行里粗解析端口与绑定地址。
///
/// 认这些写法：`--port 5173` / `--port=5173` / `-p 5173` /
/// `--host 0.0.0.0` / `--host=0.0.0.0` / `--listen 0.0.0.0:5173` / `--bind 127.0.0.1`。
pub(crate) fn parse_listen_flags(args: &[String]) -> FlagListen {
    let mut out = FlagListen::default();
    let mut i = 0usize;
    while i < args.len() {
        let arg = args[i].as_str();
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) if k.starts_with('-') => (k, Some(v.to_string())),
            _ => (arg, None),
        };
        let take_value = |i: &mut usize| -> Option<String> {
            if let Some(v) = inline.clone() {
                return Some(v);
            }
            *i += 1;
            args.get(*i).cloned()
        };
        match key {
            "--port" | "-p" => {
                if let Some(v) = take_value(&mut i) {
                    out.port = v.trim().parse::<u16>().ok();
                }
            }
            "--host" => {
                if let Some(v) = take_value(&mut i) {
                    out.host = Some(v.trim().to_string());
                }
            }
            "--listen" | "--bind" => {
                if let Some(v) = take_value(&mut i) {
                    let v = v.trim().to_string();
                    // `host:port` 形式：地址与端口一起给
                    match v.rsplit_once(':') {
                        Some((h, p)) if p.trim().parse::<u16>().is_ok() => {
                            out.host = Some(
                                h.trim()
                                    .trim_start_matches('[')
                                    .trim_end_matches(']')
                                    .to_string(),
                            );
                            out.port = p.trim().parse::<u16>().ok();
                        }
                        _ => out.host = Some(v),
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// 违规的人话（给模型与面板看，含改绑示例）。
pub(crate) fn violation_message(rule: ViolationRule, address: &str, port: u16) -> String {
    match rule {
        ViolationRule::NonLoopback => format!(
            "监听地址 {address} 不是 loopback（{rule}：{}）。\
             请改绑 127.0.0.1 或 ::1（如加 --host 127.0.0.1）；\
             确实要对外暴露，先在设置页打开「允许服务暴露到非 loopback」。",
            rule.explain()
        ),
        ViolationRule::ReservedPort => format!(
            "端口 {port} 是宿主保留端口（{rule}：{}）。请换一个端口。",
            rule.explain()
        ),
        ViolationRule::PrivilegedPort => format!(
            "端口 {port} 是特权端口（{rule}：{}）。请改用 >=1024 的端口（如加 --port 3000）。",
            rule.explain()
        ),
    }
}

/// spawn 前的端口/地址早退检查（工具层调用）。
///
/// 返回 Some((档位, 地址, 端口, 话术)) 表示命令行里写明的监听意图违规。
///
/// **平台不支持运行期判定时不能拿它当边界**：调用方要靠
/// `procs::listening_supported()` 区分「拒」与「只诊断」——Windows 上采不到
/// 运行期监听，拿 flag 解析冒充 L1 比没有边界更危险（review 会误以为有闸）。
pub(crate) fn precheck_listen(
    args: &[String],
    allow_non_loopback: bool,
) -> Option<(ViolationRule, String, u16, String)> {
    let flags = parse_listen_flags(args);
    let reserved = reserved_ports();
    let (rule, address, port) = judge_flags(&flags, &reserved, allow_non_loopback)?;
    let message = violation_message(rule, &address, port);
    Some((rule, address, port, message))
}

/// 采一个服务整棵树的监听 socket 并判定。
///
/// 口径与资源采样一致：**整棵进程树**（组长 + 全部后代）——dev server 的活
/// 多半在子进程里，只看组长会漏掉真正在监听的那个。
fn check_service_listening<R: tauri::Runtime>(
    app: &AppHandle<R>,
    entry: &ServiceEntry,
    reserved: &HashSet<u16>,
    allow_non_loopback: bool,
) -> GateVerdict {
    let (state, pid) = {
        let g = entry.lock().unwrap_or_else(|e| e.into_inner());
        (g.state, g.pid)
    };
    // 启动期（Starting）也要判：bind 落定多在 ready 之前，只认 Running 的话
    // 启动期加密采样整轮空转，最坏检测延迟就变成 30s 而不是 10s。
    if !matches!(state, ServiceState::Running | ServiceState::Starting) || pid == 0 {
        return GateVerdict::Ok;
    }
    let snap = take_snapshot();
    let tree = tree_pids(&snap, pid);
    let sample = listening_sockets(&tree);
    let listening = merge_sockets(
        tree.iter()
            .filter_map(|p| sample.by_pid.get(p).cloned())
            .collect(),
    );
    let verdict = judge_sockets(&listening, sample.complete, reserved, allow_non_loopback);
    if let GateVerdict::Violated {
        rule,
        address,
        port,
    } = &verdict
    {
        handle_violation(app, entry, *rule, address, *port, ViolationAction::Killed);
    }
    verdict
}

/// 处置一次违规：记 last_violation + 写审计 + 通知中心 + 杀进程组。
///
/// 三个通道都要：审计给事后追溯，面板给当下响应，通知中心给「用户不在设置页
/// 前面」那类时刻——只写审计日志、面板只显示「down 了」而不说为什么，
/// 用户没法响应。
fn handle_violation<R: tauri::Runtime>(
    app: &AppHandle<R>,
    entry: &ServiceEntry,
    rule: ViolationRule,
    address: &str,
    port: u16,
    action: ViolationAction,
) {
    // 还在跑才处置：加密采样与 30s 巡检可能同时看到同一条违规，
    // 重复处置会写两条审计、杀两次组。
    let still_up = entry
        .lock()
        .map(|s| s.state == ServiceState::Running || s.state == ServiceState::Starting)
        .unwrap_or(false);
    if !still_up {
        return;
    }
    let now = crate::db::now_ms();
    let violation = Violation {
        rule,
        address: address.to_string(),
        port,
        at_ms: now,
        action,
    };
    let (service_id, label) = {
        let mut g = entry.lock().unwrap_or_else(|e| e.into_inner());
        g.last_violation = Some(violation.clone());
        let label = if g.name.is_empty() {
            g.command.clone()
        } else {
            g.name.clone()
        };
        (g.id.clone(), label)
    };
    crate::audit::write_event(
        app,
        AuditLevel::Warn,
        "service.port_violation",
        &[
            ("service_id", service_id.clone()),
            ("rule", rule.as_str().to_string()),
            ("address", address.to_string()),
            ("port", port.to_string()),
            ("action", action.as_str().to_string()),
            ("detail", rule.explain().to_string()),
            ("label", label.clone()),
        ],
    );
    // 通知中心也落一条：服务被杀是用户必须知道的事，而审计日志与设置页里
    // 那行字都得用户主动去看。
    let notif_id = format!("service:{service_id}:{ALERT_VIOLATION}:{now}");
    let payload = serde_json::json!({
        "serviceId": service_id,
        "alertType": ALERT_VIOLATION,
        "rule": rule.as_str(),
        "address": address,
        "port": port,
        "label": label,
    });
    if let Err(e) = crate::notifications::insert_service_alert(
        app,
        &notif_id,
        &format!("{label} · 端口违规"),
        &violation_message(rule, address, port),
        &payload,
    ) {
        eprintln!("[bot::services] 端口违规落通知中心失败：{e}");
    }
    if action == ViolationAction::Killed {
        // 走既有信号阶梯杀进程组（不是裸 kill）
        // 停不掉也要看得见：stop_entry 自己写审计，这里再留一行诊断
        if let Err(e) = stop_entry(app, entry, "port_violation") {
            eprintln!("[bot::services] 端口违规杀组未成功：{e}");
        }
    }
}

/// 启动期加密采样：0 / 0.5 / 1 / 2 / 5 / 10 秒各采一次，违规立即终止。
///
/// 服务退出就提前收工（别对着一个死进程空采样）。
fn spawn_startup_listening_watch<R: tauri::Runtime>(
    app: &AppHandle<R>,
    entry: ServiceEntry,
    reserved: HashSet<u16>,
    allow_non_loopback: bool,
) {
    // 线程要 'static 的句柄：克隆一份搬进去
    let app = app.clone();
    std::thread::spawn(move || {
        let start = Instant::now();
        for offset_ms in STARTUP_SAMPLE_OFFSETS_MS {
            let target = Duration::from_millis(offset_ms);
            if start.elapsed() < target {
                std::thread::sleep(target - start.elapsed());
            }
            let alive = entry
                .lock()
                .map(|s| s.state == ServiceState::Running || s.state == ServiceState::Starting)
                .unwrap_or(false);
            if !alive {
                return;
            }
            let verdict = check_service_listening(&app, &entry, &reserved, allow_non_loopback);
            if let GateVerdict::Violated { .. } = verdict {
                // 违规已处置（杀组），停掉这轮加密采样
                return;
            }
        }
    });
}

/// 一轮监听采样与判定：所有在跑服务 + 宿主自身一次采完。
///
/// 宿主自身的监听端口用来刷新**保留集**（静态 ∪ 动态）——复用同一份采样，
/// 不额外调一次系统调用。
fn enforce_listening_gate<R: tauri::Runtime>(app: &AppHandle<R>) {
    if !crate::bot::procs::listening_supported() {
        // Windows：采不到 → 不判（Unsupported 语义，见 judge_sockets）。
        // 这里不静默「判了没事」——判定层返回的是 Unsupported，不是 Ok。
        return;
    }
    let table = crate::app_state::services(app);
    let entries: Vec<ServiceEntry> = {
        let guard = lock_table(&table);
        guard.values().cloned().collect()
    };
    let self_pid = std::process::id();
    let snap = take_snapshot();
    // 一次平台调用采完：所有在跑服务的树 + 宿主自身
    let mut all_pids: Vec<u32> = vec![self_pid];
    let mut trees: Vec<(ServiceEntry, Vec<u32>)> = Vec::new();
    for entry in entries {
        let (state, pid) = {
            let g = entry.lock().unwrap_or_else(|e| e.into_inner());
            (g.state, g.pid)
        };
        if !matches!(state, ServiceState::Running | ServiceState::Starting) || pid == 0 {
            continue;
        }
        let tree = tree_pids(&snap, pid);
        all_pids.extend(tree.iter().copied());
        trees.push((entry, tree));
    }
    let sample = listening_sockets(&all_pids);
    note_sample_status(app, sample.complete);
    if !sample.complete {
        // 采集失败 = 没判，不是「判了没事」。保留集动态部分也不动
        // （refresh_reserved_ports 在采样失败时原样保留旧集合）。
        return;
    }
    // 保留集动态部分 = 宿主自身在监听的端口
    let host_ports: Vec<u16> = sample
        .by_pid
        .get(&self_pid)
        .map(|list| list.iter().map(|s| s.port).collect())
        .unwrap_or_default();
    refresh_reserved_ports(&host_ports, sample.complete);
    let reserved = reserved_ports();
    let allow_non_loopback = expose_net_allowed(app);
    for (entry, tree) in trees {
        let listening = merge_sockets(
            tree.iter()
                .filter_map(|p| sample.by_pid.get(p).cloned())
                .collect(),
        );
        if let GateVerdict::Violated {
            rule,
            address,
            port,
        } = judge_sockets(&listening, sample.complete, &reserved, allow_non_loopback)
        {
            handle_violation(app, &entry, rule, &address, port, ViolationAction::Killed);
        }
    }
}

/// 受管服务的**合计**资源观测状态（跨服务口径）。
///
/// 单服务的采样在各自登记条目里；合计口径是集合属性，没有"某一条"能放，
/// 单独存一份（运行期状态一律进 AppState，见 `app_state.rs` 的总表）。
#[derive(Debug, Clone, Default)]
pub(crate) struct ServicesUsage {
    /// 全部在跑服务合计 RSS（字节）
    pub(crate) total_rss_bytes: u64,
    /// 合计口径的告警项（条件消失前只告警一次）
    pub(crate) warnings: Vec<String>,
    /// 最近一次采样时刻
    pub(crate) sampled_at_ms: Option<i64>,
}

/// 资源告警判定结果。
#[derive(Debug, Clone, Default)]
pub(crate) struct ResourceVerdict {
    /// 判定后的告警集合（条件消失的项会被移除，条件再现时能重新告警）
    pub(crate) warnings: Vec<String>,
    /// 本轮**新出现**的告警（只有这些要写审计）
    pub(crate) new_warnings: Vec<String>,
    /// 新的 CPU 高占用起始时刻
    pub(crate) cpu_busy_since_ms: Option<i64>,
    /// 本轮算出的 CPU 占用率（百分比）
    pub(crate) cpu_pct: Option<f64>,
}

/// 资源告警判定（纯函数）。
///
/// 只告警**不杀**：CPU 上限对"正在构建的 dev server"是误杀（见设计稿 §5.1），
/// 内存也没有跨平台的硬限额（macOS 没有等价机制，Linux 上设 RLIMIT_AS 会让
/// node 启动即失败）。失控了出声，用户自己停。
pub(crate) fn evaluate_resources(
    sample: ResourceSample,
    last_cpu_sample: Option<(f64, i64)>,
    cpu_busy_since_ms: Option<i64>,
    prev_warnings: &[String],
    now_ms: i64,
) -> ResourceVerdict {
    // 占用率走公共公式（与 MCP 侧同一条口径）
    let cpu_pct = last_cpu_sample.and_then(|(prev_cpu, prev_at)| {
        crate::bot::procs::cpu_usage_pct(prev_cpu, prev_at, sample.cpu_seconds, now_ms)
    });

    // "持续"判定：占用率高就把起点钉住（第一次高时记为 now），掉回阈值以下就清零
    let mut busy_since = cpu_busy_since_ms;
    if let Some(pct) = cpu_pct {
        if pct >= CPU_WARN_PCT {
            busy_since = Some(busy_since.unwrap_or(now_ms));
        } else {
            busy_since = None;
        }
    }
    let cpu_warn = cpu_pct.map(|p| p >= CPU_WARN_PCT).unwrap_or(false)
        && busy_since
            .map(|s| now_ms - s >= CPU_WARN_SUSTAIN_MS)
            .unwrap_or(false);

    let mut warnings: Vec<String> = Vec::new();
    if sample.rss_bytes >= RSS_WARN_BYTES {
        warnings.push(WARN_RSS.to_string());
    }
    if cpu_warn {
        warnings.push(WARN_CPU.to_string());
    }
    let new_warnings = warnings
        .iter()
        .filter(|w| !prev_warnings.contains(w))
        .cloned()
        .collect();

    ResourceVerdict {
        warnings,
        new_warnings,
        cpu_busy_since_ms: busy_since,
        cpu_pct,
    }
}

/// 无 readyPattern 时的短存活确认窗口：起得来且这一刻还活着就算 Running。
const LIVENESS_CONFIRM_MS: u64 = 200;

// ── 数据结构 ──

/// 受管服务状态。终态 `Exited` / `Failed` 不再变更。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    /// 已 spawn，等就绪判据通过
    Starting,
    /// 已确认存活
    Running,
    /// 停止流程进行中
    Stopping,
    /// 进程已退出（正常结束或被杀）
    Exited,
    /// 根本没起来（spawn 失败 / 闸拒绝）
    Failed,
}

impl ServiceState {
    pub fn as_str(self) -> &'static str {
        match self {
            ServiceState::Starting => "starting",
            ServiceState::Running => "running",
            ServiceState::Stopping => "stopping",
            ServiceState::Exited => "exited",
            ServiceState::Failed => "failed",
        }
    }

    /// 是否仍占用并发名额（已终止的不算）。
    pub(crate) fn is_active(self) -> bool {
        matches!(
            self,
            ServiceState::Starting | ServiceState::Running | ServiceState::Stopping
        )
    }
}

/// 登记表条目。`Child` 句柄不在这里——spawn 后立刻移交给 watcher 线程，
/// 停止只需要 pid（发信号），不需要 `Child`。
#[derive(Debug, Clone)]
pub struct ManagedService {
    pub(crate) id: String,
    /// 用途标签（用户可读），可为空串
    pub(crate) name: String,
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) pid: u32,
    /// unix 进程组 id（= pid，因 spawn 时 `process_group(0)`）
    pub(crate) pgid: i32,
    pub state: ServiceState,
    pub(crate) started_at_ms: i64,
    pub(crate) exited_at_ms: Option<i64>,
    pub(crate) exit_code: Option<i32>,
    pub(crate) stop_reason: Option<String>,
    pub(crate) stdout_path: PathBuf,
    pub(crate) stderr_path: PathBuf,
    /// 归属会话（仅记录，不触发自动回收）
    pub(crate) owner_session: Option<String>,
    /// 就绪瞬间的 stdout 尾部快照。**不参与日志截断**——日志只留最近输出，
    /// 而启动横幅（监听地址）在长跑服务上早就被冲掉了。
    pub(crate) ready_snapshot: Option<String>,
    /// 最近一次资源采样（整棵进程树合计）。
    pub(crate) sample: Option<ResourceSample>,
    /// 最近一次算出的 CPU 占用率（百分比，100 = 一个核跑满）。
    pub(crate) cpu_pct: Option<f64>,
    /// 上一次采样（累计 CPU 秒 + 时刻），两次采样之差 ÷ 时间差 = 占用率。
    pub(crate) last_cpu_sample: Option<(f64, i64)>,
    /// CPU 高占用起始时刻。占用率掉回阈值以下即清零（"持续"才算问题）。
    pub(crate) cpu_busy_since_ms: Option<i64>,
    /// 当前告警项。同一项在条件消失前只告警一次，不反复刷审计。
    pub(crate) warnings: Vec<String>,
    /// 最近一次端口/地址违规。**正交于生命周期状态**——服务死了但违规记录
    /// 还在，用户得知道它为什么死。
    pub(crate) last_violation: Option<Violation>,
}

impl ManagedService {
    fn new(
        id: String,
        name: String,
        command: String,
        args: Vec<String>,
        cwd: PathBuf,
        pid: u32,
        pgid: i32,
        stdout_path: PathBuf,
        stderr_path: PathBuf,
        owner_session: Option<String>,
    ) -> Self {
        Self {
            id,
            name,
            command,
            args,
            cwd,
            pid,
            pgid,
            state: ServiceState::Starting,
            started_at_ms: crate::db::now_ms(),
            exited_at_ms: None,
            exit_code: None,
            stop_reason: None,
            stdout_path,
            stderr_path,
            owner_session,
            ready_snapshot: None,
            sample: None,
            cpu_pct: None,
            last_cpu_sample: None,
            cpu_busy_since_ms: None,
            warnings: Vec::new(),
            last_violation: None,
        }
    }

    /// 已运行时长（毫秒）。已退出则算到退出时刻。
    pub(crate) fn uptime_ms(&self) -> i64 {
        let end = self.exited_at_ms.unwrap_or_else(crate::db::now_ms);
        (end - self.started_at_ms).max(0)
    }
}

/// 登记表值：一条服务 + 它的运行态。`Arc<Mutex<..>>` 让 watcher 线程与工具调用
/// 侧都能就地改状态，不用重建整张表。
pub(crate) type ServiceEntry = Arc<Mutex<ManagedService>>;

// ── 日志文件 ──

/// 服务日志根目录（与 `py_runs_root` 同款：系统 temp 下按功能分目录）。
pub(crate) fn services_root() -> PathBuf {
    std::env::temp_dir().join("wmessage-services")
}

/// 建服务的工作目录（stdout.log / stderr.log 落这里）。
fn make_service_dir(id: &str) -> Result<PathBuf, String> {
    let dir = services_root().join(id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建服务目录失败：{e}"))?;
    Ok(dir)
}

/// 以 **append 模式**打开日志文件。
///
/// 必须 append：子进程持有同一个 fd，`O_APPEND` 让它每次写都落到**当前文件末尾**。
/// 宿主截断后（§ truncate_output_file）子进程续写会接在新末尾，而不是按旧偏移
/// 写出"前面一段空洞"的稀疏文件。
fn open_append(path: &Path) -> Result<std::fs::File, String> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("打开日志文件失败（{}）：{e}", path.display()))
}

/// 读文件末尾 `n` 字节。
fn read_tail_bytes(path: &Path, n: usize) -> Vec<u8> {
    let Ok(meta) = std::fs::metadata(path) else {
        return Vec::new();
    };
    let len = meta.len();
    if len == 0 {
        return Vec::new();
    }
    let start = len.saturating_sub(n as u64);
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    use std::io::{Read, Seek, SeekFrom};
    if f.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    buf
}

/// 读文件末尾若干行（最多 `n` 行）。
pub(crate) fn tail_lines(path: &Path, n: usize) -> Vec<String> {
    let bytes = read_tail_bytes(path, 8 * 1024);
    if bytes.is_empty() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].iter().map(|s| s.to_string()).collect()
}

/// 截断日志文件到 `cap` 以内，只保留尾部。
///
/// 返回被丢弃的字节数。
///
/// 三个 fd 的开法必须一致（都是 append）：
/// - 子进程侧：append（唯一真相源，持续写）
/// - 宿主读侧：只读
/// - 宿主写侧：**append**（普通写会从偏移 0 覆盖，把刚写回的尾部和子进程并发写冲掉）
///
/// 已知代价：`set_len(0)` 与写回之间有几毫秒窗口，窗口内子进程的写可能丢失或
/// 顺序错乱。日志尾部本就是"最近发生了什么"的近似，丢几毫秒不影响诊断；
/// 关键信息（启动横幅 / 监听地址）由 `ready_snapshot` 单独固化。
fn truncate_output_file(path: &Path, cap: u64) -> Result<u64, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("读日志元数据失败：{e}"))?;
    let len = meta.len();
    if len <= cap {
        return Ok(0);
    }
    let keep = (cap / 2) as usize;
    let tail = read_tail_bytes(path, keep);
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| format!("打开日志文件写失败：{e}"))?;
    f.set_len(0).map_err(|e| format!("截断日志文件失败：{e}"))?;
    drop(f);
    let mut w = open_append(path)?;
    use std::io::Write;
    w.write_all(&tail)
        .map_err(|e| format!("写回日志尾部失败：{e}"))?;
    Ok(len - tail.len() as u64)
}

// ── 启动 ──

/// 启动参数（工具层解析后传进来）。
#[derive(Debug, Clone)]
pub(crate) struct StartRequest {
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) name: String,
    pub(crate) ready_pattern: Option<String>,
    pub(crate) ready_timeout_secs: u64,
    pub(crate) session_id: Option<String>,
}

/// 启动结果。pid / 状态在登记表里，不在这里重复存一份（两处状态会漂）。
#[derive(Debug, Clone)]
pub(crate) struct StartOutcome {
    pub(crate) id: String,
}

/// 白名单校验 + 逐臂字面量构造。
///
/// 逐臂构造意味着源码层面不存在"运行时变量 → 进程名"的路径；
/// 白名单是 MCP 侧同一份 `STDIO_LAUNCH_ALLOWLIST`（单一事实源）。
fn build_command(command: &str, args: &[String], cwd: &Path) -> Result<Command, String> {
    if !STDIO_LAUNCH_ALLOWLIST.contains(&command) {
        return Err(format!(
            "命令「{command}」不在启动器白名单内（允许：{}）",
            STDIO_LAUNCH_ALLOWLIST.join(" / ")
        ));
    }
    let mut cmd = match command {
        "npx" => Command::new("npx"),
        "bunx" => Command::new("bunx"),
        "uvx" => Command::new("uvx"),
        "pipx" => Command::new("pipx"),
        "node" => Command::new("node"),
        "deno" => Command::new("deno"),
        "python" => Command::new("python"),
        "python3" => Command::new("python3"),
        "docker" => Command::new("docker"),
        "podman" => Command::new("podman"),
        other => {
            return Err(format!(
                "命令「{other}」不在启动器白名单内（允许：{}）",
                STDIO_LAUNCH_ALLOWLIST.join(" / ")
            ))
        }
    };
    cmd.args(args).current_dir(cwd);
    Ok(cmd)
}

/// 起一个服务并登记。**阻塞**：含就绪等待，必须挪到 blocking 线程。
#[allow(clippy::too_many_lines)]
pub(crate) fn do_start<R: tauri::Runtime>(
    app: &AppHandle<R>,
    req: &StartRequest,
) -> Result<StartOutcome, String> {
    let id = format!("svc_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    // 白名单校验先做：命令不合法时连目录都不建，更不会占名额
    let mut cmd = build_command(&req.command, &req.args, &req.cwd)?;
    let dir = make_service_dir(&id)?;
    let stdout_path = dir.join("stdout.log");
    let stderr_path = dir.join("stderr.log");
    let stdout_file = open_append(&stdout_path)?;
    let stderr_file = open_append(&stderr_path)?;

    // 名额预留：检查与占位在**同一把锁**里做完——先查后插会让并发启动
    // 各自看到「还有空位」，结果起出超过上限的服务。占位条目状态是
    // Starting（算活跃），spawn 失败时删掉。
    let table = crate::app_state::services(app);
    let entry: ServiceEntry = Arc::new(Mutex::new(ManagedService::new(
        id.clone(),
        req.name.clone(),
        req.command.clone(),
        req.args.clone(),
        req.cwd.clone(),
        0,
        0,
        stdout_path.clone(),
        stderr_path.clone(),
        req.session_id.clone(),
    )));
    {
        let mut guard = lock_table(&table);
        let active = guard
            .values()
            .filter(|e| e.lock().map(|s| s.state.is_active()).unwrap_or(false))
            .count();
        if active >= MAX_CONCURRENT_SERVICES {
            // 没起成东西，目录也别留
            let _ = std::fs::remove_dir_all(&dir);
            return Err(format!(
                "受管服务数已达上限（{MAX_CONCURRENT_SERVICES} 个），请先 stop 掉不再用的服务"
            ));
        }
        guard.insert(id.clone(), entry.clone());
    }

    cmd.stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // 自成进程组：整组发信号用（组杀/组信号），且组外逃逸的子孙由 ppid 树补
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(crate::bot::procs::CREATE_NO_WINDOW);
    }

    let started = Instant::now();
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            // 撤掉预留名额（spawn 失败 = 什么都没起）
            lock_table(&table).remove(&id);
            let _ = std::fs::remove_dir_all(&dir);
            crate::audit::write_event(
                app,
                AuditLevel::Warn,
                "service.start_failed",
                &[
                    ("command", req.command.clone()),
                    ("reason", format!("spawn 失败：{e}")),
                ],
            );
            return Err(format!("启动「{}」失败：{e}", req.command));
        }
    };
    let pid = child.id();
    let pgid = pid as i32;
    {
        let mut g = entry.lock().unwrap_or_else(|e| e.into_inner());
        g.pid = pid;
        g.pgid = pgid;
    }

    // spawn 窗口里可能已经被 stop 过（那时 pid 还是 0，登记表记的是「没有进程」，
    // stop 如实标了已退出）。现在进程真的起来了，但那条 stop 已经「完成」——
    // 不收掉它，它就是宿主管不到的孤儿。必须立刻杀掉，并让这次启动失败。
    let stopped_during_spawn = {
        let g = entry.lock().unwrap_or_else(|e| e.into_inner());
        !matches!(g.state, ServiceState::Starting)
    };
    if stopped_during_spawn {
        // 走同一套停止阶梯（含子孙回收）——刚起来的进程也可能已经 fork 了
        let _ = crate::bot::procs::stop_tree(pid, pgid, |_| {
            child.try_wait().map(|s| s.is_some()).unwrap_or(false)
        });
        let _ = child.wait();
        lock_table(&table).remove(&id);
        let _ = std::fs::remove_dir_all(&dir);
        return Err("服务在启动过程中已被停止".to_string());
    }

    crate::audit::write_event(
        app,
        AuditLevel::Info,
        "service.started",
        &[
            ("service_id", id.clone()),
            ("command", req.command.clone()),
            (
                "args_preview",
                crate::bot::truncate_for_log(&req.args.join(" "), 400),
            ),
            ("cwd", req.cwd.to_string_lossy().to_string()),
            ("pid", pid.to_string()),
        ],
    );

    // 宿主死亡看门狗：宿主非正常死亡（崩溃 / 被强杀 / OOM）时，服务进程组必须
    // 跟着走——否则留下的是宿主管不到的孤儿（还占着端口，下次启动直接撞端口闸）。
    // 装不上不拦启动：看门狗是兜底，不是服务能不能跑的前提。
    watchdog::attach(&id, pgid, &child);

    // watcher 线程：wait() 回收子进程并把状态转 Exited。不同步 wait 会留僵尸。
    {
        let entry = entry.clone();
        let app = app.clone();
        let watch_id = id.clone();
        std::thread::spawn(move || {
            let status = child.wait();
            let mut guard = entry.lock().unwrap_or_else(|e| e.into_inner());
            guard.state = ServiceState::Exited;
            guard.exited_at_ms = Some(crate::db::now_ms());
            guard.exit_code = status.ok().and_then(|s| s.code());
            // 服务自己退了 → 看门狗助手没有存在的意义了，收掉（别留空转的 sh）
            watchdog::disarm(&watch_id);
            let exit_code = guard.exit_code;
            let uptime = guard.uptime_ms();
            // 是宿主停的（还是它自己退的）——退出码单独一个数分不出来：
            // Windows 上 `taskkill /F` 的退出码就是 1，和「服务自己挂了」撞脸。
            let stop_reason = guard.stop_reason.clone();
            drop(guard);
            crate::audit::write_event(
                &app,
                AuditLevel::Info,
                "service.exited",
                &exited_event_kv(&watch_id, pid, exit_code, uptime, stop_reason),
            );
        });
    }

    // 启动期加密采样监听（0/0.5/1/2/5/10s）：dev server 起来到 bind 落定可能
    // 隔好几秒，30s 一轮的巡检在这个窗口里会漏。发现违规立即终止，不等窗口。
    spawn_startup_listening_watch(
        app,
        entry.clone(),
        reserved_ports(),
        expose_net_allowed(app),
    );

    // 就绪判定
    let outcome = match &req.ready_pattern {
        Some(pattern) => wait_ready_pattern(app, &entry, pattern, req.ready_timeout_secs, &id),
        None => {
            std::thread::sleep(Duration::from_millis(LIVENESS_CONFIRM_MS));
            let alive = entry
                .lock()
                .map(|s| s.state == ServiceState::Starting)
                .unwrap_or(false);
            if alive {
                ReadyOutcome::Ready
            } else {
                ReadyOutcome::Exited
            }
        }
    };

    if outcome != ReadyOutcome::Ready {
        // 就绪超时 / 提前退出：主动收掉这个服务，别留半死不活的东西
        let reason = "ready_timeout";
        // 先判定、后停止：停了之后状态必然是 Exited、退出码必然是停止手段的
        // （Windows 上 taskkill /F = 1），那时候再读状态就归因错了——
        // 「我们超时杀的」会被说成「它启动后立即退出（退出码 1）」。
        let snapshot = resolve_ready_failure(&entry, || {
            // 停不掉也要看得见：审计里有 service.stop_failed，这里再留一行诊断
            if let Err(e) = stop_entry(app, &entry, reason) {
                eprintln!("[bot::services] 就绪超时的服务收不掉：{e}");
            }
        });
        let stderr_tail = tail_lines(&stderr_path, TAIL_LINES);
        let stdout_tail = tail_lines(&stdout_path, TAIL_LINES);
        let mut detail = String::new();
        if !stdout_tail.is_empty() {
            detail.push_str("stdout: ");
            detail.push_str(&stdout_tail.join(" | "));
        }
        if !stderr_tail.is_empty() {
            if !detail.is_empty() {
                detail.push('；');
            }
            detail.push_str("stderr: ");
            detail.push_str(&stderr_tail.join(" | "));
        }
        // 端口闸违规由巡检线程落：取最新的（违规致死优先于「没就绪」——
        // 模型要拿到规则、地址和改绑示例才可能下一轮改对）。
        let violation = entry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last_violation
            .clone();
        let failure = classify_ready_failure(
            &outcome,
            &snapshot,
            violation.as_ref(),
            req.ready_timeout_secs,
        );
        let msg = ready_failure_message(&req.command, &failure, &detail);
        crate::audit::write_event(
            app,
            AuditLevel::Warn,
            "service.start_failed",
            &[
                ("service_id", id.clone()),
                ("command", req.command.clone()),
                ("reason", msg.clone()),
            ],
        );
        return Err(msg);
    }

    // 就绪：固化 stdout 尾部快照（不参与后续截断）+ 记审计
    let ready_snapshot = tail_text(&stdout_path, READY_SNAPSHOT_BYTES);
    {
        let mut guard = entry.lock().unwrap_or_else(|e| e.into_inner());
        guard.ready_snapshot = Some(ready_snapshot.clone());
        guard.state = ServiceState::Running;
    }
    crate::audit::write_event(
        app,
        AuditLevel::Info,
        "service.ready",
        &[
            ("service_id", id.clone()),
            ("pid", pid.to_string()),
            ("elapsed_ms", started.elapsed().as_millis().to_string()),
            ("stdout_head", head_chars(&ready_snapshot, 1024)),
            ("stdout_tail", tail_chars(&ready_snapshot, 1024)),
        ],
    );

    Ok(StartOutcome { id })
}

/// 就绪判定的结果。
///
/// 「进程自己退了」与「等超时了」必须分开：两者的根因不同，模型拿到
/// 「启动后立即退出」会去查「这个命令为什么自己挂了」（改命令行、改依赖），
/// 而真相常常是「readyPattern 没匹配上 / 输出没 flush」——诊断方向完全相反。
#[derive(Debug, Clone, PartialEq, Eq)]
enum ReadyOutcome {
    /// stdout 出现匹配正则 → 就绪
    Ready,
    /// 进程自己退了（watcher 已记下它自己的退出码）
    Exited,
    /// 到点还没就绪，进程还活着
    TimedOut,
    /// readyPattern 不是合法正则（压根没开始等）
    BadPattern(String),
}

/// 就绪失败的**停止前快照**。
///
/// 必须在停止之前取：停了之后状态必然是 Exited、退出码必然是停止手段的
/// （Windows 上 `taskkill /F` 的退出码就是 1），那时候读到的一切都归因错了。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReadyFailureSnapshot {
    /// 停止之前它是不是已经自己退了
    exited_early: bool,
    /// 它自己退时的退出码（停止之前取的才是它自己的）
    exit_code: Option<i32>,
}

/// 就绪失败的归类（决定给模型看的文案与根因）。
#[derive(Debug, Clone)]
enum ReadyFailure {
    /// 端口闸违规致死
    Violation(Violation),
    /// readyPattern 不是合法正则
    BadPattern(String),
    /// 进程自己退了（退出码是它自己的）
    ExitedEarly { exit_code: Option<i32> },
    /// 等超时了（是我们停的）
    TimedOut { timeout_secs: u64 },
}

/// 取停止前的状态快照（纯读，不改状态）。
fn ready_failure_snapshot(entry: &ServiceEntry) -> ReadyFailureSnapshot {
    let g = entry.lock().unwrap_or_else(|e| e.into_inner());
    ReadyFailureSnapshot {
        exited_early: g.state == ServiceState::Exited,
        exit_code: g.exit_code,
    }
}

/// 就绪失败的两步：**先判定、后停止**（顺序是语义的一部分）。
///
/// 停了之后状态必然变 Exited、退出码必然变成停止手段的——那时候再读状态，
/// 「我们超时杀的」就会被说成「它启动后立即退出（退出码 1）」。
fn resolve_ready_failure<F: FnOnce()>(entry: &ServiceEntry, stop: F) -> ReadyFailureSnapshot {
    let snapshot = ready_failure_snapshot(entry);
    stop();
    snapshot
}

/// 就绪失败的归类（纯函数，供单测钉住）。
///
/// 优先级：端口闸违规 > readyPattern 非法 > 进程自己退 > 等超时。
fn classify_ready_failure(
    outcome: &ReadyOutcome,
    snapshot: &ReadyFailureSnapshot,
    violation: Option<&Violation>,
    timeout_secs: u64,
) -> ReadyFailure {
    if let Some(v) = violation {
        return ReadyFailure::Violation(v.clone());
    }
    match outcome {
        ReadyOutcome::BadPattern(e) => ReadyFailure::BadPattern(e.clone()),
        // 进程自己退了：退出码取停止前的快照（不是我们 kill 出来的那个）
        _ if snapshot.exited_early => ReadyFailure::ExitedEarly {
            exit_code: snapshot.exit_code,
        },
        _ => ReadyFailure::TimedOut { timeout_secs },
    }
}

/// 就绪失败的文案（纯函数，供单测钉住）。
///
/// 「等超时」与「进程自己退」是两种病历：前者说「你的 readyPattern 没匹配上
/// （输出多半没 flush）」，后者才报它自己的退出码。两者混起来，模型会被引到
/// 错的方向上重试好几轮。
fn ready_failure_message(command: &str, failure: &ReadyFailure, detail: &str) -> String {
    match failure {
        ReadyFailure::Violation(v) => {
            let mut m = format!(
                "服务「{command}」已被端口闸终止：{}",
                violation_message(v.rule, &v.address, v.port)
            );
            if !detail.is_empty() {
                m.push(' ');
                m.push_str(detail);
            }
            m
        }
        ReadyFailure::BadPattern(e) => {
            format!("服务「{command}」的 readyPattern 不是合法正则：{e}。已停止。{detail}")
        }
        ReadyFailure::ExitedEarly { exit_code } => match exit_code {
            Some(c) => format!("服务「{command}」启动后立即退出（退出码 {c}）。{detail}"),
            None => format!("服务「{command}」启动后立即退出。{detail}"),
        },
        ReadyFailure::TimedOut { timeout_secs } => {
            format!("服务「{command}」在 {timeout_secs}s 内未就绪，已停止。{detail}")
        }
    }
}

/// `service.exited` 的 kv（纯函数，供测试钉住）。
///
/// **必须带 stop_reason**：退出码单独一个数分不出根因——Windows 上
/// `taskkill /F` 的退出码是 1，和「服务自己挂了」撞脸；stop_reason=none
/// 才是「它自己退的」。
fn exited_event_kv(
    service_id: &str,
    pid: u32,
    exit_code: Option<i32>,
    uptime_ms: i64,
    stop_reason: Option<String>,
) -> Vec<(&'static str, String)> {
    vec![
        ("service_id", service_id.to_string()),
        ("pid", pid.to_string()),
        (
            "exit_code",
            exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "none".to_string()),
        ),
        ("uptime_ms", uptime_ms.to_string()),
        (
            "stop_reason",
            stop_reason.unwrap_or_else(|| "none".to_string()),
        ),
    ]
}

/// 就绪等待：stdout 出现匹配正则即算就绪。
fn wait_ready_pattern<R: tauri::Runtime>(
    app: &AppHandle<R>,
    entry: &ServiceEntry,
    pattern: &str,
    timeout_secs: u64,
    id: &str,
) -> ReadyOutcome {
    let re = match regex::Regex::new(pattern) {
        Ok(r) => r,
        Err(e) => {
            crate::audit::write_event(
                app,
                AuditLevel::Warn,
                "service.start_failed",
                &[
                    ("service_id", id.to_string()),
                    ("reason", format!("readyPattern 不是合法正则：{e}")),
                ],
            );
            return ReadyOutcome::BadPattern(e.to_string());
        }
    };
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let stdout_path = entry
            .lock()
            .map(|s| s.stdout_path.clone())
            .unwrap_or_default();
        let text = String::from_utf8_lossy(&read_tail_bytes(&stdout_path, 64 * 1024)).to_string();
        if re.is_match(&text) {
            return ReadyOutcome::Ready;
        }
        let state = entry
            .lock()
            .map(|s| s.state)
            .unwrap_or(ServiceState::Failed);
        if state == ServiceState::Exited || state == ServiceState::Failed {
            return ReadyOutcome::Exited;
        }
        if Instant::now() >= deadline {
            return ReadyOutcome::TimedOut;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 读文件末尾 `n` 字节并转成文本（尾部可能是半个 UTF-8 字符，替换掉即可）。
fn tail_text(path: &Path, n: usize) -> String {
    String::from_utf8_lossy(&read_tail_bytes(path, n)).to_string()
}

/// 取文本头部 n 个字符（按字符切，不切碎 UTF-8）。
fn head_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// 取文本尾部 n 个字符（按字符切，不切碎 UTF-8）。
fn tail_chars(s: &str, n: usize) -> String {
    let len = s.chars().count();
    s.chars().skip(len.saturating_sub(n)).collect()
}

// ── 停止 ──

/// 停止结果。
#[derive(Debug, Clone)]
pub(crate) struct StopOutcome {
    pub(crate) stopped: bool,
    /// 实际生效的手段（unix：`SIGINT` / `SIGTERM` / `SIGKILL`；
    /// Windows：`taskkill` / `taskkill_failed`；另有 `already_exited`）
    pub(crate) method: String,
    pub(crate) descendants_killed: usize,
    pub(crate) exit_code: Option<i32>,
    pub(crate) elapsed_ms: u64,
}

/// 停止一个服务：降级信号阶梯 + 脱离进程组的子孙回收。
///
/// 阻塞：含等待窗口，必须挪到 blocking 线程。
///
/// **只报「停掉了」的前提是进程真的退了**：等不到退出就把状态放回原样
/// （服务留在活动集合里，退出时还会再收一次），并返回一条明确错误——
/// 写 `Exited` 等于告诉退出清理「这个不用收了」，而它明明还在跑。
pub(crate) fn stop_entry<R: tauri::Runtime>(
    app: &AppHandle<R>,
    entry: &ServiceEntry,
    reason: &str,
) -> Result<StopOutcome, String> {
    let started = Instant::now();
    let (pid, pgid, stdout_path, stderr_path, id, prev_state) = {
        let mut g = entry.lock().unwrap_or_else(|e| e.into_inner());
        if g.state == ServiceState::Exited || g.state == ServiceState::Failed {
            return Ok(StopOutcome {
                stopped: false,
                method: "already_exited".to_string(),
                descendants_killed: 0,
                exit_code: g.exit_code,
                elapsed_ms: 0,
            });
        }
        let prev = g.state;
        g.state = ServiceState::Stopping;
        g.stop_reason = Some(reason.to_string());
        (
            g.pid,
            g.pgid,
            g.stdout_path.clone(),
            g.stderr_path.clone(),
            g.id.clone(),
            prev,
        )
    };

    crate::audit::write_event(
        app,
        AuditLevel::Info,
        "service.stop_requested",
        &[
            ("service_id", id.clone()),
            ("reason", reason.to_string()),
            ("pid", pid.to_string()),
        ],
    );

    // 停止阶梯 + 子孙回收：走公共进程树模块（受管服务层与 MCP stdio 共用一套）。
    // 等待口径传「登记表状态」——watcher 的 wait() 是权威（进程真退了状态才变）。
    let tree = match crate::bot::procs::stop_tree(pid, pgid, |grace| wait_state_exit(entry, grace))
    {
        Ok(tree) => tree,
        Err(e) => {
            // 本平台没有停止手段：状态放回原样，服务继续留在活动集合里
            restore_state(entry, prev_state);
            crate::audit::write_event(
                app,
                AuditLevel::Warn,
                "service.stop_unsupported",
                &[
                    ("service_id", id.clone()),
                    ("reason", e.clone()),
                    ("pid", pid.to_string()),
                ],
            );
            return Err(format!("{e}（无法停止受管服务）"));
        }
    };
    let method = tree.method;
    let killed = tree.descendants_killed;

    // 没有进程可停（pid 0 占位条目 / pid 1）：这个服务本来就没在跑，
    // 记成已退出是如实的。spawn 窗口里真的起来的那一个，由 do_start 侧兜底收掉。
    if method == "no_process" {
        {
            let mut g = entry.lock().unwrap_or_else(|e| e.into_inner());
            g.state = ServiceState::Exited;
            g.exited_at_ms = Some(crate::db::now_ms());
        }
        crate::audit::write_event(
            app,
            AuditLevel::Info,
            "service.stopped",
            &[
                ("service_id", id.clone()),
                ("method", method.clone()),
                ("elapsed_ms", started.elapsed().as_millis().to_string()),
                ("descendants_killed", "0".to_string()),
            ],
        );
        return Ok(StopOutcome {
            stopped: true,
            method,
            descendants_killed: 0,
            exit_code: None,
            elapsed_ms: started.elapsed().as_millis() as u64,
        });
    }

    for (p, old, new) in &tree.reused_skipped {
        crate::audit::write_event(
            app,
            AuditLevel::Warn,
            "service.pid_reuse_skipped",
            &[
                ("service_id", id.clone()),
                ("pid", p.to_string()),
                ("old_starttime", old.clone()),
                ("new_starttime", new.clone()),
            ],
        );
    }

    // 等 watcher 收尸，拿到退出码
    let exited = wait_state_exit(entry, Duration::from_millis(STOP_EXIT_WAIT_MS));
    let (exit_code, state) = {
        let g = entry.lock().unwrap_or_else(|e| e.into_inner());
        (g.exit_code, g.state)
    };
    if !exited {
        // 进程还活着：状态放回原样（不许写 Exited），并明确报失败
        restore_state(entry, prev_state);
        crate::audit::write_event(
            app,
            AuditLevel::Warn,
            "service.stop_failed",
            &[
                ("service_id", id.clone()),
                ("reason", "进程仍在运行".to_string()),
                ("method", method.clone()),
                ("pid", pid.to_string()),
            ],
        );
        return Err(format!("进程仍在运行（pid={pid}），停止未生效"));
    }
    if state != ServiceState::Exited {
        let mut g = entry.lock().unwrap_or_else(|e| e.into_inner());
        g.state = ServiceState::Exited;
        g.exited_at_ms = Some(crate::db::now_ms());
    }
    // 服务停了 → 看门狗助手收掉（别留一个空转的 sh 进程）
    watchdog::disarm(&id);

    crate::audit::write_event(
        app,
        AuditLevel::Info,
        "service.stopped",
        &[
            ("service_id", id.clone()),
            ("method", method.clone()),
            ("elapsed_ms", started.elapsed().as_millis().to_string()),
            ("descendants_killed", killed.to_string()),
        ],
    );

    // 日志截断（长跑服务的日志会无限涨）
    truncate_stream(app, &id, "stdout", &stdout_path);
    truncate_stream(app, &id, "stderr", &stderr_path);

    Ok(StopOutcome {
        stopped: true,
        method,
        descendants_killed: killed,
        exit_code,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

/// 停止失败时把状态放回停止前的那个。
///
/// 关键：失败的服务必须留在活动集合里（`is_active`）——写 `Exited` 等于
/// 告诉退出清理「这个不用收了」，而它明明还在跑。
///
/// 只在状态还是 `Stopping` 时回退：watcher 若在这期间把状态转成了终态
/// （进程确实退了），那就不该再把它说成「还在跑」。
fn restore_state(entry: &ServiceEntry, prev: ServiceState) {
    let mut g = entry.lock().unwrap_or_else(|e| e.into_inner());
    if g.state == ServiceState::Stopping {
        g.state = prev;
    }
}

/// 截断一个日志流到上限，超限才记审计。
fn truncate_stream<R: tauri::Runtime>(app: &AppHandle<R>, id: &str, stream: &str, path: &Path) {
    let Ok(dropped) = truncate_output_file(path, OUTPUT_CAP_BYTES) else {
        return;
    };
    if dropped == 0 {
        return;
    }
    crate::audit::write_event(
        app,
        AuditLevel::Info,
        "service.output_truncated",
        &[
            ("service_id", id.to_string()),
            ("stream", stream.to_string()),
            ("bytes", dropped.to_string()),
            ("cap", OUTPUT_CAP_BYTES.to_string()),
        ],
    );
}

/// 巡检所有服务的日志：超限的截到上限（只留最近输出）。
pub(crate) fn enforce_output_caps<R: tauri::Runtime>(app: &AppHandle<R>) {
    for s in list_all(app) {
        truncate_stream(app, &s.id, "stdout", &s.stdout_path);
        truncate_stream(app, &s.id, "stderr", &s.stderr_path);
    }
}

/// 资源观测一轮：采样在跑服务的资源占用，更新告警状态，必要时记审计。
///
/// 口径是**整棵进程树**（组长 + 全部后代）合计：dev server 的活多半在子进程里
/// （vite 拉起的 esbuild），只看组长会漏掉真正在烧的那个。
///
/// 只告警不杀——理由见 `evaluate_resources`。
pub(crate) fn sample_resources<R: tauri::Runtime>(app: &AppHandle<R>) {
    let snap = take_snapshot();
    let now = crate::db::now_ms();
    let table = crate::app_state::services(app);
    let entries: Vec<ServiceEntry> = {
        let guard = lock_table(&table);
        guard.values().cloned().collect()
    };
    let mut total_rss = 0u64;
    for entry in entries {
        let (state, pid) = {
            let g = entry.lock().unwrap_or_else(|e| e.into_inner());
            (g.state, g.pid)
        };
        if state != ServiceState::Running {
            continue;
        }
        // 组长不在快照里 = 进程已经没了（这一轮采不到它）
        let Some(sample) = tree_sample(&snap, pid) else {
            continue;
        };
        total_rss = total_rss.saturating_add(sample.rss_bytes);

        let mut g = entry.lock().unwrap_or_else(|e| e.into_inner());
        let prev_warnings = g.warnings.clone();
        let verdict = evaluate_resources(
            sample,
            g.last_cpu_sample,
            g.cpu_busy_since_ms,
            &prev_warnings,
            now,
        );
        let service_id = g.id.clone();
        let label = if g.name.is_empty() {
            g.command.clone()
        } else {
            g.name.clone()
        };
        g.sample = Some(sample);
        g.cpu_pct = verdict.cpu_pct;
        g.last_cpu_sample = Some((sample.cpu_seconds, now));
        g.cpu_busy_since_ms = verdict.cpu_busy_since_ms;
        g.warnings = verdict.warnings.clone();
        drop(g);

        for w in &verdict.new_warnings {
            write_resource_warning(
                app,
                &service_id,
                "service",
                &label,
                w,
                sample.rss_bytes,
                verdict.cpu_pct,
            );
        }
        // 条件消失的告警：通知中心里那条也收掉，否则同一条告警反复出现会
        // 在角标里堆成一摞（每次出现是新一集，旧的不撤就都留着）。
        for w in prev_warnings
            .iter()
            .filter(|w| !verdict.warnings.contains(*w))
        {
            if let Err(e) = crate::notifications::resolve_service_alert(
                app,
                &service_id,
                ALERT_RESOURCE,
                Some(w),
            ) {
                eprintln!("[bot::services] 资源告警撤收失败：{e}");
            }
        }
    }

    // 合计口径：跨服务的集合属性，没有归属条目
    let usage = crate::app_state::services_usage(app);
    let mut u = usage.lock().unwrap_or_else(|e| e.into_inner());
    u.total_rss_bytes = total_rss;
    u.sampled_at_ms = Some(now);
    let total_warn = total_rss >= RSS_TOTAL_WARN_BYTES;
    let already = u.warnings.iter().any(|w| w == WARN_TOTAL_RSS);
    if total_warn {
        if !already {
            u.warnings.push(WARN_TOTAL_RSS.to_string());
        }
    } else {
        u.warnings.retain(|w| w != WARN_TOTAL_RSS);
    }
    drop(u);
    if total_warn && !already {
        write_resource_warning(
            app,
            "*",
            "total",
            "全部受管服务",
            WARN_TOTAL_RSS,
            total_rss,
            None,
        );
    } else if !total_warn && already {
        // 合计口径的条件消失了：通知中心那条一起收掉
        if let Err(e) = crate::notifications::resolve_service_alert(
            app,
            "*",
            ALERT_RESOURCE,
            Some(WARN_TOTAL_RSS),
        ) {
            eprintln!("[bot::services] 合计资源告警撤收失败：{e}");
        }
    }
}

/// 记一条资源告警：审计 + 系统通知。
///
/// 系统通知是给「用户不在设置页前面」准备的——面板上的告警只有盯着才看得见，
/// 而资源跑飞属于「等用户发现时机器已经卡了」那类事。
fn write_resource_warning<R: tauri::Runtime>(
    app: &AppHandle<R>,
    service_id: &str,
    scope: &str,
    label: &str,
    kind: &str,
    rss_bytes: u64,
    cpu_pct: Option<f64>,
) {
    let rss_mb = rss_bytes as f64 / 1024.0 / 1024.0;
    let (threshold, body) = match kind {
        WARN_RSS => (
            "rss>512MB",
            format!("{label}内存 {rss_mb:.0} MB（阈值 512 MB）"),
        ),
        WARN_CPU => (
            "cpu>95%_for_5min",
            format!(
                "{label} CPU 持续跑满（>95% 持续 5 分钟，当前 {:.0}%）",
                cpu_pct.unwrap_or(0.0)
            ),
        ),
        _ => (
            "total_rss>1024MB",
            format!("{label}合计内存 {rss_mb:.0} MB（阈值 1 GB）"),
        ),
    };
    let mut kv = vec![
        ("service_id", service_id.to_string()),
        ("scope", scope.to_string()),
        ("rss_mb", format!("{:.1}", rss_mb)),
        ("threshold", threshold.to_string()),
    ];
    if let Some(p) = cpu_pct {
        kv.push(("cpu_pct", format!("{:.1}", p)));
    }
    crate::audit::write_event(app, AuditLevel::Warn, "service.resource_warning", &kv);
    notify_system(app, "受管服务资源告警", &body);
    // 应用内通知中心也落一条：系统通知依赖通知插件，插件没装就静默跳过——
    // 那台机器上这条告警就只剩设置页里的一行字，用户不看就没有。
    let notif_id = format!("service:{service_id}:{kind}:{}", crate::db::now_ms());
    let payload = serde_json::json!({
        "serviceId": service_id,
        "alertType": ALERT_RESOURCE,
        "warning": kind,
        "scope": scope,
        "label": label,
    });
    if let Err(e) = crate::notifications::insert_service_alert(
        app,
        &notif_id,
        &format!("{label} · 资源告警"),
        &body,
        &payload,
    ) {
        eprintln!("[bot::services] 资源告警落通知中心失败：{e}");
    }
}

/// 发一条系统通知。
///
/// 插件没装就跳过（测试里的 mock app 没有这个插件）——通知发不出去不是业务错误，
/// 但也不该因此把巡检循环带崩。
fn notify_system<R: tauri::Runtime>(app: &AppHandle<R>, title: &str, body: &str) {
    use tauri::Manager;
    let Some(state) = app.try_state::<tauri_plugin_notification::Notification<R>>() else {
        return;
    };
    if let Err(e) = state.inner().builder().title(title).body(body).show() {
        eprintln!("[bot::services] 系统通知发送失败：{e}");
    }
}

/// 巡检一轮：输出上限 + 资源观测。
fn sweep_round<R: tauri::Runtime>(app: &AppHandle<R>) {
    enforce_output_caps(app);
    sample_resources(app);
    // P2-③：监听采样与判定（发现违规 → 杀进程组 + 记审计 + 面板标红）
    enforce_listening_gate(app);
}

/// 启动输出上限巡检：每 30s 一轮（App 启动时调用）。
///
/// 周期取 30s：长跑服务的日志增长与 CPU 跑满都是「分钟级」的，
/// 巡检太密只是白跑 IO。
/// 单轮失败不杀循环：截断失败只丢日志，不影响服务本身。
pub fn start_output_sweeper<R: tauri::Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(OUTPUT_SWEEP_SECS));
        ticker.tick().await; // 消耗首个立即触发的 tick
        loop {
            ticker.tick().await;
            sweep_round(&app);
        }
    });
}

/// 等服务状态转为终态。返回是否已终止。
fn wait_state_exit(entry: &ServiceEntry, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let state = entry
            .lock()
            .map(|s| s.state)
            .unwrap_or(ServiceState::Failed);
        if state == ServiceState::Exited || state == ServiceState::Failed {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(crate::bot::procs::ALIVE_POLL_MS));
    }
}

/// 停止一个服务（按 id）。
pub(crate) fn stop_service<R: tauri::Runtime>(
    app: &AppHandle<R>,
    id: &str,
    reason: &str,
) -> Result<StopOutcome, String> {
    let table = crate::app_state::services(app);
    let entry = {
        let guard = lock_table(&table);
        guard.get(id).cloned()
    };
    let Some(entry) = entry else {
        return Err(format!("找不到服务「{id}」，用 list_services 看现存服务"));
    };
    stop_entry(app, &entry, reason)
}

/// 退出清理：停掉所有仍在跑的服务。并发执行，总时长受单个服务的阶梯约束。
pub(crate) fn stop_all<R: tauri::Runtime>(app: &AppHandle<R>) -> usize {
    let table = crate::app_state::services(app);
    let entries: Vec<ServiceEntry> = {
        let guard = lock_table(&table);
        guard
            .values()
            .filter(|e| e.lock().map(|s| s.state.is_active()).unwrap_or(false))
            .cloned()
            .collect()
    };
    if entries.is_empty() {
        return 0;
    }
    let mut stopped = 0usize;
    for entry in entries {
        // 失败由 stop_entry 自己写审计（service.stop_failed / service.stop_unsupported）
        if let Ok(o) = stop_entry(app, &entry, "app_exit") {
            if o.stopped {
                stopped += 1;
            }
        }
    }
    // 清掉日志目录（退出即收干净；诊断信息已在审计与 stderr 尾部里）
    let root = services_root();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for e in entries.flatten() {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    let _ = std::fs::remove_dir(&root);
    stopped
}

// ── 登记表访问 ──

/// 取登记表锁。毒化时取内层值继续用（同仓库既有口径：毒化要留痕但不阻断业务）。
fn lock_table(
    table: &Arc<Mutex<HashMap<String, ServiceEntry>>>,
) -> std::sync::MutexGuard<'_, HashMap<String, ServiceEntry>> {
    table.lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] bot::services 服务登记表: {e:?}");
        e.into_inner()
    })
}

/// 列出全部服务（按 id 排序，输出稳定）。
pub fn list_all<R: tauri::Runtime>(app: &AppHandle<R>) -> Vec<ManagedService> {
    let table = crate::app_state::services(app);
    let guard = lock_table(&table);
    let mut out: Vec<ManagedService> = guard
        .values()
        .map(|e| e.lock().unwrap_or_else(|x| x.into_inner()).clone())
        .collect();
    out.sort_by(|a, b| a.started_at_ms.cmp(&b.started_at_ms).then(a.id.cmp(&b.id)));
    out
}

// ── 闸 ──

/// 授权开关的 flag 文件路径（与 `py-enabled.flag` 同款：开关落在运行期 flags 目录）。
pub(crate) fn services_flag_path<R: tauri::Runtime>(app: &AppHandle<R>) -> PathBuf {
    crate::paths::flags_dir(app).join("services-enabled.flag")
}

/// 授权开关是否打开。
pub fn enabled<R: tauri::Runtime>(app: &AppHandle<R>) -> bool {
    services_flag_path(app).exists()
}

/// 写开关状态。
///
/// 只吞 NotFound：权限 / 占用类失败必须上抛——盘上残留 flag 时
/// `service_get_enabled` 仍返 true，与刚返回给调用方的「已禁用」自相矛盾。
pub fn set_enabled<R: tauri::Runtime>(app: &AppHandle<R>, on: bool) -> Result<bool, String> {
    let path = services_flag_path(app);
    if on {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败：{e}"))?;
        }
        std::fs::write(&path, b"1").map_err(|e| format!("写入开关失败：{e}"))?;
    } else {
        match std::fs::remove_file(&path) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("删除开关失败：{e}")),
        }
    }
    Ok(enabled(app))
}

/// 读开关状态（设置页）。
#[tauri::command]
pub fn service_get_enabled(app: AppHandle) -> bool {
    enabled(&app)
}

/// 写开关状态（设置页）。
#[tauri::command]
pub fn service_set_enabled(app: AppHandle, enabled: bool) -> Result<bool, String> {
    set_enabled(&app, enabled)
}

// ── L1 放开开关（允许服务暴露到非 loopback） ──

/// 「允许服务暴露到非 loopback」的 flag 文件路径（与 `services-enabled.flag` 同款）。
pub(crate) fn expose_net_flag_path<R: tauri::Runtime>(app: &AppHandle<R>) -> PathBuf {
    crate::paths::flags_dir(app).join("services-expose-net.flag")
}

/// 非 loopback 暴露是否被允许（L1 的放开方式，默认关）。
pub fn expose_net_allowed<R: tauri::Runtime>(app: &AppHandle<R>) -> bool {
    expose_net_flag_path(app).exists()
}

/// 写「允许服务暴露到非 loopback」开关。
pub fn set_expose_net_allowed<R: tauri::Runtime>(
    app: &AppHandle<R>,
    on: bool,
) -> Result<bool, String> {
    let path = expose_net_flag_path(app);
    if on {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败：{e}"))?;
        }
        std::fs::write(&path, b"1").map_err(|e| format!("写入开关失败：{e}"))?;
    } else {
        match std::fs::remove_file(&path) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("删除开关失败：{e}")),
        }
    }
    Ok(expose_net_allowed(app))
}

/// 读「允许服务暴露到非 loopback」开关（设置页）。
#[tauri::command]
pub fn service_get_expose_net_allowed(app: AppHandle) -> bool {
    expose_net_allowed(&app)
}

/// 写「允许服务暴露到非 loopback」开关（设置页）。
#[tauri::command]
pub fn service_set_expose_net_allowed(app: AppHandle, allowed: bool) -> Result<bool, String> {
    set_expose_net_allowed(&app, allowed)
}

// ── 序列化 ──

/// 服务条目 → JSON（给模型看）。
pub(crate) fn service_to_json(s: &ManagedService, with_tail: bool) -> serde_json::Value {
    let mut obj = serde_json::json!({
        "serviceId": s.id,
        "name": s.name,
        "command": s.command,
        "args": s.args,
        "cwd": s.cwd.to_string_lossy(),
        "pid": s.pid,
        "state": s.state.as_str(),
        "startedAt": s.started_at_ms,
        "uptimeSecs": s.uptime_ms() / 1000,
        "exitCode": s.exit_code,
        "sessionId": s.owner_session,
    });
    if let Some(r) = &s.ready_snapshot {
        obj["readySnapshot"] = serde_json::Value::String(r.clone());
    }
    // 资源观测：整棵进程树合计（只告警不杀，见 evaluate_resources）
    if let Some(sample) = &s.sample {
        obj["rssMb"] = serde_json::json!(round1(sample.rss_bytes as f64 / 1024.0 / 1024.0));
    }
    if let Some(p) = s.cpu_pct {
        obj["cpuPct"] = serde_json::json!(round1(p));
    }
    obj["warnings"] = serde_json::json!(s.warnings);
    // 违规（正交于生命周期状态）：服务死了但违规记录还在，用户得知道它为什么死
    if let Some(v) = &s.last_violation {
        obj["violation"] = serde_json::json!({
            "rule": v.rule.as_str(),
            "address": v.address,
            "port": v.port,
            "at": v.at_ms,
            "action": v.action.as_str(),
            "detail": v.rule.explain(),
        });
    }
    if with_tail {
        obj["stdoutTail"] = serde_json::json!(tail_lines(&s.stdout_path, TAIL_LINES));
        obj["stderrTail"] = serde_json::json!(tail_lines(&s.stderr_path, TAIL_LINES));
    }
    obj
}

/// 保留一位小数（给模型看的数字不用小数点后一堆位）。
fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// 受管服务全景 → JSON（模型与设置页共用一份口径）。
///
/// 顶层带合计口径（合计 RSS 与合计告警）——单服务数据在 `services` 数组里。
pub(crate) fn services_snapshot_json<R: tauri::Runtime>(
    app: &AppHandle<R>,
    with_tail: bool,
) -> serde_json::Value {
    let list = list_all(app);
    let arr: Vec<serde_json::Value> = list.iter().map(|s| service_to_json(s, with_tail)).collect();
    let usage = crate::app_state::services_usage(app);
    let u = usage.lock().unwrap_or_else(|e| e.into_inner());
    serde_json::json!({
        "services": arr,
        "totalRssMb": round1(u.total_rss_bytes as f64 / 1024.0 / 1024.0),
        "warnings": u.warnings,
        "sampledAt": u.sampled_at_ms,
        // 本平台能不能采到监听 socket。Windows 不能——面板据此说明
        // 「本平台暂不支持监听端口采集」，而不是让用户对着空白猜。
        "listeningSupported": crate::bot::procs::listening_supported(),
        // 最近一次采样是否成功。false = 采集路径存在但本次采不到——端口闸
        // 本轮**没判**（不是「判了没事」），面板必须说出来。
        "listeningSampleOk": LAST_SAMPLE_OK.load(Ordering::Relaxed),
    })
}

/// 读受管服务全景（设置页）。
#[tauri::command]
pub fn services_list(app: AppHandle) -> serde_json::Value {
    services_snapshot_json(&app, true)
}

/// 停一个受管服务（设置页）。
#[tauri::command]
pub async fn services_stop(
    app: AppHandle,
    service_id: String,
) -> Result<serde_json::Value, String> {
    let handle = app.clone();
    let id = service_id.clone();
    // 停止含等待窗口（信号阶梯），必须挪出命令线程
    let outcome =
        tauri::async_runtime::spawn_blocking(move || stop_service(&handle, &id, "settings"))
            .await
            .map_err(|e| format!("停止任务失败：{e}"))?;
    match outcome {
        Ok(o) => Ok(serde_json::json!({
            "serviceId": service_id,
            "stopped": o.stopped,
            "method": o.method,
            "descendantsKilled": o.descendants_killed,
            "exitCode": o.exit_code,
            "elapsedMs": o.elapsed_ms,
        })),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests;
