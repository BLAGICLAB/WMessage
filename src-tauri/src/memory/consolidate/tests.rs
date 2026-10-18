//! consolidate 单测：指令解析健壮性 / merge / distill / contradiction
//! 事务应用（假向量注入）/ 候选收集 / 到点判定。不依赖 LLM 与 ONNX 模型。

use super::*;
use crate::memory::store::{self, NewItem};

fn mem_db() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    store::ensure_table(&conn).unwrap();
    conn
}

fn insert(
    conn: &rusqlite::Connection,
    kind: &str,
    content: &str,
    importance: i64,
    now: i64,
) -> String {
    let item = NewItem {
        kind: kind.into(),
        content: content.into(),
        tags: vec![],
        importance,
        source: "user_stated".into(),
    };
    match store::insert_item(conn, &item, None, now).unwrap().0 {
        store::InsertOutcome::Inserted(m) => m.id,
        _ => panic!("应插入"),
    }
}

/// 假向量：内容哈希定维（同内容同向量），只验证「重算被调用且落库」
fn fake_embed(text: &str) -> Option<Vec<f32>> {
    let mut v = vec![0f32; 512];
    v[text.len() % 512] = 1.0;
    Some(v)
}

/// 与 apply_ops 签名配套的预计算向量（模拟生产「持锁前批量算好嵌入」流程）
fn fake_embs(ops: &[ConsolidateOp]) -> Vec<Option<Vec<f32>>> {
    ops.iter().map(|op| fake_embed(op.content())).collect()
}

// ── 解析健壮性 ──

#[test]
fn parse_valid_ops() {
    let text = r#"{"ops":[
      {"action":"merge","ids":["a","b"],"content":"合并内容"},
      {"action":"contradiction","keep":"a","drop":"c","content":"裁决内容"},
      {"action":"distill","ids":["a","b"],"content":"规律"}
    ]}"#;
    let ops = parse_ops(text);
    assert_eq!(ops.len(), 3);
    assert!(matches!(&ops[0], ConsolidateOp::Merge { ids, .. } if ids == &["a", "b"]));
    assert!(
        matches!(&ops[1], ConsolidateOp::Contradiction { keep, drop_id, .. } if keep == "a" && drop_id == "c")
    );
    assert!(matches!(&ops[2], ConsolidateOp::Distill { .. }));
}

#[test]
fn parse_with_code_fence_and_prose() {
    let text = "好的，整理结果如下：\n```json\n{\"ops\":[{\"action\":\"distill\",\"ids\":[\"x\"],\"content\":\"规律\"}]}\n```\n以上。";
    let ops = parse_ops(text);
    assert_eq!(ops.len(), 1, "围栏+前后散文应被剥掉");
}

#[test]
fn parse_invalid_json_returns_empty() {
    assert!(parse_ops("这不是 JSON").is_empty());
    assert!(parse_ops("").is_empty());
    assert!(parse_ops("{\"ops\": 不是数组}").is_empty());
    assert!(parse_ops("{}").is_empty(), "缺 ops 字段");
}

#[test]
fn parse_lesson_string_form() {
    let text = r#"{"ops":[],"lesson":"遇到重复记录先合并再回答"}"#;
    assert_eq!(
        parse_lesson(text).as_deref(),
        Some("遇到重复记录先合并再回答")
    );
}

#[test]
fn parse_lesson_object_form_with_fence_and_prose() {
    let text =
        "整理如下：\n```json\n{\"ops\":[],\"lesson\":{\"content\":\"先去重再总结\"}}\n```\n以上。";
    assert_eq!(parse_lesson(text).as_deref(), Some("先去重再总结"));
}

