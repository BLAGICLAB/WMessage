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

/// jsonl 读取共享内核（OCR r2 medium 采纳：record/entry 两处 30 行 read_all 收敛
/// 单点防漂移）。语义（拍板 #17=C）：文件不存在 → Ok(空)；首个非空行损坏 = 结构级
/// 损坏 → Err（fail-closed）；中间坏行 → stderr 留痕跳过返回好行（已知代价：坏行 id
/// 缺失时 dedup 调用方可能同 id 再追加——无害重复）。
pub(crate) fn read_jsonl<T: serde::de::DeserializeOwned>(
    path: &std::path::Path,
    label: &str,
) -> Result<Vec<T>, String> {
    use std::io::BufRead;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let f = std::fs::File::open(path).map_err(|e| format!("打开 {path:?} 失败：{e}"))?;
    let reader = std::io::BufReader::new(f);
    let mut out = Vec::new();
    let mut first_parsed = false;
    for (i, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| format!("读取第 {} 行失败：{e}", i + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<T>(&line) {
            Ok(v) => out.push(v),
            Err(e) if out.is_empty() && !first_parsed => {
                return Err(format!("第 1 行 JSON 错误：{e}"));
            }
            Err(e) => {
                eprintln!("[evolution] {label} jsonl 第 {} 行损坏已跳过：{e}", i + 1);
            }
        }
        first_parsed = true;
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
/// **一把锁覆盖两个文件是有意设计（OCR C3-4）**：toggle/delete 一次操作**同时**改两个文件，
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

/// 反思完成后的桥接入口（`memory::consolidate::run_consolidation` 末尾调用）。
///
/// 签名严格按 spec：不接收 AppHandle / session_id / 任何反向依赖 memory 内部的状态。
/// emit 所需的 AppHandle 由 `emit::register_app_handle` 在 App 启动时注册一次，
/// 本函数通过全局 OnceLock 取出——consolidate 侧 caller 无需关心。
pub fn post_consolidation(ops: &[ConsolidateOp], report: &ConsolidateReport) {
    let proposals = derive::derive_proposals(ops, report);
    // Phase 2：emit（audit）之前先把达门槛的子集挑出来交给 apply——
    // emit 的 24h dedup 会吞掉重复提案，apply 侧靠 evo:<proposal_id> 持久幂等，
    // 两条去重链路互不影响。
    let gated: Vec<_> = proposals
        .iter()
        .filter(|p| apply::auto_apply_gate(p))
        .cloned()
        .collect();

    // 补 R6 A 漏的连线（老板 12:29 拍板）：落盘候选池，让 shadow 钩子能读到。
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
            Ok(n) if n > 0 => crate::audit_event!(
                app,
                crate::audit::AuditLevel::Info,
                "evolution.proposal.persisted",
                "count" => n.to_string(),
            ),
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
    apply::apply_from_consolidation(gated);
}
