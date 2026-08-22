"""planner 测试 — 启发式 + LLM 模式 + 校验。"""
from __future__ import annotations

import pytest

from agent_core.planner import (
    plan,
    _heuristic_plan,
    _validate_plan,
    PlanError,
    PlanValidationError,
    PlanLLMError,
)
from agent_core.types import TaskGraph, TaskNode, VerifyHint


# ───────────────────────── 启发式基础 ─────────────────────────


def test_empty_query_returns_empty_graph():
    """空 query → 0 个 task。"""
    r = plan("")
    assert r.graph.tasks == []
    assert "为空" in r.reasoning


def test_whitespace_only_query_returns_empty_graph():
    """纯空白 query → 0 个 task（与空等价）。"""
    r = plan("   \n\t  ")
    assert r.graph.tasks == []


def test_single_sentence_creates_one_task():
    """单句 → 1 个 task，deps 为空。"""
    r = plan("实现一个 hello world 函数")
    assert len(r.graph.tasks) == 1
    assert r.graph.tasks[0].id == "t1"
    assert r.graph.tasks[0].deps == []
    assert "hello world" in r.graph.tasks[0].title


def test_multi_sentence_chinese_full_stop():
    """中文句号切分 → 多 task，线性 deps。"""
    r = plan("读 config.yaml。改 timeout 为 30。跑 pytest。")
    assert len(r.graph.tasks) == 3
    assert [t.id for t in r.graph.tasks] == ["t1", "t2", "t3"]
    assert r.graph.tasks[1].deps == ["t1"]
    assert r.graph.tasks[2].deps == ["t2"]


def test_multi_sentence_english_full_stop():
    """英文句号切分 → 多 task。"""
    r = plan("Read the file. Edit line 5. Run tests.")
    assert len(r.graph.tasks) == 3
    assert [t.id for t in r.graph.tasks] == ["t1", "t2", "t3"]


def test_multi_sentence_mixed_punctuation():
    """混合标点（；！？）切分。"""
    r = plan("先 git pull；再 npm install；最后 npm test！")
    assert len(r.graph.tasks) == 3


def test_default_verify_hint_applied():
    """默认每个 task 带 command `echo done` 占位 verify_hint。"""
    r = plan("写一个单元测试")
    assert r.graph.tasks[0].verify_hint is not None
    assert r.graph.tasks[0].verify_hint.kind == "command"
    assert r.graph.tasks[0].verify_hint.expect == "exit_0"


def test_mode_default_is_auto():
    """TaskGraph.mode 默认 auto。"""
    r = plan("随便做点啥")
    assert r.graph.mode == "auto"


def test_root_query_stored():
    """TaskGraph.root_query 保留原始 query。"""
    q = "  实现 cache  +  invalidate  "
    r = plan(q)
    assert r.graph.root_query == q.strip()


# ───────────────────────── Skill body 命中 ─────────────────────────


def test_skill_body_wraps_into_single_task():
    """有 skill_body → 整段作为 1 个 task（不切分）。"""
    skill = "# 归档演示\n调用 list_tasks 拿所有任务。"
    r = plan("演示任务归档", skill_body=skill)
    assert len(r.graph.tasks) == 1
    assert "演示任务归档" in r.graph.tasks[0].title
    assert "Skill" in r.reasoning


def test_empty_skill_body_falls_back_to_heuristic():
    """skill_body 是空字符串 → 走启发式。"""
    r = plan("A。B。", skill_body="")
    assert len(r.graph.tasks) == 2


def test_whitespace_skill_body_falls_back_to_heuristic():
    """skill_body 是纯空白 → 走启发式。"""
    r = plan("A。B。", skill_body="   \n\t  ")
    assert len(r.graph.tasks) == 2


def test_skill_body_long_query_truncates_title():
    """Skill body + 长 query → title 截断到 80 字符。"""
    long_q = "演示" + "啊" * 200
    r = plan(long_q, skill_body="任意 skill")
    title = r.graph.tasks[0].title
    assert len(title) <= 81  # 80 + "…"
    assert title.endswith("…")


