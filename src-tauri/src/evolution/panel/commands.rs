//! R5 决策面板 · 后端 Tauri commands
//!
//! 6 个 commands：
//! 1. `evolution_list_proposals(status?)` — 候选池列表
//! 2. `evolution_promote_proposal(id, interactive, session_id)` — 晋升（Pooled → Promoted + 创建 ChangeRecord）
//! 3. `evolution_reject_proposal(id, interactive, session_id)` — 拒绝（任意 → Rejected）
//! 4. `evolution_keep_shadow(id)` — 延长 shadow 期（重置 TTL）
//! 5. `evolution_list_changes()` — ChangeRecord 列表（回滚 UI）
//! 6. `evolution_rollback_change(id, interactive, session_id)` — 回滚（删 mem_item + status=RolledBack）
//!
//! U20 增量：
//! - W1 人工批准执行器：policy 层（MemoryHint）提案 toggle ON 即 `apply_one`
//!   落库（幂等），CR pending→Active——此前批准只登记 pending、无执行器生效；
//! - W2 治理开关：`evolution_get/set_apply_policy`（evolution.applyPolicy 二档）；
//! - W3 冒烟：`evolution_metrics` 薄壳包 observe::compute_metrics。
//!
//! 全部走 ask_user_confirm 复用 ConfirmMap（spec R0 #1 默认 A）。

use std::path::PathBuf;
use tauri::AppHandle;

use crate::audit::AuditLevel;
use crate::bot_slash;
use crate::db::paths;
use crate::evolution::apply::{self, ApplyOutcome};
use crate::evolution::candidate::{self, ProposalEntry, ProposalStatus};
use crate::evolution::change::{self, ApprovalSource, ChangeRecord, ChangeStatus, EvolutionLayer};
use crate::evolution::lock_evolution_store;
use crate::evolution::observe::{self, ObserveMetrics};
use crate::evolution::policy::{read_apply_policy, set_apply_policy, ApplyPolicy};
use crate::evolution::proposal::EvolutionProposal;

// ───────────────────────── 路径辅助 ─────────────────────────

fn proposals_path<R: tauri::Runtime>(app: &AppHandle<R>) -> PathBuf {
    paths::data_dir(app).join("evolution-proposals.jsonl")
}

fn changes_path<R: tauri::Runtime>(app: &AppHandle<R>) -> PathBuf {
    paths::data_dir(app).join("evolution-changes.jsonl")
}

fn applied_path<R: tauri::Runtime>(app: &AppHandle<R>) -> PathBuf {
    paths::data_dir(app).join("evolution-applied.jsonl")
}

// ───────────────────────── 读写辅助 ─────────────────────────

fn load_proposals<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<Vec<ProposalEntry>, String> {
    candidate::read_all(&proposals_path(app))
}

fn load_changes<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<Vec<ChangeRecord>, String> {
    change::read_all(&changes_path(app))
}

/// 整体重写 jsonl（更新 status / TTL 用）。
/// 全量序列化后走 tmp+rename 原子落盘——truncate+逐行写在崩溃时留空/半截文件
///（jsonl 是提案生命周期唯一持久化）。
fn rewrite_jsonl<T: serde::Serialize>(path: &PathBuf, entries: &[T]) -> Result<(), String> {
    use std::fmt::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建目录 {parent:?} 失败：{e}"))?;
    }
    let mut buf = String::new();
    for e in entries {
        let line = serde_json::to_string(e).map_err(|e| format!("序列化失败：{e}"))?;
        writeln!(buf, "{line}").map_err(|e| format!("拼接缓冲失败：{e}"))?;
    }
    crate::db::paths::atomic_write(path, &buf)
}

fn parse_status_filter(s: &str) -> Result<ProposalStatus, String> {
    match s {
        "pooled" => Ok(ProposalStatus::Pooled),
        "promoted" => Ok(ProposalStatus::Promoted),
        "expired" => Ok(ProposalStatus::Expired),
        "rejected" => Ok(ProposalStatus::Rejected),
        other => Err(format!("未知 ProposalStatus：{other}")),
    }
}

// ───────────────────────── Toggle / Delete（老板 16:05 拍板）─────────────────────────
//
// 老板设计语义：
// - toggle ON  → 写 ChangeRecord(status=pending)，dedup by proposal_id（已有 pending 不重写）
// - toggle OFF → 移除 ChangeRecord，proposal.status → Rejected
// - delete     → 删 proposals.jsonl 行 + 级联删 changes.jsonl 中 status=pending 行
//                （老板拍板：只允许删 Pending；active/rolled_back 用 Rollback）
// Promote/Reject 命令保留但语义改成 toggle ON/OFF（兼容老 UI/调用方）

