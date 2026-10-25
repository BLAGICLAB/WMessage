//! Mock LLM + reqwest 端到端
//!
//! 把 `tests/mock_llm.rs` 的 server 接到 reqwest::Client，
//! 验证 SSE 响应能被生产同构的解析逻辑消费。
//!
//! 测试改用 `wmessage_lib::bot::parse_sse_chunk`，
//! 消除与 `bot::run_model_loop` 的解析逻辑重复。OpenAI SSE 格式演化只改 bot.rs 一处。
//!
//! 设计要点：
//! - 用真实的 reqwest::Client（与 bot.rs 共享依赖）
//! - SSE 行级解析委托给 `wmessage_lib::bot::parse_sse_chunk`（生产同构）
//! - 不依赖 Tauri runtime（macOS EventLoop 主线程限制）
//! - bytes-level 简化：reqwest stream API 在 tokio::test 多线程下 Unpin cast 复杂，
//!   全量读 bytes 后按行调 parse_sse_chunk（生产代码按 \n 行切 SSE，
//!   见 bot_model_loop.rs 的 drain_sse_lines）

//
// 集成测试 crate 的根：共享夹具 + harness + 各主题子模块。
// 拆自原 tests/llm_integration.rs（4645 行单文件）：共享辅助项留在根，
// 各主题的测试函数进子模块并 `use super::*` 取用——根的私有项对子模块可见，
// 于是夹具一行可见性都不用改。

mod mock_llm_re_export {
    include!("../mock_llm.rs");
}

use mock_llm_re_export::{MockBehavior, MockLlmServer, ToolCallResponse};
use wmessage_lib::bot::registry::ToolResult;
use wmessage_lib::bot::{
    accumulate_tool_call_delta, drain_sse_lines, parse_sse_chunk, ToolCallDelta,
};

/// 中间件 API 的 AppHandle 首参占位（签名扩展后测试适配）。
/// 这些用例只命中闸门判定，不触发 audit 落盘路径；App 泄漏给测试进程，退出即回收。
fn mock_handle() -> tauri::AppHandle<tauri::test::MockRuntime> {
    Box::leak(Box::new(tauri::test::mock_app()))
        .handle()
        .clone()
}

// ────────────────────────────────────────────────────────────────────
// SSE 解析：F-6 step 4 refactor 后改用 wmessage_lib::bot::parse_sse_chunk
// 累计多行 SSE chunk 为 ParsedStream（text + tool_calls + finish_reason）
// ────────────────────────────────────────────────────────────────────

#[derive(Default, Debug)]
pub struct ParsedStream {
    pub text: String,
    /// (id, name, arguments)——生产同一累积函数 `accumulate_tool_call_delta` 的输出形态
    /// （批次3审计 T-1：消除与生产累积层的双份实现漂移）  audit-ok
    pub tool_calls: Vec<(String, String, String)>,
    pub finish_reason: Option<String>,
    /// 200 流内错误载荷（批次3审计 P1-3：OneAPI 类网关在 200 流内发 {"error":...}）  audit-ok
    pub error: Option<String>,
}

/// 解析 SSE bytes（内部按行调 `wmessage_lib::bot::parse_sse_chunk`）
pub fn parse_sse_bytes(bytes: &[u8]) -> ParsedStream {
    let text = String::from_utf8_lossy(bytes);
    let mut out = ParsedStream::default();
    for line in text.lines() {
        let Some(parsed) = parse_sse_chunk(line) else {
            continue;
        };
        if parsed.is_done {
            continue;
        }
        if let Some(c) = parsed.content {
            out.text.push_str(&c);
        }
        // 批次3审计 T-1：用生产同一累积函数按 index 归位（不再 extend 平铺）  audit-ok
        for d in parsed.tool_calls {
            accumulate_tool_call_delta(&mut out.tool_calls, &d);
        }
        if parsed.error.is_some() {
            out.error = parsed.error;
        }
        if let Some(reason) = parsed.finish_reason {
            out.finish_reason = Some(reason);
        }
    }
    out
}

/// reqwest POST body helper
fn make_body(user_msg: &str) -> serde_json::Value {
    serde_json::json!({
        "model": "deepseek-chat",
        "messages": [{"role": "user", "content": user_msg}],
        "stream": true,
    })
}

