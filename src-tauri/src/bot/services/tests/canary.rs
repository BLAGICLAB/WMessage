//! 采集金丝雀：采样自称成功却看不见宿主必在监听的端口 = 采集失明。

use super::*;

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
    let src = include_str!("../gate.rs");
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