/// ON 返回 Some（锁内新写入或 dedup 命中的既有 ChangeRecord），OFF 返回 None。
/// promote 直接用返回值——**不要在锁外再 load_changes 找**（TOCTOU：并发
/// delete 会让锁外 find 落空，错误文案还会误导成「toggle_inner 写失败」）。
///
/// B2-4（拍板①）：已回滚提案允许再次 toggle ON（前端有回滚历史时先弹二次确认，
/// 见 EvolutionPanel onToggle）——新 CR 拿行级唯一 change_id（next_unique_change_id），
/// 回滚按 id 定位不再撞旧行。
///
/// U20 W1：policy 层（MemoryHint）提案 ON 即人工批准执行器落库——
/// `apply_one` 写 lesson（evo:<pid> 持久幂等，重复批零副作用）→ applied.jsonl
/// 留痕 → CR pending 合法流转到 Active。落库被防劫持闸拒绝（ConflictRefused）
/// 时 CR 保持 pending + 返回 Err（面板错误提示），可重试或停用。
/// 非 policy 层（Prompt/ToolSchema/SkillHint）维持原行为：只登记 pending CR。
/// auto 轨（consolidate 自动应用）不经此处，零改动。
///
/// 锁纪律（三段式，OCR R1 高位采纳：apply 不进 EVOLUTION_STORE_LOCK）：
/// ① 锁内——dedup/建 CR(pending) + 提案晋升 + rewrite（登记即落盘，此后的
///   apply 失败天然留下「待决策」盘面：CR pending 可重试/停用/删除）；
/// ② 锁外——human_apply_one（open_db + DB_WRITE_LOCK + apply_one）+ applied
///   留痕 + 审计（同 auto 轨「DB 全家桶不进 store 锁、锁内不夹审计 IO」纪律）；
/// ③ 锁内——CR 合法流转到 Active，仅状态真正变化才 rewrite（重复批 no-op）。
#[doc(hidden)] // 内部内核，pub 仅为集成测试可见（run_extract_with 先例）
pub fn toggle_inner<R: tauri::Runtime>(
    app: &AppHandle<R>,
    proposal_id: &str,
    enabled: bool,
) -> Result<Option<ChangeRecord>, String> {
    // W1：嵌入在持锁前算（同 apply_from_consolidation「嵌入不进锁」纪律）——
    // 无锁预读只为定位 suggestion 文本；预读落空（并发写/非 policy 层）= None，
    // lesson 照常落库仅无向量（auto 轨 embed_failed 同口径）。
    let pre_emb = if enabled {
        precompute_human_apply_embedding(app, proposal_id)
    } else {
        None
    };

    // ── 段 ①：持 store 锁完成登记（CR pending + 提案晋升）──
    let (mut cr, apply_target) = {
        let _g = lock_evolution_store(); // OCR C3-4：覆盖整个 load→mutate→rewrite 窗口
        let p_path = proposals_path(app);
        let c_path = changes_path(app);

        let mut proposals = load_proposals(app)?;
        let idx = proposals
            .iter()
            .position(|e| e.proposal_id == proposal_id)
            .ok_or_else(|| format!("proposal {proposal_id} 不存在"))?;
        let mut entry = proposals[idx].clone();

        let mut changes = load_changes(app)?;

        if !enabled {
            // Toggle OFF：移除 pending ChangeRecord
            let before = changes.len();
            changes
                .retain(|c| !(c.proposal_id == proposal_id && c.status == ChangeStatus::Pending));
            if changes.len() == before {
                return Err(format!(
                    "proposal {proposal_id} 没有 pending ChangeRecord，无法 toggle OFF"
                ));
            }
            rewrite_jsonl(&c_path, &changes)?;
            entry.status = ProposalStatus::Rejected;
            proposals[idx] = entry.clone();
            rewrite_jsonl(&p_path, &proposals)?;
            crate::audit_event!(
                app,
                AuditLevel::Warn,
                "evolution.toggle_off",
                "proposal_id" => proposal_id.to_string(),
            );
            return Ok(None);
        }

        // B4-4（P3 组）：终态校验——Rejected/Expired 是终态，不允许 toggle ON
        // 复活（此检查在 EVOLUTION_STORE_LOCK 内 = promote 段 B 的段间复检）；
        // RolledBack 复用走前端二次确认（拍板①），此处放行
        if matches!(
            entry.status,
            ProposalStatus::Rejected | ProposalStatus::Expired
        ) {
            return Err(format!(
                "proposal {proposal_id} 状态为 {:?}（终态），不可再启用",
                entry.status
            ));
        }
        // Toggle ON：dedup（已有 pending/shadowing/shadow_passed 不重写）。
        // U20 W1 口径加 Active：已生效提案重复拨 ON 不再新开 CR 行（重复批准
        // 零副作用），lesson 幂等由 apply_one 的 evo:<pid> 查重兜底。
        let existing = changes.iter().find(|c| {
            c.proposal_id == proposal_id
                && matches!(
                    c.status,
                    ChangeStatus::Pending
                        | ChangeStatus::Shadowing
                        | ChangeStatus::ShadowPassed
                        | ChangeStatus::Active
                )
        });
        let mut cr = match existing {
            Some(c) => {
                crate::audit_event!(
                    app,
                    AuditLevel::Info,
                    "evolution.toggle_on_dedup",
                    "proposal_id" => proposal_id.to_string(),
                );
                c.clone()
            }
            None => {
                let mut cr = candidate::to_change_record(&entry, true);
                cr.approval_source = ApprovalSource::HumanApproved;
                cr.human_approver = Some("boss".into());
                cr.status = ChangeStatus::Pending;
                // B2-4（P1-EV5）：复用 toggle（重启用）场景下基础 change_id 已被
                // 旧行占用——旧实现直接同 id 再 append，rollback 的 position() 首
                // 匹配永远命中旧行（二次回滚永久卡死）。改派生行级唯一 id：
                // chg-<pid> / chg-<pid>-2 / chg-<pid>-3 …（change::unique_change_id_for，
                // shadow/apply 同款）；并挂 parent_id 血缘指向前一条 CR。
                cr.change_id = change::unique_change_id_for(&changes, proposal_id);
                cr.parent_id = changes
                    .iter()
                    .filter(|c| c.proposal_id == proposal_id)
                    .last()
                    .map(|c| c.change_id.clone());
                change::append_change(&c_path, &cr)?;
                crate::audit_event!(
                    app,
                    AuditLevel::Info,
                    "evolution.toggle_on",
                    "proposal_id" => proposal_id.to_string(),
                    "change_id" => cr.change_id.clone(),
                );
                cr
            }
        };
        entry.status = ProposalStatus::Promoted;
        proposals[idx] = entry.clone();
        rewrite_jsonl(&p_path, &proposals)?;

        // U20 W1：只有 policy 层（MemoryHint）落 lesson 记忆；落库本身在段 ②
        // （锁外）执行——此处只把待落库提案带出锁。layer→category 双射反推，
        // Parameter/Code 两层（派生侧不产）防御性 Err。
        let apply_target = if entry.layer == EvolutionLayer::Policy {
            Some(entry_to_proposal(&entry).ok_or_else(|| {
                format!(
                    "proposal {proposal_id} layer={:?} 无对应提案类别，无法落库",
                    entry.layer
                )
            })?)
        } else {
            None
        };
        (cr, apply_target)
    };

    // ── 段 ②：锁外落库（失败语义：Err 向上抛，段 ① 盘面即「待决策」）──
    if let Some(p) = &apply_target {
        let outcome = human_apply_one(app, p, pre_emb.as_deref())?;
        match outcome {
            ApplyOutcome::Applied => {
                apply::append_applied_record(&applied_path(app), p, crate::memory::now_ms())?;
                crate::audit_event!(
                    app,
                    AuditLevel::Info,
                    "evolution.human_applied",
                    "proposal_id" => p.proposal_id.clone(),
                    "mem_key" => apply::evolution_key(&p.proposal_id),
                );
            }
            ApplyOutcome::AlreadyPresent => {}
            ApplyOutcome::ConflictRefused { target_key } => {
                // B2-1 防劫持闸：响亮留痕 + 面板错误提示（CR 保持 pending）
                crate::audit_event!(
                    app,
                    AuditLevel::Warn,
                    "evolution.apply_conflict",
                    "proposal_id" => p.proposal_id.clone(),
                    "target_key" => target_key.clone(),
                    "action" => "human_apply_refused_foreign_merge",
                );
                return Err(format!(
                    "落库被拒绝：该建议与既有记忆「{target_key}」语义撞车（防记忆劫持闸）。提案已登记为待决策，可重试、停用或删除。"
                ));
            }
        }

        // ── 段 ③：锁内把 CR 流转到 Active——重新 load 定位（段 ② 期间文件
        // 可能被并发改），仅状态真正变化才 rewrite（重复批 no-op 零写放大）。
        // pending 的 CR 不可被回滚（rollback_precheck 拒绝），段 ② 窗口无竞争面。
        if cr.status != ChangeStatus::Active {
            let _g = lock_evolution_store();
            mark_human_applied(&mut cr)?;
            let mut changes = load_changes(app)?;
            let pos = changes.iter().position(|c| c.change_id == cr.change_id).ok_or_else(|| {
                // 落库已生效但 CR 行没了（段 ② 期间被并发删除）：fail-closed 响亮报错，
                // 不静默返回 Active 成功——否则审计轨迹与现实脱节
                format!(
                    "CR {} 落库后找不到对应行（可能已被并发删除），本次批准的记忆已生效但没有变更记录，请刷新面板核实",
                    cr.change_id
                )
            })?;
            changes[pos] = cr.clone();
            rewrite_jsonl(&changes_path(app), &changes)?;
        }
    }
    Ok(Some(cr))
}

