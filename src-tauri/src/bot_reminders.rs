//! 机器人提醒：设一个到点提醒，到点发一条系统通知——**只说话，不做事**。
//!
//! 与内容型定时作业（`bot_scheduler` + `db::schedule_jobs`）的分界：作业到点
//! 新建任务卡交给机器人执行，提醒到点只把一句话递到用户面前。定时串格式与
//! 触发点计算与作业共用（`bot_scheduler::validate_schedule` /
//! `occurrence_after`），但存储与触发路径各走各的——提醒不进执行历史、不占
//! 重试与自动暂停语义。
//!
//! 触发通道：系统通知（`tauri-plugin-notification`，与任务卡截止提醒
//! `due_notify` 同一套）。macOS 需用户授权系统通知（未授权时静默失败，
//! 授权流程在前端启动时发起，与 due_notify 一致）。

use chrono::{DateTime, Datelike, Duration, Local, NaiveDateTime, NaiveTime};
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

use crate::bot::registry::ToolResult;
use crate::db::reminders::Reminder;

/// 一次性提醒错过多久就不再补提醒（超过 24h 视为过期，静默丢弃）
const ONE_SHOT_GRACE: Duration = Duration::hours(24);
/// 周期提醒错过多久就不补提醒（关机 / 休眠期间错过的到点，恢复后只推进不打扰）
const RECUR_GRACE: Duration = Duration::hours(2);

// ── 时间解析（纯函数，可测）────────────────────────────────────────────────

/// 重复类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repeat {
    None,
    Daily,
    Weekly,
    Monthly,
}

impl Repeat {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "none" | "once" => Ok(Repeat::None),
            "daily" | "day" => Ok(Repeat::Daily),
            "weekly" | "week" => Ok(Repeat::Weekly),
            "monthly" | "month" => Ok(Repeat::Monthly),
            other => Err(format!(
                "未知的重复类型：{other}（可用 none / daily / weekly / monthly）"
            )),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Repeat::None => "仅一次",
            Repeat::Daily => "每天",
            Repeat::Weekly => "每周",
            Repeat::Monthly => "每月",
        }
    }
}

/// 解析 when → 基准时刻（本地时区）。
///
/// 接受的写法：
/// - `2026-10-22T09:00` / `2026-10-22 09:00`：绝对时间
/// - `09:00`：今天的 HH:MM；今天已过则顺延到明天
/// - `+30m` / `+2h` / `+1d` / `+90`：相对现在（m=分 h=时 d=天，裸数字按分钟）
fn parse_base(when: &str, now: DateTime<Local>) -> Result<DateTime<Local>, String> {
    let s = when.trim();
    if s.is_empty() {
        return Err("提醒时间不能为空".into());
    }
    if let Some(rest) = s.strip_prefix('+') {
        return parse_relative(rest, now);
    }
    for fmt in ["%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(s, fmt) {
            return crate::bot_scheduler::resolve_local(dt)
                .ok_or_else(|| format!("提醒时间无法解析：{when}"));
        }
    }
    if let Ok(t) = NaiveTime::parse_from_str(s, "%H:%M") {
        let today = now.date_naive().and_time(t);
        let occ = crate::bot_scheduler::resolve_local(today)
            .ok_or_else(|| format!("提醒时间无法解析：{when}"))?;
        return Ok(if occ > now {
            occ
        } else {
            occ + Duration::days(1)
        });
    }
    Err(format!(
        "提醒时间无法解析：{when}（可用 2026-10-22T09:00 / 09:00 / +30m）"
    ))
}

/// 相对时间：`30m` / `2h` / `1d` / 裸数字（按分钟）
fn parse_relative(rest: &str, now: DateTime<Local>) -> Result<DateTime<Local>, String> {
    let r = rest.trim().to_ascii_lowercase();
    let (num_part, unit) = match r.chars().last() {
        Some(c) if !c.is_ascii_digit() => (&r[..r.len() - 1], &r[r.len() - 1..]),
        _ => (r.as_str(), "m"),
    };
    let n: i64 = num_part
        .trim()
        .parse()
        .map_err(|_| format!("相对时间无法解析：+{r}（可用 +30m / +2h / +1d）"))?;
    if n <= 0 {
        return Err("相对时间必须为正数".into());
    }
    let dur = match unit {
        "m" | "min" => Duration::minutes(n),
        "h" | "hour" => Duration::hours(n),
        "d" | "day" => Duration::days(n),
        other => return Err(format!("相对时间单位无法解析：{other}（可用 m / h / d）")),
    };
    Ok(now + dur)
}

