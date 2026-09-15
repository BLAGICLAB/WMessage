//! 定时任务卡调度器：
//!
//! 负责 ⏰ 到点自动执行任务卡（每日/每周/每月/一次性 at:），与 bot_chat.rs 的
//! 用户交互流解耦：
//! - 解析 schedule 字符串（parse_hm / occurrence_after / at_expired / sched_last_dt）
//! - 防重入守卫（SchedGuard，Drop 自动清理，防止 panic 后任务卡死锁）
//! - 30s 扫描循环（start_scheduler），到点调 run_task_in_chat（任务执行聊天化：
//!   每次执行新建会话、流式可见、可按会话 /stop、完成/失败发系统通知）；
//!   绕开 exec_steps 逐步确认（无人在场，直接整体执行）

use chrono::Datelike;
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

/// 定时任务执行完成/失败的系统通知：
/// 通知只是提醒（点击拉起应用后按 ⏰ 前缀会话回看完整执行记录）；
/// 通知发送失败（未授权等）只记日志，不影响执行收尾。
fn notify_scheduled_done(
    app: &AppHandle,
    task_title: &str,
    result: &crate::error::CommandResult<crate::bot_chat::TaskChatRun>,
) {
    let title_short = crate::bot::truncate_for_log(task_title.trim(), 30);
    let (title, body) = match result {
        Ok(r) => (
            format!("⏰ 定时任务完成：{title_short}"),
            crate::bot::truncate_for_log(r.result.text.trim(), 120),
        ),
        Err(e) => (
            format!("⏰ 定时任务失败：{title_short}"),
            crate::bot::truncate_for_log(&e.message(), 120),
        ),
    };
    if let Err(e) = app
        .notification()
        .builder()
        .title(&title)
        .body(&body)
        .show()
    {
        eprintln!("[sched] 系统通知发送失败（未授权？）：{e}");
    }
}

// ───────────────────────── 定时任务卡（阶段二：⏰ 到点自动执行） ─────────────────────────

/// 单次定时执行的整体超时：最坏 50 轮 × LLM 300s 可跑
/// 数小时，无上限会把调度循环堵死。30 分钟对正常任务足够宽松；超时 drop 执行流
///（守卫 RAII 自动释放），记 sched_timeout 审计并兜底复位 bot_assigned 头像标记。
const SCHED_TASK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// 正在执行的定时任务 id（防同一任务并发重复跑）
// SCHED_RUNNING 的定义与 sched_running() 访问器已集中到 `crate::app_state`
// （SchedGuard 本体与 Drop 清理语义未动）。
use crate::app_state::sched_running;

/// 调度防重入 RAII 守卫：Drop（含 panic 展开）时自动清理，保证任务 id 不残留
/// （清理不能只放在 run_scheduled 末尾：panic 时该卡会永久失效）
///
/// 表已迁入 `AppState`。Drop 里拿不到 `app`，所以在 acquire 时把表句柄
/// （`Arc<Mutex<HashSet<String>>>` 克隆）带进守卫，Drop 用手里这份清理——
/// 同一个 `Mutex` 实例，清理时机与语义与迁移前一致。
struct SchedGuard {
    task_id: String,
    running: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
}

impl SchedGuard {
    fn acquire<R: tauri::Runtime>(app: &tauri::AppHandle<R>, task_id: &str) -> Option<Self> {
        let running = sched_running(app);
        {
            let mut set = running.lock().unwrap_or_else(|e| e.into_inner());
            if set.contains(task_id) {
                return None;
            }
            set.insert(task_id.to_string());
        }
        Some(Self {
            task_id: task_id.to_string(),
            running,
        })
    }
}

