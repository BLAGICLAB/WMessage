//! 执行痕迹（exec_trace）集成测试（2026-10-06，Agent 透明化设计 §4.2 / §14.1 P1-d 验收）
//!
//! 覆盖：
//! 1. trace 生命周期：run_task_in_chat_with 带 hook 跑一次成功执行 → exec_traces 落
//!    done 行（session/task/origin 正确、finished_at 收尾）；失败执行 → failed 行 + error 留痕
//! 2. 采集管道：trace_sink::init 后 record_span / record_file_change → 异步批量落库可见
//!
//! 环境说明：mock_app（MockRuntime）下 data_dir 落系统 app_data_dir
//! （probe_log_dir 在 cargo target 路径下旁路便携分支，见 paths.rs::is_cargo_target_dir
//! 注释），与真实部署隔离；任务/会话/痕迹行用 uuid 隔离 + 收尾清理。
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

// 测试串行化闸：guard 跨 await 持有是刻意为之（整个用例独占共享 deps 库），
// 用 tokio 的异步锁避免 await_holding_lock，语义与 std 串行等价
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
        // 首个非 OK 的 DELETE 错误浮出（仅诊断，不引入事务；uuid 隔离下
        // 清理失败意味着共享 deps 库堆积残留，静默吞掉会掩盖根因）
        let mut first_err: Option<rusqlite::Error> = None;
        let mut note = |r: rusqlite::Result<usize>| {
            if let Err(e) = r {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        };
        note(conn.execute("DELETE FROM tasks WHERE id = ?1", [task_id]));
        note(conn.execute(
            "DELETE FROM bot_messages WHERE session_id = ?1",
            [session_id],
        ));
        note(conn.execute("DELETE FROM bot_sessions WHERE id = ?1", [session_id]));
        // trace 家族双路删除：成功路径按 session_id、失败路径按 task_id——
        // 失败收尾拿不到 TaskChatRun.session_id，trace 行落的是运行期生成的
        // 会话 id（实测残留实锤：deps 库里堆积 et-* failed 行），按 '-' 清不到
        note(conn.execute(
            "DELETE FROM exec_spans WHERE trace_id IN (SELECT id FROM exec_traces WHERE session_id = ?1)",
            [session_id],
        ));
        note(conn.execute(
            "DELETE FROM file_changes WHERE trace_id IN (SELECT id FROM exec_traces WHERE session_id = ?1)",
            [session_id],
        ));
        note(conn.execute(
            "DELETE FROM exec_traces WHERE session_id = ?1",
            [session_id],
        ));
        note(conn.execute(
            "DELETE FROM exec_spans WHERE trace_id IN (SELECT id FROM exec_traces WHERE task_id = ?1)",
            [task_id],
        ));
        note(conn.execute(
            "DELETE FROM file_changes WHERE trace_id IN (SELECT id FROM exec_traces WHERE task_id = ?1)",
            [task_id],
        ));
        note(conn.execute("DELETE FROM exec_traces WHERE task_id = ?1", [task_id]));
        if let Some(e) = first_err {
            eprintln!("[exec_trace] cleanup DELETE 失败（共享库残留风险）: {e}");
        }
    }
    // bot-enabled.flag 刻意**不删**：nextest 下 exec_trace 与 task_chat_exec 两个二进制
    // 并行进程共享 target/debug/deps/runtime/flags/——一方 cleanup 删 flag 会让另一方的
    // run_task_in_chat 撞 BotDisabled（实锤：推送门禁两用例齐挂）。setup 幂等重写，
    // flag 常驻只意味着后续测试默认机器人开（需要关的测试自行删）。
}

/// Drop 守卫：assert panic（栈展开）也执行 cleanup，失败路径不再向共享 deps
/// 库残留 trace 行。声明于 task_id 已知之后、首个可能 panic 的断言之前；
/// session_id 在运行收尾拿到后 set_session 补录（此前 drop 只按 task_id 清，
/// 覆盖 tasks + trace 家族，已足够堵失败路径）。
struct TraceCleanupGuard {
    handle: tauri::AppHandle<tauri::test::MockRuntime>,
    task_id: String,
    session_id: std::cell::RefCell<String>,
}