#[test]
fn parse_lesson_missing_or_empty_is_none() {
    assert!(parse_lesson(r#"{"ops":[]}"#).is_none(), "缺字段 → None");
    assert!(
        parse_lesson(r#"{"ops":[],"lesson":""}"#).is_none(),
        "空串 → None"
    );
    assert!(
        parse_lesson(r#"{"ops":[],"lesson":{"other":1}}"#).is_none(),
        "对象缺 content → None"
    );
    assert!(parse_lesson("这不是 JSON").is_none(), "整体解析失败 → None");
}

#[test]
fn empty_ops_with_lesson_still_reaches_store() {
    // 复现：LLM 输出 `{"ops":[],"lesson":"..."}` 时，早退分支若在写 lesson
    // 之前直接 return，lesson 被静默丢弃、架空「反思双产物」。
    let text = r#"{"ops":[],"lesson":"回归验证：遇到重复记录先合并再回答"}"#;
    let ops = parse_ops(text);
    let lesson = parse_lesson(text);
    assert!(ops.is_empty(), "前置：ops 解析为空");
    let lesson = lesson.expect("lesson 应被解析出");
    let conn = mem_db();
    assert!(
        store_lesson(
            &conn,
            &lesson,
            None,
            1_700_000_000_000,
            &store::StoreParams::default()
        )
        .unwrap(),
        "空 ops 下 lesson 仍要落库"
    );
    assert!(store::load_all(&conn)
        .unwrap()
        .iter()
        .any(|m| m.content == lesson));
}

#[test]
fn parse_skips_malformed_and_unknown_ops() {
    let text = r#"{"ops":[
      {"action":"unknown_action","ids":["a"]},
      {"action":"merge","ids":["only-one"],"content":"x"},
      {"action":"merge","ids":["a","b"]},
      {"action":"contradiction","keep":"a","drop":"a","content":"自相矛盾"},
      {"action":"distill","ids":[],"content":"无来源"},
      {"action":"distill","ids":["a"],"content":"合法"}
    ]}"#;
    let ops = parse_ops(text);
    assert_eq!(
        ops.len(),
        1,
        "未知动作/单 id merge/缺 content/keep==drop/空 ids 都应跳过"
    );
    assert!(matches!(&ops[0], ConsolidateOp::Distill { .. }));
}

// ── 指令应用（事务） ──

#[test]
fn apply_merge_updates_target_and_deletes_sources() {
    let mut conn = mem_db();
    let now = 1_000_000;
    let a = insert(&conn, "fact", "用户不吃辣", 3, now);
    let b = insert(&conn, "fact", "用户讨厌辛辣", 4, now + 1);
    let c = insert(&conn, "fact", "用户对花椒过敏", 2, now + 2);
    let ops = vec![ConsolidateOp::Merge {
        ids: vec![a.clone(), b.clone(), c.clone()],
        content: "用户不吃辣且对花椒过敏".into(),
    }];
    let report = apply_ops(&mut conn, &ops, &fake_embs(&ops), now + 9).unwrap();
    assert_eq!(report.merged, 2, "3 合 1 = 删 2 条来源");
    let all = store::load_all(&conn).unwrap();
    assert_eq!(all.len(), 1);
    // 目标 = importance 最高的 b
    assert_eq!(all[0].id, b);
    assert_eq!(all[0].content, "用户不吃辣且对花椒过敏");
    assert!(all[0].embedding.is_some(), "合并后向量应重算");
    assert_eq!(all[0].updated_at_ms, now + 9);
    let _ = a;
}

#[test]
fn apply_merge_with_missing_ids_skipped() {
    let mut conn = mem_db();
    let now = 1_000_000;
    let a = insert(&conn, "fact", "条目A", 3, now);
    let ops = vec![ConsolidateOp::Merge {
        ids: vec![a, "不存在的id".into()],
        content: "合并".into(),
    }];
    let report = apply_ops(&mut conn, &ops, &fake_embs(&ops), now).unwrap();
    assert_eq!(report.merged, 0, "现存条目 <2 → 跳过");
    assert_eq!(store::load_all(&conn).unwrap().len(), 1);
}

// ── lesson 直写 ──

#[test]
fn store_lesson_inserts_lesson_row() {
    let conn = mem_db();
    let now = 1_000_000;
    assert!(store_lesson(
        &conn,
        "下次排期前先确认时区",
        None,
        now,
        &store::StoreParams::default()
    )
    .unwrap());
    let all = store::load_all(&conn).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].kind, "lesson");
    assert_eq!(all[0].importance, 3);
    assert_eq!(all[0].source, "system");
    assert_eq!(all[0].tags, vec![LESSON_SOURCE_TAG]);
}

