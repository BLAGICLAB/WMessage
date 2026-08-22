"""react_step 测试 — parse + validate + react_step 主入口。"""
from __future__ import annotations

import pytest

from agent_core.react_step import (
    react_step,
    parse_action,
    validate_action,
    LLMResponse,
    ReactParseError,
    ReactValidationError,
    ReactStepError,
)
from agent_core.types import (
    ReactState,
    TaskNode,
    ActionKind,
)
from tests.conftest import MockLLM


# ───────────────────────── parse_action ─────────────────────────


def test_parse_tool_call_valid():
    """典型 tool_call 解析。"""
    text = """Thought: I need to read the file first.
Action: tool_call
Action Input: {"name": "read_file", "args": {"path": "src/main.py"}}"""
    a = parse_action(text)
    assert a.kind == ActionKind.TOOL_CALL.value
    assert a.tool_name == "read_file"
    assert a.tool_args == {"path": "src/main.py"}
    assert "read the file" in a.thought.lower()


def test_parse_task_done_valid():
    """task_done 解析。"""
    text = """Thought: All done.
Action: task_done
Action Input: Added 3 unit tests, all passing."""
    a = parse_action(text)
    assert a.kind == ActionKind.TASK_DONE.value
    assert "3 unit tests" in a.result


def test_parse_ask_user_valid():
    """ask_user 解析。"""
    text = """Thought: I need clarification.
Action: ask_user
Action Input: Which API version are you targeting?"""
    a = parse_action(text)
    assert a.kind == ActionKind.ASK_USER.value
    assert "API version" in a.question


def test_parse_missing_thought_still_parses():
    """缺 Thought 行也能解析（thought 为空）。"""
    text = """Action: task_done
Action Input: All good"""
    a = parse_action(text)
    assert a.kind == ActionKind.TASK_DONE.value
    assert a.thought == ""


def test_parse_missing_action_raises():
    """缺 Action 行 → ReactParseError。"""
    text = """Thought: hmm
Action Input: nothing"""
    with pytest.raises(ReactParseError, match="Action"):
        parse_action(text)


def test_parse_empty_raises():
    """空字符串 → ReactParseError。"""
    with pytest.raises(ReactParseError):
        parse_action("")


def test_parse_whitespace_only_raises():
    """纯空白 → ReactParseError。"""
    with pytest.raises(ReactParseError):
        parse_action("   \n\t  ")


def test_parse_tool_call_bad_json_raises():
    """tool_call Action Input 不是 JSON → ReactParseError。"""
    text = """Thought: x
Action: tool_call
Action Input: not json"""
    with pytest.raises(ReactParseError, match="JSON"):
        parse_action(text)


def test_parse_tool_call_non_dict_json_raises():
    """tool_call Action Input 是 JSON 但不是 object → ReactParseError。"""
    text = """Thought: x
Action: tool_call
Action Input: [1, 2, 3]"""
    with pytest.raises(ReactParseError, match="object"):
        parse_action(text)


def test_parse_unknown_action_kind_raises():
    """未知 Action kind → ReactParseError。"""
    text = """Thought: x
Action: fly_to_moon
Action Input: somewhere"""
    with pytest.raises(ReactParseError, match="未知 Action kind"):
        parse_action(text)


def test_parse_multiline_input():
    """Action Input 跨多行。"""
    text = """Thought: complex
Action: task_done
Action Input: 第一行
第二行
第三行"""
    a = parse_action(text)
    assert "第一行" in a.result
    assert "第三行" in a.result


# ───────────────────────── validate_action ─────────────────────────


def test_validate_tool_call_empty_name_raises():
    """tool_call.name 空 → ValidationError。"""
    from agent_core.types import ReactAction

    a = ReactAction.tool_call(name="", args={})
    with pytest.raises(ReactValidationError, match="tool_name"):
        validate_action(a)


def test_validate_tool_call_not_in_whitelist_raises():
    """tool 不在白名单 → ValidationError。"""
    from agent_core.types import ReactAction

    a = ReactAction.tool_call(name="rm_rf", args={"path": "/"})
    with pytest.raises(ReactValidationError, match="白名单"):
        validate_action(a, tools_whitelist=["read_file", "write_file"])


def test_validate_tool_call_in_whitelist_passes():
    """tool 在白名单 → 通过。"""
    from agent_core.types import ReactAction

    a = ReactAction.tool_call(name="read_file", args={"path": "x"})
    validate_action(a, tools_whitelist=["read_file", "write_file"])


def test_validate_tool_call_no_whitelist_passes():
    """不传白名单 → 任何 tool 都允许。"""
    from agent_core.types import ReactAction

    a = ReactAction.tool_call(name="anything", args={})
    validate_action(a)  # 不抛


def test_validate_tool_call_non_dict_args_raises():
    """tool_args 不是 dict → ValidationError。"""
    from agent_core.types import ReactAction

    a = ReactAction(kind=ActionKind.TOOL_CALL.value, tool_name="x", tool_args="not a dict")
    with pytest.raises(ReactValidationError, match="dict"):
        validate_action(a)


def test_validate_task_done_non_str_result_raises():
    """task_done.result 非 str → ValidationError。"""
    from agent_core.types import ReactAction

    a = ReactAction(kind=ActionKind.TASK_DONE.value, result=123)
    with pytest.raises(ReactValidationError, match="str"):
        validate_action(a)


def test_validate_ask_user_empty_question_raises():
    """ask_user.question 空 → ValidationError。"""
    from agent_core.types import ReactAction

    a = ReactAction.ask_user(question="   ")
    with pytest.raises(ReactValidationError, match="question"):
        validate_action(a)


