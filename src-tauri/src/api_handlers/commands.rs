//! 5 个 tauri 命令（api_start / api_stop / api_status / api_rotate_token） + 持锁辅助。
//!
//! 持锁实现（api_start_locked / api_stop_locked）供多个 cmd 复用，
//! 避免「检查→stop→start」之间的抢锁窗口（api_rotate_token 场景）。
//!
//! 锁边界约定:state.0 锁只覆盖内存状态变更(is_some 检查 / start_api 端口绑定 /
//! join 线程 / Option 更新);文件 I/O(token load / flag 读写)一律在锁释放后做。
//! 这避免锁跨越慢操作(文件 I/O / SSE 网络)放大串行化面。
//!
//! api_stop_for_exit 不是命令——是 lib.rs 退出路径专用(保留 runtime/flags/api-enabled.flag,
//! 下次启动按 flag 自动恢复)。通过 mod.rs 顶层 `pub use` 透传给 lib.rs。

use tauri::{AppHandle, Emitter};

use crate::api::{TaskStore, TauriStore, API_PORT};
use crate::api_auth::{
    clear_enabled_flag, load_or_create_token, write_enabled_flag, write_token_file,
};
use crate::api_server::{start_api, ApiState, EventHub, RunningApi};
use crate::audit::AuditLevel;
use crate::audit_event;
use crate::db;
use crate::error::{CommandError, CommandResult};

use super::ratelimit::log_line;
use super::sse::{stop_sse_writers, API_HUB_KEY, SSE_STOP_JOIN_TIMEOUT};
use super::types::{ApiInfo, ApiStatus};

// ───────────────────────── api_start ─────────────────────────

#[tauri::command]
pub fn api_start(app: AppHandle, state: tauri::State<'_, ApiState>) -> CommandResult<ApiInfo> {
    // Phase 1: token 文件 I/O 在锁外(load_or_create_token 会 read/write token 文件)
    let token = load_or_create_token(&app)?;
    // 锁 poisoning 审计:同 ab74025 惯例
    let mut g = state.0.lock().map_err(|e| {
        eprintln!("[mutex_poisoned] api_handlers::commands::state.0: {e:?}");
        e.to_string()
    })?;
    let info = api_start_locked(&app, token, &mut g)?;
    // Phase 3: enabled flag 文件写入在锁外(避免与 start_api 的端口绑定/spawn 串行)
    drop(g);
    write_enabled_flag(&app);
    Ok(info)
}

/// api_start 的持锁实现:供 api_start / api_rotate_token
/// 复用,调用方必须已持 `state.0` 锁(rotate 全程持锁,检查与操作原子)。
///
/// token 必须由调用方在锁外加载(load_or_create_token)。本函数不再做文件 I/O。
/// enabled flag 写入也由调用方在锁外做(write_enabled_flag)。
fn api_start_locked(
    app: &AppHandle,
    token: String,
    g: &mut Option<RunningApi>,
) -> CommandResult<ApiInfo> {
    // 尸体 join 最坏 ~400ms(accept recv_timeout tick),
    // accept 线程不等 worker,持锁清理安全
    if let Some(running) = g.as_ref() {
        // 与 api_status 同款活性检查——accept 线程已死
        // (recv_error 退出)时不得误报成功;清尸体后继续走下面的重启
        let alive = running
            .handle
            .as_ref()
            .map(|h| !h.is_finished())
            .unwrap_or(false);
        if alive {
            return Ok(ApiInfo {
                port: API_PORT,
                token: Some(token),
            });
        }
    }
    if let Some(mut r) = g.take() {
        r.shutdown.store(true, Ordering::SeqCst);
        if let Some(h) = r.handle.take() {
            let _ = h.join();
        }
    }
    let store: Arc<dyn TaskStore> = Arc::new(TauriStore {
        app: app.clone(),
        // id 持久化,跨重启保持单调(否则客户端 Last-Event-ID 去重会静默丢事件)
        hub: EventHub::persisted(db::data_dir(app).join("api-event-id.txt")),
    });
    let emit_app = app.clone();
    let emit: Option<Box<dyn Fn(&db::Task) + Send + Sync>> = Some(Box::new(
        move |task: &db::Task| {
            // 复用挂件→主窗口的既有通道:主窗口合并状态并广播给挂件。
            // `source: Api` 告诉主窗口:数据已由 API 线程落盘,只合并 UI 状态,不要回写
            // (回写会用旧事件快照覆盖 API 的新写入,导致归档/软删被回滚的竞态)
            let payload = serde_json::json!({ "upserts": [task], "deletes": [], "source": crate::mutation::MutationOrigin::Api.as_str() });
            let _ = emit_app.emit_to("main", "tasks-updated", &payload);
        },
    ));
    let log_path = Some(db::data_dir(app).join("api.log"));
    let audit_app = app.clone();
    let on_error: Option<Box<dyn Fn(AuditLevel, &str, &str) + Send + Sync>> =
        Some(Box::new(move |lvl, ev, msg| {
            audit_event!(&audit_app, lvl, ev, "error" => msg);
        }));
    // 提前取 hub 身份键（store 随后被 move 进 start_api），
    // api_stop 据此通知并 join 该 hub 的 SSE writer
    let hub_key = store.event_hub().hub_id();
    // 显式映射 HttpStartFailed——String 错误经
    // From<String> 落成无结构的 Internal,前端按 code 分支永远等不到 HTTP_START_FAILED
    let running =
        start_api(API_PORT, token.clone(), store, emit, log_path, on_error).map_err(|e| {
            CommandError::HttpStartFailed {
                port: API_PORT,
                reason: e,
            }
        })?;
    API_HUB_KEY.store(hub_key, Ordering::SeqCst);
    *g = Some(running);
    // write_enabled_flag 移到 caller(锁外)
    Ok(ApiInfo {
        port: API_PORT,
        token: Some(token),
    })
}

