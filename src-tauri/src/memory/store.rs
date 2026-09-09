//! 记忆 v2 存储（2026-09-09，设计 docs/BOT-MEMORY-V2-DESIGN.md）：
//! 新表 mem_items（不动旧表 bot_facts 的数据），统一容量上限 500 条，
//! 语义去重（余弦阈值合并/提示）+ 受保护条目免淘汰。
//!
//! 纯函数取 &Connection（内存库可单测，与 db.rs fact_* 同先例）；
//! 锁/open_db/AppHandle 包装在 memory/mod.rs 门面层。

/// 统一容量上限（修掉旧系统 fact 200 / 全表 300 的双层上限分裂）
pub const MAX_MEM_ITEMS: i64 = 500;

/// 语义去重阈值：余弦 ≥ MERGE 视为同一条 → 合并更新，不新增
pub const DEDUP_MERGE_COSINE: f64 = 0.92;
/// 冲突提示区间：HINT ≤ 余弦 < MERGE → 不拦截，把相似条目拼进工具结果让模型裁决
pub const DEDUP_HINT_COSINE: f64 = 0.75;

/// 冲突提示最多带几条相似记忆
const CONFLICT_HINT_TOP: usize = 3;

/// 记忆条目（mem_items 行）
#[derive(Clone, Debug)]
pub struct MemItem {
    pub id: String,
    pub kind: String, // profile|preference|fact|event|summary|reflection
    pub content: String,
    pub tags: Vec<String>,
    pub importance: i64, // 1-5
    pub source: String,  // user_stated|model_inferred|system
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub access_count: i64,
    pub last_accessed_at_ms: Option<i64>,
    /// 512 维 L2 归一化向量；None = 降级模式写入/导入失败
    pub embedding: Option<Vec<f32>>,
}

/// 建表（幂等；open_db 后每次调用都跑一次 IF NOT EXISTS，与旧系统 ensure_* 同模式）
pub fn ensure_table(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS mem_items(
           id TEXT PRIMARY KEY,
           kind TEXT NOT NULL,
           content TEXT NOT NULL,
           tags TEXT NOT NULL DEFAULT '',
           importance INTEGER NOT NULL DEFAULT 3,
           source TEXT NOT NULL DEFAULT 'model_inferred',
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           access_count INTEGER NOT NULL DEFAULT 0,
           last_accessed_at TEXT,
           embedding BLOB
         );",
    )
    .map_err(|e| e.to_string())
}

/// ms 时间戳 → TEXT（RFC3339；表契约是 TEXT，排序/解析均安全）
pub fn ts_to_text(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

/// TEXT → ms（解析失败归 0 = 最旧，参与淘汰/衰减时不会虚增价值）
pub fn ts_to_ms(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(0)
}

fn embedding_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn blob_to_embedding(b: &[u8]) -> Option<Vec<f32>> {
    if b.len() % 4 != 0 || b.is_empty() {
        return None;
    }
    Some(
        b.chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
    )
}

/// 全表读取（≤500 条 × ~2KB 向量，微秒级；不引 sqlite-vec 扩展）
pub fn load_all(conn: &rusqlite::Connection) -> Result<Vec<MemItem>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, kind, content, tags, importance, source, created_at, updated_at,
                    access_count, last_accessed_at, embedding FROM mem_items",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(MemItem {
                id: r.get(0)?,
                kind: r.get(1)?,
                content: r.get(2)?,
                tags: {
                    let t: String = r.get(3)?;
                    t.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                },
                importance: r.get(4)?,
                source: r.get(5)?,
                created_at_ms: ts_to_ms(&r.get::<_, String>(6)?),
                updated_at_ms: ts_to_ms(&r.get::<_, String>(7)?),
                access_count: r.get(8)?,
                last_accessed_at_ms: r
                    .get::<_, Option<String>>(9)?
                    .map(|s| ts_to_ms(&s))
                    .filter(|ms| *ms > 0),
                embedding: r
                    .get::<_, Option<Vec<u8>>>(10)?
                    .and_then(|b| blob_to_embedding(&b)),
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// 余弦相似度（向量均假定已 L2 归一化；维度不齐/缺失 → None）
pub fn cosine(a: Option<&[f32]>, b: Option<&[f32]>) -> Option<f64> {
    let (a, b) = (a?, b?);
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let dot: f64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| (*x as f64) * (*y as f64))
        .sum();
    let na = a.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    let nb = b.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return None;
    }
    Some(dot / (na * nb))
}

/// 受保护条目：importance=5 且用户口述 —— 永不被容量淘汰
pub fn is_protected(item: &MemItem) -> bool {
    item.importance >= 5 && item.source == "user_stated"
}

