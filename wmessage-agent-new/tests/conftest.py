"""共享 fixtures / helpers。"""
from __future__ import annotations

import io
import json
import os
import sys
import time
from typing import Any, Dict, List, Optional, Union

import pytest

# 把项目根加到 path，确保 `from agent_core.xxx import ...` 能跑
PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if PROJECT_ROOT not in sys.path:
    sys.path.insert(0, PROJECT_ROOT)


# ───────────────────────── Mock LLM ─────────────────────────


class MockLLM:
    """可编程的 mock LLM 客户端。

    用法：
        llm = MockLLM()
        llm.queue_response("Thought: ...")
        llm.queue_response("Thought: ...")
        # 每次 complete() 弹一个；用完抛 IndexError
    """

    def __init__(self, responses: Optional[List[str]] = None):
        self.responses = list(responses or [])
        self.calls: List[str] = []

    def queue_response(self, text: str) -> None:
        self.responses.append(text)

    def complete(self, prompt: str, system: Optional[str] = None) -> Any:
        from agent_core.react_step import LLMResponse

        self.calls.append(prompt)
        if not self.responses:
            raise IndexError("MockLLM 响应队列已空")
        return LLMResponse(text=self.responses.pop(0), prompt_tokens=len(prompt))


# ───────────────────────── Mock VerifyExecutor ─────────────────────────


class MockVerifyExecutor:
    """可编程的 mock VerifyExecutor。"""

    def __init__(
        self,
        command_result: Optional[Any] = None,
        file_hash_result: Optional[str] = None,
        assert_result: Any = None,
        llm_judge_result: Optional[tuple] = None,
        execute_result: Optional[Union[str, Callable[[str, Any], str]]] = None,
        execute_results: Optional[Dict[str, str]] = None,
    ):
        from agent_core.verify import CommandResult

        self.command_result = command_result or CommandResult(0, "", "")
        self.file_hash_result = file_hash_result or "deadbeef" * 8
        self.assert_result = assert_result if assert_result is not None else True
        self.llm_judge_result = llm_judge_result or (False, 0.0, "mock")
        # execute() 可返回固定字符串 / 按 tool_name 返回不同响应 / callable
        self.execute_result = execute_result
        self.execute_results = execute_results or {}
        self.calls: List[Dict[str, Any]] = []

    def run_command(self, cmd: str, timeout_s: int = 60) -> Any:
        self.calls.append({"method": "run_command", "cmd": cmd, "timeout_s": timeout_s})
        return self.command_result

    def file_hash(self, path: str) -> str:
        self.calls.append({"method": "file_hash", "path": path})
        return self.file_hash_result

    def eval_assert(self, expression: str) -> Any:
        self.calls.append({"method": "eval_assert", "expression": expression})
        return self.assert_result

    def llm_judge(self, rubric: str, output: str, threshold: float) -> tuple:
        self.calls.append(
            {"method": "llm_judge", "rubric": rubric, "threshold": threshold}
        )
        return self.llm_judge_result

    def execute(self, name: str, args: Any) -> str:
        self.calls.append({"method": "execute", "name": name, "args": args})
        if callable(self.execute_result):
            return self.execute_result(name, args)
        if name in self.execute_results:
            return self.execute_results[name]
        if self.execute_result is not None:
            return self.execute_result
        return f"mock execute {name}({args})"


# ───────────────────────── IPC 测试 helpers ─────────────────────────


class IPCTestHarness:
    """驱动 IPCServer 的测试辅助 — 用 io.StringIO 模拟 stdin/stdout。"""

    def __init__(self, server: Any):
        self.server = server
        self.stdin = io.StringIO()
        self.stdout = io.StringIO()
        self.stderr = io.StringIO()

    def send_request(
        self,
        method: str,
        params: Optional[Dict[str, Any]] = None,
        id: Any = 1,
    ) -> None:
        msg = {"jsonrpc": "2.0", "method": method, "params": params or {}, "id": id}
        self.stdin.write(json.dumps(msg) + "\n")

    def send_event(self, event: str, **kwargs: Any) -> None:
        msg = {"jsonrpc": "2.0", "method": "event", "params": {"event": event, **kwargs}}
        self.stdin.write(json.dumps(msg) + "\n")

    def send_raw(self, raw: str) -> None:
        self.stdin.write(raw + "\n")

    def run(self) -> None:
        self.stdin.seek(0)
        self.server.run(self.stdin, self.stdout, self.stderr)

    def read_lines(self) -> List[Dict[str, Any]]:
        self.stdout.seek(0)
        out = []
        for line in self.stdout.read().splitlines():
            if line.strip():
                out.append(json.loads(line))
        return out

    def responses(self) -> List[Dict[str, Any]]:
        """只返回 response（带 id 的）。"""
        return [line for line in self.read_lines() if "id" in line and "event" not in line]

    def events(self) -> List[Dict[str, Any]]:
        """只返回 event。"""
        return [line for line in self.read_lines() if "event" in line]
