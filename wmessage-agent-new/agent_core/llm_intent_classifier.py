"""LLM L2 Intent Classifier — Phase 3 step 4。

设计：
- L1（关键词）未命中时用 LLM 判意图
- LLM 用 skill 的 description + intents 作 few-shot
- 输出 skill name + 置信度，低于阈值返回 None（走通用 LLM 决策）

性能：
- 单次 LLM 调用 ~200-500ms（远高于 L1 子串匹配）
- 仅在 L1 未命中时触发（节省 token）
- LLM 失败 → fallback None（不影响主流程）
"""
from __future__ import annotations

import json
from typing import Any, Dict, List, Optional

from llm_provider.base import LLMProvider, LLMRequest, LLMProviderToReactAdapter


# 分类 prompt 模板
_CLASSIFY_PROMPT = """你是一个意图路由器。给定用户查询和可用 skill 列表，判断用户想用哪个 skill。

可用 skills：
{skills_json}

用户查询：{query}

仅返回 JSON 格式：
{{"skill": "<skill_name 或 null>", "confidence": <0.0-1.0>, "reason": "<一句话>"}}

判定规则：
- confidence < 0.6 → skill = null（不强制路由，让通用 LLM 决策）
- 完全无关的查询 → skill = null
"""


class LLMIntentClassifier:
    """LLM L2 意图路由器。"""

    def __init__(
        self,
        provider: LLMProvider,
        skills: List[Any],  # List[BuiltinSkill]
        confidence_threshold: float = 0.6,
        timeout_s: int = 5,
    ):
        self.provider = provider
        self.skills = skills
        self.threshold = confidence_threshold
        self.timeout_s = timeout_s
        # 缓存 last_result（避免重复调用）
        self.last_result: Optional[Dict[str, Any]] = None

    def classify(self, query: str) -> Optional[Dict[str, Any]]:
        """L2 分类 — 返回 {skill, confidence, reason} 或 None（低置信度）。

        Args:
            query: 用户自然语言

        Returns:
            {
                "skill": "minimax-docx" | None,
                "confidence": 0.85,
                "reason": "用户提到'生成 word 文档'"
            }
        """
        if not self.skills:
            return None

        skills_json = json.dumps(
            [
                {
                    "name": s.name,
                    "description": s.description,
                    "intents": s.intents,
                }
                for s in self.skills
            ],
            ensure_ascii=False,
        )
        prompt = _CLASSIFY_PROMPT.format(skills_json=skills_json, query=query)

        try:
            resp = self.provider.complete(LLMRequest(
                user=prompt,
                temperature=0.1,  # 低温度更稳定
                max_tokens=200,
                timeout_s=self.timeout_s,
            ))
        except Exception:
            return None  # LLM 失败 → 不强制路由

        result = self._parse_response(resp.text)
        self.last_result = result

        if result is None:
            return None
        if result.get("confidence", 0) < self.threshold:
            return None  # 低置信度 → 走通用路径
        if not result.get("skill"):
            return None
        return result

    @staticmethod
    def _parse_response(text: str) -> Optional[Dict[str, Any]]:
        """解析 LLM 返回的 JSON。

        LLM 可能返回：
        - 纯 JSON：`{"skill": "x", "confidence": 0.8, "reason": "..."}`
        - 带 markdown：````json\n{...}\n````
        - 前后有废话：`根据查询，{"skill": ...}`
        """
        if not text:
            return None
        text = text.strip()

        # 尝试直接解析
        try:
            data = json.loads(text)
            return _validate(data)
        except json.JSONDecodeError:
            pass

        # 提取 markdown 代码块
        import re
        m = re.search(r"```(?:json)?\s*(\{.*?\})\s*```", text, re.DOTALL)
        if m:
            try:
                data = json.loads(m.group(1))
                return _validate(data)
            except json.JSONDecodeError:
                pass

        # 提取第一个 {...} 块
        m = re.search(r"\{.*?\}", text, re.DOTALL)
        if m:
            try:
                data = json.loads(m.group(0))
                return _validate(data)
            except json.JSONDecodeError:
                pass

        return None


def _validate(data: Any) -> Optional[Dict[str, Any]]:
    """校验解析结果字段类型。"""
    if not isinstance(data, dict):
        return None
    skill = data.get("skill")
    confidence = data.get("confidence", 0)
    reason = data.get("reason", "")
    return {
        "skill": str(skill) if skill else None,
        "confidence": float(confidence) if isinstance(confidence, (int, float)) else 0.0,
        "reason": str(reason),
    }


class TwoLayerIntentRouter:
    """L1 + L2 双层路由 — Phase 3 主入口。"""

    def __init__(
        self,
        skills: List[Any],  # List[BuiltinSkill]
        l2_classifier: Optional[LLMIntentClassifier] = None,
    ):
        self.skills = skills
        self.l1 = _L1Router(skills)
        self.l2 = l2_classifier  # None = 纯 L1 模式

    def route(self, query: str) -> Optional[Dict[str, Any]]:
        """双层路由：L1 命中 → 直接返回；未命中 → L2 LLM 判。

        Returns:
            {skill, confidence, reason, layer: "L1" | "L2"}
        """
        # L1 关键词
        l1_result = self.l1.find(query)
        if l1_result:
            return {
                "skill": l1_result.name,
                "confidence": 0.95,  # L1 命中即高置信度
                "reason": "L1 关键词命中",
                "layer": "L1",
            }

        # L1 未命中 → L2 LLM
        if self.l2:
            l2_result = self.l2.classify(query)
            if l2_result:
                return {**l2_result, "layer": "L2"}

        # 都没命中
        return None


class _L1Router:
    """L1 关键词路由（薄包装，对外暴露 find() 方法）。"""

    def __init__(self, skills):
        self.skills = skills

    def find(self, query: str):
        q = (query or "").lower()
        for skill in self.skills:
            for intent in skill.intents:
                if intent.lower() in q:
                    return skill
        return None
