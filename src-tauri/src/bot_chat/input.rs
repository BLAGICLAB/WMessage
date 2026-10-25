//! 输入规范化面：不可见字符清洗、路由前判定、恢复提示。

use super::*;

/// FailedButRecoverable 兜底路径：把「失败原因 + 已完成产物 + 回滚状态」拼成可读提示，
/// 注入 system prompt 让 LLM 决策下一步（重试 / 调整 / 告知用户）。
/// 剥出便于单测：LLM 提示词格式不能漂移（用户在误用 Skill 后能看到一致结构）。
pub fn format_recovery_hint(
    reason: &str,
    completed_summary: &str,
    rollback_attempted: bool,
) -> String {
    format!(
        "\n\n【Skill 失败可恢复上下文】\n原因：{reason}\n已完成产物：\n{completed_summary}\n回滚已尝试：{}\n请基于以上产物决策：重试 / 调整 / 告知用户。",
        if rollback_attempted { "是" } else { "否" }
    )
}

/// 按 id 去重 TaskRef 列表，保留首次出现的标题（run_model_loop 工具循环完成后用）。
/// 剥出便于单测：dedup 顺序敏感（首次保留）有 spec 含义，不能漂移。
pub fn merge_task_refs_dedup(refs: Vec<TaskRef>) -> Vec<TaskRef> {
    // HashSet 判重（O(1) 查找）：批量大卡累积 refs 时 Vec::contains 是 O(n²)；
    // insert 返回「是否新插入」，首次保留语义不变
    let mut seen: std::collections::HashSet<String> =
        std::collections::HashSet::with_capacity(refs.len());
    refs.into_iter()
        .filter(|r| seen.insert(r.id.clone()))
        .collect()
}

/// 纯函数核（不碰 DB / app），便于把每条规则分别钉住。
pub(super) fn normalize_user_input_core(raw: &str) -> NormalizedInput {
    let mut text = String::with_capacity(raw.len());
    let mut stripped = 0usize;
    let mut changed = false;

    // 先统一行尾，再逐字符过滤：顺序反了会把 `\r\n` 里的 `\r` 当垃圾删掉，
    // 结果对、但审计里会记成「清掉了 1 个不可见字符」，误导排障。
    let unified: std::borrow::Cow<'_, str> = if raw.contains('\r') {
        changed = true;
        std::borrow::Cow::Owned(raw.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        // 不含 \r 时借用原串，避免为绝大多数输入做一次全量分配
        std::borrow::Cow::Borrowed(raw)
    };

    for c in unified.chars() {
        if INVISIBLE_JUNK.contains(&c) {
            stripped += 1;
            changed = true;
            continue;
        }
        if c == '\u{200B}' {
            // 零宽空格是「有语义的空间」→ 归成真空格，而不是当垃圾删掉
            text.push(' ');
            changed = true;
            continue;
        }
        text.push(c);
    }

    let trimmed = text.trim();
    if trimmed.len() != text.len() {
        changed = true;
    }
    NormalizedInput {
        text: trimmed.to_string(),
        changed,
        stripped,
    }
}

/// L0 入口：规范化本轮用户输入并就地改写（只动最后一条）。
///
/// 就地改 `messages` 而不是局部变量：这一轮之后，历史落库、记忆检索查询、
/// 技能推荐全都读同一个 `messages`——只清洗局部变量会让「本轮看到的」和
/// 「下一轮历史里的」不一致，于是下一轮又按原文重新判一遍，规范化等于没做。
pub(crate) fn normalize_user_input(messages: &mut [ChatMsg]) -> Option<NormalizedInput> {
    let last = messages.last_mut()?;
    if last.role != "user" {
        return None;
    }
    let out = normalize_user_input_core(&last.content);
    if out.changed {
        last.content = out.text.clone();
    }
    if out.changed {
        Some(out)
    } else {
        None
    }
}

/// 剥掉 <think>...</think> 段：非流式路径（bot_compact /
/// Planner）直接取 message.content，模型带 think 段时摘要会被写回历史、Planner 的
/// JSON 提取会被干扰。未闭合的 <think> 尾巴一并丢弃。
pub(crate) fn strip_think_blocks(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        rest = &rest[start + "<think>".len()..];
        match rest.find("</think>") {
            Some(end) => rest = &rest[end + "</think>".len()..],
            None => return out, // 未闭合：尾巴整个丢弃
        }
    }
    out.push_str(rest);
    out
}
