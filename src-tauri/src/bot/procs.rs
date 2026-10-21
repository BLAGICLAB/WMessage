//! 进程树操作：快照、子孙收集、停止阶梯。
//!
//! 两个调用方：
//! - 受管长驻服务层（`bot/services.rs`）：停止自己的长驻进程；
//! - MCP stdio 连接（`bot/mcp/manager.rs`）：退出时收掉长驻的 MCP 子进程。
//!
//! **纯进程操作**：只依赖 pid 空间与系统进程表，不碰任何登记表
//! （服务登记表、MCP 连接槽都不在这里）——所以它能整体被两个上层复用。
//!
//! 三条硬约束（改动本模块时不要破坏）：
//!
//! 1. **PID 复用必须防**。pid 会复用：同一个 pid 在两次快照里启动标识变了，
//!    说明这个 pid 已经属于另一个进程——那个进程不是我们的后代，**不能杀**。
//!    宁可漏杀（那个进程已经不是我们的后代）也不能误杀（可能杀掉用户正在跑的
//!    别的进程）。
//! 2. **停止走降级阶梯**（SIGINT → SIGTERM → SIGKILL），并递归回收**脱离进程组**
//!    的子孙——`kill` 进程组管不到 `setsid` 逃逸者。
//! 3. **整组杀要核验组长身份**。组 id 就是组长的 pid，而组长被回收后 pid 会被
//!    释放并可能分配给别人；那时 `killpg` 会打到别人的组上。

use std::collections::HashMap;
use std::process::Command;
use std::time::{Duration, Instant};

/// SIGINT 后的优雅退出等待窗口。
#[cfg(unix)]
pub(crate) const SIGINT_GRACE_MS: u64 = 100;

/// SIGTERM 后的等待窗口。
#[cfg(unix)]
pub(crate) const SIGTERM_GRACE_MS: u64 = 400;

/// 进程存活判定的轮询间隔。
pub(crate) const ALIVE_POLL_MS: u64 = 20;

/// Windows：等进程自己退出的窗口。
#[cfg(windows)]
const WINDOWS_EXIT_GRACE_MS: u64 = 300;

/// Windows：`taskkill` 之后等进程真正消失的窗口。
#[cfg(windows)]
const WINDOWS_KILL_WAIT_MS: u64 = 1000;

// ── 数据结构 ──

/// 进程快照的一行。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProcSnapshot {
    pub(crate) pid: u32,
    pub(crate) ppid: u32,
    /// 进程启动标识（绝对启动时间）。用来识别 **PID 复用**：同一个 pid 在两次
    /// 快照里启动标识变了，说明这个 pid 已经属于另一个进程——那个进程不是我们的
    /// 后代，**不能杀**。
    pub(crate) starttime: String,
    /// 常驻内存（字节）。资源观测用。
    pub(crate) rss_bytes: u64,
    /// 累计 CPU 时间（秒）。两次采样的差值 ÷ 时间差 = 占用率。
    pub(crate) cpu_seconds: f64,
}

/// 一次快照比对的结果。
#[cfg(unix)]
#[derive(Debug, Default, Clone)]
pub(crate) struct TreeScan {
    /// 本次新出现的后代（可杀）
    pub(crate) added: Vec<(u32, String)>,
    /// 本次发现 PID 复用（pid, 旧启动标识, 新启动标识）——不杀
    pub(crate) reused: Vec<(u32, String, String)>,
}

/// 一个进程树（组长 + 全部后代）的资源采样。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct ResourceSample {
    /// 整棵树的常驻内存合计（字节）
    pub(crate) rss_bytes: u64,
    /// 整棵树的累计 CPU 时间（秒）
    pub(crate) cpu_seconds: f64,
}

/// 监听 socket 的协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SocketProtocol {
    Tcp,
    Udp,
}

impl SocketProtocol {
    /// 面板与审计里用的名字。
    pub fn as_str(self) -> &'static str {
        match self {
            SocketProtocol::Tcp => "TCP",
            SocketProtocol::Udp => "UDP",
        }
    }
}

/// 一个监听 socket：TCP 的 LISTEN 态，或 UDP 已绑定端口。
///
/// 地址是**规范化后的具体地址**（不是 `*`）：通配绑在 IPv4 上记 `0.0.0.0`，
/// IPv6 上记 `::`——判断「是不是 loopback」要靠这个，通配符留着没法判。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ListeningSocket {
    pub protocol: SocketProtocol,
    /// 监听地址（`127.0.0.1` / `::1` / `0.0.0.0` / `::`）
    pub address: String,
    /// 端口
    pub port: u16,
}

// ── 进程快照（纯函数） ──

