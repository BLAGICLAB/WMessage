//! 记忆 v2 混合打分与注入取数（2026-09-09，设计 docs/BOT-MEMORY-V2-DESIGN.md 第 3 节）。
//!
//! score = 0.55·cosine(query,item) + 0.20·关键词 bigram 重合度 + 0.15·(importance/5)
//!         + 0.10·exp(-age_days/30)
//! 无向量（降级模式 / 条目缺 embedding）时语义项记 0 并把剩余权重归一（÷0.45）；
//! 此时关键词零重合直接 0 分（纯关键词模式与旧系统口径一致，保住「零命中→近期摘要兜底」）。
//! 关键词提取复刻 db.rs extract_keywords 的 CJK bigram 逻辑（旧函数保留给旧表）。

use super::store::{cosine, MemItem};

/// 检索注入条数（top-5，与旧系统口径一致）
pub const MEMORY_TOP_N: usize = 5;

/// 近期摘要条数（最近 3 条 summary/reflection，兼作零命中兜底）
pub const MEMORY_RECENT_N: usize = 3;

/// 经验教训条数（lesson 类型 top-3，2026-09-09 lesson 特性）
pub const MEMORY_LESSON_N: usize = 3;

/// 关键词提取（无依赖）：英文/数字连续段转小写成一个词；
/// 连续 CJK 字符段取字符 bigram；单字 CJK 段保留单字。去重保序。
/// （与 db.rs::extract_keywords 同一算法；旧函数保留给旧表 bot_facts 的回归基准）
pub(crate) fn extract_keywords(text: &str) -> Vec<String> {
    fn is_cjk(c: char) -> bool {
        matches!(c,
            '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{3040}'..='\u{30FF}'
            | '\u{AC00}'..='\u{D7AF}'
        )
    }
    fn flush_cjk(cjk: &mut Vec<char>, out: &mut Vec<String>) {
        if cjk.len() == 1 {
            out.push(cjk[0].to_string());
        } else {
            for w in cjk.windows(2) {
                out.push(w.iter().collect());
            }
        }
        cjk.clear();
    }
    let mut out: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut cjk: Vec<char> = Vec::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            flush_cjk(&mut cjk, &mut out);
            word.push(c.to_ascii_lowercase());
        } else {
            if !word.is_empty() {
                out.push(std::mem::take(&mut word));
            }
            if is_cjk(c) {
                cjk.push(c);
            } else {
                flush_cjk(&mut cjk, &mut out);
            }
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    flush_cjk(&mut cjk, &mut out);
    let mut seen = std::collections::HashSet::new();
    out.retain(|k| seen.insert(k.clone()));
    out
}

/// 单条混合打分（纯函数）。query_kws 为查询关键词；query_emb 为查询向量（可无）。
/// age 基准 = max(updated_at, last_accessed_at)——被反复想起的记忆衰减更慢。
pub fn hybrid_score(
    query_kws: &[String],
    query_emb: Option<&[f32]>,
    item: &MemItem,
    now_ms: i64,
) -> f64 {
    let semantic = cosine(query_emb, item.embedding.as_deref()).unwrap_or(0.0);
    let kw = if query_kws.is_empty() {
        0.0
    } else {
        let haystack = format!("{} {}", item.tags.join(" "), item.content);
        let item_kws = extract_keywords(&haystack);
        let hits = query_kws.iter().filter(|k| item_kws.contains(*k)).count();
        hits as f64 / query_kws.len() as f64
    };
    // 降级模式（无查询向量）：关键词零重合直接 0 分——纯关键词模式与旧系统口径一致，
    // 否则重要度/新近度常正项会让全表都进 top-5，「零命中 → 近期摘要兜底」永不触发
    if query_emb.is_none() && kw == 0.0 {
        return 0.0;
    }
    let base = item
        .last_accessed_at_ms
        .unwrap_or(item.updated_at_ms)
        .max(item.updated_at_ms);
    let age_days = ((now_ms - base) as f64 / 86_400_000.0).max(0.0);
    let imp = (item.importance.clamp(1, 5) as f64) / 5.0;
    let rec = (-age_days / 30.0).exp();
    if query_emb.is_some() {
        0.55 * semantic + 0.20 * kw + 0.15 * imp + 0.10 * rec
    } else {
        // 降级模式：语义项为 0，剩余权重归一（0.20+0.15+0.10=0.45）
        (0.20 * kw + 0.15 * imp + 0.10 * rec) / 0.45
    }
}

