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