impl Drop for SchedGuard {
    fn drop(&mut self) {
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.task_id);
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

/// 本地时刻解析：DST 切换日目标时刻可能不存在（春拨）
/// 或歧义（秋拨）。一律 `.single()` 的话，返回 None 时 occurrence_after 整体 None，
/// 而基准 sched_last 不变 → daily/weekly 任务永久静默失效且零日志。
/// 因此：歧义取较早者；不存在则顺延到下一个合法时刻（最多 +3h，仍无 → None 按无效处理）。
/// pub(crate)：due_notify.rs 的截止时间解析复用同一 DST 处理规则。
pub(crate) fn resolve_local(dt: chrono::NaiveDateTime) -> Option<chrono::DateTime<chrono::Local>> {
    use chrono::offset::LocalResult;
    match dt.and_local_timezone(chrono::Local) {
        LocalResult::Single(t) => Some(t),
        LocalResult::Ambiguous(a, _) => Some(a),
        LocalResult::None => (1..=3i64).find_map(|h| {
            match (dt + chrono::Duration::hours(h)).and_local_timezone(chrono::Local) {
                LocalResult::Single(t) => Some(t),
                LocalResult::Ambiguous(a, _) => Some(a),
                LocalResult::None => None,
            }
        }),
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
        let mut occ = resolve_local(after.date_naive().and_hms_opt(h, m, 0)?)?;
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
        let mut occ = resolve_local(
            (after.date_naive() + chrono::Duration::days(diff as i64)).and_hms_opt(h, m, 0)?,
        )?;
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
                if let Some(occ) = d.and_hms_opt(h, m, 0).and_then(resolve_local) {
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
        let occ = resolve_local(chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M").ok()?)?;
        return if occ > after { Some(occ) } else { None };
    }
    None
}

/// at: 一次性任务是否已错过且从未执行（应放弃补执行，防重启后补跑过期任务）
fn at_expired(sched: &str, sched_last: Option<i64>, now: chrono::DateTime<chrono::Local>) -> bool {
    let Some(at) = sched.strip_prefix("at:") else {
        return false;
    };
    if sched_last.is_some() {
        return false; // 执行过的不在此判断（由 occurrence_after 的 after 基准处理）
    }
    match chrono::NaiveDateTime::parse_from_str(at, "%Y-%m-%dT%H:%M")
        .ok()
        .and_then(resolve_local)
    {
        Some(occ) => occ <= now,
        None => true, // 解析失败按过期处理（放弃）
    }
}

/// sched_last（epoch ms）→ DateTime；None 视为远古（下一次触发立即生效）
fn sched_last_dt(ms: Option<i64>) -> chrono::DateTime<chrono::Local> {
    match ms.and_then(chrono::DateTime::from_timestamp_millis) {
        Some(u) => u.with_timezone(&chrono::Local),
        // epoch 0 必然可解析，但防御风格下不用裸 unwrap
        None => chrono::DateTime::from_timestamp_millis(0)
            .unwrap_or(chrono::DateTime::UNIX_EPOCH)
            .with_timezone(&chrono::Local),
    }
}

/// recurring 补跑时效窗口：2h。
/// 关机/休眠/机器人开关关闭期间错过的到点，恢复时距到点超过 2h 一律不补跑。
const CATCHUP_WINDOW: chrono::Duration = chrono::Duration::hours(2);

/// 到点判定（纯函数，可测）：
/// - Run：触发点已到且未超补跑窗口（或新任务首跑），正常执行；
/// - Missed：recurring 触发点已超 2h 窗口——不补跑，调用方消费掉该 occurrence
///   （sched_last 记为现在），防关机一周/开关关闭期间的到点在恢复瞬间全补跑；
/// - NotDue：未到点 / 无下次触发。
/// 边界：仅对跑过的任务（sched_last 有值）判 Missed——新任务（None）保持
/// 「下一次触发立即生效」的既有首跑语义；at: 一次性任务由 at_expired 放弃逻辑
/// 处理，不在此判 Missed。
#[derive(Debug, PartialEq, Eq)]
enum DueVerdict {
    Run,
    Missed,
    NotDue,
}

fn classify_due(
    sched: &str,
    sched_last: Option<i64>,
    now: chrono::DateTime<chrono::Local>,
) -> DueVerdict {
    match occurrence_after(sched, sched_last_dt(sched_last)) {
        Some(occ) if occ <= now => {
            if sched_last.is_some() && !sched.starts_with("at:") && now - occ > CATCHUP_WINDOW {
                DueVerdict::Missed
            } else {
                DueVerdict::Run
            }
        }
        _ => DueVerdict::NotDue,
    }
}

/// 找出到点的定时任务（未删、未归档、未完成，且 sched_last < 触发点 ≤ now）。
/// 顺带清理「错过的一次性任务」：at: 从未执行且时间已过 → 放弃并清掉 schedule
///（否则重启后 30s 内会补执行过期任务）
async fn find_due_tasks(app: &AppHandle) -> Vec<crate::db::Task> {
    let now = chrono::Local::now();
    let all = crate::db::db_load(app.clone()).await.unwrap_or_default();
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
    let mut missed_ids: Vec<String> = Vec::new();
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
            match classify_due(sched, t.sched_last, now) {
                DueVerdict::Run => Some(t),
                DueVerdict::Missed => {
                    crate::bot::audit_log(
                        app,
                        &format!(
                            "sched_missed | id: {} | schedule: {} | 到点已超 {}h 补跑窗口，跳过不补跑",
                            t.id,
                            crate::bot::truncate_for_log(sched, 40),
                            CATCHUP_WINDOW.num_hours()
                        ),
                    );
                    missed_ids.push(t.id.clone());
                    None
                }
                DueVerdict::NotDue => None,
            }
        })
        .collect();
    // 清理过期一次性任务的 schedule（基于最新数据合并，只清目标字段）。
    // 单次加载处理全部 stale id（避免每个 id 各开一次库，O(n) 次 db_load）
    if !stale_ids.is_empty() {
        if let Ok(cur) = crate::db::db_load(app.clone()).await {
            let fresh: Vec<crate::db::Task> = cur
                .into_iter()
                .filter(|t| stale_ids.contains(&t.id))
                .map(|mut t| {
                    t.expected_updated_at = t.updated_at; // RMW 基线 = 快照 updated_at
                    t.schedule = None;
                    t.updated_at = Some(now.timestamp_millis());
                    t
                })
                .collect();
            if !fresh.is_empty() {
                // 调度器写库也要广播（主窗口无轮询，不广播会长期显示旧的 ⏰ 徽标/备注）
                if crate::db::db_upsert(app.clone(), fresh.clone())
                    .await
                    .is_ok()
                {
                    crate::bot::broadcast_after_mutation(app, fresh, vec![]);
                }
            }
        }
    }
    // 消费超窗的 recurring occurrence（2h 补跑窗口见 CATCHUP_WINDOW）：
    // 不补跑，sched_last 记为现在——下一个 occurrence 顺延到未来，下个 tick 不再误判到点。
    // 与过期 at: 清理同模式：基于最新数据合并，只动 sched_last/updated_at。
    if !missed_ids.is_empty() {
        if let Ok(cur) = crate::db::db_load(app.clone()).await {
            let fresh: Vec<crate::db::Task> = cur
                .into_iter()
                .filter(|t| missed_ids.contains(&t.id))
                .map(|mut t| {
                    t.expected_updated_at = t.updated_at; // RMW 基线 = 快照 updated_at
                    t.sched_last = Some(now.timestamp_millis());
                    t.updated_at = Some(now.timestamp_millis());
                    t
                })
                .collect();
            if !fresh.is_empty() {
                if crate::db::db_upsert(app.clone(), fresh.clone())
                    .await
                    .is_ok()
                {
                    crate::bot::broadcast_after_mutation(app, fresh, vec![]);
                }
            }
        }
    }
    due
}

/// 执行一张到点的定时任务卡：先记 sched_last（防重复触发），跑执行循环，结果落备注标记
async fn run_scheduled(app: AppHandle, task: crate::db::Task) {
    // 防重入守卫：作用域结束（含 panic 展开）自动清理
    let Some(_sched_guard) = SchedGuard::acquire(&app, &task.id) else {
        return;
    };
    // 机器人开关关闭时不执行定时任务（开关不能只管 UI，后端也要拦）
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
    // 记录失败则放弃本次执行（否则下个 tick 会因 sched_last 未更新而重复触发）
    let mut marked = false;
    if let Ok(cur) = crate::db::db_load(app.clone()).await {
        if let Some(mut fresh) = cur.into_iter().find(|t| t.id == task.id) {
            fresh.expected_updated_at = fresh.updated_at; // RMW 基线 = 快照 updated_at
            fresh.sched_last = Some(now.timestamp_millis());
            fresh.updated_at = Some(now.timestamp_millis());
            marked = crate::db::db_upsert(app.clone(), vec![fresh.clone()])
                .await
                .is_ok();
            if marked {
                crate::bot::broadcast_after_mutation(&app, vec![fresh], vec![]);
            }
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

    let result = crate::bot_chat::run_task_in_chat(
        &app,
        &task.id,
        crate::bot_chat::TaskExecOrigin::Scheduled,
    )
    .await;
    let time_str = now.format("%m-%d %H:%M").to_string();
    // 定时执行完成/失败发系统通知（执行过程在新会话里
    // 流式可见、永久落库；通知只是提醒，点击拉起应用后按 ⏰ 前缀找到会话回看）。
    notify_scheduled_done(&app, &task.title, &result);

    // 执行结果写备注（模型可能已写摘要，这里前置 ⏰ 标记兜底）。
    // ⚠️ 必须基于执行后的最新数据合并：旧快照会把机器人执行期间的修改
    // （完成状态/摘要/子任务）整体回滚
    let summary = match &result {
        Ok(r) => format!(
            "⏰ 自动执行 {time_str}：{}",
            r.result.text.chars().take(300).collect::<String>()
        ),
        Err(e) => format!("⏰ 自动执行 {time_str} 失败：{e}"),
    };
    if let Ok(cur) = crate::db::db_load(app.clone()).await {
        if let Some(mut fresh) = cur.into_iter().find(|t| t.id == task.id) {
            let note = match fresh.note.as_deref().filter(|n| !n.trim().is_empty()) {
                Some(n) => format!("{summary}\n{n}"),
                None => summary,
            };
            fresh.note = Some(note);
            // 一次性定时执行完清掉 schedule（⏰ 徽标消失）。
            // 用执行后的 fresh.schedule 判断（执行期间用户改过定时，扫描快照会误清新设置）
            if fresh
                .schedule
                .as_deref()
                .is_some_and(|s| s.starts_with("at:"))
            {
                fresh.schedule = None;
            }
            fresh.expected_updated_at = fresh.updated_at; // RMW 基线 = 快照 updated_at
            fresh.updated_at = Some(chrono::Local::now().timestamp_millis());
            if crate::db::db_upsert(app.clone(), vec![fresh.clone()])
                .await
                .is_ok()
            {
                crate::bot::broadcast_after_mutation(&app, vec![fresh], vec![]);
            }
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
            let due = find_due_tasks(&app).await;
            for t in due {
                // 每张卡独立 spawn 且【不 await】：
                // spawn 后立即 await 的话，单张长任务（最坏可跑数小时）会堵死调度循环，
                // 后续所有到点任务排队。spawn 本身已隔离 panic（主循环不受影响）；
                // 同卡重入由 SchedGuard/ExecGuard 防护；另加单任务整体超时兜底。
                let app2 = app.clone();
                tauri::async_runtime::spawn(async move {
                    let id = t.id.clone();
                    let timed_out =
                        tokio::time::timeout(SCHED_TASK_TIMEOUT, run_scheduled(app2.clone(), t))
                            .await
                            .is_err();
                    if timed_out {
                        crate::bot::audit_log(
                            &app2,
                            &format!(
                                "sched_timeout | id: {id} | 超过 {} 分钟未完成，强制收尾",
                                SCHED_TASK_TIMEOUT.as_secs() / 60
                            ),
                        );
                        // 超时被 drop 的执行没走到 set_bot_assigned(false)，这里兜底复位
                        crate::bot_chat::set_bot_assigned(&app2, &id, false).await;
                    }
                });
            }
        }
    });
}

// ────────────────────────────────────────────────────────────────────
// 测试：定时解析纯函数（occurrence_after / at_expired）
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
    fn resolve_local_equals_single_for_normal_times() {
        // 非 DST 切换日的普通时刻：resolve_local 与 single 等价（DST 行为依赖系统
        // 时区，单测环境（国内无 DST）只能锁正常路径不回归）
        let naive = chrono::NaiveDate::from_ymd_opt(2026, 8, 16)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap();
        assert_eq!(
            resolve_local(naive),
            naive.and_local_timezone(chrono::Local).single()
        );
    }

    /// 回归锁：调度循环 spawn 后不得再立即 await
    ///（串行执行回退 = 一张长任务卡堵死全部后续到点任务），且必须有单任务整体超时
    #[test]
    fn scheduler_loop_does_not_await_each_task() {
        let text =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bot_scheduler.rs"))
                .unwrap();
        let pos = text
            .find("let due = find_due_tasks")
            .expect("调度循环必须存在");
        // 字符安全截取（中文注释多字节，字节下标切片会 panic）
        let scope: String = text[pos..].chars().take(1600).collect();
        assert!(
            !scope.contains("handle.await"),
            "调度循环不得串行 await 每张卡: {scope:?}"
        );
        assert!(
            scope.contains("SCHED_TASK_TIMEOUT"),
            "单任务必须有整体超时: {scope:?}"
        );
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

    // ── recurring 补跑 2h 时效窗口 ──

    fn ms(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        dt(y, mo, d, h, mi).timestamp_millis()
    }

    #[test]
    fn classify_due_fresh_within_window_runs() {
        // daily 10:00，昨天 10:00 跑过，现在 10:30——窗口内，正常执行
        let now = dt(2026, 8, 16, 10, 30);
        assert_eq!(
            classify_due("daily:10:00", Some(ms(2026, 8, 15, 10, 0)), now),
            DueVerdict::Run
        );
    }

    #[test]
    fn classify_due_stale_beyond_window_missed() {
        // daily 10:00，上次跑是 3 天前——下一 occurrence 在 3 天前，超窗 → 跳过
        let now = dt(2026, 8, 16, 12, 0);
        assert_eq!(
            classify_due("daily:10:00", Some(ms(2026, 8, 13, 10, 0)), now),
            DueVerdict::Missed
        );
        // weekly / monthly 同理
        assert_eq!(
            classify_due("weekly:1:09:00", Some(ms(2026, 8, 3, 9, 0)), now),
            DueVerdict::Missed
        );
        assert_eq!(
            classify_due("monthly:1:08:00", Some(ms(2026, 7, 1, 8, 0)), now),
            DueVerdict::Missed
        );
    }

    #[test]
    fn classify_due_window_boundary() {
        // 到点恰好 2h 前 → 仍执行；2h1s 前 → 跳过
        let last = ms(2026, 8, 15, 10, 0);
        assert_eq!(
            classify_due("daily:10:00", Some(last), dt(2026, 8, 16, 12, 0)),
            DueVerdict::Run
        );
        assert_eq!(
            classify_due("daily:10:00", Some(last), dt(2026, 8, 16, 12, 1)),
            DueVerdict::Missed
        );
    }

    #[test]
    fn classify_due_new_task_first_run_preserved() {
        // sched_last=None（新任务）保持「下一次触发立即生效」首跑语义，
        // 不因 occurrence 落在 1970 被误判 Missed
        let now = dt(2026, 8, 16, 12, 0);
        assert_eq!(classify_due("daily:10:00", None, now), DueVerdict::Run);
    }

    #[test]
    fn classify_due_at_never_missed() {
        // at: 一次性任务不走补跑窗口（由 at_expired 放弃逻辑处理）：
        // 过去的一次性任务保持既有 Run 判定（find_due_tasks 的 stale 清理负责清场）
        let now = dt(2026, 8, 16, 12, 0);
        assert_eq!(
            classify_due("at:2026-08-10T10:00", None, now),
            DueVerdict::Run
        );
    }

    #[test]
    fn classify_due_not_due() {
        let now = dt(2026, 8, 16, 9, 0);
        // 今天 10:00 未到
        assert_eq!(
            classify_due("daily:10:00", Some(ms(2026, 8, 15, 10, 0)), now),
            DueVerdict::NotDue
        );
        // 已跑过的一次性任务不再触发
        assert_eq!(
            classify_due(
                "at:2026-08-16T10:00",
                Some(ms(2026, 8, 16, 10, 0)),
                dt(2026, 8, 16, 11, 0)
            ),
            DueVerdict::NotDue
        );
        // 坏格式
        assert_eq!(classify_due("junk", None, now), DueVerdict::NotDue);
    }
}

/// `SchedGuard`（迁入 `AppState` 后）的守卫语义与实例隔离。
/// 这张表此前没有测试；本模块锁住「acquire 取注入实例 + Drop 清同一实例」这条设计，
/// 因为 Drop 里拿不到 `app`，全靠 acquire 时克隆的 Arc 句柄（写错就是守卫泄漏或误删）。
#[cfg(test)]
mod sched_guard_tests {
    use super::*;

    fn test_handle() -> tauri::AppHandle<tauri::test::MockRuntime> {
        let app = tauri::test::mock_app();
        tauri::Manager::manage(&app, crate::app_state::AppState::default());
        app.handle().clone()
    }

    fn len_of(app: &tauri::AppHandle<tauri::test::MockRuntime>) -> usize {
        crate::app_state::sched_running(app)
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    #[test]
    fn sched_guard_blocks_reentry_until_drop_on_injected_state() {
        let app = test_handle();
        let guard = SchedGuard::acquire(&app, "t-sched-1").expect("首次获取应成功");
        assert!(
            SchedGuard::acquire(&app, "t-sched-1").is_none(),
            "持有期间不得重复获取"
        );
        assert_eq!(len_of(&app), 1, "注入实例里应有 1 条在跑");
        drop(guard);
        // 注意：守卫必须绑定到变量——`assert!(SchedGuard::acquire(..).is_some())` 里它是临时值，
        // 语句结束即 Drop，会把刚插入的 id 又清掉（本测试第一次就是这么写错的）。
        let again = SchedGuard::acquire(&app, "t-sched-1").expect("Drop 后应可再获取");
        assert_eq!(
            len_of(&app),
            1,
            "Drop 清的必须是同一个实例（清错了这里就是 0）"
        );
        drop(again);
    }

    #[test]
    fn sched_guard_isolated_between_injected_instances() {
        let a = test_handle();
        let b = test_handle();
        let _ga = SchedGuard::acquire(&a, "same-id").expect("a 首次");
        let gb = SchedGuard::acquire(&b, "same-id")
            .expect("两个注入实例之间不得互相干扰（同一 id 应各自可持有）");
        assert_eq!(len_of(&a), 1);
        assert_eq!(len_of(&b), 1);
        drop(gb);
        assert_eq!(len_of(&b), 0, "Drop b 的守卫只应该清 b 的表");
        assert_eq!(len_of(&a), 1, "a 的登记不得被 b 的守卫清掉");
    }
}
