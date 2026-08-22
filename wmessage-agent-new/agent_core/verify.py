"""verify — 显式校验 task 是否真的完成。

Phase 1 实现：
- verify() 主入口：按 task.verify_hint.kind 分发
- LocalVerifyExecutor：本地实现（command / file_hash / assert 真实执行；llm_judge stub）
- VerifyExecutor 协议：测试时可注入 mock

设计约束（来自 wmessage-agent-spec.md §X.2.2 阶段 4）：
- 必须可机器校验（kind ∈ {command, file_hash, assert}）
- 不允许 LLM 自评（llm_judge 仅作弱校验辅助，需 ≥threshold 才算通过）
- 默认 max_retries=2（harness_policy.toml 可调）

四种 kind：
- command: 跑 shell 命令，expect="exit_0" | regex 匹配 stdout
- file_hash: 比对文件 SHA256
- assert: 求值 Python 表达式（Phase 1 裸 eval；Phase 2 加 sandbox）
- llm_judge: LLM 打分 ≥ threshold 通过
"""
from __future__ import annotations

import hashlib
import os
import re
import subprocess
import time
from dataclasses import dataclass
from typing import Any, Callable, Dict, Optional, Tuple

from agent_core.types import TaskNode, VerifyHint, VerifyKind, VerifyResult


# ───────────────────────── Exceptions ─────────────────────────


class VerifyError(Exception):
    """verify 错误基类。"""


class VerifyExecutorError(VerifyError):
    """执行器（命令 / 文件 / 表达式）层面失败。"""


class VerifyLLMError(VerifyError):
    """LLM judge 调用失败。"""


# ───────────────────────── Executor 协议 ─────────────────────────


@dataclass
class CommandResult:
    """run_command 的返回。"""

    exit_code: int
    stdout: str
    stderr: str

    @property
    def ok(self) -> bool:
        return self.exit_code == 0


class VerifyExecutor:
    """verify() 依赖的执行器协议。测试时可注入 mock。

    5 个方法：execute / run_command / file_hash / eval_assert / llm_judge。
    - execute(name, args) 给 ReAct tool_call 用
    - run_command / file_hash / eval_assert / llm_judge 给 verify_hint 4 种 kind 用
    Phase 1 LocalVerifyExecutor 全部 stub；Phase 2 接入真 IPC executor（透传 Rust 薄壳）。
    """

    def execute(self, name: str, args: Any) -> str:
        """通用 tool 执行（ReAct tool_call 调用）。

        Phase 1 stub — 抛 NotImplementedError，测试时由 MockVerifyExecutor 提供实现。
        Phase 2 接入 IPC executor：JSON-RPC 透传到 Rust 薄壳 execute_tool。
        """
        raise NotImplementedError

    def run_command(self, cmd: str, timeout_s: int = 60) -> CommandResult:
        raise NotImplementedError

    def file_hash(self, path: str) -> str:
        raise NotImplementedError

    def eval_assert(self, expression: str) -> Any:
        raise NotImplementedError

    def llm_judge(
        self, rubric: str, output: str, threshold: float
    ) -> Tuple[bool, float, str]:
        raise NotImplementedError


# ───────────────────────── 本地实现 ─────────────────────────


class LocalVerifyExecutor(VerifyExecutor):
    """Phase 1 本地执行器 — command / file_hash / assert 真实执行；llm_judge stub。

    注意：eval_assert 走裸 eval，无 sandbox。Phase 2 引入 RestrictedPython 隔离。
    """

    def run_command(self, cmd: str, timeout_s: int = 60) -> CommandResult:
        try:
            r = subprocess.run(
                cmd,
                shell=True,
                capture_output=True,
                text=True,
                timeout=timeout_s,
            )
            return CommandResult(
                exit_code=r.returncode,
                stdout=r.stdout,
                stderr=r.stderr,
            )
        except subprocess.TimeoutExpired as e:
            return CommandResult(
                exit_code=-1,
                stdout=e.stdout or "",
                stderr=(e.stderr or "") + f"\n[timeout after {timeout_s}s]",
            )

    def file_hash(self, path: str) -> str:
        h = hashlib.sha256()
        with open(path, "rb") as f:
            for chunk in iter(lambda: f.read(8192), b""):
                h.update(chunk)
        return h.hexdigest()

    def eval_assert(self, expression: str) -> Any:
        # ⚠️ Phase 1 裸 eval；Phase 2 加 sandbox
        return eval(expression, {"__builtins__": {}}, {})

    def llm_judge(
        self, rubric: str, output: str, threshold: float
    ) -> Tuple[bool, float, str]:
        # Phase 1 stub — 不接 LLM
        return (False, 0.0, "LLM judge 未接入 Phase 1")

    def execute(self, name: str, args: Any) -> str:
        """Phase 1 stub — 返回 mock 响应（带 UUID 方便测试）。

        Phase 2 替换为 IPC executor（透传 Rust execute_tool）。
        """
        # 已知 tool_name 给 mock 数据；未知返回 'mock result'
        if name == "list_tasks":
            return "Found 1 task: [id=00000000-0000-4000-8000-000000000001]"
        if name == "query_single_task":
            task_id = args.get("id", "") if isinstance(args, dict) else ""
            return f"Task {task_id}: title='mock', done=false"
        return f"mock result for {name}({args})"


