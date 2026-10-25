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
//! logs 日志与输出上限），这里只留类型、常量、use 与逐项转出，
//! `crate::bot::services::X` 旧路径与旧可见性保持不变。

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
        // 停止的**真实结果**也要进文案：停不掉就不能说「已停止」。
        let mut stop_note = String::from("已停止");
        let snapshot = resolve_ready_failure(&entry, || match stop_entry(app, &entry, reason) {
            Ok(o) if o.stopped => {}
            Ok(o) if o.method == "already_exited" => stop_note = "已退出".to_string(),
            Ok(o) => {
                stop_note = format!("停止未确认（method={}，进程可能仍在运行）", o.method);
            }
            Err(e) => {
                stop_note = format!("停止失败：{e}（进程仍在运行）");
                // 停不掉也要看得见：审计里有 service.stop_failed，这里再留一行诊断
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
            &stop_note,
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
    /// 等超时了（是我们停的）——`stop_note` 说的是停止的**真实结果**
    TimedOut {
        timeout_secs: u64,
        stop_note: String,
    },
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
    stop_note: &str,
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
        _ => ReadyFailure::TimedOut {
            timeout_secs,
            stop_note: stop_note.to_string(),
        },
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
        ReadyFailure::TimedOut {
            timeout_secs,
            stop_note,
        } => format!("服务「{command}」在 {timeout_secs}s 内未就绪，{stop_note}。{detail}"),
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

/// 巡检一轮：输出上限 + 资源观测。
fn sweep_round<R: tauri::Runtime>(app: &AppHandle<R>) {
    enforce_output_caps(app);
    sample_resources(app);
    // P2-③：监听采样与判定（发现违规 → 杀进程组 + 记审计 + 面板标红）
    enforce_listening_gate(app);
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

/// 本平台能不能采到进程资源占用（Windows 不能：快照层走 ps / /proc）。
///
/// 与 MCP 面板同一份判据（`procs::snapshot_supported`）。
fn snapshot_supported() -> bool {
    crate::bot::procs::snapshot_supported()
}

/// 合计 RSS 告警该不该触发（纯函数，供测试钉住）。
///
/// **采不到时返回 None——不是「没超」**：本平台没有进程快照层，合计恒为 0，
/// 拿 0 去比阈值等于把「没判」写成「判了没事」，阈值告警就永远不响。
fn total_rss_warning(total_rss: u64, snapshot_supported: bool) -> Option<bool> {
    if !snapshot_supported {
        return None;
    }
    Some(total_rss >= RSS_TOTAL_WARN_BYTES)
}

/// 合计内存能不能给（纯函数，供测试钉住）。
///
/// 采不到就不给——给 0 等于把「没判」说成「没占用」：面板写「合计内存 0 MB」
/// 会让用户以为这些服务不占内存，而真相是「这台机器采不到」。
fn total_rss_mb(total_rss_bytes: u64, snapshot_supported: bool) -> Option<f64> {
    if !snapshot_supported {
        return None;
    }
    Some(round1(total_rss_bytes as f64 / 1024.0 / 1024.0))
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
    let can_sample = snapshot_supported();
    serde_json::json!({
        "services": arr,
        // 合计内存：采不到时给 null——「没判」不是「没占用」，面板据此
        // 不显示「合计内存 0 MB」，改说「本平台采不到」。
        "totalRssMb": total_rss_mb(u.total_rss_bytes, can_sample),
        // 本平台能不能采到进程资源占用（Windows 不能）。面板据此说明
        // 「本平台暂不支持进程资源采集」，而不是让用户对着空白猜。
        "snapshotSupported": can_sample,
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

// 拆出后的子模块转出：保持 `crate::bot::services::X` 旧路径与旧可见性不变
// （pub 的转成 pub、pub(crate) 的转成 pub(crate)、模块私有的仍是私有）
pub(crate) use gate::precheck_listen;
pub use gate::{GateVerdict, Violation, ViolationAction, ViolationRule};
pub use logs::start_output_sweeper;
pub(crate) use observe::ServicesUsage;

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
mod tests;