/// 注入四段原料（与旧 MemoryInjection 同构 + lesson 段，供 format 拼「## 记忆」块）
pub struct MemInjection {
    /// importance≥4 且 kind=profile/preference（无条件带上）
    pub pinned: Vec<MemItem>,
    /// 混合打分 top-5（lesson 不进本段——lesson 有专属「经验教训」段，避免一段内容两处出现）
    pub hits: Vec<MemItem>,
    /// 最近 3 条 summary/reflection（兼作零命中兜底）
    pub recent: Vec<MemItem>,
    /// 混合打分 top-3 的 lesson（「经验教训」段；同类场景失败/纠正沉淀）
    pub lessons: Vec<MemItem>,
}

/// 注入取数（纯函数，内存数据打分；访问计数刷新由调用方拿 hit_ids 落库）。
/// 返回 (四段原料, 命中条目 id 列表（hits+lessons，访问强化口径）)。
pub fn injection_snapshot(
    items: &[MemItem],
    query: &str,
    query_emb: Option<&[f32]>,
    now_ms: i64,
) -> (MemInjection, Vec<String>) {
    let pinned: Vec<MemItem> = items
        .iter()
        .filter(|m| m.importance >= 4 && (m.kind == "profile" || m.kind == "preference"))
        .cloned()
        .collect();
    let query_kws = extract_keywords(query);
    let mut scored: Vec<(f64, MemItem)> = if query_kws.is_empty() && query_emb.is_none() {
        Vec::new()
    } else {
        items
            .iter()
            .filter(|m| !pinned.iter().any(|p| p.id == m.id))
            .map(|m| (hybrid_score(&query_kws, query_emb, m, now_ms), m.clone()))
            .filter(|(s, _)| *s > 0.0)
            .collect()
    };
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let lessons: Vec<MemItem> = scored
        .iter()
        .filter(|(_, m)| m.kind == "lesson")
        .take(MEMORY_LESSON_N)
        .map(|(_, m)| m.clone())
        .collect();
    let hits: Vec<MemItem> = scored
        .into_iter()
        .filter(|(_, m)| m.kind != "lesson")
        .take(MEMORY_TOP_N)
        .map(|(_, m)| m)
        .collect();
    let hit_ids: Vec<String> = hits
        .iter()
        .chain(lessons.iter())
        .map(|m| m.id.clone())
        .collect();
    let mut recent: Vec<MemItem> = items
        .iter()
        .filter(|m| {
            (m.kind == "summary" || m.kind == "reflection")
                && !hit_ids.contains(&m.id)
        })
        .cloned()
        .collect();
    recent.sort_by(|a, b| b.updated_at_ms.cmp(&a.updated_at_ms));
    recent.truncate(MEMORY_RECENT_N);
    (
        MemInjection {
            pinned,
            hits,
            recent,
            lessons,
        },
        hit_ids,
    )
}

/// 检索 top-n（recall_facts 工具用；纯读不刷新访问计数）
pub fn hybrid_search(
    items: &[MemItem],
    query: &str,
    query_emb: Option<&[f32]>,
    now_ms: i64,
    top_n: usize,
) -> Vec<MemItem> {
    let query_kws = extract_keywords(query);
    if query_kws.is_empty() && query_emb.is_none() {
        return Vec::new();
    }
    let mut scored: Vec<(f64, MemItem)> = items
        .iter()
        .map(|m| (hybrid_score(&query_kws, query_emb, m, now_ms), m.clone()))
        .filter(|(s, _)| *s > 0.0)
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(top_n).map(|(_, m)| m).collect()
}
