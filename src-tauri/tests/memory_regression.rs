//! 记忆模块回归基准（2026-09-05，设计 docs/BOT-MEMORY-DESIGN.md 第 8 节）
//!
//! 仿 LongMemEval 思路的脚本化多轮对话：mock LLM 队列式 behavior 驱动
//! 「告知偏好 → 闲聊 → 验证召回」「改口 → 验证覆盖」等场景，端到端验证
//! 记忆模块的召回 / 覆盖 / 冲突提示 / 推断标记 / 遗忘 / 兜底行为。
//!
//! 设计要点：
//! - 全链路走生产同一份代码：消息拼装顺序（主 system → 记忆块 system → 历史 → 本轮
//!   user）与 bot_chat 一致；取数走 memory_injection_snapshot，拼装走
//!   format_memory_block，工具循环走 run_model_loop_core（mock LLM server）
//! - 工具执行用生产同一内核 remember_fact_core（Connection 注入，绕开 AppHandle；
//!   tool_remember_fact 只剩参数解析 + open_db + 锁的薄壳）
//! - 断言对象是「发给 LLM 的真实请求体」（mock server 记录 request_bodies）——
//!   记忆块是否出现在 system 消息序列、工具结果是否回填，看出站请求而非测试侧自拼
//! - 每个场景独立 tempfile 库文件，互不影响

mod mock_llm_shared {
    include!("mock_llm.rs");
}

use mock_llm_shared::{MockBehavior, MockLlmServer, ToolCallResponse};
use wmessage_lib::bot::{
    noop_replan, remember_fact_core, run_model_loop_core, ApiProvider, AuditLevel, LlmHttp,
    ModelLoopDeps, DEFAULT_MAX_TOKENS,
    StopGuard, TaskRef,
};
use wmessage_lib::bot_chat::{format_memory_block, summarize_http, truncate_with_summary_core, ChatMsg};
use wmessage_lib::db::{ensure_bot_facts_memory_columns, memory_injection_snapshot, memory_insert};

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

// ────────────────────────────────────────────────────────────────────
// 测试基建：mock LLM 连接参数 / 副作用 noop 出口 / 对话驱动器
// ────────────────────────────────────────────────────────────────────

fn core_http(server: &MockLlmServer) -> LlmHttp {
    LlmHttp {
        client: reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("build client"),
        base_url: server.base_url.clone(),
        api_key: "test-key".into(),
        model: "mock-model".into(),
        provider: ApiProvider::Openai,
        max_tokens: DEFAULT_MAX_TOKENS,
    }
}

/// 副作用出口全 noop（与 llm_integration.rs CoreHarness 同模式）
struct Harness {
    stop: StopGuard,
    emit: Box<dyn Fn(&str, serde_json::Value) + Send + Sync>,
    audit: Box<dyn Fn(AuditLevel, &'static str, Vec<(&'static str, String)>) + Send + Sync>,
    audit_log: Box<dyn Fn(&str) + Send + Sync>,
    skill_finish: Box<dyn Fn(bool, &str) -> String + Send + Sync>,
}

impl Harness {
    fn new() -> Self {
        Self {
            stop: StopGuard::new(false, None),
            emit: Box::new(|_, _| {}),
            audit: Box::new(|_, _, _| {}),
            audit_log: Box::new(|_| {}),
            skill_finish: Box::new(|_, _| String::new()),
        }
    }

    fn deps(&self) -> ModelLoopDeps<'_> {
        ModelLoopDeps {
            emit: &*self.emit,
            audit: &*self.audit,
            audit_log: &*self.audit_log,
            skill_finish: &*self.skill_finish,
        }
    }
}

/// 工具执行：remember_fact 走生产内核 remember_fact_core（参数归一化与
/// tool_remember_fact 一致：category/source 白名单、importance clamp 由 mock
/// 侧给合法值，越界值不属于本回归基准的覆盖面）
fn exec_remember(conn: &rusqlite::Connection, args: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(args).expect("工具参数应为 JSON");
    let key = v["key"].as_str().unwrap_or("").trim().to_string();
    let value = v["value"].as_str().unwrap_or("").trim().to_string();
    let category = v["category"].as_str().unwrap_or("general");
    let importance = v["importance"].as_i64().unwrap_or(3).clamp(1, 5);
    let source = v["source"].as_str().unwrap_or("user_stated");
    remember_fact_core(conn, &key, &value, category, importance, source, now_ms())
}

