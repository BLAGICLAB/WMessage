"""intent_router + builtin Skill 测试。"""
from __future__ import annotations

import pytest

from agent_core.intent_router import route_intent, list_routable_skills
from agent_core.skills.builtin import (
    BUILTIN_SKILLS,
    BuiltinSkill,
    DOCX_SKILL,
    XLSX_SKILL,
    PPTX_SKILL,
    find_builtin_skill_by_intent,
    get_builtin_skill,
)


# ───────────────────────── 内置 Skill 元数据 ─────────────────────────


def test_all_three_builtin_skills_defined():
    """3 个文档生成 Skill 全部定义。"""
    assert "minimax-docx" in BUILTIN_SKILLS
    assert "minimax-xlsx" in BUILTIN_SKILLS
    assert "minimax-pptx" in BUILTIN_SKILLS
    assert len(BUILTIN_SKILLS) == 3


def test_docx_skill_metadata():
    """docx Skill 元数据。"""
    s = DOCX_SKILL
    assert s.name == "minimax-docx"
    assert s.mode == "auto"
    assert s.risk == "low"
    assert "doc_make_word" in s.tools_whitelist
    assert len(s.intents) >= 5


def test_xlsx_skill_metadata():
    """xlsx Skill 元数据。"""
    s = XLSX_SKILL
    assert s.name == "minimax-xlsx"
    assert "doc_make_excel" in s.tools_whitelist
    assert "table" in s.tags


def test_pptx_skill_metadata():
    """pptx Skill 元数据。"""
    s = PPTX_SKILL
    assert s.name == "minimax-pptx"
    assert "doc_make_ppt" in s.tools_whitelist
    assert "presentation" in s.tags


def test_get_builtin_skill():
    """get_builtin_skill 按 name 查。"""
    s = get_builtin_skill("minimax-docx")
    assert s is DOCX_SKILL


def test_get_builtin_skill_not_found():
    """不存在的 name → None。"""
    assert get_builtin_skill("nonexistent") is None


# ───────────────────────── 关键词路由 ─────────────────────────


def test_route_docx_chinese():
    """中文触发 docx。"""
    assert route_intent("帮我生成一个 Word 文档") is DOCX_SKILL


def test_route_docx_short():
    """简短触发。"""
    assert route_intent("做个 word") is DOCX_SKILL


def test_route_xlsx_chinese():
    """中文触发 xlsx。"""
    assert route_intent("做个表格") is XLSX_SKILL


def test_route_xlsx_english_keyword():
    """英文关键词触发 xlsx。"""
    assert route_intent("导出 xlsx 文件") is XLSX_SKILL


def test_route_pptx_chinese():
    """中文触发 pptx。"""
    assert route_intent("做个 PPT") is PPTX_SKILL


def test_route_pptx_english_keyword():
    """英文关键词触发 pptx。"""
    assert route_intent("create pptx") is PPTX_SKILL


def test_route_pptx_幻灯片():
    """'幻灯片' 触发 pptx。"""
    assert route_intent("做个幻灯片") is PPTX_SKILL


def test_route_no_match_returns_none():
    """无匹配返回 None。"""
    assert route_intent("讲个笑话") is None
    assert route_intent("") is None
    assert route_intent("hello world") is None


def test_route_case_insensitive():
    """大小写不敏感。"""
    assert route_intent("生成 WORD") is DOCX_SKILL
    assert route_intent("Create PPTX") is PPTX_SKILL


def test_route_with_extra_context():
    """含其他文字也能匹配（子串匹配）。"""
    assert route_intent("我想让你帮我生成一个 docx 文件") is DOCX_SKILL


def test_route_unicode_keywords():
    """Unicode 关键词也能匹配。"""
    # 中文逗号、空格等不影响
    assert route_intent("做个，excel，表格") is XLSX_SKILL


def test_route_priority_docx_before_xlsx_for_ambiguous():
    """模糊查询时的优先级（先到先得）。

    注：当前路由顺序按 BUILTIN_SKILLS dict 插入顺序 — docx → xlsx → pptx。
    '生成文件' 这种无明确关键词的查询应不命中（仅 '生成' / '文件' 不在任一 skill 的 intents）。
    """
    # '生成文件' 没有完整匹配任一 intent → 应 None（不是 docx）
    assert route_intent("生成文件") is None


def test_route_all_skills_matchable():
    """每个 builtin skill 至少一个 intent 能命中。"""
    for skill in BUILTIN_SKILLS.values():
        # 第一个 intent 是最有代表性的，应能命中
        s = route_intent(skill.intents[0])
        assert s is not None
        assert s.name == skill.name


# ───────────────────────── list_routable_skills ─────────────────────────


def test_list_routable_skills():
    """list_routable_skills 返回所有 builtin。"""
    skills = list_routable_skills()
    assert len(skills) == 3
    assert DOCX_SKILL in skills
    assert XLSX_SKILL in skills
    assert PPTX_SKILL in skills


# ───────────────────────── find_builtin_skill_by_intent ─────────────────────────


def test_find_builtin_skill_by_intent_basic():
    """find_builtin_skill_by_intent 基础用法。"""
    assert find_builtin_skill_by_intent("docx") is DOCX_SKILL


def test_find_builtin_skill_by_intent_none():
    """find_builtin_skill_by_intent 无命中。"""
    assert find_builtin_skill_by_intent("xyz") is None


# ───────────────────────── Skill 字段稳定性 ─────────────────────────


def test_docx_tools_whitelist_exactly_one():
    """docx 只暴露 doc_make_word（限制 LLM 调用范围）。"""
    assert DOCX_SKILL.tools_whitelist == ["doc_make_word"]


def test_xlsx_tools_whitelist_exactly_one():
    """xlsx 只暴露 doc_make_excel。"""
    assert XLSX_SKILL.tools_whitelist == ["doc_make_excel"]


def test_pptx_tools_whitelist_exactly_one():
    """pptx 只暴露 doc_make_ppt。"""
    assert PPTX_SKILL.tools_whitelist == ["doc_make_ppt"]


def test_intents_have_chinese_coverage():
    """每个 skill 的 intents 至少含 1 个中文。"""
    for skill in BUILTIN_SKILLS.values():
        has_zh = any(any("\u4e00" <= c <= "\u9fff" for c in i) for i in skill.intents)
        assert has_zh, f"{skill.name} intents 缺中文：{skill.intents}"


def test_intents_are_unique_per_skill():
    """每个 skill 的 intents 互不重复。"""
    for skill in BUILTIN_SKILLS.values():
        assert len(skill.intents) == len(set(skill.intents)), f"{skill.name} intents 重复"


def test_skill_risk_level():
    """所有 builtin skill 都是 low risk（文档生成无破坏性）。"""
    for skill in BUILTIN_SKILLS.values():
        assert skill.risk == "low"


def test_skill_mode_is_auto():
    """所有 builtin skill 都是 auto mode（不送 LLM 决策顺序）。"""
    for skill in BUILTIN_SKILLS.values():
        assert skill.mode == "auto"


def test_skill_timeout_reasonable():
    """timeout 在合理范围（30-300s）。"""
    for skill in BUILTIN_SKILLS.values():
        assert 30 <= skill.timeout_s <= 300
