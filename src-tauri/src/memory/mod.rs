//! 记忆 v2 门面（2026-09-09，设计 docs/BOT-MEMORY-V2-DESIGN.md）：
//! 替代旧关键词记忆体（db.rs bot_facts 系列）的运行时路径。
//! 系统未上线即切换：旧表 bot_facts 废弃不导入（无数据迁移；v1 代码已于
//! 2026-09-10 整体删除（commit e1234a2），老库残表无害不清理）。
//!
//! - 存储：store.rs（新表 mem_items，统一 500 上限 + 语义去重 + 容量淘汰）
//! - 嵌入：embed.rs（bge-small-zh-v1.5 本地 ONNX 推理，缺失时全局降级关键词模式）
//! - 打分：rank.rs（0.55 语义 + 0.20 关键词 + 0.15 重要度 + 0.10 新近度）
//!
//! 纪律：所有 DB 访问 = DB_WRITE_LOCK + spawn_blocking 单写者（同 db.rs 记忆模块先例）；
//! 嵌入计算一律在阻塞闭包内（持锁前算好），不占 async worker。
//! 任何一步失败：注入路径静默降级为「无记忆块」+ WARN 审计；工具路径返回错误文本
//! 给模型（与旧工具行为一致），绝不 panic。

pub mod consolidate;
pub mod embed;
pub mod rank;
pub mod store;

use crate::error::{CommandError, CommandResult};
use rank::MemInjection;
use store::{InsertOutcome, MemItem, NewItem};
use tauri::AppHandle;

/// 注入快照三段输出沿用旧 v1 预算值（保持记忆块体积不变）
const MEMORY_BUDGET_CHARS: usize = 4_000;

/// content 上限（表契约 ≤800 字；remember_fact 工具侧 value ≤500 更严，在入参校验处拦）
pub(crate) const MAX_CONTENT_CHARS: usize = 800;

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub(crate) fn truncate_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

// ───────────────────────── 记忆块拼装（四段式，前三段沿用旧 v1 format_memory_block 输出格式） ─────────────────────────

/// 「## 记忆」拼装（纯函数）：
/// pinned（importance≥4 的 profile/preference）→「用户画像与偏好」；
/// 检索 top-5 →「相关记忆」；最近 3 条 summary/reflection →「近期摘要」（零命中兜底）；
/// lesson top-3 →「经验教训」（2026-09-09 追加在最末，超预算最先被砍）。
/// 行格式与旧系统完全一致：fact 类 `- [kind]{[推断]}key：content`（无 key 省略「key：」），
/// summary/reflection 类 `- [日期]{[推断]}content`；超预算从后往前砍（画像段不砍）。
pub fn format_memory_block(inj: &MemInjection) -> Option<String> {
    fn inferred(m: &MemItem) -> &'static str {
        if m.source == "model_inferred" {
            "[推断]"
        } else {
            ""
        }
    }
    fn key_of(m: &MemItem) -> &str {
        m.tags.first().map(|s| s.as_str()).unwrap_or("")
    }
    fn fact_line(m: &MemItem) -> String {
        let key = key_of(m);
        if key.is_empty() {
            format!("- [{}]{}{}", m.kind, inferred(m), m.content)
        } else {
            format!("- [{}]{}{}：{}", m.kind, inferred(m), key, m.content)
        }
    }
    fn dated_line(m: &MemItem, fmt: &str) -> String {
        let date = chrono::DateTime::from_timestamp_millis(m.updated_at_ms)
            .map(|dt| dt.with_timezone(&chrono::Local).format(fmt).to_string())
            .unwrap_or_default();
        format!("- [{date}]{}{}", inferred(m), m.content)
    }
    fn is_dated(m: &MemItem) -> bool {
        m.kind == "summary" || m.kind == "reflection"
    }
    let mut sections: Vec<(&str, Vec<String>)> = Vec::new();
    if !inj.pinned.is_empty() {
        sections.push((
            "### 用户画像与偏好",
            inj.pinned.iter().map(fact_line).collect(),
        ));
    }
    if !inj.hits.is_empty() {
        sections.push((
            "### 相关记忆",
            inj.hits
                .iter()
                .map(|m| {
                    if is_dated(m) {
                        dated_line(m, "%Y-%m-%d")
                    } else {
                        fact_line(m)
                    }
                })
                .collect(),
        ));
    }
    if !inj.recent.is_empty() {
        sections.push((
            "### 近期摘要",
            inj.recent.iter().map(|m| dated_line(m, "%m-%d")).collect(),
        ));
    }
    // lesson 段（2026-09-09 lesson 特性）：追加在最后——超预算从后往前砍时它最先被砍，
    // 画像段不砍的规则不变。lesson 内容自含场景，纯文本 bullet。
    if !inj.lessons.is_empty() {
        sections.push((
            "### 经验教训",
            inj.lessons
                .iter()
                .map(|m| format!("- {}{}", inferred(m), m.content))
                .collect(),
        ));
    }
    if sections.is_empty() {
        return None;
    }
    let block_chars = |sections: &[(&str, Vec<String>)]| -> usize {
        "## 记忆".chars().count()
            + sections
                .iter()
                .map(|(h, ls)| {
                    h.chars().count() + 1
                        + ls.iter().map(|l| l.chars().count() + 1).sum::<usize>()
                })
                .sum::<usize>()
    };
    // 超预算从后往前砍（画像段不砍）
    let trimmable_from = if inj.pinned.is_empty() { 0 } else { 1 };
    let mut guard = 0;
    while block_chars(&sections) > MEMORY_BUDGET_CHARS && guard < 10_000 {
        guard += 1;
        match sections
            .iter_mut()
            .enumerate()
            .rev()
            .find(|(i, (_, ls))| *i >= trimmable_from && !ls.is_empty())
        {
            Some((_, (_, ls))) => {
                ls.pop();
            }
            None => break,
        }
    }
    sections.retain(|(_, ls)| !ls.is_empty());
    if sections.is_empty() {
        return None;
    }
    let mut out = String::from("## 记忆");
    for (h, ls) in &sections {
        out.push('\n');
        out.push_str(h);
        for l in ls {
            out.push('\n');
            out.push_str(l);
        }
    }
    Some(out)
}