# ───────────────────────── max_tasks 限制 ─────────────────────────


def test_max_tasks_1_keeps_only_first():
    """max_tasks=1 → 只保留第 1 个 task。"""
    r = plan("A。B。C。", max_tasks=1)
    assert len(r.graph.tasks) == 1
    assert r.graph.tasks[0].id == "t1"


def test_max_tasks_caps_long_query():
    """10 句 query + max_tasks=5 → 只生成 5 个。"""
    q = "。".join([f"句{i}" for i in range(10)])
    r = plan(q, max_tasks=5)
    assert len(r.graph.tasks) == 5


def test_max_tasks_zero_raises_value_error():
    """max_tasks=0 → ValueError。"""
    with pytest.raises(ValueError, match="max_tasks"):
        plan("随便", max_tasks=0)


def test_max_tasks_negative_raises_value_error():
    """max_tasks=-1 → ValueError。"""
    with pytest.raises(ValueError, match="max_tasks"):
        plan("随便", max_tasks=-1)


# ───────────────────────── 参数校验 ─────────────────────────


def test_non_str_query_raises_value_error():
    """query 不是 str → ValueError。"""
    with pytest.raises(ValueError, match="query"):
        plan(123)  # type: ignore[arg-type]


def test_reasoning_contains_split_info():
    """reasoning 应包含拆分思路。"""
    r = plan("A。B。")
    assert "拆分" in r.reasoning or "句子" in r.reasoning


def test_long_query_warning_in_reasoning():
    """超长 query → reasoning 含警告。"""
    long_q = "做" * 2000
    r = plan(long_q, max_tasks=20)
    assert "⚠️" in r.reasoning or "上限" in r.reasoning


# ───────────────────────── TaskGraph 校验 ─────────────────────────


def test_validate_duplicate_ids_raises():
    """重复 task id → PlanValidationError。"""
    graph = TaskGraph(
        tasks=[
            TaskNode(id="t1", title="A"),
            TaskNode(id="t1", title="B"),
        ],
        root_query="x",
    )
    with pytest.raises(PlanValidationError, match="重复"):
        _validate_plan(graph)


def test_validate_missing_dep_raises():
    """dep 不存在 → PlanValidationError。"""
    graph = TaskGraph(
        tasks=[
            TaskNode(id="t1", title="A", deps=["t99"]),
        ],
        root_query="x",
    )
    with pytest.raises(PlanValidationError, match="不存在"):
        _validate_plan(graph)


def test_validate_cycle_raises():
    """环（t1→t2→t1）→ PlanValidationError。"""
    graph = TaskGraph(
        tasks=[
            TaskNode(id="t1", title="A", deps=["t2"]),
            TaskNode(id="t2", title="B", deps=["t1"]),
        ],
        root_query="x",
    )
    with pytest.raises(PlanValidationError, match="环"):
        _validate_plan(graph)


def test_validate_self_loop_raises():
    """自环（t1→t1）→ PlanValidationError。"""
    graph = TaskGraph(
        tasks=[
            TaskNode(id="t1", title="A", deps=["t1"]),
        ],
        root_query="x",
    )
    with pytest.raises(PlanValidationError, match="环"):
        _validate_plan(graph)


def test_validate_empty_title_raises():
    """task.title 为空 → PlanValidationError。"""
    graph = TaskGraph(
        tasks=[TaskNode(id="t1", title="   ")],
        root_query="x",
    )
    with pytest.raises(PlanValidationError, match="title"):
        _validate_plan(graph)


def test_validate_empty_graph_passes():
    """空 TaskGraph 合法。"""
    graph = TaskGraph(tasks=[], root_query="x")
    _validate_plan(graph)  # 不抛


def test_validate_diamond_dag_passes():
    """菱形 DAG（t1→t2,t1→t3,t2→t4,t3→t4）合法。"""
    graph = TaskGraph(
        tasks=[
            TaskNode(id="t1", title="A"),
            TaskNode(id="t2", title="B", deps=["t1"]),
            TaskNode(id="t3", title="C", deps=["t1"]),
            TaskNode(id="t4", title="D", deps=["t2", "t3"]),
        ],
        root_query="x",
    )
    _validate_plan(graph)  # 不抛


