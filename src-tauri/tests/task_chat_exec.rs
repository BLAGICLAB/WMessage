//! 任务执行聊天化全链路测试（2026-09-10，docs/TASK-CHAT-EXECUTION-DESIGN.md 第 6 节）
//!
//! 覆盖：
//! 1. run_task_in_chat 全链路（mock LLM）：会话创建（📋 标题前缀）、任务块 user 消息落库、
//!    模型循环接线（EXECUTE_SYSTEM_PROMPT + 任务块）、assistant 回复落库、任务卡回写
//!    （complete_task 工具结果落到 tasks 表——工具执行用 conn 级替身，execute_tool 的
//!    AppHandle 链路由 llm_integration 既有用例覆盖）、ChatGuard 执行期持有
//! 2. 同卡并发触发 → ExecGuard 拒绝
//! 3. 定时路径源码锁：bot_scheduler 调 run_task_in_chat、绕开 exec_steps、发系统通知
//!
//! 环境说明：mock_app（MockRuntime）下 open_db 落 target/debug/deps/wmessage.db
//! （probe_log_dir 探针行为），测试用 uuid 任务 id + 结束后清理行，不污染其他测试。

mod mock_llm_shared {
    include!("mock_llm.rs");
}

use mock_llm_shared::{MockBehavior, MockLlmServer, ToolCallResponse};
use wmessage_lib::bot::{
    noop_replan, run_model_loop_core, ApiProvider, AuditLevel, LlmHttp, ModelLoopDeps, StopGuard,
    TaskRef, ToolCallTrace, DEFAULT_MAX_TOKENS,
};
use wmessage_lib::bot_chat::{chat_guard_is_held, run_task_in_chat_with, TaskExecOrigin};

/// 三个用例共享 target/debug/deps 下的 wmessage.db 与 bot-enabled.flag——
/// 串行执行防 flag 清理竞态（cargo 各测试二进制之间本就串行，本文件内并行用例需自锁）
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// 测试环境准备：mock app + 开机器人开关 + 插任务；返回 (handle, task_id, 数据目录)。
/// 清理由调用方收尾（cleanup）——共享 target/debug/deps/wmessage.db，行级删。
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
    let task_id = format!("tc-{}", uuid::Uuid::new_v4().simple());
    conn.execute(
        "INSERT INTO tasks (id, title, col, updated_at) VALUES (?1, ?2, 'todo', ?3)",
        rusqlite::params![task_id, title, now_ms()],
    )
    .unwrap();
    (handle, task_id, dir)
}

/// 行级清理（共享库不整删——其他并行测试可能也在用）
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
    }
    let _ = std::fs::remove_file(wmessage_lib::paths::flags_dir(handle).join("bot-enabled.flag"));
}

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

