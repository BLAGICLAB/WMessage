//! 机器人多会话元数据

use serde::{Deserialize, Serialize};
use tauri::async_runtime;
use tauri::AppHandle;

use crate::error::{CommandError, CommandResult};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BotMsgRow {
    pub role: String,
    pub content: String,
    pub refs_json: Option<String>,
    pub thinking: Option<String>,
    pub tools_json: Option<String>,
    /// 回合终态（bot_model_loop::TurnStatus 的机器读标识）。
    /// None = 用户消息 / 后端自己写的行 / 旧行——没有回合概念，不是「进行中」
    pub turn_status: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BotSession {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// 子 agent 执行会话标记（U20D 批 5：会话路由从「标题 🧩 前缀」升级为
    /// 结构化字段；runner 建会话时置 1，普通会话/存量旧会话为 0）
    pub is_subagent: bool,
}

/// 会话列表（子 agent 会话不列出）。
///
/// 走 spawn_blocking：open_db 每次都跑全套 DDL + 迁移，是实打实的同步磁盘 I/O。
/// 同步命令在 Tauri v2 里跑主线程，聊天页一挂载就拉一次列表，等于把界面冻住。
#[tauri::command]
pub async fn bot_sessions_load(app: AppHandle) -> CommandResult<Vec<BotSession>> {
    async_runtime::spawn_blocking(move || {
        let conn = super::open_db(&app)?;
        let mut stmt = conn
            .prepare(
                // 子 agent 会话不列出：它是内部执行标识（tool_guard 的闸与流式路由
                // 都以 session id 为键），对用户只以聊天内嵌的子 agent 区域呈现。
                // 一堆执行现场混进对话列表会把真正的对话淹掉
                "SELECT id, title, created_at, updated_at, is_subagent FROM bot_sessions \
                 WHERE is_subagent = 0 ORDER BY updated_at DESC",
            )
            .map_err(|e| CommandError::DbError(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(BotSession {
                    id: r.get::<_, String>(0)?,
                    title: r.get::<_, String>(1)?,
                    created_at: r.get::<_, i64>(2)?,
                    updated_at: r.get::<_, i64>(3)?,
                    is_subagent: r.get::<_, i64>(4)? != 0,
                })
            })
            .map_err(|e| CommandError::DbError(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| CommandError::DbError(e.to_string()))
    })
    .await
    .map_err(|e| CommandError::from(format!("会话列表读取线程 join 失败：{e}")))?
}

//
pub fn bot_session_create_inner(
    conn: &rusqlite::Connection,
    title: Option<String>,
) -> Result<BotSession, String> {
    bot_session_create_with_kind_inner(conn, title, false)
}

/// 子 agent 执行会话创建（U20D 批 5）：is_subagent=1 结构化标记——
/// 前端停止键/斜杠 /stop 按 `Session.isSubagent` 路由 cancel_subagent，
/// 不再依赖标题 emoji 前缀。
pub fn bot_session_create_subagent_inner(
    conn: &rusqlite::Connection,
    title: Option<String>,
) -> Result<BotSession, String> {
    bot_session_create_with_kind_inner(conn, title, true)
}

fn bot_session_create_with_kind_inner(
    conn: &rusqlite::Connection,
    title: Option<String>,
    is_subagent: bool,
) -> Result<BotSession, String> {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let now = chrono::Utc::now().timestamp_millis();
    let title = title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "新对话".into());
    conn.execute(
        "INSERT INTO bot_sessions (id, title, created_at, updated_at, is_subagent) VALUES (?1, ?2, ?3, ?3, ?4)",
        rusqlite::params![id, title, now, is_subagent as i64],
    )
    .map_err(|e| e.to_string())?;
    Ok(BotSession {
        id,
        title,
        created_at: now,
        updated_at: now,
        is_subagent,
    })
}

/// 某会话的 per-onion 元配置（两层配置的 DB 侧）。
///
/// 独立小查询而不进 `bot_sessions_load` 的列表 SELECT：会话列表每次整表读进
/// 内存，多两列是小事，但**子 agent 会话被 load 显式过滤掉**——真正用 onion
/// 配置的就是它们，配置读不到等于这套配置没有消费者。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionOnion {
    /// 指向 `<数据目录>/onions/<name>.yaml` 的文件名（不含扩展名）
    pub config: String,
    /// 会话级 max_turn 覆盖；None = 用 yaml / 全局默认
    pub max_turn: Option<i64>,
}

