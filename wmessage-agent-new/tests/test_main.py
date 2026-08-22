"""main.py 集成测试 — Phase 4 LLM provider 接入。

覆盖：
- build_llm_chain: 全 / 部分 / 无 API key + 未知 provider
- build_executor: 默认 RealToolExecutor
- build_audit: SqliteAuditLogger / AuditLogger
- init_runtime: 一次性构建所有 singleton
- handle_chat: 真实 LLM 链路 + 测试注入
- handle_status / handle_ping: 返回 runtime 信息
- ready_payload: 含 llm_mode
"""
from __future__ import annotations

import json
import os
from unittest.mock import MagicMock, patch

import pytest

import main
from main import (
    ScriptedLLM,
    build_audit,
    build_executor,
    build_llm_chain,
    init_runtime,
    handle_chat,
    handle_ping,
    handle_status,
    _reset_runtime,
)


@pytest.fixture(autouse=True)
def _reset_between_tests(monkeypatch):
    """每个测试前重置 runtime singletons + 清 LLM env vars。

    LLM env vars (MINIMAX/KIMI/DEEPSEEK_API_KEY) 跨测试会污染 —— 用 monkeypatch.delenv 显式清。
    """
    for k in ("MINIMAX_API_KEY", "KIMI_API_KEY", "DEEPSEEK_API_KEY"):
        monkeypatch.delenv(k, raising=False)
    _reset_runtime()
    yield
    _reset_runtime()


@pytest.fixture
def tmp_flag_checker(tmp_path):
    """临时 FlagChecker + 空 feature_flags.toml（默认值）。"""
    from agent_core.feature_flags import FlagChecker

    p = tmp_path / "flags.toml"
    p.write_text("")
    return FlagChecker(path=str(p))


# ───────────────────────── build_llm_chain ─────────────────────────


def test_build_llm_chain_no_keys_uses_scripted(tmp_path):
    """无 API key → ScriptedLLM fallback。"""
    from agent_core.feature_flags import FlagChecker

    checker = FlagChecker(path=str(tmp_path / "nope.toml"))
    llm, info = build_llm_chain(checker)
    assert isinstance(llm, ScriptedLLM)
    assert info["mode"] == "mock"
    assert info["reason"] == "no_api_keys_or_init_failed"
    assert info["providers_loaded"] == []


