"""audit_log 测试 — 结构化事件 + 文件落盘 + 便利方法。"""
from __future__ import annotations

import json
import re
from pathlib import Path

import pytest

from agent_core.audit_log import AuditLevel, AuditLogger, AuditEvent


# ───────────────────────── 核心 log() ─────────────────────────


def test_log_formats_basic_line():
    """log() 输出 [ts] LEVEL | event | k=v 格式。"""
    logger = AuditLogger()
    line = logger.log(AuditLevel.INFO, "test.event", key="value")
    # 格式：[1234567890123] Info | test.event | key=value
    assert re.match(r"^\[\d+\] Info \| test\.event \| key=value$", line)


def test_log_skips_none_values():
    """None 值不写入 kv。"""
    logger = AuditLogger()
    line = logger.log("Info", "test.event", key="value", skip=None)
    assert "skip" not in line
    assert "None" not in line


def test_log_truncates_long_values():
    """> 300 字符的值截断。"""
    logger = AuditLogger()
    long_val = "x" * 500
    line = logger.log("Info", "t", val=long_val)
    assert "..." in line
    # 截断后长度有限
    assert len(line) < 350


def test_log_escapes_newlines():
    """多行值转义为 \\n。"""
    logger = AuditLogger()
    line = logger.log("Info", "t", val="line1\nline2")
    assert "\\n" in line
    assert "line1" in line
    assert "line2" in line


def test_log_accepts_string_level():
    """level 可传字符串（不限于 AuditLevel 枚举）。"""
    logger = AuditLogger()
    line = logger.log("Info", "t", k="v")
    assert " Info " in line
    line2 = logger.log("CustomLevel", "t", k="v")
    assert " CustomLevel " in line2


def test_log_writes_to_file(tmp_path):
    """log() 写入指定文件。"""
    p = tmp_path / "audit.log"
    logger = AuditLogger(path=p)
    logger.info("test.event", key="value1")
    logger.info("test.event2", key="value2")
    assert p.exists()
    content = p.read_text(encoding="utf-8")
    lines = content.strip().split("\n")
    assert len(lines) == 2
    assert "value1" in lines[0]
    assert "value2" in lines[1]


def test_log_without_path_returns_line_only():
    """path=None → 仅返回 line，不写盘。"""
    logger = AuditLogger()
    line = logger.info("t", k="v")
    assert "Info" in line
    assert "t" in line


def test_log_appends_not_overwrites(tmp_path):
    """log() 追加写入，不覆盖。"""
    p = tmp_path / "audit.log"
    logger1 = AuditLogger(path=p)
    logger1.info("first", k="v1")
    logger2 = AuditLogger(path=p)
    logger2.info("second", k="v2")
    content = p.read_text(encoding="utf-8")
    assert "first" in content
    assert "second" in content


# ───────────────────────── 便利方法 ─────────────────────────


def test_info_method():
    """info() = log(Info, ...)。"""
    logger = AuditLogger()
    line = logger.info("t", k="v")
    assert "Info" in line


def test_warn_method():
    """warn() = log(Warn, ...)。"""
    logger = AuditLogger()
    line = logger.warn("t", k="v")
    assert "Warn" in line


def test_error_method():
    """error() = log(Error, ...)。"""
    logger = AuditLogger()
    line = logger.error("t", k="v")
    assert "Error" in line


def test_user_message_method(tmp_path):
    """user_message() 包含 session_id + message。"""
    p = tmp_path / "a.log"
    logger = AuditLogger(path=p)
    logger.user_message(session_id="s1", message="hello world")
    content = p.read_text(encoding="utf-8")
    assert AuditEvent.USER_MESSAGE in content
    assert "session_id=s1" in content
    assert "hello world" in content


def test_tool_call_method():
    """tool_call() 包含 task_id + tool + args。"""
    logger = AuditLogger()
    line = logger.tool_call(task_id="t1", tool="read_file", args_preview={"path": "x"})
    assert AuditEvent.TOOL_CALL in line
    assert "task_id=t1" in line
    assert "tool=read_file" in line


def test_tool_return_info_level():
    """tool_return 默认 Info level。"""
    logger = AuditLogger()
    line = logger.tool_return(task_id="t1", tool="read_file", result_preview="ok")
    assert " Info " in line


def test_tool_return_error_level():
    """tool_return 可传 Error level。"""
    logger = AuditLogger()
    line = logger.tool_return(
        task_id="t1",
        tool="read_file",
        result_preview="",
        level=AuditLevel.ERROR,
        error="FileNotFound",
    )
    assert " Error " in line
    assert "error=FileNotFound" in line


def test_tool_return_duration_ms():
    """tool_return 含 duration_ms。"""
    logger = AuditLogger()
    line = logger.tool_return(
        task_id="t1", tool="x", result_preview="ok", duration_ms=123
    )
    assert "duration_ms=123" in line