/// 脚本化对话驱动器：一份 tempfile 库 + 一个 mock server + 会话历史。
/// run_turn 复刻 bot_chat 主流程的消息拼装（记忆块以独立 system 消息紧跟主
/// system prompt），经 run_model_loop_core 打到 mock LLM。
struct RegressionChat {
    server: MockLlmServer,
    conn: rusqlite::Connection,
    history: Vec<(String, String)>,
    _dir: tempfile::TempDir,
}

impl RegressionChat {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("wm.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE bot_facts (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL);",
        )
        .unwrap();
        ensure_bot_facts_memory_columns(&conn).unwrap();
        Self {
            server: MockLlmServer::start(),
            conn,
            history: Vec::new(),
            _dir: dir,
        }
    }

    /// 跑一轮对话：注入记忆块 → 拼装消息 → mock LLM 工具循环 → 历史落账
    async fn run_turn(&mut self, h: &Harness, user_text: &str) -> String {
        // 与 bot_chat 同一取数/拼装内核；失败在测试里直接暴露（生产路径是静默降级）
        let inj = memory_injection_snapshot(&self.conn, user_text, now_ms()).expect("注入取数");
        let mut msgs = vec![serde_json::json!({"role": "system", "content": "主提示词"})];
        if let Some(block) = format_memory_block(&inj) {
            msgs.push(serde_json::json!({"role": "system", "content": block}));
        }
        for (r, c) in &self.history {
            msgs.push(serde_json::json!({"role": r, "content": c}));
        }
        msgs.push(serde_json::json!({"role": "user", "content": user_text}));

        let conn = &self.conn;
        let exec = move |name: String, args: String| async move {
            assert_eq!(name, "remember_fact", "本回归基准只产生 remember_fact 工具调用");
            (exec_remember(conn, &args), Vec::<TaskRef>::new())
        };
        let (text, _) = run_model_loop_core(
            &core_http(&self.server),
            msgs,
            10,
            &h.stop,
            None,
            &h.deps(),
            exec,
            noop_replan,
        )
        .await
        .expect("对话轮应成功");
        self.history.push(("user".into(), user_text.into()));
        self.history.push(("assistant".into(), text.clone()));
        text
    }

    fn bodies(&self) -> Vec<String> {
        self.server.request_bodies()
    }
}

/// 从请求 body 原文里取记忆块（## 记忆开头的 system 消息）content；无则 None
fn memory_block_in(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).expect("请求体应为 JSON");
    v["messages"].as_array()?.iter().find_map(|m| {
        let content = m["content"].as_str()?;
        (m["role"].as_str() == Some("system") && content.starts_with("## 记忆"))
            .then(|| content.to_string())
    })
}

