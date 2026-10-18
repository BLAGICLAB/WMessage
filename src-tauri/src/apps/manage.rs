//! apps M2 开发态：bot 工具领域核（apps_list / apps_create / apps_update_files）
//!
//! 设计稿 `docs/APPS-DESIGN-2026-10-15.md` §4（聊天即 IDE）与
//! `docs/APP-DEV-CONVENTIONS.md` §5：
//! - `apps_create` 落库为 **草稿** + 脚本文件集落盘（整文件提交，不做增量合并）
//! - 用户在程序页试跑（草稿可运行）→ 满意后 AI 调 `apps_update_files` 置 enabled 上架
//! - `apps_list` 列非归档程序防重名
//!
//! 核心全部是 `(dir, conn)` 纯函数（不依赖 AppHandle），单测直打；
//! AppHandle（路径解析 / open_db）只在文件尾部 tool_* 包装层出现。
//!
//! 失败语义：库是真相源——文件集写一半失败时只上抛错误不回滚（重试即整体覆盖，
//! 无库行的孤儿文件程序页不可见）；预置的用户数据（data/）不在清理范围。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use tauri::AppHandle;

use crate::bot::registry::ToolResult;
use crate::db;
use crate::db::apps as db_apps;
use crate::db::apps::App;
use crate::error::CommandError;
use crate::py::document::spawn_blocking_map;

use super::commands::manifest_contract as mc;

// ── 归档 / 恢复（纯函数核心，Tauri 命令直调，单测直打） ──

/// 归档：程序页用户主动操作，bot 工具不受理（`apps_update_files` 的描述里
/// 写明这条边界）。只改生命周期状态——脚本、附加文件、版本快照、运行历史
/// 全部原地保留，所以这是一次可逆操作，不是删除。
pub fn archive_core(conn: &Connection, id: &str, now_ms: i64) -> Result<App, CommandError> {
    let cur = db_apps::get_app(conn, id)
        .map_err(CommandError::DbError)?
        .ok_or_else(|| CommandError::TaskNotFound(id.to_string()))?;
    if cur.status == mc::STATUS_ARCHIVED {
        return Ok(cur); // 幂等：重复点归档不该报错
    }
    let next = App {
        status: mc::STATUS_ARCHIVED.into(),
        updated_at: now_ms,
        ..cur
    };
    db_apps::upsert_app(conn, &next).map_err(CommandError::DbError)?;
    Ok(next)
}

/// 恢复归档程序 → **草稿**（不是 enabled）：恢复只是把程序放回程序页让它能看见
/// 能试跑，重新上架得用户满意后由 AI 走 `apps_update_files` 或程序页上架流程
pub fn unarchive_core(conn: &Connection, id: &str, now_ms: i64) -> Result<App, CommandError> {
    let cur = db_apps::get_app(conn, id)
        .map_err(CommandError::DbError)?
        .ok_or_else(|| CommandError::TaskNotFound(id.to_string()))?;
    if cur.status != mc::STATUS_ARCHIVED {
        return Err(CommandError::DomainRule {
            domain: "apps".into(),
            reason: format!(
                "程序「{id}」不在已归档状态，无需恢复（当前 {}）",
                cur.status
            ),
        });
    }
    let next = App {
        status: mc::STATUS_DRAFT.into(),
        enabled_at: None,
        updated_at: now_ms,
        ..cur
    };
    db_apps::upsert_app(conn, &next).map_err(CommandError::DbError)?;
    Ok(next)
}

// ── 彻底删除（归档后的终态操作，纯函数核心，Tauri 命令直调，单测直打） ──

/// 彻底删除：抹掉库内三张表的行 + 删掉整个程序目录。**不可逆。**
///
/// 只接受已归档程序——这是「先归档、再删除」两道刻意动作的价值所在：程序页上的
/// 那个删除键是归档（可恢复），删除键出现在归档视图里，两步走完才轮到不可逆的
/// 这一步。程序页直接删等于一次点击就没了代码，而这里的代码多半是 AI 写的、
/// 重写一遍不便宜。
///
/// 顺序刻意是「先库后文件」：反过来的话，进程在两步之间崩掉会留下一个
/// 程序页还看得见、脚本却已经没了的条目，那比留下一个没人看得见的孤儿目录
/// 难查得多（库里说有、盘上没有）。
///
/// `data/`（AI 预置）与 `input/`（用户自己放的数据）都在目录内，一并删除——
/// 确认框已逐项写明这一点，这里不替用户保留任何东西。
pub fn delete_core(dir: &Path, conn: &Connection, id: &str) -> Result<(), CommandError> {
    let cur = db_apps::get_app(conn, id)
        .map_err(CommandError::DbError)?
        .ok_or_else(|| CommandError::TaskNotFound(id.to_string()))?;
    if cur.status != mc::STATUS_ARCHIVED {
        return Err(CommandError::DomainRule {
            domain: "apps".into(),
            reason: format!(
                "只有已归档的程序才能彻底删除；「{id}」当前是 {}，请先在程序页归档",
                cur.status
            ),
        });
    }
    db_apps::delete_app_rows(conn, id).map_err(CommandError::DbError)?;
    // 目录可能已经不在（用户手工删过、或从别处同步来的元数据）：
    // 那正是我们要的终态，不算失败。
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(CommandError::IoError(format!(
            "程序记录已删除，但目录 {} 清不掉：{e}",
            dir.display()
        ))),
    }
}

// ── 上限（对齐 bot_skills::manage 的 create_skill 同款量级） ──

/// app.py 源码字符上限（100K 字符 ≈ 常规脚本的百倍裕度）
pub const SCRIPT_MAX_CHARS: usize = 100 * 1024;
/// 单个附加文件字节上限；附加文件个数上限；文件集总字节上限
pub const EXTRA_FILE_MAX_BYTES: usize = 200 * 1024;
pub const EXTRA_FILE_MAX_COUNT: usize = 10;
pub const FILE_SET_MAX_TOTAL_BYTES: usize = 1024 * 1024;

// ── 校验（纯函数，创建/更新/Tauri 命令共用） ──

/// id 必须是合法相对路径分量（程序目录 `apps/<id>/`）
pub fn validate_app_id(id: &str) -> Result<(), CommandError> {
    if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(CommandError::InvalidArgument {
            field: "id".into(),
            value: id.to_string(),
            reason: "id 不能为空、不能含路径分隔符或 `..`".into(),
        });
    }
    Ok(())
}

