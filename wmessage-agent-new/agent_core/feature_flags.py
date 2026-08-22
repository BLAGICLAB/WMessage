"""Feature Flags — Phase 3 step 7。

灰度开关配置：用户 / 会话 / 功能三粒度。

来源：wmessage-agent-spec.md §X.6 + §Y.5 / feature_flags.toml

Phase 3 实现：
- TOML 配置加载（tomli 已装）
- 三粒度开关（user / session / feature）
- 灰度白名单 / 百分比控制
- 热加载（mtime 检测）

不实现：远程配置中心、A/B test 框架（Phase 4+）
"""
from __future__ import annotations

import os
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional, Set, Union


# ───────────────────────── 配置 schema ─────────────────────────


@dataclass
class FeatureFlags:
    """feature_flags.toml 配置 schema。"""

    runtime: str = "python"  # "python" | "nodejs"（兜底）
    reflexion_enabled: bool = False
    llm_l2_router: bool = False
    orchestrator_worker: bool = False
    llm_primary: str = "minimax-m3"
    llm_fallback: List[str] = field(default_factory=lambda: ["kimi", "deepseek"])
    python_agent_users: List[str] = field(default_factory=list)
    python_agent_sessions: List[str] = field(default_factory=list)
    python_agent_percent: int = 0  # 0-100，灰度百分比（未来用）
    audit_sqlite_enabled: bool = False  # Phase 3 step 6 默认 false 避免副作用
    tool_guard_whitelist_only: bool = False
    extra: Dict[str, Any] = field(default_factory=dict)


def default_feature_flags() -> FeatureFlags:
    """默认配置（保守 — 全部关，Python agent 默认开启）。"""
    return FeatureFlags(
        runtime="python",
        llm_primary="minimax-m3",
        python_agent_percent=100,  # 全量 Python agent
    )


# ───────────────────────── 加载器 ─────────────────────────


def load_feature_flags(path: Optional[Union[str, Path]] = None) -> FeatureFlags:
    """从 TOML 文件加载配置，失败时返回默认。

    Args:
        path: TOML 路径（None = 用默认 config/feature_flags.toml）

    Returns:
        FeatureFlags
    """
    if path is None:
        return default_feature_flags()

    p = Path(path)
    if not p.exists():
        return default_feature_flags()

    try:
        import tomli
        with open(p, "rb") as f:
            data = tomli.load(f)
    except Exception:
        return default_feature_flags()

    return parse_feature_flags(data)


def parse_feature_flags(data: Dict[str, Any]) -> FeatureFlags:
    """从 dict 解析为 FeatureFlags。

    支持嵌套：
    - [agent_runtime] runtime, python_agent_users, ...
    - [features] reflexion_enabled, llm_l2_router, ...
    - [llm] primary, fallback
    - [gray] ...
    """
    runtime_section = data.get("agent_runtime", {})
    features_section = data.get("features", {})
    llm_section = data.get("llm", {})
    gray_section = data.get("gray", {})

    # 收集未知字段到 extra（保留可扩展性）
    known_top_keys = {"agent_runtime", "features", "llm", "gray"}
    extra = {k: v for k, v in data.items() if k not in known_top_keys}

    return FeatureFlags(
        runtime=str(runtime_section.get("runtime", "python")),
        reflexion_enabled=bool(features_section.get("reflexion_enabled", False)),
        llm_l2_router=bool(features_section.get("llm_l2_router", False)),
        orchestrator_worker=bool(features_section.get("orchestrator_worker", False)),
        llm_primary=str(llm_section.get("primary", "minimax-m3")),
        llm_fallback=list(llm_section.get("fallback", ["kimi", "deepseek"])),
        python_agent_users=list(gray_section.get("python_agent_users", [])),
        python_agent_sessions=list(gray_section.get("python_agent_sessions", [])),
        python_agent_percent=int(gray_section.get("python_agent_percent", 0)),
        audit_sqlite_enabled=bool(features_section.get("audit_sqlite_enabled", False)),
        tool_guard_whitelist_only=bool(features_section.get("tool_guard_whitelist_only", False)),
        extra=extra,
    )


# ───────────────────────── 运行时检查 ─────────────────────────


@dataclass
class FlagChecker:
    """运行时 flag 检查器 — 带热加载支持。

    用法：
        checker = FlagChecker(path="config/feature_flags.toml")
        if checker.is_user_in_rollout("u-123"):
            # 用 Python agent
        if checker.is_feature_enabled("llm_l2_router"):
            # 启用 L2 路由
    """

    def __init__(self, path: Optional[Union[str, Path]] = None):
        self.path = Path(path) if path else None
        self._cache: Optional[FeatureFlags] = None
        self._mtime: Optional[float] = None

    def flags(self) -> FeatureFlags:
        """获取当前 flags（带 mtime 热加载缓存）。"""
        if self.path is None or not self.path.exists():
            if self._cache is None:
                self._cache = default_feature_flags()
            return self._cache

        try:
            mtime = self.path.stat().st_mtime
            if self._cache is None or mtime != self._mtime:
                self._cache = load_feature_flags(self.path)
                self._mtime = mtime
        except OSError:
            if self._cache is None:
                self._cache = default_feature_flags()

        return self._cache

    def is_user_in_rollout(self, user_id: Optional[str]) -> bool:
        """判断用户是否在 Python agent 灰度名单。"""
        f = self.flags()
        # 白名单优先
        if user_id and user_id in f.python_agent_users:
            return True
        # 百分比（确定性 hash）
        if f.python_agent_percent >= 100:
            return True
        if f.python_agent_percent <= 0:
            return False
        if user_id is None:
            return False
        # 用 hash 决定：同一 user_id 永远在同一桶
        bucket = hash(user_id) % 100
        return bucket < f.python_agent_percent

    def is_session_in_rollout(self, session_id: Optional[str]) -> bool:
        """判断会话是否在 Python agent 灰度名单。"""
        f = self.flags()
        if session_id and session_id in f.python_agent_sessions:
            return True
        return False

    def is_runtime_python(self) -> bool:
        return self.flags().runtime == "python"

    def is_feature_enabled(self, name: str) -> bool:
        """通用 feature 检查。"""
        f = self.flags()
        return bool(getattr(f, name, False))

    def primary_llm(self) -> str:
        return self.flags().llm_primary

    def fallback_llms(self) -> List[str]:
        return list(self.flags().llm_fallback)


def get_flag_checker(path: Optional[str] = None) -> FlagChecker:
    """获取 FlagChecker — 默认 config/feature_flags.toml。

    Args:
        path: TOML 路径（None = 从 env WMSG_AGENT_FLAGS 取，默认 ./config/feature_flags.toml）

    Returns:
        FlagChecker 实例
    """
    if path is None:
        path = os.environ.get("WMSG_AGENT_FLAGS", "config/feature_flags.toml")
    return FlagChecker(path=path)
