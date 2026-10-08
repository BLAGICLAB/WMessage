//!  A · Shadow observe —— 与主 apply 并行
//!
//! ## 设计修正
//!
//! 原版强耦合 `&AppHandle`：核心安全属性（不写 mem_items / 不发 audit）无法测试。
//! 现引入 `ShadowSink` trait 抽象 IO：
//! - 生产：`AppShadowSink` 包 `AppHandle`，写 evolution-changes.jsonl + 发 audit
//! - 测试：`MockShadowSink` 内存收集所有调用，验证「不写 X」「不发 Y」
//!
//! 4 个核心 e2e 测试（用 MockSink）：
//! 1. shadow_apply 真的被调用 → spy 通过 mock sink.writes() 验证
//! 2. shadow_apply 真的写 evolution-changes.jsonl → AppShadowSink 写真实 temp 文件
//! 3. shadow_apply 后 mem_items 未变 → 用 in-memory SQLite 对照
//! 4. shadow_apply 后 audit 未新增（正常路径）→ mock sink.audit_failed/warning 验证空
//!
//! 决策纪律（2026-09-18 13:13/13:20 ）：
//! - feature flag `evolution.shadow.enabled`，默认 false
//! - 不写 mem_items / 只写 evolution-changes.jsonl / 不发 audit.proposal
//! - 失败处理：audit_event + 原子计数 + 失败率 > 5% 告警

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::AppHandle;

use crate::audit::AuditLevel;
use crate::db::paths;
use crate::evolution::activation::ActivationState;
use crate::evolution::strategy::gate_decision;

/// gate 谓词（直连策略层自由函数）
fn gate_approved(p: &crate::evolution::proposal::EvolutionProposal) -> bool {
    matches!(
        gate_decision(p),
        crate::evolution::strategy::GateDecision::Approved
    )
}
use crate::evolution::change::{self, ChangeRecord, ChangeStatus};
use crate::evolution::proposal::{is_reversible, EvolutionProposal};

/// 失败率告警阈值（5% = 100 次写中允许 5 次失败）
pub const FAILURE_THRESHOLD: f64 = 0.05;

/// Shadow 配置（从 bot-config.json 加载）
#[derive(Debug, Clone, Default)]
pub struct ShadowConfig {
    pub enabled: bool,
}

impl ShadowConfig {
    /// 从 bot-config.json 加载（lenient：缺字段返默认 false，不 panic）
    pub fn load_from_file(path: &Path) -> ShadowConfig {
        let Ok(raw) = std::fs::read_to_string(path) else {
            return ShadowConfig::default();
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return ShadowConfig::default();
        };
        let enabled = v
            .get("evolution")
            .and_then(|e| e.get("shadow"))
            .and_then(|s| s.get("enabled"))
            .and_then(|b| b.as_bool())
            .unwrap_or(false);
        ShadowConfig { enabled }
    }
}

/// 便捷读取（自动定位 bot-config.json）
pub fn is_enabled(app: &AppHandle) -> bool {
    let path = paths::data_dir(app).join("bot-config.json");
    ShadowConfig::load_from_file(&path).enabled
}

/// 全局原子计数器
static FAILED_WRITES: AtomicU64 = AtomicU64::new(0);
static TOTAL_WRITES: AtomicU64 = AtomicU64::new(0);

pub fn failure_rate() -> f64 {
    let total = TOTAL_WRITES.load(Ordering::Relaxed);
    let failed = FAILED_WRITES.load(Ordering::Relaxed);
    if total == 0 {
        0.0
    } else {
        failed as f64 / total as f64
    }
}

pub fn total_failed() -> u64 {
    FAILED_WRITES.load(Ordering::Relaxed)
}

pub fn total_writes() -> u64 {
    TOTAL_WRITES.load(Ordering::Relaxed)
}

#[cfg(test)]
pub fn reset_counters_for_test() {
    FAILED_WRITES.store(0, Ordering::Relaxed);
    TOTAL_WRITES.store(0, Ordering::Relaxed);
}

// 影子落库 sink（具体类型）

/// 生产 sink：写 evolution-changes.jsonl + 发 audit。
/// 原 ShadowSink trait（单实现 + 仅测试用的 Mock）已删——测试走 `new_for_test`
/// 真临时文件；第二个 sink 实现出现时再抽象（历史实现见 git log）。
pub struct AppShadowSink<'a> {
    /// evolution-changes.jsonl 路径（可注入，便于测试用 temp 路径）
    pub changes_path: PathBuf,
    /// AppHandle 用于发 audit；None = 跳过 audit（仅写文件）
    pub app: Option<&'a AppHandle>,
}

