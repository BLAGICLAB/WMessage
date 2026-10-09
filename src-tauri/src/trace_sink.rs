//! 执行痕迹采集管道（Agent 透明化设计 §4.2，/d）：
//!
//! - **采集**：dispatch 每次工具调用产一条 span（含文件变更证据），`record_*` 经
//!   mpsc fire-and-forget 进后台 writer——观测面失败/满队列一律不阻断工具结果。
//! - **落库**：writer 任务持有长连接攒批（try_recv 排空，上限 64 条/批）事务写入；
//!   写失败丢弃半批并重开连接（open_db 幂等）。与主写者共用 `DB_WRITE_LOCK` 纪律。
//! - **生命周期**：`begin_trace` / `end_trace` 由 run_task_in_chat_with 在执行首尾调用
//!   （建 exec_traces 行 + 注册 `AppState.trace_registry`；收尾汇总 LoopTrace 统计 + 注销）。
//!   registry 无映射的会话（主聊天 / DSL 调度器遗留路径）零采集开销。
//!
//! 隐私：span 与 diff 全文只落本地 SQLite（SPAN_TEXT_MAX 钳制），不进任何上报管道。

use std::sync::OnceLock;
use tauri::AppHandle;
use tokio::sync::mpsc;

/// 单批落库上限（channel 里就绪多少收多少，最多 64 条/事务）
const WRITE_BATCH_MAX: usize = 64;

// 采集记录（owned，跨 channel 传输）

pub struct SpanRecord {
    pub trace_id: i64,
    /// 模型循环轮次（无循环上下文的调用方传 0；span 只在 trace 存在的执行流里产生）
    pub turn: i64,
    pub tool_call_id: Option<String>,
    pub name: String,
    pub args: Option<String>,
    pub result: Option<String>,
    pub ok: bool,
    ///  预留（恒 None）； 接 evolution error_kind 分类器
    pub error_class: Option<String>,
    pub duration_ms: Option<i64>,
    pub created_at: i64,
}

pub struct FileChangeRecord {
    pub trace_id: i64,
    /// span 关联留 P1-c+（span 与 change 异步入库，行级外联不做，按 trace_id + 时间线对齐）
    pub path: String,
    pub kind: &'static str,
    pub added: i64,
    pub deleted: i64,
    pub diff: Option<String>,
    pub truncated: bool,
    pub before_ref: Option<String>,
    pub before_sha: Option<String>,
    pub after_sha: Option<String>,
    pub created_at: i64,
}

/// 由 bot_fs::FileChangeReceipt 映射（dispatch 侧调用）
pub(crate) fn file_change_record(
    trace_id: i64,
    rc: &crate::bot_fs::FileChangeReceipt,
) -> FileChangeRecord {
    FileChangeRecord {
        trace_id,
        path: rc.path.clone(),
        kind: rc.kind,
        added: rc.added,
        deleted: rc.deleted,
        diff: rc.diff.clone(),
        truncated: rc.truncated,
        before_ref: rc.before_ref.clone(),
        before_sha: rc.before_sha.clone(),
        after_sha: rc.after_sha.clone(),
        created_at: chrono::Utc::now().timestamp_millis(),
    }
}

enum TraceWrite {
    Span(SpanRecord),
    FileChange(FileChangeRecord),
}

static SINK: OnceLock<mpsc::Sender<TraceWrite>> = OnceLock::new();

/// lib.rs setup 启动（OnceLock 幂等；重复调用静默跳过）。泛型 Runtime——测试可接 mock。
pub fn init<R: tauri::Runtime>(app: AppHandle<R>) {
    let (tx, rx) = mpsc::channel(2048);
    if SINK.set(tx).is_err() {
        return;
    }
    tauri::async_runtime::spawn(writer(app, rx));
}

/// dispatch 工具执行后采集一条 span（fire-and-forget：sink 未初始化/满队列 → 丢弃）
pub fn record_span(rec: SpanRecord) {
    if let Some(tx) = SINK.get() {
        let _ = tx.try_send(TraceWrite::Span(rec));
    }
}

/// 文件变更证据采集（与 span 同管道；emit `bot-file-changed` 由 dispatch 侧发）
pub fn record_file_change(rec: FileChangeRecord) {
    if let Some(tx) = SINK.get() {
        let _ = tx.try_send(TraceWrite::FileChange(rec));
    }
}