// ────────────────────────────────────────────────────────────────────
// run_model_loop_core 真路径集成测试
//
// 背景：P0-1/P0-5 等修复密集区（HTTP 错误包装 / 流截断防线 / 重试退避）  audit-ok
// 原先沉在 run_model_loop 函数体内、直连 AppHandle，无法集成测试。
// 重构后核心 loop 可注入 LlmHttp + 副作用出口（ModelLoopDeps），
// 以下用例全部对 mock LLM server 跑生产同一份代码。
// ────────────────────────────────────────────────────────────────────

use std::sync::{Arc, Mutex};
use wmessage_lib::bot::{
    noop_replan, run_model_loop_core, ApiProvider, AuditLevel, LlmHttp, ModelLoopDeps, StopGuard,
    ToolCallTrace, DEFAULT_MAX_TOKENS,
};
use wmessage_lib::db::trace::SessionTools;
use wmessage_lib::error::CommandError;

/// 指向 mock server 的连接参数（与生产薄壳同一构造路径：trim_end_matches('/') 等
/// 在 core 内处理，这里直接给 base_url）
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
        // RE-1：mock 测试不发推理字段（None = 不注入，既有断言不受影响）
        reasoning: wmessage_lib::bot::reasoning::ReasoningWire::None,
        // 条目级采样参数 / system prompt（U13）：既有用例不发不追加
        temperature: None,
        top_p: None,
        system_prompt: None,
    }
}

/// Anthropic 协议变体：同一 mock server，
/// 主循环应打 /v1/messages 并用 x-api-key 鉴权
fn core_http_anthropic(server: &MockLlmServer) -> LlmHttp {
    LlmHttp {
        provider: ApiProvider::Anthropic,
        ..core_http(server)
    }
}

/// 收集结构化审计事件名
type AuditEvents = Arc<Mutex<Vec<&'static str>>>;

struct CoreHarness {
    stop: StopGuard,
    emit: Box<dyn Fn(&str, serde_json::Value) + Send + Sync>,
    audit: Box<dyn Fn(AuditLevel, &'static str, Vec<(&'static str, String)>) + Send + Sync>,
    skill_finish: Box<dyn Fn(bool, &str) -> String + Send + Sync>,
    /// 阶段 3.3：核心只读「活动技能快照」（等价 `bot_skills::active_skill_run_for`）
    active_skill_run:
        Box<dyn Fn(Option<&str>) -> Option<wmessage_lib::bot_skills::SkillRun> + Send + Sync>,
    /// 本会话调用过的工具名（调用过 / 成功过两档，幻觉守卫「点名判据」的排除集）。
    /// None = 查不到 → 点名相关的判据整档让开；要验「跨提问的重述是真话、不被拦」
    /// 用 `set_session_tools`。
    session_tools: Box<dyn Fn(Option<&str>) -> Option<SessionTools> + Send + Sync>,
    /// `session_tools` 闭包背后的数据，供测试写入
    session_tools_data: Arc<Mutex<Option<SessionTools>>>,
    /// 执行类谎报守卫的触发词表。默认填**内置默认表**（与生产一致），
    /// 否则依赖守卫的既有用例会因空词表静默失效；要验自定义表改这个字段。
    guard_claim_keywords: Vec<String>,
    /// 落盘凭证注入点：列出的路径判成「查不到凭证」（幻觉守卫的「这个路径是
    /// 工具产物吗」那道判据）。空表 = 全部有凭证 = 放行。
    file_receipts_missing: Arc<Mutex<Vec<String>>>,
    /// 落盘凭证闭包（从 `file_receipts_missing` 读）。存进 harness 而不是每次
    /// 现造——`deps()` 要的是 `&'a dyn Fn`，现造就得 `Box::leak` 逃逸，
    /// 多轮用例下泄漏随调用次数增长。
    file_receipts: Box<dyn Fn(Option<&str>, &[String]) -> Vec<String> + Send + Sync>,
    /// 思考段回灌开关（默认关）。开启后 assistant(tool_calls) 消息带上本轮
    /// reasoning_content 原文——要验「开/关两态的消息形状」用。
    reasoning_echo: bool,
    audit_events: AuditEvents,
}

