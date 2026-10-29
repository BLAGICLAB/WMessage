//! 历史摘要面：截断点选择、摘要请求、压缩。

use super::*;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{LazyLock, Mutex};

/// 截断点计算（纯函数）：返回保留起点下标（=丢弃条数）。最旧的先丢，最后一条永远保留。
/// 剥出供截断即摘要复用——摘要路径需要「将被丢弃的消息」而不只是丢弃条数。
///
/// **滞回**：超预算时不是贴着预算边裁，而是裁到 `target_ratio × budget`
/// （默认 70%），留出余量。贴着边裁的后果是每轮可能只丢一两条——历史体量
/// 缓涨时几乎每轮都触发一次裁剪，而每次裁剪都会改掉请求数组的缓存前缀，
/// 把「长会话只付一次全价」打碎成「每轮都付全价」。留 30% 余量能稳住
/// 好几轮的前缀，代价只是多裁一点历史。
///
/// `target_ratio` 钳在 0.5..=0.9：低于 0.5 一次裁太狠（历史断崖式变短），
/// 高于 0.9 等于没有余量。
pub(super) fn truncate_split_point(
    messages: &[ChatMsg],
    budget: usize,
    target_ratio: f64,
) -> usize {
    // 没超预算就不动（历史一条不丢）
    let total_all: usize = messages.iter().map(|m| m.content.chars().count()).sum();
    if total_all <= budget {
        return 0;
    }
    let target = (budget as f64 * target_ratio.clamp(0.5, 0.9)) as usize;
    let mut total = 0usize;
    let mut keep_from = messages.len();
    for (i, m) in messages.iter().enumerate().rev() {
        let n = m.content.chars().count();
        if total + n > target && i != messages.len() - 1 {
            break;
        }
        total += n;
        keep_from = i;
    }
    keep_from
}

/// 截断即摘要编排内核（注入摘要器，mock LLM 测试可全链路驱动）：
/// 超预算时对「将被丢弃的消息」调一次摘要；返回 (保留的消息, 摘要, 丢弃条数)。
/// 摘要失败/为空 → summary=None，调用方静默退回直接丢弃的原行为。
/// pub：tests/llm_integration.rs 直用（与 bot::run_model_loop_core 同先例）。
pub async fn truncate_with_summary_core<S, Fut>(
    messages: Vec<ChatMsg>,
    budget: usize,
    target_ratio: f64,
    summarize: S,
) -> (Vec<ChatMsg>, Option<String>, usize)
where
    S: FnOnce(Vec<ChatMsg>) -> Fut,
    Fut: std::future::Future<Output = CommandResult<String>>,
{
    let keep_from = truncate_split_point(&messages, budget, target_ratio);
    if keep_from == 0 {
        return (messages, None, 0);
    }
    let dropped_msgs: Vec<ChatMsg> = messages[..keep_from].to_vec();
    let summary = summarize(dropped_msgs)
        .await
        .ok()
        .filter(|s| !s.trim().is_empty());
    (
        messages.into_iter().skip(keep_from).collect(),
        summary,
        keep_from,
    )
}

/// 截断摘要的进程内复用水位：session → (dropped 集合指纹, 摘要正文)。
///
/// 历史每轮都从 DB 全量重载（截断不落库），split 点稳定时 dropped 集合
/// 逐轮完全相同——没有这层水位，截断后的每一轮都会对着同一段消息重付
/// 一次摘要 LLM 调用（该请求走独立通道不进 llm.usage，是观测盲区里的
/// 真实开销）。指纹相同 ⇒ dropped 集合逐字节相同 ⇒ 旧摘要仍然完整覆盖。
/// 每会话一条，条数超限整体重置（丢了的会话下轮重摘要一次，正确性不受影响）。
static SUMMARY_REUSE: LazyLock<Mutex<HashMap<String, (u64, String)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(super) fn dropped_fingerprint(dropped: &[ChatMsg]) -> u64 {
    let mut h = DefaultHasher::new();
    dropped.len().hash(&mut h);
    for m in dropped {
        m.role.hash(&mut h);
        m.content.hash(&mut h);
    }
    h.finish()
}

pub(super) fn cached_summary(session_id: Option<&str>, dropped: &[ChatMsg]) -> Option<String> {
    let sid = session_id?;
    let fp = dropped_fingerprint(dropped);
    let map = SUMMARY_REUSE.lock().ok()?;
    match map.get(sid) {
        Some((f, s)) if *f == fp => Some(s.clone()),
        _ => None,
    }
}

