//! 任务卡截止日期系统通知：
//!
//! 30s 后台扫描（仿 bot_scheduler::start_scheduler），对活跃任务卡按截止时间发系统通知：
//! - 截止前 1 小时一条「任务即将截止」（任务创建时距截止已不足 1h 的，首次扫描立即补发）
//! - 截止时刻一条「任务已到截止时间」（截止后 24h 内有效；超 24h 的老任务不补发，
//!   直接标记已发——防升级/重启后对历史任务轰炸通知）
//!
//! 平台差异约定（macOS / Windows 系统通知行为不同，代码层面统一走插件 API）：
//! - macOS：系统通知必须显式授权（UNUserNotificationCenter），未授权时通知静默失败——
//!   授权请求在前端 App.tsx 启动时发起（isPermissionGranted → requestPermission）；
//!   且未签名/开发构建可能不弹横幅，属系统限制，非代码可解。
//! - Windows：toast 通知无需此授权流程（requestPermission 直接 granted），
//!   但会被「专注助手/勿扰」抑制，且 toast 数秒后自动收入通知中心——均为系统行为。
//!
//! 去重持久化：数据目录 due-notify-state.json，task_id → { due, h1, t0 }；
//! 任务 due 变化时两条标志重置（重新武装）；任务完成/删除/归档/不存在时清理条目。

use std::collections::HashMap;

use chrono::{DateTime, Duration, Local};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

/// 截止通知补发时效窗口：超过此时长的历史截止不补发（防重启/升级后轰炸）
const T0_WINDOW: Duration = Duration::hours(24);

/// 单任务通知状态：due 原文 + 两条已发标志
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskNotifyState {
    pub due: String,
    #[serde(default)]
    pub h1: bool,
    #[serde(default)]
    pub t0: bool,
}

/// 全量去重状态：task_id → TaskNotifyState（进程内 Mutex 保护，变更后落盘）
static NOTIFY_STATE: std::sync::OnceLock<
    std::sync::Mutex<Option<HashMap<String, TaskNotifyState>>>,
> = std::sync::OnceLock::new();

fn notify_state() -> &'static std::sync::Mutex<Option<HashMap<String, TaskNotifyState>>> {
    NOTIFY_STATE.get_or_init(|| std::sync::Mutex::new(None))
}

fn state_path(app: &AppHandle) -> std::path::PathBuf {
    crate::db::data_dir(app).join("due-notify-state.json")
}

/// 从磁盘读去重状态（文件缺失/损坏都回空表——损坏留痕不吞错，宁可重发不丢通知语义）
fn load_state(app: &AppHandle) -> HashMap<String, TaskNotifyState> {
    let p = state_path(app);
    match std::fs::read_to_string(&p) {
        Ok(raw) => match serde_json::from_str(&raw) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("[due_notify] 状态文件损坏，按空重建：{} ({e})", p.display());
                HashMap::new()
            }
        },
        Err(_) => HashMap::new(),
    }
}

/// 状态落盘：原子写（tmp+rename），崩溃不留半截 JSON（复用 db::atomic_write 既有方案）
fn save_state(app: &AppHandle, state: &HashMap<String, TaskNotifyState>) {
    let Ok(raw) = serde_json::to_string(state) else {
        return;
    };
    if let Err(e) = crate::db::atomic_write(&state_path(app), &raw) {
        eprintln!("[due_notify] 状态落盘失败：{e}");
    }
}

/// 截止时间解析（纯函数，可测）：
/// - "YYYY-MM-DDTHH:mm" → 该本地时刻（DST 歧义/不存在由 bot_scheduler::resolve_local 统一处理）
/// - "YYYY-MM-DD"（仅日期）→ 当天 23:59（视为「这一天结束」）
/// - 其余/解析失败 → None（视为无截止，不发通知）
fn parse_due_dt(due: &str) -> Option<DateTime<Local>> {
    let s = due.trim();
    if let Ok(t) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M") {
        return crate::bot_scheduler::resolve_local(t);
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return crate::bot_scheduler::resolve_local(d.and_hms_opt(23, 59, 0)?);
    }
    None
}

/// 通知种类（审计日志 kind 字段用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotifyKind {
    /// 截止前 1 小时
    H1,
    /// 截止时刻
    T0,
}

impl NotifyKind {
    fn as_str(&self) -> &'static str {
        match self {
            NotifyKind::H1 => "h1",
            NotifyKind::T0 => "t0",
        }
    }
}

/// 本轮待发通知（含文案渲染所需的上下文）
#[derive(Debug, PartialEq, Eq)]
struct Pending {
    id: String,
    title: String,
    kind: NotifyKind,
    /// true = due 带时刻（MM-DD HH:mm）；false = 仅日期（MM-DD）
    has_time: bool,
    due_dt: DateTime<Local>,
}