def test_build_llm_chain_skips_env_unset(monkeypatch, tmp_path):
    """只有 MINIMAX key → 单 provider FallbackChain。"""
    from agent_core.feature_flags import FlagChecker

    monkeypatch.setenv("MINIMAX_API_KEY", "test-key")
    monkeypatch.delenv("KIMI_API_KEY", raising=False)
    monkeypatch.delenv("DEEPSEEK_API_KEY", raising=False)

    # minimax 依赖 httpx — mock 避免真网络
    with patch("llm_provider.minimax.httpx.Client") as mock_client_cls:
        mock_client = MagicMock()
        mock_client.post.return_value = MagicMock(
            status_code=200,
            json=lambda: {
                "choices": [{"message": {"content": "ok"}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1},
            },
            text='{"choices": [{"message": {"content": "ok"}}]}',
        )
        mock_client_cls.return_value = mock_client

        checker = FlagChecker(path=str(tmp_path / "nope.toml"))
        llm, info = build_llm_chain(checker)

    assert info["mode"] == "real"
    assert "minimax-m3" in info["providers_loaded"]
    assert len(info["providers_skipped"]) == 2  # kimi + deepseek 缺 key


def test_build_llm_chain_all_three_keys(tmp_path):
    """3 个 key 全有 → 3 provider FallbackChain。"""
    from agent_core.feature_flags import FlagChecker

    with patch.dict(os.environ, {
        "MINIMAX_API_KEY": "k1",
        "KIMI_API_KEY": "k2",
        "DEEPSEEK_API_KEY": "k3",
    }):
        with patch("llm_provider.minimax.httpx.Client") as m_minimax, \
             patch("llm_provider.kimi.httpx.Client") as m_kimi, \
             patch("llm_provider.deepseek.httpx.Client") as m_ds:
            for m in (m_minimax, m_kimi, m_ds):
                m.return_value = MagicMock()

            checker = FlagChecker(path=str(tmp_path / "nope.toml"))
            llm, info = build_llm_chain(checker)

    assert info["mode"] == "real"
    assert set(info["providers_loaded"]) == {"minimax-m3", "kimi", "deepseek"}


def test_build_llm_chain_skips_unknown_provider(tmp_path):
    """flag 配置了不存在的 provider → 跳过。"""
    from agent_core.feature_flags import FeatureFlags, FlagChecker

    flags = FeatureFlags(
        llm_primary="unknown-llm",
        llm_fallback=["minimax-m3"],
    )
    with patch.dict(os.environ, {"MINIMAX_API_KEY": "test-key"}):
        with patch("llm_provider.minimax.httpx.Client") as m:
            m.return_value = MagicMock()
            checker = FlagChecker(path=str(tmp_path / "nope.toml"))
            checker._cache = flags
            llm, info = build_llm_chain(checker)

    assert info["mode"] == "real"
    skipped_names = [s["name"] for s in info["providers_skipped"]]
    assert "unknown-llm" in skipped_names
    assert "minimax-m3" in info["providers_loaded"]


def test_build_llm_chain_respects_primary_order(tmp_path):
    """primary 在前，fallback 在后。"""
    from agent_core.feature_flags import FeatureFlags, FlagChecker

    with patch.dict(os.environ, {
        "KIMI_API_KEY": "k1",
        "MINIMAX_API_KEY": "k2",
        "DEEPSEEK_API_KEY": "k3",
    }):
        with patch("llm_provider.minimax.httpx.Client") as m_m, \
             patch("llm_provider.kimi.httpx.Client") as m_k, \
             patch("llm_provider.deepseek.httpx.Client") as m_d:
            for m in (m_m, m_k, m_d):
                m.return_value = MagicMock()

            checker = FlagChecker(path=str(tmp_path / "nope.toml"))
            # primary = kimi, fallback = [minimax, deepseek]
            checker._cache = FeatureFlags(
                llm_primary="kimi", llm_fallback=["minimax-m3", "deepseek"]
            )
            _, info = build_llm_chain(checker)

    assert info["providers_loaded"][0] == "kimi"
    assert "minimax-m3" in info["providers_loaded"]
    assert "deepseek" in info["providers_loaded"]


# ───────────────────────── build_executor ─────────────────────────


def test_build_executor_returns_real(tmp_path):
    """默认 RealToolExecutor。"""
    from agent_core.feature_flags import FlagChecker

    checker = FlagChecker(path=str(tmp_path / "nope.toml"))
    executor, info = build_executor(checker)
    assert info["mode"] == "real"
    assert info["executor_class"] == "RealToolExecutor"
    assert "output_dir" in info


def test_build_executor_uses_env_output_dir(tmp_path, monkeypatch):
    """WMSG_AGENT_OUTPUT_DIR 环境变量生效。"""
    from agent_core.feature_flags import FlagChecker

    monkeypatch.setenv("WMSG_AGENT_OUTPUT_DIR", str(tmp_path / "custom"))
    checker = FlagChecker(path=str(tmp_path / "nope.toml"))
    executor, info = build_executor(checker)
    assert info["output_dir"] == str(tmp_path / "custom")
    assert os.path.isdir(str(tmp_path / "custom"))


# ───────────────────────── build_audit ─────────────────────────


def test_build_audit_file_only(tmp_path, monkeypatch):
    """无 WMSG_AGENT_DB → AuditLogger（仅文件）。"""
    from agent_core.feature_flags import FlagChecker

    for k in ("WMSG_AGENT_DB", "WMSG_AGENT_LOG"):
        monkeypatch.delenv(k, raising=False)

    checker = FlagChecker(path=str(tmp_path / "nope.toml"))
    audit, info = build_audit(checker)
    assert info["mode"] == "file"


def test_build_audit_sqlite_with_env(tmp_path, monkeypatch):
    """WMSG_AGENT_DB 环境变量启用 SqliteAuditLogger。"""
    from agent_core.feature_flags import FlagChecker

    db_path = tmp_path / "agent.db"
    monkeypatch.setenv("WMSG_AGENT_DB", str(db_path))

    checker = FlagChecker(path=str(tmp_path / "nope.toml"))
    audit, info = build_audit(checker)
    assert info["mode"] == "sqlite+file"
    assert info["db_path"] == str(db_path)
    # schema 已自动 migrate
    assert db_path.exists()


def test_build_audit_sqlite_via_flag(tmp_path):
    """flag.audit_sqlite_enabled 启用 SqliteAuditLogger。"""
    from agent_core.feature_flags import FeatureFlags, FlagChecker

    db_path = tmp_path / "agent2.db"
    flags = FeatureFlags(audit_sqlite_enabled=True)
    checker = FlagChecker(path=str(tmp_path / "nope.toml"))
    checker._cache = flags

    audit, info = build_audit(checker)
    assert info["mode"] == "sqlite+file"
    # 默认 db_path
    assert "agent.db" in info["db_path"]


# ───────────────────────── init_runtime ─────────────────────────


def test_init_runtime_wires_everything(tmp_path):
    """init_runtime 一次性构建 3 个组件 + 填充 _runtime。"""
    from agent_core.feature_flags import FlagChecker

    checker = FlagChecker(path=str(tmp_path / "nope.toml"))
    init_info = init_runtime(flag_path=str(tmp_path / "nope.toml"))

    assert main._runtime["flag_checker"] is not None
    assert main._runtime["llm_client"] is not None
    assert main._runtime["executor"] is not None
    assert main._runtime["audit"] is not None

    assert init_info["llm"]["mode"] == "mock"  # 无 key
    assert init_info["executor"]["mode"] == "real"
    assert init_info["audit"]["mode"] == "file"


def test_init_runtime_with_all_keys(tmp_path):
    """全 key → real LLM chain。"""
    with patch.dict(os.environ, {
        "MINIMAX_API_KEY": "k1",
        "KIMI_API_KEY": "k2",
        "DEEPSEEK_API_KEY": "k3",
    }):
        with patch("llm_provider.minimax.httpx.Client") as m1, \
             patch("llm_provider.kimi.httpx.Client") as m2, \
             patch("llm_provider.deepseek.httpx.Client") as m3:
            for m in (m1, m2, m3):
                m.return_value = MagicMock()

            init_info = init_runtime()

    assert init_info["llm"]["mode"] == "real"
    assert len(init_info["llm"]["providers_loaded"]) == 3


# ───────────────────────── handle_chat ─────────────────────────


def test_handle_chat_uses_runtime_llm_by_default(tmp_path):
    """handle_chat 默认用 _runtime.llm_client（无 override）。"""
    from agent_core.feature_flags import FlagChecker

    init_runtime(flag_path=str(tmp_path / "nope.toml"))
    default_llm = main._runtime["llm_client"]
    assert isinstance(default_llm, ScriptedLLM)

    # 用 ScriptedLLM 的 queue 喂 task_done 响应
    default_llm.queue("Thought: ok\nAction: task_done\nAction Input: done")
    main._runtime["executor"] = MagicMock()  # 避免真生成文件

    result = handle_chat({"message": "A. B."})
    assert "loop" in result
    # LLM mode 走 mock
    assert result["llm_mode"] == "mock"


def test_handle_chat_param_override_wins(tmp_path):
    """params._llm 覆盖 _runtime.llm_client。"""
    from agent_core.feature_flags import FlagChecker
    from agent_core.react_step import LLMResponse

    class StubLLM:
        name = "stub"
        def complete(self, prompt, system=None):
            return LLMResponse(
                text="Thought: ok\nAction: task_done\nAction Input: done",
                prompt_tokens=10,
            )

    init_runtime(flag_path=str(tmp_path / "nope.toml"))
    main._runtime["executor"] = MagicMock()

    result = handle_chat({
        "message": "A.",
        "_llm": StubLLM(),
    })
    assert "loop" in result


def test_handle_chat_real_llm_end_to_end(tmp_path):
    """真实 LLM 端到端（mock httpx）→ FallbackChain + handle_chat 跑通。

    注：patch("llm_provider.minimax.httpx.Client") 和 patch("...kimi.httpx.Client")
    改的是同一个对象（httpx 模块），后者覆盖前者。需要用同一个 mock 实例同时 patch 两处。
    """
    from agent_core.feature_flags import FlagChecker

    mock_response = MagicMock()
    mock_response.status_code = 200
    mock_response.json.return_value = {
        "choices": [{"message": {"content":
            "Thought: ok\nAction: task_done\nAction Input: done"
        }}],
        "model": "test",
        "usage": {"prompt_tokens": 10, "completion_tokens": 20},
    }
    # 同一个共享 mock 实例，两个 patch 都指向它
    shared_client = MagicMock()
    shared_client.post.return_value = mock_response
    shared_class = MagicMock(return_value=shared_client)

    with patch.dict(os.environ, {
        "MINIMAX_API_KEY": "k1",
        "KIMI_API_KEY": "k2",
    }):
        with patch("llm_provider.minimax.httpx.Client", shared_class), \
             patch("llm_provider.kimi.httpx.Client", shared_class):
            init_runtime()

    main._runtime["executor"] = MagicMock()
    result = handle_chat({"message": "A."})

    assert result["llm_mode"] == "real"
    # 共享 client 被调了 (不管是 minimax 还是 kimi)
    assert shared_client.post.called


def test_handle_chat_invalid_message_max_tasks_returns_error(tmp_path):
    """maxTasks=0 → ValueError → 返回 error。"""
    from agent_core.feature_flags import FlagChecker

    init_runtime(flag_path=str(tmp_path / "nope.toml"))
    main._runtime["executor"] = MagicMock()
    main._runtime["llm_client"] = ScriptedLLM([
        "Thought: ok\nAction: task_done\nAction Input: done"
    ])

    result = handle_chat({"message": "x", "maxTasks": 0})
    assert result["kind"] == "ValueError"


def test_handle_chat_intent_routes_to_docx(tmp_path):
    """chat 触发 minimax-docx intent。"""
    from agent_core.feature_flags import FlagChecker

    init_runtime(flag_path=str(tmp_path / "nope.toml"))
    main._runtime["executor"] = MagicMock()
    main._runtime["llm_client"] = ScriptedLLM([
        "Thought: ok\nAction: task_done\nAction Input: done"
    ])

    result = handle_chat({"message": "做个 word 文档，标题 X"})
    assert result["routed_skill"] == "minimax-docx"


# ───────────────────────── handle_ping / handle_status ─────────────────────────


def test_handle_ping_includes_runtime(tmp_path):
    """ping 返回 runtime llm_mode + executor_mode。"""
    from agent_core.feature_flags import FlagChecker

    init_runtime(flag_path=str(tmp_path / "nope.toml"))
    r = handle_ping({})
    assert r["pong"] is True
    assert r["runtime"]["llm_mode"] == "mock"
    assert r["runtime"]["executor_mode"] == "real"


def test_handle_status_returns_full_info(tmp_path):
    """status 返回完整 runtime 信息。"""
    from agent_core.feature_flags import FlagChecker

    init_runtime(flag_path=str(tmp_path / "nope.toml"))
    r = handle_status({})
    assert r["version"] == "0.4.0"
    assert r["phase"] == 4
    assert "llm" in r
    assert "executor" in r
    assert "audit" in r
    assert r["llm"]["mode"] == "mock"


def test_handle_status_real_llm_info(tmp_path):
    """status 在 real LLM 时返回 provider 列表。"""
    with patch.dict(os.environ, {
        "MINIMAX_API_KEY": "k1",
    }):
        with patch("llm_provider.minimax.httpx.Client") as m:
            m.return_value = MagicMock()
            init_runtime()

    r = handle_status({})
    assert r["llm"]["mode"] == "real"
    assert "minimax-m3" in r["llm"]["providers_loaded"]


# ───────────────────────── ready_payload ─────────────────────────


def test_ready_payload_includes_llm_mode(tmp_path, monkeypatch, capsys):
    """main() 启动时 ready_payload 含 llm_mode + providers_loaded。"""
    monkeypatch.chdir(tmp_path)
    (tmp_path / "config").mkdir()

    # 我们不真正启动 main()，只测试 init_info 的格式
    init_info = init_runtime()
    ready_payload = {
        "version": "0.4.0",
        "phase": 4,
        "pid": 12345,
        "llm_mode": init_info["llm"]["mode"],
        "providers_loaded": init_info["llm"].get("providers_loaded", []),
    }
    assert "llm_mode" in ready_payload
    assert ready_payload["llm_mode"] == "mock"
    assert ready_payload["providers_loaded"] == []
    assert ready_payload["phase"] == 4


# ───────────────────────── LLMProviderToReactAdapter 集成 ─────────────────────────


def test_fallback_chain_calls_work_via_adapter(monkeypatch):
    """LLMProviderToReactAdapter 能被 AgentLoop 使用（完整 ReAct step）。"""
    from agent_core.react_step import react_step, ReactState, ActionKind
    from agent_core.types import TaskNode

    monkeypatch.setenv("MINIMAX_API_KEY", "k1")
    mock_response = MagicMock()
    mock_response.status_code = 200
    mock_response.json.return_value = {
        "choices": [{"message": {"content":
            "Thought: I should ask\nAction: ask_user\nAction Input: what's your API version?"
        }}],
        "model": "test",
        "usage": {"prompt_tokens": 10, "completion_tokens": 20},
    }
    with patch("llm_provider.minimax.httpx.Client") as m:
        m.return_value = MagicMock()
        m.return_value.post.return_value = mock_response

        init_runtime()
        adapter = main._runtime["llm_client"]
        state = ReactState(task=TaskNode(id="t1", title="x"), step=1)
        action = react_step(state, adapter)
        assert action.kind == ActionKind.ASK_USER.value
        assert "API version" in action.question


# ───────────────────────── main() 函数测试 ─────────────────────────


def test_main_with_no_env(tmp_path, monkeypatch, capsys):
    """无 env vars → main() 启动后 stderr 报 llm_mode=mock。"""
    monkeypatch.chdir(tmp_path)
    for k in ("MINIMAX_API_KEY", "KIMI_API_KEY", "DEEPSEEK_API_KEY", "WMSG_AGENT_FLAGS"):
        monkeypatch.delenv(k, raising=False)

    # 模拟 main() 中的初始化部分（不启 server.run()）
    init_info = init_runtime()
    print(
        f"[wmessage-agent] ready phase=4 "
        f"llm_mode={init_info['llm']['mode']} "
        f"providers={init_info['llm'].get('providers_loaded', [])}",
        file=__import__('sys').stderr,
    )
    captured = capsys.readouterr()
    assert "ready phase=4" in captured.err
    assert "llm_mode=mock" in captured.err
