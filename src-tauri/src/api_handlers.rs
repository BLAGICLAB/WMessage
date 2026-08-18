//! Endpoint 处理函数 + tauri 命令（`api_handlers`）
//!
//! 完整模块组：
//! - `api_server`   : 服务生命周期 / 端口绑定 / EventHub
//! - `api_auth`     : token 管理 / 鉴权 / 开关持久化
//! - `api_handlers` : 此文件 — endpoint handlers + tauri commands
//! - `api`          : 数据层 `TaskStore` + `MemStore` + `TauriStore`
//!
//! 端点：
//! - GET    /api/health       健康检查（唯一免鉴权，仅返回服务状态）
//! - GET    /api/tasks        任务列表（默认活跃：不含回收站/归档；`?status=todo|doing|done` 按列过滤；`?trash=1` 回收站；`?archived=1` 归档；`?all=1` 全量）
//! - GET    /api/tasks/:id    单条任务
//! - POST   /api/tasks        新建任务 `{ title, note?, status?, filePath?, fileIsDir?, due?, tags? }`
//! - PUT    /api/tasks/:id    更新任务 `{ title?, note?, status?, filePath?, fileIsDir?, due?, tags?, archived?, deleted? }`
//! - DELETE /api/tasks/:id    软删（进回收站，幂等）
//! - GET    /api/events       SSE 实时推送（`?since=<事件id>` 断线重放，事件带 id）
//!
//! 任务变更后：向 SSE 客户端广播 + 向主窗口发 tasks-updated 事件（看板自动刷新）。
//! 操作日志：数据目录 `api.log`（访问 + 变更）。token 轮换：`api_rotate_token`。

use std::io::{Cursor, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{sync_channel, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tiny_http::{Header, Method, Request, Response, StatusCode};

use crate::api::{TaskStore, TauriStore, API_PORT};
use crate::api_auth::{
    clear_enabled_flag, load_or_create_token, verify_bearer, write_enabled_flag,
};
use crate::api_server::{ApiState, EventHub, start_api};
use crate::audit::AuditLevel;
use crate::audit_event;
use crate::db;
use crate::error::{CommandError, CommandResult};

/// 请求体上限（防内存打爆）
const MAX_BODY_BYTES: u64 = 1_000_000;
/// 每分钟请求上限（仅回环，防失控脚本）
const RATE_LIMIT_PER_MIN: u32 = 120;
/// SSE 并发连接上限（A6：每连接一个 writer 线程，不设上限可被连接洪泛耗尽线程）
const MAX_SSE_CLIENTS: usize = 32;

// ───────────────────────── 公共返回类型 ─────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ApiInfo {
    pub port: u16,
    pub token: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ApiStatus {
    pub enabled: bool,
    pub port: u16,
    pub token: String,
}

// ───────────────────────── 路由 ─────────────────────────

pub fn handle_request(
    req: Request,
    token: &str,
    store: &Arc<dyn TaskStore>,
    emit_fn: &Option<Arc<dyn Fn(&db::Task) + Send + Sync>>,
    log: &Option<PathBuf>,
) {
    let method = req.method().clone();
    let url = req.url().to_string();
    let (path, query) = split_query(&url);

    // 健康检查：唯一免鉴权 + 免限流端点（仅返回服务状态，无数据泄露）
    if method == Method::Get && path == "/api/health" {
        health(req);
        return;
    }

    // Bearer 鉴权先于限流：避免 401 风暴消耗合法用户的预算（本地 DoS）
    if !verify_bearer(&req, token) {
        let _ = req.respond(json_err(StatusCode(401), "unauthorized"));
        return;
    }

    // 限流（在鉴权后，仅作用于合法请求）
    if !rate_check() {
        let _ = req.respond(json_err(StatusCode(429), "too many requests"));
        return;
    }

    log_line(log, &format!("{} {}", method, path));

    match (&method, path.as_str()) {
        (Method::Get, "/api/tasks") => list_tasks(req, store, &query),
        (Method::Get, "/api/events") => sse_connect(req, store, &query),
        (Method::Post, "/api/tasks") => create_task(req, store, emit_fn, log),
        _ => {
            if let Some(id) = path.strip_prefix("/api/tasks/") {
                match method {
                    Method::Get => get_task(req, store, id),
                    Method::Put => update_task(req, store, id, emit_fn, log),
                    Method::Delete => delete_task(req, store, id, emit_fn, log),
                    _ => {
                        let _ = req.respond(json_err(StatusCode(405), "method not allowed"));
                    }
                }
            } else {
                let _ = req.respond(json_err(StatusCode(404), "not found"));
            }
        }
    }
}

/// 拆分 path 与 query（不做 URL decode 之外的处理；值仅限简单 token）
fn split_query(url: &str) -> (String, String) {
    match url.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (url.to_string(), String::new()),
    }
}

/// 取 query 参数（`?a=1&b=2` 形式，重复键取第一个；简单 percent-decode）
fn query_param<'a>(query: &'a str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (percent_decode(k) == key).then(|| percent_decode(v))
    })
}