def test_llm_request_method():
    """llm_request() 含 model + prompt_tokens。"""
    logger = AuditLogger()
    line = logger.llm_request(model="minimax-m3", prompt_tokens=100)
    assert AuditEvent.LLM_REQUEST in line
    assert "model=minimax-m3" in line
    assert "prompt_tokens=100" in line


def test_llm_response_method():
    """llm_response() 含 completion_tokens + duration_ms。"""
    logger = AuditLogger()
    line = logger.llm_response(
        model="minimax-m3", completion_tokens=50, duration_ms=1200
    )
    assert AuditEvent.LLM_RESPONSE in line
    assert "completion_tokens=50" in line
    assert "duration_ms=1200" in line


def test_skill_start_method():
    """skill_start() 含 skill_run_id + skill + mode。"""
    logger = AuditLogger()
    line = logger.skill_start(skill_run_id="sr-1", skill="test-skill", mode="auto")
    assert AuditEvent.SKILL_START in line
    assert "skill_run_id=sr-1" in line
    assert "skill=test-skill" in line
    assert "mode=auto" in line


def test_skill_finish_ok_uses_info():
    """skill_finish(status=ok) 用 Info level。"""
    logger = AuditLogger()
    line = logger.skill_finish(
        skill_run_id="sr-1", status="ok", completed_steps=3, total_steps=3
    )
    assert " Info " in line
    assert AuditEvent.SKILL_FINISH in line


def test_skill_finish_failed_uses_warn():
    """skill_finish(status!=ok) 用 Warn level。"""
    logger = AuditLogger()
    line = logger.skill_finish(
        skill_run_id="sr-1", status="failed", end_reason="verify_failed"
    )
    assert " Warn " in line
    assert "end_reason=verify_failed" in line


def test_replan_triggered_method():
    """replan_triggered() 含 reason + graph_id + count。"""
    logger = AuditLogger()
    line = logger.replan_triggered(
        reason="task_failed", graph_id="v1", replan_count=2
    )
    assert AuditEvent.REPLAN_TRIGGERED in line
    assert "reason=task_failed" in line
    assert "count=2" in line


def test_verify_methods():
    """verify_start / pass / fail 三种方法。"""
    logger = AuditLogger()
    s = logger.verify_start(task_id="t1", kind="command")
    p = logger.verify_pass(task_id="t1", evidence="exit=0")
    f = logger.verify_fail(task_id="t1", reason="non-zero exit")
    assert AuditEvent.VERIFY_START in s
    assert AuditEvent.VERIFY_PASS in p
    assert AuditEvent.VERIFY_FAIL in f
    assert " Warn " in f  # fail 用 Warn


def test_task_done_method():
    """task_done() 含 task_id + attempts。"""
    logger = AuditLogger()
    line = logger.task_done(task_id="t1", attempts=1)
    assert AuditEvent.TASK_DONE in line
    assert "attempts=1" in line


def test_task_fail_method():
    """task_fail() 用 Warn level。"""
    logger = AuditLogger()
    line = logger.task_fail(task_id="t1", reason="verify failed", attempts=2)
    assert " Warn " in line
    assert "attempts=2" in line


# ───────────────────────── Event 命名空间 ─────────────────────────


def test_audit_event_constants():
    """AuditEvent 常量值稳定（不能随便改，否则审计兼容性破）。"""
    assert AuditEvent.USER_MESSAGE == "user.message"
    assert AuditEvent.PRE_STEP_ROUTE_SKILL == "pre_step.route_skill"
    assert AuditEvent.TOOL_CALL == "tool.call"
    assert AuditEvent.SKILL_FINISH == "skill.finish"
    assert AuditEvent.REPLAN_TRIGGERED == "replan.triggered"


def test_audit_level_enum():
    """AuditLevel 枚举值。"""
    assert AuditLevel.INFO.value == "Info"
    assert AuditLevel.WARN.value == "Warn"
    assert AuditLevel.ERROR.value == "Error"


# ───────────────────────── 文件创建 ─────────────────────────


def test_log_creates_parent_directory(tmp_path):
    """父目录不存在 → 自动创建。"""
    p = tmp_path / "subdir" / "deep" / "audit.log"
    logger = AuditLogger(path=p)
    logger.info("t", k="v")
    assert p.exists()
    assert p.parent.is_dir()


def test_log_handles_os_error_gracefully(tmp_path):
    """OS 错误（如路径无效）→ 不抛错。"""
    # macOS/Linux: root 路径不可写
    bad_path = tmp_path / "nonexistent" / ".." / ".." / "etc" / "passwd" / "audit.log"
    logger = AuditLogger(path=bad_path)
    # 不应抛错
    logger.info("t", k="v")