/// span 文本钳制（SPAN_TEXT_MAX，UTF-8 边界安全）；超限发 `trace.span_overflow` 审计。
pub(crate) fn clamp_span_text<R: tauri::Runtime>(
    app: &AppHandle<R>,
    tool: &str,
    field: &str,
    s: String,
) -> Option<String> {
    if s.is_empty() {
        return None;
    }
    if s.len() <= crate::db::SPAN_TEXT_MAX {
        return Some(s);
    }
    crate::audit::write_event(
        app,
        crate::audit::AuditLevel::Warn,
        "trace.span_overflow",
        &[
            ("tool", tool.to_string()),
            ("field", field.to_string()),
            ("len", s.len().to_string()),
        ],
    );
    Some(crate::db::clamp_text(&s, crate::db::SPAN_TEXT_MAX))
}

// trace 生命周期

/// run_task_in_chat（壳）与 run_task_in_chat_with（内核）之间传递 trace 归属与
/// LoopTrace 统计的挂钩：壳造槽 → 内核填归属 + 收尾 → 壳的闭包填统计。
#[derive(Default)]
pub struct TraceCapture {
    pub trace_id: Option<i64>,
    pub session_id: Option<String>,
    pub stats: Option<crate::bot_model_loop::LoopTrace>,
}

/// 执行首：建 exec_traces 行 + 注册 session→trace 映射（span 采集开关）。
/// 观测面失败降级 None（不阻断执行），只 eprintln 留痕。
pub(crate) async fn begin_trace<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: &str,
    task_id: Option<String>,
    origin: &str,
    title: Option<String>,
) -> Option<i64> {
    let app2 = app.clone();
    let sid = session_id.to_string();
    let origin = origin.to_string();
    // 审计 kv 用副本（本体 move 进 spawn_blocking 闭包）
    let origin_label = origin.clone();
    let task_id_label = task_id.clone().unwrap_or_else(|| "-".into());
    let started_at = chrono::Utc::now().timestamp_millis();
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<i64, String> {
        let conn = crate::db::open_db(&app2)?;
        crate::db::trace_start(
            &conn,
            &crate::db::NewTrace {
                session_id: &sid,
                task_id: task_id.as_deref(),
                origin: &origin,
                title: title.as_deref(),
                started_at,
            },
        )
    })
    .await;
    match r {
        Ok(Ok(id)) => {
            if let Ok(mut m) = crate::app_state::trace_registry(app).lock() {
                m.insert(session_id.to_string(), id);
            }
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Info,
                "trace.start",
                &[
                    ("trace_id", id.to_string()),
                    ("session_id", session_id.to_string()),
                    ("origin", origin_label),
                    ("task_id", task_id_label),
                ],
            );
            Some(id)
        }
        Ok(Err(e)) => {
            eprintln!("[trace_sink] trace 建立失败（观测面降级）：{e}");
            None
        }
        Err(e) => {
            eprintln!("[trace_sink] trace 建立线程失败（观测面降级）：{e}");
            None
        }
    }
}

/// 执行尾：注销 session 映射 + trace_finish（汇总 LoopTrace 统计）+ trace.complete 审计。
/// 无论成败都要调（与 begin_trace 成对）；trace 未建立（begin 降级）时静默跳过。
/// `stopped`：用户停止（run_model_loop 的停止返回是 Ok，须单独传标志区分 done）
pub(crate) async fn end_trace<R: tauri::Runtime>(
    app: &AppHandle<R>,
    hook: &std::sync::Arc<std::sync::Mutex<TraceCapture>>,
    ok: bool,
    stopped: bool,
    err: Option<String>,
) {
    let (trace_id, session_id, stats) = {
        let Ok(mut c) = hook.lock() else { return };
        (c.trace_id.take(), c.session_id.take(), c.stats.take())
    };
    let Some(trace_id) = trace_id else { return };
    if let Some(sid) = &session_id {
        if let Ok(mut m) = crate::app_state::trace_registry(app).lock() {
            m.remove(sid);
        }
    }
    // status 三态：stopped 优先（停止不是失败，也不是正常完成）
    let status = if stopped {
        crate::db::TRACE_STATUS_STOPPED
    } else if ok {
        crate::db::TRACE_STATUS_DONE
    } else {
        crate::db::TRACE_STATUS_FAILED
    };
    let (turn_count, tool_calls, prompt_tokens, completion_tokens) = match &stats {
        Some(t) => (
            t.turn_count as i64,
            t.tool_calls.len() as i64,
            t.prompt_tokens as i64,
            t.completion_tokens as i64,
        ),
        None => (0, 0, 0, 0),
    };
    let finished_at = chrono::Utc::now().timestamp_millis();
    let app2 = app.clone();
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<crate::db::TraceRow, String> {
        let conn = crate::db::open_db(&app2)?;
        crate::db::trace_finish(
            &conn,
            trace_id,
            &crate::db::TraceFinish {
                status,
                finished_at,
                turn_count,
                tool_calls,
                prompt_tokens,
                completion_tokens,
                error: err.as_deref(),
                model: stats.as_ref().and_then(|t| t.model.as_deref()),
            },
        )?;
        // 行理论上必在；并发清理/异常缺行时降级为 Err（走「收尾失败」降级日志），不 panic
        crate::db::trace_get_row(&conn, trace_id)?
            .ok_or_else(|| format!("trace_finish 后 trace {trace_id} 行缺失"))
    })
    .await;
    match r {
        Ok(Ok(row)) => {
            let ms = row.finished_at.unwrap_or(finished_at) - row.started_at;
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Info,
                "trace.complete",
                &[
                    ("trace_id", trace_id.to_string()),
                    ("status", row.status),
                    ("turns", row.turn_count.to_string()),
                    ("tools", row.tool_calls.to_string()),
                    ("files", row.files_changed.to_string()),
                    ("prompt_tok", row.prompt_tokens.to_string()),
                    ("completion_tok", row.completion_tokens.to_string()),
                    ("ms", ms.to_string()),
                ],
            );
        }
        Ok(Err(e)) => eprintln!("[trace_sink] trace 收尾失败（观测面降级）：{e}"),
        Err(e) => eprintln!("[trace_sink] trace 收尾线程失败（观测面降级）：{e}"),
    }
}

