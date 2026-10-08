//! 记忆检索评估（U18-MEMEVAL）：给记忆系统装「尺子」——检索质量的量化基线。
//!
//! 两个手动评估器（#[ignore]，`cargo test --test memory_eval -- --ignored --nocapture`）：
//! - `eval_recall_report`：黄金查询集（fixtures/memory_golden.json，拟真种子 + 30 查询）
//!   → 真实 ONNX 嵌入播种内存库 → 逐条 query 走生产同构 `hybrid_search_with`
//!   （`RankParams::of(&MemoryTuning::default())`）→ 打印 recall@1 / recall@5 分项
//!   （画像偏好 / 事实 / 教训 / 混合改口）与总榜；断言总体 recall@5 > 0 仅防评估器
//!   自身坏掉（嵌入失效/检索全零），不设质量门槛（基线只记录不设限）。
//! - `eval_extract_sample`：从真实记忆库随机抽 30 条 model_inferred 抽取产物
//!   （profile/preference/fact）导出 fixtures/memory_extract_sample.jsonl
//!   （human_label 留空）供人工标注噪音率。只读打开真实库，导出文件含隐私
//!   内容，已进 .gitignore 不入库。
//!
//! 确定性：嵌入推理与打分均为纯计算，同一机器两次运行结果一致（spec 记录两跑对比）。
//! 环境要求：仓库根有 bge-small-zh-v1.5/（与 embed 冒烟测试同前提）。

use serde::Deserialize;
use wmessage_lib::memory::embed;
use wmessage_lib::memory::rank;
use wmessage_lib::memory::store::{self, InsertOutcome, NewItem, StoreParams};
use wmessage_lib::memory::MemoryTuning;

/// 仓库根目录（embed.rs 开发期模型解析同款：CARGO_MANIFEST_DIR 的上级）
fn repo_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.to_path_buf())
        .expect("CARGO_MANIFEST_DIR 必有上级目录")
}

// ───────────────────────── 黄金集结构 ─────────────────────────

#[derive(Deserialize)]
struct Golden {
    seeds: Vec<GoldenSeed>,
    queries: Vec<GoldenQuery>,
}

#[derive(Deserialize)]
struct GoldenSeed {
    id: String,
    kind: String,
    content: String,
    importance: i64,
    tags: Vec<String>,
    source: String,
}

#[derive(Deserialize)]
struct GoldenQuery {
    id: String,
    category: String,
    query: String,
    expect_ids: Vec<String>,
}

/// 分项报表的类目顺序（固定输出顺序，两次运行可逐行对比）
const CATEGORY_ORDER: [&str; 4] = ["profile", "fact", "lesson", "correction"];
const CATEGORY_LABELS: [(&str, &str); 4] = [
    ("profile", "画像偏好"),
    ("fact", "稳定事实"),
    ("lesson", "经验教训"),
    ("correction", "混合改口"),
];

