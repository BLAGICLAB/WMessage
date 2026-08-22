"""react_step — ReAct 单步逻辑（思考 / 工具 / 观察）。

Phase 1 实现：
- react_step() 主入口：构造 prompt → 调 LLM → 解析输出 → 返回 ReactAction
- parse_action()：从 LLM 文本解析 action（tool_call / task_done / ask_user）
- validate_action()：校验 action 合法性（tool_name 白名单 / args schema）

LLM 输出格式（ReAct 风格，参考 ReAct 论文）：
    Thought: <reasoning>
    Action: tool_call
    Action Input: {"name": "read_file", "args": {"path": "src/main.py"}}

    或：
    Thought: <reasoning>
    Action: task_done
    Action Input: <result text>

    或：
    Thought: <reasoning>
    Action: ask_user
    Action Input: <question>

设计约束（来自 wmessage-agent-spec.md §X.2.2 阶段 2）：
- 每个 task 默认 30 轮 ReAct 上限（harness_policy.toml 可调）
- tool_name 必须在 tools_whitelist 内（Skill body 限定）
- action 解析失败 → fallback ask_user（不抛异常，让主循环决定）
"""
from __future__ import annotations

import json
import re
from dataclasses import dataclass
from typing import Any, Callable, Dict, List, Optional

from agent_core.types import ActionKind, ReactAction, ReactState


# ───────────────────────── Exceptions ─────────────────────────


class ReactStepError(Exception):
    """react_step 错误基类。"""


class ReactParseError(ReactStepError):
    """LLM 输出无法解析为 action。"""


class ReactValidationError(ReactStepError):
    """action 校验失败（tool 不在白名单 / args 缺字段）。"""


# ───────────────────────── LLM Client 协议 ─────────────────────────


@dataclass
class LLMResponse:
    """LLM 调用的最小返回结构。"""

    text: str
    prompt_tokens: int = 0
    completion_tokens: int = 0


class LLMClient:
    """LLM 客户端协议。测试时可注入 mock。

    Phase 1 用法：mock_llm = lambda prompt: LLMResponse(text="Thought: ...\nAction: ...")
    Phase 2 接入 MiniMax-M3 实现完整 streaming + tool_use。
    """

    def complete(self, prompt: str, system: Optional[str] = None) -> LLMResponse:
        raise NotImplementedError


# ───────────────────────── 主入口 ─────────────────────────


def react_step(
    state: ReactState,
    llm: LLMClient,
    tools_whitelist: Optional[List[str]] = None,
    history_window: int = 10,
) -> ReactAction:
    """单步 ReAct：think → choose action。

    Args:
        state: 当前 ReactState（task / env_state / history / step）
        llm: LLM 客户端
        tools_whitelist: 允许的 tool_name 列表（None = 全允许）
        history_window: 注入 prompt 的最近 history 步数

    Returns:
        ReactAction（tool_call / task_done / ask_user）

    Note:
        LLM 解析失败时 fallback ask_user（不让主循环崩）
    """
    if state.step < 1:
        from agent_core.types import TaskNode
        # 合法但 raise 让调用方知道
        raise ReactStepError(f"step 必须 ≥ 1，实际 {state.step}")

    prompt = _build_react_prompt(state, history_window)
    response = llm.complete(prompt)

    try:
        action = parse_action(response.text)
    except ReactParseError:
        # fallback：让用户帮忙
        return ReactAction.ask_user(
            question="LLM 输出无法解析，请换种方式描述需求",
            thought="[parse_failed] " + response.text[:200],
        )

    try:
        validate_action(action, tools_whitelist)
    except ReactValidationError as e:
        return ReactAction.ask_user(
            question=f"action 校验失败：{e}",
            thought=action.thought or "",
        )

    return action


# ───────────────────────── Prompt 构造 ─────────────────────────


_REACT_SYSTEM = """你是一个 ReAct agent。给定当前 task 和环境状态，按以下格式输出：

Thought: <你的推理>
Action: tool_call | task_done | ask_user
Action Input: <根据 Action 类型填>

- tool_call → Action Input 是 JSON {"name": "工具名", "args": {参数}}
- task_done → Action Input 是 任务结果文本
- ask_user → Action Input 是 给用户的问题
"""