/// 一轮扫描的状态合并 + 发送计划（纯函数，可测）。
///
/// `active` 为活跃任务（未删/未归档/未完成）的 (id, 标题, due 原文) 列表。
/// 规则：
/// - 状态里 task_id 不在活跃列表（或 due 解析失败）→ 条目清理；
/// - 任务 due 原文变化 → 该任务 h1/t0 标志重置（重新武装通知）；
/// - now ∈ [due-1h, due) 且 h1 未发 → 发 H1；
/// - now ∈ [due, due+24h] 且 t0 未发 → 发 T0（h1 一并标记已发：
///   重启后两个条件同时满足时只发截止通知，不连发两条）；
/// - now > due+24h（历史截止）→ 两条都标记已发，不补发。
fn plan_round(
    state: &mut HashMap<String, TaskNotifyState>,
    active: &[(String, String, String)],
    now: DateTime<Local>,
) -> Vec<Pending> {
    // 解析出有效任务（due 可解析）；解析失败视为无截止
    let valid: Vec<(&String, &String, &String, DateTime<Local>)> = active
        .iter()
        .filter_map(|(id, title, due)| parse_due_dt(due).map(|dt| (id, title, due, dt)))
        .collect();
    // 清理：已删除/完成/归档/去掉 due 的任务不再跟踪
    state.retain(|id, _| valid.iter().any(|(vid, _, _, _)| *vid == id));

    let mut pending = Vec::new();
    for (id, title, due, due_dt) in valid {
        let entry = state.entry(id.clone()).or_insert_with(|| TaskNotifyState {
            due: due.clone(),
            h1: false,
            t0: false,
        });
        // due 变化 → 重置标志（新截止时间需要重新武装两条通知）
        if entry.due != *due {
            entry.due = due.clone();
            entry.h1 = false;
            entry.t0 = false;
        }
        let has_time = due.contains('T');
        let overdue = now.signed_duration_since(due_dt);
        if overdue >= Duration::zero() {
            if overdue > T0_WINDOW {
                // 超 24h 历史截止：标记已发，不补发
                entry.h1 = true;
                entry.t0 = true;
            } else if !entry.t0 {
                entry.h1 = true;
                entry.t0 = true;
                pending.push(Pending {
                    id: id.clone(),
                    title: title.clone(),
                    kind: NotifyKind::T0,
                    has_time,
                    due_dt,
                });
            }
        } else if !entry.h1 && -overdue <= Duration::hours(1) {
            entry.h1 = true;
            pending.push(Pending {
                id: id.clone(),
                title: title.clone(),
                kind: NotifyKind::H1,
                has_time,
                due_dt,
            });
        }
    }
    pending
}

/// 通知文案（纯函数）：title/body 按种类与时间精度拼装
fn render(p: &Pending) -> (String, String) {
    let when = if p.has_time {
        p.due_dt.format("%m-%d %H:%M").to_string()
    } else {
        p.due_dt.format("%m-%d").to_string()
    };
    match p.kind {
        NotifyKind::H1 => (
            "任务即将截止".to_string(),
            format!("「{}」还有 1 小时截止（{when}）", p.title),
        ),
        NotifyKind::T0 => (
            "任务已到截止时间".to_string(),
            format!("「{}」截止时间已到（{when}）", p.title),
        ),
    }
}

/// 单轮扫描：读库 → 合并状态算计划 → 发通知 → 落盘
async fn run_round(app: &AppHandle) {
    let all = match crate::db::db_load(app.clone()).await {
        Ok(t) => t,
        Err(e) => {
            // 读库失败跳过本轮（不清状态：否则瞬时故障会丢去重记录，恢复后轰炸）
            eprintln!("[due_notify] 读取任务失败，跳过本轮：{e}");
            return;
        }
    };
    let active: Vec<(String, String, String)> = all
        .iter()
        .filter(|t| t.deleted_at.is_none() && t.archived != Some(true) && t.column != "done")
        .filter_map(|t| {
            t.due
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|d| (t.id.clone(), t.title.clone(), d.to_string()))
        })
        .collect();
    let now = Local::now();
    // 全程持锁：状态加载 → 合并 → 发送 → 落盘 原子完成，防与其他路径交叉
    //（锁中毒按项目惯例 into_inner 继续，不让通知循环 panic 退出）
    let mut guard = notify_state().lock().unwrap_or_else(|e| { eprintln!("[mutex_poisoned] due_notify::notify_state: {e:?}"); e.into_inner() });
    let state = guard.get_or_insert_with(|| load_state(app));
    let pending = plan_round(state, &active, now);
    for p in &pending {
        let (title, body) = render(p);
        match app.notification().builder().title(title).body(body).show() {
            Ok(()) => crate::bot::audit_log(
                app,
                &format!(
                    "due_notify | id: {} | kind: {} | title: {}",
                    p.id,
                    p.kind.as_str(),
                    crate::bot::truncate_for_log(&p.title, 60)
                ),
            ),
            // 发送失败：标志已在 plan_round 置为已发（避免每 30s 重试刷屏），这里只留审计
            Err(e) => crate::bot::audit_log(
                app,
                &format!(
                    "due_notify_fail | id: {} | kind: {} | title: {} | err: {}",
                    p.id,
                    p.kind.as_str(),
                    crate::bot::truncate_for_log(&p.title, 60),
                    crate::bot::truncate_for_log(&e.to_string(), 120)
                ),
            ),
        }
    }
    save_state(app, state);
}

