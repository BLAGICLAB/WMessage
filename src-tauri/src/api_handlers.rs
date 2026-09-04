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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tiny_http::{Header, Method, Request, Response, StatusCode};

use crate::api::{TaskStore, TauriStore, API_PORT};
use crate::api_auth::{
    clear_enabled_flag, load_or_create_token, verify_bearer, write_enabled_flag,
};
use crate::api_server::{ApiState, EventHub, RunningApi, start_api};
use crate::audit::AuditLevel;
use crate::audit_event;
use crate::db;
use crate::error::{CommandError, CommandResult};

/// 请求体上限（防内存打爆）
const MAX_BODY_BYTES: u64 = 1_000_000;
/// body 读取总时长上限（2026-09-04 审计 P1-2）：vendor patch 的 30s 读超时是
/// 「单次 read 系统调用」级——发完 header 后以 <30s 间隔滴注 body，每次 read 都按时
/// 返回，worker 永久占住并发名额（MAX_WORKERS=64 占满即全员 503）。
/// 分块读循环在每次 read 返回后检查总时长，滴注最迟 35s 被拒（408）。
const BODY_READ_DEADLINE: Duration = Duration::from_secs(35);
/// 每分钟请求上限（仅回环，防失控脚本）
const RATE_LIMIT_PER_MIN: u32 = 120;
/// SSE 并发连接上限（A6：每连接一个 writer 线程，不设上限可被连接洪泛耗尽线程）
const MAX_SSE_CLIENTS: usize = 32;

/// API 写操作 read-modify-write 串行化锁（2026-08-28 批次4审计 P1-2/P2-3）：
/// create/update/delete 的 load→改→upsert 两段式原先无锁，并发 API 请求
/// （MAX_WORKERS=64）在窗口内互相用旧快照整行覆盖；create 的 max_order 同病。
/// 注意：本锁只串行化 API 自身的并发写——跨路径（API vs UI/bot）的整行覆盖
/// lost-update 属已立项的「字段级合并写入」架构项，不在此锁覆盖范围。
static API_RMW_LOCK: Mutex<()> = Mutex::new(());

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
        (Method::Get, "/api/tasks") => list_tasks(req, store, &query, log),
        (Method::Get, "/api/events") => sse_connect(req, store, &query),
        (Method::Post, "/api/tasks") => create_task(req, store, emit_fn, log),
        _ => {
            if let Some(id) = path.strip_prefix("/api/tasks/") {
                match method {
                    Method::Get => get_task(req, store, id, log),
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

/// 请求体读取结果（2026-09-04 审计 P2-6）：原先一律 None 由调用方回 413，
/// 把读 IO 错误（含 30s 单次读超时）也误报成「body 过大」。现在分流：
/// `TooLarge` → 413；`IoFailed` → 408（读超时/连接中断语义）。
enum BodyRead {
    Ok(String),
    TooLarge,
    IoFailed,
}

/// 读请求体（上限 `MAX_BODY_BYTES`，总时长 `BODY_READ_DEADLINE`）。
/// Content-Length 声明即超限的直接预拒（413），不再读完才判。
fn read_body_limited(req: &mut Request) -> BodyRead {
    // 2026-09-04 审计 P1-2：按 Content-Length 预拒绝——声明 >1MB 的 body 不必读
    if let Some(declared) = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Content-Length"))
        .and_then(|h| h.value.as_str().parse::<u64>().ok())
    {
        if declared > MAX_BODY_BYTES {
            return BodyRead::TooLarge;
        }
    }
    let deadline = Instant::now() + BODY_READ_DEADLINE;
    let mut buf = Vec::new();
    let mut reader = req.as_reader().take(MAX_BODY_BYTES + 1);
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() as u64 > MAX_BODY_BYTES {
                    return BodyRead::TooLarge;
                }
                // 滴注检查：每次 read 返回后看总时长（单次 read 的 30s 超时管不到
                // 「每次都按时返回」的 slowloris，见 BODY_READ_DEADLINE 注释）
                if Instant::now() >= deadline {
                    return BodyRead::IoFailed;
                }
            }
            Err(_) => return BodyRead::IoFailed,
        }
    }
    BodyRead::Ok(String::from_utf8_lossy(&buf).into_owned())
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
// 2026-09-04 审计 P2-8：filePath 原先只 trim 不限长，单字段近百 KB 可进库；
// 取 1024（macOS PATH_MAX 量级），与 title/note 等字段一样走 over_limit
const API_MAX_FILE_PATH: usize = 1024;

fn valid_status(s: &str) -> bool {
    matches!(s, "todo" | "doing" | "done")
}

fn over_limit(v: &str, max: usize, what: &str) -> Option<String> {
    (v.chars().count() > max).then(|| format!("{what}过长（上限 {max} 字）"))
}

/// 拼一行任务变更日志（P2-2：title 过 escape_for_log——标题里的 `\n` / `| ` 会伪造
/// 日志行或撕裂多行；抽出纯函数便于单测，after_change 只做 IO）。
fn change_log_line(op: &str, task: &db::Task) -> String {
    format!(
        "change op={} id={} status={} title={}",
        op,
        task.id,
        task.column,
        crate::audit::escape_for_log(&task.title, 2 * API_MAX_TITLE)
    )
}

/// 500 对外统一文案（2026-08-28 批次4审计 P2-5）：DB 错误原文可能含 SQL 片段/路径，
/// 不回吐给客户端；原文转义后进 api.log 供排查。
fn internal_err(req: Request, log: &Option<PathBuf>, e: &str) {
    log_line(
        log,
        &format!("internal_error | {}", crate::audit::escape_for_log(e, 300)),
    );
    let _ = req.respond(json_err(StatusCode(500), "internal error"));
}

/// T1-1：upsert 失败分流——RMW 基线冲突（其他写者已改/删该行）→ 409（客户端应重读后重试）；
/// 其余错误走 500 统一文案。
fn upsert_err(req: Request, log: &Option<PathBuf>, e: &str) {
    if e.starts_with(db::CONFLICT_ERR_PREFIX) {
        log_line(
            log,
            &format!("conflict | {}", crate::audit::escape_for_log(e, 300)),
        );
        let _ = req.respond(json_err(
            StatusCode(409),
            "任务已被其他端修改或删除，请重试",
        ));
    } else {
        internal_err(req, log, e);
    }
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
    log_line(log, &change_log_line(op, task));
    if let Some(f) = emit_fn {
        f(task);
    }
}

// ───────────────────────── 处理器 ─────────────────────────

fn list_tasks(req: Request, store: &Arc<dyn TaskStore>, query: &str, log: &Option<PathBuf>) {
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
            internal_err(req, log, &e);
        }
    }
}

