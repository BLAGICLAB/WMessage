"""Phase 4 step 3 — 生产配置 + env var 集成端到端验证。

验证：
1. config/feature_flags.toml 可被 FlagChecker 加载
2. env var 覆盖 config 文件（高优先级）
3. 完整链路：FlagChecker + env var + build_llm_chain → FallbackChain 或 ScriptedLLM
4. 不同 config 文件路径（WMSG_AGENT_FLAGS env var）走对应文件
"""
from __future__ import annotations

import os
import tempfile
from pathlib import Path

import pytest

from agent_core.feature_flags import FeatureFlags, FlagChecker, default_feature_flags
from llm_provider.base import LLMProviderToReactAdapter, FallbackChain


PROJECT_ROOT = Path(__file__).parent.parent
PROD_CONFIG = PROJECT_ROOT / "config" / "feature_flags.toml"
DEV_CONFIG = PROJECT_ROOT / "config" / "feature_flags.dev.toml"


# ───────────────────────── 配置文件存在性 ─────────────────────────


def test_prod_config_file_exists():
    """生产 config 文件存在。"""
    assert PROD_CONFIG.exists(), f"missing: {PROD_CONFIG}"


def test_dev_config_file_exists():
    """dev config 文件存在。"""
    assert DEV_CONFIG.exists(), f"missing: {DEV_CONFIG}"


def test_prod_config_has_all_required_sections():
    """生产 config 含全部必需 section。"""
    content = PROD_CONFIG.read_text()
    for section in ["[agent_runtime]", "[features]", "[llm]", "[gray]"]:
        assert section in content, f"missing section {section}"


# ───────────────────────── 加载 + env var 覆盖 ─────────────────────────


def test_prod_config_loads_to_flag_checker():
    """FlagChecker 能加载生产 config。"""
    checker = FlagChecker(path=str(PROD_CONFIG))
    flags = checker.flags()
    assert flags.runtime == "python"
    assert flags.llm_primary == "minimax-m3"
    assert "kimi" in flags.llm_fallback
    assert "deepseek" in flags.llm_fallback
    assert flags.llm_l2_router is True
    assert flags.audit_sqlite_enabled is True


def test_dev_config_overrides_prod_settings():
    """dev config 可以覆盖 prod 设置（如 fallback=[]）。"""
    checker = FlagChecker(path=str(DEV_CONFIG))
    flags = checker.flags()
    assert flags.llm_fallback == []  # dev 关闭 fallback
    assert flags.runtime == "python"


def test_flag_checker_hot_reload_on_mtime_change(tmp_path):
    """文件 mtime 变化 → FlagChecker 自动重载（热加载）。"""
    p = tmp_path / "flags.toml"
    p.write_text("[agent_runtime]\nruntime = \"python\"\n")
    checker = FlagChecker(path=str(p))
    assert checker.flags().runtime == "python"

    # 修改文件
    import time
    time.sleep(0.05)
    p.write_text("[agent_runtime]\nruntime = \"nodejs\"\n")
    assert checker.flags().runtime == "nodejs"  # 自动 reload


def test_nonexistent_config_returns_defaults():
    """config 文件不存在 → defaults（runtime=python, percent=100）。"""
    checker = FlagChecker(path="/nonexistent/flags.toml")
    f = checker.flags()
    assert f.runtime == "python"
    assert f.python_agent_percent == 100
    assert f.llm_primary == "minimax-m3"


def test_env_var_wmsg_agent_flags_overrides_default_path(monkeypatch, tmp_path):
    """WMSG_AGENT_FLAGS 环境变量覆盖默认路径。"""
    p = tmp_path / "custom_flags.toml"
    p.write_text("[agent_runtime]\nruntime = \"nodejs\"\n")
    monkeypatch.setenv("WMSG_AGENT_FLAGS", str(p))
    from agent_core.feature_flags import get_flag_checker
    checker = get_flag_checker()  # 不传 path → 用 env
    assert checker.flags().runtime == "nodejs"


# ───────────────────────── LLM chain 集成 ─────────────────────────


def test_full_chain_with_prod_config_no_api_keys(monkeypatch):
    """生产 config + 无 API keys → FallbackChain 0 provider → ScriptedLLM fallback。"""
    for k in ("MINIMAX_API_KEY", "KIMI_API_KEY", "DEEPSEEK_API_KEY"):
        monkeypatch.delenv(k, raising=False)

    from agent_core.feature_flags import get_flag_checker
    from main import build_llm_chain

    checker = get_flag_checker(path=str(PROD_CONFIG))
    llm, info = build_llm_chain(checker)

    # 全缺 → mock
    assert info["mode"] == "mock"
    assert info["reason"] == "no_api_keys_or_init_failed"


