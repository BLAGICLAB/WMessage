"""db 测试 — 连接、migration、Session/SkillRun/AuditEvent DAO。"""
from __future__ import annotations

import json
import os
import time

import pytest

from agent_core.db import (
    AuditEventDAO,
    MigrationError,
    SessionDAO,
    SkillRunDAO,
    applied_migrations,
    connect,
    ensure_migrations_table,
    migrate,
)


@pytest.fixture
def db_path(tmp_path):
    return tmp_path / "test_agent.db"


@pytest.fixture
def conn(db_path):
    c = connect(db_path)
    yield c
    c.close()


@pytest.fixture
def db(conn, tmp_path):
    """连接 + 跑过 migrations。"""
    migrations_dir = os.path.join(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
        "db",
        "migrations",
    )
    migrate(conn, migrations_dir)
    return conn


@pytest.fixture
def setup_parents(db):
    """默认 parent 记录：session s1 / graph g-1 / task t-1。

    FK 约束需要：
    - skill_run.session_id → session.id
    - audit_event.{session_id, skill_run_id, graph_id, task_node_id} → 各自表

    注：skill_run 不在此 fixture 建，避免与测试 ID 冲突；需要 skill_run 的测试自己建。
    """
    SessionDAO(db).create(id="s1", channel="webchat", user_id="u1")
    now = int(time.time() * 1000)
    db.execute(
        "INSERT INTO task_graph (id, session_id, root_query, mode, status, "
        "created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        ("g-1", "s1", "test query", "auto", "planning", now, now),
    )
    db.execute(
        "INSERT INTO task_node (id, graph_id, idx, title, deps, status, "
        "created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        ("t-1", "g-1", 0, "test task", "[]", "pending", now, now),
    )
    db.commit()
    return db


# ───────────────────────── 连接 ─────────────────────────


def test_connect_creates_db_file(db_path):
    """connect() 自动建 db 文件 + 父目录。"""
    assert not db_path.exists()
    c = connect(db_path)
    try:
        assert db_path.exists()
    finally:
        c.close()


def test_connect_enables_foreign_keys(conn):
    """connect() 开启 PRAGMA foreign_keys=ON。"""
    cur = conn.execute("PRAGMA foreign_keys")
    assert cur.fetchone()[0] == 1


def test_connect_enables_wal(conn):
    """connect() 启用 WAL 模式。"""
    cur = conn.execute("PRAGMA journal_mode")
    assert cur.fetchone()[0].lower() == "wal"


def test_connect_row_factory_dict(conn):
    """row_factory=Row（dict-like 访问）。"""
    conn.execute("CREATE TABLE t (k TEXT, v INTEGER)")
    conn.execute("INSERT INTO t VALUES ('x', 1)")
    conn.commit()
    cur = conn.execute("SELECT * FROM t")
    row = cur.fetchone()
    assert row["k"] == "x"
    assert row["v"] == 1


# ───────────────────────── Migration ─────────────────────────


def test_migrate_runs_initial(db):
    """migrate() 跑 0001_initial.sql。"""
    applied = applied_migrations(db)
    assert "0001_initial.sql" in applied


def test_migrate_idempotent(db):
    """migrate() 第二次跑跳过已应用的。"""
    applied = applied_migrations(db)
    # 第二次跑应该是空
    ran_again = migrate(db, os.path.join(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
        "db",
        "migrations",
    ))
    assert ran_again == []


def test_migrate_creates_all_tables(db):
    """migration 后所有表存在。"""
    expected = {
        "session",
        "session_message",
        "task_graph",
        "task_node",
        "skill_run",
        "audit_event",
    }
    cur = db.execute(
        "SELECT name FROM sqlite_master WHERE type='table'"
    )
    tables = {r["name"] for r in cur.fetchall()}
    for t in expected:
        assert t in tables


def test_migrate_missing_dir_raises(tmp_path):
    """migrations 目录不存在 → 不报错（空迁移）。"""
    conn = connect(tmp_path / "x.db")
    ran = migrate(conn, tmp_path / "nonexistent")
    assert ran == []


def test_ensure_migrations_table_idempotent(conn):
    """ensure_migrations_table 多次调用不报错。"""
    ensure_migrations_table(conn)
    ensure_migrations_table(conn)


# ───────────────────────── SessionDAO ─────────────────────────


def test_session_create(db):
    dao = SessionDAO(db)
    sid = dao.create(id="s1", channel="webchat", user_id="u1", title="test")
    assert sid == "s1"

    s = dao.get("s1")
    assert s["id"] == "s1"
    assert s["channel"] == "webchat"
    assert s["user_id"] == "u1"
    assert s["title"] == "test"
    assert s["created_at"] > 0
    assert s["last_active_at"] == s["created_at"]


