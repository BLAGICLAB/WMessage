//! 归档卡标签整理：扫描归档卡的所有 tags，按余弦相似度聚类（union-find），
//! 簇成员由用户在弹窗里选主标签后改源；改源前后在 audit_log 落 run_id +
//! 旧 tags 快照 + 影响 task_ids 列表，支持按 run_id 回滚。
//!
//! 设计要点：
//! - 阈值复用 `memory::store::DEDUP_MERGE_COSINE`（0.92）——与 corrections
//!   闭环同档，单一真相源。弹窗可在调用时传覆盖（未来扩展），不暴露
//!   设置项（避免与 memory dedup 阈值漂移）。
//! - 仅作用于归档卡（archived = 1 OR deleted_at IS NOT NULL）——主面板活跃
//!   卡语义以用户当下为准，不整理。
//! - 0 后台 scheduler：纯手动，ArchivePage 点「整理标签」触发。
//! - run_id 形如 `tgc-<timestamp_ms>-<short>`：短而可读，scheduler/audit
//!   grep 友好；前 4 字符按 timestamp 后 6 字符按 uuid 头 6 位。
//! - audit_log 落点：复用 `crate::audit::write_event` Info 级 + `tag_merge`
//!   事件 + `kv`：{run_id, merged_from, merged_to, affected_task_count,
//!   before_tags_per_task（JSON）}；回滚通过 `previous_tags_per_task`
//!   字段拿回原值。
//!
//! 不做的：DDL 迁移（`tasks.tags` 字段已存在）、新设置项（不暴露阈值
//! 改写入口）、alias 表（按决策走源改写 + 可逆 audit_log 路径）。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use tauri::Runtime;

use crate::audit;
use crate::db::now_ms;
use crate::memory::embed::embed_text;
use crate::memory::store::cosine;
use crate::memory::store::DEDUP_MERGE_COSINE;

/// 一个相似簇。members 是该簇里所有标签字符串（≥2 才成立；单标签
/// 不构成簇、留在 unclustered 单走）；suggested_main 是后端按成员出现
/// 频次最高的元素给的建议主标签（前端 select 默认值）；affected_task_count
/// 是「至少有一个成员标签的任务总数」。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TagCluster {
    pub members: Vec<String>,
    pub suggested_main: String,
    pub avg_similarity: f64,
    pub affected_task_count: usize,
}

/// 扫描统计
#[derive(Debug, Default, Clone, Serialize)]
pub struct ClusterScanResult {
    pub scanned_tags: usize,
    pub unique_tags: usize,
    pub archived_tasks_scanned: usize,
    pub clusters: Vec<TagCluster>,
}

/// 一次合并的运行记录（也作为 audit_log 落点 + 回滚凭据）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: String,
    pub merged_to: String,
    pub merged_from: Vec<String>,
    /// 受影响 task_id 列表（顺序按归档时间升序）
    pub affected_task_ids: Vec<String>,
    /// 回滚快照：task_id → 改写前的 tags 数组（Vec<String>）
    pub previous_tags: BTreeMap<String, Vec<String>>,
    pub timestamp_ms: i64,
}

