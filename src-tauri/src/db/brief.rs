//! 双层档案设计 docs/WORKFLOW-CLARIFY-AUDIT-DESIGN-2026-10-07.md §3.1）： audit-ok
//! 工作流决策摘要（task_id IS NULL，注入**每个**后续节点——跨卡跑偏的解药）
//! + 卡片档案（task_id 非空，本卡重跑时注入）。
//!
//! 红线：
//! - 条目只经代码路径写入（问答应答端 / 返工 / 手动编辑 / 澄清继承），agent 无自由写档案工具
//! - 注入有界：全局最近 6 条 ≤1200 字、卡最近 8 条 ≤800 字——档案是上下文，不是日志堆场
//! - 条目入库即 trim 截断，超长不报错（档案是增强，不挡主流程）

use serde::Serialize;

/// 条目文本上限（入库 trim 截断）
pub const MAX_ENTRY_TEXT: usize = 200;
/// 条目原因上限
pub const MAX_ENTRY_REASON: usize = 80;
/// 注入条数：工作流级最近 6 条
pub const GLOBAL_INJECT_ENTRIES: usize = 6;
/// 注入条数：卡片级最近 8 条
pub const CARD_INJECT_ENTRIES: usize = 8;
/// 注入字数：工作流级 ≤1200 字
pub const GLOBAL_INJECT_CHARS: usize = 1200;
/// 注入字数：卡片级 ≤800 字
pub const CARD_INJECT_CHARS: usize = 800;

pub const KIND_QA: &str = "qa";
pub const KIND_REWORK: &str = "rework";
pub const KIND_MANUAL: &str = "manual";
pub const KIND_CLARIFY: &str = "clarify";
pub const SOURCE_USER: &str = "user";
pub const SOURCE_AGENT: &str = "agent";
pub const SOURCE_QA: &str = "qa";
pub const SOURCE_SYSTEM: &str = "system";

/// 单源 DDL：open_db 与测试建表共用，防两处 schema 漂移
pub const BRIEF_DDL: &str = "CREATE TABLE IF NOT EXISTS brief_entries (
   id          INTEGER PRIMARY KEY AUTOINCREMENT,
   workflow_id TEXT NOT NULL,
   task_id     TEXT,
   kind        TEXT NOT NULL,
   source      TEXT NOT NULL,
   text        TEXT NOT NULL,
   reason      TEXT,
   created_at  INTEGER NOT NULL
 );
CREATE INDEX IF NOT EXISTS idx_brief_entries_wf
   ON brief_entries(workflow_id, task_id);";

