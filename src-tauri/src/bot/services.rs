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

// ── 常量 ──

/// 同时存活的受管服务数上限。超限直接拒绝启动。
pub(crate) const MAX_CONCURRENT_SERVICES: usize = 4;

/// 单流（stdout / stderr 各算）日志文件上限。
pub(crate) const OUTPUT_CAP_BYTES: u64 = 8 * 1024 * 1024;

/// 就绪瞬间固化的 stdout 尾部字节数。日志会被截断，启动横幅 / 监听地址不能跟着丢。
pub(crate) const READY_SNAPSHOT_BYTES: usize = 2 * 1024;

/// SIGINT 后的优雅退出等待窗口。
pub(crate) const SIGINT_GRACE_MS: u64 = 100;

/// SIGTERM 后的等待窗口。
pub(crate) const SIGTERM_GRACE_MS: u64 = 400;

/// 就绪等待默认上限。
pub(crate) const DEFAULT_READY_TIMEOUT_SECS: u64 = 30;

/// 就绪等待硬上限。
pub(crate) const MAX_READY_TIMEOUT_SECS: u64 = 120;

/// `list_services` 返回的输出尾部行数。
pub(crate) const TAIL_LINES: usize = 20;

/// 输出上限巡检周期（秒）。
const OUTPUT_SWEEP_SECS: u64 = 30;

/// 进程组存活判定的轮询间隔。
const ALIVE_POLL_MS: u64 = 20;

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

// ── 进程快照（纯函数） ──

/// 进程快照的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcSnapshot {
    pub(crate) pid: u32,
    pub(crate) ppid: u32,
    /// 进程启动标识（绝对启动时间）。用来识别 **PID 复用**：同一个 pid 在两次
    /// 快照里启动标识变了，说明这个 pid 已经属于另一个进程——那个进程不是我们的
    /// 后代，**不能杀**。
    pub(crate) starttime: String,
}

/// 一次快照比对的结果。
#[derive(Debug, Default, Clone)]
pub(crate) struct TreeScan {
    /// 本次新出现的后代（可杀）
    pub(crate) added: Vec<(u32, String)>,
    /// 本次发现 PID 复用（pid, 旧启动标识, 新启动标识）——不杀
    pub(crate) reused: Vec<(u32, String, String)>,
}

/// 从快照收集 `root` 的全部后代（不含 root 自身）。
///
/// 纯计算：只读传入的快照，不做任何 IO。返回 pid → 启动标识。
pub(crate) fn descendants(snap: &[ProcSnapshot], root: u32) -> HashMap<u32, String> {
    let mut children: HashMap<u32, Vec<(u32, &str)>> = HashMap::new();
    for p in snap {
        children
            .entry(p.ppid)
            .or_default()
            .push((p.pid, p.starttime.as_str()));
    }
    let mut out: HashMap<u32, String> = HashMap::new();
    let mut stack: Vec<(u32, &str)> = children.get(&root).cloned().unwrap_or_default();
    while let Some((pid, st)) = stack.pop() {
        // 已访问过就跳过：进程表理论上无环，但坏数据下不能死循环
        if out.insert(pid, st.to_string()).is_some() {
            continue;
        }
        if let Some(kids) = children.get(&pid) {
            stack.extend(kids.iter().copied());
        }
    }
    out
}

/// 对比前后两张快照，算出**新增**后代与 **PID 复用**者。
///
/// PID 复用的处置是剔除而非杀：宁可漏杀（那个进程已经不是我们的后代），
/// 也不能误杀（可能杀掉用户正在跑的别的进程）。
pub(crate) fn diff_descendants(
    prev: &HashMap<u32, String>,
    cur: &HashMap<u32, String>,
) -> TreeScan {
    let mut scan = TreeScan::default();
    for (pid, st) in cur {
        match prev.get(pid) {
            None => scan.added.push((*pid, st.clone())),
            Some(old) if old == st => {}
            Some(old) => scan.reused.push((*pid, old.clone(), st.clone())),
        }
    }
    scan
}

/// 拍一张全机进程快照。
///
/// 只读系统进程表，不碰服务登记表——这层将来要整体上提给 MCP 侧复用。
pub(crate) fn take_snapshot() -> Vec<ProcSnapshot> {
    #[cfg(target_os = "linux")]
    {
        snapshot_linux()
    }
    #[cfg(not(target_os = "linux"))]
    {
        snapshot_ps()
    }
}

/// Linux：读 `/proc/<pid>/stat`，starttime 精度到 tick（~10ms）。
#[cfg(target_os = "linux")]
fn snapshot_linux() -> Vec<ProcSnapshot> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return out;
    };
    for entry in dir.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        if let Some(s) = parse_proc_stat(pid, &text) {
            out.push(s);
        }
    }
    out
}

