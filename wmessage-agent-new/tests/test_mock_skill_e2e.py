"""Mock Skill 端到端测试 — Phase 1 验收最后一项。

完整链路：
    load_skill → plan → parse_dsl → substitute_vars → executor.execute
              → extract_task_id → ctx push

通过本测试 = Phase 1 完整收尾（不只是模块单测，端到端跑通）。
"""
from __future__ import annotations

import os

import pytest

from agent_core.skill_loader import (
    load_skill,
    parse_dsl_steps,
    parse_tool_call_args,
)
from agent_core.mock_executor import RecordingExecutor
from agent_core.skill_runner import run_mock_skill, MockSkillError, ctx_summary
from agent_core.var_substitute import (
    substitute_vars,
    extract_task_id,
)


PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MOCK_SKILL_PATH = os.path.join(PROJECT_ROOT, "tests/fixtures/mock_skill_archive_demo.md")
# 标准 UUID v4（来自 MEMORY.md C 路径 Phase 4 第 3 项踩坑：必须是合法 v4 格式）
MOCK_TASK_ID = "7c9e6679-7425-40de-944b-e07fc1f90ae7"


def _make_executor():
    """标准 mock executor — list_tasks 返回含 UUID，query_single_task 返回 task 详情。"""
    return RecordingExecutor(
        responses={
            "list_tasks": (
                f"Found 3 tasks: [{MOCK_TASK_ID}, "
                "22222222-3333-4444-5555-666666666666, "
                "33333333-4444-5555-6666-777777777777]"
            ),
            "query_single_task": (
                "Task {id}: title='Hello world', done=false, created='2026-08-22'"
            ),
        }
    )


# ───────────────────────── Fixture 可用性 ─────────────────────────


def test_fixture_exists():
    """Mock Skill fixture 文件存在。"""
    assert os.path.exists(MOCK_SKILL_PATH), f"缺失 fixture：{MOCK_SKILL_PATH}"


def test_fixture_loadable():
    """fixture 文件可被 load_skill 解析。"""
    fm, body = load_skill(MOCK_SKILL_PATH)
    assert fm["name"] == "mock-skill-archive-demo"
    assert fm["mode"] == "auto"
    assert "任务归档" in fm["intents"]
    assert "## Step 1" in body
    assert "## Step 2" in body


# ───────────────────────── 端到端：load → plan → execute → ctx ─────────────────────────


def test_e2e_returns_one_task_plan():
    """skill_body 命中 → planner 整段作为 1 个 task。"""
    result = run_mock_skill(MOCK_SKILL_PATH, "演示任务归档", _make_executor())
    assert len(result["plan"]["tasks"]) == 1
    assert "演示任务归档" in result["plan"]["tasks"][0]["title"]


def test_e2e_executes_two_steps():
    """mock Skill 有 2 step → 2 次 executor 调用。"""
    executor = _make_executor()
    result = run_mock_skill(MOCK_SKILL_PATH, "演示任务归档", executor)
    assert result["steps_executed"] == 2
    assert len(executor.calls) == 2


def test_e2e_step1_calls_list_tasks():
    """step 1 → list_tasks({})。"""
    executor = _make_executor()
    run_mock_skill(MOCK_SKILL_PATH, "演示任务归档", executor)
    assert executor.calls[0]["name"] == "list_tasks"
    assert executor.calls[0]["args"] == {}


def test_e2e_step2_calls_query_single_task_with_extracted_uuid():
    """step 2 → query_single_task({"id": "<step1 提取的 UUID>"}）。

    这是变量替换（${step1.id} → UUID）端到端验证的核心。
    """
    executor = _make_executor()
    run_mock_skill(MOCK_SKILL_PATH, "演示任务归档", executor)
    assert executor.calls[1]["name"] == "query_single_task"
    assert executor.calls[1]["args"]["id"] == MOCK_TASK_ID


def test_e2e_ctx_contains_extracted_task_ids():
    """ctx 中每个 step 的 id 字段正确提取。"""
    executor = _make_executor()
    result = run_mock_skill(MOCK_SKILL_PATH, "演示任务归档", executor)

    # step 1: list_tasks 返回含 UUID → 提取成功
    assert result["ctx"][0]["index"] == 1
    assert result["ctx"][0]["title"] == "列出所有任务"
    assert result["ctx"][0]["id"] == MOCK_TASK_ID

    # step 2: query_single_task 不返回 UUID → id 为 None
    assert result["ctx"][1]["index"] == 2
    assert result["ctx"][1]["title"] == "查询第一张卡详情"
    assert result["ctx"][1]["id"] is None


def test_e2e_reasoning_contains_skill_note():
    """reasoning 含 Skill 命中标记。"""
    result = run_mock_skill(MOCK_SKILL_PATH, "演示任务归档", _make_executor())
    assert "Skill" in result["reasoning"]