/// manifest 元数据长度契约（`docs/APP-DEV-CONVENTIONS.md` §2）
pub fn validate_manifest_meta(
    name: &str,
    description: &str,
    icon: &str,
    manual: &str,
) -> Result<(), CommandError> {
    let checks = [
        ("名称", name.chars().count(), mc::NAME_MAX),
        ("说明", description.chars().count(), mc::DESCRIPTION_MAX),
        ("图标", icon.chars().count(), mc::ICON_MAX),
        ("说明书", manual.chars().count(), mc::MANUAL_MAX),
    ];
    for (label, len, max) in checks {
        if len > max {
            return Err(CommandError::InvalidArgument {
                field: "meta".into(),
                value: String::new(),
                reason: format!("{label}超过 {max} 字符上限（当前 {len}）"),
            });
        }
    }
    Ok(())
}

/// inputSchema 校验：合法 JSON + 文件夹/表单二选一 + 表单字段 type 三选一 +
/// key 非空且不重复（M2 新增；`docs/APP-DEV-CONVENTIONS.md` §3/§5）
pub fn validate_input_schema(raw: &str) -> Result<(), String> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("inputSchema 不是合法 JSON：{e}"))?;
    let kind = v
        .get("kind")
        .and_then(|k| k.as_str())
        .ok_or("inputSchema 缺 kind（folder 或 form）")?;
    match kind {
        "folder" => Ok(()),
        "form" => {
            let fields = v
                .get("fields")
                .and_then(|f| f.as_array())
                .ok_or("表单模式 inputSchema 缺 fields 数组")?;
            let mut seen: HashSet<String> = HashSet::new();
            for f in fields {
                let key = f
                    .get("key")
                    .and_then(|k| k.as_str())
                    .ok_or("表单字段缺 key")?;
                if key.trim().is_empty() {
                    return Err("表单字段 key 不能为空".into());
                }
                if !seen.insert(key.to_string()) {
                    return Err(format!("表单字段 key「{key}」重复"));
                }
                let ty = f.get("type").and_then(|t| t.as_str()).unwrap_or("text");
                if !matches!(ty, "text" | "number" | "date") {
                    return Err(format!(
                        "表单字段「{key}」type 必须是 text/number/date 三选一，得到「{ty}」"
                    ));
                }
            }
            Ok(())
        }
        other => Err(format!(
            "inputSchema kind 必须是 folder 或 form，得到「{other}」"
        )),
    }
}

/// inputSchema 入参归一化：bot 工具侧允许传对象或字符串，统一成 JSON 文本；
/// 缺省 = 文件夹模式（最简路径，`docs/APP-DEV-CONVENTIONS.md` §3 推荐）
pub fn normalize_input_schema(v: Option<&serde_json::Value>) -> Result<String, CommandError> {
    match v {
        None | Some(serde_json::Value::Null) => Ok(r#"{"kind":"folder"}"#.into()),
        Some(serde_json::Value::String(s)) => {
            validate_input_schema(s).map_err(reason_to_invalid())?;
            Ok(s.clone())
        }
        Some(other) => {
            let s =
                serde_json::to_string(other).map_err(|e| CommandError::IoError(e.to_string()))?;
            validate_input_schema(&s).map_err(reason_to_invalid())?;
            Ok(s)
        }
    }
}

fn reason_to_invalid() -> impl Fn(String) -> CommandError {
    |reason| CommandError::InvalidArgument {
        field: "inputSchema".into(),
        value: String::new(),
        reason,
    }
}

// ── 文件集 ──

/// 校验通过后的待写附加文件（相对目标 + 内容）
struct PendingFile {
    rel: PathBuf,
    content: String,
}

/// 附加文件校验：仅允许 `data/` 之下（目录契约：input/ 是用户的、output/ 是
/// 运行时的，AI 创建期都不得写）；相对路径逐组件字符集校验（防穿越，同 create_skill）。
fn validate_extra_files(files: &[(String, String)]) -> Result<Vec<PendingFile>, CommandError> {
    if files.len() > EXTRA_FILE_MAX_COUNT {
        return Err(CommandError::InvalidArgument {
            field: "files".into(),
            value: files.len().to_string(),
            reason: format!("附加文件超过 {EXTRA_FILE_MAX_COUNT} 个上限"),
        });
    }
    let mut out = Vec::new();
    let mut total = 0usize;
    for (p, c) in files {
        let pb = Path::new(p);
        if pb.is_absolute() || p.contains("..") {
            return Err(CommandError::InvalidArgument {
                field: "files.path".into(),
                value: p.clone(),
                reason: "附加文件路径必须为程序目录内的相对路径".into(),
            });
        }
        let mut comps = pb.components();
        let first_ok = matches!(comps.next(), Some(std::path::Component::Normal(s)) if s == "data");
        if !first_ok {
            return Err(CommandError::InvalidArgument {
                field: "files.path".into(),
                value: p.clone(),
                reason: "附加文件只允许写在 data/ 之下（input/ 归用户、output/ 归运行）".into(),
            });
        }
        for comp in comps {
            let s = match comp {
                std::path::Component::Normal(s) => s.to_string_lossy().to_string(),
                _ => {
                    return Err(CommandError::InvalidArgument {
                        field: "files.path".into(),
                        value: p.clone(),
                        reason: "附加文件路径含非法组件（绝对路径/./.. 等）".into(),
                    })
                }
            };
            if s.is_empty()
                || s == "."
                || s == ".."
                || !s
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
            {
                return Err(CommandError::InvalidArgument {
                    field: "files.path".into(),
                    value: p.clone(),
                    reason: format!("附加文件路径组件「{s}」含不允许的字符"),
                });
            }
        }
        if c.len() > EXTRA_FILE_MAX_BYTES {
            return Err(CommandError::InvalidArgument {
                field: "files.content".into(),
                value: p.clone(),
                reason: format!("单文件超过 {EXTRA_FILE_MAX_BYTES} 字节上限"),
            });
        }
        total += c.len();
        if total > FILE_SET_MAX_TOTAL_BYTES {
            return Err(CommandError::InvalidArgument {
                field: "files".into(),
                value: p.clone(),
                reason: format!("文件集总大小超过 {FILE_SET_MAX_TOTAL_BYTES} 字节上限"),
            });
        }
        out.push(PendingFile {
            rel: pb.to_path_buf(),
            content: c.clone(),
        });
    }
    Ok(out)
}

/// 脚本本体校验：非空 + 字符上限
fn validate_script(script: &str) -> Result<(), CommandError> {
    if script.trim().is_empty() {
        return Err(CommandError::InvalidArgument {
            field: "script".into(),
            value: String::new(),
            reason: "script 不能为空（app.py 源码全文）".into(),
        });
    }
    if script.chars().count() > SCRIPT_MAX_CHARS {
        return Err(CommandError::InvalidArgument {
            field: "script".into(),
            value: String::new(),
            reason: format!("script 超过 {SCRIPT_MAX_CHARS} 字符上限"),
        });
    }
    Ok(())
}

/// 落盘：脚本写 `dir/app.py`（M1 起脚本路径恒定），附加文件写校验过的相对目标
fn write_file_set(
    dir: &Path,
    script: Option<&str>,
    extra: &[PendingFile],
) -> Result<(), CommandError> {
    if let Some(s) = script {
        std::fs::write(dir.join("app.py"), s).map_err(|e| CommandError::IoError(e.to_string()))?;
    }
    for f in extra {
        let target = dir.join(&f.rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CommandError::IoError(e.to_string()))?;
        }
        std::fs::write(target, &f.content).map_err(|e| CommandError::IoError(e.to_string()))?;
    }
    Ok(())
}

