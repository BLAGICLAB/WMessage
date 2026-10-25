//! MCP 连接生命周期管理（阶段 2）：按需启停 / 指纹对齐重载 / 懒重连。
//!
//! 托管形态：进程级 `OnceLock` 单例（`shared()`），lib.rs setup 触发
//! 启动重载；命令、registry 挂载层、退出清理全部经 `shared()` 访问。
//!
//! **生命周期：按需启停（静默待命 → 调用期拉起 → 调用完回收）**
//!
//! 打开设置页的启用开关**不启动任何子进程**——服务器进入「静默待命」：
//! 工具清单已知（来自磁盘快照 `tool_cache`），但没有进程在跑。模型真正调用
//! 该服务器的工具时，才拉起子进程；调用结束立刻回收，进程不跨调用存活。
//!
//! 为什么这么切：业界客户端（Claude Code / Cursor 等）默认「启动即连、
//! 常驻到退出」，一台 stdio 服务器（node/npx）常驻 80–150 MB，十来台就是
//! 几 GB 纯待机内存（调研见 `docs/MCP-ON-DEMAND-LIFECYCLE-DESIGN-2026-10-24.md`）。
//! mcp-proxy / VS Code / rho 都已转向 lazy：工具元数据与活连接解耦，
//! 发现一次工具清单即落盘，用完即断，需要时透明重连。
//!
//! 隔离模型：
//! - stdio 服务器 = 独立子进程，崩溃不伤宿主（传输断开 → 槽位转 Down）；
//! - 懒重连：Down 不是终点——下次 `ensure_connected`（工具调用前）会重新
//!   拉起进程；连接失败把错误写进槽位 + 审计，绝不 panic / 不阻塞其他服务器；
//! - 所有 await 都在锁外：std Mutex 只护 map 增删查（临界区内无 IO / 无 await），
//!   调用前把 `Arc<RunningService>` 克隆出来。
//!
//! 授权边界（安全不变式）：`spawn_service` 运行的程序只能来自
//! bot-config.json 的 `mcpServers[].command`，唯一写入路径是设置页确认弹窗后的
//! `mcp_server_save`，模型侧无任何触达该配置的工具。程序名经
//! `STDIO_LAUNCH_ALLOWLIST` 白名单 + 逐臂字面量 match 构造——不存在
//! 「变量 → 进程名」数据流（Mimosa 命令注入闸），不走 shell、无字符串拼接执行；
//! 白名单与 match 臂双清单的一致性由单测 `stdio_allowlist_and_spawn_match_stay_in_sync`
//! 锁死（ 评审：新增白名单项漏改 match 臂会静默断 stdio 路）。
//!
//! rmcp API 备忘（3.5.0，写死防漂移）：
//! - 客户端服务：`().into_dyn().serve(transport)` → `RunningService<RoleClient, Box<dyn DynService<RoleClient>>>`
//!   （DynService: Send + Sync，官方注释「store the services in a collection」即此用法）
//! - `RunningService` Deref 到 `Peer<RoleClient>`：`list_all_tools()` / `call_tool_once()`
//! - 关闭：持有 `cancellation_token()` clone，`cancel()` 触发优雅退出；
//!   子进程由 `TokioChildProcess` 的 Drop 兜底 kill（`ChildWithCleanup`）

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult, Tool};
use rmcp::service::{DynService, RoleClient, RunningService, ServiceExt};
use tauri::{Emitter, Runtime};
use tokio::process::Command;

use super::config::{McpServerConfig, McpTransport, STDIO_LAUNCH_ALLOWLIST};
use super::stdio_transport::{CappedChildProcess, MAX_STDOUT_LINE};

/// 建立连接（spawn 子进程 + initialize 握手）的总超时
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// 工具发现失败后的重试冷却（毫秒）。
///
/// 冷却的意义：没有它，一台连不上的服务器会让**每一轮对话**都去连一遍
/// （连接超时 10s 起步），整轮提问被拖死。冷却期内面板照常显示错误，
/// 用户手动开关 / 改配置即解除（`reload_from_config` 清冷却）。
const DISCOVERY_RETRY_COOLDOWN_MS: i64 = 10 * 60 * 1000;

/// 已连接客户端服务句柄。`into_dyn()` 后可作集合元素（rmcp 官方推荐存集合形态）。
type ClientService = RunningService<RoleClient, Box<dyn DynService<RoleClient>>>;

/// 单服务器连接槽。三态：
///
/// - `Idle`（静默待命）：已启用、工具清单已知，但**没有子进程在跑**——
///   这是按需启停下的常态。工具清单来自磁盘快照（`tool_cache`）或上次连接。
/// - `Connected`（活连接）：只在一次工具调用期间存在，调用完即回收。
/// - `Down`（最近失败信息）：连接失败 / 工具发现失败。
///
/// fingerprint = 配置指纹：重载时同指纹跳过重连，配置变了才断开重建。
/// `name` 缓存自配置（连接槽唯一无 AppHandle 也能拿到的东西）：
/// 阶段 3 工具挂载的 `mcp_{server}_{tool}` 前缀来源。
enum Slot {
    /// 静默待命：已启用但无进程。工具清单可见，模型可以调（调用时才拉起）。
    Idle {
        name: String,
        /// 已知工具清单（可能为空 = 服务器确实没提供工具）
        tools: Vec<Tool>,
        /// 是否完成过一次工具发现。false = 还没发现过，模型清单里还没有它的工具，
        /// 需要按需发现一次（连上→列工具→断开→落盘快照）。
        discovered: bool,
    },
    Connected {
        fingerprint: u64,
        name: String,
        tools: Vec<Tool>,
        service: Arc<ClientService>,
        /// stdio 子进程的 pid（HTTP 连接没有子进程 → None）。
        /// 停止时靠它走信号阶梯（见 `close_old`）。
        pid: Option<u32>,
        /// 子进程起来的时刻（毫秒）——面板上算「已运行多久」。
        started_at_ms: i64,
    },
    Down {
        error: String,
    },
}

/// 一份进程快照（带采样时刻）。
#[derive(Clone)]
struct SampleEntry {
    /// 采样时刻（毫秒）
    at_ms: i64,
    /// 全机进程快照
    snap: Vec<crate::bot::procs::ProcSnapshot>,
}

/// 一个 MCP 子进程的资源读数（整棵进程树合计）。
#[derive(Debug, Clone)]
pub struct ChildResource {
    /// 常驻内存（字节）
    pub rss_bytes: u64,
    /// 累计 CPU 时间（秒）
    pub cpu_seconds: f64,
    /// CPU 占用率（百分比）；第一次采样没有前值 → None
    pub cpu_pct: Option<f64>,
    /// 子进程树在监听的 socket（TCP LISTEN / UDP 已绑定）
    pub listening: Vec<crate::bot::procs::ListeningSocket>,
}

