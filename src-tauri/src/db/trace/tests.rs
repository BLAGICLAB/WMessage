use super::*;

fn setup_conn() -> rusqlite::Connection {
    super::super::test_conn(&[EXEC_TRACE_DDL])
}

fn mk_trace<'a>(
    conn: &'a rusqlite::Connection,
    session: &'a str,
    task: Option<&'a str>,
    origin: &'a str,
    started_at: i64,
) -> i64 {
    trace_start(
        conn,
        &NewTrace {
            session_id: session,
            task_id: task,
            origin,
            title: Some("测试卡"),
            started_at,
        },
    )
    .unwrap()
}

/// 建表幂等：EXEC_TRACE_DDL 连跑两遍不炸（open_db 每次启动都跑）
#[test]
fn ddl_is_idempotent() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    ensure_trace_tables(&conn).unwrap();
    ensure_trace_tables(&conn).unwrap();
}

/// 索引存在（保留期扫描列 + 三表查询列）
#[test]
fn indexes_exist() {
    let conn = setup_conn();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name IN
             ('idx_traces_task','idx_traces_session','idx_traces_finished','idx_spans_trace','idx_changes_trace','idx_changes_path')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 6, "6 个索引必须全部建出");
}

/// start→finish 往返：files_changed 从 file_changes 反计，不由调用方传
#[test]
fn trace_start_finish_roundtrip_counts_files_from_table() {
    let conn = setup_conn();
    let id = mk_trace(&conn, "s1", Some("t1"), "Manual", 1000);
    file_change_insert(
        &conn,
        &NewFileChange {
            trace_id: id,
            span_id: None,
            path: "/a/x.py",
            kind: "modify",
            added: 3,
            deleted: 1,
            diff: None,
            truncated: false,
            before_ref: None,
            before_sha: None,
            after_sha: None,
            created_at: 1001,
        },
    )
    .unwrap();
    trace_finish(
        &conn,
        id,
        &TraceFinish {
            status: TRACE_STATUS_DONE,
            finished_at: 2000,
            turn_count: 5,
            tool_calls: 8,
            prompt_tokens: 100,
            completion_tokens: 200,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            error: None,
            model: None,
        },
    )
    .unwrap();
    let t = trace_get_row(&conn, id).unwrap().unwrap();
    assert_eq!(t.status, "done");
    assert_eq!(t.finished_at, Some(2000));
    assert_eq!(t.turn_count, 5);
    assert_eq!(t.tool_calls, 8);
    assert_eq!(
        t.files_changed, 1,
        "files_changed 必须从 file_changes 表反计"
    );
    assert_eq!(t.prompt_tokens, 100);
    assert_eq!(t.error, None);
}

/// trace_get 查无此行 → None（不 Err，前端 404 语义留给命令层）
#[test]
fn trace_get_missing_returns_none() {
    let conn = setup_conn();
    assert!(trace_get_row(&conn, 999).unwrap().is_none());
}

/// span 往返：字段全量保真（ok/error_class/duration_ms）
#[test]
fn span_insert_roundtrip() {
    let conn = setup_conn();
    let id = mk_trace(&conn, "s1", None, "Scheduled", 1000);
    let span_id = span_insert(
        &conn,
        &NewSpan {
            trace_id: id,
            turn: 2,
            tool_call_id: Some("call_abc"),
            name: "edit_file",
            args: Some("{\"path\":\"/a/x.py\"}"),
            result: Some("已修改"),
            ok: false,
            error_class: Some("fs_not_found"),
            duration_ms: Some(123),
            created_at: 1001,
        },
    )
    .unwrap();
    let spans = spans_for_trace(&conn, id).unwrap();
    assert_eq!(spans.len(), 1);
    let s = &spans[0];
    assert_eq!(s.id, span_id);
    assert_eq!(s.turn, 2);
    assert_eq!(s.tool_call_id.as_deref(), Some("call_abc"));
    assert_eq!(s.name, "edit_file");
    assert!(!s.ok);
    assert_eq!(s.error_class.as_deref(), Some("fs_not_found"));
    assert_eq!(s.duration_ms, Some(123));
    // 行序 = id 序（时间线渲染依赖）
    span_insert(
        &conn,
        &NewSpan {
            trace_id: id,
            turn: 3,
            tool_call_id: None,
            name: "web_search",
            args: None,
            result: None,
            ok: true,
            error_class: None,
            duration_ms: None,
            created_at: 1002,
        },
    )
    .unwrap();
    let spans = spans_for_trace(&conn, id).unwrap();
    assert!(spans[0].id < spans[1].id);
}

/// clamp_text：按字节钳 + UTF-8 边界回退（中文多字节不拦腰）
#[test]
fn clamp_text_respects_char_boundary() {
    let short = "abc";
    assert_eq!(clamp_text(short, 100), "abc", "未超限原样返回");
    // 8 字节上限：三个 3 字节汉字只能装下两个
    let zh = "一二三四";
    let clamped = clamp_text(zh, 8);
    assert_eq!(clamped, "一二");
    assert!(clamped.len() <= 8);
    // 全 ASCII 精确钳
    assert_eq!(clamp_text("abcdefghij", 4), "abcd");
}

