//! 5 个 tauri 命令（api_start / api_stop / api_status / api_rotate_token） + 持锁辅助。
//!
//! 持锁实现（api_start_locked / api_stop_locked / api_stop_impl）供多个 cmd 复用，
//! 避免「检查→stop→start」之间的抢锁窗口（api_rotate_token 场景）。
//!
//! api_stop_for_exit 不是命令——是 lib.rs 退出路径专用（保留 runtime/flags/api-enabled.flag，
//! 下次启动按 flag 自动恢复）。通过 mod.rs 顶层 `pub use` 透传给 lib.rs。

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
    // 检查与写入在同一把锁内完成——锁释放后才 start 会让并发 invoke 双发都过检查，
    // 第二个收到误导的「端口占用」（服务其实已被第一个起好）
    let mut g = state.0.lock().map_err(|e| e.to_string())?;
    api_start_locked(&app, &mut g)
}

/// api_start 的持锁实现：供 api_start / api_rotate_token
/// 复用，调用方必须已持 `state.0` 锁（rotate 全程持锁，检查与操作原子）。
fn api_start_locked(app: &AppHandle, g: &mut Option<RunningApi>) -> CommandResult<ApiInfo> {
    // 尸体 join 最坏 ~400ms（accept recv_timeout tick），
    // accept 线程不等 worker，持锁清理安全
    if let Some(running) = g.as_ref() {
        // 与 api_status 同款活性检查——accept 线程已死
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
        // id 持久化，跨重启保持单调（否则客户端 Last-Event-ID 去重会静默丢事件）
        hub: EventHub::persisted(db::data_dir(app).join("api-event-id.txt")),
    });
    let emit_app = app.clone();
    let emit: Option<Box<dyn Fn(&db::Task) + Send + Sync>> = Some(Box::new(
        move |task: &db::Task| {
            // 复用挂件→主窗口的既有通道：主窗口合并状态并广播给挂件。
            // `source: Api` 告诉主窗口：数据已由 API 线程落盘，只合并 UI 状态，不要回写
            // （回写会用旧事件快照覆盖 API 的新写入，导致归档/软删被回滚的竞态）
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
    // 提前取 hub 分组键（store 随后被 move 进 start_api），
    // api_stop 据此通知并 join 该 hub 的 SSE writer
    let hub_key = Arc::as_ptr(store.event_hub()) as usize;
    // 显式映射 HttpStartFailed——String 错误经
    // From<String> 落成无结构的 Internal，前端按 code 分支永远等不到 HTTP_START_FAILED
    let running =
        start_api(API_PORT, token.clone(), store, emit, log_path, on_error).map_err(|e| {
            CommandError::HttpStartFailed {
                port: API_PORT,
                reason: e,
            }
        })?;
    API_HUB_KEY.store(hub_key, Ordering::SeqCst);
    *g = Some(running);
    write_enabled_flag(app);
    Ok(ApiInfo {
        port: API_PORT,
        token,
    })
}

// ───────────────────────── api_stop / api_stop_for_exit ─────────────────────────

#[tauri::command]
pub fn api_stop(app: AppHandle, state: tauri::State<'_, ApiState>) -> CommandResult<()> {
    api_stop_impl(&app, &state, true)
}

/// 应用退出路径（ExitRequested）的 API 停止——与 api_stop 同一清理
///（accept 线程 + SSE writer 全部通知并 join），但保留 runtime/flags/api-enabled.flag：
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

/// api_stop_impl 的持锁实现：供 api_rotate_token
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
        // 除 join accept 线程外，通知并 join 当前 hub 的全部 SSE writer——
        // writer 不追踪的话旧 hub 的 tx 永不 drop，writer 循环发 keepalive，
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

// ───────────────────────── api_status ─────────────────────────

#[tauri::command]
pub fn api_status(app: AppHandle, state: tauri::State<'_, ApiState>) -> CommandResult<ApiStatus> {
    let mut g = state
        .0
        .lock()
        .map_err(|e| CommandError::Internal(format!("API 状态锁失败：{e}")))?;
    // 活性检查——服务线程可能已因 recv_error 退出（panic 已被 catch_unwind 覆盖），
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
    // 未启用时不读/生成 token——否则每次查状态都
    // load_or_create_token，从未开启过 API 的用户数据目录里也会落 runtime/flags/api-token.txt。
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

// ───────────────────────── api_rotate_token ─────────────────────────

/// 重新生成 Bearer token：写新 token 文件；若服务运行中则重启生效
#[tauri::command]
pub fn api_rotate_token(
    app: AppHandle,
    state: tauri::State<'_, ApiState>,
) -> CommandResult<ApiInfo> {
    let dir = crate::paths::flags_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("api-token.txt");
    let old = std::fs::read_to_string(&path).ok();
    let token = uuid::Uuid::new_v4().simple().to_string();
    // rotate 全程持 state.0 锁——若「查 is_some → 放锁 →
    // api_stop/api_start 各自再抢锁」，窗口内用户并发 stop 会被 rotate 把服务重新拉起
    // （违背用户关闭意图）。锁内只做端口绑定/join 等毫秒级操作，无死锁风险
    // （locked 变体不再抢同一把锁）。
    let mut g = state.0.lock().map_err(|e| e.to_string())?;
    let was_running = g.is_some();
    if !was_running {
        write_token_file(&path, &token)?;
        return Ok(ApiInfo {
            port: API_PORT,
            token,
        });
    }
    // 运行中：先落新 token（api_start 从文件读取），再重启生效。
    // 重启失败则回滚旧 token 并尽力恢复服务，
    // 避免"服务已停 + flag 已清 + token 已换"三态不一致
    write_token_file(&path, &token)?;
    // api_stop 内会停掉旧 hub 的全部 SSE writer，旧 token 的连接随之断开，
    // token 失效语义彻底；api_start 重建新 hub 接受新 writer。
    // stop 失败时回滚旧 token 文件——否则留下「文件已是新 token、
    // 在跑服务仍认旧 token」的三态不一致
    if let Err(e) = api_stop_locked(&app, &mut g, true) {
        if let Some(old) = &old {
            let _ = write_token_file(&path, old);
        }
        return Err(e);
    }
    match api_start_locked(&app, &mut g) {
        Ok(info) => Ok(info),
        Err(e) => {
            if let Some(old) = old {
                let _ = write_token_file(&path, &old);
            }
            let _ = api_start_locked(&app, &mut g); // 尽力用旧 token 恢复服务
            Err(e)
        }
    }
}

// ───────────────────────── use 别名 ─────────────────────────

use std::sync::atomic::Ordering;
use std::sync::Arc;
