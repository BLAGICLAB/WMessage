//! 受管服务层的纯函数与集成测试。
//!
//! 进程树相关的纯函数（快照解析 / ppid 树收集 / PID 复用识别 / 整组杀护栏）
//! 搬到了 `bot/procs`（受管服务层与 MCP stdio 共用），那边的测试在
//! `bot/procs/tests.rs`。这里留下的是服务层自己的：登记表、状态机、
//! 日志截断、资源告警判定，以及真进程的集成用例。

use super::*;
use crate::bot::procs::pid_exists;

use tauri::Manager;

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

// ── 资源观测 ──

#[test]
fn resource_verdict_flags_rss_over_limit() {
    let sample = ResourceSample {
        rss_bytes: RSS_WARN_BYTES + 1,
        cpu_seconds: 1.0,
    };
    let v = evaluate_resources(sample, None, None, &[], 1_000);
    assert!(v.warnings.contains(&WARN_RSS.to_string()));
    assert_eq!(v.new_warnings, vec![WARN_RSS.to_string()]);
}

#[test]
fn resource_verdict_stays_quiet_below_thresholds() {
    // 正常 vite dev server 那一档（145MB）
    let sample = ResourceSample {
        rss_bytes: 145 * 1024 * 1024,
        cpu_seconds: 10.0,
    };
    let v = evaluate_resources(sample, Some((0.0, 0)), None, &[], 30_000);
    assert!(v.warnings.is_empty(), "正常服务不该告警：{:?}", v.warnings);
    assert!(v.new_warnings.is_empty());
}

#[test]
fn resource_verdict_needs_sustained_cpu_before_warning() {
    // 累计 CPU 时间每次涨 30s，墙上时间也涨 30s → 占用率恒为 100%（一个核跑满）
    let sample = ResourceSample {
        rss_bytes: 10 * 1024 * 1024,
        cpu_seconds: 30.0,
    };
    // 第一次采样：只记起点，不告警
    let v1 = evaluate_resources(sample, Some((0.0, 0)), None, &[], 30_000);
    assert!((v1.cpu_pct.unwrap() - 100.0).abs() < 0.01);
    assert!(!v1.warnings.contains(&WARN_CPU.to_string()));
    assert_eq!(v1.cpu_busy_since_ms, Some(30_000));
    // 持续不到 5 分钟：仍不告警
    let v2 = evaluate_resources(
        ResourceSample {
            cpu_seconds: 60.0,
            ..sample
        },
        Some((30.0, 30_000)),
        v1.cpu_busy_since_ms,
        &v1.warnings,
        60_000,
    );
    assert!(!v2.warnings.contains(&WARN_CPU.to_string()));
    // 持续超过 5 分钟：告警
    let v3 = evaluate_resources(
        ResourceSample {
            cpu_seconds: 330.0,
            ..sample
        },
        Some((60.0, 60_000)),
        v2.cpu_busy_since_ms,
        &v2.warnings,
        330_001,
    );
    assert!(v3.warnings.contains(&WARN_CPU.to_string()));
    assert_eq!(v3.new_warnings, vec![WARN_CPU.to_string()]);
}

#[test]
fn resource_verdict_resets_cpu_busy_when_idle_again() {
    let busy = ResourceSample {
        rss_bytes: 0,
        cpu_seconds: 30.0,
    };
    let v1 = evaluate_resources(busy, Some((0.0, 0)), None, &[], 30_000);
    assert_eq!(v1.cpu_busy_since_ms, Some(30_000));
    // 掉回空闲 → 起点清零（"持续"被打断就不算持续）
    let idle = ResourceSample {
        rss_bytes: 0,
        cpu_seconds: 30.1,
    };
    let v2 = evaluate_resources(
        idle,
        Some((30.0, 30_000)),
        v1.cpu_busy_since_ms,
        &[],
        60_000,
    );
    assert_eq!(v2.cpu_busy_since_ms, None);
    assert!(!v2.warnings.contains(&WARN_CPU.to_string()));
}

#[test]
fn resource_verdict_dedups_repeated_warning() {
    let sample = ResourceSample {
        rss_bytes: RSS_WARN_BYTES + 1,
        cpu_seconds: 1.0,
    };
    let v1 = evaluate_resources(sample, None, None, &[], 1_000);
    assert_eq!(v1.new_warnings.len(), 1);
    // 条件没消失：不再重复报（审计不刷屏）
    let v2 = evaluate_resources(sample, None, None, &v1.warnings, 31_000);
    assert!(v2.new_warnings.is_empty());
    assert!(v2.warnings.contains(&WARN_RSS.to_string()));
}

#[test]
fn sample_resources_records_rss_of_a_running_service() {
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
    sample_resources(&handle);

    let svc = list_all(&handle)
        .into_iter()
        .find(|s| s.id == out.id)
        .expect("服务应在登记表里");
    let sample = svc.sample.expect("在跑的服务应采到资源占用");
    assert!(sample.rss_bytes > 0, "RSS 必须采到非零值：{sample:?}");
    assert!(sample.cpu_seconds >= 0.0);

    // 合计口径也有值
    {
        let usage = crate::app_state::services_usage(&handle);
        let u = usage.lock().unwrap_or_else(|e| e.into_inner());
        assert!(u.total_rss_bytes >= sample.rss_bytes);
        assert!(u.sampled_at_ms.is_some());
    }

    // JSON 里带得上（模型与设置页看的是这份）
    let json = services_snapshot_json(&handle, false);
    let first = &json["services"][0];
    assert!(first["rssMb"].as_f64().unwrap_or(0.0) > 0.0);
    assert!(first["warnings"].is_array());

    let _ = stop_service(&handle, &out.id, "test");
}

#[test]
fn resource_warning_survives_missing_notification_plugin() {
    // mock app 没装通知插件 → 系统通知发不出去，但绝不能炸：
    // 巡检循环跑在后台，一次 panic 就等于整个观测停了
    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    app.manage(crate::app_state::AppState::default());
    write_resource_warning(
        &handle,
        "svc_test",
        "service",
        "测试服务",
        WARN_RSS,
        RSS_WARN_BYTES + 1,
        None,
    );
    write_resource_warning(
        &handle,
        "svc_test",
        "service",
        "测试服务",
        WARN_CPU,
        1024,
        Some(120.0),
    );
    write_resource_warning(
        &handle,
        "*",
        "total",
        "全部受管服务",
        WARN_TOTAL_RSS,
        RSS_TOTAL_WARN_BYTES + 1,
        None,
    );
}
