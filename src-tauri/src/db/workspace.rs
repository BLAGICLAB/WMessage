//! 工作区链接 CRUD

use serde::{Deserialize, Serialize};
use tauri::async_runtime;
use tauri::AppHandle;

use crate::error::{CommandError, CommandResult};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLink {
    pub id: String,
    #[serde(alias = "label")]
    pub display_name: String,
    #[serde(alias = "target")]
    pub target_uri: String,
    pub kind: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceItem {
    pub id: String,
    pub title: String,
    pub collapsed: Option<bool>,
    pub links: Vec<WorkspaceLink>,
    pub order: Option<f64>,
    pub updated_at: Option<i64>,
}

pub fn load_workspace(conn: &rusqlite::Connection) -> Result<Vec<WorkspaceItem>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, title, collapsed, links, ord, updated_at
             FROM workspace_items
             ORDER BY ord IS NULL, ord",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<f64>>(4)?,
                r.get::<_, Option<i64>>(5)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut items = Vec::new();
    for row in rows {
        let (id, title, collapsed, links, order, updated_at) = row.map_err(|e| e.to_string())?;
        let links = match serde_json::from_str(&links) {
            Ok(l) => l,
            Err(_) => {
                eprintln!("[db] 工作区条目 {id} 的 links JSON 损坏，按空读取（原值未动）");
                Vec::new()
            }
        };
        items.push(WorkspaceItem {
            id,
            title,
            collapsed: collapsed.map(|v| v != 0),
            links,
            order,
            updated_at,
        });
    }
    Ok(items)
}

/// 允许的工作区链接 kind（写入侧白名单，fail-closed）。
/// 消费侧 `bot_skills::files::collect_openable_paths` 用同一集合语义（纵深防御）。
pub const ALLOWED_LINK_KINDS: [&str; 3] = ["url", "file", "folder"];

/// 写入侧校验：任意链接 kind 不在白名单 → 返回可辨识错误 `InvalidWorkspaceLinkKind`
/// （带 kind 值 + 来源 + 目标），**不静默丢、不降级 url**。fail-closed。
fn validate_link_kinds(items: &[WorkspaceItem], source: &str) -> Result<(), CommandError> {
    for it in items {
        for l in &it.links {
            if !ALLOWED_LINK_KINDS.contains(&l.kind.as_str()) {
                return Err(CommandError::InvalidWorkspaceLinkKind {
                    kind: l.kind.clone(),
                    source: source.to_string(),
                    link: l.target_uri.clone(),
                });
            }
        }
    }
    Ok(())
}

/// 写入工作区（校验后落库）。校验失败返回 `InvalidWorkspaceLinkKind`（fail-closed）。
pub fn upsert_workspace(
    conn: &mut rusqlite::Connection,
    items: &[WorkspaceItem],
) -> Result<(), CommandError> {
    validate_link_kinds(items, "workspace_upsert")?;
    upsert_workspace_unchecked(conn, items).map_err(CommandError::from)
}

fn upsert_workspace_unchecked(
    conn: &mut rusqlite::Connection,
    items: &[WorkspaceItem],
) -> Result<(), String> {
    if items.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO workspace_items (id, title, collapsed, links, ord, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(id) DO UPDATE SET
                   title=excluded.title, collapsed=excluded.collapsed,
                   links=excluded.links, ord=excluded.ord,
                   updated_at=excluded.updated_at",
            )
            .map_err(|e| e.to_string())?;
        for it in items {
            stmt.execute(rusqlite::params![
                it.id,
                it.title,
                it.collapsed.map(|v| v as i64),
                serde_json::to_string(&it.links).unwrap_or_else(|_| "[]".into()),
                it.order,
                it.updated_at,
            ])
            .map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())
}