#[test]
fn store_lesson_source_tag_does_not_collide_with_record_lesson_key() {
    // tags[0] 是 key 命名空间：record_lesson 用 `lesson`、distill 用 `reflection`。
    // 整理 lesson 若也填 `lesson`，find_by_key_tag("lesson") 会误命中它，
    // record_lesson 下次写就会判「已存在」而跳过——幂等就废了。
    let conn = mem_db();
    store_lesson(
        &conn,
        "整理产出的一条 lesson",
        None,
        1_000,
        &store::StoreParams::default(),
    )
    .unwrap();
    let l = &store::load_all(&conn).unwrap()[0];
    assert_ne!(
        l.tags.first().map(String::as_str),
        Some("lesson"),
        "整理 lesson 不得占用 record_lesson 的 key"
    );
    assert_ne!(
        l.tags.first().map(String::as_str),
        Some("reflection"),
        "整理 lesson 不得占用 distill 的 key"
    );
}

#[test]
fn store_lesson_is_idempotent_on_verbatim_content() {
    let conn = mem_db();
    let now = 1_000_000;
    let sp = store::StoreParams::default();
    assert!(store_lesson(&conn, "同一条 lesson", None, now, &sp).unwrap());
    assert!(
        !store_lesson(&conn, "同一条 lesson", None, now + 1, &sp).unwrap(),
        "逐字重复不再落第二行"
    );
    assert_eq!(store::load_all(&conn).unwrap().len(), 1);
}

#[test]
fn store_lesson_dedup_only_scopes_to_lesson_kind() {
    // 逐字去重只认 kind=lesson：同文本的 fact 不该挡住 lesson 落库
    let conn = mem_db();
    let now = 1_000_000;
    insert(&conn, "fact", "同文本", 3, now);
    assert!(store_lesson(&conn, "同文本", None, now, &store::StoreParams::default()).unwrap());
    assert_eq!(store::load_all(&conn).unwrap().len(), 2);
}

/// 测试助手：插一条 lesson 行（与 write_lesson 同款形态）
fn insert_lesson(conn: &rusqlite::Connection, content: &str, now: i64) -> String {
    let item = NewItem {
        kind: "lesson".into(),
        content: content.into(),
        tags: vec![LESSON_SOURCE_TAG.into()],
        importance: 3,
        source: "system".into(),
    };
    match store::insert_item(conn, &item, None, now).unwrap().0 {
        store::InsertOutcome::Inserted(m) => m.id,
        other => panic!("应插入，实得 {other:?}"),
    }
}

#[test]
fn apply_merge_treats_lesson_rows_like_any_other() {
    // lesson 不再享受「不参与 merge」的特殊待遇——它就是一条普通记忆行
    let mut conn = mem_db();
    let now = 1_000_000;
    let a = insert(&conn, "fact", "用户不吃辣", 3, now);
    let b = insert(&conn, "fact", "用户讨厌辛辣", 4, now + 1);
    let lesson = insert_lesson(&conn, "偏好辛辣场景要谨慎", now + 2);
    let ops = vec![ConsolidateOp::Merge {
        ids: vec![a, b, lesson.clone()],
        content: "用户不吃辣（合并）".into(),
    }];
    let report = apply_ops(&mut conn, &ops, &fake_embs(&ops), now + 9).unwrap();
    assert_eq!(report.merged, 2, "三条合成一条，删两条来源");
    let all = store::load_all(&conn).unwrap();
    assert_eq!(all.len(), 1, "只剩合并目标");
    assert_eq!(all[0].content, "用户不吃辣（合并）");
}

#[test]
fn apply_contradiction_applies_to_lesson_rows() {
    // contradiction 同样不再绕开 lesson：keep 更新、drop 删除
    let mut conn = mem_db();
    let now = 1_000_000;
    let keep = insert_lesson(&conn, "旧教训：排期先确认时区", now);
    let drop_id = insert(&conn, "fact", "排期直接按本地时间", 3, now + 1);
    let ops = vec![ConsolidateOp::Contradiction {
        keep: keep.clone(),
        drop_id,
        content: "排期先确认用户时区".into(),
    }];
    let report = apply_ops(&mut conn, &ops, &fake_embs(&ops), now + 9).unwrap();
    assert_eq!(report.contradictions, 1);
    let all = store::load_all(&conn).unwrap();
    assert_eq!(all.len(), 1, "drop 行已删");
    assert_eq!(all[0].id, keep);
    assert_eq!(all[0].content, "排期先确认用户时区");
}

