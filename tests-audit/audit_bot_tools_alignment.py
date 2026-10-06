#!/usr/bin/env python3
"""
bot 工具面对拍（T1-QUERYTASKS 起常备，风格同 audit_module_map.py）：

工具升级（合并/改名/扩参）最容易烂在三处「Rust 单测锁不住的跨文件漂移」：
提示词残留旧工具名、ToolDef 与 schema 的名字配对、mutating 工具漏 claims_patterns
（防幻觉守卫出现盲区）。本脚本从源码文本独立对拍：

1. 提示词引用完整性：src-tauri/src/prompts/*.rs 里出现的「工具名形状」标识符
   必须 ⊆ registry.rs TOOLS_TABLE 注册名（合并/改名后提示词残留旧名 → FAIL）
2. 废弃名零残留：本轮退役的 list_tasks / search_tasks 在 prompts/ 与
   bot_skills/runtime.rs（Skill 回滚清单）零出现
3. schema↔表配对：TOOLS_TABLE 每条的 schema 常量内 function.name 必须 == ToolDef.name
4. mutating 工具 claims_patterns 非空（非 mutating 必须为空）

执行：python3 -m pytest tests-audit/audit_bot_tools_alignment.py -v
"""

import re
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "src-tauri/src"
REGISTRY = SRC / "bot/registry.rs"
PROMPTS_DIR = SRC / "prompts"
SKILL_RUNTIME = SRC / "bot_skills/runtime.rs"

# 本轮（T1-QUERYTASKS）退役的工具名；后续批次合并/改名时在此追加并清引用
RETIRED_TOOLS = {"list_tasks", "search_tasks"}

SCHEMA_CONST_RE = re.compile(r'pub const (SCHEMA_\w+): &str = r##"(.*?)"##;', re.S)
TABLE_BLOCK_RE = re.compile(
    r"pub static TOOLS_TABLE: &\[ToolDef\] = &\[(.*?)\n\];", re.S
)
ENTRY_RE = re.compile(
    r'ToolDef \{\s*name:\s*"(\w+)",\s*schema:\s*(SCHEMA_\w+),\s*'
    r"mutating:\s*(true|false),\s*claims_patterns:\s*&\[([^\]]*)\]",
    re.S,
)
# 提示词里的「工具名形状」标识符（动词_名词，与注册表命名风格一致）
TOOLISH_RE = re.compile(r"\b(?:[a-z]+_[a-z_0-9]+)\b")


def registry_text() -> str:
    assert REGISTRY.exists(), "缺 bot/registry.rs"
    return REGISTRY.read_text()


def registered_tools() -> dict:
    """解析 TOOLS_TABLE → {name: {"schema_const": str, "mutating": bool, "claims": str}}"""
    text = registry_text()
    block = TABLE_BLOCK_RE.search(text)
    assert block, "registry.rs 缺 TOOLS_TABLE 静态表"
    tools = {}
    for name, schema_const, mutating, claims in ENTRY_RE.findall(block.group(1)):
        assert name not in tools, f"TOOLS_TABLE 重复注册：{name}"
        tools[name] = {
            "schema_const": schema_const,
            "mutating": mutating == "true",
            "claims": claims,
        }
    return tools


def schema_consts() -> dict:
    return dict(SCHEMA_CONST_RE.findall(registry_text()))


def prompt_files() -> list:
    files = sorted(PROMPTS_DIR.glob("*.rs"))
    assert files, "prompts 目录为空？"
    return files


def test_table_is_wellformed():
    tools = registered_tools()
    assert len(tools) == 39, f"TOOLS_TABLE 应为 39 条（N6 后 37 主可见 + 2 子 agent），实际 {len(tools)}"
    assert not (set(tools) & RETIRED_TOOLS), "退役工具名不得再出现在 TOOLS_TABLE"


def test_prompts_never_mention_retired_tools():
    """合并/改名后提示词残留旧名 = 模型按提示调不存在的工具（silent bug）"""
    for f in prompt_files():
        text = f.read_text()
        for name in RETIRED_TOOLS:
            assert name not in text, f"{f.name} 残留退役工具名 {name}"


def test_skill_readonly_list_only_registered():
    """bot_skills/runtime.rs 的 READONLY 回滚豁免清单必须只含注册名（且不含退役名）"""
    text = SKILL_RUNTIME.read_text()
    m = re.search(r"const READONLY: \[&str; \d+\] = \[(.*?)\];", text, re.S)
    assert m, "runtime.rs 缺 READONLY 清单"
    names = set(re.findall(r'"(\w+)"', m.group(1)))
    assert not (names & RETIRED_TOOLS), f"READONLY 残留退役名：{names & RETIRED_TOOLS}"
    registered = set(registered_tools())
    unknown = names - registered
    assert not unknown, f"READONLY 含未注册工具：{unknown}"


def test_prompt_toolish_identifiers_are_registered():
    """提示词中所有「动词_名词」形状标识符 ⊆ 注册表 ∪ 已知非工具白名单"""
    registered = set(registered_tools())
    # 提示词里合法出现的非工具下划线词（目录名/字段名/中文语境词等）
    non_tool = {
        "ai_gen_files",
        "task_refs",
        "custom_colors",
        "timeout_secs",
        "wait_ms",
        "max_turns",
        "max_tool_calls",
        "max_wall_seconds",
        "model_profile",
        "parent_task_id",
        "subagent_id",
        "task_id",
        "acceptance_criteria",
        "context_summary",
        "reasoning_effort",
        "system_prompt",
        "skill_catalog",
        "gen_dir",
    }
    for f in prompt_files():
        text = f.read_text()
        for tok in set(TOOLISH_RE.findall(text)):
            if tok in registered or tok in non_tool or tok in RETIRED_TOOLS:
                # RETIRED 已由上一条测试单独报错，这里跳过避免重复
                continue
            # 形状像任务工具但不在注册表 → 大概率是改名残留
            if tok.endswith("_task") or tok.endswith("_subtask") or tok.endswith("_tasks"):
                pytest.fail(f"{f.name} 引用未注册工具名 {tok}（改名残留？）")


def test_schema_function_name_matches_tooldef_name():
    """ToolDef.name 与它引用的 schema 常量内 function.name 配对（防 silent dispatch miss）"""
    schemas = schema_consts()
    for name, info in registered_tools().items():
        body = schemas.get(info["schema_const"])
        assert body is not None, f"{name} 引用的常量 {info['schema_const']} 不存在"
        m = re.search(r'"function":\{"name":"(\w+)"', body)
        assert m, f"{info['schema_const']} 缺 function.name"
        assert (
            m.group(1) == name
        ), f"ToolDef.name={name} 与 schema function.name={m.group(1)} 漂移"


def test_mutating_tools_have_claims_patterns():
    """mutating 工具必须有防幻觉话术；非 mutating 必须为空（错填会带入伪变更检测）"""
    for name, info in registered_tools().items():
        claims = info["claims"].strip()
        if info["mutating"]:
            assert claims, f"{name} 是 mutating 但 claims_patterns 为空（防幻觉守卫盲区）"
        else:
            assert not claims, f"{name} 非 mutating 但 claims_patterns 不空"