def test_validate_unknown_kind_raises():
    """未知 kind → ValidationError。"""
    from agent_core.types import ReactAction

    a = ReactAction(kind="magic")
    with pytest.raises(ReactValidationError, match="未知 kind"):
        validate_action(a)


# ───────────────────────── react_step 主入口 ─────────────────────────


def test_react_step_parses_tool_call():
    """react_step: mock LLM 返回合法 tool_call → 返回 tool_call action。"""
    llm = MockLLM(
        responses=[
            """Thought: read it
Action: tool_call
Action Input: {"name": "read_file", "args": {"path": "x"}}"""
        ]
    )
    state = ReactState(task=TaskNode(id="t1", title="read x"), step=1)
    a = react_step(state, llm)
    assert a.kind == ActionKind.TOOL_CALL.value
    assert a.tool_name == "read_file"


def test_react_step_parses_task_done():
    """react_step: mock LLM 返回 task_done → 返回 task_done action。"""
    llm = MockLLM(
        responses=[
            """Thought: done
Action: task_done
Action Input: all tests passed"""
        ]
    )
    state = ReactState(task=TaskNode(id="t1", title="x"), step=1)
    a = react_step(state, llm)
    assert a.kind == ActionKind.TASK_DONE.value
    assert "all tests passed" in a.result


def test_react_step_parse_fail_falls_back_to_ask_user():
    """react_step: LLM 输出无法解析 → fallback ask_user（不抛）。"""
    llm = MockLLM(responses=["this is not a valid action"])
    state = ReactState(task=TaskNode(id="t1", title="x"), step=1)
    a = react_step(state, llm)
    assert a.kind == ActionKind.ASK_USER.value
    assert "无法解析" in a.question


def test_react_step_validation_fail_returns_ask_user_with_error():
    """react_step: 解析成功但校验失败 → ask_user 带错误信息。"""
    llm = MockLLM(
        responses=[
            """Thought: x
Action: tool_call
Action Input: {"name": "dangerous_tool", "args": {}}"""
        ]
    )
    state = ReactState(task=TaskNode(id="t1", title="x"), step=1)
    a = react_step(state, llm, tools_whitelist=["safe_tool"])
    assert a.kind == ActionKind.ASK_USER.value
    assert "白名单" in a.question


def test_react_step_step_less_than_1_raises():
    """step < 1 → ReactStepError。"""
    state = ReactState(task=TaskNode(id="t1", title="x"), step=0)
    llm = MockLLM()
    with pytest.raises(ReactStepError, match="step"):
        react_step(state, llm)


def test_react_step_env_state_injected_into_prompt():
    """env_state 注入 prompt。"""
    llm = MockLLM(
        responses=[
            """Thought: ok
Action: task_done
Action Input: x"""
        ]
    )
    state = ReactState(
        task=TaskNode(id="t1", title="x"),
        env_state={"files": "src/main.py", "git_status": "clean"},
        step=1,
    )
    react_step(state, llm)
    prompt = llm.calls[0]
    assert "src/main.py" in prompt
    assert "clean" in prompt


def test_react_step_history_window_respected():
    """history 超出 window 不注入。"""
    llm = MockLLM(
        responses=[
            """Thought: ok
Action: task_done
Action Input: x"""
        ]
    )
    history = [
        {"step": i, "action_kind": "tool_call", "result": f"step-{i}-out"}
        for i in range(1, 11)
    ]
    state = ReactState(
        task=TaskNode(id="t1", title="x"),
        history=history,
        step=11,
    )
    react_step(state, llm, history_window=3)
    prompt = llm.calls[0]
    # 最近 3 步：step 8, 9, 10
    assert "step-8-out" in prompt
    assert "step-9-out" in prompt
    assert "step-10-out" in prompt
    # 早期 step 不应出现
    assert "step-1-out" not in prompt


def test_react_step_title_in_prompt():
    """task.title 注入 prompt。"""
    llm = MockLLM(
        responses=[
            """Thought: ok
Action: task_done
Action Input: x"""
        ]
    )
    state = ReactState(task=TaskNode(id="t1", title="write a hello world"), step=1)
    react_step(state, llm)
    assert "write a hello world" in llm.calls[0]


def test_react_step_verify_hint_in_prompt():
    """task.verify_hint 注入 prompt。"""
    llm = MockLLM(
        responses=[
            """Thought: ok
Action: task_done
Action Input: x"""
        ]
    )
    from agent_core.types import VerifyHint

    state = ReactState(
        task=TaskNode(
            id="t1", title="x", verify_hint=VerifyHint(kind="command", command="pytest")
        ),
        step=1,
    )
    react_step(state, llm)
    assert "command" in llm.calls[0]
    assert "pytest" in llm.calls[0]


def test_react_step_calls_llm_once():
    """react_step: 单步只调一次 LLM。"""
    llm = MockLLM(
        responses=[
            """Thought: ok
Action: task_done
Action Input: x"""
        ]
    )
    state = ReactState(task=TaskNode(id="t1", title="x"), step=1)
    react_step(state, llm)
    assert len(llm.calls) == 1


def test_react_step_thought_passed_through():
    """thought 从 LLM 透传到 action.thought。"""
    llm = MockLLM(
        responses=[
            """Thought: I will run tests
Action: task_done
Action Input: x"""
        ]
    )
    state = ReactState(task=TaskNode(id="t1", title="x"), step=1)
    a = react_step(state, llm)
    assert "run tests" in a.thought
