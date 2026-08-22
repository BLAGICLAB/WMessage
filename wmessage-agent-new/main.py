"""wmessage-agent-new 进程入口。

启动 → init_runtime() 构建 LLM/Executor/Audit → emit_event("ready") → 进入 JSON-RPC 主循环。

## Phase 4（LLM 集成）

main.py 启动时根据环境变量 + feature_flags 构建真实 LLM chain：

```
API key 检测
├── 全有 → FallbackChain([MiniMax, Kimi, DeepSeek]) + LLMProviderToReactAdapter
├── 部分 → FallbackChain(能 init 的 provider) + adapter
└── 全无 → ScriptedLLM fallback（保证 dev/test 不挂）
```

可通过环境变量调整个例：
- MINIMAX_API_KEY / KIMI_API_KEY / DEEPSEEK_API_KEY
- WMSG_AGENT_FLAGS（feature_flags.toml 路径）
- WMSG_AGENT_DB（SQLite 路径，None = 仅文件）
- WMSG_AGENT_LOG（bot.log 路径）
- WMSG_AGENT_OUTPUT_DIR（doc_make_* 输出目录）

## 执行链路

Rust 薄壳 spawn 本进程 → stdin/stdout JSON-RPC
  ↓
intent_router 命中 builtin skill (L1 关键词)
  ↓
planner 生成 TaskGraph
  ↓
AgentLoop PREV 主循环
  ├─ Real LLM (FallbackChain) → react_step 决策
  ├─ RealToolExecutor.execute → python-docx 生成 .docx
  └─ AuditLogger → 文件 + SQLite 双写
"""
from __future__ import annotations

import os
import sys
import time
from typing import Any, Dict, List, Optional, Tuple

from ipc.server import IPCServer

# ───────────────────────── Agent Core ─────────────────────────
from agent_core.agent_loop import AgentLoop, AgentLoopConfig
from agent_core.audit_log import AuditLogger
from agent_core.audit_sqlite import SqliteAuditLogger
from agent_core.feature_flags import FlagChecker
from agent_core.intent_router import route_intent
from agent_core.logging_config import get_logger, new_cid, setup_logging
from agent_core.planner import (
    plan,
    PlanError,
    PlanLLMError,
    PlanValidationError,
)
from agent_core.react_step import LLMClient, LLMResponse
from agent_core.real_executor import RealToolExecutor
from agent_core.replanner import ReplanPolicy
from agent_core.verify import LocalVerifyExecutor

# ───────────────────────── LLM Provider ─────────────────────────
from llm_provider.base import (
    FallbackChain,
    LLMError,
    LLMProvider,
    LLMProviderToReactAdapter,
)
from llm_provider.minimax import MiniMaxProvider
from llm_provider.kimi import KimiProvider
from llm_provider.deepseek import DeepSeekProvider


logger = get_logger("wmsg.main")


# ───────────────────────── LLM / Executor / Audit Factory ─────────────────────────

# Provider 名称 → (env_var, class) 的映射
LLM_PROVIDER_REGISTRY: Dict[str, Tuple[str, type]] = {
    "minimax-m3": ("MINIMAX_API_KEY", MiniMaxProvider),
    "kimi": ("KIMI_API_KEY", KimiProvider),
    "deepseek": ("DEEPSEEK_API_KEY", DeepSeekProvider),
}


class ScriptedLLM(LLMClient):
    """Mock LLM — Phase 1-3 测试用。

    Phase 4：当 API key 全缺时作为 fallback（保证 dev/test 不挂）。
    """

    def __init__(self, responses: Optional[List[str]] = None):
        self.responses = list(responses or [])
        self.calls: List[str] = []

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


