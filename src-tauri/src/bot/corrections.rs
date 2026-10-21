//! 用户纠正信号检测（Round 4 学习闭环）。
//!
//! 用户消息含 push-back 模式 →
//! 1. 写 `user.correction` 审计事件到 bot.log（debug 可见）
//! 2. 持久化为 memory item（kind=fact, tags=[correction, pattern]）——已有 insert_item_with
//!    语义去重（cos ≥0.92 自动合并）天然承担同类聚合
//! 3. 聚类任务读库按 cos ≥0.78 分簇，同簇 ≥3 自动 record_lesson（fire-and-forget，
//!    与 record_correction 同节流，不阻塞 bot_chat 主路径）
//!
//! 范围：三段流水（事件 / 持久化 / 聚类触发 lesson）+ scheduler 兜底定时器。
//! 即时触发（事件驱动）与定时扫描（兜底）并存：高频 correction 跟即时路径，
//! 低频或被运行时抖动（崩溃/离线）漏掉的 cluster 由 scheduler tick 补漏。
//! 两者最终都走 `cluster_correction_lessons_blocking` 同个 DB 串行段 + store
//! 语义去重（cos ≥0.92 merge）+ 阈值触发，幂等互不破坏。
//!
//! 模式选择：白名单多字短语（"错了"/"重做"/"搞反" 等），避单字歧义
//!（"不"/"别" 单独出现命中太宽）。模糊宁可漏检不要误判——审计信号噪
//! 声太大会让聚类阈值失效（见 Round 4 设计）。新增模式须补单测。

use futures_util::FutureExt;

/// 推回模式白名单：命中其一即视为用户纠正信号。
/// 多字短语优先；单字歧义词（"不"/"别"）单独出现不在此列。
pub(crate) const CORRECTION_PATTERNS: &[&str] = &[
    "错了",
    "搞错",
    "弄错",
    "重做",
    "重来",
    "搞反",
    "弄反",
    "反了",
    "应该用",
    "应该是",
    "应该改",
    "别用",
    "别这样",
];

/// 命中返回首个命中的模式（按白名单顺序），未命中返回 None。
/// 纯函数、无副作用，便于单测。
pub(crate) fn detect_correction(text: &str) -> Option<&'static str> {
    CORRECTION_PATTERNS
        .iter()
        .find(|&pat| text.contains(pat))
        .copied()
}

/// 写入 `user.correction` 审计事件（Info 级，bot.log 单行追加）。
/// session_id 缺省时记空串（匿名入口发来的会话常见）。
pub(crate) fn emit_user_correction_event<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: Option<&str>,
    text: &str,
    pattern: &str,
) {
    crate::audit_event!(
        app,
        crate::audit::AuditLevel::Info,
        "user.correction",
        "session" => session_id.unwrap_or(""),
        "pattern" => pattern,
        "text" => text.chars().take(200).collect::<String>(),
    );
}

/// 持久化为 memory item（kind=fact, tags=[correction, pattern]）——
/// 后续聚类任务读库走现有语义去重（cos ≥ 0.92 自动合并）天然完成同类聚合。
///
/// 与 `auto_lesson_on_task_failure` 同模式：spawn_blocking 单写者 + DB_WRITE_LOCK，
/// 失败仅 WARN 审计不阻塞 bot_chat 主路径。
///
/// auto_write_enabled 关闭时不持久化（用户主动关闭自动记忆即视为拒绝一切自动入库）；
/// 审计事件仍正常写——debug 可见，不影响隐私决策。
pub(crate) async fn record_correction_fact<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    text: &str,
    pattern: &str,
) {
    // 用户关掉自动记忆 → 仅审计不持久化
    if !crate::memory::auto_write_allowed(app) {
        return;
    }
    let text_owned = text.to_string();
    let pattern_owned = pattern.to_string();
    // 嵌入在持锁前算好（持锁内 ONNX 推理会让其他写者干等）
    let emb = crate::memory::embed::embed_text(&text_owned);
    let app2 = app.clone();
    let res = crate::db::lock::with_write_conn(app, move |conn| {
        crate::memory::store::ensure_table(conn).map_err(|e| e.to_string())?;
        let item = crate::memory::store::NewItem {
            kind: "fact".to_string(),
            content: text_owned,
            tags: vec!["correction".to_string(), pattern_owned],
            importance: 3,
            source: "user_correction".to_string(),
        };
        let tuning = crate::bot::read_memory_tuning(&app2);
        let sp = crate::memory::store::StoreParams::of(&tuning);
        match crate::memory::store::insert_item_with(
            conn,
            &item,
            emb.as_deref(),
            crate::memory::now_ms(),
            &sp,
        ) {
            Ok((
                crate::memory::store::InsertOutcome::Inserted(_)
                | crate::memory::store::InsertOutcome::Merged { .. },
                _,
            )) => Ok(()),
            Ok((crate::memory::store::InsertOutcome::RejectedFull(e), _)) => Err(e),
            Err(e) => Err(e),
        }
    })
    .await;
    if let Err(e) = res {
        crate::audit_event!(
            app,
            crate::audit::AuditLevel::Warn,
            "memory.correction_save_failed",
            "pattern" => pattern,
            "error" => e,
        );
    }
}

