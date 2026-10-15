//! apps 模块 Tauri commands：list/get/run/create/update/archive + 运行历史/契约预检
//!
//! 入参字段名（camelCase）即 manifest 的字段名（name/description/icon/...）；
//! manifest 字段约束是 AI 上架工具的合同（设计稿 `docs/APPS-DESIGN-2026-10-15.md`
//! §2 与 M1 代码规范文档的 `docs/APP-DEV-CONVENTIONS.md`）。

use std::path::PathBuf;

use serde::Deserialize;
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

use crate::apps::{self as apps_runner, AppRunResult};
use crate::db::apps as db_apps;
use crate::db::apps::{App, AppRun};
use crate::error::{CommandError, CommandResult};
use crate::py::runtime::resolve_timeout;

/// 程序页 M2 全量视图（草稿 + 已启用，按 updated_at 倒序）：
/// 草稿卡带「草稿」徽章可试跑，归档不出现在程序页
#[tauri::command]
pub async fn apps_list_active(app: AppHandle) -> CommandResult<Vec<App>> {
    let conn = crate::db::open_db(&app).map_err(CommandError::DbError)?;
    db_apps::list_active_apps(&conn).map_err(CommandError::DbError)
}

/// 详情：管理面板/历史/试跑均用
#[tauri::command]
pub async fn apps_get(app: AppHandle, id: String) -> CommandResult<Option<App>> {
    let conn = crate::db::open_db(&app).map_err(CommandError::DbError)?;
    db_apps::get_app(&conn, &id).map_err(CommandError::DbError)
}