/// 采样缓存 TTL（毫秒）。
const SAMPLE_TTL_MS: i64 = 5_000;

/// 进程级单例：模型循环 / registry 挂载层拿不到 AppHandle，
/// manager 用 OnceLock 全局化（连/断/状态命令全部经 `shared()` 访问）。
/// 测试可自建局部实例（e2e 模块），互不干扰。
static MANAGER: std::sync::OnceLock<McpManager> = std::sync::OnceLock::new();

pub fn shared() -> &'static McpManager {
    MANAGER.get_or_init(|| McpManager {
        slots: Mutex::new(HashMap::new()),
        stderr_tail: Mutex::new(HashMap::new()),
        sample_cache: Mutex::new(Vec::new()),
        cwd: Mutex::new(None),
        enabled: Mutex::new(Vec::new()),
        cache_dir: Mutex::new(None),
        cache_lock: Mutex::new(()),
        call_locks: Mutex::new(HashMap::new()),
        discovery_retry_at: Mutex::new(HashMap::new()),
    })
}

/// 状态快照条目（`mcp_status` 命令 / 阶段 5 UI 消费）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSlotStatus {
    /// "connected"（调用中）| "idle"（静默待命）| "pending"（待发现工具）
    /// | "down"（失败）| "absent"（进程内无槽位：未启用或还没轮到）
    pub state: String,
    pub error: Option<String>,
    pub tool_count: usize,
}

#[derive(Default)]
pub struct McpManager {
    slots: Mutex<HashMap<String, Slot>>,
    /// 各服务器子进程 stderr 尾巴（每台 ≤20 行，诊断外部服务器启动失败用）。
    /// 阶段 6 失败隔离强化：连接失败 / 调用断连时把尾巴附进错误与审计，
    /// 不用让用户去猜服务器为什么不起来。
    stderr_tail: Mutex<HashMap<String, Arc<Mutex<std::collections::VecDeque<String>>>>>,
    /// 资源采样缓存：全机快照一次，所有已连接的 MCP 子进程共用。
    ///
    /// TTL 5s：`mcp_status` 可能被前端轮询，每次都拍一张全机 `ps`（~50ms）就重了。
    /// 留两份是为了算占用率（需要前一次的累计 CPU 时间）。
    sample_cache: Mutex<Vec<SampleEntry>>,
    /// stdio 子进程的工作目录（重载时钉成数据目录）；None = 不设，沿用进程 cwd。
    /// 见 `reload_from_config` 的设置点：打包后进程 cwd 是 `/`，不钉住的话以
    /// 工作目录为文件沙箱根的服务器（Playwright）会把整块文件系统当可访问范围。
    cwd: Mutex<Option<std::path::PathBuf>>,
    /// 启用中的服务器配置快照（重载时刷新）。
    ///
    /// 按需启停后，工具发现不再发生在「重载」那一刻，而是在模型真要用工具时
    /// （`ensure_tools_known`）——那一刻进程内没有 AppHandle 上下文可读配置，
    /// 靠这份快照拿到「该发现哪几台、用什么配置」。
    enabled: Mutex<Vec<McpServerConfig>>,
    /// 工具快照的落盘目录（数据目录）；None = 还没重载过（无盘上快照可读）。
    cache_dir: Mutex<Option<std::path::PathBuf>>,
    /// 工具快照的读-改-写互斥（发现线程与设置页可能同时写同一份文件）。
    cache_lock: Mutex<()>,
    /// 每台服务器的调用闸：同一台服务器的并发调用串行化。
    ///
    /// 「调用完即回收」要求同一时刻只有一条活连接——stdio 服务器的连接本来
    /// 也只能服务一个调用方；并行调用排队即可，不丢请求。
    call_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// 工具发现失败后的重试冷却（服务器 id → 最早可重试时刻，毫秒）。
    ///
    /// 没有冷却 = 每轮对话都去连一遍坏服务器（连接超时 10s 起步），一轮提问
    /// 直接被拖死。冷却期内面板仍显示错误，用户手动开关/改配置即解除。
    discovery_retry_at: Mutex<HashMap<String, i64>>,
}

/// 环形缓冲容量（行）
const STDERR_TAIL_LINES: usize = 20;

/// 单服务器工具快照上限：外部服务器可返回
/// 任意多工具，无上限会经挂载层每轮全量拼进 tools JSON 撑爆请求与上下文。
/// 超限截断连接快照（进程内无 AppHandle，不另发审计——`mcp.tools_discovered` 的
/// tools 计数即截断后口径，设置页「查看工具」同源可见，dev 日志留痕）。
const MAX_TOOLS_PER_SERVER: usize = 128;

/// 工具数截断：保序截尾，超限时 dev 日志留痕
fn truncate_tools(cfg: &McpServerConfig, tools: Vec<Tool>) -> Vec<Tool> {
    let total = tools.len();
    if total <= MAX_TOOLS_PER_SERVER {
        return tools;
    }
    eprintln!(
        "[wmessage] MCP 服务器「{}」发现 {} 个工具，超上限 {MAX_TOOLS_PER_SERVER}，已截断",
        cfg.name, total
    );
    tools.into_iter().take(MAX_TOOLS_PER_SERVER).collect()
}

/// HTTP 传输的客户端构造（连接口安全闸）：字面量级闸（scheme / host 字面量）
/// + DNS 级闸（域名解析出的**每个** IP 都必须公网），并把解析结果钉给 reqwest。
///
/// 字面量级闸只挡「URL 里写着内网地址」，挡不住「URL 里写着公网域名、该域名
/// 解析到内网」——内网域名、DNS rebinding、`169.254.169.254.nip.io` 这类都在
/// DNS 这一跳被拦（判据单源在 `bot_web::check_public_url`，与抓取链同一份）。
/// 钉 IP 则堵住「校验通过之后改 DNS 应答」的 TOCTOU 窗口：校验与请求之间
/// 不再二次解析。
///
/// 头（鉴权头等）随每个请求发送：挂 reqwest 默认头。
async fn http_client_for_mcp(
    url: &str,
    headers: &std::collections::BTreeMap<String, String>,
) -> Result<reqwest::Client, String> {
    // 字面量级闸（与保存口同一判据）
    let parsed =
        super::config::http_url_is_public(url).map_err(|reason| format!("URL 安全闸：{reason}"))?;
    // DNS 级闸：解析出的每个 IP 都必须公网
    let addrs = crate::bot_web::check_public_url(&parsed)
        .await
        .map_err(|e| format!("URL 安全闸：{}", e.gate_message()))?;
    let host = parsed.host_str().unwrap_or_default().to_string();
    http_client_pinned(&host, &addrs, headers)
}

