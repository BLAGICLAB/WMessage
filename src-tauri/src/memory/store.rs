//! 记忆 v2 存储（新表 mem_items，统一 500 条上限，语义去重 + 受保护条目免淘汰）：
//!
//! 容量：插入前检查，超限淘汰综合分最低者——
//! 淘汰分 = importance×2 + exp(-age_days/30) + ln(1+access_count)/5
//! （importance 权重最高；age 基准 = max(updated_at, last_accessed_at)）。
//! importance=5 且 source=user_stated 的条目不可淘汰；无可淘汰条目时拒写，
//! 原因进工具结果让模型自己清理。
//!
//! 语义去重（仅写入侧带向量时）：
//! - cos ≥ 0.92 → 同一条：合并更新已有条目（刷 content/updated_at/access+1/向量），不新增
//! - 0.75 ≤ cos < 0.92 → 不拦截，相似条目 top-3 拼进工具结果作冲突提示
//! - 降级模式（无向量）跳过去重；`remember_fact` 的同 key 覆盖语义由 tags[0] 精确匹配保证
//!
//! 向量检索：全表读 embedding 暴力余弦（500 条 × 2KB，微秒级），不引 sqlite-vec。
//!
//! 纯函数取 &Connection（内存库可单测）；锁/open_db/AppHandle 包装在 memory/mod.rs 门面层。

/// 统一容量上限（500 条单层上限，不按类型分层）
pub const MAX_MEM_ITEMS: i64 = 500;

/// 语义去重阈值：余弦 ≥ MERGE 视为同一条 → 合并更新，不新增
pub const DEDUP_MERGE_COSINE: f64 = 0.92;
/// 冲突提示区间：HINT ≤ 余弦 < MERGE → 不拦截，把相似条目拼进工具结果让模型裁决
pub const DEDUP_HINT_COSINE: f64 = 0.75;

/// 冲突提示最多带几条相似记忆
const CONFLICT_HINT_TOP: usize = 3;

/// 存储参数（ 参数化）：Default =  前常量语义，
/// insert_item 旧签名委托默认值——存量调用与测试零改动。
#[derive(Clone, Copy, Debug)]
pub struct StoreParams {
    pub capacity: i64,
    pub dedup_merge: f64,
    pub dedup_hint: f64,
}

impl Default for StoreParams {
    fn default() -> Self {
        Self {
            capacity: MAX_MEM_ITEMS,
            dedup_merge: DEDUP_MERGE_COSINE,
            dedup_hint: DEDUP_HINT_COSINE,
        }
    }
}

impl StoreParams {
    pub fn of(t: &crate::memory::MemoryTuning) -> Self {
        Self {
            capacity: t.capacity,
            dedup_merge: t.dedup_merge_cosine,
            dedup_hint: t.dedup_hint_cosine,
        }
    }
}

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

/// 建表（幂等；open_db 后每次调用都跑一次 IF NOT EXISTS，与 db.rs ensure_* 同模式）
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
    // 维度按 embed 契约收紧（bge 512 维）：非 512 维 blob（换模型/导入脏数据）
    // 原样解出会得到异形向量——与正规向量互算 cosine 恒 None（良性），
    // 但两条同维脏向量会互相命中、污染去重与召回。判 None 走降级模式
    // （关键词检索仍可用）。NaN/Inf 分量同理拒绝（会毒化排序比较）。
    if b.len() != crate::memory::embed::EMBED_DIM * 4 {
        return None;
    }
    let v: Vec<f32> = b
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    if v.iter().any(|f| !f.is_finite()) {
        return None;
    }
    Some(v)
}

/// source 契约归一（读取侧）：历史数据存在 'user' 脏值（三值契约
/// user_stated/model_inferred/system 定型前写入），不归一则 importance=5 的
/// 口述条目进不了 is_protected、会被容量淘汰误删。只改读取结果，不动盘上
/// 历史数据；编辑保存时自然落契约值。
fn normalize_source(s: String) -> String {
    if s == "user" {
        "user_stated".into()
    } else {
        s
    }
}

/// 行 → MemItem（load_all / find_by_id 共用的列映射）
fn item_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<MemItem> {
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
        source: normalize_source(r.get(5)?),
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
        .query_map([], item_from_row)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// 按 id 主键点查（整理事务内逐指令定位用，避免每条指令都全表扫描；无则 None）
pub fn find_by_id(conn: &rusqlite::Connection, id: &str) -> Result<Option<MemItem>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, kind, content, tags, importance, source, created_at, updated_at,
                    access_count, last_accessed_at, embedding FROM mem_items WHERE id = ?1",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query_map([id], item_from_row)
        .map_err(|e| e.to_string())?;
    match rows.next() {
        Some(row) => row.map(Some).map_err(|e| e.to_string()),
        None => Ok(None),
    }
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
    let base = item
        .last_accessed_at_ms
        .unwrap_or(item.updated_at_ms)
        .max(item.created_at_ms);
    let age_days = ((now_ms - base) as f64 / 86_400_000.0).max(0.0);
    item.importance as f64 * 2.0
        + (-age_days / 30.0).exp()
        + (1.0 + item.access_count.max(0) as f64).ln() / 5.0
}

