//! 记忆库面板命令（mem_list / mem_update / mem_delete / mem_stats）：
//! 设置页「记忆库」管理界面的读改删薄层，全部复用 store.rs / rank.rs / embed.rs。
//!
//! 设计约束：
//! - `MemItemView` 不含 512 维向量本体（500 条 ≈1MB 无必要），只给 hasEmbedding
//!   标志——同 BotConfigView 不含 key 本体的先例；
//! - `mem_list` 带查询词走 rank::hybrid_search（**纯读、不刷 access_count**：
//!   访问强化只属于真实聊天注入，面板搜索不算「想起」）；
//! - 内核函数收 `&Connection`（内存库可单测），锁/open_db/AppHandle 包装在命令层
//!   （同 record_lesson_core 先例）；
//! - 纪律同 memory/mod.rs：DB_WRITE_LOCK + spawn_blocking 单写者，嵌入在持锁前算。

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use super::rank;
use super::store::{self, MemItem};
use super::{embed, now_ms, MAX_CONTENT_CHARS};
use crate::error::{CommandError, CommandResult};

/// 记忆条目对外视图（不含向量本体，只给 hasEmbedding 标志）
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MemItemView {
    pub id: String,
    pub kind: String,
    pub content: String,
    pub tags: Vec<String>,
    pub importance: i64,
    pub source: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub access_count: i64,
    pub last_accessed_at: Option<i64>,
    pub has_embedding: bool,
}

impl From<&MemItem> for MemItemView {
    fn from(m: &MemItem) -> Self {
        Self {
            id: m.id.clone(),
            kind: m.kind.clone(),
            content: m.content.clone(),
            tags: m.tags.clone(),
            importance: m.importance,
            source: m.source.clone(),
            created_at: m.created_at_ms,
            updated_at: m.updated_at_ms,
            access_count: m.access_count,
            last_accessed_at: m.last_accessed_at_ms,
            has_embedding: m.embedding.is_some(),
        }
    }
}

/// 记忆库统计 + 嵌入引擎状态（embedOk/embedError 由命令层从 embed::engine_status 填）
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct MemStats {
    pub total: i64,
    /// 按 kind 计数（升序键序，前端直接渲染）
    pub by_kind: Vec<(String, i64)>,
    /// 按 source 计数
    pub by_source: Vec<(String, i64)>,
    pub with_embedding: i64,
    /// 容量上限（store::MAX_MEM_ITEMS，前端显示 N/500）
    pub capacity: i64,
    pub embed_ok: bool,
    pub embed_error: Option<String>,
}

/// 编辑态允许的记忆类型白名单（mem_items.kind 契约全集）
const EDITABLE_KINDS: [&str; 7] = [
    "profile",
    "preference",
    "fact",
    "event",
    "summary",
    "reflection",
    "lesson",
];

// ───────────────────────── 内核（&Connection，可单测） ─────────────────────────

/// 列表：无 query 按 updated_at 倒序全量；有 query 走混合检索（纯读不刷访问计数）。
/// kind 过滤（哨兵 "all" / None = 不过滤；读路径有意宽容未知 kind——新类型不被
/// 面板挡，与 update_core 的写入侧白名单不同职责）先于检索——搜索结果尊重类型筛选。
pub(crate) fn list_core(
    conn: &rusqlite::Connection,
    query: Option<&str>,
    query_emb: Option<&[f32]>,
    kind: Option<&str>,
    now_ms: i64,
) -> Result<Vec<MemItemView>, String> {
    store::ensure_table(conn)?;
    let items = store::load_all(conn)?;
    let filtered: Vec<MemItem> = match kind {
        Some(k) if !k.is_empty() && k != "all" => {
            items.into_iter().filter(|m| m.kind == k).collect()
        }
        _ => items,
    };
    let list: Vec<MemItem> = match query.filter(|q| !q.trim().is_empty()) {
        Some(q) => rank::hybrid_search(&filtered, q, query_emb, now_ms, filtered.len().max(1)),
        None => {
            let mut v = filtered;
            v.sort_by(|a, b| b.updated_at_ms.cmp(&a.updated_at_ms));
            v
        }
    };
    Ok(list.iter().map(MemItemView::from).collect())
}