def build_llm_chain(flag_checker: FlagChecker) -> Tuple[LLMClient, Dict[str, Any]]:
    """构建 LLM chain — 按 feature_flags.primary → fallback 顺序。

    至少有 1 个 provider 能 init → FallbackChain 适配器（真实 LLM）
    全缺 API key → ScriptedLLM（fallback）

    Returns:
        (llm_client, info_dict)
        info_dict: {mode, primary, fallback, providers_loaded, providers_skipped, reason?}
    """
    primary = flag_checker.primary_llm()
    fallback_names = flag_checker.fallback_llms()

    # 顺序：primary → fallback（去重保序）
    ordered: List[str] = []
    seen: set = set()
    for name in [primary] + list(fallback_names):
        if name and name not in seen:
            seen.add(name)
            ordered.append(name)

    providers: List[LLMProvider] = []
    skipped: List[Dict[str, str]] = []

    for name in ordered:
        if name not in LLM_PROVIDER_REGISTRY:
            skipped.append({"name": name, "reason": "unknown_provider"})
            continue
        env_key, cls = LLM_PROVIDER_REGISTRY[name]
        try:
            p = cls()  # 从环境变量自动读 API key
            providers.append(p)
        except LLMError as e:
            skipped.append({"name": name, "reason": str(e)[:100]})

    info: Dict[str, Any] = {
        "primary": primary,
        "fallback": fallback_names,
        "providers_loaded": [p.name for p in providers],
        "providers_skipped": skipped,
    }

    if providers:
        adapter = LLMProviderToReactAdapter(FallbackChain(providers))
        info["mode"] = "real"
        return adapter, info

    # 全缺：fallback ScriptedLLM
    info["mode"] = "mock"
    info["reason"] = "no_api_keys_or_init_failed"
    return ScriptedLLM(), info


def build_executor(flag_checker: FlagChecker) -> Tuple[LocalVerifyExecutor, Dict[str, Any]]:
    """构建 Executor — 默认 RealToolExecutor（真生成 .docx 等）。

    Phase 4：默认 RealToolExecutor（不再是 Phase 2 的 MockExecutor）。
    未来 Phase 5：可切 IPCExecutor（透传 Rust）。
    """
    output_dir = os.environ.get("WMSG_AGENT_OUTPUT_DIR", "data/output")
    return RealToolExecutor(output_dir=output_dir), {
        "mode": "real",
        "executor_class": "RealToolExecutor",
        "output_dir": output_dir,
    }


def build_audit(flag_checker: FlagChecker) -> Tuple[AuditLogger, Dict[str, Any]]:
    """构建 AuditLogger — 启用 audit_sqlite 或 WMSG_AGENT_DB 时双写。

    - flag_checker.audit_sqlite_enabled=True → SqliteAuditLogger（文件 + SQLite）
    - 否则 → AuditLogger（仅文件）
    """
    db_path = os.environ.get("WMSG_AGENT_DB")
    log_path = os.environ.get("WMSG_AGENT_LOG")

    if flag_checker.is_feature_enabled("audit_sqlite_enabled") or db_path:
        # 显式 db_path 优先
        if not db_path:
            db_path = "data/agent.db"
        return SqliteAuditLogger(path=log_path, db_path=db_path), {
            "mode": "sqlite+file",
            "db_path": db_path,
            "log_path": log_path or "(in-memory)",
        }

    return AuditLogger(path=log_path), {
        "mode": "file",
        "log_path": log_path or "(in-memory)",
    }


def get_flag_checker(path: Optional[str] = None) -> FlagChecker:
    """获取 FlagChecker — 默认 config/feature_flags.toml。

    可通过环境变量 WMSG_AGENT_FLAGS 覆盖路径。
    """
    if path is None:
        path = os.environ.get("WMSG_AGENT_FLAGS", "config/feature_flags.toml")
    return FlagChecker(path=path)


# ───────────────────────── Runtime Singleton ─────────────────────────

# init_runtime() 启动时填充。测试可调用 _reset_runtime() 重置。
_runtime: Dict[str, Any] = {
    "flag_checker": None,
    "llm_client": None,
    "llm_info": None,
    "executor": None,
    "executor_info": None,
    "audit": None,
    "audit_info": None,
}