#[test]
fn apply_contradiction_keeps_and_drops() {
    let mut conn = mem_db();
    let now = 1_000_000;
    let a = insert(&conn, "fact", "用户住在上海", 3, now);
    let b = insert(&conn, "fact", "用户住在北京", 4, now + 1);
    let ops = vec![ConsolidateOp::Contradiction {
        keep: b.clone(),
        drop_id: a.clone(),
        content: "用户现居北京（2026 年起）".into(),
    }];
    let report = apply_ops(&mut conn, &ops, &fake_embs(&ops), now + 9).unwrap();
    assert_eq!(report.contradictions, 1);
    let all = store::load_all(&conn).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].id, b);
    assert_eq!(all[0].content, "用户现居北京（2026 年起）");
    assert!(all[0].embedding.is_some(), "裁决后向量应重算");
}

#[test]
fn apply_distill_creates_reflection() {
    let mut conn = mem_db();
    let now = 1_000_000;
    let a = insert(&conn, "fact", "用户喜欢简短回复", 3, now);
    let b = insert(&conn, "fact", "用户讨厌冗长解释", 3, now);
    let ops = vec![ConsolidateOp::Distill {
        ids: vec![a, b],
        content: "用户沟通偏好：简洁优先".into(),
    }];
    let report = apply_ops(&mut conn, &ops, &fake_embs(&ops), now).unwrap();
    assert_eq!(report.distilled, 1);
    let all = store::load_all(&conn).unwrap();
    assert_eq!(all.len(), 3, "distill 新建不删来源");
    let r = all
        .iter()
        .find(|m| m.kind == "reflection")
        .expect("应有 reflection");
    assert_eq!(r.importance, 4);
    assert_eq!(r.source, "system");
}

#[test]
fn apply_distill_with_all_phantom_ids_skipped() {
    let mut conn = mem_db();
    let now = 1_000_000;
    insert(&conn, "fact", "条目", 3, now);
    let ops = vec![ConsolidateOp::Distill {
        ids: vec!["幻觉id1".into(), "幻觉id2".into()],
        content: "凭空规律".into(),
    }];
    let report = apply_ops(&mut conn, &ops, &fake_embs(&ops), now).unwrap();
    assert_eq!(report.distilled, 0, "幻觉 id 不得凭空造规律");
    assert_eq!(store::load_all(&conn).unwrap().len(), 1);
}

#[test]
fn apply_ops_empty_is_noop() {
    let mut conn = mem_db();
    let now = 1_000_000;
    insert(&conn, "fact", "条目", 3, now);
    let report = apply_ops(&mut conn, &[], &[], now).unwrap();
    assert_eq!(report, ConsolidateReport::default());
}

// ── 候选收集 / 到点判定 / 配置序列化 ──

#[test]
fn gather_candidates_window_and_active() {
    let conn = mem_db();
    let now = 10_000_000_000i64;
    // 窗口内新条目
    let a = insert(&conn, "fact", "新条目", 3, now - 1000);
    // 窗口外但活跃（access_count 高）
    let b = insert(&conn, "fact", "老但活跃", 3, now - 30 * 86_400_000);
    conn.execute("UPDATE mem_items SET access_count = 5 WHERE id = ?1", [&b])
        .unwrap();
    // 窗口外且不活跃 → 不进候选
    let _c = insert(&conn, "fact", "老且冷清", 3, now - 30 * 86_400_000);
    let got = gather_candidates(&conn, None, now, 100).unwrap();
    let ids: Vec<&str> = got.iter().map(|m| m.id.as_str()).collect();
    assert!(ids.contains(&a.as_str()), "窗口内条目应入选");
    assert!(ids.contains(&b.as_str()), "活跃老条目应入选");
    assert_eq!(got.len(), 2, "老且冷清的条目不入选");
    // since_ms（上次整理时间）覆盖默认 7 天窗口
    let got2 = gather_candidates(&conn, Some(now + 1000), now, 100).unwrap();
    assert_eq!(got2.len(), 1, "since 在未来时只剩活跃条目");
}