def test_e2e_frontmatter_passed_through():
    """frontmatter 完整返回。"""
    result = run_mock_skill(MOCK_SKILL_PATH, "演示任务归档", _make_executor())
    fm = result["frontmatter"]
    assert fm["name"] == "mock-skill-archive-demo"
    assert fm["mode"] == "auto"
    assert fm["risk"] == "low"
    assert "归档演示" in fm["intents"]


# ───────────────────────── 变量替换（独立单测）─────────────────────────


def test_substitute_vars_resolves_step_id():
    """${step1.id} 替换为 ctx[0].id。"""
    ctx = [{"index": 1, "id": "uuid-1", "result": "r1"}]
    assert substitute_vars("${step1.id}", ctx) == "uuid-1"


def test_substitute_vars_resolves_step_result():
    """${step1.result} 替换为 ctx[0].result。"""
    ctx = [{"index": 1, "id": "uuid-1", "result": "hello\nworld"}]
    assert substitute_vars("${step1.result}", ctx) == "hello\nworld"


def test_substitute_vars_resolves_prev():
    """${prev.id} / ${prev.result} 指向 ctx 末尾。"""
    ctx = [
        {"index": 1, "id": "uuid-1", "result": "r1"},
        {"index": 2, "id": "uuid-2", "result": "r2"},
    ]
    assert substitute_vars("${prev.id}", ctx) == "uuid-2"
    assert substitute_vars("${prev.result}", ctx) == "r2"


def test_substitute_vars_preserves_unknown_index():
    """越界 index 保留原样。"""
    ctx = [{"index": 1, "id": "uuid-1", "result": "r1"}]
    assert substitute_vars("${step99.id}", ctx) == "${step99.id}"


def test_substitute_vars_preserves_unknown_field():
    """未知 field 保留原样。"""
    ctx = [{"index": 1, "id": "uuid-1", "result": "r1"}]
    assert substitute_vars("${step1.unknown}", ctx) == "${step1.unknown}"


def test_substitute_vars_preserves_when_id_none():
    """id 字段为 None 时保留原样。"""
    ctx = [{"index": 1, "id": None, "result": "r1"}]
    assert substitute_vars("${step1.id}", ctx) == "${step1.id}"


def test_substitute_vars_empty_ctx():
    """空 ctx 时所有 ${stepN.*} 保留原样（${prev.*} 也保留因为无 prev）。"""
    assert substitute_vars("${step1.id}", []) == "${step1.id}"
    assert substitute_vars("${prev.id}", []) == "${prev.id}"


def test_substitute_vars_no_placeholder():
    """无占位符直接返回原文本。"""
    assert substitute_vars("plain text", []) == "plain text"


def test_substitute_vars_non_str_passthrough():
    """非 str 输入原样返回。"""
    assert substitute_vars(123, []) == 123
    assert substitute_vars(None, []) is None
    assert substitute_vars({"k": "v"}, []) == {"k": "v"}


def test_substitute_vars_multiple_placeholders():
    """同一文本多个占位符。"""
    ctx = [{"index": 1, "id": "uuid-1", "result": "result-1"}]
    text = "id=${step1.id}, result=${step1.result}"
    out = substitute_vars(text, ctx)
    assert out == "id=uuid-1, result=result-1"


# ───────────────────────── UUID 提取（独立单测）─────────────────────────


def test_extract_task_id_first_uuid():
    """取首个 UUID。"""
    text = f"Found 3 tasks: [{MOCK_TASK_ID}, 22222222-3333-4444-5555-666666666666]"
    assert extract_task_id(text) == MOCK_TASK_ID


def test_extract_task_id_none_when_no_uuid():
    """无 UUID → None。"""
    assert extract_task_id("no uuid here") is None


def test_extract_task_id_handles_empty():
    """空 / None 输入 → None。"""
    assert extract_task_id("") is None
    assert extract_task_id(None) is None


def test_extract_task_id_lowercased():
    """UUID 转小写返回。"""
    text = "id=ABCDEF12-3456-7890-ABCD-EF1234567890"
    result = extract_task_id(text)
    assert result == "abcdef12-3456-7890-abcd-ef1234567890"


# ───────────────────────── Skill loader（独立单测）─────────────────────────


def test_load_skill_toml_frontmatter():
    """TOML frontmatter 解析。"""
    fm, body = load_skill(MOCK_SKILL_PATH)
    assert fm["name"] == "mock-skill-archive-demo"
    assert isinstance(fm["intents"], list)
    assert "任务归档" in fm["intents"]