/// 从快照收集 `root` 的全部后代（不含 root 自身）。
///
/// 纯计算：只读传入的快照，不做任何 IO。返回 pid → 启动标识。
pub(crate) fn descendants(snap: &[ProcSnapshot], root: u32) -> HashMap<u32, String> {
    // pid 0 / 1 不能当树根：
    // - pid 0 是「调用方的进程组」哨兵（`kill(0, sig)` 发给整个进程组）；
    // - pid 1 是 init/launchd——**全机进程都是它的后代**。
    // 以它们为根，收集到的就是「宿主机上所有进程」，随后逐个 SIGKILL = 宿主自杀。
    if root <= 1 {
        return HashMap::new();
    }
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
#[cfg(unix)]
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
/// 只读系统进程表，不碰任何上层登记表——这层将来要整体上提给更多调用方复用。
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
///
/// 字段位置（1-based，pid 与 comm 之后从 state 起算）：
/// 4=ppid、14=utime、15=stime、22=starttime、24=rss（页数）。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_proc_stat(pid: u32, text: &str) -> Option<ProcSnapshot> {
    let close = text.rfind(')')?;
    let rest = &text[close + 1..];
    let t: Vec<&str> = rest.split_whitespace().collect();
    if t.len() < 22 {
        return None;
    }
    let ppid = t[1].parse::<u32>().ok()?;
    let utime: u64 = t[11].parse().ok()?;
    let stime: u64 = t[12].parse().ok()?;
    let rss_pages: u64 = t[21].parse().ok()?;
    Some(ProcSnapshot {
        pid,
        ppid,
        starttime: t[19].to_string(),
        rss_bytes: rss_pages.saturating_mul(page_size()),
        cpu_seconds: (utime + stime) as f64 / clock_ticks(),
    })
}

/// 一页的字节数（Linux 的 rss 以页计）。
#[cfg(unix)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn page_size() -> u64 {
    // SAFETY: sysconf 是纯查询，返回 -1 表示未知，此时按 4096 兜底。
    let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if v > 0 {
        v as u64
    } else {
        4096
    }
}

/// 非 unix 没有 `sysconf`（libc 是 unix-only 依赖）。这个值只在 Linux 的
/// `/proc` 解析路径上生效，别的平台上它只保证纯函数可测——取值取 Linux 常规值。
#[cfg(not(unix))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn page_size() -> u64 {
    4096
}

/// 一秒多少个 clock tick（utime/stime 以 tick 计）。
#[cfg(unix)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn clock_ticks() -> f64 {
    // SAFETY: sysconf 是纯查询，返回 -1 表示未知，此时按 100 兜底（Linux 常规值）。
    let v = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if v > 0 {
        v as f64
    } else {
        100.0
    }
}

/// 非 unix 没有 `sysconf`——同 `page_size`，只保证纯函数可测。
#[cfg(not(unix))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn clock_ticks() -> f64 {
    100.0
}

/// macOS 等无 `/proc` 的平台：调 `ps` 取 pid / ppid / lstart / rss / 累计 CPU 时间。
///
/// `lstart` 是绝对启动时间（"Wed Oct 21 18:15:00 2026"，恒为 5 段），
/// 同一进程在两次快照里必然相同，不同进程必然不同（除非同一秒内启动，
/// 而 pid 复用要在同一秒内完成回收+重分配，实际不可能）。
#[cfg(not(target_os = "linux"))]
fn snapshot_ps() -> Vec<ProcSnapshot> {
    let Ok(out) = Command::new("ps")
        .args([
            "-o", "pid=", "-o", "ppid=", "-o", "lstart=", "-o", "rss=", "-o", "time=", "-ax",
        ])
        .output()
    else {
        return Vec::new();
    };
    parse_ps_rows(&String::from_utf8_lossy(&out.stdout))
}

/// `ps` 输出解析。非 Linux 平台上是主路径，Linux 上仅供测试覆盖同一套解析逻辑。
///
/// 列序：pid / ppid / lstart(5 段) / rss(KB) / time(累计 CPU 时间)。
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) fn parse_ps_rows(text: &str) -> Vec<ProcSnapshot> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        // pid + ppid + lstart 的 5 段 + rss + time
        if t.len() < 9 {
            continue;
        }
        let (Ok(pid), Ok(ppid)) = (t[0].parse::<u32>(), t[1].parse::<u32>()) else {
            continue;
        };
        let rss_kb: u64 = t[7].parse().unwrap_or(0);
        out.push(ProcSnapshot {
            pid,
            ppid,
            starttime: t[2..7].join(" "),
            rss_bytes: rss_kb.saturating_mul(1024),
            cpu_seconds: parse_ps_time(t[8]).unwrap_or(0.0),
        });
    }
    out
}

