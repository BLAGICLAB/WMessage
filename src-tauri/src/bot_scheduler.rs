//! 定时任务调度器：
//!
//! 负责 ⏰ 到点自动执行内容型定时作业（每日/每周/每月/一次性 at:），与 bot_chat.rs 的
//! 用户交互流解耦：
//! - 解析 schedule 字符串（parse_hm / occurrence_after / at_expired / sched_last_dt）
//! - 防重入守卫（SchedGuard，Drop 自动清理，防止 panic 后任务卡死锁）
//! - 30s 扫描循环（start_scheduler），到点新建任务卡并交 run_task_in_chat
//!  （任务执行聊天化：每次执行新建会话、流式可见、可按会话 /stop、完成/失败发
//!   系统通知）；绕开 exec_steps 逐步确认（无人在场，直接整体执行）

use chrono::Datelike;
use futures_util::FutureExt;
use tauri::{AppHandle, Emitter};
use tauri_plugin_notification::NotificationExt;

use crate::error::{CommandError, CommandResult};

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

/// 定时错过补跑窗口的系统通知（cron 监控核心教训：静默失败最危险）。
/// Missed 分支每个 occurrence 只走一次，天然去重；发送失败只记日志。
fn notify_schedule_missed(app: &AppHandle, target_title: &str) {
    let title_short = crate::bot::truncate_for_log(target_title.trim(), 30);
    if let Err(e) = app
        .notification()
        .builder()
        .title(format!("⏰ 定时已错过：{title_short}"))
        .body("到点已超 2 小时补跑窗口，本次已跳过；下一周期照常执行")
        .show()
    {
        eprintln!("[sched] 错过通知发送失败（未授权？）：{e}");
    }
}

// 定时任务卡（阶段二：⏰ 到点自动执行）

/// 正在执行的定时任务 id（防同一任务并发重复跑）
// SCHED_RUNNING 的定义与 sched_running() 访问器已集中到 `crate::app_state`
// （SchedGuard 本体与 Drop 清理语义未动）。
use crate::app_state::sched_running;

/// 调度防重入 RAII 守卫：Drop（含 panic 展开）时自动清理，保证任务 id 不残留
/// （清理不能只放在执行收尾处：panic 时该 id 会永久失效）
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
            let mut set = crate::db::lock::lock_or_recover(&running, "app_state::sched_running");
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
        crate::db::lock::lock_or_recover(&self.running, "bot_scheduler::SchedGuard::running")
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
        let weekday = after.weekday().num_days_from_monday() + 1; // 1=周一
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

/// 定时串格式校验（写入口单源）：只验格式合法性，不判过去/未来——
/// 过期的 at: 由调度器 at_expired 清理语义消费。作业写路径共用，防格式规则漂移。
pub(crate) fn validate_schedule(s: &str) -> Result<(), String> {
    if let Some(t) = s.strip_prefix("daily:") {
        return parse_hm(t)
            .map(|_| ())
            .ok_or_else(|| "daily 格式应为 daily:HH:MM（如 daily:09:30）".to_string());
    }
    if let Some(t) = s.strip_prefix("weekly:") {
        let bad = || "weekly 格式应为 weekly:D:HH:MM（D=1..7，周一起）".to_string();
        let Some((d, rest)) = t.split_once(':') else {
            return Err(bad());
        };
        let dow_ok = d
            .parse::<u32>()
            .map(|dow| (1..=7).contains(&dow))
            .unwrap_or(false);
        if !(dow_ok && parse_hm(rest).is_some()) {
            return Err(bad());
        }
        return Ok(());
    }
    if let Some(t) = s.strip_prefix("monthly:") {
        let bad = || "monthly 格式应为 monthly:DD:HH:MM（DD=1..31）".to_string();
        let Some((dd, rest)) = t.split_once(':') else {
            return Err(bad());
        };
        let day_ok = dd
            .parse::<u32>()
            .map(|day| (1..=31).contains(&day))
            .unwrap_or(false);
        if !(day_ok && parse_hm(rest).is_some()) {
            return Err(bad());
        }
        return Ok(());
    }
    if let Some(t) = s.strip_prefix("at:") {
        return chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M")
            .map(|_| ())
            .map_err(|_| "at 格式应为 at:YYYY-MM-DDTHH:MM（如 at:2026-10-08T14:00）".to_string());
    }
    Err("无法识别的定时格式（支持 daily:/weekly:/monthly:/at:）".to_string())
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

/// 调度扫描周期（秒），start_scheduler 的 interval 与 at: 首跑宽限共用同一常量防漂移
const SCHED_TICK_SECS: u64 = 30;

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

// 内容型定时作业（scheduled_jobs）

/// 作业标题截断（卡标题取内容首行；超长全文进 note 不丢语义）
const JOB_TITLE_MAX: usize = 80;

/// content → 新建任务卡的 (title, note)：首行为标题（截 JOB_TITLE_MAX），
/// 剩余行或多行全文进 note（标题截断时 note 存原文，不丢内容）
fn job_card_title_note(content: &str) -> (String, Option<String>) {
    let content = content.trim();
    let first_line = content.lines().next().unwrap_or("").trim();
    let mut title: String = first_line.chars().take(JOB_TITLE_MAX).collect();
    if title.is_empty() {
        title = content.chars().take(JOB_TITLE_MAX).collect();
    }
    let note = if content == title {
        None
    } else {
        Some(content.to_string())
    };
    (title, note)
}

async fn load_jobs_for_sched(app: &AppHandle) -> Vec<crate::db::ScheduledJob> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::db::open_db(&app).and_then(|conn| crate::db::load_scheduled_jobs(&conn))
    })
    .await
    .ok()
    .and_then(|r| r.ok())
    .unwrap_or_default()
}