/// 由「基准时刻 + 重复类型」拼出规范定时串（与作业同格式）。
///
/// 重复类型只取基准时刻的「时刻部分」与「星期 / 日序」——「每天 9 点」这类
/// 说法里没有别的锚点，基准时刻就是锚点本身。
fn build_schedule(base: DateTime<Local>, repeat: Repeat) -> String {
    let h = base.format("%H").to_string();
    let m = base.format("%M").to_string();
    match repeat {
        Repeat::None => format!("at:{}", base.format("%Y-%m-%dT%H:%M")),
        Repeat::Daily => format!("daily:{h}:{m}"),
        // weekday().num_days_from_monday() + 1 → 1=周一（与作业格式一致）
        Repeat::Weekly => format!(
            "weekly:{}:{h}:{m}",
            base.weekday().num_days_from_monday() + 1
        ),
        Repeat::Monthly => format!("monthly:{}:{h}:{m}", base.format("%d")),
    }
}

// ── 工具 ───────────────────────────────────────────────────────────────────

/// schedule_reminder：设一个到点提醒。
///
/// text = 提醒内容；when = 时间（绝对 / HH:MM / 相对）；repeat = 重复类型。
pub async fn tool_schedule_reminder(app: &AppHandle, args: &str) -> ToolResult {
    let v = crate::bot::parse_args(args);
    let raw_text = v["text"].as_str().unwrap_or("");
    let text = match crate::db::reminders::validate_reminder_text(raw_text) {
        Ok(t) => t,
        // 「提醒内容不能为空」首字「提」非 error/warn 前缀 → ok
        Err(e) => return ToolResult::ok(e, Vec::new()),
    };
    let when = v["when"].as_str().unwrap_or("").trim().to_string();
    if when.is_empty() {
        return ToolResult::ok(
            "schedule_reminder 缺少 when 参数（提醒时间，可用 2026-10-22T09:00 / 09:00 / +30m）"
                .to_string(),
            Vec::new(),
        );
    }
    let repeat = match Repeat::parse(v["repeat"].as_str().unwrap_or("none")) {
        Ok(r) => r,
        Err(e) => return ToolResult::ok(e, Vec::new()),
    };
    let now = Local::now();
    let base = match parse_base(&when, now) {
        Ok(b) => b,
        Err(e) => return ToolResult::ok(e, Vec::new()),
    };
    let schedule = build_schedule(base, repeat);
    if crate::bot_scheduler::validate_schedule(&schedule).is_err() {
        return ToolResult::ok(format!("提醒时间无法解析：{when}"), Vec::new());
    }
    // 首次触发：基准在将来就用基准；已过则退到该规则的下一次（一次性无下一次 → 报错）
    let first = if base > now {
        base
    } else {
        match crate::bot_scheduler::occurrence_after(&schedule, now) {
            Some(t) => t,
            None => {
                return ToolResult::ok(
                    format!("提醒时间已过：{when}（请给一个将来的时间）"),
                    Vec::new(),
                )
            }
        }
    };
    let id = uuid::Uuid::new_v4().simple().to_string();
    let text_for_db = text.clone();
    let schedule_for_db = schedule.clone();
    let first_ms = first.timestamp_millis();
    let app2 = app.clone();
    let inserted = tauri::async_runtime::spawn_blocking(move || -> Result<Reminder, String> {
        let _g = crate::db::lock_db_write();
        let conn = crate::db::open_db(&app2)?;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let r = Reminder {
            id,
            text: text_for_db,
            schedule: schedule_for_db,
            remind_at: Some(first_ms),
            enabled: None,
            created_at: Some(now_ms),
            updated_at: Some(now_ms),
            fired_at: None,
            fired_count: None,
        };
        crate::db::reminders::insert_reminder(&conn, &r)?;
        Ok(r)
    })
    .await
    .unwrap_or_else(|e| Err(format!("提醒写入线程失败：{e}")));
    match inserted {
        Ok(r) => {
            crate::bot::audit_log(
                app,
                &format!(
                    "reminder.set | id: {} | schedule: {} | at: {}",
                    r.id,
                    r.schedule,
                    crate::bot::truncate_for_log(&r.text, 60)
                ),
            );
            // 「已设提醒」首字「已」非 error/warn 前缀 → ok
            ToolResult::ok(
                format!(
                    "已设提醒：{} 「{}」（{}）。到点会以系统通知提醒用户，提醒 id: {}",
                    first.format("%Y-%m-%d %H:%M"),
                    text,
                    repeat.label(),
                    r.id
                ),
                Vec::new(),
            )
        }
        Err(e) => ToolResult::ok(format!("设置提醒失败：{e}"), Vec::new()),
    }
}

