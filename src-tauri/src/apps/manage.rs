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

/// id 必须是合法相对路径分量（程序目录 `apps/<id>/`）。
/// 白名单而不是黑名单：`join` 只对「恰好一个普通分量」的 id 才不会走出
/// `apps/`——`.`（折叠回根）、空格、非 ASCII 都不在白名单内。
pub fn validate_app_id(id: &str) -> Result<(), CommandError> {
    let ok = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !ok {
        return Err(CommandError::InvalidArgument {
            field: "id".into(),
            value: id.to_string(),
            reason: "id 只能是字母/数字/`-`/`_`（单个目录分量）".into(),
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

/// 程序声明的单次运行时长（秒）：钳到全局天花板。
///
/// 口径（创建与更新共用）：
/// - `None` = 未声明（运行时用内置默认）
/// - `Some(0)` = 清除声明（回落默认；更新时「传 0」=「取消声明」）
/// - `Some(n)` = 声明 n 秒，超出天花板的部分钳掉（落盘值 = 生效值）
pub fn normalize_declared_timeout(v: Option<u64>) -> Option<u64> {
    match v {
        None | Some(0) => None,
        Some(n) => Some(crate::py::runtime::clamp_timeout_secs(n)),
    }
}

/// 程序声明值的钳制是否改动了原值（用于如实回报「你要的 7200s 按 1800s 生效」）
pub fn declared_timeout_clamped(raw: Option<u64>) -> bool {
    match raw {
        Some(n) if n > 0 => Some(n) != normalize_declared_timeout(Some(n)),
        _ => false,
    }
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
    timeout_secs: Option<u64>,
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
        timeout_secs: normalize_declared_timeout(timeout_secs),
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
        // 声明的运行时长**不进版本快照**：回滚回的是内容与 manifest，
        // 运行预算属于当前这台机器上的运行条件，回滚不该顺手改掉它
        timeout_secs: cur.timeout_secs,
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
    timeout_secs: Option<u64>,
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
        && timeout_secs.is_none()
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
    // timeoutSecs：未传 = 保持不变；传 0 = 取消声明（回落内置默认）；传 N = 声明 N 秒
    let timeout_secs = match timeout_secs {
        None => cur.timeout_secs,
        Some(0) => None,
        Some(n) => normalize_declared_timeout(Some(n)),
    };
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
        timeout_secs,
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

/// timeoutSecs 入参：缺省/null = 未传（创建=不声明，更新=保持不变）；
/// 传 0 = 取消声明；传 N = 声明 N 秒。非整数一律报错（不猜、不静默忽略）。
fn timeout_field(v: &serde_json::Value) -> Result<Option<u64>, CommandError> {
    match v.get("timeoutSecs") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(val) => val
            .as_u64()
            .map(Some)
            .ok_or_else(|| CommandError::InvalidArgument {
                field: "timeoutSecs".into(),
                value: val.to_string(),
                reason: "timeoutSecs 必须是整数秒（非负；0 = 取消声明）".into(),
            }),
    }
}

/// 声明值的如实描述（给工具结果用）：未声明 / N 秒 / 被钳后的 N 秒
fn describe_declared_timeout(declared: Option<u64>, requested: Option<u64>) -> String {
    match declared {
        None => format!(
            "未声明（单次运行按内置默认 {} 秒）",
            crate::py::runtime::DEFAULT_TIMEOUT_SECS
        ),
        Some(n) if declared_timeout_clamped(requested) => {
            format!(
                "{n} 秒（你要的 {} 秒超出上限，按上限生效）",
                requested.unwrap_or(n)
            )
        }
        Some(n) => format!("{n} 秒"),
    }
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
                // 声明的运行时长一并列出：模型得知道这个程序能跑多久，
                // 才能在建议用户「等着」时说得准
                let timeout = match a.timeout_secs {
                    Some(n) => format!("｜最长 {n} 秒"),
                    None => String::new(),
                };
                s.push_str(&format!(
                    "- {}｜{}｜{}｜{}{}\n",
                    a.id, a.name, status, desc, timeout
                ));
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
        return ToolResult::denied(
            "apps_create 缺少 id（程序目录名：字母/数字/-/_，路径分量，不能含 / 或 ..）",
        );
    };
    if let Err(e) = validate_app_id(&id) {
        return ToolResult::denied(format!("apps_create：{e}"));
    }
    let Some(name) = required_field(&v, "name") else {
        return ToolResult::denied("apps_create 缺少 name（程序页卡片标题）");
    };
    let Some(script) = v["script"].as_str().map(|s| s.to_string()) else {
        return ToolResult::denied(
            "apps_create 缺少 script（完整 app.py 源码全文，不许片段/diff）",
        );
    };
    let description = optional_field(&v, "description").unwrap_or_default();
    let icon = optional_field(&v, "icon").unwrap_or_else(|| "📦".into());
    let manual = optional_field(&v, "manual").unwrap_or_default();
    let input_schema = match normalize_input_schema(v.get("input_schema")) {
        Ok(s) => s,
        Err(e) => return ToolResult::denied(format!("apps_create：{e}")),
    };
    let files = match files_from_args(&v) {
        Ok(f) => f,
        Err(e) => return ToolResult::denied(format!("apps_create：{e}")),
    };
    let timeout_secs = match timeout_field(&v) {
        Ok(t) => t,
        Err(e) => return ToolResult::denied(format!("apps_create：{e}")),
    };
    crate::bot::audit_log(
        app,
        &format!(
            "apps_create | {} | files: {} timeout: {}",
            crate::bot::escape_for_log(&id, 100),
            files.len(),
            crate::bot::escape_for_log(
                &timeout_secs
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "-".into()),
                20
            ),
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
            timeout_secs,
            db::now_ms(),
        )
        .map_err(|e| e.to_string())
    })
    .await;
    match created {
        Ok(created) => {
            let name = created.name;
            let timeout_desc = describe_declared_timeout(created.timeout_secs, timeout_secs);
            ToolResult::ok(
                format!(
                    "程序「{name}」已创建为草稿，目录：{dir_log}。单次运行时长：{timeout_desc}。\n\
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
        return ToolResult::denied("apps_update_files 缺少 id（要修改的程序 id，先 apps_list 查）");
    };
    let script = v["script"].as_str().map(|s| s.to_string());
    let files = match files_from_args(&v) {
        Ok(f) => f,
        Err(e) => return ToolResult::denied(format!("apps_update_files：{e}")),
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
            Err(e) => return ToolResult::denied(format!("apps_update_files：{e}")),
        },
    };
    let status = optional_field(&v, "status");
    // timeoutSecs 缺省 = 保持不变；传 0 = 取消声明；传 N = 声明 N 秒
    let timeout_secs = match timeout_field(&v) {
        Ok(t) => t,
        Err(e) => return ToolResult::denied(format!("apps_update_files：{e}")),
    };
    crate::bot::audit_log(
        app,
        &format!(
            "apps_update_files | {} | script: {} files: {} status: {} timeout: {}",
            crate::bot::escape_for_log(&id, 100),
            script.is_some(),
            files.len(),
            crate::bot::escape_for_log(status.as_deref().unwrap_or("-"), 20),
            crate::bot::escape_for_log(
                &timeout_secs
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "-".into()),
                20
            ),
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
            timeout_secs,
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
            let timeout_desc = describe_declared_timeout(next.timeout_secs, timeout_secs);
            ToolResult::ok(
                format!(
                    "程序「{}」已更新：当前状态 {}，单次运行时长：{timeout_desc}。改了脚本记得让用户在程序页重新试跑。",
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
mod tests;
