"""Mock execute_tool — Phase 1 端到端测试用。

Phase 2 替换为真透传到 Rust 薄壳的 executor（stdin/stdout JSON-RPC → Rust execute_tool）。

Phase 1 用法：
    executor = RecordingExecutor(responses={
        "list_tasks": "Found 3 tasks: [<UUID>, ...]",
        "query_single_task": "Task <UUID>: title='x', done=false",
    })
    result = executor.execute("list_tasks", {})
    assert executor.calls[0]["name"] == "list_tasks"
"""
from __future__ import annotations

from typing import Any, Callable, Dict, List, Optional, Union


class RecordingExecutor:
    """记录所有调用 + 返回预设响应。

    Phase 1 简化版 mock；Phase 2 替换为真 IPC executor。
    """

    def __init__(
        self,
        responses: Optional[Dict[str, Union[str, Callable[[Dict[str, Any]], str]]]] = None,
    ):
        """初始化。

        Args:
            responses: tool_name → 响应。响应可以是字符串（直接返回）
                        或 callable（接收 args，返回响应文本）。
        """
        self.responses = responses or {}
        self.calls: List[Dict[str, Any]] = []

    def execute(self, name: str, args: Any) -> str:
        """执行 mock tool。"""
        self.calls.append({"name": name, "args": args})
        if name not in self.responses:
            return f"mock result for {name}({args})"
        resp = self.responses[name]
        if callable(resp):
            return resp(args if isinstance(args, dict) else {})
        return resp

    def last_call(self) -> Optional[Dict[str, Any]]:
        """最近一次调用。"""
        return self.calls[-1] if self.calls else None

    def reset(self) -> None:
        """清空调用记录（不响应）。"""
        self.calls = []
        self.responses = {}
