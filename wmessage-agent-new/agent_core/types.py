"""共享 dataclass — 在 planner / verify / react_step / IPC 之间流转。"""
from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Dict, List, Optional


# ───────────────────────── Task Graph ─────────────────────────


@dataclass
class TaskNode:
    """DAG 节点 — 单一可验证单元。"""

    id: str
    title: str
    deps: List[str] = field(default_factory=list)  # 依赖的 task id 列表
    verify_hint: Optional["VerifyHint"] = None  # 校验提示

    def to_dict(self) -> Dict[str, Any]:
        return {
            "id": self.id,
            "title": self.title,
            "deps": list(self.deps),
            "verifyHint": self.verify_hint.to_dict() if self.verify_hint else None,
        }


@dataclass
class TaskGraph:
    """DAG 任务图 — planner 的输出。"""

    tasks: List[TaskNode]
    root_query: str
    mode: str = "auto"  # auto | interactive
    plan_version: int = 1

    def to_dict(self) -> Dict[str, Any]:
        return {
            "tasks": [t.to_dict() for t in self.tasks],
            "rootQuery": self.root_query,
            "mode": self.mode,
            "planVersion": self.plan_version,
        }


@dataclass
class PlanResult:
    """planner 的返回结果。"""

    graph: TaskGraph
    reasoning: str  # 规划思路（注入 system prompt）

    def to_dict(self) -> Dict[str, Any]:
        return {
            "graph": self.graph.to_dict(),
            "reasoning": self.reasoning,
        }


# ───────────────────────── Verify ─────────────────────────


class VerifyKind(str, Enum):
    """verify_hint.kind 枚举。"""

    COMMAND = "command"  # 强校验：跑命令，按退出码/输出判定
    FILE_HASH = "file_hash"  # 强校验：比对文件 SHA256
    ASSERT = "assert"  # 强校验：求值 Python 表达式
    LLM_JUDGE = "llm_judge"  # 弱校验：LLM 打分


@dataclass
class VerifyHint:
    """task 的 verify 提示。"""

    kind: str  # VerifyKind value
    command: Optional[str] = None  # kind=command
    expect: Optional[str] = None  # kind=command: "exit_0" | regex
    path: Optional[str] = None  # kind=file_hash
    sha256: Optional[str] = None  # kind=file_hash
    expression: Optional[str] = None  # kind=assert
    rubric: Optional[str] = None  # kind=llm_judge
    threshold: Optional[float] = None  # kind=llm_judge: 0-1

    def to_dict(self) -> Dict[str, Any]:
        return {k: v for k, v in self.__dict__.items() if v is not None}

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "VerifyHint":
        return cls(
            kind=d["kind"],
            command=d.get("command"),
            expect=d.get("expect"),
            path=d.get("path"),
            sha256=d.get("sha256"),
            expression=d.get("expression"),
            rubric=d.get("rubric"),
            threshold=d.get("threshold"),
        )


@dataclass
class VerifyResult:
    """verify() 的返回。"""

    passed: bool
    output: str  # 人类可读
    evidence: str = ""  # 机器可校验的证据
    error: Optional[str] = None  # 错误信息（如有）
    kind: str = ""  # VerifyKind value
    duration_ms: int = 0

    def to_dict(self) -> Dict[str, Any]:
        return {
            "passed": self.passed,
            "output": self.output,
            "evidence": self.evidence,
            "error": self.error,
            "kind": self.kind,
            "durationMs": self.duration_ms,
        }


# ───────────────────────── React Step ─────────────────────────


class ActionKind(str, Enum):
    """react_step 的动作类型。"""

    TOOL_CALL = "tool_call"  # 调工具
    TASK_DONE = "task_done"  # 当前 task 完成
    ASK_USER = "ask_user"  # 问用户


@dataclass
class ReactState:
    """ReAct 当前状态。"""

    task: TaskNode
    env_state: Dict[str, Any] = field(default_factory=dict)  # 文件/日志/git
    history: List[Dict[str, Any]] = field(default_factory=list)  # 历史 step
    step: int = 0  # 当前 step 号（从 1 开始）

    def to_dict(self) -> Dict[str, Any]:
        return {
            "task": self.task.to_dict(),
            "envState": self.env_state,
            "history": list(self.history),
            "step": self.step,
        }


@dataclass
class ReactAction:
    """react_step 的返回动作。"""

    kind: str  # ActionKind value
    # tool_call 字段
    tool_name: Optional[str] = None
    tool_args: Optional[Dict[str, Any]] = None
    thought: Optional[str] = None  # LLM 思考链
    # task_done 字段
    result: Optional[str] = None
    # ask_user 字段
    question: Optional[str] = None

    def to_dict(self) -> Dict[str, Any]:
        d: Dict[str, Any] = {"kind": self.kind}
        if self.tool_name is not None:
            d["toolName"] = self.tool_name
        if self.tool_args is not None:
            d["toolArgs"] = self.tool_args
        if self.thought is not None:
            d["thought"] = self.thought
        if self.result is not None:
            d["result"] = self.result
        if self.question is not None:
            d["question"] = self.question
        return d

    @classmethod
    def tool_call(cls, name: str, args: Dict[str, Any], thought: str = "") -> "ReactAction":
        return cls(kind=ActionKind.TOOL_CALL.value, tool_name=name, tool_args=args, thought=thought)

    @classmethod
    def task_done(cls, result: str, thought: str = "") -> "ReactAction":
        return cls(kind=ActionKind.TASK_DONE.value, result=result, thought=thought)

    @classmethod
    def ask_user(cls, question: str, thought: str = "") -> "ReactAction":
        return cls(kind=ActionKind.ASK_USER.value, question=question, thought=thought)
