//! apps 模块 Tauri commands：list/run/create/archive/恢复 + 运行历史/版本回滚
//!
//! 入参字段名（camelCase）即 manifest 的字段名（name/description/icon/...）；
//! manifest 字段约束是 AI 上架工具的合同（设计稿 `docs/APPS-DESIGN-2026-10-15.md`
//! §2 与 M1 代码规范文档的 `docs/APP-DEV-CONVENTIONS.md`）。
//! 修改/删除元数据的 bot 路径走 manage 核（apps_update_files），页面路径
//! 只保留用户主动的归档/恢复/回滚——命令层不再有全量 update 口。

use std::path::PathBuf;

use serde::Deserialize;
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

use crate::apps::commands::manifest_contract as mc;
use crate::apps::{self as apps_runner, AppRunResult};
use crate::db::apps as db_apps;
use crate::db::apps::{App, AppRun};
use crate::error::{CommandError, CommandResult};

/// 程序页 M2 全量视图（草稿 + 已启用，按 updated_at 倒序）：
/// 草稿卡带「草稿」徽章可试跑，归档不出现在程序页
#[tauri::command]
pub async fn apps_list_active(app: AppHandle) -> CommandResult<Vec<App>> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::db::open_db(&app).map_err(CommandError::DbError)?;
        db_apps::list_active_apps(&conn).map_err(CommandError::DbError)
    })
    .await
    .map_err(|e| CommandError::from(format!("程序列表线程 join 失败：{e}")))?
}

/// 跑一个 app。表单模式把 `params` 传 JSON 串；文件夹模式传 None
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunArgs {
    pub id: String,
    pub params: Option<String>,
    pub timeout_secs: Option<u64>,
}

#[tauri::command]
pub async fn apps_run(app: AppHandle, args: RunArgs) -> CommandResult<AppRunResult> {
    let a = {
        let app2 = app.clone();
        let id = args.id.clone();
        tauri::async_runtime::spawn_blocking(move || {
            db_apps::get_app(&open_db_or(&app2)?, &id)
                .map_err(CommandError::DbError)?
                .ok_or_else(|| CommandError::TaskNotFound(id.clone()))
        })
        .await
        .map_err(|e| CommandError::from(format!("程序读取线程 join 失败：{e}")))?
    }?;
    // M2 试跑闭环：草稿可运行（AI 创建后用户先试跑再上架）；归档不可运行
    if a.status == mc::STATUS_ARCHIVED {
        return Err(CommandError::InvalidArgument {
            field: "id".into(),
            value: a.status.clone(),
            reason: "已归档程序不可运行".into(),
        });
    }
    // Python 执行面统一闸：「允许机器人执行 Python」关着且非 yolo 时，程序页
    // 同样不能跑——同一执行面同一口径，关掉开关不能只挡工具不挡程序
    crate::py::commands::ensure_py_enabled(&app).map_err(|reason| {
        CommandError::InvalidArgument {
            field: "pythonEnabled".into(),
            value: "false".into(),
            reason,
        }
    })?;
    // 超时来源优先级：调用方显式传入 > 程序声明的运行时长 > 内置默认。
    // 程序声明是**这个程序自己的属性**（登记时就知道它要跑多久），
    // 全局墙钟不该拿小程序的尺子量批量程序。resolve_run_timeout 统一把
    // 结果钳到 apps 天花板（1800s）——谁给的数都越不过那道顶。
    let timeout = resolve_run_timeout(args.timeout_secs, a.timeout_secs);
    // 执行内部阻塞等子进程至多 timeout 秒，必须挪进阻塞线程池（钉死 async
    // worker 会拖垮其余命令）；不持全局写锁——长跑的子进程不能挡住别的写者，
    // 内部两次短写靠 busy_timeout 兜底。
    // 停止接线：每次运行起一个后台 StopGuard（非交互、无会话归属），令牌
    // 传进阻塞运行——/stop 全停与退出清理都能中断在途程序（最长 1800s
    // 不能停就是失控）；guard 活到运行结束，Drop 自动注销。
    let app_name = a.name.clone();
    let res = {
        let app2 = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let guard = crate::bot::StopGuard::new(&app2, false, None);
            let token = guard.token();
            let r = apps_runner::run(
                &a,
                &a.script_path,
                args.params.as_deref(),
                Some(timeout),
                &app2,
                Some(&token),
            );
            drop(guard);
            r
        })
        .await
        .map_err(|e| CommandError::from(format!("程序运行线程 join 失败：{e}")))?
        .map_err(runner_err)?
    };
    // 系统通知：跑完/失败给个轻提示（M1 不做结果面板里的丰富展示）
    let title = format!(
        "程序「{}」{}",
        app_name,
        match res.exit_code {
            Some(0) => "运行完成",
            _ => "运行失败",
        }
    );
    let _ = app
        .notification()
        .builder()
        .title(title)
        .body(res.summary.clone().unwrap_or_default())
        .show();
    Ok(res)
}