// ───────────────────────── api_stop / api_stop_for_exit ─────────────────────────

#[tauri::command]
pub fn api_stop(app: AppHandle, state: tauri::State<'_, ApiState>) -> CommandResult<()> {
    // 用户显式关闭:清 api-enabled.flag,否则下次启动会按 flag 自动恢复
    api_stop_impl(&app, &state, true)
}

/// 应用退出路径(ExitRequested)的 API 停止——与 api_stop 同一清理
/// (accept 线程 + SSE writer 全部通知并 join),但保留 runtime/flags/api-enabled.flag:
/// 退出不是用户关开关,下次启动应按 flag 自动恢复服务。
/// 泛型 Runtime:cleanup_on_exit 的 mock runtime 测试可直调。
pub fn api_stop_for_exit<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &ApiState,
) -> CommandResult<()> {
    // 退出不是用户关开关:保留 flag 供下次启动恢复
    api_stop_impl(app, state, false)
}

/// `clear_enabled` 由调用方语义决定:`api_stop`(用户显式关闭)→ true,
/// `api_stop_for_exit`(应用退出,下次启动按 flag 恢复)→ false。
/// flag 文件 I/O 在锁释放后做(遵循本模块「文件 I/O 不持锁」约定)。
fn api_stop_impl<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &ApiState,
    clear_enabled: bool,
) -> CommandResult<()> {
    // 锁 poisoning 审计:同 ab74025 惯例
    let mut g = state.0.lock().map_err(|e| {
        eprintln!("[mutex_poisoned] api_handlers::commands::state.0: {e:?}");
        e.to_string()
    })?;
    api_stop_locked(app, &mut g)?;
    drop(g);
    if clear_enabled {
        clear_enabled_flag(app);
    }
    Ok(())
}

/// api_stop_impl 的持锁实现:供 api_rotate_token
/// 在全程持 `state.0` 锁前提下复用,消除「检查→stop→start」之间的抢锁窗口。
///
/// 不做 enabled flag 清理——由调用方在锁释放后决定
/// (api_stop 清;api_stop_for_exit 保留给下次启动;api_rotate_token 走 start 路径覆盖)。
fn api_stop_locked<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    g: &mut Option<RunningApi>,
) -> CommandResult<()> {
    if let Some(mut r) = g.take() {
        r.shutdown.store(true, Ordering::SeqCst);
        if let Some(h) = r.handle.take() {
            let _ = h.join();
        }
        // 除 join accept 线程外,通知并 join 当前 hub 的全部 SSE writer——
        // writer 不追踪的话旧 hub 的 tx 永不 drop,writer 循环发 keepalive,
        // 旧客户端僵尸挂连且线程随 rotate 无界泄漏;5s 仍不退出的 detach + ERROR 审计
        let key = API_HUB_KEY.load(Ordering::SeqCst);
        let audit_app = app.clone();
        stop_sse_writers(key, SSE_STOP_JOIN_TIMEOUT, &mut |line: &str| {
            audit_event!(&audit_app, AuditLevel::Error, "sse_writer_leaked", "error" => line);
        });
    }
    Ok(())
}

// ───────────────────────── api_status ─────────────────────────

