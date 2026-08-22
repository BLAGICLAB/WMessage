"""tool_guard — Phase 3 step 5。

拦截高风险 tool_call，符合 SPEC §X.5 + §Z 黑名单约定。

Phase 3 黑名单（来自 WMessage Skill DSL 黑名单 + spec）：
- create_word_revisions: Word 修订模式专用，只在 Skill 内可用
- link_file_to_task: Skill 末尾绑产物专用

黑名单 + 白名单两种模式：
- 默认黑名单模式：仅 blacklist 中的工具被阻断，其余允许
- 白名单模式（更严）：只允许 whitelist 中的工具

注意：本模块是 Python 侧的二次防护，Rust 薄壳 bot.rs 也有同名黑名单。
两处都拦截 → 双保险。
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import FrozenSet, Optional


# 来自 SPEC §X.5 黑名单
DEFAULT_BLACKLIST: FrozenSet[str] = frozenset({
    "create_word_revisions",
    "link_file_to_task",
})

# 示例白名单（高安全场景用）
DEFAULT_WHITELIST: FrozenSet[str] = frozenset({
    "list_tasks",
    "create_task",
    "edit_task",
    "complete_task",
    "delete_task",
    "search_tasks",
    "query_single_task",
    "doc_make_word",
    "doc_make_excel",
    "doc_make_ppt",
    "doc_make_pdf",
    "doc_extract",
    "run_python",
    "web_search",
    "fetch_url",
    "use_skill",
})


@dataclass
class ToolGuardConfig:
    """tool_guard 配置。"""

    blacklist: FrozenSet[str] = DEFAULT_BLACKLIST
    whitelist: FrozenSet[str] = DEFAULT_BLACKLIST  # 默认空 = 白名单模式关闭
    whitelist_only: bool = False  # False = 黑名单模式；True = 白名单模式

    @classmethod
    def from_dict(cls, d: Optional[Dict[str, Any]] = None) -> "ToolGuardConfig":
        if not d:
            return cls()
        blacklist = frozenset(d.get("blacklist", DEFAULT_BLACKLIST))
        whitelist = frozenset(d.get("whitelist", DEFAULT_WHITELIST))
        whitelist_only = bool(d.get("whitelist_only", False))
        return cls(
            blacklist=blacklist,
            whitelist=whitelist,
            whitelist_only=whitelist_only,
        )


class ToolGuard:
    """tool_call 拦截器。

    用法：
        guard = ToolGuard(ToolGuardConfig())
        decision = guard.check("doc_make_word", args)
        if decision.allowed:
            executor.execute(...)
        else:
            audit.warn("pre_execute.deny", tool=name, reason=decision.reason)
    """

    def __init__(self, config: Optional[ToolGuardConfig] = None):
        self.config = config or ToolGuardConfig()

    def check(self, tool_name: str, args: Any = None) -> "GuardDecision":
        """检查 tool 是否可执行。"""
        if tool_name in self.config.blacklist:
            return GuardDecision(
                allowed=False,
                reason=f"tool '{tool_name}' 在黑名单中",
                tool=tool_name,
            )

        if self.config.whitelist_only:
            if tool_name not in self.config.whitelist:
                return GuardDecision(
                    allowed=False,
                    reason=f"tool '{tool_name}' 不在白名单（白名单模式）",
                    tool=tool_name,
                )

        return GuardDecision(
            allowed=True,
            reason="",
            tool=tool_name,
        )


@dataclass
class GuardDecision:
    """拦截决策。"""

    allowed: bool
    reason: str
    tool: str = ""

    def to_dict(self) -> Dict[str, Any]:
        return {
            "allowed": self.allowed,
            "reason": self.reason,
            "tool": self.tool,
        }
