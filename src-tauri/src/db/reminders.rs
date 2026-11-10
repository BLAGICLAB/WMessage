//! 提醒（机器人提醒）存储：**到点说一句话**，不建卡、不执行任何内容。
//!
//! 与内容型定时作业（`schedule_jobs`）的本质区别：作业到点要**做事**（新建
//! 任务卡交给机器人执行），提醒到点只**说话**（发一条系统通知「该 X 了」）。
//! 两者共用定时串格式（daily:/weekly:/monthly:/at:）与触发点计算
//! （`bot_scheduler::occurrence_after`），但存储与触发路径各走各的——提醒不进
//! 执行历史、不占重试与自动暂停语义。
//!
//! 写路径：新建/编辑走 upsert（带 RMW 基线——提醒有了 UI 编辑入口，改内容与
//! 改时间会互相覆盖，基线不匹配即拒写）；暂停/恢复是单列操作，走定点
//! UPDATE，不带基线（携带整行快照反而会让「界面显示旧值 → 用户改内容 →
//! 把 enabled 覆盖回去」成为可能）。

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// 单源 DDL：open_db 与测试建表共用
pub const REMINDERS_DDL: &str = "CREATE TABLE IF NOT EXISTS reminders (
   id          TEXT PRIMARY KEY,
   text        TEXT NOT NULL,
   schedule    TEXT NOT NULL,
   remind_at   INTEGER,
   enabled     INTEGER,
   created_at  INTEGER,
   updated_at  INTEGER,
   fired_at    INTEGER,
   fired_count INTEGER
 );";

/// 提醒内容长度上限（一句话提醒）
pub const MAX_REMINDER_TEXT: usize = 200;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Reminder {
    pub id: String,
    /// 提醒内容（到点原样通知用户）
    pub text: String,
    /// 规范定时串：daily:HH:MM / weekly:D:HH:MM / monthly:DD:HH:MM /
    /// at:YYYY-MM-DDTHH:MM（与内容型作业同格式，触发点计算共用一套）
    pub schedule: String,
    /// 下次触发时间（epoch ms）；None = 无未来触发点（该行会被清理）
    pub remind_at: Option<i64>,
    /// 定时启用开关：None/Some(true) = 启用，Some(false) = 暂停（保留配置）
    #[serde(default)]
    pub enabled: Option<bool>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    /// 最近一次触发时间（epoch ms）
    #[serde(default)]
    pub fired_at: Option<i64>,
    /// 已触发次数
    #[serde(default)]
    pub fired_count: Option<i64>,
    /// RMW 基线（并发编辑防护；写入口用，不序列化给前端）
    #[serde(default, skip_serializing)]
    pub expected_updated_at: Option<i64>,
}

impl Reminder {
    pub fn enabled(&self) -> bool {
        self.enabled != Some(false)
    }
    /// 是否一次性提醒（at: 触发后即删除）
    pub fn one_shot(&self) -> bool {
        self.schedule.trim_start().starts_with("at:")
    }
}

const REMINDER_COLS: &str = "SELECT id, text, schedule, remind_at, enabled, created_at, \
     updated_at, fired_at, fired_count FROM reminders";

fn reminder_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Reminder> {
    Ok(Reminder {
        id: r.get(0)?,
        text: r.get(1)?,
        schedule: r.get(2)?,
        remind_at: r.get(3)?,
        enabled: r.get::<_, Option<i64>>(4)?.map(|v| v != 0),
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
        fired_at: r.get(7)?,
        fired_count: r.get(8)?,
        expected_updated_at: None,
    })
}

/// 全部提醒（按下次触发时间升序：先到点的排前面）
pub fn load_reminders(conn: &rusqlite::Connection) -> Result<Vec<Reminder>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "{REMINDER_COLS} ORDER BY COALESCE(remind_at, 0) ASC, created_at ASC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], reminder_from_row)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 到点待触发的提醒（调度器每轮取；只取 enabled 且 remind_at 已到点的）
pub fn load_due_reminders(
    conn: &rusqlite::Connection,
    now_ms: i64,
) -> Result<Vec<Reminder>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "{REMINDER_COLS} WHERE enabled IS NOT 0 AND remind_at IS NOT NULL \
             AND remind_at <= ?1 ORDER BY remind_at ASC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([now_ms], reminder_from_row)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