/// list_reminders：列出当前提醒（按下次触发时间排序）
pub async fn tool_list_reminders(app: &AppHandle, _args: &str) -> ToolResult {
    let app2 = app.clone();
    let rows = tauri::async_runtime::spawn_blocking(move || -> Result<Vec<Reminder>, String> {
        let conn = crate::db::open_db(&app2)?;
        crate::db::reminders::load_reminders(&conn)
    })
    .await
    .unwrap_or_else(|e| Err(format!("提醒读取线程失败：{e}")));
    match rows {
        Ok(list) => {
            if list.is_empty() {
                // 「当前没有设置任何提醒」首字「当」非 error/warn 前缀 → ok
                return ToolResult::ok("当前没有设置任何提醒".to_string(), Vec::new());
            }
            let mut lines = Vec::new();
            for r in &list {
                let at = r
                    .remind_at
                    .and_then(|ms| DateTime::from_timestamp_millis(ms))
                    .map(|t| t.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "无下次触发".to_string());
                lines.push(format!(
                    "- {}（id: {}）{} 触发 · {}",
                    r.text,
                    r.id,
                    at,
                    if r.enabled() {
                        "已启用"
                    } else {
                        "已暂停"
                    }
                ));
            }
            ToolResult::ok(
                format!("当前有 {} 条提醒：\n{}", list.len(), lines.join("\n")),
                Vec::new(),
            )
        }
        Err(e) => ToolResult::ok(format!("读取提醒失败：{e}"), Vec::new()),
    }
}

/// cancel_reminder：取消一条提醒（按 id）
pub async fn tool_cancel_reminder(app: &AppHandle, args: &str) -> ToolResult {
    let v = crate::bot::parse_args(args);
    let id = v["id"].as_str().unwrap_or("").trim().to_string();
    if id.is_empty() {
        return ToolResult::ok(
            "cancel_reminder 缺少 id 参数（用 list_reminders 查 id）".to_string(),
            Vec::new(),
        );
    }
    let app2 = app.clone();
    let id_for_db = id.clone();
    let res = tauri::async_runtime::spawn_blocking(
        move || -> Result<(bool, Option<Reminder>), String> {
            let _g = crate::db::lock_db_write();
            let conn = crate::db::open_db(&app2)?;
            let row = crate::db::reminders::load_reminder(&conn, &id_for_db)?;
            let deleted = crate::db::reminders::delete_reminder(&conn, &id_for_db)?;
            Ok((deleted, row))
        },
    )
    .await
    .unwrap_or_else(|e| Err(format!("提醒取消线程失败：{e}")));
    match res {
        Ok((true, None)) => ToolResult::ok(format!("已取消提醒（id: {id}）"), Vec::new()),
        Ok((true, Some(row))) => {
            crate::bot::audit_log(
                app,
                &format!(
                    "reminder.cancel | id: {} | text: {}",
                    row.id,
                    crate::bot::truncate_for_log(&row.text, 60)
                ),
            );
            // 「已取消提醒」首字「已」非 error/warn 前缀 → ok
            ToolResult::ok(
                format!("已取消提醒：「{}」（id: {}）", row.text, row.id),
                Vec::new(),
            )
        }
        Ok((false, _)) => ToolResult::ok(
            format!("没有找到 id 为 {id} 的提醒（用 list_reminders 查现有提醒）"),
            Vec::new(),
        ),
        Err(e) => ToolResult::ok(format!("取消提醒失败：{e}"), Vec::new()),
    }
}

// ── 调度触发 ───────────────────────────────────────────────────────────────