/// 编辑保存：content 必填 ≤800 字、kind 白名单、条目须存在；importance 由
/// update_by_id 钳制 1..=5。tags/source 不可编辑（key 覆盖语义与来源审计不被
/// 面板破坏）；content 变更的向量重算由调用方传入新向量（update_by_id 的
/// CASE 语义：内容没变时保留旧向量）。
pub(crate) fn update_core(
    conn: &rusqlite::Connection,
    id: &str,
    content: &str,
    importance: i64,
    kind: &str,
    embedding: Option<&[f32]>,
    now_ms: i64,
) -> Result<MemItemView, String> {
    store::ensure_table(conn)?;
    let content = content.trim();
    if content.is_empty() {
        return Err("记忆内容不能为空（要删除请用删除按钮）".into());
    }
    if content.chars().count() > MAX_CONTENT_CHARS {
        return Err(format!("记忆内容太长（≤{MAX_CONTENT_CHARS} 字）"));
    }
    if !EDITABLE_KINDS.contains(&kind) {
        return Err(format!("未知记忆类型：{kind}"));
    }
    let existing = store::load_all(conn)?
        .into_iter()
        .find(|m| m.id == id)
        .ok_or_else(|| "记忆不存在或已被删除".to_string())?;
    store::update_by_id(
        conn,
        id,
        content,
        importance,
        &existing.source,
        kind,
        embedding,
        now_ms,
    )?;
    let updated = store::load_all(conn)?
        .into_iter()
        .find(|m| m.id == id)
        .ok_or_else(|| "更新后读取失败".to_string())?;
    Ok(MemItemView::from(&updated))
}

/// 删除单条（显式用户操作允许删任何条目，含 evo: 前缀的自进化教训——
/// 前端 confirm 文案负责提示影响）。返回是否真删到。
pub(crate) fn delete_core(conn: &rusqlite::Connection, id: &str) -> Result<bool, String> {
    store::ensure_table(conn)?;
    Ok(store::delete_by_ids(conn, &[id.to_string()])? > 0)
}

/// 统计（SQL 聚合——不在写锁内反序列化全表 ~1MB 向量；引擎状态由命令层填）。
/// with_embedding 按「embedding 列非 NULL」口径：写入侧只落合法向量，
/// 不存在「有 blob 但解析失败」的行。
pub(crate) fn stats_core(conn: &rusqlite::Connection) -> Result<MemStats, String> {
    store::ensure_table(conn)?;
    let mut stats = MemStats {
        capacity: store::MAX_MEM_ITEMS,
        ..Default::default()
    };
    stats.total = conn
        .query_row("SELECT COUNT(*) FROM mem_items", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    stats.with_embedding = conn
        .query_row(
            "SELECT COUNT(*) FROM mem_items WHERE embedding IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    for (col, dest) in [
        ("kind", &mut stats.by_kind),
        ("source", &mut stats.by_source),
    ] {
        let sql = format!("SELECT {col}, COUNT(*) FROM mem_items GROUP BY {col}");
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| e.to_string())?;
        let mut m: std::collections::BTreeMap<String, i64> = Default::default();
        for row in rows {
            let (k, n) = row.map_err(|e| e.to_string())?;
            *m.entry(k).or_default() += n;
        }
        *dest = m.into_iter().collect();
    }
    Ok(stats)
}

// ───────────────────────── tauri 命令 ─────────────────────────

/// 写闸获取（DB_WRITE_LOCK 是 Mutex<()> 写入闸，连接在闸内现开——同 memory/mod.rs 口径）
fn lock_db() -> std::sync::MutexGuard<'static, ()> {
    crate::db::DB_WRITE_LOCK.lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] memory::panel DB_WRITE_LOCK: {e:?}");
        e.into_inner()
    })
}

/// 列表 + 可选混合检索（面板搜索）。纯读：不刷新 access_count。
#[tauri::command]
pub async fn mem_list(
    app: AppHandle,
    query: Option<String>,
    kind: Option<String>,
) -> CommandResult<Vec<MemItemView>> {
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<Vec<MemItemView>, String> {
        let q = query.as_deref().map(str::trim).filter(|s| !s.is_empty());
        // 检索向量在持锁前算（ONNX 推理不占 DB 写锁临界区）
        let emb = q.and_then(|s| embed::embed_text(s));
        let _g = lock_db();
        let conn = crate::db::open_db(&app).map_err(|e| e.to_string())?;
        list_core(&conn, q, emb.as_deref(), kind.as_deref(), now_ms())
    })
    .await;
    r.map_err(|e| CommandError::from(format!("记忆列表线程 join 失败：{e}")))?
        .map_err(CommandError::DbError)
}

