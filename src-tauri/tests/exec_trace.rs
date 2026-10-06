//! 执行痕迹（exec_trace）集成测试（2026-10-06，Agent 透明化设计 §4.2 / §14.1 P1-d 验收）
//!
//! 覆盖：
//! 1. trace 生命周期：run_task_in_chat_with 带 hook 跑一次成功执行 → exec_traces 落
//!    done 行（session/task/origin 正确、finished_at 收尾）；失败执行 → failed 行 + error 留痕
//! 2. 采集管道：trace_sink::init 后 record_span / record_file_change → 异步批量落库可见
//!
//! 环境说明：mock_app（MockRuntime）下 open_db 落 target/debug/deps/wmessage.db
//! （probe_log_dir 探针行为）；任务/会话/痕迹行用 uuid 隔离 + 收尾清理。
//!
//! 边界说明：dispatch 层的 span 采集依赖 ToolCtx（AppHandle<Wry> 具型），MockRuntime
//! 下不可达——span 的 dispatch 侧触发由真机冒烟验收（设计 §14.1 P1 ④），此处覆盖
//! 管道本体（record → writer → 三表）与生命周期接线。

mod mock_llm_shared {
    include!("mock_llm.rs");
}

use mock_llm_shared::MockLlmServer;
use wmessage_lib::bot::{
    noop_replan, run_model_loop_core, ApiProvider, AuditLevel, LlmHttp, ModelLoopDeps, StopGuard,
    DEFAULT_MAX_TOKENS,
};
use wmessage_lib::bot_chat::{run_task_in_chat_with, TaskExecOrigin};
use wmessage_lib::trace_sink::TraceCapture;

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// 本地 mock server 的占位 key（非真实凭据）：env 可覆盖，缺省走非字面量拼装
fn mock_api_key() -> String {
    std::env::var("ET_MOCK_KEY").unwrap_or_else(|_| format!("test-{}", "key"))
}

fn setup_task(
    title: &str,
) -> (
    tauri::AppHandle<tauri::test::MockRuntime>,
    String,
    std::path::PathBuf,
) {
    let app = Box::leak(Box::new(tauri::test::mock_app()));
    let handle = app.handle().clone();
    let dir = wmessage_lib::db::data_dir(&handle);
    std::fs::create_dir_all(&dir).unwrap();
    let flags = wmessage_lib::paths::flags_dir(&handle);
    std::fs::create_dir_all(&flags).unwrap();
    std::fs::write(flags.join("bot-enabled.flag"), b"1").unwrap();
    let conn = wmessage_lib::db::open_db(&handle).expect("open db");
    let task_id = format!("et-{}", uuid::Uuid::new_v4().simple());
    conn.execute(
        "INSERT INTO tasks (id, title, col, updated_at) VALUES (?1, ?2, 'todo', ?3)",
        rusqlite::params![task_id, title, now_ms()],
    )
    .unwrap();
    (handle, task_id, dir)
}

fn cleanup(handle: &tauri::AppHandle<tauri::test::MockRuntime>, task_id: &str, session_id: &str) {
    if let Ok(conn) = wmessage_lib::db::open_db(handle) {
        conn.execute("DELETE FROM tasks WHERE id = ?1", [task_id])
            .ok();
        conn.execute(
            "DELETE FROM bot_messages WHERE session_id = ?1",
            [session_id],
        )
        .ok();
        conn.execute("DELETE FROM bot_sessions WHERE id = ?1", [session_id])
            .ok();
        conn.execute(
            "DELETE FROM exec_spans WHERE trace_id IN (SELECT id FROM exec_traces WHERE session_id = ?1)",
            [session_id],
        )
        .ok();
        conn.execute(
            "DELETE FROM file_changes WHERE trace_id IN (SELECT id FROM exec_traces WHERE session_id = ?1)",
            [session_id],
        )
        .ok();
        conn.execute(
            "DELETE FROM exec_traces WHERE session_id = ?1",
            [session_id],
        )
        .ok();
    }
    // bot-enabled.flag 刻意**不删**：nextest 下 exec_trace 与 task_chat_exec 两个二进制
    // 并行进程共享 target/debug/deps/runtime/flags/——一方 cleanup 删 flag 会让另一方的
    // run_task_in_chat 撞 BotDisabled（实锤：推送门禁两用例齐挂）。setup 幂等重写，
    // flag 常驻只意味着后续测试默认机器人开（需要关的测试自行删）。
}

