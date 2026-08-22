"""db — SQLite 连接 + migration runner + DAO。

Phase 1 实现：
- connect() 打开 SQLite（默认 data/agent.db）
- migrate() 跑 migrations/*.sql（按文件名顺序，已跑过的跳过）
- session_dao / skill_run_dao / audit_event_dao 三个 DAO 模块
- 在事务里执行（PRAGMA foreign_keys=ON）

数据模型来自 wmessage-agent-spec.md §Z。
"""
from __future__ import annotations

import json
import sqlite3
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Dict, Iterator, List, Optional, Tuple, Union


# ───────────────────────── 连接 / Migration ─────────────────────────


def connect(db_path: Union[str, Path]) -> sqlite3.Connection:
    """打开 SQLite 连接（默认 db_path.parent 自动 mkdir）。

    Args:
        db_path: 数据库文件路径

    Returns:
        sqlite3.Connection — row_factory=Row（dict-like 访问）
    """
    p = Path(db_path)
    p.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(str(p))
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA foreign_keys = ON")
    conn.execute("PRAGMA journal_mode = WAL")
    return conn


def ensure_migrations_table(conn: sqlite3.Connection) -> None:
    """确保 _migrations 表存在（记录已跑 migration）。"""
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS _migrations (
            name TEXT PRIMARY KEY,
            applied_at INTEGER NOT NULL
        )
        """
    )
    conn.commit()


def applied_migrations(conn: sqlite3.Connection) -> List[str]:
    """返回已跑过的 migration 名称列表。"""
    ensure_migrations_table(conn)
    cur = conn.execute("SELECT name FROM _migrations ORDER BY name")
    return [r["name"] for r in cur.fetchall()]


def migrate(conn: sqlite3.Connection, migrations_dir: Union[str, Path]) -> List[str]:
    """跑 migrations_dir 下所有 .sql 文件，按文件名顺序，已跑过跳过。

    Args:
        conn: 数据库连接
        migrations_dir: migrations 目录路径

    Returns:
        本次跑过的 migration 列表

    Note:
        每个 migration 包在一个事务里；失败回滚。
    """
    ensure_migrations_table(conn)
    applied = set(applied_migrations(conn))

    files = sorted(Path(migrations_dir).glob("*.sql"))
    ran: List[str] = []
    for f in files:
        if f.name in applied:
            continue
        sql = f.read_text(encoding="utf-8")
        try:
            conn.executescript(sql)  # executescript 自带事务
            conn.execute(
                "INSERT INTO _migrations (name, applied_at) VALUES (?, ?)",
                (f.name, int(time.time() * 1000)),
            )
            conn.commit()
            ran.append(f.name)
        except sqlite3.Error as e:
            conn.rollback()
            raise MigrationError(f"migration {f.name} 失败：{e}") from e
    return ran


class MigrationError(Exception):
    """migration 执行失败。"""


# ───────────────────────── DAO helpers ─────────────────────────


@contextmanager
def transaction(conn: sqlite3.Connection) -> Iterator[sqlite3.Connection]:
    """with conn.begin() / commit / rollback 上下文。"""
    try:
        yield conn
        conn.commit()
    except Exception:
        conn.rollback()
        raise


def _to_json(v: Any) -> Optional[str]:
    """dict / list → JSON 字符串；None → None。"""
    if v is None:
        return None
    return json.dumps(v, ensure_ascii=False)


def _from_json(v: Optional[str]) -> Any:
    """JSON 字符串 → dict / list；None → None。"""
    if v is None:
        return None
    return json.loads(v)


# ───────────────────────── Session DAO ─────────────────────────


class SessionDAO:
    """session / session_message 表 CRUD。"""

    def __init__(self, conn: sqlite3.Connection):
        self.conn = conn

    def create(
        self,
        id: str,
        channel: str,
        user_id: Optional[str] = None,
        title: Optional[str] = None,
        metadata: Optional[dict] = None,
    ) -> str:
        """创建 session，返回 session_id。"""
        now = int(time.time() * 1000)
        with transaction(self.conn) as c:
            c.execute(
                "INSERT INTO session (id, user_id, channel, title, created_at, last_active_at, metadata) "
                "VALUES (?, ?, ?, ?, ?, ?, ?)",
                (id, user_id, channel, title, now, now, _to_json(metadata)),
            )
        return id

    def get(self, id: str) -> Optional[Dict[str, Any]]:
        """查 session。"""
        cur = self.conn.execute("SELECT * FROM session WHERE id = ?", (id,))
        row = cur.fetchone()
        return _row_to_session(row) if row else None

    def list(
        self,
        user_id: Optional[str] = None,
        limit: int = 50,
    ) -> List[Dict[str, Any]]:
        """列 session（按 last_active_at desc）。"""
        sql = "SELECT * FROM session"
        params: List[Any] = []
        if user_id:
            sql += " WHERE user_id = ?"
            params.append(user_id)
        sql += " ORDER BY last_active_at DESC LIMIT ?"
        params.append(limit)
        cur = self.conn.execute(sql, params)
        return [_row_to_session(r) for r in cur.fetchall()]

    def touch(self, id: str) -> None:
        """更新 last_active_at（每次有新消息时调）。"""
        now = int(time.time() * 1000)
        with transaction(self.conn) as c:
            c.execute("UPDATE session SET last_active_at = ? WHERE id = ?", (now, id))

    def append_message(
        self,
        session_id: str,
        role: str,
        content: str,
        meta: Optional[dict] = None,
    ) -> int:
        """追加一条消息，返回 message id。"""
        ts = int(time.time() * 1000)
        with transaction(self.conn) as c:
            cur = c.execute(
                "INSERT INTO session_message (session_id, role, content, ts_ms, meta) "
                "VALUES (?, ?, ?, ?, ?)",
                (session_id, role, content, ts, _to_json(meta)),
            )
            c.execute(
                "UPDATE session SET last_active_at = ? WHERE id = ?",
                (ts, session_id),
            )
            return cur.lastrowid

    def get_messages(
        self, session_id: str, limit: int = 100
    ) -> List[Dict[str, Any]]:
        """查 session 的消息（按 ts asc）。"""
        cur = self.conn.execute(
            "SELECT * FROM session_message WHERE session_id = ? "
            "ORDER BY ts_ms ASC LIMIT ?",
            (session_id, limit),
        )
        return [_row_to_message(r) for r in cur.fetchall()]


def _row_to_session(row: sqlite3.Row) -> Dict[str, Any]:
    return {
        "id": row["id"],
        "user_id": row["user_id"],
        "channel": row["channel"],
        "title": row["title"],
        "created_at": row["created_at"],
        "last_active_at": row["last_active_at"],
        "metadata": _from_json(row["metadata"]),
    }


def _row_to_message(row: sqlite3.Row) -> Dict[str, Any]:
    return {
        "id": row["id"],
        "session_id": row["session_id"],
        "role": row["role"],
        "content": row["content"],
        "ts_ms": row["ts_ms"],
        "meta": _from_json(row["meta"]),
    }


# ───────────────────────── SkillRun DAO ─────────────────────────


class SkillRunDAO:
    """skill_run 表 CRUD。"""

    def __init__(self, conn: sqlite3.Connection):
        self.conn = conn

    def create(
        self,
        id: str,
        session_id: str,
        skill_name: str,
        mode: str,
        risk: str,
        total_steps: int,
        graph_id: Optional[str] = None,
    ) -> str:
        """创建 skill_run，返回 skill_run_id。"""
        now = int(time.time() * 1000)
        with transaction(self.conn) as c:
            c.execute(
                "INSERT INTO skill_run (id, session_id, graph_id, skill_name, mode, risk, "
                "status, started_at, total_steps) "
                "VALUES (?, ?, ?, ?, ?, ?, 'running', ?, ?)",
                (id, session_id, graph_id, skill_name, mode, risk, now, total_steps),
            )
        return id

    def finish(
        self,
        id: str,
        status: str,
        end_reason: str,
        completed_steps: int,
        replan_count: int = 0,
        ctx: Optional[list] = None,
        error: Optional[str] = None,
    ) -> None:
        """收尾 skill_run。"""
        now = int(time.time() * 1000)
        with transaction(self.conn) as c:
            c.execute(
                "UPDATE skill_run SET status=?, end_reason=?, finished_at=?, "
                "completed_steps=?, replan_count=?, ctx=?, error=? WHERE id=?",
                (
                    status,
                    end_reason,
                    now,
                    completed_steps,
                    replan_count,
                    _to_json(ctx or []),
                    error,
                    id,
                ),
            )

    def get(self, id: str) -> Optional[Dict[str, Any]]:
        cur = self.conn.execute("SELECT * FROM skill_run WHERE id = ?", (id,))
        row = cur.fetchone()
        return _row_to_skill_run(row) if row else None

    def list_by_session(
        self, session_id: str, limit: int = 50
    ) -> List[Dict[str, Any]]:
        cur = self.conn.execute(
            "SELECT * FROM skill_run WHERE session_id = ? "
            "ORDER BY started_at DESC LIMIT ?",
            (session_id, limit),
        )
        return [_row_to_skill_run(r) for r in cur.fetchall()]


def _row_to_skill_run(row: sqlite3.Row) -> Dict[str, Any]:
    return {
        "id": row["id"],
        "session_id": row["session_id"],
        "graph_id": row["graph_id"],
        "skill_name": row["skill_name"],
        "mode": row["mode"],
        "risk": row["risk"],
        "status": row["status"],
        "started_at": row["started_at"],
        "finished_at": row["finished_at"],
        "total_steps": row["total_steps"],
        "completed_steps": row["completed_steps"],
        "replan_count": row["replan_count"],
        "end_reason": row["end_reason"],
        "ctx": _from_json(row["ctx"]),
        "error": row["error"],
    }


# ───────────────────────── AuditEvent DAO ─────────────────────────


class AuditEventDAO:
    """audit_event 表 CRUD。"""

    def __init__(self, conn: sqlite3.Connection):
        self.conn = conn

    def insert(
        self,
        level: str,
        event: str,
        payload: dict,
        session_id: Optional[str] = None,
        skill_run_id: Optional[str] = None,
        graph_id: Optional[str] = None,
        task_node_id: Optional[str] = None,
        message: Optional[str] = None,
        ts_ms: Optional[int] = None,
    ) -> int:
        """追加审计事件，返回 row id。"""
        ts = ts_ms or int(time.time() * 1000)
        with transaction(self.conn) as c:
            cur = c.execute(
                "INSERT INTO audit_event (ts_ms, level, event, session_id, skill_run_id, "
                "graph_id, task_node_id, payload, message) "
                "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    ts,
                    level,
                    event,
                    session_id,
                    skill_run_id,
                    graph_id,
                    task_node_id,
                    _to_json(payload),
                    message,
                ),
            )
            return cur.lastrowid

    def query(
        self,
        session_id: Optional[str] = None,
        event: Optional[str] = None,
        level: Optional[str] = None,
        limit: int = 100,
    ) -> List[Dict[str, Any]]:
        """查审计事件（按 ts desc）。"""
        sql = "SELECT * FROM audit_event WHERE 1=1"
        params: List[Any] = []
        if session_id:
            sql += " AND session_id = ?"
            params.append(session_id)
        if event:
            sql += " AND event = ?"
            params.append(event)
        if level:
            sql += " AND level = ?"
            params.append(level)
        sql += " ORDER BY ts_ms DESC LIMIT ?"
        params.append(limit)
        cur = self.conn.execute(sql, params)
        return [_row_to_audit(r) for r in cur.fetchall()]


def _row_to_audit(row: sqlite3.Row) -> Dict[str, Any]:
    return {
        "id": row["id"],
        "ts_ms": row["ts_ms"],
        "level": row["level"],
        "event": row["event"],
        "session_id": row["session_id"],
        "skill_run_id": row["skill_run_id"],
        "graph_id": row["graph_id"],
        "task_node_id": row["task_node_id"],
        "payload": _from_json(row["payload"]),
        "message": row["message"],
    }