/// 容量淘汰：达上限时删淘汰分最低的非保护条目；无可淘汰 → Err（拒写，信息给模型）
fn evict_if_needed(conn: &rusqlite::Connection, now_ms: i64, capacity: i64) -> Result<(), String> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM mem_items", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if count < capacity {
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
    /// 语义去重合并（余弦 ≥0.92）：刷新已有条目的 content/updated_at/access，未新增。
    /// orig_key = 被合并原条目的 tags[0]（item.tags 已被新值覆盖，提示文案要靠它指认原条目）
    Merged {
        item: MemItem,
        orig_key: Option<String>,
    },
    /// 容量满且无可淘汰条目 → 拒写
    RejectedFull(String),
    /// 带 `evo:` key 的 lesson 命中了**异 key** 的
    /// merge 目标（cosine ≥0.92）——merge-on-write 会把 lesson 内容+tags 覆盖到
    /// 既有记忆行（实证发生过行被劫持），此场景拒写不落库，由 apply 层记
    /// `evolution.apply_conflict` audit。target_key = 被拒绝合并的原行 key。
    RefusedForeignMerge { target_key: String },
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
    insert_item_with(conn, item, embedding, now_ms, &StoreParams::default())
}

/// 带参数变体：容量与去重阈值由调用方传（读 memoryTuning）
pub fn insert_item_with(
    conn: &rusqlite::Connection,
    item: &NewItem,
    embedding: Option<&[f32]>,
    now_ms: i64,
    p: &StoreParams,
) -> Result<(InsertOutcome, Vec<String>), String> {
    // 标签落盘按逗号拼接、读取按逗号切分：带逗号的标签会在读取时被劈成两条，
    // tags[0] 的 key 语义（同 key 覆盖 / evo 幂等查重）随之漂移，这里拒写并由
    // 调用方把报错带回去（去掉逗号即可重试）
    if item.tags.iter().any(|t| t.contains(',')) {
        return Err("记忆标签不能包含逗号「,」（存储层按逗号分隔标签），请去掉逗号后重试".into());
    }
    ensure_table(conn)?;
    let all = load_all(conn)?;
    // 语义去重（仅有向量时；降级模式无余弦可算，跳过）
    if let Some(emb) = embedding {
        let mut best: Option<(f64, &MemItem)> = None;
        let mut hints: Vec<(f64, &MemItem)> = Vec::new();
        for m in &all {
            if let Some(c) = cosine(Some(emb), m.embedding.as_deref()) {
                if c >= p.dedup_merge {
                    if best.is_none_or(|(s, _)| c > s) {
                        best = Some((c, m));
                    }
                } else if c >= p.dedup_hint {
                    hints.push((c, m));
                }
            }
        }
        if let Some((_, target)) = best {
            // lesson（evo: key）不许 merge 进异 key 既有行——
            // merge-on-write 是整行覆盖（content+tags 都换成 lesson 的），会把
            // 目标行劫持成 lesson、原记忆丢失。同 key = 本链路自己的重放，放行。
            let incoming_key = item.tags.first().cloned();
            let target_key = target.tags.first().cloned().unwrap_or_default();
            if incoming_key
                .as_deref()
                .map(|k| k.starts_with("evo:"))
                .unwrap_or(false)
                && target_key != incoming_key.unwrap_or_default()
            {
                return Ok((
                    InsertOutcome::RefusedForeignMerge { target_key },
                    Vec::new(),
                ));
            }
            let orig_key = target.tags.first().cloned();
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
            return Ok((
                InsertOutcome::Merged {
                    item: merged,
                    orig_key,
                },
                Vec::new(),
            ));
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
                    format!(
                        "key={}, value={}",
                        key,
                        m.content.chars().take(80).collect::<String>()
                    )
                }
            })
            .collect();
        return insert_new(conn, item, embedding, now_ms, p).map(|outcome| (outcome, hint_texts));
    }
    insert_new(conn, item, embedding, now_ms, p).map(|outcome| (outcome, Vec::new()))
}

fn insert_new(
    conn: &rusqlite::Connection,
    item: &NewItem,
    embedding: Option<&[f32]>,
    now_ms: i64,
    p: &StoreParams,
) -> Result<InsertOutcome, String> {
    if let Err(e) = evict_if_needed(conn, now_ms, p.capacity) {
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

/// 同 key 覆盖更新（不触发去重/淘汰；重算向量由调用方传入）。
/// 向量语义：content 变了 → 以新向量为准（None = 降级模式写，旧向量随内容一并作废弃 NULL，
/// 防语义检索按旧内容命中）；content 没变（仅动 importance/kind 等）→ 无新向量时保留旧向量。
pub fn update_by_id(
    conn: &rusqlite::Connection,
    id: &str,
    content: &str,
    importance: i64,
    source: &str,
    kind: &str,
    embedding: Option<&[f32]>,
    now_ms: i64,
) -> Result<usize, String> {
    conn.execute(
        // CASE 里的 content 读到的是更新前的旧值，?1 是新 content——同一参数
        // 身兼两职是刻意的：内容真变了才换向量。动 SQL 时别拆 ?1，拆了就比错对象。
        "UPDATE mem_items SET content = ?1, importance = ?2, source = ?3, kind = ?4,
                updated_at = ?5,
                embedding = CASE WHEN content <> ?1 THEN ?6 ELSE COALESCE(?6, embedding) END
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
    .map_err(|e| e.to_string())
    // 影响行数必须上浮：0 行 = id 未命中（查改之间被并发删除），调用方忽略
    // 会把改写假成功——panel 报不存在、consolidate 跳过该 op 防幻影合并
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
pub fn touch_accessed(
    conn: &rusqlite::Connection,
    ids: &[String],
    now_ms: i64,
) -> Result<(), String> {
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