/// 启动截止通知后台循环：每 30s 扫一次活跃任务卡的截止时间（App 启动时调用）
pub fn start_due_notifier(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));
        ticker.tick().await; // 消耗首个立即触发的 tick
        loop {
            ticker.tick().await;
            run_round(&app).await;
        }
    });
}

// ────────────────────────────────────────────────────────────────────
// 测试：截止时间解析 + 状态合并纯函数（plan_round）
// ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod due_notify_tests {
    use super::*;

    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        chrono::NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
            .and_local_timezone(Local)
            .single()
            .unwrap()
    }

    fn active(id: &str, title: &str, due: &str) -> (String, String, String) {
        (id.into(), title.into(), due.into())
    }

    // ── parse_due_dt：两种格式 / 非法格式 / 仅日期→23:59 ──

    #[test]
    fn parse_due_datetime_format() {
        assert_eq!(
            parse_due_dt("2026-09-05T18:00"),
            Some(dt(2026, 9, 5, 18, 0))
        );
    }

    #[test]
    fn parse_due_date_only_means_end_of_day() {
        assert_eq!(parse_due_dt("2026-09-05"), Some(dt(2026, 9, 5, 23, 59)));
    }

    #[test]
    fn parse_due_invalid_formats() {
        assert_eq!(parse_due_dt("2026/09/05"), None);
        assert_eq!(parse_due_dt("2026-09-05 18:00"), None); // 空格分隔不支持
        assert_eq!(parse_due_dt("2026-09-05T25:00"), None); // 非法小时
        assert_eq!(parse_due_dt("2026-02-30"), None); // 非法日期
        assert_eq!(parse_due_dt(""), None);
        assert_eq!(parse_due_dt("junk"), None);
    }

    // ── plan_round：h1/t0 判定边界 ──

    #[test]
    fn h1_fires_exactly_one_hour_before() {
        let mut state = HashMap::new();
        let tasks = vec![active("a", "写报告", "2026-09-05T18:00")];
        // 恰 1h 前 → 发 H1
        let p = plan_round(&mut state, &tasks, dt(2026, 9, 5, 17, 0));
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].kind, NotifyKind::H1);
        assert!(state["a"].h1 && !state["a"].t0);
        // 1h 零 1 秒前 → 不发
        let mut state2 = HashMap::new();
        let p2 = plan_round(&mut state2, &tasks, dt(2026, 9, 5, 16, 59));
        assert!(p2.is_empty());
    }

    #[test]
    fn t0_fires_exactly_at_due() {
        let mut state = HashMap::new();
        let tasks = vec![active("a", "写报告", "2026-09-05T18:00")];
        // 恰在截止时刻 → 发 T0（不是 H1）
        let p = plan_round(&mut state, &tasks, dt(2026, 9, 5, 18, 0));
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].kind, NotifyKind::T0);
        assert!(state["a"].h1 && state["a"].t0);
    }

    #[test]
    fn restart_missed_h1_sends_only_t0() {
        // 应用重启时已错过 1 小时前提醒（两个条件同时满足）→ 只发截止通知，两条都标记
        let mut state = HashMap::new();
        let tasks = vec![active("a", "写报告", "2026-09-05T18:00")];
        let p = plan_round(&mut state, &tasks, dt(2026, 9, 5, 18, 30));
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].kind, NotifyKind::T0);
        assert!(state["a"].h1 && state["a"].t0);
    }

    #[test]
    fn stale_beyond_24h_not_resent() {
        // 截止已超 24h：不补发，但两条标记已发（防升级/重启后轰炸）
        let mut state = HashMap::new();
        let tasks = vec![active("a", "写报告", "2026-09-05T18:00")];
        let p = plan_round(&mut state, &tasks, dt(2026, 9, 7, 0, 0));
        assert!(p.is_empty());
        assert!(state["a"].h1 && state["a"].t0);
    }

    #[test]
    fn t0_window_boundary_24h() {
        // 恰 24h 后 → 仍补发；24h 零 1 秒 → 不补发
        let mut s1 = HashMap::new();
        let tasks = vec![active("a", "写报告", "2026-09-05T18:00")];
        let p1 = plan_round(&mut s1, &tasks, dt(2026, 9, 6, 18, 0));
        assert_eq!(p1.len(), 1);
        assert_eq!(p1[0].kind, NotifyKind::T0);
        let mut s2 = HashMap::new();
        let p2 = plan_round(&mut s2, &tasks, dt(2026, 9, 6, 18, 1));
        assert!(p2.is_empty());
    }

    #[test]
    fn date_only_task_uses_end_of_day() {
        // 仅日期任务：当天白天不触发，23:59 前 1h（22:59）发 H1，23:59 发 T0
        let tasks = vec![active("a", "写报告", "2026-09-05")];
        let mut state = HashMap::new();
        assert!(plan_round(&mut state, &tasks, dt(2026, 9, 5, 10, 0)).is_empty());
        let p = plan_round(&mut state, &tasks, dt(2026, 9, 5, 22, 59));
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].kind, NotifyKind::H1);
        assert!(!p[0].has_time); // 仅日期 → 文案显示 MM-DD
        let p2 = plan_round(&mut state, &tasks, dt(2026, 9, 5, 23, 59));
        assert_eq!(p2.len(), 1);
        assert_eq!(p2[0].kind, NotifyKind::T0);
    }

    // ── plan_round：状态合并（due 变更重置 / 清理 / 不重复发） ──

    #[test]
    fn due_change_rearms_notifications() {
        let mut state = HashMap::new();
        // 旧 due 已发过 T0
        let old = vec![active("a", "写报告", "2026-09-05T18:00")];
        plan_round(&mut state, &old, dt(2026, 9, 5, 18, 0));
        assert!(state["a"].t0);
        // due 改到明天 → 标志重置，新截止前 1h 重新武装
        let new = vec![active("a", "写报告", "2026-09-06T18:00")];
        assert!(plan_round(&mut state, &new, dt(2026, 9, 5, 19, 0)).is_empty());
        assert!(!state["a"].h1 && !state["a"].t0);
        let p = plan_round(&mut state, &new, dt(2026, 9, 6, 17, 0));
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].kind, NotifyKind::H1);
    }

    #[test]
    fn inactive_or_undated_tasks_are_cleaned_up() {
        let mut state = HashMap::new();
        let tasks = vec![
            active("a", "写报告", "2026-09-05T18:00"),
            active("b", "坏数据", "not-a-date"),
        ];
        plan_round(&mut state, &tasks, dt(2026, 9, 5, 17, 0));
        assert!(state.contains_key("a"));
        assert!(!state.contains_key("b")); // 解析失败不跟踪
                                           // a 完成/删除（不在活跃列表）→ 条目清理
        plan_round(&mut state, &[], dt(2026, 9, 5, 17, 30));
        assert!(state.is_empty());
    }

    #[test]
    fn no_duplicate_across_ticks() {
        let mut state = HashMap::new();
        let tasks = vec![active("a", "写报告", "2026-09-05T18:00")];
        let p1 = plan_round(&mut state, &tasks, dt(2026, 9, 5, 17, 0));
        assert_eq!(p1.len(), 1);
        // 下一轮同一窗口 → 不再发
        assert!(plan_round(&mut state, &tasks, dt(2026, 9, 5, 17, 0)).is_empty());
        assert!(plan_round(&mut state, &tasks, dt(2026, 9, 5, 17, 30)).is_empty());
        // 到点 → T0 只发一次
        assert_eq!(
            plan_round(&mut state, &tasks, dt(2026, 9, 5, 18, 0)).len(),
            1
        );
        assert!(plan_round(&mut state, &tasks, dt(2026, 9, 5, 18, 1)).is_empty());
    }

    #[test]
    fn render_text_by_kind_and_precision() {
        let h1 = Pending {
            id: "a".into(),
            title: "写报告".into(),
            kind: NotifyKind::H1,
            has_time: true,
            due_dt: dt(2026, 9, 5, 18, 0),
        };
        let (t, b) = render(&h1);
        assert_eq!(t, "任务即将截止");
        assert_eq!(b, "「写报告」还有 1 小时截止（09-05 18:00）");
        let t0 = Pending {
            kind: NotifyKind::T0,
            has_time: false,
            ..h1
        };
        let (t, b) = render(&t0);
        assert_eq!(t, "任务已到截止时间");
        assert_eq!(b, "「写报告」截止时间已到（09-05）");
    }
}
