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

/// 起一个会待够久的子进程（当停止流程的目标）。
///
/// unix 用 `sleep`，Windows 用 ping 计时——两边都能撑过停止流程的等待窗口。
fn spawn_sleeper() -> std::process::Child {
    #[cfg(unix)]
    let mut cmd = {
        let mut c = std::process::Command::new("sleep");
        c.arg("30");
        c
    };
    #[cfg(windows)]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "ping -n 31 127.0.0.1 >NUL"]);
        c
    };
    cmd.spawn().expect("应能起子进程")
}

#[test]
fn stop_that_cannot_confirm_exit_reports_failure_and_keeps_the_service_active() {
    // 「停不掉」不许被写成「已停」：写 Exited 等于把它从活动集合里摘掉
    // （`is_active` 过滤），而退出时它还得再收一次。
    //
    // 造一个收不到退出确认的服务：进程真的在跑，但**没有 watcher 线程**
    // ——登记表永远等不到「进程退了」这个事实。
    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    app.manage(crate::app_state::AppState::default());

    let mut child = spawn_sleeper();
    let pid = child.id();

    let entry: ServiceEntry = Arc::new(Mutex::new(ManagedService::new(
        "svc_stuck".to_string(),
        String::new(),
        "python3".to_string(),
        Vec::new(),
        std::env::temp_dir(),
        pid,
        pid as i32,
        PathBuf::new(),
        PathBuf::new(),
        None,
    )));
    entry.lock().unwrap_or_else(|e| e.into_inner()).state = ServiceState::Running;

    let r = stop_entry(&handle, &entry, "test");
    assert!(r.is_err(), "停不掉必须报错，不能回报「已停止」");

    let g = entry.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(
        g.state,
        ServiceState::Running,
        "停止失败时状态必须放回原样——写 Exited 等于宣告它不用收了"
    );
    assert!(
        g.state.is_active(),
        "还在跑的服务必须留在活动集合里（退出时还要再收一次）"
    );

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn stop_of_a_service_that_never_started_reports_no_process() {
    // pid 0 = spawn 前的占位条目：没有进程可停，如实说「没有进程」，
    // 而不是「已停止」。更关键的是：**不许牵连别的进程**——
    // 拿 pid 0 当树根会把 launchd 的整棵子树（全机用户进程）收进来。
    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    app.manage(crate::app_state::AppState::default());

    let entry: ServiceEntry = Arc::new(Mutex::new(ManagedService::new(
        "svc_never_started".to_string(),
        String::new(),
        "python3".to_string(),
        Vec::new(),
        std::env::temp_dir(),
        0,
        0,
        PathBuf::new(),
        PathBuf::new(),
        None,
    )));
    entry.lock().unwrap_or_else(|e| e.into_inner()).state = ServiceState::Starting;

    let outcome = stop_entry(&handle, &entry, "test").expect("没有进程可停不报错");
    assert_eq!(outcome.method, "no_process");
    assert!(outcome.stopped, "本来就没在跑，等于「已不在运行」");
    assert_eq!(
        entry.lock().unwrap_or_else(|e| e.into_inner()).state,
        ServiceState::Exited,
        "没有进程的条目记成已退出（它本来就不在跑）"
    );
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

// ── P2-③：端口 / 监听地址闸 ──

#[test]
fn parse_listen_flags_reads_common_forms() {
    let args = |v: &[&str]| -> Vec<String> { v.iter().map(|s| s.to_string()).collect() };

    // --port 5173
    let f = parse_listen_flags(&args(&["--port", "5173"]));
    assert_eq!(f.port, Some(5173));
    assert_eq!(f.host, None);

    // --port=5173
    let f = parse_listen_flags(&args(&["--port=5173"]));
    assert_eq!(f.port, Some(5173));

    // -p 5173
    let f = parse_listen_flags(&args(&["-p", "5173"]));
    assert_eq!(f.port, Some(5173));

    // --host 0.0.0.0
    let f = parse_listen_flags(&args(&["--host", "0.0.0.0"]));
    assert_eq!(f.host.as_deref(), Some("0.0.0.0"));

    // --host=127.0.0.1
    let f = parse_listen_flags(&args(&["--host=127.0.0.1"]));
    assert_eq!(f.host.as_deref(), Some("127.0.0.1"));

    // --listen 0.0.0.0:8080（地址与端口一起给）
    let f = parse_listen_flags(&args(&["--listen", "0.0.0.0:8080"]));
    assert_eq!(f.host.as_deref(), Some("0.0.0.0"));
    assert_eq!(f.port, Some(8080));

    // --bind 127.0.0.1（只有地址）
    let f = parse_listen_flags(&args(&["--bind", "127.0.0.1"]));
    assert_eq!(f.host.as_deref(), Some("127.0.0.1"));
    assert_eq!(f.port, None);

    // 无关参数不误伤
    let f = parse_listen_flags(&args(&["--verbose", "--outdir", "/tmp"]));
    assert_eq!(f.host, None);
    assert_eq!(f.port, None);
}

#[test]
fn parse_listen_flags_ignores_a_trailing_flag_without_value() {
    let args = vec!["--port".to_string()];
    let f = parse_listen_flags(&args);
    assert_eq!(f.port, None);
}

#[test]
fn is_loopback_tells_wildcard_apart() {
    assert!(is_loopback("127.0.0.1"));
    assert!(is_loopback("127.1.2.3"));
    assert!(is_loopback("::1"));
    // 通配不是 loopback——这正是 L1 要拦的
    assert!(!is_loopback("0.0.0.0"));
    assert!(!is_loopback("::"));
    assert!(!is_loopback("192.168.1.10"));
}

#[test]
fn judge_sockets_flags_non_loopback_bind() {
    let socks = vec![ListeningSocket {
        protocol: crate::bot::procs::SocketProtocol::Tcp,
        address: "0.0.0.0".to_string(),
        port: 7000,
    }];
    let reserved = HashSet::new();
    match judge_sockets(&socks, true, &reserved, false) {
        GateVerdict::Violated {
            rule,
            address,
            port,
        } => {
            assert_eq!(rule, ViolationRule::NonLoopback);
            assert_eq!(address, "0.0.0.0");
            assert_eq!(port, 7000);
        }
        other => panic!("应判 L1 违规，实际 {other:?}"),
    }
    // 开关放开 → 不再报 L1
    assert_eq!(
        judge_sockets(&socks, true, &reserved, true),
        GateVerdict::Ok
    );
}

#[test]
fn judge_sockets_flags_reserved_and_privileged_ports() {
    let mut reserved = HashSet::new();
    reserved.insert(4763u16);
    let mk = |port: u16, addr: &str| ListeningSocket {
        protocol: crate::bot::procs::SocketProtocol::Tcp,
        address: addr.to_string(),
        port,
    };

    // L2：占了宿主保留端口（地址是 loopback 也照样占）
    match judge_sockets(&[mk(4763, "127.0.0.1")], true, &reserved, false) {
        GateVerdict::Violated { rule, port, .. } => {
            assert_eq!(rule, ViolationRule::ReservedPort);
            assert_eq!(port, 4763);
        }
        other => panic!("应判 L2 违规，实际 {other:?}"),
    }

    // L3：特权端口
    match judge_sockets(&[mk(80, "127.0.0.1")], true, &reserved, false) {
        GateVerdict::Violated { rule, port, .. } => {
            assert_eq!(rule, ViolationRule::PrivilegedPort);
            assert_eq!(port, 80);
        }
        other => panic!("应判 L3 违规，实际 {other:?}"),
    }

    // 普通 loopback 端口 → 没违规
    assert_eq!(
        judge_sockets(&[mk(5173, "127.0.0.1")], true, &reserved, false),
        GateVerdict::Ok
    );
}

#[test]
fn judge_sockets_unsupported_when_sample_failed() {
    // 采集失败（lsof 起不来 / /proc/net/* 不可读）与「确实没人在监听」
    // 在类型层必须分开：前者是**没判**，不是「判了没事」。
    let reserved = HashSet::new();
    // 采空 + 采成功 → Ok（真没人在监听）
    assert_eq!(
        judge_sockets(&[], true, &reserved, false),
        GateVerdict::Ok,
        "采成功但没采到 = 判了没事"
    );
    // 采空 + 采失败 → Unsupported（没判）
    assert_eq!(
        judge_sockets(&[], false, &reserved, false),
        GateVerdict::Unsupported,
        "采集失败不得判成 Ok"
    );
    // 采样失败时即便采到东西也不判——那批数据不可信
    let socks = vec![ListeningSocket {
        protocol: crate::bot::procs::SocketProtocol::Tcp,
        address: "0.0.0.0".to_string(),
        port: 7000,
    }];
    assert_eq!(
        judge_sockets(&socks, false, &reserved, false),
        GateVerdict::Unsupported,
        "采样失败时不得据其判违规"
    );
}

#[test]
fn reserved_dynamic_survives_a_failed_sample() {
    // 采样失败时保留集动态部分**不清空**：清空了 L2（防抢占宿主端口）
    // 就跟着一起静默失效了。
    refresh_reserved_ports(&[52001], true);
    assert!(reserved_ports().contains(&52001));
    refresh_reserved_ports(&[], false);
    assert!(
        reserved_ports().contains(&52001),
        "采样失败不得把动态保留集清空"
    );
    // 采样恢复后正常刷新
    refresh_reserved_ports(&[52002], true);
    assert!(reserved_ports().contains(&52002));
    assert!(!reserved_ports().contains(&52001));
}

// ── 采集金丝雀：采样自称成功却看不见宿主必在监听的端口 = 采集失明 ──

fn canary_sock(pid: u32, port: u16) -> (u32, Vec<ListeningSocket>) {
    (
        pid,
        vec![ListeningSocket {
            protocol: crate::bot::procs::SocketProtocol::Tcp,
            address: "127.0.0.1".to_string(),
            port,
        }],
    )
}

#[test]
fn canary_blind_separates_empty_from_blind() {
    use crate::bot::procs::ListeningSample;
    use std::collections::HashMap;

    let port = crate::api::API_PORT;
    // 采成功且看得见金丝雀端口 → 不失明
    let mut by_pid = HashMap::new();
    by_pid.insert(canary_sock(7, port).0, canary_sock(7, port).1);
    let seen = ListeningSample {
        by_pid,
        complete: true,
    };
    assert!(
        !canary_blind(&seen, Some(port)),
        "看得见宿主端口 = 采样可信"
    );

    // 采成功却看不见 → 失明（lsof 权限不足一类：不报错也不出行）
    let empty = ListeningSample {
        by_pid: HashMap::new(),
        complete: true,
    };
    assert!(
        canary_blind(&empty, Some(port)),
        "自称成功却看不见宿主必在监听的端口 = 采集失明，不得当「判了没事」"
    );

    // 采集失败（!complete）→ 金丝雀不重复报（那条路径已有自己的语义）
    let failed = ListeningSample {
        by_pid: HashMap::new(),
        complete: false,
    };
    assert!(
        !canary_blind(&failed, Some(port)),
        "采集失败路径已有审计，金丝雀不得双重上报"
    );

    // API 没开（无金丝雀基准）→ 永不失明（不制造误报源）
    assert!(!canary_blind(&empty, None), "没有基准就不判失明");
}

#[test]
fn api_canary_port_tracks_running_api_liveness() {
    use crate::api_server::{ApiState, RunningApi};

    // 未 manage ApiState → 无金丝雀
    let app = tauri::test::mock_app();
    assert_eq!(api_canary_port(app.handle()), None);

    // manage 了但没起服务 → 无金丝雀
    app.manage(ApiState::default());
    assert_eq!(api_canary_port(app.handle()), None);

    // 活的 accept 线程 → 有金丝雀（宿主此刻必在监听 API_PORT）
    let alive = ApiState(std::sync::Mutex::new(Some(RunningApi {
        shutdown: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        handle: Some(std::thread::spawn(std::thread::park)),
        token: "t".to_string(),
        active: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    })));
    let app2 = tauri::test::mock_app();
    app2.manage(alive);
    assert_eq!(
        api_canary_port(app2.handle()),
        Some(crate::api::API_PORT),
        "API 活着时金丝雀端口必须在场"
    );

    // accept 线程已死（is_finished）→ 不算在监听。
    // join 会消费句柄，等退出只能轮询 is_finished（&self）
    let dead_handle = std::thread::spawn(|| {});
    while !dead_handle.is_finished() {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let dead = ApiState(std::sync::Mutex::new(Some(RunningApi {
        shutdown: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        handle: Some(dead_handle),
        token: "t".to_string(),
        active: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    })));
    let app3 = tauri::test::mock_app();
    app3.manage(dead);
    assert_eq!(api_canary_port(app3.handle()), None);
}

/// 巡检必须先过金丝雀再进判定：canary_blind 的结论要汇入 complete，
/// 否则金丝雀只是旁路日志、判定层照样把失明采样当「判了没事」。
/// 巡检在 AppHandle 上跑（Wry 专属），单测直驱不了——源码文本守卫钉接线。
#[test]
fn patrol_wires_canary_before_judging() {
    let src = include_str!("../services.rs");
    let prod = src
        .split_once("#[cfg(test)]")
        .map(|(p, _)| p)
        .unwrap_or(src);
    let patrol = prod
        .split_once("fn enforce_listening_gate")
        .expect("应能定位巡检函数");
    let patrol = patrol.1;
    let canary = patrol
        .find("canary_blind(&sample, canary_port)")
        .expect("巡检必须调金丝雀判定");
    let note = patrol
        .find("note_sample_status")
        .expect("巡检必须记录采样状态");
    assert!(
        canary < note,
        "金丝雀必须在采样状态记录之前跑（失明要汇入 complete，不是旁路日志）"
    );
}

#[test]
fn judge_flags_mirrors_the_runtime_rules() {
    let mut reserved = HashSet::new();
    reserved.insert(4763u16);

    let f = FlagListen {
        host: Some("0.0.0.0".to_string()),
        port: Some(5173),
    };
    let (rule, _, _) = judge_flags(&f, &reserved, false).expect("应判 L1");
    assert_eq!(rule, ViolationRule::NonLoopback);
    assert!(judge_flags(&f, &reserved, true).is_none(), "开关放开则不报");

    let f = FlagListen {
        host: Some("127.0.0.1".to_string()),
        port: Some(4763),
    };
    let (rule, _, port) = judge_flags(&f, &reserved, false).expect("应判 L2");
    assert_eq!(rule, ViolationRule::ReservedPort);
    assert_eq!(port, 4763);

    let f = FlagListen {
        host: Some("127.0.0.1".to_string()),
        port: Some(80),
    };
    let (rule, _, port) = judge_flags(&f, &reserved, false).expect("应判 L3");
    assert_eq!(rule, ViolationRule::PrivilegedPort);
    assert_eq!(port, 80);
}

#[test]
fn reserved_ports_always_hold_the_host_api_port() {
    // 静态保底：宿主 API server 可选启停，4763 不在监听时也得保留
    assert!(reserved_ports().contains(&crate::api::API_PORT));
}

/// 找一个空闲端口（先占 :0 拿到端口即释放；与别的测试不撞固定端口）。
fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("应能绑一个临时端口");
    let p = l.local_addr().expect("应有本地地址").port();
    drop(l);
    p
}

/// 启动期绑到非 loopback → 端口闸在启动期就杀组，错误里带规则与改绑示例。
///
/// 这条同时钉住「启动期也判」：bind 落定多在 ready 之前，只认 Running 的话
/// 启动期加密采样整轮空转，闸就漏了最危险的那一段窗口。
#[test]
fn startup_service_bound_to_non_loopback_is_killed_with_violation_reason() {
    if !crate::bot::procs::listening_supported() {
        eprintln!("跳过：本平台采不到监听 socket，闸不生效");
        return;
    }
    let Some((py, _)) = long_running_launcher() else {
        eprintln!("跳过：环境里没有 python3");
        return;
    };
    let port = free_port();
    let script = format!(
        "import socket, time\ns = socket.socket()\n\
         s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)\n\
         s.bind(('0.0.0.0', {port}))\n\
         s.listen(8)\n\
         print('bound', flush=True)\n\
         while True: time.sleep(0.2)\n"
    );
    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    app.manage(crate::app_state::AppState::default());

    let req = StartRequest {
        command: py,
        args: vec!["-c".to_string(), script],
        cwd: std::env::temp_dir(),
        name: "l1".to_string(),
        // 永远匹配不到的 readyPattern：让启动停在 Starting，等端口闸来判
        ready_pattern: Some("__never_matches__".to_string()),
        ready_timeout_secs: 10,
        session_id: None,
    };
    let err = do_start(&handle, &req).expect_err("非 loopback 监听应被端口闸终止");
    assert!(err.contains("端口闸"), "错误要说清是端口闸终止：{err}");
    assert!(err.contains("loopback"), "错误要带 L1 规则：{err}");
    assert!(err.contains("127.0.0.1"), "错误要带改绑示例：{err}");

    // 违规进了登记表（面板 badge 的数据源），状态机本身不动
    let all = list_all(&handle);
    assert_eq!(all.len(), 1);
    let v = all[0]
        .last_violation
        .as_ref()
        .expect("登记表要记下这次违规");
    assert_eq!(v.rule, ViolationRule::NonLoopback);
    assert_eq!(v.port, port);
    assert_eq!(v.action, ViolationAction::Killed);
    assert_eq!(all[0].state, ServiceState::Exited);

    // 通知中心也要有：服务被杀是用户必须知道的事，审计日志与设置页那行字
    // 都要用户主动去看
    let alerts = service_alerts_in_center(&handle);
    let hit = alerts
        .iter()
        .find(|v| v["serviceId"] == serde_json::json!(all[0].id))
        .expect("通知中心要有这条端口违规");
    assert_eq!(hit["alertType"], serde_json::json!("port_violation"));
    assert_eq!(hit["rule"], serde_json::json!("L1"));
    assert_eq!(hit["port"], serde_json::json!(port));
}

/// 通知中心里的服务告警（kind = service_alert）。
fn service_alerts_in_center<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Vec<serde_json::Value> {
    let conn = crate::db::open_db(app).expect("应能打开库");
    let mut stmt = conn
        .prepare("SELECT payload FROM notifications WHERE kind = ?1")
        .expect("应能查 notifications");
    let rows = stmt
        .query_map([crate::notifications::KIND_SERVICE_ALERT], |r| {
            r.get::<_, String>(0)
        })
        .expect("应能查 payload");
    rows.filter_map(|r| r.ok())
        .filter_map(|p| serde_json::from_str::<serde_json::Value>(&p).ok())
        .collect()
}

/// 资源告警要落到应用内通知中心。
///
/// 系统通知（`notify_system`）依赖通知插件，插件没装就静默跳过——那台机器上
/// 这条告警就只剩设置页里的一行字，用户不看就没有。通知中心走自己的库，
/// 永远在。
#[test]
fn resource_warning_lands_in_the_notification_center() {
    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    app.manage(crate::app_state::AppState::default());
    let service_id = format!(
        "svc_test_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );

    write_resource_warning(
        &handle,
        &service_id,
        "service",
        "测试服务",
        WARN_RSS,
        700 * 1024 * 1024,
        None,
    );

    let alerts = service_alerts_in_center(&handle);
    let hit = alerts
        .iter()
        .find(|v| v["serviceId"] == serde_json::json!(service_id))
        .expect("资源告警应落到通知中心");
    assert_eq!(hit["alertType"], serde_json::json!("resource_warning"));
    assert_eq!(hit["warning"], serde_json::json!(WARN_RSS));
    assert_eq!(hit["label"], serde_json::json!("测试服务"));
}

// ── 就绪失败的归因（先判后停） ──

/// 造一个「还活着」的服务条目（就绪等待超时前的状态）。
fn alive_entry() -> ServiceEntry {
    Arc::new(Mutex::new(ManagedService::new(
        "svc_ready".to_string(),
        String::new(),
        "python".to_string(),
        Vec::new(),
        std::env::temp_dir(),
        4242,
        4242,
        PathBuf::new(),
        PathBuf::new(),
        None,
    )))
}

/// 回归：就绪超时不得被说成「启动后立即退出」。
///
/// 顺序是语义的一部分：**先判定、后停止**。停了之后状态必然是 Exited、
/// 退出码必然是停止手段的（Windows 上 `taskkill /F` 的退出码就是 1）——
/// 那时再读状态，「我们超时杀的」就会被说成「它启动后立即退出（退出码 1）」。
#[test]
fn ready_timeout_is_not_reported_as_exited_early() {
    let entry = alive_entry();
    {
        let mut g = entry.lock().unwrap_or_else(|e| e.into_inner());
        g.state = ServiceState::Starting;
    }

    // 模拟停止：taskkill 之后状态变 Exited、退出码变 1（Windows 上的真实签名）
    let snapshot = resolve_ready_failure(&entry, || {
        let mut g = entry.lock().unwrap_or_else(|e| e.into_inner());
        g.state = ServiceState::Exited;
        g.exit_code = Some(1);
    });

    assert!(
        !snapshot.exited_early,
        "停止之前它还活着——不能判成「启动后立即退出」"
    );
    assert!(
        snapshot.exit_code.is_none(),
        "停止之前没有退出码：{:?}",
        snapshot.exit_code
    );

    let failure = classify_ready_failure(&ReadyOutcome::TimedOut, &snapshot, None, 5, "已停止");
    let msg = ready_failure_message("python", &failure, "");
    assert!(
        msg.contains("在 5s 内未就绪"),
        "就绪超时的文案要说清是超时：{msg}"
    );
    assert!(
        !msg.contains("退出码"),
        "就绪超时不是退出码问题（那个 1 是我们 taskkill 出来的）：{msg}"
    );
    assert!(
        !msg.contains("立即退出"),
        "就绪超时不是「启动后立即退出」：{msg}"
    );
}

/// 回归：进程自己退了 → 报它**自己的**退出码。
#[test]
fn exited_early_keeps_the_process_own_exit_code() {
    let entry = alive_entry();
    {
        let mut g = entry.lock().unwrap_or_else(|e| e.into_inner());
        g.state = ServiceState::Exited;
        g.exit_code = Some(3);
    }

    let snapshot = resolve_ready_failure(&entry, || {
        // 进程已经退了，停止是空操作（already_exited）
    });
    assert!(snapshot.exited_early, "它自己退了");
    assert_eq!(snapshot.exit_code, Some(3), "退出码是它自己的");

    let failure = classify_ready_failure(&ReadyOutcome::Exited, &snapshot, None, 5, "已停止");
    let msg = ready_failure_message("python", &failure, "");
    assert!(msg.contains("启动后立即退出"), "{msg}");
    assert!(msg.contains("退出码 3"), "{msg}");
}

/// 端口闸违规致死：文案保持原样（规则 + 地址 + 改绑示例），优先于「没就绪」。
#[test]
fn port_violation_message_wins_over_timeout() {
    let violation = Violation {
        rule: ViolationRule::NonLoopback,
        address: "0.0.0.0".to_string(),
        port: 8080,
        at_ms: 0,
        action: ViolationAction::Killed,
    };
    let snapshot = ReadyFailureSnapshot {
        exited_early: false,
        exit_code: None,
    };
    let failure = classify_ready_failure(
        &ReadyOutcome::TimedOut,
        &snapshot,
        Some(&violation),
        5,
        "已停止",
    );
    let msg = ready_failure_message("python", &failure, "");
    assert!(msg.contains("已被端口闸终止"), "{msg}");
    assert!(msg.contains("L1"), "{msg}");
    assert!(msg.contains("127.0.0.1"), "要给出改绑示例：{msg}");
}

/// readyPattern 不是合法正则：说清是模式写错了，不是「没就绪」。
#[test]
fn bad_ready_pattern_is_not_reported_as_timeout() {
    let snapshot = ReadyFailureSnapshot {
        exited_early: false,
        exit_code: None,
    };
    let failure = classify_ready_failure(
        &ReadyOutcome::BadPattern("repetition quantifier expects a valid decimal".to_string()),
        &snapshot,
        None,
        30,
        "已停止",
    );
    let msg = ready_failure_message("python", &failure, "");
    assert!(msg.contains("readyPattern"), "{msg}");
    assert!(!msg.contains("未就绪"), "模式非法不是超时：{msg}");
}

/// 回归：`service.exited` 必须带 stop_reason。
///
/// 退出码单独一个数分不出根因：Windows 上 `taskkill /F` 的退出码就是 1，
/// 和「服务自己挂了」撞脸——只看 exit_code=1 的人会以为服务自己退的。
#[test]
fn exited_audit_line_distinguishes_our_kill_from_its_own_exit() {
    let kv = exited_event_kv(
        "svc_x",
        1234,
        Some(1),
        30_772,
        Some("ready_timeout".to_string()),
    );
    let get = |k: &str| kv.iter().find(|(key, _)| *key == k).map(|(_, v)| v.clone());
    assert_eq!(get("exit_code").as_deref(), Some("1"));
    assert_eq!(get("uptime_ms").as_deref(), Some("30772"));
    assert_eq!(
        get("stop_reason").as_deref(),
        Some("ready_timeout"),
        "是我们停的：要写明停它的原因"
    );

    // 它自己退的：没有 stop_reason → 写 none，不能留空
    let kv = exited_event_kv("svc_y", 1235, Some(3), 900, None);
    let get = |k: &str| kv.iter().find(|(key, _)| *key == k).map(|(_, v)| v.clone());
    assert_eq!(get("exit_code").as_deref(), Some("3"));
    assert_eq!(
        get("stop_reason").as_deref(),
        Some("none"),
        "自己退的写 none——不能留空让人以为字段丢了"
    );
}

// ── 资源采集能力：采不到 ≠ 没占用 ──

/// 回归：本平台采不到进程快照时，面板不能显示「合计内存 0 MB」。
///
/// 空不等于没占用——那是**没判**。Windows 上快照层走 ps / /proc，采不到，
/// 合计恒为 0；显示成「合计内存 0 MB」会让用户以为这些服务不占内存。
#[test]
fn total_rss_is_not_published_when_the_platform_cannot_sample() {
    // 采得到 → 给数
    assert_eq!(
        total_rss_mb(145 * 1024 * 1024, true),
        Some(145.0),
        "采得到时给合计内存"
    );
    // 采不到 → 不给数（给 0 等于把「没判」说成「没占用」）
    assert_eq!(total_rss_mb(0, false), None, "采不到就不给合计内存");
    assert_eq!(
        total_rss_mb(145 * 1024 * 1024, false),
        None,
        "采不到时即便有字节数也不给——那个数不是采来的"
    );
}

/// 回归：采不到时，合计 RSS 的阈值告警是**没判**，不是「没超」。
///
/// 拿恒为 0 的合计去比 1GB 阈值，等于把「这台机器采不到」写成「内存没超」，
/// 阈值告警在 Windows 上永远不响。
#[test]
fn total_rss_threshold_is_not_judged_when_nothing_can_be_sampled() {
    assert_eq!(
        total_rss_warning(0, false),
        None,
        "采不到不是「没超」——是没判"
    );
    assert_eq!(
        total_rss_warning(RSS_TOTAL_WARN_BYTES, false),
        None,
        "采不到时连超了也不判"
    );
    assert_eq!(total_rss_warning(0, true), Some(false), "采得到：没超");
    assert_eq!(
        total_rss_warning(RSS_TOTAL_WARN_BYTES, true),
        Some(true),
        "采得到：超了"
    );
}

/// 快照 JSON 必须声明本平台能不能采到资源（前端按它显示能力提示）。
#[test]
fn snapshot_json_declares_whether_resources_can_be_sampled() {
    let app = tauri::test::mock_app();
    app.manage(crate::app_state::AppState::default());
    let json = services_snapshot_json(app.handle(), false);
    assert_eq!(
        json.get("snapshotSupported"),
        Some(&serde_json::json!(crate::bot::procs::snapshot_supported())),
        "snapshotSupported 键名/类型不能变（前端按它显示「本平台不支持采集」）"
    );
    if !crate::bot::procs::snapshot_supported() {
        assert!(
            json["totalRssMb"].is_null(),
            "采不到时合计内存给 null，不给 0：{}",
            json["totalRssMb"]
        );
    }
}

/// 端到端复现「就绪超时被误报成启动后立即退出」：真 spawn、真等超时、真停。
///
/// 纯函数判定只能证明分类逻辑对；这条走完整流程——服务本身健康得很，
/// 只是 readyPattern 永远匹配不上，于是被我们停掉。文案必须说「未就绪」，
/// 不能说「启动后立即退出」（那会把模型引到「这个命令为什么自己挂了」）。
#[test]
fn ready_timeout_end_to_end_is_not_reported_as_exited_early() {
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
        name: "ready-timeout-demo".to_string(),
        ready_pattern: Some("__never_match__".to_string()),
        ready_timeout_secs: 2,
        session_id: None,
    };

    let err = do_start(&handle, &req).expect_err("就绪超时必须失败");
    assert!(err.contains("未就绪"), "就绪超时的文案要说清是超时：{err}");
    assert!(
        !err.contains("立即退出"),
        "就绪超时不是「启动后立即退出」（服务活得好好的，是我们停的）：{err}"
    );
    assert!(
        !err.contains("退出码"),
        "就绪超时不报退出码（那个码是我们停它时产生的，不是它自己的）：{err}"
    );
}

/// 回归：停不掉时不能报「已停止」。
///
/// 停止的真实结果要进文案：进程还在跑却说「已停止」，模型会以为服务已经
/// 收干净了（下一轮换个端口重试 → 撞端口占用）。
#[test]
fn timeout_message_reports_the_real_stop_result() {
    let snapshot = ReadyFailureSnapshot {
        exited_early: false,
        exit_code: None,
    };

    let stopped = classify_ready_failure(&ReadyOutcome::TimedOut, &snapshot, None, 5, "已停止");
    let msg = ready_failure_message("python", &stopped, "");
    assert!(msg.contains("在 5s 内未就绪，已停止"), "{msg}");

    let failed = classify_ready_failure(
        &ReadyOutcome::TimedOut,
        &snapshot,
        None,
        5,
        "停止失败：停不掉（进程仍在运行）",
    );
    let msg = ready_failure_message("python", &failed, "");
    assert!(msg.contains("未就绪"), "{msg}");
    assert!(!msg.contains("已停止"), "停不掉就不能说「已停止」：{msg}");
    assert!(msg.contains("进程仍在运行"), "要说清进程还在：{msg}");
}

/// 采集失败这件事本身要**留痕**且**如实报「没判」**：审计记一条 WARN，
/// 快照 JSON 的 `listeningSampleOk` 翻 false；且**只在「成功 → 失败」转折记一条**
/// ——巡检每 30s 一轮，失败时逐轮记会刷屏。
///
/// S07 卡的判据分三截守：面板那半由前端用例守（`ServicesPanel.test.tsx`
/// 的「采集失败：明说端口闸本轮未判」），「动态保留集不清空」由
/// `reserved_dynamic_survives_a_failed_sample` 守，这条守中间那截——
/// 失败有没有落到日志与状态里。
#[test]
fn failed_listening_sample_is_audited_once_and_reported_as_not_judged() {
    let app = tauri::test::mock_app();
    app.manage(crate::app_state::AppState::default());
    let handle = app.handle().clone();
    // 「本轮采成功」是进程级全局状态：退出前还回去，别把同进程后面的用例带进失败态
    struct SampleOkGuard(tauri::AppHandle<tauri::test::MockRuntime>);
    impl Drop for SampleOkGuard {
        fn drop(&mut self) {
            // 翻回成功不写审计（只有「成功 → 失败」的转折才记）
            note_sample_status(&self.0, true);
        }
    }
    let _guard = SampleOkGuard(handle.clone());

    let log_path = crate::db::data_dir(&handle).join("bot.log");
    note_sample_status(&handle, true); // 前置：上一轮采成功

    // 失败一轮 → 留痕。日志是共享的追加文件，只看新增的那段
    let before = file_len(&log_path);
    note_sample_status(&handle, false);
    let appended = read_from(&log_path, before);
    assert!(
        appended.contains("WARN | service.listening_sample_failed"),
        "采集失败必须留一条 WARN 审计，实际新增：{appended:?}"
    );

    // 状态如实报「没判」，不能报成「没事」
    let json = services_snapshot_json(&handle, false);
    assert_eq!(
        json.get("listeningSampleOk"),
        Some(&serde_json::json!(false)),
        "采集失败要报「本轮没判」，不能报成「没事」：{json}"
    );

    // 再失败一轮 → 不再记（不刷屏）
    let mid = file_len(&log_path);
    note_sample_status(&handle, false);
    let appended2 = read_from(&log_path, mid);
    assert!(
        !appended2.contains("service.listening_sample_failed"),
        "连续失败不该逐轮刷屏，实际新增：{appended2:?}"
    );
}

fn file_len(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// 读日志文件从 `from` 字节起新增的内容（探针的水位线口径：共享日志只能看增量）
fn read_from(path: &std::path::Path, from: u64) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    text.get(from as usize..).unwrap_or("").to_string()
}
