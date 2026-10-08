//! MCP 连接生命周期管理（阶段 2）：连 / 断 / 指纹对齐重载 / 懒重连。
//!
//! 托管形态：进程级 `OnceLock` 单例（`shared()`），lib.rs setup 触发
//! 启动重载；命令、registry 挂载层、退出清理全部经 `shared()` 访问。
//!
//! 隔离模型：
//! - stdio 服务器 = 独立子进程，崩溃不伤宿主（传输断开 → 槽位转 Down）；
//! - 懒重连：Down 不是终点——下次 `ensure_connected`（工具调用前 / 全量重载）
//!   会重新拉起进程；连接失败把错误写进槽位 + 审计，绝不 panic / 不阻塞其他服务器；
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
use rmcp::transport::TokioChildProcess;
use tauri::{Emitter, Runtime};
use tokio::process::Command;

use super::config::{McpServerConfig, McpTransport, STDIO_LAUNCH_ALLOWLIST};

/// 建立连接（spawn 子进程 + initialize 握手）的总超时
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// 已连接客户端服务句柄。`into_dyn()` 后可作集合元素（rmcp 官方推荐存集合形态）。
type ClientService = RunningService<RoleClient, Box<dyn DynService<RoleClient>>>;

/// 单服务器连接槽：Connected（缓存 tools 快照）/ Down（最近失败信息）。
/// fingerprint = 配置指纹：重载时同指纹跳过重连，配置变了才断开重建。
/// `name` 缓存自配置（连接槽唯一无 AppHandle 也能拿到的东西）：
/// 阶段 3 工具挂载的 `mcp_{server}_{tool}` 前缀来源。
enum Slot {
    Connected {
        fingerprint: u64,
        name: String,
        tools: Vec<Tool>,
        service: Arc<ClientService>,
    },
    Down {
        error: String,
    },
}

/// 进程级单例：模型循环 / registry 挂载层拿不到 AppHandle，
/// manager 用 OnceLock 全局化（连/断/状态命令全部经 `shared()` 访问）。
/// 测试可自建局部实例（e2e 模块），互不干扰。
static MANAGER: std::sync::OnceLock<McpManager> = std::sync::OnceLock::new();

pub fn shared() -> &'static McpManager {
    MANAGER.get_or_init(|| McpManager {
        slots: Mutex::new(HashMap::new()),
        stderr_tail: Mutex::new(HashMap::new()),
    })
}

/// 状态快照条目（`mcp_status` 命令 / 阶段 5 UI 消费）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSlotStatus {
    /// "connected" | "down" | "absent"（absent = 进程内无槽位：未启用或还没轮到）
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
}

/// 环形缓冲容量（行）
const STDERR_TAIL_LINES: usize = 20;

/// 单服务器工具快照上限：外部服务器可返回
/// 任意多工具，无上限会经挂载层每轮全量拼进 tools JSON 撑爆请求与上下文。
/// 超限截断连接快照（进程内无 AppHandle，不另发审计——`mcp.connected` 的
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