/// `ps` 的 `time` 列解析：`[[DD-]HH:]MM:SS[.ss]` → 秒。
///
/// 解析不出来就返回 None（当成 0 秒处理，不猜）。
pub(crate) fn parse_ps_time(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // 天数段（形如 `1-02:03:04`）
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<f64>().ok()?, r),
        None => (0.0, s),
    };
    let parts: Vec<&str> = rest.split(':').collect();
    let secs = match parts.len() {
        3 => {
            // HH:MM:SS
            let h: f64 = parts[0].parse().ok()?;
            let m: f64 = parts[1].parse().ok()?;
            let sec: f64 = parts[2].parse().ok()?;
            h * 3600.0 + m * 60.0 + sec
        }
        2 => {
            // MM:SS
            let m: f64 = parts[0].parse().ok()?;
            let sec: f64 = parts[1].parse().ok()?;
            m * 60.0 + sec
        }
        _ => return None,
    };
    Some(days * 86400.0 + secs)
}

/// 两次采样之间的 CPU 占用率（百分比）。
///
/// delta 累计 CPU 时间 ÷ delta 墙上时间。**受管服务层与 MCP 侧同一条公式**——
/// 两处各算一遍迟早会漂。
///
/// 没有前值（第一次采样）或墙上时间没走（同一毫秒）时返回 None。
pub(crate) fn cpu_usage_pct(
    prev_cpu_secs: f64,
    prev_at_ms: i64,
    cur_cpu_secs: f64,
    cur_at_ms: i64,
) -> Option<f64> {
    if cur_at_ms <= prev_at_ms {
        return None;
    }
    let wall_secs = (cur_at_ms - prev_at_ms) as f64 / 1000.0;
    Some((cur_cpu_secs - prev_cpu_secs) / wall_secs * 100.0)
}

/// 把一次进程快照折成某个服务整棵树的资源采样。
///
/// 树 = 组长自身 + 快照里的全部后代。组长不在快照里（已退出）时返回 None。
pub(crate) fn tree_sample(snap: &[ProcSnapshot], root: u32) -> Option<ResourceSample> {
    let index: HashMap<u32, &ProcSnapshot> = snap.iter().map(|p| (p.pid, p)).collect();
    let root_row = index.get(&root)?;
    let mut sample = ResourceSample {
        rss_bytes: root_row.rss_bytes,
        cpu_seconds: root_row.cpu_seconds,
    };
    for pid in descendants(snap, root).keys() {
        if let Some(row) = index.get(pid) {
            sample.rss_bytes = sample.rss_bytes.saturating_add(row.rss_bytes);
            sample.cpu_seconds += row.cpu_seconds;
        }
    }
    Some(sample)
}

// ── 监听 socket（可见性；P2-③ 闸复用同一份解析） ──

/// 本平台能不能采到监听 socket。
///
/// Windows 不能（既没有 `/proc` 也没有 `lsof`）。判定层据此返回
/// **Unsupported（没判）而不是 Ok（判了没事）**——安全闸在某个平台静默
/// 不生效是缺陷，不是功能缺失。
pub fn listening_supported() -> bool {
    cfg!(any(target_os = "linux", target_os = "macos"))
}

/// 一次监听采样的结果：**采到什么** + **采没采成**。
///
/// 两件事必须分开：采空了可能是「确实没人在监听」（**判了没事**），也可能是
/// 「采集器坏了 / 权限不足 / 系统文件表耗尽」（**没判**）。类型层不分开，
/// 判定层就只能把两种情况都当成 Ok——L1 那条真安全边界会在采集器失灵时
/// 静默失效：没有审计、没有通知、面板无变化。
#[derive(Debug, Clone, Default)]
pub struct ListeningSample {
    /// 按 pid 归属的监听 socket（采到什么算什么）
    pub by_pid: HashMap<u32, Vec<ListeningSocket>>,
    /// 采样是否成功。false = 采集路径存在但本次采不到（lsof 起不来 / 报错、
    /// `/proc/net/*` 不可读）——此时 `by_pid` 不可信，判定层必须当「没判」。
    pub complete: bool,
}

impl ListeningSample {
    /// 没有 pid 可采：没有采集动作发生，不算采集失败。
    fn empty() -> Self {
        Self {
            by_pid: HashMap::new(),
            complete: true,
        }
    }
}