/// `/proc/<pid>/stat` 解析。非 Linux 平台上是主路径，Linux 上仅供测试覆盖同一套解析逻辑。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_proc_stat(pid: u32, text: &str) -> Option<ProcSnapshot> {
    let close = text.rfind(')')?;
    let rest = &text[close + 1..];
    let t: Vec<&str> = rest.split_whitespace().collect();
    if t.len() < 20 {
        return None;
    }
    let ppid = t[1].parse::<u32>().ok()?;
    Some(ProcSnapshot {
        pid,
        ppid,
        starttime: t[19].to_string(),
    })
}

/// macOS 等无 `/proc` 的平台：调 `ps` 取 pid / ppid / lstart。
///
/// `lstart` 是绝对启动时间（"Wed Oct 21 18:15:00 2026"，恒为 5 段），
/// 同一进程在两次快照里必然相同，不同进程必然不同（除非同一秒内启动，
/// 而 pid 复用要在同一秒内完成回收+重分配，实际不可能）。
#[cfg(not(target_os = "linux"))]
fn snapshot_ps() -> Vec<ProcSnapshot> {
    let Ok(out) = Command::new("ps")
        .args(["-o", "pid=", "-o", "ppid=", "-o", "lstart=", "-ax"])
        .output()
    else {
        return Vec::new();
    };
    parse_ps_lstart(&String::from_utf8_lossy(&out.stdout))
}

/// `ps` 输出解析。非 Linux 平台上是主路径，Linux 上仅供测试覆盖同一套解析逻辑。
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) fn parse_ps_lstart(text: &str) -> Vec<ProcSnapshot> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        // pid + ppid + lstart 的 5 段
        if t.len() < 7 {
            continue;
        }
        let (Ok(pid), Ok(ppid)) = (t[0].parse::<u32>(), t[1].parse::<u32>()) else {
            continue;
        };
        out.push(ProcSnapshot {
            pid,
            ppid,
            starttime: t[2..7].join(" "),
        });
    }
    out
}

// ── 信号与存活判定 ──

#[cfg(unix)]
fn send_signal(pid: u32, sig: i32) -> bool {
    // pid 0 必须拒掉：kill(0, sig) 的语义是"给调用方的整个进程组发信号"，
    // 等于宿主给自己发信号。登记表占位条目（还没 spawn 成功）pid 就是 0。
    if pid == 0 {
        return false;
    }
    // SAFETY: kill(2) 对任意 pid 安全；ESRCH/EPERM 都表示"没打上"，
    // 调用方据此判进程已走。信号值为 SIGINT/SIGTERM/SIGKILL 常量。
    unsafe { libc::kill(pid as libc::pid_t, sig) == 0 }
}

#[cfg(unix)]
fn signal_group(pgid: i32, sig: i32) -> bool {
    // killpg(0, sig) 同样是"调用方的整个进程组"——组 id 拿不到时宁可不发。
    if pgid <= 0 {
        return false;
    }
    // SAFETY: killpg(2) 对任意 pgid 安全；pgid 是我们自己 spawn 时用
    // process_group(0) 建的组，不可能是宿主的组。
    unsafe { libc::killpg(pgid as libc::pid_t, sig) == 0 }
}

#[cfg(unix)]
const SIGINT: i32 = libc::SIGINT;
#[cfg(unix)]
const SIGTERM: i32 = libc::SIGTERM;
#[cfg(unix)]
const SIGKILL: i32 = libc::SIGKILL;

#[cfg(not(unix))]
fn send_signal(_pid: u32, _sig: i32) -> bool {
    false
}

#[cfg(not(unix))]
fn signal_group(_pgid: i32, _sig: i32) -> bool {
    false
}

