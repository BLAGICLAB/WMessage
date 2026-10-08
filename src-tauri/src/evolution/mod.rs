//! 自进化系统：经验捕获 + 提案产出（Phase 1）+ 提案应用闭环（Phase 2）。
//!
//! 设计原则（spec 硬约束）：
//! - 依赖方向：本模块 import `memory::consolidate` 的公开类型
//!   (`ConsolidateOp` / `ConsolidateReport`)，memory 模块不感知 evolution 存在；
//! - 不调第二次 LLM（必须复用 consolidate 已有反思调用）；
//! - 不写新数据库表 / 不改 schema（Phase 2 应用复用现有 mem_items 表）；
//! - 不改 prompt / TOOLS schema / 命令名 / 事件名 / JSON 字段 / 错误码；
//! - 不接收 session_id / 不接收任何反向依赖 memory 内部的状态。
//!
//! 模块分层：
//! - `trace`    数据结构：ExecutionTrace / ToolCallSummary / TraceOutcome（Phase 1.1）
//! - `proposal` 数据结构：EvolutionProposal + 枚举 + proposal_id 归一化（Phase 1.2）
//! - `derive`   纯函数：从 `[ConsolidateOp]` 启发式派生 proposals（Phase 1.3）
//! - `emit`     副作用：写 audit + 进程内 24h dedup（Phase 1.4）
//! - `apply`    应用：MemoryHint + High/Medium 提案落 lesson 记忆闭环生效（Phase 2）
//!
//! Phase 2 闭环：达门槛提案写入 mem_items（kind=lesson，tags[0]=evo:<proposal_id>
//! 持久幂等），下轮对话经 `memory::injection_block` 的 lesson 槽位自动带出，
//! 行为随之改变；留痕 evolution-applied.jsonl，回滚 = 删同 key 记忆。
//! PromptHint / ToolSchemaHint / SkillHint 永不自动应用，仍只写 audit。

pub mod activation;
pub mod apply;
pub mod candidate;
pub mod change;
pub mod policy;
//  事故恢复：本行原属并行 evolution 批次 A 的未提交修改，
// 被误执行的 git reset --hard 冲掉——strategy.rs（未跟踪）幸存，补回登记
pub mod strategy;

/// jsonl 读取共享内核（record/entry 两处 30 行 read_all 收敛
/// 单点防漂移）。语义（ 自愈化，原的 fail-closed 已按审计修正）：
/// 文件不存在 → Ok(空)；**任何行损坏 → 先把原文件整体备份为 `<name>.corrupt`
///（已存在不覆盖——损坏是持久态时热路径最多拷一次）再跳过坏行**（首行损坏不再
/// 永久 fail-closed——实证一个坏字节能让面板永久打不开且无自愈出口），stderr
/// 留痕。已知代价：坏行 id 缺失时 dedup 调用方可能同 id 再追加——无害重复；
/// 备份保证取证/手工修复有入口。
pub(crate) fn read_jsonl<T: serde::de::DeserializeOwned>(
    path: &std::path::Path,
    label: &str,
) -> Result<Vec<T>, String> {
    use std::io::BufRead;
    // 打开即判存在：NotFound = 无文件返回空——消除「exists 通过后、open 前被删」
    // 间隙里的伪 Err（并发 append 场景下 jsonl 只增不删，此竞态仅剩换名/重建触发）
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("打开 {path:?} 失败：{e}")),
    };
    let reader = std::io::BufReader::new(f);
    let mut out = Vec::new();
    let mut corrupt_backed_up = false;
    for (i, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| format!("读取第 {} 行失败：{e}", i + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<T>(&line) {
            Ok(v) => out.push(v),
            Err(e) => {
                // 首个坏行触发一次整体备份（固定名 `.corrupt`，已存在则
                // 不覆盖——评审 HIGH 采纳：损坏是持久态时热路径每次读都不该
                // 重拷全文件；首个快照即取证所需），跳过坏行继续（好行不丢）。
                // 快照与并发 append 之间允许轻微偏移（备份是 best-effort 取证）。
                if !corrupt_backed_up {
                    corrupt_backed_up = true;
                    let backup = path.with_file_name(format!(
                        "{}.corrupt",
                        path.file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| "changes".into())
                    ));
                    if backup.exists() {
                        eprintln!(
                            "[evolution] {label} jsonl 第 {} 行损坏（备份已存在 {backup:?}），跳过坏行：{e}",
                            i + 1
                        );
                    } else {
                        match std::fs::copy(path, &backup) {
                            Ok(_) => eprintln!(
                                "[evolution] {label} jsonl 第 {} 行损坏（已整体备份到 {backup:?}），跳过坏行：{e}",
                                i + 1
                            ),
                            Err(be) => eprintln!(
                                "[evolution] {label} jsonl 第 {} 行损坏（备份失败：{be}），跳过坏行：{e}",
                                i + 1
                            ),
                        }
                    }
                } else {
                    eprintln!("[evolution] {label} jsonl 第 {} 行损坏已跳过：{e}", i + 1);
                }
            }
        }
    }
    Ok(out)
}
pub mod derive;
pub mod emit;
pub mod observe;
pub mod panel;
pub mod proposal;
pub mod sandbox;
pub mod trace;

use crate::memory::consolidate::{ConsolidateOp, ConsolidateReport};