// ───────────────────────── 注入（聊天主路径 / 任务卡执行共用） ─────────────────────────

/// 注入取数薄壳：embed →（持锁）快照 + 命中刷新访问计数 → 拼装。
/// 任何失败一律 None 静默降级为无记忆块 + WARN 审计（同旧系统行为）。
pub async fn injection_block<R: tauri::Runtime>(app: &tauri::AppHandle<R>, query: &str) -> Option<String> {
    let app2 = app.clone();
    let query = query.to_string();
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<MemInjection, String> {
        // 嵌入在持锁前算（ONNX 推理 ~数十 ms，不占 DB 写锁临界区）
        let emb = embed::embed_text(&query);
        let _g = crate::db::DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = crate::db::open_db(&app2)?;
        store::ensure_table(&conn)?;
        let items = store::load_all(&conn)?;
        let (inj, hit_ids) = rank::injection_snapshot(&items, &query, emb.as_deref(), now_ms());
        store::touch_accessed(&conn, &hit_ids, now_ms())?;
        Ok(inj)
    })
    .await;
    match r {
        Ok(Ok(inj)) => format_memory_block(&inj),
        Ok(Err(e)) => {
            crate::audit_event!(&app, crate::audit::AuditLevel::Warn, "memory.injection_failed",
                "error" => e);
            None
        }
        Err(e) => {
            crate::audit_event!(&app, crate::audit::AuditLevel::Warn, "memory.injection_failed",
                "error" => format!("记忆检索线程 join 失败：{e}"));
            None
        }
    }
}

// ───────────────────────── remember_fact / recall_facts 工具（名称与 schema 不变，内部切新 store） ─────────────────────────

/// 入参校验（与旧 validate_fact_kv 同规则同文案：key ≤50、value ≤500，空 value=删除）
fn validate_fact_kv(key: &str, value: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err("失败：key 不能为空".into());
    }
    if key.chars().count() > 50 {
        return Err("失败：key 太长（≤50 字）".into());
    }
    if value.chars().count() > 500 {
        return Err("失败：value 太长（≤500 字）".into());
    }
    Ok(())
}

/// category → kind 映射（profile/preference 独立成 kind，project/general 归 fact）
fn category_to_kind(category: &str) -> &'static str {
    match category {
        "profile" => "profile",
        "preference" => "preference",
        _ => "fact",
    }
}

