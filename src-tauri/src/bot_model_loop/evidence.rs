//! 证据来源层：从消息里抽文本、来源与数字是否可溯源。
//!
//! 本文件由 bot_model_loop.rs 拆分而来（纯搬移，行为零变化）。

/// 证据池里单次调用保留的返回文本上限（字符）。
///
/// 工具返回本身已被回灌漏斗钳过（全局 128KB、per-tool 另有更小的预算层，
/// 见 `bot_model_loop::output`），这里再钳一层是防极端值把
/// 内存吃光。截掉的部分模型多半也没看见——真截在中间的数判成「无出处」是
/// 误伤方向，宁可放过。
pub(crate) const EVIDENCE_OUTPUT_CHARS: usize = crate::bot::constants::EVIDENCE_OUTPUT_CHARS;

/// 一次工具调用的证据：参数与返回。
///
/// 凭证池记的是「哪个工具成功过」——存在性。要判「模型报出来的这个数是哪来的」
/// 「这次调用到底替模型核实了什么」，得留着返回内容与参数本身：那是另一根轴。
#[derive(Debug, Clone)]
pub(crate) struct ToolEvidence {
    /// 这次调用成功没有（失败与成功的返回都是模型看得见的内容，出处比对都要算）
    pub(crate) ok: bool,
    /// 调用的参数（原始 JSON 串）
    pub(crate) args: String,
    /// 工具返回的文本（模型看到的那一份）
    pub(crate) output: String,
}

/// 消息里的文本内容（字符串，或多模态数组里的 text 段）。
fn msg_text(msg: &serde_json::Value) -> Option<String> {
    match msg.get("content") {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Array(parts)) => {
            let mut buf = String::new();
            for p in parts {
                if let Some(t) = p.get("text").and_then(|t| t.as_str()) {
                    buf.push_str(t);
                    buf.push('\n');
                }
            }
            if buf.is_empty() {
                None
            } else {
                Some(buf)
            }
        }
        _ => None,
    }
}

/// 对话里模型见过的「非模型自产」内容：用户消息 + 工具消息。
///
/// assistant 消息不算出处——模型自己上一轮说过的数字不是证据，它说的话不能
/// 给自己作证。跨提问的旧工具消息算：那是上一轮真的跑出来的。
pub(crate) fn context_sources(msgs: &[serde_json::Value]) -> Vec<String> {
    msgs.iter()
        .filter(|m| {
            matches!(
                m.get("role").and_then(|r| r.as_str()),
                Some("user") | Some("tool")
            )
        })
        .filter_map(msg_text)
        .collect()
}

/// 数值在出处里找得到吗。
///
/// 三条路算「找得到」：字面命中（去千分位逗号后子串命中）、四舍五入等价
/// （工具给 1234.56、模型报 1235）、以及出处里本来就有这个数。
/// 四舍五入那条例外是给「工具给的是原值、模型报的是它自己抄下来的近似值」
/// 留的——那种是如实转述，判成编造就是误伤。
pub(crate) fn number_sourced(value: &str, sources: &[String]) -> bool {
    let needle: String = value.chars().filter(|c| *c != ',').collect();
    if needle.is_empty() {
        return true;
    }
    let decimals = needle.split_once('.').map(|(_, d)| d.len()).unwrap_or(0);
    // 解析不过的不是数值，按「比不出来」处理（当前只有数字会走到这儿，
    // 但别让以后的形态悄悄进来当成有出处）。
    if needle.parse::<f64>().is_err() {
        return false;
    }
    for s in sources {
        if s.replace(',', "").contains(&needle) {
            return true;
        }
    }
    for s in sources {
        for n in crate::bot::registry::numbers_in_text(s) {
            if format!("{n:.decimals$}") == needle {
                return true;
            }
        }
    }
    false
}
