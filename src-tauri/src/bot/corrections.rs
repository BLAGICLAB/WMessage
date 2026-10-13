//! 用户纠正信号检测（Round 4 学习闭环的入口）。
//!
//! 用户消息含 push-back 模式 →
//! 1. 写 `user.correction` 审计事件到 bot.log（debug 可见）
//! 2. 持久化为 memory item（kind=fact, tags=[correction, pattern]）——后续聚类任务
//!    读记忆库走现有语义去重自动完成同类聚合，≥3 自动 record_lesson（Phase 3 接入）
//!
//! 范围（Commit 2）：事件打点 + 持久化。Phase 3 才接「聚类阈值触发 lesson」流。
//! 不在本模块引入调度/批处理——单点职责，「事件落盘」与「聚合」解耦。
//!
//! 模式选择：白名单多字短语（"错了"/"重做"/"搞反" 等），避单字歧义
//!（"不"/"别" 单独出现命中太宽）。模糊宁可漏检不要误判——审计信号噪
//! 声太大会让聚类阈值失效（见 Round 4 设计）。新增模式须补单测。

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
/// session_id 缺省时记空串（用户从挂件/匿名入口发来的会话常见）。
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
    let app2 = app.clone();
    let res = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let emb = crate::memory::embed::embed_text(&text_owned);
        let _g = crate::db::DB_WRITE_LOCK.lock().unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] corrections DB_WRITE_LOCK: {e:?}");
            e.into_inner()
        });
        let conn = crate::db::open_db(&app2).map_err(|e| e.to_string())?;
        crate::memory::store::ensure_table(&conn).map_err(|e| e.to_string())?;
        let item = crate::memory::store::NewItem {
            kind: "fact".to_string(),
            content: text_owned.clone(),
            tags: vec!["correction".to_string(), pattern_owned.clone()],
            importance: 3,
            source: "user_correction".to_string(),
        };
        let tuning = crate::bot::read_memory_tuning(&app2);
        let sp = crate::memory::store::StoreParams::of(&tuning);
        match crate::memory::store::insert_item_with(
            &conn,
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
            Ok((crate::memory::store::InsertOutcome::RefusedForeignMerge { target_key }, _)) => {
                Err(format!(
                    "correction 与受保护条目「{target_key}」语义冲突，拒写"
                ))
            }
            Ok((crate::memory::store::InsertOutcome::RejectedFull(e), _)) => Err(e),
            Err(e) => Err(e),
        }
    })
    .await;
    match res {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Warn,
                "memory.correction_save_failed",
                "pattern" => pattern,
                "error" => e,
            );
        }
        Err(e) => {
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Warn,
                "memory.correction_save_failed",
                "pattern" => pattern,
                "error" => format!("correction 写入线程 join 失败：{e}"),
            );
        }
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
}
