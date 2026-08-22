"""wmessage-agent-new 进程入口。

启动 → emit_event("ready") → 进入 JSON-RPC 主循环。

Phase 2 实现：
- chat: 调 planner 生成 TaskGraph → 接入 AgentLoop 跑 PREV 主循环 → 返回完整结果
- tool.execute: stub "not implemented in Phase 2"
- ping: 返回 pong
- shutdown: 设置 server.stop()，主循环退出
- event: 接收客户端事件推送（仅记录 stderr）

Phase 1 → Phase 2 升级点：
- chat handler 不再只返回 PlanResult，而是接 AgentLoop 跑 PREV 主循环
- 注入 LLM client（Phase 2 mock，Phase 3 接入 MiniMax-M3）
- 注入 executor（Phase 2 mock 返回已知响应，Phase 3 接入 IPC executor）
- 注入 AuditLogger（Phase 2 内存模式，Phase 3 落 SQLite）

Phase 3 扩展：
- intent_router 前置路由
- Skill body 加载 + 真正 Skill 调用
- llm_provider/ 接入真实模型
- tool.execute 真正执行（透传 Rust 薄壳）
"""
from __future__ import annotations

import os
import sys
import time
from typing import Any, Dict, Optional

from ipc.server import IPCServer
from agent_core.agent_loop import AgentLoop, AgentLoopConfig, LoopStatus
from agent_core.audit_log import AuditLogger
from agent_core.db import connect, migrate
from agent_core.intent_router import route_intent
from agent_core.planner import (
    plan,
    PlanError,
    PlanLLMError,
    PlanValidationError,
)
from agent_core.react_step import LLMClient, LLMResponse
from agent_core.replanner import ReplanPolicy
from agent_core.types import TaskGraph
from agent_core.verify import LocalVerifyExecutor


# ───────────────────────── LLM / Executor ─────────────────────────


class ScriptedLLM(LLMClient):
    """Phase 2 mock LLM — 按队列返回 ReAct 响应。

    Phase 3 替换为 MiniMax-M3 / Kimi / DeepSeek 实现。
    """

    def __init__(self, responses: Optional[list] = None):
        self.responses = list(responses or [])
        self.calls = []

    def queue(self, text: str) -> None:
        self.responses.append(text)

    def complete(self, prompt: str, system: Optional[str] = None) -> LLMResponse:
        self.calls.append(prompt)
        if not self.responses:
            # 默认 fallback：task_done
            return LLMResponse(
                text="Thought: ok\nAction: task_done\nAction Input: done",
                prompt_tokens=len(prompt),
            )
        return LLMResponse(text=self.responses.pop(0), prompt_tokens=len(prompt))


class MockExecutor(LocalVerifyExecutor):
    """Phase 2 mock executor — 同时支持 ReAct tool.execute + VerifyExecutor 4 种 verify。

    - execute(name, args) → ReAct 工具调用（预设响应或 fallback mock 字符串）
    - run_command / file_hash / eval_assert / llm_judge → 继承 LocalVerifyExecutor 的真实实现
      （run_command 真跑 subprocess，verify 走默认 `echo "task done"` 能通过）

    Phase 3 替换为 IPCExecutor（stdin/stdout JSON-RPC → Rust execute_tool）。
    """

    def __init__(
        self,
        tool_responses: Optional[Dict[str, str]] = None,
    ):
        super().__init__()
        self.tool_responses = tool_responses or {}
        self.tool_calls: list = []

    def execute(self, name: str, args: Any) -> str:
        """ReAct 工具调用 — 预设响应或 fallback。"""
        self.tool_calls.append({"name": name, "args": args})
        if name in self.tool_responses:
            return self.tool_responses[name]
        # 已知 tool 给 mock 数据；未知返回 fallback
        if name == "list_tasks":
            return "Found 1 task: [id=00000000-0000-4000-8000-000000000001]"
        if name == "query_single_task":
            task_id = args.get("id", "") if isinstance(args, dict) else ""
            return f"Task {task_id}: title='mock', done=false"
        return f"mock execute {name}({args})"


# ───────────────────────── Handlers ─────────────────────────


# server 实例用于 handle_shutdown 设置 stop flag
_server_ref: dict = {}