// ── 创建（草稿）核 ──

/// apps_create 领域核：防重名 → 校验 → 文件集落盘 → 落库草稿。
/// status 固定 draft（上架 = 用户试跑满意后 update 置 enabled，设计稿 §4）。
#[allow(clippy::too_many_arguments)]
/// 重名拒绝：登记走 upsert（ON CONFLICT 覆盖），不先查会静默覆盖已有程序
/// 的全部元数据。页面手工上架与 bot 创建同规
pub fn ensure_app_absent(conn: &Connection, id: &str) -> Result<(), CommandError> {
    if db_apps::get_app(conn, id)
        .map_err(CommandError::DbError)?
        .is_some()
    {
        return Err(CommandError::DomainRule {
            domain: "apps".into(),
            reason: format!(
                "程序「{id}」已存在：改用 apps_update_files 修改，或先 apps_list 看现有程序换个 id"
            ),
        });
    }
    Ok(())
}

pub fn create_app_core(
    dir: &Path,
    conn: &Connection,
    id: &str,
    name: &str,
    description: &str,
    icon: &str,
    input_schema: &str,
    script: &str,
    files: &[(String, String)],
    manual: &str,
    now_ms: i64,
) -> Result<App, CommandError> {
    validate_app_id(id)?;
    ensure_app_absent(conn, id)?;
    validate_manifest_meta(name, description, icon, manual)?;
    validate_input_schema(input_schema).map_err(reason_to_invalid())?;
    validate_script(script)?;
    let extra = validate_extra_files(files)?;
    std::fs::create_dir_all(dir).map_err(|e| CommandError::IoError(e.to_string()))?;
    write_file_set(dir, Some(script), &extra)?;
    let app = App {
        id: id.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        icon: icon.to_string(),
        input_schema: input_schema.to_string(),
        script_path: "app.py".into(),
        manual: manual.to_string(),
        status: mc::STATUS_DRAFT.into(),
        enabled_at: None,
        created_at: now_ms,
        updated_at: now_ms,
    };
    // 落库 + 版本快照同一事务：快照写不进去 = 整体没发生（库是真相源）
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    db_apps::upsert_app(&tx, &app).map_err(CommandError::DbError)?;
    record_app_version(&tx, dir, &app, "create", now_ms)?;
    tx.commit()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    Ok(app)
}

// ── 版本快照（为何不用 git 见 db::apps::APP_VERSIONS_DDL 注） ──

/// 每程序滚动保留的快照数（单版 KB 级，100 版 ≈ 1MB 上界）
pub const APP_VERSIONS_KEEP: i64 = 100;

/// 记一条版本快照：读盘上 app.py 全文 + 当前 manifest，写入下一版本号并滚动裁剪。
/// 必须在调用方事务内调用（Transaction 解引用为 Connection，直接传即可）
pub fn record_app_version(
    conn: &Connection,
    dir: &Path,
    a: &App,
    source: &str,
    now_ms: i64,
) -> Result<(), CommandError> {
    let script = std::fs::read_to_string(dir.join("app.py"))
        .map_err(|e| CommandError::IoError(e.to_string()))?;
    let version = db_apps::next_app_version(conn, &a.id).map_err(CommandError::DbError)?;
    let v = db_apps::AppVersion {
        id: 0,
        app_id: a.id.clone(),
        version,
        name: a.name.clone(),
        description: a.description.clone(),
        icon: a.icon.clone(),
        input_schema: a.input_schema.clone(),
        script,
        manual: a.manual.clone(),
        source: source.to_string(),
        created_at: now_ms,
    };
    db_apps::insert_app_version(conn, &v).map_err(CommandError::DbError)?;
    db_apps::trim_app_versions(conn, &a.id, APP_VERSIONS_KEEP).map_err(CommandError::DbError)
}

/// 回滚核 = 历史快照内容整体写回：manifest 五项（名称/说明/图标/声明/说明书）
/// 与脚本回旧版；**生命周期不动**（status/enabled_at 保持现状，回滚的是内容不是状态）。
/// 回滚本身产出新版本（source="restore"），可再次回滚撤销。
pub fn restore_version_core(
    dir: &Path,
    conn: &Connection,
    app_id: &str,
    version: i64,
    now_ms: i64,
) -> Result<App, CommandError> {
    let snap = db_apps::get_app_version(conn, app_id, version)
        .map_err(CommandError::DbError)?
        .ok_or_else(|| CommandError::TaskNotFound(format!("{app_id}@v{version}")))?;
    let cur = db_apps::get_app(conn, app_id)
        .map_err(CommandError::DbError)?
        .ok_or_else(|| CommandError::TaskNotFound(app_id.to_string()))?;
    // 归档 = 「先别动它」（update 双口同拒）：回滚会整体重写脚本与 manifest，
    // 属内容变更，恢复后再做
    if cur.status == mc::STATUS_ARCHIVED {
        return Err(CommandError::DomainRule {
            domain: "apps".into(),
            reason: "已归档程序不开放回滚；请先在程序页恢复".into(),
        });
    }
    std::fs::create_dir_all(dir).map_err(|e| CommandError::IoError(e.to_string()))?;
    write_file_set(dir, Some(&snap.script), &[])?;
    let next = App {
        id: cur.id.clone(),
        name: snap.name.clone(),
        description: snap.description.clone(),
        icon: snap.icon.clone(),
        input_schema: snap.input_schema.clone(),
        script_path: "app.py".into(),
        manual: snap.manual.clone(),
        status: cur.status.clone(),
        enabled_at: cur.enabled_at,
        created_at: cur.created_at,
        updated_at: now_ms,
    };
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    db_apps::upsert_app(&tx, &next).map_err(CommandError::DbError)?;
    record_app_version(&tx, dir, &next, "restore", now_ms)?;
    tx.commit()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    Ok(next)
}

// ── 更新（整文件集）核 ──

