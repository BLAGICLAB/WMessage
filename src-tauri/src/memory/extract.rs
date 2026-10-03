//! 自动记忆抽取（U16）：交互式会话收尾，异步抽取「值得跨会话记住」的用户
//! 画像/偏好/稳定事实，入库或进待确认队列——记忆生态从「被动等模型调工具」
//! 走向「主动从对话中学习」。
//!
//! 触发链：bot_chat 收尾（trace 挂点旁）→ [`maybe_extract_from_session`]
//! （同步快速门禁 + 限频 check-and-set）→ spawn 异步管线（LLM 抽取 → 解析 →
//! 入库/入队）。fire-and-forget：任何一步失败只记 WARN 审计，绝不影响主对话。
//!
//! 门禁（拍板口径）：
//! - 仅交互式聊天（任务执行/定时/Skill 会话语料是任务指令，不是用户画像）；
//! - `memoryControl.autoExtract`：off（默认）/ auto（直接入库）/ confirm（进
//!   待确认队列 mem_pending，用户过目后入库）；
//! - `memoryControl.autoWriteEnabled` = false 时抽取整体不跑（U15 总闸优先）。
//!
//! 限频：每会话 [`EXTRACT_MIN_INTERVAL_SECS`] 内最多抽取一次（进程内
//! check-and-set，gate 通过即占窗口——抽取失败也等下一窗口，防失败风暴）。
//!
//! 入库复用：auto 档走 `store::insert_item`（语义去重/容量/淘汰全现成，
//! source=model_inferred → 注入块 [推断] 徽标天然可辨）；confirm 档进
//! mem_pending（上限 [`PENDING_CAP`]，满则丢最旧），approve 时才真正入库。

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use super::panel::MemImportReport;
use super::store::{self, NewItem};
use super::{auto_extract_mode, auto_write_enabled, AutoExtract};
use crate::error::{CommandError, CommandResult};
use crate::prompts::EXTRACT_PROMPT;

/// 抽取限频：每会话两次抽取的最小间隔（30 分钟；节流是 LLM 成本第一道闸）
pub const EXTRACT_MIN_INTERVAL_SECS: i64 = 30 * 60;

/// 参与抽取的最近消息条数（6 轮对话）
const EXTRACT_MAX_MSGS: usize = 12;

/// 每条参与抽取的消息内容截断（防超长消息撑爆抽取请求）
const EXTRACT_MSG_CHARS: usize = 400;

/// 单条抽取结果的 content 上限（入库契约 ≤800；抽取从严 500，同 remember 工具）
const MAX_EXTRACT_CONTENT_CHARS: usize = 500;

/// 待确认队列上限：满则丢最旧（抽取静默放弃，不做无限堆积）
pub const PENDING_CAP: i64 = 50;

/// kind 白名单：抽取只产出这三类用户信息（summary/reflection/lesson 等是
/// 系统流水线的 kind，不由抽取产生）
const EXTRACT_KINDS: [&str; 3] = ["profile", "preference", "fact"];

// ───────────────────────── mem_pending 表 ─────────────────────────

/// 待确认条目（对外视图）
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MemPendingView {
    pub id: i64,
    pub content: String,
    pub kind: String,
    pub importance: i64,
    pub session_id: String,
    pub created_at: i64,
}

fn ensure_pending_table(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS mem_pending(
           id         INTEGER PRIMARY KEY AUTOINCREMENT,
           content    TEXT NOT NULL,
           kind       TEXT NOT NULL,
           importance INTEGER NOT NULL DEFAULT 2,
           session_id TEXT NOT NULL,
           created_at TEXT NOT NULL
         );",
    )
    .map_err(|e| e.to_string())
}