/// 钉住解析结果的 HTTP 客户端（DNS TOCTOU 防护）：host 固定解析到已校验过的
/// 地址，reqwest 不再二次 DNS；TLS SNI / 证书校验仍按原域名。
/// addrs 的端口必须是真实端口（`check_public_url` 已按 URL 端口重写）。
fn http_client_pinned(
    host: &str,
    addrs: &[std::net::SocketAddr],
    headers: &std::collections::BTreeMap<String, String>,
) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        // 评审 SSRF 项：禁跟随重定向——公网端点 302 到内网地址即穿透「仅公网」
        // 闸；Streamable HTTP 不依赖跨主机重定向
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(host, addrs);
    // 尾点变体一并登记：DNS 查的是去尾点的名字（check_public_url 归一过），
    // reqwest 查表用的是 URL 里的原样 host——两种形态都登记，钉 IP 一定命中
    let bare = host.trim_end_matches('.');
    if !bare.is_empty() && bare != host {
        builder = builder.resolve_to_addrs(bare, addrs);
    }
    if !headers.is_empty() {
        let mut map = reqwest::header::HeaderMap::new();
        for (k, v) in headers {
            match (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                reqwest::header::HeaderValue::from_str(v.as_str()),
            ) {
                (Ok(name), Ok(value)) => {
                    map.insert(name, value);
                }
                _ => return Err(format!("非法的自定义头：{k}")),
            }
        }
        builder = builder.default_headers(map);
    }
    builder
        .build()
        .map_err(|e| format!("HTTP 客户端构建失败：{e}"))
}

/// 串行化触碰进程级 `shared()` 的测试：cargo test 默认多线程并行，
/// e2e 用例的连接存活窗口会让 registry 的
/// `tools_json_with_mcp_without_connections_is_borrowed_static`（断言全局无连接）
/// flaky，共享槽位残留也会跨用例污染。registry 侧断言拿同一把锁（sync 测试用
/// `blocking_lock`，须在 block_on 之前取）。tokio Mutex：守卫 Send，e2e 持锁
/// 跨 await 不触发 clippy await_holding_lock（评审 M-15）。
#[cfg(test)]
pub(crate) static SHARED_MCP_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 配置指纹：任何影响连接/行为的字段变化都会换指纹（id 变化 = 换条目，天然换槽）。
///
/// **显式枚举字段**：不能用 `serde_json::to_value(cfg)`——env/headers 是机密
/// 字段，`Serialize` 里被 `skip_serializing` 剥掉，用序列化值算指纹就看不见
/// 它们：改了 API token / 环境变量不换指纹 = 不重连 = 旧凭据继续用
/// （配置已保存、面板已刷新，实际行为没变）。
pub fn fingerprint(cfg: &McpServerConfig) -> u64 {
    let mut h = DefaultHasher::new();
    cfg.id.hash(&mut h);
    cfg.name.hash(&mut h);
    cfg.transport.hash(&mut h);
    cfg.command.hash(&mut h);
    cfg.args.hash(&mut h);
    cfg.url.hash(&mut h);
    cfg.env.hash(&mut h);
    cfg.headers.hash(&mut h);
    cfg.timeout_secs.hash(&mut h);
    cfg.enabled.hash(&mut h);
    h.finish()
}

impl McpManager {
    fn slots(&self) -> std::sync::MutexGuard<'_, HashMap<String, Slot>> {
        // Mutex 中毒只在持锁段 panic 时发生；本结构持锁段无 panic 点，
        // 中毒 = 不可恢复内部错误，按上锁惯例 unwrap 让其响亮暴露
        self.slots.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 确保该服务器已连接且连接对应的正是当前配置（指纹一致）。
    /// 幂等：已连接且同指纹 → no-op。失败返回错误文案（槽位转 Down）。
    ///
    /// 这是「拉起一条活连接」的底层原语——按需启停下它只在两处被调用：
    /// 工具调用（`call_tool`）与工具发现（`discover_tools`）。
    /// **重载不再调用它**（打开开关不启进程）。
    pub async fn ensure_connected(&self, cfg: &McpServerConfig) -> Result<(), String> {
        let fp = fingerprint(cfg);
        if let Some(Slot::Connected { fingerprint, .. }) = self.slots().get(&cfg.id) {
            if *fingerprint == fp {
                return Ok(());
            }
        }
        // 配置变了 / Down / 无槽：断旧（cancel 触发优雅退出，不阻塞等待；
        // 子进程由传输 Drop 兜底 kill）再连新。并发同 id 的极端情况：两线程
        // 各自 spawn，后插入者覆盖槽位，旧连接 Arc 落地即被 kill（瞬态自愈）。
        // stderr 旧尾巴同步清掉（评审 M-17）：同 id 重连复用环形缓冲，
        // 不清会把旧进程的诊断输出混进新进程的错误文案。
        let old = self.slots().remove(&cfg.id);
        self.prune_stderr(&cfg.id);
        Self::close_old(old).await;
        match self.spawn_service(cfg).await {
            Ok((service, tools, pid)) => {
                self.slots().insert(
                    cfg.id.clone(),
                    Slot::Connected {
                        fingerprint: fp,
                        name: cfg.name.clone(),
                        tools,
                        service,
                        pid,
                        started_at_ms: crate::db::now_ms(),
                    },
                );
                Ok(())
            }
            Err(e) => {
                self.slots()
                    .insert(cfg.id.clone(), Slot::Down { error: e.clone() });
                Err(e)
            }
        }
    }

    /// 断开并移除槽位（幂等：无槽 no-op）。取消令牌触发优雅退出，
    /// 子进程由 TokioChildProcess Drop 兜底 kill（不孤儿）。
    /// 锁内只做 remove，guard 先落地再 await（guard 过 await = future 非 Send）。
    pub async fn disconnect(&self, id: &str) {
        let old = self.slots().remove(id);
        self.prune_stderr(id);
        // 停用 / 删除 = 用户显式动作 → 发现冷却一并解除（下次可直接重试）
        self.discovery_retry_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        Self::close_old(old).await;
    }

