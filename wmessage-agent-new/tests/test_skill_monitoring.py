"""skill_monitoring 测试 — audit_event 表查询 skill outcomes。"""
from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path

import pytest

from agent_core.db import AuditEventDAO, SessionDAO, SkillRunDAO, connect, migrate
from agent_core.skill_monitoring import (
    format_outcomes_table,
    query_recent_skill_outcomes,
    summarize_status,
)

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MIGRATIONS_DIR = os.path.join(PROJECT_ROOT, "db", "migrations")


@pytest.fixture
def db_path(tmp_path):
    """初始化 SQLite schema，返回 db 路径。"""
    p = str(tmp_path / "agent.db")
    conn = connect(p)
    migrate(conn, MIGRATIONS_DIR)
    conn.close()
    return p


def _insert_event(dao, event, payload, ts_ms, skill_run_id=None, session_id="s1"):
    """辅助：插一条 audit_event（自动建 session + skill_run 父行 防 FK 约束）。"""
    # 一次性 insert：先创建 session + skill_run 父行，再 insert audit_event
    conn = dao.conn
    cur = conn.execute("SELECT 1 FROM session WHERE id = ?", (session_id,))
    if cur.fetchone() is None:
        conn.execute(
            "INSERT INTO session (id, channel, created_at, last_active_at, metadata) "
            "VALUES (?, 'test', ?, ?, '{}')",
            (session_id, ts_ms, ts_ms),
        )
    if skill_run_id:
        cur = conn.execute("SELECT 1 FROM skill_run WHERE id = ?", (skill_run_id,))
        if cur.fetchone() is None:
            skill_name = payload.get("skill", "test-skill")
            conn.execute(
                "INSERT INTO skill_run (id, session_id, skill_name, mode, risk, status, started_at, total_steps) "
                "VALUES (?, ?, ?, 'auto', 'low', 'running', ?, 1)",
                (skill_run_id, session_id, skill_name, ts_ms),
            )
    conn.commit()
    dao.insert(
        level="Info",
        event=event,
        payload=payload,
        session_id=session_id,
        skill_run_id=skill_run_id,
        ts_ms=ts_ms,
    )


# ───────────────────────── query_recent_skill_outcomes ─────────────────────────


def test_empty_db_returns_empty_list(db_path):
    """空 DB → 空 list。"""
    assert query_recent_skill_outcomes(db_path) == []


def test_only_skill_start_marks_in_progress(db_path):
    """只有 skill.start 没 finish → status=in_progress。"""
    conn = connect(db_path)
    dao = AuditEventDAO(conn)
    _insert_event(dao, "skill.start", {"skill": "minimax-docx", "mode": "auto"}, 1000, skill_run_id="sr-1")
    conn.close()

    outcomes = query_recent_skill_outcomes(db_path)
    assert len(outcomes) == 1
    assert outcomes[0]["status"] == "in_progress"
    assert outcomes[0]["skill_name"] == "minimax-docx"
    assert outcomes[0]["duration_ms"] is None
    assert outcomes[0]["finished_at"] is None


def test_skill_start_and_finish_returns_completed(db_path):
    """skill.start + skill.finish → status=ok, duration_ms 算出。"""
    conn = connect(db_path)
    dao = AuditEventDAO(conn)
    _insert_event(dao, "skill.start", {"skill": "minimax-docx", "mode": "auto"}, 1000, skill_run_id="sr-1")
    _insert_event(dao, "skill.finish", {"status": "ok", "end_reason": "ok"}, 1500, skill_run_id="sr-1")
    conn.close()

    outcomes = query_recent_skill_outcomes(db_path)
    assert len(outcomes) == 1
    assert outcomes[0]["status"] == "ok"
    assert outcomes[0]["end_reason"] == "ok"
    assert outcomes[0]["duration_ms"] == 500
    assert outcomes[0]["finished_at"] is not None