/// 串行化触碰进程级 `shared()` 的测试：cargo test 默认多线程并行，
/// e2e 用例的连接存活窗口会让 registry 的
/// `tools_json_with_mcp_without_connections_is_borrowed_static`（断言全局无连接）
/// flaky，共享槽位残留也会跨用例污染。registry 侧断言拿同一把锁（sync 测试用
/// `blocking_lock`，须在 block_on 之前取）。tokio Mutex：守卫 Send，e2e 持锁
/// 跨 await 不触发 clippy await_holding_lock（评审 M-15）。
#[cfg(test)]
pub(crate) static SHARED_MCP_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 配置指纹：任何影响连接/行为的字段变化都会换指纹（id 变化 = 换条目，天然换槽）。
pub fn fingerprint(cfg: &McpServerConfig) -> u64 {
    let value = serde_json::to_value(cfg).unwrap_or(serde_json::Value::Null);
    let mut h = DefaultHasher::new();
    value.to_string().hash(&mut h);
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
            Ok((service, tools)) => {
                self.slots().insert(
                    cfg.id.clone(),
                    Slot::Connected {
                        fingerprint: fp,
                        name: cfg.name.clone(),
                        tools,
                        service,
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
        Self::close_old(old).await;
    }

    async fn close_old(slot: Option<Slot>) {
        if let Some(Slot::Connected { service, .. }) = slot {
            service.cancellation_token().cancel();
        }
    }

    /// 拉起传输 + initialize 握手 + 首次工具发现。
    /// 程序名白名单校验在 config::validate_server（保存口）+ 此处双重把关。
    /// stderr 管道化进环形缓冲（阶段 6）：子进程的诊断输出不再丢失。
    async fn spawn_service(
        &self,
        cfg: &McpServerConfig,
    ) -> Result<(Arc<ClientService>, Vec<Tool>), String> {
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
                    .stdout(std::process::Stdio::piped());
                // Windows 桌面应用不弹控制台窗（与 bot_py Python 子进程同惯例）
                #[cfg(windows)]
                {
                    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                    cmd.creation_flags(CREATE_NO_WINDOW);
                }
                let (transport, stderr) = TokioChildProcess::builder(cmd)
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .map_err(|e| format!("启动命令「{command}」失败：{e}"))?;
                // stderr 后台收割：行进环形缓冲，进程退出时流关闭、任务自然结束
                if let Some(stderr) = stderr {
                    let tail = self.stderr_ring(&cfg.id);
                    tauri::async_runtime::spawn(async move {
                        use tokio::io::{AsyncBufReadExt, BufReader};
                        let reader = BufReader::new(stderr);
                        let mut lines = reader.lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            // 行级字节帽：子进程刷超长行时 BufReader::lines 会整行
                            // 分配，4k 字符截断只影响尾部诊断展示不影响判定
                            let line = if line.len() > 4000 {
                                let mut t: String = line.chars().take(4000).collect();
                                t.push('…');
                                t
                            } else {
                                line
                            };
                            let mut ring = tail.lock().unwrap_or_else(|e| e.into_inner());
                            if ring.len() >= STDERR_TAIL_LINES {
                                ring.pop_front();
                            }
                            ring.push_back(line);
                        }
                    });
                }
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
                Ok((Arc::new(service), tools))
            }
            // Streamable HTTP（阶段 7/8）：发请求前再过一次 URL 安全闸
            //（保存口 validate_server 已把关，此处防手改配置文件绕过）
            McpTransport::Http => {
                let url = cfg.url.as_deref().unwrap_or("").trim().to_string();
                if let Err(reason) = super::config::http_url_is_public(&url) {
                    return Err(format!("URL 安全闸：{reason}"));
                }
                use rmcp::transport::streamable_http_client::{
                    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
                };
                // 自定义头（鉴权头等）挂 reqwest 默认头：随每个请求发送
                let mut client = reqwest::Client::builder();
                if !cfg.headers.is_empty() {
                    let mut map = reqwest::header::HeaderMap::new();
                    for (k, v) in &cfg.headers {
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
                    client = client.default_headers(map);
                }
                let client = client
                    // 评审 SSRF 项：禁跟随重定向——公网端点 302 到内网地址
                    // 即穿透「仅公网」闸；Streamable HTTP 不依赖跨主机重定向
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .map_err(|e| format!("HTTP 客户端构建失败：{e}"))?;
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
                Ok((Arc::new(service), tools))
            }
        }
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

    /// 按配置全量对齐（启动 / 配置变更后调用）：启用且有槽同指纹 → 跳过；
    /// 启用无槽 / 指纹变了 → 连接；禁用 / 已删除但有槽 → 断开。
    /// 单台失败只记审计，不影响其他服务器（失败隔离）。
    /// 具体形态 `AppHandle`（= Wry）：load_config 是具体签名，本方法仅生产路径调用。
    pub async fn reload_from_config(&self, app: &tauri::AppHandle) {
        let servers = crate::bot::config::io::load_config(app)
            .mcp_servers
            .unwrap_or_default();
        let mut want: HashMap<&str, &McpServerConfig> = HashMap::new();
        for s in servers.iter().filter(|s| s.enabled) {
            want.insert(s.id.as_str(), s);
        }
        // 断开：槽位里有、但配置里不再启用（或整体不存在）的 id
        let stale: Vec<String> = self
            .slots()
            .keys()
            .filter(|id| !want.contains_key(id.as_str()))
            .cloned()
            .collect();
        for id in stale {
            self.disconnect(&id).await;
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Info,
                "mcp.disconnected",
                &[("id", id.clone())],
            );
        }
        // 连接：新增 / 指纹变化 / Down 重试（ensure_connected 内部幂等）
        for (id, cfg) in want {
            match self.ensure_connected(cfg).await {
                Ok(()) => {
                    let n = self.status_of(id).tool_count.to_string();
                    crate::audit::write_event(
                        app,
                        crate::audit::AuditLevel::Info,
                        "mcp.connected",
                        &[
                            ("id", id.to_string()),
                            ("name", cfg.name.clone()),
                            ("transport", cfg.transport_kind().as_str().to_string()),
                            ("tools", n),
                        ],
                    );
                }
                Err(e) => {
                    crate::audit::write_event(
                        app,
                        crate::audit::AuditLevel::Warn,
                        "mcp.connect_failed",
                        &[
                            ("id", id.to_string()),
                            ("name", cfg.name.clone()),
                            ("err", e),
                        ],
                    );
                }
            }
        }
        let _ = app.emit("mcp-status-changed", ());
    }

    /// 已连接服务器的缓存工具快照（连接成功时的 list_all_tools 结果；
    /// 未连接 / 无槽 → 空）。阶段 3 schema 注入与设置页「查看工具」共用。
    pub fn tools_of(&self, id: &str) -> Vec<Tool> {
        match self.slots().get(id) {
            Some(Slot::Connected { tools, .. }) => tools.clone(),
            _ => Vec::new(),
        }
    }

    /// 已连接服务器的挂载视图（阶段 3 schema 注入 + 阶段 4 调用反查）：
    /// (服务器 id, 服务器名, tools)。名字来自连接槽缓存（ensure_connected 时带出）。
    /// 按 id 排序输出（评审）：HashMap 迭代序非确定，而不确定性会让
    /// 撞名消歧（mount::build_mounted_tools 按「构建序」分配 `_2`）跨重启/
    /// 重连翻转——同名 function 静默换服务器路由。排序后构建序恒定。
    pub fn mounted(&self) -> Vec<(String, String, Vec<Tool>)> {
        let mut out: Vec<(String, String, Vec<Tool>)> = self
            .slots()
            .iter()
            .filter_map(|(id, slot)| match slot {
                Slot::Connected { name, tools, .. } if !tools.is_empty() => {
                    Some((id.clone(), name.clone(), tools.clone()))
                }
                _ => None,
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// 单服务器槽位状态快照（mcp_status 命令）
    pub fn status_of(&self, id: &str) -> McpSlotStatus {
        match self.slots().get(id) {
            Some(Slot::Connected { tools, .. }) => McpSlotStatus {
                state: "connected".into(),
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

    /// 调用外部工具（阶段 4 dispatch 消费）。入口先 ensure_connected（懒重连：
    /// Down / 配置已变都会在这里重建），再带超时调用。
    /// 连接级失败（TransportClosed/Send）→ 槽位转 Down，返回明确错误文案，
    /// 下次调用自动重连；**同一调用不原地重试**（外部工具可能有副作用，防双执行）。
    pub async fn call_tool(
        &self,
        cfg: &McpServerConfig,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<CallToolResult, String> {
        self.ensure_connected(cfg).await?;
        let service = self.connected_service(&cfg.id, fingerprint(cfg))?;
        let mut params = CallToolRequestParams::new(tool.to_string());
        params.arguments = args.as_object().cloned();
        let timeout = Duration::from_secs(cfg.effective_timeout_secs());
        match tokio::time::timeout(timeout, service.call_tool_once(params)).await {
            Err(_) => Err(format!("工具调用超时（{}s）：{tool}", timeout.as_secs())),
            Ok(Err(rmcp::ServiceError::TransportClosed)) => {
                self.mark_down(&cfg.id, "连接已断开".into());
                Err(self.format_with_stderr_tail(
                    &cfg.id,
                    "MCP 服务器连接已断开（已标记，下次调用自动重连）".into(),
                ))
            }
            Ok(Err(e @ rmcp::ServiceError::TransportSend(_))) => {
                self.mark_down(&cfg.id, e.to_string());
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

    fn mark_down(&self, id: &str, error: String) {
        self.slots().insert(id.to_string(), Slot::Down { error });
    }

    /// 退出清理：取消全部连接令牌并清空槽位。子进程 kill 由
    /// TokioChildProcess Drop 兜底（作用域结束时 Arc 落地触发）。
    pub async fn shutdown(&self) {
        let slots: Vec<(String, Slot)> = self.slots().drain().collect();
        for (id, slot) in slots {
            // stderr 尾巴一并清（ 评审：per-id 路径 disconnect/ensure_connected
            // 都 prune，批量 shutdown 漏了会留残留诊断，下个同 id 服务器错配）
            self.prune_stderr(&id);
            Self::close_old(Some(slot)).await;
        }
    }
}

/// 退出清理的同步外壳（cleanup_on_exit 是同步上下文）：清进程级单例全部连接
pub fn shutdown_all<R: Runtime>(_app: &tauri::AppHandle<R>) {
    tauri::async_runtime::block_on(async move { shared().shutdown().await });
}

#[cfg(test)]
mod send_probe {
    //! 编译探针：McpManager 进 tauri 托管状态 + spawn 的 future 必须 Send/Sync。
    //! rmcp 类型若失去 Send/Sync（升级换版常见回归），在这里响亮失败。
    use super::*;

    #[test]
    fn manager_and_service_are_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<McpManager>();
        assert_sync::<McpManager>();
        type S = RunningService<RoleClient, Box<dyn DynService<RoleClient>>>;
        assert_send::<S>();
        assert_sync::<S>();
    }

    ///  评审（HIGH）：spawn_service 的逐臂字面量 match 与白名单是两份清单，
    /// 新增白名单项漏改 match 会「contains 过、match 落 _ 臂」静默断 stdio 路。
    /// 这里锁 parity：表内每项必有构造臂、臂返回的字面量必须等于命令本身。
    /// match 臂保留字面量是有意的（Mimosa 闸：无「变量 → 进程名」数据流）。
    #[test]
    fn stdio_allowlist_and_spawn_match_stay_in_sync() {
        for cmd in STDIO_LAUNCH_ALLOWLIST {
            let arm = match *cmd {
                "npx" => "npx",
                "bunx" => "bunx",
                "uvx" => "uvx",
                "pipx" => "pipx",
                "node" => "node",
                "deno" => "deno",
                "python" => "python",
                "python3" => "python3",
                "docker" => "docker",
                "podman" => "podman",
                other => panic!("白名单项「{other}」缺少 spawn_service 构造臂（双清单漂移）"),
            };
            assert_eq!(arm, *cmd, "构造臂字面量必须等于白名单项本身");
        }
    }
}

#[cfg(test)]
mod e2e_stdio {
    //! 真实子进程端到端：tests/fixtures/mcp_echo_server.py（零依赖手写协议桩）
    //! 走完 spawn → initialize → tools/list → tools/call → 懒重连 → shutdown。
    use super::*;
    use crate::bot::mcp::config::McpServerConfig;

    fn echo_cfg(id: &str) -> McpServerConfig {
        McpServerConfig {
            id: id.into(),
            name: "echo".into(),
            transport: "stdio".into(),
            command: Some("python3".into()),
            args: vec![concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/mcp_echo_server.py"
            )
            .into()],
            env: Default::default(),
            url: None,
            headers: Default::default(),
            timeout_secs: Some(30),
            enabled: true,
        }
    }

    async fn ensure_python3() -> bool {
        // 无 python3 的环境软跳过（不失败）：spawn 一次 `python3 --version` 探测
        tokio::process::Command::new("python3")
            .arg("--version")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[tokio::test]
    async fn connect_discover_call_roundtrip() {
        if !ensure_python3().await {
            eprintln!("[skip] 无 python3，跳过 MCP stdio e2e");
            return;
        }
        // 与 registry 的「无连接」断言互斥（连接存活窗口会让它 flaky），
        // 并先清一次残留（前序用例 panic 时槽位可能未清）
        let _serial = SHARED_MCP_TEST_LOCK.lock().await;
        shared().shutdown().await;
        // 走进程级单例（与生产同路径）：阶段 3 挂载层读的就是 shared()
        let mgr = shared();
        let cfg = echo_cfg("e2e-conn");
        mgr.ensure_connected(&cfg)
            .await
            .expect("连接 echo 服务器应成功");
        // 工具发现
        let tools = mgr.tools_of("e2e-conn");
        assert_eq!(tools.len(), 1, "应发现 1 个工具");
        assert_eq!(tools[0].name, "echo");
        assert_eq!(mgr.status_of("e2e-conn").state, "connected");
        // 阶段 3：挂载进主 agent 工具清单 + dispatch 反查
        let with = crate::bot::registry::tools_json_with_mcp(None);
        assert!(
            with.contains("\"mcp_echo_echo\""),
            "主 agent 清单应含 mcp_echo_echo"
        );
        let mounted = crate::bot::mcp::mount::find_mounted("mcp_echo_echo").expect("反查应命中");
        assert_eq!(mounted.tool_name, "echo");
        assert_eq!(mounted.server_id, "e2e-conn");
        // 阶段 4：调用内核（dispatch 路由的下游）——真实远端调用 + 结果整形
        let direct =
            crate::bot::mcp::mount::execute_mcp_direct(&cfg, &mounted, r#"{"text": "路由测试"}"#)
                .await;
        assert_eq!(direct.status, crate::bot::registry::ToolStatus::Ok);
        assert_eq!(direct.text, "echo: 路由测试");
        // 非法参数 JSON：不再透传 Null 到远端（ 评审）——本地明确回 warn
        // 文本「参数非法」，模型拿着错误自己重发，省一轮下游类型困惑
        let direct_bad =
            crate::bot::mcp::mount::execute_mcp_direct(&cfg, &mounted, "not-json").await;
        assert_eq!(direct_bad.status, crate::bot::registry::ToolStatus::Warn);
        assert!(
            direct_bad.text.contains("不是合法 JSON"),
            "应本地拒非法参数而非透传远端：{}",
            direct_bad.text
        );
        // 模型编造的工具名：挂载表反查不命中（execute_mcp_tool 会回「未知工具」Error）
        assert!(
            crate::bot::mcp::mount::find_mounted("mcp_echo_nope").is_none(),
            "挂载表外名字不应命中"
        );
        // 阶段 6：stderr 管道捕获——桩启动即写一行 stderr，轮询等待环形缓冲收进
        let mut stderr_seen = false;
        for _ in 0..20 {
            if mgr
                .stderr_tail_snapshot("e2e-conn", 5)
                .iter()
                .any(|l| l.contains("echo-test stderr ready"))
            {
                stderr_seen = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert!(stderr_seen, "stderr 环形缓冲应捕获桩的启动输出");
        // 工具调用
        let result = mgr
            .call_tool(&cfg, "echo", serde_json::json!({ "text": "你好" }))
            .await
            .expect("echo 调用应成功");
        assert_ne!(result.is_error, Some(true), "不应是错误结果");
        let text = result
            .content
            .iter()
            .find_map(|c| c.as_text().map(|t| t.text.clone()))
            .unwrap_or_default();
        assert_eq!(text, "echo: 你好");
        mgr.shutdown().await;
        assert_eq!(mgr.status_of("e2e-conn").state, "absent", "shutdown 清槽");
        // 摘除后立即不可见（验收标准：删除/禁用后工具立即不可见）
        assert!(
            !crate::bot::registry::tools_json_with_mcp(None).contains("mcp_echo_echo"),
            "断开后工具应立即从清单消失"
        );
    }

    #[tokio::test]
    async fn missing_binary_marks_down_and_lazy_reconnects() {
        if !ensure_python3().await {
            eprintln!("[skip] 无 python3，跳过 MCP stdio e2e");
            return;
        }
        let mgr = McpManager::default();
        // 白名单内但参数指向不存在的脚本 → spawn/握手失败 → Down
        let mut bad = echo_cfg("e2e-bad");
        bad.args = vec!["/nonexistent/nope.py".into()];
        let err = mgr.ensure_connected(&bad).await.expect_err("应失败");
        assert!(err.contains("失败") || err.contains("超时"), "实际：{err}");
        assert_eq!(mgr.status_of("e2e-bad").state, "down");
        assert!(mgr.status_of("e2e-bad").error.is_some());
        // 懒重连：换成正确配置后再次 ensure_connected 即恢复（无需手动 disconnect）
        let good = echo_cfg("e2e-bad");
        mgr.ensure_connected(&good).await.expect("重建应成功");
        assert_eq!(mgr.status_of("e2e-bad").state, "connected");
        mgr.shutdown().await;
    }

    #[tokio::test]
    async fn http_branch_url_gate_blocks_before_any_request() {
        // URL 安全闸在连接口：localhost 手改配置文件也进不来（阶段 7 验收）
        let mgr = McpManager::default();
        let mut cfg = echo_cfg("e2e-http-gate");
        cfg.transport = "http".into();
        cfg.command = None;
        cfg.args = Vec::new();
        cfg.url = Some("http://127.0.0.1:9999/mcp".into());
        let err = mgr
            .ensure_connected(&cfg)
            .await
            .expect_err("应被安全闸拦下");
        assert!(err.contains("URL 安全闸"), "实际：{err}");
        assert_eq!(mgr.status_of("e2e-http-gate").state, "down");
    }

    #[tokio::test]
    async fn http_branch_connect_error_maps_to_clean_message() {
        // HTTP 传输错误路径冒烟：.invalid TLD DNS 快速失败 → 错误文案映射。
        //（公网连通性不作单测依赖；Streamable HTTP 的 serve/discover/call 机制
        // 与 stdio 共用，已由 echo 桩 e2e 覆盖全链路。）
        let mgr = McpManager::default();
        let mut cfg = echo_cfg("e2e-http-dns");
        cfg.transport = "http".into();
        cfg.command = None;
        cfg.args = Vec::new();
        cfg.url = Some("https://mcp-nonexistent.invalid/mcp".into());
        cfg.timeout_secs = Some(crate::bot::mcp::config::MCP_TIMEOUT_MIN_SECS);
        let err = mgr.ensure_connected(&cfg).await.expect_err("应连接失败");
        assert!(
            err.contains("失败") || err.contains("超时"),
            "错误应可读：{err}"
        );
        assert_eq!(mgr.status_of("e2e-http-dns").state, "down");
    }
}