/// 提醒单轮扫描：到点的发系统通知，周期提醒推进到下一次，一次性提醒发完即删。
///
/// 由调度器每 30s 调一次（与内容型作业同一个 tick，不另起后台线程）。
pub async fn reminders_tick(app: &AppHandle) {
    let now = Local::now();
    let now_ms = now.timestamp_millis();
    let app2 = app.clone();
    let due = tauri::async_runtime::spawn_blocking(move || -> Result<Vec<Reminder>, String> {
        let conn = crate::db::open_db(&app2)?;
        crate::db::reminders::load_due_reminders(&conn, now_ms)
    })
    .await
    .unwrap_or_else(|e| Err(format!("提醒扫描线程失败：{e}")));
    let due = match due {
        Ok(d) => d,
        Err(e) => {
            crate::bot::audit_log(app, &format!("reminder.scan_failed | {e}"));
            return;
        }
    };
    for r in due {
        let at = r.remind_at.unwrap_or(now_ms);
        let late = Duration::milliseconds((now_ms - at).max(0));
        if r.one_shot() {
            if late > ONE_SHOT_GRACE {
                // 过期太久（关机 / 休眠多日）：静默丢弃，不补一条过期提醒
                crate::bot::audit_log(
                    app,
                    &format!(
                        "reminder.expired | id: {} | 错过 {:?} 超过补提醒窗口，丢弃",
                        r.id, late
                    ),
                );
                delete_row(app, &r.id).await;
                continue;
            }
            fire(app, &r);
            delete_row(app, &r.id).await;
        } else {
            if late > RECUR_GRACE {
                // 错过太久：只推进到下一次，不打扰（否则每次开机都补一条）
                let next = crate::bot_scheduler::occurrence_after(&r.schedule, now);
                advance_row(app, &r.id, now_ms, next.map(|t| t.timestamp_millis())).await;
                continue;
            }
            fire(app, &r);
            let next = crate::bot_scheduler::occurrence_after(&r.schedule, now);
            advance_row(app, &r.id, now_ms, next.map(|t| t.timestamp_millis())).await;
        }
    }
}

/// 发系统通知（失败只审计不重试：通知通道不可用时反复重试只会刷屏）
fn fire(app: &AppHandle, r: &Reminder) {
    match app
        .notification()
        .builder()
        .title("⏰ 提醒")
        .body(&r.text)
        .show()
    {
        Ok(()) => crate::bot::audit_log(
            app,
            &format!(
                "reminder.fired | id: {} | text: {}",
                r.id,
                crate::bot::truncate_for_log(&r.text, 60)
            ),
        ),
        Err(e) => crate::bot::audit_log(
            app,
            &format!(
                "reminder.fire_failed | id: {} | err: {}",
                r.id,
                crate::bot::truncate_for_log(&e.to_string(), 120)
            ),
        ),
    }
}

/// 推进下次触发时间
async fn advance_row(app: &AppHandle, id: &str, fired_at: i64, next_at: Option<i64>) {
    let id = id.to_string();
    let app2 = app.clone();
    let app_log = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let _g = crate::db::lock_db_write();
        let conn = crate::db::open_db(&app2)?;
        crate::db::reminders::advance_reminder(&conn, &id, fired_at, next_at)
    })
    .await
    .unwrap_or_else(|e| Err(format!("提醒推进线程失败：{e}")))
    .unwrap_or_else(|e| {
        crate::bot::audit_log(&app_log, &format!("reminder.advance_failed | {e}"));
    });
}

