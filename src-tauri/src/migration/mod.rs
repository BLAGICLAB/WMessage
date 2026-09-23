//! 桌面文件自动迁移清理：任务进入归档列表（完成满 7 天）后，按用户规则表迁移其绑定文件。
//!
//! 安全约束：
//! - 只处理 archived=true 且未删除的任务——常规看板任务的文件绝不移动/删除
//! - delete 类规则默认关闭（模版即如此），仅当用户显式启用才执行删除
//! - 源文件不存在 / 权限不足 / 同名冲突：记日志跳过，不崩溃、不覆盖、不强制删除
//! - 移动成功后任务 filePath 同步更新为新路径，附件链接持续可用
//!
//! 模块分层：
//! - `types`    公共类型（JournalEntry / RulesFile / Rule / Report / Status）
//! - `journal`  B1 操作 journal：pending → committed / cleared，DB 一致性兜底
//! - `recovery` 启动 replay：journal 仍 pending 时修复 DB 绑定
//! - `rules`    规则表读写 / CSV 解析 / 路径常量
//! - `ops`      文件操作 + 日志 + 防重入 RAII
//! - `run`      迁移引擎 + 后台轮询线程
//! - `commands` 6 个 tauri 命令（lib.rs 用全路径 `migration::commands::migration_*`）
//!
//! 公开路径稳定（lib.rs / 其他模块引用）：
//! - `migration::spawn_polling`    — 由本模块顶层 `pub use` 透传 run::spawn_polling
//! - `migration::ARCHIVE_AFTER_MS` — run 公开常量
//! - `migration::now_ms`           — ops 公开
//! - `migration::filename_matches` — ops 公开
//! - `migration::resolve_archive_dir` — ops 公开
//! - `migration::validate_rules`   — rules 公开
//! - `migration::load_rules`       — rules 公开
//! - `migration::journal_replay_pending` — recovery 公开

pub mod commands;
pub mod journal;
pub mod ops;
pub mod recovery;
pub mod rules;
pub mod run;
pub mod types;

// lib.rs 调用 `migration::spawn_polling(handle)` 的路径必须保持稳定。
// 不放在 commands 里（避免误让 invoke_handler 把它当 command 注册）；
// 也不让 lib.rs 写全路径 `run::spawn_polling`（增加调用方认知负担）。
pub use run::spawn_polling;

#[cfg(test)]
mod tests {
    // 测试跨 7 个子模块，super::* 只拿 mod.rs 顶层项，所以这里显式列每个子模块的入口。
    // 一并 use 上 ops::MAX_MOVE_REMOVE_FAILURES（pub(crate)）供永久跳过断言用。
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use super::*;
    use crate::migration::commands::tail_log_lines;
    use crate::migration::journal::{
        db_write_lock, journal_cleared, journal_cleared_inner, journal_committed,
        journal_committed_inner, journal_find_pending_inner, journal_pending,
        journal_pending_inner,
    };
    use crate::migration::ops::{
        claim_dst_name, conflict_free_name, copy_dir_recursive, expand_year_placeholder,
        filename_matches, move_entry, move_remove_permanently_failed, record_move_remove_failure,
        MAX_MOVE_REMOVE_FAILURES,
    };
    use crate::migration::recovery::{decide_src_missing, file_path_untouched, SrcMissingAction};
    use crate::migration::rules::{parse_rules_csv, save_rules_to, validate_rules};
    use crate::migration::types::{JournalEntry, MigrationRule, RulesFile};

    fn rule(keywords: Vec<&str>, action: &str, dir: &str) -> MigrationRule {
        MigrationRule {
            id: "r1".into(),
            enabled: true,
            keywords: keywords.iter().map(|s| s.to_string()).collect(),
            action: action.into(),
            archive_dir: dir.into(),
        }
    }

    #[test]
    fn filename_match_case_insensitive() {
        let r = rule(vec!["工资"], "move", "工资/{year}");
        assert!(filename_matches("工资表2026.xlsx", &r));
        assert!(filename_matches("十月工资单.pdf", &r));
        assert!(!filename_matches("GONGZI.xlsx", &r)); // 中文关键字不匹配拼音
        assert!(filename_matches("2026工资.xlsx", &r));
        assert!(!filename_matches("报销单.xlsx", &r));
    }

    #[test]
    fn filename_match_english_case_insensitive() {
        let r = rule(vec!["Report"], "move", "报表/{year}");
        assert!(filename_matches("quarterly-report.docx", &r));
        assert!(filename_matches("REPORT_FINAL.docx", &r));
        assert!(!filename_matches("note.txt", &r));
    }

    #[test]
    fn empty_keywords_never_match() {
        let r = rule(vec![], "move", "x");
        assert!(!filename_matches("anything.txt", &r));
    }

    // ── P2-8：save_rules 原子写 ──