// ────────────────────────────────────────────────────────────────────
// 场景 1：跨轮偏好召回（告知偏好 → 闲聊 → 提问验证召回）
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn scenario_1_preference_recalled_across_chitchat() {
    let mut chat = RegressionChat::new();
    let h = Harness::new();

    // 第 1 轮：用户告知偏好 → mock LLM 返回 remember_fact 工具调用 → 最终文本
    chat.server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "remember_fact".into(),
        arguments: r#"{"key":"口味","value":"不吃辣","category":"preference","importance":5}"#.into(),
    }));
    chat.server.push_behavior(MockBehavior::TextReply("已记住您不吃辣".into()));
    chat.run_turn(&h, "记住：我不吃辣").await;

    // 库终态：category/importance 随写入落库
    let (value, cat, imp): (String, String, i64) = chat
        .conn
        .query_row(
            "SELECT value, category, importance FROM bot_facts WHERE key = '口味'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((value.as_str(), cat.as_str(), imp), ("不吃辣", "preference", 5));
    // 第 1 轮第 2 次请求（工具结果回填轮）应带 remember_fact 的执行结果
    assert!(
        chat.bodies()[1].contains("已记住「口味」"),
        "工具结果应回填给 LLM：{}",
        chat.bodies()[1]
    );

    // 中间 3 轮闲聊（与偏好零关键词重叠 → 检索零命中）
    for text in ["今天天气不错", "随便聊聊", "嗯嗯"] {
        chat.server.push_behavior(MockBehavior::TextReply("好的".into()));
        chat.run_turn(&h, text).await;
    }
    // 闲聊轮：检索零命中不出「相关记忆」段；importance=5 → pinned 画像段常驻
    let mid_block = memory_block_in(&chat.bodies()[2]).expect("闲聊轮请求应有记忆块（pinned）");
    assert!(mid_block.contains("口味：不吃辣"), "importance=5 无条件注入");
    assert!(!mid_block.contains("### 相关记忆"), "闲聊零命中不应出相关记忆段：{mid_block}");

    // 第 5 轮：提问（与「口味/不吃辣」仍无关键词重叠，纯靠 pinned 召回）
    chat.server.push_behavior(MockBehavior::TextReply("推荐清蒸鱼".into()));
    let reply = chat.run_turn(&h, "晚餐推荐什么").await;
    assert_eq!(reply, "推荐清蒸鱼");
    let last_block = memory_block_in(chat.bodies().last().unwrap()).expect("末轮请求应带记忆块");
    assert!(last_block.contains("### 用户画像与偏好"));
    assert!(
        last_block.contains("口味：不吃辣"),
        "跨 4 轮后偏好仍应在记忆块中：{last_block}"
    );
}

// ────────────────────────────────────────────────────────────────────
// 场景 2：改口覆盖而非堆积（同 key 重写 → 库里只有新值 → 注入带新不带旧）
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn scenario_2_correction_overwrites_not_accumulates() {
    let mut chat = RegressionChat::new();
    let h = Harness::new();

    for value in ["上海", "北京"] {
        chat.server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
            name: "remember_fact".into(),
            arguments: format!(r#"{{"key":"城市","value":"{value}"}}"#),
        }));
        chat.server.push_behavior(MockBehavior::TextReply("好的".into()));
    }
    chat.run_turn(&h, "我在上海").await;
    chat.run_turn(&h, "改一下，我现在在北京").await;

    // 库终态：同 key 只有一条且是新值
    let count: i64 = chat
        .conn
        .query_row("SELECT COUNT(*) FROM bot_facts WHERE key = '城市'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1, "同 key 覆盖不堆积");
    let value: String = chat
        .conn
        .query_row("SELECT value FROM bot_facts WHERE key = '城市'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(value, "北京");

    // 第 3 轮：检索命中「城市」→ 记忆块带北京不带上海
    chat.server.push_behavior(MockBehavior::TextReply("好的".into()));
    chat.run_turn(&h, "我所在城市的事").await;
    let block = memory_block_in(chat.bodies().last().unwrap()).expect("应有记忆块");
    assert!(block.contains("城市：北京"), "应注入新值：{block}");
    assert!(!block.contains("上海"), "旧值不应再出现：{block}");
}

// ────────────────────────────────────────────────────────────────────
// 场景 3：冲突提示回传（相近措辞不同 key 写入 → 工具结果带相似已有记忆提示）
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn scenario_3_conflict_hint_returned_to_llm() {
    let mut chat = RegressionChat::new();
    let h = Harness::new();

    chat.server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "remember_fact".into(),
        arguments: r#"{"key":"城市","value":"上海"}"#.into(),
    }));
    chat.server.push_behavior(MockBehavior::TextReply("记下了".into()));
    chat.run_turn(&h, "我在上海").await;

    // 相近措辞、不同 key 的写入 → 冲突提示
    chat.server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "remember_fact".into(),
        arguments: r#"{"key":"居住地","value":"上海浦东"}"#.into(),
    }));
    chat.server.push_behavior(MockBehavior::TextReply("已覆盖旧记忆".into()));
    chat.run_turn(&h, "我住在上海浦东").await;

    // 第 2 轮第 2 次请求（工具结果回填轮）应把冲突提示带给 LLM
    let tool_round = &chat.bodies()[3];
    assert!(tool_round.contains("相似已有记忆"), "应回传冲突提示：{tool_round}");
    assert!(
        tool_round.contains("key=城市, value=上海"),
        "提示应带已有 key/value 明细：{tool_round}"
    );
    assert!(tool_round.contains("如需更新请用同 key 覆盖"), "提示应引导覆盖：{tool_round}");
    // mock 选择仍写新 key → 两条都在（覆盖/保留的决策权在模型，提示只是信息）
    let count: i64 = chat
        .conn
        .query_row("SELECT COUNT(*) FROM bot_facts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

// ────────────────────────────────────────────────────────────────────
// 场景 4：推断标记（model_inferred 带 [推断] 前缀，user_stated 不带）
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn scenario_4_inferred_memories_carry_prefix() {
    let mut chat = RegressionChat::new();
    let h = Harness::new();

    // 一轮两个工具调用：一条模型推断、一条用户口述
    chat.server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "remember_fact".into(),
        arguments: r#"{"key":"使用平台","value":"macOS 开发","source":"model_inferred"}"#.into(),
    }));
    chat.server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "remember_fact".into(),
        arguments: r#"{"key":"偏好语言","value":"中文","source":"user_stated"}"#.into(),
    }));
    chat.server.push_behavior(MockBehavior::TextReply("都记下了".into()));
    chat.run_turn(&h, "记两条").await;

    // 下一轮提问同时命中两条
    chat.server.push_behavior(MockBehavior::TextReply("好的".into()));
    chat.run_turn(&h, "我的使用平台和偏好语言分别是什么").await;
    let block = memory_block_in(chat.bodies().last().unwrap()).expect("应有记忆块");
    let inferred_line = block.lines().find(|l| l.contains("使用平台")).expect("推断条目应注入");
    assert!(inferred_line.contains("[推断]"), "model_inferred 应带 [推断] 前缀：{inferred_line}");
    let stated_line = block.lines().find(|l| l.contains("偏好语言")).expect("口述条目应注入");
    assert!(!stated_line.contains("[推断]"), "user_stated 不应带前缀：{stated_line}");
}

