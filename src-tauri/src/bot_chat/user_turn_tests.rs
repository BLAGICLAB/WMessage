//! 用户轮真源化入口的锚点测试（BACKEND-TRUTH 方案批 2）。
//!
//! 真源化后用户行的落库权在后端：client_msg_id 即行身份、事件回显同 id、
//! 上下文由库里全量行序重建。busy 零落行靠命令前置顺序（守卫判定先于落行），
//! 由本文件的接线守卫钉住；守卫判 err 的语义已有专测（chat_exec_guard_tests）。

use super::*;

fn test_handle() -> AppHandle<tauri::test::MockRuntime> {
    let app = tauri::test::mock_app();
    tauri::Manager::manage(&app, crate::app_state::AppState::default());
    app.handle().clone()
}

/// 共享库首开 warm-up：open_db 的整套建表 DDL 不在进程锁内，两个并行测试
/// 同时首开同一个全新库文件会撞 SQLite 文级写锁（零等待即报 locked）。
/// 锁内先行一次开库把 DDL 串行走完，之后的开库全是 IF NOT EXISTS no-op。
fn warm_shared_db(app: &AppHandle<tauri::test::MockRuntime>) {
    let _g = crate::db::lock_db_write();
    let conn = crate::db::open_db(app).expect("warm-up 开库");
    drop(conn);
}

/// 锚点⑧：用户行按 client_msg_id 落库、事件回显同 id；refs 原样落列
#[test]
fn user_row_lands_with_client_id_and_event_echoes_it() {
    let app = test_handle();
    warm_shared_db(&app);
    let sid = format!("utx-{}", uuid::Uuid::new_v4());
    // 事件载荷断言借回放缓冲：生产里用户行事件先于模型循环 begin、不进缓冲
    //（失步恢复由 load 兜底），这里手动 begin 只为借缓冲断言载荷形状
    crate::bot::stream_log::begin(&sid);
    let refs = r#"[{"id":"t1","title":"周报"}]"#;
    tauri::async_runtime::block_on(user_turn_context(
        &app,
        Some(&sid),
        "整理一下",
        "cmid-1",
        Some(refs),
    ))
    .expect("落行成功");
    let conn = crate::db::open_db(&app).expect("开库");
    let (msg_id, refs_json): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT msg_id, refs FROM bot_messages WHERE session_id = ?1",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("读行");
    assert_eq!(msg_id.as_deref(), Some("cmid-1"), "行身份 = client_msg_id");
    assert_eq!(refs_json.as_deref(), Some(refs), "refs 原样落列");
    let replay = crate::bot::stream_log::replay(&sid, 0);
    let ev = replay
        .events
        .iter()
        .find(|e| e.event == "bot-msg-upserted")
        .expect("用户行事件已广播");
    assert_eq!(ev.payload["msgId"], "cmid-1", "事件回显同 id");
}

/// 锚点⑩：上下文末条 = 刚落的用户行且原文照传——附件/引用块的解析是
/// L0 清洗与意图路由各自的职责，本入口只透传，任何重组都会让路由错位
#[test]
fn context_rebuilds_from_db_with_verbatim_last_user_content() {
    let app = test_handle();
    warm_shared_db(&app);
    let sid = format!("utx-{}", uuid::Uuid::new_v4());
    // 库里先有一轮旧对话（汇报轮 append 场景同源：库里有前端内存可能没有的行）。
    // 裸写共享库（建表 seed 都算写）必须整段持进程写锁——与并行测试的
    // with_write_conn 互斥，否则 SQLite 文件级写锁零等待直接报 locked
    let conn = crate::db::open_db(&app).expect("开库");
    {
        let _g = crate::db::lock_db_write();
        conn.execute_batch(BOT_MESSAGES_TEST_SEED)
            .expect("建会话表");
        conn.execute(
            "INSERT INTO bot_messages (session_id, msg_id, ord, role, content, created_at)
             VALUES (?1, 'old-a', 0, 'user', '旧问题', 1000),
                    (?1, 'old-b', 1, 'assistant', '旧回答', 1000)",
            [&sid],
        )
        .expect("seed");
    }
    drop(conn);
    let content = "帮我把这个整理成清单 [已选任务]\n- id=t1，标题=周报\n[附件文件]\n- /tmp/a.md";
    let ctx = tauri::async_runtime::block_on(user_turn_context(
        &app,
        Some(&sid),
        content,
        "cmid-2",
        None,
    ))
    .expect("上下文重建成功");
    assert_eq!(ctx.len(), 3, "库里旧行 + 刚落的用户行");
    assert_eq!(ctx[0].content, "旧问题", "旧行按行序在前");
    let last = ctx.last().expect("非空");
    assert_eq!(last.role, "user");
    assert_eq!(last.content, content, "末条原文照传，不清洗不重组");
}