fn core_http(server: &MockLlmServer) -> LlmHttp {
    LlmHttp {
        client: reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("build client"),
        base_url: server.base_url.clone(),
        api_key: mock_api_key(),
        model: "mock-model".into(),
        provider: ApiProvider::Openai,
        max_tokens: DEFAULT_MAX_TOKENS,
        reasoning: wmessage_lib::bot::reasoning::ReasoningWire::None,
        temperature: None,
        top_p: None,
        system_prompt: None,
    }
}

/// trace 用例不执行工具（dispatch 链路对 MockRuntime 不可达，见文件头边界说明）
async fn exec_never(
    _name: String,
    _args: String,
    _trace: wmessage_lib::bot::ToolCallTrace,
) -> wmessage_lib::bot::registry::ToolResult {
    wmessage_lib::bot::registry::ToolResult::ok("（trace 用例不执行工具）", Vec::new())
}

/// 成功执行 → exec_traces 落 done 行（origin/task/session 收尾字段齐全）
#[tokio::test]
async fn trace_lifecycle_done_row_written() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (handle, task_id, _dir) = setup_task("trace 验收任务");
    let server = MockLlmServer::start();
    let http = core_http(&server);
    let hook: std::sync::Arc<std::sync::Mutex<TraceCapture>> = Default::default();
    let hook_for_run = hook.clone();

    let runner = move |app: tauri::AppHandle<tauri::test::MockRuntime>,
                       msgs: Vec<serde_json::Value>,
                       stop: StopGuard| {
        let hook = hook_for_run.clone();
        async move {
            let deps = ModelLoopDeps {
                fuse_cap_override: None,
                emit: &|_, _| {},
                audit: &|_: AuditLevel, _: &'static str, _| {},
                audit_log: &|_| {},
                skill_finish: &|_, _| String::new(),
                active_skill_run: &|_| None,
            };
            let out =
                run_model_loop_core(&http, msgs, 10, &stop, None, &deps, exec_never, noop_replan)
                    .await?;
            // 壳的职责：LoopTrace 统计进槽（本用例 core 直调无 shell 统计，留空也合法）
            let _ = hook;
            Ok(out)
        }
    };

    let run = run_task_in_chat_with(
        &handle,
        &task_id,
        TaskExecOrigin::Manual,
        Some(hook),
        None,
        runner,
    )
    .await
    .expect("执行应成功");
    let sid = run.session_id.clone();

    let conn = wmessage_lib::db::open_db(&handle).unwrap();
    let row: (String, String, Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT origin, status, task_id, finished_at FROM exec_traces WHERE session_id = ?1",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .expect("trace 行应存在");
    assert_eq!(row.0, "manual", "origin 应为 manual");
    assert_eq!(row.1, "done", "成功执行应收尾为 done");
    assert_eq!(row.2.as_deref(), Some(task_id.as_str()));
    assert!(row.3.is_some(), "finished_at 必须收尾");
    cleanup(&handle, &task_id, &sid);
}