pub(super) fn store_summary_reuse(session_id: Option<&str>, dropped: &[ChatMsg], summary: &str) {
    let Some(sid) = session_id else {
        return;
    };
    let fp = dropped_fingerprint(dropped);
    if let Ok(mut map) = SUMMARY_REUSE.lock() {
        if map.len() > 128 && !map.contains_key(sid) {
            map.clear();
        }
        map.insert(sid.to_string(), (fp, summary.to_string()));
    }
}

/// 截断即摘要（设计 5.2）生产薄壳：
/// 超预算时先生成摘要并落库，返回 (保留的消息, 带
/// 「[早前对话摘要]」前缀的摘要内容, 丢弃条数)；LLM/落库任何一步失败都退回
/// 直接丢弃，绝不阻塞或弄挂主对话流程。
/// dropped 集合与上一轮相同（指纹命中）→ 复用进程内摘要不再发请求，
/// 并留 `chat.summary_reused` 审计让「省了这次调用」可见。
pub(crate) async fn truncate_chat_history_with_summary(
    app: &AppHandle,
    session_id: Option<&str>,
    messages: Vec<ChatMsg>,
    budget: usize,
    target_ratio: f64,
) -> (Vec<ChatMsg>, Option<String>, usize) {
    let sid = session_id.map(str::to_string);
    let (kept, summary, dropped) =
        truncate_with_summary_core(messages, budget, target_ratio, |dropped_msgs| {
            let sid = sid.clone();
            async move {
                if let Some(s) = cached_summary(sid.as_deref(), &dropped_msgs) {
                    crate::audit_event!(app, crate::audit::AuditLevel::Info, "chat.summary_reused",
                        "session" => sid.as_deref().unwrap_or("-"));
                    return Ok(s);
                }
                summarize_messages(app, SUMMARY_SYSTEM_PROMPT, &dropped_msgs)
                    .await
                    .inspect(|s| store_summary_reuse(sid.as_deref(), &dropped_msgs, s))
            }
        })
        .await;
    if let Some(summary) = &summary {
        persist_summary(app, session_id, summary).await;
    }
    (
        kept,
        summary.map(|s| format!("[早前对话摘要] {s}")),
        dropped,
    )
}

/// 摘要落库（设计 5.2）：全失败兜底——出错只记审计不重试不影响对话
/// （摘要已在本轮历史里，落库丢了下轮截断还会再摘要）。
pub(super) async fn persist_summary(app: &AppHandle, session_id: Option<&str>, summary: &str) {
    if let Err(e) = crate::memory::save_summary(app, session_id, summary).await {
        crate::audit_event!(app, crate::audit::AuditLevel::Warn, "memory.summary_save_failed",
            "error" => e.message());
    }
}

