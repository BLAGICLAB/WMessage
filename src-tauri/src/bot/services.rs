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
//!
//! 本文件是门面：实现按面拆到 `services/` 子模块（gate 端口闸 / observe 资源观测 /
//! logs 日志与输出上限 / start 启动 / stop 停止 / serialize 序列化），这里只留
//! 类型、常量、use 与逐项转出，`crate::bot::services::X` 旧路径与旧可见性保持不变。

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

mod gate;
mod logs;
mod observe;
mod serialize;
mod start;
mod stop;

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

/// 巡检一轮：输出上限 + 资源观测。
fn sweep_round<R: tauri::Runtime>(app: &AppHandle<R>) {
    enforce_output_caps(app);
    sample_resources(app);
    // P2-③：监听采样与判定（发现违规 → 杀进程组 + 记审计 + 面板标红）
    enforce_listening_gate(app);
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
/// flag 走 0600（与 UI 自动化开关同款）：0644 时同机其它进程能直接创建这个
/// 文件把「允许机器人启动本地服务」的用户同意顶开。只吞 NotFound：权限 /
/// 占用类失败必须上抛——盘上残留 flag 时 `service_get_enabled` 仍返 true，
/// 与刚返回给调用方的「已禁用」自相矛盾。开关变更必须留痕。
pub fn set_enabled<R: tauri::Runtime>(app: &AppHandle<R>, on: bool) -> Result<bool, String> {
    set_flag_with_audit(app, &services_flag_path(app), on, "service.enable_toggle")
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
///
/// L1 暴露闸的放开方式，与启用开关同款 0600 + 审计——这个 flag 一旦被
/// 同机进程静默创建，受管服务就能绑 0.0.0.0 暴露到局域网，事后必须查得到
/// 「是谁在什么时候放开的」。
pub fn set_expose_net_allowed<R: tauri::Runtime>(
    app: &AppHandle<R>,
    on: bool,
) -> Result<bool, String> {
    set_flag_with_audit(
        app,
        &expose_net_flag_path(app),
        on,
        "service.expose_net_toggle",
    )
}

/// 服务权限 flag 的统一写入口：0600 写 / 删 + 变更审计（带 enabled 与 os 用户）。
/// 盘上已有宽权限旧 flag 时，write_flag_file 会顺手收紧成 0600。
fn set_flag_with_audit<R: tauri::Runtime>(
    app: &AppHandle<R>,
    path: &PathBuf,
    on: bool,
    event: &'static str,
) -> Result<bool, String> {
    let before = path.exists();
    if on {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败：{e}"))?;
        }
        crate::ui::write_flag_file(path).map_err(|e| format!("写入开关失败：{e}"))?;
    } else {
        match std::fs::remove_file(path) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("删除开关失败：{e}")),
        }
    }
    let after = path.exists();
    if before != after {
        crate::audit::write_event(
            app,
            crate::audit::AuditLevel::Info,
            event,
            &[
                ("enabled", after.to_string()),
                ("user", crate::ui::os_user()),
            ],
        );
    }
    Ok(after)
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

// 拆出后的子模块转出：保持 `crate::bot::services::X` 旧路径与旧可见性不变
// （pub 的转成 pub、pub(crate) 的转成 pub(crate)、模块私有的仍是私有）
pub(crate) use gate::precheck_listen;
pub use gate::{GateVerdict, Violation, ViolationAction, ViolationRule};
pub use logs::start_output_sweeper;
pub(crate) use observe::ServicesUsage;

// 新拆出的三块：启动 / 停止 / 序列化。转出后 `crate::bot::services::X`
// 旧路径与旧可见性不变（原先模块私有的仍是模块私有）。
pub(crate) use serialize::{service_to_json, services_snapshot_json};
pub(crate) use start::do_start;
pub(crate) use stop::{stop_all, stop_entry, stop_service};

// 跨模块共用项：门面转出后由 services/ 各子模块经 `use super::*` 取用
use gate::{
    enforce_listening_gate, reserved_ports, spawn_startup_listening_watch, violation_message,
    LAST_SAMPLE_OK,
};
use logs::{
    enforce_output_caps, make_service_dir, open_append, read_tail_bytes, services_root, tail_lines,
    truncate_stream,
};
use observe::sample_resources;
use serialize::{snapshot_supported, total_rss_warning};

// 仅测试用到的内部项：转出后由 services/tests.rs 经 `use super::*` 取用
#[cfg(test)]
use gate::{
    api_canary_port, canary_blind, is_loopback, judge_flags, judge_sockets, note_sample_status,
    parse_listen_flags, refresh_reserved_ports, FlagListen,
};
#[cfg(test)]
use logs::truncate_output_file;
#[cfg(test)]
use observe::{evaluate_resources, write_resource_warning};
#[cfg(test)]
use serialize::total_rss_mb;
#[cfg(test)]
use start::{
    build_command, classify_ready_failure, exited_event_kv, head_chars, ready_failure_message,
    resolve_ready_failure, tail_chars, ReadyFailureSnapshot, ReadyOutcome,
};

#[cfg(test)]
mod tests;