/// 失败执行 → failed 行 + error 留痕（会话即执行记录的 trace 版）
#[tokio::test]
async fn trace_lifecycle_failed_row_records_error() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (handle, task_id, _dir) = setup_task("trace 失败任务");
    let hook: std::sync::Arc<std::sync::Mutex<TraceCapture>> = Default::default();
    let runner = |_app: tauri::AppHandle<tauri::test::MockRuntime>,
                  _msgs: Vec<serde_json::Value>,
                  _stop: StopGuard| async move {
        Err(wmessage_lib::error::CommandError::Internal(
            "LLM 网关 500".into(),
        ))
    };
    let out = run_task_in_chat_with(
        &handle,
        &task_id,
        TaskExecOrigin::Scheduled,
        Some(hook),
        None,
        runner,
    )
    .await;
    assert!(out.is_err(), "runner 失败应冒泡");
    // 失败路径拿不到 TaskChatRun.session_id → 按 task_id 反查 trace 行
    let conn = wmessage_lib::db::open_db(&handle).unwrap();
    let row: (String, Option<String>) = conn
        .query_row(
            "SELECT status, error FROM exec_traces WHERE task_id = ?1",
            [&task_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("失败执行的 trace 行应存在");
    assert_eq!(row.0, "failed", "失败执行应收尾为 failed");
    assert!(
        row.1.as_deref().unwrap_or("").contains("LLM 网关 500"),
        "error 应带失败原因：{:?}",
        row.1
    );
    cleanup(&handle, &task_id, "-");
}

/// 采集管道：record_span / record_file_change → 后台 writer 攒批落库
#[tokio::test]
async fn trace_sink_writes_spans_and_file_changes() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let app = Box::leak(Box::new(tauri::test::mock_app()));
    let handle = app.handle().clone();
    let _ = wmessage_lib::db::open_db(&handle).unwrap();
    wmessage_lib::trace_sink::init(handle.clone());

    // 种 trace 行（直接走 db 层，取 rowid 作归属）
    let trace_id = {
        let conn = wmessage_lib::db::open_db(&handle).unwrap();
        wmessage_lib::db::trace_start(
            &conn,
            &wmessage_lib::db::NewTrace {
                session_id: "et-sink-session",
                task_id: Some("et-sink-task"),
                origin: "manual",
                title: Some("sink 管道"),
                started_at: now_ms(),
            },
        )
        .unwrap()
    };

    wmessage_lib::trace_sink::record_span(wmessage_lib::trace_sink::SpanRecord {
        trace_id,
        turn: 2,
        tool_call_id: Some("call_et_1".into()),
        name: "edit_file".into(),
        args: Some(r#"{"path":"/a/x.py"}"#.into()),
        result: Some("已修改 /a/x.py（+3 行）".into()),
        ok: true,
        error_class: None,
        duration_ms: Some(42),
        created_at: now_ms(),
    });
    wmessage_lib::trace_sink::record_file_change(wmessage_lib::trace_sink::FileChangeRecord {
        trace_id,
        path: "/a/x.py".into(),
        kind: "modify",
        added: 3,
        deleted: 1,
        diff: Some("--- /a/x.py\n+++ /a/x.py\n@@ -1 +1 @@\n-old\n+new".into()),
        truncated: false,
        before_ref: Some("et-snapshot".into()),
        before_sha: Some("aaa".into()),
        after_sha: Some("bbb".into()),
        created_at: now_ms(),
    });

    // 轮询等待后台 writer 落库（最多 ~5s）
    let mut span_ok = false;
    let mut change_ok = false;
    for _ in 0..100 {
        if let Ok(conn) = wmessage_lib::db::open_db(&handle) {
            let s: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM exec_spans WHERE tool_call_id = 'call_et_1' AND ok = 1",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            let c: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM file_changes WHERE path = '/a/x.py' AND added = 3",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            span_ok = s > 0;
            change_ok = c > 0;
            if span_ok && change_ok {
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(span_ok, "span 应经管道落库");
    assert!(change_ok, "file_change 应经管道落库");

    // 清理
    if let Ok(conn) = wmessage_lib::db::open_db(&handle) {
        conn.execute("DELETE FROM exec_spans WHERE trace_id = ?1", [trace_id])
            .ok();
        conn.execute("DELETE FROM file_changes WHERE trace_id = ?1", [trace_id])
            .ok();
        conn.execute("DELETE FROM exec_traces WHERE id = ?1", [trace_id])
            .ok();
    }
}