# ───────────────────────── LLM 模式 ─────────────────────────


def test_llm_mode_parses_valid_json():
    """LLM 模式：返回合法 JSON → 正确解析。"""
    llm_json = """{
        "tasks": [
            {"id": "t1", "title": "读 config", "deps": [], "verifyHint": null},
            {"id": "t2", "title": "改 timeout", "deps": ["t1"], "verifyHint": null}
        ],
        "reasoning": "线性两步"
    }"""
    r = plan("改 timeout", llm=lambda p: llm_json)
    assert len(r.graph.tasks) == 2
    assert r.graph.tasks[1].deps == ["t1"]
    assert r.reasoning == "线性两步"
    # verifyHint 为 null 时走默认
    assert r.graph.tasks[0].verify_hint.kind == "command"


def test_llm_mode_parses_verify_hint():
    """LLM 模式：verifyHint 完整解析。"""
    llm_json = """{
        "tasks": [
            {"id": "t1", "title": "跑测试", "deps": [],
             "verifyHint": {"kind": "command", "command": "pytest", "expect": "exit_0"}}
        ],
        "reasoning": "ok"
    }"""
    r = plan("跑测试", llm=lambda p: llm_json)
    assert r.graph.tasks[0].verify_hint.kind == "command"
    assert r.graph.tasks[0].verify_hint.command == "pytest"


def test_llm_mode_invalid_json_raises_llm_error():
    """LLM 模式：返回非 JSON → PlanLLMError。"""
    with pytest.raises(PlanLLMError, match="JSON"):
        plan("test", llm=lambda p: "not json at all")


def test_llm_mode_llm_call_raises():
    """LLM 模式：llm() 抛异常 → PlanLLMError。"""
    def bad_llm(p):
        raise RuntimeError("API down")
    with pytest.raises(PlanLLMError, match="LLM 调用失败"):
        plan("test", llm=bad_llm)


def test_llm_mode_tasks_not_list_raises():
    """LLM 模式：tasks 不是 list → PlanLLMError。"""
    with pytest.raises(PlanLLMError, match="tasks"):
        plan("test", llm=lambda p: '{"tasks": "not a list"}')


def test_llm_mode_task_not_dict_raises():
    """LLM 模式：task 不是 dict → PlanLLMError。"""
    with pytest.raises(PlanLLMError, match="task 不是 dict"):
        plan("test", llm=lambda p: '{"tasks": ["x"]}')


def test_llm_mode_respects_max_tasks():
    """LLM 模式：max_tasks 限制返回 task 数。"""
    llm_json = """{
        "tasks": [
            {"id": "t1", "title": "A", "deps": []},
            {"id": "t2", "title": "B", "deps": ["t1"]},
            {"id": "t3", "title": "C", "deps": ["t2"]}
        ],
        "reasoning": "ok"
    }"""
    r = plan("test", max_tasks=2, llm=lambda p: llm_json)
    assert len(r.graph.tasks) == 2


def test_llm_mode_validates_result():
    """LLM 模式：返回的图仍要走校验。"""
    llm_json = """{
        "tasks": [{"id": "t1", "title": "A", "deps": ["t99"]}],
        "reasoning": "ok"
    }"""
    with pytest.raises(PlanValidationError):
        plan("test", llm=lambda p: llm_json)


# ───────────────────────── to_dict roundtrip ─────────────────────────


def test_plan_result_to_dict():
    """PlanResult.to_dict() 字段齐全。"""
    r = plan("A。B。")
    d = r.to_dict()
    assert "graph" in d
    assert "reasoning" in d
    assert "tasks" in d["graph"]
    assert d["graph"]["mode"] == "auto"


def test_task_node_to_dict():
    """TaskNode.to_dict() 字段齐全。"""
    r = plan("A")
    d = r.graph.tasks[0].to_dict()
    assert "id" in d
    assert "title" in d
    assert "deps" in d
    assert "verifyHint" in d