def test_full_chain_with_minimax_only(monkeypatch):
    """仅 MINIMAX_API_KEY → 1 provider FallbackChain。"""
    monkeypatch.setenv("MINIMAX_API_KEY", "test-key-1")
    monkeypatch.delenv("KIMI_API_KEY", raising=False)
    monkeypatch.delenv("DEEPSEEK_API_KEY", raising=False)

    from agent_core.feature_flags import get_flag_checker
    from main import build_llm_chain

    checker = get_flag_checker(path=str(PROD_CONFIG))
    llm, info = build_llm_chain(checker)

    assert info["mode"] == "real"
    assert info["providers_loaded"] == ["minimax-m3"]
    assert "deepseek" in [s["name"] for s in info["providers_skipped"]]


def test_full_chain_with_all_three_keys(monkeypatch):
    """3 API keys + prod config → 3 provider FallbackChain（primary=minimax-m3）。"""
    monkeypatch.setenv("MINIMAX_API_KEY", "k1")
    monkeypatch.setenv("KIMI_API_KEY", "k2")
    monkeypatch.setenv("DEEPSEEK_API_KEY", "k3")

    from agent_core.feature_flags import get_flag_checker
    from main import build_llm_chain

    # prod config 有 fallback=["kimi", "deepseek"]
    checker = get_flag_checker(path=str(PROD_CONFIG))
    llm, info = build_llm_chain(checker)

    assert info["mode"] == "real"
    assert info["providers_loaded"][0] == "minimax-m3"  # prod primary
    assert set(info["providers_loaded"]) == {"minimax-m3", "kimi", "deepseek"}


# ───────────────────────── Runtime + fallback 策略 ─────────────────────────


def test_prod_runtime_default_is_python():
    """production runtime 默认 python。"""
    from main import build_executor, build_audit
    import os as _os

    checker = FlagChecker(path=str(PROD_CONFIG))
    f = checker.flags()
    # Runtime 是 python（不需 build chain）
    assert f.runtime == "python"
    # feature flag audit_sqlite_enabled=True
    assert f.audit_sqlite_enabled is True


def test_gray_percent_deterministic(monkeypatch, tmp_path):
    """python_agent_percent 100 → 全部 user in rollout。"""
    for k in ("MINIMAX_API_KEY", "KIMI_API_KEY", "DEEPSEEK_API_KEY", "WMSG_AGENT_FLAGS"):
        monkeypatch.delenv(k, raising=False)

    flags = FeatureFlags(python_agent_percent=100)
    assert flags.python_agent_percent == 100

    # 通过 FlagChecker 校验
    p = tmp_path / "f.toml"
    p.write_text("[gray]\npython_agent_percent = 100\n")
    checker = FlagChecker(path=str(p))
    assert checker.is_user_in_rollout("user-1") is True
    assert checker.is_user_in_rollout("user-2") is True
    assert checker.is_user_in_rollout("any-user") is True


def test_gray_percent_zero_excludes_all(monkeypatch, tmp_path):
    """python_agent_percent 0 → 全部 user NOT in rollout。"""
    p = tmp_path / "f.toml"
    p.write_text("[gray]\npython_agent_percent = 0\n")
    checker = FlagChecker(path=str(p))
    assert checker.is_user_in_rollout("user-1") is False


def test_gray_white_list_overrides_percent(monkeypatch, tmp_path):
    """白名单 user 即使 percent=0 也在 rollout。"""
    p = tmp_path / "f.toml"
    p.write_text(
        "[gray]\n"
        "python_agent_users = [\"vip-user\"]\n"
        "python_agent_percent = 0\n"
    )
    checker = FlagChecker(path=str(p))
    assert checker.is_user_in_rollout("vip-user") is True
    assert checker.is_user_in_rollout("normal-user") is False


# ───────────────────────── config 内容校验 ─────────────────────────


def test_prod_config_no_secrets_in_file():
    """生产 config 文件不包含任何 API key（敏感信息只放 env var）。

    只检查 uncommented 行 — 注释里举例 `MINIMAX_API_KEY=<your-key>` 是占位符不算泄露。
    """
    uncommented = "\n".join(
        line for line in PROD_CONFIG.read_text().splitlines()
        if not line.lstrip().startswith("#")
    )
    # 检查不应包含 sk- 前缀（典型 API key 格式）
    assert "sk-" not in uncommented, "config 文件不应包含 API key"
    # 检查不应包含 MINIMAX_API_KEY= 赋值行（uncommented）
    assert "MINIMAX_API_KEY=" not in uncommented


def test_prod_config_sensible_defaults():
    """生产 config 默认值合理（保守 + 安全）。"""
    checker = FlagChecker(path=str(PROD_CONFIG))
    f = checker.flags()
    # 全量 = 100（稳态）
    assert f.python_agent_percent == 100
    # 风险特性默认关（reflexion / orchestrator 未实现）
    assert f.reflexion_enabled is False
    assert f.orchestrator_worker is False
    # 安全特性默认开
    assert f.audit_sqlite_enabled is True
    assert f.llm_l2_router is True


def test_dev_config_documents_override_pattern():
    """dev config 用注释说明 override pattern。"""
    content = DEV_CONFIG.read_text()
    assert "WMSG_AGENT_FLAGS" in content  # 注释里说明怎么用
    assert "覆盖" in content or "override" in content.lower()
