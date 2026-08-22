"""LLM Provider 抽象 — Phase 3。

设计：
- LLMProvider 抽象类 — complete() / stream()
- 3 个实现：MiniMax-M3 (主) / Kimi (fallback 1) / DeepSeek (fallback 2)
- FallbackChain — 主失败自动 fallback
- 配置走环境变量 + config.toml
"""
from llm_provider.base import (
    LLMProvider,
    LLMRequest,
    LLMResponse,
    LLMChunk,
    FallbackChain,
    LLMError,
    LLMAuthError,
    LLMRateLimitError,
)
from llm_provider.minimax import MiniMaxProvider
from llm_provider.kimi import KimiProvider
from llm_provider.deepseek import DeepSeekProvider

__all__ = [
    "LLMProvider",
    "LLMRequest",
    "LLMResponse",
    "LLMChunk",
    "FallbackChain",
    "LLMError",
    "LLMAuthError",
    "LLMRateLimitError",
    "MiniMaxProvider",
    "KimiProvider",
    "DeepSeekProvider",
]