def handle_chat(params: dict) -> dict:
    """主 chat 入口 — PREV 主循环 + Intent Router。

    Phase 2 流程：
    1. intent_router 关键词 L1 → 命中 builtin skill（minimax-docx/xlsx/pptx）
       命中 → tools_whitelist = skill.tools_whitelist, skill_body = skill descr
       未命中 → tools_whitelist = None（LLM 全开）
    2. planner 生成 TaskGraph（heuristic + 可选 LLM），skill_body 注入
    3. AgentLoop 跑 PREV 主循环（Plan → Execute → Replan → Verify）
    4. 返回完整结果（plan + loop_result + audit_lines + routed_skill）

    Args:
        params.message: 用户输入
        params.context: 上下文（文件树 / git / 报错）
        params.skillBody: 显式注入 skill body（覆盖 intent_router）
        params.skillName: Skill 名（用于审计）
        params.sessionId: 会话 ID（用于 DB 持久化）
        params.maxTasks: 最大任务数（默认 20）

    Returns:
        dict (含 plan / loop_result / audit_lines / routed_skill) 或 {"error": ..., "kind": ...}
    """
    message = params.get("message", "")
    context = params.get("context", {}) or {}
    explicit_skill_body = params.get("skillBody")
    skill_name = params.get("skillName", "user_query")
    session_id = params.get("sessionId")
    max_tasks = params.get("maxTasks", 20)

    # Phase 2: intent routing (builtin skills)
    routed_skill = None
    tools_whitelist = None
    skill_body = explicit_skill_body  # 显式传入优先

    if not skill_body:
        routed_skill = route_intent(message)
        if routed_skill:
            tools_whitelist = routed_skill.tools_whitelist
            skill_body = (
                f"# {routed_skill.name} (内置 Skill)\n\n"
                f"{routed_skill.description}\n\n"
                f"可用工具：{', '.join(routed_skill.tools_whitelist)}\n"
                f"最大 ReAct 步数：{routed_skill.react_max_steps}\n"
                f"超时：{routed_skill.timeout_s}s\n"
            )
            skill_name = routed_skill.name  # audit 标记

    # Plan
    try:
        plan_result = plan(query=message, context=context, skill_body=skill_body, max_tasks=max_tasks)
    except PlanValidationError as e:
        return {"error": str(e), "kind": "PlanValidationError"}
    except PlanLLMError as e:
        return {"error": str(e), "kind": "PlanLLMError"}
    except PlanError as e:
        return {"error": str(e), "kind": "PlanError"}
    except ValueError as e:
        return {"error": str(e), "kind": "ValueError"}

    # Wire AgentLoop
    llm = params.get("_llm") or ScriptedLLM()
    executor = params.get("_executor") or MockExecutor()

    audit = AuditLogger()  # 内存模式（不落盘）

    config = AgentLoopConfig(
        react_max_steps=10,
        max_task_attempts=2,
        replan_policy=ReplanPolicy(),
        enable_replan=True,
    )

    loop = AgentLoop(llm=llm, executor=executor, audit=audit, config=config)

    # Run PREV
    loop_result = loop.run(
        plan_result.graph,
        env_state=context,
        tools_whitelist=tools_whitelist,
        skill_name=skill_name,
    )

    return {
        "plan": plan_result.to_dict(),
        "reasoning": plan_result.reasoning,
        "routed_skill": routed_skill.name if routed_skill else None,
        "loop": {
            "status": loop_result.status,
            "completed_tasks": loop_result.completed_tasks,
            "failed_tasks": loop_result.failed_tasks,
            "replan_count": loop_result.replan_count,
            "duration_ms": loop_result.duration_ms,
            "ctx": loop_result.ctx,
            "error": loop_result.error,
        },
        "audit_lines": loop_result.audit_lines,
    }


def handle_tool_execute(params: dict) -> dict:
    """tool.execute — Phase 2 stub（与 Phase 1 一致）。

    Phase 3 接入：调 IPC executor 透传 Rust 侧。
    """
    return {
        "error": "tool.execute not implemented in Phase 2",
        "phase": 2,
        "tool": params.get("name"),
        "args": params.get("args"),
    }


def handle_ping(params: dict) -> dict:
    """心跳。"""
    return {"pong": True, "ts": int(time.time() * 1000)}


def handle_shutdown(params: dict) -> dict:
    """优雅退出。"""
    server = _server_ref.get("server")
    if server is not None:
        server.stop()
    return {"status": "shutting_down"}


def handle_event_push(params: dict) -> None:
    """Phase 2 占位：客户端通过 'event' method 推事件，本地仅记录。"""
    print(f"[event] {params}", file=sys.stderr)


# ───────────────────────── main ─────────────────────────


HANDLERS = {
    "chat": handle_chat,
    "tool.execute": handle_tool_execute,
    "ping": handle_ping,
    "shutdown": handle_shutdown,
}


def main() -> int:
    server = IPCServer(
        handlers=HANDLERS,
        ready_payload={"version": "0.2.0", "phase": 2, "pid": __import__("os").getpid()},
        on_event=handle_event_push,
    )
    _server_ref["server"] = server
    try:
        server.run()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
