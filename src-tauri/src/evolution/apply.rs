//! Phase 2：提案应用层——把达门槛的提案落成「lesson」记忆，闭环生效。
//!
//! 闭环路径：consolidate 反思 → derive 提案 → emit 写 audit → **本模块落记忆**
//! → 下轮对话 `injection_block` 自动带出（lesson 专属槽位 `rank::MEMORY_LESSON_N`）
//! → 行为改变。
//!
//! ## 设计约束
//!
//! - 只自动应用 `MemoryHint` 且 `impact ∈ {High, Medium}`；Low 与其余三类
//!   （PromptHint / ToolSchemaHint / SkillHint）永不自动应用，只走 emit 的 audit；
//! - 幂等：`tags[0] = "evo:<proposal_id>"`，写入前 `find_by_key_tag` 查重——
//!   持久幂等，同时弥补 emit 层进程内 24h dedup 重启丢失的缺口；
//! - 回滚 = `delete_by_key_tag("evo:<proposal_id>")`（见 `rollback_applied`）；
//! - 应用留痕：`{data_dir}/evolution-applied.jsonl` 每行一条 JSON（提案 id、
//!   记忆 key、应用时间、impact、summary），审计与人工回滚的依据；
//! - AppHandle 复用 `emit` 的全局注册（`post_consolidation` 签名不变的约束不变）；
//! - importance 从 impact 派生（High=4 / Medium=3），source="system"——
//!   可淘汰、非受保护，记忆库满时按既有淘汰分正常出局。

use rusqlite::Connection;

use super::proposal::{EvolutionProposal, ImpactLevel, ProposalCategory};
use crate::memory::store::{self, NewItem};

/// 自动应用门槛（纯函数）：仅 MemoryHint + High/Medium。
pub(crate) fn auto_apply_gate(p: &EvolutionProposal) -> bool {
    p.category == ProposalCategory::MemoryHint && p.impact != ImpactLevel::Low
}

/// 提案对应的记忆幂等 key（tags[0]）。
pub fn evolution_key(proposal_id: &str) -> String {
    format!("evo:{proposal_id}")
}

/// 单条应用结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// 新写入一条 lesson 记忆
    Applied,
    /// 同 key 已存在（持久幂等命中），跳过
    AlreadyPresent,
    /// B2-1（P1-EV3）：merge-on-write 命中异 key 既有行被拒写（防记忆劫持）——
    /// 调用方记 evolution.apply_conflict audit（store 层无 AppHandle）
    ConflictRefused { target_key: String },
}

/// 批量应用报告。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ApplyReport {
    pub applied: usize,
    pub already_present: usize,
    /// B2-1：拒写（防劫持）条数
    pub conflicts_refused: usize,
}

/// 应用单条提案（纯函数内核，注入连接与向量，内存库可单测）：
/// 幂等查重 → 写 lesson 记忆。语义去重由 `insert_item` 承担（≥0.92 合并更新）。
///
/// 调用前先过 `auto_apply_gate`；本函数不再重复判定类别/门槛。
/// B2-1（P1-EV3）幂等双保险：`find_by_key_tag` 只认 tags[0]，lesson 被 merge
/// 吸收/挪位后 key 查不到会重复应用——补两层残留复查：
/// ① key tag 出现在**任意** tag 位（挪位不丢）；② 同 kind=lesson 且内容逐字相同
/// （吸收后内容残存）。真正的语义相似由 store 层拒写闸兜底（ConflictRefused）。
pub fn apply_one(
    conn: &Connection,
    p: &EvolutionProposal,
    embedding: Option<&[f32]>,
    now_ms: i64,
) -> Result<ApplyOutcome, String> {
    let key = evolution_key(&p.proposal_id);
    let all = store::load_all(conn)?;
    if all.iter().any(|m| m.tags.iter().any(|t| t == &key)) {
        return Ok(ApplyOutcome::AlreadyPresent);
    }
    if all
        .iter()
        .any(|m| m.kind == "lesson" && m.content == p.suggestion.text)
    {
        return Ok(ApplyOutcome::AlreadyPresent);
    }
    drop(all);
    let item = NewItem {
        kind: "lesson".to_string(),
        content: p.suggestion.text.clone(),
        tags: vec![key, "evolution".to_string()],
        importance: match p.impact {
            ImpactLevel::High => 4,
            _ => 3,
        },
        source: "system".to_string(),
    };
    let (outcome, _merged_ids) = store::insert_item(conn, &item, embedding, now_ms)?;
    match outcome {
        store::InsertOutcome::RefusedForeignMerge { target_key } => {
            Ok(ApplyOutcome::ConflictRefused { target_key })
        }
        _ => Ok(ApplyOutcome::Applied),
    }
}