/// 列出这些进程各自监听的 socket（TCP 的 LISTEN 态 + UDP 已绑定的端口）。
///
/// 只读系统 socket 表，不碰任何上层登记表。Linux 走 `/proc/<pid>/fd` 的 socket
/// inode → 配 `/proc/net/{tcp,tcp6,udp,udp6}`；macOS 走 `lsof`。
///
/// 返回**按 pid 归属**的映射：同一个 socket 可能被父子进程同时持有（fd 被继承），
/// 归属清楚了上层才能算「整棵树的监听合计」。
///
/// **这层只采不判**：采到什么返回什么，采没采成一并报（`ListeningSample::complete`）。
/// L1/L2/L3 的判定属 P2-③ 的闸，但判定用的数据必须来自这一份解析——两个条目
/// 各写一套 socket 解析就是重复采样 + 两套口径。
///
/// 平台覆盖：Linux 与 macOS 有路径；**Windows 没有**（`lsof` 不存在，
/// 而现有进程快照层在 Windows 上本来就退化了——它依赖 `ps`）。Windows 要做
/// 得走 `netstat -ano`，另开条目。
pub fn listening_sockets(pids: &[u32]) -> ListeningSample {
    let pids: Vec<u32> = pids.iter().copied().filter(|p| *p != 0).collect();
    if pids.is_empty() {
        return ListeningSample::empty();
    }
    #[cfg(target_os = "linux")]
    {
        listening_sockets_linux(&pids)
    }
    #[cfg(target_os = "macos")]
    {
        listening_sockets_lsof(&pids)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        // 本平台没有采集路径：不是「采空」，是「没采」。
        ListeningSample {
            by_pid: HashMap::new(),
            complete: false,
        }
    }
}

/// 组长 + 全部后代的 pid 列表（树口径，与 `tree_sample` 一致）。
pub(crate) fn tree_pids(snap: &[ProcSnapshot], root: u32) -> Vec<u32> {
    let mut out = vec![root];
    out.extend(descendants(snap, root).into_keys());
    out
}

/// 合并多组监听 socket（去重 + 按端口排序，面板展示要稳定顺序）。
pub fn merge_sockets(groups: Vec<Vec<ListeningSocket>>) -> Vec<ListeningSocket> {
    let mut list: Vec<ListeningSocket> = groups.into_iter().flatten().collect();
    list.sort_by(|a, b| {
        (a.port, &a.address, a.protocol.as_str()).cmp(&(b.port, &b.address, b.protocol.as_str()))
    });
    list.dedup_by(|a, b| a.protocol == b.protocol && a.address == b.address && a.port == b.port);
    list
}

/// Linux：读 `/proc/<pid>/fd` 拿 socket inode，再去 `/proc/net/*` 按 inode 配对。
#[cfg(target_os = "linux")]
fn listening_sockets_linux(pids: &[u32]) -> ListeningSample {
    // socket 表只读一次：inode → socket（多个 pid 共用这张表）
    let mut by_inode: HashMap<u64, ListeningSocket> = HashMap::new();
    let mut readable = 0usize;
    for (proto, path) in [
        (SocketProtocol::Tcp, "/proc/net/tcp"),
        (SocketProtocol::Tcp, "/proc/net/tcp6"),
        (SocketProtocol::Udp, "/proc/net/udp"),
        (SocketProtocol::Udp, "/proc/net/udp6"),
    ] {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        readable += 1;
        for (inode, sock) in parse_proc_net(&text, proto) {
            by_inode.insert(inode, sock);
        }
    }
    // 四张表一张都读不到 = 采集路径存在但本次不可用（受限容器 / hidepid）
    // ——不能当成「没有人在监听」。
    if readable == 0 {
        return ListeningSample {
            by_pid: HashMap::new(),
            complete: false,
        };
    }
    let mut out: HashMap<u32, Vec<ListeningSocket>> = HashMap::new();
    if by_inode.is_empty() {
        return ListeningSample {
            by_pid: out,
            complete: true,
        };
    }
    for pid in pids {
        let mut socks = Vec::new();
        for inode in socket_inodes(*pid) {
            if let Some(sock) = by_inode.get(&inode) {
                socks.push(sock.clone());
            }
        }
        if !socks.is_empty() {
            out.insert(*pid, merge_sockets(vec![socks]));
        }
    }
    ListeningSample {
        by_pid: out,
        complete: true,
    }
}

/// 某个进程持有的 socket inode（`/proc/<pid>/fd` 里形如 `socket:[12345]` 的链接）。
#[cfg(target_os = "linux")]
fn socket_inodes(pid: u32) -> Vec<u64> {
    let Ok(dir) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in dir.flatten() {
        let Ok(target) = std::fs::read_link(entry.path()) else {
            continue;
        };
        let text = target.to_string_lossy();
        let Some(rest) = text.strip_prefix("socket:[") else {
            continue;
        };
        let Some(num) = rest.strip_suffix(']') else {
            continue;
        };
        if let Ok(inode) = num.parse::<u64>() {
            out.push(inode);
        }
    }
    out
}

