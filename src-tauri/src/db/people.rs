//! 成员注册表（任务图谱设计 §1.2）：多人任务卡汇总的归属人字典。
//! 本人 personId 由 profile::ensure_person_id 生成并在此表留 is_self=1 行；
//! 外来成员随导入信封 upsert（最新 name 覆盖）。头像列预留（v1 信封不携带）。

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::error::CommandResult;

pub const PEOPLE_DDL: &str = "CREATE TABLE IF NOT EXISTS people (
   id         TEXT PRIMARY KEY,
   name       TEXT NOT NULL,
   avatar     TEXT,
   is_self    INTEGER NOT NULL DEFAULT 0,
   updated_at INTEGER
 );";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PeopleEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub is_self: bool,
}

/// 信封里流转的最小成员卡（无 avatar / is_self；设计 §1.4）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PeopleCard {
    pub id: String,
    pub name: String,
}

pub fn people_load(conn: &rusqlite::Connection) -> Result<Vec<PeopleEntry>, String> {
    let mut stmt = conn
        .prepare("SELECT id, name, avatar, is_self FROM people ORDER BY is_self DESC, name")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(PeopleEntry {
                id: r.get(0)?,
                name: r.get(1)?,
                avatar: r.get(2)?,
                is_self: r.get::<_, i64>(3)? != 0,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 单条 upsert（存在则刷 name/avatar + 时间戳；is_self 只升不降——普通 upsert
/// 不允许把本人行降级为成员）。调用方负责事务/写锁。
pub fn people_upsert_entry(
    conn: &rusqlite::Connection,
    id: &str,
    name: &str,
    is_self: bool,
) -> Result<(), String> {
    let name = name.trim();
    if id.trim().is_empty() || name.is_empty() {
        return Ok(()); // 空卡防御：不写入也不报错（信封内容不可信任）
    }
    conn.execute(
        "INSERT INTO people (id, name, is_self, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET
           name=excluded.name, updated_at=excluded.updated_at,
           is_self=CASE WHEN people.is_self THEN 1 ELSE excluded.is_self END",
        rusqlite::params![
            id,
            name,
            is_self as i64,
            chrono::Utc::now().timestamp_millis()
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 批量 upsert 信封成员卡（is_self 恒 false；本人行由 ensure_person_id 单独落）
pub fn people_upsert_cards(
    conn: &rusqlite::Connection,
    cards: &[PeopleCard],
) -> Result<(), String> {
    for c in cards {
        people_upsert_entry(conn, &c.id, &c.name, false)?;
    }
    Ok(())
}

/// 导入时兜底：任务引用了信封未携带资料的 personId → 占位行，图谱不出现悬空归属。
/// 已知 id（含本人行——ensure_person_id 落库）不覆盖。
pub fn people_ensure_placeholder(conn: &rusqlite::Connection, id: &str) -> Result<(), String> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM people WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    if count == 0 {
        people_upsert_entry(conn, id, "未知成员", false)?;
    }
    Ok(())
}

#[tauri::command]
pub fn people_list(app: AppHandle) -> CommandResult<Vec<PeopleEntry>> {
    let conn = super::open_db(&app)?;
    let mut list = people_load(&conn)?;
    // 本人行 name 用 profile 现值覆盖（profile_set_name 不回写 people，单一事实源在 profile）
    if let Some(self_name) = crate::profile::self_display_name(&app) {
        for e in list.iter_mut() {
            if e.is_self {
                e.name = self_name.clone();
            }
        }
    }
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(PEOPLE_DDL).unwrap();
        conn
    }

    #[test]
    fn people_upsert_inserts_and_updates_name() {
        let conn = setup_conn();
        people_upsert_entry(&conn, "p1", "张三", false).unwrap();
        // 最新名覆盖
        people_upsert_entry(&conn, "p1", "张三丰", false).unwrap();
        let list = people_load(&conn).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "张三丰");
        assert!(!list[0].is_self);
    }

    #[test]
    fn people_upsert_never_demotes_self_row() {
        let conn = setup_conn();
        people_upsert_entry(&conn, "me", "我", true).unwrap();
        // 信封里捎带了本人的卡（is_self=false）→ 不得把本人行降级
        people_upsert_entry(&conn, "me", "我", false).unwrap();
        let list = people_load(&conn).unwrap();
        assert!(list[0].is_self, "is_self 只升不降");
    }

    #[test]
    fn people_upsert_rejects_blank_id_or_name() {
        let conn = setup_conn();
        people_upsert_entry(&conn, "", "张三", false).unwrap();
        people_upsert_entry(&conn, "p1", "   ", false).unwrap();
        assert!(people_load(&conn).unwrap().is_empty(), "空卡不写入");
    }

    #[test]
    fn people_cards_batch_and_placeholder_fill() {
        let conn = setup_conn();
        people_upsert_cards(
            &conn,
            &[PeopleCard {
                id: "a".into(),
                name: "李四".into(),
            }],
        )
        .unwrap();
        // 任务引用了信封未携带的 pid → 占位；已知 pid 不覆盖
        people_ensure_placeholder(&conn, "b").unwrap();
        people_ensure_placeholder(&conn, "a").unwrap();
        let list = people_load(&conn).unwrap();
        assert_eq!(list.len(), 2);
        let b = list.iter().find(|e| e.id == "b").unwrap();
        assert_eq!(b.name, "未知成员");
        let a = list.iter().find(|e| e.id == "a").unwrap();
        assert_eq!(a.name, "李四", "已知成员不被占位名覆盖");
    }

    #[test]
    fn people_load_orders_self_first() {
        let conn = setup_conn();
        people_upsert_entry(&conn, "z", "张三", false).unwrap();
        people_upsert_entry(&conn, "me", "我", true).unwrap();
        let list = people_load(&conn).unwrap();
        assert_eq!(list[0].id, "me", "本人排最前（图例首列）");
    }
}
