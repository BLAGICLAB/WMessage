//! U20-EVOGOV 集成：confirm 档端到端 + 人工批准执行器（W1）+ 指标冒烟（W3）。
//!
//! **单用例叙事**：nextest 每测试一进程，而共享 deps 数据目录下的 evolution
//! jsonl 是整文件重写（无按行合并），两个进程并发互踩必丢条目（lib 内首版
//! 实测复现「proposal 不存在」）。故凡触碰共享 evolution-proposals/changes/
//! applied.jsonl 的断言全部合并进本文件一个用例（同进程天然串行），lib 侧
//! 只留纯函数单测；条目 id 用 u20-gov 前缀防撞库，用毕清理（承 U19 实录）。
//!
//! post_consolidation 字面入口依赖 emit::APP_HANDLE（OnceLock<AppHandle<Wry>>，
//! 测试进程不可注册 MockRuntime），端到端按其真实构成拆三段覆盖——
//! ① ops → `derive_proposals`（真启发式）② 候选池 `write_proposals`（真写盘）
//! ③ confirm 分流谓词 `policy::auto_apply_allowed`（生产同一函数，读真
//! bot-config.json）；三段即 post_consolidation 的全部决策面，apply 段由
//! toggle 执行器真跑（lesson/CR/applied 留痕全落真库真文件）。

use wmessage_lib::db;
use wmessage_lib::db::paths;
use wmessage_lib::evolution::candidate::{self, ProposalStatus};
use wmessage_lib::evolution::change::{self, ApprovalSource, ChangeStatus};
use wmessage_lib::evolution::derive::derive_proposals;
use wmessage_lib::evolution::observe;
use wmessage_lib::evolution::policy::{self, ApplyPolicy};
use wmessage_lib::evolution::proposal::{
    Evidence, EvolutionProposal, ImpactLevel, ProposalCategory, ProposalOrigin, ProposalTarget,
    Suggestion,
};
use wmessage_lib::memory::embed;
use wmessage_lib::memory::store::{self, NewItem};

/// Box::leak 刻意泄漏 App（拿到 'static 句柄）；集成测试同款惯用形
/// （memory_conflict / llm_integration / skill_e2e 五处先例），泄漏量个位数。
fn mock_handle() -> tauri::AppHandle<tauri::test::MockRuntime> {
    Box::leak(Box::new(tauri::test::mock_app()))
        .handle()
        .clone()
}

/// 共享库写互斥的测试取锁（仓库 C3-1 约定：poison 留痕后 into_inner 复原）
fn db_write_lock() -> std::sync::MutexGuard<'static, ()> {
    db::DB_WRITE_LOCK.lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] tests::evolution_gov db::DB_WRITE_LOCK: {e:?}");
        e.into_inner()
    })
}

fn proposals_path(app: &tauri::AppHandle<tauri::test::MockRuntime>) -> std::path::PathBuf {
    paths::data_dir(app).join("evolution-proposals.jsonl")
}

fn changes_path(app: &tauri::AppHandle<tauri::test::MockRuntime>) -> std::path::PathBuf {
    paths::data_dir(app).join("evolution-changes.jsonl")
}

fn applied_path(app: &tauri::AppHandle<tauri::test::MockRuntime>) -> std::path::PathBuf {
    paths::data_dir(app).join("evolution-applied.jsonl")
}

/// 清场：删三 jsonl（本组用例独占写）+ 按标记 key 删库内 lesson/靶子。
fn cleanup(app: &tauri::AppHandle<tauri::test::MockRuntime>, ids: &[String]) {
    let _ = std::fs::remove_file(proposals_path(app));
    let _ = std::fs::remove_file(changes_path(app));
    let _ = std::fs::remove_file(applied_path(app));
    if let Ok(conn) = db::open_db(app) {
        let _db = db_write_lock();
        for id in ids {
            let _ = store::delete_by_key_tag(&conn, &format!("evo:{id}"));
            let _ = store::delete_by_key_tag(&conn, &format!("u20-gov-target-{id}"));
        }
    }
}

/// bot-config.json 现场恢复（共享 deps 目录；set 只动 evolution.applyPolicy，
/// 其余字段原样保留；结束恢复原样 / 原本没有则删）。
fn restore_cfg(original_cfg: Option<String>, cfg: &std::path::Path) {
    match original_cfg {
        Some(raw) => {
            let _ = std::fs::write(cfg, raw);
        }
        None => {
            let _ = std::fs::remove_file(cfg);
        }
    }
}

