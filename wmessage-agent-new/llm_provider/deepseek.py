"""DeepSeek provider — Phase 3 fallback 2。

DeepSeek API（OpenAI 兼容格式）。
环境变量：DEEPSEEK_API_KEY
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


class DeepSeekProvider(LLMProvider):
    """DeepSeek provider (OpenAI 兼容 API)。"""

    name = "deepseek"
    DEFAULT_BASE_URL = "https://api.deepseek.com/v1"
    DEFAULT_MODEL = "deepseek-chat"

    def __init__(
        self,
        api_key: Optional[str] = None,
        base_url: Optional[str] = None,
        model: Optional[str] = None,
        timeout: float = 60.0,
        client: Optional[httpx.Client] = None,
    ):
        self.api_key = api_key or os.environ.get("DEEPSEEK_API_KEY", "")
        if not self.api_key:
            raise LLMAuthError("DEEPSEEK_API_KEY 未设置")
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
            raise LLMTimeoutError(f"DeepSeek timeout: {e}") from e
        except httpx.HTTPError as e:
            raise LLMUnavailableError(f"DeepSeek HTTP error: {e}") from e

        if resp.status_code == 401:
            raise LLMAuthError(f"DeepSeek auth failed")
        if resp.status_code == 429:
            raise LLMRateLimitError(f"DeepSeek rate limited")
        if resp.status_code >= 500:
            raise LLMUnavailableError(f"DeepSeek server error: {resp.status_code}")
        if resp.status_code != 200:
            raise LLMError(f"DeepSeek unexpected {resp.status_code}: {resp.text[:200]}")

        data = resp.json()
        text = data["choices"][0]["message"]["content"]
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