def test_session_get_not_found(db):
    dao = SessionDAO(db)
    assert dao.get("nonexistent") is None


def test_session_create_with_metadata(db):
    dao = SessionDAO(db)
    dao.create(id="s1", channel="webchat", metadata={"k": "v", "list": [1, 2]})
    s = dao.get("s1")
    assert s["metadata"] == {"k": "v", "list": [1, 2]}


def test_session_list_by_user(db):
    dao = SessionDAO(db)
    dao.create(id="s1", channel="webchat", user_id="u1")
    dao.create(id="s2", channel="feishu", user_id="u1")
    dao.create(id="s3", channel="webchat", user_id="u2")
    sessions = dao.list(user_id="u1")
    assert len(sessions) == 2
    assert {s["id"] for s in sessions} == {"s1", "s2"}


def test_session_list_limit(db):
    dao = SessionDAO(db)
    for i in range(5):
        dao.create(id=f"s{i}", channel="webchat", user_id="u1")
    sessions = dao.list(limit=3)
    assert len(sessions) == 3


def test_session_touch_updates_last_active(db):
    dao = SessionDAO(db)
    dao.create(id="s1", channel="webchat")
    s_before = dao.get("s1")
    time.sleep(0.01)
    dao.touch("s1")
    s_after = dao.get("s1")
    assert s_after["last_active_at"] > s_before["last_active_at"]
    assert s_after["created_at"] == s_before["created_at"]


def test_session_append_message(db):
    dao = SessionDAO(db)
    dao.create(id="s1", channel="webchat")
    mid = dao.append_message("s1", "user", "hello")
    assert mid > 0
    messages = dao.get_messages("s1")
    assert len(messages) == 1
    assert messages[0]["role"] == "user"
    assert messages[0]["content"] == "hello"
    assert messages[0]["session_id"] == "s1"


def test_session_append_message_touches_session(db):
    """append_message 自动 touch session（更新 last_active_at）。"""
    dao = SessionDAO(db)
    dao.create(id="s1", channel="webchat")
    s_before = dao.get("s1")
    time.sleep(0.01)
    dao.append_message("s1", "user", "hi")
    s_after = dao.get("s1")
    assert s_after["last_active_at"] > s_before["last_active_at"]


def test_session_message_with_meta(db):
    dao = SessionDAO(db)
    dao.create(id="s1", channel="webchat")
    dao.append_message("s1", "assistant", "ok", meta={"skill_run_id": "sr-1"})
    msgs = dao.get_messages("s1")
    assert msgs[0]["meta"] == {"skill_run_id": "sr-1"}


def test_session_messages_in_order(db):
    dao = SessionDAO(db)
    dao.create(id="s1", channel="webchat")
    dao.append_message("s1", "user", "first")
    dao.append_message("s1", "assistant", "second")
    dao.append_message("s1", "user", "third")
    msgs = dao.get_messages("s1")
    assert [m["content"] for m in msgs] == ["first", "second", "third"]


def test_session_cascade_delete_messages(db):
    """session 删除 → messages 一起删。"""
    dao = SessionDAO(db)
    dao.create(id="s1", channel="webchat")
    dao.append_message("s1", "user", "x")
    db.execute("DELETE FROM session WHERE id = 's1'")
    db.commit()
    msgs = dao.get_messages("s1")
    assert msgs == []


# ───────────────────────── SkillRunDAO ─────────────────────────


def test_skill_run_create(setup_parents):
    dao = SkillRunDAO(setup_parents)
    dao.create(
        id="sr-1",
        session_id="s1",
        skill_name="test-skill",
        mode="auto",
        risk="low",
        total_steps=3,
    )
    r = dao.get("sr-1")
    assert r["skill_name"] == "test-skill"
    assert r["status"] == "running"
    assert r["total_steps"] == 3
    assert r["completed_steps"] == 0
    assert r["ctx"] == []


def test_skill_run_with_graph(setup_parents):
    dao = SkillRunDAO(setup_parents)
    dao.create(
        id="sr-1",
        session_id="s1",
        skill_name="test",
        mode="auto",
        risk="low",
        total_steps=2,
        graph_id="g-1",
    )
    r = dao.get("sr-1")
    assert r["graph_id"] == "g-1"


def test_skill_run_finish_ok(setup_parents):
    dao = SkillRunDAO(setup_parents)
    dao.create(
        id="sr-1", session_id="s1", skill_name="t", mode="auto",
        risk="low", total_steps=3
    )
    dao.finish(
        id="sr-1",
        status="done",
        end_reason="ok",
        completed_steps=3,
        replan_count=1,
        ctx=[{"index": "t1", "result": "x"}],
    )
    r = dao.get("sr-1")
    assert r["status"] == "done"
    assert r["end_reason"] == "ok"
    assert r["completed_steps"] == 3
    assert r["replan_count"] == 1
    assert r["ctx"] == [{"index": "t1", "result": "x"}]
    assert r["finished_at"] is not None