/// file_change 往返：before/after 证据链字段全量保真
#[test]
fn file_change_roundtrip_with_rollback_evidence() {
    let conn = setup_conn();
    let id = mk_trace(&conn, "s1", None, "Workflow", 1000);
    file_change_insert(
        &conn,
        &NewFileChange {
            trace_id: id,
            span_id: Some(7),
            path: "/a/x.py",
            kind: "modify",
            added: 12,
            deleted: 4,
            diff: Some("--- a/x.py\n+++ b/x.py\n@@ -1,3 +1,4 @@"),
            truncated: true,
            before_ref: Some("42"),
            before_sha: Some("aaa"),
            after_sha: Some("bbb"),
            created_at: 1001,
        },
    )
    .unwrap();
    let changes = file_changes_for_trace(&conn, id).unwrap();
    assert_eq!(changes.len(), 1);
    let c = &changes[0];
    assert_eq!(c.span_id, Some(7));
    assert_eq!(c.kind, "modify");
    assert_eq!(c.added, 12);
    assert_eq!(c.deleted, 4);
    assert_eq!(
        c.diff.as_deref(),
        Some("--- a/x.py\n+++ b/x.py\n@@ -1,3 +1,4 @@")
    );
    assert!(c.truncated);
    assert_eq!(c.before_ref.as_deref(), Some("42"));
    assert_eq!(c.before_sha.as_deref(), Some("aaa"));
    assert_eq!(c.after_sha.as_deref(), Some("bbb"));
}

/// trace_list：过滤组合 + started_at 倒序 + limit 钳制
#[test]
fn trace_list_filters_order_and_limit() {
    let conn = setup_conn();
    mk_trace(&conn, "s1", Some("t1"), "Manual", 1000);
    mk_trace(&conn, "s1", Some("t1"), "Manual", 3000);
    mk_trace(&conn, "s2", Some("t1"), "Scheduled", 2000);
    mk_trace(&conn, "s2", Some("t2"), "Workflow", 4000);

    // started_at 倒序
    let all = trace_query(&conn, None, None, None, 100).unwrap();
    let starts: Vec<i64> = all.iter().map(|t| t.started_at).collect();
    let mut sorted = starts.clone();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(starts, sorted, "必须 started_at 倒序");

    // origin 过滤
    let sched = trace_query(&conn, None, None, Some("Scheduled"), 100).unwrap();
    assert_eq!(sched.len(), 1);
    assert_eq!(sched[0].session_id, "s2");
    assert_eq!(sched[0].origin, "Scheduled");

    // task + origin 组合过滤
    let t2_wf = trace_query(&conn, Some("t2"), None, Some("Workflow"), 100).unwrap();
    assert_eq!(t2_wf.len(), 1);
    assert_eq!(t2_wf[0].task_id.as_deref(), Some("t2"));

    // session 过滤
    let s1 = trace_query(&conn, None, Some("s1"), None, 100).unwrap();
    assert_eq!(s1.len(), 2);

    // limit 钳制：请求 0 → 至少给 1；请求超量 → 全量
    assert_eq!(trace_query(&conn, None, None, None, 0).unwrap().len(), 1);
    assert_eq!(trace_query(&conn, None, None, None, 2).unwrap().len(), 2);
    assert_eq!(trace_query(&conn, None, None, None, 9999).unwrap().len(), 4);
}

/// 保留期：只删「已收尾且过期」与「僵尸 running」，未过期/活跃行保留；子表级联删
#[test]
fn retire_traces_before_cascades_and_keeps_live_rows() {
    let conn = setup_conn();
    // ① 已收尾、过期（finished_at=100 < 5000）→ 删
    let old = mk_trace(&conn, "s1", None, "Manual", 50);
    span_insert(
        &conn,
        &NewSpan {
            trace_id: old,
            turn: 1,
            tool_call_id: None,
            name: "edit_file",
            args: None,
            result: None,
            ok: true,
            error_class: None,
            duration_ms: None,
            created_at: 60,
        },
    )
    .unwrap();
    file_change_insert(
        &conn,
        &NewFileChange {
            trace_id: old,
            span_id: None,
            path: "/a",
            kind: "create",
            added: 1,
            deleted: 0,
            diff: None,
            truncated: false,
            before_ref: None,
            before_sha: None,
            after_sha: None,
            created_at: 61,
        },
    )
    .unwrap();
    trace_finish(
        &conn,
        old,
        &TraceFinish {
            status: TRACE_STATUS_DONE,
            finished_at: 100,
            turn_count: 1,
            tool_calls: 1,
            prompt_tokens: 0,
            completion_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            error: None,
            model: None,
        },
    )
    .unwrap();
    // ② 已收尾、未过期（finished_at=9000）→ 留
    let fresh = mk_trace(&conn, "s2", None, "Manual", 8000);
    trace_finish(
        &conn,
        fresh,
        &TraceFinish {
            status: TRACE_STATUS_DONE,
            finished_at: 9000,
            turn_count: 1,
            tool_calls: 1,
            prompt_tokens: 0,
            completion_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            error: None,
            model: None,
        },
    )
    .unwrap();
    // ③ 僵尸 running（started_at=50 < 5000，永远等不到收尾）→ 删
    mk_trace(&conn, "s3", None, "Batch", 50);
    // ④ 活跃 running（started_at=8000）→ 留
    mk_trace(&conn, "s4", None, "Batch", 8000);

    let removed = retire_traces_before(&conn, 5000).unwrap();
    assert_eq!(removed, 2, "过期收尾 + 僵尸 running 各删一条");

    assert!(
        trace_get_row(&conn, old).unwrap().is_none(),
        "过期收尾必须删"
    );
    assert!(
        spans_for_trace(&conn, old).unwrap().is_empty(),
        "子表 spans 必须级联删"
    );
    assert!(
        file_changes_for_trace(&conn, old).unwrap().is_empty(),
        "子表 file_changes 必须级联删"
    );
    assert!(
        trace_get_row(&conn, fresh).unwrap().is_some(),
        "未过期必须留"
    );
    assert_eq!(trace_query(&conn, None, None, None, 100).unwrap().len(), 2);
}