pub fn delete_workspace(conn: &mut rusqlite::Connection, ids: &[String]) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    {
        let mut stmt = tx
            .prepare("DELETE FROM workspace_items WHERE id = ?1")
            .map_err(|e| e.to_string())?;
        for id in ids {
            stmt.execute([id]).map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn workspace_load(app: AppHandle) -> CommandResult<Vec<WorkspaceItem>> {
    async_runtime::spawn_blocking(move || {
        let conn = super::open_db(&app)?;
        load_workspace(&conn).map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::from(format!("工作区读取线程 join 失败：{e}")))?
}

#[tauri::command]
pub async fn workspace_upsert(app: AppHandle, items: Vec<WorkspaceItem>) -> CommandResult<()> {
    async_runtime::spawn_blocking(move || {
        if items.is_empty() {
            return Ok(());
        }
        let _g = super::DB_WRITE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut conn = super::open_db(&app)?;
        upsert_workspace(&mut conn, &items).map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::from(format!("工作区写入线程 join 失败：{e}")))?
}

#[tauri::command]
pub async fn workspace_delete(app: AppHandle, ids: Vec<String>) -> CommandResult<()> {
    async_runtime::spawn_blocking(move || {
        if ids.is_empty() {
            return Ok(());
        }
        let _g = super::DB_WRITE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut conn = super::open_db(&app)?;
        delete_workspace(&mut conn, &ids).map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::from(format!("工作区删除线程 join 失败：{e}")))?
}

#[tauri::command]
pub async fn workspace_export(app: AppHandle, path: String) -> CommandResult<usize> {
    super::tasks::check_export_path(&path)?;
    async_runtime::spawn_blocking(move || {
        let conn = super::open_db(&app)?;
        let items = load_workspace(&conn)?;
        let json = serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?;
        super::paths::atomic_write(std::path::Path::new(&path), &json)
            .map_err(|e| format!("写入文件失败：{e}"))?;
        Ok(items.len())
    })
    .await
    .map_err(|e| CommandError::from(format!("工作区导出线程 join 失败：{e}")))?
}

/// 导入合并（校验后落库）。校验失败返回 `InvalidWorkspaceLinkKind`（fail-closed）。
pub fn workspace_import_merge(
    conn: &mut rusqlite::Connection,
    ext: &[WorkspaceItem],
) -> Result<usize, CommandError> {
    validate_link_kinds(ext, "workspace_import_merge")?;
    workspace_import_merge_unchecked(conn, ext).map_err(CommandError::from)
}

fn workspace_import_merge_unchecked(
    conn: &mut rusqlite::Connection,
    ext: &[WorkspaceItem],
) -> Result<usize, String> {
    use rusqlite::OptionalExtension;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut merged = 0usize;
    for it in ext {
        if it.id.trim().is_empty() {
            continue;
        }
        let cur: Option<Option<i64>> = tx
            .query_row(
                "SELECT updated_at FROM workspace_items WHERE id = ?1",
                rusqlite::params![it.id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let cur_ua = cur.flatten().unwrap_or(0);
        let take = it.updated_at.unwrap_or(0) > cur_ua;
        if take {
            tx.execute(
                "INSERT INTO workspace_items (id, title, collapsed, links, ord, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(id) DO UPDATE SET
                   title=excluded.title, collapsed=excluded.collapsed,
                   links=excluded.links, ord=excluded.ord,
                   updated_at=excluded.updated_at",
                rusqlite::params![
                    it.id,
                    it.title,
                    it.collapsed.map(|v| v as i64),
                    serde_json::to_string(&it.links).unwrap_or_else(|_| "[]".into()),
                    it.order,
                    it.updated_at,
                ],
            )
            .map_err(|e| e.to_string())?;
            merged += 1;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(merged)
}

#[tauri::command]
pub async fn workspace_import(app: AppHandle, path: String) -> CommandResult<usize> {
    async_runtime::spawn_blocking(move || {
        let raw = std::fs::read_to_string(&path).map_err(|e| format!("无法读取所选文件：{e}"))?;
        let ext: Vec<WorkspaceItem> =
            serde_json::from_str(&raw).map_err(|e| format!("不是有效的工作区链接 JSON：{e}"))?;
        if ext.is_empty() {
            return Ok(0);
        }
        let _g = super::DB_WRITE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut conn = super::open_db(&app)?;
        workspace_import_merge(&mut conn, &ext).map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::from(format!("工作区导入线程 join 失败：{e}")))?
}
