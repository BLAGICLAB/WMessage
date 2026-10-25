//! 资源观测：合计口径判定、采样一轮、采不到 ≠ 没占用。

use super::*;

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
