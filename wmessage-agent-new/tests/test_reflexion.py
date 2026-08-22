"""Reflexion 反思循环测试 — Phase 5。

覆盖：
- rule-based 反思（无 LLM）
- 存储 + 检索
- format_for_prompt 输出
- 集成到 AgentLoop（成功 + 失败都触发 reflect）
- DAO 单元测试
"""
from __future__ import annotations

from pathlib import Path

import pytest

from agent_core.db import ReflexionDAO, connect, migrate
from agent_core.reflexion import ReflexionEngine
from agent_core.types import TaskNode, VerifyHint
from agent_core.verify import CommandResult


PROJECT_ROOT = Path(__file__).parent.parent
MIGRATIONS_DIR = PROJECT_ROOT / "db" / "migrations"


@pytest.fixture
def db_path(tmp_path):
    p = str(tmp_path / "agent.db")
    conn = connect(p)
    migrate(conn, MIGRATIONS_DIR)
    conn.close()
    return p


def _stub_llm(n=3):
    """返回 n 个 task_done 响应（覆盖 replan 触发后的多次 LLM 调用）。"""
    from tests.conftest import MockLLM
    return MockLLM(["Thought: ok\nAction: task_done\nAction Input: done"] * n)


# ───────────────────────── _rule_reflect ─────────────────────────


class TestRuleReflect:
    def setup_method(self):
        self.engine = ReflexionEngine(db_path="/tmp/x")

    def test_timeout_error(self):
        why, improvement = self.engine._rule_reflect("执行超时 60s")
        assert why == "timeout"
        assert "timeout" in improvement.lower()

    def test_verify_error_chinese(self):
        why, improvement = self.engine._rule_reflect("verify failed: 断言失败")
        assert why == "verify_failed"
        assert "verify" in improvement.lower() or "断言" in improvement

    def test_import_error(self):
        why, improvement = self.engine._rule_reflect(
            "ModuleNotFoundError: No module named 'foo'"
        )
        assert why == "missing_dependency"
        assert "import" in improvement.lower() or "依赖" in improvement

    def test_permission_error(self):
        why, improvement = self.engine._rule_reflect("PermissionError: [Errno 13]")
        assert why == "permission_denied"

    def test_command_failed_error(self):
        why, improvement = self.engine._rule_reflect("Command failed with exit_code=1")
        assert why == "command_failed"

    def test_unknown_error(self):
        why, improvement = self.engine._rule_reflect("some random weird failure")
        assert why == "unknown"


# ───────────────────────── reflect + 存储 ─────────────────────────


class TestReflectStore:
    def test_success_task_stores_short_record(self, db_path):
        engine = ReflexionEngine(db_path=db_path)
        task = TaskNode(id="t1", title="成功任务")
        refl = engine.reflect(task, task_result_status="ok", task_run_id="sr-1")

        assert refl.task_status == "ok"
        assert refl.task_title == "成功任务"
        assert refl.what_went_wrong == ""

        dao = ReflexionDAO(connect(db_path))
        stored = dao.list_recent()
        assert len(stored) == 1
        assert stored[0]["task_title"] == "成功任务"
        assert stored[0]["task_status"] == "ok"

    def test_failed_task_stores_full_reflection(self, db_path):
        engine = ReflexionEngine(db_path=db_path)
        task = TaskNode(id="t1", title="失败任务")
        refl = engine.reflect(
            task, task_result_status="failed",
            task_result_error="执行超时 60s", task_run_id="sr-1",
        )

        assert refl.task_status == "failed"
        assert refl.what_went_wrong == "执行超时 60s"
        assert refl.why == "timeout"  # 规则引擎分类
        assert "timeout" in refl.improvement.lower()

        dao = ReflexionDAO(connect(db_path))
        stored = dao.list_recent()
        assert len(stored) == 1
        assert stored[0]["why"] == "timeout"

    def test_multiple_reflections_ordered_by_ts(self, db_path):
        engine = ReflexionEngine(db_path=db_path)

        for i in range(3):
            task = TaskNode(id=f"t{i}", title=f"任务{i}")
            engine.reflect(task, task_result_status="ok", task_run_id=f"sr-{i}")

        dao = ReflexionDAO(connect(db_path))
        recent = dao.list_recent(limit=3)
        assert len(recent) == 3
        # 最新的在前面
        assert recent[0]["task_title"] == "任务2"
        assert recent[1]["task_title"] == "任务1"
        assert recent[2]["task_title"] == "任务0"


# ───────────────────────── recall_relevant ─────────────────────────


class TestRecallRelevant:
    def test_recall_by_keyword(self, db_path):
        engine = ReflexionEngine(db_path=db_path)

        engine.reflect(TaskNode(id="t1", title="修复 minimax-docx bug"), "ok")
        engine.reflect(TaskNode(id="t2", title="更新 minimax-pptx 样式"), "ok")
        engine.reflect(TaskNode(id="t3", title="部署 minimax-xlsx"), "ok")

        relevant = engine.recall_relevant("修复 minimax-docx 标题", top_k=3)
        assert len(relevant) >= 1
        assert "docx" in relevant[0]["task_title"].lower()

    def test_recall_limit(self, db_path):
        engine = ReflexionEngine(db_path=db_path)

        for i in range(10):
            engine.reflect(TaskNode(id=f"t{i}", title=f"minimax-docx task {i}"), "ok")

        relevant = engine.recall_relevant("minimax-docx", top_k=3)
        assert len(relevant) == 3

    def test_recall_empty(self, db_path):
        engine = ReflexionEngine(db_path=db_path)
        relevant = engine.recall_relevant("any query", top_k=3)
        assert relevant == []


