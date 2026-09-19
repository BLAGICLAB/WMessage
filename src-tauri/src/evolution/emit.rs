//! Phase 1.4：副作用层 —— 把 `EvolutionProposal` 写进 audit + 进程内 24h 去重。
//!
//! ## 设计动机
//!
//! `memory::consolidate::run_consolidation` 末尾追加一行 `evolution::post_consolidation(...)`，
//! 但 post_consolidation 的签名严格限定 `(&[ConsolidateOp], &ConsolidateReport)`，不接收 AppHandle。
//! 而 `audit_event!` 必须有 AppHandle 才能写 `bot.log`。
//!
//! 解法：本模块维护一个全局 `OnceLock<AppHandle<tauri::Wry>>`，
//! 由 `lib.rs::run()` 的 `.setup` 回调里调一次 `register_app_handle` 完成注册。
//! `emit_proposals` 内部从全局取出 AppHandle，调 `audit_event!` 写 audit。
//! 这一来 post_consolidation 仍只接 ops + report（caller 无感）；
//! 二来 memory 侧不依赖 evolution 任何内部符号。
//!
//! ## 去重语义
//!
//! 进程内 `HashMap<proposal_id, last_emit_ms>`，24h 之外清掉。
//! 同一 proposal_id 24h 内第二次 emit 静默跳过（不写 audit、不计数）。
//! 注意：**不落盘**，进程重启后 dedup 状态丢失——这是 spec 显式要求。
//!
//! ## dedup 与 audit 解耦
//!
//! dedup 总是执行（即使 AppHandle 未注册）；audit 仅在 AppHandle 就绪时执行。
//! 这样测试不必构造 mock Wry AppHandle 即可验 dedup 行为。
//! 生产环境若 AppHandle 未注册属启动顺序错误，eprintln 后仅跳过 audit 不跳过 dedup——
//! 修复启动顺序后重启即可，不丢去重状态。
//!
//! ## 不变量
//!
//! - 不调 LLM；
//! - 不写数据库表；
//! - 不发应用事件（仅 audit）；
//! - AppHandle 未注册时 `emit_proposals` 不 panic，只 eprintln。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use tauri::AppHandle;

use super::proposal::EvolutionProposal;

/// dedup TTL：24 小时（spec）。
const EMIT_DEDUP_TTL_HOURS: i64 = 24;
const EMIT_DEDUP_TTL_MS: i64 = EMIT_DEDUP_TTL_HOURS * 3_600_000;

/// AppHandle 全局（`lib.rs` 启动时注册一次）。
static APP_HANDLE: OnceLock<AppHandle<tauri::Wry>> = OnceLock::new();

/// 进程内 24h dedup 表：`proposal_id` → 上次 emit 时间戳（epoch ms）。
static EMITTED: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();

fn emitted_map() -> &'static Mutex<HashMap<String, i64>> {
    EMITTED.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 注册 AppHandle（仅 Tauri 启动时调一次，后续重复 set 静默失败）。
///
/// 调用方：`lib.rs::run()` 的 `.setup` 回调，紧跟其它 `app.manage(...)` 之后。
/// 必须在 `memory::consolidate::start_consolidation_scheduler` 之前调，
/// 否则 10 分钟内的首次 consolidate emit 会因 AppHandle 未就绪而走"跳过 audit"
/// 路径（不影响 dedup，但 audit 行不会写）。
///
/// 类型固定 `AppHandle<tauri::Wry>`：Tauri 默认 runtime 是 Wry，
/// production 全局只有这一个。测试路径不调本函数（见 `emit_proposals` 解耦说明）。
pub fn register_app_handle(app: AppHandle<tauri::Wry>) {
    let _ = APP_HANDLE.set(app);
}

/// 读取已注册的 AppHandle（供同模块的 `trace::maybe_record_trace` 等其他需要
/// 写 audit 的 evolution 子模块使用）。未注册返回 `None`，调用方应 eprintln 后
/// 跳过 audit 写入而非 panic。
pub(crate) fn app_handle() -> Option<&'static AppHandle<tauri::Wry>> {
    APP_HANDLE.get()
}

/// emit 报告（测试用）。生产代码忽略返回值。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct EmitReport {
    /// 处理过（计入 dedup 表）的 proposal 数；audit 是否实际写入取决于 AppHandle。
    pub written: usize,
    /// 因 24h 内重复而被跳过的 proposal 数。
    pub deduped: usize,
}