/// 锚点⑨的接线守卫：命令入口里守卫判定必须先于用户行落行。
/// 旧链路「回合开始先存快照」把被拒消息孤立在库里（下轮上下文多一条没被
/// 理会的用户发言）；真源化后守卫判 err 即返回，任何落库都不发生。
#[test]
fn guard_check_precedes_user_row_write_in_chat_entry() {
    let src = include_str!("../bot_chat.rs");
    let start = src.find("async fn bot_chat_entry(").expect("入口函数起点");
    let body = &src[start..];
    let end = body.find("\n}\n").map(|i| i + 3).unwrap_or(body.len());
    let entry = &body[..end];
    let guard = entry
        .find("ChatGuard::acquire")
        .expect("入口必须先持会话守卫");
    let write = entry
        .find("user_turn_context")
        .expect("入口必须经 user_turn_context 落用户行");
    assert!(
        guard < write,
        "守卫判定必须先于用户行落行（busy 零落行）：guard={guard} write={write}"
    );
    assert!(
        !entry.contains("INSERT INTO"),
        "命令入口不得直接写表：用户行的唯一写口是行内核"
    );
}

/// 锚点⑯（接线半边）：终稿落行恒在主链之后——终态事件在循环唯一出口发出，
/// 主链返回后这里才 take + 落行 + 广播，占位替换依赖此序
#[test]
fn final_row_lands_after_the_main_chain_in_chat_entry() {
    let src = include_str!("../bot_chat.rs");
    let start = src.find("async fn bot_chat_entry(").expect("入口函数起点");
    let body = &src[start..];
    let end = body.find("\n}\n").map(|i| i + 3).unwrap_or(body.len());
    let entry = &body[..end];
    let chain = entry.find("bot_chat_guarded(").expect("主链调用");
    let take = entry.find("turn_record::take").expect("聚合收走");
    let emit = entry
        .find("emit_row_upserted(app, Some(sid), &row)")
        .expect("终稿事件广播");
    assert!(
        chain < take && take < emit,
        "主链 → take → 终稿事件，顺序不可倒：chain={chain} take={take} emit={emit}"
    );
}