/// scheduled_jobs 行定点更新（RMW：updated_at 匹配 `IS` 才写，防覆盖并发编辑）
async fn update_job_sched_fields(
    app: &AppHandle,
    id: &str,
    expected_updated_at: Option<i64>,
    sched_last: Option<i64>,
    now_ms: i64,
) -> bool {
    let id = id.to_string();
    let id_audit = id.clone();
    let outcome = crate::db::lock::with_write_conn(app, move |conn| {
        let n = conn
            .execute(
                "UPDATE scheduled_jobs SET sched_last = ?1, updated_at = ?2 \
                 WHERE id = ?3 AND updated_at IS ?4",
                rusqlite::params![sched_last, now_ms, id, expected_updated_at],
            )
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("updated_at 不匹配（期间被删除或修改），放弃写入".to_string());
        }
        Ok(())
    })
    .await;
    let ok = outcome.is_ok();
    if !ok {
        crate::bot::audit_log(
            app,
            &format!("sched_job_update_skip | id: {id_audit} | RMW 未写中，放弃本次触发"),
        );
    }
    ok
}

async fn delete_job_row(app: &AppHandle, id: &str) {
    let id = id.to_string();
    let id_audit = id.clone();
    let outcome = crate::db::lock::with_write_conn(app, move |conn| {
        crate::db::delete_scheduled_job(conn, &id)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .await;
    // 删除失败只留痕不重试：at: 的删除是「成功或最终失败都算已触发」的有意消费
    //（sched_last 已推进，残留行不会被再判到期），有意不加 CAS
    if let Err(e) = outcome {
        crate::bot::audit_log(
            app,
            &format!(
                "sched_job_delete_skip | id: {id_audit} | {}",
                crate::bot::truncate_for_log(&e, 120)
            ),
        );
    }
}

/// 到点执行一个内容型定时作业：新建任务卡 → 交机器人执行（run_task_in_chat
/// 既有链路：新会话流式可见、结果写卡、完成/失败系统通知全继承）。
async fn run_scheduled_job(app: AppHandle, job: crate::db::ScheduledJob) {
    // 防重入守卫：与任务卡/工作流共用 SchedGuard 表（id 空间不相交）
    let Some(_sched_guard) = SchedGuard::acquire(&app, &job.id) else {
        return;
    };
    // 机器人开关关闭时不触发（同任务卡/工作流：开关不能只管 UI）
    if !crate::bot_slash::bot_get_enabled(app.clone()) {
        crate::bot::audit_log(
            &app,
            &format!("sched_job_skip | id: {} | 机器人开关未开启", job.id),
        );
        return;
    }
    crate::bot::audit_log(
        &app,
        &format!(
            "sched_job_run | id: {} | schedule: {} | content: {}",
            job.id,
            job.schedule,
            crate::bot::truncate_for_log(&job.content, 60)
        ),
    );
    let now = chrono::Local::now();
    // 先记 sched_last（30s 扫描周期内不会重复触发；失败放弃本次，同任务卡）
    if !update_job_sched_fields(
        &app,
        &job.id,
        job.updated_at,
        Some(now.timestamp_millis()),
        now.timestamp_millis(),
    )
    .await
    {
        return;
    }
    // 根据内容新建任务卡（标题 = 内容首行截断，全文进 note 不丢语义）
    let (title, note) = job_card_title_note(&job.content);
    // 排期 = 定时触发时刻（向下吸附 30 分钟）起 30 分钟块；窗口外不设排期
    let plan_window = scheduled_plan_window(now);
    let mut card = crate::db::Task {
        plan_start: plan_window.as_ref().map(|(s, _)| s.clone()),
        plan_end: plan_window.as_ref().map(|(_, e)| e.clone()),
        parent_id: None,
        id: uuid::Uuid::new_v4().simple().to_string(),
        title: title.clone(),
        due: None,
        note,
        tags: None,
        files: None,
        column: crate::db::TaskStatus::Todo,
        subtasks: None,
        completed_at: None,
        archived: Some(false),
        deleted_at: None,
        collapsed: None,
        order: None,
        updated_at: Some(now.timestamp_millis()),
        bot_assigned: None,
        assignee: None,
        budget: None,
        result: None,
        depends_on: None,
        model: None,
        owner_id: None,
        created_at: Some(now.timestamp_millis()),
        expected_updated_at: None,
    };
    // 插到列表顶部（与 bot 建卡同规则：当前最小 order - 1）
    if let Ok(all) = crate::db::db_load(app.clone()).await {
        let min = all
            .iter()
            .filter_map(|t| t.order)
            .fold(f64::INFINITY, f64::min);
        card.order = Some(if min.is_finite() { min - 1.0 } else { 0.0 });
    }
    if let Err(e) = crate::db::db_upsert(app.clone(), vec![card.clone()]).await {
        // 建卡失败：回滚本次 sched_last 前推（RMW 基线 = 上一步写入的 updated_at）。
        // 不回滚的话 at: 一次性作业本次触发永久丢失（既不再到期也不算过期），
        // 周期作业也要白等一整个周期；回滚后下个扫描周期自动重试建卡。
        let reverted = update_job_sched_fields(
            &app,
            &job.id,
            Some(now.timestamp_millis()),
            job.sched_last,
            now.timestamp_millis(),
        )
        .await;
        crate::bot::audit_log(
            &app,
            &format!(
                "sched_job_err | id: {} | 建卡失败：{}；sched_last 回滚{}",
                job.id,
                crate::bot::truncate_for_log(&e.to_string(), 160),
                if reverted {
                    "成功，下个扫描周期重试"
                } else {
                    "失败，本次触发仍会丢弃"
                }
            ),
        );
        return;
    }
    crate::bot::broadcast_after_mutation(&app, vec![card.clone()], vec![]);

    // 交机器人执行（TaskExecOrigin::Scheduled：新会话 + ⏰ 前缀 + 结果写卡）
    // 执行状态实时广播（SchedulePage 行内「执行中」；补齐「执行中不可见」缺口）
    let _ = app.emit(
        "sched-status",
        serde_json::json!({ "taskId": card.id, "jobId": job.id, "phase": "started" }),
    );
    let fired_at = now.timestamp_millis();
    let started = std::time::Instant::now();
    let result = crate::bot_chat::run_task_in_chat(
        &app,
        &card.id,
        crate::bot_chat::TaskExecOrigin::Scheduled,
        None,
    )
    .await;
    // 收尾状态广播（sessionId 供前端跳 ⏰ 执行会话）
    let _ = app.emit(
        "sched-status",
        serde_json::json!({
            "taskId": card.id,
            "jobId": job.id,
            "phase": if result.is_ok() { "done" } else { "failed" },
            "sessionId": result.as_ref().ok().map(|r| r.session_id.clone()),
        }),
    );
    notify_scheduled_done(&app, &title, &result);
    crate::bot::audit_log(
        &app,
        &format!("sched_job_done | id: {} | card: {}", job.id, card.id),
    );

    // 收尾（借鉴 XXL-JOB 调度日志 + Temporal pauseOnFailure）：
    // 落历史 → 更新状态徽标 → 失败按 retry_max 排重试 → 重试用尽且开了自动暂停 → 暂停+通知
    let duration_ms = started.elapsed().as_millis() as i64;
    let err_text = result.as_ref().err().map(|e| e.message());
    let ok_summary = result
        .as_ref()
        .ok()
        .map(|r| r.result.text.trim().to_string())
        .filter(|s| !s.is_empty());
    finalize_job_run(
        &app,
        &job,
        result.is_ok(),
        err_text.as_deref(),
        ok_summary.as_deref(),
        fired_at,
        duration_ms,
        &card.id,
    )
    .await;
}

/// 作业执行收尾：历史落库 + 状态徽标 + 重试排程 + 失败自动暂停。
/// 一次性 at: 延迟到**最终结果**（成功或重试用尽）才删，给瞬时故障留重试机会。
async fn finalize_job_run(
    app: &AppHandle,
    job: &crate::db::ScheduledJob,
    ok: bool,
    err_text: Option<&str>,
    ok_summary: Option<&str>,
    fired_at: i64,
    duration_ms: i64,
    card_id: &str,
) {
    // 1) 历史落库（成功记摘要，失败记错误；截断防长文本撑表）
    {
        let job_id = job.id.clone();
        let card_id = card_id.to_string();
        let (status, summary) = if ok {
            (
                "ok",
                ok_summary.map(|s| s.chars().take(300).collect::<String>()),
            )
        } else {
            (
                "fail",
                Some(
                    err_text
                        .unwrap_or("未知错误")
                        .chars()
                        .take(300)
                        .collect::<String>(),
                ),
            )
        };
        let outcome = crate::db::lock::with_write_conn(app, move |conn| {
            crate::db::record_job_run(
                conn,
                &job_id,
                fired_at,
                status,
                Some(duration_ms),
                summary.as_deref(),
                Some(card_id.as_str()),
            )
            .map_err(|e| e.to_string())
        })
        .await;
        // 历史落库失败留痕：徽标/通知已发但历史缺行，静默丢会查不到执行记录
        if let Err(e) = outcome {
            crate::bot::audit_log(
                app,
                &format!(
                    "sched_job_history_skip | id: {} | {}",
                    job.id,
                    crate::bot::truncate_for_log(&e, 160)
                ),
            );
        }
    }
    // 2) 状态 / 重试 / 自动暂停裁决
    let retry_max = job.retry_max_or_zero();
    let retry_no = job.retry_count.unwrap_or(0);
    let will_retry = !ok && retry_no < retry_max;
    let final_fail = !ok && !will_retry;
    let auto_pause = final_fail && job.pause_on_failure == Some(true);
    let now_ms = chrono::Local::now().timestamp_millis();
    {
        let job_id = job.id.clone();
        let last_status = if ok { "ok" } else { "fail" };
        let last_error = if ok {
            None
        } else {
            err_text.map(|s| s.chars().take(300).collect::<String>())
        };
        let (retry_at, retry_count) = if will_retry {
            (Some(now_ms + crate::db::RETRY_DELAY_MS), Some(retry_no + 1))
        } else {
            (None, None) // 成功或重试用尽：清零（下一周期从零计）
        };
        let outcome = crate::db::lock::with_write_conn(app, move |conn| {
            crate::db::update_job_run_state(
                conn,
                &job_id,
                Some(last_status),
                last_error.as_deref(),
                retry_at,
                retry_count,
                auto_pause,
                now_ms,
            )
            .map_err(|e| e.to_string())
        })
        .await;
        // 状态落库失败留痕：UI 已报「已排重试/已自动暂停」但状态未写中，
        // 重试不会触发、暂停不生效，必须可查
        if let Err(e) = outcome {
            crate::bot::audit_log(
                app,
                &format!(
                    "sched_job_state_skip | id: {} | {}",
                    job.id,
                    crate::bot::truncate_for_log(&e, 160)
                ),
            );
        }
    }
    // 3) 通知 / 审计
    if will_retry {
        crate::bot::audit_log(
            app,
            &format!(
                "sched_job_retry | id: {} | 第 {}/{} 次重试已排（{} 分钟后）",
                job.id,
                retry_no + 1,
                retry_max,
                crate::db::RETRY_DELAY_MS / 60_000
            ),
        );
        return; // at: 一次性暂不删，等最终结果
    }
    if auto_pause {
        let title_short = crate::bot::truncate_for_log(&job.content, 30);
        if let Err(e) = app
            .notification()
            .builder()
            .title(format!("⏸ 定时任务已自动暂停：{title_short}"))
            .body("执行重试用尽仍失败；已暂停该定时任务，检查后在模块中恢复")
            .show()
        {
            eprintln!("[sched] 自动暂停通知发送失败（未授权？）：{e}");
        }
        crate::bot::audit_log(app, &format!("sched_job_autopaused | id: {}", job.id));
    }
    // 4) 一次性 at: 消费（成功或最终失败都算已触发）
    if job.schedule.starts_with("at:") {
        delete_job_row(app, &job.id).await;
    }
}

/// spawn 一个作业执行（panic 隔离 + 审计，供到点分发与重试触发共用）
fn spawn_job_run(app: &AppHandle, job: crate::db::ScheduledJob) {
    let app2 = app.clone();
    let app_audit = app.clone();
    let job_id = job.id.clone();
    tauri::async_runtime::spawn(async move {
        // per-task panic 可见性（同任务卡/工作流路径）
        let body = std::panic::AssertUnwindSafe(run_scheduled_job(app2, job));
        if let Err(panic) = body.catch_unwind().await {
            let msg = crate::audit::panic_message(panic);
            crate::bot::audit_log(
                &app_audit,
                &format!(
                    "sched_job_panic | id: {job_id} | {} | 作业 panic 已隔离",
                    crate::bot::truncate_for_log(&msg, 200)
                ),
            );
        }
    });
}

/// 内容型作业单轮：过期 at: 清理 + 重试触发 + 错过消费 + 到点分发（语义与任务卡/工作流一致）
async fn jobs_tick(app: &AppHandle) {
    let now = chrono::Local::now();
    for job in load_jobs_for_sched(app).await {
        if !job.sched_enabled() {
            continue; // 暂停（保留配置）
        }
        // 重试触发（XXL-JOB 失败重试借鉴）：retry_at 到点直接触发，不等下一周期 occurrence；
        // 未到点本轮跳过（防与 occurrence 判定叠加重复触发）
        if let Some(retry_at) = job.retry_at {
            if retry_at <= now.timestamp_millis() {
                spawn_job_run(app, job);
            }
            continue;
        }
        // 过期一次性（从未执行且已过点）→ 删除作业（防重启后补跑过期任务）
        if at_expired(&job.schedule, job.sched_last, now) {
            crate::bot::audit_log(
                app,
                &format!("sched_job_stale | id: {} | 过期一次性作业，删除", job.id),
            );
            delete_job_row(app, &job.id).await;
            continue;
        }
        match classify_due(&job.schedule, job.sched_last, now) {
            DueVerdict::Run => spawn_job_run(app, job),
            DueVerdict::Missed => {
                crate::bot::audit_log(
                    app,
                    &format!(
                        "sched_job_missed | id: {} | schedule: {} | 到点已超 {}h 补跑窗口，跳过不补跑",
                        job.id,
                        crate::bot::truncate_for_log(&job.schedule, 40),
                        CATCHUP_WINDOW.num_hours()
                    ),
                );
                notify_schedule_missed(app, &job.content);
                let _ = update_job_sched_fields(
                    app,
                    &job.id,
                    job.updated_at,
                    Some(now.timestamp_millis()),
                    now.timestamp_millis(),
                )
                .await;
            }
            DueVerdict::NotDue => {}
        }
    }
}

/// 定时作业建卡的排期窗口常量——与前端 `TaskTimeline/week.ts` 的
/// DAY_START_MIN / DAY_END_MIN / SNAP_MIN 三常量镜像（改一处必须同步
/// 另一处，否则网格裁剪/吸附口径漂移）：日窗口 8:00–18:00、吸附 30 分钟、
/// 定时卡默认时长 30 分钟。
const SCHED_PLAN_DAY_START_MIN: i64 = 8 * 60;
const SCHED_PLAN_DAY_END_MIN: i64 = 18 * 60;
const SCHED_PLAN_SNAP_MIN: i64 = 30;
const SCHED_PLAN_DUR_MIN: i64 = 30;

/// 定时作业生成卡的排期窗口：触发时刻向下吸附 30 分钟为块起点，
/// +30 分钟为块终点；起点或终点越出日窗口（8:00–18:00）→ None——
/// 窗口外不设排期，照旧建卡+执行（网格只画窗口内，越界块会被裁剪不可见，
/// 设了反而产生「看不见的排期」）。返回格式 "YYYY-MM-DDTHH:mm"
///（与 due / 前端 buildPlanDT 同构）。
fn scheduled_plan_window(now: chrono::DateTime<chrono::Local>) -> Option<(String, String)> {
    use chrono::Timelike;
    let minute_of_day = (now.hour() as i64) * 60 + (now.minute() as i64);
    let start_min = (minute_of_day / SCHED_PLAN_SNAP_MIN) * SCHED_PLAN_SNAP_MIN;
    let end_min = start_min + SCHED_PLAN_DUR_MIN;
    if start_min < SCHED_PLAN_DAY_START_MIN || end_min > SCHED_PLAN_DAY_END_MIN {
        return None;
    }
    let date = now.format("%Y-%m-%d").to_string();
    let hm = |m: i64| format!("{:02}:{:02}", m / 60, m % 60);
    Some((
        format!("{date}T{}", hm(start_min)),
        format!("{date}T{}", hm(end_min)),
    ))
}

/// 立即执行一个内容型定时作业（定时任务模块「立即执行」入口）：不等到点，
/// 走与到点触发完全相同的建卡 + 执行链路；at: 一次性同样消费（已触发即删）
#[tauri::command]
pub async fn scheduled_job_fire(app: AppHandle, id: String) -> CommandResult<()> {
    let (app_read, id_read) = (app.clone(), id.clone());
    let job =
        tauri::async_runtime::spawn_blocking(move || -> CommandResult<crate::db::ScheduledJob> {
            let conn = crate::db::open_db(&app_read)?;
            crate::db::load_scheduled_job(&conn, &id_read)
                .map_err(CommandError::from)?
                .ok_or(CommandError::TaskNotFound(id_read.clone()))
        })
        .await
        .map_err(|e| CommandError::from(format!("定时作业读取线程 join 失败：{e}")))??;
    // 有 guard 在跑（到点触发进行中）→ 静默跳过（与扫描分发同语义）
    if sched_running(&app)
        .lock()
        .map(|s| s.contains(&id))
        .unwrap_or(true)
    {
        return Ok(());
    }
    run_scheduled_job(app, job).await;
    Ok(())
}

/// 启动定时调度器：每 30s 扫一次到点作业并执行（App 启动时调用）
pub fn start_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(SCHED_TICK_SECS));
        ticker.tick().await; // 消耗首个立即触发的 tick
        loop {
            ticker.tick().await;
            // panic recovery：单轮 panic 不杀调度器，审计后续跑。
            // AssertUnwindSafe 安全依据：scheduler_tick 只读 db + spawn 任务，
            // 不持任何「跨 await 必须保持」的不变量，panic 后无可破坏状态。
            // 关闭信号不加：生命周期 = App，退出时 runtime 整体回收。
            let tick = std::panic::AssertUnwindSafe(scheduler_tick(&app));
            if let Err(panic) = tick.catch_unwind().await {
                let msg = crate::audit::panic_message(panic);
                crate::bot::audit_log(
                    &app,
                    &format!(
                        "sched_tick_panic | {} | 单轮 panic 已兜底，调度器继续",
                        crate::bot::truncate_for_log(&msg, 200)
                    ),
                );
            }
        }
    });
}

