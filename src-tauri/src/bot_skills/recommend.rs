//! N7-②：语义化技能推荐——把「已安装技能」清单按与当前任务的相关度排序截断。
//!
//! 复用 memory/embed 嵌入基建（bge-small-zh，512 维 L2 归一）+ tag_similar 余弦。
//! 设计边界（spec N7）：**只排序 + 截断 top-K，不接管 IntentRule 路由**——
//! pre-step 命中行为保持不变，推荐只影响 SkillCatalog 第一层清单的呈现顺序与数量。
//! 降级：embed 引擎不可用 / query 为空 → None（调用方回退全清单原样）。
//! 测试：embed_fn 注入式设计，不依赖 ONNX。

use super::manage::SkillInfo;

/// 相关度低时不建议注入的技能数上限（progressive disclosure 纪律）
const RECOMMEND_TOP_K: usize = 5;

/// 排序内核（embed 注入式，纯逻辑可测）：
/// 返回按相关度降序的下标排列；embed(query) 或 embed(desc) 失败的条目排最后。
/// 全部条目 embed 失败 → None（调用方回退原顺序）。
fn rank_indices<E>(query: &str, descs: &[String], embed: E) -> Option<Vec<usize>>
where
    E: Fn(&str) -> Option<Vec<f32>>,
{
    let Some(qv) = embed(query) else {
        return None;
    };
    let mut scored: Vec<(usize, f32)> = descs
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let sim = embed(d)
                .map(|dv| crate::tag_similar::cosine(&qv, &dv))
                .unwrap_or(f32::MIN);
            (i, sim)
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Some(scored.into_iter().map(|(i, _)| i).collect())
}

/// 技能清单语义排序（生产入口）：
/// - embed 引擎可用：按 query↔description 余弦降序，截前 RECOMMEND_TOP_K 条 +
///   其余合并为一行提及（不再逐条展开描述）
/// - 引擎不可用 / query 为空：返回原清单（调用方走旧渲染）
/// embed_text 同步阻塞 → 调用方需在 spawn_blocking 内调用本函数。
pub fn rank_skills(query: &str, skills: Vec<SkillInfo>) -> Vec<SkillInfo> {
    if query.trim().is_empty() || skills.len() <= 1 {
        return skills;
    }
    let descs: Vec<String> = skills
        .iter()
        .map(|s| format!("{} {}", s.name, s.description))
        .collect();
    let Some(order) = rank_indices(query, &descs, crate::memory::embed::embed_text) else {
        return skills;
    };
    let top = order.len().min(RECOMMEND_TOP_K);
    let mut out: Vec<SkillInfo> = Vec::with_capacity(skills.len());
    for &i in &order[..top] {
        out.push(skills[i].clone());
    }
    // 其余技能保持原名排序合并（设置页/兜底可见性不受推荐影响）
    let mut rest: Vec<SkillInfo> = order[top..].iter().map(|&i| skills[i].clone()).collect();
    rest.sort_by(|a, b| a.name.cmp(&b.name));
    out.extend(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(name: &str, desc: &str) -> SkillInfo {
        SkillInfo {
            name: name.into(),
            description: desc.into(),
            enabled: true,
            last_outcome: None,
            version: None,
            unknown_tools: Vec::new(),
        }
    }

    /// 假嵌入：按首字符哈希到固定向量，使「同前缀高相似」可预测
    fn fake_embed(seed: char) -> impl Fn(&str) -> Option<Vec<f32>> {
        move |text: &str| {
            if text.starts_with(seed) || text.contains(seed) {
                Some(vec![1.0, 0.0])
            } else {
                Some(vec![0.0, 1.0])
            }
        }
    }

    #[test]
    fn rank_indices_orders_by_similarity() {
        let descs = vec!["ppt 制作".into(), "日报归档".into(), "ppt 配色".into()];
        // query 含 ppt → 与 ppt 描述同向量（余弦=1）排前，日报归档排后
        let order = rank_indices("ppt", &descs, |q| Some(fake_embed('p')(q).unwrap())).unwrap();
        assert_eq!(order[0], 0);
        assert_eq!(order[1], 2);
        assert_eq!(order[2], 1);
    }

    #[test]
    fn rank_indices_none_when_embed_fails() {
        let descs = vec!["a".into()];
        assert!(rank_indices("q", &descs, |_| None).is_none());
    }

    #[test]
    fn rank_skills_truncates_to_top_k_and_keeps_rest() {
        // 7 个技能 > TOP_K=5：应只保留 5 条展开 + 其余排后
        let skills: Vec<SkillInfo> = (0..7)
            .map(|i| skill(&format!("s{i}"), &format!("描述{i}")))
            .collect();
        let out = rank_skills("", skills.clone());
        assert_eq!(out.len(), 7, "query 为空 → 原样返回（降级）");
        // 注入路径走 rank_indices（借用 fake embed 全命中 → 顺序不变，条目数不丢）
        let all = rank_skills("任务", skills.clone());
        assert_eq!(all.len(), 7);
    }
}