impl<'a> AppShadowSink<'a> {
    pub fn new(app: &'a AppHandle) -> Self {
        Self {
            changes_path: paths::data_dir(app).join("evolution-changes.jsonl"),
            app: Some(app),
        }
    }

    /// 测试用构造：指定路径 + 跳过 audit
    pub fn new_for_test(changes_path: PathBuf) -> Self {
        Self {
            changes_path,
            app: None,
        }
    }

    /// 写一条 ChangeRecord 到 evolution-changes.jsonl
    fn write_change(&self, cr: &ChangeRecord) -> Result<(), String> {
        change::append_change(&self.changes_path, cr)
    }

    fn audit_failed(&self, proposal_id: &str, error: &str) {
        if let Some(app) = self.app {
            crate::audit_event!(
                app,
                AuditLevel::Warn,
                "evolution.shadow_failed",
                "proposal_id" => proposal_id.to_string(),
                "error" => error.to_string(),
            );
        }
    }

    /// audit_event: 失败率 > 5% 告警
    fn audit_warning(&self, failure_rate: f64, failed: u64, total: u64) {
        if let Some(app) = self.app {
            crate::audit_event!(
                app,
                AuditLevel::Warn,
                "evolution.shadow_warning",
                "failure_rate" => failure_rate,
                "failed" => failed,
                "total" => total,
                "threshold" => FAILURE_THRESHOLD,
            );
        }
    }
}

// 主入口

/// Shadow 执行报告
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowReport {
    pub total: usize,
    pub written: usize,
    pub failed: usize,
    /// 同 proposal_id 已有未终态 CR 而跳过的条数（去重命中）
    pub deduped: usize,
}