/// 单入口：审计事件 + 持久化一并做（bot_chat 调这一个）。
/// 持久化是 fire-and-forget，调用方不 await——主路径零阻塞。
pub(crate) fn record_correction<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: Option<&str>,
    text: &str,
    pattern: &str,
) {
    emit_user_correction_event(app, session_id, text, pattern);
    // async 函数不阻塞当前任务——通过 spawn 把它丢去后台执行，
    // 即使失败也只是写 WARN 审计行（详见 record_correction_fact）
    let app2 = app.clone();
    let text_owned = text.to_string();
    let pattern_owned = pattern.to_string();
    tauri::async_runtime::spawn(async move {
        record_correction_fact(&app2, &text_owned, &pattern_owned).await;
    });
}

#[cfg(test)]
mod correction_tests {
    use super::*;

    /// record_correction_fact 内部 spawn_blocking 块会构造同样的 NewItem —— 用内存库
    /// 直接执行同样的写入路径，校验 schema（kind/tags/source/importance）符合后续聚类
    /// 任务「tags 含 correction」的查询口径。
    #[test]
    fn correction_fact_payload_schema() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::memory::store::ensure_table(&conn).unwrap();
        let item = crate::memory::store::NewItem {
            kind: "fact".to_string(),
            content: "你搞错了，应该是 Y".to_string(),
            tags: vec!["correction".to_string(), "错了".to_string()],
            importance: 3,
            source: "user_correction".to_string(),
        };
        let now = crate::memory::now_ms();
        let (outcome, _) = crate::memory::store::insert_item_with(
            &conn,
            &item,
            None,
            now,
            &crate::memory::store::StoreParams::default(),
        )
        .unwrap();
        let stored = match outcome {
            crate::memory::store::InsertOutcome::Inserted(m) => m,
            other => panic!("首次无向量直插应返回 Inserted，实际 {other:?}"),
        };
        assert_eq!(stored.kind, "fact");
        assert_eq!(stored.importance, 3);
        assert_eq!(stored.source, "user_correction");
        assert_eq!(
            stored.tags,
            vec!["correction".to_string(), "错了".to_string()]
        );
        // 同 kind/source 的内容应能 load_all 取回，供后续聚类查询
        let all = crate::memory::store::load_all(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].tags.contains(&"correction".to_string()));
    }

    // 命中：常见 push-back 短语（按白名单顺序命中首个匹配）
    #[test]
    fn detects_unambiguous_push_backs() {
        // "搞错了" 含 "错了" 子串，先命中
        // 公共子串「错了」先匹配（"搞错了"/"弄错了吧" 都含此串）
        assert_eq!(detect_correction("你搞错了，应该是 X"), Some("错了"));
        assert_eq!(detect_correction("弄错了吧"), Some("错了"));
        // 公共子串「反了」先匹配（"弄反了" 含此串）
        assert_eq!(detect_correction("做反了，从头再来"), Some("反了"));
        assert_eq!(detect_correction("重做一次吧"), Some("重做"));
        // "不是" 单独不在白名单（中文里太宽，问句/否定都会命中）
        assert_eq!(detect_correction("用 Y，不是 X"), None);
        assert_eq!(detect_correction("别用 X 了"), Some("别用"));
        assert_eq!(detect_correction("应该是 X 才对"), Some("应该是"));
    }

    // 不命中：常见中性消息
    #[test]
    fn does_not_match_neutral_messages() {
        assert_eq!(detect_correction("帮我创建一个任务"), None);
        assert_eq!(detect_correction("查询一下今天的天气"), None);
        assert_eq!(detect_correction("你说的对不对？"), None); // 问句不视为纠正
        assert_eq!(detect_correction(""), None);
    }

    // 白名单与单字避歧义：单字 "不"/"别" 不在列
    #[test]
    fn single_char_avoided_to_reduce_false_positives() {
        for s in ["不要", "别说了", "不行啊", "不能这样"] {
            // 单字歧义短语不应命中（宁可漏检）
            assert_eq!(detect_correction(s), None, "{s} 不应误判");
        }
    }

    /// 在内存库预设 3 条同主题 corrections + 1 条远处噪音，验证聚类触发 lesson。
    ///
    /// 向量策略：3 条 corrections 共享 v[0]=0.9，但 2 维分量分布在不同下标上，
    /// 成对 cos=0.81——卡在 [CLUSTER_COSINE_THRESHOLD=0.78, DEDUP_MERGE_COSINE=0.92)
    /// 区间。cluster 阶段把它们并成一簇（≥3 触发 lesson），store 阶段保留 3 行不合并。
    /// 噪音 v_noise 在 v[0]=0 与三条 corrections 完全正交，独立成一簇 1 条进不了阈值。
    fn write_fact(
        conn: &rusqlite::Connection,
        content: &str,
        emb: Option<&[f32]>,
        now: i64,
        scenario_tag: &str,
    ) {
        let item = crate::memory::store::NewItem {
            kind: "fact".to_string(),
            content: content.to_string(),
            tags: vec!["correction".to_string(), scenario_tag.to_string()],
            importance: 3,
            source: "user_correction".to_string(),
        };
        let _ = crate::memory::store::insert_item_with(
            conn,
            &item,
            emb,
            now,
            &crate::memory::store::StoreParams::default(),
        )
        .unwrap();
    }

    /// [0.9, sqrt(1-0.81), 0, ..., 0] but with the secondary component at axis `axis`.
    /// All such vectors share v[0]=0.9 → pairwise cos = 0.9^2 + remainder = 0.81.
    fn topic_vec(axis: usize) -> Vec<f32> {
        let dim = crate::memory::embed::EMBED_DIM;
        let mut v = vec![0f32; dim];
        v[0] = 0.9_f32;
        v[axis.min(dim - 1)] = (1.0_f32 - 0.81_f32).sqrt();
        v
    }

    /// one-hot at index `axis`, length EMBED_DIM — orthogonal to topic vectors.
    fn noise_vec(axis: usize) -> Vec<f32> {
        let dim = crate::memory::embed::EMBED_DIM;
        let mut v = vec![0f32; dim];
        v[axis.min(dim - 1)] = 1.0;
        v
    }

    #[test]
    fn cluster_three_similar_writes_one_lesson() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::memory::store::ensure_table(&conn).unwrap();
        let now = 1_000_000i64;
        // 3 条同主题 corrections，content 各异，tags[1] 区分但都含 correction
        for (axis, content) in [
            (1usize, "用户搞错了,应该用 Y"),
            (2, "用户弄错了, 应该是 Z"),
            (3, "用户反了，应该是 W"),
        ] {
            write_fact(&conn, content, Some(&topic_vec(axis)), now, "错了");
        }
        // 1 条远处噪音（cos=0 与三条 corrections）
        write_fact(&conn, "远端噪音", Some(&noise_vec(7)), now, "错了");

        let stats = cluster_correction_lessons_blocking(
            &conn,
            &crate::memory::store::StoreParams::default(),
            now,
            0,
        )
        .unwrap();
        assert_eq!(stats.scanned, 4, "4 条 correction 都能扫到");
        assert_eq!(stats.clusters_above_threshold, 1, "近端 3 条成一簇");
        assert_eq!(stats.lessons_written, 1, "≥3 触发 lesson");

        let all = crate::memory::store::load_all(&conn).unwrap();
        // lesson 痕迹：tags 同时含 "lesson" 与 "user_correction"（store 语义去重时
        // 可能并入既有 correction 行，导致 kind 仍是 fact——但 tags/source 已被 lesson 覆盖，
        // 等价于「correction 提升为 lesson」。这里按 tags 口径校验而不是 kind。
        let lesson_tagged: Vec<_> = all
            .iter()
            .filter(|m| {
                m.tags.iter().any(|t| t == "lesson")
                    && m.tags.iter().any(|t| t == "user_correction")
            })
            .collect();
        assert_eq!(lesson_tagged.len(), 1, "lesson 痕迹行（合并也算）");
        let l = &lesson_tagged[0];
        assert_eq!(l.source, "system", "lesson 来源应是 system（自动聚类生成）");
        assert_eq!(l.importance, 4, "lesson 默认 importance=4");
        assert_eq!(l.kind, "lesson", "Merged 路径应被校正成 kind=lesson");
    }

    #[test]
    fn cluster_below_threshold_writes_no_lesson() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::memory::store::ensure_table(&conn).unwrap();
        let now = 1_000_000i64;
        // 2 条同主题（不到 3 不触发）
        for (axis, content) in [(1usize, "第一条"), (2, "第二条")] {
            write_fact(&conn, content, Some(&topic_vec(axis)), now, "错了");
        }
        let stats = cluster_correction_lessons_blocking(
            &conn,
            &crate::memory::store::StoreParams::default(),
            now,
            0,
        )
        .unwrap();
        assert_eq!(stats.scanned, 2);
        assert_eq!(stats.clusters_above_threshold, 0);
        assert_eq!(stats.lessons_written, 0);
        let all = crate::memory::store::load_all(&conn).unwrap();
        assert!(all.iter().all(|m| m.kind != "lesson"));
    }

    #[test]
    fn cluster_since_ms_filters_older_corrections() {
        // since_ms > 0 走 load_corrections_since 增量；老 correction 不应被扫到，
        // 单 tick 不再 O(N) 全表 IO。修正前这条线会让 scheduler 在 correction ≥1k
        // 时单 tick ≥100ms（Round 5 末挂账，本测试锁死升级路径）。
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::memory::store::ensure_table(&conn).unwrap();
        let now = 1_000_000i64;
        // 3 条 correction 用不同 updated_at 写入：t=100/200/300
        write_fact(&conn, "老 correction A", Some(&topic_vec(1)), 100, "错了");
        write_fact(&conn, "老 correction B", Some(&topic_vec(2)), 200, "错了");
        write_fact(&conn, "新 correction C", Some(&topic_vec(3)), 300, "错了");

        // since=150 → 只命中 200/300 两条；3 条同源近邻不够到 threshold=3
        // 不写 lesson，但 stats.scanned 应等于 2（不是 3）。
        let stats = cluster_correction_lessons_blocking(
            &conn,
            &crate::memory::store::StoreParams::default(),
            now,
            150,
        )
        .unwrap();
        assert_eq!(
            stats.scanned, 2,
            "since=150 应只扫到 updated_at > 150 的两条"
        );
        assert_eq!(stats.clusters_above_threshold, 0);
        assert_eq!(stats.lessons_written, 0);

        // since=0 全表 → 3 条同源近邻成簇到 threshold，触发 1 个 lesson。
        let stats = cluster_correction_lessons_blocking(
            &conn,
            &crate::memory::store::StoreParams::default(),
            now,
            0,
        )
        .unwrap();
        assert_eq!(stats.scanned, 3);
        assert_eq!(stats.clusters_above_threshold, 1);
        assert_eq!(stats.lessons_written, 1);
    }
}