/// apps_update_files 领域核：要求已存在；整文件集提交——script 给了就整体重写
/// app.py（不收 diff/片段），metadata 字段缺省 = 保持不变；
/// status 只接受 draft/enabled（archived 是用户在程序页的主动操作，工具口拒绝）。
#[allow(clippy::too_many_arguments)]
pub fn update_app_core(
    dir: &Path,
    conn: &Connection,
    id: &str,
    script: Option<&str>,
    files: &[(String, String)],
    name: Option<&str>,
    description: Option<&str>,
    icon: Option<&str>,
    input_schema: Option<&str>,
    manual: Option<&str>,
    status: Option<&str>,
    now_ms: i64,
) -> Result<App, CommandError> {
    let cur = db_apps::get_app(conn, id)
        .map_err(CommandError::DbError)?
        .ok_or_else(|| CommandError::TaskNotFound(id.to_string()))?;
    if cur.status == mc::STATUS_ARCHIVED {
        return Err(CommandError::DomainRule {
            domain: "apps".into(),
            reason: "已归档程序不开放修改；请用户先在程序页恢复".into(),
        });
    }
    let next_status = match status {
        None => cur.status.clone(),
        Some(s) if s == mc::STATUS_DRAFT || s == mc::STATUS_ENABLED => s.to_string(),
        Some(other) => {
            return Err(CommandError::InvalidArgument {
                field: "status".into(),
                value: other.to_string(),
                reason: "status 只接受 draft / enabled（归档请用户在程序页操作）".into(),
            })
        }
    };
    // 没有任何实质变更直接拒绝：让模型自查意图（改脚本/改声明/改状态必占其一）
    if script.is_none()
        && files.is_empty()
        && name.is_none()
        && description.is_none()
        && icon.is_none()
        && input_schema.is_none()
        && manual.is_none()
        && status.is_none()
    {
        return Err(CommandError::InvalidArgument {
            field: "id".into(),
            value: id.to_string(),
            reason: "没有任何变更字段：改脚本传完整 script，其他字段按需传".into(),
        });
    }
    let name = name.unwrap_or(&cur.name);
    let description = description.unwrap_or(&cur.description);
    let icon = icon.unwrap_or(&cur.icon);
    let manual = manual.unwrap_or(&cur.manual);
    let input_schema = match input_schema {
        Some(s) => {
            validate_input_schema(s).map_err(reason_to_invalid())?;
            s.to_string()
        }
        None => cur.input_schema.clone(),
    };
    validate_manifest_meta(name, description, icon, manual)?;
    if let Some(s) = script {
        validate_script(s)?;
    }
    let extra = validate_extra_files(files)?;
    std::fs::create_dir_all(dir).map_err(|e| CommandError::IoError(e.to_string()))?;
    write_file_set(dir, script, &extra)?;
    // enabled_at：首次上架（非 enabled → enabled）写当前时刻；回草稿保留原值
    let enabled_at = if cur.status != mc::STATUS_ENABLED && next_status == mc::STATUS_ENABLED {
        Some(now_ms)
    } else {
        cur.enabled_at
    };
    let next = App {
        id: cur.id.clone(),
        name: name.to_string(),
        description: description.to_string(),
        icon: icon.to_string(),
        input_schema,
        script_path: "app.py".into(),
        manual: manual.to_string(),
        status: next_status,
        enabled_at,
        created_at: cur.created_at,
        updated_at: now_ms,
    };
    // 落库 + 版本快照同一事务（口径同创建核）
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    db_apps::upsert_app(&tx, &next).map_err(CommandError::DbError)?;
    record_app_version(&tx, dir, &next, "update", now_ms)?;
    tx.commit()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    Ok(next)
}

// ── bot 工具包装层（AppHandle / 落盘目录解析只在这一段出现） ──

/// 必填字符串字段：trim 后非空才返回（语义标记，实现同 optional_field）
fn required_field(v: &serde_json::Value, key: &str) -> Option<String> {
    optional_field(v, key)
}

/// 可选元数据字段：空串视同未传（更新语义 = 保持不变）
fn optional_field(v: &serde_json::Value, key: &str) -> Option<String> {
    v[key]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// files 数组 → (path, content) 对；条目缺字段返回错误文本
fn files_from_args(v: &serde_json::Value) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    if let Some(arr) = v["files"].as_array() {
        for f in arr {
            match (f["path"].as_str(), f["content"].as_str()) {
                (Some(p), Some(c)) => out.push((p.to_string(), c.to_string())),
                _ => return Err("files 条目必须同时包含 path 与 content".into()),
            }
        }
    }
    Ok(out)
}

/// apps_list：列非归档程序（草稿 + 已启用），防重名 / 找现有 id 用
pub async fn tool_apps_list(app: &AppHandle, _args: &str) -> ToolResult {
    let owned = app.clone();
    let rows = spawn_blocking_map(move || -> Result<Vec<App>, String> {
        let conn = db::open_db(&owned).map_err(|e| e.to_string())?;
        db_apps::list_active_apps(&conn)
    })
    .await;
    match rows {
        Ok(rows) if rows.is_empty() => ToolResult::ok(
            "还没有程序（可用 apps_create 创建）".to_string(),
            Vec::new(),
        ),
        Ok(rows) => {
            let draft = rows.iter().filter(|a| a.status == mc::STATUS_DRAFT).count();
            let mut s = format!(
                "共 {} 个程序（草稿 {draft} / 已启用 {}）：\n",
                rows.len(),
                rows.len() - draft
            );
            for a in &rows {
                let status = if a.status == mc::STATUS_DRAFT {
                    "草稿"
                } else {
                    "已启用"
                };
                let desc = if a.description.is_empty() {
                    "（无说明）"
                } else {
                    a.description.as_str()
                };
                s.push_str(&format!("- {}｜{}｜{}｜{}\n", a.id, a.name, status, desc));
            }
            ToolResult::ok(s, Vec::new())
        }
        Err(e) => ToolResult::error(format!("读取程序列表失败：{e}"), Vec::new()),
    }
}