// ────────────────────────────────────────────────────────────────────
// 场景 5：遗忘（空 value 删除 → 后续记忆块不再出现）
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn scenario_5_empty_value_forgets() {
    let mut chat = RegressionChat::new();
    let h = Harness::new();

    chat.server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "remember_fact".into(),
        arguments: r#"{"key":"爱好","value":"摄影"}"#.into(),
    }));
    chat.server.push_behavior(MockBehavior::TextReply("记下了".into()));
    chat.run_turn(&h, "我爱好摄影").await;

    // 删除：空 value
    chat.server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "remember_fact".into(),
        arguments: r#"{"key":"爱好","value":""}"#.into(),
    }));
    chat.server.push_behavior(MockBehavior::TextReply("已忘掉".into()));
    chat.run_turn(&h, "忘掉我的爱好").await;
    assert!(
        chat.bodies()[3].contains("已删除记忆「爱好」"),
        "删除结果应回填给 LLM：{}",
        chat.bodies()[3]
    );
    let count: i64 = chat
        .conn
        .query_row("SELECT COUNT(*) FROM bot_facts WHERE key = '爱好'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0, "库终态：条目已删");

    // 后续轮：唯一的记忆已删 → 无记忆块（全空 → None）；历史里用户自己提过「摄影」
    // 属正常回显，断言口径是 system 消息序列（注入侧）不再带该条目
    chat.server.push_behavior(MockBehavior::TextReply("我不知道了".into()));
    chat.run_turn(&h, "我的爱好是什么").await;
    let last = chat.bodies().last().unwrap().clone();
    assert!(memory_block_in(&last).is_none(), "全空库应无记忆块：{last}");
    let v: serde_json::Value = serde_json::from_str(&last).unwrap();
    let leaked = v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"].as_str() == Some("system"))
        .any(|m| m["content"].as_str().unwrap_or("").contains("摄影"));
    assert!(!leaked, "已遗忘条目不应出现在任何 system 消息里");
}

