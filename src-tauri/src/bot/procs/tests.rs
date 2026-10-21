//! 进程树模块的纯函数测试。
//!
//! 这一层被受管服务层与 MCP stdio 连接共用，所以它的行为必须独立于上层
//! 登记表：快照解析、ppid 树收集、PID 复用识别、整组杀护栏。

use super::*;
use std::collections::{HashMap, HashSet};

fn snap(pid: u32, ppid: u32, st: &str) -> ProcSnapshot {
    ProcSnapshot {
        pid,
        ppid,
        starttime: st.to_string(),
        rss_bytes: 0,
        cpu_seconds: 0.0,
    }
}

/// 带资源数据的快照行（资源观测用）。
fn snap_rss(pid: u32, ppid: u32, st: &str, rss_bytes: u64, cpu_seconds: f64) -> ProcSnapshot {
    ProcSnapshot {
        pid,
        ppid,
        starttime: st.to_string(),
        rss_bytes,
        cpu_seconds,
    }
}

// ── 进程树收集 ──

#[test]
fn descendants_collects_multi_level_tree() {
    let s = vec![
        snap(100, 1, "t100"),
        snap(101, 100, "t101"),
        snap(102, 101, "t102"),
        snap(103, 102, "t103"),
        snap(200, 1, "t200"),
    ];
    let d = descendants(&s, 100);
    let pids: HashSet<u32> = d.keys().copied().collect();
    assert_eq!(pids, HashSet::from([101, 102, 103]));
    assert!(!pids.contains(&100), "root 自身不算后代");
    assert!(!pids.contains(&200), "无祖先关系的进程不进集合");
    assert_eq!(d.get(&103).map(String::as_str), Some("t103"));
}

#[test]
fn descendants_of_leaf_is_empty() {
    let s = vec![snap(100, 1, "t100"), snap(101, 100, "t101")];
    assert!(descendants(&s, 101).is_empty());
}

#[test]
fn descendants_tolerates_cycle() {
    // 坏数据：a 的父亲是 b，b 的父亲是 a。收集必须终止，不能死循环
    let s = vec![snap(1, 2, "t1"), snap(2, 1, "t2")];
    let d = descendants(&s, 1);
    assert_eq!(d.len(), 2);
}

#[test]
fn descendants_of_unknown_root_is_empty() {
    let s = vec![snap(100, 1, "t100")];
    assert!(descendants(&s, 999).is_empty());
}

// ── 增量比对与 PID 复用 ──

#[test]
fn diff_reports_newly_appeared_descendants() {
    let prev: HashMap<u32, String> = HashMap::new();
    let cur: HashMap<u32, String> = HashMap::from([(101, "t101".to_string())]);
    let scan = diff_descendants(&prev, &cur);
    assert_eq!(scan.added.len(), 1);
    assert!(scan.reused.is_empty());
}

#[test]
fn diff_flags_pid_reuse_instead_of_treating_it_as_new() {
    // 同一个 pid，启动标识变了 → 被复用 → 剔除不杀
    let prev: HashMap<u32, String> = HashMap::from([(101, "old".to_string())]);
    let cur: HashMap<u32, String> = HashMap::from([(101, "new".to_string())]);
    let scan = diff_descendants(&prev, &cur);
    assert!(scan.added.is_empty());
    assert_eq!(scan.reused.len(), 1);
    assert_eq!(scan.reused[0].0, 101);
}

#[test]
fn diff_ignores_unchanged_process() {
    let prev: HashMap<u32, String> = HashMap::from([(101, "t101".to_string())]);
    let cur: HashMap<u32, String> = HashMap::from([(101, "t101".to_string())]);
    let scan = diff_descendants(&prev, &cur);
    assert!(scan.added.is_empty());
    assert!(scan.reused.is_empty());
}

// ── 快照解析 ──

#[test]
fn ps_lstart_parser_splits_pid_ppid_and_five_lstart_tokens() {
    // ps -o pid= -o ppid= -o lstart= -o rss= -o time= 的行：lstart 恒为 5 段
    let text = "  100     1 Wed Oct 21 18:15:00 2026  20480 00:01:30\n  101   100 Wed Oct 21 18:15:01 2026  10240 00:00:05\n";
    let s = parse_ps_rows(text);
    assert_eq!(s.len(), 2);
    assert_eq!(s[0].pid, 100);
    assert_eq!(s[0].ppid, 1);
    assert_eq!(s[0].starttime, "Wed Oct 21 18:15:00 2026");
    assert_eq!(s[1].ppid, 100);
    // rss 是 KB → 字节
    assert_eq!(s[0].rss_bytes, 20480 * 1024);
    // time 是累计 CPU 时间 → 秒
    assert!((s[0].cpu_seconds - 90.0).abs() < 0.01);
    assert!((s[1].cpu_seconds - 5.0).abs() < 0.01);
}