/// 编辑保存（content/importance/kind；tags 与 source 不可改）。
/// content 变更时重算嵌入（持锁前算）。
#[tauri::command]
pub async fn mem_update(
    app: AppHandle,
    id: String,
    content: String,
    importance: i64,
    kind: String,
) -> CommandResult<MemItemView> {
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<MemItemView, String> {
        let emb = embed::embed_text(&content);
        let _g = lock_db();
        let conn = crate::db::open_db(&app).map_err(|e| e.to_string())?;
        update_core(
            &conn,
            &id,
            &content,
            importance,
            &kind,
            emb.as_deref(),
            now_ms(),
        )
    })
    .await;
    r.map_err(|e| CommandError::from(format!("记忆更新线程 join 失败：{e}")))?
        .map_err(CommandError::DbError)
}

/// 删除单条。返回是否真删到（false = 本来就不存在）。
#[tauri::command]
pub async fn mem_delete(app: AppHandle, id: String) -> CommandResult<bool> {
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<bool, String> {
        let _g = lock_db();
        let conn = crate::db::open_db(&app).map_err(|e| e.to_string())?;
        delete_core(&conn, &id)
    })
    .await;
    r.map_err(|e| CommandError::from(format!("记忆删除线程 join 失败：{e}")))?
        .map_err(CommandError::DbError)
}

// ───────────────────────── 导出 / 导入（U15） ─────────────────────────

/// 导出文件版本（结构变更 +1；导入端兼容 ≤ 当前版本）
pub const MEM_EXPORT_VERSION: u32 = 1;

/// 导出条目：不带 id（导入端 insert_item 重新生成，原 id 是内部 uuid）；
/// 向量随行带出（JSON 浮点数组）——导入机即使引擎降级也不丢语义检索。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MemExportItem {
    pub kind: String,
    pub content: String,
    pub tags: Vec<String>,
    pub importance: i64,
    pub source: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub access_count: i64,
    pub last_accessed_at: Option<i64>,
    pub embedding: Option<Vec<f32>>,
}

/// 导出文件顶层结构
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MemExportFile {
    pub version: u32,
    pub exported_at: i64,
    pub count: usize,
    pub items: Vec<MemExportItem>,
}

/// 导入报告：inserted 新增 / merged 语义去重合并 / skipped 跳过（空内容、
/// 容量满拒写、evo: 异 key 冲突拒写、未知结构行）
#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemImportReport {
    pub inserted: usize,
    pub merged: usize,
    pub skipped: usize,
}

/// 导出报告（轻量：前端只读 count；完整文件已落盘，不把 ~1MB items 走 IPC 回传）
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MemExportReport {
    pub count: usize,
}

/// 导出内核（纯结构构建：序列化在命令层锁外做，不占 DB 写锁临界区）
pub(crate) fn export_core(items: &[MemItem], exported_at: i64) -> MemExportFile {
    MemExportFile {
        version: MEM_EXPORT_VERSION,
        exported_at,
        count: items.len(),
        items: items
            .iter()
            .map(|m| MemExportItem {
                kind: m.kind.clone(),
                content: m.content.clone(),
                tags: m.tags.clone(),
                importance: m.importance,
                source: m.source.clone(),
                created_at: m.created_at_ms,
                updated_at: m.updated_at_ms,
                access_count: m.access_count,
                last_accessed_at: m.last_accessed_at_ms,
                embedding: m.embedding.clone(),
            })
            .collect(),
    }
}

/// 导入文件解析 + 版本守卫（锁外做：文件 IO / JSON 解析不占 DB 写锁临界区）
pub(crate) fn parse_import(json: &str) -> Result<MemExportFile, String> {
    let file: MemExportFile =
        serde_json::from_str(json).map_err(|e| format!("导入文件解析失败：{e}"))?;
    if file.version > MEM_EXPORT_VERSION {
        return Err(format!(
            "导入文件版本过新（v{}，本机支持 ≤v{MEM_EXPORT_VERSION}），请先升级应用",
            file.version
        ));
    }
    Ok(file)
}

