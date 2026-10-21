//! 受管服务层的纯函数与集成测试。
//!
//! 纯函数部分（进程树收集 / PID 复用识别 / 日志截断 / 快照解析）不依赖任何
//! 外部进程，是本层最容易出错也最该被锁住的部分。

use super::*;
use std::collections::{HashMap, HashSet};

use tauri::Manager;

fn snap(pid: u32, ppid: u32, st: &str) -> ProcSnapshot {
    ProcSnapshot {
        pid,
        ppid,
        starttime: st.to_string(),
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
    let prev: HashMap<u32, String> = HashMap::from([(101, "t101".to_string())]);
    let cur: HashMap<u32, String> =
        HashMap::from([(101, "t101".to_string()), (102, "t102".to_string())]);
    let scan = diff_descendants(&prev, &cur);
    assert_eq!(scan.added, vec![(102, "t102".to_string())]);
    assert!(scan.reused.is_empty());
}

#[test]
fn diff_flags_pid_reuse_instead_of_treating_it_as_new() {
    // 同一个 pid，启动标识变了 → 被复用了，不算新增后代
    let prev: HashMap<u32, String> = HashMap::from([(101, "t101".to_string())]);
    let cur: HashMap<u32, String> = HashMap::from([(101, "tNEW".to_string())]);
    let scan = diff_descendants(&prev, &cur);
    assert!(scan.added.is_empty(), "复用的 pid 不得进新增集合");
    assert_eq!(
        scan.reused,
        vec![(101, "t101".to_string(), "tNEW".to_string())]
    );
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
    // ps -o pid= -o ppid= -o lstart= 的行：lstart 恒为 5 段
    let text = "  100     1 Wed Oct 21 18:15:00 2026\n  101   100 Wed Oct 21 18:15:01 2026\n";
    let s = parse_ps_lstart(text);
    assert_eq!(s.len(), 2);
    assert_eq!(s[0].pid, 100);
    assert_eq!(s[0].ppid, 1);
    assert_eq!(s[0].starttime, "Wed Oct 21 18:15:00 2026");
    assert_eq!(s[1].ppid, 100);
}

#[test]
fn ps_lstart_parser_skips_malformed_lines() {
    let text = "garbage\n  100     1 Wed Oct 21 18:15:00 2026\n";
    let s = parse_ps_lstart(text);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].pid, 100);
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
}

// ── 日志截断 ──