/// 单轮扫描 + 分发（从 start_scheduler 抽出以便 catch_unwind 兜底整轮）。
async fn scheduler_tick(app: &AppHandle) {
    // 内容型定时作业扫描（到点新建任务卡并执行）
    jobs_tick(app).await;
}

// 定时任务模块命令

/// 定时任务模块列表条目（schedule_overview 返回）。
/// nextRunAt 由 Rust 端 occurrence_after 单源计算（前端不重写解析器——
/// weekly:NaN 双端解析漂移事故的教训，见 src/format.ts 注释）。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleEntry {
    /// "job"（内容型，到点新建任务卡并执行）
    pub kind: &'static str,
    pub target_id: String,
    pub title: String,
    /// 任务卡 note 截断
    pub detail: Option<String>,
    pub schedule: String,
    pub sched_last: Option<i64>,
    /// 下次触发时间（epoch ms）；None = 无未来触发点（如已过期的 at:，将在下轮被清理）
    pub next_run_at: Option<i64>,
    /// 有 occurrence 已错过补跑窗口未执行（前端显示「已错过」徽标）
    pub missed: bool,
    /// false = 暂停（配置保留）
    pub enabled: bool,
    /// 最近一次最终执行结果：None=从未跑，Some("ok"/"fail")
    pub last_status: Option<String>,
    /// 最近一次失败的错误摘要
    pub last_error: Option<String>,
    /// 失败重试次数上限
    pub retry_max: i64,
    /// 失败后自动暂停
    pub pause_on_failure: bool,
}

