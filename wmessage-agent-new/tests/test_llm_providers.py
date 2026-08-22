"""LLM Provider 测试 — mock httpx，避免真实 API 调用。"""
from __future__ import annotations

import json
import os
from unittest.mock import MagicMock, patch

import pytest

from llm_provider.base import (
    FallbackChain,
    LLMChunk,
    LLMAuthError,
    LLMProvider,
    LLMRequest,
    LLMResponse,
    LLMRateLimitError,
    LLMTimeoutError,
    LLMUnavailableError,
)


# ───────────────────────── 测试辅助 ─────────────────────────


class FakeHTTPResponse:
    """模拟 httpx.Response。"""

    def __init__(self, status_code: int, json_data=None, text=""):
        self.status_code = status_code
        self._json = json_data
        self.text = text or (json.dumps(json_data) if json_data else "")

    def json(self):
        return self._json

    def raise_for_status(self):
        if self.status_code >= 400:
            raise Exception(f"HTTP {self.status_code}")


def fake_response(status_code: int, **kwargs) -> FakeHTTPResponse:
    """构造一个标准 fake response。"""
    if status_code == 200:
        return FakeHTTPResponse(
            200,
            {
                "choices": [{"message": {"content": "test response"}}],
                "model": "test-model",
                "usage": {"prompt_tokens": 10, "completion_tokens": 20},
            },
        )
    return FakeHTTPResponse(status_code, text=kwargs.get("text", ""))


# ───────────────────────── LLMRequest / LLMResponse ─────────────────────────


def test_llm_request_defaults():
    """LLMRequest 默认值。"""
    req = LLMRequest(user="hello")
    assert req.user == "hello"
    assert req.system is None
    assert req.model is None
    assert req.temperature == 0.7
    assert req.max_tokens == 4096
    assert req.stream is False


def test_llm_response_basic():
    """LLMResponse 字段。"""
    r = LLMResponse(text="hi", model="x", prompt_tokens=10, completion_tokens=20)
    assert r.text == "hi"
    assert r.prompt_tokens == 10
    assert r.completion_tokens == 20


# ───────────────────────── FallbackChain ─────────────────────────


class FakeProvider(LLMProvider):
    """可配置的 mock provider。

    responses 列表里元素可以是 str（只给文本）或 LLMResponse 对象（完整字段）。
    """

    def __init__(self, name="fake", responses=None, error=None):
        self.name = name
        self.responses = list(responses or [])
        self.error = error
        self.calls = []

    def complete(self, request: LLMRequest) -> LLMResponse:
        self.calls.append(request.user)
        if self.error:
            raise self.error
        if self.responses:
            r = self.responses.pop(0)
            if isinstance(r, str):
                return LLMResponse(text=r)
            return r
        return LLMResponse(text="default response")


def test_fallback_chain_first_success():
    """主 provider 成功 → 不 fallback。"""
    p1 = FakeProvider(name="p1", responses=["primary response"])
    p2 = FakeProvider(name="p2", responses=["fallback response"])
    chain = FallbackChain([p1, p2])
    resp = chain.complete(LLMRequest(user="hi"))
    assert resp.text == "primary response"
    assert len(p1.calls) == 1
    assert len(p2.calls) == 0


def test_fallback_chain_triggers_on_unavailable():
    """主 unavailable → fallback。"""
    p1 = FakeProvider(name="p1", error=LLMUnavailableError("p1 down"))
    p2 = FakeProvider(name="p2", responses=["fallback response"])
    chain = FallbackChain([p1, p2])
    resp = chain.complete(LLMRequest(user="hi"))
    assert resp.text == "fallback response"
    assert len(p1.calls) == 1
    assert len(p2.calls) == 1


def test_fallback_chain_triggers_on_rate_limit():
    """主 429 → fallback。"""
    p1 = FakeProvider(name="p1", error=LLMRateLimitError("rate"))
    p2 = FakeProvider(name="p2", responses=["ok"])
    chain = FallbackChain([p1, p2])
    resp = chain.complete(LLMRequest(user="hi"))
    assert resp.text == "ok"