fn eval_recall_report_impl() {
    let path = repo_root().join("fixtures/memory_golden.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读黄金集失败（{}）：{e}", path.display()));
    let golden: Golden =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("黄金集解析失败：{e}"));

    // 真实嵌入可用性前置：模型缺失时此评估无意义，响亮失败而不是静默打 0 分
    embed::engine_status()
        .unwrap_or_else(|e| panic!("嵌入引擎不可用（仓库根应有 bge-small-zh-v1.5/）：{e}"));

    // 内存库播种：dedup_merge=1.0 关闭语义合并——黄金集要一条一行、id 稳定
    //（检索评估不评写入路径，种子间偶发高相似不该吞行）
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    store::ensure_table(&conn).unwrap();
    let sp = StoreParams {
        dedup_merge: 1.0,
        ..StoreParams::default()
    };
    let now = 1_790_000_000_000i64; // 固定时间基准：全部条目同刻写入，新近度项均匀
                                    // 行 id（入库时生成的 uuid）→ 种子 id（黄金集稳定 id）：recall 判定用种子 id
    let mut row_to_seed: std::collections::HashMap<String, String> = Default::default();
    for s in &golden.seeds {
        let emb = embed::embed_text(&s.content).unwrap_or_else(|| panic!("种子 {} 嵌入失败", s.id));
        let item = NewItem {
            kind: s.kind.clone(),
            content: s.content.clone(),
            tags: s.tags.clone(),
            importance: s.importance,
            source: s.source.clone(),
        };
        match store::insert_item_with(&conn, &item, Some(&emb), now, &sp).unwrap() {
            (InsertOutcome::Inserted(m), _) => {
                row_to_seed.insert(m.id, s.id.clone());
            }
            other => panic!("种子 {} 应独立插入，实得 {other:?}", s.id),
        }
    }
    let items = store::load_all(&conn).unwrap();
    assert_eq!(
        items.len(),
        golden.seeds.len(),
        "播种条数与种子数不一致（存在合并/淘汰）"
    );

    // recall@5 的「5」与实际 top_n 必须一致：显式锁死，防 MemoryTuning 默认值
    // 漂移后指标名悄悄变成 recall@top_n（debug_assert 兜底）
    let params = rank::RankParams {
        top_n: 5,
        ..rank::RankParams::of(&MemoryTuning::default())
    };
    debug_assert_eq!(params.top_n, 5);
    let mut per_cat: std::collections::BTreeMap<String, (usize, usize, usize)> = Default::default();
    let mut misses: Vec<String> = Vec::new();
    for q in &golden.queries {
        let emb = embed::embed_text(&q.query).unwrap_or_else(|| panic!("查询 {} 嵌入失败", q.id));
        let hits = rank::hybrid_search_with(&items, &q.query, Some(emb.as_slice()), now, &params);
        let hit_seed_ids: Vec<&str> = hits
            .iter()
            .filter_map(|m| row_to_seed.get(&m.id).map(|s| s.as_str()))
            .collect();
        let hit1 = hit_seed_ids
            .first()
            .map(|s| q.expect_ids.iter().any(|e| e == s))
            .unwrap_or(false);
        let hit5 = hit_seed_ids
            .iter()
            .any(|s| q.expect_ids.iter().any(|e| e == s));
        let e = per_cat.entry(q.category.clone()).or_insert((0, 0, 0));
        e.0 += hit1 as usize;
        e.1 += hit5 as usize;
        e.2 += 1;
        if !hit5 {
            misses.push(format!(
                "  {}「{}」expect={:?} top5种子={hit_seed_ids:?}",
                q.id, q.query, q.expect_ids
            ));
        }
    }

    println!(
        "\n══════ U18 记忆检索基线报告（seeds={} / queries={}）══════",
        golden.seeds.len(),
        golden.queries.len()
    );
    let (mut all1, mut all5, mut alln) = (0usize, 0usize, 0usize);
    for cat in CATEGORY_ORDER {
        let (h1, h5, n) = per_cat.get(cat).copied().unwrap_or((0, 0, 0));
        let label = CATEGORY_LABELS
            .iter()
            .find(|(c, _)| *c == cat)
            .map(|(_, l)| *l)
            .unwrap_or(cat);
        println!(
            "[{label}] recall@1 = {:.2}（{h1}/{n}）  recall@5 = {:.2}（{h5}/{n}）",
            h1 as f64 / n.max(1) as f64,
            h5 as f64 / n.max(1) as f64,
        );
        all1 += h1;
        all5 += h5;
        alln += n;
    }
    let r1 = all1 as f64 / alln.max(1) as f64;
    let r5 = all5 as f64 / alln.max(1) as f64;
    println!("[总榜]   recall@1 = {r1:.4}  recall@5 = {r5:.4}（{all1}/{alln} / {all5}/{alln}）");
    if misses.is_empty() {
        println!("未命中：无");
    } else {
        println!("未命中明细（top5 内无期望条目）：");
        for m in &misses {
            println!("{m}");
        }
    }

    // 评估器自检（非质量门槛）：总体 recall@5 > 0 说明嵌入→播种→检索链路活着
    assert!(r5 > 0.0, "总体 recall@5 = {r5}，评估链路自身疑似坏掉");
}

