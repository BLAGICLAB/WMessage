//! 记忆抽取的纯函数与集成测试。

use super::super::MemoryControl;
use super::*;

/// 收下时的存储故障要报 DbError（库故障），不是 blanket 的 Internal——
/// 两者在前端给的是不同提示（「含操作步骤反馈日志」vs「含复现步骤」）。
#[test]
fn pending_store_error_is_db_error() {
    let err = pending_store_error("唯一键冲突");
    assert!(
        matches!(err, crate::error::CommandError::DbError(ref d) if d.contains("唯一键冲突")),
        "存储故障应报 DbError，got {err:?}"
    );
    assert!(
        err.to_string().contains("本批未提交任何变更"),
        "要说明这批没提交（用户可重试）：{err}"
    );
}

fn mem_db() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    ensure_pending_table(&conn).unwrap();
    conn
}

// ── 档位解析 ──

#[test]
fn auto_extract_mode_parses_and_defaults_off() {
    assert_eq!(auto_extract_mode(None), AutoExtract::Off);
    let off = Some(MemoryControl::default());
    assert_eq!(auto_extract_mode(off.as_ref()), AutoExtract::Off);
    for (s, want) in [
        ("auto", AutoExtract::Auto),
        ("confirm", AutoExtract::Confirm),
        ("off", AutoExtract::Off),
        ("垃圾值", AutoExtract::Off),
        ("", AutoExtract::Off),
    ] {
        let ctrl = Some(MemoryControl {
            auto_extract: s.into(),
            ..MemoryControl::default()
        });
        assert_eq!(auto_extract_mode(ctrl.as_ref()), want, "mode({s})");
    }
}

#[test]
fn serde_default_fills_auto_extract_off_for_old_blocks() {
    // 历史版本落盘的 memoryControl 块无 autoExtract 字段 → 反序列化补 off
    let ctrl: MemoryControl =
        serde_json::from_str(r#"{"injectionEnabled":true,"autoWriteEnabled":false}"#).unwrap();
    assert_eq!(ctrl.auto_extract, "off");
    assert_eq!(auto_extract_mode(Some(&ctrl)), AutoExtract::Off);
}

// ── 限频 ──

#[test]
fn throttle_due_first_and_interval() {
    const MIN: i64 = EXTRACT_MIN_INTERVAL_SECS;
    assert!(throttle_due(None, 1_000_000, MIN), "从未抽取 → 到期");
    assert!(!throttle_due(
        Some(1_000_000),
        1_000_000 + MIN * 1000 - 1,
        MIN
    ));
    assert!(throttle_due(Some(1_000_000), 1_000_000 + MIN * 1000, MIN));
}

#[test]
fn throttle_check_set_occupies_window() {
    let now = 10_000_000;
    assert!(throttle_check_set("sess-throttle-test", now), "首次应到期");
    assert!(
        !throttle_check_set("sess-throttle-test", now + 1),
        "窗口内不再触发"
    );
    assert!(
        throttle_check_set("sess-throttle-test", now + EXTRACT_MIN_INTERVAL_SECS * 1000),
        "跨过间隔重新到期"
    );
}

// ── 解析矩阵 ──

#[test]
fn parse_extract_accepts_array_and_clamps() {
    let text = r#"[{"content":"用户偏好简洁回复","kind":"preference","importance":3},
                       {"content":"用户是后端工程师","kind":"profile","importance":9}]"#;
    let facts = parse_extract(text);
    assert_eq!(facts.len(), 2);
    assert_eq!(facts[0].kind, "preference");
    assert_eq!(facts[1].importance, 5, "importance 钳到 5");
}

#[test]
fn parse_extract_tolerates_fences_and_prose() {
    let fenced = "好的，以下是抽取结果：\n```json\n[{\"content\":\"喜欢中文回复\",\"kind\":\"preference\",\"importance\":2}]\n```\n以上。";
    assert_eq!(parse_extract(fenced).len(), 1);
    assert!(parse_extract("我觉得这段对话没什么可记的。").is_empty());
    assert!(parse_extract("[]").is_empty());
    assert!(parse_extract("").is_empty());
}