def test_multiple_skill_runs_returned_newest_first(db_path):
    """多个 skill_run 按 start_ts 降序排。"""
    conn = connect(db_path)
    dao = AuditEventDAO(conn)
    _insert_event(dao, "skill.start", {"skill": "minimax-docx", "mode": "auto"}, 1000, skill_run_id="sr-1")
    _insert_event(dao, "skill.finish", {"status": "ok"}, 1100, skill_run_id="sr-1")
    _insert_event(dao, "skill.start", {"skill": "minimax-xlsx", "mode": "auto"}, 2000, skill_run_id="sr-2")
    _insert_event(dao, "skill.finish", {"status": "failed", "end_reason": "verify_failed"}, 2500, skill_run_id="sr-2")
    _insert_event(dao, "skill.start", {"skill": "minimax-pptx", "mode": "auto"}, 3000, skill_run_id="sr-3")
    conn.close()

    outcomes = query_recent_skill_outcomes(db_path)
    assert len(outcomes) == 3
    # sr-3 最新（in_progress）
    assert outcomes[0]["skill_run_id"] == "sr-3"
    assert outcomes[0]["status"] == "in_progress"
    # sr-2 次新（failed）
    assert outcomes[1]["skill_run_id"] == "sr-2"
    assert outcomes[1]["status"] == "failed"
    # sr-1 最旧（ok）
    assert outcomes[2]["skill_run_id"] == "sr-1"
    assert outcomes[2]["status"] == "ok"


def test_filter_by_skill_name(db_path):
    """skill_name 过滤。"""
    conn = connect(db_path)
    dao = AuditEventDAO(conn)
    _insert_event(dao, "skill.start", {"skill": "minimax-docx", "mode": "auto"}, 1000, skill_run_id="sr-1")
    _insert_event(dao, "skill.finish", {"status": "ok"}, 1100, skill_run_id="sr-1")
    _insert_event(dao, "skill.start", {"skill": "minimax-xlsx", "mode": "auto"}, 2000, skill_run_id="sr-2")
    _insert_event(dao, "skill.finish", {"status": "ok"}, 2100, skill_run_id="sr-2")
    conn.close()

    outcomes = query_recent_skill_outcomes(db_path, skill_name="minimax-docx")
    assert len(outcomes) == 1
    assert outcomes[0]["skill_name"] == "minimax-docx"


def test_limit_parameter(db_path):
    """limit 参数生效。"""
    conn = connect(db_path)
    dao = AuditEventDAO(conn)
    for i in range(10):
        _insert_event(dao, "skill.start", {"skill": f"s{i}", "mode": "auto"}, 1000 + i * 100, skill_run_id=f"sr-{i}")
        _insert_event(dao, "skill.finish", {"status": "ok"}, 1100 + i * 100, skill_run_id=f"sr-{i}")
    conn.close()

    outcomes = query_recent_skill_outcomes(db_path, limit=3)
    assert len(outcomes) == 3


def test_skips_non_skill_events(db_path):
    """非 skill.start/finish 事件被忽略。"""
    conn = connect(db_path)
    dao = AuditEventDAO(conn)
    _insert_event(dao, "tool.call", {"tool": "x"}, 1000, skill_run_id="sr-1")
    _insert_event(dao, "user.message", {"msg": "hi"}, 1100, skill_run_id=None)
    _insert_event(dao, "skill.start", {"skill": "minimax-docx", "mode": "auto"}, 2000, skill_run_id="sr-2")
    _insert_event(dao, "skill.finish", {"status": "ok"}, 2100, skill_run_id="sr-2")
    conn.close()

    outcomes = query_recent_skill_outcomes(db_path)
    assert len(outcomes) == 1
    assert outcomes[0]["skill_run_id"] == "sr-2"