pub fn load_reminder(conn: &rusqlite::Connection, id: &str) -> Result<Option<Reminder>, String> {
    conn.query_row(
        &format!("{REMINDER_COLS} WHERE id = ?1"),
        [id],
        reminder_from_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// 新建/覆盖写（调用方持 lock_db_write；RMW 基线检查同 upsert_tasks）
///
/// 基线不匹配（行已被其他写者改/删）拒绝写，防并发编辑互相覆盖；
/// 基线为 None 时退化为纯新建（机器人工具建提醒走这条）。
pub fn upsert_reminder(conn: &rusqlite::Connection, r: &Reminder) -> Result<(), String> {
    debug_assert!(
        super::holding_db_write(),
        "upsert_reminder 必须在持有 DB_WRITE_LOCK（lock_db_write()）时调用"
    );
    if let Some(expected) = r.expected_updated_at {
        let cur: Option<Option<i64>> = conn
            .query_row(
                "SELECT updated_at FROM reminders WHERE id = ?1",
                [&r.id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let conflict = super::tasks::rmw_conflict(expected, cur);
        if conflict {
            return Err(format!(
                "{}：提醒 {} 读快照后已被其他写者修改，写回被拒",
                super::tasks::CONFLICT_ERR_PREFIX,
                r.id
            ));
        }
    }
    conn.execute(
        "INSERT INTO reminders (id, text, schedule, remind_at, enabled, created_at, updated_at, fired_at, fired_count)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
         ON CONFLICT(id) DO UPDATE SET
           text=excluded.text, schedule=excluded.schedule,
           remind_at=excluded.remind_at, enabled=excluded.enabled,
           fired_at=excluded.fired_at, fired_count=excluded.fired_count,
           updated_at=excluded.updated_at",
        rusqlite::params![
            r.id,
            r.text,
            r.schedule,
            r.remind_at,
            r.enabled.map(|b| b as i64),
            r.created_at,
            r.updated_at,
            r.fired_at,
            r.fired_count,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 暂停/恢复提醒（定点更新：只动 enabled + updated_at）
///
/// 返回是否命中行（false = 行不存在）。
pub fn set_reminder_enabled(
    conn: &rusqlite::Connection,
    id: &str,
    enabled: bool,
    updated_at: i64,
) -> Result<bool, String> {
    let n = conn
        .execute(
            "UPDATE reminders SET enabled = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![enabled as i64, updated_at, id],
        )
        .map_err(|e| e.to_string())?;
    Ok(n > 0)
}

/// 触发后推进：写最近触发时间 + 下次触发时间（remind_at = None 的行交给调用方删除）
pub fn advance_reminder(
    conn: &rusqlite::Connection,
    id: &str,
    fired_at: i64,
    next_at: Option<i64>,
) -> Result<(), String> {
    conn.execute(
        "UPDATE reminders SET fired_at = ?1, fired_count = COALESCE(fired_count, 0) + 1, \
           remind_at = ?2, updated_at = ?3 WHERE id = ?4",
        rusqlite::params![fired_at, next_at, fired_at, id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 删除提醒（一次性提醒触发后 / 用户取消）
pub fn delete_reminder(conn: &rusqlite::Connection, id: &str) -> Result<bool, String> {
    let n = conn
        .execute("DELETE FROM reminders WHERE id = ?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(n > 0)
}

/// 提醒内容校验（纯函数，可测）
///
/// 返回裁好的内容；错误是给机器人工具读的纯文案（不进 CommandError——
/// 提醒没有设置页入口，只有工具层一个调用方）。
pub fn validate_reminder_text(text: &str) -> Result<String, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("提醒内容不能为空".into());
    }
    if t.chars().count() > MAX_REMINDER_TEXT {
        return Err(format!("提醒内容超过 {MAX_REMINDER_TEXT} 字上限"));
    }
    Ok(t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(REMINDERS_DDL).unwrap();
        conn
    }

    fn sample(id: &str, remind_at: Option<i64>) -> Reminder {
        Reminder {
            id: id.into(),
            text: "喝水".into(),
            schedule: "daily:09:00".into(),
            remind_at,
            enabled: None,
            created_at: Some(1),
            updated_at: Some(1),
            fired_at: None,
            fired_count: None,
            expected_updated_at: None,
        }
    }

    #[test]
    fn upsert_and_load_roundtrip() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        upsert_reminder(&conn, &sample("r1", Some(1_000))).unwrap();
        let all = load_reminders(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].text, "喝水");
        assert!(all[0].enabled(), "None = 启用");
        assert!(!all[0].one_shot(), "daily 不是一次性");
    }

    #[test]
    fn due_reminders_only_returns_reached_enabled_rows() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        upsert_reminder(&conn, &sample("due", Some(500))).unwrap();
        upsert_reminder(&conn, &sample("later", Some(5_000))).unwrap();
        let mut paused = sample("paused", Some(500));
        paused.enabled = Some(false);
        upsert_reminder(&conn, &paused).unwrap();
        let due = load_due_reminders(&conn, 1_000).unwrap();
        assert_eq!(due.len(), 1, "只有到点且启用的行");
        assert_eq!(due[0].id, "due");
    }

    #[test]
    fn advance_moves_next_fire_and_counts() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        upsert_reminder(&conn, &sample("r1", Some(500))).unwrap();
        advance_reminder(&conn, "r1", 600, Some(900)).unwrap();
        let row = load_reminder(&conn, "r1").unwrap().unwrap();
        assert_eq!(row.remind_at, Some(900));
        assert_eq!(row.fired_at, Some(600));
        assert_eq!(row.fired_count, Some(1));
    }

    #[test]
    fn delete_reminder_reports_whether_row_existed() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        upsert_reminder(&conn, &sample("r1", Some(1))).unwrap();
        assert!(delete_reminder(&conn, "r1").unwrap());
        assert!(!delete_reminder(&conn, "r1").unwrap(), "重复删除返回 false");
    }

    #[test]
    fn validate_text_rejects_blank_and_over_limit() {
        assert!(validate_reminder_text("   ").is_err(), "空白拒绝");
        let big = "字".repeat(MAX_REMINDER_TEXT + 1);
        assert!(validate_reminder_text(&big).is_err(), "超长拒绝");
        assert_eq!(
            validate_reminder_text(" 喝水 ").unwrap(),
            "喝水",
            "两端空白裁掉"
        );
    }

    // ── 暂停/恢复：单列定点更新 ──

    #[test]
    fn set_enabled_toggles_flag_and_reports_missing_row() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        upsert_reminder(&conn, &sample("r1", Some(500))).unwrap();
        assert!(set_reminder_enabled(&conn, "r1", false, 900).unwrap());
        let row = load_reminder(&conn, "r1").unwrap().unwrap();
        assert_eq!(row.enabled, Some(false), "暂停位写进去了");
        assert!(!row.enabled(), "enabled=false 即暂停");
        assert_eq!(row.updated_at, Some(900), "updated_at 跟着动");
        // 恢复
        assert!(set_reminder_enabled(&conn, "r1", true, 901).unwrap());
        assert!(load_reminder(&conn, "r1").unwrap().unwrap().enabled());
        // 行不存在
        assert!(
            !set_reminder_enabled(&conn, "nope", false, 902).unwrap(),
            "行不存在返回 false（不是静默成功）"
        );
    }

    /// 暂停的全部价值就在这一条：调度器按 enabled 过滤，暂停即不再到点触发。
    #[test]
    fn paused_reminder_is_never_due() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        upsert_reminder(&conn, &sample("r1", Some(500))).unwrap();
        assert_eq!(load_due_reminders(&conn, 1_000).unwrap().len(), 1);
        set_reminder_enabled(&conn, "r1", false, 900).unwrap();
        assert!(
            load_due_reminders(&conn, 1_000).unwrap().is_empty(),
            "暂停后不再进入待触发清单"
        );
    }

    // ── RMW 基线：并发编辑防护 ──

    #[test]
    fn upsert_rejects_stale_baseline() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        upsert_reminder(&conn, &sample("r1", Some(500))).unwrap();
        // 读快照（基线 = updated_at = 1）
        let mut snapshot = load_reminder(&conn, "r1").unwrap().unwrap();
        assert_eq!(snapshot.updated_at, Some(1));
        // 期间调度器推进了这条提醒（updated_at 变了）
        advance_reminder(&conn, "r1", 600, Some(900)).unwrap();
        // 拿旧快照写回 → 拒
        snapshot.text = "改内容".into();
        snapshot.expected_updated_at = snapshot.updated_at;
        snapshot.updated_at = Some(700);
        let err = upsert_reminder(&conn, &snapshot).unwrap_err();
        assert!(
            err.contains("写冲突"),
            "基线不匹配必须报写冲突，实际：{err}"
        );
        // 行内容未被旧快照覆盖
        let cur = load_reminder(&conn, "r1").unwrap().unwrap();
        assert_eq!(cur.text, "喝水", "被拒的写回不得改内容");
        assert_eq!(cur.fired_count, Some(1), "触发计数不受影响");
    }

    #[test]
    fn upsert_edit_preserves_trigger_counters() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        upsert_reminder(&conn, &sample("r1", Some(500))).unwrap();
        advance_reminder(&conn, "r1", 600, Some(900)).unwrap();
        // 编辑：改内容，基线取快照的 updated_at
        let mut row = load_reminder(&conn, "r1").unwrap().unwrap();
        row.text = "改内容".into();
        row.expected_updated_at = row.updated_at;
        row.updated_at = Some(700);
        upsert_reminder(&conn, &row).unwrap();
        let cur = load_reminder(&conn, "r1").unwrap().unwrap();
        assert_eq!(cur.text, "改内容");
        assert_eq!(cur.fired_count, Some(1), "编辑不得清掉触发计数");
        assert_eq!(cur.fired_at, Some(600), "编辑不得清掉最近触发时间");
        assert_eq!(cur.remind_at, Some(900), "编辑不得改下次触发时间");
    }
}