impl CoreHarness {
    fn new() -> Self {
        Self::with_session(None)
    }

    /// 会话归属版：子 agent 会话的预算闸与熔断上限都从 stop.session_id()
    /// 派生（tool_guard 全局注册表），None = 主会话默认。
    fn with_session(session_id: Option<&str>) -> Self {
        let events: AuditEvents = Arc::new(Mutex::new(Vec::new()));
        let events2 = events.clone();
        let sub_tools: Arc<Mutex<Option<SessionTools>>> =
            Arc::new(Mutex::new(Some(SessionTools::default())));
        let sub_tools2 = sub_tools.clone();
        let missing: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let missing2 = missing.clone();
        Self {
            // 非交互实例：不发流式事件，stop 标志未置位
            stop: StopGuard::new(&mock_handle(), false, session_id.map(str::to_string)),
            emit: Box::new(|_, _| {}),
            audit: Box::new(move |_, event, _| {
                events2.lock().unwrap().push(event);
            }),
            skill_finish: Box::new(|_, _| String::new()),
            // 用真 accessor（走兜底实例，与生产语义一致）；本组用例不插 run → 恒 None。
            // handle 构造时捕获一次：闭包里每次调 mock_handle() 会 Box::leak 一个新
            // mock_app，多轮用例下泄漏随调用次数增长；兜底注册表是全局 OnceLock，
            // 复用同一 handle 语义不变
            active_skill_run: {
                let h = mock_handle();
                Box::new(move |sid| wmessage_lib::bot_skills::active_skill_run_for(&h, sid))
            },
            session_tools: Box::new(move |_| sub_tools2.lock().unwrap().clone()),
            session_tools_data: sub_tools,
            guard_claim_keywords: wmessage_lib::bot::registry::default_execution_claim_patterns(),
            file_receipts_missing: missing,
            file_receipts: Box::new(move |_sid, paths: &[String]| {
                let missing = missing2.lock().unwrap();
                paths
                    .iter()
                    .filter(|p| missing.iter().any(|m| m.eq_ignore_ascii_case(p)))
                    .cloned()
                    .collect()
            }),
            audit_events: events,
            reasoning_echo: false,
        }
    }

    /// opt-in 思考段回灌：默认关（维持现状），要验开态的消息形状用这个。
    fn with_reasoning_echo(mut self) -> Self {
        self.reasoning_echo = true;
        self
    }

    /// 依赖装配（默认关 final 自检，避免对历史用例产生回归干扰；
    /// 新自检测试用 deps_with_gap(opt) opt-in）
    fn deps(&self) -> ModelLoopDeps<'_> {
        // 默认关 final 自检（避免对历史用例产生回归干扰）；新自检测试用 deps_with_gap(opt) opt-in
        ModelLoopDeps {
            fuse_cap_override: None,
            conv_sidecar_dir: None,
            reasoning_echo: self.reasoning_echo,
            emit: &*self.emit,
            audit: &*self.audit,
            skill_finish: &*self.skill_finish,
            active_skill_run: &*self.active_skill_run,
            final_selfcheck_round_gap: Some(0),
            final_selfcheck_http: None,
            on_round: &|rounds| {
                let _ = rounds;
            },
            turn_status: None,
            wall_budget: None,
            session_tools: Some(&*self.session_tools),
            guard_claim_keywords: &self.guard_claim_keywords,
            file_receipts: Some(&*self.file_receipts),
        }
    }

    /// opt-in final 自检：gap 传入 None 走默认 8 轮；Some(N) 走 N；Some(0) 关闭
    fn deps_with_gap(&self, gap: Option<u32>) -> ModelLoopDeps<'_> {
        ModelLoopDeps {
            fuse_cap_override: None,
            conv_sidecar_dir: None,
            reasoning_echo: self.reasoning_echo,
            emit: &*self.emit,
            audit: &*self.audit,
            skill_finish: &*self.skill_finish,
            active_skill_run: &*self.active_skill_run,
            final_selfcheck_round_gap: gap,
            final_selfcheck_http: None,
            on_round: &|rounds| {
                let _ = rounds;
            },
            turn_status: None,
            wall_budget: None,
            session_tools: Some(&*self.session_tools),
            guard_claim_keywords: &self.guard_claim_keywords,
            file_receipts: Some(&*self.file_receipts),
        }
    }

    /// 侧调终稿自检：gap=1（首次满足即触发），自检请求打到独立的
    /// selfcheck server（非流式 JSON，同 summarize_http 路径）
    fn deps_with_selfcheck<'a>(&'a self, selfcheck: &'a LlmHttp) -> ModelLoopDeps<'a> {
        let mut deps = self.deps_with_gap(Some(1));
        deps.final_selfcheck_http = Some(selfcheck);
        deps
    }

    fn events(&self) -> Vec<&'static str> {
        self.audit_events.lock().unwrap().clone()
    }

    /// 声明本会话调用过且成功过这些工具（点名判据的排除集）
    fn set_session_tools(&self, names: &[&str]) {
        let mut g = self.session_tools_data.lock().unwrap();
        *g = Some(SessionTools {
            called: names.iter().map(|s| s.to_string()).collect(),
            succeeded: names.iter().map(|s| s.to_string()).collect(),
        });
    }
}

