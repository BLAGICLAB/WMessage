"""Skill Registry + Tool Guard + AuditLogger SQLite 扩展测试。"""
from __future__ import annotations

import os
import tempfile
from pathlib import Path

import pytest

from agent_core.skill_registry import SkillRegistry, DEFAULT_CUSTOM_SKILLS_DIRS
from agent_core.tool_guard import (
    ToolGuard,
    ToolGuardConfig,
    DEFAULT_BLACKLIST,
    DEFAULT_WHITELIST,
)
from agent_core.audit_sqlite import SqliteAuditLogger
from agent_core.audit_log import AuditLevel, AuditEvent
from agent_core.db import AuditEventDAO, connect


# ───────────────────────── SkillRegistry ─────────────────────────


def test_skill_registry_empty_init():
    """空 registry 不含任何 skill。"""
    reg = SkillRegistry()
    assert len(reg) == 0
    assert reg.list_all() == []


def test_skill_registry_register_builtin():
    """register_builtin 加载 3 个 builtin。"""
    reg = SkillRegistry()
    n = reg.register_builtin()
    assert n == 3
    assert len(reg) == 3
    assert "minimax-docx" in reg
    assert "minimax-xlsx" in reg
    assert "minimax-pptx" in reg


def test_skill_registry_register_builtin_idempotent():
    """register_builtin 多次调用不重复注册。"""
    reg = SkillRegistry()
    reg.register_builtin()
    n2 = reg.register_builtin()
    assert n2 == 0  # 已注册的跳过


def test_skill_registry_find_by_intent():
    """find_by_intent 关键词匹配。"""
    reg = SkillRegistry()
    reg.register_builtin()
    assert reg.find_by_intent("做个 word").name == "minimax-docx"
    assert reg.find_by_intent("做个表格").name == "minimax-xlsx"
    assert reg.find_by_intent("做个 PPT").name == "minimax-pptx"


def test_skill_registry_find_no_match():
    """无命中返回 None。"""
    reg = SkillRegistry()
    reg.register_builtin()
    assert reg.find_by_intent("讲个笑话") is None


def test_skill_registry_load_custom_dir(tmp_path):
    """从自定义目录加载 SKILL.md。"""
    # 创建测试用 skill 目录
    skill_dir = tmp_path / "my-skill"
    skill_dir.mkdir()
    (skill_dir / "SKILL.md").write_text(
        "---\n"
        "name = \"my-skill\"\n"
        "description = \"我的自定义 skill\"\n"
        "mode = \"auto\"\n"
        "risk = \"low\"\n"
        "intents = [\"做测试\", \"跑测试\"]\n"
        "tools_whitelist = [\"run_tests\"]\n"
        "---\n\n"
        "# My Skill\n"
    )

    reg = SkillRegistry()
    n = reg.load_custom_dir(str(tmp_path))
    assert n == 1
    assert "my-skill" in reg
    skill = reg.get("my-skill")
    assert skill.tools_whitelist == ["run_tests"]
    assert "做测试" in skill.intents


def test_skill_registry_builtin_wins_over_custom(tmp_path):
    """同名 custom 不覆盖 builtin。"""
    skill_dir = tmp_path / "minimax-docx"
    skill_dir.mkdir()
    (skill_dir / "SKILL.md").write_text(
        "---\n"
        "name = \"minimax-docx\"\n"
        "description = \"覆盖版\"\n"
        "intents = [\"\"]\n"
        "tools_whitelist = [\"\"]\n"
        "---\n"
    )

    reg = SkillRegistry()
    reg.register_builtin()
    n = reg.load_custom_dir(str(tmp_path))
    assert n == 0  # builtin 已存在，custom 不覆盖
    skill = reg.get("minimax-docx")
    assert skill.description != "覆盖版"


def test_skill_registry_nonexistent_dir():
    """不存在的目录 → 0 个 skill（不报错）。"""
    reg = SkillRegistry()
    n = reg.load_custom_dir("/nonexistent/path/here")
    assert n == 0


def test_skill_registry_load_default():
    """load_default 返回 builtin + 0+ custom。"""
    reg = SkillRegistry()
    n = reg.load_default()
    assert n >= 3  # 至少 3 个 builtin
    assert len(reg) >= 3