/// Drop 守卫：本用例触碰共享 deps 目录的 evolution 三 jsonl + 库内标记行 +
/// bot-config.json。中途 assert 失败（栈展开）时段尾显式清场会被跳过，残留
/// 会污染**下一轮运行**（nextest 每测试一进程，但数据目录跨轮共享）——drop
/// 兜底执行同样的清理；清理幂等（文件/行不存在均忽略），段中已清过也无副作用。
struct GovGuard {
    app: tauri::AppHandle<tauri::test::MockRuntime>,
    ids: std::cell::RefCell<Vec<String>>,
    original_cfg: Option<String>,
    cfg: std::path::PathBuf,
}

impl GovGuard {
    fn add_id(&self, id: &str) {
        self.ids.borrow_mut().push(id.to_string());
    }
}

impl Drop for GovGuard {
    fn drop(&mut self) {
        cleanup(&self.app, &self.ids.borrow());
        restore_cfg(self.original_cfg.clone(), &self.cfg);
    }
}

fn mk_proposal(
    id: &str,
    category: ProposalCategory,
    impact: ImpactLevel,
    text: &str,
) -> EvolutionProposal {
    EvolutionProposal {
        proposal_id: id.into(),
        created_at_ms: 1_700_000_000_000,
        origin: ProposalOrigin::ConsolidationReflection,
        category,
        target: ProposalTarget::MemoryPolicy {
            policy: "u20-gov".into(),
        },
        impact,
        evidence: Evidence {
            summary: format!("u20-gov evidence {id}"),
            occurrence_count: 1,
            window_hours: 24,
            related_refs: vec![format!("ref-{id}")],
        },
        suggestion: Suggestion {
            text: text.into(),
            structured_patch: None,
        },
    }
}