/// 淘汰分（越低越先淘汰）：importance 权重最高 + 最近访问/更新新近度 + 访问次数
pub(crate) fn evict_score(item: &MemItem, now_ms: i64) -> f64 {
    let base = item.last_accessed_at_ms.unwrap_or(item.updated_at_ms).max(item.created_at_ms);
    let age_days = ((now_ms - base) as f64 / 86_400_000.0).max(0.0);
    item.importance as f64 * 2.0
        + (-age_days / 30.0).exp()
        + (1.0 + item.access_count.max(0) as f64).ln() / 5.0
}

/// 容量淘汰：达上限时删淘汰分最低的非保护条目；无可淘汰 → Err（拒写，信息给模型）
fn evict_if_needed(conn: &rusqlite::Connection, now_ms: i64) -> Result<(), String> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM mem_items", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if count < MAX_MEM_ITEMS {
        return Ok(());
    }
    let all = load_all(conn)?;
    let victim = all
        .iter()
        .filter(|m| !is_protected(m))
        .min_by(|a, b| {
            evict_score(a, now_ms)
                .partial_cmp(&evict_score(b, now_ms))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|m| m.id.clone());
    match victim {
        Some(id) => {
            conn.execute("DELETE FROM mem_items WHERE id = ?1", [&id])
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        None => Err(format!(
            "记忆已达 {MAX_MEM_ITEMS} 条上限且剩余全是受保护的重要记忆（importance=5 用户口述），请先删除不需要的"
        )),
    }
}

/// 插入结果（remember/导入路径据此拼工具结果文本）
#[derive(Debug)]
pub enum InsertOutcome {
    /// 新增成功
    Inserted(MemItem),
    /// 语义去重合并（余弦 ≥0.92）：刷新已有条目的 content/updated_at/access，未新增
    Merged(MemItem),
    /// 容量满且无可淘汰条目 → 拒写
    RejectedFull(String),
}

/// 新条目草稿
pub struct NewItem {
    pub kind: String,
    pub content: String,
    pub tags: Vec<String>,
    pub importance: i64,
    pub source: String,
}

/// 写入（语义去重 + 容量淘汰）：
/// - embedding 为 Some 时先全表余弦去重：≥0.92 合并更新已有条目；0.75~0.92 仅记录为
///   冲突提示（返回值带出，不拦截）
/// - embedding 为 None（降级模式）跳过去重直接插入
/// 返回 (结果, 冲突提示列表[content 摘要])
pub fn insert_item(
    conn: &rusqlite::Connection,
    item: &NewItem,
    embedding: Option<&[f32]>,
    now_ms: i64,
) -> Result<(InsertOutcome, Vec<String>), String> {
    ensure_table(conn)?;
    let all = load_all(conn)?;
    // 语义去重（仅有向量时；降级模式无余弦可算，跳过）
    if let Some(emb) = embedding {
        let mut best: Option<(f64, &MemItem)> = None;
        let mut hints: Vec<(f64, &MemItem)> = Vec::new();
        for m in &all {
            if let Some(c) = cosine(Some(emb), m.embedding.as_deref()) {
                if c >= DEDUP_MERGE_COSINE {
                    if best.map_or(true, |(s, _)| c > s) {
                        best = Some((c, m));
                    }
                } else if c >= DEDUP_HINT_COSINE {
                    hints.push((c, m));
                }
            }
        }
        if let Some((_, target)) = best {
            conn.execute(
                "UPDATE mem_items SET content = ?1, tags = ?2, importance = ?3, source = ?4,
                        updated_at = ?5, access_count = access_count + 1, last_accessed_at = ?5,
                        embedding = ?6
                 WHERE id = ?7",
                rusqlite::params![
                    item.content,
                    item.tags.join(","),
                    item.importance.clamp(1, 5),
                    item.source,
                    ts_to_text(now_ms),
                    embedding_to_blob(emb),
                    target.id,
                ],
            )
            .map_err(|e| e.to_string())?;
            let merged = MemItem {
                content: item.content.clone(),
                tags: item.tags.clone(),
                importance: item.importance.clamp(1, 5),
                source: item.source.clone(),
                updated_at_ms: now_ms,
                last_accessed_at_ms: Some(now_ms),
                access_count: target.access_count + 1,
                embedding: Some(emb.to_vec()),
                ..target.clone()
            };
            return Ok((InsertOutcome::Merged(merged), Vec::new()));
        }
        hints.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let hint_texts: Vec<String> = hints
            .iter()
            .take(CONFLICT_HINT_TOP)
            .map(|(_, m)| {
                let key = m.tags.first().cloned().unwrap_or_default();
                if key.is_empty() {
                    m.content.chars().take(80).collect()
                } else {
                    format!("key={}, value={}", key, m.content.chars().take(80).collect::<String>())
                }
            })
            .collect();
        return insert_new(conn, item, embedding, now_ms)
            .map(|outcome| (outcome, hint_texts));
    }
    insert_new(conn, item, embedding, now_ms).map(|outcome| (outcome, Vec::new()))
}

fn insert_new(
    conn: &rusqlite::Connection,
    item: &NewItem,
    embedding: Option<&[f32]>,
    now_ms: i64,
) -> Result<InsertOutcome, String> {
    if let Err(e) = evict_if_needed(conn, now_ms) {
        return Ok(InsertOutcome::RejectedFull(e));
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    conn.execute(
        "INSERT INTO mem_items (id, kind, content, tags, importance, source, created_at, updated_at, embedding)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8)",
        rusqlite::params![
            id,
            item.kind,
            item.content,
            item.tags.join(","),
            item.importance.clamp(1, 5),
            item.source,
            ts_to_text(now_ms),
            embedding.map(embedding_to_blob),
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(InsertOutcome::Inserted(MemItem {
        id,
        kind: item.kind.clone(),
        content: item.content.clone(),
        tags: item.tags.clone(),
        importance: item.importance.clamp(1, 5),
        source: item.source.clone(),
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
        access_count: 0,
        last_accessed_at_ms: None,
        embedding: embedding.map(|e| e.to_vec()),
    }))
}

/// 按 tag 精确命中（remember_fact 同 key 覆盖语义：key 落 tags[0]）
pub fn find_by_key_tag(conn: &rusqlite::Connection, key: &str) -> Result<Option<MemItem>, String> {
    Ok(load_all(conn)?
        .into_iter()
        .find(|m| m.tags.first().map(|t| t.as_str()) == Some(key)))
}

/// 同 key 覆盖更新（不触发去重/淘汰；重算向量由调用方传入）
pub fn update_by_id(
    conn: &rusqlite::Connection,
    id: &str,
    content: &str,
    importance: i64,
    source: &str,
    kind: &str,
    embedding: Option<&[f32]>,
    now_ms: i64,
) -> Result<(), String> {
    conn.execute(
        "UPDATE mem_items SET content = ?1, importance = ?2, source = ?3, kind = ?4,
                updated_at = ?5, embedding = COALESCE(?6, embedding)
         WHERE id = ?7",
        rusqlite::params![
            content,
            importance.clamp(1, 5),
            source,
            kind,
            ts_to_text(now_ms),
            embedding.map(embedding_to_blob),
            id,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 删除（remember_fact 空 value = 遗忘语义）：返回是否真删到
pub fn delete_by_key_tag(conn: &rusqlite::Connection, key: &str) -> Result<bool, String> {
    let Some(m) = find_by_key_tag(conn, key)? else {
        return Ok(false);
    };
    conn.execute("DELETE FROM mem_items WHERE id = ?1", [&m.id])
        .map(|n| n > 0)
        .map_err(|e| e.to_string())
}

/// 指定 kind 条数（Reflection 触发判定）
pub fn count_by_kind(conn: &rusqlite::Connection, kind: &str) -> Result<i64, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM mem_items WHERE kind = ?1",
        rusqlite::params![kind],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

/// 最旧 n 条指定 kind（Reflection 取最旧 10 条 summary），返回 (id, content)
pub fn oldest_by_kind(
    conn: &rusqlite::Connection,
    kind: &str,
    n: i64,
) -> Result<Vec<(String, String)>, String> {
    conn.prepare(
        "SELECT id, content FROM mem_items WHERE kind = ?1 ORDER BY created_at ASC LIMIT ?2",
    )
    .and_then(|mut s| {
        s.query_map(rusqlite::params![kind, n], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map(|iter| iter.filter_map(|r| r.ok()).collect::<Vec<_>>())
    })
    .map_err(|e| e.to_string())
}

/// 按 id 批量删除（Reflection 合成后清原摘要）
pub fn delete_by_ids(conn: &rusqlite::Connection, ids: &[String]) -> Result<usize, String> {
    let mut n = 0;
    for id in ids {
        n += conn
            .execute("DELETE FROM mem_items WHERE id = ?1", rusqlite::params![id])
            .map_err(|e| e.to_string())?;
    }
    Ok(n)
}

/// 命中刷新访问计数（注入路径；同一连接内原子）
pub fn touch_accessed(conn: &rusqlite::Connection, ids: &[String], now_ms: i64) -> Result<(), String> {
    let ts = ts_to_text(now_ms);
    for id in ids {
        conn.execute(
            "UPDATE mem_items SET access_count = access_count + 1, last_accessed_at = ?1 WHERE id = ?2",
            rusqlite::params![ts, id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
