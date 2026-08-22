"""logging_config — stdlib logging 配置（Phase 4 step 1.5）。

设计原则（参考 LangChain / OpenAI / Anthropic 行业惯例）：
1. 零依赖（用 stdlib `logging`）
2. 与 audit_log 正交：
   - audit_log → 结构化业务事件 → SQLite 可查
   - stdlib logging → 程序调试 + 错误栈 → 文件轮转
3. 双 handler：stdout（dev）+ RotatingFileHandler（生产）
4. correlation id (cid)：每个 chat 请求贯穿 handler → LLM → executor

环境变量覆盖：
- WMSG_LOG_LEVEL=DEBUG|INFO|WARNING|ERROR（默认 INFO）
- WMSG_LOG_PATH=data/bot-debug.log（默认 data/bot-debug.log）
- WMSG_LOG_STDOUT=1|0（默认 1）
"""
from __future__ import annotations

import logging
import os
import sys
from logging.handlers import RotatingFileHandler
from typing import Optional

_LOG_FORMAT = "%(asctime)s.%(msecs)03d | %(levelname)-7s | %(name)s | %(message)s"
_DATE_FORMAT = "%Y-%m-%d %H:%M:%S"

# 默认值
DEFAULT_LEVEL = "INFO"
DEFAULT_LOG_PATH = "data/bot-debug.log"
DEFAULT_MAX_BYTES = 10 * 1024 * 1024  # 10 MB
DEFAULT_BACKUP_COUNT = 5

_initialized = False


def setup_logging(
    log_path: Optional[str] = None,
    level: Optional[str] = None,
    enable_stdout: Optional[bool] = None,
    max_bytes: int = DEFAULT_MAX_BYTES,
    backup_count: int = DEFAULT_BACKUP_COUNT,
) -> logging.Logger:
    """配置 root logger — idempotent。

    Args:
        log_path: 日志文件路径（None = 从 env WMSG_LOG_PATH，默认 data/bot-debug.log）
        level: 日志级别 DEBUG/INFO/WARNING/ERROR（None = 从 env WMSG_LOG_LEVEL，默认 INFO）
        enable_stdout: 是否输出到 stdout（None = 从 env WMSG_LOG_STDOUT，默认 True）
        max_bytes: 单文件最大字节（轮转阈值）
        backup_count: 保留历史文件数

    Returns:
        root logger
    """
    global _initialized

    if log_path is None:
        log_path = os.environ.get("WMSG_LOG_PATH", DEFAULT_LOG_PATH)
    if level is None:
        level = os.environ.get("WMSG_LOG_LEVEL", DEFAULT_LEVEL)
    if enable_stdout is None:
        enable_stdout = os.environ.get("WMSG_LOG_STDOUT", "1") not in ("0", "false", "False")

    root = logging.getLogger()
    root.setLevel(level.upper())

    # 避免重复添加 handler（init_runtime 调用多次时）
    if _initialized:
        return root
    _initialized = True

    formatter = logging.Formatter(_LOG_FORMAT, datefmt=_DATE_FORMAT)

    # 1. stdout handler（dev 模式可见）
    if enable_stdout:
        sh = logging.StreamHandler(sys.stderr)
        sh.setFormatter(formatter)
        sh.setLevel(level.upper())
        root.addHandler(sh)

    # 2. 文件轮转 handler（生产模式持久化）
    if log_path:
        # 确保目录存在
        log_dir = os.path.dirname(log_path)
        if log_dir:
            os.makedirs(log_dir, exist_ok=True)

        fh = RotatingFileHandler(
            log_path,
            maxBytes=max_bytes,
            backupCount=backup_count,
            encoding="utf-8",
        )
        fh.setFormatter(formatter)
        fh.setLevel(level.upper())
        root.addHandler(fh)

    # 3. 全局异常兜底（任何未捕获异常 → 文件）
    def _excepthook(exc_type, exc_value, exc_tb):
        if issubclass(exc_type, KeyboardInterrupt):
            sys.__excepthook__(exc_type, exc_value, exc_tb)
            return
        root.error(
            "Uncaught exception",
            exc_info=(exc_type, exc_value, exc_tb),
        )

    sys.excepthook = _excepthook

    root.info(
        "logging initialized: level=%s path=%s stdout=%s max_bytes=%d backup_count=%d",
        level.upper(),
        log_path or "(none)",
        enable_stdout,
        max_bytes,
        backup_count,
    )

    return root


def get_logger(name: str) -> logging.Logger:
    """便利 helper — 调用方用 `get_logger(__name__)`。"""
    return logging.getLogger(name)


def new_cid() -> str:
    """生成新的 correlation id（8 字符 hex）。"""
    import uuid as _uuid

    return _uuid.uuid4().hex[:8]


def shutdown_logging() -> None:
    """关闭所有 handler（测试 cleanup / 进程退出时调用）。"""
    global _initialized

    root = logging.getLogger()
    for handler in list(root.handlers):
        try:
            handler.close()
        except Exception:
            pass
        root.removeHandler(handler)
    _initialized = False
