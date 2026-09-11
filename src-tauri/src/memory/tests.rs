//! memory v2 单元测试：
//! 假 embedding 注入测语义去重三分支 / 容量淘汰 / 降级模式 / 混合打分。
//! 全部用内存库 + 手工向量，不依赖 ONNX 模型（真实模型冒烟见 embed 模块 ignored 测试）。

use super::rank;
use super::store::{self, InsertOutcome, MemItem, NewItem};

fn mem_db() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    store::ensure_table(&conn).unwrap();
    conn
}

fn item(kind: &str, content: &str) -> NewItem {
    NewItem {
        kind: kind.to_string(),
        content: content.to_string(),
        tags: Vec::new(),
        importance: 3,
        source: "user_stated".to_string(),
    }
}

/// 512 维单热向量（第 i 维为 1）
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

#[test]
fn dedup_merge_branch_high_cosine() {
    let conn = mem_db();
    let now = 1_000_000;
    let mut first_item = item("fact", "用户不吃辣");
    first_item.tags = vec!["口味".into()];
    let (r1, _) = store::insert_item(&conn, &first_item, Some(&onehot(0)), now).unwrap();
    let InsertOutcome::Inserted(first) = r1 else {
        panic!("首条应插入")
    };
    // 同向量（cos=1 ≥ 0.92）→ 合并更新，不新增
    let mut second = item("fact", "用户不吃辣，微辣也不行");
    second.tags = vec!["忌口".into()];
    let (r2, hints) = store::insert_item(&conn, &second, Some(&onehot(0)), now + 1).unwrap();
    assert!(hints.is_empty());
    let InsertOutcome::Merged { item: m, orig_key } = r2 else {
        panic!("应合并")
    };
    assert_eq!(m.id, first.id, "合并更新的是同一条");
    assert_eq!(m.content, "用户不吃辣，微辣也不行");
    assert_eq!(m.access_count, 1, "合并刷新访问计数");
    assert_eq!(
        orig_key.as_deref(),
        Some("口味"),
        "orig_key 带出被合并原条目的 key（非新 key）"
    );
    assert_eq!(m.tags, vec!["忌口"], "条目本身 tags 已覆盖为新值");
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM mem_items", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "合并不新增行");
}

#[test]
fn dedup_hint_branch_mid_cosine() {
    let conn = mem_db();
    let now = 1_000_000;
    let mut tagged = item("fact", "住在上海");
    tagged.tags = vec!["城市".into()];
    store::insert_item(&conn, &tagged, Some(&onehot(0)), now).unwrap();
    // cos=0.8 ∈ [0.75, 0.92) → 不拦截，带冲突提示
    let (r, hints) = store::insert_item(
        &conn,
        &item("fact", "住在上海浦东"),
        Some(&tilted(0.8)),
        now + 1,
    )
    .unwrap();
    assert!(matches!(r, InsertOutcome::Inserted(_)), "中间区间仍插入");
    assert_eq!(hints.len(), 1);
    assert!(
        hints[0].contains("key=城市"),
        "提示带已有 key：{}",
        hints[0]
    );
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM mem_items", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 2);
}

#[test]
fn dedup_low_cosine_inserts_without_hint() {
    let conn = mem_db();
    let now = 1_000_000;
    store::insert_item(&conn, &item("fact", "喜欢摄影"), Some(&onehot(0)), now).unwrap();
    let (r, hints) =
        store::insert_item(&conn, &item("fact", "讨厌香菜"), Some(&onehot(1)), now + 1).unwrap();
    assert!(matches!(r, InsertOutcome::Inserted(_)));
    assert!(hints.is_empty());
}

#[test]
fn dedup_skipped_in_degraded_mode() {
    let conn = mem_db();
    let now = 1_000_000;
    // 无向量（降级模式）：相同内容也照插，不做语义去重
    store::insert_item(&conn, &item("fact", "用户不吃辣"), None, now).unwrap();
    let (r, hints) = store::insert_item(&conn, &item("fact", "用户不吃辣"), None, now + 1).unwrap();
    assert!(matches!(r, InsertOutcome::Inserted(_)));
    assert!(hints.is_empty());
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM mem_items", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 2);
}

