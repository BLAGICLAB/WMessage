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

/// 消息里的文本内容（字符串，或多模态数组里的非注入 text 段）。
fn msg_text(msg: &serde_json::Value) -> Option<String> {
    use crate::bot_chat::MERGED_INJECTION_HEAD;
    match msg.get("content") {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Array(parts)) => {
            let mut buf = String::new();
            for p in parts {
                if let Some(t) = p.get("text").and_then(|t| t.as_str()) {
                    // 注入块独立成块（多模态形态）：按头锚整块跳过，
                    // 用户原话块原样保留
                    if t.starts_with(MERGED_INJECTION_HEAD) {
                        continue;
                    }
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

/// 整条注入消息的锚点族：这三族消息**整条**都不算出处。
/// 【自检】/【终稿自检】携带的 snippet 是模型自己的终稿/工具返回摘要——
/// 模型自产内容借 user 角色回流出处池，就是给自己作证的洗白通道。
pub(crate) const INJECTED_MSG_PREFIXES: &[&str] =
    &[SYSTEM_NOTICE_PREFIX, "【自检】", "【终稿自检】"];

/// 一条消息文本里真正可作为出处的部分。
///
/// - 以注入锚开头的整条消息（系统提示/自检族）→ None；
/// - 带「系统背景」头锚的并进块（摘要/记忆，见 bot_chat::inject）→ 只留
///   边界锚之后的用户原话——原话是真人说的算出处，注入段是模型自产不算；
/// - 普通文本 → 原样。
fn usable_source_text(text: &str) -> Option<String> {
    use crate::bot_chat::{MERGED_INJECTION_BOUNDARY, MERGED_INJECTION_HEAD};
    if INJECTED_MSG_PREFIXES.iter().any(|p| text.starts_with(p)) {
        return None;
    }
    if text.starts_with(MERGED_INJECTION_HEAD) {
        return match text.rfind(MERGED_INJECTION_BOUNDARY) {
            Some(i) => {
                let user_text = text[i + MERGED_INJECTION_BOUNDARY.len()..].trim();
                if user_text.is_empty() {
                    None
                } else {
                    Some(user_text.to_string())
                }
            }
            // 有头锚无边界（理论不发生）：宁缺勿滥，整条不算出处
            None => None,
        };
    }
    Some(text.to_string())
}

/// 对话里模型见过的「非模型自产」内容：用户消息 + 工具消息。
///
/// assistant 消息不算出处——模型自己上一轮说过的数字不是证据，它说的话不能
/// 给自己作证。跨提问的旧工具消息算：那是上一轮真的跑出来的。
/// 系统注入提示（[`INJECTED_MSG_PREFIXES`] 全族）与并进用户消息的背景块
/// （[`crate::bot_chat::MERGED_INJECTION_HEAD`]）也不算出处，理由同上。
pub(crate) fn context_sources(msgs: &[serde_json::Value]) -> Vec<String> {
    msgs.iter()
        .filter(|m| {
            matches!(
                m.get("role").and_then(|r| r.as_str()),
                Some("user") | Some("tool")
            )
        })
        .filter_map(msg_text)
        .filter_map(|t| usable_source_text(&t))
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
        if number_occurs_with_boundary(&s.replace(',', ""), &needle) {
            return true;
        }
    }
    for s in sources {
        for n in crate::bot::registry::numbers_in_text(s) {
            // 舍入等价只给「工具给了小数、模型报整数/近似值」的如实转述
            // （工具给 1234.56、模型报 1235）。两侧都是整数时「四舍五入」
            // 等于 ±1 整数放水——工具说 5049 模型报 5050 也算有出处，不认。
            if n.fract() == 0.0 && decimals == 0 {
                continue;
            }
            if format!("{n:.decimals$}") == needle {
                return true;
            }
        }
    }
    false
}

/// 子串命中但要求两侧不贴数字：出处里的 150500 不是「输出 5050」的出处。
/// 前侧连带堵小数点（出处 3.5050 的分数尾不是 5050）；后侧只堵数字——
/// 「输出 5050.」的句点不是小数续接。
fn number_occurs_with_boundary(hay: &str, needle: &str) -> bool {
    let h: Vec<char> = hay.chars().collect();
    let n: Vec<char> = needle.chars().collect();
    if n.is_empty() || n.len() > h.len() {
        return false;
    }
    for i in 0..=(h.len() - n.len()) {
        if h[i..i + n.len()] == n[..] {
            let before_free = i == 0 || !(h[i - 1].is_ascii_digit() || h[i - 1] == '.');
            let after_free = i + n.len() == h.len() || !h[i + n.len()].is_ascii_digit();
            if before_free && after_free {
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

    /// 整条注入消息（系统提示/自检族）不进出处池：模型终稿、工具返回摘要
    /// 都是模型自产内容——借 user 角色回流出处池就是洗白通道。
    #[test]
    fn context_sources_excludes_injected_notice_messages() {
        let msgs = vec![
            serde_json::json!({"role": "user", "content": "真话：报价 1000"}),
            serde_json::json!({"role": "assistant", "content": "编造 7777"}),
            serde_json::json!({"role": "user", "content": "【系统提示】已跑完，输出 7777"}),
            serde_json::json!({"role": "user", "content": "【终稿自检】本轮回复（开头 500 字）：报价 7777。仅回答：Y=无幻觉"}),
            serde_json::json!({"role": "user", "content": "【自检】上一条工具调用 create_task 返回成功"}),
            serde_json::json!({"role": "tool", "content": "工具真跑过：1234"}),
        ];
        let sources = context_sources(&msgs);
        let joined = sources.join("\n---\n");
        assert!(
            sources.iter().any(|s| s.contains("报价 1000")),
            "用户原话在"
        );
        assert!(sources.iter().any(|s| s.contains("1234")), "tool 消息在");
        assert!(
            !joined.contains("7777"),
            "三族注入消息整条不得进出处池：{joined}"
        );
    }

    /// 并进用户消息的背景块（摘要/记忆）剥掉、用户原话保留：
    /// 摘要含历史 assistant 终稿、记忆块含模型自写事实——都不得自证。
    #[test]
    fn context_sources_strips_merged_injection_keeps_user_text() {
        use crate::bot_chat::{MERGED_INJECTION_BOUNDARY, MERGED_INJECTION_HEAD};
        let merged = format!(
            "{MERGED_INJECTION_HEAD}\n[早前对话摘要] 上一轮报价 7777\n\n### 相关记忆\n- [fact]报价 7777{MERGED_INJECTION_BOUNDARY}现在改报价 1000"
        );
        let msgs = vec![serde_json::json!({"role": "user", "content": merged})];
        let sources = context_sources(&msgs);
        assert_eq!(sources.len(), 1);
        assert!(sources[0].contains("现在改报价 1000"), "用户原话保留");
        assert!(
            !sources[0].contains("7777"),
            "摘要与记忆里的模型自产数不得进出处池：{}",
            sources[0]
        );
    }

    /// 多模态消息：注入块独立成块 → 整块跳过，用户原话块保留。
    #[test]
    fn context_sources_skips_injected_block_in_multimodal() {
        use crate::bot_chat::MERGED_INJECTION_HEAD;
        let msgs = vec![serde_json::json!({"role": "user", "content": [
            {"type": "text", "text": format!("{MERGED_INJECTION_HEAD}\n摘要里编 7777")},
            {"type": "text", "text": "用户原话：只认 5555"},
            {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAA"}}
        ]})];
        let sources = context_sources(&msgs);
        assert_eq!(sources.len(), 1);
        assert!(sources[0].contains("只认 5555"));
        assert!(!sources[0].contains("7777"), "注入块整块剥离");
    }

    /// 字面命中要求两侧不贴数字：出处里的 150500 不是「输出 5050」的出处。
    #[test]
    fn literal_substring_inside_larger_number_is_not_sourced() {
        let sources = vec!["累计 150500 行".to_string()];
        assert!(
            !number_sourced("5050", &sources),
            "大数的子串不得当出处（编造 5050 命中 150500 即洗白）"
        );
        assert!(number_sourced("150500", &sources), "精确值照常命中");
        // 句点收尾是标点不是小数续接
        assert!(number_sourced("5050", &["输出 5050.".to_string()]));
        // 小数点前置堵死：3.5050 的分数尾不是 5050
        assert!(!number_sourced("5050", &["测得 3.5050".to_string()]));
    }

    /// 舍入等价只给「工具给小数、模型报整数」的如实转述：
    /// 整数对整数的 ±0.5 窗口是放水（工具说 5049 模型报 5050 也算有出处）。
    #[test]
    fn rounding_equivalence_needs_fractional_source_or_decimals() {
        // 正例保留：工具给 1234.56、模型报 1235
        assert!(number_sourced("1235", &["耗时 1234.56 ms".to_string()]));
        // 工具给 5049.6、模型报 5050：如实四舍五入
        assert!(number_sourced("5050", &["共 5049.6 行".to_string()]));
        // 整数源对整数 needle：±1 放水关闭
        assert!(
            !number_sourced("5050", &["共 5049 行".to_string()]),
            "工具说 5049、模型报 5050 不是四舍五入，是改数"
        );
        assert!(
            !number_sourced("5051", &["共 5050 行".to_string()]),
            "反向同堵"
        );
    }
}