/// emit 一批 proposal：写 audit（如 AppHandle 就绪）+ dedup（总是执行）。
///
/// 入参 Vec 由 caller 负责构造（通常是 `derive::derive_proposals` 的输出）。
pub fn emit_proposals(proposals: Vec<EvolutionProposal>) -> EmitReport {
    let mut report = EmitReport::default();
    if proposals.is_empty() {
        return report;
    }
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut g = emitted_map().lock().unwrap_or_else(|e| { eprintln!("[mutex_poisoned] evolution::emit::emitted_map: {e:?}"); e.into_inner() });
    // 清掉超过 24h 的 id（防止 map 无限增长）
    g.retain(|_, ts| now_ms - *ts < EMIT_DEDUP_TTL_MS);

    let app = APP_HANDLE.get(); // Option<&AppHandle<tauri::Wry>>

    for p in proposals {
        if g.contains_key(&p.proposal_id) {
            report.deduped += 1;
            continue;
        }
        if let Some(app) = app {
            // 写一条 audit：event = "evolution.proposal"
            // kv 字段锁死——后续若加新字段会破坏 `grep evolution.proposal bot.log` 解析
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Info,
                "evolution.proposal",
                "proposal_id" => p.proposal_id.clone(),
                "category" => p.category.as_str(),
                "impact" => p.impact.as_str(),
                "origin" => format!("{:?}", p.origin),
                "target" => p.target.tag(),
                "occurrence_count" => p.evidence.occurrence_count,
                "window_hours" => p.evidence.window_hours,
                "summary" => p.evidence.summary.clone(),
                "suggestion" => p.suggestion.text.clone(),
            );
        } else {
            // AppHandle 未注册（理论上 setup 已调，不应发生；test 路径或启动顺序错误时）
            // 不 panic；dedup 仍记录——修复启动顺序重启后不丢去重状态
            eprintln!(
                "[evolution] AppHandle 未注册，跳过 audit 写入；proposal_id={}",
                p.proposal_id
            );
        }
        g.insert(p.proposal_id, now_ms);
        report.written += 1;
    }
    report
}