/// 导入前置（锁外做全量输入级校验 + 预嵌入）：内容/kind/source 契约外的条目
/// 直接计 skipped，不做无谓 ONNX 推理。返回 (合法条目+向量, skipped 数)。
/// 有向量的条目原样带过，无向量的现场重算（降级时 None = 关键词模式写入）。
fn prepare_import_items(
    items: &[MemExportItem],
    reembed: &dyn Fn(&str) -> Option<Vec<f32>>,
) -> (Vec<(MemExportItem, Option<Vec<f32>>)>, usize) {
    let mut valid = Vec::new();
    let mut skipped = 0usize;
    for it in items {
        let content = it.content.trim();
        if content.is_empty() || content.chars().count() > MAX_CONTENT_CHARS {
            skipped += 1;
            continue;
        }
        if !EDITABLE_KINDS.contains(&it.kind.as_str()) {
            skipped += 1;
            continue;
        }
        // source 三值契约 + 历史脏值 'user' 归一（与 store::normalize_source 同口径）
        let source = match it.source.as_str() {
            "user_stated" | "model_inferred" | "system" => it.source.clone(),
            "user" => "user_stated".to_string(),
            _ => {
                skipped += 1;
                continue;
            }
        };
        let emb = it.embedding.clone().or_else(|| reembed(content));
        valid.push((
            MemExportItem {
                source,
                ..it.clone()
            },
            emb,
        ));
    }
    (valid, skipped)
}

/// 导入内核（&Connection 可单测）：合法条目逐条走 insert_item 既有语义去重。
/// 只增不删。skipped 口径：容量满拒写、evo: 异 key 冲突拒写（输入级校验已在
/// prepare 完成）；**存储故障（Err）不算 skipped——直接中止并上抛**，
/// 不把真实故障伪装成「输入被跳过」。
pub(crate) fn import_items(
    conn: &rusqlite::Connection,
    prepared: &[(MemExportItem, Option<Vec<f32>>)],
    now_ms: i64,
) -> Result<MemImportReport, String> {
    store::ensure_table(conn)?;
    let mut report = MemImportReport::default();
    for (item, emb) in prepared {
        let draft = store::NewItem {
            kind: item.kind.clone(),
            content: item.content.trim().to_string(),
            tags: item.tags.clone(),
            importance: item.importance,
            source: item.source.clone(),
        };
        match store::insert_item(conn, &draft, emb.as_deref(), now_ms) {
            Ok((store::InsertOutcome::Inserted(_), _)) => report.inserted += 1,
            Ok((store::InsertOutcome::Merged { .. }, _)) => report.merged += 1,
            Ok((store::InsertOutcome::RejectedFull(_), _)) => report.skipped += 1,
            Ok((store::InsertOutcome::RefusedForeignMerge { .. }, _)) => report.skipped += 1,
            Err(e) => {
                return Err(format!(
                    "导入中止（存储故障，已完成 {} 条）：{e}",
                    report.inserted + report.merged
                ))
            }
        }
    }
    Ok(report)
}

/// 导入便捷入口（单测用）：json → 解析 → 前置校验+预嵌入 → 逐条导入
pub(crate) fn import_core(
    conn: &rusqlite::Connection,
    json: &str,
    reembed: &dyn Fn(&str) -> Option<Vec<f32>>,
    now_ms: i64,
) -> Result<MemImportReport, String> {
    let file = parse_import(json)?;
    let (prepared, skipped) = prepare_import_items(&file.items, reembed);
    let mut report = import_items(conn, &prepared, now_ms)?;
    report.skipped += skipped;
    Ok(report)
}

/// 记忆库统计 + 嵌入引擎状态（首次调用触发引擎懒加载，与注入路径同一份缓存）
#[tauri::command]
pub async fn mem_stats(app: AppHandle) -> CommandResult<MemStats> {
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<MemStats, String> {
        let mut stats = {
            let _g = lock_db();
            let conn = crate::db::open_db(&app).map_err(|e| e.to_string())?;
            stats_core(&conn)?
        };
        match embed::engine_status() {
            Ok(()) => stats.embed_ok = true,
            Err(e) => stats.embed_error = Some(e),
        }
        Ok(stats)
    })
    .await;
    r.map_err(|e| CommandError::from(format!("记忆统计线程 join 失败：{e}")))?
        .map_err(CommandError::DbError)
}

