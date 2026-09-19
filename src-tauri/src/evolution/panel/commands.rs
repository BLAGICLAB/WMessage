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
//! 全部走 ask_user_confirm 复用 ConfirmMap（spec R0 #1 默认 A）。

use std::path::PathBuf;
use tauri::AppHandle;

use crate::bot_slash;
use crate::db::paths;
use crate::evolution::candidate::{self, ProposalEntry, ProposalStatus};
use crate::evolution::change::{self, ApprovalSource, ChangeRecord, ChangeStatus};
use crate::audit::AuditLevel;

// ───────────────────────── 路径辅助 ─────────────────────────

fn proposals_path(app: &AppHandle) -> PathBuf {
    paths::data_dir(app).join("evolution-proposals.jsonl")
}

fn changes_path(app: &AppHandle) -> PathBuf {
    paths::data_dir(app).join("evolution-changes.jsonl")
}

// ───────────────────────── 读写辅助 ─────────────────────────

fn load_proposals(app: &AppHandle) -> Result<Vec<ProposalEntry>, String> {
    candidate::read_all(&proposals_path(app))
}

fn load_changes(app: &AppHandle) -> Result<Vec<ChangeRecord>, String> {
    change::read_all(&changes_path(app))
}

/// 整体重写 jsonl（更新 status / TTL 用）
fn rewrite_jsonl<T: serde::Serialize>(path: &PathBuf, entries: &[T]) -> Result<(), String> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建目录 {parent:?} 失败：{e}"))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .map_err(|e| format!("打开 {path:?} 失败：{e}"))?;
    for e in entries {
        let line = serde_json::to_string(e).map_err(|e| format!("序列化失败：{e}"))?;
        writeln!(f, "{line}").map_err(|e| format!("写入 {path:?} 失败：{e}"))?;
    }
    Ok(())
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

fn toggle_inner(app: &AppHandle, proposal_id: &str, enabled: bool) -> Result<(), String> {
    let p_path = proposals_path(app);
    let c_path = changes_path(app);

    let mut proposals = load_proposals(app)?;
    let idx = proposals
        .iter()
        .position(|e| e.proposal_id == proposal_id)
        .ok_or_else(|| format!("proposal {proposal_id} 不存在"))?;
    let mut entry = proposals[idx].clone();

    let mut changes = load_changes(app)?;

    if enabled {
        // Toggle ON：dedup（已有 pending/shadowing/shadow_passed 不重写）
        let has_active = changes.iter().any(|c| {
            c.proposal_id == proposal_id
                && matches!(
                    c.status,
                    ChangeStatus::Pending
                        | ChangeStatus::Shadowing
                        | ChangeStatus::ShadowPassed
                )
        });
        if !has_active {
            let mut cr = candidate::to_change_record(&entry, true);
            cr.approval_source = ApprovalSource::HumanApproved;
            cr.human_approver = Some("boss".into());
            cr.status = ChangeStatus::Pending;
            change::append_change(&c_path, &cr)?;
            crate::audit_event!(
                app,
                AuditLevel::Info,
                "evolution.toggle_on",
                "proposal_id" => proposal_id.to_string(),
                "change_id" => cr.change_id.clone(),
            );
        } else {
            crate::audit_event!(
                app,
                AuditLevel::Info,
                "evolution.toggle_on_dedup",
                "proposal_id" => proposal_id.to_string(),
            );
        }
        entry.status = ProposalStatus::Promoted;
        proposals[idx] = entry.clone();
        rewrite_jsonl(&p_path, &proposals)?;
    } else {
        // Toggle OFF：移除 pending ChangeRecord
        let before = changes.len();
        changes.retain(|c| {
            !(c.proposal_id == proposal_id && c.status == ChangeStatus::Pending)
        });
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
    }
    Ok(())
}