    /// 每台服务器的调用闸：同一台服务器的并发调用串行化。
    ///
    /// 「调用完即回收」要求同一时刻只有一条活连接（stdio 服务器的连接本来
    /// 也只服务一个调用方）；并行调用排队即可，不丢请求。
    fn call_lock(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.call_locks.lock().unwrap_or_else(|e| e.into_inner());
        locks
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// 工具清单是否已知（已知 = 不用再发现一次）。
    pub fn tools_known(&self, id: &str) -> bool {
        match self.slots().get(id) {
            Some(Slot::Idle { discovered, .. }) => *discovered,
            Some(Slot::Connected { .. }) => true,
            _ => false,
        }
    }

    /// 清掉某台服务器的工具发现冷却（用户显式动作：开关 / 改配置 / 点「工具」）。
    pub fn clear_discovery_cooldown(&self, id: &str) {
        self.discovery_retry_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
    }

    /// 为某台服务器按需发现一次工具清单（设置页「查看工具」触发）。
    ///
    /// 服务器不在启用列表里 → 不做（禁用 / 已删除的服务器没有工具可看）。
    pub async fn discover_tools_for(&self, app: &tauri::AppHandle, id: &str) {
        let cfg = self
            .enabled
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|c| c.id == id)
            .cloned();
        let Some(cfg) = cfg else { return };
        self.discover_tools(app, &cfg).await;
    }

    /// 工具发现失败后的冷却期是否未过（未过则本轮不重试）。
    fn discovery_cooling(&self, id: &str) -> bool {
        let retry_at = self
            .discovery_retry_at
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match retry_at.get(id) {
            Some(at) => crate::db::now_ms() < *at,
            None => false,
        }
    }

    /// 按需发现工具清单：**只有快照缺失的启用服务器才连一次**。
    ///
    /// 由主 agent 会话进入模型循环前调用（子 agent 工具面静态、不挂 MCP 工具，
    /// 不该在这里拉起服务器）。发现完立刻断开连接——工具清单已落盘，
    /// 之后不必再连；模型真要调用时再按需拉起。
    pub async fn ensure_tools_known(&self, app: &tauri::AppHandle) {
        let targets: Vec<McpServerConfig> = self
            .enabled
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for cfg in targets {
            if self.tools_known(&cfg.id) {
                continue;
            }
            if self.discovery_cooling(&cfg.id) {
                continue;
            }
            self.discover_tools(app, &cfg).await;
        }
    }