/// 最小 percent-decode（`%XX`；urlencoded 约定里 `+` 解码为空格，审计 P3）
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ───────────────────────── 响应辅助 ─────────────────────────

fn json_ok<T: Serialize>(status: StatusCode, v: &T) -> Response<Cursor<Vec<u8>>> {
    let body = serde_json::to_vec(v).unwrap_or_else(|_| b"{}".to_vec());
    Response::from_data(body)
        .with_status_code(status)
        .with_header(
            Header::from_bytes(
                &b"Content-Type"[..],
                &b"application/json; charset=utf-8"[..],
            )
            .expect("静态 header 字节不可能失败"),
        )
}

fn json_err(status: StatusCode, msg: &str) -> Response<Cursor<Vec<u8>>> {
    json_ok(status, &serde_json::json!({ "error": msg }))
}

/// 健康检查：免鉴权，仅返回服务状态
fn health(req: Request) {
    let _ = req.respond(json_ok(
        StatusCode(200),
        &serde_json::json!({ "ok": true, "service": "wmessage-api" }),
    ));
}

// ───────────────────────── 速率限制 / 日志 ─────────────────────────

static RATE: Mutex<(u64, u32)> = Mutex::new((0, 0));

/// 每分钟最多 `RATE_LIMIT_PER_MIN` 次；窗口基于毫秒时间戳
fn rate_check() -> bool {
    let now = now_ms() as u64;
    let mut g = RATE.lock().unwrap_or_else(|e| e.into_inner());
    if now.saturating_sub(g.0) > 60_000 {
        *g = (now, 0);
    }
    if g.1 >= RATE_LIMIT_PER_MIN {
        return false;
    }
    g.1 += 1;
    true
}