/// remember_fact 工具（v2）：同 key（tags[0] 精确匹配）覆盖更新；新条目走语义去重 +
/// 容量淘汰。工具名/参数 schema 不变，模型无感。返回工具结果文本（含冲突提示）。
pub async fn tool_remember_fact(
    app: &AppHandle,
    args: &str,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = crate::bot::parse_args(args);
    let key = v["key"].as_str().unwrap_or("").trim().to_string();
    let value = v["value"].as_str().unwrap_or("").trim().to_string();
    if let Err(e) = validate_fact_kv(&key, &value) {
        return (e, Vec::new());
    }
    let category: &'static str = match v["category"].as_str().map(|s| s.trim()) {
        Some("profile") => "profile",
        Some("preference") => "preference",
        Some("project") => "project",
        _ => "general",
    };
    let importance = v["importance"].as_i64().unwrap_or(3).clamp(1, 5);
    let source = match v["source"].as_str() {
        Some("model_inferred") => "model_inferred",
        _ => "user_stated",
    };
    let app = app.clone();
    let r = tauri::async_runtime::spawn_blocking(move || -> String {
        let emb = if value.is_empty() {
            None
        } else {
            embed::embed_text(&value)
        };
        let _g = crate::db::DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = match crate::db::open_db(&app) {
            Ok(c) => c,
            Err(e) => return format!("失败：打开数据库出错：{e}"),
        };
        if let Err(e) = store::ensure_table(&conn) {
            return format!("失败：{e}");
        }
        let now = now_ms();
        // 空 value = 删除该 key 的记忆
        if value.is_empty() {
            return match store::delete_by_key_tag(&conn, &key) {
                Ok(true) => format!("已删除记忆「{key}」"),
                Ok(false) => format!("记忆「{key}」本来就不存在"),
                Err(e) => format!("失败：{e}"),
            };
        }
        // 同 key 覆盖（保持旧 remember_fact 语义：覆盖更新不堆积、不占新名额）
        match store::find_by_key_tag(&conn, &key) {
            Ok(Some(existing)) => {
                if let Err(e) = store::update_by_id(
                    &conn,
                    &existing.id,
                    &value,
                    importance,
                    source,
                    category_to_kind(category),
                    emb.as_deref(),
                    now,
                ) {
                    return format!("失败：{e}");
                }
                format!("已记住「{key}」：{value}")
            }
            Ok(None) => {
                let item = NewItem {
                    kind: category_to_kind(category).to_string(),
                    content: value.clone(),
                    tags: vec![key.clone()],
                    importance,
                    source: source.to_string(),
                };
                match store::insert_item(&conn, &item, emb.as_deref(), now) {
                    Ok((InsertOutcome::Inserted(_), hints)) => {
                        let msg = format!("已记住「{key}」：{value}");
                        if hints.is_empty() {
                            msg
                        } else {
                            format!(
                                "{msg}。相似已有记忆：[{}]——如需更新请用同 key 覆盖",
                                hints.join("；")
                            )
                        }
                    }
                    Ok((InsertOutcome::Merged(m), _)) => format!(
                        "已记住「{key}」：{value}（与已有记忆语义重复，已合并更新原有条目「{}」）",
                        m.tags.first().cloned().unwrap_or_default()
                    ),
                    Ok((InsertOutcome::RejectedFull(e), _)) => format!("失败：{e}"),
                    Err(e) => format!("失败：{e}"),
                }
            }
            Err(e) => format!("失败：{e}"),
        }
    })
    .await;
    match r {
        Ok(s) => (s, Vec::new()),
        Err(e) => (format!("失败：记忆写入线程 join 失败：{e}"), Vec::new()),
    }
}

/// recall_facts 工具（v2）：有 query 走混合检索 top-5，无 query 全量按 updated_at 倒序。
/// 纯读，不刷新访问计数。
pub async fn tool_recall_facts(
    app: &AppHandle,
    args: &str,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = crate::bot::parse_args(args);
    let query = v["query"].as_str().unwrap_or("").trim().to_string();
    let app = app.clone();
    let r = tauri::async_runtime::spawn_blocking(move || -> String {
        let emb = if query.is_empty() {
            None
        } else {
            embed::embed_text(&query)
        };
        let _g = crate::db::DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = match crate::db::open_db(&app) {
            Ok(c) => c,
            Err(e) => return format!("失败：打开数据库出错：{e}"),
        };
        if let Err(e) = store::ensure_table(&conn) {
            return format!("失败：{e}");
        }
        let items = match store::load_all(&conn) {
            Ok(v) => v,
            Err(e) => return format!("失败：{e}"),
        };
        let line_of = |m: &MemItem| {
            let key = m.tags.first().map(|s| s.as_str()).unwrap_or(&m.kind);
            format!("- {key}：{}", m.content)
        };
        if !query.is_empty() {
            let hits = rank::hybrid_search(&items, &query, emb.as_deref(), now_ms(), rank::MEMORY_TOP_N);
            if hits.is_empty() {
                return "没有找到相关记忆".into();
            }
            let lines: Vec<String> = hits.iter().map(|m| line_of(m)).collect();
            return format!("最相关 {} 条：\n{}", lines.len(), lines.join("\n"));
        }
        if items.is_empty() {
            return "（还没有任何长期记忆）".into();
        }
        let mut all = items;
        all.sort_by(|a, b| b.updated_at_ms.cmp(&a.updated_at_ms));
        let lines: Vec<String> = all.iter().map(|m| line_of(m)).collect();
        format!("已记住 {} 条：\n{}", lines.len(), lines.join("\n"))
    })
    .await;
    match r {
        Ok(s) => (s, Vec::new()),
        Err(e) => (format!("失败：记忆检索线程 join 失败：{e}"), Vec::new()),
    }
}

