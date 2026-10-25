//! 序列化面：服务条目 → JSON 与受管服务全景快照。

use super::*;

// ── 序列化 ──

/// 服务条目 → JSON（给模型看）。
pub(crate) fn service_to_json(s: &ManagedService, with_tail: bool) -> serde_json::Value {
    let mut obj = serde_json::json!({
        "serviceId": s.id,
        "name": s.name,
        "command": s.command,
        "args": s.args,
        "cwd": s.cwd.to_string_lossy(),
        "pid": s.pid,
        "state": s.state.as_str(),
        "startedAt": s.started_at_ms,
        "uptimeSecs": s.uptime_ms() / 1000,
        "exitCode": s.exit_code,
        "sessionId": s.owner_session,
    });
    if let Some(r) = &s.ready_snapshot {
        obj["readySnapshot"] = serde_json::Value::String(r.clone());
    }
    // 资源观测：整棵进程树合计（只告警不杀，见 evaluate_resources）
    if let Some(sample) = &s.sample {
        obj["rssMb"] = serde_json::json!(round1(sample.rss_bytes as f64 / 1024.0 / 1024.0));
    }
    if let Some(p) = s.cpu_pct {
        obj["cpuPct"] = serde_json::json!(round1(p));
    }
    obj["warnings"] = serde_json::json!(s.warnings);
    // 违规（正交于生命周期状态）：服务死了但违规记录还在，用户得知道它为什么死
    if let Some(v) = &s.last_violation {
        obj["violation"] = serde_json::json!({
            "rule": v.rule.as_str(),
            "address": v.address,
            "port": v.port,
            "at": v.at_ms,
            "action": v.action.as_str(),
            "detail": v.rule.explain(),
        });
    }
    if with_tail {
        obj["stdoutTail"] = serde_json::json!(tail_lines(&s.stdout_path, TAIL_LINES));
        obj["stderrTail"] = serde_json::json!(tail_lines(&s.stderr_path, TAIL_LINES));
    }
    obj
}

/// 保留一位小数（给模型看的数字不用小数点后一堆位）。
pub(super) fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// 本平台能不能采到进程资源占用（Windows 不能：快照层走 ps / /proc）。
///
/// 与 MCP 面板同一份判据（`procs::snapshot_supported`）。
pub(super) fn snapshot_supported() -> bool {
    crate::bot::procs::snapshot_supported()
}

/// 合计 RSS 告警该不该触发（纯函数，供测试钉住）。
///
/// **采不到时返回 None——不是「没超」**：本平台没有进程快照层，合计恒为 0，
/// 拿 0 去比阈值等于把「没判」写成「判了没事」，阈值告警就永远不响。
pub(super) fn total_rss_warning(total_rss: u64, snapshot_supported: bool) -> Option<bool> {
    if !snapshot_supported {
        return None;
    }
    Some(total_rss >= RSS_TOTAL_WARN_BYTES)
}

/// 合计内存能不能给（纯函数，供测试钉住）。
///
/// 采不到就不给——给 0 等于把「没判」说成「没占用」：面板写「合计内存 0 MB」
/// 会让用户以为这些服务不占内存，而真相是「这台机器采不到」。
pub(super) fn total_rss_mb(total_rss_bytes: u64, snapshot_supported: bool) -> Option<f64> {
    if !snapshot_supported {
        return None;
    }
    Some(round1(total_rss_bytes as f64 / 1024.0 / 1024.0))
}

/// 受管服务全景 → JSON（模型与设置页共用一份口径）。
///
/// 顶层带合计口径（合计 RSS 与合计告警）——单服务数据在 `services` 数组里。
pub(crate) fn services_snapshot_json<R: tauri::Runtime>(
    app: &AppHandle<R>,
    with_tail: bool,
) -> serde_json::Value {
    let list = list_all(app);
    let arr: Vec<serde_json::Value> = list.iter().map(|s| service_to_json(s, with_tail)).collect();
    let usage = crate::app_state::services_usage(app);
    let u = usage.lock().unwrap_or_else(|e| e.into_inner());
    let can_sample = snapshot_supported();
    serde_json::json!({
        "services": arr,
        // 合计内存：采不到时给 null——「没判」不是「没占用」，面板据此
        // 不显示「合计内存 0 MB」，改说「本平台采不到」。
        "totalRssMb": total_rss_mb(u.total_rss_bytes, can_sample),
        // 本平台能不能采到进程资源占用（Windows 不能）。面板据此说明
        // 「本平台暂不支持进程资源采集」，而不是让用户对着空白猜。
        "snapshotSupported": can_sample,
        "warnings": u.warnings,
        "sampledAt": u.sampled_at_ms,
        // 本平台能不能采到监听 socket。Windows 不能——面板据此说明
        // 「本平台暂不支持监听端口采集」，而不是让用户对着空白猜。
        "listeningSupported": crate::bot::procs::listening_supported(),
        // 最近一次采样是否成功。false = 采集路径存在但本次采不到——端口闸
        // 本轮**没判**（不是「判了没事」），面板必须说出来。
        "listeningSampleOk": LAST_SAMPLE_OK.load(Ordering::Relaxed),
    })
}