// ────────────────────────────────────────────────────────────────────
// 场景 6：零命中回退（检索全 0 分 → 近期摘要兜底；全空库 → None，不 panic 不报错）
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn scenario_6_zero_hit_fallback_and_empty_library_ok() {
    // A：库里有 summary 但本轮闲聊零命中 → 记忆块仍有「近期摘要」兜底段，无「相关记忆」
    let mut chat = RegressionChat::new();
    let h = Harness::new();
    let now = now_ms();
    for i in 0..3 {
        memory_insert(
            &chat.conn,
            &format!("summary:sA:{i}"),
            &format!("第{i}段：讨论了记忆模块选型"),
            "summary",
            2,
            now - 1000 + i,
        )
        .unwrap();
    }
    chat.server.push_behavior(MockBehavior::TextReply("哈哈".into()));
    chat.run_turn(&h, "哈哈今天真开心").await;
    let block = memory_block_in(chat.bodies().last().unwrap()).expect("零命中也应有兜底记忆块");
    assert!(block.contains("### 近期摘要"), "回退兜底段应在：{block}");
    assert!(block.contains("讨论了记忆模块选型"), "兜底应带最近摘要内容：{block}");
    assert!(!block.contains("### 相关记忆"), "零命中不出相关记忆段：{block}");

    // B：全空库 → 记忆块 None（请求里只有主 system 一条），对话照常完成
    let mut empty_chat = RegressionChat::new();
    empty_chat.server.push_behavior(MockBehavior::TextReply("你好".into()));
    let reply = empty_chat.run_turn(&h, "你好").await;
    assert_eq!(reply, "你好");
    let body = empty_chat.bodies().last().unwrap().clone();
    assert!(memory_block_in(&body).is_none(), "全空库不应注入记忆块");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let system_count = v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"].as_str() == Some("system"))
        .count();
    assert_eq!(system_count, 1, "只剩主 system prompt 一条");
}

// ────────────────────────────────────────────────────────────────────
// 场景 7：跨会话摘要召回（会话 A 截断产摘要落库 → 会话 B 命中关键词召回）
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn scenario_7_cross_session_summary_recall() {
    let mut chat = RegressionChat::new();
    let h = Harness::new();

    // 会话 A：超预算历史触发截断即摘要（生产同一编排内核 truncate_with_summary_core
    // + summarize_http 直连 mock 非流式 JsonReply），摘要落库 kind=summary
    chat.server.push_behavior(MockBehavior::JsonReply("用户在做记忆模块设计".into()));
    let long = "x".repeat(60);
    let session_a = vec![
        ChatMsg { role: "user".into(), content: long.clone() },
        ChatMsg { role: "assistant".into(), content: long },
        ChatMsg { role: "user".into(), content: "本轮问题".into() },
    ];
    let client = reqwest::Client::new();
    let base_url = chat.server.base_url.clone();
    let (_kept, summary, dropped) = truncate_with_summary_core(session_a, 63, move |dropped_msgs| {
        let client = client.clone();
        let base_url = base_url.clone();
        async move {
            summarize_http(&client, &base_url, "test-key", "mock-model", "总结", &dropped_msgs, ApiProvider::Openai, DEFAULT_MAX_TOKENS).await
        }
    })
    .await;
    assert_eq!(dropped, 2, "会话 A 最旧两条被截断");
    let summary = summary.expect("截断应产出摘要");
    let now = now_ms();
    memory_insert(&chat.conn, &format!("summary:sessA:{now}"), &summary, "summary", 2, now).unwrap();

    // 会话 B：全新历史（同一记忆表，与生产一致——bot_facts 跨会话共享），
    // 用户消息含摘要关键词 → 检索命中该摘要进「相关记忆」段
    chat.server.push_behavior(MockBehavior::TextReply("进展顺利".into()));
    chat.run_turn(&h, "记忆模块进展如何").await;
    let block = memory_block_in(chat.bodies().last().unwrap()).expect("会话 B 请求应带记忆块");
    assert!(block.contains("### 相关记忆"), "摘要应以检索命中进相关记忆段：{block}");
    assert!(block.contains("用户在做记忆模块设计"), "跨会话摘要应被召回：{block}");
    // 命中即访问强化
    let ac: i64 = chat
        .conn
        .query_row(
            &format!("SELECT access_count FROM bot_facts WHERE key = 'summary:sessA:{now}'"),
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ac, 1, "摘要命中应刷新 access_count");
}