/// 回滚：删除某提案落下的记忆。返回是否有条目被删。
pub fn rollback_applied(conn: &Connection, proposal_id: &str) -> Result<bool, String> {
    store::delete_by_key_tag(conn, &evolution_key(proposal_id))
}

/// applied.jsonl 的一行记录。
#[derive(serde::Serialize)]
struct AppliedRecord<'a> {
    proposal_id: &'a str,
    mem_key: String,
    applied_at_ms: i64,
    impact: &'a str,
    summary: &'a str,
}

/// 追加一条应用留痕（纯函数内核，路径由调用方给，测试用临时文件）。
pub fn append_applied_record(
    path: &std::path::Path,
    p: &EvolutionProposal,
    now_ms: i64,
) -> Result<(), String> {
    let rec = AppliedRecord {
        proposal_id: &p.proposal_id,
        mem_key: evolution_key(&p.proposal_id),
        applied_at_ms: now_ms,
        impact: p.impact.as_str(),
        summary: &p.evidence.summary,
    };
    let line = serde_json::to_string(&rec).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    writeln!(f, "{line}").map_err(|e| e.to_string())
}

/// 门面：consolidate 反思后的应用入口（`post_consolidation` 调用）。
///
/// 入参应已过 `auto_apply_gate`。内部派生独立阻塞任务（嵌入在持锁前算，
/// 与 memory 门面层同纪律），fire-and-forget：失败只进 audit / eprintln，
/// 不影响 consolidate 主链路。
pub fn apply_from_consolidation(proposals: Vec<EvolutionProposal>) {
    if proposals.is_empty() {
        return;
    }
    let Some(app) = super::emit::app_handle() else {
        eprintln!(
            "[evolution] AppHandle 未注册，跳过 apply（{} 条提案）",
            proposals.len()
        );
        return;
    };
    let app = app.clone();

    // [R6 A] shadow 钩子（仅当 evolution.shadow.enabled=true 时）
    // 克隆 proposals 供 shadow spawn（主 apply 仍用原 proposals）
    let shadow_proposals = if crate::evolution::observe::shadow::is_enabled(&app) {
        Some(proposals.clone())
    } else {
        None
    };
    // [R6 A] 先 clone app 供 shadow spawn（主 spawn 会移走 app）
    let app_for_shadow = app.clone();

    tauri::async_runtime::spawn(async move {
        let app2 = app.clone();
        let r = tauri::async_runtime::spawn_blocking(move || -> Result<ApplyReport, String> {
            // 嵌入在持锁前批量算好（ONNX 推理数十 ms，不占 DB 写锁临界区）
            let embs: Vec<Option<Vec<f32>>> = proposals
                .iter()
                .map(|p| crate::memory::embed::embed_text(&p.suggestion.text))
                .collect();
            // embed_text 返回 None = 空文本或推理失败（调用侧不可区分）——
            // 无向量的 lesson 永不可语义召回，留痕不静默（audit 落 bot.log）。
            let embed_failed = embs.iter().filter(|e| e.is_none()).count();
            if embed_failed > 0 {
                crate::audit_event!(
                    &app2,
                    crate::audit::AuditLevel::Warn,
                    "evolution.embed_failed",
                    "failed" => embed_failed.to_string(),
                    "total" => proposals.len().to_string(),
                );
            }
            // 锁外预计算：open_db（文件打开 syscall，内部不拿 DB_WRITE_LOCK）、
            // ledger 路径、now——不占临界区
            let conn = crate::db::open_db(&app2)?;
            let now = crate::memory::now_ms();
            let ledger = crate::db::paths::data_dir(&app2).join("evolution-applied.jsonl");
            // DDL 单独一个短临界区
            {
                let _g = crate::db::DB_WRITE_LOCK.lock().unwrap_or_else(|e| {
                    eprintln!("[mutex_poisoned] evolution::apply DB_WRITE_LOCK: {e:?}");
                    e.into_inner()
                });
                store::ensure_table(&conn)?;
            }
            let mut report = ApplyReport::default();
            for (p, emb) in proposals.iter().zip(embs.iter()) {
                // 临界区仅覆盖 SQL 写；jsonl append + audit emit 在锁外——
                // 无关 DB 写者不再被整条流水线（syscall/emit）串行化
                let outcome = {
                    let _g = crate::db::DB_WRITE_LOCK.lock().unwrap_or_else(|e| {
                        eprintln!("[mutex_poisoned] evolution::apply DB_WRITE_LOCK: {e:?}");
                        e.into_inner()
                    });
                    apply_one(&conn, p, emb.as_deref(), now)?
                };
                match outcome {
                    ApplyOutcome::Applied => {
                        report.applied += 1;
                        append_applied_record(&ledger, p, now)?;
                        // B1-3（P0-EV2 另半）：补写 ChangeRecord（Active + AutoApplied）
                        // 到 evolution-changes.jsonl——面板回滚只读这个文件，此前自动
                        // 应用只落 applied.jsonl（面板不读它），回滚对自动应用不可达。
                        // 构造走 change::derive::auto_applied_from_proposal（合法流转，
                        // 不裸写 status）；构造/落盘失败仅 Warn audit——lesson 已生效，
                        // 不让留痕问题炸掉整条 apply。
                        let cr_path =
                            crate::db::paths::data_dir(&app2).join("evolution-changes.jsonl");
                        // B2-2（P1-EV4）：changes.jsonl 收敛单写者锁——apply 与
                        // shadow/panel 同锁（EVOLUTION_STORE_LOCK），闭包内同步持锁
                        // 不跨 await。锁序：此处 DB_WRITE_LOCK 已释放才取本锁，
                        // 与 panel（本锁内开 DB 连接）无环。
                        match super::change::derive::auto_applied_from_proposal(p, now) {
                            Ok(mut cr) => {
                                let _store = super::lock_evolution_store();
                                // B2-4：chg-<pid> 可能被 toggle/shadow 历史行占用，
                                // 锁内读存量派生唯一 id 再落行
                                let rows = super::change::read_all(&cr_path).unwrap_or_default();
                                cr.change_id =
                                    super::change::unique_change_id_for(&rows, &cr.proposal_id);
                                if let Err(e) = super::change::append_change(&cr_path, &cr) {
                                    crate::audit_event!(
                                        &app2,
                                        crate::audit::AuditLevel::Warn,
                                        "evolution.apply_change_record_failed",
                                        "proposal_id" => p.proposal_id.clone(),
                                        "error" => e,
                                    );
                                }
                            }
                            Err(e) => {
                                crate::audit_event!(
                                    &app2,
                                    crate::audit::AuditLevel::Warn,
                                    "evolution.apply_change_record_failed",
                                    "proposal_id" => p.proposal_id.clone(),
                                    "error" => e,
                                );
                            }
                        }
                        crate::audit_event!(
                            &app2,
                            crate::audit::AuditLevel::Info,
                            "evolution.applied",
                            "proposal_id" => p.proposal_id.clone(),
                            "impact" => p.impact.as_str(),
                            "mem_key" => evolution_key(&p.proposal_id),
                        );
                    }
                    ApplyOutcome::AlreadyPresent => report.already_present += 1,
                    ApplyOutcome::ConflictRefused { target_key } => {
                        // B2-1（P1-EV3 防劫持）：拒写响亮留痕，不静默丢 lesson
                        report.conflicts_refused += 1;
                        crate::audit_event!(
                            &app2,
                            crate::audit::AuditLevel::Warn,
                            "evolution.apply_conflict",
                            "proposal_id" => p.proposal_id.clone(),
                            "target_key" => target_key,
                            "action" => "lesson_write_refused_foreign_merge",
                        );
                    }
                }
            }
            Ok(report)
        })
        .await;
        match r {
            Ok(Ok(report)) if report.applied > 0 => {
                crate::bot::audit_log_hook(
                    &app,
                    &format!(
                        "evolution_apply | applied: {} | already_present: {}",
                        report.applied, report.already_present
                    ),
                );
            }
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                crate::audit_event!(&app, crate::audit::AuditLevel::Warn, "evolution.apply_failed",
                    "error" => e);
            }
            Err(e) => {
                crate::audit_event!(&app, crate::audit::AuditLevel::Warn, "evolution.apply_failed",
                    "error" => format!("应用线程 join 失败：{e}"));
            }
        }
    });

    // [R6 A] fire shadow（fire-and-forget；仅当 flag=true 时执行）
    if let Some(proposals_shadow) = shadow_proposals {
        let app_shadow = app_for_shadow;
        // 并发语义（老板 13:20 拍板后补）：
        // - app_for_shadow 是 Arc<Wry> 的克隆，廉价（reference count bump）
        // - 此 spawn 失败（panic）由 tauri::async_runtime 捕获，不传播到主 spawn
        // - 任务生命周期：spawn_blocking 不阻塞此 spawn；fire-and-forget 无超时（依赖外部观察）
        // - 共享状态：main spawn 用 proposals 引用，shadow spawn 用 proposals_shadow 克隆——无共享 mutation
        tauri::async_runtime::spawn(async move {
            crate::evolution::observe::shadow::shadow_apply_for_batch_with_app(
                proposals_shadow,
                &app_shadow,
            )
            .await;
        });
    }
}