/// W1 内核：ProposalEntry → EvolutionProposal 还原（apply_one 入参）。
/// layer ↔ category 按 candidate::derive_layer 的双射反推；Parameter/Code 两层
/// 当前无 category 对应（派生侧只产 4 层），返 None（调用方跳过落库）。
/// evidence/suggestion 从 entry 透传字段重组；structured_patch 未落盘，恒 None。
fn entry_to_proposal(entry: &ProposalEntry) -> Option<EvolutionProposal> {
    use crate::evolution::proposal::{Evidence, ProposalCategory, Suggestion};
    let category = match entry.layer {
        EvolutionLayer::Policy => ProposalCategory::MemoryHint,
        EvolutionLayer::PromptHint => ProposalCategory::PromptHint,
        EvolutionLayer::ToolSchema => ProposalCategory::ToolSchemaHint,
        EvolutionLayer::Skill => ProposalCategory::SkillHint,
        EvolutionLayer::Parameter | EvolutionLayer::Code => return None,
    };
    Some(EvolutionProposal {
        proposal_id: entry.proposal_id.clone(),
        created_at_ms: entry.created_at_ms,
        origin: entry.origin,
        category,
        target: entry.target.clone(),
        impact: entry.impact,
        evidence: Evidence {
            summary: entry.summary.clone(),
            occurrence_count: entry.occurrence_count,
            window_hours: entry.window_hours,
            related_refs: entry.related_refs.clone(),
        },
        suggestion: Suggestion {
            text: entry.suggestion_text.clone(),
            structured_patch: None,
        },
    })
}

/// W1：toggle ON 落库前的嵌入预计算（锁外调用）。只对 policy 层提案算
///（非 policy 不落库，白算几十 ms ONNX）；读失败/找不到条目返 None 不阻塞。
fn precompute_human_apply_embedding<R: tauri::Runtime>(
    app: &AppHandle<R>,
    proposal_id: &str,
) -> Option<Vec<f32>> {
    let entries = candidate::read_all(&proposals_path(app)).ok()?;
    let entry = entries.iter().find(|e| e.proposal_id == proposal_id)?;
    if entry.layer != EvolutionLayer::Policy {
        return None;
    }
    crate::memory::embed::embed_text(&entry.suggestion_text)
}

/// W1 人工批准执行器内核：apply_one 写 lesson（幂等由 evo:<pid> key 查重承担）。
/// 本函数只取 DB_WRITE_LOCK，**不持** EVOLUTION_STORE_LOCK（唯一调用方
/// toggle_inner 段② = 锁外落库段）；store 锁与 DB 锁不同时嵌套持有
///（apply 轨先放 DB 锁再取 store 锁，两向无环），死锁面为零。
fn human_apply_one<R: tauri::Runtime>(
    app: &AppHandle<R>,
    p: &EvolutionProposal,
    embedding: Option<&[f32]>,
) -> Result<ApplyOutcome, String> {
    let conn = crate::db::open_db(app)?;
    let _db_write = crate::db::DB_WRITE_LOCK.lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] evolution::panel DB_WRITE_LOCK (human_apply): {e:?}");
        e.into_inner()
    });
    crate::memory::store::ensure_table(&conn)?;
    apply::apply_one(&conn, p, embedding, crate::memory::now_ms())
}

/// W1：人工批准落库成功后 CR 合法流转到 Active——瞬时走完
/// Pending→Shadowing→ShadowPassed→Approved→Active（同 auto_applied_from_proposal；
/// Pending→Active 直跳被 status.rs 硬约束②拦截，不裸写）。已 Active（重复批）
/// 幂等跳过。CR 可能被 shadow 观察推进到中途态（Shadowing/ShadowPassed，段①
/// dedup 会复用既有行）——从当前态续走余下流转，同态重走会被 status.rs 判
/// 「状态未变」报错。approval_source 保持 HumanApproved 不变（自动/人工来源显式区分）。
fn mark_human_applied(cr: &mut ChangeRecord) -> Result<(), String> {
    use ChangeStatus::{Active, Approved, ShadowPassed, Shadowing};
    if cr.status == Active {
        return Ok(()); // 幂等：重复批不 Err 不回退
    }
    let path = [Shadowing, ShadowPassed, Approved, Active];
    // 起点解析：Pending 走全路径；已在路径上的状态从**下一站**续走
    //（含当前态会触发 status.rs 的「状态未变」拒绝）；
    // 路径外的非终态（Rejected 等终态/异常态）给可操作中文报错，fail-closed。
    let begin = match cr.status {
        ChangeStatus::Pending => 0,
        other => path
            .iter()
            .position(|s| *s == other)
            .map(|i| i + 1)
            .ok_or_else(|| {
                format!(
                    "CR 状态 {other:?} 不在 Pending→Active 路径上，无法流转到 Active（请刷新面板核实状态）"
                )
            })?,
    };
    for step in &path[begin..] {
        change::transition(cr.status, *step)?;
        cr.status = *step;
    }
    Ok(())
}

