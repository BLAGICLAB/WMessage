"""verify 测试 — 4 种 kind + LocalVerifyExecutor + Mock executor。"""
from __future__ import annotations

import os
import tempfile

import pytest

from agent_core.verify import (
    verify,
    LocalVerifyExecutor,
    CommandResult,
    VerifyError,
    VerifyExecutorError,
    VerifyLLMError,
)
from agent_core.types import TaskNode, VerifyHint


# ───────────────────────── command kind ─────────────────────────


def test_command_exit_0_passed():
    """command exit_0 + 实际退出 0 → 通过。"""
    hint = VerifyHint(kind="command", command="echo done", expect="exit_0")
    task = TaskNode(id="t1", title="x", verify_hint=hint)
    ex = LocalVerifyExecutor()

    r = verify(task, ex)
    assert r.passed is True
    assert r.kind == "command"
    assert "exit_code=0" in r.output


def test_command_non_zero_exit_failed():
    """command exit_0 + 实际退出 1 → 不通过。"""
    hint = VerifyHint(kind="command", command="false", expect="exit_0")
    task = TaskNode(id="t1", title="x", verify_hint=hint)
    ex = LocalVerifyExecutor()

    r = verify(task, ex)
    assert r.passed is False
    assert "exit_code=1" in r.output


def test_command_regex_match_passed():
    """command expect 是 regex + stdout 命中 → 通过。"""
    hint = VerifyHint(kind="command", command="echo hello world", expect=r"hello\s+world")
    task = TaskNode(id="t1", title="x", verify_hint=hint)
    ex = LocalVerifyExecutor()

    r = verify(task, ex)
    assert r.passed is True
    assert "命中" in r.output


def test_command_regex_miss_failed():
    """command expect 是 regex + stdout 不命中 → 不通过。"""
    hint = VerifyHint(kind="command", command="echo foo", expect=r"^bar$")
    task = TaskNode(id="t1", title="x", verify_hint=hint)
    ex = LocalVerifyExecutor()

    r = verify(task, ex)
    assert r.passed is False
    assert "未命中" in r.output


def test_command_missing_command_raises():
    """command kind 但 command 字段为空 → VerifyExecutorError。"""
    hint = VerifyHint(kind="command")
    task = TaskNode(id="t1", title="x", verify_hint=hint)
    ex = LocalVerifyExecutor()

    with pytest.raises(VerifyExecutorError, match="command"):
        verify(task, ex)


def test_command_duration_ms_recorded():
    """duration_ms 字段被填。"""
    hint = VerifyHint(kind="command", command="echo done", expect="exit_0")
    task = TaskNode(id="t1", title="x", verify_hint=hint)
    ex = LocalVerifyExecutor()

    r = verify(task, ex)
    assert r.duration_ms >= 0


# ───────────────────────── file_hash kind ─────────────────────────


def test_file_hash_match_passed(tmp_path):
    """file_hash 匹配 → 通过。"""
    f = tmp_path / "x.txt"
    f.write_text("hello")
    import hashlib

    expected = hashlib.sha256(b"hello").hexdigest()
    hint = VerifyHint(kind="file_hash", path=str(f), sha256=expected)
    task = TaskNode(id="t1", title="x", verify_hint=hint)
    ex = LocalVerifyExecutor()

    r = verify(task, ex)
    assert r.passed is True


def test_file_hash_mismatch_failed(tmp_path):
    """file_hash 不匹配 → 不通过。"""
    f = tmp_path / "x.txt"
    f.write_text("hello")
    hint = VerifyHint(kind="file_hash", path=str(f), sha256="0" * 64)
    task = TaskNode(id="t1", title="x", verify_hint=hint)
    ex = LocalVerifyExecutor()

    r = verify(task, ex)
    assert r.passed is False
    assert "hash mismatch" in (r.error or "")


