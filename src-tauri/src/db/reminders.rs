//! 提醒（机器人提醒）存储：**到点说一句话**，不建卡、不执行任何内容。
//!
//! 与内容型定时作业（`schedule_jobs`）的本质区别：作业到点要**做事**（新建
//! 任务卡交给机器人执行），提醒到点只**说话**（发一条系统通知「该 X 了」）。
//! 两者共用定时串格式（daily:/weekly:/monthly:/at:）与触发点计算
//! （`bot_scheduler::occurrence_after`），但存储与触发路径各走各的——提醒不进
//! 执行历史、不占重试与自动暂停语义。
//!
//! 不做 RMW 基线：提醒只由机器人工具建/取消，没有并发编辑入口（RMW 是给
//! 「UI 边看边改」的行准备的，字段加了没人用就是死字段）。

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::error::{CommandError, CommandResult};

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

/// 新建提醒（调用方持 lock_db_write）
pub fn insert_reminder(conn: &rusqlite::Connection, r: &Reminder) -> Result<(), String> {
    debug_assert!(
        super::holding_db_write(),
        "insert_reminder 必须在持有 DB_WRITE_LOCK（lock_db_write()）时调用"
    );
    conn.execute(
        "INSERT INTO reminders (id, text, schedule, remind_at, enabled, created_at, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        rusqlite::params![
            r.id,
            r.text,
            r.schedule,
            r.remind_at,
            r.enabled.map(|b| b as i64),
            r.created_at,
            r.updated_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
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

// Tauri commands

/// 提醒内容校验（纯函数，可测）
pub fn validate_reminder_text(text: &str) -> CommandResult<String> {
    let t = text.trim();
    if t.is_empty() {
        return Err(CommandError::InvalidArgument {
            field: "text".into(),
            value: text.to_string(),
            reason: "提醒内容不能为空".into(),
        });
    }
    if t.chars().count() > MAX_REMINDER_TEXT {
        return Err(CommandError::InvalidArgument {
            field: "text".into(),
            value: text.to_string(),
            reason: format!("提醒内容超过 {MAX_REMINDER_TEXT} 字上限"),
        });
    }
    Ok(t.to_string())
}

/// 新建提醒（设置页「新建提醒」入口，与机器人工具同一条写路径）
#[tauri::command]
pub async fn reminder_create(
    app: AppHandle,
    text: String,
    schedule: String,
) -> CommandResult<Reminder> {
    let text = validate_reminder_text(&text)?;
    crate::bot_scheduler::validate_schedule(schedule.trim()).map_err(|reason| {
        CommandError::InvalidArgument {
            field: "schedule".into(),
            value: schedule.clone(),
            reason,
        }
    })?;
    let app_emit = app.clone();
    let row = tauri::async_runtime::spawn_blocking(move || -> CommandResult<Reminder> {
        let _g = super::lock_db_write();
        let conn = super::open_db(&app)?;
        let now = chrono::Utc::now().timestamp_millis();
        let remind_at = crate::bot_reminders::first_fire_at(schedule.trim())
            .map(|t| t.timestamp_millis())
            .ok_or(CommandError::InvalidArgument {
                field: "schedule".into(),
                value: schedule.trim().to_string(),
                reason: "触发时间已过或无效".into(),
            })?;
        let r = Reminder {
            id: uuid::Uuid::new_v4().simple().to_string(),
            text,
            schedule: schedule.trim().to_string(),
            remind_at: Some(remind_at),
            enabled: None,
            created_at: Some(now),
            updated_at: Some(now),
            fired_at: None,
            fired_count: None,
        };
        insert_reminder(&conn, &r).map_err(CommandError::from)?;
        Ok(r)
    })
    .await
    .map_err(|e| CommandError::from(format!("提醒创建线程 join 失败：{e}")))??;
    crate::audit::write_event(
        &app_emit,
        crate::audit::AuditLevel::Info,
        "reminder_create",
        &[
            ("reminderId", row.id.clone()),
            ("schedule", row.schedule.clone()),
        ],
    );
    Ok(row)
}

/// 提醒列表
#[tauri::command]
pub async fn reminder_list(app: AppHandle) -> CommandResult<Vec<Reminder>> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = super::open_db(&app)?;
        load_reminders(&conn).map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::from(format!("提醒列表线程 join 失败：{e}")))?
}

/// 删除提醒（取消提醒）
#[tauri::command]
pub async fn reminder_delete(app: AppHandle, id: String) -> CommandResult<bool> {
    let app_emit = app.clone();
    let id_for_audit = id.clone();
    let ok = tauri::async_runtime::spawn_blocking(move || -> CommandResult<bool> {
        let _g = super::lock_db_write();
        let conn = super::open_db(&app)?;
        delete_reminder(&conn, &id).map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::from(format!("提醒删除线程 join 失败：{e}")))??;
    crate::audit::write_event(
        &app_emit,
        crate::audit::AuditLevel::Info,
        "reminder_delete",
        &[("reminderId", id_for_audit)],
    );
    Ok(ok)
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
        }
    }

    #[test]
    fn insert_and_load_roundtrip() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        insert_reminder(&conn, &sample("r1", Some(1_000))).unwrap();
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
        insert_reminder(&conn, &sample("due", Some(500))).unwrap();
        insert_reminder(&conn, &sample("later", Some(5_000))).unwrap();
        let mut paused = sample("paused", Some(500));
        paused.enabled = Some(false);
        insert_reminder(&conn, &paused).unwrap();
        let due = load_due_reminders(&conn, 1_000).unwrap();
        assert_eq!(due.len(), 1, "只有到点且启用的行");
        assert_eq!(due[0].id, "due");
    }

    #[test]
    fn advance_moves_next_fire_and_counts() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        insert_reminder(&conn, &sample("r1", Some(500))).unwrap();
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
        insert_reminder(&conn, &sample("r1", Some(1))).unwrap();
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
}
