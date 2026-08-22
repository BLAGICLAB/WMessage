"""audit_log — 结构化事件 + 与老 bot.log 兼容。

事件格式（与 Rust 侧 audit_event 一致）：
    [ts.毫秒] LEVEL | event | k=v | k=v ...

Phase 1 实现：
- 8+ 类结构化事件
- 落本地文件（兼容 Rust bot.log 文本格式）
- 便利方法 user_message / tool_call / tool_return / skill_start / skill_finish / llm_request / llm_response

Phase 2 扩展：
- 落 SQLite（audit_event 表）
- 与 Rust 侧审计事件统一（cross-language trace）
"""
from __future__ import annotations

import os
import time
from enum import Enum
from pathlib import Path
from typing import Any, Dict, Optional, Union


class AuditLevel(str, Enum):
    """事件分级（与 Rust 侧 AuditLevel 一致）。"""

    INFO = "Info"
    WARN = "Warn"
    ERROR = "Error"


# 命名空间统一（来自 wmessage-agent-spec.md §X.6 触发钩子一览）
class AuditEvent:
    """事件命名空间常量。"""

    USER_MESSAGE = "user.message"
    PRE_STEP_ROUTE_SKILL = "pre_step.route_skill"
    PRE_STEP_ROUTE_FAILED = "pre_step.route_failed"
    PRE_EXECUTE_DENY = "pre_execute.deny"
    SKILL_START = "skill.start"
    TOOL_CALL = "tool.call"
    TOOL_RETURN = "tool.return"
    LLM_REQUEST = "llm.request"
    LLM_RESPONSE = "llm.response"
    VERIFY_START = "verify.start"
    VERIFY_PASS = "verify.pass"
    VERIFY_FAIL = "verify.fail"
    SKILL_FINISH = "skill.finish"
    REPLAN_TRIGGERED = "replan.triggered"
    TASK_DONE = "task.done"
    TASK_FAIL = "task.fail"