def test_skill_registry_load_defaults_verbose():
    """load_defaults_verbose 返回 (count, details)。"""
    reg = SkillRegistry()
    count, details = reg.load_defaults_verbose()
    assert count >= 3
    assert isinstance(details, dict)
    for d in DEFAULT_CUSTOM_SKILLS_DIRS:
        assert d in details


def test_skill_registry_names():
    """names() 返回所有 skill 名。"""
    reg = SkillRegistry()
    reg.register_builtin()
    assert set(reg.names()) == {"minimax-docx", "minimax-xlsx", "minimax-pptx"}


def test_skill_registry_contains():
    """__contains__ 检查 name。"""
    reg = SkillRegistry()
    reg.register_builtin()
    assert "minimax-docx" in reg
    assert "nonexistent" not in reg


def test_skill_registry_get():
    """get(name) 查 skill。"""
    reg = SkillRegistry()
    reg.register_builtin()
    assert reg.get("minimax-docx") is not None
    assert reg.get("nonexistent") is None


def test_custom_to_builtin_minimal_fm():
    """最小 frontmatter（只有 name）→ BuiltinSkill 默认值。"""
    skill = SkillRegistry._custom_to_builtin(
        "x", {"name": "x", "intents": ["t"], "tools_whitelist": ["a"]}
    )
    assert skill is not None
    assert skill.mode == "auto"
    assert skill.risk == "low"
    assert skill.react_max_steps == 15
    assert skill.timeout_s == 60


def test_custom_to_builtin_missing_name():
    """缺 name → None。"""
    assert SkillRegistry._custom_to_builtin("x", {"intents": []}) is None


def test_custom_to_builtin_string_intents():
    """intents 是字符串（不是列表）→ 单元素列表。"""
    skill = SkillRegistry._custom_to_builtin(
        "x", {"name": "x", "intents": "single"}
    )
    assert skill.intents == ["single"]


# ───────────────────────── ToolGuard ─────────────────────────


def test_tool_guard_default_config():
    """ToolGuard 默认配置（黑名单模式）。"""
    guard = ToolGuard()
    assert guard.config.whitelist_only is False
    assert "create_word_revisions" in guard.config.blacklist
    assert "link_file_to_task" in guard.config.blacklist


def test_tool_guard_allows_normal_tool():
    """正常工具允许。"""
    guard = ToolGuard()
    d = guard.check("list_tasks", {})
    assert d.allowed is True
    assert d.tool == "list_tasks"


def test_tool_guard_denies_blacklisted():
    """黑名单工具拒绝。"""
    guard = ToolGuard()
    d = guard.check("create_word_revisions", {})
    assert d.allowed is False
    assert "黑名单" in d.reason


def test_tool_guard_denies_link_file_to_task():
    """link_file_to_task 黑名单。"""
    guard = ToolGuard()
    d = guard.check("link_file_to_task", {"path": "x"})
    assert d.allowed is False


def test_tool_guard_whitelist_mode_allows_listed():
    """白名单模式 → 列出的允许。"""
    guard = ToolGuard(ToolGuardConfig(
        blacklist=frozenset(),
        whitelist=DEFAULT_WHITELIST,
        whitelist_only=True,
    ))
    assert guard.check("list_tasks", {}).allowed is True
    assert guard.check("doc_make_word", {}).allowed is True


def test_tool_guard_whitelist_mode_denies_unlisted():
    """白名单模式 → 未列出的拒绝。"""
    guard = ToolGuard(ToolGuardConfig(
        blacklist=frozenset(),
        whitelist=DEFAULT_WHITELIST,
        whitelist_only=True,
    ))
    d = guard.check("unknown_tool", {})
    assert d.allowed is False
    assert "白名单" in d.reason


def test_tool_guard_both_modes():
    """黑名单优先于白名单（在白名单中 + 黑名单 = 黑名单）。"""
    guard = ToolGuard(ToolGuardConfig(
        blacklist=frozenset({"doc_make_word"}),
        whitelist=DEFAULT_WHITELIST,  # 包含 doc_make_word
        whitelist_only=True,
    ))
    d = guard.check("doc_make_word", {})
    assert d.allowed is False  # 黑名单赢


