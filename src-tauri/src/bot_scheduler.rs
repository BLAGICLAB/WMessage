//! 定时任务卡调度器（F-6 step 5 拆分 2026-08-18）：
//!
//! 负责 ⏰ 到点自动执行任务卡（每日/每周/每月/一次性 at:），与 bot_chat.rs 的
//! 用户交互流解耦：
//! - 解析 schedule 字符串（parse_hm / occurrence_after / at_expired / sched_last_dt）
//! - 防重入守卫（SchedGuard，Drop 自动清理，防止 panic 后任务卡死锁）
//! - 30s 扫描循环（start_scheduler），到点调 execute_task_core 复用 bot_chat.rs 的
//!   模型循环，interactive=false 让 /stop 不影响后台任务
//!
//! 设计目标：bot_chat 的交互入口 / bot_execute_task 按钮入口 / 调度器自动入口
//! 三者共用 execute_task_core（保留在 bot_chat.rs），差异仅在 interactive flag。

use chrono::Datelike;
use tauri::AppHandle;

// ───────────────────────── 定时任务卡（阶段二：⏰ 到点自动执行） ─────────────────────────

/// 正在执行的定时任务 id（防同一任务并发重复跑）
static SCHED_RUNNING: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();

fn sched_running() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    SCHED_RUNNING.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// 调度防重入 RAII 守卫：Drop（含 panic 展开）时自动清理，保证任务 id 不残留
/// （二次审计 P1：原先清理在 run_scheduled 末尾，panic 时该卡永久失效）
struct SchedGuard(String);

impl SchedGuard {
    fn acquire(task_id: &str) -> Option<Self> {
        let mut set = sched_running().lock().unwrap_or_else(|e| e.into_inner());
        if set.contains(task_id) {
            return None;
        }
        set.insert(task_id.to_string());
        Some(Self(task_id.to_string()))
    }
}