class AuditLogger:
    """结构化审计日志。

    用法：
        logger = AuditLogger(path="/tmp/bot.log")
        logger.user_message(session_id="s1", message="hello")
        # → [1734...ms] Info | user.message | session_id=s1 | message=hello

    线程模型：Phase 1 单线程，append 模式写入文件。
    Phase 2 加锁（与 Rust 侧 BOT_LOG_LOCK 对齐）。
    """

    def __init__(self, path: Optional[Union[str, Path]] = None):
        """初始化。

        Args:
            path: 日志文件路径。None = 仅内存（不落盘）
        """
        self.path = Path(path) if path else None
        self._fh: Optional[Any] = None  # lazy 文件句柄（避免每次 log 都 open/close）

    # ───────────────────────── 核心 ─────────────────────────

    def log(
        self,
        level: Union[AuditLevel, str],
        event: str,
        **kwargs: Any,
    ) -> str:
        """记录一条事件。返回格式化后的行。

        Args:
            level: AuditLevel 或字符串
            event: 事件名（AuditEvent 常量）
            **kwargs: 附加字段

        Returns:
            格式化后的日志行（带 timestamp + level + event + kv）
        """
        level_str = level.value if isinstance(level, AuditLevel) else str(level)
        ts_ms = int(time.time() * 1000)
        # 字段值转字符串，None 跳过
        kv_parts = []
        for k, v in kwargs.items():
            if v is None:
                continue
            # 多行值截断（避免日志被刷屏）
            v_str = str(v)
            if "\n" in v_str:
                v_str = v_str.replace("\n", "\\n")[:200]
            if len(v_str) > 300:
                v_str = v_str[:300] + "..."
            kv_parts.append(f"{k}={v_str}")

        kv = " | ".join(kv_parts)
        line = f"[{ts_ms}] {level_str} | {event}" + (f" | {kv}" if kv else "")

        if self.path is not None:
            self._append(line)
        return line

    def _append(self, line: str) -> None:
        """追加一行到日志文件（保持句柄打开以避免累积 open/close 开销）。"""
        if self.path is None:
            return
        try:
            if self._fh is None:
                self.path.parent.mkdir(parents=True, exist_ok=True)
                self._fh = open(self.path, "a", encoding="utf-8")
            self._fh.write(line + "\n")
            self._fh.flush()
        except OSError:
            try:
                if self._fh:
                    self._fh.close()
            except Exception:
                pass
            self._fh = None

    def close(self) -> None:
        """关闭文件句柄（shutdown 时调用）。"""
        if self._fh is not None:
            try:
                self._fh.flush()
                self._fh.close()
            except Exception:
                pass
            self._fh = None

    def __del__(self) -> None:
        self.close()

    # ───────────────────────── 便利方法 ─────────────────────────

    def info(self, event: str, **kwargs: Any) -> str:
        return self.log(AuditLevel.INFO, event, **kwargs)

    def warn(self, event: str, **kwargs: Any) -> str:
        return self.log(AuditLevel.WARN, event, **kwargs)

    def error(self, event: str, **kwargs: Any) -> str:
        return self.log(AuditLevel.ERROR, event, **kwargs)

    def user_message(self, session_id: str, message: str) -> str:
        return self.info(AuditEvent.USER_MESSAGE, session_id=session_id, message=message)

    def pre_step_route_skill(self, skill: str, intents: Any = None) -> str:
        return self.info(
            AuditEvent.PRE_STEP_ROUTE_SKILL, skill=skill, intents=intents
        )

    def pre_step_route_failed(self, query: str, reason: str) -> str:
        return self.warn(AuditEvent.PRE_STEP_ROUTE_FAILED, query=query, reason=reason)

    def pre_execute_deny(self, tool: str, reason: str) -> str:
        return self.warn(AuditEvent.PRE_EXECUTE_DENY, tool=tool, reason=reason)

    def skill_start(self, skill_run_id: str, skill: str, mode: str = "auto") -> str:
        return self.info(
            AuditEvent.SKILL_START, skill_run_id=skill_run_id, skill=skill, mode=mode
        )

    def tool_call(self, task_id: str, tool: str, args_preview: Any = None) -> str:
        return self.info(
            AuditEvent.TOOL_CALL, task_id=task_id, tool=tool, args=args_preview
        )

    def tool_return(
        self,
        task_id: str,
        tool: str,
        result_preview: Any = None,
        level: AuditLevel = AuditLevel.INFO,
        error: Optional[str] = None,
        duration_ms: Optional[int] = None,
    ) -> str:
        kwargs: Dict[str, Any] = {
            "task_id": task_id,
            "tool": tool,
            "result": result_preview,
        }
        if error:
            kwargs["error"] = error
        if duration_ms is not None:
            kwargs["duration_ms"] = duration_ms
        return self.log(level, AuditEvent.TOOL_RETURN, **kwargs)

    def llm_request(self, model: str, prompt_tokens: int = 0) -> str:
        return self.info(
            AuditEvent.LLM_REQUEST, model=model, prompt_tokens=prompt_tokens
        )

    def llm_response(
        self,
        model: str,
        completion_tokens: int = 0,
        duration_ms: int = 0,
        level: AuditLevel = AuditLevel.INFO,
    ) -> str:
        return self.log(
            level,
            AuditEvent.LLM_RESPONSE,
            model=model,
            completion_tokens=completion_tokens,
            duration_ms=duration_ms,
        )

    def verify_start(self, task_id: str, kind: str) -> str:
        return self.info(AuditEvent.VERIFY_START, task_id=task_id, kind=kind)

    def verify_pass(self, task_id: str, evidence: Any = None) -> str:
        return self.info(AuditEvent.VERIFY_PASS, task_id=task_id, evidence=evidence)

    def verify_fail(self, task_id: str, reason: str) -> str:
        return self.warn(AuditEvent.VERIFY_FAIL, task_id=task_id, reason=reason)

    def skill_finish(
        self,
        skill_run_id: str,
        status: str,
        end_reason: str = "ok",
        completed_steps: int = 0,
        total_steps: int = 0,
    ) -> str:
        level = AuditLevel.INFO if status == "ok" else AuditLevel.WARN
        return self.log(
            level,
            AuditEvent.SKILL_FINISH,
            skill_run_id=skill_run_id,
            status=status,
            end_reason=end_reason,
            completed_steps=completed_steps,
            total_steps=total_steps,
        )

    def replan_triggered(
        self, reason: str, graph_id: str, replan_count: int = 1
    ) -> str:
        return self.warn(
            AuditEvent.REPLAN_TRIGGERED,
            reason=reason,
            graph_id=graph_id,
            count=replan_count,
        )

    def task_done(self, task_id: str, attempts: int = 1) -> str:
        return self.info(AuditEvent.TASK_DONE, task_id=task_id, attempts=attempts)

    def task_fail(self, task_id: str, reason: str, attempts: int = 1) -> str:
        return self.warn(
            AuditEvent.TASK_FAIL, task_id=task_id, reason=reason, attempts=attempts
        )