fn delete_inner(app: &AppHandle, proposal_id: &str, cascade_source: bool) -> Result<(), String> {
    let _g = lock_evolution_store(); // OCR C3-4：覆盖整个 load→mutate→rewrite 窗口
    let p_path = proposals_path(app);
    let c_path = changes_path(app);

    // 加载 proposal entry（拿 related_refs，cascade 时用）
    let proposals = load_proposals(app)?;
    let entry = proposals
        .iter()
        .find(|e| e.proposal_id == proposal_id)
        .ok_or_else(|| format!("proposal {proposal_id} 不存在"))?
        .clone();
    let related_refs = entry.related_refs.clone();

    // 1. 删 proposals.jsonl 行（废案：下次 consolidation / LLM 也不会执行）
    let mut proposals = proposals;
    proposals.retain(|e| e.proposal_id != proposal_id);
    rewrite_jsonl(&p_path, &proposals)?;

    // 2. 级联删 changes.jsonl：仅删 status=pending（其他状态走 Rollback）
    let mut changes = load_changes(app)?;
    let before = changes.len();
    changes.retain(|c| !(c.proposal_id == proposal_id && c.status == ChangeStatus::Pending));
    let cascaded = before - changes.len();
    rewrite_jsonl(&c_path, &changes)?;

    // 3. 可选 cascade 源记忆（老板 16:35 拍板：默认关，复选框选）
    //    B4-4（P3 组）：mem_items 删除持 DB_WRITE_LOCK（此前裸连接写绕全局写锁）
    let mut mem_deleted = 0usize;
    if cascade_source {
        let conn = crate::db::open_db(app)?;
        let _db_write = crate::db::DB_WRITE_LOCK.lock().unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] evolution::panel DB_WRITE_LOCK (cascade): {e:?}");
            e.into_inner()
        });
        mem_deleted = cascade_delete_mem_items(&conn, &related_refs, &proposal_id)?;
    }

    crate::audit_event!(
        app,
        AuditLevel::Warn,
        "evolution.delete",
        "proposal_id" => proposal_id.to_string(),
        "cascaded_changes" => cascaded.to_string(),
        "cascaded_mem_items" => mem_deleted.to_string(),
        "cascade_source" => cascade_source.to_string(),
    );
    Ok(())
}

/// B1-2（P0-EV2 半）：cascade_source 的记忆级联内核（注入连接，内存库可单测）——
/// related_refs 源记忆整删 + apply 落下的 lesson 按 key_tag（evo:<id>）删。
/// 只删 related_refs 会把废案提案的 lesson 留成孤儿（injection_block 永带出）。
pub(crate) fn cascade_delete_mem_items(
    conn: &rusqlite::Connection,
    related_refs: &[String],
    proposal_id: &str,
) -> Result<usize, String> {
    let mut n = if related_refs.is_empty() {
        0
    } else {
        crate::memory::store::delete_by_ids(conn, related_refs)?
    };
    if crate::evolution::apply::rollback_applied(conn, proposal_id)? {
        n += 1;
    }
    Ok(n)
}

/// Tauri command：toggle 开关（UI 主入口，老板 16:05 拍板）
/// enabled=true  → toggle_inner(true)  写 ChangeRecord（pending）+ 标 promoted
/// enabled=false → toggle_inner(false) 移除 ChangeRecord + 标 rejected
/// 不走 ConfirmMap（toggle 是 UI 显式行为，不像 Promote/Reject 高危）
#[tauri::command]
pub async fn evolution_toggle_proposal(
    app: AppHandle,
    proposal_id: String,
    enabled: bool,
) -> Result<(), String> {
    let app2 = app.clone();
    let proposal_id2 = proposal_id.clone();
    // 阻塞 fs IO（jsonl load→mutate→rewrite）移出 async worker
    crate::py::document::spawn_blocking_map(move || {
        toggle_inner(&app2, &proposal_id2, enabled).map(|_| ())
    })
    .await?;
    // 通知中心回写：提案已决策（ON→done / OFF→dismissed）
    notify_resolved(
        &app,
        &proposal_id,
        if enabled {
            crate::notifications::STATUS_DONE
        } else {
            crate::notifications::STATUS_DISMISSED
        },
    );
    Ok(())
}

/// Tauri command：彻底废案（delete emoji 入口）
/// 行为口径（拍板④ 2026-09-29：维持现行为，注释对齐）：
/// - proposals.jsonl 行**无论 status 一律删除**（「只允许删 Pending」的旧说法
///   与实现不符；Active 的 CR 仍可走 Rollback，删除不动它）；
/// - changes.jsonl 仅级联删 status=pending 行（其他状态保留作历史）；
/// - `cascade_source=true` 时连同源 mem_items + evo lesson 一起删（防 24h 重生）。
#[tauri::command]
pub async fn evolution_delete_proposal(
    app: AppHandle,
    proposal_id: String,
    cascade_source: bool,
) -> Result<(), String> {
    let app2 = app.clone();
    let proposal_id2 = proposal_id.clone();
    // 阻塞 fs IO + 级联 mem_items 删除（SQLite）移出 async worker
    crate::py::document::spawn_blocking_map(move || {
        delete_inner(&app2, &proposal_id2, cascade_source)
    })
    .await?;
    // 通知中心回写：提案已删除 → 对应消息 dismissed
    notify_resolved(&app, &proposal_id, crate::notifications::STATUS_DISMISSED);
    Ok(())
}

// ───────────────────────── Commands ─────────────────────────

/// 列出候选池（status 可选过滤）
#[tauri::command]
pub async fn evolution_list_proposals(
    app: AppHandle,
    status: Option<String>,
) -> Result<Vec<ProposalEntry>, String> {
    // 阻塞 fs IO（全量 load jsonl）移出 async worker
    crate::py::document::spawn_blocking_map(move || {
        let mut entries = load_proposals(&app)?;
        if let Some(s) = status {
            let target = parse_status_filter(&s)?;
            entries.retain(|e| e.status == target);
        }
        Ok(entries)
    })
    .await
}