#[test]
fn ps_lstart_parser_skips_malformed_lines() {
    let text = "garbage\n  100     1 Wed Oct 21 18:15:00 2026  20480 00:01:30\n";
    let s = parse_ps_rows(text);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].pid, 100);
}

#[test]
fn ps_time_parser_handles_hms_ms_and_days() {
    assert!((parse_ps_time("00:01:30").unwrap() - 90.0).abs() < 0.01);
    assert!((parse_ps_time("01:30").unwrap() - 90.0).abs() < 0.01);
    assert!((parse_ps_time("1-00:00:00").unwrap() - 86400.0).abs() < 0.01);
    assert!((parse_ps_time("00:00:01.50").unwrap() - 1.5).abs() < 0.01);
    assert!(parse_ps_time("").is_none());
    assert!(parse_ps_time("abc").is_none());
}

#[test]
fn proc_stat_parser_handles_spaces_in_comm() {
    // comm 字段在括号里且可能含空格 → 必须从最后一个 ')' 之后切
    let stat = "3210 (my proc (x)) S 100 3210 3210 0 -1 4194304 100 0 0 0 \
                10 5 0 0 20 0 3 0 987654 12345678 4242 18446744073709551615 \
                1 1 0 0 0 0 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0";
    let s = parse_proc_stat(3210, stat).expect("应能解析");
    assert_eq!(s.pid, 3210);
    assert_eq!(s.ppid, 100);
    assert_eq!(s.starttime, "987654");
    // utime=10 + stime=5 tick（100 tick = 1s）→ 0.15s
    assert!((s.cpu_seconds - 0.15).abs() < 0.001);
    // rss 以页计（页大小随平台：macOS arm64 是 16KB，Linux x64 是 4KB）
    assert_eq!(s.rss_bytes / page_size(), 4242);
}

// ── 资源采样 ──

#[test]
fn tree_sample_sums_whole_process_tree() {
    // 组长 100MB/1s + 子进程 50MB/2s + 孙子 25MB/3s
    let s = vec![
        snap_rss(100, 1, "t100", 100 * 1024 * 1024, 1.0),
        snap_rss(101, 100, "t101", 50 * 1024 * 1024, 2.0),
        snap_rss(102, 101, "t102", 25 * 1024 * 1024, 3.0),
        snap_rss(200, 1, "t200", 999 * 1024 * 1024, 9.0),
    ];
    let sample = tree_sample(&s, 100).expect("组长在快照里");
    assert_eq!(sample.rss_bytes, 175 * 1024 * 1024);
    assert!((sample.cpu_seconds - 6.0).abs() < 0.001);
    // 组长不在快照里 → 采不到
    assert!(tree_sample(&s, 999).is_none());
}

// ── 整组杀护栏 ──

#[test]
fn group_kill_allowed_only_for_same_leader_process() {
    let mut map: HashMap<u32, &str> = HashMap::new();
    map.insert(100, "t100");
    // 启动标识一致 → 组长还是那个进程 → 允许整组杀
    assert!(group_kill_allowed(&map, 100, Some("t100")));
    // 启动标识变了 → pid 已被复用 → 不许整组杀
    assert!(!group_kill_allowed(&map, 100, Some("t-other")));
    // 组长已不在快照里（走了且被回收）→ 无法核验 → 不许整组杀
    assert!(!group_kill_allowed(&map, 100, None));
    assert!(!group_kill_allowed(&map, 999, Some("t100")));
}

#[cfg(unix)]
#[test]
fn send_signal_rejects_pid_zero() {
    // kill(0, sig) 的语义是"给调用方的整个进程组发信号"——发出去就是宿主自杀。
    // 登记表占位条目（spawn 还没成功）pid 就是 0，必须拒掉。
    assert!(!send_signal(0, libc::SIGINT));
    assert!(!send_signal(0, libc::SIGKILL));
    assert!(!signal_group(0, libc::SIGKILL));
}

