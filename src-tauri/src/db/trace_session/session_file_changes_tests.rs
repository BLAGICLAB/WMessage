use super::*;
use crate::db::trace::{
    file_change_insert, trace_start, FileChangeRow, NewFileChange, NewTrace, EXEC_TRACE_DDL,
};
use std::fs;
use std::path::PathBuf;

fn sha(s: &str) -> String {
    crate::bot::sha256_hex(s)
}

/// 造一条 file_changes 行（字段给全，测试不依赖真实落盘路径）
fn mk_row(
    id: i64,
    path: &str,
    kind: &str,
    before_ref: Option<&str>,
    before_sha: Option<&str>,
    after_sha: Option<&str>,
) -> FileChangeRow {
    FileChangeRow {
        id,
        trace_id: 1,
        span_id: None,
        path: path.to_string(),
        kind: kind.to_string(),
        added: 0,
        deleted: 0,
        diff: None,
        truncated: false,
        before_ref: before_ref.map(str::to_string),
        before_sha: before_sha.map(str::to_string),
        after_sha: after_sha.map(str::to_string),
        created_at: 1000 + id,
    }
}

/// 临时工作区：checkpoints 目录 + 一个目标文件路径（文件内容由各测试自己写）
fn workspace(tag: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let ck = dir.path().join("checkpoints");
    fs::create_dir_all(&ck).unwrap();
    let target = dir.path().join("x.txt");
    let _ = tag;
    (dir, ck, target)
}

/// 累计口径：基线（第一次改前的快照）vs 现在 → 给净变化，不是最后一次
#[test]
fn session_mode_diffs_against_session_start() {
    let (_d, ck, target) = workspace("acc");
    let before = "a\nb\nc\n";
    fs::write(ck.join("snap1"), before).unwrap();
    let now = "a\nb2\nc\n";
    fs::write(&target, now).unwrap();

    let mut rows = vec![mk_row(
        1,
        target.to_str().unwrap(),
        "modify",
        Some("snap1"),
        Some(&sha(before)),
        Some(&sha(now)),
    )];
    // 第二次改动：只给累计口径下会被折叠掉的干扰项
    rows.push(mk_row(
        2,
        target.to_str().unwrap(),
        "modify",
        Some("snap2"),
        None,
        Some(&sha(now)),
    ));

    let s = summarize_path(
        target.to_str().unwrap(),
        &rows,
        read_before_snapshot(&rows, &ck),
        read_current_file(target.to_str().unwrap()),
    );
    assert_eq!(s.mode, DiffMode::Session);
    assert_eq!(s.added, 1, "累计 + 行：b → b2");
    assert_eq!(s.deleted, 1, "累计 − 行：b → b2");
    assert_eq!(s.changes, 2);
    assert!(!s.drifted, "after_sha 与盘上内容一致 → 不算漂移");
    let diff = s.diff.unwrap();
    assert!(
        diff.contains("-b"),
        "累计 diff 必须含被替换掉的行：\n{diff}"
    );
    assert!(diff.contains("+b2"), "累计 diff 必须含新行：\n{diff}");
    assert!(!diff.contains("c\n"), "未改动的行不该整段进 diff：\n{diff}");
}

/// 新建文件：会话开始前文件不存在 → 基线是空串，累计 diff = 整个文件都是新增
#[test]
fn create_file_session_mode_treats_baseline_as_empty() {
    let (_d, ck, target) = workspace("create");
    let now = "x\ny\n";
    fs::write(&target, now).unwrap();
    let rows = vec![mk_row(
        1,
        target.to_str().unwrap(),
        "create",
        None,
        None,
        Some(&sha(now)),
    )];
    let s = summarize_path(
        target.to_str().unwrap(),
        &rows,
        read_before_snapshot(&rows, &ck),
        read_current_file(target.to_str().unwrap()),
    );
    assert_eq!(s.mode, DiffMode::Session);
    assert_eq!(s.added, 2);
    assert_eq!(s.deleted, 0);
    assert!(s.diff.unwrap().contains("+x"));
}

/// 起始快照缺失（写盘时快照失败降级）：给不出累计 → 降级逐次并说明原因
#[test]
fn missing_snapshot_degrades_to_per_change_with_reason() {
    let (_d, ck, target) = workspace("nosnap");
    fs::write(&target, "now\n").unwrap();
    let mut rows = vec![mk_row(
        1,
        target.to_str().unwrap(),
        "modify",
        None,
        None,
        None,
    )];
    rows[0].added = 3;
    rows[0].deleted = 1;
    let s = summarize_path(
        target.to_str().unwrap(),
        &rows,
        read_before_snapshot(&rows, &ck),
        read_current_file(target.to_str().unwrap()),
    );
    assert_eq!(s.mode, DiffMode::PerChange);
    assert_eq!(s.added, 3, "降级时行数是各次之和");
    assert_eq!(s.deleted, 1);
    let note = s.note.unwrap();
    assert!(note.contains("快照缺失"), "降级必须说明原因：{note}");
    assert!(note.contains("逐次改动"), "降级必须说清给的是什么：{note}");
}

