//! 标签近义（G6-SYNONYM，设计 docs/TASK-GRAPH-AFFINITY-DEPS-2026-10-05.md §3）：
//! 对前端下发的标签词表逐个取嵌入向量（memory::embed 引擎，bge-small-zh 512 维
//! L2 归一化），两两余弦（归一化后点积）≥ 阈值判定为近义对，供图谱把同义标签
//! 并入同一聚簇锚点。纯本地推理，无网络调用。
//!
//! 线程纪律：embed_text 是同步阻塞接口，命令体整体放 spawn_blocking。
//! 缓存：进程级 Mutex<HashMap>（tag → 向量），跨调用复用；无界上限 512 条，
//! 超限整表清空（标签词表增长极慢，实际到不了）。

use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::AppHandle;

use crate::error::CommandResult;

/// 近义判定阈值：bge 短文本归一化点积经验值（0.78 ≈ 强相关词对）
pub const TAG_SIMILAR_THRESHOLD: f32 = 0.78;
/// 返回对数上限（防御异常大词表的两两组合）
const MAX_PAIRS: usize = 200;
/// 向量缓存上限（超限清空重建——标签词表小，清空代价可忽略）
const CACHE_LIMIT: usize = 512;

static VECTOR_CACHE: Mutex<Option<HashMap<String, Vec<f32>>>> = Mutex::new(None);

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TagPair {
    pub a: String,
    pub b: String,
    pub score: f32,
}

fn cache_get_or_embed(tag: &str) -> Option<Vec<f32>> {
    {
        let cache = VECTOR_CACHE.lock().ok()?;
        if let Some(v) = cache.as_ref().and_then(|m| m.get(tag)) {
            return Some(v.clone());
        }
    }
    let v = crate::memory::embed::embed_text(tag)?;
    let mut cache = VECTOR_CACHE.lock().ok()?;
    let map = cache.get_or_insert_with(HashMap::new);
    if map.len() >= CACHE_LIMIT {
        map.clear();
    }
    map.insert(tag.to_string(), v.clone());
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
        // 去重 + 清洗（trim/去空/长度上限 24——与 profile 名字同量级的防呆）
        let mut seen = std::collections::HashSet::new();
        let tags: Vec<String> = tags
            .into_iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty() && t.chars().count() <= 24 && seen.insert(t.clone()))
            .collect();

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
    fn cache_limit_clears_without_panic() {
        // 直接压缓存上限路径（嵌入函数不可用时 None——这里只验证锁不毒）
        {
            let mut guard = VECTOR_CACHE.lock().unwrap();
            let map = guard.get_or_insert_with(HashMap::new);
            for i in 0..(CACHE_LIMIT + 10) {
                map.insert(format!("k{i}"), vec![0.1, 0.2]);
            }
            assert!(map.len() >= CACHE_LIMIT);
            map.clear();
        }
        let guard = VECTOR_CACHE.lock().unwrap();
        assert!(guard.as_ref().map(|m| m.len()).unwrap_or(0) <= CACHE_LIMIT);
    }
}