pub fn bot_session_onion(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Option<SessionOnion>, String> {
    let mut stmt = conn
        .prepare("SELECT onion_config, onion_max_turn FROM bot_sessions WHERE id = ?1")
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query(rusqlite::params![session_id])
        .map_err(|e| e.to_string())?;
    let Some(row) = rows.next().map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let config: Option<String> = row.get(0).map_err(|e| e.to_string())?;
    let max_turn: Option<i64> = row.get(1).map_err(|e| e.to_string())?;
    Ok(config
        .filter(|c| !c.trim().is_empty())
        .map(|config| SessionOnion { config, max_turn }))
}

/// 给会话绑定 per-onion 配置（设置页导入后调用；None = 解除绑定）。
pub fn bot_session_set_onion(
    conn: &rusqlite::Connection,
    session_id: &str,
    config: Option<&str>,
    max_turn: Option<i64>,
) -> Result<(), String> {
    let n = conn
        .execute(
            "UPDATE bot_sessions SET onion_config = ?2, onion_max_turn = ?3 WHERE id = ?1",
            rusqlite::params![session_id, config, max_turn],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err(format!("会话 {session_id} 不存在"));
    }
    Ok(())
}

/// 新建会话。
///
/// 走 spawn_blocking：open_db（建连 + PRAGMA + 幂等 DDL）是同步磁盘 I/O，
/// 同步命令在 Tauri v2 里跑主线程。
#[tauri::command]
pub async fn bot_session_create(
    app: AppHandle,
    title: Option<String>,
) -> CommandResult<BotSession> {
    async_runtime::spawn_blocking(move || {
        // open_db（建连 + PRAGMA + 幂等 DDL）是同步磁盘 I/O，放在写锁外——
        // 临界区只覆盖 SQL 写，别让建连耗时串行化其他写者
        let conn = super::open_db(&app)?;
        let _g = crate::db::lock::lock_or_recover(
            &super::DB_WRITE_LOCK,
            "db::bot_sessions DB_WRITE_LOCK",
        );
        bot_session_create_inner(&conn, title).map_err(CommandError::DbError)
    })
    .await
    .map_err(|e| CommandError::from(format!("会话创建线程 join 失败：{e}")))?
}

pub fn bot_session_delete_inner(conn: &mut rusqlite::Connection, id: &str) -> CommandResult<()> {
    let tx = conn
        .transaction()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    tx.execute("DELETE FROM bot_messages WHERE session_id = ?1", [id])
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    // 子 agent 行随会话走：subagents 无外键，不清就成永久孤儿（会话都没了，
    // list_subagents 再也不会查它）。只清终态行——在途行的 runner 还要靠它
    // 收尾，删了会让 finalize 找不到行；这类行只能等下次启动的僵尸清扫收掉
    //（本进程运行期内没有第二次清扫）
    tx.execute(
        "DELETE FROM subagents
          WHERE (parent_session_id = ?1 OR session_id = ?1)
            AND status IN ('succeeded','failed','cancelled','budget_exceeded')",
        [id],
    )
    .map_err(|e| CommandError::DbError(e.to_string()))?;
    // 在途子 agent 一并取消：父会话没了，它的产物没人收、它的行没人看得见，
    // 再跑下去是纯浪费（烧完整份预算、还照落盘写文件）。置终态后 runner 的
    // 2s watcher 见到终态即停工具循环。
    //
    // 行留着不删——runner 收尾还要读它（删了 finalize 找不到行）。
    let live = crate::db::subagents::load_live_subagents_by_session(&tx, id)
        .map_err(CommandError::DbError)?;
    let now = chrono::Utc::now().timestamp_millis();
    for row in live {
        crate::db::subagents::update_subagent_status(
            &tx,
            &row.id,
            crate::db::subagents::SubagentStatus::Cancelled,
            now,
            Some("父会话已删除"),
        )
        .map_err(CommandError::DbError)?;
    }
    tx.execute("DELETE FROM bot_sessions WHERE id = ?1", [id])
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    tx.commit()
        .map_err(|e| CommandError::DbError(e.to_string()))
}

#[tauri::command]
pub async fn bot_session_delete(app: AppHandle, id: String) -> CommandResult<()> {
    async_runtime::spawn_blocking(move || {
        let _g = crate::db::lock::lock_or_recover(
            &super::DB_WRITE_LOCK,
            "db::bot_sessions DB_WRITE_LOCK",
        );
        let mut conn = super::open_db(&app)?;
        bot_session_delete_inner(&mut conn, &id)
    })
    .await
    .map_err(|e| CommandError::from(format!("会话删除线程 join 失败：{e}")))?
}

/// 重命名会话。
///
/// 走 spawn_blocking：open_db（建连 + PRAGMA + 幂等 DDL）是同步磁盘 I/O，
/// 同步命令在 Tauri v2 里跑主线程。
#[tauri::command]
pub async fn bot_session_rename(app: AppHandle, id: String, title: String) -> CommandResult<()> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err(CommandError::InvalidArgument {
            field: "title".into(),
            value: String::new(),
            reason: "会话标题不能为空（或全为空白字符）".into(),
        });
    }
    async_runtime::spawn_blocking(move || {
        // 同 bot_session_create：open_db 在写锁外，临界区只覆盖 UPDATE
        let conn = super::open_db(&app)?;
        let _g = crate::db::lock::lock_or_recover(
            &super::DB_WRITE_LOCK,
            "db::bot_sessions DB_WRITE_LOCK",
        );
        let now = chrono::Utc::now().timestamp_millis();
        let rows = conn
            .execute(
                "UPDATE bot_sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![title, now, id],
            )
            .map_err(|e| e.to_string())?;
        if rows == 0 {
            return Err(CommandError::InvalidArgument {
                field: "id".into(),
                value: id.to_string(),
                reason: "会话不存在（重命名未命中任何行）".into(),
            });
        }
        Ok(())
    })
    .await
    .map_err(|e| CommandError::from(format!("会话重命名线程 join 失败：{e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE bot_sessions (
                id          TEXT PRIMARY KEY,
                title       TEXT NOT NULL,
                created_at  INTEGER NOT NULL,
                updated_at  INTEGER NOT NULL,
                is_subagent INTEGER NOT NULL DEFAULT 0
            );",
        )
        .unwrap();
        conn
    }

    /// 接线守卫：这三个会话命令不得是同步命令。
    ///
    /// Tauri v2 的同步命令在主线程执行，而 `open_db` 每次都跑全套 DDL + 迁移
    /// （实打实的同步磁盘 I/O）——聊天页一挂载就拉一次会话列表，界面会被冻住。
    #[test]
    fn session_commands_run_off_the_main_thread() {
        let src = include_str!("bot_sessions.rs");
        for name in [
            "bot_sessions_load",
            "bot_session_create",
            "bot_session_rename",
        ] {
            let at = src
                .find(&format!("pub async fn {name}("))
                .unwrap_or_else(|| panic!("{name} 必须是 async 命令，否则跑在主线程"));
            // 光 async 不够：async 命令体里直接做同步阻塞 I/O，只是把阻塞从
            // 主线程挪到了执行线程。必须下到 spawn_blocking。
            // 按字符切窗（按字节切会切在多字节汉字中间）
            let body: String = src[at..].chars().take(600).collect();
            assert!(
                body.contains("spawn_blocking"),
                "{name} 必须在 spawn_blocking 里跑——async 命令体里直接做同步 I/O \
                 只是把阻塞挪了地方"
            );
        }
    }

    /// U20D 批 5：subagent 变体写 is_subagent=1，普通创建默认 0——
    /// 前端停止键路由（cancel_subagent vs bot_stop）的数据源
    #[test]
    fn subagent_flag_persists_and_loads() {
        let conn = mem_db();
        let sub = bot_session_create_subagent_inner(&conn, Some("子任务：调研".into())).unwrap();
        let normal = bot_session_create_inner(&conn, Some("普通会话".into())).unwrap();
        assert!(sub.is_subagent, "subagent 变体必须置 1");
        assert!(sub.title.starts_with("子任务："), "标题无 emoji 前缀");
        assert!(!normal.is_subagent, "普通创建默认 0");

        let mut stmt = conn
            .prepare("SELECT id, title, created_at, updated_at, is_subagent FROM bot_sessions")
            .unwrap();
        let rows: Vec<BotSession> = stmt
            .query_map([], |r| {
                Ok(BotSession {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    created_at: r.get(2)?,
                    updated_at: r.get(3)?,
                    is_subagent: r.get::<_, i64>(4)? != 0,
                })
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(rows.len(), 2, "两条会话都落库");
        assert_eq!(
            rows.iter().filter(|s| s.is_subagent).count(),
            1,
            "恰好一条标记为子 agent 会话"
        );
    }

    /// 前后端的字段名靠 camelCase serde 对齐：前端发 `turnStatus`，后端收
    /// `turn_status`。名字对不上不会报错——serde 把多出来的键丢掉，字段静默
    /// 变 None，回合终态就这么没了。
    #[test]
    fn turn_status_uses_camel_case_on_the_wire() {
        let row = BotMsgRow {
            role: "assistant".into(),
            content: "答".into(),
            refs_json: None,
            thinking: None,
            tools_json: None,
            turn_status: Some("completed".into()),
        };
        let v = serde_json::to_value(&row).expect("序列化");
        assert_eq!(v["turnStatus"], "completed", "线上字段名必须是 turnStatus");
        let back: BotMsgRow = serde_json::from_value(serde_json::json!({
            "role": "assistant",
            "content": "答",
            "turnStatus": "completed",
        }))
        .expect("反序列化");
        assert_eq!(
            back.turn_status.as_deref(),
            Some("completed"),
            "前端发什么后端就得收什么"
        );
    }
}