def test_load_skill_yaml_fallback(tmp_path):
    """YAML frontmatter fallback。"""
    p = tmp_path / "yaml_skill.md"
    p.write_text(
        "---\n"
        "name: yaml-skill\n"
        "mode: auto\n"
        "intents:\n"
        "  - foo\n"
        "  - bar\n"
        "---\n"
        "body content\n"
        "## Step 1: x\n"
        "y()\n"
    )
    fm, body = load_skill(str(p))
    assert fm["name"] == "yaml-skill"
    assert "foo" in fm["intents"]
    assert "bar" in fm["intents"]


def test_load_skill_no_frontmatter(tmp_path):
    """无 frontmatter → ({}, 全文)。"""
    p = tmp_path / "no_fm.md"
    p.write_text("just body\n## Step 1: x\n")
    fm, body = load_skill(str(p))
    assert fm == {}
    assert "just body" in body


def test_parse_dsl_extracts_two_steps():
    """DSL parser 提取 2 个 step。"""
    _, body = load_skill(MOCK_SKILL_PATH)
    steps = parse_dsl_steps(body)
    assert len(steps) == 2
    assert steps[0]["index"] == 1
    assert "列出所有任务" in steps[0]["title"]
    assert steps[0]["tool_call"].startswith("list_tasks")
    assert steps[1]["index"] == 2
    assert "${step1.id}" in steps[1]["tool_call"]


def test_parse_dsl_empty_body():
    """空 body → 空列表。"""
    assert parse_dsl_steps("") == []
    assert parse_dsl_steps("no steps here") == []


def test_parse_tool_call_args_with_json():
    """'list_tasks({})' → ('list_tasks', {})。"""
    name, args = parse_tool_call_args("list_tasks({})")
    assert name == "list_tasks"
    assert args == {}


def test_parse_tool_call_args_with_nested_json():
    """嵌套 JSON 解析。"""
    name, args = parse_tool_call_args(
        'query_single_task({"id": "uuid-1"})'
    )
    assert name == "query_single_task"
    assert args == {"id": "uuid-1"}


def test_parse_tool_call_args_invalid():
    """无效格式 / 非 JSON → 约定（name=提取的工具名或 None, args=None 或 {}）。"""
    assert parse_tool_call_args("not a call") == (None, None)
    name, args = parse_tool_call_args("foo(invalid json)")
    # 实际：name 提取成功但 args 不是合法 JSON → (name, None)
    assert name == "foo"
    assert args is None


# ───────────────────────── ctx summary（独立单测）─────────────────────────


def test_ctx_summary_empty():
    """空 ctx → (无已完成步骤)。"""
    assert ctx_summary([]) == "(无已完成步骤)"


def test_ctx_summary_renders_steps():
    """ctx 渲染为人类可读文本。"""
    ctx = [
        {"index": 1, "title": "列出", "id": "uuid-1", "result": "Found 3"},
        {"index": 2, "title": "查询", "id": None, "result": "Task details"},
    ]
    s = ctx_summary(ctx)
    assert "step 1" in s
    assert "step 2" in s
    assert "Found 3" in s


# ───────────────────────── 端到端 + 错误路径 ──────────────────────────


def test_e2e_raises_on_invalid_dsl(tmp_path):
    """DSL 解析失败 → MockSkillError。"""
    p = tmp_path / "bad.md"
    p.write_text(
        "---\nname = 'bad'\n---\n"
        "## Step 1: title\n"
        "this is not a tool_call\n"
        "## Step 2: title2\n"
        "valid_call({})\n"
    )
    with pytest.raises(MockSkillError, match="解析失败"):
        run_mock_skill(str(p), "x", _make_executor())


def test_e2e_executor_called_with_substituted_args():
    """executor 收到的 args 必须是替换后的（不是模板）。"""
    executor = _make_executor()
    run_mock_skill(MOCK_SKILL_PATH, "演示", executor)
    # 第二步的 args["id"] 必须是真实 UUID，不能是 "${step1.id}"
    assert executor.calls[1]["args"]["id"] != "${step1.id}"
    assert executor.calls[1]["args"]["id"] == MOCK_TASK_ID


def test_e2e_full_chain_uuid_propagation():
    """端到端验证：UUID 从 step 1 传递到 step 2 的 args。

    这是 mock Skill 通路的「灵魂」 — 证明变量替换在 e2e 链路里真的工作。
    """
    executor = _make_executor()
    result = run_mock_skill(MOCK_SKILL_PATH, "x", executor)

    # step 1 返回含 UUID → extract_task_id → ctx[0].id
    step1_id = result["ctx"][0]["id"]
    assert step1_id == MOCK_TASK_ID

    # step 2 调用 query_single_task 时，args["id"] 就是 step1_id（变量替换生效）
    assert executor.calls[1]["args"]["id"] == step1_id