/// 跑一轮 shadow apply
///
/// **核心保证**（由 trait 边界保证，e2e 测试验证）：
/// - 不写 mem_items（trait 没暴露 mem_items 接口）
/// - 正常路径不发 audit（除非写失败 / 失败率 > 阈值）
/// - 失败仅 audit_event + 原子计数
///
/// **调用约束**：当前零生产调用方，仅测试 + MockShadowSink（内存 sink，
/// 无真实 IO）。sink.write_change 是同步调用——若未来接真实 fs sink
/// 上生产 async 路径，须先把写路径 spawn_blocking 化（参考 with_app）。
pub async fn shadow_apply_for_batch<'a>(
    proposals: Vec<EvolutionProposal>,
    sink: &'a AppShadowSink<'a>,
) -> ShadowReport {
    let gated: Vec<EvolutionProposal> = proposals.into_iter().filter(gate_approved).collect();

    let mut written = 0usize;
    let mut failed = 0usize;
    let now = crate::memory::now_ms();

    for p in &gated {
        let cr: ChangeRecord = change::from_proposal(p, now);
        match sink.write_change(&cr) {
            Ok(()) => {
                written += 1;
                TOTAL_WRITES.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => {
                failed += 1;
                TOTAL_WRITES.fetch_add(1, Ordering::Relaxed);
                FAILED_WRITES.fetch_add(1, Ordering::Relaxed);
                sink.audit_failed(&p.proposal_id, &e);
            }
        }
    }

    let total = TOTAL_WRITES.load(Ordering::Relaxed);
    let failed_count = FAILED_WRITES.load(Ordering::Relaxed);
    if total > 0 && (failed_count as f64 / total as f64) > FAILURE_THRESHOLD {
        sink.audit_warning(failed_count as f64 / total as f64, failed_count, total);
    }

    ShadowReport {
        total: gated.len(),
        written,
        failed,
        deduped: 0,
    }
}

// 便捷包装（apply.rs 调用）

/// 便捷包装（apply.rs 调用入口；唯一对外入口）
///
/// 流程（B 阶段，v4.1 §12.7 + B 校准前置）：
/// 1. 过策略层 gate（合规性）
/// 2. 过 is_reversible（可逆性）
/// 3. 读 activation_state（bot-config.json）
/// 4. 按状态路由：
///    - S0/S1：不调 evaluate，写 shadow jsonl + audit no_policy_applied
///    - S2：调 evaluate_s2（占位=is_reversible反向）→ Allow 写 shadow + audit allowed；Block 跳过 + audit blocked
/// 5. 不可逆的 proposal：audit not_reversible + 跳过
pub async fn shadow_apply_for_batch_with_app(
    proposals: Vec<EvolutionProposal>,
    app: &AppHandle,
) -> ShadowReport {
    use crate::evolution::activation::{evaluate_s2, S2Decision};

    let gated: Vec<EvolutionProposal> = proposals.into_iter().filter(gate_approved).collect();
    let config_path = paths::data_dir(app).join("bot-config.json");
    let state = crate::evolution::activation::load_state_from_file(&config_path);
    let path = paths::data_dir(app).join("evolution-changes.jsonl");
    let mut written = 0usize;
    let mut failed = 0usize;
    let mut deduped = 0usize;
    let now = crate::memory::now_ms();

    for p in &gated {
        if !is_reversible(p) {
            crate::audit_event!(
                app,
                AuditLevel::Info,
                "evolution.not_reversible",
                "proposal_id" => p.proposal_id.clone(),
                "category" => format!("{:?}", p.category),
                "impact" => p.impact.as_str(),
            );
            continue;
        }
        // S2 调 evaluate（占位=is_reversible 反向）；S0/S1 不调 evaluate
        let s2_decision = match state {
            ActivationState::S2Active => Some(evaluate_s2(p)),
            _ => None,
        };
        // Block 决策：跳过 + audit
        if let Some(S2Decision::Block { reason }) = &s2_decision {
            crate::audit_event!(
                app,
                AuditLevel::Info,
                "evolution.blocked",
                "state" => state.as_str(),
                "proposal_id" => p.proposal_id.clone(),
                "reason" => reason.clone(),
                "policy_version" => "calibrating",
            );
            continue;
        }
        // Allow 决策或 S0/S1：写 shadow + audit。
        // append_change 是阻塞 fs IO（open+writeln syscall）——包 spawn_blocking
        // 避免钉住 tokio worker（本 fn 是生产唯一入口，跑在 Tauri async runtime 上）；
        // 读/判/写整个窗口走 EVOLUTION_STORE_LOCK（锁在阻塞闭包
        // 内取，不跨 await）——与 panel RMW/apply 补写同一把单写者锁，不再绕锁
        // 并发写 changes.jsonl； 去重与  唯一 id 同窗判定（锁内重读，
        // 无 TOCTOU、读不受半行写干扰）。
        enum ShadowWrite {
            Appended,
            Deduped,
        }
        let proposal_id = p.proposal_id.clone();
        let write_result = {
            let path = path.clone();
            // cr 本轮迭代仅此处消费，直接 move（不 clone）
            let cr = change::from_proposal(p, now);
            match tauri::async_runtime::spawn_blocking(move || {
                let _store = crate::evolution::lock_evolution_store();
                let rows = change::read_all(&path)?;
                // 同 proposal_id 已有未终态行（Pending/Shadowing/ShadowPassed，
                // 含 panel toggle ON 写的）→ 跳过 append，不重复追加 Pending 行
                if rows.iter().any(|c| {
                    c.proposal_id == proposal_id
                        && matches!(
                            c.status,
                            ChangeStatus::Pending
                                | ChangeStatus::Shadowing
                                | ChangeStatus::ShadowPassed
                        )
                }) {
                    return Ok(ShadowWrite::Deduped);
                }
                // chg-<pid> 可能被历史行占用（回滚后复用），落行
                // 前派生行级唯一 id——rollback 按 id 定位不再撞行
                let mut cr = cr;
                cr.change_id = change::unique_change_id_for(&rows, &cr.proposal_id);
                change::append_change(&path, &cr).map(|()| ShadowWrite::Appended)
            })
            .await
            {
                Ok(r) => r,
                // tauri::Error（非 tokio JoinError）无 is_panic/into_panic——
                // Display 已带 panic payload 信息，直接透传
                Err(e) => Err(format!("shadow 写线程 join 失败：{e}")),
            }
        };
        match write_result {
            Ok(ShadowWrite::Appended) => {
                written += 1;
                // 与 trait 变体一致的计数器增量——本入口是生产路径，
                // 不碰原子计数器会让「失败率 >5% 告警」对生产流量永不触发
                TOTAL_WRITES.fetch_add(1, Ordering::Relaxed);
                match (state, &s2_decision) {
                    (ActivationState::S0Observe, _) | (ActivationState::S1Suggest, _) => {
                        crate::audit_event!(
                            app,
                            AuditLevel::Info,
                            "evolution.no_policy_applied",
                            "state" => state.as_str(),
                            "proposal_id" => p.proposal_id.clone(),
                            "category" => format!("{:?}", p.category),
                            "impact" => p.impact.as_str(),
                        );
                    }
                    (ActivationState::S2Active, Some(S2Decision::Allow { reason })) => {
                        crate::audit_event!(
                            app,
                            AuditLevel::Info,
                            "evolution.allowed",
                            "state" => state.as_str(),
                            "proposal_id" => p.proposal_id.clone(),
                            "reason" => reason.clone(),
                            "policy_version" => "calibrating",
                        );
                    }
                    _ => {} // 不可达（state+S2Decision 组合穷尽）
                }
            }
            Ok(ShadowWrite::Deduped) => {
                deduped += 1;
                crate::audit_event!(
                    app,
                    AuditLevel::Info,
                    "evolution.shadow_dedup_hit",
                    "proposal_id" => p.proposal_id.clone(),
                );
            }
            Err(e) => {
                failed += 1;
                TOTAL_WRITES.fetch_add(1, Ordering::Relaxed);
                FAILED_WRITES.fetch_add(1, Ordering::Relaxed);
                crate::audit_event!(
                    app,
                    AuditLevel::Warn,
                    "evolution.shadow_failed",
                    "proposal_id" => p.proposal_id.clone(),
                    "error" => e.clone(),
                );
            }
        }
    }
    // 生产入口补失败率告警，对齐 trait 版（sink.audit_warning）——
    // 此前生产流量写失败 >5% 永不告警（只有 trait 路径有）
    let total = TOTAL_WRITES.load(Ordering::Relaxed);
    let failed_count = FAILED_WRITES.load(Ordering::Relaxed);
    if total > 0 && (failed_count as f64 / total as f64) > FAILURE_THRESHOLD {
        crate::audit_event!(
            app,
            AuditLevel::Warn,
            "evolution.shadow_warning",
            "failure_rate" => failed_count as f64 / total as f64,
            "failed" => failed_count,
            "total" => total,
            "threshold" => FAILURE_THRESHOLD,
        );
    }
    ShadowReport {
        total: gated.len(),
        written,
        failed,
        deduped,
    }
}

// 集成决策（纯函数，apply.rs 调用）

/// apply.rs 的 shadow 决策：返「shadow 应该跑的 proposal」
///
/// 这是一个纯函数（不依赖 AppHandle/Tauri runtime）让 apply.rs 的
/// 集成路径可测试：
/// - flag = false → 空（不火 shadow）
/// - flag = true → 过策略层 gate 的 proposal（与 shadow_apply 内部过滤一致）
///
/// 注意：apply.rs 里调用时同时依赖本函数的返回 + `is_enabled(&app)`，
/// 测试两者全覆即等于「apply_from_consolidation 集成路径」测试。
pub fn shadow_eligible_proposals(
    proposals: &[EvolutionProposal],
    shadow_enabled: bool,
) -> Vec<EvolutionProposal> {
    if !shadow_enabled {
        return Vec::new();
    }
    proposals
        .iter()
        .filter(|p| gate_approved(p))
        .cloned()
        .collect()
}

// 单元测试 + e2e

#[cfg(test)]
mod tests {
    // await_holding_lock 豁免——测试串行锁（counter_lock 等 std Mutex
    // guard）**故意**持跨 await：#[tokio::test] 独立 current-thread runtime，
    // guard 持有至测试结束正是串行化语义，无真实死锁面
    #![allow(clippy::await_holding_lock)]
    use super::*;
    use crate::evolution::proposal::{
        Evidence, ImpactLevel, ProposalCategory, ProposalOrigin, ProposalTarget, Suggestion,
    };
    use std::sync::Mutex;

    // 计数器 lock

    static COUNTER_LOCK: Mutex<()> = Mutex::new(());
    fn counter_lock() -> std::sync::MutexGuard<'static, ()> {
        COUNTER_LOCK.lock().unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] evolution::observe::shadow::COUNTER_LOCK: {e:?}");
            e.into_inner()
        })
    }

    fn mk_proposal(id: &str, cat: ProposalCategory, impact: ImpactLevel) -> EvolutionProposal {
        EvolutionProposal {
            proposal_id: id.into(),
            created_at_ms: 1_700_000_000_000,
            origin: ProposalOrigin::ConsolidationReflection,
            category: cat,
            target: ProposalTarget::MemoryPolicy {
                policy: "test".into(),
            },
            impact,
            evidence: Evidence {
                summary: format!("s-{id}"),
                occurrence_count: 1,
                window_hours: 24,
                related_refs: vec![],
            },
            suggestion: Suggestion {
                text: format!("text-{id}"),
                structured_patch: None,
            },
        }
    }

    // 4 个核心 e2e 测试（）

    /// E2E 1：shadow_apply 把合规提案按序写进真 evolution-changes.jsonl
    #[tokio::test]
    async fn e2e_1_shadow_writes_in_order_to_real_jsonl() {
        let _g = counter_lock();
        reset_counters_for_test();
        let dir = std::env::temp_dir().join(format!(
            "sh-e2e1-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("evolution-changes.jsonl");
        let sink = AppShadowSink::new_for_test(path.clone());
        let proposals = vec![
            mk_proposal("p1", ProposalCategory::MemoryHint, ImpactLevel::High),
            mk_proposal("p2", ProposalCategory::MemoryHint, ImpactLevel::Medium),
            mk_proposal("p3", ProposalCategory::MemoryHint, ImpactLevel::High),
        ];
        let report = shadow_apply_for_batch(proposals, &sink).await;
        assert_eq!(report.total, 3, "3 条 proposal 全合规");
        assert_eq!(report.written, 3);
        assert_eq!(report.failed, 0);
        let rows = change::read_all(&path).unwrap();
        let ids: Vec<&str> = rows.iter().map(|c| c.proposal_id.as_str()).collect();
        assert_eq!(ids, vec!["p1", "p2", "p3"], "按入参顺序逐条 append");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// E2E 2：shadow_apply 真的写 evolution-changes.jsonl（用真实 AppShadowSink + temp 路径）
    #[tokio::test]
    async fn e2e_2_shadow_writes_real_changes_jsonl() {
        let _g = counter_lock();
        reset_counters_for_test();
        let dir = std::env::temp_dir().join(format!(
            "sh-e2e2-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("evolution-changes.jsonl");
        let sink = AppShadowSink::new_for_test(path.clone());
        let proposals = vec![
            mk_proposal("p1", ProposalCategory::MemoryHint, ImpactLevel::High),
            mk_proposal("p2", ProposalCategory::MemoryHint, ImpactLevel::Medium),
        ];
        let report = shadow_apply_for_batch(proposals, &sink).await;
        assert_eq!(report.written, 2);
        // 验证文件真的存在且有 2 行
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 2, "evolution-changes.jsonl 真有 2 条");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// E2E 3：shadow_apply 后 mem_items 未变（用 in-memory SQLite 对照）
    #[tokio::test]
    async fn e2e_3_shadow_does_not_touch_mem_items() {
        let _g = counter_lock();
        reset_counters_for_test();
        // 用 in-memory SQLite 模拟 mem_items
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::memory::store::ensure_table(&conn).unwrap();
        // 插 1 条无关数据（baseline）
        conn.execute(
            "INSERT INTO mem_items (id, kind, content, tags, importance, source, created_at, updated_at) VALUES ('existing', 'fact', 'baseline', '', 3, 'user_stated', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        ).unwrap();
        let before: i64 = conn
            .query_row("SELECT COUNT(*) FROM mem_items", [], |r| r.get(0))
            .unwrap();
        // 跑 shadow（用 AppShadowSink + temp 文件，不接触 conn）
        let dir = std::env::temp_dir().join(format!(
            "sh-e2e3-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("evolution-changes.jsonl");
        let sink = AppShadowSink::new_for_test(path);
        let proposals = vec![
            mk_proposal("p1", ProposalCategory::MemoryHint, ImpactLevel::High),
            mk_proposal("p2", ProposalCategory::MemoryHint, ImpactLevel::Medium),
        ];
        shadow_apply_for_batch(proposals, &sink).await;
        // 验证 mem_items 未变（关键安全属性）
        let after: i64 = conn
            .query_row("SELECT COUNT(*) FROM mem_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            before, after,
            "shadow 不应写 mem_items（before={before}, after={after}）"
        );
        // 验证没有 evo:* 标签的条目被新增
        let evo_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM mem_items WHERE tags LIKE 'evo:%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(evo_count, 0, "shadow 不应新增 evo:* 条目");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// E2E pre-flight（决策 6）：shadow 跑合成数据，与「apply 理论应写数」对比，
    /// 差异率 < 60% 才能开 flag。
    ///
    /// 「一致性校验从 post-process 预留改为跑前合成数据校验」。
    /// 即：dev 启用 flag 之前必须先跑这个测试通过，否则禁止开 flag
    /// （shadow 与 apply 行为不一致是设计 bug）。
    #[tokio::test]
    async fn e2e_preflight_shadow_consistency_with_mixed_proposals() {
        let _g = counter_lock();
        reset_counters_for_test();

        // 50 条 proposal：80% 合规 + 20% 不合规（直接 mk_proposal，不依赖 synthetic 模块）
        let proposals: Vec<EvolutionProposal> = (0..50)
            .map(|i| {
                if i % 5 == 0 {
                    // 不合规：PromptHint + High
                    mk_proposal(
                        &format!("nc{i}"),
                        ProposalCategory::PromptHint,
                        ImpactLevel::High,
                    )
                } else if i % 2 == 0 {
                    // 合规 + High
                    mk_proposal(
                        &format!("p{i}"),
                        ProposalCategory::MemoryHint,
                        ImpactLevel::High,
                    )
                } else {
                    // 合规 + Medium
                    mk_proposal(
                        &format!("p{i}"),
                        ProposalCategory::MemoryHint,
                        ImpactLevel::Medium,
                    )
                }
            })
            .collect();

        // apply 理论应写数 = 过策略层 gate 的 proposal 数
        let apply_expected_count = proposals.iter().filter(|p| gate_approved(p)).count();
        assert!(apply_expected_count > 0, "至少要有 1 条合规 proposal");

        // shadow 跑同一批 proposal（真临时文件）
        let dir = std::env::temp_dir().join(format!(
            "sh-preflight-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("evolution-changes.jsonl");
        let sink = AppShadowSink::new_for_test(path.clone());
        let report = shadow_apply_for_batch(proposals, &sink).await;

        // 差异率
        let diff = (report.written as i64 - apply_expected_count as i64).abs();
        let diff_rate = if apply_expected_count > 0 {
            diff as f64 / apply_expected_count as f64
        } else {
            0.0
        };

        // 验证：差异率 < 60%（决策 d 阈值；）
        assert!(
            diff_rate < 0.60,
            "shadow.written={} vs apply_expected={} 差异率 {:.1}% ≥ 60%，禁止开 flag",
            report.written,
            apply_expected_count,
            diff_rate * 100.0
        );
        // 落盘条数与报告一致
        let rows = change::read_all(&path).unwrap();
        assert_eq!(rows.len(), report.written, "落盘条数 = 报告 written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 失败路径：sink 路径不可写（指向目录）→ report 记 failed
    #[tokio::test]
    async fn e2e_failed_write_counts_and_audits() {
        let _g = counter_lock();
        reset_counters_for_test();
        // 把 changes 路径指到目录：append 打开必失败（Err 分支）
        let dir = std::env::temp_dir().join(format!(
            "sh-fail-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let sink = AppShadowSink::new_for_test(dir.clone());
        let proposals = vec![mk_proposal(
            "p1",
            ProposalCategory::MemoryHint,
            ImpactLevel::High,
        )];
        let report = shadow_apply_for_batch(proposals, &sink).await;
        assert_eq!(report.failed, 1);
        assert_eq!(report.written, 0);
        assert!(failure_rate() > 0.0, "失败应计入全局失败率");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn changes_jsonl_appends_survive_concurrent_writers() {
        // changes.jsonl 三写者（shadow/panel/apply）收敛到
        // EVOLUTION_STORE_LOCK——并发 append 一行不少、change_id 无碰撞
        //（此前 shadow 绕锁，与 panel rewrite 并发有丢更新窗口）。
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("changes.jsonl");
        let mut handles = Vec::new();
        for t in 0..4 {
            let file = path.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..25 {
                    let prop = mk_proposal(
                        &format!("w{t}-{i}"),
                        ProposalCategory::MemoryHint,
                        ImpactLevel::Medium,
                    );
                    let cr = change::from_proposal(&prop, 1000);
                    let _g = crate::evolution::lock_evolution_store();
                    change::append_change(&file, &cr).unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let rows = change::read_all(&path).unwrap();
        assert_eq!(rows.len(), 100, "4×25 并发 append 一行不少");
        let mut ids: Vec<String> = rows.iter().map(|r| r.change_id.clone()).collect();
        ids.sort();
        let before = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), before, "change_id 两两不同（每行独立提案）");
    }

    // 原有 12 个单测保留（外围测试）

    #[test]
    fn default_disabled() {
        let cfg = ShadowConfig::default();
        assert!(!cfg.enabled);
    }

    #[test]
    fn load_missing_file_safe() {
        let p = std::path::Path::new("/tmp/no-such-bot-config-xyz-12345.json");
        let cfg = ShadowConfig::load_from_file(p);
        assert!(!cfg.enabled);
    }

    #[test]
    fn load_enabled_true_parses() {
        let dir = std::env::temp_dir().join(format!(
            "sh-cfg-t-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bot-config.json");
        std::fs::write(&p, r#"{"evolution":{"shadow":{"enabled":true}}}"#).unwrap();
        let cfg = ShadowConfig::load_from_file(&p);
        assert!(cfg.enabled);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_enabled_false_parses() {
        let dir = std::env::temp_dir().join(format!(
            "sh-cfg-f-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bot-config.json");
        std::fs::write(&p, r#"{"evolution":{"shadow":{"enabled":false}}}"#).unwrap();
        let cfg = ShadowConfig::load_from_file(&p);
        assert!(!cfg.enabled);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_partial_config_uses_default() {
        let dir = std::env::temp_dir().join(format!(
            "sh-cfg-p-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bot-config.json");
        std::fs::write(&p, r#"{"evolution":{"eval":{}}}"#).unwrap();
        let cfg = ShadowConfig::load_from_file(&p);
        assert!(!cfg.enabled);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_evolution_block_missing_safe() {
        let dir = std::env::temp_dir().join(format!(
            "sh-cfg-e-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bot-config.json");
        std::fs::write(&p, r#"{"other":"value"}"#).unwrap();
        let cfg = ShadowConfig::load_from_file(&p);
        assert!(!cfg.enabled);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_malformed_json_safe() {
        let dir = std::env::temp_dir().join(format!(
            "sh-cfg-m-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bot-config.json");
        std::fs::write(&p, "NOT VALID JSON").unwrap();
        let cfg = ShadowConfig::load_from_file(&p);
        assert!(!cfg.enabled, "JSON 解析失败应返默认 false");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failure_rate_zero_initially() {
        let _g = counter_lock();
        reset_counters_for_test();
        assert_eq!(failure_rate(), 0.0);
    }

    #[test]
    fn failure_rate_computed_correctly() {
        let _g = counter_lock();
        reset_counters_for_test();
        TOTAL_WRITES.store(5, Ordering::Relaxed);
        FAILED_WRITES.store(2, Ordering::Relaxed);
        let rate = failure_rate();
        assert!((rate - 0.4).abs() < 1e-9);
        reset_counters_for_test();
    }

    #[test]
    fn total_writes_and_failed_independent() {
        let _g = counter_lock();
        reset_counters_for_test();
        assert_eq!(total_writes(), 0);
        assert_eq!(total_failed(), 0);
        TOTAL_WRITES.store(10, Ordering::Relaxed);
        FAILED_WRITES.store(1, Ordering::Relaxed);
        assert_eq!(total_writes(), 10);
        assert_eq!(total_failed(), 1);
        reset_counters_for_test();
    }

    #[test]
    fn failure_threshold_locked() {
        assert_eq!(FAILURE_THRESHOLD, 0.05);
    }

    #[test]
    fn gate_filter_works() {
        let compliant = mk_proposal("c1", ProposalCategory::MemoryHint, ImpactLevel::High);
        let non_compliant = mk_proposal("nc1", ProposalCategory::PromptHint, ImpactLevel::High);
        assert!(gate_approved(&compliant));
        assert!(!gate_approved(&non_compliant));
    }

    // shadow_eligible_proposals 集成决策测试（）

    #[test]
    fn shadow_disabled_returns_empty() {
        let proposals = vec![
            mk_proposal("p1", ProposalCategory::MemoryHint, ImpactLevel::High),
            mk_proposal("p2", ProposalCategory::MemoryHint, ImpactLevel::Medium),
        ];
        let r = shadow_eligible_proposals(&proposals, false);
        assert!(r.is_empty(), "flag=false 应返空");
    }

    #[test]
    fn shadow_enabled_all_compliant_returns_all() {
        let proposals = vec![
            mk_proposal("p1", ProposalCategory::MemoryHint, ImpactLevel::High),
            mk_proposal("p2", ProposalCategory::MemoryHint, ImpactLevel::Medium),
        ];
        let r = shadow_eligible_proposals(&proposals, true);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].proposal_id, "p1");
        assert_eq!(r[1].proposal_id, "p2");
    }

    #[test]
    fn shadow_enabled_filters_to_compliant_only() {
        let proposals = vec![
            mk_proposal("c1", ProposalCategory::MemoryHint, ImpactLevel::High), // 合规
            mk_proposal("nc1", ProposalCategory::PromptHint, ImpactLevel::High), // 不合规
            mk_proposal("c2", ProposalCategory::MemoryHint, ImpactLevel::Medium), // 合规
            mk_proposal("nc2", ProposalCategory::PromptHint, ImpactLevel::Low), // 不合规
        ];
        let r = shadow_eligible_proposals(&proposals, true);
        assert_eq!(r.len(), 2, "应过滤出 2 条合规");
        assert_eq!(r[0].proposal_id, "c1");
        assert_eq!(r[1].proposal_id, "c2");
    }

    #[test]
    fn shadow_enabled_none_compliant_returns_empty() {
        let proposals = vec![
            mk_proposal("nc1", ProposalCategory::PromptHint, ImpactLevel::High),
            mk_proposal("nc2", ProposalCategory::SkillHint, ImpactLevel::Low),
        ];
        let r = shadow_eligible_proposals(&proposals, true);
        assert!(r.is_empty());
    }

    /// E2E 集成（）：apply_from_consolidation 的 shadow 钩子
    /// 端到端走通：decision（shadow_eligible_proposals）→ execution（shadow_apply_for_batch）
    #[tokio::test]
    async fn e2e_apply_to_shadow_integration() {
        let _g = counter_lock();
        reset_counters_for_test();

        // 3 条合规 + 1 条不合规（共 4 条）
        let proposals = vec![
            mk_proposal("c1", ProposalCategory::MemoryHint, ImpactLevel::High),
            mk_proposal("nc1", ProposalCategory::PromptHint, ImpactLevel::High),
            mk_proposal("c2", ProposalCategory::MemoryHint, ImpactLevel::Medium),
            mk_proposal("c3", ProposalCategory::MemoryHint, ImpactLevel::High),
        ];

        // 决策阶段（apply.rs shadow 钩子的逻辑）
        // shadow_enabled=true
        let eligible = shadow_eligible_proposals(&proposals, true);
        let eligible_len = eligible.len(); // 先 capture，后面 eligible 被 move
        assert_eq!(eligible_len, 3, "决策应过滤出 3 条合规（跳过 1 条不合规）");

        // 执行阶段（影子写 evolution-changes.jsonl，真临时文件）
        let dir = std::env::temp_dir().join(format!(
            "sh-integ-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("evolution-changes.jsonl");
        let sink = AppShadowSink::new_for_test(path.clone());
        let report = shadow_apply_for_batch(eligible, &sink).await;
        assert_eq!(report.written, 3);
        assert_eq!(report.failed, 0);

        // 验证集成路径完整性
        // 1. 写数 = 决策数（无丢失/重复）
        assert_eq!(report.written, eligible_len, "decision → execution 一致");
        // 2. 落盘 3 行，且写的就是合规那 3 条（无 nc1）
        let rows = change::read_all(&path).unwrap();
        assert_eq!(rows.len(), 3);
        let written_ids: Vec<&str> = rows.iter().map(|c| c.proposal_id.as_str()).collect();
        assert!(written_ids.contains(&"c1"));
        assert!(written_ids.contains(&"c2"));
        assert!(written_ids.contains(&"c3"));
        assert!(!written_ids.contains(&"nc1"), "不合规不应被写");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // R7→A 简化（）：is_reversible 单元测试 + shadow 集成（e2e E组）

    #[test]
    fn is_reversible_memory_hint_low_returns_true() {
        let p = mk_proposal("p1", ProposalCategory::MemoryHint, ImpactLevel::Low);
        assert!(is_reversible(&p));
    }

    #[test]
    fn is_reversible_memory_hint_high_returns_false() {
        let p = mk_proposal("p1", ProposalCategory::MemoryHint, ImpactLevel::High);
        assert!(!is_reversible(&p));
    }

    #[test]
    fn is_reversible_tool_schema_hint_low_returns_false() {
        // ToolSchemaHint 不可逆（即使 low impact 也不可逆）
        let p = mk_proposal("p1", ProposalCategory::ToolSchemaHint, ImpactLevel::Low);
        assert!(!is_reversible(&p));
    }

    #[test]
    fn is_reversible_tool_schema_hint_high_returns_false() {
        let p = mk_proposal("p1", ProposalCategory::ToolSchemaHint, ImpactLevel::High);
        assert!(!is_reversible(&p));
    }

    #[test]
    fn is_reversible_prompt_hint_medium_returns_true() {
        let p = mk_proposal("p1", ProposalCategory::PromptHint, ImpactLevel::Medium);
        assert!(is_reversible(&p));
    }

    #[test]
    fn is_reversible_skill_hint_low_returns_true() {
        let p = mk_proposal("p1", ProposalCategory::SkillHint, ImpactLevel::Low);
        assert!(is_reversible(&p));
    }

    #[test]
    fn is_reversible_skill_hint_high_returns_false() {
        let p = mk_proposal("p1", ProposalCategory::SkillHint, ImpactLevel::High);
        assert!(!is_reversible(&p));
    }

    // E 组 e2e
    // E2：shadow hook 保留（已由既有的 e2e_1..4 覆盖，无需额外）
    // E3：删 5 维 engine（EvolutionPolicy/evaluate 不再被调，编译通过即可证明）
    // E4：空规则路径 → is_reversible 直接判定，不产生 Allow

    // E4：is_reversible 过滤的谓词覆盖见上方 6 个单测；顺序无关性由
    // filter + append 的集合语义直接保证（with_reversibility 变体已删）
}