/// 黄金查询集检索基线（手动跑）：
/// `cargo test --test memory_eval eval_recall_report -- --ignored --nocapture`
#[test]
#[ignore = "真实模型检索评估（慢，手动跑）"]
fn eval_recall_report() {
    eval_recall_report_impl();
}

// ───────────────────────── 抽取产物采样（人工标注用） ─────────────────────────

/// 采样条目（JSONL 一行一条）：human_label 留空供标注方填
///（good=值得记 / bad=噪音 / fix=值得记但表述差）
#[derive(serde::Serialize)]
struct SampleLine {
    content: String,
    kind: String,
    importance: i64,
    created_at: String,
    human_label: String,
}

/// 评估库路径：WMESSAGE_EVAL_DB 显式指定优先，否则 macOS 默认用户库
///（手动评估工具读真实库；其他平台请用 env 指定）
fn eval_db_path() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("WMESSAGE_EVAL_DB") {
        let p = p.trim().to_string();
        if !p.is_empty() {
            return Some(std::path::PathBuf::from(p));
        }
    }
    let home = std::env::var("HOME").ok()?;
    let p = std::path::PathBuf::from(home)
        .join("Library/Application Support/com.renshi.wmessage/wmessage.db");
    p.is_file().then_some(p)
}

/// 导出抽取产物人工标注采样（手动跑，只读真实库）：
/// `cargo test --test memory_eval eval_extract_sample -- --ignored --nocapture`
/// 输出 fixtures/memory_extract_sample.jsonl（含真实记忆内容，已 gitignore 不入库）。
#[test]
#[ignore = "读真实用户库采样（手动跑；导出文件含隐私，不入库）"]
fn eval_extract_sample() {
    const SAMPLE_N: i64 = 30;
    let path = eval_db_path()
        .unwrap_or_else(|| panic!("找不到评估库：设 WMESSAGE_EVAL_DB 或确认 macOS 默认用户库存在"));
    // 只读打开：评估绝不写用户真实库
    let conn =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap_or_else(|e| panic!("只读打开 {} 失败：{e}", path.display()));
    let mut stmt = conn
        .prepare(
            "SELECT content, kind, importance, created_at FROM mem_items
             WHERE source = 'model_inferred' AND kind IN ('profile','preference','fact')
             ORDER BY RANDOM() LIMIT ?1",
        )
        .unwrap_or_else(|e| panic!("mem_items 查询失败（库缺表？）：{e}"));
    let rows: Vec<(String, String, i64, String)> = stmt
        .query_map(rusqlite::params![SAMPLE_N], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    if rows.is_empty() {
        println!(
            "库 {} 无 model_inferred 抽取产物（autoExtract 可能从未开启），无可采样",
            path.display()
        );
        return;
    }
    let lines: Vec<String> = rows
        .into_iter()
        .map(|(content, kind, importance, created_at)| {
            serde_json::to_string(&SampleLine {
                content,
                kind,
                importance,
                created_at,
                human_label: String::new(),
            })
            .unwrap()
        })
        .collect();
    let out_path = repo_root().join("fixtures/memory_extract_sample.jsonl");
    // 原子写：先写 .tmp 再 rename，中断不留含 PII 片段的半截采样文件
    let tmp_path = out_path.with_extension("jsonl.tmp");
    std::fs::write(&tmp_path, format!("{}\n", lines.join("\n")))
        .unwrap_or_else(|e| panic!("写采样临时文件失败：{e}"));
    std::fs::rename(&tmp_path, &out_path).unwrap_or_else(|e| panic!("rename 采样文件失败：{e}"));
    // 采样含真实用户内容（PII）：Unix 上收紧到 0600（best-effort，Windows 走 ACL 另议）
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(0o600));
    }
    println!(
        "已导出 {} 条抽取产物 → {}（human_label 留空，标注口径：good/bad/fix）",
        lines.len(),
        out_path.display()
    );
}