def test_skill_run_finish_with_error(setup_parents):
    dao = SkillRunDAO(setup_parents)
    dao.create(
        id="sr-1", session_id="s1", skill_name="t", mode="auto",
        risk="low", total_steps=3
    )
    dao.finish(
        id="sr-1",
        status="failed",
        end_reason="verify_failed",
        completed_steps=0,
        error="verify failed at t1",
    )
    r = dao.get("sr-1")
    assert r["status"] == "failed"
    assert r["error"] == "verify failed at t1"


def test_skill_run_list_by_session(setup_parents):
    dao = SkillRunDAO(setup_parents)
    dao.create(
        id="sr-1", session_id="s1", skill_name="t", mode="auto",
        risk="low", total_steps=1
    )
    dao.create(
        id="sr-2", session_id="s1", skill_name="t2", mode="auto",
        risk="low", total_steps=2
    )
    # 注：sr-3 用了 session s2，但 setup_parents 只建了 s1，所以这里只能列出 s1 的
    runs = dao.list_by_session("s1")
    assert len(runs) == 2
    assert {r["id"] for r in runs} == {"sr-1", "sr-2"}


# ───────────────────────── AuditEventDAO ─────────────────────────


def test_audit_insert_basic(db):
    dao = AuditEventDAO(db)
    rid = dao.insert(
        level="Info",
        event="user.message",
        payload={"session_id": "s1", "message": "hello"},
    )
    assert rid > 0


def test_audit_insert_with_all_fields(setup_parents):
    # setup_parents 建了 s1 / g-1 / t-1，sr-1 需要再补建（避免 ID 冲突）
    SkillRunDAO(setup_parents).create(
        id="sr-1", session_id="s1", skill_name="t", mode="auto",
        risk="low", total_steps=1, graph_id="g-1"
    )

    dao = AuditEventDAO(setup_parents)
    rid = dao.insert(
        level="Warn",
        event="pre_execute.deny",
        payload={"tool": "dangerous", "reason": "blacklisted"},
        session_id="s1",
        skill_run_id="sr-1",
        graph_id="g-1",
        task_node_id="t-1",
        message="tool dangerous was blocked",
    )
    assert rid > 0
    events = dao.query(session_id="s1")
    assert events[0]["level"] == "Warn"
    assert events[0]["event"] == "pre_execute.deny"
    assert events[0]["task_node_id"] == "t-1"
    assert events[0]["message"] == "tool dangerous was blocked"


def test_audit_query_by_event(db):
    dao = AuditEventDAO(db)
    dao.insert(level="Info", event="tool.call", payload={"tool": "x"})
    dao.insert(level="Info", event="tool.return", payload={"tool": "x"})
    dao.insert(level="Info", event="tool.call", payload={"tool": "y"})
    calls = dao.query(event="tool.call")
    assert len(calls) == 2


def test_audit_query_by_level(db):
    dao = AuditEventDAO(db)
    dao.insert(level="Info", event="x", payload={})
    dao.insert(level="Warn", event="y", payload={})
    dao.insert(level="Error", event="z", payload={})
    errors = dao.query(level="Error")
    assert len(errors) == 1
    assert errors[0]["event"] == "z"


def test_audit_query_limit(db):
    dao = AuditEventDAO(db)
    for i in range(10):
        dao.insert(level="Info", event="x", payload={"i": i})
    events = dao.query(limit=5)
    assert len(events) == 5


def test_audit_query_ordered_desc(db):
    """查询按 ts_ms DESC 排序。"""
    dao = AuditEventDAO(db)
    dao.insert(level="Info", event="x", payload={"i": 1}, ts_ms=1000)
    dao.insert(level="Info", event="x", payload={"i": 2}, ts_ms=2000)
    dao.insert(level="Info", event="x", payload={"i": 3}, ts_ms=3000)
    events = dao.query(event="x")
    assert [e["payload"]["i"] for e in events] == [3, 2, 1]


# ───────────────────────── JSON 序列化 ─────────────────────────


def test_json_roundtrip_dict(db):
    dao = SessionDAO(db)
    meta = {"list": [1, 2], "nested": {"k": "v"}, "unicode": "你好"}
    dao.create(id="s1", channel="webchat", metadata=meta)
    assert dao.get("s1")["metadata"] == meta


def test_json_roundtrip_none(db):
    """None metadata → None。"""
    dao = SessionDAO(db)
    dao.create(id="s1", channel="webchat", metadata=None)
    assert dao.get("s1")["metadata"] is None