#[test]
fn truncate_keeps_tail_and_drops_head() {
    let path = std::env::temp_dir().join(format!("wm-svc-trunc-{}", uuid::Uuid::new_v4()));
    // 前 100 字节是头，后 100 字节是尾
    let mut content = vec![b'H'; 100];
    content.extend(vec![b'T'; 100]);
    std::fs::write(&path, &content).expect("写临时文件");

    let dropped = truncate_output_file(&path, 128).expect("截断");
    assert!(dropped > 0);
    let after = std::fs::read(&path).expect("读回");
    assert!(
        after.len() <= 128,
        "截断后大小必须 <= cap，实际 {}",
        after.len()
    );
    // 只保留末尾 cap/2 字节：原文件尾部 64 字节全是 'T'，头部 100 个 'H' 全丢
    assert_eq!(after.len(), 64, "截断后应剩 cap/2 字节");
    assert!(after.iter().all(|b| *b == b'T'), "留下的必须全是尾部字节");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn truncate_is_noop_under_cap() {
    let path = std::env::temp_dir().join(format!("wm-svc-trunc-{}", uuid::Uuid::new_v4()));
    std::fs::write(&path, b"short").expect("写临时文件");
    let dropped = truncate_output_file(&path, 1024).expect("截断");
    assert_eq!(dropped, 0);
    assert_eq!(std::fs::read(&path).expect("读回"), b"short");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn truncate_rewrite_does_not_create_holes() {
    // 关键：宿主写回用 append 模式 → 截断后续写落在新末尾，不产生 NUL 空洞
    let path = std::env::temp_dir().join(format!("wm-svc-trunc-{}", uuid::Uuid::new_v4()));
    let mut content = vec![b'H'; 100];
    content.extend(vec![b'T'; 100]);
    std::fs::write(&path, &content).expect("写临时文件");

    truncate_output_file(&path, 128).expect("截断");
    // 模拟子进程续写（append）
    {
        let mut f = open_append(&path).expect("append 打开");
        use std::io::Write;
        f.write_all(b"NEXT").expect("续写");
    }
    let after = std::fs::read(&path).expect("读回");
    assert!(!after.contains(&0), "文件里不该有 NUL 空洞");
    assert_eq!(after.len(), 64 + 4, "截断后 64 字节 + 续写 4 字节");
    assert!(
        after.ends_with(b"NEXT"),
        "续写必须落在文件末尾，实际尾部 {:?}",
        String::from_utf8_lossy(&after[after.len().saturating_sub(8)..])
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn tail_lines_returns_last_n_lines() {
    let path = std::env::temp_dir().join(format!("wm-svc-tail-{}", uuid::Uuid::new_v4()));
    std::fs::write(&path, "l1\nl2\nl3\nl4\nl5\n").expect("写临时文件");
    let t = tail_lines(&path, 2);
    assert_eq!(t, vec!["l4".to_string(), "l5".to_string()]);
    let _ = std::fs::remove_file(&path);
}

// ── 白名单与状态 ──

#[test]
fn build_command_rejects_non_allowlisted_launcher() {
    let r = build_command("bash", &[], Path::new("/tmp"));
    assert!(r.is_err(), "白名单外的启动器必须拒绝");
    let msg = r.err().unwrap_or_default();
    assert!(msg.contains("白名单"), "错误文案要点明白名单：{msg}");
}

#[test]
fn build_command_accepts_allowlisted_launcher() {
    let r = build_command("node", &["--version".to_string()], Path::new("/tmp"));
    assert!(r.is_ok(), "白名单内的启动器必须放行");
}

#[test]
fn build_command_allowlist_matches_mcp_source_of_truth() {
    // 与 MCP 侧同一份白名单（单一事实源）：多出来的启动器都该被拒
    for cmd in STDIO_LAUNCH_ALLOWLIST {
        assert!(
            build_command(cmd, &[], Path::new("/tmp")).is_ok(),
            "白名单内的 {cmd} 应放行"
        );
    }
    for cmd in ["bash", "sh", "/bin/sh", "curl", "sh -c"] {
        assert!(
            build_command(cmd, &[], Path::new("/tmp")).is_err(),
            "{cmd} 不在白名单内，必须拒绝"
        );
    }
}

#[test]
fn state_active_only_for_live_states() {
    assert!(ServiceState::Starting.is_active());
    assert!(ServiceState::Running.is_active());
    assert!(ServiceState::Stopping.is_active());
    assert!(!ServiceState::Exited.is_active());
    assert!(!ServiceState::Failed.is_active());
}

// ── 集成：真进程 ──

/// 找一个能长驻的启动器做集成测试。
///
/// 用白名单里的**裸名**（`python3`），不是绝对路径——白名单只认裸名，
/// 绝对路径的第三方二进制是刻意不放行的。
fn long_running_launcher() -> Option<(String, Vec<String>)> {
    if which("python3").is_none() && which("python").is_none() {
        return None;
    }
    let cmd = if which("python3").is_some() {
        "python3"
    } else {
        "python"
    };
    Some((
        cmd.to_string(),
        vec![
            "-c".to_string(),
            "import time\nwhile True: time.sleep(1)\n".to_string(),
        ],
    ))
}

/// 找可执行文件（不新增依赖：走 PATH 手工解析）。
fn which(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}

#[test]
fn start_and_stop_a_real_service() {
    let Some((py, args)) = long_running_launcher() else {
        eprintln!("跳过：环境里没有 python3");
        return;
    };
    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    app.manage(crate::app_state::AppState::default());

    let req = StartRequest {
        command: py,
        args,
        cwd: std::env::temp_dir(),
        name: "test".to_string(),
        ready_pattern: None,
        ready_timeout_secs: 10,
        session_id: None,
    };
    let out = do_start(&handle, &req).expect("服务应能启动");

    // 登记表里能查到
    let all = list_all(&handle);
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].state, ServiceState::Running);
    assert!(all[0].pid > 0, "登记表里的 pid 必须有效");
    assert_eq!(all[0].id, out.id);

    // 停得掉
    let stopped = stop_service(&handle, &out.id, "test").expect("停止应成功");
    assert!(stopped.stopped);
    let all = list_all(&handle);
    assert_eq!(all[0].state, ServiceState::Exited);
}

#[test]
fn start_rejects_command_outside_allowlist() {
    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    app.manage(crate::app_state::AppState::default());
    let req = StartRequest {
        command: "bash".to_string(),
        args: vec!["-c".to_string(), "sleep 100".to_string()],
        cwd: std::env::temp_dir(),
        name: String::new(),
        ready_pattern: None,
        ready_timeout_secs: 5,
        session_id: None,
    };
    let r = do_start(&handle, &req);
    assert!(r.is_err(), "白名单外的命令必须拒绝");
    assert!(list_all(&handle).is_empty(), "被拒的启动不得留下登记条目");
}

#[test]
fn stop_is_idempotent_for_exited_service() {
    let Some((py, args)) = long_running_launcher() else {
        eprintln!("跳过：环境里没有 python3");
        return;
    };
    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    app.manage(crate::app_state::AppState::default());

    let req = StartRequest {
        command: py,
        args,
        cwd: std::env::temp_dir(),
        name: String::new(),
        ready_pattern: None,
        ready_timeout_secs: 10,
        session_id: None,
    };
    let out = do_start(&handle, &req).expect("服务应能启动");
    let first = stop_service(&handle, &out.id, "test").expect("首次停止成功");
    assert!(first.stopped);
    let second = stop_service(&handle, &out.id, "test").expect("二次停止不报错");
    assert!(!second.stopped, "已退出的服务再停一次不重复计成功");
    assert_eq!(second.method, "already_exited");
}

#[test]
fn stop_unknown_service_is_an_error() {
    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    app.manage(crate::app_state::AppState::default());
    let r = stop_service(&handle, "svc_00000000", "test");
    assert!(r.is_err(), "未知 id 必须报错（模型传错了 id 是真问题）");
}

#[test]
fn stop_reclaims_descendant_that_escaped_the_process_group() {
    // 核心行为：服务自己 spawn 一个 setsid 逃逸的孙进程（脱离进程组），
    // 停掉服务后孙进程必须一起消失——否则宿主留下管不到的孤儿。
    //
    // 孙进程的 ppid 仍是服务进程（setsid 只改会话/组，不改 ppid），
    // 所以它能被 ppid 树收集到；进程组杀抓不到它（它在别的组）。
    let Some((py, _)) = long_running_launcher() else {
        eprintln!("跳过：环境里没有 python3");
        return;
    };
    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    app.manage(crate::app_state::AppState::default());

    let script = "import os, subprocess, sys, time\n\
                  p = subprocess.Popen([sys.executable, '-c', 'import time\\nwhile True: time.sleep(1)'], start_new_session=True)\n\
                  print(p.pid, flush=True)\n\
                  while True: time.sleep(1)\n";
    let req = StartRequest {
        command: py,
        args: vec!["-c".to_string(), script.to_string()],
        cwd: std::env::temp_dir(),
        name: String::new(),
        ready_pattern: None,
        ready_timeout_secs: 10,
        session_id: None,
    };
    let out = do_start(&handle, &req).expect("服务应能启动");

    // 从 stdout 日志里读出孙进程 pid
    let stdout_path = list_all(&handle)[0].stdout_path.clone();
    let grandchild = wait_for_printed_pid(&stdout_path, Duration::from_secs(10))
        .expect("孙进程 pid 应被打印到 stdout");
    assert!(pid_exists(grandchild), "孙进程此刻应活着");

    let stopped = stop_service(&handle, &out.id, "test").expect("停止应成功");
    assert!(stopped.stopped);

    // 孙进程必须被回收
    let deadline = Instant::now() + Duration::from_secs(5);
    while pid_exists(grandchild) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !pid_exists(grandchild),
        "setsid 逃逸的孙进程没被回收（pid {grandchild}）——宿主会留下管不到的孤儿"
    );
}

/// 等 stdout 日志里出现一个纯数字行（孙进程 pid）。
fn wait_for_printed_pid(path: &Path, timeout: Duration) -> Option<u32> {
    let deadline = Instant::now() + timeout;
    loop {
        for line in tail_lines(path, 20) {
            if let Ok(pid) = line.trim().parse::<u32>() {
                if pid > 1 {
                    return Some(pid);
                }
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn service_does_not_inherit_cpu_limit_and_survives_burn() {
    // 核心回归：服务跑一个持续烧 CPU 的进程，必须活得比 run_python 的
    // RLIMIT_CPU（= timeout+10）长得多。run_python 里同样的脚本 2s 就被
    // SIGXCPU 杀掉（见设计文档 §5.1 的实测）。
    #[cfg(unix)]
    {
        let Some((py, _)) = long_running_launcher() else {
            eprintln!("跳过：环境里没有 python3");
            return;
        };
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        app.manage(crate::app_state::AppState::default());

        let req = StartRequest {
            command: py,
            args: vec![
                "-c".to_string(),
                "t=__import__('time').time()\nwhile __import__('time').time()-t<8: pass\n"
                    .to_string(),
            ],
            cwd: std::env::temp_dir(),
            name: String::new(),
            ready_pattern: None,
            ready_timeout_secs: 10,
            session_id: None,
        };
        let out = do_start(&handle, &req).expect("服务应能启动");
        // 烧 8s CPU（远超 run_python 默认 60s 超时对应的 70s CPU 限额的一半，
        // 也远超我们测试里给的短超时）——服务必须还活着
        std::thread::sleep(Duration::from_millis(3000));
        let state = {
            let all = list_all(&handle);
            all[0].state
        };
        assert_eq!(
            state,
            ServiceState::Running,
            "烧 CPU 的长驻服务不得被限额杀掉"
        );
        let _ = stop_service(&handle, &out.id, "test");
    }
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
    assert!(!send_signal(0, SIGINT));
    assert!(!send_signal(0, SIGKILL));
    assert!(!signal_group(0, SIGKILL));
}

#[test]
fn head_and_tail_chars_split_without_breaking_utf8() {
    let s = "服务端已启动，监听 http://127.0.0.1:5173";
    assert_eq!(head_chars(s, 3), "服务端");
    assert_eq!(head_chars(s, 4), "服务端已");
    assert_eq!(tail_chars(s, 4), "5173");
    // 头尾都短于 n 时各自拿全量
    assert_eq!(head_chars("ab", 10), "ab");
    assert_eq!(tail_chars("ab", 10), "ab");
}

#[test]
fn every_allowlisted_launcher_has_a_construction_arm() {
    // 白名单与逐臂构造必须一一对应：白名单加了新启动器却没加构造分支，
    // 启动会在最后一刻才报"不在白名单内"（错误信息还指向白名单，误导）。
    for name in STDIO_LAUNCH_ALLOWLIST {
        assert!(
            build_command(name, &[], &std::env::temp_dir()).is_ok(),
            "白名单里的启动器「{name}」没有对应的逐臂构造分支"
        );
    }
}