/// 晋升提案（Pooled → Promoted + 创建 ChangeRecord）
///
/// 复用 ConfirmMap：弹出 widget 弹窗等用户确认。
/// 确认通过后：
/// 老板 16:05 拍板：Promote/Reject 语义 = toggle ON/OFF（兼容老 UI/调用方）
/// 仍走 ConfirmMap（高危操作需显式确认），内部转 toggle_inner
#[tauri::command]
pub async fn evolution_promote_proposal(
    app: AppHandle,
    proposal_id: String,
    interactive: bool,
    session_id: Option<String>,
) -> Result<ChangeRecord, String> {
    // 段 A（阻塞 IO：load + 校验 + 组 detail）移出 async worker；
    // confirm 是 await，两段阻塞闭包分列其前后（await 不跨闭包）
    let detail = {
        let app = app.clone();
        let proposal_id = proposal_id.clone();
        crate::py::document::spawn_blocking_map(move || {
            let entries = load_proposals(&app)?;
            let entry = entries
                .iter()
                .find(|e| e.proposal_id == proposal_id)
                .ok_or_else(|| format!("proposal {proposal_id} 不存在"))?;

            if entry.status == ProposalStatus::Rejected {
                return Err(format!("proposal {proposal_id} 已被拒绝"));
            }

            Ok(format!(
                "启用提案\nproposal_id: {}\nsummary: {}\nimpact: {:?}\nlayer: {:?}\n\n批准后将写入 ChangeRecord 并进入沙箱流程",
                entry.proposal_id, entry.summary, entry.impact, entry.layer
            ))
        })
        .await?
    };
    let approved = bot_slash::ask_user_confirm(
        &app,
        "evolution_promote",
        &detail,
        interactive,
        session_id.as_deref(),
    )
    .await;
    if !approved {
        return Err("用户拒绝启用".into());
    }

    // 段 B（阻塞 IO：持锁 RMW + rewrite）
    let app2 = app.clone();
    let proposal_id2 = proposal_id.clone();
    let cr = crate::py::document::spawn_blocking_map(move || {
        toggle_inner(&app2, &proposal_id2, true)?
            .ok_or_else(|| "toggle_inner ON 未返回 ChangeRecord（不变量破坏）".to_string())
    })
    .await?;
    // 通知中心回写：提案已启用 → 对应消息 done
    notify_resolved(&app, &proposal_id, crate::notifications::STATUS_DONE);
    Ok(cr)
}

/// 老板 16:05 拍板：Reject 语义 = toggle OFF（兼容老 API）
/// 仍走 ConfirmMap，警告用户要移除 ChangeRecord
#[tauri::command]
pub async fn evolution_reject_proposal(
    app: AppHandle,
    proposal_id: String,
    interactive: bool,
    session_id: Option<String>,
) -> Result<(), String> {
    // 段 A（阻塞 IO：load + 校验 + 组 detail）移出 async worker
    let detail = {
        let app = app.clone();
        let proposal_id = proposal_id.clone();
        crate::py::document::spawn_blocking_map(move || {
            let entries = load_proposals(&app)?;
            let entry = entries
                .iter()
                .find(|e| e.proposal_id == proposal_id)
                .ok_or_else(|| format!("proposal {proposal_id} 不存在"))?;

            if entry.status == ProposalStatus::Rejected {
                return Err(format!("proposal {proposal_id} 已拒绝"));
            }

            Ok(format!(
                "停用提案\nproposal_id: {}\nsummary: {}\n\n批准后将移除对应的 ChangeRecord（如有）",
                entry.proposal_id, entry.summary
            ))
        })
        .await?
    };
    let approved = bot_slash::ask_user_confirm(
        &app,
        "evolution_reject",
        &detail,
        interactive,
        session_id.as_deref(),
    )
    .await;
    if !approved {
        return Err("用户取消停用操作".into());
    }
    // 段 B（阻塞 IO：持锁 RMW + rewrite）
    let app2 = app.clone();
    let proposal_id2 = proposal_id.clone();
    crate::py::document::spawn_blocking_map(move || {
        toggle_inner(&app2, &proposal_id2, false).map(|_| ()) // OFF 恒 None
    })
    .await?;
    // 通知中心回写：提案已停用 → 对应消息 dismissed
    notify_resolved(&app, &proposal_id, crate::notifications::STATUS_DISMISSED);
    Ok(())
}

/// 通知中心回写：提案已被处理，把对应消息解决掉（失败不阻断主流程，stderr 留痕）
fn notify_resolved<R: tauri::Runtime>(app: &AppHandle<R>, proposal_id: &str, status: &str) {
    let result = crate::db::open_db(app).and_then(|conn| {
        crate::notifications::notif_resolve(&conn, &format!("evo:{proposal_id}"), status)
    });
    match result {
        Ok(()) => crate::notifications::emit_changed(app),
        Err(e) => eprintln!("[notifications] evolution 提案消息回写失败：{e}"),
    }
}

/// Keep Shadow：延长 shadow 期（重置 expires_at_ms = now + TTL）
///
/// 不修改 status（仍为 Pooled），仅刷新 TTL。
#[tauri::command]
pub async fn evolution_keep_shadow(
    app: AppHandle,
    proposal_id: String,
) -> Result<ProposalEntry, String> {
    // 阻塞 fs IO（持锁 load→mutate→rewrite）移出 async worker；
    // store 锁随闭包走，不跨 await
    crate::py::document::spawn_blocking_map(move || {
        let _g = lock_evolution_store(); // OCR C3-4：覆盖整个 load→mutate→rewrite 窗口
        let path = proposals_path(&app);
        let mut entries = load_proposals(&app)?;

        let idx = entries
            .iter()
            .position(|e| e.proposal_id == proposal_id)
            .ok_or_else(|| format!("proposal {proposal_id} 不存在"))?;
        let mut entry = entries[idx].clone();

        if entry.status != ProposalStatus::Pooled {
            return Err(format!(
                "proposal {proposal_id} 不在 Pooled 状态（当前 {:?}）",
                entry.status
            ));
        }

        // 重置 TTL（再续 14 天）
        let now_ms = chrono::Utc::now().timestamp_millis();
        entry.expires_at_ms = candidate::compute_expires_at(now_ms);
        entries[idx] = entry.clone();
        rewrite_jsonl(&path, &entries)?;
        Ok(entry)
    })
    .await
}