/// 起始快照指纹不符（已损坏）：不能拿它当基线——否则累计 diff 是错的且看不出来
#[test]
fn corrupted_snapshot_degrades_instead_of_producing_wrong_diff() {
    let (_d, ck, target) = workspace("corrupt");
    fs::write(ck.join("snap1"), "tampered\n").unwrap();
    fs::write(&target, "a\nb\n").unwrap();
    let rows = vec![mk_row(
        1,
        target.to_str().unwrap(),
        "modify",
        Some("snap1"),
        Some(&sha("original\n")),
        Some(&sha("a\nb\n")),
    )];
    let s = summarize_path(
        target.to_str().unwrap(),
        &rows,
        read_before_snapshot(&rows, &ck),
        read_current_file(target.to_str().unwrap()),
    );
    assert_eq!(s.mode, DiffMode::PerChange);
    assert!(
        s.note.unwrap().contains("指纹不符"),
        "快照损坏必须说明原因，不能静默给出错误的累计 diff"
    );
}

/// 二进制文件：没有行级 diff → 不给空 diff 面板，明说并指路
#[test]
fn binary_file_has_no_line_diff() {
    let (_d, ck, target) = workspace("bin");
    fs::write(&target, b"\x00\x01\x02binary").unwrap();
    let rows = vec![mk_row(
        1,
        target.to_str().unwrap(),
        "modify",
        None,
        None,
        Some(&sha("x")),
    )];
    let s = summarize_path(
        target.to_str().unwrap(),
        &rows,
        read_before_snapshot(&rows, &ck),
        read_current_file(target.to_str().unwrap()),
    );
    assert_eq!(s.mode, DiffMode::NoLineDiff);
    assert_eq!(s.added, 0);
    assert_eq!(s.deleted, 0);
    assert!(s.note.unwrap().contains("二进制"));
}

/// AI 写完后文件又被改过：累计 diff 含非 AI 的改动 → 必须标出来
#[test]
fn drift_after_last_ai_write_is_flagged() {
    let (_d, ck, target) = workspace("drift");
    let before = "a\n";
    fs::write(ck.join("snap1"), before).unwrap();
    let now = "a\nuser edit\n";
    fs::write(&target, now).unwrap();
    let rows = vec![mk_row(
        1,
        target.to_str().unwrap(),
        "modify",
        Some("snap1"),
        Some(&sha(before)),
        Some(&sha("a\n")),
    )];
    let s = summarize_path(
        target.to_str().unwrap(),
        &rows,
        read_before_snapshot(&rows, &ck),
        read_current_file(target.to_str().unwrap()),
    );
    assert_eq!(s.mode, DiffMode::Session);
    assert!(s.drifted, "盘上内容 ≠ AI 最后一次写盘的内容 → 必须标漂移");
    assert!(s.note.unwrap().contains("非 AI"));
}

/// 文件已不在（被用户删了）：现状读不到 → 降级逐次
#[test]
fn missing_current_file_degrades_to_per_change() {
    let (_d, ck, target) = workspace("gone");
    // 起始快照在（隔离出「现状缺失」这一个原因），但目标文件已被删
    fs::write(ck.join("snap1"), "a\n").unwrap();
    let rows = vec![mk_row(
        1,
        target.to_str().unwrap(),
        "modify",
        Some("snap1"),
        Some(&sha("a\n")),
        Some(&sha("a\n")),
    )];
    let s = summarize_path(
        target.to_str().unwrap(),
        &rows,
        read_before_snapshot(&rows, &ck),
        read_current_file(target.to_str().unwrap()),
    );
    assert_eq!(s.mode, DiffMode::PerChange);
    assert!(s.note.unwrap().contains("读不到当前文件"));
}

/// 会话隔离：别的会话的改动不混进来
#[test]
fn file_changes_for_session_is_scoped_by_session() {
    let conn = super::super::test_conn(&[EXEC_TRACE_DDL]);
    let t1 = trace_start(
        &conn,
        &NewTrace {
            session_id: "s1",
            task_id: None,
            origin: "Chat",
            title: None,
            started_at: 1000,
        },
    )
    .unwrap();
    let t2 = trace_start(
        &conn,
        &NewTrace {
            session_id: "s2",
            task_id: None,
            origin: "Chat",
            title: None,
            started_at: 1000,
        },
    )
    .unwrap();
    for (trace_id, path) in [(t1, "/a/x.py"), (t2, "/b/y.py")] {
        file_change_insert(
            &conn,
            &NewFileChange {
                trace_id,
                span_id: None,
                path,
                kind: "modify",
                added: 1,
                deleted: 0,
                diff: None,
                truncated: false,
                before_ref: None,
                before_sha: None,
                after_sha: None,
                created_at: 1001,
            },
        )
        .unwrap();
    }
    let rows = file_changes_for_session(&conn, "s1").unwrap();
    assert_eq!(rows.len(), 1, "只该看到本会话的改动");
    assert_eq!(rows[0].path, "/a/x.py");
}

/// 按路径分组：保持首次出现顺序，组内保持落盘先后
#[test]
fn group_by_path_keeps_first_appearance_order() {
    let rows = vec![
        mk_row(1, "/a", "modify", None, None, None),
        mk_row(2, "/b", "modify", None, None, None),
        mk_row(3, "/a", "modify", None, None, None),
    ];
    let groups = group_by_path(rows);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].0, "/a");
    assert_eq!(groups[0].1.len(), 2, "同路径的多次改动要归到一组");
    assert_eq!(groups[0].1[0].id, 1);
    assert_eq!(groups[0].1[1].id, 3, "组内保持落盘先后");
    assert_eq!(groups[1].0, "/b");
}