/// 保留期截止时间戳：钳制与折算（手动命令与自动清扫共用同一函数，
/// 口径分叉会让公示的保留期与实际删除范围在边界对不上）
#[test]
fn retention_cutoff_ms_clamps_and_converts() {
    let now = 1_000_000_000_000i64;
    assert_eq!(retention_cutoff_ms(30, now), now - 30 * 86_400_000);
    // 越界钳到 1..=365：0/负数不解释成「删光」，999 也不放行
    assert_eq!(retention_cutoff_ms(0, now), now - 86_400_000);
    assert_eq!(retention_cutoff_ms(-7, now), now - 86_400_000);
    assert_eq!(retention_cutoff_ms(999, now), now - 365 * 86_400_000);
}

/// 词元统计窗口：None = 上界（前端不写死天数，要多少给多少），
/// Some(n) 钳到 1..=上界。三个命令共用这一份口径。
#[test]
fn usage_window_days_resolves_to_backend_bound() {
    assert_eq!(resolve_usage_window_days(None), USAGE_STATS_MAX_DAYS);
    assert_eq!(resolve_usage_window_days(Some(30)), 30);
    assert_eq!(resolve_usage_window_days(Some(0)), 1);
    assert_eq!(resolve_usage_window_days(Some(-5)), 1);
    assert_eq!(resolve_usage_window_days(Some(9999)), USAGE_STATS_MAX_DAYS);
}

/// 自动清扫必须真接在启动路径上，且走与手动命令同一个内核——
/// 「公示 30 天却没人删」正是本条要修的谎，只测函数存在等于没测接线。
/// 接线只能在 lib.rs 的 setup 里（AppHandle 型命令无法在单测里驱动），
/// 故以源码文本钉住，局限同 dispatch 的入参闸守卫。
#[test]
fn retention_sweep_is_wired_into_app_startup() {
    assert_eq!(
        TRACE_RETENTION_DAYS, 30,
        "设置页公示的默认保留期，改它要连提示文案一起改"
    );
    let lib_src = include_str!("../../lib.rs");
    assert!(
        lib_src.contains("db::trace::spawn_retention_sweep"),
        "启动路径必须挂上保留期清扫，否则公示的保留期无人执行"
    );
}

/// 旧库迁移：缓存 token 两列（成本统计的 cache read/write 计价档）必须补齐，
/// 且补完可正常写入读取——漏 ADD COLUMN 的话 trace_finish 的 UPDATE 直接报错。
#[test]
fn ensure_trace_tables_migrates_cache_token_columns_idempotent() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    // 旧 schema：两列都缺（比 model 测试更旧的库形态）
    conn.execute_batch(
        "CREATE TABLE exec_traces (
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           session_id TEXT NOT NULL, task_id TEXT, origin TEXT NOT NULL, title TEXT,
           status TEXT NOT NULL DEFAULT 'running', started_at INTEGER NOT NULL,
           finished_at INTEGER, turn_count INTEGER NOT NULL DEFAULT 0,
           tool_calls INTEGER NOT NULL DEFAULT 0, files_changed INTEGER NOT NULL DEFAULT 0,
           prompt_tokens INTEGER NOT NULL DEFAULT 0, completion_tokens INTEGER NOT NULL DEFAULT 0,
           error TEXT, model TEXT);",
    )
    .unwrap();
    ensure_trace_tables(&conn).unwrap();
    for col in ["cache_read_tokens", "cache_write_tokens"] {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('exec_traces') WHERE name = ?1",
                [col],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "旧库必须补出 {col} 列");
    }
    ensure_trace_tables(&conn).unwrap(); // 幂等：再跑一遍不炸不加

    // 迁移后写入读取闭环：缓存档数字要能原样读回，且不污染 prompt/completion
    let id = mk_trace(&conn, "s1", None, "chat", 1000);
    trace_finish(
        &conn,
        id,
        &TraceFinish {
            status: TRACE_STATUS_DONE,
            finished_at: 2000,
            turn_count: 3,
            tool_calls: 0,
            prompt_tokens: 10,
            completion_tokens: 20,
            cache_read_tokens: 40_000,
            cache_write_tokens: 5_000,
            error: None,
            model: Some("glm-5.3"),
        },
    )
    .unwrap();
    let row = trace_query(&conn, None, None, None, 10).unwrap().remove(0);
    assert_eq!(row.cache_read_tokens, 40_000);
    assert_eq!(row.cache_write_tokens, 5_000);
    assert_eq!(
        row.prompt_tokens, 10,
        "缓存 token 必须独立记账，混进 prompt_tokens 就等于把缓存收益算没"
    );
    assert_eq!(row.completion_tokens, 20);
}

