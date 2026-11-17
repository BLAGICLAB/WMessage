//! 证据来源层：从消息里抽文本、来源与数字是否可溯源。
//!
//! 本文件由 bot_model_loop.rs 拆分而来（纯搬移，行为零变化）。

/// 证据池里单次调用保留的返回文本上限（字符）。
///
/// 工具返回本身已被回灌漏斗钳过（全局 128KB、per-tool 另有更小的预算层，
/// 见 `bot_model_loop::output`），这里再钳一层是防极端值把
/// 内存吃光。截断时头尾各留一半（见 [`evidence_output`]）。
pub(crate) const EVIDENCE_OUTPUT_CHARS: usize = crate::bot::constants::EVIDENCE_OUTPUT_CHARS;

/// 证据池里单次调用的返回文本：超上限时头尾各留一半（字符口径）。
///
/// 头尾各半不是审美选择，是与回灌漏斗对齐：漏斗给模型看的是「头 + 指针
/// + 尾」（见 `bot_model_loop::output` 的 `preview_with_pointer` /
/// `cap_tool_output`），证据池若只留头部，模型引用尾半段里的数就会被判
/// 「无出处」——真话被误伤，而返工指令还会诱导它重跑已成功的工具
/// （副作用翻倍）。头尾都留，模型看得见的就都算得出处。
pub(crate) fn evidence_output(text: &str) -> String {
    let total = text.chars().count();
    if total <= EVIDENCE_OUTPUT_CHARS {
        return text.to_string();
    }
    let half = EVIDENCE_OUTPUT_CHARS / 2;
    let head: String = text.chars().take(half).collect();
    let tail: String = text.chars().skip(total - half).collect();
    format!(
        "{head}\n…（中间 {} 字符未入证据池）…\n{tail}",
        total - EVIDENCE_OUTPUT_CHARS
    )
}

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

/// 系统注入提示的前缀（全仓统一：守卫返工提示、步数软警告、失败换招提示
/// 都用它）。这类消息是系统对模型说的话，不是用户说的——计入出处就等于
/// 让系统提示给自己作证：守卫判「无出处」时注入的返工提示里带着被判的
/// 那个数，下一轮模型照着复述就把自己洗白了，「返工后仍编造必挂更正」
/// 的第二道防线对它静默失效。
pub(crate) const SYSTEM_NOTICE_PREFIX: &str = "【系统提示】";

/// 对话里模型见过的「非模型自产」内容：用户消息 + 工具消息。
///
/// assistant 消息不算出处——模型自己上一轮说过的数字不是证据，它说的话不能
/// 给自己作证。跨提问的旧工具消息算：那是上一轮真的跑出来的。
/// 系统注入提示（[`SYSTEM_NOTICE_PREFIX`]）也不算出处，理由见该常量。
pub(crate) fn context_sources(msgs: &[serde_json::Value]) -> Vec<String> {
    msgs.iter()
        .filter(|m| {
            matches!(
                m.get("role").and_then(|r| r.as_str()),
                Some("user") | Some("tool")
            )
        })
        .filter_map(msg_text)
        .filter(|t| !t.starts_with(SYSTEM_NOTICE_PREFIX))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 证据池超上限时头尾都留：漏斗给模型看的是「头 + 指针 + 尾」，只留头部
    /// 的话模型引用尾半段的数会被判「无出处」（真话误伤）
    #[test]
    fn evidence_output_keeps_both_ends() {
        let text = format!("HEAD{}TAIL", "x".repeat(EVIDENCE_OUTPUT_CHARS));
        let out = evidence_output(&text);
        assert!(out.starts_with("HEAD"), "头部必须保留");
        assert!(out.ends_with("TAIL"), "尾部必须保留");
        assert!(
            out.chars().count() < EVIDENCE_OUTPUT_CHARS + 64,
            "总体量仍受上限约束：{}",
            out.chars().count()
        );
    }

    /// 未超上限时原样保留（不插截断说明）
    #[test]
    fn evidence_output_keeps_short_text_verbatim() {
        let text = "耗时 1234.56 ms".to_string();
        assert_eq!(evidence_output(&text), text);
    }

    /// 尾半段里的数仍算得出处（模型看得见的就算数）
    #[test]
    fn number_in_kept_tail_counts_as_sourced() {
        let text = format!("{} 末尾结论：99887766", "x".repeat(EVIDENCE_OUTPUT_CHARS));
        let out = evidence_output(&text);
        let sources = vec![out];
        assert!(
            number_sourced("99887766", &sources),
            "模型看得见的尾半段里的数必须有出处，否则真话被判编造"
        );
    }
}