#[test]
fn gather_candidates_caps_at_limit() {
    let conn = mem_db();
    let now = 10_000_000_000i64;
    for i in 0..110 {
        insert(&conn, "fact", &format!("条目{i}"), 3, now - 1000 + i);
    }
    let got = gather_candidates(&conn, None, now, 100).unwrap();
    assert_eq!(got.len(), 100, "上限 100 条");
}

#[test]
fn classify_due_verdicts() {
    let now = 10_000_000_000i64;
    let mut cfg = ConsolidationConfig::default();
    assert_eq!(
        classify_due(&cfg, now),
        DueVerdict::InitBaseline,
        "从未整理过先记基线"
    );
    cfg.last_run_at = Some(now);
    assert_eq!(classify_due(&cfg, now), DueVerdict::Off, "刚整理过不到点");
    assert_eq!(
        classify_due(&cfg, now + 24 * 3600 * 1000 + 1),
        DueVerdict::Run,
        "daily 过 24h 到点"
    );
    cfg.enabled = false;
    assert_eq!(
        classify_due(&cfg, now + 365 * 24 * 3600 * 1000),
        DueVerdict::Off,
        "开关关永不跑"
    );
    cfg.enabled = true;
    cfg.interval = "off".into();
    assert_eq!(classify_due(&cfg, now), DueVerdict::Off);
    cfg.interval = "12h".into();
    assert_eq!(classify_due(&cfg, now + 12 * 3600 * 1000), DueVerdict::Run);
    cfg.interval = "weekly".into();
    assert_eq!(classify_due(&cfg, now + 24 * 3600 * 1000), DueVerdict::Off);
    assert_eq!(
        classify_due(&cfg, now + 7 * 24 * 3600 * 1000),
        DueVerdict::Run
    );
    cfg.interval = "垃圾值".into();
    assert_eq!(
        classify_due(&cfg, now + 365 * 24 * 3600 * 1000),
        DueVerdict::Off,
        "非法频率按关闭"
    );
}

#[test]
fn consolidation_config_serde_roundtrip_and_defaults() {
    // 老配置 JSON 没有 memoryConsolidation 字段 → BotConfig 反序列化回 None → 视图层回默认
    let cfg: crate::bot::BotConfig = serde_json::from_str("{}").unwrap();
    assert!(cfg.memory_consolidation.is_none());
    let resolved = cfg.memory_consolidation.unwrap_or_default();
    assert!(resolved.enabled);
    assert_eq!(resolved.interval, "daily");
    assert_eq!(resolved.last_run_at, None);
    // 写读回环
    let cc = ConsolidationConfig {
        enabled: false,
        interval: "weekly".into(),
        last_run_at: Some(12345),
    };
    let json = serde_json::to_string(&cc).unwrap();
    assert!(
        json.contains("\"lastRunAt\":12345"),
        "camelCase 字段名：{json}"
    );
    let back: ConsolidationConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back, cc);
}

#[test]
fn apply_ops_distill_respects_store_params_capacity() {
    // distill 走 insert_item_with——容量=1 时新 distill 条目会挤掉库中既有条目
    let mut conn = mem_db();
    let sp = store::StoreParams {
        capacity: 1,
        ..store::StoreParams::default()
    };
    // 预置一条占位记忆（distill 防幻觉守卫要求 ids 引用真实条目）
    let seed = store::insert_item_with(
        &conn,
        &NewItem {
            kind: "fact".into(),
            content: "占位条目".into(),
            tags: vec![],
            importance: 1,
            source: "system".into(),
        },
        None,
        100,
        &sp,
    )
    .unwrap();
    let seed_id = match seed.0 {
        store::InsertOutcome::Inserted(m) => m.id,
        other => panic!("应直接插入：{other:?}"),
    };
    let ops = parse_ops(&format!(
        r#"{{"ops":[{{"action":"distill","ids":["{seed_id}"],"content":"提炼规律"}}]}}"#
    ));
    let report = apply_ops_with(&mut conn, &ops, &fake_embs(&ops), 200, &sp).unwrap();
    assert_eq!(report.distilled, 1);
    let all = store::load_all(&conn).unwrap();
    assert_eq!(all.len(), 1, "容量=1：distill 条目挤掉占位条目");
    assert_eq!(all[0].content, "提炼规律");
}