/// 解析 `/proc/net/tcp{,6}` 或 `/proc/net/udp{,6}`。
///
/// 行格式（空白分隔）：
/// `sl local_address rem_address st tx_queue:rx_queue tr:tm->when retrnsmt uid timeout inode ...`
/// - `local_address` = 十六进制地址:十六进制端口（地址是**小端字节序**打印的）
/// - `st` = 状态，TCP 的 `0A` 是 LISTEN
/// - `inode` = 第 10 列
///
/// TCP 只认 LISTEN 态；UDP 没有 LISTEN 态，**绑定即占用端口**，全收。
pub fn parse_proc_net(text: &str, proto: SocketProtocol) -> Vec<(u64, ListeningSocket)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 10 {
            continue;
        }
        // 表头行没有 `:`，自然被跳过——不靠「跳过第一行」这种位置假设
        let Some((addr_hex, port_hex)) = cols[1].split_once(':') else {
            continue;
        };
        if proto == SocketProtocol::Tcp && cols[3] != "0A" {
            continue;
        }
        let Some(address) = parse_hex_address(addr_hex) else {
            continue;
        };
        let Ok(port) = u16::from_str_radix(port_hex, 16) else {
            continue;
        };
        let Ok(inode) = cols[9].parse::<u64>() else {
            continue;
        };
        out.push((
            inode,
            ListeningSocket {
                protocol: proto,
                address,
                port,
            },
        ));
    }
    out
}

/// 解析 `/proc/net/*` 里的十六进制地址（内核按小端打印）→ 可读地址文本。
///
/// IPv4：4 字节，整体反序（`0100007F` → `127.0.0.1`）。
/// IPv6：16 字节，**每 4 字节一组**反序——内核按 `__be32` 打印，组内是反的、
/// 组间是正的，整体反会把 4 个字的顺序也倒过来。
fn parse_hex_address(hex: &str) -> Option<String> {
    let bytes = hex_to_bytes(hex)?;
    match bytes.len() {
        4 => {
            Some(std::net::Ipv4Addr::from(<[u8; 4]>::try_from(bytes.as_slice()).ok()?).to_string())
        }
        16 => {
            Some(std::net::Ipv6Addr::from(<[u8; 16]>::try_from(bytes.as_slice()).ok()?).to_string())
        }
        _ => None,
    }
}

/// 十六进制 → 字节，并按内核的小端打印规则反序。
fn hex_to_bytes(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }
    let mut raw = Vec::with_capacity(hex.len() / 2);
    for i in (0..hex.len()).step_by(2) {
        raw.push(u8::from_str_radix(&hex[i..i + 2], 16).ok()?);
    }
    match raw.len() {
        4 => {
            raw.reverse();
            Some(raw)
        }
        16 => {
            for word in raw.chunks_mut(4) {
                word.reverse();
            }
            Some(raw)
        }
        _ => None,
    }
}

/// macOS：走 `lsof`（一次调用带上全部 pid）。
#[cfg(target_os = "macos")]
fn listening_sockets_lsof(pids: &[u32]) -> ListeningSample {
    let pid_arg = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let mut by_pid: HashMap<u32, Vec<ListeningSocket>> = HashMap::new();
    let mut complete = true;
    for (proto, filters) in [
        (SocketProtocol::Tcp, vec!["-iTCP", "-sTCP:LISTEN"]),
        (SocketProtocol::Udp, vec!["-iUDP"]),
    ] {
        let Some(text) = run_lsof(&pid_arg, &filters) else {
            // lsof 起不来 / 报错 = 采集失败，不是「没匹配到」
            complete = false;
            continue;
        };
        for line in text.lines() {
            let Some((pid, sock)) = parse_lsof_line(line, proto) else {
                continue;
            };
            by_pid.entry(pid).or_default().push(sock);
        }
    }
    ListeningSample {
        by_pid: by_pid
            .into_iter()
            .map(|(pid, socks)| (pid, merge_sockets(vec![socks])))
            .collect(),
        complete,
    }
}

/// 调 `lsof` 取指定 pid 集合的网络监听行。
///
/// `None` = **lsof 跑不起来或跑出了错**（两个候选路径都起不来 / lsof 自己往
/// stderr 报了错）；`Some(text)` = 跑通了，text 可能为空——**没有匹配项时
/// lsof 退出码非 0、stdout 与 stderr 都是空的**（空 stdout 是「没匹配」，
/// 不是「采集失败」）。
///
/// 这两件事必须分开：「没有匹配项」是采空但采成功，「lsof 起不来 / 报错」是
/// 采集失败——后者会让 L1 闸静默失效。
#[cfg(target_os = "macos")]
fn run_lsof(pids: &str, filters: &[&str]) -> Option<String> {
    // lsof 在 /usr/sbin；GUI 启动的 app 拿到的 PATH 有时不含它，两个路径都试
    for bin in ["lsof", "/usr/sbin/lsof"] {
        let mut cmd = Command::new(bin);
        cmd.args(["-nP", "-a", "-p", pids]);
        for f in filters {
            cmd.arg(f);
        }
        let Ok(out) = cmd.output() else {
            continue;
        };
        // lsof 报错时 stderr 有内容（选项非法 / 系统文件表耗尽等）；没有匹配项时
        // stdout 与 stderr 都是空的。这个候选自己报错了就换下一个。
        if !out.stderr.is_empty() {
            continue;
        }
        return Some(String::from_utf8_lossy(&out.stdout).to_string());
    }
    None
}