#[tauri::command]
pub fn api_status(app: AppHandle, state: tauri::State<'_, ApiState>) -> CommandResult<ApiStatus> {
    // 锁 poisoning 审计:这里 wrap 成 CommandError::Internal,
    // 但 eprintln! 一行标记 poisoning 事件,日志聚合能据此告警
    let mut g = state
        .0
        .lock()
        .map_err(|e| CommandError::Internal(format!("API 状态锁失败:{e}")))?;
    // 活性检查——服务线程可能已因 recv_error 退出(panic 已被 catch_unwind 覆盖),
    // 仅看 Option::is_some 会把死服务报成"已开启"
    let enabled = g
        .as_ref()
        .and_then(|r| r.handle.as_ref())
        .map(|h| !h.is_finished())
        .unwrap_or(false);
    // 锁内:清理尸体 + 记录是否需要清 enabled flag。
    // enabled flag 文件写入移出锁外(持锁做会被其他 API 命令串行化)
    let needs_clear = if !enabled && g.is_some() {
        *g = None;
        true
    } else {
        false
    };
    drop(g);
    if needs_clear {
        clear_enabled_flag(&app);
    }
    // 未启用时不读/生成 token——否则每次查状态都
    // load_or_create_token,从未开启过 API 的用户数据目录里也会落 runtime/flags/api-token.txt。
    // 前端只在 enabled 时展示 token(SettingsPage),disabled 态字段直接缺席
    // (ApiStatus 用 skip_serializing_if = "Option::is_none" 剔除,见 types.rs)
    let token = if enabled {
        Some(load_or_create_token(&app)?)
    } else {
        None
    };
    Ok(ApiStatus {
        enabled,
        port: API_PORT,
        token,
    })
}

// ───────────────────────── api_rotate_token ─────────────────────────

/// 重新生成 Bearer token:写新 token 文件;若服务运行中则重启生效
#[tauri::command]
pub fn api_rotate_token(
    app: AppHandle,
    state: tauri::State<'_, ApiState>,
) -> CommandResult<ApiInfo> {
    // Phase 1: 文件 I/O 全部在锁外。
    // rotate 全程持锁的意图是「is_some 检查 → stop → start」原子;
    // 文件 I/O(read old / write new / rollback)与该原子性无关,不应持锁做。
    let dir = crate::paths::flags_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("api-token.txt");
    let old = std::fs::read_to_string(&path).ok();
    let token = uuid::Uuid::new_v4().simple().to_string();
    // 投机写新 token 文件。后续 start_locked 会从文件读这个新 token。
    // 若 start 失败,Phase 3 会回滚到 old token 文件。
    write_token_file(&path, &token)?;

    // Phase 2: 锁内做 stop+start(若服务运行中),保持「检查→操作」原子。
    enum RotateOutcome {
        NotRunning(ApiInfo),
        Started(ApiInfo),
    }
    let outcome: Result<RotateOutcome, CommandError> = (|| {
        // 锁 poisoning 审计:同 ab74025 惯例
        let mut g = state.0.lock().map_err(|e| {
            eprintln!("[mutex_poisoned] api_handlers::commands::state.0: {e:?}");
            e.to_string()
        })?;
        if g.is_none() {
            return Ok(RotateOutcome::NotRunning(ApiInfo {
                port: API_PORT,
                token: Some(token.clone()),
            }));
        }
        // 运行中:先 stop 再 start。
        // stop 不清 enabled flag(start 会写);start 不写 enabled flag(由 Phase 3 补写)
        api_stop_locked(&app, &mut g)?;
        api_start_locked(&app, token.clone(), &mut g)
            .map(|info| RotateOutcome::Started(info))
    })();

    // Phase 3: 锁释放后再做文件 I/O。
    match outcome {
        Ok(RotateOutcome::NotRunning(info)) => {
            // 未运行:只更新 token 文件,不动 enabled flag(flag 本来就未设)
            Ok(info)
        }
        Ok(RotateOutcome::Started(info)) => {
            // 运行中重启:start_locked 没写 flag,此处补写
            write_enabled_flag(&app);
            Ok(info)
        }
        Err(e) => {
            // 回滚 token 文件(锁外)
            if let Some(old) = &old {
                let _ = write_token_file(&path, old);
            }
            // 尽力用旧 token 恢复服务(再次抢锁,best-effort)。
            // Phase 2 失败→此处抢锁之间存在 TOCTOU 窗口:并发的 api_start /
            // api_rotate_token 可能已插入。因此:
            //   ① 锁内若已有存活服务 → 并发 api_start 已接手,不得用 old_token
            //      短路/覆盖(否则返回的 token 与活服务绑定的 token 不一致),
            //      直接放弃恢复并原样抛错;
            //   ② 否则在锁内重读磁盘 token 作为权威值(并发 rotate/stop 可能已改写),
            //      不再用捕获的 old_token,避免内存服务与磁盘 token 不一致。
            if old.is_some() {
                match state.0.lock() {
                    Ok(mut g) => {
                        let alive = g
                            .as_ref()
                            .and_then(|r| r.handle.as_ref())
                            .map(|h| !h.is_finished())
                            .unwrap_or(false);
                        if alive {
                            return Err(e);
                        }
                        let authoritative = std::fs::read_to_string(&path)
                            .ok()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty());
                        if let Some(tok) = authoritative {
                            if api_start_locked(&app, tok, &mut g).is_ok() {
                                write_enabled_flag(&app);
                            }
                        }
                    }
                    Err(ee) => {
                        eprintln!("[mutex_poisoned] api_handlers::commands::state.0: {ee:?}");
                    }
                }
            }
            Err(e)
        }
    }
}

// ───────────────────────── use 别名 ─────────────────────────

use std::sync::atomic::Ordering;
use std::sync::Arc;
