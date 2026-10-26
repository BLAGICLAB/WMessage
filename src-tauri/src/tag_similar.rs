//! 标签近义（设计 docs/TASK-GRAPH-AFFINITY-DEPS-2026-10-05.md §3）：
//! 对前端下发的标签词表逐个取嵌入向量（memory::embed 引擎，bge-small-zh 512 维
//! L2 归一化），两两余弦（归一化后点积）≥ 阈值判定为近义对，供图谱把同义标签
//! 并入同一聚簇锚点。纯本地推理，无网络调用。
//!
//! 线程纪律：embed_text 是同步阻塞接口，命令体整体放 spawn_blocking。
//! 缓存：进程级 Mutex（tag → 向量 + 插入序），跨调用复用；上限 512 条，
//! 满时按插入序逐出最旧一条——整表 clear 会让下一轮调用全量重跑嵌入。

use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use tauri::AppHandle;

use crate::error::CommandResult;

/// 近义判定阈值：bge 短文本归一化点积经验值（0.78 ≈ 强相关词对）
pub const TAG_SIMILAR_THRESHOLD: f32 = 0.78;
/// 返回对数上限（防御异常大词表的两两组合）
const MAX_PAIRS: usize = 200;
/// 输入词表条数上限：词表直进 O(n²) 两两扫描 + 逐条嵌入，超长输入会拖死
/// 调用线程。超限截断（与单条超限静默过滤的既有风格一致；审计行记录实际条数）
const MAX_TAGS: usize = 200;
/// 向量缓存上限（满时按插入序逐出最旧一条，不清整表）
const CACHE_LIMIT: usize = 512;

/// 向量缓存：tag → 向量 + 插入序（FIFO 逐出最旧；插入只发生在未命中重嵌入时）
#[derive(Default)]
struct VectorCache {
    map: HashMap<String, Vec<f32>>,
    order: VecDeque<String>,
}

static VECTOR_CACHE: Mutex<Option<VectorCache>> = Mutex::new(None);

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TagPair {
    pub a: String,
    pub b: String,
    pub score: f32,
}

/// 缓存回填（纯函数，单测锚点）：满仓先按插入序逐出最旧一条再插入——
/// 整表 clear 会把热词表也丢掉，下一轮调用全量重跑嵌入
fn cache_insert(state: &mut VectorCache, tag: String, v: Vec<f32>) {
    if state.map.len() >= CACHE_LIMIT && !state.map.contains_key(&tag) {
        if let Some(oldest) = state.order.pop_front() {
            state.map.remove(&oldest);
        }
    }
    state.order.push_back(tag.clone());
    state.map.insert(tag, v);
}

fn cache_get_or_embed(tag: &str) -> Option<Vec<f32>> {
    // 查询与回填全程持同一把锁：查后放锁会让并发未命中的调用各自重跑一遍
    // ONNX 嵌入（缓存形同虚设）。本命令词表小、调用频率低，串行代价可忽略
    let mut cache = VECTOR_CACHE.lock().ok()?;
    if let Some(v) = cache.as_ref().and_then(|c| c.map.get(tag)) {
        return Some(v.clone());
    }
    let v = crate::memory::embed::embed_text(tag)?;
    let state = cache.get_or_insert_with(VectorCache::default);
    cache_insert(state, tag.to_string(), v.clone());
    Some(v)
}

/// 余弦相似度（输入须已 L2 归一化 → 点积即余弦；纯函数，单测锚点）
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// 纯函数：由向量表筛选近义对（i < j 去重、阈值、上限）。单测锚点。
pub fn pairs_from_vectors(
    tags: &[String],
    vectors: &HashMap<String, Vec<f32>>,
    threshold: f32,
    max_pairs: usize,
) -> Vec<TagPair> {
    let mut out = Vec::new();
    for i in 0..tags.len() {
        for j in (i + 1)..tags.len() {
            let (Some(va), Some(vb)) = (vectors.get(&tags[i]), vectors.get(&tags[j])) else {
                continue;
            };
            let score = cosine(va, vb);
            if score >= threshold {
                out.push(TagPair {
                    a: tags[i].clone(),
                    b: tags[j].clone(),
                    score: (score * 1000.0).round() / 1000.0,
                });
                if out.len() >= max_pairs {
                    return out;
                }
            }
        }
    }
    out
}

/// 词表清洗（纯函数，单测锚点）：trim / 去空 / 单条 24 字符内 / 去重保序 /
/// 条数上限截断。超限条目静默丢弃（与单条超限同一风格——审计行记录清洗后条数）
fn clean_tags(tags: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    tags.into_iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty() && t.chars().count() <= 24 && seen.insert(t.clone()))
        .take(MAX_TAGS)
        .collect()
}