/// 导入/导出路径守卫：仅接受 .json（对话框流程默认带后缀；裸 String 路径
/// 加这道最低成本闸，防误写/误读任意非 JSON 文件）。更细的白名单登记缓议。
fn ensure_json_path(path: &str) -> CommandResult<()> {
    if !path.to_lowercase().ends_with(".json") {
        return Err(CommandError::InvalidArgument {
            field: "path".into(),
            value: path.into(),
            reason: "仅支持 .json 文件".into(),
        });
    }
    Ok(())
}

/// 导出全量记忆到 JSON 文件（路径经前端 plugin-dialog save 获取，Rust 侧写文件）。
/// 向量随行带出——导入机即使引擎降级也不丢语义检索。
/// 锁内只做 DB 读取；序列化、原子写（临时文件 → rename，防半截 JSON）都在锁外。
#[tauri::command]
pub async fn mem_export(app: AppHandle, path: String) -> CommandResult<MemExportReport> {
    ensure_json_path(&path)?;
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<MemExportReport, String> {
        let file = {
            let _g = lock_db();
            let conn = crate::db::open_db(&app).map_err(|e| e.to_string())?;
            store::ensure_table(&conn)?;
            let items = store::load_all(&conn)?;
            export_core(&items, now_ms())
        };
        let json =
            serde_json::to_string_pretty(&file).map_err(|e| format!("导出序列化失败：{e}"))?;
        crate::db::atomic_write(std::path::Path::new(&path), &json)
            .map_err(|e| format!("写入导出文件失败：{e}"))?;
        Ok(MemExportReport { count: file.count })
    })
    .await;
    r.map_err(|e| CommandError::from(format!("记忆导出线程 join 失败：{e}")))?
        .map_err(CommandError::from)
}