def init_runtime(flag_path: Optional[str] = None) -> Dict[str, Any]:
    """初始化 runtime singletons — main() 启动时调用一次。

    Returns:
        dict: {llm, executor, audit} — 各组件的 info
    """
    flag_checker = get_flag_checker(flag_path)
    _runtime["flag_checker"] = flag_checker

    llm, llm_info = build_llm_chain(flag_checker)
    _runtime["llm_client"] = llm
    _runtime["llm_info"] = llm_info

    executor, exec_info = build_executor(flag_checker)
    _runtime["executor"] = executor
    _runtime["executor_info"] = exec_info

    audit, audit_info = build_audit(flag_checker)
    _runtime["audit"] = audit
    _runtime["audit_info"] = audit_info

    return {
        "llm": llm_info,
        "executor": exec_info,
        "audit": audit_info,
    }


def _reset_runtime() -> None:
    """测试 helper — 重置 runtime singletons。"""
    for k in list(_runtime.keys()):
        _runtime[k] = None


# ───────────────────────── Handlers ─────────────────────────

_server_ref: Dict[str, Any] = {}


def handle_chat(params: Dict[str, Any]) -> Dict[str, Any]:
    """主 chat 入口 — PREV 主循环 + Intent Router + 真实 LLM 链。

    Args:
        params.message: 用户输入
        params.context: 上下文（文件树 / git / 报错）
        params.skillBody: 显式注入 skill body（覆盖 intent_router）
        params.skillName: Skill 名（用于审计）
        params.sessionId: 会话 ID（用于 DB 持久化）
        params.maxTasks: 最大任务数（默认 20）

        测试注入（params 私有字段）：
        params._llm: 覆盖默认 LLM client
        params._executor: 覆盖默认 executor
        params._audit: 覆盖默认 audit logger

    Returns:
        dict (含 plan / loop / audit_lines / routed_skill) 或 {"error": ..., "kind": ...}
    """
    message = params.get("message", "")
    context = params.get("context", {}) or {}
    explicit_skill_body = params.get("skillBody")
    skill_name = params.get("skillName", "user_query")
    session_id = params.get("sessionId")
    max_tasks = params.get("maxTasks", 20)

    # Intent routing (L1 关键词 → builtin skill)
    routed_skill = None
    tools_whitelist = None
    skill_body = explicit_skill_body

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
            skill_name = routed_skill.name

    # Plan
    try:
        plan_result = plan(
            query=message,
            context=context,
            skill_body=skill_body,
            max_tasks=max_tasks,
        )
    except (PlanValidationError, PlanLLMError, PlanError, ValueError) as e:
        return {"error": str(e), "kind": type(e).__name__}

    # Wire AgentLoop — 默认从 _runtime，测试可 params.get("_X") 注入
    llm = params.get("_llm") or _runtime["llm_client"]
    executor = params.get("_executor") or _runtime["executor"]
    audit = params.get("_audit") or _runtime["audit"]

    config = AgentLoopConfig(
        react_max_steps=10,
        max_task_attempts=2,
        replan_policy=ReplanPolicy(),
        enable_replan=True,
    )

    loop = AgentLoop(llm=llm, executor=executor, audit=audit, config=config)

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
        "llm_mode": _runtime["llm_info"]["mode"] if _runtime["llm_info"] else "unknown",
    }


def handle_tool_execute(params: Dict[str, Any]) -> Dict[str, Any]:
    """tool.execute — Phase 4 stub。

    Phase 5 接入：调 IPC executor 透传 Rust 薄壳 execute_tool。
    """
    return {
        "error": "tool.execute not implemented in Phase 4",
        "phase": 4,
        "tool": params.get("name"),
        "args": params.get("args"),
    }


def handle_ping(params: Dict[str, Any]) -> Dict[str, Any]:
    """心跳 — 包含 runtime 状态。"""
    return {
        "pong": True,
        "ts": int(time.time() * 1000),
        "runtime": {
            "llm_mode": _runtime["llm_info"]["mode"] if _runtime["llm_info"] else "unknown",
            "executor_mode": _runtime["executor_info"]["mode"] if _runtime["executor_info"] else "unknown",
        },
    }