/// 上架（手工路径；M2 起 AI 走 bot 工具 apps_create 落草稿，不走本命令）。
/// 脚本文件必须已落在 `{data_dir}/apps/<id>/` 下，命令只登记元数据 + 检查脚本可读
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateArgs {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub input_schema: String,
    /// 相对 app_dir 的脚本路径（M1 恒 `app.py`）
    pub script_path: String,
    pub manual: String,
    /// draft / enabled；M1 推荐传 enabled
    pub status: String,
    /// 声明的单次运行时长（秒）：None = 未声明（运行时用内置默认）
    pub timeout_secs: Option<u64>,
}

#[tauri::command]
pub async fn apps_create(app: AppHandle, args: CreateArgs) -> CommandResult<App> {
    let a = build_app_from_create(&app, &args)?;
    apps_runner::preflight_script(&app, &a, &a.script_path).map_err(runner_err)?;
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> CommandResult<App> {
        // 与 bot 工具同款写闸：页面手工上架与 AI 改程序并发时串行化，
        // 防 SQLITE_BUSY 与版本号竞态（db::DB_WRITE_LOCK 的约定）
        let _g = crate::db::lock_db_write();
        let conn = crate::db::open_db(&app2).map_err(CommandError::DbError)?;
        // upsert 语义会静默覆盖同 id 程序，重名必须先拒（与 bot 核同规）
        crate::apps::manage::ensure_app_absent(&conn, &a.id)?;
        let dir = apps_runner::app_dir(&app2, &a.id);
        // 落库 + 版本快照同一事务（口径同 manage 核）
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| CommandError::DbError(e.to_string()))?;
        db_apps::upsert_app(&tx, &a.clone()).map_err(CommandError::DbError)?;
        crate::apps::manage::record_app_version(&tx, &dir, &a, "create", crate::db::now_ms())?;
        tx.commit()
            .map_err(|e| CommandError::DbError(e.to_string()))?;
        Ok(a)
    })
    .await
    .map_err(|e| CommandError::from(format!("程序上架线程 join 失败：{e}")))?
}

/// 在系统文件管理器中打开程序的 output/ 目录（不存在则先创建）。
/// 返回打开的目录路径。路径由 id 派生（id 已是路径分量校验过的），
/// 用户主动点击触发，无 LLM 参与——不走 bot 工具的白名单闸。
#[tauri::command]
pub async fn apps_open_output(app: AppHandle, id: String) -> CommandResult<String> {
    {
        let app2 = app.clone();
        let id2 = id.clone();
        let found = tauri::async_runtime::spawn_blocking(move || {
            db_apps::get_app(&open_db_or(&app2)?, &id2)
                .map_err(CommandError::DbError)?
                .ok_or_else(|| CommandError::TaskNotFound(id2.clone()))
        })
        .await
        .map_err(|e| CommandError::from(format!("程序读取线程 join 失败：{e}")))?;
        let _ = found?;
    }
    let dir = apps_runner::app_dir(&app, &id).join("output");
    std::fs::create_dir_all(&dir).map_err(|e| CommandError::DbError(e.to_string()))?;
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .reveal_item_in_dir(&dir)
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    Ok(dir.to_string_lossy().to_string())
}

/// 已归档程序列表（程序页「已归档」视图 + 恢复入口）
#[tauri::command]
pub async fn apps_list_archived(app: AppHandle) -> CommandResult<Vec<App>> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::db::open_db(&app).map_err(CommandError::DbError)?;
        db_apps::list_archived_apps(&conn).map_err(CommandError::DbError)
    })
    .await
    .map_err(|e| CommandError::from(format!("归档列表线程 join 失败：{e}")))?
}

/// 归档（程序页用户动作；脚本/数据/版本/运行历史原地保留，可恢复）
#[tauri::command]
pub async fn apps_archive(app: AppHandle, id: String) -> CommandResult<App> {
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _g = crate::db::lock_db_write();
        let conn = crate::db::open_db(&app2).map_err(CommandError::DbError)?;
        crate::apps::manage::archive_core(&conn, &id, crate::db::now_ms())
    })
    .await
    .map_err(|e| CommandError::from(format!("程序归档线程 join 失败：{e}")))?
}