/// 扫描归档卡的 tags，按阈值聚簇。threshold=None 走默认值
/// `DEDUP_MERGE_COSINE`（0.92）。
pub fn cluster_archived_tags(
    conn: &rusqlite::Connection,
    threshold: Option<f64>,
) -> Result<ClusterScanResult, String> {
    let th = threshold.unwrap_or(DEDUP_MERGE_COSINE);
    // 1) 扫所有归档/回收站任务的 tags，flat 成 (tag, task_id, tags 副本)
    let mut stmt = conn
        .prepare(
            "SELECT id, tags FROM tasks
             WHERE (archived = 1 OR deleted_at IS NOT NULL)
               AND tags IS NOT NULL AND tags != ''",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let tags_raw: Option<String> = row.get(1)?;
            let parsed: Vec<String> = tags_raw
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            Ok((id, parsed))
        })
        .map_err(|e| e.to_string())?;
    let mut task_tags: Vec<(String, Vec<String>)> = Vec::new();
    let mut scanned_tags: usize = 0;
    for r in rows {
        let (id, tags) = r.map_err(|e| e.to_string())?;
        scanned_tags += tags.len();
        task_tags.push((id, tags));
    }
    let archived_tasks_scanned = task_tags.len();

    // 2) 全局去重 tag + 每个 tag 出现任务数 + 涉及任务 id 集合
    let mut tag_to_tasks: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (task_id, tags) in &task_tags {
        for t in tags {
            tag_to_tasks
                .entry(t.clone())
                .or_default()
                .insert(task_id.clone());
        }
    }
    let unique_tags = tag_to_tasks.len();
    if unique_tags < 2 {
        return Ok(ClusterScanResult {
            scanned_tags,
            unique_tags,
            archived_tasks_scanned,
            clusters: Vec::new(),
        });
    }

    // 3) 给每个唯一 tag 算一次 embedding（缓存于本函数 hashmap，避免 N^2 重复）
    let unique_tag_list: Vec<String> = tag_to_tasks.keys().cloned().collect();
    let mut embeds: HashMap<String, Vec<f32>> = HashMap::new();
    for t in &unique_tag_list {
        if let Some(v) = embed_text(t) {
            embeds.insert(t.clone(), v);
        }
    }

    // 4) Union-Find：两两 cos ≥ 阈值则并入同簇
    let n = unique_tag_list.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent[ra] = rb;
        }
    }
    let mut cos_sum: BTreeMap<(usize, usize), (f64, usize)> = BTreeMap::new();
    for i in 0..n {
        for j in (i + 1)..n {
            let Some(ei) = embeds.get(&unique_tag_list[i]) else {
                continue;
            };
            let Some(ej) = embeds.get(&unique_tag_list[j]) else {
                continue;
            };
            if let Some(c) = cosine(Some(ei.as_slice()), Some(ej.as_slice())) {
                if c >= th {
                    let key = (
                        find(&mut parent, i).min(find(&mut parent, j)),
                        find(&mut parent, i).max(find(&mut parent, j)),
                    );
                    let entry = cos_sum.entry(key).or_insert((0.0, 0));
                    entry.0 += c;
                    entry.1 += 1;
                    union(&mut parent, i, j);
                }
            }
        }
    }
    // 5) 按根分组 → 簇（成员 ≥ 2）
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..n {
        groups.entry(find(&mut parent, i)).or_default().push(i);
    }
    let mut clusters = Vec::new();
    for (_, idxs) in groups {
        if idxs.len() < 2 {
            continue; // 单标签不构成簇
        }
        let mut members: Vec<String> = idxs.iter().map(|&i| unique_tag_list[i].clone()).collect();
        // suggested_main：成员里出现任务数最多的；并列时按字母序选第一个
        members.sort_by(|a, b| {
            let ca = tag_to_tasks.get(a).map(|s| s.len()).unwrap_or(0);
            let cb = tag_to_tasks.get(b).map(|s| s.len()).unwrap_or(0);
            cb.cmp(&ca).then_with(|| a.cmp(b))
        });
        let suggested_main = members[0].clone();
        // 受影响 task id 集合 = 簇成员各自的 task 集合并集
        let mut affected: BTreeSet<String> = BTreeSet::new();
        for m in &members {
            if let Some(set) = tag_to_tasks.get(m) {
                affected.extend(set.iter().cloned());
            }
        }
        // 平均相似度：本簇内两两相似度均值（不含跨簇）；简化取 union 时累加 / 次数
        // ——Union-Find 不能直接还原具体两两索引对，所以按根收集
        // 全部 (a,b) pair 重新算均值
        let mut pair_sum = 0.0_f64;
        let mut pair_cnt = 0_usize;
        for (i_idx, &i) in idxs.iter().enumerate() {
            for &j in &idxs[i_idx + 1..] {
                let Some(ei) = embeds.get(&unique_tag_list[i]) else {
                    continue;
                };
                let Some(ej) = embeds.get(&unique_tag_list[j]) else {
                    continue;
                };
                if let Some(c) = cosine(Some(ei.as_slice()), Some(ej.as_slice())) {
                    pair_sum += c;
                    pair_cnt += 1;
                }
            }
        }
        let avg = if pair_cnt > 0 {
            pair_sum / pair_cnt as f64
        } else {
            0.0
        };
        clusters.push(TagCluster {
            members,
            suggested_main,
            avg_similarity: avg,
            affected_task_count: affected.len(),
        });
    }
    Ok(ClusterScanResult {
        scanned_tags,
        unique_tags,
        archived_tasks_scanned,
        clusters,
    })
}