/// 聚类阈值：同簇 correction 数量 ≥ 此值 → 触发 lesson 自动沉淀。
/// 选 3 是经验值——单条太敏感（用户偶尔口误也算），3 条才像稳定偏好。
pub(crate) const CLUSTER_LESSON_THRESHOLD: usize = 3;

/// 聚类相似度阈值（余弦）：超过视作同主题聚合。
/// 0.78 在 0.75（store 的冲突提示线）和 0.92（自动合并线）之间；偏激进保证同语义入同簇。
pub(crate) const CLUSTER_COSINE_THRESHOLD: f64 = 0.78;

/// 聚类触发 lesson 的统计结果（供审计）
#[derive(Debug, Default)]
pub(crate) struct ClusterLessonStats {
    pub scanned: usize,
    pub clusters_above_threshold: usize,
    pub lessons_written: usize,
}

/// 聚类同步内核：从 mem_items 拉所有 tags 含 "correction" 的条目，按 cos ≥ 阈值 Union-Find
/// 聚簇，簇 ≥ 阈值即视为稳定模式，调 record_lesson_core 沉淀教训。
///
/// 用纯函数 + 借用 conn，便于单测；薄壳 `try_cluster_correction_lessons` 拿 AppHandle 时
/// 再加 auto_write_allowed 闸 + spawn_blocking。
///
/// 代表条目取簇内 `id` 最小的（确定性，避免每次重排漂移让 lesson content 反复换面）。
pub(crate) fn cluster_correction_lessons_blocking(
    conn: &rusqlite::Connection,
    sp: &crate::memory::store::StoreParams,
    now_ms: i64,
    since_ms: i64,
) -> Result<ClusterLessonStats, String> {
    let mut stats = ClusterLessonStats::default();
    // since_ms <= 0 走全表 load_all（保留原即时路径语义：刚写一条 correction 就
    // 立刻触发聚类不能漏历史同源）；since_ms > 0 走 store 的 load_corrections_since
    // 增量加载，避免 scheduler 每 tick 把全表加载进内存。
    let corrections_owned: Vec<crate::memory::store::MemItem> = if since_ms <= 0 {
        crate::memory::store::load_all(conn)?
            .into_iter()
            .filter(|m| m.kind == "fact" && m.tags.iter().any(|t| t == "correction"))
            .collect()
    } else {
        crate::memory::store::load_corrections_since(conn, since_ms)?
    };
    let corrections: Vec<&crate::memory::store::MemItem> = corrections_owned.iter().collect();
    stats.scanned = corrections.len();
    if corrections.is_empty() {
        return Ok(stats);
    }

    // Union-Find 聚类：成对 cos ≥ 阈值则并入同簇
    let n = corrections.len();
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
    for i in 0..n {
        for j in (i + 1)..n {
            let cos = crate::memory::store::cosine(
                corrections[i].embedding.as_deref(),
                corrections[j].embedding.as_deref(),
            );
            if let Some(c) = cos {
                if c >= CLUSTER_COSINE_THRESHOLD {
                    union(&mut parent, i, j);
                }
            }
        }
    }

    // 按根分组
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> =
        std::collections::BTreeMap::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }

    for (_, mut idxs) in groups {
        if idxs.len() < CLUSTER_LESSON_THRESHOLD {
            continue;
        }
        stats.clusters_above_threshold += 1;
        // 代表条目：簇内 id 字典序最小（确定性）
        idxs.sort_by_key(|&i| &corrections[i].id);
        let rep = corrections[idxs[0]];
        let written = crate::memory::record_lesson_core(
            conn,
            &rep.content,
            "user_correction",
            "system",
            rep.embedding.as_deref(),
            now_ms,
            sp,
        )
        .is_ok();
        if written {
            stats.lessons_written += 1;
        }
    }
    Ok(stats)
}