#[test]
fn capacity_eviction_order_and_protection() {
    let conn = mem_db();
    let now = 1_000_000;
    // 灌满 500 条（importance=1，越旧越该淘汰）
    for i in 0..store::MAX_MEM_ITEMS {
        let mut it = item("fact", &format!("旧记忆{i}"));
        it.importance = 1;
        store::insert_item(&conn, &it, None, now - (1000 - i) * 86_400_000).unwrap();
    }
    // 把其中一条提为受保护（importance=5 + user_stated）
    let protected_id = store::load_all(&conn).unwrap()[0].id.clone();
    conn.execute(
        "UPDATE mem_items SET importance = 5, source = 'user_stated' WHERE id = ?1",
        [&protected_id],
    )
    .unwrap();
    // 再插一条 → 淘汰最低分的非保护条目，受保护条目存活
    let (r, _) = store::insert_item(&conn, &item("fact", "新记忆"), None, now).unwrap();
    assert!(matches!(r, InsertOutcome::Inserted(_)));
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM mem_items", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, store::MAX_MEM_ITEMS, "容量封顶 500");
    let survived: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM mem_items WHERE id = ?1",
            [&protected_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(survived, 1, "importance=5 且 user_stated 不可淘汰");
}

#[test]
fn capacity_full_of_protected_rejects_write() {
    let conn = mem_db();
    let now = 1_000_000;
    for i in 0..store::MAX_MEM_ITEMS {
        let mut it = item("fact", &format!("受保护{i}"));
        it.importance = 5;
        it.source = "user_stated".into();
        store::insert_item(&conn, &it, None, now + i).unwrap();
    }
    let (r, _) = store::insert_item(&conn, &item("fact", "挤不进来"), None, now + 9999).unwrap();
    let InsertOutcome::RejectedFull(msg) = r else {
        panic!("应拒写")
    };
    assert!(msg.contains("上限"), "{msg}");
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM mem_items", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, store::MAX_MEM_ITEMS);
}

#[test]
fn hybrid_score_semantic_dominates() {
    let now = 1_000_000_000;
    let mk = |content: &str, emb: Vec<f32>, importance: i64| MemItem {
        id: "x".into(),
        kind: "fact".into(),
        content: content.into(),
        tags: vec![],
        importance,
        source: "user_stated".into(),
        created_at_ms: now,
        updated_at_ms: now,
        access_count: 0,
        last_accessed_at_ms: None,
        embedding: Some(emb),
    };
    let q = onehot(0);
    let kws = vec!["不存在的关键词".to_string()];
    // 语义相同但零关键词重合 vs 关键词重合但语义正交
    let sem_hit = mk("语义相关", onehot(0), 3);
    let kw_hit = mk("不存在的关键词", onehot(1), 3);
    let s_sem = rank::hybrid_score(&kws, Some(&q), &sem_hit, now);
    let s_kw = rank::hybrid_score(&kws, Some(&q), &kw_hit, now);
    assert!(s_sem > s_kw, "语义权重 0.55 应主导：{s_sem} vs {s_kw}");
}

#[test]
fn hybrid_score_degraded_renormalizes_weights() {
    let now = 1_000_000_000;
    let item = MemItem {
        id: "x".into(),
        kind: "fact".into(),
        content: "关键词".into(),
        tags: vec![],
        importance: 5,
        source: "user_stated".into(),
        created_at_ms: now,
        updated_at_ms: now,
        access_count: 0,
        last_accessed_at_ms: None,
        embedding: None,
    };
    let kws = rank::extract_keywords("关键词");
    let s = rank::hybrid_score(&kws, None, &item, now);
    // 无向量：kw=1（全命中）、imp=1、rec=1 → (0.2+0.15+0.10)/0.45 = 1.0
    assert!((s - 1.0).abs() < 1e-9, "降级模式满分应归一到 1.0：{s}");
}

#[test]
fn hybrid_score_mixed_mode_guards_vectorless_items() {
    // 混合库（查询有向量、部分条目无向量）：无向量条目关键词零重合 → 0 分，
    // 不得靠重要度/新近度常正项混进 top-N
    let now = 1_000_000_000;
    let mk = |content: &str, emb: Option<Vec<f32>>| MemItem {
        id: "x".into(),
        kind: "fact".into(),
        content: content.into(),
        tags: vec![],
        importance: 5, // 拉满重要度/新近度，放大常正项
        source: "user_stated".into(),
        created_at_ms: now,
        updated_at_ms: now,
        access_count: 0,
        last_accessed_at_ms: None,
        embedding: emb,
    };
    let q = onehot(0);
    let kws = rank::extract_keywords("关键词"); // CJK bigram：["关键", "键词"]
                                                // 无向量 + 零重合 → 0 分（修复前 = 0.15+0.10 > 0 会漏进 top-5）
    let no_vec_no_kw = mk("完全无关的内容", None);
    assert_eq!(
        rank::hybrid_score(&kws, Some(&q), &no_vec_no_kw, now),
        0.0,
        "混合模式下无向量零重合条目必须 0 分"
    );
    // 无向量 + 有重合 → 保留关键词得分（降级检索正路）
    let no_vec_kw = mk("关键词命中", None);
    assert!(
        rank::hybrid_score(&kws, Some(&q), &no_vec_kw, now) > 0.0,
        "无向量但关键词有重合应正常得分"
    );
    // 有向量 + 零重合 → 语义可算，不适用守卫（既有行为不变）
    let with_vec = mk("完全无关的内容", Some(onehot(1)));
    assert!(
        rank::hybrid_score(&kws, Some(&q), &with_vec, now) > 0.0,
        "有向量条目由语义项负责，守卫不适用"
    );
    // 检索层验证：混合库中无向量零重合条目不进结果
    let hits = rank::hybrid_search(&[no_vec_no_kw], "关键词", Some(&q), now, 5);
    assert!(hits.is_empty(), "无向量零重合条目不得进 top-N");
}

#[test]
fn injection_snapshot_three_sections() {
    let now = 1_000_000_000;
    let mk = |id: &str, kind: &str, content: &str, importance: i64| MemItem {
        id: id.into(),
        kind: kind.into(),
        content: content.into(),
        tags: vec![id.to_string()],
        importance,
        source: "user_stated".into(),
        created_at_ms: now,
        updated_at_ms: now,
        access_count: 0,
        last_accessed_at_ms: None,
        embedding: None,
    };
    let items = vec![
        mk("p1", "profile", "用户是程序员", 5), // pinned
        mk("f1", "fact", "关键词命中条目", 3),
        mk("s1", "summary", "最近摘要", 2),
    ];
    let (inj, hit_ids) = rank::injection_snapshot(&items, "关键词", None, now);
    assert_eq!(inj.pinned.len(), 1);
    assert!(inj.pinned[0].id == "p1");
    assert!(!hit_ids.is_empty(), "关键词命中应进 hits");
    assert!(
        inj.pinned.iter().all(|p| !hit_ids.contains(&p.id)),
        "pinned 不进 hits"
    );
    assert_eq!(inj.recent.len(), 1);
    assert_eq!(inj.recent[0].id, "s1");
}

#[test]
fn degraded_full_flow_keyword_only_no_panic() {
    // 降级模式全流程（无向量）：插入 → 注入快照 → 拼装，不 panic
    let conn = mem_db();
    let now = 1_000_000_000;
    let mut it = item("preference", "不吃辣");
    it.importance = 5;
    it.tags = vec!["口味".into()];
    store::insert_item(&conn, &it, None, now).unwrap();
    store::insert_item(&conn, &item("summary", "之前聊了饮食"), None, now).unwrap();
    let items = store::load_all(&conn).unwrap();
    let (inj, hit_ids) = rank::injection_snapshot(&items, "晚餐吃什么", None, now);
    store::touch_accessed(&conn, &hit_ids, now).unwrap();
    let block = super::format_memory_block(&inj).expect("pinned 存在必有记忆块");
    assert!(block.starts_with("## 记忆"));
    assert!(block.contains("### 用户画像与偏好"));
    assert!(block.contains("口味：不吃辣"), "{block}");
    assert!(block.contains("### 近期摘要"), "{block}");
}

#[test]
fn recall_key_tag_update_and_delete() {
    let conn = mem_db();
    let now = 1_000_000;
    let mut it = item("fact", "上海");
    it.tags = vec!["城市".into()];
    store::insert_item(&conn, &it, Some(&onehot(0)), now).unwrap();
    let existing = store::find_by_key_tag(&conn, "城市")
        .unwrap()
        .expect("应找到");
    store::update_by_id(
        &conn,
        &existing.id,
        "北京",
        3,
        "user_stated",
        "fact",
        Some(&onehot(0)),
        now + 1,
    )
    .unwrap();
    let updated = store::find_by_key_tag(&conn, "城市").unwrap().unwrap();
    assert_eq!(updated.content, "北京", "同 key 覆盖更新");
    assert_eq!(store::load_all(&conn).unwrap().len(), 1, "覆盖不堆积");
    assert!(store::delete_by_key_tag(&conn, "城市").unwrap());
    assert!(
        !store::delete_by_key_tag(&conn, "城市").unwrap(),
        "重复删除返回 false"
    );
}

#[test]
fn degraded_update_clears_stale_embedding() {
    let conn = mem_db();
    let now = 1_000_000;
    let mut it = item("fact", "旧内容");
    it.tags = vec!["k".into()];
    store::insert_item(&conn, &it, Some(&onehot(0)), now).unwrap();
    let id = store::find_by_key_tag(&conn, "k").unwrap().unwrap().id;
    // 降级模式（无新向量）覆盖更新 content → 旧向量随内容作废（置 NULL）
    store::update_by_id(
        &conn,
        &id,
        "新内容",
        3,
        "user_stated",
        "fact",
        None,
        now + 1,
    )
    .unwrap();
    let m = store::find_by_key_tag(&conn, "k").unwrap().unwrap();
    assert_eq!(m.content, "新内容");
    assert!(m.embedding.is_none(), "内容变了且无新向量 → 旧向量必须清空");
}

#[test]
fn same_content_update_preserves_embedding() {
    let conn = mem_db();
    let now = 1_000_000;
    let mut it = item("fact", "内容不变");
    it.tags = vec!["k".into()];
    store::insert_item(&conn, &it, Some(&onehot(0)), now).unwrap();
    let id = store::find_by_key_tag(&conn, "k").unwrap().unwrap().id;
    // content 没变、只动 importance（无新向量）→ 旧向量保留
    store::update_by_id(
        &conn,
        &id,
        "内容不变",
        5,
        "user_stated",
        "fact",
        None,
        now + 1,
    )
    .unwrap();
    let m = store::find_by_key_tag(&conn, "k").unwrap().unwrap();
    assert_eq!(m.importance, 5);
    assert!(m.embedding.is_some(), "内容没变 → 保留旧向量");
}

#[test]
fn validate_fact_kv_rejects_ascii_comma() {
    assert!(super::validate_fact_kv("a,b", "v")
        .unwrap_err()
        .contains("逗号"));
    assert!(
        super::validate_fact_kv("a，b", "v").is_ok(),
        "中文逗号不受影响"
    );
    assert!(
        super::validate_fact_kv("k", "含,逗号").is_ok(),
        "value 不进 tags，逗号合法"
    );
}

// ───────────────────────── lesson（教训记忆） ─────────────────────────

#[test]
fn record_lesson_writes_with_defaults() {
    let conn = mem_db();
    let now = 1_000_000;
    let msg = super::record_lesson_core(
        &conn,
        "执行文档修订任务时先用 extract_document 读原文，不要凭记忆改写",
        "create_word_revisions",
        "model_inferred",
        None,
        now,
    );
    assert!(msg.starts_with("已记录教训"), "{msg}");
    let items = store::load_all(&conn).unwrap();
    assert_eq!(items.len(), 1);
    let l = &items[0];
    assert_eq!(l.kind, "lesson");
    assert_eq!(l.importance, 4, "lesson 默认 importance=4");
    assert_eq!(l.source, "model_inferred");
    assert_eq!(l.tags, vec!["lesson", "create_word_revisions"]);
}

#[test]
fn record_lesson_semantic_dedup_merges() {
    let conn = mem_db();
    let now = 1_000_000;
    super::record_lesson_core(&conn, "教训A", "场景", "system", Some(&onehot(0)), now);
    // 同向量（cos=1）→ 合并更新不新增
    let msg = super::record_lesson_core(
        &conn,
        "教训A补充版",
        "场景",
        "system",
        Some(&onehot(0)),
        now + 1,
    );
    assert!(msg.contains("合并"), "{msg}");
    let items = store::load_all(&conn).unwrap();
    assert_eq!(items.len(), 1, "同类教训合并不堆积");
    assert_eq!(items[0].content, "教训A补充版");
}

#[test]
fn record_lesson_validation() {
    let conn = mem_db();
    assert!(super::record_lesson_core(&conn, "", "s", "system", None, 0).contains("不能为空"));
    let long = "x".repeat(801);
    assert!(super::record_lesson_core(&conn, &long, "s", "system", None, 0).contains("太长"));
    let long_s = "s".repeat(51);
    assert!(
        super::record_lesson_core(&conn, "ok", &long_s, "system", None, 0).contains("scenario")
    );
    // scenario 含英文逗号被拒（tags 逗号分隔存储）；中文逗号不受影响
    assert!(super::record_lesson_core(&conn, "ok", "a,b", "system", None, 0).contains("逗号"));
    assert!(
        super::record_lesson_core(&conn, "ok", "文档，修订", "system", None, 0)
            .starts_with("已记录教训")
    );
}

#[test]
fn task_failure_lesson_content_shape() {
    let (content, tags) = super::task_failure_lesson("写周报", "LLM API 401 未授权");
    assert!(
        content.contains("写周报") && content.contains("401"),
        "{content}"
    );
    assert!(content.len() <= 800);
    assert_eq!(tags, vec!["lesson", "task_exec"]);
    // 超长错误截断
    let (c2, _) = super::task_failure_lesson("t", &"e".repeat(1000));
    assert!(c2.chars().count() <= 800);
}

#[test]
fn lesson_injected_in_fourth_section() {
    let conn = mem_db();
    let now = 1_000_000_000;
    // 一条 lesson + 一条普通 fact + 一条 summary
    super::record_lesson_core(&conn, "修订文档前先备份", "文档修订", "system", None, now);
    let mut f = item("fact", "关键词普通记忆");
    f.importance = 3;
    store::insert_item(&conn, &f, None, now).unwrap();
    store::insert_item(&conn, &item("summary", "摘要一条"), None, now).unwrap();
    let items = store::load_all(&conn).unwrap();
    let (inj, hit_ids) = rank::injection_snapshot(&items, "文档修订 关键词", None, now);
    assert_eq!(inj.lessons.len(), 1, "lesson 应进第四段");
    assert!(
        inj.hits.iter().all(|m| m.kind != "lesson"),
        "lesson 不进相关记忆段"
    );
    assert!(
        hit_ids.contains(&inj.lessons[0].id),
        "lesson 命中也算访问强化"
    );
    let block = super::format_memory_block(&inj).unwrap();
    assert!(block.contains("### 经验教训"), "{block}");
    assert!(block.contains("修订文档前先备份"), "{block}");
    // 段序：经验教训 在 近期摘要 之后
    let p1 = block.find("### 近期摘要").unwrap();
    let p2 = block.find("### 经验教训").unwrap();
    assert!(p1 < p2, "经验教训段追加在最后：{block}");
}

#[test]
fn lesson_section_absent_without_lessons() {
    let conn = mem_db();
    let now = 1_000_000;
    store::insert_item(&conn, &item("fact", "关键词条目"), None, now).unwrap();
    let items = store::load_all(&conn).unwrap();
    let (inj, _) = rank::injection_snapshot(&items, "关键词", None, now);
    let block = super::format_memory_block(&inj).unwrap();
    assert!(!block.contains("经验教训"), "无 lesson 不出第四段：{block}");
}
