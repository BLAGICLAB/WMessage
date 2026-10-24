use super::*;
use crate::db::Task;

/// 编排器源码全景：拆分后代码分布在门面与子模块里，源码守卫按函数定位时扫这一份。
///
/// 顺序要紧：子模块在前、门面在后——守卫用 `split_once("#[cfg(test)]")`
/// 取「首个测试模块之前」的生产切片，门面里的测试模块排在最后，
/// 子模块的代码才落进生产切片。
fn orch_prod_src() -> String {
    [
        include_str!("../bot/subagent_digest.rs"),
        include_str!("../bot/subagent_repair.rs"),
        include_str!("../bot/subagent_runner.rs"),
        include_str!("../bot/subagent_spawn.rs"),
        include_str!("../bot/subagent_tools.rs"),
        include_str!("../bot_orchestrator.rs"),
    ]
    .concat()
}
fn test_conn() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    // tasks 表内联最小 schema（列序与 open_db 一致，含  新增三列）；
    // subagents 表与生产同源 DDL（单源真相）
    conn.execute_batch(crate::db::tasks::TASKS_DDL).unwrap();
    conn.execute_batch(crate::db::SUBAGENTS_DDL).unwrap();
    conn
}
fn parent_card(conn: &rusqlite::Connection) -> String {
    let card = Task {
        plan_start: None,
        plan_end: None,
        parent_id: None,
        id: "parent-1".into(),
        title: "主编排卡".into(),
        due: None,
        note: None,
        tags: None,
        files: None,
        column: crate::db::TaskStatus::Todo,
        subtasks: None,
        completed_at: None,
        archived: Some(false),
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
        created_at: None,
        expected_updated_at: None,
    };
    crate::db::upsert_tasks(conn, std::slice::from_ref(&card)).unwrap();
    "parent-1".into()
}
fn fake_state(status: SubagentStatus) -> CheckState {
    CheckState {
        subagent_id: "sa_wait_test".into(),
        task_id: None,
        session_id: None,
        status,
        progress: serde_json::json!({ "subtasksDone": 0, "subtasksTotal": 1 }),
        result: None,
        error: None,
    }
}
fn digest_row(id: &str, status: SubagentStatus, result: Option<&str>) -> SubagentRow {
    SubagentRow {
        id: id.into(),
        status,
        profile: SubagentProfile::Coder,
        model: None,
        parent_session_id: Some("main-sess".into()),
        session_id: Some(format!("sub-{id}")),
        task_id: None,
        objective: format!("目标-{id}"),
        trace_id: format!("trc-{id}"),
        acceptance_json: None,
        context_summary: None,
        budget_json: None,
        result_json: result.map(str::to_string),
        error: (status == SubagentStatus::Failed).then(|| "LLM 5xx".to_string()),
        created_at: 1_000,
        started_at: Some(1_001),
        finished_at: Some(1_002),
        prompt_tokens: None,
        completion_tokens: None,
        cache_read_tokens: None,
    }
}
const OK_RESULT: &str = r#"{"status":"succeeded","summary":"覆盖 5 个竞品，含官网 URL","artifacts":[{"path":"report.md","description":"竞品对比"},{"path":"data.csv"}]}"#;

fn req(parent: Option<String>) -> SpawnRequest {
    SpawnRequest {
        objective: "调研五个竞品并输出报告".into(),
        profile: SubagentProfile::Research,
        stage: None,
        acceptance_criteria: vec!["覆盖至少 5 个产品".into(), "每条含官网 URL".into()],
        context_summary: None,
        budget: None,
        model_profile: None,
        onion: None,
        parent_task_id: parent,
        parent_session_id: Some("main-sess".into()),
    }
}

mod lifecycle;
mod outcome;
mod spawn;