/// 并发补列不得报错：`open_db` 每次调用都跑 ensure_trace_tables，而首装时
/// setup / 僵尸清扫 / 首条命令各开一个连接，两个连接会同时探测到「列不存在」
/// 再同时 ALTER。回归形态：日志里那条
/// `subagent_zombie_sweep_db_fail | err: duplicate column name: cache_read_tokens`
/// ——它让整个僵尸清扫提前返回（return），上一世的 running 行收不了尾。
#[test]
fn ensure_trace_tables_tolerates_concurrent_add_column() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE exec_traces (
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           session_id TEXT NOT NULL, task_id TEXT, origin TEXT NOT NULL, title TEXT,
           status TEXT NOT NULL DEFAULT 'running', started_at INTEGER NOT NULL,
           finished_at INTEGER, turn_count INTEGER NOT NULL DEFAULT 0,
           tool_calls INTEGER NOT NULL DEFAULT 0, files_changed INTEGER NOT NULL DEFAULT 0,
           prompt_tokens INTEGER NOT NULL DEFAULT 0, completion_tokens INTEGER NOT NULL DEFAULT 0,
           error TEXT, model TEXT);",
    )
    .unwrap();
    // 模拟「另一连接刚把列加上」：直接 ALTER 制造竞态后的既有状态
    conn.execute_batch(
        "ALTER TABLE exec_traces ADD COLUMN cache_read_tokens INTEGER NOT NULL DEFAULT 0",
    )
    .unwrap();
    // 再跑 ensure：必须在已存在列的情况下正常返回，不能 Err
    ensure_trace_tables(&conn).expect("列已存在时 ensure 必须幂等成功");
    for col in ["model", "cache_read_tokens", "cache_write_tokens"] {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('exec_traces') WHERE name = ?1",
                [col],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "{col} 列应恰好存在一次");
    }
}

/// 保留期空转：无可删行返回 0，不 Err（定时器空跑是常态路径）
#[test]
fn retire_traces_before_noop_returns_zero() {
    let conn = setup_conn();
    mk_trace(&conn, "s1", None, "Manual", 8000);
    assert_eq!(retire_traces_before(&conn, 5000).unwrap(), 0);
    assert_eq!(trace_query(&conn, None, None, None, 100).unwrap().len(), 1);
}

/// 旧库迁移：无 model 列的 exec_traces（升级前建的库）经 ensure_trace_tables
/// 补齐 model 列；新库（DDL 已含列）跑第二遍不重复加（幂等）
#[test]
fn ensure_trace_tables_migrates_model_column_idempotent() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    // 旧 schema：手工建不含 model 的表（模拟升级前的库）
    conn.execute_batch(
        "CREATE TABLE exec_traces (
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           session_id TEXT NOT NULL, task_id TEXT, origin TEXT NOT NULL, title TEXT,
           status TEXT NOT NULL DEFAULT 'running', started_at INTEGER NOT NULL,
           finished_at INTEGER, turn_count INTEGER NOT NULL DEFAULT 0,
           tool_calls INTEGER NOT NULL DEFAULT 0, files_changed INTEGER NOT NULL DEFAULT 0,
           prompt_tokens INTEGER NOT NULL DEFAULT 0, completion_tokens INTEGER NOT NULL DEFAULT 0,
           error TEXT);",
    )
    .unwrap();
    ensure_trace_tables(&conn).unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('exec_traces') WHERE name = 'model'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1, "旧库必须补出 model 列");
    ensure_trace_tables(&conn).unwrap(); // 幂等：再跑一遍不炸不加
                                         // 迁移后可正常写入读取
    let id = mk_trace(&conn, "s1", None, "chat", 1000);
    trace_finish(
        &conn,
        id,
        &TraceFinish {
            status: TRACE_STATUS_DONE,
            finished_at: 2000,
            turn_count: 1,
            tool_calls: 0,
            prompt_tokens: 10,
            completion_tokens: 20,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            error: None,
            model: Some("glm-5.3"),
        },
    )
    .unwrap();
    let t = trace_get_row(&conn, id).unwrap().unwrap();
    assert_eq!(t.model.as_deref(), Some("glm-5.3"), "finish 必须写入模型名");
}

/// model COALESCE 语义：None 收尾保留原值（无→NULL），Some 覆盖
#[test]
fn trace_finish_model_coalesce() {
    let conn = setup_conn();
    let id = mk_trace(&conn, "s1", None, "chat", 1000);
    trace_finish(
        &conn,
        id,
        &TraceFinish {
            status: TRACE_STATUS_FAILED,
            finished_at: 1100,
            turn_count: 0,
            tool_calls: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            error: Some("boom"),
            model: None,
        },
    )
    .unwrap();
    assert_eq!(
        trace_get_row(&conn, id).unwrap().unwrap().model,
        None,
        "None 收尾不写模型"
    );
}