impl TraceCleanupGuard {
    fn set_session(&self, sid: &str) {
        *self.session_id.borrow_mut() = sid.to_string();
    }
}

impl Drop for TraceCleanupGuard {
    fn drop(&mut self) {
        cleanup(&self.handle, &self.task_id, &self.session_id.borrow());
    }
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
    let _serial = SERIAL.lock().await;
    let (handle, task_id, _dir) = setup_task("trace 验收任务");
    let guard = TraceCleanupGuard {
        handle: handle.clone(),
        task_id: task_id.clone(),
        session_id: std::cell::RefCell::new(String::new()),
    };
    let server = MockLlmServer::start();
    let http = core_http(&server);
    let hook: std::sync::Arc<std::sync::Mutex<TraceCapture>> = Default::default();
    let hook_for_run = hook.clone();

    let runner = move |_app: tauri::AppHandle<tauri::test::MockRuntime>,
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
    guard.set_session(&sid);

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
    // 清场由 guard drop 兜底（panic 路径同样清理）
}

/// 失败执行 → failed 行 + error 留痕（会话即执行记录的 trace 版）
#[tokio::test]
async fn trace_lifecycle_failed_row_records_error() {
    let _serial = SERIAL.lock().await;
    let (handle, task_id, _dir) = setup_task("trace 失败任务");
    // 失败路径拿不到真实 session_id（trace 行落的是运行期生成的会话 id），
    // guard 的 session 位留空即可——task_id 双路删除负责清 trace 家族
    let _guard = TraceCleanupGuard {
        handle: handle.clone(),
        task_id: task_id.clone(),
        session_id: std::cell::RefCell::new(String::new()),
    };
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
    // 清场由 guard drop 兜底（按 task_id 清 trace 家族；panic 路径同样生效）
}

/// 采集管道：record_span / record_file_change → 后台 writer 攒批落库
#[tokio::test]
async fn trace_sink_writes_spans_and_file_changes() {
    let _serial = SERIAL.lock().await;
    let app = Box::leak(Box::new(tauri::test::mock_app()));
    let handle = app.handle().clone();
    let _ = wmessage_lib::db::open_db(&handle).unwrap();
    wmessage_lib::trace_sink::init(handle.clone());

    // 标识带 per-run uuid 后缀：共享 deps 库里上次 panic 的残留行
    // 不会让本次「根本没写库」也命中断言；会话/任务 id 同样唯一，
    // guard drop 清理即按精确标识 scope
    let run = uuid::Uuid::new_v4().simple().to_string();
    let session_id = format!("et-sink-session-{run}");
    let task_id = format!("et-sink-task-{run}");
    let tool_call_id = format!("call_et_1-{run}");
    let file_path = format!("/a/x-{run}.py");
    // RAII 清场 guard：不读取，靠 Drop 兜底（下划线前缀消 unused 警告，Drop 语义不变）
    let _guard = TraceCleanupGuard {
        handle: handle.clone(),
        task_id: task_id.clone(),
        session_id: std::cell::RefCell::new(session_id.clone()),
    };

    // 种 trace 行（直接走 db 层，取 rowid 作归属）
    let trace_id = {
        let conn = wmessage_lib::db::open_db(&handle).unwrap();
        wmessage_lib::db::trace_start(
            &conn,
            &wmessage_lib::db::NewTrace {
                session_id: &session_id,
                task_id: Some(&task_id),
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
        tool_call_id: Some(tool_call_id.clone()),
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
        path: file_path.clone(),
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

    // 轮询等待后台 writer 落库（最多 ~5s）：断言按本 run 的精确标识
    let mut span_ok = false;
    let mut change_ok = false;
    for _ in 0..100 {
        if let Ok(conn) = wmessage_lib::db::open_db(&handle) {
            let s: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM exec_spans WHERE tool_call_id = ?1 AND ok = 1",
                    [&tool_call_id],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            let c: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM file_changes WHERE path = ?1 AND added = 3",
                    [&file_path],
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

    // 清场由 guard drop 兜底（panic 路径同样按精确标识清理）
}
