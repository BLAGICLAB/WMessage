//! U19 写入时冲突裁决——改口语料端到端（mock LLM + 真实嵌入 + 真实 SQLite）。
//!
//! 走真管线 [`wmessage_lib::memory::extract::run_extract_with`]：LLM 抽取
//! （mock）→ 语义嵌入（真实 ONNX）→ top-1 相似候选 → LLM 逐条裁决（mock）→
//! 应用。断言改口语料 ≥9/10 组「更新原条目」而非新增（mock 确定性下应为
//! 10/10，≥9 是 spec 验收口径）。
//!
//! 环境依赖：仓库根 bge-small-zh-v1.5/（冲突候选靠真实嵌入算相似度；引擎
//! 不可用时本测试无意义，打印提示后跳过——不阻塞无模型环境的全量测试）。
//! 共享 target/debug/deps 数据目录地雷（paths.rs 留档）：语料用唯一标记 key
//! （tags[0]=u19-conflict-*）+ 开场清理前移 + 结尾清理（承 U15 实录口径）。

mod mock_llm_shared {
    include!("mock_llm.rs");
}
use mock_llm_shared::{MockBehavior, MockLlmServer};

use wmessage_lib::bot::ApiProvider;
use wmessage_lib::bot_chat::{summarize_http, ChatMsg};
use wmessage_lib::db;
use wmessage_lib::memory::embed;
use wmessage_lib::memory::extract::run_extract_with;
use wmessage_lib::memory::store::{self, InsertOutcome, NewItem, StoreParams};
use wmessage_lib::memory::AutoExtract;

fn mock_handle() -> tauri::AppHandle<tauri::test::MockRuntime> {
    Box::leak(Box::new(tauri::test::mock_app()))
        .handle()
        .clone()
}

/// 改口语料：同句仅换关键值（其余语境全同）——贴近真实改口的「小增量」形态，
/// 语义相似度稳定落在候选阈值（dedupHint=0.75）之上；内容本身即唯一标记
fn correction_corpus() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        (
            "u19-conflict-00",
            "用户目前居住的城市是上海市",
            "用户目前居住的城市是成都市",
        ),
        (
            "u19-conflict-01",
            "用户目前主力使用的编程语言是Python",
            "用户目前主力使用的编程语言是Rust",
        ),
        (
            "u19-conflict-02",
            "用户现在日常使用的手机是iPhone",
            "用户现在日常使用的手机是小米",
        ),
        (
            "u19-conflict-03",
            "用户要求邮件回复的风格保持正式",
            "用户要求邮件回复的风格保持简洁",
        ),
        (
            "u19-conflict-04",
            "wmessage目前部署在腾讯云服务器上",
            "wmessage目前部署在NAS上",
        ),
        (
            "u19-conflict-05",
            "用户固定参加的例会安排在周一",
            "用户固定参加的例会安排在周三",
        ),
        (
            "u19-conflict-06",
            "用户目前的健身频率是每周三次",
            "用户目前的健身频率是每周五次",
        ),
        (
            "u19-conflict-07",
            "用户平时喝咖啡只喝美式",
            "用户平时喝咖啡只喝拿铁",
        ),
        (
            "u19-conflict-08",
            "用户终端默认使用的shell是bash",
            "用户终端默认使用的shell是zsh",
        ),
        (
            "u19-conflict-09",
            "用户在macOS上装软件用Homebrew",
            "用户在macOS上装软件用MacPorts",
        ),
    ]
}

/// 按内容精确清理（历史失败轮 fallback 插入的行没有 key 标签，按内容兜底）
fn delete_by_content(conn: &rusqlite::Connection, content: &str) {
    conn.execute(
        "DELETE FROM mem_items WHERE content = ?1",
        rusqlite::params![content],
    )
    .ok();
}

/// 语料双向清理：key 标签行 + 新旧内容行（开场前移 + 收尾兜底，承 U15 实录口径）
fn cleanup_corpus(conn: &rusqlite::Connection, corpus: &[(&str, &str, &str)]) {
    for (key, old, new) in corpus {
        store::delete_by_key_tag(conn, key).ok();
        delete_by_content(conn, old);
        delete_by_content(conn, new);
    }
}

/// 非流式 OpenAI 应答（content 原文；mock 侧负责包 envelope 与转义）
fn openai_reply(content: String) -> MockBehavior {
    MockBehavior::OpenAiJsonReply(content)
}