    /// 工具发现：连上 → 列工具 → 断开 → 落盘快照 → 槽位转静默待命。
    ///
    /// 失败时槽位转 Down 并进入冷却（冷却期内不再重试）——没有冷却的话，
    /// 一台坏服务器会让**每一轮对话**都去连一遍（连接超时 10s 起步），
    /// 整轮提问被拖死。
    async fn discover_tools(&self, app: &tauri::AppHandle, cfg: &McpServerConfig) {
        // 有活连接在跑（调用进行中）→ 不动它：活连接的工具清单就是最新的，
        // 覆盖槽位会把在途调用的连接抽走。
        if matches!(self.slots().get(&cfg.id), Some(Slot::Connected { .. })) {
            return;
        }
        let fp = fingerprint(cfg);
        // 换了一代进程：旧进程的 stderr 尾巴不能混进新一代的诊断
        self.prune_stderr(&cfg.id);
        match self.spawn_service(cfg).await {
            Ok((service, tools, pid)) => {
                // 这一条连接只为拿工具清单而存在：发现完即断
                Self::close_old(Some(Slot::Connected {
                    fingerprint: fp,
                    name: cfg.name.clone(),
                    tools: tools.clone(),
                    service,
                    pid,
                    started_at_ms: crate::db::now_ms(),
                }))
                .await;
                if let Err(e) = self.write_tool_cache(cfg, fp, &tools) {
                    crate::audit::write_event(
                        app,
                        crate::audit::AuditLevel::Warn,
                        "mcp.tool_cache_write_failed",
                        &[("id", cfg.id.clone()), ("err", e)],
                    );
                }
                let tool_count = tools.len();
                self.slots().insert(
                    cfg.id.clone(),
                    Slot::Idle {
                        name: cfg.name.clone(),
                        tools,
                        discovered: true,
                    },
                );
                self.discovery_retry_at
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&cfg.id);
                crate::audit::write_event(
                    app,
                    crate::audit::AuditLevel::Info,
                    "mcp.tools_discovered",
                    &[
                        ("id", cfg.id.clone()),
                        ("name", cfg.name.clone()),
                        ("tools", tool_count.to_string()),
                    ],
                );
            }
            Err(e) => {
                let e = self.format_with_stderr_tail(&cfg.id, e);
                self.slots()
                    .insert(cfg.id.clone(), Slot::Down { error: e.clone() });
                self.discovery_retry_at
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(
                        cfg.id.clone(),
                        crate::db::now_ms() + DISCOVERY_RETRY_COOLDOWN_MS,
                    );
                crate::audit::write_event(
                    app,
                    crate::audit::AuditLevel::Warn,
                    "mcp.discover_failed",
                    &[
                        ("id", cfg.id.clone()),
                        ("name", cfg.name.clone()),
                        ("err", e),
                    ],
                );
            }
        }
    }

    /// 工具清单落盘（快照 = 「工具可见性」与「活连接」解耦的载体）。
    ///
    /// 读-改-写全程持 `cache_lock`：模型循环的按需发现与设置页点「工具」可能
    /// 同时写同一份文件，不互斥会互相覆盖条目（丢一份 = 下次多发现一次）。
    ///
    /// 落盘失败不影响本次会话（内存里已有清单，模型照样看得见）——由调用方
    /// 记审计。
    fn write_tool_cache(
        &self,
        cfg: &McpServerConfig,
        fp: u64,
        tools: &[Tool],
    ) -> Result<(), String> {
        let Some(dir) = self
            .cache_dir
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        else {
            return Err("数据目录未初始化".into());
        };
        let _g = self.cache_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut cache = super::tool_cache::ToolCache::load(&dir);
        cache.put(&cfg.id, fp, &cfg.name, tools.to_vec());
        cache.save(&dir)
    }

    /// 调用完回收活连接：进程停掉，槽位转回静默待命（工具清单留着）。
    ///
    /// 「调用完即关」是按需启停的核心：子进程不跨调用存活，下一次调用重新拉起。
    /// 期间若 reload 写入了新槽（配置变了），不覆盖它。
    async fn retire(&self, id: &str, fp: u64) {
        let old = {
            let mut slots = self.slots();
            match slots.get(id) {
                Some(Slot::Connected { fingerprint, .. }) if *fingerprint == fp => slots.remove(id),
                _ => None,
            }
        };
        let Some(slot) = old else { return };
        let Slot::Connected { name, tools, .. } = &slot else {
            return;
        };
        let name = name.clone();
        let tools = tools.clone();
        Self::close_old(Some(slot)).await;
        let mut slots = self.slots();
        if !slots.contains_key(id) {
            slots.insert(
                id.to_string(),
                Slot::Idle {
                    name,
                    tools,
                    discovered: true,
                },
            );
        }
    }

    /// 断开一个连接：先走信号阶梯把子进程树停干净，再取消令牌让 rmcp 收尾。
    ///
    /// 顺序不能反：rmcp 的 `TokioChildProcess` Drop 只会 fire-and-forget 一个
    /// SIGKILL——没有优雅退出窗口、不回收脱离进程组的子孙，而且那个 kill 是
    /// `tokio::spawn` 出去的，应用正在退出时它可能根本没来得及跑（子进程留成孤儿）。
    /// 阶梯先跑完，Drop 的 kill 就只是对死进程的一记空操作。
    async fn close_old(slot: Option<Slot>) -> bool {
        if let Some(Slot::Connected { service, pid, .. }) = slot {
            let stopped = match pid {
                Some(pid) => {
                    stop_stdio_tree(pid).await;
                    true
                }
                // HTTP 连接没有本地子进程
                None => false,
            };
            service.cancellation_token().cancel();
            return stopped;
        }
        false
    }

    /// 拉起传输 + initialize 握手 + 首次工具发现。
    /// 程序名白名单校验在 config::validate_server（保存口）+ 此处双重把关。
    /// stderr 管道化进环形缓冲（阶段 6）：子进程的诊断输出不再丢失。
    async fn spawn_service(
        &self,
        cfg: &McpServerConfig,
    ) -> Result<(Arc<ClientService>, Vec<Tool>, Option<u32>), String> {
        match cfg.transport_kind() {
            McpTransport::Stdio => {
                let command = cfg.command.as_deref().unwrap_or("").trim();
                if !STDIO_LAUNCH_ALLOWLIST.contains(&command) {
                    return Err(format!(
                        "命令「{command}」不在启动器白名单内（允许：{}）",
                        STDIO_LAUNCH_ALLOWLIST.join(" / ")
                    ));
                }
                // 白名单逐臂字面量构造：源码层面不存在「运行时变量 → 进程名」路径
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
                    // 与 contains 侧同文案（ 评审 parity 单测锁双清单；此臂
                    // 实际不可达，防御保留）
                    _ => {
                        return Err(format!(
                            "命令「{command}」不在启动器白名单内（允许：{}）",
                            STDIO_LAUNCH_ALLOWLIST.join(" / ")
                        ))
                    }
                };
                cmd.args(&cfg.args)
                    .envs(cfg.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());
                if let Some(dir) = self.cwd.lock().unwrap_or_else(|e| e.into_inner()).clone() {
                    cmd.current_dir(dir);
                }
                // 自成进程组：停止时整组发信号（组外逃逸的子孙由 ppid 树补）。
                // 不建组的话子进程与宿主同组，整组杀会连宿主一起带走。
                #[cfg(unix)]
                {
                    cmd.process_group(0);
                }
                // Windows 桌面应用不弹控制台窗（与 bot_py Python 子进程同惯例）
                #[cfg(windows)]
                {
                    cmd.creation_flags(crate::bot::procs::CREATE_NO_WINDOW);
                }
                let mut child = cmd
                    .spawn()
                    .map_err(|e| format!("启动命令「{command}」失败：{e}"))?;
                // pid 留着给停止路径（信号阶梯要按 pid 找进程组与子孙）
                let child_pid = child.id();
                let stdout = child
                    .stdout
                    .take()
                    .ok_or_else(|| "子进程 stdout 未管道化".to_string())?;
                let stdin = child
                    .stdin
                    .take()
                    .ok_or_else(|| "子进程 stdin 未管道化".to_string())?;
                // stderr 后台收割：行进环形缓冲，进程退出时流关闭、任务自然结束
                if let Some(stderr) = child.stderr.take() {
                    let tail = self.stderr_ring(&cfg.id);
                    tauri::async_runtime::spawn(async move {
                        crate::bot::mcp::stdio_transport::harvest_stderr(
                            stderr,
                            tail,
                            STDERR_TAIL_LINES,
                        )
                        .await;
                    });
                }
                // 读侧带行长上限的传输：外部 MCP 服务器一条不带换行的长行
                // 就能把宿主内存撑爆（rmcp 原生 read_until 无上限，见
                // mcp/stdio_transport 模块头）
                let transport = CappedChildProcess::new(child, stdout, stdin, MAX_STDOUT_LINE);
                let service = tokio::time::timeout(CONNECT_TIMEOUT, ().into_dyn().serve(transport))
                    .await
                    .map_err(|_| {
                        self.format_with_stderr_tail(
                            &cfg.id,
                            format!("连接超时（{}s）：{command}", CONNECT_TIMEOUT.as_secs()),
                        )
                    })?
                    .map_err(|e| {
                        self.format_with_stderr_tail(&cfg.id, format!("初始化握手失败：{e}"))
                    })?;
                // 评审项：tools/list 无响应的服务器不能挂死重载循环与
                // 懒重连路径——initialize 与工具发现分别包超时
                let tools = tokio::time::timeout(CONNECT_TIMEOUT, service.list_all_tools())
                    .await
                    .map_err(|_| {
                        self.format_with_stderr_tail(
                            &cfg.id,
                            format!("工具发现超时（{}s）", CONNECT_TIMEOUT.as_secs()),
                        )
                    })?
                    .map_err(|e| {
                        self.format_with_stderr_tail(&cfg.id, format!("工具发现失败：{e}"))
                    })?;
                let tools = truncate_tools(cfg, tools);
                Ok((Arc::new(service), tools, child_pid))
            }
            // Streamable HTTP（阶段 7/8）：发请求前再过 URL 安全闸
            //（保存口 validate_server 已把关，此处防手改配置文件绕过）
            McpTransport::Http => {
                let url = cfg.url.as_deref().unwrap_or("").trim().to_string();
                let client = http_client_for_mcp(&url, &cfg.headers).await?;
                use rmcp::transport::streamable_http_client::{
                    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
                };
                let transport = StreamableHttpClientTransport::with_client(
                    client,
                    StreamableHttpClientTransportConfig::with_uri(url),
                );
                let service = tokio::time::timeout(CONNECT_TIMEOUT, ().into_dyn().serve(transport))
                    .await
                    .map_err(|_| format!("连接超时（{}s）", CONNECT_TIMEOUT.as_secs()))?
                    .map_err(|e| format!("初始化握手失败：{e}"))?;
                let tools = tokio::time::timeout(CONNECT_TIMEOUT, service.list_all_tools())
                    .await
                    .map_err(|_| format!("工具发现超时（{}s）", CONNECT_TIMEOUT.as_secs()))?
                    .map_err(|e| format!("工具发现失败：{e}"))?;
                let tools = truncate_tools(cfg, tools);
                // HTTP 连接没有本地子进程
                Ok((Arc::new(service), tools, None))
            }
        }
    }

    /// 已连接的本地子进程：serverId → (pid, 起来时刻)
    pub fn connected_children(&self) -> HashMap<String, (u32, i64)> {
        let guard = self.slots();
        let mut out = HashMap::new();
        for (id, slot) in guard.iter() {
            if let Slot::Connected {
                pid: Some(pid),
                started_at_ms,
                ..
            } = slot
            {
                out.insert(id.clone(), (*pid, *started_at_ms));
            }
        }
        out
    }

    /// 采样 MCP 子进程的资源占用（整棵进程树合计），带 5s TTL 缓存。
    ///
    /// 口径与受管服务层**同一套代码**（`bot::procs` 的 take_snapshot + tree_sample
    /// + cpu_usage_pct）——两处各写一遍迟早会漂。
    ///
    /// 阻塞：可能拍一张全机 `ps`（~50ms），调用方要挪到 blocking 线程。
    pub fn sample_children(&self, pids: &[u32]) -> HashMap<u32, ChildResource> {
        self.sample_children_within(pids, SAMPLE_TTL_MS)
    }

    /// 同上，但 TTL 可指定（测试用 0 = 每次都重采，逼出「第二次才有占用率」那条路径）。
    fn sample_children_within(&self, pids: &[u32], ttl_ms: i64) -> HashMap<u32, ChildResource> {
        // 没有子进程可采 → 不拍全机 ps。按需启停下这恰恰是常态（服务器平时
        // 静默待命、没有进程），而面板可见时每 5s 轮询一次——每次拍一张全机
        // 快照（~50ms + 一次进程表遍历）在没有要采的东西时纯属白干。
        if pids.is_empty() {
            return HashMap::new();
        }
        let mut guard = self.sample_cache.lock().unwrap_or_else(|e| e.into_inner());
        let now_ms = crate::db::now_ms();
        let fresh = guard
            .first()
            .map(|e| now_ms - e.at_ms < ttl_ms)
            .unwrap_or(false);
        if !fresh {
            guard.insert(
                0,
                SampleEntry {
                    at_ms: now_ms,
                    snap: crate::bot::procs::take_snapshot(),
                },
            );
            guard.truncate(2);
        }
        let cur = &guard[0];
        let prev = guard.get(1);
        // 监听 socket：整棵树（组长 + 后代）合计。一次平台调用采完所有 pid，
        // 再按树分发——每个 pid 单独调一次是 N 倍开销。
        let trees: Vec<(u32, Vec<u32>)> = pids
            .iter()
            .map(|pid| (*pid, crate::bot::procs::tree_pids(&cur.snap, *pid)))
            .collect();
        let all_pids: Vec<u32> = trees.iter().flat_map(|(_, t)| t.iter().copied()).collect();
        let sockets = crate::bot::procs::listening_sockets(&all_pids);
        let mut out = HashMap::new();
        for (pid, tree) in trees {
            let Some(sample) = crate::bot::procs::tree_sample(&cur.snap, pid) else {
                continue;
            };
            let cpu_pct = prev.and_then(|p| {
                let prev_sample = crate::bot::procs::tree_sample(&p.snap, pid)?;
                crate::bot::procs::cpu_usage_pct(
                    prev_sample.cpu_seconds,
                    p.at_ms,
                    sample.cpu_seconds,
                    cur.at_ms,
                )
            });
            let listening = crate::bot::procs::merge_sockets(
                tree.iter()
                    .filter_map(|p| sockets.by_pid.get(p).cloned())
                    .collect(),
            );
            out.insert(
                pid,
                ChildResource {
                    rss_bytes: sample.rss_bytes,
                    cpu_seconds: sample.cpu_seconds,
                    cpu_pct,
                    listening,
                },
            );
        }
        out
    }

    /// 取（无则建）该服务器的 stderr 环形缓冲句柄
    fn stderr_ring(&self, id: &str) -> Arc<Mutex<std::collections::VecDeque<String>>> {
        self.stderr_tail
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(id.to_string())
            .or_default()
            .clone()
    }

    /// 尾巴快照（最后 n 行，给错误文案/审计用）
    fn stderr_tail_snapshot(&self, id: &str, n: usize) -> Vec<String> {
        let ring = self.stderr_tail.lock().unwrap_or_else(|e| e.into_inner());
        match ring.get(id) {
            Some(q) => {
                let q = q.lock().unwrap_or_else(|e| e.into_inner());
                q.iter().rev().take(n).rev().cloned().collect()
            }
            None => Vec::new(),
        }
    }

    /// 失败文案 + stderr 尾巴（有尾巴才附，最后 3 行）
    fn format_with_stderr_tail(&self, id: &str, msg: String) -> String {
        let tail = self.stderr_tail_snapshot(id, 3);
        if tail.is_empty() {
            msg
        } else {
            format!("{msg}；服务器输出：{}", tail.join(" / "))
        }
    }

    /// 清掉已移除服务器的 stderr 缓冲（disconnect / reload 对齐时调）
    fn prune_stderr(&self, id: &str) {
        self.stderr_tail
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
    }

    /// 按配置全量对齐（启动 / 配置变更后调用）。
    ///
    /// **不预连任何子进程**：打开启用开关只是「允许机器人用这台服务器」，
    /// 进程留到模型真要调用它的工具时才拉起（`call_tool`），调用完即回收。
    /// 工具清单来自盘上快照（`tool_cache`）——没有活连接也能挂给模型。
    ///
    /// 单台失败只记审计，不影响其他服务器（失败隔离）。
    /// 具体形态 `AppHandle`（= Wry）：load_config 是具体签名，本方法仅生产路径调用。
    pub async fn reload_from_config(&self, app: &tauri::AppHandle) {
        let dir = crate::db::data_dir(app);
        *self.cwd.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir.clone());
        *self.cache_dir.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir.clone());
        let servers = crate::bot::config::io::load_config(app)
            .mcp_servers
            .unwrap_or_default();
        // 盘上工具快照（指纹一致才有效——配置变了这份就是陈旧的）
        let cache = super::tool_cache::ToolCache::load(&dir);
        let disconnected = self.apply_config(&servers, &cache).await;
        for id in disconnected {
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Info,
                "mcp.disconnected",
                &[("id", id.clone())],
            );
        }
        let _ = app.emit("mcp-status-changed", ());
    }

    /// 按配置对齐槽位（无 IO / 无 AppHandle，单测直打）。
    ///
    /// - 启用中的服务器 → **静默待命**（不连！工具清单取自盘上快照）
    /// - 不再启用 / 已删除的 → 断开并摘槽
    /// - 指纹变了 → 断开旧连接（换了一代连接，stderr 尾巴也一并清掉）
    ///
    /// 返回被摘掉的服务器 id（调用方记审计）。
    pub async fn apply_config(
        &self,
        servers: &[McpServerConfig],
        cache: &super::tool_cache::ToolCache,
    ) -> Vec<String> {
        // 启用中的配置快照：按需发现工具时用（那一刻进程内没有 AppHandle 上下文）
        let enabled: Vec<McpServerConfig> = servers.iter().filter(|s| s.enabled).cloned().collect();
        *self.enabled.lock().unwrap_or_else(|e| e.into_inner()) = enabled.clone();
        let mut want: HashMap<&str, &McpServerConfig> = HashMap::new();
        for s in enabled.iter() {
            want.insert(s.id.as_str(), s);
        }
        // 断开：槽位里有、但配置里不再启用（或整体不存在）的 id
        let stale: Vec<String> = self
            .slots()
            .keys()
            .filter(|id| !want.contains_key(id.as_str()))
            .cloned()
            .collect();
        for id in &stale {
            self.disconnect(id).await;
        }
        // 静默待命：启用中的服务器转 Idle（工具清单取自盘上快照）
        for (id, cfg) in want {
            let fp = fingerprint(cfg);
            // 活连接且指纹一致 → 保留（调用进行中，调用完它自己回收）
            if matches!(
                self.slots().get(id),
                Some(Slot::Connected { fingerprint, .. }) if *fingerprint == fp
            ) {
                continue;
            }
            // 其余情形（无槽 / 指纹变了 / Down / Idle）→ 断开旧的再转 Idle
            let old = self.slots().remove(id);
            if old.is_some() {
                // 换了一代连接：旧进程的 stderr 尾巴不能混进新一代的诊断
                self.prune_stderr(id);
            }
            Self::close_old(old).await;
            let (tools, discovered) = match cache.entry(id, fp) {
                Some(e) => (e.tools.clone(), true),
                None => (Vec::new(), false),
            };
            self.slots().insert(
                id.to_string(),
                Slot::Idle {
                    name: cfg.name.clone(),
                    tools,
                    discovered,
                },
            );
            // 配置刚变过 = 用户显式动作 → 发现冷却解除（下次可直接重试）
            self.discovery_retry_at
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(id);
        }
        stale
    }

    /// 已连接服务器的缓存工具快照（连接成功时的 list_all_tools 结果；
    /// 静默待命时取盘上快照；未连接 / 无槽 → 空）。阶段 3 schema 注入与设置页
    /// 「查看工具」共用。
    pub fn tools_of(&self, id: &str) -> Vec<Tool> {
        match self.slots().get(id) {
            Some(Slot::Connected { tools, .. }) => tools.clone(),
            Some(Slot::Idle { tools, .. }) => tools.clone(),
            _ => Vec::new(),
        }
    }

    /// 可挂载服务器的视图（阶段 3 schema 注入 + 阶段 4 调用反查）：
    /// (服务器 id, 服务器名, tools)。名字来自连接槽缓存（发现/连接时带出）。
    /// **静默待命的服务器也算在内**——按需启停下服务器平时不连着，
    /// 但工具照样要挂给模型，否则模型看不见它、永远调不到它。
    /// 按 id 排序输出（评审）：HashMap 迭代序非确定，而不确定性会让撞名消歧的
    /// **首次分配**顺序跨重启翻转。排序后首次分配恒定；之后的稳定由
    /// `mount::NameRegistry` 兜住（名字一经分配就不再变，断连/重连不漂移）。
    pub fn mounted(&self) -> Vec<(String, String, Vec<Tool>)> {
        let mut out: Vec<(String, String, Vec<Tool>)> = self
            .slots()
            .iter()
            .filter_map(|(id, slot)| match slot {
                Slot::Connected { name, tools, .. } if !tools.is_empty() => {
                    Some((id.clone(), name.clone(), tools.clone()))
                }
                Slot::Idle { name, tools, .. } if !tools.is_empty() => {
                    Some((id.clone(), name.clone(), tools.clone()))
                }
                _ => None,
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// 单服务器槽位状态快照（mcp_status 命令）
    ///
    /// 三态 + 一个待发现态：
    /// - `connected`：调用中（活连接）
    /// - `idle`：静默待命——已启用、工具清单已知、**没有进程在跑**（按需启停常态）
    /// - `pending`：已启用但工具清单还没发现（首次对话时按需发现一次）
    /// - `down`：最近一次连接 / 发现失败
    /// - `absent`：进程内无槽位（未启用或还没轮到）
    pub fn status_of(&self, id: &str) -> McpSlotStatus {
        match self.slots().get(id) {
            Some(Slot::Connected { tools, .. }) => McpSlotStatus {
                state: "connected".into(),
                error: None,
                tool_count: tools.len(),
            },
            Some(Slot::Idle {
                tools, discovered, ..
            }) => McpSlotStatus {
                state: if *discovered { "idle" } else { "pending" }.into(),
                error: None,
                tool_count: tools.len(),
            },
            Some(Slot::Down { error, .. }) => McpSlotStatus {
                state: "down".into(),
                error: Some(error.clone()),
                tool_count: 0,
            },
            None => McpSlotStatus {
                state: "absent".into(),
                error: None,
                tool_count: 0,
            },
        }
    }

    /// 取当前配置对应的服务句柄（调用前检查指纹：配置已变 → 提示重连）。
    fn connected_service(&self, id: &str, fp: u64) -> Result<Arc<ClientService>, String> {
        match self.slots().get(id) {
            Some(Slot::Connected {
                fingerprint,
                service,
                ..
            }) if *fingerprint == fp => Ok(service.clone()),
            // 槽在但指纹过期（ 评审：与「真没连」分开报，刚改完配置的用户
            // 不再看到误导性的「服务器未连接」；ensure_connected 的懒重连会接手）
            Some(Slot::Connected { .. }) => Err("配置已变更，正在自动重连，请重试".into()),
            Some(Slot::Down { error, .. }) => Err(format!("服务器不可用：{error}")),
            _ => Err("服务器未连接".into()),
        }
    }

    /// 调用外部工具（阶段 4 dispatch 消费）。
    ///
    /// **按需启停**：这一刻才拉起子进程（此前服务器静默待命、没有进程在跑），
    /// 调用结束立刻回收——进程不跨调用存活。
    ///
    /// 同一台服务器的并发调用串行（`call_lock`）：活连接一次只服务一个调用方。
    /// 入口先 ensure_connected（懒重连：Down / 配置已变都会在这里重建），
    /// 再带超时调用。
    /// 连接级失败（TransportClosed/Send）→ 槽位转 Down，返回明确错误文案，
    /// 下次调用自动重连；**同一调用不原地重试**（外部工具可能有副作用，防双执行）。
    pub async fn call_tool(
        &self,
        cfg: &McpServerConfig,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<CallToolResult, String> {
        let call_lock = self.call_lock(&cfg.id);
        let _serial = call_lock.lock().await;
        let fp = fingerprint(cfg);
        self.ensure_connected(cfg).await?;
        let service = self.connected_service(&cfg.id, fp)?;
        let result = self.invoke_tool(&service, cfg, tool, args).await;
        // 调用完即回收：子进程不跨调用存活（下一次调用重新拉起）
        self.retire(&cfg.id, fp).await;
        result
    }

    /// 带超时的工具调用内核（连接级失败 → 槽位转 Down）。
    async fn invoke_tool(
        &self,
        service: &Arc<ClientService>,
        cfg: &McpServerConfig,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<CallToolResult, String> {
        let mut params = CallToolRequestParams::new(tool.to_string());
        params.arguments = args.as_object().cloned();
        let timeout = Duration::from_secs(cfg.effective_timeout_secs());
        match tokio::time::timeout(timeout, service.call_tool_once(params)).await {
            Err(_) => Err(format!("工具调用超时（{}s）：{tool}", timeout.as_secs())),
            Ok(Err(rmcp::ServiceError::TransportClosed)) => {
                self.mark_down(&cfg.id, fingerprint(cfg), "连接已断开".into());
                Err(self.format_with_stderr_tail(
                    &cfg.id,
                    "MCP 服务器连接已断开（已标记，下次调用自动重连）".into(),
                ))
            }
            Ok(Err(e @ rmcp::ServiceError::TransportSend(_))) => {
                self.mark_down(&cfg.id, fingerprint(cfg), e.to_string());
                Err(self.format_with_stderr_tail(
                    &cfg.id,
                    format!("MCP 服务器连接异常（已标记，下次调用自动重连）：{e}"),
                ))
            }
            Ok(Err(e)) => Err(e.to_string()),
            Ok(Ok(CallToolResponse::Complete(result))) => Ok(result),
            Ok(Ok(_)) => {
                Err("服务器返回了未完成的调用状态（input_required/task），暂不支持".into())
            }
        }
    }

    /// 连接级失败标记 Down。带调用方所见连接的指纹：槽位 Connected 但指纹已变
    /// = 连接已被重建（新代），本次失败属陈旧代，不得覆盖新槽。
    fn mark_down(&self, id: &str, fp: u64, error: String) {
        let mut slots = self.slots();
        if let Some(Slot::Connected { fingerprint, .. }) = slots.get(id) {
            if *fingerprint != fp {
                return;
            }
        }
        slots.insert(id.to_string(), Slot::Down { error });
    }

    /// 退出清理：停掉本地子进程树 + 取消全部连接令牌并清空槽位。
    ///
    /// 返回停掉的本地子进程数（HTTP 连接没有子进程，不计）。
    pub async fn shutdown(&self) -> usize {
        let slots: Vec<(String, Slot)> = self.slots().drain().collect();
        let mut stopped = 0usize;
        for (id, slot) in slots {
            // stderr 尾巴一并清（ 评审：per-id 路径 disconnect/ensure_connected
            // 都 prune，批量 shutdown 漏了会留残留诊断，下个同 id 服务器错配）
            self.prune_stderr(&id);
            if Self::close_old(Some(slot)).await {
                stopped += 1;
            }
        }
        stopped
    }
}

/// 停掉一个 MCP stdio 子进程树：走与受管服务层同一套信号阶梯。
///
/// 与受管服务的差别只有一处：那边等的是「登记表状态转终态」，这边等的是
/// 「进程真的不在了」（存活探测）——MCP 连接没有登记表。
///
/// 阶梯含等待窗口（100ms + 400ms）与若干次进程表快照，是阻塞操作，
/// 必须挪到 blocking 线程，不能占着 async 运行时的线程。
async fn stop_stdio_tree(pid: u32) {
    let handle = tauri::async_runtime::spawn_blocking(move || {
        crate::bot::procs::stop_tree(pid, pid as i32, |grace| {
            crate::bot::procs::wait_pid_exit(pid, grace)
        })
    });
    match handle.await {
        Ok(Ok(tree)) => {
            // 阶梯跑完不等于进程真没了——再核一次，别把「还在跑」记成「已停」
            if crate::bot::procs::pid_exists(pid) {
                eprintln!(
                    "[mcp] stdio 子进程树停止后仍在运行：pid={pid} method={}",
                    tree.method
                );
            } else {
                eprintln!(
                    "[mcp] stdio 子进程树已停：pid={pid} method={} 补杀子孙={} 耗时={}ms",
                    tree.method, tree.descendants_killed, tree.elapsed_ms
                );
            }
        }
        // 本平台没有停止手段——不是「这个进程停不掉」，是这台机器根本没法停它
        Ok(Err(e)) => eprintln!("[mcp] stdio 子进程树无法停止（pid={pid}）：{e}"),
        Err(e) => eprintln!("[mcp] stdio 子进程树停止失败（pid={pid}）：{e}"),
    }
}

/// 退出清理的同步外壳（cleanup_on_exit 是同步上下文）：清进程级单例全部连接。
/// 返回停掉的本地子进程数（供审计记账）。
pub fn shutdown_all<R: Runtime>(_app: &tauri::AppHandle<R>) -> usize {
    tauri::async_runtime::block_on(async move { shared().shutdown().await })
}

#[cfg(test)]
mod send_probe;

#[cfg(test)]
mod fingerprint_tests;

#[cfg(test)]
mod http_gate;

#[cfg(test)]
mod e2e_stdio;