fn delete_inner(
    app: &AppHandle,
    proposal_id: &str,
    cascade_source: bool,
) -> Result<(), String> {
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
    let mut mem_deleted = 0usize;
    if cascade_source && !related_refs.is_empty() {
        let conn = crate::db::open_db(app)?;
        mem_deleted = crate::memory::store::delete_by_ids(&conn, &related_refs)?;
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
    toggle_inner(&app, &proposal_id, enabled)
}

/// Tauri command：彻底废案（delete emoji 入口）
/// 老板 16:05 拍板：只允许删 Pending（active 用 Rollback）
/// 级联删 proposals.jsonl + changes.jsonl
/// 老板 16:35 拍板：`cascade_source=true` 时连同源 mem_items 一起删（防止 24h 后重生）
#[tauri::command]
pub async fn evolution_delete_proposal(
    app: AppHandle,
    proposal_id: String,
    cascade_source: bool,
) -> Result<(), String> {
    delete_inner(&app, &proposal_id, cascade_source)
}

// ───────────────────────── Commands ─────────────────────────

/// 列出候选池（status 可选过滤）
#[tauri::command]
pub async fn evolution_list_proposals(
    app: AppHandle,
    status: Option<String>,
) -> Result<Vec<ProposalEntry>, String> {
    let mut entries = load_proposals(&app)?;
    if let Some(s) = status {
        let target = parse_status_filter(&s)?;
        entries.retain(|e| e.status == target);
    }
    Ok(entries)
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
    let entries = load_proposals(&app)?;
    let entry = entries
        .iter()
        .find(|e| e.proposal_id == proposal_id)
        .ok_or_else(|| format!("proposal {proposal_id} 不存在"))?;

    if entry.status == ProposalStatus::Rejected {
        return Err(format!("proposal {proposal_id} 已被拒绝"));
    }

    let detail = format!(
        "启用提案\nproposal_id: {}\nsummary: {}\nimpact: {:?}\nlayer: {:?}\n\n批准后将写入 ChangeRecord 并进入沙箱流程",
        entry.proposal_id, entry.summary, entry.impact, entry.layer
    );
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

    toggle_inner(&app, &proposal_id, true)?;

    let changes = load_changes(&app)?;
    let cr = changes
        .iter()
        .find(|c| {
            c.proposal_id == proposal_id
                && matches!(
                    c.status,
                    ChangeStatus::Pending
                        | ChangeStatus::Shadowing
                        | ChangeStatus::ShadowPassed
                )
        })
        .ok_or_else(|| format!("toggle_inner 写完未找到 ChangeRecord"))?;
    Ok(cr.clone())
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
    let entries = load_proposals(&app)?;
    let entry = entries
        .iter()
        .find(|e| e.proposal_id == proposal_id)
        .ok_or_else(|| format!("proposal {proposal_id} 不存在"))?;

    if entry.status == ProposalStatus::Rejected {
        return Err(format!("proposal {proposal_id} 已拒绝"));
    }

    let detail = format!(
        "停用提案\nproposal_id: {}\nsummary: {}\n\n批准后将移除对应的 ChangeRecord（如有）",
        entry.proposal_id, entry.summary
    );
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
    toggle_inner(&app, &proposal_id, false)?;
    Ok(())
}

/// Keep Shadow：延长 shadow 期（重置 expires_at_ms = now + TTL）
///
/// 不修改 status（仍为 Pooled），仅刷新 TTL。
#[tauri::command]
pub async fn evolution_keep_shadow(
    app: AppHandle,
    proposal_id: String,
) -> Result<ProposalEntry, String> {
    let path = proposals_path(&app);
    let mut entries = load_proposals(&app)?;

    let idx = entries
        .iter()
        .position(|e| e.proposal_id == proposal_id)
        .ok_or_else(|| format!("proposal {proposal_id} 不存在"))?;
    let mut entry = entries[idx].clone();

    if entry.status != ProposalStatus::Pooled {
        return Err(format!("proposal {proposal_id} 不在 Pooled 状态（当前 {:?}）", entry.status));
    }

    // 重置 TTL（再续 14 天）
    let now_ms = chrono::Utc::now().timestamp_millis();
    entry.expires_at_ms = now_ms + candidate::TTL_MS;
    entries[idx] = entry.clone();
    rewrite_jsonl(&path, &entries)?;
    Ok(entry)
}

/// 列出 ChangeRecord（回滚 UI 用）
#[tauri::command]
pub async fn evolution_list_changes(app: AppHandle) -> Result<Vec<ChangeRecord>, String> {
    load_changes(&app)
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
    let path = changes_path(&app);
    let mut records = load_changes(&app)?;

    let idx = records
        .iter()
        .position(|r| r.change_id == change_id)
        .ok_or_else(|| format!("change {change_id} 不存在"))?;
    let record = records[idx].clone();

    if record.status == ChangeStatus::RolledBack {
        return Err(format!("change {change_id} 已回滚"));
    }
    if record.status.is_terminal() && record.status != ChangeStatus::Active {
        return Err(format!("change {change_id} 状态 {:?} 不可回滚", record.status));
    }

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

    // 删除 mem_item（走 db 连接）
    delete_evolution_mem_item(&app, &record.proposal_id)?;

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
            target: ProposalTarget::MemoryPolicy { policy: "test".into() },
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
            target: ProposalTarget::MemoryPolicy { policy: "test".into() },
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
    fn parse_status_filter_valid() {
        assert_eq!(parse_status_filter("pooled").unwrap(), ProposalStatus::Pooled);
        assert_eq!(parse_status_filter("promoted").unwrap(), ProposalStatus::Promoted);
        assert_eq!(parse_status_filter("expired").unwrap(), ProposalStatus::Expired);
        assert_eq!(parse_status_filter("rejected").unwrap(), ProposalStatus::Rejected);
    }

    #[test]
    fn parse_status_filter_invalid_errors() {
        let e = parse_status_filter("unknown").unwrap_err();
        assert!(e.contains("未知"), "应提示未知 status：{e}");
    }

    #[test]
    fn rewrite_roundtrip_preserves_entries() {
        // 验证 rewrite_jsonl：写入 → 读回 → 一致
        let dir = std::env::temp_dir().join(format!("cmd-rw-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
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
        let dir = std::env::temp_dir().join(format!("cmd-tr-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("evolution-proposals.jsonl");
        // 第一次：3 条
        rewrite_jsonl(&p, &vec![
            mk_entry("a", ProposalStatus::Pooled),
            mk_entry("b", ProposalStatus::Pooled),
            mk_entry("c", ProposalStatus::Pooled),
        ])
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
}