/// 解析一行 `lsof -nP` 的网络行。
///
/// 列：`COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME [STATE]`
/// - `TYPE` 列给 `IPv4` / `IPv6`——通配地址 `*:7000` 要靠它区分是 `0.0.0.0` 还是 `::`
/// - `NAME` 列形如 `TCP *:7000 (LISTEN)` / `TCP 127.0.0.1:5231 (LISTEN)` / `TCP [::1]:5231 (LISTEN)`
///
/// 协议名的位置靠「前一个 token 是 IPv4/IPv6」认，不靠「第一个 TCP/UDP token」——
/// 进程名撞上 TCP 这种位置假设会错行。
///
/// 返回 (pid, socket)：pid 用来把 socket 归属到进程（fd 被父子继承时两边都算持有）。
pub fn parse_lsof_line(line: &str, proto: SocketProtocol) -> Option<(u32, ListeningSocket)> {
    let cols: Vec<&str> = line.split_whitespace().collect();
    let pid: u32 = cols.get(1)?.parse().ok()?;
    // 协议名的位置不能靠「第一个 TCP/UDP token」认（进程名可能撞上这个词）——
    // 取**后面跟着能拆出 `地址:端口`、前面有 IPv4/IPv6 类型列**的那一个。
    for (idx, c) in cols.iter().enumerate() {
        if *c != proto.as_str() {
            continue;
        }
        let Some(addr_port) = cols.get(idx + 1) else {
            continue;
        };
        // TYPE 列在协议名之前（列序：TYPE … NODE NAME），取其最后一次出现
        let ipv6 = cols[..idx].iter().rev().any(|t| *t == "IPv6");
        let Some((address, port)) = split_addr_port(addr_port, ipv6) else {
            continue;
        };
        return Some((
            pid,
            ListeningSocket {
                protocol: proto,
                address,
                port,
            },
        ));
    }
    None
}

/// 拆 `*:7000` / `127.0.0.1:5231` / `[::1]:5231` → (地址, 端口)。
fn split_addr_port(s: &str, ipv6: bool) -> Option<(String, u16)> {
    let (addr, port) = s.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let addr = addr.trim_start_matches('[').trim_end_matches(']');
    let address = match addr {
        "*" => {
            if ipv6 {
                "::".to_string()
            } else {
                "0.0.0.0".to_string()
            }
        }
        other => other.to_string(),
    };
    Some((address, port))
}

// ── 信号与存活判定 ──