/// 契约预检：M1 仅校验脚本可读/非空；类型/schema 校验留给 M2
#[tauri::command]
pub async fn apps_preflight(
    app: AppHandle,
    app_id: String,
    script_rel: Option<String>,
) -> CommandResult<()> {
    let a = db_apps::get_app(&open_db_or(&app)?, &app_id)
        .map_err(CommandError::DbError)?
        .ok_or_else(|| CommandError::TaskNotFound(app_id.clone()))?;
    let rel = script_rel.as_deref().unwrap_or(&a.script_path);
    apps_runner::preflight_script(&app, &a, rel).map_err(CommandError::DbError)
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
    let a = db_apps::get_app(&open_db_or(&app)?, &args.id)
        .map_err(CommandError::DbError)?
        .ok_or_else(|| CommandError::TaskNotFound(args.id.clone()))?;
    // M2 试跑闭环：草稿可运行（AI 创建后用户先试跑再上架）；归档不可运行
    if a.status == "archived" {
        return Err(CommandError::InvalidArgument {
            field: "id".into(),
            value: a.status.clone(),
            reason: "已归档程序不可运行".into(),
        });
    }
    let (timeout, _) = resolve_timeout(args.timeout_secs);
    // 跑（执行内部捕所有错误并回写 app_runs status=failed）
    let res = apps_runner::run(
        &a,
        &a.script_path,
        args.params.as_deref(),
        Some(timeout),
        &app,
        None,
    )
    .map_err(CommandError::DbError)?;
    // 系统通知：跑完/失败给个轻提示（M1 不做结果面板里的丰富展示）
    let title = format!(
        "程序「{}」{}",
        a.name,
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
}

#[tauri::command]
pub async fn apps_create(app: AppHandle, args: CreateArgs) -> CommandResult<App> {
    let a = build_app_from_create(&app, &args)?;
    apps_runner::preflight_script(&app, &a, &a.script_path).map_err(CommandError::DbError)?;
    let conn = crate::db::open_db(&app).map_err(CommandError::DbError)?;
    let dir = apps_runner::app_dir(&app, &a.id);
    // 落库 + 版本快照同一事务（口径同 manage 核）
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    db_apps::upsert_app(&tx, &a).map_err(CommandError::DbError)?;
    crate::apps::manage::record_app_version(&tx, &dir, &a, "create", crate::db::now_ms())?;
    tx.commit()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    Ok(a)
}

/// 改 description/icon/manual/status/script_path——脚本本体不重写
/// （改本体走 M2 的 `apps_update_files`，AI 拿整文件集上来才稳）
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateArgs {
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub input_schema: Option<String>,
    pub script_path: Option<String>,
    pub manual: Option<String>,
    pub status: Option<String>,
}

#[tauri::command]
pub async fn apps_update(app: AppHandle, args: UpdateArgs) -> CommandResult<App> {
    let conn = crate::db::open_db(&app).map_err(CommandError::DbError)?;
    let cur = db_apps::get_app(&conn, &args.id)
        .map_err(CommandError::DbError)?
        .ok_or_else(|| CommandError::TaskNotFound(args.id.clone()))?;
    let status = args.status.clone().unwrap_or_else(|| cur.status.clone());
    let next = App {
        name: args.name.unwrap_or(cur.name),
        description: args.description.unwrap_or(cur.description),
        icon: args.icon.unwrap_or(cur.icon),
        input_schema: args.input_schema.unwrap_or(cur.input_schema),
        script_path: args.script_path.clone().unwrap_or(cur.script_path),
        manual: args.manual.unwrap_or(cur.manual),
        status,
        ..cur
    };
    // script_path 若改，落盘前预检
    if args.script_path.is_some() {
        apps_runner::preflight_script(&app, &next, &next.script_path)
            .map_err(CommandError::DbError)?;
    }
    // status 若改，enabled → 写 enabled_at
    let now = crate::db::now_ms();
    let next = if cur.status != "enabled" && next.status == "enabled" {
        App {
            enabled_at: Some(now),
            updated_at: now,
            ..next
        }
    } else {
        App {
            updated_at: now,
            ..next
        }
    };
    // 落库 + 版本快照同一事务（元数据改动也是一次内容变更）
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    db_apps::upsert_app(&tx, &next).map_err(CommandError::DbError)?;
    crate::apps::manage::record_app_version(
        &tx,
        &apps_runner::app_dir(&app, &next.id),
        &next,
        "update",
        now,
    )?;
    tx.commit()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    Ok(next)
}

/// 在系统文件管理器中打开程序的 output/ 目录（不存在则先创建）。
/// 返回打开的目录路径。路径由 id 派生（id 已是路径分量校验过的），
/// 用户主动点击触发，无 LLM 参与——不走 bot 工具的白名单闸。
#[tauri::command]
pub async fn apps_open_output(app: AppHandle, id: String) -> CommandResult<String> {
    db_apps::get_app(&open_db_or(&app)?, &id)
        .map_err(CommandError::DbError)?
        .ok_or_else(|| CommandError::TaskNotFound(id.clone()))?;
    let dir = apps_runner::app_dir(&app, &id).join("output");
    std::fs::create_dir_all(&dir).map_err(|e| CommandError::DbError(e.to_string()))?;
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .reveal_item_in_dir(&dir)
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    Ok(dir.to_string_lossy().to_string())
}

/// 归档（程序页可隐藏，但仍可查 + 试跑）
#[tauri::command]
pub async fn apps_archive(app: AppHandle, id: String) -> CommandResult<App> {
    let conn = crate::db::open_db(&app).map_err(CommandError::DbError)?;
    let cur = db_apps::get_app(&conn, &id)
        .map_err(CommandError::DbError)?
        .ok_or_else(|| CommandError::TaskNotFound(id.clone()))?;
    let next = App {
        status: "archived".into(),
        updated_at: crate::db::now_ms(),
        ..cur
    };
    db_apps::upsert_app(&conn, &next).map_err(CommandError::DbError)?;
    Ok(next)
}

/// 最近运行历史（程序页 + 管理面板）
#[tauri::command]
pub async fn apps_runs_list(
    app: AppHandle,
    app_id: String,
    limit: Option<i64>,
) -> CommandResult<Vec<AppRun>> {
    apps_runner::recent_runs(&app, &app_id, limit.unwrap_or(20)).map_err(CommandError::DbError)
}

/// 版本快照列表（程序页「版本」页签；新→旧，不含脚本全文防 IPC 大载荷）
#[tauri::command]
pub async fn apps_versions_list(
    app: AppHandle,
    app_id: String,
    limit: Option<i64>,
) -> CommandResult<Vec<db_apps::AppVersionMeta>> {
    let conn = open_db_or(&app)?;
    db_apps::list_app_version_metas(&conn, &app_id, limit.unwrap_or(100))
        .map_err(CommandError::DbError)
}

/// 回滚到历史版本（程序页用户动作）：内容整体回旧版、生命周期状态保留，
/// 回滚本身记为新版本（可再回滚撤销）。无 LLM 参与，不走 bot 工具白名单闸
#[tauri::command]
pub async fn apps_version_restore(
    app: AppHandle,
    app_id: String,
    version: i64,
) -> CommandResult<App> {
    let conn = open_db_or(&app)?;
    let dir = apps_runner::app_dir(&app, &app_id);
    crate::apps::manage::restore_version_core(&dir, &conn, &app_id, version, crate::db::now_ms())
}

// ─── helpers ─────────────────────────────────────────────────────────

fn open_db_or(app: &AppHandle) -> Result<rusqlite::Connection, CommandError> {
    crate::db::open_db(app).map_err(CommandError::DbError)
}

fn build_app_from_create(app: &AppHandle, args: &CreateArgs) -> Result<App, CommandError> {
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
