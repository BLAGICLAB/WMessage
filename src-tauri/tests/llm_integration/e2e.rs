//! 端到端用例：记忆闸、任务卡新列往返、工具图片入模。

use super::*;

#[tokio::test]
async fn memory_gate_scenarios_injection_and_auto_write() {
    let app = mock_handle();
    let seed_content = SEED_CONTENT_PLACEHOLDER;

    // 清理前移：先扫掉历史运行留在共享库的种子/漏写条目与共享配置文件——
    // 即使本运行中途 panic，下一次运行开场也会自愈（不依赖收尾路径执行）。
    clean_gate_leftovers(&app);

    // 场景一：注入关 → 记忆保留在库但不产出注入块
    write_config_with_memory_control(&app, false, true);
    seed_memory(&app, seed_content);
    assert!(
        wmessage_lib::memory::injection_block(&app, "测试记忆")
            .await
            .is_none(),
        "注入关闭时不得产出记忆块"
    );

    // 场景二：注入开 → 照常产出且含库内内容
    write_config_with_memory_control(&app, true, true);
    let block = wmessage_lib::memory::injection_block(&app, "测试记忆")
        .await
        .expect("注入开启 + 库有记忆应产出记忆块");
    assert!(
        block.contains("## 记忆") && block.contains(seed_content),
        "{block}"
    );
    // 结构窄化：fact 条目应以标准行格式落在记忆块（kind 行格式契约）
    assert!(
        block.contains(&format!("- [fact]{seed_content}")),
        "fact 种子应以标准行格式出现：{block}"
    );

    // 场景三：自动写入关 → 两工具返回「已关闭」提示且不落库。
    // 断言走键位（find_by_key_tag）而非全表计数——共享库可能被并行测试
    // 进程写入别的条目，计数口径会误伤；本工具写入的键唯一可精确判定。
    write_config_with_memory_control(&app, true, false);
    let conn = wmessage_lib::db::open_db(&app).unwrap();
    let r = wmessage_lib::memory::tool_remember_fact(&app, r#"{"key":"测试键","value":"测试值"}"#)
        .await;
    assert!(r.text.contains("无需记录"), "{}", r.text);
    let r = wmessage_lib::memory::tool_record_lesson(&app, r#"{"lesson":"某教训"}"#).await;
    assert!(r.text.contains("无需记录"), "{}", r.text);
    assert!(
        wmessage_lib::memory::store::find_by_key_tag(&conn, "测试键")
            .unwrap()
            .is_none(),
        "门禁关闭时不得写入「测试键」"
    );

    // 收尾同样恢复现场（成功路径；中途 panic 由下一次运行的开场清理兜底）
    clean_gate_leftovers(&app);
}

/// A：模型循环把 query_tasks / edit_task 的 (name, args) 原样送达 executor，
/// 工具结果以 role:tool 回填并续聊到最终文本（工具升级不破循环协议）。
#[tokio::test]
async fn t1_model_loop_delivers_query_and_edit_tool_calls() {
    // 轮次 1：query_tasks（view/tag/limit 新参数）→ 轮次 2：edit_task（model/owner 新参数）→
    // 轮次 3：最终文本
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "query_tasks".into(),
        arguments: r#"{"view":"done","tag":"周报","limit":10}"#.into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "edit_task".into(),
        arguments: r#"{"taskId":"t1e2e-active","model":"mock-model-t1","owner":"T1测试成员甲"}"#
            .into(),
    }));
    server.push_behavior(MockBehavior::TextReply("已更新 1 张卡".into()));
    let h = CoreHarness::new();

    let calls: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let calls2 = calls.clone();
    let exec = move |name: String, args: String, _trace: ToolCallTrace| {
        let calls = calls2.clone();
        async move {
            let is_query = name == "query_tasks";
            calls.lock().unwrap().push((name, args));
            if is_query {
                ToolResult::ok("- [已完成] T1样例卡（id=t1e2e-done）", Vec::new())
            } else {
                ToolResult::ok("已更新任务「样例」（执行模型、归属人）", Vec::new())
            }
        }
    };

    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        6,
        &h.stop,
        None,
        &h.deps(),
        exec,
        noop_replan,
    )
    .await
    .expect("工具回路应 Ok");

    assert_eq!(text, "已更新 1 张卡");
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &[
            (
                "query_tasks".to_string(),
                r#"{"view":"done","tag":"周报","limit":10}"#.to_string()
            ),
            (
                "edit_task".to_string(),
                r#"{"taskId":"t1e2e-active","model":"mock-model-t1","owner":"T1测试成员甲"}"#
                    .to_string()
            ),
        ],
        "新工具名与新参数应原样抵达 executor"
    );
    assert_eq!(server.request_count(), 3, "两次工具调用后第三轮收尾");
}