fn user_msgs() -> Vec<serde_json::Value> {
    vec![serde_json::json!({"role": "user", "content": "hi"})]
}

/// 工具执行 stub 永不调用版（纯文本/错误路径不应触发任何工具执行）
async fn exec_never(name: String, args: String, _trace: ToolCallTrace) -> ToolResult {
    panic!("此路径不应执行工具：{name} {args}");
}

use wmessage_lib::bot_chat::{summarize_http, ChatMsg};

fn chat_msg(role: &str, content: &str) -> ChatMsg {
    ChatMsg {
        role: role.into(),
        content: content.into(),
    }
}

// ───────────── U15 记忆可控开关：双门禁真路径（mock app 数据目录） ─────────────
//
// ⚠️ db::data_dir 在 cargo test 下解析到 target/debug/deps/——该目录的
// bot-config.json 与 wmessage.db 现按**测试进程**隔离（paths.rs「测试期隔离」
// 一段；nextest 是进程级并行，共用一个库会互踩写锁——本次实证为偶发失败）。
// 本测试原先的「共享库历史脏数据」顾虑随之消失，但断言口径保持增量不放松：
// 进程内仍可能残留上轮同进程用例写入的行。因此：
// - 三个门禁场景合并为一个测试（nextest 下即单进程，配置写读全程本进程有序）；
// - DB 断言一律增量口径（共享库里有历史条目，不做绝对断言）；
// - 结束时恢复共享现场（删本测试写的配置文件 + 清理种子条目与历史遗留）。

fn write_config_with_memory_control(
    handle: &tauri::AppHandle<tauri::test::MockRuntime>,
    injection: bool,
    auto_write: bool,
) {
    let cfg = serde_json::json!({
        "baseUrl": "http://127.0.0.1:1",
        "model": "mock-model",
        "memoryControl": {
            "injectionEnabled": injection,
            "autoWriteEnabled": auto_write,
        },
    });
    std::fs::write(
        wmessage_lib::bot::config_path(handle),
        serde_json::to_string(&cfg).unwrap(),
    )
    .unwrap();
}

fn seed_memory(handle: &tauri::AppHandle<tauri::test::MockRuntime>, content: &str) {
    // insert_item 内部已 ensure_table，无需前置
    let conn = wmessage_lib::db::open_db(handle).unwrap();
    let item = wmessage_lib::memory::store::NewItem {
        kind: "fact".into(),
        content: content.into(),
        tags: vec![],
        importance: 3,
        source: "user_stated".into(),
    };
    // 1_000 = 固定占位时间戳（created/updated_at；检索断言只看内容不看排序）
    match wmessage_lib::memory::store::insert_item(&conn, &item, None, 1_000).unwrap() {
        (wmessage_lib::memory::store::InsertOutcome::Inserted(_), _) => {}
        other => panic!("种记忆应直接插入：{other:?}"),
    }
}

const SEED_CONTENT_PLACEHOLDER: &str = "测试记忆内容甲乙丙丁";

