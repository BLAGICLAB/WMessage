"""AuditLogger SQLite 扩展 — Phase 3 step 6。

扩展 AuditLogger 支持双写：文件 + SQLite audit_event 表。

设计：
- 默认仅写文件（兼容 Phase 2 行为）
- 显式传入 db_path → 同时落 SQLite（性能 ~0.1ms/事件）
- SQLite 写入失败不阻塞主流程（写文件继续）
"""
from __future__ import annotations

from pathlib import Path
from typing import Any, Dict, Optional, Union

from agent_core.audit_log import AuditLevel, AuditLogger
from agent_core.db import AuditEventDAO, SessionDAO, SkillRunDAO, connect


class SqliteAuditLogger(AuditLogger):
    """AuditLogger 子类：写文件 + SQLite 双写。

    用法：
        logger = SqliteAuditLogger(
            path="/tmp/bot.log",
            db_path="/tmp/agent.db",
        )
        logger.user_message(session_id="s1", message="hello")
        # → 写 bot.log + audit_event 表
    """

    def __init__(
        self,
        path: Optional[Union[str, Path]] = None,
        db_path: Optional[Union[str, Path]] = None,
    ):
        super().__init__(path=path)
        self.db_path = Path(db_path) if db_path else None
        self._cache_dao: Optional[AuditEventDAO] = None
        self._cache_session_dao: Optional[SessionDAO] = None
        # db_path 提供时自动跑 migrations（确保 schema 存在）
        if self.db_path:
            self._ensure_schema()

    def _ensure_schema(self) -> None:
        """确保 SQLite schema 已应用（创建表 + 索引）。"""
        if self.db_path is None:
            return
        try:
            import os
            from agent_core.db import migrate

            # db/migrations 路径
            here = os.path.dirname(os.path.abspath(__file__))
            migrations_dir = os.path.join(
                os.path.dirname(here), "db", "migrations"
            )
            conn = connect(self.db_path)
            migrate(conn, migrations_dir)
            conn.close()
        except Exception:
            # schema 创建失败不阻塞（运行时 INSERT 会失败但不崩）
            pass

    def _ensure_dao(self) -> Optional[AuditEventDAO]:
        """懒加载 DAO（db_path 为 None → 不持久化）。"""
        if self.db_path is None:
            return None
        if self._cache_dao is None:
            try:
                conn = connect(self.db_path)
                self._cache_dao = AuditEventDAO(conn)
                self._cache_session_dao = SessionDAO(conn)
            except Exception:
                return None
        return self._cache_dao

    def log(
        self,
        level: Union[AuditLevel, str],
        event: str,
        **kwargs: Any,
    ) -> str:
        """记录事件：先写文件，再写 SQLite。"""
        line = super().log(level, event, **kwargs)
        # SQLite 写入失败不抛错（best-effort）
        try:
            self._write_to_sqlite(level, event, kwargs)
        except Exception:
            pass
        return line

    def _write_to_sqlite(
        self, level: Union[AuditLevel, str], event: str, kwargs: Dict[str, Any]
    ) -> None:
        """落 audit_event 表。"""
        dao = self._ensure_dao()
        if dao is None:
            return

        # 自动建 parent（避免 FK 约束静默吞 insert）
        self._ensure_parent_rows(kwargs)

        # 提取关联字段
        session_id = kwargs.get("session_id")
        skill_run_id = kwargs.get("skill_run_id")
        graph_id = kwargs.get("graph_id")
        task_node_id = kwargs.get("task_node_id")
        message = kwargs.get("message") or self._summarize(event, kwargs)

        level_str = level.value if isinstance(level, AuditLevel) else str(level)

        dao.insert(
            level=level_str,
            event=event,
            payload=kwargs,
            session_id=session_id,
            skill_run_id=skill_run_id,
            graph_id=graph_id,
            task_node_id=task_node_id,
            message=message,
        )

    def _ensure_parent_rows(self, kwargs: Dict[str, Any]) -> None:
        """audit_event FK 依赖：session / skill_run / graph / task_node — 如不存在自动建。

        业务场景下调用方可能只传 task_id（最常见）或 skill_run_id，FK 链可能断裂。
        自动派生：skill_run → session，task_node → graph → session。
        """
        if self._cache_dao is None:
            return
        conn = self._cache_dao.conn

        # 提取可能存在的 ID
        session_id = kwargs.get("session_id")
        skill_run_id = kwargs.get("skill_run_id")
        graph_id = kwargs.get("graph_id")
        task_node_id = kwargs.get("task_node_id")

        # 1. 派生 session_id（skill_run 或 task_node 没给 session 时派生一个）
        if not session_id and (skill_run_id or task_node_id):
            seed = skill_run_id or task_node_id or "default"
            session_id = f"audit_s_{seed}"
            kwargs["session_id"] = session_id

        # 2. 派生 graph_id（task_node 没给 graph 时派生）
        if task_node_id and not graph_id:
            graph_id = f"audit_g_{task_node_id}"
            kwargs["graph_id"] = graph_id

        # 3. 建 session
        if session_id and self._cache_session_dao:
            if not self._cache_session_dao.get(session_id):
                try:
                    self._cache_session_dao.create(
                        id=session_id,
                        channel="audit_logger",
                        metadata={"source": "auto_created"},
                    )
                except Exception:
                    pass

        # 4. 建 skill_run
        if skill_run_id and session_id:
            try:
                row = conn.execute(
                    "SELECT 1 FROM skill_run WHERE id = ?", (skill_run_id,)
                ).fetchone()
                if row is None:
                    now = int(__import__("time").time() * 1000)
                    conn.execute(
                        "INSERT INTO skill_run (id, session_id, skill_name, mode, risk, "
                        "status, started_at, total_steps) "
                        "VALUES (?, ?, 'audit_auto', 'auto', 'low', 'running', ?, 0)",
                        (skill_run_id, session_id, now),
                    )
                    conn.commit()
            except Exception:
                pass

        # 5. 建 task_graph
        if graph_id and session_id:
            try:
                row = conn.execute(
                    "SELECT 1 FROM task_graph WHERE id = ?", (graph_id,)
                ).fetchone()
                if row is None:
                    now = int(__import__("time").time() * 1000)
                    conn.execute(
                        "INSERT INTO task_graph (id, session_id, root_query, mode, status, "
                        "created_at, updated_at) VALUES (?, ?, '', 'auto', 'planning', ?, ?)",
                        (graph_id, session_id, now, now),
                    )
                    conn.commit()
            except Exception:
                pass

        # 6. 建 task_node
        if task_node_id and graph_id:
            try:
                row = conn.execute(
                    "SELECT 1 FROM task_node WHERE id = ?", (task_node_id,)
                ).fetchone()
                if row is None:
                    now = int(__import__("time").time() * 1000)
                    conn.execute(
                        "INSERT INTO task_node (id, graph_id, idx, title, deps, status, "
                        "created_at, updated_at) VALUES (?, ?, 0, 'audit_auto', '[]', "
                        "'pending', ?, ?)",
                        (task_node_id, graph_id, now, now),
                    )
                    conn.commit()
            except Exception:
                pass

    @staticmethod
    def _summarize(event: str, kwargs: Dict[str, Any]) -> str:
        """生成人类可读摘要（≤ 200 字符）。"""
        parts = [event]
        for k in ("tool", "skill", "task_id"):
            if k in kwargs:
                parts.append(f"{k}={kwargs[k]}")
        return " ".join(parts)[:200]