/// 主叙事：空指标 → confirm 档 → ops 派生 → 入池 → toggle ON 落库 →
/// 重复 toggle 幂等 → 非 policy 层不落库 → 防劫持拒写。
#[test]
fn confirm_mode_e2e_toggle_applies_and_is_idempotent() {
    let app = mock_handle();
    let nonpolicy_id = "u20-gov-nonpolicy";
    let conflict_id = "u20-gov-conflict";
    let cfg = paths::data_dir(&app).join("bot-config.json");
    let original_cfg = std::fs::read_to_string(&cfg).ok();
    // 清场守卫先立（panic 展开也清）；pid 段 0 派生后再补进 ids
    let guard = GovGuard {
        app: app.clone(),
        ids: std::cell::RefCell::new(vec![nonpolicy_id.into(), conflict_id.into()]),
        original_cfg: original_cfg.clone(),
        cfg: cfg.clone(),
    };

    // ── 段 0：真 ops → 真启发式派生（post_consolidation 前半；先派生拿稳定
    // pid——derive 对同 ops 确定性，跨次运行同 id，清理才能对准上一轮残留）──
    let ops = vec![
        wmessage_lib::memory::consolidate::ConsolidateOp::Contradiction {
            keep: "u20-gov-keep-1".into(),
            drop_id: "u20-gov-drop-1".into(),
            content: "矛盾裁决语料：用户主仓库从 GitHub 迁到自建 GitLab".into(),
        },
    ];
    let proposals = derive_proposals(
        &ops,
        &wmessage_lib::memory::consolidate::ConsolidateReport::default(),
    );
    assert_eq!(
        proposals.len(),
        1,
        "Contradiction op 必产 1 条 MemoryHint High"
    );
    assert_eq!(proposals[0].category, ProposalCategory::MemoryHint);
    assert_eq!(
        proposals[0].impact,
        ImpactLevel::High,
        "门槛内（auto 档会被自动轨吃掉的那类）"
    );
    let pid = proposals[0].proposal_id.clone();
    guard.add_id(&pid);

    cleanup(
        &app,
        &[pid.clone(), nonpolicy_id.into(), conflict_id.into()],
    );

    // ── 段 1：空 jsonl → 四指标零值（W3 薄壳的纯函数内核）──
    let empty_proposals = candidate::read_all(&proposals_path(&app)).unwrap();
    let empty_changes = change::read_all(&changes_path(&app)).unwrap();
    let empty_applied = wmessage_lib::eval::metrics::read_applied(&applied_path(&app)).unwrap();
    assert!(
        empty_proposals.is_empty() && empty_changes.is_empty() && empty_applied.is_empty(),
        "前置：三 jsonl 已清场"
    );
    let now = chrono::Utc::now().timestamp_millis();
    let m = observe::compute_metrics(
        &empty_proposals,
        &empty_changes,
        &empty_applied,
        now,
        now - 30 * 86_400_000,
    );
    assert_eq!(
        (
            m.candidate_generation_rate,
            m.approval_rate,
            m.rollback_rate,
            m.pollution_survival_days
        ),
        (0.0, 0.0, 0.0, 0.0),
        "空 jsonl 全零值"
    );
    assert_eq!(
        (
            m.proposal_total,
            m.promoted_count,
            m.rolled_back_count,
            m.active_lessons
        ),
        (0, 0, 0, 0)
    );

    // ── 段 2：confirm 档接线（写真 bot-config.json，生产同一读取器）──
    // 共享目录上 llm_integration 的 U15 门禁用例会整文件覆写/删除 bot-config.json
    //（并行进程），set→assert 之间被插一脚会读到无 applyPolicy 的版本——set 幂等，
    // 重试三次对冲（共享目录互踩的既有容忍口径，paths.rs 留档）。
    let mut gated_ok = false;
    for _ in 0..3 {
        policy::set_apply_policy(&app, ApplyPolicy::Confirm).unwrap();
        if !policy::auto_apply_allowed(Some(&app)) {
            gated_ok = true;
            break;
        }
    }
    assert!(
        gated_ok,
        "confirm 档：post_consolidation 分流谓词必须拒绝自动落库"
    );

    // ── 段 3：候选池真写盘（write_proposals，post_consolidation 同一函数）──
    // write_proposals 现返回新写入条目（通知中心逐条落消息用，T1 后 N1 批次）
    let written = candidate::write_proposals(&app, &proposals).unwrap();
    assert_eq!(written.len(), 1, "新提案应入池");
    assert_eq!(written[0].proposal_id, pid, "返回条目即新写入的提案");
    let pooled = candidate::read_all(&proposals_path(&app)).unwrap();
    assert_eq!(pooled.len(), 1);
    assert_eq!(pooled[0].proposal_id, pid);
    assert_eq!(
        pooled[0].status,
        ProposalStatus::Pooled,
        "confirm 档全留池等板"
    );

    // ── 段 4：面板 toggle ON → 人工批准执行器落库（W1）──
    let cr = wmessage_lib::evolution::panel::commands::toggle_inner(&app, &pid, true)
        .expect("policy 层 toggle ON 应成功")
        .expect("ON 返回 CR");
    assert_eq!(
        cr.status,
        ChangeStatus::Active,
        "落库成功 CR 合法流转到 Active"
    );
    assert_eq!(cr.approval_source, ApprovalSource::HumanApproved);
    assert_eq!(cr.human_approver.as_deref(), Some("boss"));

    // lesson 真落 mem_items：evo:<pid> key（下轮 injection_block 自动带出的通路）
    let conn = db::open_db(&app).unwrap();
    store::ensure_table(&conn).unwrap();
    let lesson = store::find_by_key_tag(&conn, &format!("evo:{pid}"))
        .unwrap()
        .expect("toggle ON 应落 evo:<pid> lesson");
    assert_eq!(lesson.kind, "lesson");
    assert_eq!(lesson.source, "system");
    assert_eq!(lesson.content, proposals[0].suggestion.text);

    // applied.jsonl 留痕 + 提案晋升
    let applied = wmessage_lib::eval::metrics::read_applied(&applied_path(&app)).unwrap();
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].proposal_id, pid);
    assert_eq!(applied[0].mem_key, format!("evo:{pid}"));
    let promoted = candidate::read_all(&proposals_path(&app)).unwrap();
    assert_eq!(promoted[0].status, ProposalStatus::Promoted);

    // ── 段 5：重复 toggle ON 幂等（同提案批两次一条 lesson，零堆积）──
    wmessage_lib::evolution::panel::commands::toggle_inner(&app, &pid, true).unwrap();
    let all = store::load_all(&conn).unwrap();
    let lesson_count = all
        .iter()
        .filter(|m| m.tags.iter().any(|t| t == &format!("evo:{pid}")))
        .count();
    assert_eq!(lesson_count, 1, "重复批仍是一条 lesson");
    let applied2 = wmessage_lib::eval::metrics::read_applied(&applied_path(&app)).unwrap();
    assert_eq!(applied2.len(), 1, "重复批不追加 applied 留痕");
    let changes = change::read_all(&changes_path(&app)).unwrap();
    assert_eq!(changes.len(), 1, "dedup 含 Active：重复批不新开 CR 行");
    assert_eq!(changes[0].status, ChangeStatus::Active);

    // ── 段 6：非 policy 层（PromptHint）toggle ON 维持旧口径 ──
    let nonpolicy = mk_proposal(
        nonpolicy_id,
        ProposalCategory::PromptHint,
        ImpactLevel::High,
        &format!("prompt 建议文本 {nonpolicy_id}"),
    );
    let now_ms = chrono::Utc::now().timestamp_millis();
    candidate::append(
        &proposals_path(&app),
        &candidate::from_proposal(&nonpolicy, now_ms),
    )
    .unwrap();
    let cr2 = wmessage_lib::evolution::panel::commands::toggle_inner(&app, nonpolicy_id, true)
        .unwrap()
        .unwrap();
    assert_eq!(
        cr2.status,
        ChangeStatus::Pending,
        "非 policy 层只登记 pending"
    );
    assert!(
        store::find_by_key_tag(&conn, &format!("evo:{nonpolicy_id}"))
            .unwrap()
            .is_none(),
        "非 policy 层不落 lesson"
    );

    // ── 段 7：防劫持拒写（ConflictRefused → Err + CR pending + 原行不劫持）──
    // 依赖真实嵌入引擎构造 cos=1.0 撞车；引擎不可用时跳过本段（不阻塞无模型环境）。
    let conflict_text = format!("u20-gov 防劫持语料 {conflict_id}");
    if let Some(emb) = embed::embed_text(&conflict_text) {
        let conflict = mk_proposal(
            conflict_id,
            ProposalCategory::MemoryHint,
            ImpactLevel::Medium,
            &conflict_text,
        );
        candidate::append(
            &proposals_path(&app),
            &candidate::from_proposal(&conflict, now_ms),
        )
        .unwrap();
        // 既有用户记忆：同文本嵌入（cos=1.0 必命中 merge 目标）+ 异 key
        {
            let _db = db_write_lock();
            store::insert_item(
                &conn,
                &NewItem {
                    kind: "fact".into(),
                    content: format!("既有用户记忆 {conflict_id}"),
                    tags: vec![format!("u20-gov-target-{conflict_id}")],
                    importance: 3,
                    source: "user_stated".into(),
                },
                Some(&emb),
                now_ms,
            )
            .unwrap();
        }
        let err = wmessage_lib::evolution::panel::commands::toggle_inner(&app, conflict_id, true)
            .expect_err("防劫持闸应拒绝落库");
        assert!(err.contains("拒绝"), "错误应说明防劫持拒绝：{err}");
        let changes = change::read_all(&changes_path(&app)).unwrap();
        let cr3 = changes
            .iter()
            .find(|c| c.proposal_id == conflict_id)
            .expect("被拒提案 CR 已登记");
        assert_eq!(cr3.status, ChangeStatus::Pending, "被拒提案保持待决策");
        assert!(
            store::find_by_key_tag(&conn, &format!("evo:{conflict_id}"))
                .unwrap()
                .is_none(),
            "拒写不落 lesson"
        );
        let target = store::find_by_key_tag(&conn, &format!("u20-gov-target-{conflict_id}"))
            .unwrap()
            .expect("既有记忆仍在");
        assert_eq!(
            target.content,
            format!("既有用户记忆 {conflict_id}"),
            "原行未被劫持"
        );
    } else {
        eprintln!("[u20-gov] 嵌入引擎不可用，跳过防劫持段");
    }

    // ── 段 8：auto 档谓词复位 ──
    policy::set_apply_policy(&app, ApplyPolicy::Auto).unwrap();
    assert!(
        policy::auto_apply_allowed(Some(&app)),
        "auto 档放行自动落库"
    );

    // ── 清场：段中显式清一次 + guard drop 兜底（幂等，panic 路径也生效）──
    cleanup(&app, &[pid, nonpolicy_id.into(), conflict_id.into()]);
}