/// 列出 ChangeRecord（回滚 UI 用）
#[tauri::command]
pub async fn evolution_list_changes(app: AppHandle) -> Result<Vec<ChangeRecord>, String> {
    // 阻塞 fs IO（全量 load jsonl）移出 async worker
    crate::py::document::spawn_blocking_map(move || load_changes(&app)).await
}

/// U20 W3：四指标薄壳——读三个 jsonl 走 observe::compute_metrics 纯函数。
/// 观察窗口 30 天（同 bin/observe_run.rs 默认口径）；空文件 = 纯函数零值。
#[tauri::command]
pub async fn evolution_metrics(app: AppHandle) -> Result<ObserveMetrics, String> {
    // 阻塞 fs IO（三份 jsonl 全量 load）移出 async worker
    crate::py::document::spawn_blocking_map(move || {
        let applied = crate::eval::metrics::read_applied(&applied_path(&app))?;
        let now = crate::memory::now_ms();
        let window_start = now - 30 * 86_400_000;
        Ok(observe::compute_metrics(
            &load_proposals(&app)?,
            &load_changes(&app)?,
            &applied,
            now,
            window_start,
        ))
    })
    .await
}

/// U20 W2：读应用策略档位（serde 小写序列化 "auto"/"confirm"；缺字段/读失败
/// = auto = 现状）。返回类型化枚举（OCR R1 采纳），非法值在读取层已归一 auto。
#[tauri::command]
pub async fn evolution_get_apply_policy(app: AppHandle) -> Result<ApplyPolicy, String> {
    crate::py::document::spawn_blocking_map(move || Ok(read_apply_policy(&app))).await
}

/// U20 W2：点档即时落盘（设置页自进化头部 radiogroup，同记忆三档先例）。
#[tauri::command]
pub async fn evolution_set_apply_policy(app: AppHandle, policy: String) -> Result<(), String> {
    crate::py::document::spawn_blocking_map(move || {
        let p = ApplyPolicy::from_config_str(&policy)
            .ok_or_else(|| format!("未知应用策略：{policy}（仅 auto / confirm）"))?;
        set_apply_policy(&app, p)
    })
    .await
}

/// 回滚 ChangeRecord
///
/// 步骤：
/// 1. ConfirmMap 弹窗确认
/// 2. 找到 ChangeRecord 对应的 mem_item（tags[0] = "evo:<proposal_id>"）
/// 3. 删除 mem_item
/// 4. 状态 → RolledBack（rewrite evolution-changes.jsonl）
#[tauri::command]
pub async fn evolution_rollback_change(
    app: AppHandle,
    change_id: String,
    interactive: bool,
    session_id: Option<String>,
) -> Result<ChangeRecord, String> {
    // ① 无锁只读快照 + 校验（仅供确认弹窗）—— 确认是 await，**不能持同步锁跨越**
    //   （std MutexGuard 跨 await → future 不 Send，tauri command 编译器直接拒）。
    //   阻塞 fs IO（load jsonl）包 spawn_blocking 移出 async worker。
    let record = {
        let app = app.clone();
        let change_id = change_id.clone();
        crate::py::document::spawn_blocking_map(move || {
            let records = load_changes(&app)?;
            let r = records
                .iter()
                .find(|r| r.change_id == change_id)
                .ok_or_else(|| format!("change {change_id} 不存在"))?
                .clone();
            rollback_precheck(&r, &change_id)?;
            Ok(r)
        })
        .await?
    };

    let detail = format!(
        "回滚 ChangeRecord\nchange_id: {}\nproposal_id: {}\nsummary: {}\n\n回滚将删除对应 mem_items 记录",
        record.change_id, record.proposal_id, record.suggestion_text
    );
    let approved = bot_slash::ask_user_confirm(
        &app,
        "evolution_rollback",
        &detail,
        interactive,
        session_id.as_deref(),
    )
    .await;
    if !approved {
        return Err("用户取消回滚".into());
    }

    // ② 持锁执行 RMW（同步、不跨 await）——重新 load + 重新校验，保证临界区语义；
    //    阻塞 IO（含 SQLite 删 mem_item + rewrite）包 spawn_blocking 移出 async worker
    crate::py::document::spawn_blocking_map(move || rollback_change_locked(&app, &change_id)).await
}

/// 回滚前置校验（无锁快照与锁内复检共用，避免两处语义漂移）。
fn rollback_precheck(record: &ChangeRecord, change_id: &str) -> Result<(), String> {
    if record.status == ChangeStatus::RolledBack {
        return Err(format!("change {change_id} 已回滚"));
    }
    if record.status.is_terminal() && record.status != ChangeStatus::Active {
        return Err(format!(
            "change {change_id} 状态 {:?} 不可回滚",
            record.status
        ));
    }
    Ok(())
}

/// 回滚的 RMW 部分（**同步**、持 store 锁）：load → 校验 → 删 mem_item → 改状态 → rewrite。
/// 不含 `await` —— 故可安全被 async command 调用（锁不跨 await，future 保持 Send）。
fn rollback_change_locked(app: &AppHandle, change_id: &str) -> Result<ChangeRecord, String> {
    let _g = lock_evolution_store(); // OCR C3-4：覆盖整个 load→mutate→rewrite 窗口
    let path = changes_path(app);
    let mut records = load_changes(app)?;

    let idx = records
        .iter()
        .position(|r| r.change_id == change_id)
        .ok_or_else(|| format!("change {change_id} 不存在"))?;
    let record = records[idx].clone();
    rollback_precheck(&record, change_id)?;

    // 删除 mem_item（走 db 连接）
    delete_evolution_mem_item(app, &record.proposal_id)?;

    // 状态 → RolledBack + 写回 jsonl
    let mut updated = record.clone();
    updated.status = ChangeStatus::RolledBack;
    updated.rolled_back_at = Some(chrono::Utc::now().timestamp_millis());
    updated.rollback_reason = Some("human_rollback_via_panel".into());
    records[idx] = updated.clone();
    rewrite_jsonl(&path, &records)?;
    Ok(updated)
}