fn build_schedule_entry(
    kind: &'static str,
    target_id: String,
    title: String,
    detail: Option<String>,
    sched: &str,
    sched_last: Option<i64>,
    enabled: bool,
    now: chrono::DateTime<chrono::Local>,
) -> ScheduleEntry {
    ScheduleEntry {
        kind,
        target_id,
        title,
        detail,
        schedule: sched.to_string(),
        sched_last,
        next_run_at: occurrence_after(sched, now).map(|t| t.timestamp_millis()),
        missed: matches!(classify_due(sched, sched_last, now), DueVerdict::Missed)
            || at_expired(sched, sched_last, now),
        enabled,
        last_status: None,
        last_error: None,
        retry_max: 0,
        pause_on_failure: false,
    }
}

/// 定时任务模块列表（内容型作业）。
#[tauri::command]
pub async fn schedule_overview(app: AppHandle) -> CommandResult<Vec<ScheduleEntry>> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::db::open_db(&app)?;
        let now = chrono::Local::now();
        let mut out = Vec::new();
        // 内容型作业（到点新建任务卡并执行）
        for j in crate::db::load_scheduled_jobs(&conn).map_err(CommandError::from)? {
            let sched = j.schedule.trim();
            if sched.is_empty() {
                continue;
            }
            let mut e = build_schedule_entry(
                "job",
                j.id.clone(),
                j.content.clone(),
                None,
                sched,
                j.sched_last,
                j.sched_enabled(),
                now,
            );
            e.last_status = j.last_status.clone();
            e.last_error = j.last_error.clone();
            e.retry_max = j.retry_max_or_zero();
            e.pause_on_failure = j.pause_on_failure == Some(true);
            out.push(e);
        }
        Ok(out)
    })
    .await
    .map_err(|e| CommandError::from(format!("定时列表线程 join 失败：{e}")))?
}