def handle_status(params: Dict[str, Any]) -> Dict[str, Any]:
    """运行时状态查询 — 前端展示用。"""
    return {
        "version": "0.4.0",
        "phase": 4,
        "llm": _runtime["llm_info"],
        "executor": _runtime["executor_info"],
        "audit": _runtime["audit_info"],
    }


def handle_shutdown(params: Dict[str, Any]) -> Dict[str, Any]:
    """优雅退出。"""
    server = _server_ref.get("server")
    if server is not None:
        server.stop()
    return {"status": "shutting_down"}


def handle_event_push(params: Dict[str, Any]) -> None:
    """Phase 4 占位：客户端通过 'event' method 推事件，本地仅记录。"""
    print(f"[event] {params}", file=sys.stderr)


# ───────────────────────── main ─────────────────────────

HANDLERS = {
    "chat": handle_chat,
    "tool.execute": handle_tool_execute,
    "ping": handle_ping,
    "shutdown": handle_shutdown,
    "status": handle_status,
}


def main() -> int:
    # CLI mode： --query-skill-outcomes 查 audit_event 返回 skill outcomes（Phase 4 step 2）
    if "--query-skill-outcomes" in sys.argv:
        return _cli_query_skill_outcomes()

    # Phase 4: stdlib logging 初始化（先于 init_runtime，捕启动异常）
    setup_logging()

    # Phase 4: 启动时构建 LLM chain + executor + audit
    try:
        init_info = init_runtime()
    except Exception:
        logger.exception("init_runtime failed at startup")
        raise

    server = IPCServer(
        handlers=HANDLERS,
        ready_payload={
            "version": "0.4.0",
            "phase": 4,
            "pid": __import__("os").getpid(),
            "llm_mode": init_info["llm"]["mode"],
            "providers_loaded": init_info["llm"].get("providers_loaded", []),
        },
        on_event=handle_event_push,
    )
    _server_ref["server"] = server

    logger.info(
        "wmessage-agent ready phase=4 llm_mode=%s providers=%s",
        init_info["llm"]["mode"],
        init_info["llm"].get("providers_loaded", []),
    )

    try:
        server.run()
    except KeyboardInterrupt:
        logger.info("KeyboardInterrupt received, shutting down")
    except Exception:
        logger.exception("server.run() crashed")
        raise

    return 0


def _cli_query_skill_outcomes() -> int:
    """CLI 子命令：查询最近 skill outcomes（Phase 4 step 2 给 Tauri SkillsPanel 用）。"""
    import argparse

    from agent_core.skill_monitoring import (
        format_outcomes_table,
        query_recent_skill_outcomes,
        summarize_status,
    )

    parser = argparse.ArgumentParser(
        prog="wmessage-agent",
        description="查询 agent.db audit_event 表中最近的 skill outcomes。",
    )
    parser.add_argument(
        "--query-skill-outcomes",
        action="store_true",
        required=True,
        help="启用 skill outcomes 查询模式",
    )
    parser.add_argument("--skill", help="只查指定 skill 名")
    parser.add_argument("--limit", type=int, default=10)
    parser.add_argument(
        "--db",
        default=os.environ.get("WMSG_AGENT_DB", "data/agent.db"),
        help="agent.db 路径",
    )
    parser.add_argument(
        "--format", choices=["json", "table"], default="table"
    )

    opts = parser.parse_args()

    if not os.path.exists(opts.db):
        print(f"DB not found: {opts.db}", file=sys.stderr)
        return 1

    outcomes = query_recent_skill_outcomes(
        opts.db, skill_name=opts.skill, limit=opts.limit
    )

    if opts.format == "json":
        # JSON 输出：outcomes + summary 两块
        import json as _json

        out = {
            "outcomes": outcomes,
            "summary": summarize_status(outcomes),
            "count": len(outcomes),
        }
        print(_json.dumps(out, indent=2, ensure_ascii=False))
    else:
        # Table 输出
        print(format_outcomes_table(outcomes))
        summary = summarize_status(outcomes)
        if summary:
            print()
            print("Summary: " + ", ".join(
                f"{k}={v}" for k, v in sorted(summary.items())
            ))

    return 0


if __name__ == "__main__":
    sys.exit(main())