#[tokio::test]
async fn conflict_adjudication_updates_original_on_corrections() {
    // 嵌入引擎前置：候选发现靠真实语义相似度，无模型时本测试无意义（跳过）
    if embed::engine_status().is_err() {
        eprintln!("[skip] 嵌入引擎不可用（仓库根缺 bge-small-zh-v1.5/），改口集成测试跳过");
        return;
    }
    let corpus = correction_corpus();
    let app = mock_handle();
    let server = MockLlmServer::start();

    // 开场清理前移：清掉上次运行可能残留的语料行（panic 后下一次运行自愈）
    let conn = db::open_db(&app).expect("共享测试库应可打开");
    store::ensure_table(&conn).unwrap();
    cleanup_corpus(&conn, &corpus);

    // 播种原条目（真实嵌入 + 唯一标记 key）：记录行 id 供裁决回指
    let sp = StoreParams {
        dedup_merge: 1.0, // 种子互不合并：一条一行 id 稳定
        ..StoreParams::default()
    };
    let mut orig_ids: Vec<String> = Vec::new();
    for (i, (key, old, _)) in corpus.iter().enumerate() {
        let emb = embed::embed_text(old).expect("真实嵌入应可用");
        let item = NewItem {
            kind: "fact".to_string(),
            content: old.to_string(),
            tags: vec![key.to_string()],
            importance: 3,
            source: "user_stated".to_string(),
        };
        match store::insert_item_with(&conn, &item, Some(&emb), 1_000 + i as i64, &sp).unwrap() {
            (InsertOutcome::Inserted(m), _) => orig_ids.push(m.id),
            other => panic!("语料种子 {key} 应独立插入，实得 {other:?}"),
        }
    }
    drop(conn); // 管线自己开连接，测试侧先放手

    // 预存 mock 行为（按管线调用序消费）：每组 抽取 → 裁决(update 原 id)
    for ((_, _, new), orig_id) in corpus.iter().zip(&orig_ids) {
        let extract_json = format!(r#"[{{"content":"{new}","kind":"fact","importance":3}}]"#);
        server.push_behavior(openai_reply(extract_json));
        let adjudicate_json =
            format!(r#"[{{"index":0,"action":"update","existing_id":"{orig_id}"}}]"#);
        server.push_behavior(openai_reply(adjudicate_json));
    }

    // 逐组走真管线（LLM 调用方 = summarize_http 直连 mock，同 run_model_loop_core 先例）
    let client = reqwest::Client::new();
    for (key, _, new) in &corpus {
        let msgs = vec![
            ChatMsg {
                role: "user".to_string(),
                content: format!("改口一下：{new}"),
            },
            ChatMsg {
                role: "assistant".to_string(),
                content: "好的，已经更新记忆。".to_string(),
            },
        ];
        let base_url = server.base_url.clone();
        let client = client.clone();
        let llm = move |prompt: &'static str, msgs: Vec<ChatMsg>| {
            let client = client.clone();
            let base_url = base_url.clone();
            async move {
                summarize_http(
                    &client,
                    &base_url,
                    "test-key",
                    "mock-model",
                    prompt,
                    &msgs,
                    ApiProvider::Openai,
                    2_048,
                    None,
                    None,
                )
                .await
            }
        };
        run_extract_with(&app, "u19-conflict-test", msgs, AutoExtract::Auto, llm)
            .await
            .unwrap_or_else(|e| panic!("管线应成功（{key}）：{e}"));
    }

    // 断言：裁决阶段真的 engaged（每组 2 次 LLM 调用：抽取 + 裁决）
    assert_eq!(
        server.request_count(),
        corpus.len() * 2,
        "每组应恰好调用两次 LLM（抽取+裁决），实得 {}",
        server.request_count()
    );
    // 裁决请求里应带候选记忆清单（build_adjudication_msgs 接线 sanity）
    let bodies = server.request_bodies();
    assert!(
        bodies.iter().any(|b| b.contains("已有记忆 id=")),
        "裁决请求应含候选记忆清单"
    );

    // 断言：≥9/10 组「更新原条目」（mock 确定性下应 10/10）
    let conn = db::open_db(&app).expect("共享测试库应可打开");
    let mut updated = 0usize;
    for (key, old, new) in &corpus {
        let row = store::find_by_key_tag(&conn, key)
            .unwrap()
            .unwrap_or_else(|| panic!("原条目 {key} 不应消失"));
        assert_eq!(
            row.tags.first().map(String::as_str),
            Some(*key),
            "改写保留原 tags（key 覆盖语义不破）"
        );
        if row.content == *new {
            updated += 1;
        } else {
            eprintln!("[warn] 组 {key} 未更新为改口内容：{}", row.content);
        }
        let stale: Vec<store::MemItem> = store::load_all(&conn)
            .unwrap()
            .into_iter()
            .filter(|m| m.content == *old)
            .collect();
        assert!(
            stale.is_empty(),
            "组 {key}：旧内容不应残留堆积（改口不新增）"
        );
    }
    assert!(
        updated >= 9,
        "改口 ≥9/10 应更新原条目，实得 {updated}/{}",
        corpus.len()
    );

    // 收尾清理（best-effort，失败不影响结论——开场清理会兜底）
    cleanup_corpus(&conn, &corpus);
}
