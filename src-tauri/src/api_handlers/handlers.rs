//! HTTP 路由 + 任务 CRUD 处理器 + 响应辅助 + 序列化锁。
//!
//! 路由分发在 `handle_request`：健康检查 → 鉴权 → 限流 → 路径/方法匹配。
//! 写操作 create/update/delete 持 API_RMW_LOCK 串行化（防并发 load→改→upsert
//! 窗口内互相用旧快照整行覆盖）；跨路径（API vs UI/bot）的 lost-update 不在此锁覆盖范围。

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tiny_http::{Header, Method, Request, Response, StatusCode};

use crate::api::TaskStore;
use crate::db;
use crate::task_out::TaskOut;

use super::body::{read_body_limited, BodyRead};
use super::ratelimit::{log_line, rate_check};
use super::util::{
    after_change, change_log_line, internal_err, parse_status, upsert_err, valid_status, CreateReq,
    UpdateReq, API_MAX_DUE, API_MAX_FILE_PATH, API_MAX_NOTE, API_MAX_TAGS, API_MAX_TAG_LEN,
    API_MAX_TITLE,
};
use super::util::{now_ms, over_limit};

// ───────────────────────── 序列化锁 ─────────────────────────

/// API 写操作 read-modify-write 串行化锁：
/// create/update/delete 的 load→改→upsert 两段式若无锁，并发 API 请求
/// （MAX_WORKERS=64）会在窗口内互相用旧快照整行覆盖；create 的 max_order 同病。
/// 注意：本锁只串行化 API 自身的并发写——跨路径（API vs UI/bot）的整行覆盖
/// lost-update 属已立项的「字段级合并写入」架构项，不在此锁覆盖范围。
static API_RMW_LOCK: Mutex<()> = Mutex::new(());

// ───────────────────────── 响应辅助 ─────────────────────────

pub(crate) fn json_ok<T: Serialize>(status: StatusCode, v: &T) -> Response<Cursor<Vec<u8>>> {
    // 序列化失败显式 500 + 错误体——静默降级成 2xx + `{}` 会把数据完整性
    // 问题伪装成成功响应
    let body = match serde_json::to_vec(v) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[api] 响应序列化失败（返回 500）：{e}");
            return Response::from_data(br#"{"error":"response serialization failed"}"#.to_vec())
                .with_status_code(StatusCode(500))
                .with_header(
                    Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"application/json; charset=utf-8"[..],
                    )
                    .expect("静态 header 字节不可能失败"),
                );
        }
    };
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

pub(crate) fn json_err(status: StatusCode, msg: &str) -> Response<Cursor<Vec<u8>>> {
    json_ok(status, &serde_json::json!({ "error": msg }))
}