    /// tmp 写失败（注入：rules.json.tmp 占成目录）→ 返回 Err，rules.json 保持原状
    #[test]
    fn save_rules_atomic_failure_keeps_original() {
        let dir = std::env::temp_dir().join(format!("wm-rules-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rules.json");
        let original = "{\"version\":1,\"rules\":[]}";
        std::fs::write(&path, original).unwrap();
        // 失败注入：tmp 路径占成目录 → atomic_write 写临时文件必败
        std::fs::create_dir_all(dir.join("rules.json.tmp")).unwrap();

        let rules = RulesFile {
            version: 1,
            rules: vec![rule(vec!["工资"], "move", "工资/{year}")],
        };
        let r = save_rules_to(&path, &rules);
        assert!(r.is_err(), "tmp 写失败必须返回 Err");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            original,
            "写失败时 rules.json 必须保持原状（不留半截）"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 正常写：内容完整可解析、无 tmp 残件
    #[test]
    fn save_rules_atomic_success_no_tmp_left() {
        let dir = std::env::temp_dir().join(format!("wm-rules-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rules.json");
        let rules = RulesFile {
            version: 1,
            rules: vec![rule(vec!["工资"], "move", "工资/{year}")],
        };
        save_rules_to(&path, &rules).unwrap();
        let parsed: RulesFile =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed.rules.len(), 1);
        assert_eq!(parsed.rules[0].keywords, vec!["工资".to_string()]);
        assert!(
            !dir.join("rules.json.tmp").exists(),
            "原子写完成后不得残留 tmp 文件"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// B1: journal SQL 基础流。构造临时 DB + 创建 migration_journal 表，
    /// 验证 pending → committed/cleared 状态变迁。
    fn setup_journal_db() -> (std::path::PathBuf, rusqlite::Connection) {
        let dir = std::env::temp_dir().join(format!("wm-jrn-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE migration_journal (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                op          TEXT    NOT NULL,
                src         TEXT    NOT NULL,
                dst         TEXT,
                task_id     TEXT    NOT NULL,
                state       TEXT    NOT NULL,
                created_at  INTEGER NOT NULL
            );",
        )
        .unwrap();
        (dir, conn)
    }

    /// B1: pending → committed 是 happy path，启动 replay 跳过该行
    #[test]
    fn journal_pending_to_committed_flow() {
        let (dir, conn) = setup_journal_db();
        let id = journal_pending_inner(
            &conn,
            "move",
            Path::new("/src/a"),
            Some(Path::new("/dst/a")),
            "task-1",
            1000,
        )
        .unwrap();
        assert!(id > 0);

        // 验证插入后 state = pending
        let state: String = conn
            .query_row(
                "SELECT state FROM migration_journal WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "pending");

        journal_committed_inner(&conn, id).unwrap();

        let state: String = conn
            .query_row(
                "SELECT state FROM migration_journal WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "committed");

        // replay 查询会跳过该行（WHERE state = 'pending'）
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM migration_journal WHERE state = 'pending'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// B1: pending → cleared（操作未启动 / 已失败），replay 不该修复
    #[test]
    fn journal_pending_to_cleared_flow() {
        let (dir, conn) = setup_journal_db();
        let id = journal_pending_inner(&conn, "delete", Path::new("/x/y"), None, "task-2", 2000)
            .unwrap();
        journal_cleared_inner(&conn, id).unwrap();
        let state: String = conn
            .query_row(
                "SELECT state FROM migration_journal WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "cleared");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// NEW-B-2: journal 写持 DB_WRITE_LOCK 与 db_upsert 等写串行——锁被占时阻塞等待
    /// 锁释放后再写（不再各起新连接靠 2s busy_timeout 兜底），全程无 busy 失败。
    #[test]
    fn journal_writes_serialize_on_db_write_lock() {
        let (dir, conn) = setup_journal_db();
        // 模拟 db_upsert 持锁临界区（spawn_blocking 线程里持锁 200ms）
        let holder = std::thread::spawn(|| {
            let _g = db_write_lock();
            std::thread::sleep(Duration::from_millis(200));
        });
        std::thread::sleep(Duration::from_millis(50)); // 确保 holder 先拿到锁
        let start = std::time::Instant::now();
        // 锁被占用期间 journal 三次写都应等待而非 busy 报错
        let id = journal_pending(
            &conn,
            "move",
            Path::new("/src/lock"),
            Some(Path::new("/dst/lock")),
            "task-lock",
        )
        .unwrap();
        journal_committed(&conn, id).unwrap();
        journal_cleared(&conn, id).unwrap();
        let waited = start.elapsed();
        holder.join().unwrap();
        assert!(
            waited >= Duration::from_millis(100),
            "journal 写应等待锁释放（实测 {waited:?}），而非立即失败"
        );
        let state: String = conn
            .query_row(
                "SELECT state FROM migration_journal WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "cleared", "三次写在持锁串行下全部成功落库");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// B1: replay 模拟场景——move 成功但 db_upsert 崩，journal 保持 pending。
    /// 重启后 replay 检查 dst 存在 + src 不存在 → 调用 recover_move_db
    /// （这里不测 recover_move_db 本身，只验证检测逻辑的状态断言）。
    #[test]
    fn journal_replay_detects_move_completed_state() {
        let (dir, conn) = setup_journal_db();

        // 模拟「上一轮：pending 已写、move_entry 成功、db_upsert 崩溃」
        let src_path = dir.join("src.txt");
        let dst_path = dir.join("dst.txt");
        std::fs::write(&src_path, b"hello").unwrap();
        // 模拟 move 完成：写文件到 dst，删 src
        std::fs::rename(&src_path, &dst_path).unwrap();

        let id = journal_pending_inner(&conn, "move", &src_path, Some(&dst_path), "task-3", 3000)
            .unwrap();

        // 此刻模拟 replay 检测：dst 存在 + src 不存在
        assert!(!src_path.exists(), "模拟：src 应已被移走");
        assert!(dst_path.exists(), "模拟：dst 应已存在");
        // 验证 journal 仍 pending
        let state: String = conn
            .query_row(
                "SELECT state FROM migration_journal WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "pending", "db_upsert 崩后 journal 仍 pending");

        // replay 会检查 dst.exists() && !src.exists() → 调用 recover_move_db
        // （不能在这里调 recover_move_db，因为需要 AppHandle + 完整 task 表）
        std::fs::remove_dir_all(&dir).ok();
    }

    /// NEW-B-1: journal_find_pending_inner happy path——按 (task_id, src) 找到最新 pending。
    #[test]
    fn journal_find_pending_finds_matching_entry() {
        let (dir, conn) = setup_journal_db();
        let src = Path::new("/src/a");
        let id = journal_pending_inner(
            &conn,
            "move",
            src,
            Some(Path::new("/dst/a")),
            "task-9",
            1000,
        )
        .unwrap();

        let found = journal_find_pending_inner(&conn, "task-9", src).unwrap();
        let e = found.expect("应找到 pending 条目");
        assert_eq!(e.id, id);
        assert_eq!(e.op, "move");
        assert_eq!(e.dst.as_deref(), Some("/dst/a"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// NEW-B-1: 失败路径——committed/cleared、别的 task、别的 src 都不得命中。
    #[test]
    fn journal_find_pending_ignores_non_pending_and_other_keys() {
        let (dir, conn) = setup_journal_db();
        let src = Path::new("/src/a");

        // committed 的不算 pending
        let id1 = journal_pending_inner(
            &conn,
            "move",
            src,
            Some(Path::new("/dst/a")),
            "task-9",
            1000,
        )
        .unwrap();
        journal_committed_inner(&conn, id1).unwrap();
        assert!(
            journal_find_pending_inner(&conn, "task-9", src)
                .unwrap()
                .is_none(),
            "committed 条目不应命中"
        );

        // cleared 的不算 pending
        let id2 = journal_pending_inner(&conn, "delete", src, None, "task-9", 2000).unwrap();
        journal_cleared_inner(&conn, id2).unwrap();
        assert!(
            journal_find_pending_inner(&conn, "task-9", src)
                .unwrap()
                .is_none(),
            "cleared 条目不应命中"
        );

        // 别的 task / 别的 src 不命中
        journal_pending_inner(
            &conn,
            "move",
            src,
            Some(Path::new("/dst/b")),
            "task-other",
            3000,
        )
        .unwrap();
        journal_pending_inner(
            &conn,
            "move",
            Path::new("/src/other"),
            Some(Path::new("/dst/c")),
            "task-9",
            4000,
        )
        .unwrap();
        assert!(
            journal_find_pending_inner(&conn, "task-9", src)
                .unwrap()
                .is_none(),
            "其他 task/src 的 pending 不应串扰"
        );

        // MI-04a：同 (task, src) 重复 pending 复用既有行（不再产生 id DESC 下的孤儿）
        let id3 = journal_pending_inner(
            &conn,
            "move",
            src,
            Some(Path::new("/dst/old")),
            "task-9",
            5000,
        )
        .unwrap();
        let id4 = journal_pending_inner(
            &conn,
            "move",
            src,
            Some(Path::new("/dst/new")),
            "task-9",
            6000,
        )
        .unwrap();
        assert_eq!(id3, id4, "同 key 重入应复用既有 pending 行");
        let e = journal_find_pending_inner(&conn, "task-9", src)
            .unwrap()
            .unwrap();
        assert_eq!(e.id, id3, "复用后 find 命中同一行");
        assert_eq!(
            e.dst.as_deref(),
            Some("/dst/new"),
            "复用时 dst 应刷新为最新"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// MI-04a：同 key 二次 pending → 同 id + 行数恒 1 + op/dst 刷新；异 key 不受影响。
    #[test]
    fn journal_pending_dedups_same_key() {
        let (dir, conn) = setup_journal_db();
        let src = Path::new("/src/a");

        let id1 =
            journal_pending_inner(&conn, "move", src, Some(Path::new("/dst/1")), "t", 100).unwrap();
        let id2 = journal_pending_inner(&conn, "delete", src, None, "t", 200).unwrap();
        assert_eq!(id1, id2, "同 (task_id, src) 重入应复用同一 pending 行");

        let pending_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM migration_journal
                 WHERE task_id = 't' AND src = '/src/a' AND state = 'pending'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pending_count, 1, "同 key 至多一条 pending");

        let e = journal_find_pending_inner(&conn, "t", src)
            .unwrap()
            .unwrap();
        assert_eq!(e.op, "delete", "复用时 op 应刷新为最新");
        assert_eq!(e.dst, None, "复用时 dst 应刷新为最新（含清空）");

        // 异 key 仍各自 INSERT 新行
        let id3 =
            journal_pending_inner(&conn, "move", Path::new("/src/b"), None, "t", 300).unwrap();
        assert!(id3 > id1, "异 src 应新建 pending 行");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// NEW-B-1: decide_src_missing 全分支。
    #[test]
    fn decide_src_missing_all_branches() {
        let mk = |op: &str, dst: Option<&str>| JournalEntry {
            id: 7,
            op: op.into(),
            src: "/src/a".into(),
            dst: dst.map(|s| s.into()),
            task_id: "t".into(),
        };

        // move pending + dst 存在 → 就地修复绑定
        match decide_src_missing(Some(mk("move", Some("/dst/a"))), true) {
            SrcMissingAction::RepairMove { dst, journal_id } => {
                assert_eq!(dst, "/dst/a");
                assert_eq!(journal_id, 7);
            }
            _ => panic!("move+dst 存在应 RepairMove"),
        }

        // move pending + dst 也丢失 → 解绑并关闭 journal
        match decide_src_missing(Some(mk("move", Some("/dst/a"))), false) {
            SrcMissingAction::Unbind { commit_journal } => assert_eq!(commit_journal, Some(7)),
            _ => panic!("move+dst 丢失应 Unbind"),
        }

        // move pending 但 dst 字段为 NULL（数据异常）→ 解绑并关闭 journal
        match decide_src_missing(Some(mk("move", None)), true) {
            SrcMissingAction::Unbind { commit_journal } => assert_eq!(commit_journal, Some(7)),
            _ => panic!("move 无 dst 应 Unbind"),
        }

        // delete pending → 解绑本就是终态，但需提交 journal 关闭环路
        match decide_src_missing(Some(mk("delete", None)), false) {
            SrcMissingAction::Unbind { commit_journal } => assert_eq!(commit_journal, Some(7)),
            _ => panic!("delete pending 应 Unbind+commit"),
        }

        // 无 pending → 纯解绑，无 journal 要关
        match decide_src_missing(None, false) {
            SrcMissingAction::Unbind { commit_journal } => assert_eq!(commit_journal, None),
            _ => panic!("无 pending 应 Unbind"),
        }
    }

    /// NEW-B-1: replay 防覆盖谓词——file_path 仍指向 src 才允许修复。
    #[test]
    fn file_path_untouched_guard() {
        assert!(
            file_path_untouched(Some("/src/a"), "/src/a"),
            "仍指向 src → 允许修复"
        );
        assert!(
            !file_path_untouched(Some("/other/b"), "/src/a"),
            "用户重绑 → 禁止覆盖"
        );
        assert!(!file_path_untouched(None, "/src/a"), "已解绑 → 禁止回写");
    }

    #[test]
    fn conflict_free_name_sequence() {
        let dir = std::env::temp_dir().join(format!("wm-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "1").unwrap();
        std::fs::write(dir.join("a (1).txt"), "1").unwrap();
        let got = conflict_free_name(&dir, "a.txt").unwrap();
        assert_eq!(got.file_name().unwrap().to_string_lossy(), "a (2).txt");
        let got2 = conflict_free_name(&dir, "b.txt").unwrap();
        assert_eq!(got2.file_name().unwrap().to_string_lossy(), "b.txt");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// P2-6 TOCTOU：两个并发任务都选定 "name (1).pdf"——先落盘者占位后，
    /// 后走 claim_dst_name 的必须递增到 "name (2).pdf"，不覆盖前者。
    #[test]
    fn claim_dst_name_bumps_suffix_when_race_claimed() {
        let dir = std::env::temp_dir().join(format!("wm-toctou-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("name.pdf"), b"original").unwrap();

        // 任务 A、B 同时选定 name (1).pdf（竞态窗口：选定相同）
        let chosen_a = conflict_free_name(&dir, "name.pdf").unwrap();
        assert_eq!(
            chosen_a.file_name().unwrap().to_string_lossy(),
            "name (1).pdf"
        );
        // A 先落盘（move_entry 创建目标文件）
        std::fs::write(&chosen_a, b"a-wins").unwrap();

        // B 走 claim：落盘前复检发现 name (1).pdf 已被抢占 → 递增重选
        let claimed_b = claim_dst_name(&dir, "name.pdf").unwrap();
        assert_eq!(
            claimed_b.file_name().unwrap().to_string_lossy(),
            "name (2).pdf",
            "被并发抢占后必须递增后缀，不得覆盖 A 的文件"
        );
        assert_eq!(
            std::fs::read(dir.join("name (1).pdf")).unwrap(),
            b"a-wins",
            "A 的文件不得被覆盖"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// P2-6：无抢占时 claim 与 conflict_free_name 选名一致（直通路径不回归）
    #[test]
    fn claim_dst_name_no_race_picks_first_free() {
        let dir = std::env::temp_dir().join(format!("wm-toctou-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("r.pdf"), b"x").unwrap();
        let got = claim_dst_name(&dir, "r.pdf").unwrap();
        assert_eq!(got.file_name().unwrap().to_string_lossy(), "r (1).pdf");
        let free = claim_dst_name(&dir, "new.pdf").unwrap();
        assert_eq!(free.file_name().unwrap().to_string_lossy(), "new.pdf");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn archive_dir_year_expansion() {
        // 直调生产展开函数（原来内联复刻 replace 逻辑，生产改动测试不红的弱断言）
        let expanded = expand_year_placeholder("工资/{year}");
        let year = chrono::Local::now().format("%Y").to_string();
        assert_eq!(expanded, format!("工资/{year}"));
        assert!(!expanded.contains('{'));
    }

    #[test]
    fn copy_dir_recursive_works() {
        let base = std::env::temp_dir().join(format!("wm-cp-{}", uuid::Uuid::new_v4()));
        let src = base.join("源");
        let dst = base.join("目标");
        std::fs::create_dir_all(src.join("子")).unwrap();
        std::fs::write(src.join("a.txt"), "A").unwrap();
        std::fs::write(src.join("子").join("b.txt"), "B").unwrap();
        copy_dir_recursive(&src, &dst).unwrap();
        assert_eq!(std::fs::read_to_string(dst.join("a.txt")).unwrap(), "A");
        assert_eq!(
            std::fs::read_to_string(dst.join("子").join("b.txt")).unwrap(),
            "B"
        );
        assert!(src.exists()); // 拷贝不改源
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn move_entry_moves_directory() {
        // 文件夹任务：整个目录应被移动（rename 对目录同样生效）
        let base = std::env::temp_dir().join(format!("wm-mv-{}", uuid::Uuid::new_v4()));
        let src = base.join("项目资料");
        let dst = base.join("归档").join("项目资料");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.txt"), "x").unwrap();
        std::fs::create_dir_all(base.join("归档")).unwrap();
        move_entry(&src, &dst).unwrap();
        assert!(dst.join("a.txt").exists());
        assert!(!src.exists());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn move_entry_file_conflict_names_dir() {
        // 同名冲突命名对目录同样生效
        let base = std::env::temp_dir().join(format!("wm-mv2-{}", uuid::Uuid::new_v4()));
        let src = base.join("项目");
        let dst = base.join("归档");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::create_dir_all(dst.join("项目")).unwrap(); // 已有同名目录
        let got = conflict_free_name(&dst, "项目").unwrap();
        assert_eq!(got.file_name().unwrap().to_string_lossy(), "项目 (1)");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn serde_parses_camel_case_archive_dir() {
        // 用户导入的规则表使用 camelCase 字段名（archiveDir），必须能正确解析
        let json = r#"{"id":"x","enabled":true,"keywords":["工资"],"action":"move","archiveDir":"工资/{year}"}"#;
        let r: MigrationRule = serde_json::from_str(json).unwrap();
        assert_eq!(r.archive_dir, "工资/{year}");
        assert_eq!(r.action, "move");
        // 反序列化后重新序列化，字段名保持 camelCase（模版下载与保存格式一致）
        let out = serde_json::to_string(&r).unwrap();
        assert!(out.contains("archiveDir"));
    }

    #[test]
    fn validate_rejects_bad_rules() {
        let mut bad = RulesFile {
            version: 1,
            rules: vec![rule(vec!["a"], "fly", "x")],
        };
        assert!(validate_rules(&bad).is_err());
        bad.rules = vec![rule(vec!["a"], "move", " ")];
        assert!(validate_rules(&bad).is_err());
        bad.rules = vec![rule(vec![""], "move", "x")];
        assert!(validate_rules(&bad).is_err());
        bad.rules = vec![rule(vec!["a"], "delete", "")];
        assert!(validate_rules(&bad).is_ok());
    }

    // ────── 文件移动 / 同名冲突（基于纯 Path API，不依赖 AppHandle） ──────

    /// move_entry 基本：源文件 → 目标路径（无冲突）。原文件消失，新文件就位。
    #[test]
    fn move_file_basic() {
        let base = std::env::temp_dir().join(format!("wm-mv-basic-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        let src = base.join("report.pdf");
        let dst = base.join("归档").join("report.pdf");
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::write(&src, b"hello world").unwrap();

        move_entry(&src, &dst).unwrap();
        assert!(dst.exists(), "目标文件应存在");
        assert!(!src.exists(), "源文件应已被移走");
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "hello world");
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// 同名冲突 → 加 ` (1)` 后缀。直接测 conflict_free_name（move_entry 会用它）。
    #[test]
    fn move_file_collision_adds_suffix_one() {
        let base = std::env::temp_dir().join(format!("wm-mv-col1-{}", uuid::Uuid::new_v4()));
        let dst_dir = base.join("归档");
        std::fs::create_dir_all(&dst_dir).unwrap();
        // 预占同名文件
        std::fs::write(dst_dir.join("report.pdf"), b"old").unwrap();

        let chosen = conflict_free_name(&dst_dir, "report.pdf").unwrap();
        assert_eq!(
            chosen.file_name().unwrap().to_string_lossy(),
            "report (1).pdf"
        );

        // 模拟实际迁移：用 chosen 作为目标路径做 move_entry（用同名空 src 占位）
        // 此场景是冲突检测本身已被 move_entry 调用前的 conflict_free_name 解决
        // 这里仅断言 conflict_free_name 选出的名字可用——后续真实 move 由调用方负责
        assert!(!chosen.exists(), "选出的名字不应已存在");
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// 多个同名：name / name (1) 已存在 → 选择 name (2)
    #[test]
    fn move_file_collision_multiple_suffixes_picks_two() {
        let base = std::env::temp_dir().join(format!("wm-mv-col2-{}", uuid::Uuid::new_v4()));
        let dst_dir = base.join("归档");
        std::fs::create_dir_all(&dst_dir).unwrap();
        std::fs::write(dst_dir.join("report.pdf"), b"original").unwrap();
        std::fs::write(dst_dir.join("report (1).pdf"), b"first dup").unwrap();

        let chosen = conflict_free_name(&dst_dir, "report.pdf").unwrap();
        assert_eq!(
            chosen.file_name().unwrap().to_string_lossy(),
            "report (2).pdf"
        );
        assert!(!chosen.exists());
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// 扩展名/无扩展名的冲突场景应都能正确处理
    #[test]
    fn conflict_free_name_handles_extensionless_files() {
        let base = std::env::temp_dir().join(format!("wm-col-noext-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("README"), b"a").unwrap();
        std::fs::write(base.join("README (1)"), b"b").unwrap();

        let chosen = conflict_free_name(&base, "README").unwrap();
        assert_eq!(chosen.file_name().unwrap().to_string_lossy(), "README (2)");
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// move_entry 在源不存在时返回 Err（不 panic）：保护测试
    #[test]
    fn move_file_source_missing_returns_error() {
        let base = std::env::temp_dir().join(format!("wm-mv-missing-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        let src = base.join("ghost.txt"); // 从未创建
        let dst = base.join("归档").join("ghost.txt");

        let err = move_entry(&src, &dst);
        assert!(err.is_err(), "源不存在应返回 Err");
        assert!(!dst.exists(), "目标未被创建");
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// move_entry 成功拷贝文件后保留源文件以外的目录结构（拷贝语义）不适用于 move_entry 本身，
    /// 但应保证源是文件时 fs::rename 路径正确。此场景测试：跨目录 rename（同一 temp_dir 下）
    #[test]
    fn move_entry_handles_nested_dst_directory() {
        let base = std::env::temp_dir().join(format!("wm-mv-nested-{}", uuid::Uuid::new_v4()));
        let src = base.join("发票").join("input.pdf");
        let dst_dir = base.join("归档").join("发票").join("2026");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, b"x").unwrap();
        std::fs::create_dir_all(&dst_dir).unwrap();

        move_entry(&src, &dst_dir.join("input.pdf")).unwrap();
        assert!(dst_dir.join("input.pdf").exists());
        assert!(!src.exists());
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// 跨卷移动：fs::rename 失败后应退化到 copy+remove（这里同卷下也会走 rename，但验证路径不崩）
    #[test]
    fn move_entry_to_file_basic_file() {
        let base = std::env::temp_dir().join(format!("wm-mv-file-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        let src = base.join("data.bin");
        let dst = base.join("dest.bin");
        std::fs::write(&src, vec![1u8, 2, 3, 4, 5]).unwrap();

        move_entry(&src, &dst).unwrap();
        assert!(dst.exists());
        assert!(!src.exists());
        assert_eq!(std::fs::read(&dst).unwrap(), vec![1u8, 2, 3, 4, 5]);
        std::fs::remove_dir_all(&base).unwrap();
    }

    // ────── archive_dir year 占位符独立验证（不依赖 AppHandle::desktop_dir） ──────

    /// {year} 在多个上下文路径中都能被替换
    #[test]
    fn archive_dir_year_placeholder_in_nested_path() {
        for template in ["工资/{year}", "归档/{year}/Q4", "{year}/发票"] {
            let expanded = expand_year_placeholder(template);
            assert!(
                !expanded.contains("{year}"),
                "模板 {template:?} 仍有未替换的占位符"
            );
            // 展开后的路径应是合理的本地路径
            assert!(PathBuf::from(&expanded).is_absolute() || expanded.contains('/'));
        }
    }

    /// 多个 {year} 占位符在同一个模板里都会被替换
    #[test]
    fn archive_dir_year_placeholder_replaced_multiple_times() {
        let year = chrono::Local::now().format("%Y").to_string();
        let expanded = expand_year_placeholder("{year}/sub/{year}");
        assert_eq!(expanded, format!("{year}/sub/{year}"));
        assert!(!expanded.contains("{year}"));
    }

    // ────── parse_rules_csv 增强测试（CSV 解析逻辑） ──────

    #[test]
    fn parse_rules_csv_basic_template() {
        // 复用官方模版：\u{feff}启用,文件名关键字,动作,归档目录\n是,工资，...
        let text = "\u{feff}启用,文件名关键字,动作,归档目录\n是,工资，报销,移动归档,工资/{year}\n否,临时,删除文件,\n";
        let rules = parse_rules_csv(text).expect("官方模版应可解析");
        assert_eq!(rules.rules.len(), 2);

        let r0 = &rules.rules[0];
        assert!(r0.enabled);
        assert_eq!(r0.keywords, vec!["工资", "报销"]);
        assert_eq!(r0.action, "move");
        assert_eq!(r0.archive_dir, "工资/{year}");

        let r1 = &rules.rules[1];
        assert!(!r1.enabled);
        assert_eq!(r1.keywords, vec!["临时"]);
        assert_eq!(r1.action, "delete");
        assert_eq!(r1.archive_dir, ""); // delete 规则无视归档目录
    }

    #[test]
    fn parse_rules_csv_handles_missing_bom() {
        // 不带 BOM 也能解析（有些人手写 CSV）
        let text = "启用,文件名关键字,动作,归档目录\n是,发票,移动,发票/{year}\n";
        let rules = parse_rules_csv(text).expect("no-bom CSV should parse");
        assert_eq!(rules.rules.len(), 1);
        assert_eq!(rules.rules[0].keywords, vec!["发票"]);
    }

    #[test]
    fn parse_rules_csv_skips_blank_rows() {
        // 含空行 / 仅空格的行应被跳过
        let text = "启用,文件名关键字,动作,归档目录\n是,工资,移动,工资/{year}\n,\n   ,\n是,发票,移动,发票/{year}\n";
        let rules = parse_rules_csv(text).expect("blank rows should be skipped");
        assert_eq!(rules.rules.len(), 2);
        assert_eq!(rules.rules[0].keywords, vec!["工资"]);
        assert_eq!(rules.rules[1].keywords, vec!["发票"]);
    }

    #[test]
    fn parse_rules_csv_accepts_alternative_keywords_and_actions() {
        // 关键字支持中英文逗号 / 顿号 / 分号分隔；动作支持 移动/移动归档/move/Move
        let text = "启用,文件名关键字,动作,归档目录\ntrue,a；b；c，d,Move,X\n";
        let rules = parse_rules_csv(text).expect("valid row should parse");
        assert_eq!(rules.rules.len(), 1);
        assert!(rules.rules[0].enabled, "true 应被识别为启用");
        assert_eq!(
            rules.rules[0].keywords,
            vec!["a", "b", "c", "d"],
            "混合中英文分号 / 逗号分隔"
        );
        assert_eq!(
            rules.rules[0].action, "move",
            "Move 大小写变体应被归一化为 move"
        );
    }

    #[test]
    fn parse_rules_csv_rejects_empty() {
        // 只有表头、无任何有效规则行 → 错误
        let text = "启用,文件名关键字,动作,归档目录\n";
        let err = parse_rules_csv(text);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("没有解析出任何规则"));
    }

    #[test]
    fn parse_rules_csv_rejects_unknown_action() {
        // 动作不在白名单 → 报错（指明行号）
        let text = "启用,文件名关键字,动作,归档目录\n是,工资,飞行,X\n";
        let err = parse_rules_csv(text).expect_err("未知动作应报错");
        assert!(
            err.to_string().contains("飞行"),
            "错误信息应提到无效动作名：{err}"
        );
        assert!(err.to_string().contains("第 2 行"), "错误信息应指明行号");
    }

    #[test]
    fn parse_rules_csv_requires_four_columns_in_header() {
        // 表头列不全 → 报错
        let text = "启用,文件名关键字,动作\n是,a,移动,X\n";
        let err = parse_rules_csv(text).expect_err("缺列应报错");
        assert!(err.to_string().contains("四列"));
    }

    #[test]
    fn parse_rules_csv_rejects_empty_action() {
        // 空动作不再静默按 move 处理 → 行级报错（fail-closed）
        let text = "启用,文件名关键字,动作,归档目录\n是,工资,,工资/{year}\n";
        let err = parse_rules_csv(text).expect_err("空动作应报错");
        assert!(
            err.to_string().contains("第 2 行"),
            "错误信息应指明行号：{err}"
        );
        assert!(
            err.to_string().contains("动作"),
            "错误信息应提到动作：{err}"
        );
    }

    #[test]
    fn parse_rules_csv_rejects_unknown_enabled() {
        // 启用列白名单外 token（含空）→ 行级报错，不再静默按 false 处理
        let text = "启用,文件名关键字,动作,归档目录\nmaybe,工资,移动归档,工资/{year}\n";
        let err = parse_rules_csv(text).expect_err("未知启用值应报错");
        assert!(
            err.to_string().contains("maybe"),
            "错误信息应提到无效启用值：{err}"
        );
        assert!(
            err.to_string().contains("第 2 行"),
            "错误信息应指明行号：{err}"
        );
    }

    #[test]
    fn parse_rules_csv_accepts_explicit_disabled_variants() {
        // 显式关闭白名单：否 / false / 0
        let text =
            "启用,文件名关键字,动作,归档目录\n否,a,删除文件,\nfalse,b,删除文件,\n0,c,删除文件,\n";
        let rules = parse_rules_csv(text).expect("显式关闭变体应可解析");
        assert_eq!(rules.rules.len(), 3);
        assert!(rules.rules.iter().all(|r| !r.enabled));
    }

    #[test]
    fn validate_rules_accepts_delete_without_archive_dir() {
        // delete 规则允许 archive_dir 为空
        let rf = RulesFile {
            version: 1,
            rules: vec![rule(vec!["tmp"], "delete", "")],
        };
        assert!(validate_rules(&rf).is_ok());
    }

    #[test]
    fn validate_rules_rejects_move_with_blank_archive_dir() {
        let mut rf = RulesFile {
            version: 1,
            rules: vec![rule(vec!["a"], "move", "")],
        };
        assert!(validate_rules(&rf).is_err());
        // 全空白也算空
        rf.rules = vec![rule(vec!["a"], "move", "   ")];
        assert!(validate_rules(&rf).is_err());
    }

    /// NEW-B-5: 同一 src 的 remove 失败计数越阈后永久跳过；不同 src 互不影响。
    #[test]
    fn move_remove_failure_counter_permanent_skip() {
        let key = format!("/tmp/wm-mrf-{}", uuid::Uuid::new_v4());
        for i in 1..MAX_MOVE_REMOVE_FAILURES {
            assert_eq!(record_move_remove_failure(&key), i);
            assert!(
                !move_remove_permanently_failed(&key),
                "第 {i} 次失败不应触发永久跳过"
            );
        }
        assert_eq!(record_move_remove_failure(&key), MAX_MOVE_REMOVE_FAILURES);
        assert!(
            move_remove_permanently_failed(&key),
            "第 {MAX_MOVE_REMOVE_FAILURES} 次失败后应永久跳过"
        );
        assert!(
            !move_remove_permanently_failed("/tmp/wm-mrf-never-seen"),
            "其他 src 不受影响"
        );
    }

    /// NEW-B-5: 模拟跨卷 remove 持续失败——每轮 copy 新冲突名都成功但 src 删不掉；
    /// 计数越阈后永久跳过，之后不再 copy（归档副本数封顶在阈值，不会无限累积）。
    #[test]
    fn move_remove_persistent_failure_stops_duplicate_copies() {
        let base = std::env::temp_dir().join(format!("wm-mrf2-{}", uuid::Uuid::new_v4()));
        let archive = base.join("归档");
        std::fs::create_dir_all(&archive).unwrap();
        let src = base.join("报告.pdf");
        std::fs::write(&src, b"x").unwrap();
        let src_str = src.to_string_lossy().to_string();

        let mut copies = 0usize;
        for _round in 1..8 {
            // 与 run_migration_inner 同序：先查永久跳过，再 copy
            if move_remove_permanently_failed(&src_str) {
                break;
            }
            let dst = conflict_free_name(&archive, "报告.pdf").unwrap();
            // 模拟 move_entry 跨卷回退：copy 成功 + remove 失败（Windows 占用，src 仍在）
            std::fs::copy(&src, &dst).unwrap();
            copies += 1;
            if src.exists() && dst.exists() {
                record_move_remove_failure(&src_str);
            }
        }
        assert_eq!(
            copies, MAX_MOVE_REMOVE_FAILURES as usize,
            "副本数应封顶在阈值（不会无限 copy）"
        );
        assert!(move_remove_permanently_failed(&src_str));
        // 归档目录里确实只有阈值个副本
        let n = std::fs::read_dir(&archive).unwrap().count();
        assert_eq!(n, MAX_MOVE_REMOVE_FAILURES as usize);
        std::fs::remove_dir_all(&base).ok();
    }
}