#[test]
fn parse_extract_skips_contract_violations() {
    // 空 content / 未知 kind / 缺 kind → 跳过；超长 content 截断
    let long = "长".repeat(MAX_EXTRACT_CONTENT_CHARS + 50);
    let text = format!(
        r#"[{{"content":"","kind":"fact","importance":2}},
               {{"content":"未知类型","kind":"magic","importance":2}},
               {{"content":"缺 kind","importance":2}},
               {{"content":"{long}","kind":"fact","importance":2}}]"#
    );
    let facts = parse_extract(&text);
    assert_eq!(facts.len(), 1);
    assert_eq!(
        facts[0].content.chars().count(),
        MAX_EXTRACT_CONTENT_CHARS,
        "超长内容截断到上限"
    );
}

// ── 待确认队列 ──

#[test]
fn pending_insert_list_delete_roundtrip() {
    let conn = mem_db();
    pending_insert(&conn, "待确认一", "preference", 3, "s1", 1_000).unwrap();
    pending_insert(&conn, "待确认二", "fact", 2, "s1", 2_000).unwrap();
    let list = pending_list(&conn).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].content, "待确认二", "新→旧排序");
    assert_eq!(list[0].created_at, 2_000);
    assert_eq!(pending_delete(&conn, &[list[0].id]).unwrap(), 1);
    assert_eq!(pending_list(&conn).unwrap().len(), 1);
}

#[test]
fn pending_cap_evicts_oldest() {
    let conn = mem_db();
    for i in 0..PENDING_CAP + 5 {
        pending_insert(&conn, &format!("条目{i}"), "fact", 2, "s", 1_000 + i).unwrap();
    }
    let list = pending_list(&conn).unwrap();
    assert_eq!(list.len() as i64, PENDING_CAP, "队列钉在上限");
    // 最旧的 5 条（条目0..条目4）被挤掉
    assert!(list.iter().all(|v| !v.content.contains("条目0")));
    assert!(list.iter().any(|v| v.content.contains("条目5")));
}

#[test]
fn pending_get_returns_row_or_none() {
    let conn = mem_db();
    pending_insert(&conn, "某条", "fact", 2, "s", 1_000).unwrap();
    let id = pending_list(&conn).unwrap()[0].id;
    assert_eq!(pending_get(&conn, id).unwrap().unwrap().content, "某条");
    assert!(pending_get(&conn, 999).unwrap().is_none());
}

// ── 写入时冲突裁决──

/// 512 维单热向量（与 memory/tests.rs 同款假向量）
fn onehot(i: usize) -> Vec<f32> {
    let mut v = vec![0f32; 512];
    v[i % 512] = 1.0;
    v
}

/// 与 onehot(0) 余弦 = w 的向量
fn tilted(w: f32) -> Vec<f32> {
    let mut v = vec![0f32; 512];
    v[0] = w;
    v[1] = (1.0 - w * w).max(0.0).sqrt();
    v
}

fn fact_db() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    store::ensure_table(&conn).unwrap();
    conn
}

fn seeded_item(
    conn: &rusqlite::Connection,
    kind: &str,
    content: &str,
    tags: &[&str],
    emb: Option<&[f32]>,
    ms: i64,
) -> String {
    let item = store::NewItem {
        kind: kind.to_string(),
        content: content.to_string(),
        tags: tags.iter().map(|s| s.to_string()).collect(),
        importance: 3,
        source: "user_stated".to_string(),
    };
    match store::insert_item(conn, &item, emb, ms).unwrap() {
        (store::InsertOutcome::Inserted(m), _) => m.id,
        other => panic!("应直接插入：{other:?}"),
    }
}

fn mkfact(content: &str, kind: &str, importance: i64) -> ExtractedFact {
    ExtractedFact {
        content: content.to_string(),
        kind: kind.to_string(),
        importance,
    }
}