/// 暂停/恢复定时（定时任务模块）：只翻 enabled 位，schedule 配置原样保留。
/// 暂停期间调度器跳过；恢复后下个 tick 照常判定（错过的 occurrence 走补跑窗口语义）。
#[tauri::command]
pub async fn schedule_set_enabled(
    app: AppHandle,
    kind: String,
    id: String,
    enabled: bool,
) -> CommandResult<()> {
    match kind.as_str() {
        "job" => {
            let app_emit = app.clone();
            let row = tauri::async_runtime::spawn_blocking(
                move || -> CommandResult<crate::db::ScheduledJob> {
                    let _g = crate::db::lock_db_write();
                    let conn = crate::db::open_db(&app)?;
                    let mut j = crate::db::load_scheduled_job(&conn, &id)?
                        .ok_or(CommandError::TaskNotFound(id.clone()))?;
                    j.enabled = Some(enabled);
                    j.expected_updated_at = j.updated_at; // RMW 基线 = 快照 updated_at
                    j.updated_at = Some(chrono::Utc::now().timestamp_millis());
                    crate::db::upsert_scheduled_job(&conn, &j).map_err(CommandError::from)?;
                    Ok(j)
                },
            )
            .await
            .map_err(|e| CommandError::from(format!("定时开关线程 join 失败：{e}")))??;
            crate::audit::write_event(
                &app_emit,
                crate::audit::AuditLevel::Info,
                "sched_enabled",
                &[("jobId", row.id.clone()), ("enabled", enabled.to_string())],
            );
            Ok(())
        }
        other => Err(CommandError::InvalidArgument {
            field: "kind".into(),
            value: other.to_string(),
            reason: "kind 只能是 job".into(),
        }),
    }
}