/// B：DB 往返 + RMW 写回（真 SQL/真写锁/共享库文件）——工具层读写的全部新列
/// 在 SQLite 的落库与读出；edit_task 的「load → 改 → 自戳 → upsert」模式验证。
/// （db_upsert/db_load 命令是 AppHandle<Wry> 类型，mock runtime 下走 conn 层同函数：
/// open_db + lock_db_write + upsert_tasks + load_all，与生产读写同一代码路径。）
/// schedule 列按 T1 定时单源契约断言：open_db 迁移把非空 tasks.schedule 迁往
/// scheduled_jobs（job-<task_id>）并清空任务卡侧——任务卡 schedule 不再是写路径。
#[tokio::test]
async fn t1_db_roundtrip_new_columns_and_rmw_edit() {
    let app = mock_handle();
    t1_cleanup(&app);

    // 种子：新列全非空的卡（model/owner_id/depends_on/子任务/created_at）
    let mut t = t1_seed_task(
        "t1e2e-active",
        T1_TITLE_ACTIVE,
        wmessage_lib::db::TaskStatus::Todo,
    );
    t.model = Some("mock-model-t1".into());
    t.owner_id = Some(T1_OWNER_ID.into());
    t.depends_on = Some(vec!["t1e2e-dep".into()]);
    t.subtasks = Some(vec![wmessage_lib::db::Subtask {
        id: "t1e2e-sub-1".into(),
        text: "核对参数".into(),
        done: false,
        task_id: None,
    }]);
    {
        let conn = wmessage_lib::db::open_db(&app).unwrap();
        let _g = wmessage_lib::db::lock_db_write();
        wmessage_lib::db::upsert_tasks(&conn, std::slice::from_ref(&t)).unwrap();
    }

    // 往返：load_all 读出的字段与写入一致（edit_task/query_single_task 的读取接缝）
    let tasks = {
        let conn = wmessage_lib::db::open_db(&app).unwrap();
        wmessage_lib::db::load_all(&conn).unwrap()
    };
    let loaded = tasks.iter().find(|t| t.id == "t1e2e-active").unwrap();
    assert_eq!(loaded.model.as_deref(), Some("mock-model-t1"));
    assert_eq!(loaded.owner_id.as_deref(), Some(T1_OWNER_ID));
    assert_eq!(
        loaded.depends_on.as_deref(),
        Some(&["t1e2e-dep".to_string()][..])
    );
    assert_eq!(loaded.created_at, Some(1_000));
    assert_eq!(loaded.subtasks.as_ref().unwrap()[0].id, "t1e2e-sub-1");

    // RMW 编辑模式（tool_edit_task 同款）：基线 = 快照 updated_at，改 model/owner + 勾子任务
    let mut next = loaded.clone();
    next.model = None; // 空串清除语义 → None
    next.owner_id = None; // 归属本人语义 → None
    next.subtasks.as_mut().unwrap()[0].done = true; // subtaskId 勾选语义
    next.expected_updated_at = next.updated_at; // RMW 写回基线
    next.updated_at = Some(2_000);
    {
        let conn = wmessage_lib::db::open_db(&app).unwrap();
        let _g = wmessage_lib::db::lock_db_write();
        wmessage_lib::db::upsert_tasks(&conn, std::slice::from_ref(&next)).unwrap();
    }

    let tasks = {
        let conn = wmessage_lib::db::open_db(&app).unwrap();
        wmessage_lib::db::load_all(&conn).unwrap()
    };
    let after = tasks.iter().find(|t| t.id == "t1e2e-active").unwrap();
    assert_eq!(after.model, None, "清除后应落 NULL");
    assert_eq!(after.owner_id, None);
    assert!(after.subtasks.as_ref().unwrap()[0].done, "勾选应落库");

    t1_cleanup(&app);
}

/// N5：工具随结果附图 → 图作为紧随 tool 消息的 user 消息进上下文。
/// 断言第二轮请求体：tool 消息文本在前，紧跟 user 消息带〔系统附图〕+ data URL 图片。
#[tokio::test]
async fn n5_tool_image_appended_as_user_message() {
    // 临时「截图」文件：image_part_from_file 只校验扩展名/大小，不解码内容
    let dir = std::env::temp_dir().join(format!(
        "wm-n5-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join("shot.png");
    std::fs::write(&img, b"\x89PNG\r\n\x1a\n fake-png-bytes").unwrap();
    let img_path = img.display().to_string();

    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "screenshot".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::TextReply("看到了".into()));
    let h = CoreHarness::new();

    let p = img_path.clone();
    let exec = move |_name: String, _args: String, _trace: ToolCallTrace| {
        let p = p.clone();
        async move { ToolResult::ok_with_images(format!("已截屏：{p}"), Vec::new(), vec![p]) }
    };

    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        4,
        &h.stop,
        None,
        &h.deps(),
        exec,
        noop_replan,
    )
    .await
    .expect("工具回路应 Ok");
    assert_eq!(text, "看到了");
    assert_eq!(server.request_count(), 2, "工具后应续聊一轮");

    let bodies = server.request_bodies();
    assert_eq!(bodies.len(), 2);
    // 字符安全截断（断言失败时的诊断输出，避免多字节字符字节切片 panic）
    let head: String = bodies[1].chars().take(3000).collect();
    // 第二轮请求体：tool 消息文本在前
    assert!(bodies[1].contains("已截屏"), "{head}");
    // 紧随的 user 消息：〔系统附图〕提示 + data URL 图片
    assert!(bodies[1].contains("系统附图"), "缺附图提示：{head}");
    assert!(
        bodies[1].contains("data:image/png;base64,"),
        "缺 base64 图片"
    );
    // 图在 tool 消息之后（user 消息里），不在 tool 消息内（OpenAI 协议 tool 只收文本）
    let tool_pos = bodies[1].find("已截屏").unwrap();
    let user_img_pos = bodies[1].find("data:image/png;base64,").unwrap();
    assert!(user_img_pos > tool_pos, "图应出现在 tool 消息之后");

    std::fs::remove_dir_all(&dir).ok();
}