// ───────────────────────── lesson（教训记忆，2026-09-09） ─────────────────────────
//
// kind='lesson'，importance 默认 4，tags[0]='lesson' + 场景标签。写入走正常语义去重
//（同类失败的教训合并更新而不是堆积）。两个来源：record_lesson 工具（模型主动，
// source=model_inferred）与 run_task_in_chat 失败自动沉淀（source=system）。

/// lesson 入参校验（纯函数）：lesson 必填 ≤800 字；scenario 可选 ≤50 字
fn validate_lesson(lesson: &str, scenario: &str) -> Result<(), String> {
    if lesson.is_empty() {
        return Err("失败：lesson 内容不能为空".into());
    }
    if lesson.chars().count() > MAX_CONTENT_CHARS {
        return Err(format!("失败：lesson 太长（≤{MAX_CONTENT_CHARS} 字）"));
    }
    if scenario.chars().count() > 50 {
        return Err("失败：scenario 太长（≤50 字）".into());
    }
    Ok(())
}

/// lesson 写入内核（抽离 Connection + 注入向量，内存库可单测）：
/// 语义去重由 insert_item 承担（≥0.92 合并、0.75~0.92 冲突提示）。
/// 返回工具结果文本。
pub fn record_lesson_core(
    conn: &rusqlite::Connection,
    lesson: &str,
    scenario: &str,
    source: &str,
    embedding: Option<&[f32]>,
    now_ms: i64,
) -> String {
    let lesson = lesson.trim();
    let scenario = scenario.trim();
    if let Err(e) = validate_lesson(lesson, scenario) {
        return e;
    }
    let mut tags = vec!["lesson".to_string()];
    if !scenario.is_empty() {
        tags.push(scenario.to_string());
    }
    let item = NewItem {
        kind: "lesson".to_string(),
        content: lesson.to_string(),
        tags,
        importance: 4,
        source: source.to_string(),
    };
    match store::insert_item(conn, &item, embedding, now_ms) {
        Ok((InsertOutcome::Inserted(_), hints)) => {
            let msg = format!("已记录教训：{}", truncate_chars(lesson, 60));
            if hints.is_empty() {
                msg
            } else {
                format!("{msg}。相似已有记忆：[{}]——如需更新请用同场景覆盖", hints.join("；"))
            }
        }
        Ok((InsertOutcome::Merged(_), _)) => {
            format!("已记录教训（与已有教训语义重复，已合并更新）：{}", truncate_chars(lesson, 60))
        }
        Ok((InsertOutcome::RejectedFull(e), _)) => format!("失败：{e}"),
        Err(e) => format!("失败：{e}"),
    }
}

/// record_lesson 工具：模型被用户纠正 / 工具连续失败 / 发现更优做法时主动记教训。
pub async fn tool_record_lesson(
    app: &AppHandle,
    args: &str,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = crate::bot::parse_args(args);
    let lesson = v["lesson"].as_str().unwrap_or("").trim().to_string();
    let scenario = v["scenario"].as_str().unwrap_or("").trim().to_string();
    if let Err(e) = validate_lesson(&lesson, &scenario) {
        return (e, Vec::new());
    }
    let app2 = app.clone();
    let r = tauri::async_runtime::spawn_blocking(move || -> String {
        let emb = embed::embed_text(&lesson);
        let _g = crate::db::DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = match crate::db::open_db(&app2) {
            Ok(c) => c,
            Err(e) => return format!("失败：打开数据库出错：{e}"),
        };
        if let Err(e) = store::ensure_table(&conn) {
            return format!("失败：{e}");
        }
        record_lesson_core(&conn, &lesson, &scenario, "model_inferred", emb.as_deref(), now_ms())
    })
    .await;
    match r {
        Ok(s) => (s, Vec::new()),
        Err(e) => (format!("失败：教训写入线程 join 失败：{e}"), Vec::new()),
    }
}