/// 删除 evolution mem_item（tags[0] = "evo:<proposal_id>"）
///
/// 通过 store::delete_by_key_tag 实现。
/// 错误不阻塞回滚（mem_item 可能已不存在，例如重启后丢失）。
fn delete_evolution_mem_item(app: &AppHandle, proposal_id: &str) -> Result<(), String> {
    use crate::db;
    let key = format!("evo:{proposal_id}");
    let conn = db::open_db(app).map_err(|e| format!("打开 DB 失败：{e}"))?;
    // B4-4（P3 组）：mem_items 删除持 DB_WRITE_LOCK（此前裸连接写绕全局写锁）
    let _db_write = crate::db::DB_WRITE_LOCK.lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] evolution::panel DB_WRITE_LOCK (rollback): {e:?}");
        e.into_inner()
    });
    let deleted = crate::memory::store::delete_by_key_tag(&conn, &key)
        .map_err(|e| format!("删除 mem_item 失败：{e}"))?;
    if !deleted {
        eprintln!("[evolution] rollback: mem_item {key} 不存在（可能已淘汰）");
    }
    Ok(())
}

// ───────────────────────── 单元测试（pure helpers）─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::change::{ApprovalSource, ChangeRecord, ChangeStatus, EvolutionLayer};
    use crate::evolution::proposal::{ImpactLevel, ProposalOrigin, ProposalTarget};

    fn mk_entry(id: &str, status: ProposalStatus) -> ProposalEntry {
        ProposalEntry {
            proposal_id: id.into(),
            change_id: format!("chg-{id}"),
            layer: EvolutionLayer::Policy,
            impact: ImpactLevel::Medium,
            origin: ProposalOrigin::ConsolidationReflection,
            target: ProposalTarget::MemoryPolicy {
                policy: "test".into(),
            },
            suggestion_text: format!("text-{id}"),
            mem_key: format!("evo:{id}"),
            related_refs: vec![],
            summary: format!("s-{id}"),
            occurrence_count: 1,
            window_hours: 24,
            created_at_ms: 1_700_000_000_000,
            expires_at_ms: 1_700_000_000_000 + 86_400_000 * 14,
            status,
        }
    }

    fn mk_change(id: &str, status: ChangeStatus) -> ChangeRecord {
        ChangeRecord {
            change_id: id.into(),
            parent_id: None,
            schema_version: 1,
            layer: EvolutionLayer::Policy,
            origin: ProposalOrigin::ConsolidationReflection,
            proposal_id: id.trim_start_matches("chg-").into(),
            target: ProposalTarget::MemoryPolicy {
                policy: "test".into(),
            },
            suggestion_text: format!("text-{id}"),
            mem_key: format!("evo:{}", id.trim_start_matches("chg-")),
            impact: ImpactLevel::Medium,
            eval_before: None,
            eval_after: None,
            status,
            hard_constraint_compliance: true,
            approval_source: ApprovalSource::AutoApplied,
            human_approver: None,
            created_at_ms: 1_700_000_000_000,
            rolled_back_at: None,
            rollback_reason: None,
        }
    }

    #[test]
    fn cascade_mem_items_deletes_refs_and_lesson() {
        // B1-2（P0-EV2 半）回归：cascade_source 必须连 apply 落下的 lesson
        // （tags[0]=evo:<id>）一起删——只删 related_refs 会留孤儿 lesson，
        // injection_block 永远带出废案建议。
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::memory::store::ensure_table(&conn).unwrap();
        let lesson = crate::memory::store::NewItem {
            kind: "lesson".into(),
            content: "废案 lesson".into(),
            tags: vec!["evo:orph1".into(), "evolution".into()],
            importance: 3,
            source: "system".into(),
        };
        crate::memory::store::insert_item(&conn, &lesson, None, 1000).unwrap();
        let src = crate::memory::store::NewItem {
            kind: "fact".into(),
            content: "源记忆".into(),
            tags: vec!["src:ref1".into()],
            importance: 2,
            source: "user".into(),
        };
        crate::memory::store::insert_item(&conn, &src, None, 1001).unwrap();
        let src_id = crate::memory::store::find_by_key_tag(&conn, "src:ref1")
            .unwrap()
            .expect("前置：源记忆入库")
            .id;
        assert!(
            crate::memory::store::find_by_key_tag(&conn, "evo:orph1")
                .unwrap()
                .is_some(),
            "前置：lesson 在库"
        );
        let n = super::cascade_delete_mem_items(&conn, &[src_id], "orph1").unwrap();
        assert_eq!(n, 2, "源记忆 + lesson 各删一条");
        assert!(
            crate::memory::store::find_by_key_tag(&conn, "evo:orph1")
                .unwrap()
                .is_none(),
            "lesson 应随级联删除"
        );
        // 空 related_refs + 无 lesson：幂等 0
        let n2 = super::cascade_delete_mem_items(&conn, &[], "orph1").unwrap();
        assert_eq!(n2, 0, "重复级联幂等");
    }

    #[test]
    fn unique_change_id_avoids_collision() {
        // B2-4（P1-EV5）：复用 toggle 时基础 id 被旧行占用 → -2/-3 递增；
        // 空闲则直接用基础 id（首次启用行为不变）。实现抽到 change::derive
        // 供 shadow/apply 同用，此处锁行为。
        let rolled = vec![mk_change("chg-p9", ChangeStatus::RolledBack)];
        assert_eq!(change::unique_change_id_for(&rolled, "p9"), "chg-p9-2");
        let mut two = rolled.clone();
        two.push(mk_change("chg-p9-2", ChangeStatus::RolledBack));
        assert_eq!(change::unique_change_id_for(&two, "p9"), "chg-p9-3");
        let empty: Vec<ChangeRecord> = vec![];
        assert_eq!(change::unique_change_id_for(&empty, "p1"), "chg-p1");
    }

    #[test]
    fn parse_status_filter_valid() {
        assert_eq!(
            parse_status_filter("pooled").unwrap(),
            ProposalStatus::Pooled
        );
        assert_eq!(
            parse_status_filter("promoted").unwrap(),
            ProposalStatus::Promoted
        );
        assert_eq!(
            parse_status_filter("expired").unwrap(),
            ProposalStatus::Expired
        );
        assert_eq!(
            parse_status_filter("rejected").unwrap(),
            ProposalStatus::Rejected
        );
    }

    #[test]
    fn parse_status_filter_invalid_errors() {
        let e = parse_status_filter("unknown").unwrap_err();
        assert!(e.contains("未知"), "应提示未知 status：{e}");
    }

    #[test]
    fn rewrite_roundtrip_preserves_entries() {
        // 验证 rewrite_jsonl：写入 → 读回 → 一致
        let dir = std::env::temp_dir().join(format!(
            "cmd-rw-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("evolution-proposals.jsonl");
        let entries = vec![
            mk_entry("a", ProposalStatus::Pooled),
            mk_entry("b", ProposalStatus::Promoted),
        ];
        rewrite_jsonl(&p, &entries).unwrap();
        let read = candidate::read_all(&p).unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].proposal_id, "a");
        assert_eq!(read[1].status, ProposalStatus::Promoted);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rewrite_truncates_old_entries() {
        // 验证 truncate：第二次 rewrite 应清掉旧条目
        let dir = std::env::temp_dir().join(format!(
            "cmd-tr-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("evolution-proposals.jsonl");
        // 第一次：3 条
        rewrite_jsonl(
            &p,
            &vec![
                mk_entry("a", ProposalStatus::Pooled),
                mk_entry("b", ProposalStatus::Pooled),
                mk_entry("c", ProposalStatus::Pooled),
            ],
        )
        .unwrap();
        assert_eq!(candidate::read_all(&p).unwrap().len(), 3);
        // 第二次：1 条
        rewrite_jsonl(&p, &vec![mk_entry("a", ProposalStatus::Promoted)]).unwrap();
        let read = candidate::read_all(&p).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].status, ProposalStatus::Promoted);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rewrite_atomic_leaves_no_tmp() {
        // 原子形态回归：tmp+rename 后目标内容正确且无 .tmp 残留
        let dir = std::env::temp_dir().join(format!(
            "cmd-atomic-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("evolution-proposals.jsonl");
        let entries = vec![mk_entry("a", ProposalStatus::Pooled)];
        rewrite_jsonl(&p, &entries).unwrap();
        let read = candidate::read_all(&p).unwrap();
        assert_eq!(read.len(), 1);
        let tmp = p.with_file_name("evolution-proposals.jsonl.tmp");
        assert!(!tmp.exists(), "rename 后不应残留 tmp：{tmp:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_evolution_mem_item_builds_correct_key() {
        // 验证 key 派生："evo:" + proposal_id
        let id = "abc123";
        let expected = "evo:abc123";
        let key = format!("evo:{id}");
        assert_eq!(key, expected);
    }

    // ─── ApprovalSource / ChangeStatus 锁死 ───

    #[test]
    fn human_approved_distinguished_from_auto_applied() {
        let auto = ApprovalSource::AutoApplied;
        let human = ApprovalSource::HumanApproved;
        assert_ne!(auto, human, "硬约束 ② 要求显式区分");
    }

    #[test]
    fn is_terminal_active_is_not() {
        // Active 可回滚（不是终态）
        assert!(!ChangeStatus::Active.is_terminal());
        assert!(ChangeStatus::RolledBack.is_terminal());
        assert!(ChangeStatus::Rejected.is_terminal());
        assert!(ChangeStatus::Expired.is_terminal());
    }

    #[test]
    fn w1_entry_to_proposal_roundtrip() {
        let mut e = mk_entry("u20-w1e", ProposalStatus::Pooled);
        e.summary = "证据摘要".into();
        e.occurrence_count = 7;
        e.window_hours = 48;
        e.related_refs = vec!["r1".into()];
        let p = entry_to_proposal(&e).expect("policy 层应可还原");
        assert_eq!(p.proposal_id, e.proposal_id);
        assert_eq!(
            p.category,
            crate::evolution::proposal::ProposalCategory::MemoryHint
        );
        assert_eq!(p.evidence.summary, "证据摘要");
        assert_eq!(p.evidence.occurrence_count, 7);
        assert_eq!(p.evidence.window_hours, 48);
        assert_eq!(p.evidence.related_refs, vec!["r1"]);
        assert_eq!(p.suggestion.text, e.suggestion_text);
        assert!(p.suggestion.structured_patch.is_none());

        // layer ↔ category 双射的其余三角
        for (layer, cat) in [
            (
                EvolutionLayer::PromptHint,
                crate::evolution::proposal::ProposalCategory::PromptHint,
            ),
            (
                EvolutionLayer::ToolSchema,
                crate::evolution::proposal::ProposalCategory::ToolSchemaHint,
            ),
            (
                EvolutionLayer::Skill,
                crate::evolution::proposal::ProposalCategory::SkillHint,
            ),
        ] {
            e.layer = layer;
            assert_eq!(entry_to_proposal(&e).unwrap().category, cat);
        }
        // Parameter/Code 无 category 对应 → None（跳过落库）
        e.layer = EvolutionLayer::Parameter;
        assert!(entry_to_proposal(&e).is_none());
        e.layer = EvolutionLayer::Code;
        assert!(entry_to_proposal(&e).is_none());
    }

    #[test]
    fn w1_mark_human_applied_walks_legally_and_idempotent() {
        let mut cr = mk_change("chg-idem", ChangeStatus::Pending);
        cr.approval_source = ApprovalSource::HumanApproved;
        mark_human_applied(&mut cr).unwrap();
        assert_eq!(cr.status, ChangeStatus::Active);
        assert_eq!(
            cr.approval_source,
            ApprovalSource::HumanApproved,
            "来源不漂移"
        );
        // 已 Active（重复批）幂等跳过——再走一遍不 Err 也不回退
        mark_human_applied(&mut cr).unwrap();
        assert_eq!(cr.status, ChangeStatus::Active);
    }

    /// 中途态 CR（shadow 观察推进到 Shadowing/ShadowPassed，段① dedup 复用）
    /// 从当前状态续走到 Active，不因同态流转报「状态未变」
    #[test]
    fn w1_mark_human_applied_resumes_from_midflow_status() {
        for start in [
            ChangeStatus::Shadowing,
            ChangeStatus::ShadowPassed,
            ChangeStatus::Approved,
        ] {
            let mut cr = mk_change("chg-mid", start);
            cr.approval_source = ApprovalSource::HumanApproved;
            mark_human_applied(&mut cr)
                .unwrap_or_else(|e| panic!("{start:?} 应能续走到 Active：{e}"));
            assert_eq!(cr.status, ChangeStatus::Active, "{start:?} 终点应为 Active");
        }
    }
}
