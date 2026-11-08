//! ask 档分类器兜底（灰度）。
//!
//! 触发条件四条全满足才实际调模型：perm_mode==Ask（调用点判）+
//! interactive（调用点判）+ 灰度开关 `autoGateClassifier`（bot-config，默认
//! false=行为零变化）+ selfcheckModelId 可解析且厂商已验证。
//!
//! 三态判决：allow = 免弹窗直接放行；deny = 直接拒（人话理由，模型可换路）；
//! unsure（模型不可用 / 超时 / 输出不可解析 / 连拦达限）= 落回原弹窗流程
//! ——**fail-safe：任何异常都绝不变成放行**。
//!
//! 模型槽位复用 selfcheckModelId（终稿自检同一解析链：resolve_model_override
//! + 厂商验证）；请求形态 = 单条消息 + max_tokens 8 + temperature 0 + 8s 超时
//! （对齐 Claude auto mode 的「分类器审动作」层，连拦 3 次退回人工）。
//! 判定调用不进 llm.usage 记账（侧调口径），审计 `auto_gate.classifier`。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use tauri::AppHandle;

/// 分类器请求超时（秒）：动作判定必须快，弹窗兜底在 60s 等人
const CLASSIFIER_TIMEOUT_SECS: u64 = 8;
/// 判定输出预算（首词判定足够）
const CLASSIFIER_MAX_TOKENS: u32 = 8;
/// 连续 deny 达限后本会话停用分类器（回归全弹窗），对齐 Claude auto mode
const DENY_STREAK_LIMIT: usize = 3;

/// 会话级连拦计数（进程级 store）
fn deny_streaks() -> &'static Mutex<HashMap<String, usize>> {
    static MAP: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 分组挂载同款进程级开关：bot-config 的 `autoGateClassifier`，load_config /
/// update_config_file 两处同步（tools_json_for 无 app 参数是同款约束）
static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn enabled() -> bool {
    ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_enabled(v: bool) {
    ENABLED.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// 从配置同步开关（io.rs 两处调用）
pub fn sync_from_cfg(cfg: &crate::bot::config::types::BotConfig) {
    set_enabled(cfg.auto_gate_classifier.unwrap_or(false));
}

/// 三态判决
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// 免弹窗直接放行
    Allow,
    /// 直接拒（人话理由给模型）
    Deny,
    /// 分类器没给出可判定答案——落回原弹窗流程（fail-safe）
    Unsure,
}

/// 判定提示词：只做「用户本回合的意图是否覆盖这个操作」的判别，输出单词
const CLASSIFIER_SYSTEM: &str = "你是权限闸的动作分类器。根据用户本轮诉求判断机器人当前要执行的这个文件操作是否属于该诉求的合理组成部分。规则：用户明确要求或直接服务于用户目标的读写 → allow；超出诉求范围、敏感路径（系统目录/配置文件/凭据类）、用户诉求含糊不清 → deny。只回答一个词：allow 或 deny。";

/// 单字段配置快照（resolve 后的模型连接参数）
struct ClassifierModel {
    base_url: String,
    api_key: String,
    model: String,
    provider: crate::bot::ApiProvider,
}

/// 解析分类器模型槽位（复用 selfcheckModelId + 厂商验证链）；失败 → None（unsure）
fn resolve_classifier_model(cfg: &crate::bot::config::types::BotConfig) -> Option<ClassifierModel> {
    let entry_id = cfg.selfcheck_model_id.as_ref()?;
    let resolved =
        crate::bot::resolve_model_override(cfg.models_by_provider.as_ref(), entry_id, None).ok()?;
    crate::bot::ensure_vendor_verified(resolved.vendor.as_deref(), &cfg.verified_vendors).ok()?;
    Some(ClassifierModel {
        base_url: resolved.base_url,
        api_key: resolved.api_key,
        model: resolved.model,
        provider: crate::bot::ApiProvider::from_cfg(Some(resolved.api_provider.as_str())),
    })
}

/// 会话最近一条真人用户消息（意图来源）：闸点拿不到消息栈，从真源查。
/// 取头 400 字（注入块拼在原话前面，content 尾部才是原话——取尾段不可靠，
/// 取整条前 400 字已够判别）。
fn recent_user_intent(conn: &rusqlite::Connection, session_id: &str) -> Option<String> {
    let raw: String = conn
        .query_row(
            "SELECT content FROM bot_messages
             WHERE session_id = ?1 AND role = 'user'
             ORDER BY ord DESC LIMIT 1",
            [session_id],
            |r| r.get(0),
        )
        .ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(400).collect())
}