def test_file_hash_file_not_exists_failed():
    """file_hash 文件不存在 → 不通过（不抛）。"""
    hint = VerifyHint(kind="file_hash", path="/nonexistent/path", sha256="0" * 64)
    task = TaskNode(id="t1", title="x", verify_hint=hint)
    ex = LocalVerifyExecutor()

    r = verify(task, ex)
    assert r.passed is False
    assert "不存在" in r.output


def test_file_hash_missing_path_raises():
    """file_hash 缺 path → VerifyExecutorError。"""
    hint = VerifyHint(kind="file_hash", sha256="0" * 64)
    task = TaskNode(id="t1", title="x", verify_hint=hint)
    ex = LocalVerifyExecutor()

    with pytest.raises(VerifyExecutorError, match="path"):
        verify(task, ex)


def test_file_hash_missing_sha256_raises():
    """file_hash 缺 sha256 → VerifyExecutorError。"""
    hint = VerifyHint(kind="file_hash", path="/tmp/x")
    task = TaskNode(id="t1", title="x", verify_hint=hint)
    ex = LocalVerifyExecutor()

    with pytest.raises(VerifyExecutorError, match="sha256"):
        verify(task, ex)


def test_file_hash_case_insensitive(tmp_path):
    """file_hash 比较大小写不敏感。"""
    f = tmp_path / "x.txt"
    f.write_text("hello")
    import hashlib

    h = hashlib.sha256(b"hello").hexdigest()
    hint = VerifyHint(kind="file_hash", path=str(f), sha256=h.upper())
    task = TaskNode(id="t1", title="x", verify_hint=hint)
    ex = LocalVerifyExecutor()

    r = verify(task, ex)
    assert r.passed is True


# ───────────────────────── assert kind ─────────────────────────


def test_assert_true_passed():
    """assert True → 通过。"""
    hint = VerifyHint(kind="assert", expression="True")
    task = TaskNode(id="t1", title="x", verify_hint=hint)
    ex = LocalVerifyExecutor()

    r = verify(task, ex)
    assert r.passed is True
    assert "True" in r.output


def test_assert_false_failed():
    """assert False → 不通过。"""
    hint = VerifyHint(kind="assert", expression="False")
    task = TaskNode(id="t1", title="x", verify_hint=hint)
    ex = LocalVerifyExecutor()

    r = verify(task, ex)
    assert r.passed is False
    assert "assert False" in (r.error or "")


def test_assert_arithmetic_passed():
    """assert 1+1==2 → 通过。"""
    hint = VerifyHint(kind="assert", expression="1+1==2")
    task = TaskNode(id="t1", title="x", verify_hint=hint)
    ex = LocalVerifyExecutor()

    r = verify(task, ex)
    assert r.passed is True


def test_assert_expression_raises_failed():
    """assert 表达式求值抛异常 → 不通过（不抛）。"""
    hint = VerifyHint(kind="assert", expression="1/0")
    task = TaskNode(id="t1", title="x", verify_hint=hint)
    ex = LocalVerifyExecutor()

    r = verify(task, ex)
    assert r.passed is False
    assert "ZeroDivision" in r.error or "division" in (r.error or "").lower()


def test_assert_missing_expression_raises():
    """assert kind 缺 expression → VerifyExecutorError。"""
    hint = VerifyHint(kind="assert")
    task = TaskNode(id="t1", title="x", verify_hint=hint)
    ex = LocalVerifyExecutor()

    with pytest.raises(VerifyExecutorError, match="expression"):
        verify(task, ex)


# ───────────────────────── llm_judge kind ─────────────────────────


def test_llm_judge_stub_returns_failed():
    """llm_judge kind + LocalVerifyExecutor（stub）→ 不通过。"""
    hint = VerifyHint(kind="llm_judge", rubric="所有测试通过", threshold=0.7)
    task = TaskNode(id="t1", title="x", verify_hint=hint)
    ex = LocalVerifyExecutor()

    r = verify(task, ex)
    assert r.passed is False
    assert "未接入" in r.output