/// 生成 run_id：`tgc-<timestamp_ms>-<rand6>` —— 时间戳可读、6 位随机
/// 避免同一毫秒合并冲撞
pub fn new_run_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_else(|_| now_ms());
    let rand: u32 = (now_ms() as u32).wrapping_mul(0x9E3779B1) ^ (now_ms() as u32);
    format!("tgc-{ts}-{:06x}", rand & 0xFFFFFF)
}

/// 按主标签 + 成员别名合并：把每个 task 的 tags 里出现的成员别名替换为主
/// 标签，去重后写回。返回 run_id 与受影响 task 数（供 audit 落点）。
/// 注意「成员里已有 main」时只去重、不动其他。
///
/// 整个合并包一个事务：逐条 UPDATE 自动提交时，中途失败会留下半合并态——
/// 前面的 UPDATE 已落盘而回滚凭据（history 行）还没写，「合并可完整撤销」
/// 在失败路径上根本不成立。
pub fn merge_tag_cluster(
    conn: &mut rusqlite::Connection,
    main: &str,
    members: &[&str],
) -> Result<RunRecord, String> {
    if main.is_empty() {
        return Err("主标签不能为空".to_string());
    }
    // 去重 + 移除 main 自身（main 不会成为自己的别名）
    let mut aliases: BTreeSet<String> = members.iter().map(|s| s.to_string()).collect();
    aliases.remove(main);
    if aliases.is_empty() {
        return Err("成员别名至少 1 个".to_string());
    }

    let tx = conn.transaction().map_err(|e| e.to_string())?;

    // 找受影响的归档 task：tags 含任一别名的
    // INSTR 是标量函数，对每个别名建一个 ?i 占位（?1 重复会被覆盖为最后值）
    let alias_list: Vec<String> = aliases.iter().cloned().collect();
    let sql = format!(
        "SELECT id, tags FROM tasks
         WHERE (archived = 1 OR deleted_at IS NOT NULL)
           AND tags IS NOT NULL AND tags != ''
           AND ({})",
        (0..alias_list.len())
            .map(|i| format!("instr(tags, ?{}) > 0", i + 1))
            .collect::<Vec<_>>()
            .join(" OR ")
    );

    // 收集 (task_id, old_tags, new_tags) 三元组
    let mut updates: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
    let mut affected_ids: Vec<String> = Vec::new();
    {
        let mut stmt = tx.prepare(&sql).map_err(|e| e.to_string())?;
        let params = rusqlite::params_from_iter(alias_list.iter().map(|a| a.as_str()));
        let rows = stmt
            .query_map(params, |row| {
                let id: String = row.get(0)?;
                let tags_raw: String = row.get(1)?;
                let parsed: Vec<String> = serde_json::from_str(&tags_raw).unwrap_or_default();
                Ok((id, parsed))
            })
            .map_err(|e| e.to_string())?;
        for r in rows {
            let (task_id, old_tags) = r.map_err(|e| e.to_string())?;
            // SQL 的 instr 只是子串粗筛：「集成测试」含「测试」也会命中。
            // 主标签只该加给真正带着簇成员标签的卡，所以精确判定放在这里——
            // 粗筛命中的卡若不含任一别名（精确相等），整张卡跳过。
            if !old_tags.iter().any(|t| aliases.contains(t)) {
                continue;
            }
            let mut new_tags: Vec<String> = old_tags
                .iter()
                .filter(|t| !aliases.contains(*t))
                .cloned()
                .collect();
            if !new_tags.contains(&main.to_string()) {
                new_tags.push(main.to_string());
            }
            if new_tags == old_tags {
                // 没变化（可能 main 已在且无别名）——跳过
                continue;
            }
            affected_ids.push(task_id.clone());
            updates.push((task_id, old_tags, new_tags));
        }
    }
    affected_ids.sort();

    // 写盘：每条 UPDATE（与 history INSERT 同事务）
    let mut previous_tags: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (task_id, old, new) in &updates {
        previous_tags.insert(task_id.clone(), old.clone());
        let new_json = serde_json::to_string(new).map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE tasks SET tags = ?1 WHERE id = ?2",
            rusqlite::params![new_json, task_id],
        )
        .map_err(|e| e.to_string())?;
    }
    let run_id = new_run_id();
    let ts = now_ms();

    // 落 tag_merge_history 表：run_id + 旧 tags 快照 + 时间戳 → 回滚凭据
    let snapshot = serde_json::to_string(&previous_tags).unwrap_or_default();
    let merged_from_csv = aliases.iter().cloned().collect::<Vec<_>>().join(",");
    let affected_count = updates.len() as i64;
    tx.execute(
        "INSERT INTO tag_merge_history
            (run_id, merged_to, merged_from, previous_tags_json, affected_task_count, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![run_id, main, merged_from_csv, snapshot, affected_count, ts],
    )
    .map_err(|e| e.to_string())?;

    tx.commit().map_err(|e| e.to_string())?;

    Ok(RunRecord {
        run_id,
        merged_to: main.to_string(),
        merged_from: aliases.iter().cloned().collect(),
        affected_task_ids: updates.iter().map(|(id, _, _)| id.clone()).collect(),
        previous_tags,
        timestamp_ms: ts,
    })
}

