//! 统一持久化副作用队列的派发器（表结构见 `db::effects`）：
//! 单 tokio 任务轮询 `effect_queue`，到点行交给对应 kind 的 handler，
//! 收尾用 `kind+key+seq+generation` 四元组 CAS 写回——成功删行、Later 顺延
//! 不计数、失败计数退避、超限停投递并审计。
//!
//! 入队方（8 个通知发送点等）commit 后调 [`kick`] 唤醒；启动即扫一轮，
//! 上次崩溃遗留的 due 行自然重跑（at-least-once，消费方必须幂等）。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tauri::{AppHandle, Manager};

use crate::db::effects::EffectRow;

/// 通知投递 kind（系统通知：8 个发送点统一走队列重试）
pub(crate) const KIND_NOTIFY: &str = "notify";
/// 子 agent 终态落库 kind：进程内重试不持久化，崩溃即丢——改队列续跑
pub(crate) const KIND_SUBAGENT_FINALIZE: &str = "subagent_finalize";
/// 子 agent runner 派发 kind：「插行了但 runner 没起」变成可恢复待办
pub(crate) const KIND_SUBAGENT_DISPATCH: &str = "subagent_dispatch";

/// 派发待办的业务 key（入队方与清扫豁免判断共用，防两处拼串漂移）
pub(crate) fn subagent_dispatch_key(subagent_id: &str) -> String {
    format!("subagent_dispatch:{subagent_id}")
}

/// handler 失败分类：
/// - [`EffectError::Later`]：现在跑不了（环境未就绪等），只顺延 due、不消耗重试配额
/// - [`EffectError::Fail`]：真失败，attempts+1、指数退避、超限停投递
#[derive(Debug)]
pub(crate) enum EffectError {
    Later,
    Fail(String),
}

/// 每 kind 配置：并发槽位 / 退避基线与上限 / 重试上限（超限留行停投递）
#[derive(Clone, Copy)]
struct KindConfig {
    limit: usize,
    base_ms: u64,
    cap_ms: u64,
    max_attempts: i64,
}

fn config_of(kind: &str) -> Option<KindConfig> {
    match kind {
        KIND_NOTIFY => Some(KindConfig {
            limit: 2,
            base_ms: 2_000,
            cap_ms: 60_000,
            max_attempts: 5,
        }),
        KIND_SUBAGENT_FINALIZE => Some(KindConfig {
            limit: 4,
            base_ms: 1_000,
            cap_ms: 16_000,
            max_attempts: 5,
        }),
        KIND_SUBAGENT_DISPATCH => Some(KindConfig {
            limit: 4,
            base_ms: 2_000,
            cap_ms: 60_000,
            max_attempts: 5,
        }),
        _ => None,
    }
}

fn registered_kinds() -> [&'static str; 3] {
    [KIND_NOTIFY, KIND_SUBAGENT_FINALIZE, KIND_SUBAGENT_DISPATCH]
}

/// 退避 `min(cap, base × 2^attempts)`（attempts 为本次失败前的计次）
fn backoff_delay(cfg: &KindConfig, attempts: i64) -> u64 {
    let exp = attempts.clamp(0, 16) as u32;
    cfg.base_ms.saturating_mul(1u64 << exp).min(cfg.cap_ms)
}

static KICK: std::sync::OnceLock<tokio::sync::mpsc::UnboundedSender<()>> =
    std::sync::OnceLock::new();

/// 唤醒派发器（入队 commit 后调用；未启动时静默——测试环境无派发器）
pub(crate) fn kick() {
    if let Some(tx) = KICK.get() {
        let _ = tx.send(());
    }
}

/// 启动派发器（App setup 调一次；重复调用是编程错误，静默跳过）
pub(crate) fn spawn_dispatcher(app: &AppHandle) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    if KICK.set(tx).is_err() {
        return;
    }
    let dispatcher = Arc::new(Dispatcher {
        app: app.clone(),
        inflight: std::sync::Mutex::new(HashMap::new()),
    });
    tauri::async_runtime::spawn(dispatcher.clone().run(rx));
}

struct Dispatcher {
    app: AppHandle,
    /// in-flight 行的 key 集合（per-kind）：并发放 入口的「不重复派同一行」由
    /// dispatch_cycle 过滤保证，补位唤醒由 attempt 收尾 kick 触发
    inflight: std::sync::Mutex<HashMap<&'static str, HashSet<String>>>,
}