/// 幂等建表（open_db 迁移链调用）
pub fn ensure_brief_entries(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(BRIEF_DDL).map_err(|e| e.to_string())
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BriefEntry {
    pub id: i64,
    pub workflow_id: String,
    pub task_id: Option<String>,
    pub kind: String,
    pub source: String,
    pub text: String,
    pub reason: Option<String>,
    pub created_at: i64,
}

fn chars_truncate(s: &str, cap: usize) -> String {
    s.chars().take(cap).collect()
}

/// 落一条档案（入库口，所有代码路径写入都走这里）：
/// text/reason trim 截断；text 空白拒绝（调用方保证语义，占位条目自己造文本）。
/// 返回条目 id。task_id=None = 工作流级（决策摘要层）。
pub fn brief_insert(
    conn: &rusqlite::Connection,
    workflow_id: &str,
    task_id: Option<&str>,
    kind: &str,
    source: &str,
    text: &str,
    reason: Option<&str>,
) -> Result<i64, String> {
    let text_trimmed = text.trim();
    if text_trimmed.is_empty() {
        return Err("档案条目文本不能为空".into());
    }
    conn.execute(
        "INSERT INTO brief_entries (workflow_id, task_id, kind, source, text, reason, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            workflow_id,
            task_id,
            kind,
            source,
            chars_truncate(text_trimmed, MAX_ENTRY_TEXT),
            reason
                .map(|r| chars_truncate(r.trim(), MAX_ENTRY_REASON))
                .filter(|r| !r.is_empty()),
            chrono::Utc::now().timestamp_millis()
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

fn row_to_entry(r: &rusqlite::Row) -> rusqlite::Result<BriefEntry> {
    Ok(BriefEntry {
        id: r.get(0)?,
        workflow_id: r.get(1)?,
        task_id: r.get(2)?,
        kind: r.get(3)?,
        source: r.get(4)?,
        text: r.get(5)?,
        reason: r.get(6)?,
        created_at: r.get(7)?,
    })
}

const ENTRY_COLS: &str = "id, workflow_id, task_id, kind, source, text, reason, created_at";

/// 工作流级条目（task_id IS NULL），新→旧
pub fn brief_list_global(
    conn: &rusqlite::Connection,
    workflow_id: &str,
) -> Result<Vec<BriefEntry>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {ENTRY_COLS} FROM brief_entries
             WHERE workflow_id = ?1 AND task_id IS NULL ORDER BY id DESC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([workflow_id], row_to_entry)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// 卡片级条目，新→旧
pub fn brief_list_card(
    conn: &rusqlite::Connection,
    task_id: &str,
) -> Result<Vec<BriefEntry>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {ENTRY_COLS} FROM brief_entries
             WHERE task_id = ?1 ORDER BY id DESC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([task_id], row_to_entry)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// 条目 → 注入行：`- 文本（因为：原因）`
fn entry_line(e: &BriefEntry) -> String {
    match &e.reason {
        Some(r) if !r.trim().is_empty() => format!("- {}（因为：{}）", e.text, r),
        _ => format!("- {}", e.text),
    }
}

/// 取最近 cap 条并按时间正序拼行（最新在末尾），累计字符超限即停（至少保留一条）
fn bounded_lines(entries: Vec<BriefEntry>, cap_entries: usize, cap_chars: usize) -> Vec<String> {
    let taken: Vec<BriefEntry> = entries.into_iter().take(cap_entries).collect();
    let mut lines: Vec<String> = Vec::with_capacity(taken.len());
    let mut used = 0usize;
    for e in taken.iter().rev() {
        let line = entry_line(e);
        let n = line.chars().count();
        if !lines.is_empty() && used + n > cap_chars {
            lines.push("（更多历史记录已省略）".into());
            break;
        }
        used += n;
        lines.push(line);
    }
    lines
}

/// 注入上下文：双层各一段；两层都空 → None（不注入空段稀释提示词）
#[derive(Debug, PartialEq, Clone)]
pub struct BriefContext {
    pub global: Option<String>,
    pub card: Option<String>,
}

const GLOBAL_HEADER: &str = "【工作流决策摘要——用户已确认的方向与约定，优先级高于任务卡原文】";
const CARD_HEADER: &str = "【本卡历史记录——这张卡之前发生过的事】";

/// 组装注入段（runner 装配 TaskExecCtx 时调用；纯查询，调用方放 spawn_blocking）。
/// task_id=None 时只查全局层。
pub fn brief_for_injection(
    conn: &rusqlite::Connection,
    workflow_id: &str,
    task_id: Option<&str>,
) -> Result<Option<BriefContext>, String> {
    let global_entries = brief_list_global(conn, workflow_id)?;
    let global = if global_entries.is_empty() {
        None
    } else {
        let lines = bounded_lines(global_entries, GLOBAL_INJECT_ENTRIES, GLOBAL_INJECT_CHARS);
        Some(format!("{GLOBAL_HEADER}\n{}", lines.join("\n")))
    };
    let card = match task_id {
        Some(tid) => {
            let card_entries = brief_list_card(conn, tid)?;
            if card_entries.is_empty() {
                None
            } else {
                let lines = bounded_lines(card_entries, CARD_INJECT_ENTRIES, CARD_INJECT_CHARS);
                Some(format!("{CARD_HEADER}\n{}", lines.join("\n")))
            }
        }
        None => None,
    };
    if global.is_none() && card.is_none() {
        return Ok(None);
    }
    Ok(Some(BriefContext { global, card }))
}

/// 工作流删除时级联清档案（两层都清——档案是工作流的附属，不独立存在）
pub fn brief_delete_workflow(
    conn: &rusqlite::Connection,
    workflow_id: &str,
) -> Result<usize, String> {
    conn.execute(
        "DELETE FROM brief_entries WHERE workflow_id = ?1",
        [workflow_id],
    )
    .map_err(|e| e.to_string())
}

/// 任务卡删除时清该卡条目（工作流级条目不动——它们属于整张工作流）
pub fn brief_delete_task(conn: &rusqlite::Connection, task_id: &str) -> Result<usize, String> {
    conn.execute("DELETE FROM brief_entries WHERE task_id = ?1", [task_id])
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        ensure_brief_entries(&conn).unwrap();
        conn
    }

    #[test]
    fn insert_trims_caps_and_rejects_blank() {
        let conn = mem_conn();
        let long = "长".repeat(MAX_ENTRY_TEXT + 10);
        let id = brief_insert(&conn, "wf1", None, KIND_QA, SOURCE_USER, &long, None).unwrap();
        let rows = brief_list_global(&conn, "wf1").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].text.chars().count(), MAX_ENTRY_TEXT);
        assert!(brief_insert(&conn, "wf1", None, KIND_QA, SOURCE_USER, "   ", None).is_err());
    }

    #[test]
    fn reason_truncated_and_blank_reason_dropped() {
        let conn = mem_conn();
        brief_insert(
            &conn,
            "wf1",
            Some("t1"),
            KIND_QA,
            SOURCE_USER,
            "答了",
            Some("  "), // 空白原因 → 不落 reason
        )
        .unwrap();
        let rows = brief_list_card(&conn, "t1").unwrap();
        assert!(rows[0].reason.is_none());
    }

    #[test]
    fn injection_layers_and_caps() {
        let conn = mem_conn();
        for i in 0..8 {
            brief_insert(
                &conn,
                "wf1",
                None,
                KIND_QA,
                SOURCE_USER,
                &format!("全局{i}"),
                None,
            )
            .unwrap();
        }
        for i in 0..3 {
            brief_insert(
                &conn,
                "wf1",
                Some("t1"),
                KIND_QA,
                SOURCE_USER,
                &format!("卡内{i}"),
                None,
            )
            .unwrap();
        }
        let ctx = brief_for_injection(&conn, "wf1", Some("t1"))
            .unwrap()
            .unwrap();
        let g = ctx.global.unwrap();
        assert!(g.starts_with(GLOBAL_HEADER));
        // 全局层取最近 6 条（共 8 条，全局0/1 被裁）、时间正序：全局2 在首、全局7 在末
        assert!(g.contains("- 全局2"));
        assert!(g.contains("- 全局7"));
        assert!(!g.contains("- 全局1"));
        assert!(!g.contains("- 全局0"));
        assert!(g.find("- 全局2").unwrap() < g.find("- 全局7").unwrap());
        // 卡层独立成段，混不进全局
        let c = ctx.card.unwrap();
        assert!(c.starts_with(CARD_HEADER));
        assert!(c.contains("- 卡内2"));
        assert!(!c.contains("全局"));
    }

    #[test]
    fn injection_char_cap_stops_early_but_keeps_one() {
        let conn = mem_conn();
        // 每条 ~250 字：前 3 条共 750 字 ≤ 800 上限，第 4 条加不进 → 省略提示
        let big = "字".repeat(250);
        for _ in 0..4 {
            brief_insert(&conn, "wf1", Some("t1"), KIND_QA, SOURCE_USER, &big, None).unwrap();
        }
        let ctx = brief_for_injection(&conn, "wf1", Some("t1"))
            .unwrap()
            .unwrap();
        let c = ctx.card.unwrap();
        assert_eq!(c.lines().count(), 5); // 头 + 3 条 + 省略提示
        assert!(c.contains("（更多历史记录已省略）"));
    }

    #[test]
    fn injection_none_when_no_entries() {
        let conn = mem_conn();
        assert!(brief_for_injection(&conn, "wf1", Some("t1"))
            .unwrap()
            .is_none());
        // 只有全局层时卡层为 None
        brief_insert(&conn, "wf1", None, KIND_QA, SOURCE_USER, "全局决策", None).unwrap();
        let ctx = brief_for_injection(&conn, "wf1", Some("t1"))
            .unwrap()
            .unwrap();
        assert!(ctx.global.is_some());
        assert!(ctx.card.is_none());
    }

    #[test]
    fn delete_workflow_cascades_both_layers_but_delete_task_keeps_global() {
        let conn = mem_conn();
        brief_insert(&conn, "wf1", None, KIND_QA, SOURCE_USER, "全局", None).unwrap();
        brief_insert(&conn, "wf1", Some("t1"), KIND_QA, SOURCE_USER, "卡内", None).unwrap();
        brief_insert(&conn, "wf2", None, KIND_QA, SOURCE_USER, "别家", None).unwrap();
        assert_eq!(brief_delete_task(&conn, "t1").unwrap(), 1);
        assert_eq!(brief_list_global(&conn, "wf1").unwrap().len(), 1);
        assert_eq!(brief_delete_workflow(&conn, "wf1").unwrap(), 1);
        assert!(brief_list_global(&conn, "wf1").unwrap().is_empty());
        assert_eq!(brief_list_global(&conn, "wf2").unwrap().len(), 1);
    }
}
