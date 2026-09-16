// ─────────────────── System prompt slot 顺序与拼接 ───────────────────
//
// 把 system prompt 的多段拼接从脆弱的 `format!("{}{}", a, b)` 顺序依赖
// 改成显式的「声明顺序」+「按 slot 排序」模型。
//
// 设计动机：
// - 原 bot_chat.rs:534-776 用 3-4 处 `format!("{}{}", system_content, ...)`
//   顺序拼接，每处都靠位置正确。重构时易漏改一处、错位不会编译报错。
// - PromptSlot 的 `Ord` 由变体声明顺序决定（`#[derive(PartialOrd, Ord)]`），
//   build 时统一 sort_by_key，caller 用任意顺序 push，输出永远稳定。
// - 段间分隔符由 caller 控制（push 进来的 content 自带 `\n\n`），
//   build 不擅自加，避免「有时加、有时没加」的隐式行为。
//
// 行为契约：
// - empty content（`String::is_empty()`）在 build 时跳过
// - 同 slot 多次 push：按 push 顺序拼接（定义行为，单测覆盖）
// - 不同 slot：按 `PromptSlot` 声明顺序拼接（Base → GenDir → SkillCatalog
//   → SkillBody → Recovery → Plan）
//
// 不变量：
// - build 是 deterministic 的：相同输入永远相同输出
// - 排序是 stable 的（按 slot 分组后，组内保留 push 顺序）

/// System prompt 槽位枚举。**变体声明顺序即拼接顺序**——加新 slot 必须在
/// 文件末尾追加，不能插中间，否则会改变既有调用点的输出。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PromptSlot {
    /// 静态 SYSTEM_PROMPT（常驻首段）
    Base,
    /// gen_dir_rule（生成目录规则）
    GenDir,
    /// build_skill_block（已安装技能清单的「名称+描述」层，progressive disclosure 第一层）
    SkillCatalog,
    /// interactive 模式技能的正文（fall-through 给 LLM 的具体内容）
    SkillBody,
    /// recovery_hint（auto-mode 失败兜底，LLM 决策下一步的依据）
    Recovery,
    /// plan_block（动态计划步骤，PREVR 第二层）
    Plan,
}

/// System prompt 多段拼接器。push 顺序自由，build 时按 `PromptSlot` 排序。
///
/// 示例：
/// ```ignore
/// let mut b = SystemPromptBuilder::new();
/// b.push(PromptSlot::Base, SYSTEM_PROMPT);
/// b.push(PromptSlot::GenDir, format!("\n\n{}", gen_dir_rule(&app)));
/// b.push(PromptSlot::SkillCatalog, format!("\n\n{}", build_skill_block(&app)));
/// // 任意顺序追加交互段
/// b.push(PromptSlot::Recovery, recovery_hint);
/// b.push(PromptSlot::Plan, plan_block);
/// let system_content = b.build();
/// ```
#[derive(Debug, Default)]
pub struct SystemPromptBuilder {
    parts: Vec<(PromptSlot, String)>,
}

impl SystemPromptBuilder {
    pub fn new() -> Self {
        Self { parts: Vec::new() }
    }

    /// 推入一段内容。caller 控制分隔符（典型做法：除首段外的段自带 `\n\n` 前缀）。
    /// 返回 `&mut Self` 支持链式调用。
    pub fn push(&mut self, slot: PromptSlot, content: impl Into<String>) -> &mut Self {
        self.parts.push((slot, content.into()));
        self
    }

    /// 按 `PromptSlot` 声明顺序拼接所有非空段。空段跳过，同 slot 多段按 push 顺序拼接。
    pub fn build(self) -> String {
        let mut sorted = self.parts;
        // 显式按 slot 枚举顺序排序（依赖 `#[derive(PartialOrd, Ord)]` 的变体顺序）
        sorted.sort_by_key(|(slot, _)| *slot);
        let mut out = String::new();
        for (_slot, content) in sorted {
            if !content.is_empty() {
                out.push_str(&content);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_builder_returns_empty() {
        let b = SystemPromptBuilder::new();
        assert_eq!(b.build(), "");
    }

    #[test]
    fn skips_empty_slots() {
        // 只 push Base + Recovery,其它都不 push
        let mut b = SystemPromptBuilder::new();
        b.push(PromptSlot::Base, "BASE");
        b.push(PromptSlot::Recovery, "RECOVERY");
        // 验证空字符串 content 也被跳过:push 一个 Plan 空内容
        b.push(PromptSlot::Plan, "");
        // 输出只含 Base + Recovery 两段
        let out = b.build();
        assert_eq!(out, "BASERECOVERY");
        // 不应出现空段产生的边界
        assert!(!out.contains("BASERECOVERY\n"));
    }

    #[test]
    fn builds_in_declared_order() {
        // 故意乱序 push 6 段
        let mut b = SystemPromptBuilder::new();
        b.push(PromptSlot::Plan, "[Plan]");
        b.push(PromptSlot::Base, "[Base]");
        b.push(PromptSlot::Recovery, "[Recovery]");
        b.push(PromptSlot::GenDir, "[GenDir]");
        b.push(PromptSlot::SkillBody, "[SkillBody]");
        b.push(PromptSlot::SkillCatalog, "[SkillCatalog]");
        // build 后应严格按声明顺序:Base → GenDir → SkillCatalog → SkillBody → Recovery → Plan
        let out = b.build();
        assert_eq!(
            out,
            "[Base][GenDir][SkillCatalog][SkillBody][Recovery][Plan]"
        );
    }

    #[test]
    fn same_slot_pushed_twice_concatenates() {
        // 同 slot push 两次 → 按 push 顺序拼接(定义行为)
        let mut b = SystemPromptBuilder::new();
        b.push(PromptSlot::Recovery, "FIRST");
        b.push(PromptSlot::Recovery, "_SECOND");
        let out = b.build();
        assert_eq!(out, "FIRST_SECOND");
    }

    #[test]
    fn same_slot_pushed_twice_after_sort_still_concatenates() {
        // 验证 sort 后组内顺序仍按 push 顺序:先 push Plan,再 push Base,
        // sort 后 Base 在前,但两个 Recovery 仍按 push 顺序拼接。
        let mut b = SystemPromptBuilder::new();
        b.push(PromptSlot::Recovery, "_BETA");
        b.push(PromptSlot::Base, "[Base]");
        b.push(PromptSlot::Recovery, "_ALPHA");
        let out = b.build();
        // sort 后:Base 段在前,两个 Recovery 段相邻,顺序为 _BETA → _ALPHA
        assert_eq!(out, "[Base]_BETA_ALPHA");
    }

    #[test]
    fn caller_owns_separators() {
        // 验证 build 不擅自加分隔符:push 之间没 \n,输出也没 \n
        let mut b = SystemPromptBuilder::new();
        b.push(PromptSlot::Base, "AAA");
        b.push(PromptSlot::GenDir, "BBB");
        b.push(PromptSlot::SkillCatalog, "CCC");
        assert_eq!(b.build(), "AAABBBCCC");
    }

    #[test]
    fn caller_can_include_separators_in_content() {
        // 验证 caller 在 content 里自带 \n\n,build 原样输出
        let mut b = SystemPromptBuilder::new();
        b.push(PromptSlot::Base, "AAA");
        b.push(PromptSlot::GenDir, "\n\nBBB");
        b.push(PromptSlot::SkillCatalog, "\n\nCCC");
        assert_eq!(b.build(), "AAA\n\nBBB\n\nCCC");
    }
}