impl Dispatcher {
    async fn run(self: Arc<Self>, mut rx: tokio::sync::mpsc::UnboundedReceiver<()>) {
        loop {
            self.dispatch_cycle().await;
            // 下一唤醒点 = 最早 due；无行时睡长觉等 kick
            let next = read_next_due(&self.app).await;
            let now = crate::db::now_ms();
            let delay = match next {
                Some(d) => (d - now).clamp(50, 3_600_000) as u64,
                None => 3_600_000,
            };
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_millis(delay)) => {}
                _ = rx.recv() => {}
            }
        }
    }

    async fn dispatch_cycle(self: &Arc<Self>) {
        let now = crate::db::now_ms();
        for kind in registered_kinds() {
            let Some(cfg) = config_of(kind) else {
                continue;
            };
            let rows = fetch_due_rows(&self.app, kind, now, cfg.max_attempts).await;
            let rows = match rows {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("[effects] 抓取待派发行失败：{e}");
                    continue;
                }
            };
            let busy = self.busy_keys(kind);
            let room = cfg.limit.saturating_sub(busy.len());
            let ready: Vec<EffectRow> = rows
                .into_iter()
                .filter(|r| !busy.contains(&r.key))
                .take(room)
                .collect();
            for row in ready {
                self.claim(kind, &row.key);
                self.spawn_attempt(kind, cfg, row);
            }
        }
    }

    fn busy_keys(&self, kind: &'static str) -> HashSet<String> {
        let m = crate::db::lock::lock_or_recover(&self.inflight, "bot::effects::inflight");
        m.get(kind).cloned().unwrap_or_default()
    }

    fn claim(&self, kind: &'static str, key: &str) {
        let mut m = crate::db::lock::lock_or_recover(&self.inflight, "bot::effects::inflight");
        m.entry(kind).or_default().insert(key.to_string());
    }

    fn release(&self, kind: &'static str, key: &str) {
        let mut m = crate::db::lock::lock_or_recover(&self.inflight, "bot::effects::inflight");
        if let Some(set) = m.get_mut(kind) {
            set.remove(key);
        }
    }

    fn spawn_attempt(self: &Arc<Self>, kind: &'static str, cfg: KindConfig, row: EffectRow) {
        let this = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            this.attempt(kind, cfg, row).await;
        });
    }

    /// 跑一次 handler 并 CAS 收尾。崩溃在任何一步：行留在库里（due 已过或
    /// 未收尾），重启后重跑——at-least-once。
    async fn attempt(self: Arc<Self>, kind: &'static str, cfg: KindConfig, row: EffectRow) {
        let outcome = run_handler(&self.app, kind, &row.key, &row.payload).await;
        let fail_msg = match &outcome {
            Err(EffectError::Fail(e)) => Some(e.clone()),
            _ => None,
        };
        let backoff = backoff_delay(&cfg, row.attempts);
        let app2 = self.app.clone();
        let row2 = row.clone();
        let result = crate::db::lock::with_write_conn(&self.app, move |conn| match outcome {
            Ok(()) => crate::db::effects::finish_ok(conn, &row2).map(|_| None),
            Err(EffectError::Later) => {
                let due = crate::db::now_ms() + cfg.base_ms as i64;
                crate::db::effects::reschedule(conn, &row2, due).map(|_| None)
            }
            Err(EffectError::Fail(err)) => {
                let due = crate::db::now_ms() + backoff as i64;
                crate::db::effects::record_failure(conn, &row2, &truncate_err(&err), due)
            }
        })
        .await;
        match result {
            Ok(Some(attempts)) if attempts >= cfg.max_attempts => {
                give_up_audit(&app2, kind, &row, fail_msg.as_deref().unwrap_or("未知错误"));
            }
            Ok(_) => {}
            Err(e) => {
                // 收尾写失败：行留原状，due 到点后重跑（可能重复执行一次，at-least-once）
                eprintln!("[effects] 收尾写失败（行留原状待重跑）：{e}");
            }
        }
        self.release(kind, &row.key);
        kick();
    }
}

async fn read_next_due<R: tauri::Runtime>(app: &AppHandle<R>) -> Option<i64> {
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Option<i64>, String> {
        let conn = crate::db::open_db(&app2)?;
        crate::db::effects::next_due(&conn)
    })
    .await
    .unwrap_or_else(|e| Err(format!("effect 派发线程失败：{e}")))
    .unwrap_or(None)
}

async fn fetch_due_rows<R: tauri::Runtime>(
    app: &AppHandle<R>,
    kind: &str,
    now: i64,
    max_attempts: i64,
) -> Result<Vec<EffectRow>, String> {
    let app2 = app.clone();
    let kind = kind.to_string();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<EffectRow>, String> {
        let conn = crate::db::open_db(&app2)?;
        crate::db::effects::fetch_due(&conn, &kind, now, max_attempts, 32)
    })
    .await
    .unwrap_or_else(|e| Err(format!("effect 派发线程失败：{e}")))
}

