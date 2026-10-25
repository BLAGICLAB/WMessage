//! 工具结果回灌的体积漏斗：per-tool 预算与全局 cap 取小、超限转存与指针回灌。
//!
//! 本文件由 bot_model_loop.rs 拆分而来（纯搬移，行为零变化）。

/// 单条工具结果回灌默认上限（字节，UTF-8）。业界对齐——Claude Code 文件读 25K token、
/// Bash 30K 输出、Gemini CLI 40K 输出。配置 Some(0) = 显式关闭截断。
///
/// 为什么按字节而不是字符：这条上限的职责是「别把模型上下文撑爆」，而撑爆的是
/// token，不是字符数。字节到 token 的换算跨语种稳定得多（中文约 3 字节/token、
/// ASCII 约 4 字节/token），字符数则随语种漂三倍（同样 30000，中文占 3 倍字节、
/// 只占 1/4 的 token）。且同一漏斗里 trace 侧的 `db::SPAN_TEXT_MAX` 与溢出指针
/// 文案本来就按字节报，字符口径会让同一次溢出在两个地方显示成两个数。
pub(crate) const DEFAULT_TOOL_OUTPUT_CAP_BYTES: usize = 30_000;

/// 截断时尾部保留字节数：错误堆栈/汇总性内容常在尾部，只留头部会丢关键信息。
const TOOL_OUTPUT_TAIL_KEEP_BYTES: usize = 6_000;

/// 文档类工具：输出不是「工具结果回灌」，而是**模型明确要读的文档**。
///
/// 这类输出不按「结果回灌」漏斗截断/转存——截掉它不是省上下文，是删任务。
/// 实测：57KB 的技能正文按 8KB 头尾节选后，86% 丢失，「三、工作流程（七步法）」
/// 整节落在丢失的中段，模型不知道流程怎么走，只好自己去 grep 找标题，
/// 工具用错 → 技能判死 → 整个回合结束。
///
/// 技能正文自身有上限（`bot_skills::state::MAX_SKILL_BODY` = 50KB），
/// 所以放行不构成无界风险：per-tool 预算给到 65536 已经装得下任何一篇技能正文。
const DOCUMENT_OUTPUT_TOOLS: [&str; 1] = ["use_skill"];

/// 本次工具结果回灌的实际天花板（字节）：per-tool 预算与全局配置 cap 取小。
///
/// 两个都要，不能只要一个：
/// - per-tool 预算（`ToolDef.max_output_bytes`）管「这个工具天生输出就大」——
///   fetch_url / read_text_file / list_files 可以给到 65536，其余默认 8192。
/// - 全局 cap 管用户在设置页调的总量，也是唯一兜底——per-tool 预算被调到比它大时
///   不能放任，否则设置页那个旋钮就成了空设置。
///
/// 例外：文档类工具（`DOCUMENT_OUTPUT_TOOLS`）按 per-tool 预算走，不与全局 cap
/// 取小——全局 cap 调的是「工具结果」的总量，把「模型要读的文档」也压进去就是
/// 把任务删了；文档类工具的体量另有自身上限兜着。
pub(crate) fn effective_output_cap(tool: &str, global_cap_bytes: usize) -> usize {
    let per_tool = crate::bot::registry::tools_index()
        .get(tool)
        .map(|t| t.max_output_bytes)
        .unwrap_or(global_cap_bytes);
    if DOCUMENT_OUTPUT_TOOLS.contains(&tool) {
        return per_tool;
    }
    per_tool.min(global_cap_bytes)
}

/// 超预算时的渲染：优先转存溢出文件 + 指针；转存失败退回纯头尾截断。
///
/// 退回路径必须保留：落盘失败时若直接放行原文本，模型就白挨这一轮大上下文；
/// 若返回空，结果凭空消失更糟。`cap_tool_output` 的四一分在无第二份拷贝时是对的，
/// 这正是它作为兜底而非主路径的原因。
fn spill_or_cap<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    tool: &str,
    text: String,
    cap_bytes: usize,
) -> String {
    match crate::overflow::spill(app, &text) {
        Ok(path) => {
            let notice = crate::overflow::pointer_text(tool, &path, &text);
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Info,
                "tool.output.spilled",
                &[
                    ("tool", tool.to_string()),
                    ("original", text.len().to_string()),
                    ("cap", cap_bytes.to_string()),
                    ("path", path.display().to_string()),
                ],
            );
            preview_with_pointer(&text, &notice, cap_bytes)
        }
        Err(e) => {
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Warn,
                "tool.output.spill_failed",
                &[
                    ("tool", tool.to_string()),
                    ("cap", cap_bytes.to_string()),
                    ("err", e),
                ],
            );
            cap_tool_output(&text, cap_bytes)
        }
    }
}