/// 健康检查：免鉴权，仅返回服务状态
fn health(req: Request) {
    let _ = req.respond(json_ok(
        StatusCode(200),
        &serde_json::json!({ "ok": true, "service": "wmessage-api" }),
    ));
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
    if !crate::api_auth::verify_bearer(&req, token) {
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
        (Method::Get, "/api/events") => super::sse::sse_connect(req, store, &query),
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

/// 取 query 参数（`?a=1&b=2` 形式，重复键取第一个；url crate form_urlencoded 解码，
/// 含 `%XX` 与 `+`→空格）
pub(crate) fn query_param(query: &str, key: &str) -> Option<String> {
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
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
        // 读 IO 错误/超时不是「body 过大」，回 408
        BodyRead::IoFailed => {
            let _ = req.respond(json_err(StatusCode(408), "body read failed or timed out"));
            return;
        }
        // 多 Content-Length / parse 失败 → 400 拒绝
        BodyRead::Malformed => {
            let _ = req.respond(json_err(StatusCode(400), "malformed Content-Length header"));
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
    // 长度校验与 update_task / bot 工具侧对齐
    if let Some(e) = over_limit(&title, API_MAX_TITLE, "任务标题") {
        let _ = req.respond(json_err(StatusCode(400), &e));
        return;
    }
    if let Some(e) = super::validate::check_field(input.note.as_deref(), API_MAX_NOTE, "备注") {
        let _ = req.respond(json_err(StatusCode(400), &e));
        return;
    }
    if let Some(e) = super::validate::check_field(input.due.as_deref(), API_MAX_DUE, "截止时间")
    {
        let _ = req.respond(json_err(StatusCode(400), &e));
        return;
    }
    // filePath 与 title/note/due 同规则限长
    if let Some(e) =
        super::validate::check_field(input.file_path.as_deref(), API_MAX_FILE_PATH, "文件路径")
    {
        let _ = req.respond(json_err(StatusCode(400), &e));
        return;
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
    // 校验与解析同源(parse_status):一次拿到 enum,后续不再 parse().expect(),
    // 杜绝 valid_status 与 parse 日后分叉时在 handler 线程上 panic。
    let column: db::TaskStatus = match input.status.as_deref() {
        None | Some("") => db::TaskStatus::Todo,
        Some(s) => match parse_status(s) {
            Some(c) => c,
            None => {
                let _ = req.respond(json_err(StatusCode(400), "status 必须是 todo/doing/done"));
                return;
            }
        },
    };

    // load→max_order→upsert 全程持 API_RMW_LOCK——
    // 两段式无锁会让并发 create 算出相同 order、并发写互相用旧快照整行覆盖。
    // 锁内只做 DB 读写，错误经 labeled block 装盒、出锁后再响应（OCR C5-AP-05）——
    // 持锁跨 socket I/O 会让慢客户端串行化全部并发 API 写。
    // create 的 load/upsert 失败原都走 internal_err（500），保持。
    let task = match 'rmw: {
        let _rmw = API_RMW_LOCK.lock().unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] api_handlers::handlers API_RMW_LOCK: {e:?}");
            e.into_inner()
        });
        let all = match store.load() {
            Ok(v) => v,
            Err(e) => break 'rmw Err(e),
        };
        let max_order = all.iter().filter_map(|t| t.order).fold(0.0f64, f64::max);
        let now = now_ms();
        let task = db::Task {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            // due 与 note 同规则：trim 后存储，首尾空白不进库
            due: input
                .due
                .map(|d| d.trim().to_string())
                .filter(|d| !d.is_empty()),
            note: input
                .note
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty()),
            tags: input.tags,
            // API 入参仍是单绑定字段，双写进 files 与多文件绑定保持一致
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
            column,
            subtasks: None,
            completed_at: if column == db::TaskStatus::Done {
                Some(now)
            } else {
                None
            },
            archived: if column == db::TaskStatus::Done {
                Some(false)
            } else {
                None
            },
            deleted_at: None,
            collapsed: None,
            order: Some(max_order + 1.0),
            updated_at: Some(now),
            schedule: None,
            sched_last: None,
            bot_assigned: None,
            expected_updated_at: None, // 新建任务：无读快照基线
        };
        if let Err(e) = store.upsert(vec![task.clone()]) {
            break 'rmw Err(e);
        }
        Ok(task)
    } {
        Ok(t) => t,
        Err(e) => {
            internal_err(req, log, &e);
            return;
        }
    };
    after_change(store, &task, "created", emit_fn, log);
    let _ = req.respond(json_ok(StatusCode(201), &TaskOut::from_task(&task)));
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
        // 读 IO 错误/超时不是「body 过大」，回 408
        BodyRead::IoFailed => {
            let _ = req.respond(json_err(StatusCode(408), "body read failed or timed out"));
            return;
        }
        // 多 Content-Length / parse 失败 → 400 拒绝
        BodyRead::Malformed => {
            let _ = req.respond(json_err(StatusCode(400), "malformed Content-Length header"));
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

    // 入参校验:纯函数,不依赖 DB,锁前跑。
    // 锁后校验会在 400 应答路径上持锁串行化所有后续 API 请求(DoS 放大面),
    // 且 prepare_for_upsert 会白做一次内存戳。
    if let Err(e) = validate_update_input(&input) {
        let _ = req.respond(json_err(StatusCode(400), &e));
        return;
    }

    // 校验与解析同源:在持锁前一次性解出 enum,锁内直接复用。
    // 避免锁内 s.parse().expect() 依赖「validate_update_input 已校验」这一跨函数不变量——
    // 两者的状态字符串解读万一分叉,不再 panic 掉 handler 线程。
    let new_column: Option<crate::db::TaskStatus> = match input.status.as_deref() {
        Some(s) if !s.is_empty() => match parse_status(s) {
            Some(c) => Some(c),
            None => {
                let _ = req.respond(json_err(StatusCode(400), "status 必须是 todo/doing/done"));
                return;
            }
        },
        _ => None,
    };

    // load→改→upsert 全程持 API_RMW_LOCK(API 写串行化)。
    // 作用域块收窄锁:块一结束即释放,after_change(SSE fanout)不持锁
    // (持锁会串行化所有 API 写跨 SSE 网络延迟)。
    // 锁内只做 DB 读写，错误经 labeled block 装盒、出锁后再响应（OCR C5-AP-05）。
    enum ErrOut {
        Load(String),
        NotFound,
        Upsert(String),
    }
    let t = match 'rmw: {
        let _rmw = API_RMW_LOCK.lock().unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] api_handlers::handlers API_RMW_LOCK: {e:?}");
            e.into_inner()
        });
        let tasks = match store.load() {
            Ok(v) => v,
            Err(e) => break 'rmw Err(ErrOut::Load(e)),
        };
        let Some(idx) = tasks.iter().position(|t| t.id == id) else {
            break 'rmw Err(ErrOut::NotFound);
        };
        let mut t = tasks[idx].clone();
        // RMW 基线 = 本次 load 快照的 updated_at；upsert 写前比对，基线外有写者改行 → 409 拒写。
        // updated_at 为 NULL 的老行用「行存在性」哨兵基线
        // （BASELINE_NULL_ROW：行被删/被改都 409）
        db::prepare_for_upsert(&mut t);

        // 应用已校验入参(纯赋值,不再校验)
        if let Some(title) = input.title.as_deref() {
            t.title = title.trim().to_string();
        }
        if input.note.is_some() {
            t.note = super::validate::take_trimmed_string(input.note.as_deref());
        }
        if let Some(new_status) = new_column {
            if new_status != t.column {
                let now = now_ms();
                if new_status == crate::db::TaskStatus::Done {
                    t.completed_at = Some(now);
                    t.archived = Some(false);
                } else if t.column == crate::db::TaskStatus::Done {
                    t.completed_at = None;
                    t.archived = None;
                }
                t.column = new_status;
            }
        }
        if let Some(fp) = input.file_path.as_deref() {
            // trim 后存储，首尾空白不进库
            let fp = fp.trim();
            if fp.is_empty() {
                t.file_path = None;
                t.file_is_dir = None;
                // 旧单绑定字段清空时同步清 files
                t.files = None;
            } else {
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
        if input.due.is_some() {
            t.due = super::validate::take_trimmed_string(input.due.as_deref());
        }
        if let Some(tags) = input.tags {
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
            break 'rmw Err(ErrOut::Upsert(e));
        }
        Ok(t)
    } {
        Ok(t) => t,
        Err(ErrOut::Load(e)) => {
            internal_err(req, log, &e);
            return;
        }
        Err(ErrOut::NotFound) => {
            let _ = req.respond(json_err(StatusCode(404), "task not found"));
            return;
        }
        Err(ErrOut::Upsert(e)) => {
            upsert_err(req, log, &e);
            return;
        }
    };
    after_change(store, &t, "updated", emit_fn, log);
    let _ = req.respond(json_ok(StatusCode(200), &TaskOut::from_task(&t)));
}