/// 追加一行访问/变更日志（`None` 或写入失败则静默忽略）
fn log_line(path: &Option<PathBuf>, line: &str) {
    let Some(p) = path else { return };
    crate::db::rotate_log_if_large(p, 5 * 1024 * 1024);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
    {
        let _ = writeln!(
            f,
            "[{}] {line}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );
    }
}

/// 读请求体，超过 `MAX_BODY_BYTES` 返回 `None`（调用方回 413）
fn read_body_limited(req: &mut Request) -> Option<String> {
    let mut buf = Vec::new();
    match req
        .as_reader()
        .take(MAX_BODY_BYTES + 1)
        .read_to_end(&mut buf)
    {
        Ok(n) if n as u64 > MAX_BODY_BYTES => None,
        Ok(_) => Some(String::from_utf8_lossy(&buf).into_owned()),
        Err(_) => None,
    }
}

// ───────────────────────── 任务 JSON 形状 ─────────────────────────

/// 对外任务对象：`db::Task` 字段 + `status`（todo/doing/done，即看板列）
// A5: TaskOut 抽到 crate::task_out 模块（数据层 api.rs 也需用，不能反向依赖 api_handlers）
use crate::task_out::TaskOut;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// 字段长度上限：与 bot.rs 工具侧 MAX_* 对齐（审计 P3：原 API 侧无校验，只靠 1MB body 兜底）
const API_MAX_TITLE: usize = 200;
const API_MAX_NOTE: usize = 5000;
const API_MAX_DUE: usize = 30;
const API_MAX_TAG_LEN: usize = 30;
const API_MAX_TAGS: usize = 10;

fn valid_status(s: &str) -> bool {
    matches!(s, "todo" | "doing" | "done")
}

fn over_limit(v: &str, max: usize, what: &str) -> Option<String> {
    (v.chars().count() > max).then(|| format!("{what}过长（上限 {max} 字）"))
}

/// 任务变更后：store 内部 SSE 广播 + 前端看板刷新回调 + 变更日志
///
/// A5: hub 不再传入；SSE 广播走 `store.notify_change()`，由 store 层封装 hub。
/// 这样 handler 与 EventHub 解耦，未来加 EventBus / 持久化监听都在 store 层加。
fn after_change(
    store: &Arc<dyn TaskStore>,
    task: &db::Task,
    op: &str,
    emit_fn: &Option<Arc<dyn Fn(&db::Task) + Send + Sync>>,
    log: &Option<PathBuf>,
) {
    store.notify_change(op, task);
    log_line(
        log,
        &format!(
            "change op={} id={} status={} title={}",
            op, task.id, task.column, task.title
        ),
    );
    if let Some(f) = emit_fn {
        f(task);
    }
}

// ───────────────────────── 处理器 ─────────────────────────

fn list_tasks(req: Request, store: &Arc<dyn TaskStore>, query: &str) {
    // 过滤参数：默认活跃任务（非回收站、非归档）；trash/archived/all 改变范围，status 按列筛
    let all = query_param(query, "all") == Some("1".into());
    let trash_only = query_param(query, "trash") == Some("1".into());
    let archived_only = query_param(query, "archived") == Some("1".into());
    let status = query_param(query, "status").filter(|s| !s.is_empty());
    if let Some(s) = status.as_deref() {
        if !valid_status(s) {
            let _ = req.respond(json_err(StatusCode(400), "status 必须是 todo/doing/done"));
            return;
        }
    }
    match store.load() {
        Ok(tasks) => {
            let mut out: Vec<TaskOut> = tasks.iter().map(TaskOut::from_task).collect();
            if !all {
                out.retain(|t| {
                    if trash_only {
                        t.inner.deleted_at.is_some()
                    } else if archived_only {
                        t.inner.deleted_at.is_none() && t.inner.archived == Some(true)
                    } else {
                        t.inner.deleted_at.is_none() && t.inner.archived != Some(true)
                    }
                });
            }
            if let Some(s) = status {
                out.retain(|t| t.status == s);
            }
            let _ = req.respond(json_ok(StatusCode(200), &out));
        }
        Err(e) => {
            let _ = req.respond(json_err(StatusCode(500), &e));
        }
    }
}

fn get_task(req: Request, store: &Arc<dyn TaskStore>, id: &str) {
    match store.load() {
        Ok(tasks) => match tasks.iter().find(|t| t.id == id) {
            Some(t) => {
                let _ = req.respond(json_ok(StatusCode(200), &TaskOut::from_task(t)));
            }
            None => {
                let _ = req.respond(json_err(StatusCode(404), "task not found"));
            }
        },
        Err(e) => {
            let _ = req.respond(json_err(StatusCode(500), &e));
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateReq {
    title: String,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    file_path: Option<String>,
    #[serde(default)]
    file_is_dir: Option<bool>,
    #[serde(default)]
    due: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
}

fn create_task(
    mut req: Request,
    store: &Arc<dyn TaskStore>,
    emit_fn: &Option<Arc<dyn Fn(&db::Task) + Send + Sync>>,
    log: &Option<PathBuf>,
) {
    let Some(body) = read_body_limited(&mut req) else {
        let _ = req.respond(json_err(StatusCode(413), "body too large"));
        return;
    };
    let input: CreateReq = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => {
            let _ = req.respond(json_err(StatusCode(400), "invalid json"));
            return;
        }
    };
    let title = input.title.trim().to_string();
    if title.is_empty() {
        let _ = req.respond(json_err(StatusCode(400), "title 不能为空"));
        return;
    }
    // 长度校验与 update_task / bot 工具侧对齐（二次审计 P2-4）
    if let Some(e) = over_limit(&title, API_MAX_TITLE, "任务标题") {
        let _ = req.respond(json_err(StatusCode(400), &e));
        return;
    }
    if let Some(n) = input
        .note
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Some(e) = over_limit(n, API_MAX_NOTE, "备注") {
            let _ = req.respond(json_err(StatusCode(400), &e));
            return;
        }
    }
    if let Some(d) = input
        .due
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Some(e) = over_limit(d, API_MAX_DUE, "截止时间") {
            let _ = req.respond(json_err(StatusCode(400), &e));
            return;
        }
    }
    if let Some(tags) = input.tags.as_deref() {
        if tags.len() > API_MAX_TAGS {
            let _ = req.respond(json_err(
                StatusCode(400),
                &format!("标签最多 {API_MAX_TAGS} 个"),
            ));
            return;
        }
        for tag in tags {
            if let Some(e) = over_limit(tag, API_MAX_TAG_LEN, "标签") {
                let _ = req.respond(json_err(StatusCode(400), &e));
                return;
            }
        }
    }
    let status = match input.status.as_deref() {
        None | Some("") => "todo".to_string(),
        Some(s) if valid_status(s) => s.to_string(),
        Some(_) => {
            let _ = req.respond(json_err(StatusCode(400), "status 必须是 todo/doing/done"));
            return;
        }
    };

    let all = match store.load() {
        Ok(v) => v,
        Err(e) => {
            let _ = req.respond(json_err(StatusCode(500), &e));
            return;
        }
    };
    let max_order = all.iter().filter_map(|t| t.order).fold(0.0f64, f64::max);
    let now = now_ms();
    let task = db::Task {
        id: uuid::Uuid::new_v4().to_string(),
        title,
        // due 与 note 同规则：trim 后存储（此前存原始值，首尾空格会进库）
        due: input
            .due
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty()),
        note: input.note.filter(|n| !n.trim().is_empty()),
        tags: input.tags,
        file_path: input.file_path.filter(|p| !p.trim().is_empty()),
        file_is_dir: input.file_is_dir,
        column: status.clone(),
        subtasks: None,
        completed_at: if status == "done" { Some(now) } else { None },
        archived: if status == "done" { Some(false) } else { None },
        deleted_at: None,
        collapsed: None,
        order: Some(max_order + 1.0),
        updated_at: Some(now),
        schedule: None,
        sched_last: None,
        bot_assigned: None,
    };
    if let Err(e) = store.upsert(vec![task.clone()]) {
        let _ = req.respond(json_err(StatusCode(500), &e));
        return;
    }
    after_change(store, &task, "created", emit_fn, log);
    let _ = req.respond(json_ok(StatusCode(201), &TaskOut::from_task(&task)));
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct UpdateReq {
    title: Option<String>,
    note: Option<String>,
    status: Option<String>,
    file_path: Option<String>,
    file_is_dir: Option<bool>,
    due: Option<String>,
    tags: Option<Vec<String>>,
    archived: Option<bool>,
    deleted: Option<bool>,
}

fn update_task(
    mut req: Request,
    store: &Arc<dyn TaskStore>,
    id: &str,
    emit_fn: &Option<Arc<dyn Fn(&db::Task) + Send + Sync>>,
    log: &Option<PathBuf>,
) {
    let Some(body) = read_body_limited(&mut req) else {
        let _ = req.respond(json_err(StatusCode(413), "body too large"));
        return;
    };
    let input: UpdateReq = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => {
            let _ = req.respond(json_err(StatusCode(400), "invalid json"));
            return;
        }
    };

    let tasks = match store.load() {
        Ok(v) => v,
        Err(e) => {
            let _ = req.respond(json_err(StatusCode(500), &e));
            return;
        }
    };
    let Some(idx) = tasks.iter().position(|t| t.id == id) else {
        let _ = req.respond(json_err(StatusCode(404), "task not found"));
        return;
    };
    let mut t = tasks[idx].clone();

    if let Some(title) = input.title.as_deref() {
        let tt = title.trim();
        if !tt.is_empty() {
            if let Some(e) = over_limit(tt, API_MAX_TITLE, "任务标题") {
                let _ = req.respond(json_err(StatusCode(400), &e));
                return;
            }
            t.title = tt.to_string();
        }
    }
    if let Some(n) = input.note.as_deref() {
        let nn = n.trim();
        if !nn.is_empty() {
            if let Some(e) = over_limit(nn, API_MAX_NOTE, "备注") {
                let _ = req.respond(json_err(StatusCode(400), &e));
                return;
            }
        }
        t.note = if nn.is_empty() {
            None
        } else {
            Some(nn.to_string())
        };
    }
    if let Some(s) = input.status.as_deref() {
        if !s.is_empty() {
            if !valid_status(s) {
                let _ = req.respond(json_err(StatusCode(400), "status 必须是 todo/doing/done"));
                return;
            }
            if s != t.column {
                let now = now_ms();
                if s == "done" {
                    t.completed_at = Some(now);
                    t.archived = Some(false);
                } else if t.column == "done" {
                    t.completed_at = None;
                    t.archived = None;
                }
                t.column = s.to_string();
            }
        }
    }
    if let Some(fp) = input.file_path.as_deref() {
        if fp.trim().is_empty() {
            t.file_path = None;
            t.file_is_dir = None;
        } else {
            t.file_path = Some(fp.to_string());
            if let Some(fid) = input.file_is_dir {
                t.file_is_dir = Some(fid);
            }
        }
    } else if let Some(fid) = input.file_is_dir {
        if t.file_path.is_some() {
            t.file_is_dir = Some(fid);
        }
    }
    if let Some(due) = input.due.as_deref() {
        let dd = due.trim();
        if !dd.is_empty() {
            if let Some(e) = over_limit(dd, API_MAX_DUE, "截止时间") {
                let _ = req.respond(json_err(StatusCode(400), &e));
                return;
            }
        }
        t.due = if dd.is_empty() {
            None
        } else {
            Some(dd.to_string())
        };
    }
    if let Some(tags) = input.tags {
        if tags.len() > API_MAX_TAGS {
            let _ = req.respond(json_err(
                StatusCode(400),
                &format!("标签最多 {API_MAX_TAGS} 个"),
            ));
            return;
        }
        for tag in &tags {
            if let Some(e) = over_limit(tag, API_MAX_TAG_LEN, "标签") {
                let _ = req.respond(json_err(StatusCode(400), &e));
                return;
            }
        }
        t.tags = if tags.is_empty() { None } else { Some(tags) };
    }
    if let Some(archived) = input.archived {
        t.archived = Some(archived);
    }
    if let Some(deleted) = input.deleted {
        if deleted {
            t.deleted_at = Some(now_ms());
        } else {
            t.deleted_at = None;
        }
    }
    t.updated_at = Some(now_ms());

    if let Err(e) = store.upsert(vec![t.clone()]) {
        let _ = req.respond(json_err(StatusCode(500), &e));
        return;
    }
    after_change(store, &t, "updated", emit_fn, log);
    let _ = req.respond(json_ok(StatusCode(200), &TaskOut::from_task(&t)));
}