def test_fallback_chain_triggers_on_timeout():
    """主 timeout → fallback。"""
    p1 = FakeProvider(name="p1", error=LLMTimeoutError("timeout"))
    p2 = FakeProvider(name="p2", responses=["ok"])
    chain = FallbackChain([p1, p2])
    resp = chain.complete(LLMRequest(user="hi"))
    assert resp.text == "ok"


def test_fallback_chain_multiple_failures():
    """主 + fallback1 失败 → fallback2。"""
    p1 = FakeProvider(name="p1", error=LLMUnavailableError())
    p2 = FakeProvider(name="p2", error=LLMRateLimitError())
    p3 = FakeProvider(name="p3", responses=["final"])
    chain = FallbackChain([p1, p2, p3])
    resp = chain.complete(LLMRequest(user="hi"))
    assert resp.text == "final"
    assert len(p1.calls) == 1
    assert len(p2.calls) == 1
    assert len(p3.calls) == 1


def test_fallback_chain_all_fail():
    """全部失败 → LLMUnavailableError。"""
    p1 = FakeProvider(name="p1", error=LLMUnavailableError())
    p2 = FakeProvider(name="p2", error=LLMUnavailableError())
    chain = FallbackChain([p1, p2])
    with pytest.raises(LLMUnavailableError, match="all 2 providers failed"):
        chain.complete(LLMRequest(user="hi"))


def test_fallback_chain_does_not_catch_auth_error():
    """AuthError 不触发 fallback（直接抛 — 配置问题）。"""
    p1 = FakeProvider(name="p1", error=LLMAuthError("bad key"))
    p2 = FakeProvider(name="p2", responses=["ok"])
    chain = FallbackChain([p1, p2])
    with pytest.raises(LLMAuthError):
        chain.complete(LLMRequest(user="hi"))
    # p2 不应被调用
    assert len(p2.calls) == 0


def test_fallback_chain_empty_providers():
    """空 chain → ValueError。"""
    with pytest.raises(ValueError, match="不能为空"):
        FallbackChain([])


def test_fallback_chain_records_attempts():
    """chain.attempts 记录每次尝试。"""
    p1 = FakeProvider(name="p1", error=LLMUnavailableError("x"))
    p2 = FakeProvider(name="p2", responses=["ok"])
    chain = FallbackChain([p1, p2])
    chain.complete(LLMRequest(user="hi"))
    assert len(chain.attempts) == 2
    assert chain.attempts[0]["provider"] == "p1"
    assert chain.attempts[0]["success"] is False
    assert "x" in chain.attempts[0]["error"]
    assert chain.attempts[1]["provider"] == "p2"
    assert chain.attempts[1]["success"] is True


# ───────────────────────── MiniMax Provider ─────────────────────────


@patch.dict(os.environ, {"MINIMAX_API_KEY": "test-key"})
@patch("llm_provider.minimax.httpx.Client")
def test_minimax_complete_success(mock_client_cls):
    """MiniMaxProvider 成功调用。"""
    mock_client = MagicMock()
    mock_client.post.return_value = fake_response(200)
    mock_client_cls.return_value = mock_client

    from llm_provider.minimax import MiniMaxProvider

    p = MiniMaxProvider()
    resp = p.complete(LLMRequest(user="hello"))
    assert resp.text == "test response"
    assert resp.prompt_tokens == 10
    assert resp.completion_tokens == 20


@patch.dict(os.environ, {"MINIMAX_API_KEY": "test-key"})
@patch("llm_provider.minimax.httpx.Client")
def test_minimax_complete_auth_error(mock_client_cls):
    """401 → LLMAuthError。"""
    mock_client = MagicMock()
    mock_client.post.return_value = fake_response(401)
    mock_client_cls.return_value = mock_client

    from llm_provider.minimax import MiniMaxProvider

    p = MiniMaxProvider()
    with pytest.raises(LLMAuthError):
        p.complete(LLMRequest(user="hi"))