// ───────────────────────── 单元测试 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::proposal::{Evidence, ProposalOrigin, ProposalTarget, Suggestion};

    fn mem_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        store::ensure_table(&conn).unwrap();
        conn
    }

    fn make_proposal(id: &str, impact: ImpactLevel) -> EvolutionProposal {
        EvolutionProposal {
            proposal_id: id.into(),
            created_at_ms: 1_700_000_000_000,
            origin: ProposalOrigin::ConsolidationReflection,
            category: ProposalCategory::MemoryHint,
            target: ProposalTarget::MemoryPolicy {
                policy: "test".into(),
            },
            impact,
            evidence: Evidence {
                summary: format!("summary-{id}"),
                occurrence_count: 1,
                window_hours: 24,
                related_refs: vec![],
            },
            suggestion: Suggestion {
                text: format!("自我改进建议 {id}：合并相似记忆"),
                structured_patch: None,
            },
        }
    }

    // ─── 门槛 ───

    #[test]
    fn gate_passes_high_and_medium_memory_hint() {
        assert!(auto_apply_gate(&make_proposal("h", ImpactLevel::High)));
        assert!(auto_apply_gate(&make_proposal("m", ImpactLevel::Medium)));
    }

    #[test]
    fn gate_rejects_low() {
        assert!(!auto_apply_gate(&make_proposal("l", ImpactLevel::Low)));
    }

    #[test]
    fn gate_rejects_non_memory_hint() {
        let mut p = make_proposal("p", ImpactLevel::High);
        p.category = ProposalCategory::PromptHint;
        assert!(!auto_apply_gate(&p));
        p.category = ProposalCategory::ToolSchemaHint;
        assert!(!auto_apply_gate(&p));
        p.category = ProposalCategory::SkillHint;
        assert!(!auto_apply_gate(&p));
    }

    // ─── 应用与幂等 ───

    #[test]
    fn apply_one_inserts_lesson_with_evolution_key() {
        let conn = mem_conn();
        let p = make_proposal("a1", ImpactLevel::High);
        let r = apply_one(&conn, &p, None, 1000).unwrap();
        assert_eq!(r, ApplyOutcome::Applied);
        let m = store::find_by_key_tag(&conn, "evo:a1").unwrap().unwrap();
        assert_eq!(m.kind, "lesson");
        assert_eq!(m.source, "system");
        assert_eq!(m.importance, 4, "High → importance 4");
        assert_eq!(m.content, p.suggestion.text);
        assert_eq!(m.tags, vec!["evo:a1", "evolution"]);
    }

    #[test]
    fn apply_one_medium_maps_importance_3() {
        let conn = mem_conn();
        let p = make_proposal("m1", ImpactLevel::Medium);
        apply_one(&conn, &p, None, 1000).unwrap();
        let m = store::find_by_key_tag(&conn, "evo:m1").unwrap().unwrap();
        assert_eq!(m.importance, 3, "Medium → importance 3");
    }

    #[test]
    fn apply_one_is_idempotent_by_key() {
        let conn = mem_conn();
        let p = make_proposal("idem", ImpactLevel::High);
        assert_eq!(
            apply_one(&conn, &p, None, 1000).unwrap(),
            ApplyOutcome::Applied
        );
        assert_eq!(
            apply_one(&conn, &p, None, 2000).unwrap(),
            ApplyOutcome::AlreadyPresent,
            "同 proposal_id 第二次应用必须幂等跳过"
        );
        assert_eq!(store::count_by_kind(&conn, "lesson").unwrap(), 1);
    }

    // ─── 回滚 ───

    #[test]
    fn rollback_deletes_applied_memory() {
        let conn = mem_conn();
        let p = make_proposal("rb", ImpactLevel::High);
        apply_one(&conn, &p, None, 1000).unwrap();
        assert!(rollback_applied(&conn, "rb").unwrap(), "应删到条目");
        assert!(store::find_by_key_tag(&conn, "evo:rb").unwrap().is_none());
        assert!(
            !rollback_applied(&conn, "rb").unwrap(),
            "二次回滚返回 false（无条目可删）"
        );
    }

    // ─── 留痕 ───

    #[test]
    fn applied_ledger_appends_jsonl() {
        let dir = std::env::temp_dir().join(format!(
            "wmessage-evo-test-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let path = dir.join("sub").join("evolution-applied.jsonl");
        let p = make_proposal("j1", ImpactLevel::High);
        append_applied_record(&path, &p, 1000).unwrap();
        append_applied_record(&path, &p, 2000).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "两条记录两行");
        let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v["proposal_id"], "j1");
        assert_eq!(v["mem_key"], "evo:j1");
        assert_eq!(v["applied_at_ms"], 1000);
        assert_eq!(v["impact"], "high");
        assert_eq!(v["summary"], "summary-j1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn applied_proposal_records_active_change() {
        // B1-3（P0-EV2 另半）回归：apply 落 lesson 的同时补写 Active+AutoApplied
        // 的 ChangeRecord——面板 rollback 只读 changes.jsonl（load_changes →
        // change::read_all），缺这条则自动应用对回滚不可达（P0-EV2 的另一半）。
        // 构造走生产同一入口 auto_applied_from_proposal（合法流转达成 Active，
        // 不裸写 status），锁「写进去读得回、形态是面板回滚接受的样子」。
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("evolution-changes.jsonl");
        let p = make_proposal("cr1", ImpactLevel::Medium);
        let cr = super::super::change::derive::auto_applied_from_proposal(&p, 1000)
            .expect("门槛内提案应可合法流转到 Active");
        super::super::change::append_change(&path, &cr).unwrap();
        let all = super::super::change::read_all(&path).unwrap();
        assert_eq!(all.len(), 1, "一条 apply 一条 CR");
        assert_eq!(all[0].proposal_id, "cr1");
        assert_eq!(all[0].change_id, "chg-cr1");
        assert_eq!(
            all[0].status,
            super::super::change::record::ChangeStatus::Active
        );
        assert_eq!(
            all[0].approval_source,
            super::super::change::record::ApprovalSource::AutoApplied
        );
        assert!(all[0].hard_constraint_compliance);
    }

    #[test]
    fn auto_applied_cr_rejects_non_gate_proposal() {
        // 门槛外提案（from_proposal → 终态 Rejected）流转必须 Err（不裸写绕状态机）
        let p = make_proposal("low1", ImpactLevel::Low);
        assert!(
            super::super::change::derive::auto_applied_from_proposal(&p, 1000).is_err(),
            "Low 不过 auto_apply_gate，CR 构造应拒绝"
        );
    }

    // ─── B2-1（P1-EV3）幂等双保险 ───

    #[test]
    fn apply_one_rechecks_key_in_any_tag_position() {
        // key tag 被挪到非首位（merge/整理扰动）时仍能查重
        let conn = mem_conn();
        let item = store::NewItem {
            kind: "fact".into(),
            content: "无关内容".into(),
            tags: vec!["evolution".into(), "evo:any1".into()],
            importance: 2,
            source: "user".into(),
        };
        store::insert_item(&conn, &item, None, 900).unwrap();
        let p = make_proposal("any1", ImpactLevel::Medium);
        let r = apply_one(&conn, &p, None, 1000).unwrap();
        assert_eq!(r, ApplyOutcome::AlreadyPresent, "key 在任意位即命中");
    }

    #[test]
    fn apply_one_rechecks_same_content_lesson() {
        // key 彻底丢失但同内容 lesson 仍在（吸收残存）→ 不重复写入
        let conn = mem_conn();
        let p = make_proposal("same1", ImpactLevel::Medium);
        let item = store::NewItem {
            kind: "lesson".into(),
            content: p.suggestion.text.clone(),
            tags: vec!["unrelated".into()],
            importance: 3,
            source: "system".into(),
        };
        store::insert_item(&conn, &item, None, 900).unwrap();
        let r = apply_one(&conn, &p, None, 1000).unwrap();
        assert_eq!(r, ApplyOutcome::AlreadyPresent, "同内容 lesson 即命中");
        assert_eq!(store::load_all(&conn).unwrap().len(), 1, "不重复落行");
    }

    #[test]
    fn apply_one_refuses_foreign_merge_and_preserves_target() {
        // 同向量的既有用户记忆（异 key）：cos=1.0 命中 merge → 防劫持闸拒写
        let conn = mem_conn();
        let emb = vec![1.0f32, 0.0, 0.0];
        let user = store::NewItem {
            kind: "fact".into(),
            content: "用户既有记忆".into(),
            tags: vec!["用户键".into()],
            importance: 3,
            source: "user_stated".into(),
        };
        store::insert_item(&conn, &user, Some(&emb), 900).unwrap();
        let p = make_proposal("hij1", ImpactLevel::High);
        let r = apply_one(&conn, &p, Some(&emb), 1000).unwrap();
        match r {
            ApplyOutcome::ConflictRefused { target_key } => {
                assert_eq!(target_key, "用户键");
            }
            other => panic!("应 ConflictRefused，实得 {other:?}"),
        }
        let all = store::load_all(&conn).unwrap();
        assert_eq!(all.len(), 1, "拒写不落库");
        assert_eq!(all[0].content, "用户既有记忆", "原行未被劫持");
        assert_eq!(all[0].tags, vec!["用户键"]);
    }
}
