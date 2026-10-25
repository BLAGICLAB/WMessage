//! 资源观测面：合计口径判定、采样一轮、告警落审计与系统通知。

use super::*;

/// 受管服务的**合计**资源观测状态（跨服务口径）。
///
/// 单服务的采样在各自登记条目里；合计口径是集合属性，没有"某一条"能放，
/// 单独存一份（运行期状态一律进 AppState，见 `app_state.rs` 的总表）。
#[derive(Debug, Clone, Default)]
pub(crate) struct ServicesUsage {
    /// 全部在跑服务合计 RSS（字节）
    pub(crate) total_rss_bytes: u64,
    /// 合计口径的告警项（条件消失前只告警一次）
    pub(crate) warnings: Vec<String>,
    /// 最近一次采样时刻
    pub(crate) sampled_at_ms: Option<i64>,
}

/// 资源告警判定结果。
#[derive(Debug, Clone, Default)]
pub(crate) struct ResourceVerdict {
    /// 判定后的告警集合（条件消失的项会被移除，条件再现时能重新告警）
    pub(crate) warnings: Vec<String>,
    /// 本轮**新出现**的告警（只有这些要写审计）
    pub(crate) new_warnings: Vec<String>,
    /// 新的 CPU 高占用起始时刻
    pub(crate) cpu_busy_since_ms: Option<i64>,
    /// 本轮算出的 CPU 占用率（百分比）
    pub(crate) cpu_pct: Option<f64>,
}

/// 资源告警判定（纯函数）。
///
/// 只告警**不杀**：CPU 上限对"正在构建的 dev server"是误杀（见设计稿 §5.1），
/// 内存也没有跨平台的硬限额（macOS 没有等价机制，Linux 上设 RLIMIT_AS 会让
/// node 启动即失败）。失控了出声，用户自己停。
pub(crate) fn evaluate_resources(
    sample: ResourceSample,
    last_cpu_sample: Option<(f64, i64)>,
    cpu_busy_since_ms: Option<i64>,
    prev_warnings: &[String],
    now_ms: i64,
) -> ResourceVerdict {
    // 占用率走公共公式（与 MCP 侧同一条口径）
    let cpu_pct = last_cpu_sample.and_then(|(prev_cpu, prev_at)| {
        crate::bot::procs::cpu_usage_pct(prev_cpu, prev_at, sample.cpu_seconds, now_ms)
    });

    // "持续"判定：占用率高就把起点钉住（第一次高时记为 now），掉回阈值以下就清零
    let mut busy_since = cpu_busy_since_ms;
    if let Some(pct) = cpu_pct {
        if pct >= CPU_WARN_PCT {
            busy_since = Some(busy_since.unwrap_or(now_ms));
        } else {
            busy_since = None;
        }
    }
    let cpu_warn = cpu_pct.map(|p| p >= CPU_WARN_PCT).unwrap_or(false)
        && busy_since
            .map(|s| now_ms - s >= CPU_WARN_SUSTAIN_MS)
            .unwrap_or(false);

    let mut warnings: Vec<String> = Vec::new();
    if sample.rss_bytes >= RSS_WARN_BYTES {
        warnings.push(WARN_RSS.to_string());
    }
    if cpu_warn {
        warnings.push(WARN_CPU.to_string());
    }
    let new_warnings = warnings
        .iter()
        .filter(|w| !prev_warnings.contains(w))
        .cloned()
        .collect();

    ResourceVerdict {
        warnings,
        new_warnings,
        cpu_busy_since_ms: busy_since,
        cpu_pct,
    }
}
/// 资源观测一轮：采样在跑服务的资源占用，更新告警状态，必要时记审计。
///
/// 口径是**整棵进程树**（组长 + 全部后代）合计：dev server 的活多半在子进程里
/// （vite 拉起的 esbuild），只看组长会漏掉真正在烧的那个。
///
/// 只告警不杀——理由见 `evaluate_resources`。
pub(crate) fn sample_resources<R: tauri::Runtime>(app: &AppHandle<R>) {
    let snap = take_snapshot();
    let now = crate::db::now_ms();
    let table = crate::app_state::services(app);
    let entries: Vec<ServiceEntry> = {
        let guard = lock_table(&table);
        guard.values().cloned().collect()
    };
    let mut total_rss = 0u64;
    for entry in entries {
        let (state, pid) = {
            let g = entry.lock().unwrap_or_else(|e| e.into_inner());
            (g.state, g.pid)
        };
        if state != ServiceState::Running {
            continue;
        }
        // 组长不在快照里 = 进程已经没了（这一轮采不到它）
        let Some(sample) = tree_sample(&snap, pid) else {
            continue;
        };
        total_rss = total_rss.saturating_add(sample.rss_bytes);

        let mut g = entry.lock().unwrap_or_else(|e| e.into_inner());
        let prev_warnings = g.warnings.clone();
        let verdict = evaluate_resources(
            sample,
            g.last_cpu_sample,
            g.cpu_busy_since_ms,
            &prev_warnings,
            now,
        );
        let service_id = g.id.clone();
        let label = if g.name.is_empty() {
            g.command.clone()
        } else {
            g.name.clone()
        };
        g.sample = Some(sample);
        g.cpu_pct = verdict.cpu_pct;
        g.last_cpu_sample = Some((sample.cpu_seconds, now));
        g.cpu_busy_since_ms = verdict.cpu_busy_since_ms;
        g.warnings = verdict.warnings.clone();
        drop(g);

        for w in &verdict.new_warnings {
            write_resource_warning(
                app,
                &service_id,
                "service",
                &label,
                w,
                sample.rss_bytes,
                verdict.cpu_pct,
            );
        }
        // 条件消失的告警：通知中心里那条也收掉，否则同一条告警反复出现会
        // 在角标里堆成一摞（每次出现是新一集，旧的不撤就都留着）。
        for w in prev_warnings
            .iter()
            .filter(|w| !verdict.warnings.contains(*w))
        {
            if let Err(e) = crate::notifications::resolve_service_alert(
                app,
                &service_id,
                ALERT_RESOURCE,
                Some(w),
            ) {
                eprintln!("[bot::services] 资源告警撤收失败：{e}");
            }
        }
    }

    // 合计口径：跨服务的集合属性，没有归属条目
    let usage = crate::app_state::services_usage(app);
    let mut u = usage.lock().unwrap_or_else(|e| e.into_inner());
    // 本平台采不到进程快照 → 合计恒为 0。那不是「没占用」，是「没判」：
    // 不发布这个数，也不拿它比阈值（否则永远判成「没超」，阈值告警永不触发）。
    let Some(total_warn) = total_rss_warning(total_rss, snapshot_supported()) else {
        u.total_rss_bytes = 0;
        u.sampled_at_ms = None;
        drop(u);
        return;
    };
    u.total_rss_bytes = total_rss;
    u.sampled_at_ms = Some(now);
    let already = u.warnings.iter().any(|w| w == WARN_TOTAL_RSS);
    if total_warn {
        if !already {
            u.warnings.push(WARN_TOTAL_RSS.to_string());
        }
    } else {
        u.warnings.retain(|w| w != WARN_TOTAL_RSS);
    }
    drop(u);
    if total_warn && !already {
        write_resource_warning(
            app,
            "*",
            "total",
            "全部受管服务",
            WARN_TOTAL_RSS,
            total_rss,
            None,
        );
    } else if !total_warn && already {
        // 合计口径的条件消失了：通知中心那条一起收掉
        if let Err(e) = crate::notifications::resolve_service_alert(
            app,
            "*",
            ALERT_RESOURCE,
            Some(WARN_TOTAL_RSS),
        ) {
            eprintln!("[bot::services] 合计资源告警撤收失败：{e}");
        }
    }
}