# ───────────────────────── 主入口 ─────────────────────────


def verify(
    task: TaskNode,
    executor: VerifyExecutor,
    timeout_s: int = 60,
) -> VerifyResult:
    """校验 task 是否真的完成。

    Args:
        task: TaskNode，必须有 verify_hint
        executor: VerifyExecutor 实例
        timeout_s: command 默认超时

    Returns:
        VerifyResult(passed, output, evidence, error, kind, duration_ms)

    Raises:
        VerifyError: task 没有 verify_hint 或 kind 未知
    """
    if task.verify_hint is None:
        raise VerifyError(f"task {task.id} 没有 verify_hint")

    hint = task.verify_hint
    start = time.time()
    try:
        if hint.kind == VerifyKind.COMMAND.value:
            r = _verify_command(hint, executor, timeout_s)
        elif hint.kind == VerifyKind.FILE_HASH.value:
            r = _verify_file_hash(hint, executor)
        elif hint.kind == VerifyKind.ASSERT.value:
            r = _verify_assert(hint, executor)
        elif hint.kind == VerifyKind.LLM_JUDGE.value:
            r = _verify_llm_judge(hint, executor)
        else:
            raise VerifyError(f"未知 verify kind：{hint.kind}")
    except VerifyError:
        raise
    except Exception as e:
        duration_ms = int((time.time() - start) * 1000)
        return VerifyResult(
            passed=False,
            output=f"verify 异常：{e}",
            evidence="",
            error=repr(e),
            kind=hint.kind,
            duration_ms=duration_ms,
        )

    duration_ms = int((time.time() - start) * 1000)
    r.duration_ms = duration_ms
    return r


# ───────────────────────── 各类 kind 实现 ─────────────────────────


def _verify_command(
    hint: VerifyHint, executor: VerifyExecutor, timeout_s: int
) -> VerifyResult:
    if not hint.command:
        raise VerifyExecutorError("command kind 必须有 command 字段")

    cr = executor.run_command(hint.command, timeout_s)
    expect = hint.expect or "exit_0"

    if expect == "exit_0":
        passed = cr.ok
        output = f"exit_code={cr.exit_code}\nstdout: {cr.stdout.strip()[:500]}\nstderr: {cr.stderr.strip()[:200]}"
        evidence = f"exit={cr.exit_code}"
    else:
        # expect 视为 regex 模式
        pattern = re.compile(expect)
        passed = bool(pattern.search(cr.stdout))
        output = f"匹配 '{expect}' → {'命中' if passed else '未命中'}\nstdout: {cr.stdout.strip()[:500]}"
        evidence = f"pattern={expect} matched={passed}"

    return VerifyResult(
        passed=passed,
        output=output,
        evidence=evidence,
        error=cr.stderr if not cr.ok else None,
        kind=hint.kind,
    )


def _verify_file_hash(hint: VerifyHint, executor: VerifyExecutor) -> VerifyResult:
    if not hint.path:
        raise VerifyExecutorError("file_hash kind 必须有 path 字段")
    if not hint.sha256:
        raise VerifyExecutorError("file_hash kind 必须有 sha256 字段")

    if not os.path.exists(hint.path):
        return VerifyResult(
            passed=False,
            output=f"文件不存在：{hint.path}",
            evidence="",
            error="FileNotFound",
            kind=hint.kind,
        )

    actual = executor.file_hash(hint.path)
    passed = actual.lower() == hint.sha256.lower()
    return VerifyResult(
        passed=passed,
        output=f"期望 {hint.sha256[:16]}…\n实际 {actual[:16]}…",
        evidence=f"actual={actual}",
        error=None if passed else "hash mismatch",
        kind=hint.kind,
    )


def _verify_assert(hint: VerifyHint, executor: VerifyExecutor) -> VerifyResult:
    if not hint.expression:
        raise VerifyExecutorError("assert kind 必须有 expression 字段")

    try:
        result = executor.eval_assert(hint.expression)
    except Exception as e:
        return VerifyResult(
            passed=False,
            output=f"assert 求值异常：{e}",
            evidence="",
            error=repr(e),
            kind=hint.kind,
        )

    passed = bool(result)
    return VerifyResult(
        passed=passed,
        output=f"expression={hint.expression}\nresult={result!r}",
        evidence=f"result={result!r}",
        error=None if passed else "assert False",
        kind=hint.kind,
    )


def _verify_llm_judge(
    hint: VerifyHint, executor: VerifyExecutor
) -> VerifyResult:
    if not hint.rubric:
        raise VerifyExecutorError("llm_judge kind 必须有 rubric 字段")
    threshold = hint.threshold if hint.threshold is not None else 0.7

    try:
        passed, score, reason = executor.llm_judge(hint.rubric, "", threshold)
    except Exception as e:
        raise VerifyLLMError(f"LLM judge 失败：{e}") from e

    return VerifyResult(
        passed=passed,
        output=f"rubric={hint.rubric}\nscore={score:.2f} threshold={threshold:.2f}\n{reason}",
        evidence=f"score={score:.4f}",
        error=None if passed else f"score {score:.2f} < threshold {threshold:.2f}",
        kind=hint.kind,
    )
