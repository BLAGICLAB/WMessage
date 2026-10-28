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
/// pub(crate)：提醒模块（bot_reminders）复用同一套触发点计算。
pub(crate) fn occurrence_after(
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
pub const SCHED_TICK_SECS: u64 = 30;

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

/// 读作业表：出错必须让调用方看见。吞成空表 = 整轮调度静默停摆，
/// 且恢复后错过窗口的作业会被当「错过消费」吃掉（永远不执行、只留一条 missed）
async fn load_jobs_for_sched(app: &AppHandle) -> Result<Vec<crate::db::ScheduledJob>, String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::db::open_db(&app).and_then(|conn| crate::db::load_scheduled_jobs(&conn))
    })
    .await
    .map_err(|e| format!("读作业表线程 join 失败：{e}"))?
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
    // 防重入守卫：与任务卡/工作流共用 SchedGuard 表（id 空间不相交）。
    // 拒绝也要留痕：ask_user 等待可达 24h，期间守卫不放，次日到点的同作业
    // 会被这里吞掉——没有审计行，用户只会以为定时坏了。
    let Some(_sched_guard) = SchedGuard::acquire(&app, &job.id) else {
        crate::bot::audit_log(
            &app,
            &format!(
                "sched_job_skip | id: {} | 上一次执行仍在进行，本次到点不触发",
                job.id
            ),
        );
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
    let last_status = job.last_status.clone();
    let last_error = job.last_error.clone();
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
            // panic 发生在收尾之前 → 待触发的重试还挂在行上：
            // 不清的话下一轮扫描它又到点、又跑一次、又 panic（每 30s 一轮的无限环）。
            // 取舍：若 panic 恰好落在「已写 retry_at 之后、audit/通知之前」的窄窗，
            // 这里会把刚承诺的重试一并清零、last_status 回滚——与 job_runs 已落库
            // 的 fail 行不一致。宁可少跑一次重试也不进无限环，按此取舍。
            clear_pending_retry_after_panic(&app_audit, &job_id, last_status, last_error).await;
        }
    });
}

/// panic 兜底：清掉待触发的重试（retry_at / retry_count 归零）。
/// 状态与错误保持原样——panic 时这次跑成什么样无从得知，不猜。
async fn clear_pending_retry_after_panic(
    app: &AppHandle,
    job_id: &str,
    last_status: Option<String>,
    last_error: Option<String>,
) {
    let id = job_id.to_string();
    let outcome = crate::db::lock::with_write_conn(app, move |conn| {
        crate::db::update_job_run_state(
            conn,
            &id,
            last_status.as_deref(),
            last_error.as_deref(),
            None,  // retry_at 清零：不再被扫描轮直接触发
            None,  // retry_count 清零：下一周期从零计
            false, // panic 不算「最终失败」，不自动暂停
            chrono::Local::now().timestamp_millis(),
        )
        .map_err(|e| e.to_string())
    })
    .await;
    if let Err(e) = outcome {
        crate::bot::audit_log(
            app,
            &format!(
                "sched_job_retry_clear_failed | id: {job_id} | {}",
                crate::bot::truncate_for_log(&e, 120)
            ),
        );
    }
}

/// 本轮对单个作业的处置（纯函数，便于单测）。
#[derive(Debug, PartialEq, Eq)]
enum JobAction {
    /// 到点且开关开启 → 派发执行
    Dispatch,
    /// 到点但机器人开关关闭 → 不派发，水位也不推进
    /// （恢复后 2h 窗口内补跑、超窗判 missed——与开关关闭前的语义一致）
    Held,
    /// 重试到点且开关开启 → 派发执行
    RetryDispatch,
    /// 重试未到点 → 本轮跳过（防与 occurrence 判定叠加重复触发）
    RetryPending,
    /// 重试到点但已拖过保鲜窗口（作业被暂停数日后恢复的典型形态）→
    /// 消费掉重试回到正常排期，不恢复瞬间立刻跑一次
    RetryStale,
    /// 过期的一次性作业 → 删除
    Delete,
    /// 错过补跑窗口 → 消费该 occurrence
    Missed,
    /// 未到点 → 本轮无事
    Idle,
}