/// 从 JSON 文件导入记忆（只增不删，走既有语义去重合并；容量满按 skipped 计）。
/// 解析、输入校验与预嵌入在锁外（文件 IO + ONNX 推理不占 DB 写锁临界区）。
#[tauri::command]
pub async fn mem_import(app: AppHandle, path: String) -> CommandResult<MemImportReport> {
    ensure_json_path(&path)?;
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<MemImportReport, String> {
        let json = std::fs::read_to_string(&path).map_err(|e| format!("读取导入文件失败：{e}"))?;
        let file = parse_import(&json)?;
        let (prepared, skipped) = prepare_import_items(&file.items, &|t| embed::embed_text(t));
        let _g = lock_db();
        let conn = crate::db::open_db(&app).map_err(|e| e.to_string())?;
        let mut report = import_items(&conn, &prepared, now_ms())?;
        report.skipped += skipped;
        Ok(report)
    })
    .await;
    r.map_err(|e| CommandError::from(format!("记忆导入线程 join 失败：{e}")))?
        .map_err(CommandError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::NewItem;

    fn mem_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        store::ensure_table(&conn).unwrap();
        conn
    }

    fn new_item(kind: &str, content: &str, source: &str) -> NewItem {
        NewItem {
            kind: kind.to_string(),
            content: content.to_string(),
            tags: Vec::new(),
            importance: 3,
            source: source.to_string(),
        }
    }

    /// 512 维单热向量（第 i 维为 1，与 memory/tests.rs 同款）
    fn onehot(i: usize) -> Vec<f32> {
        let mut v = vec![0f32; 512];
        v[i % 512] = 1.0;
        v
    }

    fn insert(conn: &rusqlite::Connection, item: &NewItem, emb: Option<&[f32]>, ms: i64) -> String {
        match store::insert_item(conn, item, emb, ms).unwrap() {
            (store::InsertOutcome::Inserted(m), _) => m.id,
            other => panic!("应直接插入：{other:?}"),
        }
    }

    #[test]
    fn view_flags_embedding_without_leaking_vector() {
        let conn = mem_db();
        let a = insert(
            &conn,
            &new_item("fact", "有向量的记忆", "user_stated"),
            Some(&onehot(0)),
            1_000,
        );
        let b = insert(
            &conn,
            &new_item("fact", "无向量的记忆", "system"),
            None,
            2_000,
        );
        let views = list_core(&conn, None, None, None, 3_000).unwrap();
        assert_eq!(views.len(), 2);
        let va = views.iter().find(|v| v.id == a).unwrap();
        let vb = views.iter().find(|v| v.id == b).unwrap();
        assert!(va.has_embedding);
        assert!(!vb.has_embedding);
        let raw = serde_json::to_string(va).unwrap();
        assert!(
            !raw.contains("embedding"),
            "视图不得泄漏向量本体字段：{raw}"
        );
        assert!(raw.contains("\"hasEmbedding\":true"));
        assert!(raw.contains("\"createdAt\":1000"), "camelCase 序列化");
    }

    #[test]
    fn list_orders_by_update_desc_and_filters_kind() {
        let conn = mem_db();
        insert(
            &conn,
            &new_item("fact", "旧条目", "user_stated"),
            None,
            1_000,
        );
        insert(
            &conn,
            &new_item("preference", "新条目", "user_stated"),
            None,
            2_000,
        );
        let all = list_core(&conn, None, None, None, 3_000).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].content, "新条目", "按 updated_at 倒序");
        let prefs = list_core(&conn, None, None, Some("preference"), 3_000).unwrap();
        assert_eq!(prefs.len(), 1);
        assert_eq!(prefs[0].content, "新条目");
        assert!(list_core(&conn, None, None, Some("lesson"), 3_000)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn list_search_hits_semantic_without_touching_access_count() {
        let conn = mem_db();
        // 查询向量 = onehot(0)：命中带 onehot(0) 的条目；另一条无向量且关键词零重合 → 0 分淘汰
        let hit = insert(
            &conn,
            &new_item("fact", "甲乙丙丁", "user_stated"),
            Some(&onehot(0)),
            1_000,
        );
        let miss = insert(
            &conn,
            &new_item("fact", "戊己庚辛", "user_stated"),
            None,
            2_000,
        );
        // 查询词与两条内容都无 bigram 重合，纯语义命中
        let views = list_core(&conn, Some("西北方向"), Some(&onehot(0)), None, 3_000).unwrap();
        assert_eq!(views.len(), 1, "零分条目不得混进搜索结果");
        assert_eq!(views[0].id, hit);
        assert_ne!(views[0].id, miss);
        // 纯读：命中条目 access_count 不变（访问强化只属于聊天注入）
        let raw = store::load_all(&conn)
            .unwrap()
            .into_iter()
            .find(|m| m.id == hit)
            .unwrap();
        assert_eq!(raw.access_count, 0);
        assert_eq!(raw.last_accessed_at_ms, None);
    }

    #[test]
    fn update_edits_fields_and_reruns_vector_on_content_change() {
        let conn = mem_db();
        let id = insert(
            &conn,
            &new_item("fact", "原始内容", "user_stated"),
            Some(&onehot(1)),
            1_000,
        );
        let v = update_core(
            &conn,
            &id,
            "改过的内容",
            5,
            "preference",
            Some(&onehot(2)),
            2_000,
        )
        .unwrap();
        assert_eq!(v.content, "改过的内容");
        assert_eq!(v.kind, "preference");
        assert_eq!(v.importance, 5);
        let raw = store::load_all(&conn)
            .unwrap()
            .into_iter()
            .find(|m| m.id == id)
            .unwrap();
        assert_eq!(raw.kind, "preference");
        // content 变了 → 向量以新值为准（update_by_id CASE 语义）
        assert_eq!(raw.embedding.as_deref(), Some(&onehot(2)[..]));
        // importance=5 + 用户口述 → 受保护（面板置顶语义 = importance 调到 5）
        assert!(store::is_protected(&raw));
        // content 不变、仅动类型 → 传 None 保留旧向量
        update_core(&conn, &id, "改过的内容", 4, "profile", None, 3_000).unwrap();
        let raw = store::load_all(&conn)
            .unwrap()
            .into_iter()
            .find(|m| m.id == id)
            .unwrap();
        assert_eq!(
            raw.embedding.as_deref(),
            Some(&onehot(2)[..]),
            "内容未变保留旧向量"
        );
        assert_eq!(raw.source, "user_stated", "source 不可被面板改写");
    }

    #[test]
    fn update_validates_content_kind_and_existence() {
        let conn = mem_db();
        let id = insert(&conn, &new_item("fact", "内容", "user_stated"), None, 1_000);
        assert!(update_core(&conn, &id, "   ", 3, "fact", None, 2_000)
            .unwrap_err()
            .contains("不能为空"));
        let long = "长".repeat(MAX_CONTENT_CHARS + 1);
        assert!(update_core(&conn, &id, &long, 3, "fact", None, 2_000)
            .unwrap_err()
            .contains("太长"));
        assert!(update_core(&conn, &id, "内容", 3, "magic", None, 2_000)
            .unwrap_err()
            .contains("未知记忆类型"));
        assert!(
            update_core(&conn, "missing", "内容", 3, "fact", None, 2_000)
                .unwrap_err()
                .contains("不存在")
        );
    }

    #[test]
    fn delete_reports_whether_deleted() {
        let conn = mem_db();
        let id = insert(&conn, &new_item("fact", "待删", "user_stated"), None, 1_000);
        assert!(delete_core(&conn, &id).unwrap());
        assert!(!delete_core(&conn, &id).unwrap(), "重复删除 = false");
        assert!(store::load_all(&conn).unwrap().is_empty());
    }

    #[test]
    fn stats_counts_kind_source_and_embeddings() {
        let conn = mem_db();
        insert(
            &conn,
            &new_item("fact", "a", "user_stated"),
            Some(&onehot(0)),
            1_000,
        );
        insert(&conn, &new_item("fact", "b", "model_inferred"), None, 1_100);
        insert(&conn, &new_item("lesson", "c", "system"), None, 1_200);
        let stats = stats_core(&conn).unwrap();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.capacity, store::MAX_MEM_ITEMS);
        assert_eq!(stats.with_embedding, 1);
        assert_eq!(
            stats.by_kind,
            vec![("fact".into(), 2), ("lesson".into(), 1)]
        );
        assert_eq!(
            stats.by_source,
            vec![
                ("model_inferred".into(), 1),
                ("system".into(), 1),
                ("user_stated".into(), 1),
            ]
        );
        // 引擎状态由命令层填，内核不碰 ONNX
        assert!(!stats.embed_ok);
        assert_eq!(stats.embed_error, None);
    }

    #[test]
    fn legacy_user_source_normalized_and_protected() {
        // 历史脏数据（source='user'，三值契约之前写入）：读取侧归一为 user_stated——
        // 否则 importance=5 的口述条目进不了 is_protected，会被容量淘汰误删。
        // 时间戳用 store::ts_to_text 构造（存储格式变了一处跟随）。
        let conn = mem_db();
        let ts = store::ts_to_text(1_000);
        conn.execute(
            "INSERT INTO mem_items (id, kind, content, tags, importance, source, created_at, updated_at)
             VALUES ('legacy', 'preference', '历史口述偏好', 'k', 5, 'user', ?1, ?1)",
            rusqlite::params![ts],
        )
        .unwrap();
        let raw = store::load_all(&conn).unwrap();
        assert_eq!(raw.len(), 1);
        assert_eq!(raw[0].source, "user_stated", "读取侧归一历史脏值");
        assert!(store::is_protected(&raw[0]), "归一后享受保护语义");
        let views = list_core(&conn, None, None, None, 3_000).unwrap();
        assert_eq!(views[0].source, "user_stated");
    }

    #[test]
    fn export_roundtrip_preserves_fields_and_vector() {
        let conn = mem_db();
        insert(
            &conn,
            &new_item("preference", "喜欢简洁的回复风格", "user_stated"),
            Some(&onehot(0)),
            1_000,
        );
        let items = store::load_all(&conn).unwrap();
        let file = export_core(&items, 5_000);
        assert_eq!(file.version, MEM_EXPORT_VERSION);
        assert_eq!(file.exported_at, 5_000);
        assert_eq!(file.count, 1);
        let it = &file.items[0];
        assert_eq!(it.content, "喜欢简洁的回复风格");
        assert_eq!(it.source, "user_stated");
        assert_eq!(
            it.embedding.as_deref(),
            Some(&onehot(0)[..]),
            "向量随行带出"
        );
        let json = serde_json::to_string_pretty(&file).unwrap();
        // 导入端：空库首导 → 新增；原样重导同一文件 → 语义去重合并（幂等，不堆积）
        let dest = mem_db();
        let report = import_core(&dest, &json, &|_| None, 6_000).unwrap();
        assert_eq!(
            report,
            MemImportReport {
                inserted: 1,
                merged: 0,
                skipped: 0,
            }
        );
        let report2 = import_core(&dest, &json, &|_| None, 7_000).unwrap();
        assert_eq!(
            report2,
            MemImportReport {
                inserted: 0,
                merged: 1,
                skipped: 0,
            },
            "重导入应走语义合并"
        );
        assert_eq!(store::load_all(&dest).unwrap().len(), 1);
    }

    #[test]
    fn import_reembeds_and_counts_skips() {
        let conn = mem_db();
        // 无向量条目 → 导入端现场重算；契约外条目 → skipped：
        // 空内容 / 超长 / 未知 kind / 未知 source；'user' 历史脏值归一后入库
        let file = MemExportFile {
            version: MEM_EXPORT_VERSION,
            exported_at: 1_000,
            count: 6,
            items: vec![
                MemExportItem {
                    kind: "fact".into(),
                    content: "导入的新知识".into(),
                    tags: vec!["k".into()],
                    importance: 3,
                    source: "user_stated".into(),
                    created_at: 1_000,
                    updated_at: 1_000,
                    access_count: 0,
                    last_accessed_at: None,
                    embedding: None,
                },
                MemExportItem {
                    kind: "fact".into(),
                    content: "   ".into(),
                    tags: vec![],
                    importance: 3,
                    source: "user_stated".into(),
                    created_at: 1_000,
                    updated_at: 1_000,
                    access_count: 0,
                    last_accessed_at: None,
                    embedding: None,
                },
                MemExportItem {
                    kind: "fact".into(),
                    content: "超".repeat(MAX_CONTENT_CHARS + 1),
                    tags: vec![],
                    importance: 3,
                    source: "user_stated".into(),
                    created_at: 1_000,
                    updated_at: 1_000,
                    access_count: 0,
                    last_accessed_at: None,
                    embedding: None,
                },
                MemExportItem {
                    kind: "magic".into(),
                    content: "未知类型不落库".into(),
                    tags: vec![],
                    importance: 3,
                    source: "user_stated".into(),
                    created_at: 1_000,
                    updated_at: 1_000,
                    access_count: 0,
                    last_accessed_at: None,
                    embedding: None,
                },
                MemExportItem {
                    kind: "fact".into(),
                    content: "未知来源不落库".into(),
                    tags: vec![],
                    importance: 3,
                    source: "garbage".into(),
                    created_at: 1_000,
                    updated_at: 1_000,
                    access_count: 0,
                    last_accessed_at: None,
                    embedding: None,
                },
                MemExportItem {
                    kind: "preference".into(),
                    content: "历史脏值来源归一".into(),
                    tags: vec![],
                    importance: 3,
                    source: "user".into(),
                    created_at: 1_000,
                    updated_at: 1_000,
                    access_count: 0,
                    last_accessed_at: None,
                    embedding: None,
                },
            ],
        };
        let json = serde_json::to_string(&file).unwrap();
        // 预嵌入只收内容合法的条目（空/超长必跳过不做推理；kind/source 契约
        // 校验在 import_items 阶段，此处不感知）。逐条发不同向量——相同向量
        // 会 ≥0.92 互相合并，测不出「两条都入库」。
        let reembed_calls = std::sync::Mutex::new(0u32);
        let report = import_core(
            &conn,
            &json,
            &|c| {
                let n = {
                    let mut g = reembed_calls.lock().unwrap();
                    let n = *g;
                    *g += 1;
                    n
                };
                Some(onehot(3 + n as usize))
            },
            2_000,
        )
        .unwrap();
        assert_eq!(*reembed_calls.lock().unwrap(), 2, "预嵌入只收内容合法条目");
        assert_eq!(report.inserted, 2);
        assert_eq!(report.skipped, 4);
        let raw = store::load_all(&conn).unwrap();
        assert_eq!(raw.len(), 2);
        let reembedded = raw
            .iter()
            .find(|m| m.content == "导入的新知识")
            .expect("应入库");
        assert_eq!(
            reembedded.embedding.as_deref(),
            Some(&onehot(3)[..]),
            "重算向量落库"
        );
        let normalized = raw
            .iter()
            .find(|m| m.content == "历史脏值来源归一")
            .expect("应入库");
        assert_eq!(normalized.source, "user_stated", "'user' 历史脏值归一");
    }

    #[test]
    fn import_rejects_future_version() {
        let conn = mem_db();
        let file = format!(
            r#"{{"version":{},"exportedAt":1,"count":0,"items":[]}}"#,
            MEM_EXPORT_VERSION + 1
        );
        let err = import_core(&conn, &file, &|_| None, 1_000).unwrap_err();
        assert!(err.contains("版本过新"), "{err}");
    }
}