/// 缓存 token 必须独立聚合、且不混进 prompt/completion。
///
/// 成本口径的依据：Anthropic 四档计价里 cache read 是 0.1x、cache write 是 1.25x，
/// 与普通 input（1x）不同价。把它折进 prompt_tokens 会让长会话的缓存收益
/// 显示不出来——而长会话恰恰是缓存收益最大、也最该看见的场景。
#[test]
fn usage_stats_sums_cache_tokens_separately() {
    let conn = setup_conn();
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap();
    let today_ms = today
        .and_hms_opt(4, 0, 0)
        .unwrap()
        .and_local_timezone(chrono::Local)
        .earliest()
        .unwrap()
        .timestamp_millis();
    for (i, (cr, cw)) in [(40_000i64, 5_000i64), (60_000, 8_000)].iter().enumerate() {
        let id = mk_trace(
            &conn,
            &format!("s{i}"),
            None,
            "chat",
            today_ms + i as i64 * 100,
        );
        trace_finish(
            &conn,
            id,
            &TraceFinish {
                status: TRACE_STATUS_DONE,
                finished_at: today_ms + 1000,
                turn_count: 1,
                tool_calls: 0,
                prompt_tokens: 10,
                completion_tokens: 20,
                cache_read_tokens: *cr,
                cache_write_tokens: *cw,
                error: None,
                model: Some("glm-5.3"),
            },
        )
        .unwrap();
    }
    let days = usage_stats_daily_conn(&conn, 1, today).unwrap();
    let d = &days[0];
    assert_eq!(d.cache_read_tokens, 100_000);
    assert_eq!(d.cache_write_tokens, 13_000);
    assert_eq!(
        d.prompt_tokens, 20,
        "缓存 token 不得并入 prompt_tokens（否则缓存收益被算没）"
    );
    assert_eq!(d.completion_tokens, 40);

    // 按模型聚合同样要带上（否则「模型用量」榜的量是漏的）
    let by_model = usage_stats_by_model_conn(&conn, 1, today).unwrap();
    assert_eq!(by_model.len(), 1);
    assert_eq!(by_model[0].cache_read_tokens, 100_000);
    assert_eq!(by_model[0].cache_write_tokens, 13_000);

    // 模型×日期交叉聚合（趋势图分线数据源）同样要带
    let dm = usage_stats_daily_by_model_conn(&conn, 1, today).unwrap();
    assert_eq!(dm.len(), 1);
    assert_eq!(dm[0].cache_read_tokens, 100_000);
    assert_eq!(dm[0].cache_write_tokens, 13_000);
}

/// 词元按日聚合：缺日补零 + 升序 + 僵尸 running 不计执行次数（token 计入）
#[test]
fn usage_stats_daily_zero_fills_and_counts_finished_only() {
    let conn = setup_conn();
    // 今天：一次完整执行（done）+ 一次僵尸 running——runs 只应算 1
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap();
    let today_ms = today
        .and_hms_opt(4, 0, 0)
        .unwrap()
        .and_local_timezone(chrono::Local)
        .earliest()
        .unwrap()
        .timestamp_millis();
    let id = mk_trace(&conn, "s1", None, "chat", today_ms);
    trace_finish(
        &conn,
        id,
        &TraceFinish {
            status: TRACE_STATUS_DONE,
            finished_at: today_ms + 100,
            turn_count: 2,
            tool_calls: 3,
            prompt_tokens: 1000,
            completion_tokens: 500,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            error: None,
            model: Some("glm-5.3"),
        },
    )
    .unwrap();
    // 僵尸：不 finish，手工点 prompt_tokens 模拟半路用量
    let zombie = mk_trace(&conn, "s2", None, "chat", today_ms + 200);
    conn.execute(
        "UPDATE exec_traces SET prompt_tokens = 100 WHERE id = ?1",
        [zombie],
    )
    .unwrap();
    // 昨天：一次 stopped 执行
    let y_ms = today_ms - 86_400_000;
    let yid = mk_trace(&conn, "s1", None, "manual", y_ms);
    trace_finish(
        &conn,
        yid,
        &TraceFinish {
            status: TRACE_STATUS_STOPPED,
            finished_at: y_ms + 50,
            turn_count: 1,
            tool_calls: 1,
            prompt_tokens: 200,
            completion_tokens: 300,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            error: None,
            model: None,
        },
    )
    .unwrap();

    let days = usage_stats_daily_conn(&conn, 7, today).unwrap();
    assert_eq!(days.len(), 7, "窗口 7 天必须逐日补零返回");
    assert_eq!(days.last().unwrap().day, "2026-10-06");
    assert_eq!(days[0].prompt_tokens, 0, "窗口外沿必须补零");
    let today_row = days.last().unwrap();
    assert_eq!(today_row.prompt_tokens, 1100, "token 含僵尸行");
    assert_eq!(today_row.completion_tokens, 500);
    assert_eq!(today_row.runs, 1, "僵尸 running 不算执行次数");
    let y_row = &days[days.len() - 2];
    assert_eq!((y_row.prompt_tokens, y_row.completion_tokens), (200, 300));
    assert_eq!(y_row.runs, 1, "stopped 也算已收尾");
}

/// 按模型聚合：分组求和 + 总 tokens 降序 + NULL model 参与分组（前端显示未知）
#[test]
fn usage_stats_by_model_groups_orders_and_keeps_null() {
    let conn = setup_conn();
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap();
    let t0 = today
        .and_hms_opt(4, 0, 0)
        .unwrap()
        .and_local_timezone(chrono::Local)
        .earliest()
        .unwrap()
        .timestamp_millis();
    let finish = |conn: &rusqlite::Connection, id: i64, p: i64, c: i64, model: Option<&str>| {
        trace_finish(
            conn,
            id,
            &TraceFinish {
                status: TRACE_STATUS_DONE,
                finished_at: t0 + 100,
                turn_count: 1,
                tool_calls: 0,
                prompt_tokens: p,
                completion_tokens: c,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                error: None,
                model,
            },
        )
        .unwrap();
    };
    finish(
        &conn,
        mk_trace(&conn, "s1", None, "chat", t0),
        100,
        100,
        Some("glm-5.3"),
    );
    finish(
        &conn,
        mk_trace(&conn, "s1", None, "chat", t0 + 1),
        400,
        400,
        Some("glm-5.3"),
    );
    finish(
        &conn,
        mk_trace(&conn, "s2", None, "manual", t0 + 2),
        300,
        100,
        Some("kimi-k3"),
    );
    finish(
        &conn,
        mk_trace(&conn, "s3", None, "manual", t0 + 3),
        50,
        50,
        None,
    );

    let rows = usage_stats_by_model_conn(&conn, 7, today).unwrap();
    assert_eq!(rows.len(), 3, "glm-5.3 / kimi-k3 / NULL 三组");
    assert_eq!(rows[0].model.as_deref(), Some("glm-5.3"));
    assert_eq!(
        (rows[0].prompt_tokens, rows[0].completion_tokens),
        (500, 500)
    );
    assert_eq!(rows[1].model.as_deref(), Some("kimi-k3"));
    assert_eq!(rows[2].model, None, "NULL 模型保留（前端显示未知）");
    assert!(
        rows[0].prompt_tokens + rows[0].completion_tokens
            >= rows[1].prompt_tokens + rows[1].completion_tokens,
        "必须按总 tokens 降序"
    );
}