/// evolution 存储（proposals + changes 两个 jsonl）的**进程内单锁**。
///
/// **一把锁覆盖两个文件是有意设计**：toggle/delete 一次操作**同时**改两个文件，
/// 若按单文件各配一把锁，会出现「需要同时持两把锁」的顺序问题（死锁面）。
/// **改动时勿「优化」成按文件锁。**
/// 覆盖所有写路径：panel commands（toggle/delete/keep_shadow/rollback）+
/// post_consolidation 的 write_proposals。**entry/record 的 append/read_all
/// 内部不加锁**（外层已持锁，内层加锁必死锁）。
pub(crate) static EVOLUTION_STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 取 evolution 存储锁（poison 走仓库既有 `[mutex_poisoned]` 约定）。
/// 临界区必须覆盖**完整 RMW 窗口**（load → mutate → rewrite），不是只锁 rewrite。
pub(crate) fn lock_evolution_store() -> std::sync::MutexGuard<'static, ()> {
    EVOLUTION_STORE_LOCK.lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] evolution::EVOLUTION_STORE_LOCK: {e:?}");
        e.into_inner()
    })
}

/// 提案入池后向通知中心逐条落持久化消息（幂等 id：evo:{proposal_id}）。
/// 只对未被自动应用、需要用户决策的提案调用（post_consolidation 已过滤）。
fn notify_evolution_proposals<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    entries: &[candidate::ProposalEntry],
) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    let conn = crate::db::open_db(app).map_err(|e| e.to_string())?;
    let mut inserted_any = false;
    for e in entries {
        let title: String = e.suggestion_text.chars().take(60).collect();
        let body: String = e.summary.chars().take(120).collect();
        let payload = serde_json::json!({ "proposalId": e.proposal_id });
        let inserted = crate::notifications::notif_insert(
            &conn,
            &format!("evo:{}", e.proposal_id),
            crate::notifications::KIND_EVOLUTION,
            &title,
            &body,
            &payload,
        )?;
        inserted_any |= inserted;
    }
    if inserted_any {
        crate::notifications::emit_changed(app);
    }
    Ok(())
}

/// 反思完成后的桥接入口（`memory::consolidate::run_consolidation` 末尾调用）。
///
/// 签名严格按 spec：不接收 AppHandle / session_id / 任何反向依赖 memory 内部的状态。
/// emit 所需的 AppHandle 由 `emit::register_app_handle` 在 App 启动时注册一次，
/// 本函数通过全局 OnceLock 取出——consolidate 侧 caller 无需关心。
pub fn post_consolidation(ops: &[ConsolidateOp], report: &ConsolidateReport) {
    let proposals = derive::derive_proposals(ops, report);
    // 治理开关只读一次：下方「通知过滤」与「apply 闸」两处决策共用同一值——
    // 两次独立读盘在并发改档（设置页 toggle）时可能不一致，通知与落库口径分裂
    let auto_allowed = policy::auto_apply_allowed(emit::app_handle());
    // Phase 2：emit（audit）之前先把达门槛的子集挑出来交给 apply——
    // emit 的 24h dedup 会吞掉重复提案，apply 侧靠 evo:<proposal_id> 持久幂等，
    // 两条去重链路互不影响。
    // gate 判定走策略层自由函数
    let gated: Vec<_> = {
        use crate::evolution::strategy::gate_decision;
        proposals
            .iter()
            .filter(|p| {
                matches!(
                    gate_decision(p),
                    crate::evolution::strategy::GateDecision::Approved
                )
            })
            .cloned()
            .collect()
    };

    // 补  A 漏的连线（）：落盘候选池，让 shadow 钩子能读到。
    // dedup by proposal_id：跳过 24h 内已存在的。
    if let Some(app) = emit::app_handle() {
        // write_proposals 内部是 read_all→dedup→append 完整 RMW——必须整体
        // 持 store 锁（与 panel 的 toggle/rewrite 同一把，否则两套机制互踩）。
        // 锁只覆盖 write_proposals 本身；audit 写在锁外（不持锁跨磁盘 I/O）。
        let persist = {
            let _g = lock_evolution_store();
            candidate::write_proposals(app, &proposals)
        };
        match persist {
            Ok(new_entries) if !new_entries.is_empty() => {
                crate::audit_event!(
                    app,
                    crate::audit::AuditLevel::Info,
                    "evolution.proposal.persisted",
                    "count" => new_entries.len().to_string(),
                );
                // 通知中心逐条落消息；auto 档下达门槛、即将被自动应用的提案不打扰
                //（auto_allowed 为函数头单次读取，见上）
                let to_notify: Vec<_> = new_entries
                    .into_iter()
                    .filter(|e| {
                        !(auto_allowed && gated.iter().any(|p| p.proposal_id == e.proposal_id))
                    })
                    .collect();
                if let Err(e) = notify_evolution_proposals(app, &to_notify) {
                    eprintln!("[evolution] 提案通知落库失败：{e}");
                }
            }
            Ok(_) => {}
            Err(e) => crate::audit_event!(
                app,
                crate::audit::AuditLevel::Warn,
                "evolution.proposal_persist_failed",
                "error" => e.clone(),
            ),
        }
    } else {
        // app_handle 未注册时整个落盘块被跳过——留痕不静默（audit_event! 需要
        // AppHandle，None 分支只能用 eprintln）。
        eprintln!(
            "[evolution] app_handle 未注册，候选池落盘跳过（{} 条 proposals）",
            proposals.len()
        );
    }

    emit::emit_proposals(proposals);
    //  治理归一：applyPolicy=confirm 档不再自动落库，达门槛提案全留候选池
    // 等决策板人工批准（W1 执行器在 toggle ON 时落库）。缺字段/非法值/无句柄
    //（测试环境）= auto = 改前行为（默认档零变化）；confirm 分流留 Info 审计。
    if auto_allowed {
        apply::apply_from_consolidation(gated);
    } else if let Some(app) = emit::app_handle() {
        crate::audit_event!(
            app,
            crate::audit::AuditLevel::Info,
            "evolution.apply_deferred",
            "count" => gated.len().to_string(),
            "policy" => "confirm",
        );
    }
}