/// 入队（容量满丢最旧）：now_ms 供 created_at
fn pending_insert(
    conn: &rusqlite::Connection,
    content: &str,
    kind: &str,
    importance: i64,
    session_id: &str,
    now_ms: i64,
) -> Result<(), String> {
    ensure_pending_table(conn)?;
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM mem_pending", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if count >= PENDING_CAP {
        conn.execute(
            "DELETE FROM mem_pending WHERE id = (SELECT id FROM mem_pending ORDER BY id LIMIT 1)",
            [],
        )
        .map_err(|e| e.to_string())?;
    }
    conn.execute(
        "INSERT INTO mem_pending (content, kind, importance, session_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            content,
            kind,
            importance.clamp(1, 5),
            session_id,
            super::store::ts_to_text(now_ms),
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn pending_list(conn: &rusqlite::Connection) -> Result<Vec<MemPendingView>, String> {
    ensure_pending_table(conn)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, content, kind, importance, session_id, created_at
             FROM mem_pending ORDER BY id DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(MemPendingView {
                id: r.get(0)?,
                content: r.get(1)?,
                kind: r.get(2)?,
                importance: r.get(3)?,
                session_id: r.get(4)?,
                created_at: super::store::ts_to_ms(&r.get::<_, String>(5)?),
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// 按 id 集合删除，返回删到的条数
fn pending_delete(conn: &rusqlite::Connection, ids: &[i64]) -> Result<usize, String> {
    ensure_pending_table(conn)?;
    let mut n = 0;
    for id in ids {
        n += conn
            .execute("DELETE FROM mem_pending WHERE id = ?1", [id])
            .map_err(|e| e.to_string())?;
    }
    Ok(n)
}

/// 取待确认行（approve 用）
struct PendingRow {
    content: String,
    kind: String,
    importance: i64,
}

fn pending_get(conn: &rusqlite::Connection, id: i64) -> Result<Option<PendingRow>, String> {
    ensure_pending_table(conn)?;
    conn.query_row(
        "SELECT content, kind, importance FROM mem_pending WHERE id = ?1",
        [id],
        |r| {
            Ok(PendingRow {
                content: r.get(0)?,
                kind: r.get(1)?,
                importance: r.get(2)?,
            })
        },
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other.to_string()),
    })
}

// ───────────────────────── 限频（进程内） ─────────────────────────

fn last_extract_map() -> std::sync::MutexGuard<'static, std::collections::HashMap<String, i64>> {
    static LAST_EXTRACT_MS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, i64>>,
    > = std::sync::OnceLock::new();
    LAST_EXTRACT_MS
        .get_or_init(std::sync::Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// 限频判定（纯函数，单测直打）：None = 从未抽过 → 到期
fn throttle_due(last: Option<i64>, now_ms: i64, min_interval_secs: i64) -> bool {
    match last {
        None => true,
        Some(last) => now_ms.saturating_sub(last) >= min_interval_secs * 1000,
    }
}

/// check-and-set：到期则写入 now 并返回 true（占住窗口——抽取失败也等下一窗口，
/// 防失败风暴刷 LLM）；未到期返回 false
fn throttle_check_set(session_id: &str, now_ms: i64) -> bool {
    let mut map = last_extract_map();
    let due = throttle_due(
        map.get(session_id).copied(),
        now_ms,
        EXTRACT_MIN_INTERVAL_SECS,
    );
    if due {
        map.insert(session_id.to_string(), now_ms);
    }
    due
}

// ───────────────────────── 抽取解析 ─────────────────────────

/// 单条抽取结果
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ExtractedFact {
    pub content: String,
    pub kind: String,
    pub importance: i64,
}

/// 解析抽取输出（容错同 parse_ops 风格：剥围栏 → 截取首尾方括号 →
/// 逐条校验：content 非空（超长截断）、kind 白名单、importance 钳 1..=5）
pub(crate) fn parse_extract(text: &str) -> Vec<ExtractedFact> {
    let t = text.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    let t = t.strip_suffix("```").unwrap_or(t).trim();
    let Some(start) = t.find('[') else {
        return Vec::new();
    };
    let Some(end) = t.rfind(']').filter(|e| *e > start) else {
        return Vec::new();
    };
    let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(&t[start..=end]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        let Some(content) = item["content"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let Some(kind) = item["kind"].as_str().map(str::trim) else {
            continue;
        };
        if !EXTRACT_KINDS.contains(&kind) {
            continue;
        }
        let importance = item["importance"].as_i64().unwrap_or(2).clamp(1, 5);
        out.push(ExtractedFact {
            content: super::truncate_chars(content, MAX_EXTRACT_CONTENT_CHARS),
            kind: kind.to_string(),
            importance,
        });
    }
    out
}

// ───────────────────────── 触发与管线 ─────────────────────────

/// 会话收尾触发入口（fire-and-forget；同步快速门禁 + 限频，过了才 spawn 管线）。
/// 挂在 bot_chat 最终 return 处；非交互 / 未开档 / 总闸关 / 限频未到 → 直接返回。
pub fn maybe_extract_from_session(app: AppHandle, session_id: Option<String>, interactive: bool) {
    if !interactive {
        return;
    }
    let Some(session_id) = session_id.filter(|s| !s.trim().is_empty()) else {
        return;
    };
    let ctrl = crate::bot::read_memory_control(&app);
    if !auto_write_enabled(ctrl.as_ref()) {
        return; // U15 总闸优先：模型主动记忆关 = 抽取也不跑
    }
    let mode = auto_extract_mode(ctrl.as_ref());
    if mode == AutoExtract::Off {
        return;
    }
    if !throttle_check_set(&session_id, super::now_ms()) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let app_for_audit = app.clone();
        if let Err(e) = run_extract(app, session_id, mode).await {
            eprintln!("[memory] 自动抽取失败（静默降级）：{e}");
            crate::audit_event!(
                &app_for_audit,
                crate::audit::AuditLevel::Warn,
                "memory.extract_failed",
                "error" => e
            );
        }
    });
}

/// 抽取管线：读会话最近消息 → LLM 抽取 → 解析 → 入库/入队。
/// 全程失败只 Err（调用方 eprintln + 后续 WARN 由调用处审计），不重试。
async fn run_extract(app: AppHandle, session_id: String, mode: AutoExtract) -> Result<(), String> {
    let msgs = load_recent_messages(app.clone(), &session_id).await?;
    if msgs.len() < 2 {
        return Ok(()); // 单条消息（用户刚开聊）无可抽取
    }
    let text = crate::bot_chat::summarize_messages(&app, EXTRACT_PROMPT, &msgs)
        .await
        .map_err(|e| format!("抽取 LLM 调用失败：{}", e.message()))?;
    let facts = parse_extract(&text);
    if facts.is_empty() {
        return Ok(());
    }
    let now = super::now_ms();
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        match mode {
            AutoExtract::Auto => {
                // 预嵌入与配置读取都在锁外（ONNX 推理/文件 IO 不持写锁）；
                // 单条入库失败聚合计数，收口一条 Warn 审计（不刷屏）
                let embs: Vec<Option<Vec<f32>>> = facts
                    .iter()
                    .map(|f| super::embed::embed_text(&f.content))
                    .collect();
                let sp = store::StoreParams::of(&crate::bot::read_memory_tuning(&app2));
                let _g = super::store_lock();
                let conn = crate::db::open_db(&app2).map_err(|e| e.to_string())?;
                let mut failed = 0usize;
                for (f, emb) in facts.iter().zip(embs) {
                    let item = NewItem {
                        kind: f.kind.clone(),
                        content: f.content.clone(),
                        tags: Vec::new(),
                        importance: f.importance,
                        source: "model_inferred".to_string(),
                    };
                    if let Err(e) = store::insert_item_with(&conn, &item, emb.as_deref(), now, &sp)
                    {
                        failed += 1;
                        eprintln!("[memory] 抽取条目入库失败（跳过）：{e}");
                    }
                }
                if failed > 0 {
                    crate::audit_event!(
                        &app2,
                        crate::audit::AuditLevel::Warn,
                        "memory.extract_insert_failed",
                        "count" => failed
                    );
                }
            }
            AutoExtract::Confirm => {
                // 配置读取在锁外；队列写入事务化（半批不留中间态）
                let _ = crate::bot::read_memory_tuning(&app2);
                let _g = super::store_lock();
                let mut conn = crate::db::open_db(&app2).map_err(|e| e.to_string())?;
                let tx = conn.transaction().map_err(|e| e.to_string())?;
                for f in &facts {
                    pending_insert(&tx, &f.content, &f.kind, f.importance, &session_id, now)?;
                }
                tx.commit().map_err(|e| e.to_string())?;
            }
            AutoExtract::Off => return Ok(()), // 入口已挡；防御臂显式返回防门禁被删后静默吞管线
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("抽取入库线程 join 失败：{e}"))?
}

/// 读会话最近 EXTRACT_MAX_MSGS 条 user/assistant 消息（每条截断）
async fn load_recent_messages(
    app: AppHandle,
    session_id: &str,
) -> Result<Vec<crate::bot_chat::ChatMsg>, String> {
    let sid = session_id.to_string();
    let rows =
        tauri::async_runtime::spawn_blocking(move || -> Result<Vec<(String, String)>, String> {
            let conn = crate::db::open_db(&app).map_err(|e| e.to_string())?;
            let mut stmt = conn
                .prepare(
                    "SELECT role, content FROM bot_messages
                 WHERE session_id = ?1 AND role IN ('user','assistant')
                 ORDER BY id DESC LIMIT ?2",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(rusqlite::params![sid, EXTRACT_MAX_MSGS], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| format!("历史读取线程 join 失败：{e}"))??;
    // 反转回时间正序
    Ok(rows
        .into_iter()
        .rev()
        .map(|(role, content)| crate::bot_chat::ChatMsg {
            role,
            content: super::truncate_chars(&content, EXTRACT_MSG_CHARS),
        })
        .collect())
}

// ───────────────────────── tauri 命令（待确认队列） ─────────────────────────

/// 待确认队列列表（新→旧）
#[tauri::command]
pub async fn mem_pending_list(app: AppHandle) -> CommandResult<Vec<MemPendingView>> {
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<Vec<MemPendingView>, String> {
        let _g = super::store_lock();
        let conn = crate::db::open_db(&app).map_err(|e| e.to_string())?;
        pending_list(&conn)
    })
    .await;
    r.map_err(|e| CommandError::from(format!("待确认列表线程 join 失败：{e}")))?
        .map_err(CommandError::DbError)
}

/// 收下待确认条目：逐条走 insert_item 既有语义去重入库。
/// 三段式锁纪律：锁内取行 → 锁外预嵌入（ONNX 推理不持写锁）→ 锁内入库并删除。
#[tauri::command]
pub async fn mem_pending_approve(app: AppHandle, ids: Vec<i64>) -> CommandResult<MemImportReport> {
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<MemImportReport, String> {
        // 去重（保序）：防前端重发导致同条目双嵌入/双入库
        let mut unique_ids: Vec<i64> = Vec::new();
        for id in ids {
            if !unique_ids.contains(&id) {
                unique_ids.push(id);
            }
        }
        // 第一段（锁内）：取待确认行
        let rows: Vec<(i64, PendingRow)> = {
            let _g = super::store_lock();
            let conn = crate::db::open_db(&app).map_err(|e| e.to_string())?;
            let mut out = Vec::new();
            for id in &unique_ids {
                if let Some(row) = pending_get(&conn, *id)? {
                    out.push((*id, row));
                }
            }
            out
        };
        if rows.is_empty() {
            return Ok(MemImportReport::default());
        }
        // 第二段（锁外）：预嵌入 + 参数读取
        let prepared: Vec<(i64, PendingRow, Option<Vec<f32>>)> = rows
            .into_iter()
            .map(|(id, row)| {
                let emb = super::embed::embed_text(&row.content);
                (id, row, emb)
            })
            .collect();
        let sp = store::StoreParams::of(&crate::bot::read_memory_tuning(&app));
        // 第三段（锁内）：入库 + 删队列。
        // 入库 Err → 中止上抛且**不删队列行**（用户可重试，条目不丢）；
        // RejectedFull/RefusedForeignMerge → skipped 并删行（数据性拒收不可重试）
        let mut report = MemImportReport::default();
        let _g = super::store_lock();
        let conn = crate::db::open_db(&app).map_err(|e| e.to_string())?;
        for (id, row, emb) in &prepared {
            let item = NewItem {
                kind: row.kind.clone(),
                content: row.content.clone(),
                tags: Vec::new(),
                importance: row.importance,
                source: "model_inferred".to_string(),
            };
            match store::insert_item_with(&conn, &item, emb.as_deref(), super::now_ms(), &sp) {
                Ok((store::InsertOutcome::Inserted(_), _)) => report.inserted += 1,
                Ok((store::InsertOutcome::Merged { .. }, _)) => report.merged += 1,
                Ok((store::InsertOutcome::RejectedFull(_), _))
                | Ok((store::InsertOutcome::RefusedForeignMerge { .. }, _)) => report.skipped += 1,
                Err(e) => {
                    return Err(format!(
                        "收下中止（存储故障，已入库 {} 条，未处理条目仍在列表）：{e}",
                        report.inserted + report.merged
                    ))
                }
            }
            pending_delete(&conn, &[*id])?;
        }
        Ok(report)
    })
    .await;
    r.map_err(|e| CommandError::from(format!("收下线程 join 失败：{e}")))?
        .map_err(CommandError::from)
}

/// 忽略待确认条目（直接删除），返回删到的条数
#[tauri::command]
pub async fn mem_pending_reject(app: AppHandle, ids: Vec<i64>) -> CommandResult<usize> {
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<usize, String> {
        let _g = super::store_lock();
        let conn = crate::db::open_db(&app).map_err(|e| e.to_string())?;
        pending_delete(&conn, &ids)
    })
    .await;
    r.map_err(|e| CommandError::from(format!("忽略线程 join 失败：{e}")))?
        .map_err(CommandError::DbError)
}

#[cfg(test)]
mod tests {
    use super::super::MemoryControl;
    use super::*;

    fn mem_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        ensure_pending_table(&conn).unwrap();
        conn
    }

    // ── 档位解析 ──

    #[test]
    fn auto_extract_mode_parses_and_defaults_off() {
        assert_eq!(auto_extract_mode(None), AutoExtract::Off);
        let off = Some(MemoryControl::default());
        assert_eq!(auto_extract_mode(off.as_ref()), AutoExtract::Off);
        for (s, want) in [
            ("auto", AutoExtract::Auto),
            ("confirm", AutoExtract::Confirm),
            ("off", AutoExtract::Off),
            ("垃圾值", AutoExtract::Off),
            ("", AutoExtract::Off),
        ] {
            let ctrl = Some(MemoryControl {
                auto_extract: s.into(),
                ..MemoryControl::default()
            });
            assert_eq!(auto_extract_mode(ctrl.as_ref()), want, "mode({s})");
        }
    }

    #[test]
    fn serde_default_fills_auto_extract_off_for_old_blocks() {
        // U15 时期落盘的 memoryControl 块无 autoExtract 字段 → 反序列化补 off
        let ctrl: MemoryControl =
            serde_json::from_str(r#"{"injectionEnabled":true,"autoWriteEnabled":false}"#).unwrap();
        assert_eq!(ctrl.auto_extract, "off");
        assert_eq!(auto_extract_mode(Some(&ctrl)), AutoExtract::Off);
    }

    // ── 限频 ──

    #[test]
    fn throttle_due_first_and_interval() {
        const MIN: i64 = EXTRACT_MIN_INTERVAL_SECS;
        assert!(throttle_due(None, 1_000_000, MIN), "从未抽取 → 到期");
        assert!(!throttle_due(
            Some(1_000_000),
            1_000_000 + MIN * 1000 - 1,
            MIN
        ));
        assert!(throttle_due(Some(1_000_000), 1_000_000 + MIN * 1000, MIN));
    }

    #[test]
    fn throttle_check_set_occupies_window() {
        let now = 10_000_000;
        assert!(throttle_check_set("sess-throttle-test", now), "首次应到期");
        assert!(
            !throttle_check_set("sess-throttle-test", now + 1),
            "窗口内不再触发"
        );
        assert!(
            throttle_check_set("sess-throttle-test", now + EXTRACT_MIN_INTERVAL_SECS * 1000),
            "跨过间隔重新到期"
        );
    }

    // ── 解析矩阵 ──

    #[test]
    fn parse_extract_accepts_array_and_clamps() {
        let text = r#"[{"content":"用户偏好简洁回复","kind":"preference","importance":3},
                       {"content":"用户是后端工程师","kind":"profile","importance":9}]"#;
        let facts = parse_extract(text);
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].kind, "preference");
        assert_eq!(facts[1].importance, 5, "importance 钳到 5");
    }

    #[test]
    fn parse_extract_tolerates_fences_and_prose() {
        let fenced = "好的，以下是抽取结果：\n```json\n[{\"content\":\"喜欢中文回复\",\"kind\":\"preference\",\"importance\":2}]\n```\n以上。";
        assert_eq!(parse_extract(fenced).len(), 1);
        assert!(parse_extract("我觉得这段对话没什么可记的。").is_empty());
        assert!(parse_extract("[]").is_empty());
        assert!(parse_extract("").is_empty());
    }

    #[test]
    fn parse_extract_skips_contract_violations() {
        // 空 content / 未知 kind / 缺 kind → 跳过；超长 content 截断
        let long = "长".repeat(MAX_EXTRACT_CONTENT_CHARS + 50);
        let text = format!(
            r#"[{{"content":"","kind":"fact","importance":2}},
               {{"content":"未知类型","kind":"magic","importance":2}},
               {{"content":"缺 kind","importance":2}},
               {{"content":"{long}","kind":"fact","importance":2}}]"#
        );
        let facts = parse_extract(&text);
        assert_eq!(facts.len(), 1);
        assert_eq!(
            facts[0].content.chars().count(),
            MAX_EXTRACT_CONTENT_CHARS,
            "超长内容截断到上限"
        );
    }

    // ── 待确认队列 ──

    #[test]
    fn pending_insert_list_delete_roundtrip() {
        let conn = mem_db();
        pending_insert(&conn, "待确认一", "preference", 3, "s1", 1_000).unwrap();
        pending_insert(&conn, "待确认二", "fact", 2, "s1", 2_000).unwrap();
        let list = pending_list(&conn).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].content, "待确认二", "新→旧排序");
        assert_eq!(list[0].created_at, 2_000);
        assert_eq!(pending_delete(&conn, &[list[0].id]).unwrap(), 1);
        assert_eq!(pending_list(&conn).unwrap().len(), 1);
    }

    #[test]
    fn pending_cap_evicts_oldest() {
        let conn = mem_db();
        for i in 0..PENDING_CAP + 5 {
            pending_insert(&conn, &format!("条目{i}"), "fact", 2, "s", 1_000 + i).unwrap();
        }
        let list = pending_list(&conn).unwrap();
        assert_eq!(list.len() as i64, PENDING_CAP, "队列钉在上限");
        // 最旧的 5 条（条目0..条目4）被挤掉
        assert!(list.iter().all(|v| !v.content.contains("条目0")));
        assert!(list.iter().any(|v| v.content.contains("条目5")));
    }

    #[test]
    fn pending_get_returns_row_or_none() {
        let conn = mem_db();
        pending_insert(&conn, "某条", "fact", 2, "s", 1_000).unwrap();
        let id = pending_list(&conn).unwrap()[0].id;
        assert_eq!(pending_get(&conn, id).unwrap().unwrap().content, "某条");
        assert!(pending_get(&conn, 999).unwrap().is_none());
    }
}