/// 榜位排序口径 = 面板展示口径（输入+输出）。缓存不进排序：
/// 缓存读取单价约为普通输入的十分之一，把它算进名次会让「榜上第一」
/// 的模型在饼图扇形上明显偏小，名次与图形对不上。
#[test]
fn usage_stats_by_model_ranks_ignoring_cache_tokens() {
    let conn = setup_conn();
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap();
    let t0 = today
        .and_hms_opt(4, 0, 0)
        .unwrap()
        .and_local_timezone(chrono::Local)
        .earliest()
        .unwrap()
        .timestamp_millis();
    let finish =
        |conn: &rusqlite::Connection, id: i64, at: i64, p: i64, c: i64, cr: i64, model: &str| {
            trace_finish(
                conn,
                id,
                &TraceFinish {
                    status: TRACE_STATUS_DONE,
                    finished_at: at + 100,
                    turn_count: 1,
                    tool_calls: 0,
                    prompt_tokens: p,
                    completion_tokens: c,
                    cache_read_tokens: cr,
                    cache_write_tokens: 0,
                    error: None,
                    model: Some(model),
                },
            )
            .unwrap();
        };
    // 有缓存的大模型：展示口径 500，但含缓存合计 50_500
    finish(
        &conn,
        mk_trace(&conn, "s1", None, "chat", t0),
        t0,
        300,
        200,
        50_000,
        "cached",
    );
    // 无缓存的小模型：展示口径 800，含缓存合计也是 800
    finish(
        &conn,
        mk_trace(&conn, "s2", None, "chat", t0 + 1),
        t0 + 1,
        500,
        300,
        0,
        "plain",
    );

    let rows = usage_stats_by_model_conn(&conn, 7, today).unwrap();
    assert_eq!(
        rows[0].model.as_deref(),
        Some("plain"),
        "榜位按输入+输出，缓存不参与"
    );
    assert_eq!(rows[1].model.as_deref(), Some("cached"));
    assert_eq!(
        rows[1].cache_read_tokens, 50_000,
        "缓存数仍然照常返回，只是不参与排序"
    );
}

/// 按模型×按日聚合：day 升序、(day, model) 分组、NULL model 保留、缺日不返回（前端补零）
#[test]
fn usage_stats_daily_by_model_groups_and_orders() {
    let conn = setup_conn();
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap();
    let t0 = today
        .and_hms_opt(4, 0, 0)
        .unwrap()
        .and_local_timezone(chrono::Local)
        .earliest()
        .unwrap()
        .timestamp_millis();
    let finish = |conn: &rusqlite::Connection, id: i64, at: i64, p: i64, model: Option<&str>| {
        trace_finish(
            conn,
            id,
            &TraceFinish {
                status: TRACE_STATUS_DONE,
                finished_at: at + 100,
                turn_count: 1,
                tool_calls: 0,
                prompt_tokens: p,
                completion_tokens: p,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                error: None,
                model,
            },
        )
        .unwrap();
    };
    // 今天：glm-5.3 两行 + kimi 一行；昨天：NULL 模型一行
    finish(
        &conn,
        mk_trace(&conn, "s1", None, "chat", t0),
        t0,
        100,
        Some("glm-5.3"),
    );
    finish(
        &conn,
        mk_trace(&conn, "s1", None, "chat", t0 + 1),
        t0 + 1,
        200,
        Some("glm-5.3"),
    );
    finish(
        &conn,
        mk_trace(&conn, "s2", None, "chat", t0 + 2),
        t0 + 2,
        50,
        Some("kimi-k3"),
    );
    finish(
        &conn,
        mk_trace(&conn, "s3", None, "manual", t0 - 86_400_000),
        t0 - 86_400_000,
        30,
        None,
    );

    let rows = usage_stats_daily_by_model_conn(&conn, 7, today).unwrap();
    assert_eq!(
        rows.len(),
        3,
        "10-05 NULL + 10-06 两模型 = 3 行；缺日不补零"
    );
    let days: Vec<&str> = rows.iter().map(|r| r.day.as_str()).collect();
    assert_eq!(days, ["2026-10-05", "2026-10-06", "2026-10-06"], "day 升序");
    assert_eq!(rows[0].model, None, "NULL 模型排在同日最前（NULLs first）");
    assert_eq!((rows[0].prompt_tokens, rows[0].completion_tokens), (30, 30));
    assert_eq!(rows[1].model.as_deref(), Some("glm-5.3"));
    assert_eq!(
        (rows[1].prompt_tokens, rows[1].completion_tokens),
        (300, 300),
        "同日同模型求和"
    );
    assert_eq!(rows[2].model.as_deref(), Some("kimi-k3"));
}