impl Drop for SchedGuard {
    fn drop(&mut self) {
        sched_running()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// 解析 "HH:MM"
fn parse_hm(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.split_once(':')?;
    let h = h.parse::<u32>().ok()?;
    let m = m.parse::<u32>().ok()?;
    if h < 24 && m < 60 {
        Some((h, m))
    } else {
        None
    }
}

/// 定时格式：
/// - "daily:HH:MM" 每天
/// - "weekly:D:HH:MM" 每周（D=1..7，周一起）
/// - "at:YYYY-MM-DDTHH:MM" 一次性
/// 返回在 after 之后最近的触发时间；一次性已过或格式无效返回 None。
fn occurrence_after(
    schedule: &str,
    after: chrono::DateTime<chrono::Local>,
) -> Option<chrono::DateTime<chrono::Local>> {
    if let Some(t) = schedule.strip_prefix("daily:") {
        let (h, m) = parse_hm(t)?;
        let mut occ = after
            .date_naive()
            .and_hms_opt(h, m, 0)?
            .and_local_timezone(chrono::Local)
            .single()?;
        if occ <= after {
            occ += chrono::Duration::days(1);
        }
        return Some(occ);
    }
    if let Some(t) = schedule.strip_prefix("weekly:") {
        let (d, rest) = t.split_once(':')?;
        let dow = d.parse::<u32>().ok()?;
        if !(1..=7).contains(&dow) {
            return None;
        }
        let (h, m) = parse_hm(rest)?;
        let weekday = after.weekday().num_days_from_monday() as u32 + 1; // 1=周一
        let diff = (dow + 7 - weekday) % 7;
        let mut occ = (after.date_naive() + chrono::Duration::days(diff as i64))
            .and_hms_opt(h, m, 0)?
            .and_local_timezone(chrono::Local)
            .single()?;
        if occ <= after {
            occ += chrono::Duration::days(7);
        }
        return Some(occ);
    }
    if let Some(t) = schedule.strip_prefix("monthly:") {
        // monthly:DD:HH:MM —— 每月 DD 日 HH:MM（该月无 DD 日如 2 月 31 日则顺延）
        let (dd, rest) = t.split_once(':')?;
        let day = dd.parse::<u32>().ok()?;
        if !(1..=31).contains(&day) {
            return None;
        }
        let (h, m) = parse_hm(rest)?;
        let mut y = after.year();
        let mut mo = after.month();
        for _ in 0..12 {
            if let Some(d) = chrono::NaiveDate::from_ymd_opt(y, mo, day) {
                if let Some(occ) = d
                    .and_hms_opt(h, m, 0)
                    .and_then(|dt| dt.and_local_timezone(chrono::Local).single())
                {
                    if occ > after {
                        return Some(occ);
                    }
                }
            }
            mo += 1;
            if mo > 12 {
                mo = 1;
                y += 1;
            }
        }
        return None;
    }
    if let Some(t) = schedule.strip_prefix("at:") {
        let occ = chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M")
            .ok()?
            .and_local_timezone(chrono::Local)
            .single()?;
        return if occ > after { Some(occ) } else { None };
    }
    None
}

/// at: 一次性任务是否已错过且从未执行（应放弃补执行，防重启后补跑过期任务，审计 P1）
fn at_expired(sched: &str, sched_last: Option<i64>, now: chrono::DateTime<chrono::Local>) -> bool {
    let Some(at) = sched.strip_prefix("at:") else {
        return false;
    };
    if sched_last.is_some() {
        return false; // 执行过的不在此判断（由 occurrence_after 的 after 基准处理）
    }
    match chrono::NaiveDateTime::parse_from_str(at, "%Y-%m-%dT%H:%M")
        .ok()
        .and_then(|dt| dt.and_local_timezone(chrono::Local).single())
    {
        Some(occ) => occ <= now,
        None => true, // 解析失败按过期处理（放弃）
    }
}

/// sched_last（epoch ms）→ DateTime；None 视为远古（下一次触发立即生效）
fn sched_last_dt(ms: Option<i64>) -> chrono::DateTime<chrono::Local> {
    match ms.and_then(chrono::DateTime::from_timestamp_millis) {
        Some(u) => u.with_timezone(&chrono::Local),
        // epoch 0 必然可解析，但防御风格下不用裸 unwrap（审计 P3）
        None => chrono::DateTime::from_timestamp_millis(0)
            .unwrap_or(chrono::DateTime::UNIX_EPOCH)
            .with_timezone(&chrono::Local),
    }
}

/// 找出到点的定时任务（未删、未归档、未完成，且 sched_last < 触发点 ≤ now）。
/// 顺带清理「错过的一次性任务」：at: 从未执行且时间已过 → 放弃并清掉 schedule
///（审计 P1：否则重启后 30s 内会补执行过期任务）
fn find_due_tasks(app: &AppHandle) -> Vec<crate::db::Task> {
    let now = chrono::Local::now();
    let all = crate::db::db_load(app.clone()).unwrap_or_default();
    let stale_ids: Vec<String> = all
        .iter()
        .filter(|t| {
            t.deleted_at.is_none()
                && t.archived != Some(true)
                && t.column != "done"
                && t.schedule
                    .as_deref()
                    .map(str::trim)
                    .map(|s| at_expired(s, t.sched_last, now))
                    .unwrap_or(false)
        })
        .map(|t| t.id.clone())
        .collect();
    let due: Vec<crate::db::Task> = all
        .into_iter()
        .filter_map(|t| {
            if t.deleted_at.is_some() || t.archived == Some(true) || t.column == "done" {
                return None;
            }
            let Some(sched) = t
                .schedule
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                return None;
            };
            match occurrence_after(sched, sched_last_dt(t.sched_last)) {
                Some(occ) if occ <= now => Some(t),
                _ => None,
            }
        })
        .collect();
    // 清理过期一次性任务的 schedule（基于最新数据合并，只清目标字段）。
    // 单次加载处理全部 stale id（审计 P3：原先每个 id 各开一次库，O(n) 次 db_load）
    if !stale_ids.is_empty() {
        if let Ok(cur) = crate::db::db_load(app.clone()) {
            let fresh: Vec<crate::db::Task> = cur
                .into_iter()
                .filter(|t| stale_ids.contains(&t.id))
                .map(|mut t| {
                    t.schedule = None;
                    t.updated_at = Some(now.timestamp_millis());
                    t
                })
                .collect();
            if !fresh.is_empty() {
                let _ = crate::db::db_upsert(app.clone(), fresh);
            }
        }
    }
    due
}

/// 执行一张到点的定时任务卡：先记 sched_last（防重复触发），跑执行循环，结果落备注标记
async fn run_scheduled(app: AppHandle, task: crate::db::Task) {
    // 防重入守卫：作用域结束（含 panic 展开）自动清理
    let Some(_sched_guard) = SchedGuard::acquire(&task.id) else {
        return;
    };
    // 机器人开关关闭时不执行定时任务（二次审计 P2-3：开关只管 UI 不管后端）
    if !crate::bot_slash::bot_get_enabled(app.clone()) {
        crate::bot::audit_log(
            &app,
            &format!("sched_skip | id: {} | 机器人开关未开启", task.id),
        );
        return;
    }
    let now = chrono::Local::now();
    crate::bot::audit_log(
        &app,
        &format!(
            "sched_run | id: {} | title: {} | schedule: {}",
            task.id,
            crate::bot::truncate_for_log(&task.title, 60),
            task.schedule.as_deref().unwrap_or("")
        ),
    );

    // 先记 sched_last：30s 扫描周期内不会重复触发。
    // 基于库中最新数据合并（勿用扫描快照：会覆盖用户在扫描后的编辑）。
    // 记录失败则放弃本次执行（审计 P3：否则下个 tick 会因 sched_last 未更新而重复触发）
    let mut marked = false;
    if let Ok(cur) = crate::db::db_load(app.clone()) {
        if let Some(mut fresh) = cur.into_iter().find(|t| t.id == task.id) {
            fresh.sched_last = Some(now.timestamp_millis());
            fresh.updated_at = Some(now.timestamp_millis());
            marked = crate::db::db_upsert(app.clone(), vec![fresh]).is_ok();
        }
    }
    if !marked {
        crate::bot::audit_log(
            &app,
            &format!(
                "sched_skip | id: {} | 记录 sched_last 失败，放弃本次执行",
                task.id
            ),
        );
        return;
    }

    let result = crate::bot_chat::execute_task_core(&app, &task.id, false).await;
    let time_str = now.format("%m-%d %H:%M").to_string();

    // 执行结果写备注（模型可能已写摘要，这里前置 ⏰ 标记兜底）。
    // ⚠️ 必须基于执行后的最新数据合并：旧快照会把机器人执行期间的修改
    // （完成状态/摘要/子任务）整体回滚（审计 P0 已修复）
    let summary = match result {
        Ok(r) => format!(
            "⏰ 自动执行 {time_str}：{}",
            r.text.chars().take(300).collect::<String>()
        ),
        Err(e) => format!("⏰ 自动执行 {time_str} 失败：{e}"),
    };
    if let Ok(cur) = crate::db::db_load(app.clone()) {
        if let Some(mut fresh) = cur.into_iter().find(|t| t.id == task.id) {
            let note = match fresh.note.as_deref().filter(|n| !n.trim().is_empty()) {
                Some(n) => format!("{summary}\n{n}"),
                None => summary,
            };
            fresh.note = Some(note);
            // 一次性定时执行完清掉 schedule（⏰ 徽标消失）。
            // 用执行后的 fresh.schedule 判断（审计 P2：执行期间用户改过定时，扫描快照会误清新设置）
            if fresh
                .schedule
                .as_deref()
                .is_some_and(|s| s.starts_with("at:"))
            {
                fresh.schedule = None;
            }
            fresh.updated_at = Some(chrono::Local::now().timestamp_millis());
            let _ = crate::db::db_upsert(app.clone(), vec![fresh]);
        }
    }
    crate::bot::audit_log(&app, &format!("sched_done | id: {}", task.id));
}

/// 启动定时调度器：每 30s 扫一次到点任务卡并顺序执行（App 启动时调用）
pub fn start_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));
        ticker.tick().await; // 消耗首个立即触发的 tick
        loop {
            ticker.tick().await;
            let due = find_due_tasks(&app);
            for t in due {
                // 每张卡 spawn 到独立任务再 await：单张卡 panic 只废这一张，
                // 不会杀死调度器主循环（否则后续所有定时任务静默失效，审计 P0）
                let handle = tauri::async_runtime::spawn(run_scheduled(app.clone(), t));
                let _ = handle.await;
            }
        }
    });
}

