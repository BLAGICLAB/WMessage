"""logging_config 测试 — stdlib logging 集成。"""
from __future__ import annotations

import logging
import logging.handlers
import os
import sys
from pathlib import Path

import pytest

from agent_core.logging_config import (
    DEFAULT_BACKUP_COUNT,
    DEFAULT_LOG_PATH,
    DEFAULT_MAX_BYTES,
    get_logger,
    new_cid,
    setup_logging,
    shutdown_logging,
)


@pytest.fixture(autouse=True)
def _reset_logging():
    """每个测试后清空 handler + 重置 _initialized 标记。"""
    yield
    shutdown_logging()
    # 恢复默认 level 避免跨测试污染
    logging.getLogger().setLevel(logging.WARNING)


def test_setup_logging_creates_handlers(tmp_path):
    """setup_logging 创建 stdout + 文件 handler。"""
    log_path = str(tmp_path / "test.log")
    setup_logging(log_path=log_path, level="DEBUG")

    root = logging.getLogger()
    handlers = root.handlers
    # 至少 1 个文件 handler（stdout 在 subprocess/capture 模式下可能不存在）
    file_handlers = [h for h in handlers if isinstance(h, logging.handlers.RotatingFileHandler)]
    assert len(file_handlers) >= 1


def test_setup_logging_creates_log_file(tmp_path):
    """setup_logging 创建日志文件并写入。"""
    log_path = str(tmp_path / "test.log")
    setup_logging(log_path=log_path)

    logger = logging.getLogger("wmsg.test")
    logger.info("hello world")
    # RotatingFileHandler 默认 lazy flush，需手动 flush 或 close
    logging.getLogger().handlers[0].flush()

    assert os.path.exists(log_path)
    with open(log_path) as f:
        content = f.read()
    assert "hello world" in content
    assert "INFO" in content


def test_setup_logging_creates_parent_directory(tmp_path):
    """log 文件父目录不存在 → 自动创建。"""
    log_path = str(tmp_path / "nested" / "deep" / "app.log")
    setup_logging(log_path=log_path)
    assert os.path.dirname(log_path)
    assert os.path.isdir(os.path.dirname(log_path))


def test_setup_logging_respects_level(tmp_path):
    """level 参数生效（DEBUG 输出 / INFO 过滤）。"""
    log_path = str(tmp_path / "test.log")
    setup_logging(log_path=log_path, level="ERROR")

    logger = logging.getLogger("wmsg.test")
    logger.debug("debug msg")
    logger.error("error msg")
    logging.getLogger().handlers[0].flush()

    with open(log_path) as f:
        content = f.read()
    assert "error msg" in content
    assert "debug msg" not in content  # DEBUG 被 ERROR 级别过滤


def test_setup_logging_idempotent(tmp_path):
    """多次调用 setup_logging 不重复添加 handler。"""
    log_path = str(tmp_path / "test.log")
    setup_logging(log_path=log_path)
    n1 = len(logging.getLogger().handlers)
    setup_logging(log_path=log_path)  # 再次调用
    n2 = len(logging.getLogger().handlers)
    assert n1 == n2


def test_setup_logging_env_overrides(tmp_path, monkeypatch):
    """环境变量覆盖默认配置。"""
    log_path = str(tmp_path / "env.log")
    monkeypatch.setenv("WMSG_LOG_PATH", log_path)
    monkeypatch.setenv("WMSG_LOG_LEVEL", "WARNING")
    monkeypatch.setenv("WMSG_LOG_STDOUT", "0")

    setup_logging()  # 不传参，走 env

    root = logging.getLogger()
    assert root.level == logging.WARNING
    # log 文件应存在
    logger = logging.getLogger("wmsg.test")
    logger.warning("warn msg")
    logging.getLogger().handlers[0].flush()

    assert os.path.exists(log_path)


def test_setup_logging_rotation(tmp_path):
    """max_bytes 触发文件轮转。"""
    log_path = str(tmp_path / "rot.log")
    # max_bytes=100 → 写 200 字节必轮转
    setup_logging(log_path=log_path, max_bytes=100, backup_count=2)

    logger = logging.getLogger("wmsg.test")
    for i in range(20):
        logger.info("line %d with some padding to exceed the limit", i)
    logging.getLogger().handlers[0].flush()

    # 主文件存在
    assert os.path.exists(log_path)
    # 至少 1 个轮转备份
    rotated = list(tmp_path.glob("rot.log.*"))
    assert len(rotated) >= 1


def test_get_logger_returns_named_logger():
    """get_logger(__name__) 返回命名 logger。"""
    setup_logging()
    logger = get_logger("wmsg.test.module")
    assert logger.name == "wmsg.test.module"
    assert isinstance(logger, logging.Logger)


def test_new_cid_format():
    """new_cid 返回 8 字符 hex。"""
    cid = new_cid()
    assert len(cid) == 8
    assert all(c in "0123456789abcdef" for c in cid)
    # 两次调用返回不同值
    assert new_cid() != new_cid()


def test_excepthook_logs_unhandled_exception(tmp_path):
    """全局 excepthook 捕获未处理异常 → 写日志。"""
    log_path = str(tmp_path / "excepthook.log")
    setup_logging(log_path=log_path)

    # 触发未捕获异常
    try:
        raise ValueError("test uncaught")
    except ValueError:
        sys.excepthook(*sys.exc_info())

    logging.getLogger().handlers[0].flush()

    with open(log_path) as f:
        content = f.read()
    assert "Uncaught exception" in content
    assert "ValueError" in content
    assert "test uncaught" in content


def test_excepthook_keyboard_interrupt_passthrough(tmp_path):
    """KeyboardInterrupt 走原 excepthook（不写日志）。"""
    log_path = str(tmp_path / "excepthook.log")
    setup_logging(log_path=log_path)

    # 模拟 KeyboardInterrupt
    original = sys.excepthook

    class MockKI(Exception):
        pass

    # 用一个继承 KeyboardInterrupt 的异常测试 passthrough
    try:
        raise KeyboardInterrupt()
    except KeyboardInterrupt:
        # excepthook 应调用 sys.__excepthook__
        sys.excepthook(KeyboardInterrupt, KeyboardInterrupt(), None)

    logging.getLogger().handlers[0].flush()

    with open(log_path) as f:
        content = f.read()
    # KeyboardInterrupt 不应触发 "Uncaught exception" 日志
    assert "Uncaught exception" not in content


def test_shutdown_clears_handlers(tmp_path):
    """shutdown 后 handlers 清空，可重新 setup。"""
    log_path = str(tmp_path / "test.log")
    setup_logging(log_path=log_path)
    assert len(logging.getLogger().handlers) >= 1

    shutdown_logging()
    assert len(logging.getLogger().handlers) == 0

    # 重新 setup 应工作
    setup_logging(log_path=log_path)
    assert len(logging.getLogger().handlers) >= 1