def _build_react_prompt(state: ReactState, history_window: int) -> str:
    parts = [_REACT_SYSTEM, ""]
    parts.append(f"## 当前 Task")
    parts.append(f"id: {state.task.id}")
    parts.append(f"title: {state.task.title}")
    if state.task.verify_hint:
        vh = state.task.verify_hint
        # 按 kind 输出关键字段（不全部输出，prompt 太长）
        if vh.kind == "command" and vh.command:
            parts.append(f"verify: {vh.kind} | command: {vh.command} | expect: {vh.expect or 'exit_0'}")
        elif vh.kind == "assert" and vh.expression:
            parts.append(f"verify: {vh.kind} | expression: {vh.expression}")
        elif vh.kind == "file_hash" and vh.path:
            parts.append(f"verify: {vh.kind} | path: {vh.path}")
        elif vh.kind == "llm_judge" and vh.rubric:
            parts.append(f"verify: {vh.kind} | rubric: {vh.rubric}")
        else:
            parts.append(f"verify: kind={vh.kind}")
    parts.append("")

    # Phase 5: Reflexion 反思（历史任务经验）优先显示
    if state.env_state and "reflections" in state.env_state:
        parts.append(state.env_state["reflections"])
        parts.append("")

    if state.env_state:
        parts.append("## 环境状态")
        for k, v in state.env_state.items():
            if k == "reflections":
                continue  # 已在上方显示
            parts.append(f"- {k}: {_truncate(str(v), 200)}")
        parts.append("")

    if state.history:
        parts.append("## 历史 Step（最近 {} 步）".format(min(len(state.history), history_window)))
        for h in state.history[-history_window:]:
            parts.append(f"- step {h.get('step')}: action={h.get('action_kind')} result={_truncate(str(h.get('result', '')), 200)}")
        parts.append("")

    parts.append(f"## 当前 step")
    parts.append(f"step {state.step}，请输出 Thought + Action + Action Input：")
    return "\n".join(parts)


def _truncate(s: str, n: int) -> str:
    return s if len(s) <= n else s[:n] + "…"


# ───────────────────────── 输出解析 ─────────────────────────


_THOUGHT_RE = re.compile(r"^\s*Thought:\s*(.+?)(?=\n\s*Action:|\Z)", re.DOTALL | re.MULTILINE)
_ACTION_RE = re.compile(r"^\s*Action:\s*(\w+)\s*$", re.MULTILINE)
_ACTION_INPUT_RE = re.compile(r"^\s*Action Input:\s*(.+?)(?=\n\s*Thought:|\Z)", re.DOTALL | re.MULTILINE)


def parse_action(llm_text: str) -> ReactAction:
    """从 LLM 文本解析 ReactAction。

    解析规则：
    1. Thought: <text>
    2. Action: tool_call | task_done | ask_user
    3. Action Input: <JSON for tool_call> | <text for task_done/ask_user>

    Raises:
        ReactParseError: 解析失败
    """
    if not llm_text or not llm_text.strip():
        raise ReactParseError("LLM 输出为空")

    thought_match = _THOUGHT_RE.search(llm_text)
    action_match = _ACTION_RE.search(llm_text)
    input_match = _ACTION_INPUT_RE.search(llm_text)

    if not action_match:
        raise ReactParseError(f"未找到 Action 行：\n{llm_text[:200]}")

    kind = action_match.group(1).strip().lower()
    thought = thought_match.group(1).strip() if thought_match else ""
    raw_input = input_match.group(1).strip() if input_match else ""

    if kind == ActionKind.TOOL_CALL.value:
        try:
            data = json.loads(raw_input)
        except json.JSONDecodeError as e:
            raise ReactParseError(f"tool_call Action Input 不是 JSON：{e}\n原始：{raw_input[:100]}") from e
        if not isinstance(data, dict):
            raise ReactParseError("tool_call Action Input 必须是 JSON object")
        return ReactAction.tool_call(
            name=str(data.get("name", "")),
            args=dict(data.get("args", {})),
            thought=thought,
        )

    if kind == ActionKind.TASK_DONE.value:
        return ReactAction.task_done(result=raw_input, thought=thought)

    if kind == ActionKind.ASK_USER.value:
        return ReactAction.ask_user(question=raw_input, thought=thought)

    raise ReactParseError(f"未知 Action kind：{kind}")


# ───────────────────────── 校验 ─────────────────────────


def validate_action(
    action: ReactAction,
    tools_whitelist: Optional[List[str]] = None,
) -> None:
    """校验 action 合法性。

    校验项：
    - kind 必须是有效枚举
    - tool_call：name 非空 + 在白名单内
    - tool_call：args 必须是 dict
    - task_done：result 必须是 str
    - ask_user：question 必须是 str

    Raises:
        ReactValidationError: 校验失败
    """
    if action.kind not in {k.value for k in ActionKind}:
        raise ReactValidationError(f"未知 kind：{action.kind}")

    if action.kind == ActionKind.TOOL_CALL.value:
        if not action.tool_name:
            raise ReactValidationError("tool_call 必须有 tool_name")
        if tools_whitelist is not None and action.tool_name not in tools_whitelist:
            raise ReactValidationError(
                f"tool '{action.tool_name}' 不在白名单，允许：{tools_whitelist}"
            )
        if not isinstance(action.tool_args, dict):
            raise ReactValidationError(f"tool_args 必须是 dict，实际 {type(action.tool_args).__name__}")

    elif action.kind == ActionKind.TASK_DONE.value:
        if not isinstance(action.result, str):
            raise ReactValidationError("task_done.result 必须是 str")

    elif action.kind == ActionKind.ASK_USER.value:
        if not isinstance(action.question, str) or not action.question.strip():
            raise ReactValidationError("ask_user.question 必须是非空 str")
