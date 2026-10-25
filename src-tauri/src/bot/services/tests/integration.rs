//! 集成用例：真起一个受管服务、真停它、真回收逃逸子孙。

use super::*;

// ── 集成：真进程 ──

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