/// 单个作业本轮怎么处置（纯函数）：判定顺序 = 重试 → 过期一次性 → 到点判定。
/// 机器人开关关闭时到点的作业判 Held（不派发、水位不推进）——
/// 派发了也会在 run_scheduled_job 里被拒，每 30s 一次 spawn + 一条跳过审计
/// 就是一天几千条噪声；而水位不推进才保住「恢复后补跑」的语义。
fn classify_job(
    job: &crate::db::ScheduledJob,
    now: chrono::DateTime<chrono::Local>,
    bot_on: bool,
) -> JobAction {
    if let Some(retry_at) = job.retry_at {
        if retry_at > now.timestamp_millis() {
            return JobAction::RetryPending;
        }
        // 重试保鲜窗口：与正常到点的补跑窗口同宽。重试本该几分钟内发生，
        // 拖过窗口（作业暂停期间数日、进程长期关闭）后再跑已无意义——
        // 恢复瞬间立刻执行一次只会让用户莫名其妙。
        if now.timestamp_millis() - retry_at > CATCHUP_WINDOW.num_milliseconds() {
            return JobAction::RetryStale;
        }
        return if bot_on {
            JobAction::RetryDispatch
        } else {
            JobAction::Held
        };
    }
    if at_expired(&job.schedule, job.sched_last, now) {
        return JobAction::Delete;
    }
    match classify_due(&job.schedule, job.sched_last, now) {
        DueVerdict::Run => {
            if bot_on {
                JobAction::Dispatch
            } else {
                JobAction::Held
            }
        }
        DueVerdict::Missed => JobAction::Missed,
        DueVerdict::NotDue => JobAction::Idle,
    }
}

/// 内容型作业单轮：过期 at: 清理 + 重试触发 + 错过消费 + 到点分发（语义与任务卡/工作流一致）
async fn jobs_tick(app: &AppHandle) {
    let now = chrono::Local::now();
    let jobs = match load_jobs_for_sched(app).await {
        Ok(j) => j,
        Err(e) => {
            crate::bot::audit_log(app, &format!("sched_jobs_scan_failed | {e}"));
            return;
        }
    };
    // 机器人开关关闭时本轮不派发任何作业（见 classify_job 的 Held）
    let bot_on = crate::bot_slash::bot_get_enabled(app.clone());
    for job in jobs {
        if !job.sched_enabled() {
            continue; // 暂停（保留配置）
        }
        match classify_job(&job, now, bot_on) {
            JobAction::Dispatch | JobAction::RetryDispatch => spawn_job_run(app, job),
            // 开关关闭：不派发，水位不推进（恢复后按原语义补跑或判 missed）
            JobAction::Held | JobAction::RetryPending | JobAction::Idle => {}
            // 过期重试：清 retry_at 回到正常排期（sched_last 是 occurrence
            // 水位，不动）；不清的话下一轮又判 RetryStale，天天一条审计噪声
            JobAction::RetryStale => {
                crate::bot::audit_log(
                    app,
                    &format!(
                        "sched_job_retry_expired | id: {} | 重试到点已超 {}h 保鲜窗口，丢弃重试回到正常排期",
                        job.id,
                        CATCHUP_WINDOW.num_hours()
                    ),
                );
                let job_id = job.id.clone();
                let now_ms = now.timestamp_millis();
                let _ = crate::db::lock::with_write_conn(app, move |conn| {
                    crate::db::update_job_run_state(
                        conn,
                        &job_id,
                        Some("retry_expired"),
                        None,
                        None, // retry_at 清零
                        None, // retry_count 清零：下一周期从零计
                        false,
                        now_ms,
                    )
                    .map_err(|e| e.to_string())
                })
                .await;
            }
            // 过期一次性（从未执行且已过点）→ 删除作业（防重启后补跑过期任务）
            JobAction::Delete => {
                crate::bot::audit_log(
                    app,
                    &format!("sched_job_stale | id: {} | 过期一次性作业，删除", job.id),
                );
                delete_job_row(app, &job.id).await;
            }
            JobAction::Missed => {
                crate::bot::audit_log(
                    app,
                    &format!(
                        "sched_job_missed | id: {} | schedule: {} | 到点已超 {}h 补跑窗口，跳过不补跑",
                        job.id,
                        crate::bot::truncate_for_log(&job.schedule, 40),
                        CATCHUP_WINDOW.num_hours()
                    ),
                );
                // 与 started/done/failed 同一事件面：错过也要让定时页的徽标
                // 即时翻「已错过」，等下次 reload 才出现就是静默
                let _ = app.emit(
                    "sched-status",
                    serde_json::json!({ "jobId": job.id, "phase": "skipped" }),
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
    // 提醒扫描（到点发系统通知，不建卡不执行）
    crate::bot_reminders::reminders_tick(app).await;
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
                    j.updated_at = Some(crate::db::now_ms());
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
mod sched_tests;

/// `SchedGuard`（迁入 `AppState` 后）的守卫语义与实例隔离。
/// 这张表此前没有测试；本模块锁住「acquire 取注入实例 + Drop 清同一实例」这条设计，
/// 因为 Drop 里拿不到 `app`，全靠 acquire 时克隆的 Arc 句柄（写错就是守卫泄漏或误删）。
#[cfg(test)]
mod sched_guard_tests;

#[cfg(test)]
mod plan_window_tests;