/// 记一条资源告警：审计 + 系统通知。
///
/// 系统通知是给「用户不在设置页前面」准备的——面板上的告警只有盯着才看得见，
/// 而资源跑飞属于「等用户发现时机器已经卡了」那类事。
pub(super) fn write_resource_warning<R: tauri::Runtime>(
    app: &AppHandle<R>,
    service_id: &str,
    scope: &str,
    label: &str,
    kind: &str,
    rss_bytes: u64,
    cpu_pct: Option<f64>,
) {
    let rss_mb = rss_bytes as f64 / 1024.0 / 1024.0;
    let (threshold, body) = match kind {
        WARN_RSS => (
            "rss>512MB",
            format!("{label}内存 {rss_mb:.0} MB（阈值 512 MB）"),
        ),
        WARN_CPU => (
            "cpu>95%_for_5min",
            format!(
                "{label} CPU 持续跑满（>95% 持续 5 分钟，当前 {:.0}%）",
                cpu_pct.unwrap_or(0.0)
            ),
        ),
        _ => (
            "total_rss>1024MB",
            format!("{label}合计内存 {rss_mb:.0} MB（阈值 1 GB）"),
        ),
    };
    let mut kv = vec![
        ("service_id", service_id.to_string()),
        ("scope", scope.to_string()),
        ("rss_mb", format!("{:.1}", rss_mb)),
        ("threshold", threshold.to_string()),
    ];
    if let Some(p) = cpu_pct {
        kv.push(("cpu_pct", format!("{:.1}", p)));
    }
    crate::audit::write_event(app, AuditLevel::Warn, "service.resource_warning", &kv);
    notify_system(app, "受管服务资源告警", &body);
    // 应用内通知中心也落一条：系统通知依赖通知插件，插件没装就静默跳过——
    // 那台机器上这条告警就只剩设置页里的一行字，用户不看就没有。
    let notif_id = format!("service:{service_id}:{kind}:{}", crate::db::now_ms());
    let payload = serde_json::json!({
        "serviceId": service_id,
        "alertType": ALERT_RESOURCE,
        "warning": kind,
        "scope": scope,
        "label": label,
    });
    if let Err(e) = crate::notifications::insert_service_alert(
        app,
        &notif_id,
        &format!("{label} · 资源告警"),
        &body,
        &payload,
    ) {
        eprintln!("[bot::services] 资源告警落通知中心失败：{e}");
    }
}

/// 发一条系统通知。
///
/// 插件没装就跳过（测试里的 mock app 没有这个插件）——通知发不出去不是业务错误，
/// 但也不该因此把巡检循环带崩。
fn notify_system<R: tauri::Runtime>(app: &AppHandle<R>, title: &str, body: &str) {
    use tauri::Manager;
    let Some(state) = app.try_state::<tauri_plugin_notification::Notification<R>>() else {
        return;
    };
    if let Err(e) = state.inner().builder().title(title).body(body).show() {
        eprintln!("[bot::services] 系统通知发送失败：{e}");
    }
}
