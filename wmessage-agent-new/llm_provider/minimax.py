"""MiniMax-M3 provider — Phase 3 主模型。

API 兼容 OpenAI Chat Completions 格式。
环境变量：MINIMAX_API_KEY
"""
from __future__ import annotations

import os
from typing import Any, Dict, List, Optional

import httpx

from llm_provider.base import (
    LLMAuthError,
    LLMError,
    LLMProvider,
    LLMRequest,
    LLMResponse,
    LLMRateLimitError,
    LLMTimeoutError,
    LLMUnavailableError,
)


class MiniMaxProvider(LLMProvider):
    """MiniMax-M3 主模型 provider。

    Args:
        api_key: API key（None = 从环境变量 MINIMAX_API_KEY 读）
        base_url: API endpoint（默认官方）
        model: 模型名（默认 MiniMax-M3）
        timeout: HTTP 超时（秒）
    """

    name = "minimax-m3"
    DEFAULT_BASE_URL = "https://api.minimax.chat/v1"
    DEFAULT_MODEL = "MiniMax-M3"

    def __init__(
        self,
        api_key: Optional[str] = None,
        base_url: Optional[str] = None,
        model: Optional[str] = None,
        timeout: float = 60.0,
        client: Optional[httpx.Client] = None,
    ):
        self.api_key = api_key or os.environ.get("MINIMAX_API_KEY", "")
        if not self.api_key:
            raise LLMAuthError("MINIMAX_API_KEY 未设置")
        self.base_url = base_url or self.DEFAULT_BASE_URL
        self.model = model or self.DEFAULT_MODEL
        self.timeout = timeout
        self.client = client if client is not None else httpx.Client(timeout=timeout)

    def complete(self, request: LLMRequest) -> LLMResponse:
        import time

        messages: List[Dict[str, str]] = []
        if request.system:
            messages.append({"role": "system", "content": request.system})
        messages.append({"role": "user", "content": request.user})

        body: Dict[str, Any] = {
            "model": request.model or self.model,
            "messages": messages,
            "temperature": request.temperature,
            "max_tokens": request.max_tokens,
            "stream": False,
        }

        start = time.time()
        try:
            resp = self.client.post(
                f"{self.base_url}/chat/completions",
                headers={
                    "Authorization": f"Bearer {self.api_key}",
                    "Content-Type": "application/json",
                },
                json=body,
                timeout=request.timeout_s or self.timeout,
            )
        except httpx.TimeoutException as e:
            raise LLMTimeoutError(f"MiniMax timeout: {e}") from e
        except httpx.HTTPError as e:
            raise LLMUnavailableError(f"MiniMax HTTP error: {e}") from e

        return self._parse_response(resp, start)

    def _parse_response(self, resp, start: float) -> LLMResponse:
        import time

        if resp.status_code == 401:
            raise LLMAuthError(f"MiniMax auth failed: {resp.status_code}")
        if resp.status_code == 429:
            raise LLMRateLimitError(f"MiniMax rate limited: {resp.status_code}")
        if resp.status_code >= 500:
            raise LLMUnavailableError(f"MiniMax server error: {resp.status_code}")
        if resp.status_code != 200:
            raise LLMError(f"MiniMax unexpected status {resp.status_code}: {resp.text[:200]}")

        data = resp.json()
        try:
            choice = data["choices"][0]
            text = choice["message"]["content"]
        except (KeyError, IndexError) as e:
            raise LLMError(f"MiniMax bad response shape: {e}") from e

        usage = data.get("usage", {})
        return LLMResponse(
            text=text,
            model=data.get("model", self.model),
            prompt_tokens=usage.get("prompt_tokens", 0),
            completion_tokens=usage.get("completion_tokens", 0),
            duration_ms=int((time.time() - start) * 1000),
            raw=data,
        )

    def __del__(self):
        try:
            self.client.close()
        except Exception:
            pass
