"""Skill Registry — Phase 3 step 3。

合并 builtin skills + 自定义 SKILL.md skills，提供统一注册表。

Phase 3 加载路径：
1. builtin: builtin.py 的 3 个文档生成 Skill
2. custom: 扫描 ~/.openclaw/skills/*/SKILL.md（用户数据目录）

加载顺序：builtin 优先注册（同名 custom 被覆盖）。
"""
from __future__ import annotations

import os
from pathlib import Path
from typing import Dict, List, Optional

from agent_core.skills.builtin import (
    BUILTIN_SKILLS,
    BuiltinSkill,
)
from agent_core.skill_loader import load_skill, SkillLoadError


# 默认扫描路径（用户数据目录）
DEFAULT_CUSTOM_SKILLS_DIRS = [
    "~/.openclaw/skills",
    "~/.wmessage/skills",
]


class SkillRegistry:
    """Skill 注册表 — 合并 builtin + custom。

    用法：
        reg = SkillRegistry()
        reg.load_default()  # 加载 builtin + 默认自定义目录
        skill = reg.find_by_intent("做个 word")
        skills = reg.list_all()
    """

    def __init__(self):
        self._skills: Dict[str, BuiltinSkill] = {}

    # ───────────────────────── 加载 ─────────────────────────

    def register_builtin(self) -> int:
        """注册所有 builtin skills。"""
        count = 0
        for name, skill in BUILTIN_SKILLS.items():
            if name not in self._skills:
                self._skills[name] = skill
                count += 1
        return count

    def load_custom_dir(self, dir_path: str) -> int:
        """从目录加载自定义 SKILL.md skills。

        Args:
            dir_path: 目录路径（支持 ~ 展开）

        Returns:
            成功加载的 skill 数量
        """
        p = Path(os.path.expanduser(dir_path)).expanduser()
        if not p.is_dir():
            return 0

        count = 0
        for entry in sorted(p.iterdir()):
            if not entry.is_dir():
                continue
            skill_md = entry / "SKILL.md"
            if not skill_md.exists():
                continue
            try:
                fm, _ = load_skill(str(skill_md))
                skill = self._custom_to_builtin(entry.name, fm)
                if skill:
                    # builtin 已存在的同名 skill 优先
                    if entry.name not in self._skills:
                        self._skills[entry.name] = skill
                        count += 1
            except SkillLoadError:
                continue
        return count

    def load_default(self) -> int:
        """加载默认 builtin + 标准自定义目录。"""
        return self.load_defaults_verbose()[0]

    def load_defaults_verbose(self) -> tuple:
        """加载默认 builtin + 标准自定义目录，返回 (count, details)。

        Returns:
            (total_count, dict of source → loaded_count)
            - total_count = builtin_count + sum(custom counts)
            - details["builtin"] = builtin_count
            - details[<dir>] = custom count for that dir
        """
        builtin_count = self.register_builtin()
        details: Dict[str, int] = {"builtin": builtin_count}
        for d in DEFAULT_CUSTOM_SKILLS_DIRS:
            n = self.load_custom_dir(d)
            details[d] = n
        total = builtin_count + sum(v for k, v in details.items() if k != "builtin")
        return total, details

    # ───────────────────────── 查询 ─────────────────────────

    def list_all(self) -> List[BuiltinSkill]:
        return list(self._skills.values())

    def get(self, name: str) -> Optional[BuiltinSkill]:
        return self._skills.get(name)

    def find_by_intent(self, query: str) -> Optional[BuiltinSkill]:
        """按 intent 关键词找 skill。"""
        q = (query or "").lower()
        for skill in self._skills.values():
            for intent in skill.intents:
                if intent.lower() in q:
                    return skill
        return None

    def names(self) -> List[str]:
        return list(self._skills.keys())

    def __len__(self) -> int:
        return len(self._skills)

    def __contains__(self, name: str) -> bool:
        return name in self._skills

    # ───────────────────────── helpers ─────────────────────────

    @staticmethod
    def _custom_to_builtin(name: str, fm: Dict[str, Any]) -> Optional[BuiltinSkill]:
        """把 SKILL.md frontmatter 转 BuiltinSkill 对象。

        自定义 skill 仅支持 frontmatter 中明确的字段（intents / mode / risk / 
        tools_whitelist / react_max_steps / timeout_s）。缺字段走默认。
        """
        if not isinstance(fm, dict) or "name" not in fm:
            return None

        intents_raw = fm.get("intents", [])
        if isinstance(intents_raw, str):
            intents = [intents_raw]
        elif isinstance(intents_raw, list):
            intents = [str(i) for i in intents_raw]
        else:
            intents = []

        tools_raw = fm.get("tools_whitelist", [])
        if isinstance(tools_raw, str):
            tools = [tools_raw]
        elif isinstance(tools_raw, list):
            tools = [str(t) for t in tools_raw]
        else:
            tools = []

        try:
            return BuiltinSkill(
                name=str(fm["name"]),
                description=str(fm.get("description", "")),
                intents=intents,
                mode=str(fm.get("mode", "auto")),
                risk=str(fm.get("risk", "low")),
                tools_whitelist=tools,
                react_max_steps=int(fm.get("react_max_steps", 15)),
                timeout_s=int(fm.get("timeout_s", 60)),
                tags=[str(t) for t in fm.get("tags", [])] if isinstance(fm.get("tags"), list) else [],
            )
        except (ValueError, TypeError):
            return None