/// 恢复归档程序 → 草稿态（回到程序页可试跑，重新上架由用户满意后再定）
#[tauri::command]
pub async fn apps_unarchive(app: AppHandle, id: String) -> CommandResult<App> {
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _g = crate::db::lock_db_write();
        let conn = crate::db::open_db(&app2).map_err(CommandError::DbError)?;
        crate::apps::manage::unarchive_core(&conn, &id, crate::db::now_ms())
    })
    .await
    .map_err(|e| CommandError::from(format!("程序恢复线程 join 失败：{e}")))?
}

/// 彻底删除归档程序：库内三张表的行 + 程序目录全部抹掉，不可逆。
/// 只受理已归档程序——归档视图里那个删除键是唯一入口。
#[tauri::command]
pub async fn apps_delete(app: AppHandle, id: String) -> CommandResult<()> {
    let app2 = app.clone();
    let id2 = id.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _g = crate::db::lock_db_write();
        let conn = crate::db::open_db(&app2).map_err(CommandError::DbError)?;
        let dir = apps_runner::app_dir(&app2, &id2);
        crate::apps::manage::delete_core(&dir, &conn, &id2)
    })
    .await
    .map_err(|e| CommandError::from(format!("程序删除线程 join 失败：{e}")))?
}

/// 最近运行历史（程序页 + 管理面板）
#[tauri::command]
pub async fn apps_runs_list(
    app: AppHandle,
    app_id: String,
    limit: Option<i64>,
) -> CommandResult<Vec<AppRun>> {
    tauri::async_runtime::spawn_blocking(move || {
        apps_runner::recent_runs(&app, &app_id, limit.unwrap_or(20)).map_err(CommandError::DbError)
    })
    .await
    .map_err(|e| CommandError::from(format!("运行历史线程 join 失败：{e}")))?
}

/// 版本快照列表（程序页「版本」页签；新→旧，不含脚本全文防 IPC 大载荷）
#[tauri::command]
pub async fn apps_versions_list(
    app: AppHandle,
    app_id: String,
    limit: Option<i64>,
) -> CommandResult<Vec<db_apps::AppVersionMeta>> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = open_db_or(&app)?;
        db_apps::list_app_version_metas(&conn, &app_id, limit.unwrap_or(100))
            .map_err(CommandError::DbError)
    })
    .await
    .map_err(|e| CommandError::from(format!("版本列表线程 join 失败：{e}")))?
}

/// 回滚到历史版本（程序页用户动作）：内容整体回旧版、生命周期状态保留，
/// 回滚本身记为新版本（可再回滚撤销）。无 LLM 参与，不走 bot 工具白名单闸
#[tauri::command]
pub async fn apps_version_restore(
    app: AppHandle,
    app_id: String,
    version: i64,
) -> CommandResult<App> {
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> CommandResult<App> {
        // fs 重写 + 事务落库，与其他写口同闸串行
        let _g = crate::db::lock_db_write();
        let conn = crate::db::open_db(&app2).map_err(CommandError::DbError)?;
        let dir = apps_runner::app_dir(&app2, &app_id);
        crate::apps::manage::restore_version_core(
            &dir,
            &conn,
            &app_id,
            version,
            crate::db::now_ms(),
        )
    })
    .await
    .map_err(|e| CommandError::from(format!("程序回滚线程 join 失败：{e}")))?
}

// ─── helpers ─────────────────────────────────────────────────────────

/// 单次运行的时长上限（纯函数，单测直打）：
///
/// 来源优先级 = 调用方显式传入 > 程序声明的运行时长 > 内置默认。
/// 天花板是 `APPS_MAX_TIMEOUT_SECS`（1800s，用户直跑的长批量活）——不复用
/// run_python 工具路径的 600s 上限：程序页没有子 agent 停滞判定，窗可以更长。
pub fn resolve_run_timeout(caller_secs: Option<u64>, declared_secs: Option<u64>) -> u64 {
    caller_secs
        .or(declared_secs)
        .unwrap_or(crate::py::runtime::DEFAULT_TIMEOUT_SECS)
        .min(crate::py::runtime::APPS_MAX_TIMEOUT_SECS)
}

fn open_db_or(app: &AppHandle) -> Result<rusqlite::Connection, CommandError> {
    crate::db::open_db(app).map_err(CommandError::DbError)
}

/// runner/预检的 String 错误 → 领域错误：此前全装进 DbError，前端按
/// 「数据库错误」归类展示，与真实成因（脚本缺失/启动失败）不符
fn runner_err(e: String) -> CommandError {
    CommandError::DomainRule {
        domain: "apps".into(),
        reason: e,
    }
}