@patch.dict(os.environ, {"MINIMAX_API_KEY": "test-key"})
@patch("llm_provider.minimax.httpx.Client")
def test_minimax_complete_rate_limit(mock_client_cls):
    """429 → LLMRateLimitError。"""
    mock_client = MagicMock()
    mock_client.post.return_value = fake_response(429)
    mock_client_cls.return_value = mock_client

    from llm_provider.minimax import MiniMaxProvider

    p = MiniMaxProvider()
    with pytest.raises(LLMRateLimitError):
        p.complete(LLMRequest(user="hi"))


@patch.dict(os.environ, {"MINIMAX_API_KEY": "test-key"})
@patch("llm_provider.minimax.httpx.Client")
def test_minimax_complete_server_error(mock_client_cls):
    """500+ → LLMUnavailableError。"""
    mock_client = MagicMock()
    mock_client.post.return_value = fake_response(500)
    mock_client_cls.return_value = mock_client

    from llm_provider.minimax import MiniMaxProvider

    p = MiniMaxProvider()
    with pytest.raises(LLMUnavailableError):
        p.complete(LLMRequest(user="hi"))


@patch.dict(os.environ, {}, clear=True)
def test_minimax_no_api_key_raises():
    """未设 API key → LLMAuthError。"""
    from llm_provider.minimax import MiniMaxProvider

    with pytest.raises(LLMAuthError, match="未设置"):
        MiniMaxProvider()


def test_minimax_default_config():
    """MiniMaxProvider 默认 base_url + model。"""
    with patch.dict(os.environ, {"MINIMAX_API_KEY": "test"}):
        from llm_provider.minimax import MiniMaxProvider

        p = MiniMaxProvider()
        assert p.base_url == "https://api.minimax.chat/v1"
        assert p.model == "MiniMax-M3"
        assert p.name == "minimax-m3"


# ───────────────────────── Kimi Provider ─────────────────────────


@patch.dict(os.environ, {"KIMI_API_KEY": "test-key"})
@patch("llm_provider.kimi.httpx.Client")
def test_kimi_complete_success(mock_client_cls):
    from llm_provider.kimi import KimiProvider

    mock_client = MagicMock()
    mock_client.post.return_value = fake_response(200)
    mock_client_cls.return_value = mock_client

    p = KimiProvider()
    resp = p.complete(LLMRequest(user="hello"))
    assert resp.text == "test response"


@patch.dict(os.environ, {"KIMI_API_KEY": "test-key"})
@patch("llm_provider.kimi.httpx.Client")
def test_kimi_complete_server_error(mock_client_cls):
    from llm_provider.kimi import KimiProvider

    mock_client = MagicMock()
    mock_client.post.return_value = fake_response(503)
    mock_client_cls.return_value = mock_client

    p = KimiProvider()
    with pytest.raises(LLMUnavailableError):
        p.complete(LLMRequest(user="hi"))


@patch.dict(os.environ, {}, clear=True)
def test_kimi_no_api_key_raises():
    from llm_provider.kimi import KimiProvider

    with pytest.raises(LLMAuthError):
        KimiProvider()


# ───────────────────────── DeepSeek Provider ─────────────────────────


@patch.dict(os.environ, {"DEEPSEEK_API_KEY": "test-key"})
@patch("llm_provider.deepseek.httpx.Client")
def test_deepseek_complete_success(mock_client_cls):
    from llm_provider.deepseek import DeepSeekProvider

    mock_client = MagicMock()
    mock_client.post.return_value = fake_response(200)
    mock_client_cls.return_value = mock_client

    p = DeepSeekProvider()
    resp = p.complete(LLMRequest(user="hello"))
    assert resp.text == "test response"