/// 非流式摘要调用内核（从 bot_compact 提炼，连接参数注入——
/// 测试直连 mock LLM，生产薄壳 summarize_messages 从 bot_get_config/read_api_key 取配置）。
/// pub：tests/llm_integration.rs 直用（与 bot::run_model_loop_core 同先例）。
/// Anthropic 兼容模式：provider/max_tokens 注入，按协议分支
/// URL/鉴权头/请求体/响应解析（Anthropic 侧转换走 bot_anthropic 纯函数）。
/// temperature/top_p（ 条目级覆盖）有值才写请求体，两协议顶层字段同名。
pub async fn summarize_http(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system_prompt: &str,
    messages: &[ChatMsg],
    provider: crate::bot::ApiProvider,
    max_tokens: u32,
    temperature: Option<f64>,
    top_p: Option<f64>,
) -> CommandResult<String> {
    let mut msgs: Vec<serde_json::Value> = vec![serde_json::json!({
        "role": "system",
        "content": system_prompt
    })];
    let mut total = 0usize;
    let mut kept: Vec<&ChatMsg> = Vec::new();
    for m in messages.iter().rev() {
        let n = m.content.chars().count();
        if total + n > SUMMARIZE_MAX_CHARS && !kept.is_empty() {
            break;
        }
        total += n;
        kept.push(m);
    }
    for m in kept.iter().rev() {
        msgs.push(serde_json::json!({"role": m.role, "content": m.content}));
    }
    let (url, mut body) = match provider {
        crate::bot::ApiProvider::Openai => (
            format!("{}/chat/completions", base_url.trim_end_matches('/')),
            serde_json::json!({
                "model": model,
                "messages": msgs,
                "stream": false
            }),
        ),
        crate::bot::ApiProvider::Anthropic => {
            let body = crate::bot_anthropic::build_anthropic_body(
                model,
                &msgs,
                &serde_json::json!([]),
                max_tokens,
                false,
            )
            .map_err(|e| format!("摘要消息转换失败：{e}"))?;
            (crate::bot_anthropic::anthropic_messages_url(base_url), body)
        }
    };
    // 条目级采样参数注入：temperature/top_p 有值才写（两协议顶层字段同名）
    crate::bot_model_loop::apply_inference_params(&mut body, temperature, top_p);
    let req = client
        .post(&url)
        // per-request 总超时（共享客户端无默认超时）：摘要单次请求预算不变
        .timeout(std::time::Duration::from_secs(60))
        .json(&body);
    let req = match provider {
        crate::bot::ApiProvider::Openai => req.bearer_auth(api_key.trim()),
        crate::bot::ApiProvider::Anthropic => {
            crate::bot_anthropic::apply_anthropic_auth(req, api_key.trim())
        }
    };
    let resp = req
        .send()
        .await
        .map_err(|e| format!("请求大模型失败：{e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(CommandError::LlmApiError {
            status: status.as_u16(),
            detail: crate::error::llm_error_detail(status.as_u16(), &text, None),
        });
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析响应失败：{e}"))?;
    let text = match provider {
        // 安全访问：choices 可能为空数组/缺失（网关错误对象），索引会 panic
        crate::bot::ApiProvider::Openai => v
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|c| c["message"]["content"].as_str())
            .unwrap_or("")
            .to_string(),
        crate::bot::ApiProvider::Anthropic => crate::bot_anthropic::parse_anthropic_response(&v),
    };
    // 模型带 <think> 段时先剥掉，防摘要带思考段写回历史
    let text = strip_think_blocks(&text).trim().to_string();
    if text.is_empty() {
        return Err(CommandError::DomainRule {
            domain: "llm".to_string(),
            reason: "模型返回了空摘要".to_string(),
        });
    }
    Ok(text)
}

/// 非流式摘要调用（截断即摘要 / /compact / 记忆整理 / 会话抽取共用）：
/// 配置获取路径与 bot_compact 原路径一致（bot_get_config 拿 base_url/model，
/// read_llm_key 拿 key，require_api_key 把空 key 映射为 ApiKeyMissing）。
/// 摘要：跟随全局当前模型。
///
/// 不接受「用另一个模型跑这次摘要」的参数——没有调用方需要。摘要场景全走
/// 当前模型：截断摘要（SUMMARY）、/compact（COMPACT）、记忆整理
/// （consolidate）、会话抽取（extract）、标签归并（task_autotag）。
/// 真要让某个任务走轻量模型，正确位置是模型条目本身（设置页配一个便宜条目
/// 切过去），不是给每次调用加一个没人传的开关。
pub(crate) async fn summarize_messages(
    app: &AppHandle,
    system_prompt: &str,
    messages: &[ChatMsg],
) -> CommandResult<String> {
    let cfg = crate::bot::bot_get_config(app.clone())?;
    let provider = crate::bot::ApiProvider::from_cfg(cfg.api_provider.as_deref());
    let active_id = cfg.active_model_id.clone();
    let base_url = cfg.base_url.clone();
    let model = cfg.model.clone();
    let provider_str = provider.as_str();
    let api_key = crate::bot::read_llm_key(
        Some(provider_str),
        active_id.as_ref(),
        cfg.models_by_provider.as_ref(),
    )?;
    require_api_key(&api_key)?;
    // 共享客户端（连接池复用）；原 60s 总超时改为 per-request 保留
    let client = crate::bot_model_loop::shared_llm_client().clone();
    // 条目级推理参数：max_tokens 条目值覆盖全局（再钳制）；temperature/top_p
    // 条目有值才发。摘要不追加条目 system_prompt（这里的 system_prompt 是固定任务提示词）
    let inference = crate::bot::effective_inference(
        Some(provider_str),
        None,
        active_id.as_ref(),
        cfg.models_by_provider.as_ref(),
    );
    summarize_http(
        &client,
        &base_url,
        &api_key,
        &model,
        system_prompt,
        messages,
        provider,
        inference.max_tokens,
        inference.temperature,
        inference.top_p,
    )
    .await
}