fn cand(id: &str) -> Option<store::MemItem> {
    Some(store::MemItem {
        id: id.to_string(),
        kind: "fact".into(),
        content: "已有记忆内容".into(),
        tags: vec![],
        importance: 3,
        source: "user_stated".into(),
        created_at_ms: 1_000,
        updated_at_ms: 1_000,
        access_count: 0,
        last_accessed_at_ms: None,
        embedding: None,
    })
}

#[test]
fn adjudication_parse_matrix() {
    let cands = vec![cand("id-a"), cand("id-b"), None];
    let text = r#"裁决如下：```json
            [{"index":0,"action":"new"},
             {"index":1,"action":"update","existing_id":"id-b"},
             {"index":2,"action":"skip"}]
            ```"#;
    assert_eq!(
        parse_adjudication(text, &cands),
        vec![
            Adjudication::New,
            Adjudication::Update("id-b".into()),
            Adjudication::New, // 无候选条目恒 New（本就不该被裁决）
        ]
    );
    // skip / 幻觉 id / 无候选条目被误裁决
    let text2 = r#"[{"index":0,"action":"skip"},
                        {"index":1,"action":"update","existing_id":"id-幻觉"},
                        {"index":2,"action":"update","existing_id":"id-a"}]"#;
    assert_eq!(
        parse_adjudication(text2, &cands),
        vec![
            Adjudication::Skip,
            Adjudication::New, // 幻觉 id ≠ 候选 id → 回退 new
            Adjudication::New, // 无候选恒 New（幻觉 id 指到有候选的别条也不放行）
        ],
        "输出长度与 candidates 对齐，越界 index 忽略"
    );
}

#[test]
fn adjudication_duplicate_update_claim_skips_and_keeps_first_write() {
    // 复现：同批两条高相似事实命中同一候选、都判 update——第二条降级 New
    // 时走 insert 语义去重合并（与首条同向量 cos=1.0 ≥ 0.92），拿第二事实
    // 覆盖首条刚改写的内容并冲掉 tags/source/importance；降级 Skip 后
    // 首条事实与溯源完好。
    let conn = fact_db();
    let now = 10_000;
    let orig_id = seeded_item(
        &conn,
        "fact",
        "用户住在上海",
        &["居住城市"],
        Some(&onehot(0)),
        1_000,
    );
    let existing = store::load_all(&conn).unwrap().remove(0);
    let cands = vec![Some(existing.clone()), Some(existing)];
    let facts = vec![
        mkfact("用户已经搬到北京海淀", "fact", 2),
        mkfact("用户已经搬到北京海淀区", "fact", 2),
    ];
    let reply = format!(
        r#"[{{"index":0,"action":"update","existing_id":"{orig_id}"}},
               {{"index":1,"action":"update","existing_id":"{orig_id}"}}]"#
    );
    let adjs = parse_adjudication(&reply, &cands);
    assert_eq!(adjs[0], Adjudication::Update(orig_id.clone()));
    assert_eq!(adjs[1], Adjudication::Skip, "同候选第二条 update 降级 Skip");

    // 两条事实与原条目同向量（cos=1.0）：降级 New 时第二条必命中 merge 覆盖
    let embs = vec![Some(onehot(0)), Some(onehot(0))];
    let counts = apply_adjudications(
        &conn,
        &facts,
        &embs,
        &adjs,
        now,
        &store::StoreParams::default(),
    )
    .unwrap();
    assert_eq!(counts.updated, 1, "首条照常改写原条目");
    assert_eq!(counts.skipped, 1, "第二条按 Skip 丢弃");
    assert_eq!(counts.inserted, 0, "第二条不落库");
    let rows = store::load_all(&conn).unwrap();
    assert_eq!(rows.len(), 1, "库里只有原条目一行");
    assert_eq!(rows[0].id, orig_id);
    assert_eq!(
        rows[0].content, "用户已经搬到北京海淀",
        "首条 content 不被第二条覆盖"
    );
    assert_eq!(rows[0].tags, vec!["居住城市"], "tags 不被第二条冲掉");
    assert_eq!(rows[0].source, "user_stated", "source 不被第二条冲掉");
    assert_eq!(rows[0].importance, 3, "importance 不被第二条冲掉");
}

#[test]
fn apply_adjudications_tx_commits_whole_batch() {
    // 多裁决整批事务化：New + Update + Skip 一批，commit 后全部生效
    let conn = fact_db();
    let orig_id = seeded_item(
        &conn,
        "fact",
        "用户住在上海",
        &["居住城市"],
        Some(&onehot(0)),
        1_000,
    );
    let facts = vec![
        mkfact("新增一条独立事实", "fact", 2),
        mkfact("用户已经搬到北京", "fact", 2),
        mkfact("跳过条不落库", "fact", 2),
    ];
    let embs = vec![Some(onehot(3)), Some(onehot(0)), None];
    let adjs = vec![
        Adjudication::New,
        Adjudication::Update(orig_id.clone()),
        Adjudication::Skip,
    ];
    let mut conn = conn;
    let counts = apply_adjudications_tx(
        &mut conn,
        &facts,
        &embs,
        &adjs,
        10_000,
        &store::StoreParams::default(),
    )
    .unwrap();
    assert_eq!(counts.inserted, 1);
    assert_eq!(counts.updated, 1);
    assert_eq!(counts.skipped, 1);
    let rows = store::load_all(&conn).unwrap();
    assert_eq!(rows.len(), 2, "New 与 Update 落库，Skip 不落");
    assert!(
        rows.iter()
            .any(|r| r.id == orig_id && r.content == "用户已经搬到北京"),
        "Update 改写原条目"
    );
    assert!(rows.iter().any(|r| r.content == "新增一条独立事实"));
}

#[test]
fn apply_adjudications_tx_rolls_back_whole_batch_on_mid_failure() {
    // 复现：auto 分支逐条 autocommit 落库时，中途硬失败留半态（首条已落、
    // 失败条丢失）。注入手段：预建带 FK 的 mem_items（kind 引用 fk_kind）+
    // defer_foreign_keys，第二条的 kind 违约被推迟到 COMMIT 才爆——事务化后
    // 整批回滚且 Err 上抛；逐条 autocommit 的旧实现首条已残留（本断言必红）。
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
             CREATE TABLE fk_kind (kind TEXT PRIMARY KEY);
             INSERT INTO fk_kind VALUES ('fact');
             CREATE TABLE mem_items(
               id TEXT PRIMARY KEY,
               kind TEXT NOT NULL REFERENCES fk_kind(kind),
               content TEXT NOT NULL,
               tags TEXT NOT NULL DEFAULT '',
               importance INTEGER NOT NULL DEFAULT 3,
               source TEXT NOT NULL DEFAULT 'model_inferred',
               created_at TEXT NOT NULL,
               updated_at TEXT NOT NULL,
               access_count INTEGER NOT NULL DEFAULT 0,
               last_accessed_at TEXT,
               embedding BLOB
             );
             PRAGMA defer_foreign_keys = ON;",
    )
    .unwrap();
    let facts = vec![
        mkfact("首条正常入库", "fact", 2),
        mkfact("次条 kind 违约", "ghost-kind", 2),
    ];
    let embs = vec![None, None];
    let adjs = vec![Adjudication::New, Adjudication::New];
    let mut conn = conn;
    let r = apply_adjudications_tx(
        &mut conn,
        &facts,
        &embs,
        &adjs,
        10_000,
        &store::StoreParams::default(),
    );
    assert!(r.is_err(), "COMMIT 期 FK 违约必须整批上抛，实际 {r:?}");
    assert!(
        store::load_all(&conn).unwrap().is_empty(),
        "中途失败必须整体回滚，首条不得残留（半态）"
    );
}

#[test]
fn adjudication_parse_bad_output_falls_back_all_new() {
    let cands = vec![cand("id-a"), cand("id-b")];
    for bad in [
        "",
        "我觉得这几条都没问题。",
        "[{broken json",
        "[]", // 合法空数组 = 没给任何裁决 → 全回退 new
        r#"[{"index":0,"action":"炸"}]"#,
    ] {
        assert_eq!(
            parse_adjudication(bad, &cands),
            vec![Adjudication::New, Adjudication::New],
            "坏输出应整体回退全 new：{bad}"
        );
    }
}

#[test]
fn plan_adjudication_top1_threshold_and_kind_scope() {
    let conn = fact_db();
    let id_hit = seeded_item(
        &conn,
        "fact",
        "用户住在上海",
        &["居住城市"],
        Some(&onehot(0)),
        1_000,
    );
    // lesson 与探测向量同向（cos=1.0，相似度最高）但不在抽取域 → 仍不算候选。
    // 用 tilted(0.8) 与已有 fact 行拉开到 dedup_merge 之下，两行才真正共存。
    let lesson = store::NewItem {
        kind: "lesson".to_string(),
        content: "住在上海的教训".to_string(),
        tags: vec!["lesson".to_string()],
        importance: 3,
        source: "user_stated".to_string(),
    };
    store::insert_item(&conn, &lesson, Some(&tilted(0.8)), 1_001).unwrap();
    let items = store::load_all(&conn).unwrap();
    assert_eq!(
        items.len(),
        2,
        "两行须真正共存，否则本用例测不到 kind 作用域"
    );
    // cos=0.8 ≥ hint(0.75) → 命中 fact 候选（而非更同向的 lesson）
    let r = plan_adjudication(&items, &[Some(tilted(0.8))], 0.75);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].as_ref().map(|m| m.id.as_str()), Some(id_hit.as_str()));
    // cos=0.5 < hint → 无候选
    assert!(plan_adjudication(&items, &[Some(tilted(0.5))], 0.75)[0].is_none());
    // 事实无向量（降级模式）→ 无候选
    assert!(plan_adjudication(&items, &[None], 0.75)[0].is_none());
    // top-1：两条候选取相似度更高者
    let conn2 = fact_db();
    seeded_item(&conn2, "fact", "弱相似", &[], Some(&onehot(1)), 1_000);
    let id_strong = seeded_item(&conn2, "fact", "强相似", &[], Some(&onehot(0)), 1_001);
    let items2 = store::load_all(&conn2).unwrap();
    let embs = vec![Some(onehot(0))]; // 与「强相似」同向（cos=1），与「弱相似」正交
    assert_eq!(
        plan_adjudication(&items2, &embs, 0.75)[0]
            .as_ref()
            .map(|m| m.id.as_str()),
        Some(id_strong.as_str())
    );
}

#[test]
fn adjudication_msgs_empty_without_candidates() {
    let facts = vec![mkfact("事实一", "fact", 2), mkfact("事实二", "fact", 2)];
    // 全无候选 → 空（调用方短路不发 LLM）
    assert!(build_adjudication_msgs(&facts, &[None, None]).is_empty());
    // 有候选 → 只列有候选的条目，编号与 facts 下标一致
    let cands = vec![cand("id-x"), None];
    let msgs = build_adjudication_msgs(&facts, &cands);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].role, "user");
    assert!(
        msgs[0].content.contains("[0]"),
        "编号对位：{}",
        msgs[0].content
    );
    assert!(msgs[0].content.contains("id=id-x"));
    assert!(
        !msgs[0].content.contains("事实二"),
        "无候选条目不进裁决消息"
    );
}

#[test]
fn adjudication_apply_update_keeps_original_columns() {
    let conn = fact_db();
    let now = 10_000;
    let orig_id = seeded_item(
        &conn,
        "preference",
        "用户住在上海",
        &["居住城市"],
        Some(&onehot(0)),
        1_000,
    );
    let facts = vec![mkfact("用户已经搬到北京海淀", "fact", 2)];
    let embs = vec![Some(onehot(2))];
    let adjs = vec![Adjudication::Update(orig_id.clone())];
    let counts = apply_adjudications(
        &conn,
        &facts,
        &embs,
        &adjs,
        now,
        &store::StoreParams::default(),
    )
    .unwrap();
    assert_eq!(
        counts,
        AdjudicationCounts {
            updated: 1,
            ..Default::default()
        }
    );
    let rows = store::load_all(&conn).unwrap();
    assert_eq!(rows.len(), 1, "改口不堆积");
    let m = &rows[0];
    assert_eq!(m.id, orig_id, "原条目被更新而非新增");
    assert_eq!(m.content, "用户已经搬到北京海淀");
    assert_eq!(
        m.tags,
        vec!["居住城市"],
        "tags 保持原口径（key 覆盖语义不破）"
    );
    assert_eq!(m.kind, "preference", "kind 保持原口径");
    assert_eq!(m.source, "user_stated", "source 保持原口径");
    assert_eq!(m.importance, 3, "importance 保持原口径");
    assert_eq!(
        m.embedding.as_deref(),
        Some(&onehot(2)[..]),
        "向量随新内容刷新"
    );
    assert_eq!(m.updated_at_ms, now);
}

#[test]
fn adjudication_apply_new_skip_and_vanished_target_fallback() {
    let conn = fact_db();
    let now = 10_000;
    let facts = vec![
        mkfact("全新事实一条", "fact", 3),
        mkfact("重复没增量", "fact", 2),
        mkfact("目标已消失的改口", "fact", 2),
    ];
    let embs = vec![Some(onehot(0)), Some(onehot(1)), Some(onehot(2))];
    let adjs = vec![
        Adjudication::New,
        Adjudication::Skip,
        Adjudication::Update("不存在的id".into()),
    ];
    let counts = apply_adjudications(
        &conn,
        &facts,
        &embs,
        &adjs,
        now,
        &store::StoreParams::default(),
    )
    .unwrap();
    assert_eq!(counts.inserted, 2, "New 落库 + 消失目标回退插入");
    assert_eq!(counts.skipped, 1);
    assert_eq!(counts.updated, 0);
    let rows = store::load_all(&conn).unwrap();
    assert_eq!(rows.len(), 2);
    let fresh = rows.iter().find(|m| m.content == "全新事实一条").unwrap();
    assert_eq!(fresh.source, "model_inferred", "抽取产物带推断来源");
    assert_eq!(fresh.kind, "fact");
    assert_eq!(fresh.importance, 3);
    assert!(
        rows.iter().all(|m| m.content != "重复没增量"),
        "skip 丢弃不落库"
    );
}

#[test]
fn adjudication_apply_capacity_reject_counts_skipped() {
    let conn = fact_db();
    let now = 10_000;
    // 灌一条受保护条目（importance=5 + user_stated）：容量=1 时无可淘汰 → 拒写
    let protected = seeded_item(&conn, "fact", "受保护条目", &[], Some(&onehot(9)), 1_000);
    conn.execute(
        "UPDATE mem_items SET importance = 5 WHERE id = ?1",
        rusqlite::params![protected],
    )
    .unwrap();
    let sp = store::StoreParams {
        capacity: 1,
        ..store::StoreParams::default()
    };
    let facts = vec![mkfact("第一条", "fact", 2), mkfact("第二条", "fact", 2)];
    let embs = vec![None, None];
    let adjs = vec![Adjudication::New, Adjudication::New];
    let counts = apply_adjudications(&conn, &facts, &embs, &adjs, now, &sp).unwrap();
    assert_eq!(counts.inserted, 0);
    assert_eq!(counts.skipped, 2, "容量满拒写计 skipped（数据性拒收）");
    assert_eq!(counts.failed, 0);
    assert_eq!(store::load_all(&conn).unwrap().len(), 1, "只剩受保护条目");
}
