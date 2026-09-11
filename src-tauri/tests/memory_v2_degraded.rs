//! 记忆 v2 降级模式验证（2026-09-09）：模型目录缺失时系统降级为纯关键词模式，
//! 嵌入/存储/检索/注入拼装全流程可用、不 panic。
//!
//! 独立集成测试二进制（独立进程）：WMESSAGE_BGE_MODEL_DIR 指向不存在目录，
//! 嵌入引擎 OnceLock 在本进程首次加载即失败并缓存 —— 之后所有 embed 调用返回 None。

use wmessage_lib::memory::rank;
use wmessage_lib::memory::store::{self, InsertOutcome, NewItem};
use wmessage_lib::memory::{embed, format_memory_block};

fn item(kind: &str, content: &str, tags: Vec<&str>, importance: i64) -> NewItem {
    NewItem {
        kind: kind.into(),
        content: content.into(),
        tags: tags.into_iter().map(|s| s.to_string()).collect(),
        importance,
        source: "user_stated".into(),
    }
}

#[test]
fn degraded_mode_full_pipeline_no_panic() {
    // 模型目录指向不存在路径 → 引擎加载失败并缓存，embed 一律 None
    std::env::set_var(embed::MODEL_DIR_ENV, "/nonexistent-wm-bge-model-dir");
    assert!(
        embed::embed_text("我不吃辣").is_none(),
        "模型缺失时 embed 必须返回 None（降级关键词模式）"
    );

    // 存储全流程：建表 → 插入（无向量，跳过去重）→ 加载
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    store::ensure_table(&conn).unwrap();
    let now = 1_757_000_000_000i64;
    let (r, hints) = store::insert_item(
        &conn,
        &item("preference", "不吃辣", vec!["口味"], 5),
        None,
        now,
    )
    .unwrap();
    assert!(matches!(r, InsertOutcome::Inserted(_)));
    assert!(hints.is_empty(), "降级模式无向量可算，不应产生冲突提示");
    store::insert_item(
        &conn,
        &item("summary", "之前聊了饮食偏好", vec![], 2),
        None,
        now,
    )
    .unwrap();

    // 检索（关键词模式）：注入快照 → 命中刷新 → 拼装记忆块
    let items = store::load_all(&conn).unwrap();
    let (inj, hit_ids) = rank::injection_snapshot(&items, "晚餐吃点什么", None, now);
    store::touch_accessed(&conn, &hit_ids, now).unwrap();
    let block = format_memory_block(&inj).expect("pinned 存在必有记忆块");
    assert!(block.starts_with("## 记忆"));
    assert!(block.contains("口味：不吃辣"), "pinned 画像段应在：{block}");
    assert!(block.contains("### 近期摘要"), "零命中兜底段应在：{block}");

    // 关键词命中路径也可用（摘要条目非 pinned，query「饮食偏好」与内容重合）
    let (inj2, _) = rank::injection_snapshot(&items, "饮食偏好", None, now);
    assert!(
        inj2.hits.iter().any(|m| m.content.contains("饮食偏好")),
        "关键词模式应能命中：{:?}",
        inj2.hits.iter().map(|m| &m.content).collect::<Vec<_>>()
    );
}