/// apps_create：脚本 + 输入声明 + 附加文件落盘为草稿程序（设计稿 §4 聊天即 IDE）
pub async fn tool_apps_create(app: &AppHandle, args: &str) -> ToolResult {
    let v = crate::bot::parse_args(args);
    let Some(id) = required_field(&v, "id") else {
        return ToolResult::ok(
            "apps_create 缺少 id（程序目录名：字母/数字/-/_，路径分量，不能含 / 或 ..）"
                .to_string(),
            Vec::new(),
        );
    };
    if let Err(e) = validate_app_id(&id) {
        return ToolResult::ok(format!("apps_create：{e}"), Vec::new());
    }
    let Some(name) = required_field(&v, "name") else {
        return ToolResult::ok(
            "apps_create 缺少 name（程序页卡片标题）".to_string(),
            Vec::new(),
        );
    };
    let Some(script) = v["script"].as_str().map(|s| s.to_string()) else {
        return ToolResult::ok(
            "apps_create 缺少 script（完整 app.py 源码全文，不许片段/diff）".to_string(),
            Vec::new(),
        );
    };
    let description = optional_field(&v, "description").unwrap_or_default();
    let icon = optional_field(&v, "icon").unwrap_or_else(|| "📦".into());
    let manual = optional_field(&v, "manual").unwrap_or_default();
    let input_schema = match normalize_input_schema(v.get("input_schema")) {
        Ok(s) => s,
        Err(e) => return ToolResult::ok(format!("apps_create：{e}"), Vec::new()),
    };
    let files = match files_from_args(&v) {
        Ok(f) => f,
        Err(e) => return ToolResult::ok(format!("apps_create：{e}"), Vec::new()),
    };
    crate::bot::audit_log(
        app,
        &format!(
            "apps_create | {} | files: {}",
            crate::bot::escape_for_log(&id, 100),
            files.len()
        ),
    );
    let owned = app.clone();
    let dir = super::app_dir(app, &id);
    let dir_log = dir.display().to_string();
    let id_for_log = id.clone();
    let created = spawn_blocking_map(move || {
        // fs + 事务写，与其他写口同闸串行（页面手工上架可并发）
        let _g = crate::db::lock_db_write();
        let conn = db::open_db(&owned).map_err(|e| e.to_string())?;
        create_app_core(
            &dir,
            &conn,
            &id,
            &name,
            &description,
            &icon,
            &input_schema,
            &script,
            &files,
            &manual,
            db::now_ms(),
        )
        .map_err(|e| e.to_string())
    })
    .await;
    match created {
        Ok(created) => {
            let name = created.name;
            ToolResult::ok(
                format!(
                    "程序「{name}」已创建为草稿，目录：{dir_log}。\n\
                     下一步：让用户把真实输入放进该目录 input/（表单模式则直接在程序页运行时填写），\
                     在程序页点「运行」试跑（草稿可运行）；失败把报错发回聊天，用 apps_update_files \
                     提交完整新脚本；用户明确满意后再调 apps_update_files 并传 status=\"enabled\" 上架。"
                ),
                Vec::new(),
            )
        }
        Err(e) => {
            crate::bot::audit_log(
                app,
                &format!(
                    "apps_create_failed | {} | {}",
                    crate::bot::escape_for_log(&id_for_log, 100),
                    crate::bot::escape_for_log(&e, 200)
                ),
            );
            ToolResult::error(format!("创建失败：{e}"), Vec::new())
        }
    }
}