/// 落盘凭证查询：只返回被工具写过的路径，大小写不敏感
#[test]
fn file_paths_with_receipt_returns_only_written_paths() {
    let conn = setup_conn();
    let tid = mk_trace(&conn, "s1", None, "chat", 1000);
    for p in ["/out/report.docx", "/out/sheet.xlsx"] {
        file_change_insert(
            &conn,
            &NewFileChange {
                trace_id: tid,
                span_id: None,
                path: p,
                kind: "create",
                added: 0,
                deleted: 0,
                diff: None,
                truncated: false,
                before_ref: None,
                before_sha: None,
                after_sha: Some("abc"),
                created_at: 1001,
            },
        )
        .unwrap();
    }
    let asked = vec![
        "/out/report.docx".to_string(),
        "/out/never_written.pdf".to_string(),
        "/OUT/SHEET.XLSX".to_string(),
    ];
    let hit = file_paths_with_receipt(&conn, &asked).unwrap();
    assert_eq!(
        hit.len(),
        2,
        "没被写过的路径不进结果；大小写不同仍算同一路径"
    );
    assert!(hit.iter().any(|h| h == "/out/report.docx"));
    assert!(
        hit.iter()
            .any(|h| h.eq_ignore_ascii_case("/out/sheet.xlsx")),
        "大小写归一后应命中表里那行：{:?}",
        hit
    );
}

/// 会话工具名单要分「调用过」与「成功过」两档
///
/// 两档分家是守卫的两道判据要的：提到某个工具时「调用过」就够（失败也是
/// 调过），声称它成功时只有「成功过」才作数。
#[test]
fn session_tool_names_splits_called_and_succeeded() {
    let conn = setup_conn();
    let tid = mk_trace(&conn, "s-tools", None, "chat", 1000);
    let add_span = |name: &str, ok: bool| {
        span_insert(
            &conn,
            &NewSpan {
                trace_id: tid,
                turn: 1,
                tool_call_id: None,
                name,
                args: None,
                result: None,
                ok,
                error_class: None,
                duration_ms: None,
                created_at: 1001,
            },
        )
        .unwrap();
    };
    add_span("write_file", true);
    add_span("complete_task", false);
    add_span("write_file", true);

    let st = session_tool_names(&conn, "s-tools").unwrap();
    assert!(
        st.called.iter().any(|n| n == "write_file")
            && st.called.iter().any(|n| n == "complete_task"),
        "调用过的一律要进 called（含失败的）：{:?}",
        st.called
    );
    assert!(
        st.succeeded.iter().any(|n| n == "write_file"),
        "成功过的要进 succeeded：{:?}",
        st.succeeded
    );
    assert!(
        !st.succeeded.iter().any(|n| n == "complete_task"),
        "失败的调用不算成功过：{:?}",
        st.succeeded
    );
    let other = session_tool_names(&conn, "s-absent").unwrap();
    assert!(
        other.called.is_empty() && other.succeeded.is_empty(),
        "没调过工具的会话两档都空：{:?}",
        other.called
    );
}
/// 凭证查询必须走 path 索引（守卫每次判定都查这道，痕迹表会一直长）
///
/// 索引建在 `path COLLATE NOCASE` 上，查询侧得同款写法才走得上去——
/// 写成 `path = ?1`（不带 COLLATE）就退化成全表扫，注释里那句「否则每次判定
/// 都要全表扫」就成了空话。这条测试把那句话钉住。
#[test]
fn file_paths_with_receipt_query_uses_path_index() {
    let conn = setup_conn();
    let mut st = conn
        .prepare(
            "EXPLAIN QUERY PLAN SELECT path FROM file_changes WHERE path = ?1 COLLATE NOCASE LIMIT 1",
        )
        .unwrap();
    let plan: Vec<String> = st
        .query_map(["/a/b.docx"], |r| r.get::<_, String>(3))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert!(
        plan.iter().any(|p| p.contains("idx_changes_path")),
        "凭证查询应走 path 索引，实际计划：{plan:?}"
    );
}

// ── 回滚 / 导出命令核心：conn + 数据目录齐全即可测（AppHandle 外壳在外）────

use std::fs;

fn tmp_data_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// 建一条变更行，kind 与指纹三件套可选注入（回滚闸口测试用）
fn mk_change(
    conn: &rusqlite::Connection,
    trace_id: i64,
    path: &str,
    kind: &str,
    before_ref: Option<&str>,
    before_sha: Option<&str>,
    after_sha: Option<&str>,
) -> i64 {
    file_change_insert(
        conn,
        &NewFileChange {
            trace_id,
            span_id: None,
            path,
            kind,
            added: 1,
            deleted: 1,
            diff: None,
            truncated: false,
            before_ref,
            before_sha,
            after_sha,
            created_at: 1001,
        },
    )
    .unwrap()
}

