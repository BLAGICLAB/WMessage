//! 公用小工具：时间戳、字段长度上限、状态校验、日志行拼接、
//! 错误响应分流、变更后回调。
//!
//! CreateReq / UpdateReq 也放此处——是任务操作请求体形状，
//! 与 over_limit / valid_status 校验工具同生命周期。

use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use tiny_http::{Request, StatusCode};

use crate::api::TaskStore;
use crate::db;

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// 字段长度上限：单源引用 bot.rs 工具侧 MAX_*（值对齐由定义处保证）
pub(crate) const API_MAX_TITLE: usize = crate::bot::MAX_TITLE;
pub(crate) const API_MAX_NOTE: usize = crate::bot::MAX_NOTE;
pub(crate) const API_MAX_DUE: usize = crate::bot::MAX_DUE;
pub(crate) const API_MAX_TAG_LEN: usize = crate::bot::MAX_TAG_LEN;
pub(crate) const API_MAX_TAGS: usize = crate::bot::MAX_TAGS;
// 取 1024（macOS PATH_MAX 量级），与 title/note 等字段一样走 over_limit
pub(crate) const API_MAX_FILE_PATH: usize = 1024;

pub(crate) fn valid_status(s: &str) -> bool {
    s.parse::<crate::db::TaskStatus>().is_ok()
}

pub(crate) fn over_limit(v: &str, max: usize, what: &str) -> Option<String> {
    (v.chars().count() > max).then(|| format!("{what}过长（上限 {max} 字）"))
}

/// 拼一行任务变更日志（title 过 escape_for_log——标题里的 `\n` / `| ` 会伪造
/// 日志行或撕裂多行；抽出纯函数便于单测，after_change 只做 IO）。
pub(crate) fn change_log_line(op: &str, task: &db::Task) -> String {
    format!(
        "change op={} id={} status={} title={}",
        op,
        task.id,
        task.column,
        crate::audit::escape_for_log(&task.title, 2 * API_MAX_TITLE)
    )
}

/// 500 对外统一文案：DB 错误原文可能含 SQL 片段/路径，
/// 不回吐给客户端；原文转义后进 api.log 供排查。
pub(crate) fn internal_err(req: Request, log: &Option<PathBuf>, e: &str) {
    super::ratelimit::log_line(
        log,
        &format!("internal_error | {}", crate::audit::escape_for_log(e, 300)),
    );
    let _ = req.respond(super::handlers::json_err(StatusCode(500), "internal error"));
}

/// upsert 失败分流——RMW 基线冲突（其他写者已改/删该行）→ 409（客户端应重读后重试）；
/// 其余错误走 500 统一文案。
pub(crate) fn upsert_err(req: Request, log: &Option<PathBuf>, e: &str) {
    if e.starts_with(db::CONFLICT_ERR_PREFIX) {
        super::ratelimit::log_line(
            log,
            &format!("conflict | {}", crate::audit::escape_for_log(e, 300)),
        );
        let _ = req.respond(super::handlers::json_err(
            StatusCode(409),
            "任务已被其他端修改或删除，请重试",
        ));
    } else {
        internal_err(req, log, e);
    }
}

/// 任务变更后：store 内部 SSE 广播 + 前端看板刷新回调 + 变更日志
///
/// hub 不传入 handler；SSE 广播走 `store.notify_change()`，由 store 层封装 hub。
/// 这样 handler 与 EventHub 解耦，未来加 EventBus / 持久化监听都在 store 层加。
pub(crate) fn after_change(
    store: &Arc<dyn TaskStore>,
    task: &db::Task,
    op: &str,
    emit_fn: &Option<Arc<dyn Fn(&db::Task) + Send + Sync>>,
    log: &Option<PathBuf>,
) {
    store.notify_change(op, task);
    super::ratelimit::log_line(log, &change_log_line(op, task));
    if let Some(f) = emit_fn {
        f(task);
    }
}

// ───────────────────────── 任务 JSON 形状 ─────────────────────────

/// 对外任务对象：`db::Task` 字段 + `status`（todo/doing/done，即看板列）
// TaskOut 抽到 crate::task_out 模块（数据层 api.rs 也需用，不能反向依赖 api_handlers）
pub(crate) use crate::task_out::TaskOut;

// ───────────────────────── 请求体形状 ─────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateReq {
    pub title: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub file_is_dir: Option<bool>,
    #[serde(default)]
    pub due: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateReq {
    pub title: Option<String>,
    pub note: Option<String>,
    pub status: Option<String>,
    pub file_path: Option<String>,
    pub file_is_dir: Option<bool>,
    pub due: Option<String>,
    pub tags: Option<Vec<String>>,
    pub archived: Option<bool>,
    pub deleted: Option<bool>,
}

// 让本模块 utils 也用 Response<Cursor<Vec<u8>>> 类型别名
// pub(crate) use crate::api_handlers::body::JsonResponse; // 暂时未用，留作以后扩展
