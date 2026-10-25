//! 就绪失败的归因（先判后停）：退出码取停止前的快照。

use super::*;

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