/// 校验 update_task 的入参。纯函数,不依赖任何 DB 状态,
/// 在持锁前跑(否则 400 应答期间仍持 API_RMW_LOCK 串行化全 API,
/// 是 DoS 放大面)。返回 Err(msg) 时调用方应回 400 + 该 msg。
fn validate_update_input(input: &UpdateReq) -> Result<(), String> {
    if let Some(title) = input.title.as_deref() {
        let tt = title.trim();
        // 显式传了 trim 后为空的 title 按 400 拒绝，与 create 对齐
        // （静默忽略会让调用方无法区分「没传」和「传了空白」）
        if tt.is_empty() {
            return Err("title 不能为空".to_string());
        }
        if let Some(e) = over_limit(tt, API_MAX_TITLE, "任务标题") {
            return Err(e);
        }
    }
    if let Some(e) = super::validate::check_field(input.note.as_deref(), API_MAX_NOTE, "备注") {
        return Err(e);
    }
    if let Some(s) = input.status.as_deref() {
        if !s.is_empty() && !valid_status(s) {
            return Err("status 必须是 todo/doing/done".to_string());
        }
    }
    if let Some(e) =
        super::validate::check_field(input.file_path.as_deref(), API_MAX_FILE_PATH, "文件路径")
    {
        return Err(e);
    }
    if let Some(e) = super::validate::check_field(input.due.as_deref(), API_MAX_DUE, "截止时间")
    {
        return Err(e);
    }
    if let Some(tags) = input.tags.as_ref() {
        if tags.len() > API_MAX_TAGS {
            return Err(format!("标签最多 {API_MAX_TAGS} 个"));
        }
        for tag in tags {
            if let Some(e) = over_limit(tag, API_MAX_TAG_LEN, "标签") {
                return Err(e);
            }
        }
    }
    Ok(())
}