/// 终稿行组装（纯函数）：聚合事实逐字段落行；终态缺失兜底 completed
/// （循环唯一出口必发终态，None 只在异常路径，与前端旧口径一致）
#[test]
fn build_final_row_maps_aggregate_fields_with_status_fallback() {
    let record = crate::bot::turn_record::TurnRecord {
        thinking: Some("想了".into()),
        tools_json: Some(r#"[{"id":"t1"}]"#.into()),
        file_changes_json: Some(r#"[{"path":"/a"}]"#.into()),
        turn_status: Some("interrupted".into()),
        partial_text: None,
    };
    let result = BotChatResult {
        text: "答复".into(),
        task_refs: vec![TaskRef {
            id: "t9".into(),
            title: "卡".into(),
        }],
        busy: false,
        trace_id: Some(7),
    };
    let row = build_final_row(record, &result);
    assert_eq!(row.role, "assistant");
    assert_eq!(row.content, "答复");
    assert_eq!(row.thinking.as_deref(), Some("想了"));
    assert_eq!(row.tools_json.as_deref(), Some(r#"[{"id":"t1"}]"#));
    assert_eq!(row.file_changes_json.as_deref(), Some(r#"[{"path":"/a"}]"#));
    assert_eq!(
        row.turn_status.as_deref(),
        Some("interrupted"),
        "有终态用终态"
    );
    assert_eq!(row.trace_id, Some(7), "trace 随行（消息 → 执行痕迹直达）");
    assert!(
        row.refs_json.as_deref().unwrap().contains("t9"),
        "引用卡随行"
    );
    assert_eq!(row.msg_id, None, "身份由行内核就地生成");
    let row2 = build_final_row(crate::bot::turn_record::TurnRecord::default(), &result);
    assert_eq!(
        row2.turn_status.as_deref(),
        Some("completed"),
        "终态缺失兜底"
    );
}

/// 锚点㉒/㉓（竞态根治的白盒形态）：前端全量快照保存通道已断代。
/// 「前端全量保存冲掉后端 append 行」的汇报竞态在写权层面不可再构造——
/// 构造不出红态即绿态的证明（设计 §十四-5 预案）。
#[test]
fn frontend_snapshot_write_path_is_retired() {
    let history = include_str!("../../../src/components/ChatPanel/useChatHistory.ts");
    assert!(
        !history.contains("persistHistory"),
        "前端历史模块不得再有 persistHistory（全量快照写权已断代）"
    );
    assert!(
        !history.contains("bot_history_save"),
        "前端不得再调用 bot_history_save"
    );
    let lib = include_str!("../lib.rs");
    assert!(
        !lib.contains("bot_history::bot_history_save,"),
        "bot_history_save 命令已断代删除，不得再注册"
    );
    // 残余保存路径全部改后端操作
    let send = include_str!("../../../src/components/ChatPanel/useChatSend.ts");
    assert!(
        send.contains("bot_history_drop"),
        "removeMessage → bot_history_drop 接线缺失"
    );
    assert!(
        send.contains("bot_history_replace_summary"),
        "compact → bot_history_replace_summary 接线缺失"
    );
    assert!(
        !send.contains("persistHistory"),
        "send 模块不得再有 persistHistory 残留"
    );
}

/// 失败行接线守卫：entry 的 Err 分支也 take + 落行，且错误继续上抛
/// （前端 catch 仍要走错误路径做本地视图兜底）
#[test]
fn failed_turn_lands_a_row_too_in_chat_entry() {
    let src = include_str!("../bot_chat.rs");
    let start = src.find("async fn bot_chat_entry(").expect("入口起点");
    let body = &src[start..];
    let end = body.find("\n}\n").map(|i| i + 3).unwrap_or(body.len());
    let entry = &body[..end];
    assert!(
        entry.contains("build_failed_row"),
        "入口 Err 分支必须落失败行（失败叙事重启后从库还原）"
    );
    assert!(
        entry.contains("return Err(e)"),
        "失败行落库后错误必须继续上抛"
    );
}

/// 失败行组装（纯函数）：partial + ⚠️ note，与前端旧 catch 合成格式一致；
/// 空正文只有 note
#[test]
fn build_failed_row_formats_partial_and_note() {
    let record = crate::bot::turn_record::TurnRecord {
        partial_text: Some("写了一半的正文".into()),
        ..Default::default()
    };
    // 显式变体：门禁拦 From<String> blanket 降级（测试里也不许）
    let e = CommandError::Internal("后端错误详情".to_string());
    let row = build_failed_row(record, &e);
    assert!(
        row.content.starts_with("写了一半的正文"),
        "已流出正文在前：{}",
        row.content
    );
    assert!(row.content.contains("⚠️"), "失败说明随行");
    assert!(row.content.contains("后端错误详情"));
    assert_eq!(row.turn_status.as_deref(), Some("failed"));
    let row2 = build_failed_row(crate::bot::turn_record::TurnRecord::default(), &e);
    assert!(row2.content.contains("⚠️"), "空正文只有 note");
    assert_eq!(row2.turn_status.as_deref(), Some("failed"));
}

/// 测试种子 DDL：身份测试需要 bot_sessions 表（bot_messages 由 open_db 基线建好）
const BOT_MESSAGES_TEST_SEED: &str = "CREATE TABLE IF NOT EXISTS bot_sessions (
             id TEXT PRIMARY KEY,
             title TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL,
             is_subagent INTEGER NOT NULL DEFAULT 0
         );";