def test_handles_corrupt_payload_gracefully(db_path):
    """payload JSON 损坏 → 用空 dict，不崩。"""
    conn = connect(db_path)
    # 先建 session / skill_run 父行（避免 FK 约束错误）
    conn.execute(
        "INSERT INTO session (id, channel, created_at, last_active_at) VALUES ('s1', 'test', 1000, 1000)"
    )
    conn.execute(
        "INSERT INTO skill_run (id, session_id, skill_name, mode, risk, status, started_at, total_steps) "
        "VALUES ('sr-1', 's1', 'test', 'auto', 'low', 'running', 1000, 1)"
    )
    # 直接写坏 payload
    conn.execute(
        """
        INSERT INTO audit_event (ts_ms, level, event, payload, skill_run_id, session_id)
        VALUES (?, ?, ?, ?, ?, ?)
        """,
        (1000, "Info", "skill.start", "not-valid-json{", "sr-1", "s1"),
    )
    conn.execute(
        """
        INSERT INTO audit_event (ts_ms, level, event, payload, skill_run_id, session_id)
        VALUES (?, ?, ?, ?, ?, ?)
        """,
        (1500, "Info", "skill.finish", "also-bad{", "sr-1", "s1"),
    )
    conn.commit()
    conn.close()

    outcomes = query_recent_skill_outcomes(db_path)
    # 坏 payload 应该 fallback 到空 dict，不崩
    assert len(outcomes) == 1
    assert outcomes[0]["skill_name"] == "unknown"
    assert outcomes[0]["status"] == "unknown"


def test_duration_ms_zero_or_positive(db_path):
    """duration_ms >= 0（同毫秒事件时为 0）。"""
    conn = connect(db_path)
    dao = AuditEventDAO(conn)
    _insert_event(dao, "skill.start", {"skill": "x", "mode": "auto"}, 1000, skill_run_id="sr-1")
    _insert_event(dao, "skill.finish", {"status": "ok"}, 1000, skill_run_id="sr-1")  # same ms
    conn.close()

    outcomes = query_recent_skill_outcomes(db_path)
    assert outcomes[0]["duration_ms"] == 0


def test_started_at_iso_format(db_path):
    """started_at 是 ISO 8601 字符串（不依赖 timezone，只验证格式）。"""
    conn = connect(db_path)
    dao = AuditEventDAO(conn)
    # 用近未来的 timestamp，避免 timezone 计算
    _insert_event(dao, "skill.start", {"skill": "x", "mode": "auto"}, 1700000000000, skill_run_id="sr-1")
    conn.close()

    outcomes = query_recent_skill_outcomes(db_path)
    # 验证 ISO 8601 格式（"YYYY-MM-DDTHH:MM:SS" + 可选微秒）
    started_at = outcomes[0]["started_at"]
    import re
    assert re.match(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}", started_at), started_at


# ───────────────────────── format_outcomes_table ─────────────────────────


def test_format_table_empty():
    """空 outcomes → "no skill outcomes found"。"""
    assert format_outcomes_table([]) == "(no skill outcomes found)"


def test_format_table_basic():
    """基本格式：header + 分隔线 + 行。"""
    outcomes = [
        {
            "skill_run_id": "sr-abc123def456",
            "skill_name": "minimax-docx",
            "mode": "auto",
            "status": "ok",
            "duration_ms": 1500,
            "started_at": "2026-08-22T20:59:44",
        }
    ]
    out = format_outcomes_table(outcomes)
    assert "skill_run_id" in out
    assert "skill_name" in out
    assert "minimax-docx" in out
    assert "ok" in out
    assert "1500ms" in out


def test_format_table_truncates_skill_run_id():
    """skill_run_id 截断到 18 字符（表格美观）。"""
    outcomes = [
        {
            "skill_run_id": "sr-" + "x" * 50,  # 超长
            "skill_name": "y",
            "mode": "auto",
            "status": "ok",
            "duration_ms": 100,
            "started_at": "2026-08-22T20:00:00",
        }
    ]
    out = format_outcomes_table(outcomes)
    # sr-xxxxxxxxx... 截到 18 字符
    assert "sr-" in out
    assert "x" * 30 not in out  # 超长部分不出现