#[cfg(unix)]
pub(crate) fn send_signal(pid: u32, sig: i32) -> bool {
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
pub(crate) fn signal_group(pgid: i32, sig: i32) -> bool {
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

/// 进程是否还存在（`kill(pid, 0)` 探测）。
///
/// 只做存在性判断，不发信号。僵尸也算存在（pid 未被回收）。
#[cfg(unix)]
pub(crate) fn pid_exists(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: kill(pid, 0) 只做存在性与权限探测，不发信号。
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Windows：进程是否还在（`OpenProcess` + `GetExitCodeProcess`）。
///
/// unix 的 `kill(pid, 0)` 在 Windows 上没有对应物。恒 false 会让
/// 「等进程退出」立刻判定「已退出」——停止阶梯因此整条空转，
/// 而上层照样回报「已停止」。
///
/// 已知边角：退出码恰好是 259（`STILL_ACTIVE`）的进程会被误判成还活着——
/// Windows 用退出码表示这个值没有特殊含义，撞上只能认。
#[cfg(windows)]
pub(crate) fn pid_exists(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: 只申请查询权限，不改进程状态；句柄用完即关。
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        // 打不开 = 进程不在（pid 已回收）或无权查询。
        return false;
    };
    let mut code = 0u32;
    // SAFETY: handle 由上面取得且有效；code 是栈上 u32，尺寸符合 API 要求。
    let queried = unsafe { GetExitCodeProcess(handle, &mut code) }.is_ok();
    // SAFETY: handle 由本函数取得，关闭一次。
    let _ = unsafe { CloseHandle(handle) };
    queried && code == STILL_ACTIVE.0 as u32
}

/// 其余平台：没有可靠的存活探测口径。
#[cfg(not(any(unix, windows)))]
pub(crate) fn pid_exists(_pid: u32) -> bool {
    false
}

/// 等进程退出（按存活探测轮询）。返回是否在窗口内退出。
pub(crate) fn wait_pid_exit(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if !pid_exists(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(ALIVE_POLL_MS));
    }
}

/// 整组杀是否安全：组长还在、且还是当初那个进程，才允许 `killpg`。
///
/// 纯函数：只吃 pid 空间的数据，不碰登记表。
///
/// 为什么不能只看"pid 还在不在"：进程被回收后 pid 就被释放了，
/// 之后这个号可能被系统分给别的进程；`kill(pid, 0)` 对两种情况都返回成功。
/// 核启动标识才能区分"还是我们的组长"和"号被借走了"。
#[cfg(unix)]
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

/// 停止一棵进程树的结果。
///
/// `method` 记的是**实际生效的手段**：`SIGINT` / `SIGTERM` / `SIGKILL`（unix）、
/// `taskkill` 及其失败形态（Windows）、`already_exited`（进程自己退了）、
/// `no_process`（没有进程可停——pid 0 占位条目 / pid 1）。
#[derive(Debug, Clone, Default)]
pub(crate) struct TreeStop {
    /// 实际生效的手段（`SIGINT` / `SIGTERM` / `SIGKILL`）
    pub(crate) method: String,
    /// 补杀的子孙数
    pub(crate) descendants_killed: usize,
    /// 识别到的 PID 复用者（pid, 旧启动标识, 新启动标识）——不杀
    pub(crate) reused_skipped: Vec<(u32, String, String)>,
    /// 总耗时（毫秒）
    pub(crate) elapsed_ms: u64,
}

/// 停止一棵进程树：SIGINT → 100ms → SIGTERM → 400ms → SIGKILL，
/// 并递归回收**脱离进程组**的子孙。
///
/// 纯进程操作：只依赖 pid 空间 + 一个「进程是否已退出」的等待回调
/// （服务层传登记表状态，别的调用方传存活探测）。
///
/// 阶梯的每一级发信号前各拍一张快照增量收集子孙，最后统一杀——
/// 边收边杀会打断正在进行的快照（父子链残缺 → 漏收）。
///
/// `Err` = **本平台没有可用的停止手段**（不是「停不掉这个进程」）。
/// 调用方必须区分这两者：前者是能力缺失，后者由等待回调的口径判定。
#[cfg(unix)]
pub(crate) fn stop_tree<F>(root_pid: u32, pgid: i32, mut wait_exit: F) -> Result<TreeStop, String>
where
    F: FnMut(Duration) -> bool,
{
    // **pid 0 / 1 不许当树根**：
    // - pid 0 是登记表的占位条目（spawn 还没成功），`kill(0, sig)` 的语义是
    //   「给调用方的整个进程组发信号」；
    // - pid 1 是 init/launchd，全机进程都是它的后代。
    // 拿它们当根去收集后代，收到的就是「宿主机上所有进程」，随后逐个 SIGKILL——
    // 那是宿主自杀。这里没有进程可停，如实返回「没有进程」。
    if root_pid <= 1 {
        return Ok(TreeStop {
            method: "no_process".to_string(),
            descendants_killed: 0,
            reused_skipped: Vec::new(),
            elapsed_ms: 0,
        });
    }
    let started = Instant::now();
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
                .find(|p| p.pid == root_pid)
                .map(|p| p.starttime.clone());
        }
        let cur = descendants(&snap, root_pid);
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

        send_signal(root_pid, sig);
        // 等到进程真的走了再进下一级
        if wait_exit(Duration::from_millis(grace_ms)) {
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
    // `kill(pid, 0)` 证明不了"还是我们那个"——进程一收尸，pid 就被
    // 释放了；这时 pid 仍存在只说明"这个号上有别的进程"。所以要核启动
    // 标识：组长还是同一个进程（标识一致）才允许整组杀。
    //
    // 组长已经走了的情况：组里剩下的成员由 ppid 树补杀覆盖（它们在
    // 快照里是组长的后代），不依赖整组杀。
    let group_safe = group_kill_allowed(&final_map, root_pid, leader_starttime.as_deref());
    if let (Some(cur), Some(old)) = (final_map.get(&root_pid), leader_starttime.as_deref()) {
        if *cur != old {
            // 组长这个 pid 已被复用：现在这个号上的不是我们的进程，剔除不杀
            reused_skipped.push((root_pid, old.to_string(), cur.to_string()));
        }
    }
    // 快照整个是空的（ps 读不到）→ 无从核验，但组长是我们自己 spawn 的，
    // 这时宁可赌一次也不能让服务活着跑（否则 stop 报「已停」而进程还在）。
    let unverifiable = final_snap.is_empty();
    if group_safe || unverifiable {
        signal_group(pgid, SIGKILL);
        // 组长自己：只在确认「还是我们那个进程」时才发最后一下。
        // 组长已优雅退出且 pid 被系统回收再分配给别人时，无条件 kill 会误杀它。
        send_signal(root_pid, SIGKILL);
    }
    let mut killed = 0usize;
    for p in &to_kill {
        if *p == root_pid {
            continue; // 主进程已单独处理
        }
        if send_signal(*p, SIGKILL) {
            killed += 1;
        }
    }

    Ok(TreeStop {
        method,
        descendants_killed: killed,
        reused_skipped,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

/// Windows 的停止阶梯：先等它自己退 → `taskkill /T /F` 强杀。
///
/// Windows 没有 unix 那套信号语义（`SIGINT` 在 Windows 上没有对应投递方式），
/// 停止走 `taskkill`：`/T` 连子孙一起杀，`/F` 不给优雅退出的机会。
///
/// **没有「关 stdin 让它自己退」这一级**：受管服务的 stdin 是 `Stdio::null()`
/// （没有可关的管道）；MCP stdio 的 stdin 管道要到连接层释放传输时才关，
/// 而那发生在本阶梯**之后**。第一级窗口只是留给「已经自己退出的进程」。
#[cfg(windows)]
pub(crate) fn stop_tree<F>(root_pid: u32, _pgid: i32, mut wait_exit: F) -> Result<TreeStop, String>
where
    F: FnMut(Duration) -> bool,
{
    // pid 0 是占位条目（没有进程可停），pid 1 是 init——都不许当目标
    if root_pid <= 1 {
        return Ok(TreeStop {
            method: "no_process".to_string(),
            descendants_killed: 0,
            reused_skipped: Vec::new(),
            elapsed_ms: 0,
        });
    }
    let started = Instant::now();
    // 已经自己退了（服务自己结束 / 连接断开）就不用动刀
    if wait_exit(Duration::from_millis(WINDOWS_EXIT_GRACE_MS)) {
        return Ok(TreeStop {
            method: "already_exited".to_string(),
            descendants_killed: 0,
            reused_skipped: Vec::new(),
            elapsed_ms: started.elapsed().as_millis() as u64,
        });
    }
    let killed = taskkill_tree(root_pid);
    // 杀完再等一次：进程真没了才算停掉（等的是调用方的权威口径）
    let gone = wait_exit(Duration::from_millis(WINDOWS_KILL_WAIT_MS));
    let method = if gone {
        "taskkill".to_string()
    } else if killed {
        // 命令跑成了，进程却还在——如实记，别写成「已停」
        "taskkill_failed".to_string()
    } else {
        // 命令没跑成（找不到 taskkill / 没权限）
        "taskkill_unavailable".to_string()
    };
    Ok(TreeStop {
        method,
        // `taskkill /T` 是整树一起杀的，宿主这边数不出被杀的子孙个数
        descendants_killed: 0,
        reused_skipped: Vec::new(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

/// 本平台没有可用的停止手段（unix 靠信号，Windows 靠 taskkill）。
///
/// 这里**必须显式失败而不是空转**：调用方据此判定「停不掉」，
/// 不许把「一个信号都没发出去」当成「已经停掉」。
#[cfg(not(any(unix, windows)))]
pub(crate) fn stop_tree<F>(_root_pid: u32, _pgid: i32, _wait_exit: F) -> Result<TreeStop, String>
where
    F: FnMut(Duration) -> bool,
{
    Err("本平台没有可用的停止手段".to_string())
}

/// Windows：整棵进程树强杀（`taskkill /PID <pid> /T /F`）。
///
/// 返回是否杀成功。杀不掉时调用方按「进程可能还活着」处理——
/// 停止结果由等待口径判定，不由这条命令的退出码判定。
#[cfg(windows)]
fn taskkill_tree(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let pid_text = pid.to_string();
    let mut cmd = match taskkill_program() {
        Some(program) => silent_cmd(&program),
        None => silent_cmd("taskkill"),
    };
    cmd.args(["/PID", &pid_text, "/T", "/F"]);
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

/// 定位 `taskkill.exe`。
///
/// 优先 `%SystemRoot%\System32\taskkill.exe` 绝对路径：GUI 子进程继承到的
/// PATH 不一定含 System32，裸名会「找不到程序」——而找不到程序与「杀掉了」
/// 在返回码上长得一模一样。
#[cfg(windows)]
fn taskkill_program() -> Option<std::path::PathBuf> {
    let root = std::env::var_os("SystemRoot")?;
    let path = std::path::PathBuf::from(root)
        .join("System32")
        .join("taskkill.exe");
    path.is_file().then_some(path)
}

/// 静默执行一条外部命令（不弹控制台窗口、不接管道）。
#[cfg(windows)]
fn silent_cmd(program: impl AsRef<std::ffi::OsStr>) -> Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// Windows 上创建控制台窗口的标志位（`CREATE_NO_WINDOW`）。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(test)]
mod tests;