/// 异步壳：bot_chat 在 record_correction 之后调一次（fire-and-forget）。
/// auto_write_allowed 关闭不触发（与持久化同门禁）。
/// since_ms > 0 走 store::load_corrections_since 增量（scheduler tick 用，
/// 每轮只扫上次 tick 之后的 correction 行）；since_ms <= 0 走全表（bot_chat
/// 即时路径用，刚写一条立刻触发不能漏历史同源）。
pub(crate) async fn try_cluster_correction_lessons<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    since_ms: i64,
) {
    if !crate::memory::auto_write_allowed(app) {
        return;
    }
    let app2 = app.clone();
    let res = crate::db::lock::with_write_conn(app, move |conn| {
        crate::memory::store::ensure_table(conn)?;
        let tuning = crate::bot::read_memory_tuning(&app2);
        let sp = crate::memory::store::StoreParams::of(&tuning);
        cluster_correction_lessons_blocking(conn, &sp, crate::memory::now_ms(), since_ms)
    })
    .await;
    match res {
        Ok(stats) if stats.clusters_above_threshold > 0 => {
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Info,
                "correction.cluster_lesson",
                "scanned" => stats.scanned.to_string(),
                "clusters" => stats.clusters_above_threshold.to_string(),
                "lessons" => stats.lessons_written.to_string(),
            );
        }
        Ok(_) => {} // 没聚到 ≥3 不打扰日志
        Err(e) => {
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Warn,
                "correction.cluster_lesson_failed",
                "error" => e,
            );
        }
    }
}