/// 前置短路（纯逻辑可测）：开关关或连拦达限 → Some(Unsure)（调用点走原弹窗）；
/// None = 继续真实判定。auto_gate_decide 开头调用。
pub(crate) fn short_circuit(session_id: Option<&str>) -> Option<Decision> {
    if !enabled() {
        return Some(Decision::Unsure);
    }
    // 连拦达限 → 本会话停用（回归全弹窗）
    let sid = session_id?;
    let streak = *deny_streaks()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(sid)
        .unwrap_or(&0);
    (streak >= DENY_STREAK_LIMIT).then_some(Decision::Unsure)
}

/// 分类器输出 → 三态判决（纯函数便于单测）：首词 allow/deny 命中，其余
/// （含模型胡说）→ unsure。deny 分支必须有生产行直测——fail-safe 的牙齿
/// 就在「deny 落 Deny 而非放行」这一行映射上。
fn parse_verdict(verdict: Option<&str>) -> Decision {
    match verdict.map(|s| s.trim()) {
        Some(v) if v.starts_with("allow") => Decision::Allow,
        Some(v) if v.starts_with("deny") => Decision::Deny,
        _ => Decision::Unsure,
    }
}

/// 判决入口。调用点已保证 perm_mode==Auto && interactive；这里只判开关 /
/// 连拦 / 模型可用，然后一次小请求出三态。
pub async fn auto_gate_decide(
    app: &AppHandle,
    session_id: Option<&str>,
    tool: &str,
    detail: &str,
) -> Decision {
    if let Some(d) = short_circuit(session_id) {
        return d;
    }
    // 模型槽位与意图：任一不可得 → unsure（不吞错：审计留痕）
    let cfg = crate::bot::config::io::load_config(app);
    let Some(model) = resolve_classifier_model(&cfg) else {
        audit_decision(app, tool, "unsure_no_model", 0);
        return Decision::Unsure;
    };
    let intent = session_id.and_then(|sid| {
        let conn = crate::db::open_db(app).ok()?;
        recent_user_intent(&conn, sid)
    });
    let Some(intent) = intent else {
        audit_decision(app, tool, "unsure_no_intent", 0);
        return Decision::Unsure;
    };

    let started = Instant::now();
    let verdict = classify(&model, tool, detail, &intent).await;
    let latency_ms = started.elapsed().as_millis() as u64;
    let decision = parse_verdict(verdict.as_deref());
    match (decision, session_id) {
        (Decision::Deny, Some(sid)) => {
            let mut map = deny_streaks().lock().unwrap_or_else(|p| p.into_inner());
            let next = map.entry(sid.to_string()).or_insert(0);
            *next += 1;
            if *next == DENY_STREAK_LIMIT {
                crate::audit_event!(
                    app,
                    crate::audit::AuditLevel::Warn,
                    "auto_gate.classifier_disabled",
                    "session" => crate::bot::truncate_for_log(sid, 64),
                    "limit" => DENY_STREAK_LIMIT,
                );
            }
        }
        (Decision::Allow, Some(sid)) => {
            deny_streaks()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(sid);
        }
        _ => {}
    }
    crate::audit::write_event(
        app,
        crate::audit::AuditLevel::Info,
        "auto_gate.classifier",
        &[
            ("decision", format!("{decision:?}")),
            ("tool", tool.to_string()),
            ("latency_ms", latency_ms.to_string()),
        ],
    );
    decision
}