def test_tool_guard_config_from_dict():
    """ToolGuardConfig.from_dict 从 dict 构造。"""
    cfg = ToolGuardConfig.from_dict({
        "blacklist": ["dangerous_tool"],
        "whitelist": ["safe_tool"],
        "whitelist_only": True,
    })
    assert "dangerous_tool" in cfg.blacklist
    assert "safe_tool" in cfg.whitelist
    assert cfg.whitelist_only is True


def test_tool_guard_config_from_empty_dict():
    """空 dict → 默认配置。"""
    cfg = ToolGuardConfig.from_dict({})
    assert "create_word_revisions" in cfg.blacklist


def test_tool_guard_config_from_none():
    """None → 默认配置。"""
    cfg = ToolGuardConfig.from_dict(None)
    assert cfg.whitelist_only is False


def test_guard_decision_to_dict():
    """GuardDecision.to_dict。"""
    from agent_core.tool_guard import GuardDecision
    d = GuardDecision(allowed=False, reason="x", tool="y")
    assert d.to_dict() == {"allowed": False, "reason": "x", "tool": "y"}


# ───────────────────────── SqliteAuditLogger ─────────────────────────


def test_sqlite_audit_logger_no_db_path(tmp_path):
    """db_path 为 None → 仅写文件。"""
    log_file = tmp_path / "audit.log"
    logger = SqliteAuditLogger(path=log_file, db_path=None)
    logger.info("test.event", k="v")
    assert log_file.exists()
    content = log_file.read_text()
    assert "test.event" in content


def test_sqlite_audit_logger_persists_to_db(tmp_path):
    """db_path 提供 → 写文件 + 写 SQLite。"""
    log_file = tmp_path / "audit.log"
    db_path = tmp_path / "agent.db"

    logger = SqliteAuditLogger(path=log_file, db_path=db_path)
    logger.user_message(session_id="s1", message="hello")

    # 文件写了
    assert log_file.exists()

    # SQLite 也写了（schema 自动 migrate）
    conn = connect(db_path)
    dao = AuditEventDAO(conn)
    events = dao.query(session_id="s1")
    assert len(events) == 1
    assert events[0]["event"] == AuditEvent.USER_MESSAGE
    assert events[0]["level"] == "Info"
    conn.close()


def test_sqlite_audit_logger_multiple_events(tmp_path):
    """多个事件都落 SQLite。"""
    db_path = tmp_path / "agent.db"

    logger = SqliteAuditLogger(db_path=db_path)
    logger.info("event.a", k="v1")
    logger.warn("event.b", k="v2")
    logger.error("event.c", k="v3")

    conn = connect(db_path)
    dao = AuditEventDAO(conn)
    all_events = dao.query(limit=10)
    assert len(all_events) == 3
    levels = {e["level"] for e in all_events}
    assert levels == {"Info", "Warn", "Error"}
    conn.close()


def test_sqlite_audit_logger_db_failure_doesnt_break(tmp_path):
    """SQLite 写失败 → 不影响文件日志。"""
    # 故意指向不存在的路径触发 IO 错误
    log_file = tmp_path / "audit.log"
    logger = SqliteAuditLogger(
        path=log_file,
        db_path=tmp_path / "nonexistent" / ".." / ".." / "etc" / "readonly" / "x.db",
    )
    # 不应抛错
    logger.info("test.event", k="v")
    # 文件还是写了
    assert log_file.exists()


def test_sqlite_audit_logger_convenience_methods(tmp_path):
    """便利方法（tool_call / skill_start 等）也落 SQLite。"""
    db_path = tmp_path / "agent.db"

    logger = SqliteAuditLogger(db_path=db_path)
    logger.tool_call(task_id="t1", tool="read_file", args_preview={"path": "x"})
    logger.tool_return(task_id="t1", tool="read_file", result_preview="ok")
    logger.skill_start(skill_run_id="sr-1", skill="test-skill", mode="auto")

    conn = connect(db_path)
    dao = AuditEventDAO(conn)
    events = dao.query(limit=10)
    events_set = {e["event"] for e in events}
    assert AuditEvent.TOOL_CALL in events_set
    assert AuditEvent.TOOL_RETURN in events_set
    assert AuditEvent.SKILL_START in events_set
    conn.close()


def test_summarize_truncates():
    """_summarize 截断 200 字符。"""
    from agent_core.audit_sqlite import SqliteAuditLogger
    long = "x" * 500
    out = SqliteAuditLogger._summarize("e", {"tool": long})
    assert len(out) <= 200