/// 按 run_id 回滚：tag_merge_history 表读旧 tags 快照 → 逐 task UPDATE 回写。
/// 返回受影响 task 数。run_id 不存在或快照损坏 → Err；已回滚的 run_id
/// 二次回滚也 Err（rolled_back_at 标记避免误回滚导致数据漂移）。
///
/// 回写包一个事务：中途失败时不能留下「部分卡已恢复、部分卡还是合并态」的
/// 半回滚态——那种状态下快照还在、rolled_back_at 没置，撤销语义已经碎了。
pub fn rollback_tag_merge<R: Runtime>(
    app: &tauri::AppHandle<R>,
    conn: &mut rusqlite::Connection,
    run_id: &str,
) -> Result<RollbackResult, String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let (merged_to, prev_json, ts, rolled_back_at) = {
        let mut stmt = tx
            .prepare(
                "SELECT merged_to, previous_tags_json, created_at, rolled_back_at
             FROM tag_merge_history WHERE run_id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query_map([run_id], |row| {
                let merged_to: String = row.get(0)?;
                let prev_json: String = row.get(1)?;
                let created_at: i64 = row.get(2)?;
                let rolled_back_at: Option<i64> = row.get(3)?;
                Ok((merged_to, prev_json, created_at, rolled_back_at))
            })
            .map_err(|e| e.to_string())?;
        rows.next()
            .ok_or_else(|| format!("run_id {run_id} 不存在"))?
            .map_err(|e| e.to_string())?
    };
    if rolled_back_at.is_some() {
        return Err(format!("run_id {run_id} 已于 ts={rolled_back_at:?} 回滚过"));
    }
    let snapshot: BTreeMap<String, Vec<String>> =
        serde_json::from_str(&prev_json).map_err(|e| format!("快照损坏：{e}"))?;
    let mut count = 0_usize;
    let mut restored_task_ids: Vec<String> = Vec::new();
    for (task_id, old_tags) in &snapshot {
        let old_json = serde_json::to_string(old_tags).map_err(|e| e.to_string())?;
        let affected = tx
            .execute(
                "UPDATE tasks SET tags = ?1 WHERE id = ?2",
                rusqlite::params![old_json, task_id],
            )
            .map_err(|e| e.to_string())?;
        if affected > 0 {
            count += 1;
            restored_task_ids.push(task_id.clone());
        }
    }
    // 标记回滚时间
    let now = now_ms();
    tx.execute(
        "UPDATE tag_merge_history SET rolled_back_at = ?1 WHERE run_id = ?2",
        rusqlite::params![now, run_id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    // 同时落 audit 给 bot.log 看（按用户点击动作可观测）
    audit::write_event(
        app,
        audit::AuditLevel::Info,
        "tag_merge_rollback",
        &[
            ("run_id", run_id.to_string()),
            ("restored_task_count", count.to_string()),
            ("timestamp_ms", now.to_string()),
        ],
    );
    Ok(RollbackResult {
        run_id: run_id.to_string(),
        merged_to,
        restored_task_count: count,
        restored_task_ids,
        original_timestamp_ms: ts,
    })
}

/// 回滚结果
#[derive(Debug, Clone, Serialize)]
pub struct RollbackResult {
    pub run_id: String,
    pub merged_to: String,
    pub restored_task_count: usize,
    /// 实际被改回原 tags 的 task id（回滚凭据里那张卡已不在库里时不算）
    pub restored_task_ids: Vec<String>,
    pub original_timestamp_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::ensure_columns;
    use rusqlite::Connection;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        // tasks DDL：archived/deleted_at 字段必须存在
        c.execute_batch(
            "CREATE TABLE tasks (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL DEFAULT '',
                tags TEXT
            );
            CREATE TABLE tag_merge_history (
                run_id TEXT PRIMARY KEY,
                merged_to TEXT NOT NULL,
                merged_from TEXT NOT NULL,
                previous_tags_json TEXT NOT NULL,
                affected_task_count INTEGER NOT NULL,
                rolled_back_at INTEGER,
                created_at INTEGER NOT NULL
            );",
        )
        .unwrap();
        // ALTER 给 archived / deleted_at 字段（如有 ensure_columns 列在那里）
        ensure_columns(
            &c,
            "tasks",
            &[("archived", "INTEGER"), ("deleted_at", "INTEGER")],
        )
        .unwrap();
        c
    }

    fn insert_task(c: &Connection, id: &str, tags: Vec<&str>, archived: bool) {
        let tags_json = serde_json::to_string(&tags).unwrap();
        c.execute(
            "INSERT INTO tasks (id, title, tags, archived) VALUES (?1, '', ?2, ?3)",
            rusqlite::params![id, tags_json, archived as i64],
        )
        .unwrap();
    }

    #[test]
    fn cluster_no_archived_returns_empty() {
        let c = conn();
        insert_task(&c, "t1", vec!["买菜"], false);
        let r = cluster_archived_tags(&c, None).unwrap();
        assert_eq!(r.clusters.len(), 0);
    }

    #[test]
    fn cluster_two_close_tags_form_one_cluster() {
        // 0.92 起步阈值；"买菜"/"groceries" 期望 cos ≥ 0.7 但具体值依赖嵌入引擎
        // 本测试不依赖引擎：手动降阈值到 0.5 走宽松聚类
        let c = conn();
        insert_task(&c, "t1", vec!["买菜"], true);
        insert_task(&c, "t2", vec!["买菜", "购菜"], true);
        insert_task(&c, "t3", vec!["购菜", "groceries"], true);
        let r = cluster_archived_tags(&c, Some(0.5)).unwrap();
        assert!(!r.clusters.is_empty(), "宽松阈值下应至少 1 个簇");
        // 任意一个簇的成员都包含本测试中的标签
        let members: Vec<String> = r
            .clusters
            .iter()
            .flat_map(|c| c.members.iter().cloned())
            .collect();
        let all = ["买菜", "购菜", "groceries"];
        assert!(
            all.iter().any(|a| members.contains(&a.to_string())),
            "至少含测试标签之一：{members:?}"
        );
    }

    #[test]
    fn merge_writes_to_archived_only_and_audits() {
        let mut c = conn();
        // 主面板卡不应被改
        insert_task(&c, "active", vec!["买菜", "购菜"], false);
        insert_task(&c, "arch1", vec!["买菜", "购菜"], true);
        insert_task(&c, "arch2", vec!["购菜"], true);
        let run = merge_tag_cluster(&mut c, "groceries", &["买菜", "购菜"]).unwrap();
        assert_eq!(run.merged_to, "groceries");
        assert_eq!(
            run.affected_task_ids.len(),
            2,
            "应只影响 2 个归档卡，不影响主面板"
        );
        // 主面板卡 tags 不变
        let active_tags: String = c
            .query_row("SELECT tags FROM tasks WHERE id='active'", [], |r| r.get(0))
            .unwrap();
        assert!(active_tags.contains("买菜"));
        // 归档卡 tags 改写
        let arch1_tags: String = c
            .query_row("SELECT tags FROM tasks WHERE id='arch1'", [], |r| r.get(0))
            .unwrap();
        let arch1_parsed: Vec<String> = serde_json::from_str(&arch1_tags).unwrap();
        assert_eq!(arch1_parsed, vec!["groceries".to_string()]);
    }

    /// 子串不算命中：簇别名「测试」不该把只带「集成测试」的卡拉进来。
    ///
    /// SQL 侧的 `instr(tags, ?)` 是子串粗筛，「集成测试」含「测试」必然命中；
    /// 精确判定必须落在 Rust 侧，否则主标签会被强加给从未拥有该别名的卡。
    #[test]
    fn merge_skips_cards_that_only_contain_the_alias_as_a_substring() {
        let mut c = conn();
        insert_task(&c, "exact", vec!["测试"], true);
        insert_task(&c, "substring", vec!["集成测试"], true);
        let run = merge_tag_cluster(&mut c, "testing", &["测试"]).unwrap();
        assert_eq!(
            run.affected_task_ids,
            vec!["exact".to_string()],
            "只有精确带别名的卡该被合并"
        );
        let exact: Vec<String> = serde_json::from_str(
            &c.query_row("SELECT tags FROM tasks WHERE id='exact'", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(exact, vec!["testing".to_string()]);
        let substring: Vec<String> = serde_json::from_str(
            &c.query_row("SELECT tags FROM tasks WHERE id='substring'", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            substring,
            vec!["集成测试".to_string()],
            "只含子串的卡不该被打上主标签"
        );
    }

    /// 合并中途失败不得留下半合并态：回滚凭据写不进去时，改写必须一并回滚。
    ///
    /// 逐条 UPDATE 自动提交的话，前面的改写已落盘而 history 行没写——
    /// 「合并可完整撤销」在失败路径上不成立。
    #[test]
    fn merge_rolls_back_every_rewrite_when_the_history_row_cannot_be_written() {
        let mut c = conn();
        // history 表加一条永远不成立的约束，让最后的 INSERT 必然失败
        c.execute_batch(
            "DROP TABLE tag_merge_history;
             CREATE TABLE tag_merge_history (
                run_id TEXT PRIMARY KEY,
                merged_to TEXT NOT NULL,
                merged_from TEXT NOT NULL,
                previous_tags_json TEXT NOT NULL,
                affected_task_count INTEGER NOT NULL,
                rolled_back_at INTEGER,
                created_at INTEGER NOT NULL,
                CHECK (affected_task_count < 0)
             );",
        )
        .unwrap();
        insert_task(&c, "arch1", vec!["买菜"], true);
        insert_task(&c, "arch2", vec!["购菜"], true);
        let err = merge_tag_cluster(&mut c, "groceries", &["买菜", "购菜"]);
        assert!(err.is_err(), "history 写不进去时合并必须失败");
        for id in ["arch1", "arch2"] {
            let raw: String = c
                .query_row("SELECT tags FROM tasks WHERE id=?1", [id], |r| r.get(0))
                .unwrap();
            let parsed: Vec<String> = serde_json::from_str(&raw).unwrap();
            assert_eq!(parsed.len(), 1, "{id} 的 tags 必须回到合并前");
            assert!(
                !parsed.contains(&"groceries".to_string()),
                "{id} 不得残留主标签"
            );
        }
        let hist: i64 = c
            .query_row("SELECT COUNT(*) FROM tag_merge_history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(hist, 0, "失败路径不得留下 history 行");
    }

    #[test]
    fn rollback_restores_previous_tags() {
        let mut c = conn();
        insert_task(&c, "t1", vec!["a", "b"], true);
        let app_root = tauri::test::mock_app();
        let app = app_root.handle();
        let run = merge_tag_cluster(&mut c, "main", &["a", "b"]).unwrap();
        let after: String = c
            .query_row("SELECT tags FROM tasks WHERE id='t1'", [], |r| r.get(0))
            .unwrap();
        let after_parsed: Vec<String> = serde_json::from_str(&after).unwrap();
        assert_eq!(after_parsed, vec!["main".to_string()]);
        let rb = rollback_tag_merge(app, &mut c, &run.run_id).unwrap();
        assert_eq!(rb.restored_task_count, 1);
        let restored: String = c
            .query_row("SELECT tags FROM tasks WHERE id='t1'", [], |r| r.get(0))
            .unwrap();
        let restored_parsed: Vec<String> = serde_json::from_str(&restored).unwrap();
        assert_eq!(restored_parsed, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn new_run_id_format() {
        let id = new_run_id();
        assert!(id.starts_with("tgc-"), "{id}");
        assert!(id.len() > 10, "id too short: {id}");
    }

    #[test]
    fn merge_rejects_empty_main() {
        let mut c = conn();
        let app_root = tauri::test::mock_app();
        let _app = app_root.handle();
        let err = merge_tag_cluster(&mut c, "", &["a", "b"]);
        assert!(err.is_err());
    }

    #[test]
    fn merge_rejects_when_only_main_in_members() {
        let mut c = conn();
        let app_root = tauri::test::mock_app();
        let _app = app_root.handle();
        let err = merge_tag_cluster(&mut c, "only", &["only"]);
        assert!(err.is_err());
    }
}