/// 进程是否还存在（`kill(pid, 0)` 探测）。测试用：断言逃逸子孙是否真被回收。
#[cfg(all(test, unix))]
fn pid_exists(pid: u32) -> bool {
    // SAFETY: kill(pid, 0) 只做存在性与权限探测，不发信号。
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(all(test, not(unix)))]
fn pid_exists(_pid: u32) -> bool {
    false
}

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

    // 已知后代：pid → 启动标识。pid 与启动标识一起存，才能在最后核验
    // "这个 pid 现在还是不是当时那个进程"（防 PID 复用误杀）。
    let mut known: HashMap<u32, String> = HashMap::new();
    let mut prev: HashMap<u32, String> = HashMap::new();
    let mut method = "SIGINT".to_string();
    let mut reused_skipped: Vec<(u32, String, String)> = Vec::new();
    // 组长的启动标识：整组杀的护栏要用（见下面的 group_safe）
    let mut leader_starttime: Option<String> = None;

    // 阶梯：每一级发信号前拍一张快照，增量收集，最后统一杀。
    for step in 0..3u32 {
        // 收集（只收集，不杀）
        let snap = take_snapshot();
        if leader_starttime.is_none() {
            leader_starttime = snap
                .iter()
                .find(|p| p.pid == pid)
                .map(|p| p.starttime.clone());
        }
        let cur = descendants(&snap, pid);
        let scan = diff_descendants(&prev, &cur);
        for (p, st) in scan.added {
            known.insert(p, st);
        }
        for (p, old, new) in scan.reused {
            // PID 被复用：这个 pid 现在属于别的进程，不杀；同时从待杀集合剔除
            known.remove(&p);
            reused_skipped.push((p, old, new));
        }
        prev = cur;

        let (sig, grace_ms) = match step {
            0 => (SIGINT, SIGINT_GRACE_MS),
            1 => (SIGTERM, SIGTERM_GRACE_MS),
            _ => (SIGKILL, 0),
        };
        if step == 2 {
            method = "SIGKILL".to_string();
            break;
        }
        if step == 1 {
            method = "SIGTERM".to_string();
        }

        send_signal(pid, sig);
        // 等到进程真的走了再进下一级（登记表状态由 watcher 的 wait() 权威更新）
        if wait_state_exit(entry, Duration::from_millis(grace_ms)) {
            break;
        }
    }

    // 最后一张快照：确认待杀集合里的 pid 仍是当时那个进程，再动手
    let final_snap = take_snapshot();
    let final_map: HashMap<u32, &str> = final_snap
        .iter()
        .map(|p| (p.pid, p.starttime.as_str()))
        .collect();
    let mut to_kill: Vec<u32> = Vec::new();
    for (p, st) in &known {
        match final_map.get(p) {
            Some(cur_st) if *cur_st == st => to_kill.push(*p),
            _ => {
                // 已退出，或 pid 已被复用 → 不杀
                reused_skipped.push((*p, st.clone(), "gone_or_reused".to_string()));
            }
        }
    }

    // 整组强杀 + 逐个补杀
    //
    // 整组杀的护栏：组长被回收后 pid 可能被系统分配给别人，而 pid 同时
    // 是组 id，复用后 killpg 会打到别人的组上。
    //
    // `kill(pid, 0)` 证明不了"还是我们那个"——watcher 一收尸，pid 就被
    // 释放了；这时 pid 仍存在只说明"这个号上有别的进程"。所以要核启动
    // 标识：组长还是同一个进程（标识一致）才允许整组杀。
    //
    // 组长已经走了的情况：组里剩下的成员由 ppid 树补杀覆盖（它们在
    // 快照里是组长的后代），不依赖整组杀。
    let group_safe = group_kill_allowed(&final_map, pid, leader_starttime.as_deref());
    if let (Some(cur), Some(old)) = (final_map.get(&pid), leader_starttime.as_deref()) {
        if *cur != old {
            // 组长这个 pid 已被复用：现在这个号上的不是我们的进程，剔除不杀
            reused_skipped.push((pid, old.to_string(), cur.to_string()));
        }
    }
    // 快照整个是空的（ps 读不到）→ 无从核验，但组长是我们自己 spawn 的，
    // 这时宁可赌一次也不能让服务活着跑（否则 stop 报「已停」而进程还在）。
    let unverifiable = final_snap.is_empty();
    if group_safe || unverifiable {
        signal_group(pgid, SIGKILL);
        // 组长自己：只在确认「还是我们那个进程」时才发最后一下。
        // 组长已优雅退出且 pid 被系统回收再分配给别人时，无条件 kill 会误杀它。
        send_signal(pid, SIGKILL);
    }
    let mut killed = 0usize;
    for p in &to_kill {
        if *p == pid {
            continue; // 主进程已单独处理
        }
        if send_signal(*p, SIGKILL) {
            killed += 1;
        }
    }

    for (p, old, new) in &reused_skipped {
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

/// 启动输出上限巡检：每 30s 一轮（App 启动时调用）。
///
/// 周期取 30s：长跑服务的日志增长是「分钟级」的，巡检太密只是白跑 IO。
/// 单轮失败不杀循环：截断失败只丢日志，不影响服务本身。
pub fn start_output_sweeper<R: tauri::Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(OUTPUT_SWEEP_SECS));
        ticker.tick().await; // 消耗首个立即触发的 tick
        loop {
            ticker.tick().await;
            enforce_output_caps(&app);
        }
    });
}

/// 整组杀是否安全：组长还在、且还是当初那个进程，才允许 `killpg`。
///
/// 纯函数：只吃 pid 空间的数据，不碰登记表。
///
/// 为什么不能只看"pid 还在不在"：watcher 收尸后 pid 就被释放了，
/// 之后这个号可能被分给别的进程；`kill(pid, 0)` 对两种情况都返回成功。
/// 核启动标识才能区分"还是我们的组长"和"号被借走了"。
pub(crate) fn group_kill_allowed(
    final_map: &HashMap<u32, &str>,
    leader_pid: u32,
    leader_starttime: Option<&str>,
) -> bool {
    match (final_map.get(&leader_pid), leader_starttime) {
        (Some(cur), Some(old)) => *cur == old,
        _ => false,
    }
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
        std::thread::sleep(Duration::from_millis(ALIVE_POLL_MS));
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
    if with_tail {
        obj["stdoutTail"] = serde_json::json!(tail_lines(&s.stdout_path, TAIL_LINES));
        obj["stderrTail"] = serde_json::json!(tail_lines(&s.stderr_path, TAIL_LINES));
    }
    obj
}

#[cfg(test)]
mod tests;