/// 清共享现场：本测试的配置文件、种子条目（含历史轮次残留）、漏写的「测试键」。
/// 清理失败只 eprintln 不 panic——共享目录的清理是尽力而为，失败信息留诊断。
fn clean_gate_leftovers(app: &tauri::AppHandle<tauri::test::MockRuntime>) {
    if let Err(e) = std::fs::remove_file(wmessage_lib::bot::config_path(app)) {
        if e.kind() != std::io::ErrorKind::NotFound {
            eprintln!("[memory_gate] 清理共享 bot-config.json 失败：{e}");
        }
    }
    let conn = wmessage_lib::db::open_db(app).unwrap();
    // mem_items 建表：数据目录已按进程隔离（paths.rs），本进程的新库里这张表
    // 还不存在——过去是蹭共享库里别的进程建好的表，换来的是跨进程依赖
    wmessage_lib::memory::store::ensure_table(&conn).unwrap();
    let junk: Vec<String> = wmessage_lib::memory::store::load_all(&conn)
        .unwrap()
        .into_iter()
        .filter(|m| {
            m.content == SEED_CONTENT_PLACEHOLDER
                || m.tags.first().map(|t| t.as_str()) == Some("测试键")
        })
        .map(|m| m.id)
        .collect();
    if let Err(e) = wmessage_lib::memory::store::delete_by_ids(&conn, &junk) {
        eprintln!("[memory_gate] 清理共享库条目失败：{e}");
    }
}

// ───────────── T1-QUERYTASKS：query_tasks / edit_task / subtaskId 端到端（两段真路径） ─────────────
//
// A. 模型循环接线：mock LLM SSE 发 query_tasks / edit_task tool_call → 真 run_model_loop_core
//    → 注入 executor 收到 (name, args) 原文 → 结果以 role:tool 回填续聊——schema 命名/参数
//    序列化经真模型循环的全链路（真 dispatch 的 AppHandle<Wry> 链路在 mock runtime 下不可调，
//    见 task_chat_exec.rs:156 既有结论，故 executor 注入——skill_e2e 同款接缝）。
// B. DB 往返：db_upsert/db_load 真命令（真写锁 + 真 SQL）验证工具层读写的全部新列
//    （model/owner_id/created_at/schedule/depends_on/subtasks）在 SQLite 的落库与读出，
//    以及 edit_task 的 RMW 写回模式（expected_updated_at 基线 → upsert）。
//
// ⚠️ 共享库（target/debug/deps/wmessage.db，见 U15 地雷注记）：断言一律键位口径
//（唯一 UUID 标题 / 唯一卡 id），开场扫残留、收尾清种子，panic 由下次开场清理兜底。

const T1_TITLE_ACTIVE: &str = "T1E2E未完成卡-量子调试甲乙";
const T1_OWNER_ID: &str = "t1e2e-owner-0001";

fn t1_seed_task(
    id: &str,
    title: &str,
    column: wmessage_lib::db::TaskStatus,
) -> wmessage_lib::db::Task {
    wmessage_lib::db::Task {
        plan_start: None,
        plan_end: None,
        parent_id: None,
        id: id.into(),
        title: title.into(),
        due: None,
        note: None,
        tags: None,
        files: None,
        column,
        subtasks: None,
        completed_at: None,
        created_at: Some(1_000),
        archived: None,
        deleted_at: None,
        collapsed: None,
        order: None,
        updated_at: Some(1_000),
        bot_assigned: None,
        assignee: None,
        budget: None,
        result: None,
        depends_on: None,
        model: None,
        owner_id: None,
        expected_updated_at: None,
    }
}

fn t1_cleanup(app: &tauri::AppHandle<tauri::test::MockRuntime>) {
    let conn = wmessage_lib::db::open_db(app).unwrap();
    for id in ["t1e2e-active", "t1e2e-done"] {
        if let Err(e) = conn.execute("DELETE FROM tasks WHERE id = ?1", [id]) {
            eprintln!("[t1e2e] 清理任务 {id} 失败：{e}");
        }
    }
    // 定时单源清理：迁移产物 job 行 + 执行历史（t1e2e-active 种 schedule 会触发迁移）
    if let Err(e) = conn.execute(
        "DELETE FROM scheduled_jobs WHERE id = ?1",
        ["job-t1e2e-active"],
    ) {
        eprintln!("[t1e2e] 清理定时作业失败：{e}");
    }
    if let Err(e) = conn.execute(
        "DELETE FROM scheduled_job_runs WHERE job_id = ?1",
        ["job-t1e2e-active"],
    ) {
        eprintln!("[t1e2e] 清理定时作业历史失败：{e}");
    }
    if let Err(e) = conn.execute("DELETE FROM people WHERE id = ?1", [T1_OWNER_ID]) {
        eprintln!("[t1e2e] 清理成员失败：{e}");
    }
}