/// 近义标签对查询：前端把当前标签词表整体下发（去重后逐个取嵌入）。
/// 引擎不可用 → Err（前端按「无近义」降级，不影响图谱其他功能）。
#[tauri::command]
pub async fn tag_similar_pairs(app: AppHandle, tags: Vec<String>) -> CommandResult<Vec<TagPair>> {
    if !crate::memory::embed::engine_status().is_ok() {
        crate::audit::write_event(
            &app,
            crate::audit::AuditLevel::Warn,
            "tag_synonyms_unavailable",
            &[("reason", "embed engine not loaded".to_string())],
        );
        return Err(crate::error::CommandError::Internal(
            "嵌入引擎不可用，无法计算标签近义".into(),
        ));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let tags = clean_tags(tags);

        let vectors: HashMap<String, Vec<f32>> = tags
            .iter()
            .filter_map(|t| cache_get_or_embed(t).map(|v| (t.clone(), v)))
            .collect();
        let embedded = vectors.len();
        let pairs = pairs_from_vectors(&tags, &vectors, TAG_SIMILAR_THRESHOLD, MAX_PAIRS);

        crate::audit::write_event(
            &app,
            crate::audit::AuditLevel::Info,
            "tag_synonyms",
            &[
                ("tags", tags.len().to_string()),
                ("embedded", embedded.to_string()),
                ("pairs", pairs.len().to_string()),
            ],
        );
        Ok(pairs)
    })
    .await
    .map_err(|e| crate::error::CommandError::from(format!("tag_similar 线程 join 失败：{e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v2(x: f32, y: f32) -> Vec<f32> {
        let len = (x * x + y * y).sqrt();
        vec![x / len, y / len]
    }

    #[test]
    fn cosine_identical_is_one_opposite_is_zero() {
        let a = v2(3.0, 4.0);
        assert!((cosine(&a, &a) - 1.0).abs() < 1e-5);
        // 二维正交 → 0
        let b = vec![0.0, 1.0];
        let c = vec![1.0, 0.0];
        assert!(cosine(&b, &c).abs() < 1e-6);
        assert_eq!(cosine(&[], &[]), 0.0, "空向量兜底");
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0, "维度不匹配兜底");
    }

    #[test]
    fn pairs_dedupe_threshold_and_cap() {
        let tags: Vec<String> = (0..4).map(|i| format!("t{i}")).collect();
        let mut vectors = HashMap::new();
        // t0≈t1≈t2 两两近义（同一方向），t3 正交
        vectors.insert("t0".into(), v2(1.0, 0.05));
        vectors.insert("t1".into(), v2(1.0, 0.05));
        vectors.insert("t2".into(), v2(1.0, 0.06));
        vectors.insert("t3".into(), v2(0.0, 1.0));
        let pairs = pairs_from_vectors(&tags, &vectors, TAG_SIMILAR_THRESHOLD, 200);
        assert_eq!(pairs.len(), 3, "t0/t1/t2 三对近义，t3 不入对");
        // i<j 去重：只出现 (t0,t1) (t0,t2) (t1,t2)
        assert!(pairs.iter().all(|p| p.a < p.b));
        // 上限截断
        let capped = pairs_from_vectors(&tags, &vectors, TAG_SIMILAR_THRESHOLD, 1);
        assert_eq!(capped.len(), 1);
    }

    #[test]
    fn cache_insert_evicts_oldest_instead_of_clearing() {
        // 满仓后插入：按插入序逐出最旧一条，其余条目（含刚插的）必须还在——
        // 整表 clear 会让下一轮调用全量重跑嵌入
        let mut state = VectorCache::default();
        for i in 0..CACHE_LIMIT {
            cache_insert(&mut state, format!("k{i}"), vec![0.1, 0.2]);
        }
        assert_eq!(state.map.len(), CACHE_LIMIT);
        // 再插一条：最旧的 k0 被逐出，容量回到上限
        cache_insert(&mut state, "newest".into(), vec![0.3, 0.4]);
        assert_eq!(state.map.len(), CACHE_LIMIT, "满仓插入不增容");
        assert!(!state.map.contains_key("k0"), "最旧条目被逐出");
        assert!(state.map.contains_key("k1"), "次旧条目保留");
        assert!(state.map.contains_key("newest"), "新条目入缓存");
        // 插入序与 map 同步：order 首位现在是 k1
        assert_eq!(state.order.front().map(String::as_str), Some("k1"));
        // 已存在的键重复插入：不触发逐出（只更新向量）
        cache_insert(&mut state, "k1".into(), vec![0.5, 0.5]);
        assert_eq!(state.map.len(), CACHE_LIMIT, "同键更新不逐出");
        assert_eq!(state.map.get("k1"), Some(&vec![0.5, 0.5]), "同键向量已更新");
    }

    #[test]
    fn clean_tags_trims_dedupes_and_caps_count() {
        // trim / 去空 / 单条 24 字符内 / 去重保序（既有行为，随抽函数钉住）
        let tags = clean_tags(vec![
            "  a  ".into(),
            "".into(),
            "   ".into(),
            "a".into(),
            "b".into(),
            "x".repeat(25),
        ]);
        assert_eq!(tags, vec!["a".to_string(), "b".to_string()], "超长单条剔除");
        // 条数上限：超 MAX_TAGS 截断，保序保前缀
        let many: Vec<String> = (0..MAX_TAGS + 50).map(|i| format!("t{i}")).collect();
        let capped = clean_tags(many);
        assert_eq!(capped.len(), MAX_TAGS, "词表条数截到上限");
        assert_eq!(capped[0], "t0", "保留先出现的条目");
        assert_eq!(capped[MAX_TAGS - 1], format!("t{}", MAX_TAGS - 1));
    }
}