/// 简单通知入队：单语句自带原子性（需要与业务写同事务的组合方直接调
/// `db::effects::enqueue` + `kick`）。入队即返回，投递由派发器接管——
/// 失败重试、超限审计，绝不静默。
pub(crate) async fn enqueue_notify<R: tauri::Runtime>(
    app: &AppHandle<R>,
    key: &str,
    title: String,
    body: String,
) -> Result<(), String> {
    let payload = serde_json::json!({ "title": title, "body": body }).to_string();
    let key = key.to_string();
    let due = crate::db::now_ms();
    crate::db::lock::with_write_conn(app, move |conn| {
        crate::db::effects::enqueue(conn, KIND_NOTIFY, &key, &payload, due)
    })
    .await?;
    kick();
    Ok(())
}

async fn run_handler(
    app: &AppHandle,
    kind: &str,
    key: &str,
    payload: &str,
) -> Result<(), EffectError> {
    match kind {
        KIND_NOTIFY => deliver_notify(app, key, payload).await,
        KIND_SUBAGENT_FINALIZE => finalize_subagent(app, payload).await,
        KIND_SUBAGENT_DISPATCH => dispatch_subagent(app, payload).await,
        other => Err(EffectError::Fail(format!("未注册的 effect kind：{other}"))),
    }
}

#[derive(serde::Deserialize)]
struct NotifyPayload {
    title: String,
    body: String,
}

async fn deliver_notify<R: tauri::Runtime>(
    app: &AppHandle<R>,
    key: &str,
    payload: &str,
) -> Result<(), EffectError> {
    let p: NotifyPayload = serde_json::from_str(payload)
        .map_err(|e| EffectError::Fail(format!("通知 payload 解析失败：{e}")))?;
    let Some(state) = app.try_state::<tauri_plugin_notification::Notification<R>>() else {
        // 插件未注册（测试 mock）：环境问题不算失败，等就绪
        return Err(EffectError::Later);
    };
    state
        .inner()
        .builder()
        .title(&p.title)
        .body(&p.body)
        .show()
        .map_err(|e| EffectError::Fail(format!("系统通知发送失败：{e}")))?;
    if let Some(id) = key.strip_prefix("reminder:") {
        crate::audit_event!(
            app,
            crate::audit::AuditLevel::Info,
            "reminder.fired",
            "id" => id,
            "text" => crate::bot::truncate_for_log(&p.body, 60)
        );
    }
    Ok(())
}