// 后台 writer（攒批落库）

async fn writer<R: tauri::Runtime>(app: AppHandle<R>, mut rx: mpsc::Receiver<TraceWrite>) {
    let mut conn: Option<rusqlite::Connection> = None;
    while let Some(first) = rx.recv().await {
        let mut batch = vec![first];
        while batch.len() < WRITE_BATCH_MAX {
            match rx.try_recv() {
                Ok(m) => batch.push(m),
                Err(_) => break,
            }
        }
        if conn.is_none() {
            match crate::db::open_db(&app) {
                Ok(c) => conn = Some(c),
                Err(e) => {
                    eprintln!(
                        "[trace_sink] 打开库失败，丢弃 {} 条观测记录：{e}",
                        batch.len()
                    );
                    continue;
                }
            }
        }
        let c = conn.take().expect("上方保证已建连");
        let batch_len = batch.len();
        let write = |c: &rusqlite::Connection| -> Result<(), String> {
            let tx = c.unchecked_transaction().map_err(|e| e.to_string())?;
            for m in &batch {
                match m {
                    TraceWrite::Span(s) => crate::db::span_insert(
                        &tx,
                        &crate::db::NewSpan {
                            trace_id: s.trace_id,
                            turn: s.turn,
                            tool_call_id: s.tool_call_id.as_deref(),
                            name: &s.name,
                            args: s.args.as_deref(),
                            result: s.result.as_deref(),
                            ok: s.ok,
                            error_class: s.error_class.as_deref(),
                            duration_ms: s.duration_ms,
                            created_at: s.created_at,
                        },
                    )?,
                    TraceWrite::FileChange(f) => crate::db::file_change_insert(
                        &tx,
                        &crate::db::NewFileChange {
                            trace_id: f.trace_id,
                            span_id: None,
                            path: &f.path,
                            kind: f.kind,
                            added: f.added,
                            deleted: f.deleted,
                            diff: f.diff.as_deref(),
                            truncated: f.truncated,
                            before_ref: f.before_ref.as_deref(),
                            before_sha: f.before_sha.as_deref(),
                            after_sha: f.after_sha.as_deref(),
                            created_at: f.created_at,
                        },
                    )?,
                };
            }
            tx.commit().map_err(|e| e.to_string())
        };
        // DB_WRITE_LOCK 纪律：与主写者 / API 写者同锁串行（临界区 = 一批小 insert）
        let result = {
            let _guard = crate::db::lock_db_write();
            write(&c)
        };
        match result {
            Ok(()) => conn = Some(c),
            Err(e) => {
                // 半批丢弃 + 连接重开（下轮 open_db 幂等重建）；观测面 best-effort。
                // 丢弃必须落审计留痕（同 trace.span_overflow 口径），否则观测面可能
                // 静默降级为零覆盖而无任何外部信号
                crate::audit::write_event(
                    &app,
                    crate::audit::AuditLevel::Warn,
                    "trace.batch_dropped",
                    &[("batch_len", batch_len.to_string()), ("err", e.clone())],
                );
                eprintln!("[trace_sink] 批量落库失败（{batch_len} 条丢弃，重连）：{e}");
            }
        }
    }
}