# ───────────────────────── format_for_prompt ─────────────────────────


class TestFormatForPrompt:
    def test_format_empty(self):
        engine = ReflexionEngine(db_path="/tmp/x")
        assert engine.format_for_prompt([]) == ""

    def test_format_with_reflections(self):
        engine = ReflexionEngine(db_path="/tmp/x")
        reflections = [
            {
                "task_title": "测试任务",
                "task_status": "failed",
                "what_went_wrong": "执行超时",
                "why": "timeout",
                "improvement": "考虑增加 timeout",
                "started_at": "2026-08-22T20:00:00",
            }
        ]
        out = engine.format_for_prompt(reflections)
        assert "测试任务" in out
        assert "failed" in out or "\u274c" in out
        assert "执行超时" in out
        assert "timeout" in out


# ───────────────────────── DAO 单元测试 ─────────────────────────


class TestReflexionDAO:
    def test_insert_and_get(self, db_path):
        conn = connect(db_path)
        dao = ReflexionDAO(conn)
        rid = dao.insert(
            task_title="test task",
            task_status="failed",
            what_went_wrong="boom",
            why="unknown",
            improvement="fix it",
            ts_ms=1000,
        )
        assert rid > 0
        rec = dao.get(rid)
        assert rec is not None
        assert rec["task_title"] == "test task"
        assert rec["extra"] == {}

    def test_list_recent_with_status_filter(self, db_path):
        conn = connect(db_path)
        dao = ReflexionDAO(conn)
        dao.insert(task_title="a", task_status="ok", ts_ms=1000)
        dao.insert(task_title="b", task_status="failed", ts_ms=2000)
        dao.insert(task_title="c", task_status="ok", ts_ms=3000)

        ok_only = dao.list_recent(task_status="ok")
        assert len(ok_only) == 2
        assert all(r["task_status"] == "ok" for r in ok_only)

    def test_insert_with_extra(self, db_path):
        conn = connect(db_path)
        dao = ReflexionDAO(conn)
        rid = dao.insert(
            task_title="x", task_status="ok",
            extra={"skill_run_id": "sr-1", "extra_field": 42},
        )
        rec = dao.get(rid)
        assert rec["extra"] == {"skill_run_id": "sr-1", "extra_field": 42}


# ───────────────────────── AgentLoop 集成 ─────────────────────────


class TestAgentLoopIntegration:
    def test_success_task_calls_reflect(self, tmp_path):
        from agent_core.agent_loop import AgentLoop, AgentLoopConfig
        from agent_core.planner import TaskGraph
        from agent_core.replanner import ReplanPolicy
        from tests.conftest import MockVerifyExecutor

        db = str(tmp_path / "agent.db")
        conn = connect(db)
        migrate(conn, MIGRATIONS_DIR)
        conn.close()

        reflexion = ReflexionEngine(db_path=db)
        task = TaskNode(
            id="t1", title="成功任务",
            verify_hint=VerifyHint(kind="command", command="echo ok", expect="exit_0"),
        )
        graph = TaskGraph(tasks=[task], root_query="x")
        executor = MockVerifyExecutor()  # 默认 OK
        loop = AgentLoop(
            llm=_stub_llm(),
            executor=executor,
            audit=None,
            config=AgentLoopConfig(replan_policy=ReplanPolicy()),
            reflexion_engine=reflexion,
        )
        loop.run(graph)

        stored = ReflexionDAO(connect(db)).list_recent()
        assert len(stored) == 1
        assert stored[0]["task_status"] == "ok"
        assert stored[0]["task_title"] == "成功任务"

    def test_failed_task_calls_reflect(self, tmp_path):
        from agent_core.agent_loop import AgentLoop, AgentLoopConfig
        from agent_core.planner import TaskGraph
        from agent_core.replanner import ReplanPolicy
        from tests.conftest import MockVerifyExecutor

        db = str(tmp_path / "agent.db")
        conn = connect(db)
        migrate(conn, MIGRATIONS_DIR)
        conn.close()

        reflexion = ReflexionEngine(db_path=db)
        task = TaskNode(
            id="t1", title="失败任务",
            verify_hint=VerifyHint(kind="command", command="false", expect="exit_0"),
        )
        graph = TaskGraph(tasks=[task], root_query="x")
        executor = MockVerifyExecutor(
            command_result=CommandResult(1, "", "fail"),
        )
        # disable_replan=True 避免 verify 失败触发 replan 导致 MockLLM 多次调用
        loop = AgentLoop(
            llm=_stub_llm(),
            executor=executor,
            audit=None,
            config=AgentLoopConfig(replan_policy=ReplanPolicy(), enable_replan=False),
            reflexion_engine=reflexion,
        )
        loop.run(graph)

        stored = ReflexionDAO(connect(db)).list_recent(task_status="failed")
        assert len(stored) == 1
        assert stored[0]["why"] in ("command_failed", "unknown")
