//! 长期记忆整理提示词（memory::consolidate 定时整理用）：输出 JSON `{"ops":[…]}`，
//! 解析失败即跳过本轮，措辞是唯一的格式防线。

/// 整理 prompt（中文；给 LLM 的条目标注真实 id，指令回指同一 id）
pub(crate) const CONSOLIDATE_PROMPT: &str = "\
你是记忆整理助手。下面是助手的长期记忆条目（格式：id | 类型 | 重要度1-5 | 内容）。\
请做一轮整理反思，只输出一个 JSON（不要输出其它任何文字），格式：\
{\"ops\":[\
{\"action\":\"merge\",\"ids\":[\"id1\",\"id2\"],\"content\":\"合并后的内容\"},\
{\"action\":\"contradiction\",\"keep\":\"保留的id\",\"drop\":\"删除的id\",\"content\":\"裁决后保留条目的新内容\"},\
{\"action\":\"distill\",\"ids\":[\"id1\",\"id2\"],\"content\":\"从这些条目提炼出的规律或反思\"}]}。\
规则：merge 用于内容重复或互补的条目；contradiction 用于互相矛盾的条目（按重要度和新旧裁决，\
保留更可靠的那条并把它的内容更新准确）；distill 用于从多条相关记忆提炼一般规律。\
没有值得做的就输出 {\"ops\":[]}。所有 content 用中文、各不超过 500 字。ids 必须原样引用上面的 id。";