// 测试：定时解析纯函数（occurrence_after / at_expired）

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
    fn validate_schedule_accepts_all_four_formats() {
        assert!(validate_schedule("daily:09:30").is_ok());
        assert!(validate_schedule("weekly:3:08:00").is_ok());
        assert!(validate_schedule("monthly:1:10:00").is_ok());
        assert!(validate_schedule("at:2026-10-08T14:00").is_ok());
        // 过期的 at: 也算格式合法（过期消费由 at_expired 语义负责）
        assert!(validate_schedule("at:2000-01-01T00:00").is_ok());
    }

    #[test]
    fn validate_schedule_rejects_malformed() {
        assert!(validate_schedule("weird").is_err());
        assert!(validate_schedule("daily:25:00").is_err());
        assert!(validate_schedule("daily:09").is_err());
        assert!(validate_schedule("weekly:8:08:00").is_err());
        assert!(validate_schedule("weekly:3:08").is_err());
        assert!(validate_schedule("monthly:32:10:00").is_err());
        assert!(validate_schedule("at:2026-10-08 14:00").is_err());
        assert!(validate_schedule("at:not-a-date").is_err());
        assert!(validate_schedule("").is_err());
    }

    #[test]
    fn job_card_title_note_extracts_first_line_and_truncates() {
        // 单行短内容 → 标题即全文，无 note
        let (t, n) = job_card_title_note("整理周报");
        assert_eq!(t, "整理周报");
        assert_eq!(n, None);
        // 多行 → 首行做标题，全文进 note 不丢语义
        let (t, n) = job_card_title_note("每日站会准备\n检查昨天的遗留\n整理今日议程");
        assert_eq!(t, "每日站会准备");
        assert_eq!(n, Some("每日站会准备\n检查昨天的遗留\n整理今日议程".into()));
        // 超长单行 → 标题截断 80 字，note 存原文
        let long = "长".repeat(120);
        let (t, n) = job_card_title_note(&long);
        assert_eq!(t.chars().count(), JOB_TITLE_MAX);
        assert_eq!(n, Some(long));
        // 空白容忍：trim 后入卡
        let (t, _) = job_card_title_note("  前后空白  ");
        assert_eq!(t, "前后空白");
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
        // 过去的一次性任务保持既有 Run 判定（jobs_tick 的 stale 清理负责清场）
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
        crate::db::lock::lock_or_recover(
            &crate::app_state::sched_running(app),
            "bot_scheduler::tests::sched_running (test)",
        )
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

#[cfg(test)]
mod plan_window_tests {
    use super::*;
    use chrono::TimeZone;

    fn at(h: u32, mi: u32) -> chrono::DateTime<chrono::Local> {
        chrono::Local
            .with_ymd_and_hms(2026, 10, 14, h, mi, 0)
            .single()
            .expect("valid local time")
    }

    #[test]
    fn in_window_snaps_and_sets_30min_block() {
        // 09:07 → 吸附 09:00–09:30
        let (s, e) = scheduled_plan_window(at(9, 7)).unwrap();
        assert_eq!(s, "2026-10-14T09:00");
        assert_eq!(e, "2026-10-14T09:30");
        // 17:50 → 吸附 17:30–18:00（终点恰在窗口边界，算入）
        let (s, e) = scheduled_plan_window(at(17, 50)).unwrap();
        assert_eq!(s, "2026-10-14T17:30");
        assert_eq!(e, "2026-10-14T18:00");
        // 08:00 整点即窗口起点
        assert!(scheduled_plan_window(at(8, 0)).is_some());
    }

    #[test]
    fn out_of_window_returns_none() {
        // 07:50 → 吸附 07:30，早于窗口起点 08:00
        assert!(scheduled_plan_window(at(7, 50)).is_none());
        // 18:05 → 吸附 18:00，终点 18:30 越出窗口终点
        assert!(scheduled_plan_window(at(18, 5)).is_none());
        // 凌晨 / 深夜作业（窗口外不设排期，照旧建卡+执行）
        assert!(scheduled_plan_window(at(0, 15)).is_none());
        assert!(scheduled_plan_window(at(22, 0)).is_none());
    }

    #[test]
    fn format_matches_build_plan_dt() {
        let (s, e) = scheduled_plan_window(at(9, 7)).unwrap();
        // 与前端 buildPlanDT 同格式：YYYY-MM-DDTHH:mm
        assert_eq!(s.len(), 16);
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[10..11], "T");
        assert_eq!(e.len(), 16);
    }
}