/// 软删（进回收站）：幂等，已删的再次 DELETE 返回 200 不再变更
fn delete_task(
    req: Request,
    store: &Arc<dyn TaskStore>,
    id: &str,
    emit_fn: &Option<Arc<dyn Fn(&db::Task) + Send + Sync>>,
    log: &Option<PathBuf>,
) {
    let tasks = match store.load() {
        Ok(v) => v,
        Err(e) => {
            let _ = req.respond(json_err(StatusCode(500), &e));
            return;
        }
    };
    let Some(idx) = tasks.iter().position(|t| t.id == id) else {
        let _ = req.respond(json_err(StatusCode(404), "task not found"));
        return;
    };
    let mut t = tasks[idx].clone();
    if t.deleted_at.is_some() {
        // 已在回收站：幂等返回当前状态
        let _ = req.respond(json_ok(StatusCode(200), &TaskOut::from_task(&t)));
        return;
    }
    t.deleted_at = Some(now_ms());
    t.updated_at = Some(now_ms());
    if let Err(e) = store.upsert(vec![t.clone()]) {
        let _ = req.respond(json_err(StatusCode(500), &e));
        return;
    }
    after_change(store, &t, "deleted", emit_fn, log);
    let _ = req.respond(json_ok(StatusCode(200), &TaskOut::from_task(&t)));
}

