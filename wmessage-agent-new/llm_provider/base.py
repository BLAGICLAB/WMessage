"""LLM Provider 抽象基类 + 异常 + Fallback 链。"""
from __future__ import annotations

import time
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from typing import Any, Dict, Iterator, List, Optional, Union


# ───────────────────────── 数据类 ─────────────────────────


@dataclass
class LLMRequest:
    """LLM 请求。"""

    user: str
    system: Optional[str] = None
    model: Optional[str] = None  # None = provider 默认
    temperature: float = 0.7
    max_tokens: int = 4096
    stream: bool = False
    timeout_s: int = 60


@dataclass
class LLMResponse:
    """LLM 完整响应。"""

    text: str
    model: str = ""
    prompt_tokens: int = 0
    completion_tokens: int = 0
    duration_ms: int = 0
    raw: Any = None  # provider 原始响应（调试用）


@dataclass
class LLMChunk:
    """流式响应的一帧。"""

    delta: str = ""
    done: bool = False
    finish_reason: str = ""
    prompt_tokens: int = 0
    completion_tokens: int = 0


# ───────────────────────── 异常 ─────────────────────────


class LLMError(Exception):
    """LLM 调用失败基类。"""


class LLMAuthError(LLMError):
    """API key 错误。"""


class LLMRateLimitError(LLMError):
    """限流。"""


class LLMTimeoutError(LLMError):
    """超时。"""


class LLMUnavailableError(LLMError):
    """服务不可用。"""


# ───────────────────────── Provider 协议 ─────────────────────────


class LLMProvider(ABC):
    """LLM Provider 抽象基类。

    Phase 3 实现：MiniMaxProvider / KimiProvider / DeepSeekProvider。
    Phase 4 可扩展：Anthropic / OpenAI / 本地 Ollama。
    """

    name: str = "base"

    @abstractmethod
    def complete(self, request: LLMRequest) -> LLMResponse:
        """同步调用，返回完整响应。"""

    def stream(self, request: LLMRequest) -> Iterator[LLMChunk]:
        """流式调用（默认实现：调 complete 后分块 yield）。

        Provider 可重写实现真正的 streaming。
        """
        resp = self.complete(request)
        if resp.text:
            yield LLMChunk(delta=resp.text, done=True)
        else:
            yield LLMChunk(done=True)


# ───────────────────────── Fallback 链 ─────────────────────────


class FallbackChain(LLMProvider):
    """多个 provider 串成链 — 主失败自动 fallback。

    用法：
        chain = FallbackChain([
            MiniMaxProvider(),
            KimiProvider(),
            DeepSeekProvider(),
        ])
        resp = chain.complete(LLMRequest(user="..."))
        # 主失败 → Kimi → DeepSeek → 全失败则抛错

    触发 fallback 的异常类型：
    - LLMUnavailableError（5xx / 网络）
    - LLMRateLimitError（429）
    - LLMTimeoutError

    不触发 fallback：
    - LLMAuthError（直接抛 — 配置问题）
    - ValueError（请求格式错）
    """

    name = "fallback_chain"

    def __init__(self, providers: List[LLMProvider]):
        if not providers:
            raise ValueError("providers 不能为空")
        self.providers = providers
        self.attempts: List[Dict[str, Any]] = []  # 每次 fallback 记录

    def complete(self, request: LLMRequest) -> LLMResponse:
        last_error: Optional[Exception] = None
        self.attempts = []

        for provider in self.providers:
            start = time.time()
            try:
                resp = provider.complete(request)
                self.attempts.append({
                    "provider": provider.name,
                    "success": True,
                    "duration_ms": int((time.time() - start) * 1000),
                })
                return resp
            except (LLMUnavailableError, LLMRateLimitError, LLMTimeoutError) as e:
                self.attempts.append({
                    "provider": provider.name,
                    "success": False,
                    "error": str(e),
                    "duration_ms": int((time.time() - start) * 1000),
                })
                last_error = e
                continue
            # LLMAuthError / 其他异常直接抛

        raise LLMUnavailableError(
            f"all {len(self.providers)} providers failed: {last_error}"
        )


# ───────────────────────── LLM 协议适配（给 react_step 用）─────────────────────────


class LLMProviderToReactAdapter:
    """把 LLMProvider 适配成 react_step.llm.complete(prompt) 接口。

    用法：
        chain = FallbackChain([MiniMaxProvider(), KimiProvider()])
        adapter = LLMProviderToReactAdapter(chain)
        # 给 agent_loop / react_step 用：
        response = adapter.complete("system + user prompt")
    """

    def __init__(self, provider: LLMProvider):
        self.provider = provider
        self.last_usage: Optional[Dict[str, int]] = None

    def complete(self, prompt: str, system: Optional[str] = None) -> "LLMResponseLike":
        """react_step 期望返回带 .text 字段的对象。"""
        from agent_core.react_step import LLMResponse

        req = LLMRequest(user=prompt, system=system)
        resp = self.provider.complete(req)
        self.last_usage = {
            "prompt_tokens": resp.prompt_tokens,
            "completion_tokens": resp.completion_tokens,
        }
        return LLMResponse(
            text=resp.text,
            prompt_tokens=resp.prompt_tokens,
        )
