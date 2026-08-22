"""replanner 测试 — 触发条件 + 重规划 + 错误分类。"""
from __future__ import annotations

import pytest

from agent_core.replanner import (
    CompletedStep,
    ReplanPolicy,
    ReplanReason,
    ReplanTrigger,
    classify_error,
    detect_env_drift,
    replan,
    should_replan_after_step,
    should_replan_global,
)
from agent_core.types import TaskGraph, TaskNode, VerifyHint


# ───────────────────────── 触发条件 ─────────────────────────


def test_should_replan_after_error():
    """task 报错 → 触发。"""
    trig = should_replan_after_step(
        ReplanPolicy(),
        task=TaskNode(id="t1", title="x"),
        react_steps_completed=1,
        last_error="boom",
    )
    assert trig.should_replan is True
    assert trig.reason == ReplanReason.TASK_FAILED
    assert "boom" in trig.detail


def test_should_replan_on_step_threshold():
    """达到步数阈值 → 触发。"""
    trig = should_replan_after_step(
        ReplanPolicy(trigger_step_threshold=3),
        task=TaskNode(id="t1", title="x"),
        react_steps_completed=3,
        last_error=None,
    )
    assert trig.should_replan is True
    assert trig.reason == ReplanReason.STEP_THRESHOLD


def test_should_replan_below_threshold_no_trigger():
    """未达步数阈值 → 不触发。"""
    trig = should_replan_after_step(
        ReplanPolicy(trigger_step_threshold=3),
        task=TaskNode(id="t1", title="x"),
        react_steps_completed=2,
        last_error=None,
    )
    assert trig.should_replan is False


def test_should_replan_no_error_below_threshold():
    """无错误且未达阈值 → 不触发。"""
    trig = should_replan_after_step(
        ReplanPolicy(),
        task=TaskNode(id="t1", title="x"),
        react_steps_completed=1,
        last_error=None,
    )
    assert trig.should_replan is False


def test_should_replan_threshold_only_at_multiples():
    """只在步数是阈值倍数时触发（不是每次都触发）。"""
    policy = ReplanPolicy(trigger_step_threshold=3)
    task = TaskNode(id="t1", title="x")
    # step 3 → trigger
    assert should_replan_after_step(policy, task, 3, None).should_replan is True
    # step 6 → trigger
    assert should_replan_after_step(policy, task, 6, None).should_replan is True
    # step 9 → trigger
    assert should_replan_after_step(policy, task, 9, None).should_replan is True


def test_should_replan_error_takes_priority_over_threshold():
    """错误触发优先于步数阈值。"""
    trig = should_replan_after_step(
        ReplanPolicy(trigger_step_threshold=100),
        task=TaskNode(id="t1", title="x"),
        react_steps_completed=1,
        last_error="boom",
    )
    assert trig.reason == ReplanReason.TASK_FAILED


# ───────────────────────── 全局上限 ─────────────────────────


def test_should_replan_global_below_limit():
    """replan_count < max → 不阻止。"""
    trig = should_replan_global(ReplanPolicy(max_replans_per_graph=5), replan_count=2)
    assert trig.should_replan is False
    assert trig.reason is None


def test_should_replan_global_at_limit():
    """replan_count >= max → 阻止再触发。"""
    trig = should_replan_global(
        ReplanPolicy(max_replans_per_graph=5), replan_count=5
    )
    assert trig.should_replan is False
    assert trig.reason == ReplanReason.GLOBAL_LIMIT
    assert "5" in trig.detail


def test_should_replan_global_step_limit_exceeded():
    """global_step_limit 命中 → 阻止。"""
    trig = should_replan_global(
        ReplanPolicy(),
        replan_count=1,
        global_step_limit=100,
        current_global_step=100,
    )
    assert trig.should_replan is False
    assert trig.reason == ReplanReason.GLOBAL_LIMIT


def test_should_replan_global_within_step_limit():
    """global_step_limit 未命中 → 不阻止。"""
    trig = should_replan_global(
        ReplanPolicy(),
        replan_count=0,
        global_step_limit=100,
        current_global_step=50,
    )
    assert trig.should_replan is False


# ───────────────────────── 环境漂移 ─────────────────────────


def test_detect_env_drift_git_status_changed():
    """git_status 变化 → 触发。"""
    trig = detect_env_drift(
        env_state={"git_status": "M src/main.py"},
        original_env_state={"git_status": "clean"},
    )
    assert trig.should_replan is True
    assert trig.reason == ReplanReason.ENV_DRIFT


def test_detect_env_drift_no_change():
    """无变化 → 不触发。"""
    trig = detect_env_drift(
        env_state={"git_status": "clean", "files_hint": ["a.py"]},
        original_env_state={"git_status": "clean", "files_hint": ["a.py"]},
    )
    assert trig.should_replan is False


def test_detect_env_drift_empty_inputs():
    """空输入 → 不触发。"""
    assert detect_env_drift({}, {}).should_replan is False
    assert detect_env_drift({"git_status": "x"}, {}).should_replan is False
    assert detect_env_drift({}, {"git_status": "x"}).should_replan is False


