//! 端口/监听地址闸（P2-③）：判据、采集金丝雀、采样与判定。

use super::*;

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
pub(super) static LAST_SAMPLE_OK: AtomicBool = AtomicBool::new(true);

/// 记一次采样结果；只在「成功 → 失败」时写审计（每 30s 一轮，失败时逐轮记会刷屏）。
pub(super) fn note_sample_status<R: tauri::Runtime>(app: &AppHandle<R>, complete: bool) {
    let prev = LAST_SAMPLE_OK.swap(complete, Ordering::Relaxed);
    // 写审计的条件是「上一轮成功、本轮失败」——`prev` 是**换之前**的值。
    // （写成 `prev || complete` 时记的是「上一轮失败、本轮还失败」：转折那一条
    // 反而没记，之后每 30s 刷一条——与本函数自述相反。）
    if !prev || complete {
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

// ── 采集金丝雀 ──────────────────────────────────────────────────────────
// 已知残留（采样成败分离落地时论证过）：lsof 跑得通但**看不见本进程外**的
// 监听时，它既不报错也不出行——「采空」与「采不到」在数据流里是同一个
// 空集合，complete=true 骗过了判定层。金丝雀用宿主自己的已知事实把两者
// 分开：API server 开着时宿主必在监听 API_PORT，采样自称成功却看不见
// 这个端口 = 采集失明，本轮不可信。API 没开时没有基准，不制造误报源。

/// 宿主已知监听端口（金丝雀锚点）：本地 HTTP API 活着 → 它必在监听 API_PORT。
pub(super) fn api_canary_port<R: tauri::Runtime>(app: &AppHandle<R>) -> Option<u16> {
    use tauri::Manager;
    let state = app.try_state::<crate::api_server::ApiState>()?;
    let g = state.0.lock().unwrap_or_else(|e| e.into_inner());
    let running = g.as_ref()?;
    let alive = running
        .handle
        .as_ref()
        .map(|h| !h.is_finished())
        .unwrap_or(false);
    alive.then_some(crate::api::API_PORT)
}

/// 金丝雀判定（纯函数）：采样自称成功却看不见宿主必在监听的端口 = 失明。
/// 采集本身失败（!complete）时不重复报——那条路径已有自己的语义与审计。
pub(super) fn canary_blind(
    sample: &crate::bot::procs::ListeningSample,
    canary: Option<u16>,
) -> bool {
    let Some(port) = canary else {
        return false;
    };
    if !sample.complete {
        return false;
    }
    !sample
        .by_pid
        .values()
        .any(|socks| socks.iter().any(|s| s.port == port))
}

/// 金丝雀上一轮是否看见（true = 采到了宿主自己的端口）。
/// 只在「看见 → 失明」转折记一条审计（30s 一轮，逐轮记会刷屏；恢复不记，
/// 持续失明由面板的「本轮未判」呈现——金丝雀失明会连带 note_sample_status
/// 报失败，面板文案沿用既有口径）。
static CANARY_SEEN: AtomicBool = AtomicBool::new(true);

fn note_canary_blind<R: tauri::Runtime>(app: &AppHandle<R>, port: u16) {
    if !CANARY_SEEN.swap(false, Ordering::Relaxed) {
        return;
    }
    crate::audit::write_event(
        app,
        AuditLevel::Warn,
        "service.listening_canary_failed",
        &[
            ("canary_port", port.to_string()),
            (
                "detail",
                "采样自称成功但看不见宿主自身监听的 API 端口——采集失明（如 lsof 权限不足），端口闸本轮未判"
                    .to_string(),
            ),
        ],
    );
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
pub(super) fn refresh_reserved_ports(host_ports: &[u16], sample_complete: bool) {
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
pub(super) fn spawn_startup_listening_watch<R: tauri::Runtime>(
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
pub(super) fn enforce_listening_gate<R: tauri::Runtime>(app: &AppHandle<R>) {
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
    // 金丝雀先行：API 开着时宿主必在监听 API_PORT（all_pids 含 self_pid），
    // 采样自称成功却看不见它 = 采集失明——与采集失败同路处理（不判、
    // 保留集不动、面板报「本轮未判」），并单独记转折审计。
    let canary_port = api_canary_port(app);
    let blind = canary_blind(&sample, canary_port);
    if blind {
        note_canary_blind(app, canary_port.expect("blind 为真时 canary 必为 Some"));
    } else {
        CANARY_SEEN.store(true, Ordering::Relaxed);
    }
    let complete = sample.complete && !blind;
    note_sample_status(app, complete);
    if !complete {
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
    refresh_reserved_ports(&host_ports, complete);
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
        } = judge_sockets(&listening, complete, &reserved, allow_non_loopback)
        {
            handle_violation(app, &entry, rule, &address, port, ViolationAction::Killed);
        }
    }
}