/// 回灌漏斗的判定与处置（L1 per-tool 预算 × L2 全局 cap 取小后的执行段）：
/// 超出才转存/截断，没超原样透传。从薄壳 execute_tool 闭包抽出并泛型化——
/// 闭包绑死 Wry AppHandle，MockRuntime 单测够不到判定本身；「恰等 cap 不
/// 转存」「转存失败兜底非空」两条边界只有直驱这里才钉得住。
pub(crate) fn apply_output_cap<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    tool_output_cap: Option<usize>,
    name: &str,
    mut text: String,
) -> String {
    if let Some(global_cap) = tool_output_cap {
        let cap = effective_output_cap(name, global_cap);
        if text.len() > cap {
            // 「超预算」审计事件在**截断发生的地方**发：阈值就是这次
            // 真正用的 cap（同一个数）。此前这条事件在 dispatch 侧按
            // per-tool 预算报，而截断用 min(per-tool, 全局 cap)——
            // 全局 cap 更紧时（默认 30000 vs 长输出工具 65536）文本被
            // 截了而审计不报，运维据此查超预算会漏掉全部由全局 cap
            // 触发的截断；反过来审计报了也可能根本没截。
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Info,
                "tool.output.over_budget",
                &[
                    ("tool", name.to_string()),
                    ("output_bytes", text.len().to_string()),
                    ("max_bytes", cap.to_string()),
                ],
            );
            text = spill_or_cap(app, name, text, cap);
        }
    }
    text
}

/// 节选预览：头一半 + 指针 + 尾一半（字节口径）；指针自身长度先从 cap 里扣。
///
/// 为什么五五分而 `cap_tool_output` 是四一：那份是「没有第二份拷贝」时的最后手段，
/// 头部承载更多价值；这里完整内容已落盘、模型能分页读回，节选的职责只是让模型
/// 判断「要不要读、读哪一段」，不是复述正文，头尾对半才不至于把结论藏在另一半。
/// 纯函数便于单测。
pub(crate) fn preview_with_pointer(text: &str, notice: &str, cap_bytes: usize) -> String {
    // 指针两侧各一段 \n\n；不扣掉它们，节选会凭空超出 cap 这 4 个字节
    const SEPS: usize = 4;
    let half = cap_bytes.saturating_sub(notice.len() + SEPS) / 2;
    let head_end = crate::bot::format::byte_head_end(text, half);
    // 头尾不得重叠：正文短于两份预算时宁可不重复也别把同一段读两遍
    let tail_start = crate::bot::format::byte_tail_start(text, half).max(head_end);
    format!(
        "{}\n\n{}\n\n{}",
        &text[..head_end],
        notice,
        &text[tail_start..]
    )
}

/// 截断到 cap 字节：头 (cap-tail) + 截断说明 + 尾 tail。不超 cap 原样返回。
/// 纯函数便于单测；调用方（薄壳 execute_tool）负责审计留痕。
pub(crate) fn cap_tool_output(text: &str, cap_bytes: usize) -> String {
    let total = text.len();
    if total <= cap_bytes {
        return text.to_string();
    }
    // 小 cap 时尾部保留退化为 cap 的 1/5，保证头部至少 4/5
    let tail_keep = TOOL_OUTPUT_TAIL_KEEP_BYTES
        .min(cap_bytes / 5)
        .min(total / 2);
    let head_keep = cap_bytes - tail_keep;
    let head_end = crate::bot::format::byte_head_end(text, head_keep);
    let tail_start = crate::bot::format::byte_tail_start(text, tail_keep).max(head_end);
    let head_bytes = head_end;
    let tail_bytes = total - tail_start;
    format!(
        "{}\n…（已按 maxToolOutputBytes={cap_bytes} 截断，原文 {total} 字节，头 {head_bytes} + 尾 {tail_bytes} 保留）…\n{}",
        &text[..head_end],
        &text[tail_start..]
    )
}
