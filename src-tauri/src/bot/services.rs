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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::AppHandle;

use crate::audit::AuditLevel;
use crate::bot::mcp::config::STDIO_LAUNCH_ALLOWLIST;
use crate::bot::procs::{take_snapshot, tree_sample, ResourceSample};

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
    // 占用率 = 两次采样之间的 CPU 时间增量 ÷ 墙上时间增量（×100 得百分比）
    let cpu_pct = match last_cpu_sample {
        Some((prev_cpu, prev_at)) if now_ms > prev_at => {
            let wall_secs = (now_ms - prev_at) as f64 / 1000.0;
            Some((sample.cpu_seconds - prev_cpu) / wall_secs * 100.0)
        }
        _ => None,
    };

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
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
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
            let exit_code = guard
                .exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "none".to_string());
            let uptime = guard.uptime_ms();
            drop(guard);
            crate::audit::write_event(
                &app,
                AuditLevel::Info,
                "service.exited",
                &[
                    ("service_id", watch_id.clone()),
                    ("pid", pid.to_string()),
                    ("exit_code", exit_code),
                    ("uptime_ms", uptime.to_string()),
                ],
            );
        });
    }

    // 就绪判定
    let ready = match &req.ready_pattern {
        Some(pattern) => wait_ready_pattern(app, &entry, pattern, req.ready_timeout_secs, &id),
        None => {
            std::thread::sleep(Duration::from_millis(LIVENESS_CONFIRM_MS));
            let alive = entry
                .lock()
                .map(|s| s.state == ServiceState::Starting)
                .unwrap_or(false);
            alive
        }
    };

    if !ready {
        // 就绪超时 / 提前退出：主动收掉这个服务，别留半死不活的东西
        let reason = "ready_timeout";
        let _ = stop_entry(app, &entry, reason);
        let stderr_tail = tail_lines(&stderr_path, TAIL_LINES);
        let stdout_tail = tail_lines(&stdout_path, TAIL_LINES);
        let mut detail = String::new();
        if !stdout_tail.is_empty() {
            detail.push_str("stdout: ");
            detail.push_str(&stdout_tail.join(" | "));
        }
        if !stderr_tail.is_empty() {
            if !detail.is_empty() {
                detail.push_str("；");
            }
            detail.push_str("stderr: ");
            detail.push_str(&stderr_tail.join(" | "));
        }
        let exited_early = entry
            .lock()
            .map(|s| s.state == ServiceState::Exited)
            .unwrap_or(false);
        let msg = if exited_early {
            format!(
                "服务「{}」启动后立即退出（{}）。{}",
                req.command,
                entry
                    .lock()
                    .map(|s| s
                        .exit_code
                        .map(|c| format!("退出码 {c}"))
                        .unwrap_or_default())
                    .unwrap_or_default(),
                detail
            )
        } else {
            format!(
                "服务「{}」在 {}s 内未就绪，已停止。{}",
                req.command, req.ready_timeout_secs, detail
            )
        };
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

/// 就绪等待：stdout 出现匹配正则即算就绪。
fn wait_ready_pattern<R: tauri::Runtime>(
    app: &AppHandle<R>,
    entry: &ServiceEntry,
    pattern: &str,
    timeout_secs: u64,
    id: &str,
) -> bool {
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
            return false;
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
            return true;
        }
        let state = entry
            .lock()
            .map(|s| s.state)
            .unwrap_or(ServiceState::Failed);
        if state == ServiceState::Exited || state == ServiceState::Failed {
            return false;
        }
        if Instant::now() >= deadline {
            return false;
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
    /// 实际生效的手段（`SIGINT` / `SIGTERM` / `SIGKILL` / `already_exited`）
    pub(crate) method: String,
    pub(crate) descendants_killed: usize,
    pub(crate) exit_code: Option<i32>,
    pub(crate) elapsed_ms: u64,
}

/// 停止一个服务：降级信号阶梯 + 脱离进程组的子孙回收。
///
/// 阻塞：含等待窗口，必须挪到 blocking 线程。
pub(crate) fn stop_entry<R: tauri::Runtime>(
    app: &AppHandle<R>,
    entry: &ServiceEntry,
    reason: &str,
) -> Result<StopOutcome, String> {
    let started = Instant::now();
    let (pid, pgid, stdout_path, stderr_path, id) = {
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
        g.state = ServiceState::Stopping;
        g.stop_reason = Some(reason.to_string());
        (
            g.pid,
            g.pgid,
            g.stdout_path.clone(),
            g.stderr_path.clone(),
            g.id.clone(),
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
    let tree = crate::bot::procs::stop_tree(pid, pgid, |grace| wait_state_exit(entry, grace));
    let method = tree.method;
    let killed = tree.descendants_killed;

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
    wait_state_exit(entry, Duration::from_millis(1000));

    let (exit_code, state) = {
        let g = entry.lock().unwrap_or_else(|e| e.into_inner());
        (g.exit_code, g.state)
    };
    if state != ServiceState::Exited {
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
        let verdict = evaluate_resources(
            sample,
            g.last_cpu_sample,
            g.cpu_busy_since_ms,
            &g.warnings,
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
        match stop_entry(app, &entry, "app_exit") {
            Ok(o) => {
                if o.stopped {
                    stopped += 1;
                }
            }
            Err(e) => {
                crate::audit::write_event(
                    app,
                    AuditLevel::Warn,
                    "service.stop_failed",
                    &[("reason", e)],
                );
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