// ── 监听 socket ──

/// `/proc/net/tcp` 的一行（表头 + 数据行）。
const PROC_NET_TCP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 26231 1 0000000000000000 100 0 0 10 0\n   1: 0100007F:1F91 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 26232 1 0000000000000000 100 0 0 10 0\n   2: 0100007F:1F92 0100007F:D431 01 00000000:00000000 00:00000000 00000000     0        0 26233 1 0000000000000000 100 0 0 10 0\n";

#[test]
fn parse_proc_net_reads_listening_sockets_only() {
    let rows = parse_proc_net(PROC_NET_TCP, SocketProtocol::Tcp);
    // 只有 LISTEN（0A）那两行；ESTABLISHED（01）不算监听
    assert_eq!(rows.len(), 2);
    let (inode, sock) = &rows[0];
    assert_eq!(*inode, 26231);
    assert_eq!(sock.protocol, SocketProtocol::Tcp);
    // 00000000 小端反序 → 0.0.0.0（通配 = 非 loopback，L1 的判据）
    assert_eq!(sock.address, "0.0.0.0");
    assert_eq!(sock.port, 8080);
    // 0100007F 小端反序 → 127.0.0.1
    assert_eq!(rows[1].1.address, "127.0.0.1");
    assert_eq!(rows[1].1.port, 8081);
}

#[test]
fn parse_proc_net_reads_ipv6_loopback() {
    // 内核按 __be32 打印，**每 4 字节组内是反的**（组间顺序不变）：
    // ::1 的地址字节是 15 个 0 + 1，最后一组打印成 "01000000"（不是 "00000001"）。
    // 同一机制的旁证：fe80:: 的第一组打印成 "000080FE"。
    let text = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 00000000000000000000000001000000:1F92 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 9 1 0000000000000000 100 0 0 10 0\n";
    let rows = parse_proc_net(text, SocketProtocol::Tcp);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1.address, "::1");
    assert_eq!(rows[0].1.port, 8082);
}

#[test]
fn parse_proc_net_keeps_udp_bound_ports() {
    // UDP 没有 LISTEN 态：绑定即占用端口，st=07（UDP）也要收
    let text = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 00000000:14E9 00000000:0000 07 00000000:00000000 00:00000000 00000000     0        0 7 1 0000000000000000 100 0 0 10 0\n";
    let rows = parse_proc_net(text, SocketProtocol::Udp);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1.protocol, SocketProtocol::Udp);
    assert_eq!(rows[0].1.address, "0.0.0.0");
    assert_eq!(rows[0].1.port, 5353);
}

#[test]
fn parse_proc_net_skips_malformed_lines() {
    let text =
        "not a socket table\n   0: broken\n   1: 00000000:ZZZZ 00000000:0000 0A 0 0 0 0 0 0\n";
    assert!(parse_proc_net(text, SocketProtocol::Tcp).is_empty());
}

#[test]
fn parse_lsof_line_splits_address_and_port() {
    let line = "ControlCe 616 renshi    9u  IPv4  0x379717086c3df0f      0t0  TCP *:7000 (LISTEN)";
    let (pid, sock) = parse_lsof_line(line, SocketProtocol::Tcp).expect("应解析出监听 socket");
    assert_eq!(pid, 616);
    // 通配地址要按 IPv4/IPv6 分开记——判 loopback 靠这个
    assert_eq!(sock.address, "0.0.0.0");
    assert_eq!(sock.port, 7000);
    assert_eq!(sock.protocol, SocketProtocol::Tcp);
}

#[test]
fn parse_lsof_line_keeps_ipv6_wildcard_distinct() {
    let line = "ControlCe 616 renshi   10u  IPv6 0x24ea8df863a2157b      0t0  TCP *:7000 (LISTEN)";
    let (_, sock) = parse_lsof_line(line, SocketProtocol::Tcp).expect("应解析出监听 socket");
    assert_eq!(sock.address, "::");
    assert_eq!(sock.port, 7000);
}

#[test]
fn parse_lsof_line_reads_loopback_addresses() {
    let (pid, v4) = parse_lsof_line(
        "node 900 me 20u IPv4 0x1 0t0 TCP 127.0.0.1:5231 (LISTEN)",
        SocketProtocol::Tcp,
    )
    .expect("应解析出监听 socket");
    assert_eq!(pid, 900);
    assert_eq!(v4.address, "127.0.0.1");
    assert_eq!(v4.port, 5231);

    let (_, v6) = parse_lsof_line(
        "node 900 me 21u IPv6 0x2 0t0 TCP [::1]:5231 (LISTEN)",
        SocketProtocol::Tcp,
    )
    .expect("应解析出监听 socket");
    assert_eq!(v6.address, "::1");
    assert_eq!(v6.port, 5231);
}