// ────────────────────────────────────────────────────────────────────
// 测试：定时解析纯函数（occurrence_after / at_expired），F-6 step 5 提取
// ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod sched_tests {
    use super::*;

    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> chrono::DateTime<chrono::Local> {
        chrono::NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
            .and_local_timezone(chrono::Local)
            .single()
            .unwrap()
    }

    #[test]
    fn daily_occurrence() {
        let after = dt(2026, 8, 16, 9, 0);
        assert_eq!(
            occurrence_after("daily:10:00", after),
            Some(dt(2026, 8, 16, 10, 0))
        );
        assert_eq!(
            occurrence_after("daily:09:00", after),
            Some(dt(2026, 8, 17, 9, 0))
        ); // 已过 → 明天
    }

    #[test]
    fn weekly_occurrence() {
        // 2026-08-16 是周日（weekday=7）
        let after = dt(2026, 8, 16, 9, 0);
        assert_eq!(
            occurrence_after("weekly:1:09:00", after),
            Some(dt(2026, 8, 17, 9, 0))
        ); // 本周一已过 → 下周一
        assert_eq!(
            occurrence_after("weekly:7:10:00", after),
            Some(dt(2026, 8, 16, 10, 0))
        ); // 今天周日 10 点未到
        assert_eq!(
            occurrence_after("weekly:7:08:00", after),
            Some(dt(2026, 8, 23, 8, 0))
        ); // 已过 → 下周日
    }

    #[test]
    fn at_occurrence() {
        let after = dt(2026, 8, 16, 9, 0);
        assert_eq!(
            occurrence_after("at:2026-08-16T10:00", after),
            Some(dt(2026, 8, 16, 10, 0))
        );
        assert_eq!(occurrence_after("at:2026-08-16T08:00", after), None); // 已过，一次性不再触发
    }

    #[test]
    fn monthly_occurrence() {
        let after = dt(2026, 8, 16, 9, 0);
        // 本月 16 日 10:00 未到 → 今天
        assert_eq!(
            occurrence_after("monthly:16:10:00", after),
            Some(dt(2026, 8, 16, 10, 0))
        );
        // 本月 15 日已过 → 下月 15 日
        assert_eq!(
            occurrence_after("monthly:15:10:00", after),
            Some(dt(2026, 9, 15, 10, 0))
        );
        // 2 月无 31 日 → 顺延到 3 月 31 日（after=2026-01-20）
        let jan = dt(2026, 1, 20, 9, 0);
        assert_eq!(
            occurrence_after("monthly:31:08:00", jan),
            Some(dt(2026, 1, 31, 8, 0))
        );
        let feb = dt(2026, 2, 1, 9, 0);
        assert_eq!(
            occurrence_after("monthly:31:08:00", feb),
            Some(dt(2026, 3, 31, 8, 0))
        );
    }

    #[test]
    fn at_expired_detection() {
        let now = dt(2026, 8, 16, 10, 0);
        // 从未执行且已过期 → 放弃
        assert!(at_expired("at:2026-08-16T09:00", None, now));
        // 未来 → 不放弃
        assert!(!at_expired("at:2026-08-16T11:00", None, now));
        // 执行过（sched_last 有值）→ 交给 occurrence_after 判断，不在此放弃
        assert!(!at_expired("at:2026-08-16T09:00", Some(1), now));
        // 非 at: 格式 → 不适用
        assert!(!at_expired("daily:09:00", None, now));
        // 坏数据 → 放弃
        assert!(at_expired("at:junk", None, now));
    }

    #[test]
    fn invalid_schedule() {
        let after = dt(2026, 8, 16, 9, 0);
        assert_eq!(occurrence_after("daily:25:00", after), None);
        assert_eq!(occurrence_after("weekly:8:09:00", after), None);
        assert_eq!(occurrence_after("", after), None);
        assert_eq!(occurrence_after("junk", after), None);
        assert_eq!(occurrence_after("at:not-a-time", after), None);
        assert_eq!(occurrence_after("monthly:0:09:00", after), None);
        assert_eq!(occurrence_after("monthly:32:09:00", after), None);
        assert_eq!(occurrence_after("monthly:5:25:00", after), None);
    }
}