/// 注册 SSE 客户端：支持 `?since=<事件id>` 断线重放，然后用 tiny_http upgrade 直写。
fn sse_connect(req: Request, store: &Arc<dyn TaskStore>, query: &str) {
    let since = query_param(query, "since").and_then(|s| s.parse::<u64>().ok());
    // A2: sync_channel(256) — 单客户端最多积压 256 条，超出则丢事件（广播不阻塞）
    let (tx, rx) = sync_channel(256);
    // 锁中毒时用 into_inner 恢复（与 broadcast 端策略一致，审计 P3：原先静默跳过，
    // 客户端注册失败则该 SSE 连接永远收不到事件）
    let hub = store.event_hub();
    {
        let mut clients = hub.clients.lock().unwrap_or_else(|e| e.into_inner());
        // A6: SSE 连接数上限 —— 超限 503，防连接洪泛耗尽线程
        if clients.len() >= MAX_SSE_CLIENTS {
            drop(clients);
            let _ = req.respond(json_err(StatusCode(503), "too many SSE connections"));
            return;
        }
        clients.push(tx);
    }
    let hub = hub.clone();
    std::thread::spawn(move || {
        let headers = vec![
            Header::from_bytes(
                &b"Content-Type"[..],
                &b"text/event-stream; charset=utf-8"[..],
            )
            .expect("静态 header 字节不可能失败"),
            Header::from_bytes(&b"Cache-Control"[..], &b"no-cache"[..])
                .expect("静态 header 字节不可能失败"),
        ];
        let resp = Response::new(StatusCode(200), headers, std::io::empty(), Some(0), None);
        let mut stream = req.upgrade("text/event-stream", resp);
        // A2: 写超时通过 recv_timeout(15s) 心跳 + 客户端断开检测协同处理
        // tiny_http ResponseBox 不提供 set_write_timeout，故通过 recv 端超时兜底
        
        // 连接成功事件（携带当前事件 id，供客户端决定下次 since 起点）
        let connected = format!(
            "data: {{\"type\":\"connected\",\"lastEventId\":{}}}\n\n",
            hub.last_id()
        );
        if stream
            .write_all(connected.as_bytes())
            .and_then(|_| stream.flush())
            .is_err()
        {
            return;
        }
        // 断线重放：补发 since 之后的历史事件
        if let Some(since) = since {
            for msg in hub.replay(since) {
                if stream.write_all(msg.as_bytes()).is_err() {
                    return;
                }
            }
            let _ = stream.flush();
        }
        loop {
            match rx.recv_timeout(Duration::from_secs(15)) {
                Ok(data) => {
                    if stream.write_all(&data).is_err() || stream.flush().is_err() {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    // SSE 心跳注释，保持连接存活
                    if stream.write_all(b": keepalive\n\n").is_err() {
                        break;
                    }
                    let _ = stream.flush();
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    });
}

// ───────────────────────── tauri 命令 ─────────────────────────

#[tauri::command]
pub fn api_start(app: AppHandle, state: tauri::State<'_, ApiState>) -> CommandResult<ApiInfo> {
    {
        let g = state.0.lock().map_err(|e| e.to_string())?;
        if g.is_some() {
            return Ok(ApiInfo {
                port: API_PORT,
                token: load_or_create_token(&app)?,
            });
        }
    }
    let token = load_or_create_token(&app)?;
    let store: Arc<dyn TaskStore> = Arc::new(TauriStore {
        app: app.clone(),
        // A6: id 持久化，跨重启保持单调（否则客户端 Last-Event-ID 去重会静默丢事件）
        hub: EventHub::persisted(db::data_dir(&app).join("api-event-id.txt")),
    });
    let emit_app = app.clone();
    let emit: Option<Box<dyn Fn(&db::Task) + Send + Sync>> =
        Some(Box::new(move |task: &db::Task| {
            // 复用挂件→主窗口的既有通道：主窗口合并状态并广播给挂件。
            // `source:"api"` 告诉主窗口：数据已由 API 线程落盘，只合并 UI 状态，不要回写
            // （回写会用旧事件快照覆盖 API 的新写入，导致归档/软删被回滚的竞态）
            let payload = serde_json::json!({ "upserts": [task], "deletes": [], "source": "api" });
            let _ = emit_app.emit_to("main", "tasks-updated", &payload);
        }));
    let log_path = Some(db::data_dir(&app).join("api.log"));
    let audit_app = app.clone();
    let on_error: Option<Box<dyn Fn(AuditLevel, &str, &str) + Send + Sync>> =
        Some(Box::new(move |lvl, ev, msg| {
            audit_event!(&audit_app, lvl, ev, "error" => msg);
        }));
    let running = start_api(API_PORT, token.clone(), store, emit, log_path, on_error)?;
    *state.0.lock().map_err(|e| e.to_string())? = Some(running);
    write_enabled_flag(&app);
    Ok(ApiInfo {
        port: API_PORT,
        token,
    })
}

#[tauri::command]
pub fn api_stop(app: AppHandle, state: tauri::State<'_, ApiState>) -> CommandResult<()> {
    let mut g = state.0.lock().map_err(|e| e.to_string())?;
    if let Some(mut r) = g.take() {
        r.shutdown.store(true, Ordering::SeqCst);
        if let Some(h) = r.handle.take() {
            let _ = h.join();
        }
    }
    clear_enabled_flag(&app);
    Ok(())
}

#[tauri::command]
pub fn api_status(app: AppHandle, state: tauri::State<'_, ApiState>) -> CommandResult<ApiStatus> {
    let mut g = state
        .0
        .lock()
        .map_err(|e| CommandError::Internal(format!("API 状态锁失败：{e}")))?;
    // A6: 活性检查 —— 服务线程可能已因 recv_error 退出（panic 已被 catch_unwind 覆盖），
    // 仅看 Option::is_some 会把死服务报成"已开启"
    let enabled = g
        .as_ref()
        .and_then(|r| r.handle.as_ref())
        .map(|h| !h.is_finished())
        .unwrap_or(false);
    if !enabled && g.is_some() {
        // 清理尸体并同步开关标志，避免下次启动按 flag 自动恢复一个已死状态
        *g = None;
        clear_enabled_flag(&app);
    }
    drop(g);
    let token = load_or_create_token(&app)?;
    Ok(ApiStatus {
        enabled,
        port: API_PORT,
        token,
    })
}

/// 重新生成 Bearer token：写新 token 文件；若服务运行中则重启生效
#[tauri::command]
pub fn api_rotate_token(
    app: AppHandle,
    state: tauri::State<'_, ApiState>,
) -> CommandResult<ApiInfo> {
    let dir = db::data_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("api-token.txt");
    let old = std::fs::read_to_string(&path).ok();
    let token = uuid::Uuid::new_v4().simple().to_string();
    let was_running = state.0.lock().map_err(|e| e.to_string())?.is_some();
    if !was_running {
        std::fs::write(&path, &token).map_err(|e| e.to_string())?;
        return Ok(ApiInfo {
            port: API_PORT,
            token,
        });
    }
    // 运行中：先落新 token（api_start 从文件读取），再重启生效。
    // A6: 重启失败则回滚旧 token 并尽力恢复服务，
    // 避免"服务已停 + flag 已清 + token 已换"三态不一致
    std::fs::write(&path, &token).map_err(|e| e.to_string())?;
    api_stop(app.clone(), state.clone())?;
    match api_start(app.clone(), state.clone()) {
        Ok(info) => Ok(info),
        Err(e) => {
            if let Some(old) = old {
                let _ = std::fs::write(&path, old);
            }
            let _ = api_start(app, state); // 尽力用旧 token 恢复服务
            Err(e)
        }
    }
}

// ───────────────────────── 单元测试 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as IoRead, Write};

    use crate::api::MemStore;

    fn http(
        port: u16,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<&str>,
    ) -> (u16, String) {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let body = body.unwrap_or("");
        let mut raw =
            format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
        if let Some(t) = token {
            raw.push_str(&format!("Authorization: Bearer {t}\r\n"));
        }
        if !body.is_empty() {
            raw.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                body.len()
            ));
        }
        raw.push_str("\r\n");
        raw.push_str(body);
        s.write_all(raw.as_bytes()).unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp).unwrap();
        let status = resp
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let body = resp.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        (status, body)
    }

    #[test]
    fn api_auth_and_crud() {
        let store: Arc<dyn TaskStore> = Arc::new(MemStore {
            tasks: Mutex::new(vec![]),
            hub: EventHub::new(),
        });
        let token = "test-token-123".to_string();
        let mut running = start_api(48821, token.clone(), store.clone(), None, None, None).unwrap();

        // 健康检查：免鉴权
        let (st, body) = http(48821, "GET", "/api/health", None, None);
        assert_eq!(st, 200);
        assert!(body.contains("wmessage-api"));

        // 无 token / 错 token → 401
        assert_eq!(http(48821, "GET", "/api/tasks", None, None).0, 401);
        assert_eq!(http(48821, "GET", "/api/tasks", Some("wrong"), None).0, 401);

        // 空列表
        let (st, body) = http(48821, "GET", "/api/tasks", Some(&token), None);
        assert_eq!(st, 200);
        assert_eq!(body.trim(), "[]");

        // 创建
        let (st, body) = http(
            48821,
            "POST",
            "/api/tasks",
            Some(&token),
            Some(r#"{"title":"测试任务","status":"doing"}"#),
        );
        assert_eq!(st, 201);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let id = v["id"].as_str().unwrap().to_string();
        assert_eq!(v["title"], "测试任务");
        assert_eq!(v["status"], "doing");
        assert_eq!(v["column"], "doing");

        // 单条
        let (st, _) = http(
            48821,
            "GET",
            &format!("/api/tasks/{id}"),
            Some(&token),
            None,
        );
        assert_eq!(st, 200);

        // 更新：title + status done → 记完成时间
        let (st, body) = http(
            48821,
            "PUT",
            &format!("/api/tasks/{id}"),
            Some(&token),
            Some(r#"{"title":"改过","status":"done"}"#),
        );
        assert_eq!(st, 200);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["title"], "改过");
        assert_eq!(v["status"], "done");
        assert!(v["completedAt"].is_number());

        // 非法 status → 400
        let (st, _) = http(
            48821,
            "PUT",
            &format!("/api/tasks/{id}"),
            Some(&token),
            Some(r#"{"status":"bad"}"#),
        );
        assert_eq!(st, 400);

        // 更新：due + tags（P0：之前不支持）
        let (st, body) = http(
            48821,
            "PUT",
            &format!("/api/tasks/{id}"),
            Some(&token),
            Some(r#"{"due":"2026-08-20T09:00","tags":["a","b"]}"#),
        );
        assert_eq!(st, 200);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["due"], "2026-08-20T09:00");
        assert_eq!(v["tags"][0], "a");

        // 更新：归档 / 恢复
        let (st, body) = http(
            48821,
            "PUT",
            &format!("/api/tasks/{id}"),
            Some(&token),
            Some(r#"{"archived":true}"#),
        );
        assert_eq!(st, 200);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["archived"],
            true
        );
        let (st, _) = http(
            48821,
            "PUT",
            &format!("/api/tasks/{id}"),
            Some(&token),
            Some(r#"{"archived":false}"#),
        );
        assert_eq!(st, 200);

        // DELETE 软删 → 默认列表排除，?trash=1 可见，重复删幂等
        let (st, _) = http(
            48821,
            "DELETE",
            &format!("/api/tasks/{id}"),
            Some(&token),
            None,
        );
        assert_eq!(st, 200);
        let (st, body) = http(48821, "GET", "/api/tasks", Some(&token), None);
        assert_eq!(st, 200);
        assert!(!body.contains(&id), "默认列表应排除回收站任务");
        let (st, body) = http(48821, "GET", "/api/tasks?trash=1", Some(&token), None);
        assert_eq!(st, 200);
        assert!(body.contains(&id), "trash=1 应包含回收站任务");
        let (st, _) = http(
            48821,
            "DELETE",
            &format!("/api/tasks/{id}"),
            Some(&token),
            None,
        );
        assert_eq!(st, 200, "重复删除应幂等 200");

        // 恢复：deleted=false
        let (st, _) = http(
            48821,
            "PUT",
            &format!("/api/tasks/{id}"),
            Some(&token),
            Some(r#"{"deleted":false}"#),
        );
        assert_eq!(st, 200);

        // 列表过滤：非法 status → 400
        let (st, _) = http(48821, "GET", "/api/tasks?status=bad", Some(&token), None);
        assert_eq!(st, 400);

        // ?status=done 只含恢复后的完成态任务（该任务之前被改到 done）
        let (st, body) = http(48821, "GET", "/api/tasks?status=done", Some(&token), None);
        assert_eq!(st, 200);
        assert!(body.contains(&id));

        // 不存在 → 404
        let (st, _) = http(48821, "GET", "/api/tasks/nope", Some(&token), None);
        assert_eq!(st, 404);

        running.shutdown.store(true, Ordering::SeqCst);
        if let Some(h) = running.handle.take() {
            let _ = h.join();
        }
    }

    #[test]
    fn sse_receives_change_events() {
        let store: Arc<dyn TaskStore> = Arc::new(MemStore {
            tasks: Mutex::new(vec![]),
            hub: EventHub::new(),
        });
        let token = "test-token-123".to_string();
        let mut running = start_api(48822, token.clone(), store.clone(), None, None, None).unwrap();

        // 建立 SSE 连接
        let mut s = std::net::TcpStream::connect(("127.0.0.1", 48822)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(
            s,
            "GET /api/events HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\n\r\n"
        )
        .unwrap();
        let mut buf = [0u8; 4096];
        let mut acc = String::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut got = false;
        while std::time::Instant::now() < deadline {
            let n = s.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            acc.push_str(&String::from_utf8_lossy(&buf[..n]));
            if acc.contains("connected") {
                got = true;
                break;
            }
        }
        assert!(got, "SSE 首事件未收到，实际内容：{acc}");

        // 另一连接 POST → SSE 应收到 tasks-changed
        http(
            48822,
            "POST",
            "/api/tasks",
            Some(&token),
            Some(r#"{"title":"sse任务"}"#),
        );
        let mut acc = String::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut ok = false;
        while std::time::Instant::now() < deadline {
            let n = s.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            acc.push_str(&String::from_utf8_lossy(&buf[..n]));
            if acc.contains("tasks-changed") {
                ok = true;
                break;
            }
        }
        assert!(ok, "SSE 未收到任务变更事件，实际内容：{acc}");

        running.shutdown.store(true, Ordering::SeqCst);
        if let Some(h) = running.handle.take() {
            let _ = h.join();
        }
    }
}