fn get_task(req: Request, store: &Arc<dyn TaskStore>, id: &str, log: &Option<PathBuf>) {
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
            internal_err(req, log, &e);
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
    let body = match read_body_limited(&mut req) {
        BodyRead::Ok(b) => b,
        BodyRead::TooLarge => {
            let _ = req.respond(json_err(StatusCode(413), "body too large"));
            return;
        }
        // P2-6（2026-09-04 审计）：读 IO 错误/超时不是「body 过大」，回 408
        BodyRead::IoFailed => {
            let _ = req.respond(json_err(StatusCode(408), "body read failed or timed out"));
            return;
        }
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
    // 2026-09-04 审计 P2-8：filePath 与 title/note/due 对齐补 over_limit
    if let Some(p) = input
        .file_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Some(e) = over_limit(p, API_MAX_FILE_PATH, "文件路径") {
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

    // P1-2/P2-3（2026-08-28 批次4审计）：load→max_order→upsert 全程持 API_RMW_LOCK——
    // 原先两段式无锁，并发 create 算出相同 order、并发写互相用旧快照整行覆盖
    let _rmw = API_RMW_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let all = match store.load() {
        Ok(v) => v,
        Err(e) => {
            internal_err(req, log, &e);
            return;
        }
    };
    let max_order = all.iter().filter_map(|t| t.order).fold(0.0f64, f64::max);
    let now = now_ms();
    let task = db::Task {
        id: uuid::Uuid::new_v4().to_string(),
        title,
        // due 与 note 同规则：trim 后存储（2026-08-28 批次4审计 P2-4：note/files 原先存原文，
        // 与注释矛盾，首尾空白进库）
        due: input
            .due
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty()),
        note: input
            .note
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty()),
        tags: input.tags,
        // 多文件绑定（2026-08-19）：API 入参仍是旧单绑定字段，双写进 files 保持一致
        files: input
            .file_path
            .as_ref()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .map(|p| {
                vec![db::TaskFile {
                    path: p,
                    is_dir: input.file_is_dir.unwrap_or(false),
                }]
            }),
        file_path: input
            .file_path
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty()),
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
        expected_updated_at: None, // 新建任务：无读快照基线（T1-1）
    };
    if let Err(e) = store.upsert(vec![task.clone()]) {
        internal_err(req, log, &e);
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
    let body = match read_body_limited(&mut req) {
        BodyRead::Ok(b) => b,
        BodyRead::TooLarge => {
            let _ = req.respond(json_err(StatusCode(413), "body too large"));
            return;
        }
        // P2-6（2026-09-04 审计）：读 IO 错误/超时不是「body 过大」，回 408
        BodyRead::IoFailed => {
            let _ = req.respond(json_err(StatusCode(408), "body read failed or timed out"));
            return;
        }
    };
    let input: UpdateReq = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => {
            let _ = req.respond(json_err(StatusCode(400), "invalid json"));
            return;
        }
    };

    // P1-2（2026-08-28 批次4审计）：load→改→upsert 全程持 API_RMW_LOCK（API 写串行化）
    let _rmw = API_RMW_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tasks = match store.load() {
        Ok(v) => v,
        Err(e) => {
            internal_err(req, log, &e);
            return;
        }
    };
    let Some(idx) = tasks.iter().position(|t| t.id == id) else {
        let _ = req.respond(json_err(StatusCode(404), "task not found"));
        return;
    };
    let mut t = tasks[idx].clone();
    // T1-1：RMW 基线 = 本次 load 快照的 updated_at；upsert 写前比对，基线外有写者改行 → 409 拒写
    // 2026-09-04 审计 P2-4：updated_at 为 NULL 的老行改用「行存在性」哨兵基线
    // （BASELINE_NULL_ROW：行被删/被改都 409）——原先基线 None = 跳过比对，老行裸奔
    t.expected_updated_at = Some(t.updated_at.unwrap_or(db::BASELINE_NULL_ROW));

    if let Some(title) = input.title.as_deref() {
        let tt = title.trim();
        // P2-9（2026-08-28 批次4审计）：显式传了 trim 后为空的 title 按 400 拒绝，
        // 与 create 对齐（原先静默忽略，调用方无法区分「没传」和「传了空白」）
        if tt.is_empty() {
            let _ = req.respond(json_err(StatusCode(400), "title 不能为空"));
            return;
        }
        if let Some(e) = over_limit(tt, API_MAX_TITLE, "任务标题") {
            let _ = req.respond(json_err(StatusCode(400), &e));
            return;
        }
        t.title = tt.to_string();
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
        // P2-4（2026-08-28 批次4审计）：trim 后存储（原先存原文，首尾空白进库）
        let fp = fp.trim();
        if fp.is_empty() {
            t.file_path = None;
            t.file_is_dir = None;
            // 多文件绑定（2026-08-19）：旧字段清空时同步清 files
            t.files = None;
        } else {
            // 2026-09-04 审计 P2-8：与 create_task 对齐补 over_limit（原先只 trim 不限长）
            if let Some(e) = over_limit(fp, API_MAX_FILE_PATH, "文件路径") {
                let _ = req.respond(json_err(StatusCode(400), &e));
                return;
            }
            t.file_path = Some(fp.to_string());
            if let Some(fid) = input.file_is_dir {
                t.file_is_dir = Some(fid);
            }
            // 双写 files（旧单绑定语义 = 唯一一条）
            t.files = Some(vec![db::TaskFile {
                path: fp.to_string(),
                is_dir: t.file_is_dir.unwrap_or(false),
            }]);
        }
    } else if let Some(fid) = input.file_is_dir {
        if t.file_path.is_some() {
            t.file_is_dir = Some(fid);
            if let Some(fp) = t.file_path.clone() {
                t.files = Some(vec![db::TaskFile {
                    path: fp,
                    is_dir: fid,
                }]);
            }
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
        upsert_err(req, log, &e);
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
    // P1-2（2026-08-28 批次4审计）：load→改→upsert 全程持 API_RMW_LOCK（API 写串行化）
    let _rmw = API_RMW_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tasks = match store.load() {
        Ok(v) => v,
        Err(e) => {
            internal_err(req, log, &e);
            return;
        }
    };
    let Some(idx) = tasks.iter().position(|t| t.id == id) else {
        let _ = req.respond(json_err(StatusCode(404), "task not found"));
        return;
    };
    let mut t = tasks[idx].clone();
    // T1-1：RMW 基线 = 本次 load 快照的 updated_at（同 update_task）；
    // 2026-09-04 审计 P2-4：NULL 老行同样走行存在性哨兵基线
    t.expected_updated_at = Some(t.updated_at.unwrap_or(db::BASELINE_NULL_ROW));
    if t.deleted_at.is_some() {
        // 已在回收站：幂等返回当前状态
        let _ = req.respond(json_ok(StatusCode(200), &TaskOut::from_task(&t)));
        return;
    }
    t.deleted_at = Some(now_ms());
    t.updated_at = Some(now_ms());
    if let Err(e) = store.upsert(vec![t.clone()]) {
        upsert_err(req, log, &e);
        return;
    }
    after_change(store, &t, "deleted", emit_fn, log);
    let _ = req.respond(json_ok(StatusCode(200), &TaskOut::from_task(&t)));
}

// ───────────────────────── SSE writer 生命周期（G1）─────────────────────────

/// SSE writer 线程注册项（G1）：按 hub 分组，stop 标志 + JoinHandle。
/// 原先 writer 线程 spawn 后无人追踪：api_stop / api_rotate_token 只 join accept
/// 线程，旧 hub 的 tx 不被 drop，writer 循环发 keepalive —— 旧客户端以为活着却
/// 永远收不到新事件，且每次 rotate 累积一批泄漏线程。
struct SseWriterReg {
    /// Arc<EventHub> 身份指针（仅作分组键，永不解引用）
    hub_key: usize,
    stop: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}

static SSE_WRITERS: Mutex<Vec<SseWriterReg>> = Mutex::new(Vec::new());

/// 当前服务实例的 hub 分组键（api_start 时记录，api_stop 据此停对应 writer）
static API_HUB_KEY: AtomicUsize = AtomicUsize::new(0);

/// writer 退出通知的兜底 join 超时（G1）：超时仍不退出的 detach + ERROR 审计
const SSE_STOP_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

fn register_sse_writer(hub_key: usize, stop: Arc<AtomicBool>, handle: std::thread::JoinHandle<()>) {
    let mut g = SSE_WRITERS.lock().unwrap_or_else(|e| e.into_inner());
    // 顺手收割已退出（客户端断开）的 writer，防注册表无界增长
    g.retain(|w| !w.handle.is_finished());
    g.push(SseWriterReg {
        hub_key,
        stop,
        handle,
    });
}

/// 停掉指定 hub 的全部 SSE writer（G1）：置 stop 标志 → 带超时 join；
/// 超时仍不退出的 drop handle（detach）并记 ERROR 审计「sse_writer_leaked」。
fn stop_sse_writers(hub_key: usize, timeout: Duration, audit: &mut dyn FnMut(&str)) {
    let writers = {
        let mut g = SSE_WRITERS.lock().unwrap_or_else(|e| e.into_inner());
        let mut taken = Vec::new();
        let mut i = 0;
        while i < g.len() {
            if g[i].hub_key == hub_key {
                taken.push(g.remove(i));
            } else {
                i += 1;
            }
        }
        taken
    };
    for w in &writers {
        w.stop.store(true, Ordering::SeqCst);
    }
    let deadline = Instant::now() + timeout;
    for w in writers {
        let h = w.handle;
        loop {
            if h.is_finished() {
                let _ = h.join();
                break;
            }
            if Instant::now() >= deadline {
                audit("sse_writer_leaked | writer join 超时未退出，已 detach");
                break; // drop(h) = detach
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// 注册 SSE 客户端：支持 `?since=<事件id>` 断线重放，然后用 tiny_http upgrade 直写。
/// 重放窗口上限 = EVENT_HISTORY（1000 条环形缓冲，api_server.rs）：溢出缺段为已知取舍。
fn sse_connect(req: Request, store: &Arc<dyn TaskStore>, query: &str) {
    // 2026-09-04 审计 P2-7：since 给了但 parse 失败（如 ?since=abc）回 400——
    // 原先静默按全新连接处理，客户端不知道自己丢了重放窗口
    let since = match query_param(query, "since") {
        Some(s) => match s.parse::<u64>() {
            Ok(v) => Some(v),
            Err(_) => {
                let _ = req.respond(json_err(
                    StatusCode(400),
                    "since 必须是非负整数（事件 id）",
                ));
                return;
            }
        },
        None => None,
    };
    // A2: sync_channel(256) — 单客户端最多积压 256 条，超出则丢事件（广播不阻塞）
    // P2-3：载荷带事件 id，writer 端据此与断线重放去重
    let (tx, rx) = sync_channel::<(u64, Vec<u8>)>(256);
    // 锁中毒时用 into_inner 恢复（与 broadcast 端策略一致，审计 P3：原先静默跳过，
    // 客户端注册失败则该 SSE 连接永远收不到事件）
    let hub = store.event_hub();
    // 2026-08-28 批次4审计 P1-3：writer 存活令牌——注册前收割死连接尸体
    // （原先只在 broadcast 失败时移除，安静期内尸体占满名额 → 新连接 503）
    let alive = Arc::new(());
    {
        let mut clients = hub.clients.lock().unwrap_or_else(|e| e.into_inner());
        clients.retain(|(_, token)| token.upgrade().is_some());
        // A6: SSE 连接数上限 —— 超限 503，防连接洪泛耗尽线程
        if clients.len() >= MAX_SSE_CLIENTS {
            drop(clients);
            let _ = req.respond(json_err(StatusCode(503), "too many SSE connections"));
            return;
        }
        clients.push((tx, Arc::downgrade(&alive)));
    }
    let hub = hub.clone();
    // G1：writer 线程纳入追踪 —— stop 标志供 api_stop 通知退出，JoinHandle 入注册表
    let hub_key = Arc::as_ptr(&hub) as usize;
    let stop = Arc::new(AtomicBool::new(false));
    let stop_w = stop.clone();
    let handle = std::thread::spawn(move || {
        // P1-3：存活令牌随 writer 线程存活，线程退出（客户端断开/服务停止）即失效
        let _alive = alive;
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
        // A2: 写超时通过 recv_timeout 心跳 + 客户端断开检测协同处理
        // tiny_http ResponseBox 不提供 set_write_timeout，故通过 recv 端超时兜底
        // G1: recv tick 从 15s 改 1s（每 15 tick 发一次心跳，对外节奏不变），
        //     使 stop 标志最迟 1s 内被轮询到，writer 能及时退出被 join

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
        // P2-3：重放与在线推送存在竞态——客户端注册进 clients 之后、重放快照之前
        // 广播的事件会同时出现在 history 与在线队列里。记录已发最大 id，
        // 在线循环里 id <= last_sent 的一律跳过（服务器侧去重）。
        let mut last_sent: u64 = since.unwrap_or(0);
        if since.is_some() {
            for (id, msg) in hub.replay(last_sent) {
                if stream.write_all(msg.as_bytes()).is_err() {
                    return;
                }
                if id > last_sent {
                    last_sent = id;
                }
            }
            let _ = stream.flush();
        }
        let mut idle_ticks = 0u32;
        loop {
            // G1：服务停止/重启时 api_stop 置位 —— 不停则旧客户端看着 keepalive
            // 以为活着，却永远收不到新 hub 的事件
            if stop_w.load(Ordering::SeqCst) {
                break;
            }
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok((id, data)) => {
                    // P2-3：重放已覆盖的事件（id <= last_sent）跳过，不重复推
                    if id <= last_sent {
                        continue;
                    }
                    last_sent = id;
                    idle_ticks = 0;
                    if stream.write_all(&data).is_err() || stream.flush().is_err() {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    idle_ticks += 1;
                    if idle_ticks >= 15 {
                        idle_ticks = 0;
                        // SSE 心跳注释，保持连接存活
                        if stream.write_all(b": keepalive\n\n").is_err() {
                            break;
                        }
                        let _ = stream.flush();
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    register_sse_writer(hub_key, stop, handle);
}

// ───────────────────────── tauri 命令 ─────────────────────────

#[tauri::command]
pub fn api_start(app: AppHandle, state: tauri::State<'_, ApiState>) -> CommandResult<ApiInfo> {
    // 2026-08-28 批次4审计 P2-8：检查与写入在同一把锁内完成——原先锁释放后才 start，
    // 并发 invoke 双发都过检查，第二个收到误导的「端口占用」（服务其实已被第一个起好）
    let mut g = state.0.lock().map_err(|e| e.to_string())?;
    api_start_locked(&app, &mut g)
}

/// api_start 的持锁实现（2026-09-04 审计 P2-5 拆出）：供 api_start / api_rotate_token
/// 复用，调用方必须已持 `state.0` 锁（rotate 全程持锁，检查与操作原子）。
fn api_start_locked(app: &AppHandle, g: &mut Option<RunningApi>) -> CommandResult<ApiInfo> {
    // 2026-09-04 审计 P1-1 后：尸体 join 最坏 ~400ms（accept recv_timeout tick），
    // accept 线程不再等 worker，持锁清理安全
    if let Some(running) = g.as_ref() {
        // 2026-09-04 审计 P2-1：与 api_status 同款活性检查——accept 线程已死
        // （recv_error 退出）时不得误报成功；清尸体后继续走下面的重启
        let alive = running
            .handle
            .as_ref()
            .map(|h| !h.is_finished())
            .unwrap_or(false);
        if alive {
            return Ok(ApiInfo {
                port: API_PORT,
                token: load_or_create_token(app)?,
            });
        }
    }
    if let Some(mut r) = g.take() {
        r.shutdown.store(true, Ordering::SeqCst);
        if let Some(h) = r.handle.take() {
            let _ = h.join();
        }
    }
    let token = load_or_create_token(app)?;
    let store: Arc<dyn TaskStore> = Arc::new(TauriStore {
        app: app.clone(),
        // A6: id 持久化，跨重启保持单调（否则客户端 Last-Event-ID 去重会静默丢事件）
        hub: EventHub::persisted(db::data_dir(app).join("api-event-id.txt")),
    });
    let emit_app = app.clone();
    let emit: Option<Box<dyn Fn(&db::Task) + Send + Sync>> =
        Some(Box::new(move |task: &db::Task| {
            // 复用挂件→主窗口的既有通道：主窗口合并状态并广播给挂件。
            // `source: Api` 告诉主窗口：数据已由 API 线程落盘，只合并 UI 状态，不要回写
            // （回写会用旧事件快照覆盖 API 的新写入，导致归档/软删被回滚的竞态）
            let payload = serde_json::json!({ "upserts": [task], "deletes": [], "source": crate::mutation::MutationOrigin::Api.as_str() });
            let _ = emit_app.emit_to("main", "tasks-updated", &payload);
        }));
    let log_path = Some(db::data_dir(app).join("api.log"));
    let audit_app = app.clone();
    let on_error: Option<Box<dyn Fn(AuditLevel, &str, &str) + Send + Sync>> =
        Some(Box::new(move |lvl, ev, msg| {
            audit_event!(&audit_app, lvl, ev, "error" => msg);
        }));
    // G1：提前取 hub 分组键（store 随后被 move 进 start_api），
    // api_stop 据此通知并 join 该 hub 的 SSE writer
    let hub_key = Arc::as_ptr(store.event_hub()) as usize;
    // P2-2（2026-08-28 批次4审计）：显式映射 HttpStartFailed——原先 String 错误经
    // From<String> 落成无结构的 Internal，前端按 code 分支永远等不到 HTTP_START_FAILED
    let running = start_api(API_PORT, token.clone(), store, emit, log_path, on_error)
        .map_err(|e| CommandError::HttpStartFailed {
            port: API_PORT,
            reason: e,
        })?;
    API_HUB_KEY.store(hub_key, Ordering::SeqCst);
    *g = Some(running);
    write_enabled_flag(app);
    Ok(ApiInfo {
        port: API_PORT,
        token,
    })
}

#[tauri::command]
pub fn api_stop(app: AppHandle, state: tauri::State<'_, ApiState>) -> CommandResult<()> {
    api_stop_impl(&app, &state, true)
}

/// P2-24：应用退出路径（ExitRequested）的 API 停止 —— 与 api_stop 同一清理
///（G1：accept 线程 + SSE writer 全部通知并 join），但保留 api-enabled.flag：
/// 退出不是用户关开关，下次启动应按 flag 自动恢复服务。
/// 泛型 Runtime：cleanup_on_exit 的 mock runtime 测试可直调。
pub fn api_stop_for_exit<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &ApiState,
) -> CommandResult<()> {
    api_stop_impl(app, state, false)
}

fn api_stop_impl<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &ApiState,
    clear_enabled: bool,
) -> CommandResult<()> {
    let mut g = state.0.lock().map_err(|e| e.to_string())?;
    api_stop_locked(app, &mut g, clear_enabled)
}

/// api_stop_impl 的持锁实现（2026-09-04 审计 P2-5 拆出）：供 api_rotate_token
/// 在全程持 `state.0` 锁的前提下复用，消除「检查→stop→start」之间的抢锁窗口。
fn api_stop_locked<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    g: &mut Option<RunningApi>,
    clear_enabled: bool,
) -> CommandResult<()> {
    if let Some(mut r) = g.take() {
        r.shutdown.store(true, Ordering::SeqCst);
        if let Some(h) = r.handle.take() {
            let _ = h.join();
        }
        // G1：除 join accept 线程外，通知并 join 当前 hub 的全部 SSE writer ——
        // 原先 writer 不被追踪，旧 hub 的 tx 永不 drop，writer 循环发 keepalive，
        // 旧客户端僵尸挂连且线程随 rotate 无界泄漏；5s 仍不退出的 detach + ERROR 审计
        let key = API_HUB_KEY.load(Ordering::SeqCst);
        let audit_app = app.clone();
        stop_sse_writers(key, SSE_STOP_JOIN_TIMEOUT, &mut |line: &str| {
            audit_event!(&audit_app, AuditLevel::Error, "sse_writer_leaked", "error" => line);
        });
    }
    if clear_enabled {
        clear_enabled_flag(app);
    }
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
    // 2026-09-04 审计 P2-9：未启用时不读/生成 token——原先每次查状态都
    // load_or_create_token，从未开启过 API 的用户数据目录里也会落 api-token.txt。
    // 前端只在 enabled 时展示 token（SettingsPage），disabled 态回空串即可。
    let token = if enabled {
        load_or_create_token(&app)?
    } else {
        String::new()
    };
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
    // 2026-09-04 审计 P2-5：rotate 全程持 state.0 锁——原先「查 is_some → 放锁 →
    // api_stop/api_start 各自再抢锁」，窗口内用户并发 stop 会被 rotate 把服务重新拉起
    // （违背用户关闭意图）。锁内只做端口绑定/join 等毫秒级操作，无死锁风险
    // （locked 变体不再抢同一把锁）。
    let mut g = state.0.lock().map_err(|e| e.to_string())?;
    let was_running = g.is_some();
    if !was_running {
        crate::api_auth::write_token_file(&path, &token)?;
        return Ok(ApiInfo {
            port: API_PORT,
            token,
        });
    }
    // 运行中：先落新 token（api_start 从文件读取），再重启生效。
    // A6: 重启失败则回滚旧 token 并尽力恢复服务，
    // 避免"服务已停 + flag 已清 + token 已换"三态不一致
    crate::api_auth::write_token_file(&path, &token)?;
    // api_stop 内会停掉旧 hub 的全部 SSE writer（G1），旧 token 的连接随之断开，
    // token 失效语义彻底；api_start 重建新 hub 接受新 writer。
    // 2026-08-28 批次4审计 P2-6：stop 失败时回滚旧 token 文件——原先 `?` 直接返回，
    // 留下「文件已是新 token、在跑服务仍认旧 token」的三态不一致
    if let Err(e) = api_stop_locked(&app, &mut g, true) {
        if let Some(old) = &old {
            let _ = crate::api_auth::write_token_file(&path, old);
        }
        return Err(e);
    }
    match api_start_locked(&app, &mut g) {
        Ok(info) => Ok(info),
        Err(e) => {
            if let Some(old) = old {
                let _ = crate::api_auth::write_token_file(&path, &old);
            }
            let _ = api_start_locked(&app, &mut g); // 尽力用旧 token 恢复服务
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

    /// 构造一个最小 db::Task（只关心 change_log_line 用到的字段）
    fn bare_task(title: &str) -> db::Task {
        db::Task {
            id: "t1".into(),
            title: title.into(),
            due: None,
            note: None,
            tags: None,
            files: None,
            file_path: None,
            file_is_dir: None,
            column: "todo".into(),
            subtasks: None,
            completed_at: None,
            archived: None,
            deleted_at: None,
            collapsed: None,
            order: None,
            updated_at: None,
            schedule: None,
            sched_last: None,
            bot_assigned: None,
            expected_updated_at: None,
        }
    }

    #[test]
    fn change_log_line_escapes_title_newline() {
        // P2-2：标题含 \n 时日志行不得出现裸换行（防伪造日志行/多行撕裂）
        let line = change_log_line("created", &bare_task("标题\n[2026-01-01] forged | x"));
        assert!(!line.contains('\n'), "日志行不得含裸换行: {line:?}");
        assert!(line.contains("标题\\n"), "换行必须转义为 \\n: {line:?}");
        assert!(!line.contains("| x"), "裸管道符必须转义: {line:?}");
    }

    #[test]
    fn change_log_line_plain_title_unchanged() {
        let line = change_log_line("updated", &bare_task("普通标题"));
        assert_eq!(line, "change op=updated id=t1 status=todo title=普通标题");
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

    // ── SSE writer 生命周期（G1：stop 通知 + 带超时 join + 泄漏审计）──

    /// 测试专用 hub 分组键：用本地 Arc 地址保证与并行测试的真实 hub 不撞
    fn test_hub_key() -> usize {
        Arc::as_ptr(&Arc::new(())) as usize
    }

    #[test]
    fn sse_writer_stop_exits_within_timeout() {
        let key = test_hub_key();
        // 模拟一个不主动退出的 writer：事件永不来，只在 1s tick 上轮询 stop 标志
        let stop = Arc::new(AtomicBool::new(false));
        let stop_w = stop.clone();
        let (_tx, rx) = sync_channel::<Vec<u8>>(1);
        let handle = std::thread::spawn(move || loop {
            if stop_w.load(Ordering::SeqCst) {
                break;
            }
            let _ = rx.recv_timeout(Duration::from_secs(1));
        });
        register_sse_writer(key, stop, handle);
        let mut audits: Vec<String> = Vec::new();
        let start = Instant::now();
        stop_sse_writers(
            key,
            SSE_STOP_JOIN_TIMEOUT,
            &mut |l: &str| audits.push(l.to_string()),
        );
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "writer 未在 stop 后及时退出：{:?}",
            start.elapsed()
        );
        assert!(audits.is_empty(), "正常退出不应记泄漏审计: {audits:?}");
    }

    #[test]
    fn sse_writer_stuck_detaches_with_leak_audit() {
        let key = test_hub_key();
        // 卡死 writer（不轮询 stop）：join 超时后必须 detach + ERROR 审计，不得永久挂住
        let stop = Arc::new(AtomicBool::new(false));
        let handle = std::thread::spawn(move || std::thread::sleep(Duration::from_secs(30)));
        register_sse_writer(key, stop, handle);
        let mut audits: Vec<String> = Vec::new();
        let start = Instant::now();
        stop_sse_writers(
            key,
            Duration::from_millis(300),
            &mut |l: &str| audits.push(l.to_string()),
        );
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "卡死 writer 不得拖住 stop：{:?}",
            start.elapsed()
        );
        assert!(
            audits.iter().any(|l| l.contains("sse_writer_leaked")),
            "缺 sse_writer_leaked 审计: {audits:?}"
        );
    }

    // ── 2026-08-28 批次4审计：空 title 400 / trim 存储 / SSE 尸体收割 ──

    fn shutdown_server(running: &mut crate::api_server::RunningApi) {
        running.shutdown.store(true, Ordering::SeqCst);
        if let Some(h) = running.handle.take() {
            let _ = h.join();
        }
    }

    /// P2-9：显式传 trim 后为空的 title 按 400 拒绝（原先静默忽略，与 create 语义不一致）
    #[test]
    fn update_blank_title_returns_400() {
        let store: Arc<dyn TaskStore> = Arc::new(MemStore {
            tasks: Mutex::new(vec![]),
            hub: EventHub::new(),
        });
        let token = "test-token-123".to_string();
        let mut running = start_api(48823, token.clone(), store.clone(), None, None, None).unwrap();

        let (st, body) = http(
            48823,
            "POST",
            "/api/tasks",
            Some(&token),
            Some(r#"{"title":"正常任务"}"#),
        );
        assert_eq!(st, 201);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let id = v["id"].as_str().unwrap().to_string();

        // 空白 title → 400（不是静默忽略）；不传 title → 200 不动标题
        let (st, _) = http(
            48823,
            "PUT",
            &format!("/api/tasks/{id}"),
            Some(&token),
            Some(r#"{"title":"   "}"#),
        );
        assert_eq!(st, 400, "空白 title 应 400");
        let (st, body) = http(
            48823,
            "PUT",
            &format!("/api/tasks/{id}"),
            Some(&token),
            Some(r#"{"note":"只改备注"}"#),
        );
        assert_eq!(st, 200);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["title"], "正常任务", "不传 title 不应动标题");

        shutdown_server(&mut running);
    }

    /// P2-4：create 的 note / filePath 首尾空白不得进库（原先存原文，与自身注释矛盾）
    #[test]
    fn create_trims_note_and_file_path() {
        let store: Arc<dyn TaskStore> = Arc::new(MemStore {
            tasks: Mutex::new(vec![]),
            hub: EventHub::new(),
        });
        let token = "test-token-123".to_string();
        let mut running = start_api(48824, token.clone(), store.clone(), None, None, None).unwrap();

        let (st, body) = http(
            48824,
            "POST",
            "/api/tasks",
            Some(&token),
            Some(r#"{"title":"t","note":"  备注内容  ","filePath":" /tmp/x.pdf "}"#),
        );
        assert_eq!(st, 201);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["note"], "备注内容", "note 应 trim 后存储: {body}");
        assert_eq!(v["filePath"], "/tmp/x.pdf", "filePath 应 trim 后存储: {body}");

        shutdown_server(&mut running);
    }

    /// P1-3：clients 里塞满死连接尸体（writer 已退出 = Weak 失效）时，
    /// 新 SSE 连接应先收割尸体再判容量——原先直接 503 直到下次广播自愈
    #[test]
    fn sse_dead_clients_pruned_before_capacity_check() {
        let store: Arc<dyn TaskStore> = Arc::new(MemStore {
            tasks: Mutex::new(vec![]),
            hub: EventHub::new(),
        });
        let token = "test-token-123".to_string();
        // 塞满 MAX_SSE_CLIENTS 个尸体（Weak::new() 永不 upgrade = writer 已死）
        {
            let hub = store.event_hub();
            let mut clients = hub.clients.lock().unwrap();
            for _ in 0..MAX_SSE_CLIENTS {
                let (tx, _rx) = sync_channel::<(u64, Vec<u8>)>(1);
                clients.push((tx, std::sync::Weak::new()));
            }
            assert_eq!(clients.len(), MAX_SSE_CLIENTS);
        }
        let mut running = start_api(48825, token.clone(), store.clone(), None, None, None).unwrap();

        // 新连接：尸体被收割后应正常接入（200 + connected 首事件），而非 503
        let mut s = std::net::TcpStream::connect(("127.0.0.1", 48825)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(
            s,
            "GET /api/events HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\n\r\n"
        )
        .unwrap();
        let mut buf = [0u8; 4096];
        let n = s.read(&mut buf).unwrap_or(0);
        let head = String::from_utf8_lossy(&buf[..n]);
        assert!(
            head.starts_with("HTTP/1.1 200"),
            "尸体占满名额时新连接仍应接入（200），实际：{head}"
        );

        // 尸体被清、新连接占位：clients 应只剩 1 个活连接
        let hub = store.event_hub();
        let alive_count = hub
            .clients
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, w)| w.upgrade().is_some())
            .count();
        assert_eq!(alive_count, 1, "尸体应被收割，仅剩新连接: {alive_count}");

        drop(s);
        shutdown_server(&mut running);
    }

    // ── 2026-09-04 审计修复（P2-4/6/7/8）──

    /// P2-4 测试基建：MemStore 已按 db.rs 语义比对 RMW 基线；本 store 在 upsert 内
    /// 先模拟「读快照→写回」窗口里的并发写（把目标行 updated_at 推进），
    /// 使 handler 锁内 load 的基线在 upsert 时必然过期 → 走通 409 路径
    struct SabotageStore {
        inner: MemStore,
    }
    impl TaskStore for SabotageStore {
        fn load(&self) -> Result<Vec<db::Task>, String> {
            self.inner.load()
        }
        fn upsert(&self, tasks: Vec<db::Task>) -> Result<(), String> {
            for t in &tasks {
                let mut g = self.inner.tasks.lock().unwrap();
                if let Some(x) = g.iter_mut().find(|x| x.id == t.id) {
                    x.updated_at = Some(x.updated_at.unwrap_or(0) + 1);
                }
            }
            self.inner.upsert(tasks)
        }
        fn event_hub(&self) -> &Arc<EventHub> {
            self.inner.event_hub()
        }
        fn notify_change(&self, op: &str, task: &db::Task) {
            self.inner.notify_change(op, task);
        }
    }

    /// 2026-09-04 审计 P2-4：基线外有写者插队 → PUT 回 409 且不覆盖对方修改。
    /// 覆盖两种基线：正常行（updated_at 时间戳基线）与 NULL 老行（行存在性基线）。
    /// 原先 MemStore::upsert 忽略基线，该路径无集成覆盖。
    #[test]
    fn update_conflict_returns_409() {
        // 预塞一条 updated_at 为 NULL 的老行（迁移前遗留）
        let inner = MemStore {
            tasks: Mutex::new(vec![bare_task("老行任务")]),
            hub: EventHub::new(),
        };
        let store: Arc<dyn TaskStore> = Arc::new(SabotageStore { inner });
        let token = "test-token-123".to_string();
        let mut running = start_api(48826, token.clone(), store.clone(), None, None, None).unwrap();

        // NULL 老行：行存在性基线 + 插队写 → 409
        let (st, body) = http(
            48826,
            "PUT",
            "/api/tasks/t1",
            Some(&token),
            Some(r#"{"title":"覆盖"}"#),
        );
        assert_eq!(st, 409, "NULL 老行被插队改必须 409: {body}");
        let tasks = store.load().unwrap();
        assert_eq!(tasks[0].title, "老行任务", "被拒写不得覆盖现行行");

        // 正常行（API 创建，updated_at 有值）：时间戳基线 + 插队写 → 409
        let (st, body) = http(
            48826,
            "POST",
            "/api/tasks",
            Some(&token),
            Some(r#"{"title":"正常任务"}"#),
        );
        assert_eq!(st, 201);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let id = v["id"].as_str().unwrap().to_string();
        let (st, body) = http(
            48826,
            "PUT",
            &format!("/api/tasks/{id}"),
            Some(&token),
            Some(r#"{"title":"覆盖"}"#),
        );
        assert_eq!(st, 409, "时间戳基线过期必须 409: {body}");
        let tasks = store.load().unwrap();
        let cur = tasks.iter().find(|t| t.id == id).unwrap();
        assert_eq!(cur.title, "正常任务", "被拒写不得覆盖现行行");

        shutdown_server(&mut running);
    }

    /// 2026-09-04 审计 P2-4：NULL 老行在无并发写时可正常更新——
    /// 行存在性基线放行（不误伤正常路径）
    #[test]
    fn update_null_updated_at_row_ok() {
        let store: Arc<dyn TaskStore> = Arc::new(MemStore {
            tasks: Mutex::new(vec![bare_task("老行任务")]),
            hub: EventHub::new(),
        });
        let token = "test-token-123".to_string();
        let mut running = start_api(48827, token.clone(), store.clone(), None, None, None).unwrap();

        let (st, body) = http(
            48827,
            "PUT",
            "/api/tasks/t1",
            Some(&token),
            Some(r#"{"title":"改好了"}"#),
        );
        assert_eq!(st, 200, "无并发写时 NULL 老行更新应放行: {body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["title"], "改好了");

        shutdown_server(&mut running);
    }

    /// 2026-09-04 审计 P2-7：`?since=abc` 解析失败必须回 400——原先静默按全新
    /// 连接处理，客户端不知自己丢了重放窗口
    #[test]
    fn sse_invalid_since_returns_400() {
        let store: Arc<dyn TaskStore> = Arc::new(MemStore {
            tasks: Mutex::new(vec![]),
            hub: EventHub::new(),
        });
        let token = "test-token-123".to_string();
        let mut running = start_api(48828, token.clone(), store.clone(), None, None, None).unwrap();

        let (st, _) = http(48828, "GET", "/api/events?since=abc", Some(&token), None);
        assert_eq!(st, 400, "非法 since 应 400");
        let (st, _) = http(48828, "GET", "/api/events?since=-1", Some(&token), None);
        assert_eq!(st, 400, "负数 since 应 400（u64 解析失败）");

        shutdown_server(&mut running);
    }

    /// 2026-09-04 审计 P2-8：filePath 超上限（1024 字）回 400，create / update 同规则
    #[test]
    fn file_path_over_limit_returns_400() {
        let store: Arc<dyn TaskStore> = Arc::new(MemStore {
            tasks: Mutex::new(vec![]),
            hub: EventHub::new(),
        });
        let token = "test-token-123".to_string();
        let mut running = start_api(48829, token.clone(), store.clone(), None, None, None).unwrap();

        let long_path = "x".repeat(API_MAX_FILE_PATH + 1);
        let (st, _) = http(
            48829,
            "POST",
            "/api/tasks",
            Some(&token),
            Some(&format!(r#"{{"title":"t","filePath":"{long_path}"}}"#)),
        );
        assert_eq!(st, 400, "create 超限 filePath 应 400");

        let (st, body) = http(
            48829,
            "POST",
            "/api/tasks",
            Some(&token),
            Some(r#"{"title":"t"}"#),
        );
        assert_eq!(st, 201);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let id = v["id"].as_str().unwrap().to_string();
        let (st, _) = http(
            48829,
            "PUT",
            &format!("/api/tasks/{id}"),
            Some(&token),
            Some(&format!(r#"{{"filePath":"{long_path}"}}"#)),
        );
        assert_eq!(st, 400, "update 超限 filePath 应 400");

        shutdown_server(&mut running);
    }

    /// 2026-09-04 审计 P1-2/P2-6：Content-Length 声明超 1MB → 立即 413，
    /// 不必等 body 读完（请求故意一字节 body 都不发：若不预拒，服务端会等 body
    /// 直到 5s 客户端读超时，测试会失败）
    #[test]
    fn oversize_content_length_rejected_early() {
        let store: Arc<dyn TaskStore> = Arc::new(MemStore {
            tasks: Mutex::new(vec![]),
            hub: EventHub::new(),
        });
        let token = "test-token-123".to_string();
        let mut running = start_api(48830, token.clone(), store.clone(), None, None, None).unwrap();

        let mut s = std::net::TcpStream::connect(("127.0.0.1", 48830)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(
            s,
            "POST /api/tasks HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: 2000000\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let _ = s.shutdown(std::net::Shutdown::Write);
        let mut resp = String::new();
        s.read_to_string(&mut resp).unwrap();
        assert!(
            resp.starts_with("HTTP/1.1 413"),
            "声明超限的 body 应立即 413: {}",
            resp.lines().next().unwrap_or("")
        );

        shutdown_server(&mut running);
    }
}