fn build_app_from_create(app: &AppHandle, args: &CreateArgs) -> Result<App, CommandError> {
    // 状态只收上架二态：任意字符串入库会让程序页状态判断与 apps_run 的
    // 归档判定全部失效；「归档」不是创建态，走 apps_archive
    if !matches!(args.status.as_str(), mc::STATUS_DRAFT | mc::STATUS_ENABLED) {
        return Err(CommandError::InvalidArgument {
            field: "status".into(),
            value: args.status.clone(),
            reason: "status 只接受 draft / enabled".into(),
        });
    }
    // 版本快照按 app.py 落盘读原文、bot 工具也恒写 app.py——放野值会让
    // 快照记错脚本、回滚写回错误内容，命令层与全链路对齐
    if args.script_path != "app.py" {
        return Err(CommandError::InvalidArgument {
            field: "scriptPath".into(),
            value: args.script_path.clone(),
            reason: "脚本路径恒为 app.py（相对 app 目录）".into(),
        });
    }
    let now = crate::db::now_ms();
    let enabled_at = if args.status == "enabled" {
        Some(now)
    } else {
        None
    };
    // M2 起 manifest 契约统一强制（id 分量 / 字段长度 / inputSchema 合法性），
    // 手工上架与 AI 上架同规（`docs/APP-DEV-CONVENTIONS.md` §2）
    crate::apps::manage::validate_app_id(&args.id)?;
    crate::apps::manage::validate_manifest_meta(
        &args.name,
        &args.description,
        &args.icon,
        &args.manual,
    )?;
    crate::apps::manage::validate_input_schema(&args.input_schema).map_err(|reason| {
        CommandError::InvalidArgument {
            field: "inputSchema".into(),
            value: String::new(),
            reason,
        }
    })?;
    // 顺带建 app 目录（让上架=可试跑原子化）
    let dir: PathBuf = apps_runner::app_dir(app, &args.id);
    std::fs::create_dir_all(&dir).map_err(|e| CommandError::DbError(e.to_string()))?;
    Ok(App {
        id: args.id.clone(),
        name: args.name.clone(),
        description: args.description.clone(),
        icon: args.icon.clone(),
        input_schema: args.input_schema.clone(),
        script_path: args.script_path.clone(),
        manual: args.manual.clone(),
        status: args.status.clone(),
        enabled_at,
        created_at: now,
        updated_at: now,
        timeout_secs: crate::apps::manage::normalize_declared_timeout(args.timeout_secs),
    })
}

/// 文档化：AI 上架工具时 inputs/scripts 形状必须满足
/// - `name` / `description` ≤ 200 / 500 字
/// - `icon` ≤ 4 字符（emoji）
/// - `input_schema` 是合法 JSON（文件夹 or 表单）
/// - `script_path` 相对 `apps/<id>/`，M1 恒为 `app.py`
/// - `status`: draft / enabled / archived
pub mod manifest_contract {
    pub const NAME_MAX: usize = 200;
    pub const DESCRIPTION_MAX: usize = 500;
    pub const ICON_MAX: usize = 8; // 留余量兼容复杂 emoji
    pub const MANUAL_MAX: usize = 8000;
    pub const STATUS_DRAFT: &str = "draft";
    pub const STATUS_ENABLED: &str = "enabled";
    pub const STATUS_ARCHIVED: &str = "archived";
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::py::runtime::{APPS_MAX_TIMEOUT_SECS, DEFAULT_TIMEOUT_SECS};

    /// 运行超时的来源优先级：调用方 > 程序声明 > 内置默认；越不过天花板
    #[test]
    fn run_timeout_prefers_caller_then_declaration_then_default() {
        assert_eq!(resolve_run_timeout(None, None), DEFAULT_TIMEOUT_SECS);
        assert_eq!(resolve_run_timeout(None, Some(1800)), 1800, "声明值生效");
        assert_eq!(
            resolve_run_timeout(Some(120), Some(1800)),
            120,
            "调用方显式值盖过声明"
        );
        assert_eq!(
            resolve_run_timeout(Some(9999), None),
            APPS_MAX_TIMEOUT_SECS,
            "超上限的调用方值钳到天花板"
        );
        assert_eq!(
            resolve_run_timeout(None, Some(9999)),
            APPS_MAX_TIMEOUT_SECS,
            "超上限的声明值钳到天花板"
        );
    }

    /// 老客户端不带 timeoutSecs 也要能反序列化（缺字段 = 未声明，不能是报错）
    #[test]
    fn create_args_tolerate_missing_timeout_field() {
        let args: CreateArgs = serde_json::from_str(
            r#"{"id":"a","name":"n","description":"","icon":"📦",
                "inputSchema":"{\"kind\":\"folder\"}","scriptPath":"app.py",
                "manual":"","status":"draft"}"#,
        )
        .expect("缺 timeoutSecs 的旧载荷要能反序列化");
        assert_eq!(args.timeout_secs, None);
    }
}