def test_detect_env_drift_files_hint_changed():
    """files_hint 变化 → 触发。"""
    trig = detect_env_drift(
        env_state={"files_hint": ["a.py", "b.py"]},
        original_env_state={"files_hint": ["a.py"]},
    )
    assert trig.should_replan is True


# ───────────────────────── 重规划 ─────────────────────────


def test_replan_preserves_completed_nodes():
    """已完成节点保留。"""
    graph = TaskGraph(
        tasks=[
            TaskNode(id="t1", title="A"),
            TaskNode(id="t2", title="B", deps=["t1"]),
            TaskNode(id="t3", title="C", deps=["t2"]),
        ],
        root_query="x",
    )
    completed = [CompletedStep(task_id="t1", title="A", result_summary="done")]
    new_graph = replan(graph, completed=completed, failed_task=None)
    assert len(new_graph.tasks) == 3
    assert new_graph.tasks[0].id == "t1"


def test_replan_increments_plan_version():
    """plan_version 递增。"""
    graph = TaskGraph(
        tasks=[TaskNode(id="t1", title="A")],
        root_query="x",
        plan_version=1,
    )
    new_graph = replan(graph, completed=[], failed_task=None)
    assert new_graph.plan_version == 2


def test_replan_failed_node_can_retry():
    """失败节点保留（agent_loop 会重新跑，attempts 已在外面 +1）。"""
    graph = TaskGraph(
        tasks=[TaskNode(id="t1", title="A")],
        root_query="x",
    )
    failed = TaskNode(id="t1", title="A")
    new_graph = replan(graph, completed=[], failed_task=failed, error="verify failed")
    assert new_graph.tasks[0].id == "t1"


def test_replan_root_query_preserved():
    """root_query 保留。"""
    graph = TaskGraph(
        tasks=[TaskNode(id="t1", title="A")],
        root_query="original query",
    )
    new_graph = replan(graph, completed=[], failed_task=None)
    assert new_graph.root_query == "original query"


def test_replan_mode_preserved():
    """mode 字段保留。"""
    graph = TaskGraph(
        tasks=[TaskNode(id="t1", title="A")],
        root_query="x",
        mode="interactive",
    )
    new_graph = replan(graph, completed=[], failed_task=None)
    assert new_graph.mode == "interactive"


def test_replan_preserves_verify_hint():
    """verify_hint 保留。"""
    vh = VerifyHint(kind="command", command="echo", expect="exit_0")
    graph = TaskGraph(
        tasks=[TaskNode(id="t1", title="A", verify_hint=vh)],
        root_query="x",
    )
    new_graph = replan(graph, completed=[], failed_task=None)
    assert new_graph.tasks[0].verify_hint == vh


def test_replan_validates_output():
    """replan 输出仍要通过 TaskGraph 校验。"""
    from agent_core.planner import _validate_plan

    graph = TaskGraph(
        tasks=[
            TaskNode(id="t1", title="A"),
            TaskNode(id="t2", title="B", deps=["t1"]),
        ],
        root_query="x",
    )
    new_graph = replan(
        graph,
        completed=[CompletedStep(task_id="t1", title="A", result_summary="ok")],
        failed_task=None,
    )
    _validate_plan(new_graph)  # 不抛


# ───────────────────────── 错误分类 ─────────────────────────


def test_classify_error_module_not_found():
    """ModuleNotFoundError → missing_module。"""
    assert classify_error("ModuleNotFoundError: No module named 'foo'") == "missing_module"


def test_classify_error_permission():
    """PermissionError → permission_denied。"""
    assert classify_error("PermissionError: [Errno 13] Permission denied") == "permission_denied"


def test_classify_error_file_not_found():
    """FileNotFoundError → file_missing。"""
    assert classify_error("FileNotFoundError: [Errno 2] No such file") == "file_missing"


def test_classify_error_assertion():
    """AssertionError → assert_failed。"""
    assert classify_error("AssertionError: 1 != 2") == "assert_failed"


def test_classify_error_timeout():
    """timeout → timeout。"""
    assert classify_error("TimeoutError") == "timeout"
    assert classify_error("执行超时 60s") == "timeout"


def test_classify_error_command_failed():
    """exit_code 非零 → command_failed。"""
    assert classify_error("CommandError: exit_code=1") == "command_failed"


def test_classify_error_unknown():
    """未知错误 → unknown。"""
    assert classify_error("random error") == "unknown"
    assert classify_error("") == "unknown"


# ───────────────────────── Policy ─────────────────────────


def test_replan_policy_defaults():
    """ReplanPolicy 默认值。"""
    p = ReplanPolicy()
    assert p.trigger_step_threshold == 3
    assert p.max_replans_per_graph == 5
    assert p.enable_env_drift_check is True


def test_replan_policy_from_dict():
    """from_dict 解析 TOML 配置。"""
    p = ReplanPolicy.from_dict({
        "trigger_step_threshold": 5,
        "max_replans_per_graph": 10,
    })
    assert p.trigger_step_threshold == 5
    assert p.max_replans_per_graph == 10
    assert p.enable_env_drift_check is True  # default


def test_replan_policy_from_empty_dict():
    """from_dict(None) / from_dict({}) → 默认。"""
    assert ReplanPolicy.from_dict(None).trigger_step_threshold == 3
    assert ReplanPolicy.from_dict({}).max_replans_per_graph == 5