def test_format_table_in_progress_no_duration():
    """in_progress 的 outcome duration 显示 '-'。"""
    outcomes = [
        {
            "skill_run_id": "sr-1",
            "skill_name": "x",
            "mode": "auto",
            "status": "in_progress",
            "duration_ms": None,
            "started_at": "2026-08-22T20:00:00",
        }
    ]
    out = format_outcomes_table(outcomes)
    assert "in_progress" in out
    assert " - " in out  # duration column


# ───────────────────────── summarize_status ─────────────────────────


def test_summarize_status_mixed():
    """汇总 status 分布。"""
    outcomes = [
        {"status": "ok"},
        {"status": "ok"},
        {"status": "failed"},
        {"status": "in_progress"},
        {"status": "unknown"},
        {"status": "ok"},
    ]
    summary = summarize_status(outcomes)
    assert summary == {"ok": 3, "failed": 1, "in_progress": 1, "unknown": 1}


def test_summarize_status_empty():
    """空 list → 空 dict。"""
    assert summarize_status([]) == {}


# ───────────────────────── CLI 集成（main.py --query-skill-outcomes）────────────────────────


def test_cli_query_skill_outcomes_json_format(tmp_path, capsys):
    """CLI --query-skill-outcomes --format json 输出 JSON（含 outcomes + summary）。"""
    db_path = str(tmp_path / "agent.db")
    conn = connect(db_path)
    migrate(conn, MIGRATIONS_DIR)
    dao = AuditEventDAO(conn)
    _insert_event(dao, "skill.start", {"skill": "minimax-docx", "mode": "auto"}, 1000, skill_run_id="sr-1")
    _insert_event(dao, "skill.finish", {"status": "ok"}, 1500, skill_run_id="sr-1")
    conn.close()

    import subprocess
    import sys as _sys

    out = subprocess.run(
        [_sys.executable, "-m", "main", "--query-skill-outcomes", "--db", db_path, "--format", "json"],
        capture_output=True, text=True, cwd=PROJECT_ROOT,
    )
    assert out.returncode == 0
    data = json.loads(out.stdout)
    # CLI 输出 dict 含 outcomes / summary / count
    assert "outcomes" in data
    assert "summary" in data
    assert "count" in data
    assert isinstance(data["outcomes"], list)
    assert data["count"] == 1
    assert data["summary"] == {"ok": 1}
    assert data["outcomes"][0]["skill_name"] == "minimax-docx"
    assert data["outcomes"][0]["status"] == "ok"


def test_cli_query_skill_outcomes_table_format(tmp_path):
    """CLI --query-skill-outcomes 默认 table 格式。"""
    db_path = str(tmp_path / "agent.db")
    conn = connect(db_path)
    migrate(conn, MIGRATIONS_DIR)
    dao = AuditEventDAO(conn)
    _insert_event(dao, "skill.start", {"skill": "minimax-docx", "mode": "auto"}, 1000, skill_run_id="sr-1")
    _insert_event(dao, "skill.finish", {"status": "ok"}, 1500, skill_run_id="sr-1")
    conn.close()

    import subprocess
    import sys as _sys
    
    out = subprocess.run(
        [_sys.executable, "-m", "main", "--query-skill-outcomes", "--db", db_path],
        capture_output=True, text=True, cwd=PROJECT_ROOT,
    )
    assert out.returncode == 0
    assert "skill_name" in out.stdout
    assert "minimax-docx" in out.stdout
    assert "ok" in out.stdout


def test_cli_db_not_found_returns_error(tmp_path):
    """DB 不存在 → exit code 1 + 错误信息。"""
    import subprocess
    import sys as _sys
    
    out = subprocess.run(
        [_sys.executable, "-m", "main", "--query-skill-outcomes", "--db", str(tmp_path / "no.db")],
        capture_output=True, text=True, cwd=PROJECT_ROOT,
    )
    assert out.returncode != 0
    assert "not found" in out.stderr.lower() or "no.db" in out.stderr