#[test]
fn parse_lsof_line_reads_udp() {
    let (_, sock) = parse_lsof_line(
        "node 900 me 22u IPv4 0x3 0t0 UDP *:5353",
        SocketProtocol::Udp,
    )
    .expect("应解析出 UDP socket");
    assert_eq!(sock.protocol, SocketProtocol::Udp);
    assert_eq!(sock.address, "0.0.0.0");
    assert_eq!(sock.port, 5353);
}

#[test]
fn parse_lsof_line_ignores_non_network_and_mismatched_proto() {
    assert!(parse_lsof_line("node 900 me cwd DIR 0x1 0t0 /tmp", SocketProtocol::Tcp).is_none());
    // 协议对不上（这条是 UDP 行，按 TCP 问）→ 不认
    assert!(parse_lsof_line(
        "node 900 me 22u IPv4 0x3 0t0 UDP *:5353",
        SocketProtocol::Tcp
    )
    .is_none());
}

/// 真子进程：起一个绑在 127.0.0.1 的 TCP 监听，验平台路径能采到。
///
/// 这是**平台级**验证——解析函数的单测锁不住「lsof / /proc 能不能真读到」，
/// 平台路径错了一直要到 P2-③ 挂闸时才发现就晚了。
#[cfg(unix)]
#[test]
fn listening_sockets_finds_a_real_child_listener() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let probe = Command::new("python3")
        .arg("-c")
        .arg(
            "import socket,time\n\
             s=socket.socket()\n\
             s.bind(('127.0.0.1',0))\n\
             s.listen(8)\n\
             print(s.getsockname()[1],flush=True)\n\
             time.sleep(30)\n",
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = probe else {
        eprintln!("[skip] 无 python3，跳过监听 socket 集成测试");
        return;
    };
    let pid = child.id();
    let mut line = String::new();
    if let Some(out) = child.stdout.take() {
        let _ = BufReader::new(out).read_line(&mut line);
    }
    let Ok(port) = line.trim().parse::<u16>() else {
        let _ = child.kill();
        let _ = child.wait();
        eprintln!("[skip] 子进程没报出端口（{line:?}），跳过");
        return;
    };

    // 采到为止（子进程刚起来时 lsof 可能还没看到）
    let mut found = false;
    for _ in 0..40 {
        let socks = listening_sockets(&[pid]);
        let mine = socks.get(&pid).cloned().unwrap_or_default();
        if mine
            .iter()
            .any(|s| s.port == port && s.address == "127.0.0.1")
        {
            found = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let socks = listening_sockets(&[pid]);
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        found,
        "应采到子进程的监听 socket 127.0.0.1:{port}，实际采到 {socks:?}"
    );
}

/// 没有任何 pid 时不做事（登记表的占位条目 pid 就是 0）。
#[test]
fn listening_sockets_of_empty_input_is_empty() {
    assert!(listening_sockets(&[]).is_empty());
    assert!(listening_sockets(&[0]).is_empty());
}

/// 组长 + 后代的 pid 列表（树口径）。
#[test]
fn tree_pids_covers_root_and_descendants() {
    let s = vec![
        snap(100, 1, "t100"),
        snap(101, 100, "t101"),
        snap(102, 101, "t102"),
        snap(200, 1, "t200"),
    ];
    let mut pids = tree_pids(&s, 100);
    pids.sort_unstable();
    assert_eq!(pids, vec![100, 101, 102]);
}

#[test]
fn parse_lsof_line_survives_a_process_named_like_the_protocol() {
    // 进程名撞上 "TCP"：命令列不能被当成协议列（位置假设会错行）
    let line = "TCP 900 me 20u IPv4 0x1 0t0 TCP 127.0.0.1:5231 (LISTEN)";
    let (pid, sock) = parse_lsof_line(line, SocketProtocol::Tcp).expect("应解析出监听 socket");
    assert_eq!(pid, 900);
    assert_eq!(sock.address, "127.0.0.1");
    assert_eq!(sock.port, 5231);
}