/// 任务执行失败的教训文案（纯函数，不调 LLM 保持轻量）：
/// content 含任务标题 + 失败原因截断，tags 带 task_exec 场景。
pub(crate) fn task_failure_lesson(title: &str, error: &str) -> (String, Vec<String>) {
    let err = truncate_chars(error.trim(), 200);
    let content = format!(
        "执行任务「{}」失败：{err}。下次执行同类任务前应先排查该原因或改用替代做法。",
        truncate_chars(title.trim(), 50)
    );
    (
        truncate_chars(&content, MAX_CONTENT_CHARS),
        vec!["lesson".to_string(), "task_exec".to_string()],
    )
}

/// run_task_in_chat 失败自动沉淀 lesson（source=system）。任何失败只记审计，绝不影响
/// 原本的失败返回路径。
pub async fn auto_lesson_on_task_failure<R: tauri::Runtime>(app: &tauri::AppHandle<R>, title: &str, error: &str) {
    let (content, _tags) = task_failure_lesson(title, error);
    let app2 = app.clone();
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let emb = embed::embed_text(&content);
        let _g = crate::db::DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = crate::db::open_db(&app2)?;
        store::ensure_table(&conn)?;
        let msg = record_lesson_core(&conn, &content, "task_exec", "system", emb.as_deref(), now_ms());
        if msg.starts_with("失败") {
            return Err(msg);
        }
        Ok(())
    })
    .await;
    match r {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            crate::audit_event!(app, crate::audit::AuditLevel::Warn, "memory.lesson_save_failed",
                "error" => e);
        }
        Err(e) => {
            crate::audit_event!(app, crate::audit::AuditLevel::Warn, "memory.lesson_save_failed",
                "error" => format!("教训写入线程 join 失败：{e}"));
        }
    }
}

// ───────────────────────── 摘要/反思流水线（截断即摘要 → mem_items） ─────────────────────────

/// 摘要落库（v2）：kind=summary, importance=2, source=model_inferred，带嵌入向量。
/// 返回落库后最旧的 REFLECTION_BATCH 条 summary（id, content），供 Reflection 触发判定。
pub async fn save_summary(
    app: &AppHandle,
    session_id: Option<&str>,
    summary: &str,
) -> CommandResult<Vec<(String, String)>> {
    let app = app.clone();
    let session_id = session_id.map(|s| s.to_string());
    let summary = truncate_chars(summary.trim(), MAX_CONTENT_CHARS);
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<(String, String)>, String> {
        let emb = embed::embed_text(&summary);
        let _g = crate::db::DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = crate::db::open_db(&app)?;
        store::ensure_table(&conn)?;
        let item = NewItem {
            kind: "summary".to_string(),
            content: summary.clone(),
            tags: vec![format!(
                "summary:{}",
                session_id.as_deref().unwrap_or("default")
            )],
            importance: 2,
            source: "model_inferred".to_string(),
        };
        store::insert_item(&conn, &item, emb.as_deref(), now_ms())?;
        store::oldest_by_kind(&conn, "summary", crate::db::REFLECTION_BATCH)
    })
    .await
    .map_err(|e| CommandError::from(format!("记忆写入线程 join 失败：{e}")))?
    .map_err(CommandError::DbError)
}

/// Reflection 落库（v2）：插入 kind=reflection, importance=3，与被合并的原 summary
/// 删除放在同一事务（半完成不留中间态）。
pub async fn apply_reflection(app: &AppHandle, delete_ids: Vec<String>, text: String) -> CommandResult<()> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let emb = embed::embed_text(&text);
        let _g = crate::db::DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = crate::db::open_db(&app)?;
        store::ensure_table(&conn)?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let item = NewItem {
            kind: "reflection".to_string(),
            content: truncate_chars(text.trim(), MAX_CONTENT_CHARS),
            tags: vec!["reflection".to_string()],
            importance: 3,
            source: "model_inferred".to_string(),
        };
        store::insert_item(&tx, &item, emb.as_deref(), now_ms())?;
        store::delete_by_ids(&tx, &delete_ids)?;
        tx.commit().map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| CommandError::from(format!("Reflection 写入线程 join 失败：{e}")))?
    .map_err(CommandError::DbError)
}

#[cfg(test)]
mod tests;