/// 判定请求：单条 user 消息（工具 + 目标 + 用户意图），双协议，8s 超时，
/// max_tokens 8、temperature 0。失败 → None（unsure）。判定不进 llm.usage。
async fn classify(
    model: &ClassifierModel,
    tool: &str,
    detail: &str,
    intent: &str,
) -> Option<String> {
    let user_msg = format!("机器人要执行的操作：工具 {tool}，{detail}\n用户本轮诉求：{intent}");
    let msgs = vec![serde_json::json!({"role": "user", "content": user_msg})];
    let (url, body) = match model.provider {
        crate::bot::ApiProvider::Openai => (
            format!("{}/chat/completions", model.base_url.trim_end_matches('/')),
            serde_json::json!({
                "model": model.model,
                "messages": [
                    {"role": "system", "content": CLASSIFIER_SYSTEM},
                    {"role": "user", "content": user_msg}
                ],
                "max_tokens": CLASSIFIER_MAX_TOKENS,
                "temperature": 0.0,
                "stream": false
            }),
        ),
        crate::bot::ApiProvider::Anthropic => {
            let body = crate::bot_anthropic::build_anthropic_body(
                &model.model,
                &msgs,
                &serde_json::json!([]),
                CLASSIFIER_MAX_TOKENS,
                false,
            )
            .ok()?;
            (
                crate::bot_anthropic::anthropic_messages_url(&model.base_url),
                body,
            )
        }
    };
    let req = crate::bot_model_loop::shared_llm_client()
        .post(&url)
        .timeout(std::time::Duration::from_secs(CLASSIFIER_TIMEOUT_SECS))
        .json(&body);
    let req = match model.provider {
        crate::bot::ApiProvider::Openai => req.bearer_auth(model.api_key.trim()),
        crate::bot::ApiProvider::Anthropic => {
            crate::bot_anthropic::apply_anthropic_auth(req, model.api_key.trim())
        }
    };
    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: serde_json::Value = resp.json().await.ok()?;
    // 双协议取正文：openai choices[0].message.content / anthropic content[0].text
    let text = v
        .pointer("/choices/0/message/content")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            v.get("content")
                .and_then(|c| c.as_array())
                .and_then(|a| a.first())
                .and_then(|b| b.get("text"))
                .and_then(|t| t.as_str())
                .map(|s| s.to_string())
        })?;
    Some(text.to_lowercase())
}

/// 判定审计（无敏感面：只有 decision/tool/latency）
fn audit_decision(app: &AppHandle, tool: &str, reason: &str, latency_ms: u64) {
    crate::audit::write_event(
        app,
        crate::audit::AuditLevel::Info,
        "auto_gate.classifier",
        &[
            ("decision", reason.to_string()),
            ("tool", tool.to_string()),
            ("latency_ms", latency_ms.to_string()),
        ],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset() {
        set_enabled(false);
        deny_streaks()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }

    /// 两条短路路径合并单测串行跑：共用进程级 ENABLED / deny_streaks 全局态，
    /// 拆成两个 #[test] 会并行互踩——开关窗口期把对方的 unsure 短路断言踩成 None。
    #[test]
    fn short_circuit_paths() {
        // 开关关 → 短路 unsure（调用点走原弹窗，行为零变化）
        reset();
        assert_eq!(short_circuit(Some("s1")), Some(Decision::Unsure));
        assert_eq!(short_circuit(None), Some(Decision::Unsure));

        // 连拦计数状态机：deny 累计到限后短路停用；清零后恢复判定
        set_enabled(true);
        let sid = "s-streak";
        assert_eq!(short_circuit(Some(sid)), None, "未达限继续判定");
        for i in 1..=DENY_STREAK_LIMIT {
            deny_streaks()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(sid.into(), i);
        }
        assert_eq!(
            short_circuit(Some(sid)),
            Some(Decision::Unsure),
            "达限停用（回归全弹窗）"
        );
        // allow 清零 → 恢复
        deny_streaks()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(sid);
        assert_eq!(short_circuit(Some(sid)), None);
        reset();
    }

    /// 判定解析（生产行直测）：deny 必须落 Deny——deny 被改成放行时本测试
    /// 必须红，fail-safe 假设才有自动化闸
    #[test]
    fn verdict_parsing() {
        assert_eq!(parse_verdict(Some("allow")), Decision::Allow);
        assert_eq!(parse_verdict(Some("deny")), Decision::Deny);
        assert_eq!(
            parse_verdict(Some("ALLOW")),
            Decision::Unsure,
            "小写化后才比对"
        );
        assert_eq!(parse_verdict(Some("maybe")), Decision::Unsure);
        assert_eq!(parse_verdict(None), Decision::Unsure);
        // 小写化入口在 classify 返回侧：to_lowercase 后 ALLOW→allow
        assert_eq!(
            parse_verdict(Some("ALLOW".to_lowercase().as_str())),
            Decision::Allow
        );
    }
}