/// 软删（进回收站）：幂等，已删的再次 DELETE 返回 200 不再变更
fn delete_task(
    req: Request,
    store: &Arc<dyn TaskStore>,
    id: &str,
    emit_fn: &Option<Arc<dyn Fn(&db::Task) + Send + Sync>>,
    log: &Option<PathBuf>,
) {
    // load→改→upsert 全程持 API_RMW_LOCK（API 写串行化）。
    // 锁内只做 DB 读写，错误/幂等重删经 labeled block 装盒、出锁后再响应
    // （OCR C5-AP-05）。
    enum ErrOut {
        Load(String),
        NotFound,
        Upsert(String),
        /// 幂等重删：已在回收站 → 出锁后 200 当前状态（不 after_change，不变）
        AlreadyDeleted(db::Task),
    }
    let t = match 'rmw: {
        let _rmw = API_RMW_LOCK.lock().unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] api_handlers::handlers API_RMW_LOCK: {e:?}");
            e.into_inner()
        });
        let tasks = match store.load() {
            Ok(v) => v,
            Err(e) => break 'rmw Err(ErrOut::Load(e)),
        };
        let Some(idx) = tasks.iter().position(|t| t.id == id) else {
            break 'rmw Err(ErrOut::NotFound);
        };
        let mut t = tasks[idx].clone();
        // RMW 基线 = 本次 load 快照的 updated_at（同 update_task）；
        // NULL 老行同样走行存在性哨兵基线
        db::prepare_for_upsert(&mut t);
        if t.deleted_at.is_some() {
            // 已在回收站：幂等返回当前状态
            break 'rmw Err(ErrOut::AlreadyDeleted(t));
        }
        t.deleted_at = Some(now_ms());
        t.updated_at = Some(now_ms());
        if let Err(e) = store.upsert(vec![t.clone()]) {
            break 'rmw Err(ErrOut::Upsert(e));
        }
        Ok(t)
    } {
        Ok(t) => t,
        Err(ErrOut::Load(e)) => {
            internal_err(req, log, &e);
            return;
        }
        Err(ErrOut::NotFound) => {
            let _ = req.respond(json_err(StatusCode(404), "task not found"));
            return;
        }
        Err(ErrOut::Upsert(e)) => {
            upsert_err(req, log, &e);
            return;
        }
        Err(ErrOut::AlreadyDeleted(t)) => {
            let _ = req.respond(json_ok(StatusCode(200), &TaskOut::from_task(&t)));
            return;
        }
    };
    after_change(store, &t, "deleted", emit_fn, log);
    let _ = req.respond(json_ok(StatusCode(200), &TaskOut::from_task(&t)));
}