// ────────────────────────────────────────────────────────────────────
// 工具语义自检（SELFCHECK）回复接线：纯 Y/N 不是给用户的答案。
// 修复前两处断裂：注入点 take() 提前消费 pending → 回复轮解析永远扑空
// （N 换策略路径死代码）；纯 Y/N 无工具调用走主出口直接 return → 回合被
// 自检终结（Y 顶掉正常汇报 / N 的「换策略中」后无任何动作）。
// 修复后：Y → 续跑让模型产出正常汇报；N → 注入换策略提示后续跑；
// 附解释 → 按正常答案终结回合。
// ────────────────────────────────────────────────────────────────────

/// 捕获 bot-chat-delta 事件的 deps（emit 默认 noop，视觉提示断言需要捕获版）
fn deps_capturing_chat_deltas(h: &CoreHarness) -> (ModelLoopDeps<'_>, Arc<Mutex<Vec<String>>>) {
    let deltas: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deltas2 = deltas.clone();
    let mut deps = h.deps();
    deps.emit = Box::leak(Box::new(move |event: &str, v: serde_json::Value| {
        if event == "bot-chat-delta" {
            if let Some(t) = v.get("text").and_then(|t| t.as_str()) {
                deltas2.lock().unwrap().push(t.to_string());
            }
        }
    }));
    (deps, deltas)
}

// ────────────────────────────────────────────────────────────────────
// 终稿自检侧调（final_selfcheck_http）：自检走独立小请求（非流式 JSON，
// summarize_http 同路径），主循环不多跑一轮——主服务器请求数 = 3 tool +
// 1 终文，判定打到独立的 side server。
// ────────────────────────────────────────────────────────────────────

/// 侧调小模型连接：复用 core_http 的连接参数（鉴权走同一 mock 口径），
/// 只换端点与模型名
fn selfcheck_http(server: &MockLlmServer) -> LlmHttp {
    LlmHttp {
        client: reqwest::Client::new(),
        base_url: server.base_url.clone(),
        model: "mock-small".into(),
        ..core_http(server)
    }
}

const SIDE_LONG_TEXT: &str = "已完成 3 项工作：列出了所有任务、统计了完成情况、生成了汇总报告。任务列表展示在文档中，每一条都核对过状态与期限，没有遗漏任何待办事项，可以放心进入下一阶段的安排，若有需要我可以继续细化每个条目。";

// ────────────────────────────────────────────────────────────────────────────
// 执行类谎报守卫：整场零工具调用 + 终稿叙述执行结果 → 打回一轮。
//
// 事故原话（session 81310e3a message 690，对应 trace turns=0 tools=0 ms=1866）：
// 用户点名 run_python 要它 sleep+打印，模型一次工具都没调，却报「执行完毕，
// sleep 120 秒后工具真实返回，打印了 done」。此前的守卫只有 mutating 一档，
// 非 mutating 工具（run_python / fetch_url / bash）的执行结果可以凭空叙述。
// ────────────────────────────────────────────────────────────────────────────

/// 捕获结构化审计的 deps（event + kv 拼一行），用于断言守卫记了什么审计
fn deps_capturing_logs<'a>(
    h: &'a CoreHarness,
    logs: &'a Arc<Mutex<Vec<String>>>,
) -> ModelLoopDeps<'a> {
    let mut deps = h.deps();
    deps.audit = {
        let logs2 = logs.clone();
        Box::leak(Box::new(
            move |_lvl: AuditLevel, event: &'static str, kv: Vec<(&'static str, String)>| {
                let kvs = kv
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                logs2.lock().unwrap().push(format!("{event} {kvs}"));
            },
        ))
    };
    deps
}

mod anthropic;
mod core;
mod e2e;
mod guard;
mod selfcheck;
mod sse;