/// 重试上限耗尽：行留库（同 key 再入队即复活），失败必须可查
fn give_up_audit<R: tauri::Runtime>(app: &AppHandle<R>, kind: &str, row: &EffectRow, err: &str) {
    if let Some(id) = row.key.strip_prefix("reminder:") {
        crate::audit_event!(
            app,
            crate::audit::AuditLevel::Warn,
            "reminder.fire_failed",
            "id" => id,
            "err" => crate::bot::truncate_for_log(err, 120)
        );
    } else if let Some(id) = row.key.strip_prefix("subagent_finalize:") {
        // 保留存量事件名：终态写库最终失败（原进程内重试耗尽的同名审计）
        crate::audit_event!(
            app,
            crate::audit::AuditLevel::Warn,
            "subagent_terminal_state_write_failed",
            "id" => id,
            "err" => crate::bot::truncate_for_log(err, 120)
        );
    } else if let Some(id) = row.key.strip_prefix("subagent_dispatch:") {
        crate::audit_event!(
            app,
            crate::audit::AuditLevel::Warn,
            "subagent_dispatch_giveup",
            "id" => id,
            "err" => crate::bot::truncate_for_log(err, 120)
        );
    } else {
        crate::audit_event!(
            app,
            crate::audit::AuditLevel::Warn,
            "effect_giveup",
            "kind" => kind,
            "key" => row.key,
            "err" => crate::bot::truncate_for_log(err, 120)
        );
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct FinalizePayload {
    subagent_id: String,
    status: String,
    finished_at: i64,
    error: String,
}

/// 子 agent 终态落库（幂等）：行已是任意终态即视为完成（重放、与正常收尾
/// 赛跑都安全），否则按状态机写入目标终态。
async fn finalize_subagent<R: tauri::Runtime>(
    app: &AppHandle<R>,
    payload: &str,
) -> Result<(), EffectError> {
    let p: FinalizePayload = serde_json::from_str(payload)
        .map_err(|e| EffectError::Fail(format!("终态 payload 解析失败：{e}")))?;
    let Some(status) = crate::db::subagents::SubagentStatus::from_str(&p.status) else {
        return Err(EffectError::Fail(format!("未知终态：{}", p.status)));
    };
    crate::db::lock::with_write_conn(app, move |conn| -> Result<(), String> {
        if let Some(row) =
            crate::db::load_subagent(conn, &p.subagent_id).map_err(|e| e.to_string())?
        {
            if row.status.is_terminal() {
                return Ok(()); // 已有终态（重放或赛跑），幂等成功
            }
        }
        crate::db::update_subagent_status(
            conn,
            &p.subagent_id,
            status,
            p.finished_at,
            Some(&p.error),
        )
        .map(|_| ())
    })
    .await
    .map_err(EffectError::Fail)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct OnionPayload {
    config: String,
    max_turn: Option<u32>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DispatchPayload {
    subagent_id: String,
    stage: Option<String>,
    onion: Option<OnionPayload>,
}

/// 子 agent runner 派发（幂等）：行已不在 queued（被取消/已被接管/已终态）
/// 即视为完成；queued 则后台起 runner，handler 立即返回。同 key 进程内不
/// 并发（in-flight 去重），跨重启无双跑（旧进程已死，行在 queued 才重派）。
async fn dispatch_subagent(app: &AppHandle, payload: &str) -> Result<(), EffectError> {
    let p: DispatchPayload = serde_json::from_str(payload)
        .map_err(|e| EffectError::Fail(format!("派发 payload 解析失败：{e}")))?;
    let stage = p
        .stage
        .as_deref()
        .and_then(crate::bot_orchestrator::SubagentStage::from_str);
    let onion = p.onion.map(|o| crate::bot_orchestrator::SpawnOnion {
        config: o.config,
        max_turn: o.max_turn,
    });
    let sid = p.subagent_id.clone();
    let status = crate::db::lock::with_write_conn(app, move |conn| -> Result<String, String> {
        crate::db::load_subagent(conn, &sid)
            .map_err(|e| e.to_string())?
            .map(|row| row.status.as_str().to_string())
            .ok_or_else(|| format!("子 agent 行不存在：{sid}"))
    })
    .await
    .map_err(EffectError::Fail)?;
    if status != crate::db::subagents::SubagentStatus::Queued.as_str() {
        return Ok(()); // 幂等：已被接管/取消/终态
    }
    // 后台起 runner（handler 立即返回）；run_subagent 具化 Wry（run_model_loop
    // 生产壳），派发器链路因此具化。同 key 进程内不并发（in-flight 去重），
    // 跨重启无双跑（旧进程已死，行在 queued 才重派）
    let app2 = app.clone();
    let sid2 = p.subagent_id;
    tauri::async_runtime::spawn(async move {
        crate::bot_orchestrator::run_subagent(app2, sid2, stage, onion).await;
    });
    Ok(())
}

fn truncate_err(err: &str) -> String {
    err.chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// attempt 测试全程读写共享测试库（同 pid 并行），两两互斥防串行干扰
    /// （async 测试体要跨 await，用 tokio 的异步锁）
    static ATTEMPT_DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// 共享测试库按 pid 复用：清掉其他测试遗留的 effect 行，保证行数断言纯净
    async fn clear_effect_queue<R: tauri::Runtime>(handle: &AppHandle<R>) {
        crate::db::lock::with_write_conn(handle, |conn| {
            conn.execute("DELETE FROM effect_queue", [])
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
        .await
        .unwrap();
    }

    #[test]
    fn backoff_grows_exponentially_and_caps() {
        let cfg = KindConfig {
            limit: 1,
            base_ms: 2_000,
            cap_ms: 60_000,
            max_attempts: 5,
        };
        assert_eq!(backoff_delay(&cfg, 0), 2_000);
        assert_eq!(backoff_delay(&cfg, 1), 4_000);
        assert_eq!(backoff_delay(&cfg, 3), 16_000);
        assert_eq!(backoff_delay(&cfg, 10), 60_000, "封顶不超 cap");
        assert_eq!(backoff_delay(&cfg, -3), 2_000, "负计次按 0 算");
    }

    #[tokio::test]
    async fn bad_payload_is_fail_and_plugin_missing_is_later() {
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        let err = deliver_notify(&handle, "k", "not-json").await;
        assert!(
            matches!(err, Err(EffectError::Fail(_))),
            "坏 payload 应 Fail"
        );
        let err = deliver_notify(&handle, "reminder:r1", r#"{"title":"t","body":"b"}"#).await;
        assert!(
            matches!(err, Err(EffectError::Later)),
            "mock 无通知插件应 Later"
        );
    }

    /// 终态落库 handler：queued 行写入目标终态；重放（崩溃后重跑）幂等成功
    #[tokio::test]
    async fn finalize_subagent_writes_terminal_and_is_idempotent() {
        let _g = ATTEMPT_DB_LOCK.lock().await;
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        let sid = "sa_effects_t1";
        let profile = crate::db::SubagentProfile::General;
        let row = crate::db::SubagentRow {
            id: sid.to_string(),
            status: crate::db::SubagentStatus::Queued,
            profile,
            model: None,
            parent_session_id: None,
            session_id: None,
            task_id: None,
            objective: "测试目标".into(),
            trace_id: "trc_t1".into(),
            acceptance_json: None,
            context_summary: None,
            result_json: None,
            error: None,
            created_at: 1,
            started_at: None,
            finished_at: None,
            prompt_tokens: None,
            completion_tokens: None,
            cache_read_tokens: None,
        };
        crate::db::lock::with_write_conn(&handle, move |conn| {
            crate::db::insert_subagent(conn, &row)
        })
        .await
        .unwrap();
        let payload = serde_json::json!({
            "subagentId": sid,
            "status": "cancelled",
            "finishedAt": 123,
            "error": "测试取消",
        })
        .to_string();
        finalize_subagent(&handle, &payload).await.unwrap();
        // 重放（崩溃后重跑同一条）：已终态 → 幂等成功，不改写
        finalize_subagent(&handle, &payload).await.unwrap();
        let (status, error): (String, String) =
            crate::db::lock::with_write_conn(&handle, move |conn| {
                conn.query_row(
                    "SELECT status, IFNULL(error, '') FROM subagents WHERE id = ?1",
                    [sid],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(|e| e.to_string())
            })
            .await
            .unwrap();
        assert_eq!(
            status, "cancelled",
            "终态应已落库（queued→cancelled 合法转移）"
        );
        assert_eq!(error, "测试取消");
    }

    /// 终态写库 handler 的重试闭环：payload 解析失败 → Fail 计数退避，重放
    /// 到 max_attempts 后停投递（attempt 收尾写库的语义在 db::effects 层已测，
    /// 这里锁「Fail 分类会把配额烧完」的闭环）
    #[tokio::test]
    async fn finalize_bad_payload_burns_retry_budget_and_stops() {
        let _g = ATTEMPT_DB_LOCK.lock().await;
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        clear_effect_queue(&handle).await;
        let key = "subagent_finalize:sa_bad";
        let due = crate::db::now_ms();
        let (k1, d1) = (key.to_string(), due);
        crate::db::lock::with_write_conn(&handle, move |conn| {
            crate::db::effects::enqueue(conn, KIND_SUBAGENT_FINALIZE, &k1, "not-json", d1)
        })
        .await
        .unwrap();
        let cfg = config_of(KIND_SUBAGENT_FINALIZE).unwrap();
        let horizon = crate::db::now_ms() + 3_600_000;
        for i in 1..=cfg.max_attempts {
            let rows = fetch_due_rows(&handle, KIND_SUBAGENT_FINALIZE, horizon, cfg.max_attempts)
                .await
                .unwrap();
            assert_eq!(rows.len(), 1, "第 {i} 轮仍应抓到（未超限）");
            let row = rows.into_iter().next().unwrap();
            // attempt 的收尾语义（Fail → record_failure）按派发器同款内联执行
            let outcome = finalize_subagent(&handle, &row.payload).await;
            let fail_msg = match &outcome {
                Err(EffectError::Fail(e)) => e.clone(),
                other => panic!("预期 Fail，实际 {other:?}"),
            };
            let backoff = backoff_delay(&cfg, row.attempts);
            let due = crate::db::now_ms() + backoff as i64;
            let row2 = row.clone();
            let attempts = crate::db::lock::with_write_conn(&handle, move |conn| {
                crate::db::effects::record_failure(conn, &row2, &truncate_err(&fail_msg), due)
            })
            .await
            .unwrap()
            .unwrap();
            assert_eq!(attempts, i, "第 {i} 次失败后计次应累进");
        }
        let rows = fetch_due_rows(&handle, KIND_SUBAGENT_FINALIZE, horizon, cfg.max_attempts)
            .await
            .unwrap();
        assert!(rows.is_empty(), "达到 max_attempts 后停投递");
    }
}