@patch.dict(os.environ, {"DEEPSEEK_API_KEY": "test-key"})
@patch("llm_provider.deepseek.httpx.Client")
def test_deepseek_complete_timeout(mock_client_cls):
    """Timeout → LLMTimeoutError。"""
    import httpx

    from llm_provider.deepseek import DeepSeekProvider

    mock_client = MagicMock()
    mock_client.post.side_effect = httpx.TimeoutException("timeout")
    mock_client_cls.return_value = mock_client

    p = DeepSeekProvider()
    with pytest.raises(LLMTimeoutError):
        p.complete(LLMRequest(user="hi"))


@patch.dict(os.environ, {}, clear=True)
def test_deepseek_no_api_key_raises():
    from llm_provider.deepseek import DeepSeekProvider

    with pytest.raises(LLMAuthError):
        DeepSeekProvider()


# ───────────────────────── FallbackChain + 3 providers 集成 ─────────────────────────


@patch.dict(
    os.environ,
    {
        "MINIMAX_API_KEY": "k1",
        "KIMI_API_KEY": "k2",
        "DEEPSEEK_API_KEY": "k3",
    },
)
def test_full_fallback_chain_minimax_kimi_deepseek():
    """完整 3-tier fallback：minimax → kimi → deepseek。

    用构造器注入 mock client 而不是 patch decorator——避免 patch decorator 顺序复杂度。
    """
    from llm_provider.minimax import MiniMaxProvider
    from llm_provider.kimi import KimiProvider
    from llm_provider.deepseek import DeepSeekProvider

    m_minimax = MagicMock()
    m_minimax.post.return_value = fake_response(500)
    m_kimi = MagicMock()
    m_kimi.post.return_value = fake_response(429)
    m_ds = MagicMock()
    m_ds.post.return_value = fake_response(200)

    chain = FallbackChain(
        [
            MiniMaxProvider(client=m_minimax),
            KimiProvider(client=m_kimi),
            DeepSeekProvider(client=m_ds),
        ]
    )
    resp = chain.complete(LLMRequest(user="hi"))
    assert resp.text == "test response"
    # 3 次尝试
    assert len(chain.attempts) == 3
    assert chain.attempts[0]["provider"] == "minimax-m3"
    assert chain.attempts[0]["success"] is False
    assert chain.attempts[1]["provider"] == "kimi"
    assert chain.attempts[1]["success"] is False
    assert chain.attempts[2]["provider"] == "deepseek"
    assert chain.attempts[2]["success"] is True


# ───────────────────────── LLM Provider 协议 ─────────────────────────


def test_provider_stream_default():
    """LLMProvider.stream 默认实现：调 complete 后 yield 整块。"""
    p = FakeProvider(name="x", responses=["streamed text"])
    chunks = list(p.stream(LLMRequest(user="hi")))
    assert len(chunks) == 1
    assert chunks[0].done is True
    assert chunks[0].delta == "streamed text"


def test_provider_name_attribute():
    """每个 provider 有 name 属性。"""
    assert FakeProvider(name="my-p").name == "my-p"


# ───────────────────────── LLMProviderToReactAdapter ─────────────────────────


def test_adapter_for_react_step():
    """adapter 把 LLMProvider 包装成 react_step 接口。"""
    from llm_provider.base import LLMProviderToReactAdapter
    from agent_core.react_step import LLMResponse

    # 传完整 LLMResponse 对象（不仅字符串）
    from llm_provider.base import LLMResponse as ProviderLLMResponse
    p = FakeProvider(
        name="x",
        responses=[ProviderLLMResponse(
            text="react text",
            prompt_tokens=10,
            completion_tokens=20,
        )],
    )
    adapter = LLMProviderToReactAdapter(p)
    resp = adapter.complete(prompt="user prompt", system="sys")
    assert isinstance(resp, LLMResponse)
    assert resp.text == "react text"
    assert resp.prompt_tokens == 10
    # last_usage 被记录
    assert adapter.last_usage == {"prompt_tokens": 10, "completion_tokens": 20}
