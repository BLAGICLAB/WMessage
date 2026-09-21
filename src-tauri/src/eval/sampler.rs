//! 从 bot_sessions 采样 eval case（R1 L4 评估层）
//!
//! spec：从 bot_sessions 采样 100-500 个 session + 从 #[test] 转换。
//! 当前 dev DB 无 bot_sessions 表（VERIFICATION.md 备注），代码就位等数据出现。
//!
//! 只读操作（不写新表、不改 schema）；硬约束 4 满足。

use rusqlite::Connection;

use super::case::{append_jsonl, EvalCase, MetricSpec};

/// 单条 session 元数据（从 bot_sessions 表读）
#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id: String,
    pub title: String,
    pub updated_at: i64,
}

/// 列出 session；缺表 / 缺 db 时返回空 vec + 警告（不 panic）
pub fn list_sessions(conn: &Connection) -> Result<Vec<SessionRow>, String> {
    // 表存在性检查：缺则返回空 vec（不抛错——eval runner 跳过）
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='bot_sessions'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| format!("查 bot_sessions 表失败：{e}"))?;
    if exists == 0 {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare("SELECT id, title, updated_at FROM bot_sessions ORDER BY updated_at DESC")
        .map_err(|e| format!("prepare bot_sessions 失败：{e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(SessionRow {
                id: r.get(0)?,
                title: r.get(1)?,
                updated_at: r.get(2)?,
            })
        })
        .map_err(|e| format!("query bot_sessions 失败：{e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("read row 失败：{e}"))?);
    }
    Ok(out)
}

/// 从 session 列表采样 N 条（按 updated_at DESC 已排序；前 N 条）
pub fn sample_sessions(sessions: &[SessionRow], n: usize) -> Vec<SessionRow> {
    sessions.iter().take(n).cloned().collect()
}

/// 把 sample 转换为 EvalCase
///
/// 每条 case 用 session id 前 8 字符 + 序号作 case_id；
/// input = session title；
/// expected_behavior = ["chat_complete"]（基线）；
/// metrics = 全部五个等权。
pub fn sessions_to_cases(sessions: &[SessionRow]) -> Vec<EvalCase> {
    sessions
        .iter()
        .enumerate()
        .map(|(i, s)| EvalCase {
            case_id: format!("sess-{}-{:04}", &s.id[..8.min(s.id.len())], i),
            source_session_id: Some(s.id.clone()),
            source_test: None,
            input: s.title.clone(),
            expected_behavior: vec!["chat_complete".into()],
            metrics: vec![
                MetricSpec {
                    metric: "task_success_rate".into(),
                    weight: 0.3,
                },
                MetricSpec {
                    metric: "tool_call_efficiency".into(),
                    weight: 0.2,
                },
                MetricSpec {
                    metric: "behavior_deviation".into(),
                    weight: 0.2,
                },
                MetricSpec {
                    metric: "rollback_rate".into(),
                    weight: 0.15,
                },
                MetricSpec {
                    metric: "pollution_survival_days".into(),
                    weight: 0.15,
                },
            ],
            tags: vec!["session-derived".into()],
        })
        .collect()
}

/// 从 #[test] 函数名派生 case
///
/// 把 test_name 形式化为 case_id、input（保留原始 test_name）；
/// tags 加 "test-derived"。
pub fn test_name_to_case(test_name: &str, index: usize) -> EvalCase {
    EvalCase {
        case_id: format!("test-{:04}-{}", index, test_name),
        source_session_id: None,
        source_test: Some(test_name.into()),
        input: test_name.into(),
        expected_behavior: vec!["test_passes".into()],
        metrics: vec![
            MetricSpec {
                metric: "task_success_rate".into(),
                weight: 0.5,
            },
            MetricSpec {
                metric: "behavior_deviation".into(),
                weight: 0.5,
            },
        ],
        tags: vec!["test-derived".into()],
    }
}

/// 把 sample 结果追加到 eval_set.jsonl
pub fn append_to_set(path: &std::path::Path, cases: &[EvalCase]) -> Result<(), String> {
    append_jsonl(path, cases)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_session(id: &str, title: &str, updated_at: i64) -> SessionRow {
        SessionRow {
            id: id.into(),
            title: title.into(),
            updated_at,
        }
    }

    #[test]
    fn sample_takes_n_most_recent() {
        let s = vec![
            mk_session("a", "t1", 3000),
            mk_session("b", "t2", 2000),
            mk_session("c", "t3", 1000),
        ];
        let n = sample_sessions(&s, 2);
        assert_eq!(n.len(), 2);
        assert_eq!(n[0].id, "a");
        assert_eq!(n[1].id, "b");
    }

    #[test]
    fn sessions_to_cases_assigns_metrics() {
        let s = vec![mk_session("abcdefgh-1234", "测试 session", 60_0000)];
        let cases = sessions_to_cases(&s);
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].case_id, "sess-abcdefgh-0000");
        assert_eq!(cases[0].input, "测试 session");
        assert_eq!(cases[0].metrics.len(), 5);
    }

    #[test]
    fn test_derived_case_shape() {
        let c = test_name_to_case("apply_one_inserts_lesson_with_evolution_key", 7);
        assert_eq!(
            c.source_test.as_deref(),
            Some("apply_one_inserts_lesson_with_evolution_key")
        );
        assert_eq!(
            c.case_id,
            "test-0007-apply_one_inserts_lesson_with_evolution_key"
        );
        assert!(c.tags.contains(&"test-derived".to_string()));
    }

    #[test]
    fn list_sessions_missing_table_returns_empty() {
        let conn = Connection::open_in_memory().unwrap();
        // 没建 bot_sessions 表
        let r = list_sessions(&conn).unwrap();
        assert!(r.is_empty());
    }

    #[test]
    fn list_sessions_reads_when_table_exists() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE bot_sessions (id TEXT, title TEXT, updated_at INTEGER);
             INSERT INTO bot_sessions VALUES ('id1', 'title1', 1000);
             INSERT INTO bot_sessions VALUES ('id2', 'title2', 2000);",
        )
        .unwrap();
        let r = list_sessions(&conn).unwrap();
        assert_eq!(r.len(), 2);
        // 按 updated_at DESC 排序 → id2 在前
        assert_eq!(r[0].id, "id2");
    }
}