def test_llm_judge_missing_rubric_raises():
    """llm_judge 缺 rubric → VerifyExecutorError。"""
    hint = VerifyHint(kind="llm_judge")
    task = TaskNode(id="t1", title="x", verify_hint=hint)
    ex = LocalVerifyExecutor()

    with pytest.raises(VerifyExecutorError, match="rubric"):
        verify(task, ex)


def test_llm_judge_threshold_default():
    """llm_judge 不传 threshold → 默认 0.7。"""
    hint = VerifyHint(kind="llm_judge", rubric="x")
    assert hint.threshold is None  # 调用 verify 时由 llm_judge 函数填默认


def test_llm_judge_evidence_contains_score():
    """llm_judge evidence 应包含 score。"""
    from tests.conftest import MockVerifyExecutor

    hint = VerifyHint(kind="llm_judge", rubric="x", threshold=0.5)
    task = TaskNode(id="t1", title="x", verify_hint=hint)
    ex = MockVerifyExecutor(llm_judge_result=(True, 0.9, "looks good"))

    r = verify(task, ex)
    assert r.passed is True
    assert "0.9" in r.evidence


# ───────────────────────── 顶层校验 ─────────────────────────


def test_no_verify_hint_raises():
    """task 没有 verify_hint → VerifyError。"""
    task = TaskNode(id="t1", title="x")
    ex = LocalVerifyExecutor()
    with pytest.raises(VerifyError, match="verify_hint"):
        verify(task, ex)


def test_unknown_kind_raises():
    """未知 kind → VerifyError。"""
    hint = VerifyHint(kind="magic")
    task = TaskNode(id="t1", title="x", verify_hint=hint)
    ex = LocalVerifyExecutor()
    with pytest.raises(VerifyError, match="未知 verify kind"):
        verify(task, ex)


def test_exception_during_execution_returns_failed_result():
    """执行器抛意外异常 → VerifyResult(passed=False, error=...)。"""
    from tests.conftest import MockVerifyExecutor

    class BoomExecutor(MockVerifyExecutor):
        def run_command(self, cmd, timeout_s=60):
            raise RuntimeError("boom")

    hint = VerifyHint(kind="command", command="x", expect="exit_0")
    task = TaskNode(id="t1", title="x", verify_hint=hint)
    ex = BoomExecutor()

    r = verify(task, ex)
    assert r.passed is False
    assert "boom" in r.output


# ───────────────────────── Mock executor 行为 ─────────────────────────


def test_mock_executor_records_calls():
    """MockVerifyExecutor 记录每个调用。"""
    from tests.conftest import MockVerifyExecutor

    ex = MockVerifyExecutor(command_result=CommandResult(0, "out", ""))
    hint = VerifyHint(kind="command", command="ls", expect="exit_0")
    task = TaskNode(id="t1", title="x", verify_hint=hint)

    verify(task, ex)
    assert len(ex.calls) == 1
    assert ex.calls[0]["method"] == "run_command"
    assert ex.calls[0]["cmd"] == "ls"


# ───────────────────────── LocalVerifyExecutor 真实执行 ─────────────────────────


def test_local_run_command_real(tmp_path):
    """LocalVerifyExecutor.run_command 真跑命令。"""
    ex = LocalVerifyExecutor()
    r = ex.run_command("echo real")
    assert r.ok is True
    assert r.stdout.strip() == "real"


def test_local_file_hash_real(tmp_path):
    """LocalVerifyExecutor.file_hash 真算 SHA256。"""
    f = tmp_path / "test.txt"
    f.write_text("hello")
    ex = LocalVerifyExecutor()
    import hashlib

    assert ex.file_hash(str(f)) == hashlib.sha256(b"hello").hexdigest()


def test_local_eval_assert_real():
    """LocalVerifyExecutor.eval_assert 真求值。"""
    ex = LocalVerifyExecutor()
    assert ex.eval_assert("2+3") == 5