#[tokio::test]
async fn run_task_in_chat_full_chain_manual_origin() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (handle, task_id, _dir) = setup_task("写季度总结报告");
    let server = MockLlmServer::start();
    // mock LLM：第一轮要求 complete_task（任务卡回写路径），第二轮给最终文本
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "complete_task".into(),
        arguments: format!(r#"{{"taskId":"{task_id}"}}"#),
    }));
    server.push_behavior(MockBehavior::TextReply("任务已完成，报告已生成".into()));

    let http = core_http(&server);
    let stop_holder: std::sync::Arc<std::sync::Mutex<Option<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let sh = stop_holder.clone();
    let handle2 = handle.clone();
    let task_id_for_runner = task_id.clone();
    let runner = move |app: tauri::AppHandle<tauri::test::MockRuntime>,
                       msgs: Vec<serde_json::Value>,
                       stop: StopGuard| {
        let http = http;
        let sh = sh.clone();
        let handle3 = handle2.clone();
        let task_id = task_id_for_runner.clone();
        async move {
            // 消息组装断言：system = EXECUTE_SYSTEM_PROMPT（含任务卡执行规则），
            // user = 任务块（含标题）
            let sys = msgs[0]["content"].as_str().unwrap_or("");
            assert!(
                sys.contains("执行一张任务卡"),
                "system 应为执行提示词：{sys}"
            );
            let user = msgs
                .iter()
                .find(|m| m["role"].as_str() == Some("user"))
                .and_then(|m| m["content"].as_str())
                .expect("应有 user 任务块");
            assert!(
                user.contains("[任务卡执行]") && user.contains("写季度总结报告"),
                "{user}"
            );
            // ChatGuard：执行期会话锁必须持有（bot_execute_task 纳入 ChatGuard 的证据）
            let sid = stop.session_id().expect("执行会话 id").to_string();
            assert!(chat_guard_is_held(&app, &sid), "执行期 ChatGuard 应持有");
            *sh.lock().unwrap() = Some(sid);
            // 阶段 3.3：核心只读「活动技能快照」的注入（须在 exec 闭包 move 走 handle3 之前克隆）
            let handle_for_skill = handle3.clone();
            let active_skill_run = move |sid: Option<&str>| {
                wmessage_lib::bot_skills::active_skill_run_for(&handle_for_skill, sid)
            };
            // 工具执行替身：execute_tool 的 AppHandle(Wry) 链路不在 mock runtime 下可调，
            // 这里按 complete_task 语义直写同一个库（列 → done），验证回写通路
            let exec = move |name: String, _args: String, _trace: ToolCallTrace| {
                let handle4 = handle3.clone();
                let task_id = task_id.clone();
                async move {
                    assert_eq!(name, "complete_task");
                    let conn = wmessage_lib::db::open_db(&handle4).expect("open db");
                    conn.execute(
                        "UPDATE tasks SET col = 'done', completed_at = ?1 WHERE id = ?2",
                        rusqlite::params![now_ms(), task_id],
                    )
                    .unwrap();
                    ("已标记完成".to_string(), Vec::<TaskRef>::new())
                }
            };
            let deps = ModelLoopDeps {
                emit: &|_, _| {},
                audit: &|_: AuditLevel, _: &'static str, _| {},
                audit_log: &|_| {},
                skill_finish: &|_, _| String::new(),
                active_skill_run: &active_skill_run,
            };
            run_model_loop_core(&http, msgs, 10, &stop, None, &deps, exec, noop_replan).await
        }
    };

    let run = run_task_in_chat_with(&handle, &task_id, TaskExecOrigin::Manual, runner)
        .await
        .expect("执行应成功");
    let sid = run.session_id.clone();
    assert_eq!(run.result.text, "任务已完成，报告已生成");
    // runner 内记录的会话 id 与返回值一致
    assert_eq!(stop_holder.lock().unwrap().as_deref(), Some(sid.as_str()));
    // 执行结束后 ChatGuard 已释放
    assert!(
        !chat_guard_is_held(&handle, &sid),
        "执行结束 ChatGuard 应释放"
    );

    // 会话创建：标题前缀 📋 任务：
    let conn = wmessage_lib::db::open_db(&handle).unwrap();
    let title: String = conn
        .query_row(
            "SELECT title FROM bot_sessions WHERE id = ?1",
            [&sid],
            |r| r.get(0),
        )
        .expect("执行会话应存在");
    assert_eq!(title, "📋 任务：写季度总结报告");
    // 消息落库：user 任务块 + assistant 回复
    let rows: Vec<(String, String)> = conn
        .prepare("SELECT role, content FROM bot_messages WHERE session_id = ?1 ORDER BY id")
        .unwrap()
        .query_map([&sid], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(rows.len(), 2, "应为 user 任务块 + assistant 回复：{rows:?}");
    assert_eq!(rows[0].0, "user");
    assert!(rows[0].1.contains("[任务卡执行]"));
    assert_eq!(rows[1].0, "assistant");
    assert_eq!(rows[1].1, "任务已完成，报告已生成");
    // 任务卡回写：complete_task 替身落库 col=done
    let col: String = conn
        .query_row("SELECT col FROM tasks WHERE id = ?1", [&task_id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(col, "done", "任务卡应被回写为完成");
    // 模型循环接线：第一轮请求带任务块，第二轮带工具结果
    let bodies = server.request_bodies();
    assert!(bodies[0].contains("[任务卡执行]"), "首轮请求应含任务块");
    assert!(
        bodies[1].contains("已标记完成"),
        "工具结果应回填：{}",
        bodies[1]
    );

    cleanup(&handle, &task_id, &sid);
}

#[tokio::test]
async fn duplicate_trigger_rejected_by_exec_guard() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (handle, task_id, _dir) = setup_task("并发防护任务");
    // 第一个执行的 runner 卡住直到放行
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let h1 = handle.clone();
    let t1 = task_id.clone();
    let first = tokio::spawn(async move {
        run_task_in_chat_with(
            &h1,
            &t1,
            TaskExecOrigin::Batch,
            move |_app, _msgs, _stop| async move {
                let _ = entered_tx.send(());
                let _ = release_rx.await;
                Ok(("done".to_string(), Vec::new()))
            },
        )
        .await
    });
    entered_rx.await.expect("第一个执行应已进入模型循环");
    // 同卡并发第二次触发 → ExecGuard 拒绝
    let sid_holder = std::sync::Mutex::new(String::new());
    let second = run_task_in_chat_with(
        &handle,
        &task_id,
        TaskExecOrigin::Scheduled,
        move |_app, _msgs, _stop| async move { Ok(("不应执行到这里".to_string(), Vec::new())) },
    )
    .await;
    let err = match second {
        Err(e) => e,
        Ok(_) => panic!("并发触发应被拒绝"),
    };
    assert!(
        err.message().contains("正在执行中"),
        "应为防重入拒绝：{}",
        err.message()
    );
    let _ = sid_holder;
    let _ = release_tx.send(());
    let r1 = first.await.expect("第一个执行线程 join");
    let run = r1.expect("第一个执行应成功");
    cleanup(&handle, &task_id, &run.session_id);
}

#[tokio::test]
async fn failure_persists_error_reply_in_session() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (handle, task_id, _dir) = setup_task("注定失败的任务");
    let runner = |_app: tauri::AppHandle<tauri::test::MockRuntime>,
                  _msgs: Vec<serde_json::Value>,
                  _stop: StopGuard| async move {
        Err(wmessage_lib::error::CommandError::Internal(
            "LLM 网关 500".into(),
        ))
    };
    let err =
        match run_task_in_chat_with(&handle, &task_id, TaskExecOrigin::Scheduled, runner).await {
            Err(e) => e,
            Ok(_) => panic!("应失败"),
        };
    assert!(err.message().contains("LLM 网关 500"));
    // 失败也落库：assistant 行为 ⚠️ 失败行（会话即执行记录，留证可回看）
    let conn = wmessage_lib::db::open_db(&handle).unwrap();
    let row: Option<(String, String)> = conn
        .prepare(
            "SELECT m.role, m.content FROM bot_messages m
             JOIN bot_sessions s ON s.id = m.session_id
             WHERE s.title LIKE '⏰ 定时：%' AND m.role = 'assistant'
             ORDER BY m.id DESC LIMIT 1",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .filter_map(|r| r.ok())
        .next();
    let (_, content) = row.expect("失败回复应落库");
    assert!(
        content.contains("⚠️ 执行失败") && content.contains("LLM 网关 500"),
        "{content}"
    );
    // 定时来源标题前缀
    let title: String = conn
        .query_row(
            "SELECT title FROM bot_sessions WHERE title LIKE '⏰ 定时：%' ORDER BY created_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(title, "⏰ 定时：注定失败的任务");
    // 清理（找不到 sid 就按标题删）
    conn.execute("DELETE FROM bot_messages WHERE session_id IN (SELECT id FROM bot_sessions WHERE title = ?1)", [&title]).ok();
    conn.execute("DELETE FROM bot_sessions WHERE title = ?1", [&title])
        .ok();
    conn.execute("DELETE FROM tasks WHERE id = ?1", [&task_id])
        .ok();
    let _ = std::fs::remove_file(wmessage_lib::paths::flags_dir(&handle).join("bot-enabled.flag"));
}

/// 定时路径源码锁（设计 3.2）：bot_scheduler 调 run_task_in_chat、绕开 exec_steps、
/// 完成/失败发系统通知（notification().builder()）；execute_task_core 已删除。
#[test]
fn scheduler_uses_run_task_in_chat_not_exec_steps() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bot_scheduler.rs"))
        .expect("bot_scheduler.rs 可读");
    assert!(
        src.contains("run_task_in_chat"),
        "定时路径应走 run_task_in_chat"
    );
    assert!(src.contains("TaskExecOrigin::Scheduled"), "定时来源标记");
    assert!(
        !src.contains("exec_steps::start") && !src.contains("exec_steps::resume"),
        "定时路径不得调逐步执行（无人在场）"
    );
    assert!(src.contains("notification()"), "完成/失败应发系统通知");
    // execute_task_core 已被 run_task_in_chat 取代（全仓库无残留调用）
    let bot_chat = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bot_chat.rs"))
        .expect("bot_chat.rs 可读");
    assert!(
        !bot_chat.contains("fn execute_task_core"),
        "execute_task_core 应已被 run_task_in_chat 取代"
    );
}