/// 删除提醒行（一次性提醒触发后 / 过期丢弃）
async fn delete_row(app: &AppHandle, id: &str) {
    let id = id.to_string();
    let app2 = app.clone();
    let app_log = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let _g = crate::db::lock_db_write();
        let conn = crate::db::open_db(&app2)?;
        crate::db::reminders::delete_reminder(&conn, &id).map(|_| ())
    })
    .await
    .unwrap_or_else(|e| Err(format!("提醒删除线程失败：{e}")))
    .unwrap_or_else(|e| {
        crate::bot::audit_log(&app_log, &format!("reminder.delete_failed | {e}"));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 10, 21, 14, 0, 0).unwrap()
    }

    #[test]
    fn parse_base_absolute_and_space_separated() {
        let b = parse_base("2026-10-22T09:00", now()).unwrap();
        assert_eq!(b.format("%Y-%m-%d %H:%M").to_string(), "2026-10-22 09:00");
        let b2 = parse_base("2026-10-22 09:00", now()).unwrap();
        assert_eq!(b2, b, "空格写法与 T 写法等价");
    }

    #[test]
    fn parse_base_hhmm_rolls_to_tomorrow_when_passed() {
        // 今天还没到 → 今天
        let b = parse_base("18:00", now()).unwrap();
        assert_eq!(b.format("%Y-%m-%d %H:%M").to_string(), "2026-10-21 18:00");
        // 今天已过 → 明天
        let b2 = parse_base("09:00", now()).unwrap();
        assert_eq!(b2.format("%Y-%m-%d %H:%M").to_string(), "2026-10-22 09:00");
    }

    #[test]
    fn parse_base_relative_units() {
        assert_eq!(
            parse_base("+30m", now())
                .unwrap()
                .format("%H:%M")
                .to_string(),
            "14:30"
        );
        assert_eq!(
            parse_base("+2h", now())
                .unwrap()
                .format("%H:%M")
                .to_string(),
            "16:00"
        );
        assert_eq!(
            parse_base("+1d", now())
                .unwrap()
                .format("%Y-%m-%d")
                .to_string(),
            "2026-10-22"
        );
        assert_eq!(
            parse_base("+90", now())
                .unwrap()
                .format("%H:%M")
                .to_string(),
            "15:30",
            "裸数字按分钟"
        );
    }

    #[test]
    fn parse_base_rejects_garbage_and_non_positive() {
        assert!(parse_base("", now()).is_err());
        assert!(parse_base("明天下午", now()).is_err());
        assert!(parse_base("+0m", now()).is_err());
        assert!(parse_base("+-5m", now()).is_err());
        assert!(parse_base("+5x", now()).is_err());
    }

    #[test]
    fn build_schedule_maps_repeat_to_canonical_form() {
        let base = parse_base("2026-10-22T09:30", now()).unwrap(); // 周四
        assert_eq!(
            build_schedule(base, Repeat::None),
            "at:2026-10-22T09:30".to_string()
        );
        assert_eq!(
            build_schedule(base, Repeat::Daily),
            "daily:09:30".to_string()
        );
        assert_eq!(
            build_schedule(base, Repeat::Weekly),
            "weekly:4:09:30".to_string(),
            "周四 → 星期 4"
        );
        assert_eq!(
            build_schedule(base, Repeat::Monthly),
            "monthly:22:09:30".to_string()
        );
    }

    #[test]
    fn built_schedules_pass_scheduler_validation() {
        for (when, repeat) in [
            ("2026-10-22T09:30", Repeat::None),
            ("2026-10-22T09:30", Repeat::Daily),
            ("2026-10-22T09:30", Repeat::Weekly),
            ("2026-10-22T09:30", Repeat::Monthly),
        ] {
            let base = parse_base(when, now()).unwrap();
            let sched = build_schedule(base, repeat);
            assert!(
                crate::bot_scheduler::validate_schedule(&sched).is_ok(),
                "{sched} 应是合法定时串"
            );
        }
    }

    #[test]
    fn repeat_parse_accepts_aliases_and_rejects_unknown() {
        assert_eq!(Repeat::parse("").unwrap(), Repeat::None);
        assert_eq!(Repeat::parse("daily").unwrap(), Repeat::Daily);
        assert_eq!(Repeat::parse("WEEKLY").unwrap(), Repeat::Weekly);
        assert_eq!(Repeat::parse("monthly").unwrap(), Repeat::Monthly);
        assert!(Repeat::parse("yearly").is_err());
    }

    /// 接线守卫：提醒三工具必须真的接进工具表
    #[test]
    fn reminder_tools_are_wired_into_registry() {
        let src = include_str!("bot/registry.rs");
        for name in ["schedule_reminder", "list_reminders", "cancel_reminder"] {
            assert!(
                src.contains(&format!("name: \"{name}\"")),
                "{name} 必须登记进 TOOLS_TABLE"
            );
        }
        assert!(
            src.contains("SCHEMA_SCHEDULE_REMINDER"),
            "schedule_reminder 必须有 schema 常量"
        );
    }
}