// ───────────────────────── 单元测试（spec 1.6 之 7）─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::proposal::{
        Evidence, ImpactLevel, ProposalCategory, ProposalOrigin, ProposalTarget, Suggestion,
    };

    /// 本模块测试共享全局 EMITTED 表（OnceLock 无法 reset），进程内并行下
    /// 表大小断言会互相干扰（cargo test --lib 实测复现）——串行锁对齐
    /// skill_e2e 的 SKILL_SCHED_TEST_LOCK 模式。
    static EMIT_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// 构造一条测试用 proposal。`id_prefix` 用于让每条 proposal 的 id 唯一，
    /// 跨测试不互相 dedup（因为 OnceLock 全局 dedup 表无法 reset）。
    fn make_proposal(id_prefix: &str) -> EvolutionProposal {
        EvolutionProposal {
            proposal_id: format!("{id_prefix}-test"),
            created_at_ms: 1_700_000_000_000,
            origin: ProposalOrigin::ConsolidationReflection,
            category: ProposalCategory::MemoryHint,
            target: ProposalTarget::MemoryPolicy {
                policy: "test".into(),
            },
            impact: ImpactLevel::Low,
            evidence: Evidence {
                summary: format!("test summary {id_prefix}"),
                occurrence_count: 1,
                window_hours: 24,
                related_refs: vec![],
            },
            suggestion: Suggestion {
                text: "test suggestion".into(),
                structured_patch: None,
            },
        }
    }

    /// 当前 dedup 表大小（仅测试可见）。
    fn emitted_count() -> usize {
        emitted_map()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// 当前 dedup 表是否含某 id（仅测试可见）。
    fn emitted_contains(id: &str) -> bool {
        emitted_map()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(id)
    }

    #[test]
    fn emit_empty_vec_is_noop() {
        let _serial = EMIT_TEST_LOCK.lock().unwrap_or_else(|e| { eprintln!("[mutex_poisoned] evolution::emit::EMIT_TEST_LOCK: {e:?}"); e.into_inner() });
        let before = emitted_count();
        let report = emit_proposals(vec![]);
        assert_eq!(report, EmitReport::default());
        assert_eq!(emitted_count(), before, "空 vec 不应动 dedup 表");
    }

    #[test]
    fn emit_first_time_writes_to_dedup() {
        let _serial = EMIT_TEST_LOCK.lock().unwrap_or_else(|e| { eprintln!("[mutex_poisoned] evolution::emit::EMIT_TEST_LOCK: {e:?}"); e.into_inner() });
        // 用 nanos 戳确保 id 唯一（OnceLock 表无法 reset）
        let unique = format!(
            "emit_first_{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let p = make_proposal(&unique);
        let before = emitted_count();
        let report = emit_proposals(vec![p.clone()]);
        assert_eq!(report.written, 1, "首次 emit 应计入 written");
        assert_eq!(report.deduped, 0);
        assert_eq!(emitted_count(), before + 1, "dedup 表新增 1 条");
        assert!(
            emitted_contains(&p.proposal_id),
            "dedup 表应含此 proposal_id"
        );
    }

    #[test]
    fn emit_deduplicates_within_window() {
        let _serial = EMIT_TEST_LOCK.lock().unwrap_or_else(|e| { eprintln!("[mutex_poisoned] evolution::emit::EMIT_TEST_LOCK: {e:?}"); e.into_inner() });
        let unique = format!(
            "emit_dedup_{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let p1 = make_proposal(&unique);
        let p2 = make_proposal(&unique); // 同 id
        let before = emitted_count();
        let r1 = emit_proposals(vec![p1]);
        assert_eq!(r1.written, 1);
        let after_first = emitted_count();
        let r2 = emit_proposals(vec![p2]);
        assert_eq!(r2.written, 0, "同 id 第二次 emit 不应再写入 dedup");
        assert_eq!(r2.deduped, 1);
        assert_eq!(
            emitted_count(),
            after_first,
            "dedup 表大小不应变（首次已记录）"
        );
        assert!(after_first > before);
    }

    #[test]
    fn emit_dedup_resets_after_24h() {
        let _serial = EMIT_TEST_LOCK.lock().unwrap_or_else(|e| { eprintln!("[mutex_poisoned] evolution::emit::EMIT_TEST_LOCK: {e:?}"); e.into_inner() });
        // 直接操作 dedup 表的 TTL 清理路径：插入一个 25h 之前的 id，
        // 下次 emit 应能再次写入。
        let unique = format!(
            "emit_ttl_{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let p = make_proposal(&unique);
        // 手工塞一个过期 id 进表
        let stale_ts = chrono::Utc::now().timestamp_millis() - (25 * 3_600_000);
        emitted_map()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(p.proposal_id.clone(), stale_ts);
        // 同 id emit：触发 retain 清理后再写
        let r = emit_proposals(vec![p.clone()]);
        assert_eq!(r.written, 1, "25h 前的同 id 应被 TTL 清理并重新写入");
        assert_eq!(r.deduped, 0);
    }

    #[test]
    fn emit_mixed_dedup_and_new() {
        let _serial = EMIT_TEST_LOCK.lock().unwrap_or_else(|e| { eprintln!("[mutex_poisoned] evolution::emit::EMIT_TEST_LOCK: {e:?}"); e.into_inner() });
        let unique_new = format!(
            "emit_mixed_new_{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let unique_old = format!(
            "emit_mixed_old_{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        // 先 emit 一条 unique_old
        let _ = emit_proposals(vec![make_proposal(&unique_old)]);
        // 然后 mixed emit：new + old
        let r = emit_proposals(vec![make_proposal(&unique_new), make_proposal(&unique_old)]);
        assert_eq!(r.written, 1, "unique_new 应被写入");
        assert_eq!(r.deduped, 1, "unique_old 应被去重");
    }

    #[test]
    fn emit_dedup_works_without_app_handle() {
        let _serial = EMIT_TEST_LOCK.lock().unwrap_or_else(|e| { eprintln!("[mutex_poisoned] evolution::emit::EMIT_TEST_LOCK: {e:?}"); e.into_inner() });
        // AppHandle 未注册时：dedup 仍正常执行；audit 被跳过但不 panic
        let unique = format!(
            "emit_no_app_{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let p = make_proposal(&unique);
        let r1 = emit_proposals(vec![p.clone()]);
        // AppHandle 未注册 → eprintln 路径；written 仍 +1（计入 dedup）
        assert_eq!(r1.written, 1, "无 AppHandle 时也应计入 dedup 表");
        assert!(emitted_contains(&p.proposal_id));
        let r2 = emit_proposals(vec![p]);
        assert_eq!(r2.written, 0);
        assert_eq!(r2.deduped, 1);
    }
}