#[test]
fn rollback_restores_before_content_when_file_untouched() {
    let conn = setup_conn();
    let id = mk_trace(&conn, "s1", Some("t1"), "Manual", 1000);
    let dir = tmp_data_dir();
    let target = dir.path().join("x.py");
    let before = "before line\n";
    let after = "after line\n";
    fs::write(&target, after).unwrap();
    let checkpoints = dir.path().join("checkpoints");
    fs::create_dir_all(&checkpoints).unwrap();
    fs::write(checkpoints.join("snap-1"), before).unwrap();
    let change = mk_change(
        &conn,
        id,
        target.to_str().unwrap(),
        "modify",
        Some("snap-1"),
        Some(&crate::bot::sha256_hex(before)),
        Some(&crate::bot::sha256_hex(after)),
    );
    let path = file_rollback_core(&conn, dir.path(), change).unwrap();
    assert_eq!(path, target.to_str().unwrap());
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        before,
        "回滚后文件必须恢复到 before 内容"
    );
}

#[test]
fn rollback_rejects_when_file_drifted_after_change() {
    let conn = setup_conn();
    let id = mk_trace(&conn, "s1", Some("t1"), "Manual", 1000);
    let dir = tmp_data_dir();
    let target = dir.path().join("x.py");
    let drifted = "after line\n+ human edit\n";
    fs::write(&target, drifted).unwrap();
    let checkpoints = dir.path().join("checkpoints");
    fs::create_dir_all(&checkpoints).unwrap();
    fs::write(checkpoints.join("snap-1"), "before line\n").unwrap();
    let change = mk_change(
        &conn,
        id,
        target.to_str().unwrap(),
        "modify",
        Some("snap-1"),
        Some(&crate::bot::sha256_hex("before line\n")),
        Some(&crate::bot::sha256_hex("after line\n")),
    );
    let err = file_rollback_core(&conn, dir.path(), change).unwrap_err();
    assert!(
        err.contains("拒绝回滚以免吞掉后续修改"),
        "漂移必须被拒，实际：{err}"
    );
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        drifted,
        "被拒后文件必须原样保留，一个字节都不能动"
    );
}

#[test]
fn rollback_rejects_create_kind_and_missing_snapshot_ref() {
    let conn = setup_conn();
    let id = mk_trace(&conn, "s1", Some("t1"), "Manual", 1000);
    let dir = tmp_data_dir();
    let create_row = mk_change(&conn, id, "/a/new.py", "create", None, None, None);
    let err = file_rollback_core(&conn, dir.path(), create_row).unwrap_err();
    assert!(err.contains("暂不支持 create 类型的回滚"), "实际：{err}");

    let modify_no_ref = mk_change(&conn, id, "/a/x.py", "modify", None, None, None);
    let err = file_rollback_core(&conn, dir.path(), modify_no_ref).unwrap_err();
    assert!(err.contains("无 before 快照"), "实际：{err}");
}

#[test]
fn rollback_rejects_tampered_snapshot_content() {
    let conn = setup_conn();
    let id = mk_trace(&conn, "s1", Some("t1"), "Manual", 1000);
    let dir = tmp_data_dir();
    let target = dir.path().join("x.py");
    fs::write(&target, "after line\n").unwrap();
    let checkpoints = dir.path().join("checkpoints");
    fs::create_dir_all(&checkpoints).unwrap();
    fs::write(checkpoints.join("snap-1"), "被换掉的内容\n").unwrap();
    let change = mk_change(
        &conn,
        id,
        target.to_str().unwrap(),
        "modify",
        Some("snap-1"),
        Some(&crate::bot::sha256_hex("before line\n")),
        Some(&crate::bot::sha256_hex("after line\n")),
    );
    let err = file_rollback_core(&conn, dir.path(), change).unwrap_err();
    assert!(err.contains("快照校验不符"), "实际：{err}");
}

#[test]
fn export_writes_trace_span_change_lines_in_order() {
    let conn = setup_conn();
    let id = mk_trace(&conn, "s1", Some("t1"), "Manual", 1000);
    span_insert(
        &conn,
        &NewSpan {
            trace_id: id,
            turn: 1,
            tool_call_id: Some("c1"),
            name: "read_text_file",
            args: Some("{}"),
            result: None,
            ok: true,
            error_class: None,
            duration_ms: Some(5),
            created_at: 1001,
        },
    )
    .unwrap();
    mk_change(&conn, id, "/a/x.py", "modify", None, None, None);
    let dir = tmp_data_dir();
    let out = trace_export_core(&conn, dir.path(), id).unwrap();
    assert!(
        out.path.ends_with(&format!("exports/trace-{id}.jsonl"))
            || out.path.ends_with(&format!("exports\\trace-{id}.jsonl")),
        "应落在 exports/ 下，实际：{}",
        out.path
    );
    assert_eq!(out.spans, 1);
    assert_eq!(out.files, 1);
    let body = fs::read_to_string(&out.path).unwrap();
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines.len(), 3, "首行 trace + 1 span + 1 file_change");
    assert!(
        lines[0].contains("\"type\":\"trace\""),
        "首行：{}",
        lines[0]
    );
    assert!(
        lines[1].contains("\"type\":\"span\"") && lines[1].contains("read_text_file"),
        "次行：{}",
        lines[1]
    );
    assert!(
        lines[2].contains("\"type\":\"file_change\"") && lines[2].contains("x.py"),
        "末行：{}",
        lines[2]
    );
}

#[test]
fn export_missing_trace_is_error_not_empty_file() {
    let conn = setup_conn();
    let dir = tmp_data_dir();
    let err = trace_export_core(&conn, dir.path(), 999).unwrap_err();
    assert!(err.contains("999"), "报错应带上 trace id，实际：{err}");
    assert!(
        !dir.path().join("exports").exists(),
        "导出失败不得留下空 exports 目录"
    );
}