/// scheduler 兜底周期：5 分钟一轮。correction 是低频信号（用户不是 syslog），
/// 远长于 store 语义去重的 cos 阈值合并间隔；同 cluster 短时间高频 correction
/// 即时路径会触发，立即覆盖不到 scheduler 周期内的也算无意义。5 min 主要服务
/// 「崩溃后重启漏掉最后一条」与「用户离线期间攒了 N 条 correction」两种场景。
pub(crate) const CLUSTER_SCHED_TICK_SECS: u64 = 300;

/// 启动 correction 聚类定时器（App 启动调用；与 start_consolidation_scheduler
/// 同模式）：每 5 分钟一轮跑 `try_cluster_correction_lessons` 做兜底扫描。
///
/// panic recovery 与静默审计失败参照 bot_scheduler::start_scheduler 与
/// memory::consolidate::start_consolidation_scheduler —— 单轮 panic 不杀循环。
/// 幂等由 store dedup + cluster 阈值 ≥ 共同保障；同一条 cluster 多次 tick 落
/// 多次 lesson 的概率由 record_lesson_core 的 cos ≥0.92 合并机制压到 0。
pub(crate) fn start_correction_cluster_scheduler(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut ticker =
            tokio::time::interval(std::time::Duration::from_secs(CLUSTER_SCHED_TICK_SECS));
        ticker.tick().await; // 消耗首个立即触发的 tick
                             // last_tick_ms: 上一轮 tick 的 now_ms，用于给 try_cluster_correction_lessons
                             // 传 since_ms 走 load_corrections_since 增量加载，避免每 tick 全表 IO。
                             // 初值 0 → 首轮走全表（与历史行为一致），之后每轮结束刷新到 now_ms。
        let last_tick_ms = std::sync::Arc::new(std::sync::Mutex::new(0i64));
        loop {
            ticker.tick().await;
            let since_ms =
                *crate::db::lock::lock_or_recover(&last_tick_ms, "bot::corrections::last_tick_ms");
            let now_ms = crate::memory::now_ms();
            // AssertUnwindSafe 安全依据：单轮只读 db + 调一次聚类扫描；
            // panic 后无可破坏状态；下一轮从头开始。
            let app2 = app.clone();
            let last_tick_ms_2 = last_tick_ms.clone();
            let tick = std::panic::AssertUnwindSafe(async move {
                try_cluster_correction_lessons(&app2, since_ms).await;
            });
            let outcome = tick.catch_unwind().await;
            // 不论成功还是 panic，都推进 last_tick_ms；否则 panic 之后下次 tick 又
            // 全表扫一遍，违背「兜底为增量」的初衷。since_ms 推进略快于实际聚类
            // 完成的瞬间，下轮扫到的 correction 子集可能漏掉聚类进行时新写的；该
            // 子集会被下一轮之后的下下轮兜住（5 min × 2 仍可接受）。
            *crate::db::lock::lock_or_recover(&last_tick_ms_2, "bot::corrections::last_tick_ms") =
                now_ms;
            if let Err(panic) = outcome {
                let msg = crate::audit::panic_message(panic);
                crate::bot::audit_log(
                    &app,
                    &format!(
                        "correction_cluster_tick_panic | {} | 单轮 panic 已兜底，调度器继续",
                        crate::bot::truncate_for_log(&msg, 200)
                    ),
                );
            }
        }
    });
}