/// apps_update_files：整文件集修改（script 给了就整体重写 app.py，不收 diff），
/// 可改元数据/输入声明/说明书；status="enabled" 即上架
pub async fn tool_apps_update_files(app: &AppHandle, args: &str) -> ToolResult {
    let v = crate::bot::parse_args(args);
    let Some(id) = required_field(&v, "id") else {
        return ToolResult::ok(
            "apps_update_files 缺少 id（要修改的程序 id，先 apps_list 查）".to_string(),
            Vec::new(),
        );
    };
    let script = v["script"].as_str().map(|s| s.to_string());
    let files = match files_from_args(&v) {
        Ok(f) => f,
        Err(e) => return ToolResult::ok(format!("apps_update_files：{e}"), Vec::new()),
    };
    let name = optional_field(&v, "name");
    let description = optional_field(&v, "description");
    let icon = optional_field(&v, "icon");
    let manual = optional_field(&v, "manual");
    // input_schema 缺省/null = 保持不变（与更新语义一致）
    let input_schema = match v.get("input_schema") {
        None | Some(serde_json::Value::Null) => None,
        Some(val) => match normalize_input_schema(Some(val)) {
            Ok(s) => Some(s),
            Err(e) => return ToolResult::ok(format!("apps_update_files：{e}"), Vec::new()),
        },
    };
    let status = optional_field(&v, "status");
    crate::bot::audit_log(
        app,
        &format!(
            "apps_update_files | {} | script: {} files: {} status: {}",
            crate::bot::escape_for_log(&id, 100),
            script.is_some(),
            files.len(),
            crate::bot::escape_for_log(status.as_deref().unwrap_or("-"), 20),
        ),
    );
    let owned = app.clone();
    let dir = super::app_dir(app, &id);
    let id_for_log = id.clone();
    let updated = spawn_blocking_map(move || {
        // fs 重写 + 事务落库，与其他写口同闸串行（页面回滚/手工上架可并发）
        let _g = crate::db::lock_db_write();
        let conn = db::open_db(&owned).map_err(|e| e.to_string())?;
        update_app_core(
            &dir,
            &conn,
            &id,
            script.as_deref(),
            &files,
            name.as_deref(),
            description.as_deref(),
            icon.as_deref(),
            input_schema.as_deref(),
            manual.as_deref(),
            status.as_deref(),
            db::now_ms(),
        )
        .map_err(|e| e.to_string())
    })
    .await;
    match updated {
        Ok(next) => {
            let status_label = if next.status == mc::STATUS_ENABLED {
                "已启用（上架）"
            } else {
                "草稿"
            };
            ToolResult::ok(
                format!(
                    "程序「{}」已更新：当前状态 {}。改了脚本记得让用户在程序页重新试跑。",
                    next.name, status_label
                ),
                Vec::new(),
            )
        }
        Err(e) => {
            crate::bot::audit_log(
                app,
                &format!(
                    "apps_update_files_failed | {} | {}",
                    crate::bot::escape_for_log(&id_for_log, 100),
                    crate::bot::escape_for_log(&e, 200)
                ),
            );
            ToolResult::error(format!("更新失败：{e}"), Vec::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (tempfile::TempDir, Connection) {
        let tmp = tempfile::tempdir().unwrap();
        let conn = Connection::open(tmp.path().join("test.db")).unwrap();
        conn.execute(db_apps::APPS_DDL, []).unwrap();
        conn.execute(db_apps::APP_VERSIONS_DDL, []).unwrap();
        // app_runs 一并建：三张表在生产里由 db::ensure 一次建齐，缺一张的夹具
        // 会让「删干净三张表」这类用例撞上 no such table 而不是它要验的东西
        conn.execute(db_apps::APP_RUNS_DDL, []).unwrap();
        (tmp, conn)
    }

    const SCRIPT: &str = "import sys\nprint('##RESULT## {\"summary\":\"hi\"}')\n";

    /// 直接落一行已上架程序（归档/恢复测试只需要库里的行，不碰文件集）
    fn seed_enabled(conn: &Connection, id: &str) -> App {
        let app = App {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            icon: "📦".into(),
            input_schema: r#"{"kind":"folder"}"#.into(),
            script_path: "app.py".into(),
            manual: String::new(),
            status: mc::STATUS_ENABLED.into(),
            enabled_at: Some(1000),
            created_at: 1000,
            updated_at: 1000,
        };
        db_apps::upsert_app(conn, &app).unwrap();
        app
    }

    fn create_args(
        id: &str,
    ) -> (
        String,
        String,
        String,
        String,
        String,
        String,
        Vec<(String, String)>,
        String,
    ) {
        (
            id.to_string(),
            "工资单匹配".to_string(),
            "把流水和工资表对上".to_string(),
            "💰".to_string(),
            r#"{"kind":"folder"}"#.to_string(),
            SCRIPT.to_string(),
            Vec::new(),
            String::new(),
        )
    }

    #[test]
    fn create_writes_draft_with_script_and_meta() {
        let (tmp, conn) = temp_db();
        let dir = tmp.path().join("wage");
        let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
        let app = create_app_core(
            &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, 1,
        )
        .unwrap();
        assert_eq!(app.status, "draft");
        assert_eq!(app.script_path, "app.py");
        assert!(app.enabled_at.is_none());
        assert!(dir.join("app.py").is_file());
        assert!(db_apps::get_app(&conn, "wage").unwrap().is_some());
    }

    #[test]
    fn create_rejects_duplicate_id_without_overwrite() {
        let (tmp, conn) = temp_db();
        let dir = tmp.path().join("wage");
        let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
        create_app_core(
            &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, 1,
        )
        .unwrap();
        let err = create_app_core(
            &dir,
            &conn,
            &id,
            &name,
            &desc,
            &icon,
            &schema,
            "print('x')",
            &files,
            &manual,
            2,
        )
        .unwrap_err();
        assert!(err.to_string().contains("已存在"), "{err}");
        // 原脚本未被覆盖
        let kept = std::fs::read_to_string(dir.join("app.py")).unwrap();
        assert_eq!(kept, SCRIPT);
    }

    #[test]
    fn create_rejects_bad_id_and_meta_and_schema() {
        let (tmp, conn) = temp_db();
        let dir = tmp.path().join("x");
        let (_i, name, desc, icon, schema, script, files, manual) = create_args("ok");
        // id 含 ..
        let err = create_app_core(
            &dir, &conn, "../evil", &name, &desc, &icon, &schema, &script, &files, &manual, 1,
        )
        .unwrap_err();
        assert!(err.to_string().contains("id"), "{err}");
        // 名称超长
        let long_name = "名".repeat(mc::NAME_MAX + 1);
        let err = create_app_core(
            &dir, &conn, "ok", &long_name, &desc, &icon, &schema, &script, &files, &manual, 1,
        )
        .unwrap_err();
        assert!(err.to_string().contains("名称"), "{err}");
        // inputSchema 表单字段 type 非法
        let err = create_app_core(
            &dir,
            &conn,
            "ok",
            &name,
            &desc,
            &icon,
            r#"{"kind":"form","fields":[{"key":"a","label":"A","type":"blob"}]}"#,
            &script,
            &files,
            &manual,
            1,
        )
        .unwrap_err();
        assert!(err.to_string().contains("text/number/date"), "{err}");
        // 空脚本
        let err = create_app_core(
            &dir, &conn, "ok", &name, &desc, &icon, &schema, "  ", &files, &manual, 1,
        )
        .unwrap_err();
        assert!(err.to_string().contains("script"), "{err}");
        assert!(!dir.exists(), "全部拒绝后不得留下目录");
    }

    /// 库是真相源：文件集写失败（data/ 被同名文件占位）时上抛错误、不落库行，
    /// 预置文件原样保留（不回滚不删除，重试即整体覆盖）
    #[test]
    fn create_fs_failure_leaves_no_db_row_and_spares_existing_data() {
        let (tmp, conn) = temp_db();
        let dir = tmp.path().join("w");
        let (id, name, desc, icon, schema, script, _files, manual) = create_args("w");
        // 预置冲突：dir/data 是文件 → 附加文件 data/x.csv 的父目录创建必失败
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("data"), "占位文件").unwrap();
        let files = vec![("data/x.csv".to_string(), "a,b".to_string())];
        let err = create_app_core(
            &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, 1,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("File exists") || err.to_string().contains("失败"),
            "{err}"
        );
        assert!(
            db_apps::get_app(&conn, "w").unwrap().is_none(),
            "fs 失败不得落库行"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("data")).unwrap(),
            "占位文件"
        );
    }

    #[test]
    fn update_rewrites_script_and_flips_status_to_enabled() {
        let (tmp, conn) = temp_db();
        let dir = tmp.path().join("wage");
        let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
        create_app_core(
            &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, 1,
        )
        .unwrap();
        let next = update_app_core(
            &dir,
            &conn,
            "wage",
            Some("print('v2')"),
            &[],
            None,
            None,
            None,
            None,
            None,
            Some("enabled"),
            2,
        )
        .unwrap();
        assert_eq!(next.status, "enabled");
        assert_eq!(next.enabled_at, Some(2));
        assert_eq!(
            std::fs::read_to_string(dir.join("app.py")).unwrap(),
            "print('v2')"
        );
        // created_at 保留
        assert_eq!(next.created_at, 1);
    }

    #[test]
    fn archive_hides_app_from_program_page_and_unarchive_brings_it_back_as_draft() {
        let (_tmp, conn) = temp_db();
        let app = seed_enabled(&conn, "salary");
        // 归档：程序页列非归档，归档后不该在里面
        let archived = archive_core(&conn, &app.id, 2000).unwrap();
        assert_eq!(archived.status, mc::STATUS_ARCHIVED);
        let active_ids: Vec<String> = db_apps::list_active_apps(&conn)
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert!(
            !active_ids.contains(&app.id.to_string()),
            "归档后仍在程序页列表里"
        );
        let archived_ids: Vec<String> = db_apps::list_archived_apps(&conn)
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert!(
            archived_ids.contains(&app.id.to_string()),
            "已归档视图里查不到"
        );
        // 恢复：回到草稿（不是直接上架——重新上架得用户满意后再定）
        let back = unarchive_core(&conn, &app.id, 3000).unwrap();
        assert_eq!(back.status, mc::STATUS_DRAFT);
        assert_eq!(back.enabled_at, None, "恢复成草稿不该留着上架时间");
        assert!(db_apps::list_active_apps(&conn)
            .unwrap()
            .iter()
            .any(|a| a.id == app.id));
    }

    #[test]
    fn archive_is_idempotent_and_unarchive_rejects_non_archived() {
        let (_tmp, conn) = temp_db();
        let app = seed_enabled(&conn, "roster");
        archive_core(&conn, &app.id, 1000).unwrap();
        // 重复点归档：幂等返回，不报错；且是 no-op 不该动 updated_at
        //（动了会让「按最近更新排序」凭空跳一次）
        let again = archive_core(&conn, &app.id, 1500).unwrap();
        assert_eq!(again.status, mc::STATUS_ARCHIVED);
        assert_eq!(again.updated_at, 1000);
        // 未归档的程序不受理恢复（点了恢复但它本来就在页面上，是 bug 不是正常路径）
        let other = seed_enabled(&conn, "other");
        assert!(unarchive_core(&conn, &other.id, 2000).is_err());
        assert!(unarchive_core(&conn, "no-such-app", 2000).is_err());
    }

    /// 删干净是「库三张表 + 目录」全没，不是「列表里看不见了」。
    ///
    /// 三张表之间没有外键：只删 apps 主行的话，app_versions / app_runs 会留下
    /// 两堆永远没人再读的孤儿行（症状是库一直在涨、程序页却空空如也），
    /// 而目录留着更是白占磁盘——用户点删除就是冲着腾地方来的。
    #[test]
    fn delete_removes_all_three_tables_and_the_directory() {
        let (tmp, conn) = temp_db();
        let dir = tmp.path().join("wage");
        let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
        create_app_core(
            &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, 1,
        )
        .unwrap();
        db_apps::record_app_run(&conn, &id, 2, "ok").unwrap();
        std::fs::create_dir_all(dir.join("input")).unwrap();
        std::fs::write(dir.join("input").join("本月.xlsx"), b"x").unwrap();
        archive_core(&conn, &id, 3).unwrap();

        delete_core(&dir, &conn, &id).unwrap();

        assert!(!dir.exists(), "程序目录没删干净（用户要的腾地方没兑现）");
        for table in ["apps", "app_versions", "app_runs"] {
            let n: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE app_id = ?1"),
                    [&id],
                    |r| r.get(0),
                )
                .unwrap_or_else(|_| {
                    conn.query_row(
                        &format!("SELECT COUNT(*) FROM {table} WHERE id = ?1"),
                        [&id],
                        |r| r.get(0),
                    )
                    .unwrap()
                });
            assert_eq!(n, 0, "{table} 里还留着 {id} 的行（无外键，不会自动级联）");
        }
        assert!(db_apps::list_archived_apps(&conn).unwrap().is_empty());
    }

    /// 删除只对已归档程序开放——「先归档、再删除」这两步不能被一次点击跳过。
    ///
    /// 顺着跳过的话，程序页上那个垃圾桶就等于真删除，而那里的代码多半是 AI 写的；
    /// 归档这一步存在的全部意义就是当那道安全网。
    #[test]
    fn delete_rejects_non_archived_and_unknown_ids() {
        let (tmp, conn) = temp_db();
        let dir = tmp.path().join("roster");
        let app = seed_enabled(&conn, "roster");
        std::fs::create_dir_all(&dir).unwrap();

        let err = delete_core(&dir, &conn, &app.id).expect_err("未归档程序不得直接删");
        assert!(
            matches!(err, CommandError::DomainRule { .. }),
            "该走业务拒绝而不是 IO/未找到错误：{err:?}"
        );
        assert!(dir.exists(), "被拒的程序目录不该被动过");
        assert!(db_apps::get_app(&conn, &app.id).unwrap().is_some());

        assert!(delete_core(&dir, &conn, "ghost").is_err(), "不存在的 id");
    }

    /// 目录早就不在了也算删成功。
    ///
    /// 程序目录可能被用户手工清过、或从别处同步来一份只有元数据的行；
    /// 那正是我们要的终态。为此报错会让「删除」在真实数据上无故失败。
    #[test]
    fn delete_tolerates_an_already_missing_directory() {
        let (tmp, conn) = temp_db();
        let app = seed_enabled(&conn, "ghost-dir");
        archive_core(&conn, &app.id, 1).unwrap();
        let dir = tmp.path().join("never-existed");

        delete_core(&dir, &conn, &app.id).expect("目录缺失是目标状态，不是失败");
        assert!(db_apps::get_app(&conn, &app.id).unwrap().is_none());
    }

    #[test]
    fn update_requires_existing_and_rejects_archived_and_bad_status() {
        let (tmp, conn) = temp_db();
        let dir = tmp.path().join("wage");
        let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
        create_app_core(
            &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, 1,
        )
        .unwrap();
        // 不存在的 id
        let err = update_app_core(
            &dir,
            &conn,
            "ghost",
            Some("x"),
            &[],
            None,
            None,
            None,
            None,
            None,
            None,
            2,
        )
        .unwrap_err();
        assert!(matches!(err, CommandError::TaskNotFound(_)), "{err}");
        // archived 拒绝改
        conn.execute("UPDATE apps SET status='archived' WHERE id='wage'", [])
            .unwrap();
        let err = update_app_core(
            &dir,
            &conn,
            "wage",
            Some("x"),
            &[],
            None,
            None,
            None,
            None,
            None,
            None,
            2,
        )
        .unwrap_err();
        assert!(err.to_string().contains("已归档"), "{err}");
        // status=archived 从工具口拒绝
        conn.execute("UPDATE apps SET status='draft' WHERE id='wage'", [])
            .unwrap();
        let err = update_app_core(
            &dir,
            &conn,
            "wage",
            None,
            &[],
            None,
            None,
            None,
            None,
            None,
            Some("archived"),
            2,
        )
        .unwrap_err();
        assert!(err.to_string().contains("draft / enabled"), "{err}");
    }

    #[test]
    fn update_rejects_noop_and_bad_extra_paths() {
        let (tmp, conn) = temp_db();
        let dir = tmp.path().join("wage");
        let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
        create_app_core(
            &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, 1,
        )
        .unwrap();
        // 无任何变更
        let err = update_app_core(
            &dir,
            &conn,
            "wage",
            None,
            &[],
            None,
            None,
            None,
            None,
            None,
            None,
            2,
        )
        .unwrap_err();
        assert!(err.to_string().contains("没有任何变更"), "{err}");
        // 附加文件越出 data/
        let err = update_app_core(
            &dir,
            &conn,
            "wage",
            None,
            &[("input/steal.csv".to_string(), "x".to_string())],
            None,
            None,
            None,
            None,
            None,
            None,
            2,
        )
        .unwrap_err();
        assert!(err.to_string().contains("data/"), "{err}");
        // 穿越
        let err = update_app_core(
            &dir,
            &conn,
            "wage",
            None,
            &[("data/../../evil".to_string(), "x".to_string())],
            None,
            None,
            None,
            None,
            None,
            None,
            2,
        )
        .unwrap_err();
        assert!(err.to_string().contains("相对路径"), "{err}");
        // 合法 data/ 文件落盘
        update_app_core(
            &dir,
            &conn,
            "wage",
            None,
            &[("data/roster.csv".to_string(), "a,b\n1,2".to_string())],
            None,
            None,
            None,
            None,
            None,
            None,
            2,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("data/roster.csv")).unwrap(),
            "a,b\n1,2"
        );
    }

    // ── 版本快照 ──

    #[test]
    fn versions_captured_on_create_update_restore() {
        let (tmp, conn) = temp_db();
        let dir = tmp.path().join("wage");
        let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
        create_app_core(
            &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, 1,
        )
        .unwrap();
        let metas = db_apps::list_app_version_metas(&conn, "wage", 100).unwrap();
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].version, 1);
        assert_eq!(metas[0].source, "create");
        update_app_core(
            &dir,
            &conn,
            "wage",
            Some("print('v2')"),
            &[],
            None,
            None,
            None,
            None,
            None,
            Some("enabled"),
            2,
        )
        .unwrap();
        let metas = db_apps::list_app_version_metas(&conn, "wage", 100).unwrap();
        assert_eq!(metas.len(), 2);
        assert_eq!(metas[0].version, 2);
        assert_eq!(metas[0].source, "update");
        // 回滚到 v1：内容回旧版、生命周期（enabled）保留、本身记 v3
        restore_version_core(&dir, &conn, "wage", 1, 3).unwrap();
        let cur = db_apps::get_app(&conn, "wage").unwrap().unwrap();
        let v1 = db_apps::get_app_version(&conn, "wage", 1).unwrap().unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("app.py")).unwrap(),
            v1.script
        );
        assert_eq!(cur.name, v1.name);
        assert_eq!(cur.status, "enabled", "回滚内容不回滚生命周期");
        assert_eq!(cur.enabled_at, Some(2));
        let metas = db_apps::list_app_version_metas(&conn, "wage", 100).unwrap();
        assert_eq!(metas.len(), 3);
        assert_eq!(metas[0].version, 3);
        assert_eq!(metas[0].source, "restore");
        // 回滚后可再取全文
        let v3 = db_apps::get_app_version(&conn, "wage", 3).unwrap().unwrap();
        assert_eq!(v3.script, v1.script);
    }

    #[test]
    fn restore_rejects_missing_version_or_app() {
        let (tmp, conn) = temp_db();
        let dir = tmp.path().join("wage");
        let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
        create_app_core(
            &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, 1,
        )
        .unwrap();
        let err = restore_version_core(&dir, &conn, "wage", 99, 2).unwrap_err();
        assert!(matches!(err, CommandError::TaskNotFound(_)), "{err}");
        let err = restore_version_core(&dir, &conn, "ghost", 1, 2).unwrap_err();
        assert!(matches!(err, CommandError::TaskNotFound(_)), "{err}");
    }

    #[test]
    fn restore_rejects_archived_app() {
        // 归档 = 先别动它：回滚会整体重写脚本与 manifest，与 update 同拒；
        // 恢复（unarchive）后才能回滚
        let (tmp, conn) = temp_db();
        let dir = tmp.path().join("wage");
        let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
        create_app_core(
            &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, 1,
        )
        .unwrap();
        archive_core(&conn, "wage", 2).unwrap();
        let err = restore_version_core(&dir, &conn, "wage", 1, 3).unwrap_err();
        assert!(matches!(err, CommandError::DomainRule { .. }), "{err}");
        // 恢复后同一版本可回滚
        unarchive_core(&conn, "wage", 4).unwrap();
        restore_version_core(&dir, &conn, "wage", 1, 5).unwrap();
    }

    #[test]
    fn trim_keeps_recent_versions_and_version_number_keeps_growing() {
        let (tmp, conn) = temp_db();
        let dir = tmp.path().join("wage");
        let (id, name, desc, icon, schema, script, files, manual) = create_args("wage");
        create_app_core(
            &dir, &conn, &id, &name, &desc, &icon, &schema, &script, &files, &manual, 1,
        )
        .unwrap();
        for i in 2..=5 {
            update_app_core(
                &dir,
                &conn,
                "wage",
                Some(&format!("print('v{i}')")),
                &[],
                None,
                None,
                None,
                None,
                None,
                None,
                i,
            )
            .unwrap();
        }
        db_apps::trim_app_versions(&conn, "wage", 3).unwrap();
        let metas = db_apps::list_app_version_metas(&conn, "wage", 100).unwrap();
        let versions: Vec<i64> = metas.iter().map(|m| m.version).collect();
        assert_eq!(versions, vec![5, 4, 3], "滚动保留最近 3 版");
        // 下一版本号继续增长，不因裁剪回退
        assert_eq!(db_apps::next_app_version(&conn, "wage").unwrap(), 6);
    }

    // ── validate_input_schema ──

    #[test]
    fn input_schema_accepts_folder_and_valid_form() {
        assert!(validate_input_schema(r#"{"kind":"folder"}"#).is_ok());
        assert!(validate_input_schema(
            r#"{"kind":"form","fields":[{"key":"month","label":"月份","type":"date","required":true}]}"#
        )
        .is_ok());
        // 字段缺 type 默认 text
        assert!(validate_input_schema(r#"{"kind":"form","fields":[{"key":"a"}]}"#).is_ok());
    }

    #[test]
    fn input_schema_rejects_bad_kind_and_field_types_and_dup_keys() {
        assert!(validate_input_schema("not json").is_err());
        assert!(validate_input_schema(r#"{"kind":"wizard"}"#).is_err());
        assert!(validate_input_schema(r#"{"kind":"form"}"#).is_err());
        assert!(
            validate_input_schema(r#"{"kind":"form","fields":[{"key":"a","type":"blob"}]}"#)
                .is_err()
        );
        assert!(
            validate_input_schema(r#"{"kind":"form","fields":[{"key":"a"},{"key":"a"}]}"#).is_err()
        );
        assert!(validate_input_schema(r#"{"kind":"form","fields":[{"key":"  "}]}"#).is_err());
    }

    // ── normalize_input_schema ──

    #[test]
    fn normalize_defaults_to_folder_and_accepts_object_or_string() {
        assert_eq!(
            normalize_input_schema(None).unwrap(),
            r#"{"kind":"folder"}"#
        );
        let obj: serde_json::Value =
            serde_json::from_str(r#"{"kind":"form","fields":[{"key":"a","type":"number"}]}"#)
                .unwrap();
        assert!(normalize_input_schema(Some(&obj)).is_ok());
        let bad: serde_json::Value = serde_json::from_str(r#"{"kind":"x"}"#).unwrap();
        assert!(normalize_input_schema(Some(&bad)).is_err());
    }
}
