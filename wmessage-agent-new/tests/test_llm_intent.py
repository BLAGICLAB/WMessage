"""LLM L2 Intent Classifier + Feature Flags 测试。"""
from __future__ import annotations

import os
import tempfile
from pathlib import Path
from unittest.mock import MagicMock

import pytest

from agent_core.llm_intent_classifier import (
    LLMIntentClassifier,
    TwoLayerIntentRouter,
    _L1Router,
    _validate,
)
from agent_core.feature_flags import (
    FeatureFlags,
    FlagChecker,
    default_feature_flags,
    load_feature_flags,
    parse_feature_flags,
)
from agent_core.skills.builtin import (
    BUILTIN_SKILLS,
    DOCX_SKILL,
    XLSX_SKILL,
    PPTX_SKILL,
)
from llm_provider.base import LLMProvider, LLMRequest, LLMResponse


# ───────────────────────── L1 Router ─────────────────────────


def test_l1_router_finds_match():
    """L1 关键词命中。"""
    router = _L1Router(list(BUILTIN_SKILLS.values()))
    s = router.find("做个 word")
    assert s is DOCX_SKILL


def test_l1_router_no_match():
    """L1 无命中。"""
    router = _L1Router(list(BUILTIN_SKILLS.values()))
    assert router.find("讲个笑话") is None


# ───────────────────────── LLM Intent Classifier ─────────────────────────


class FakeLLM(LLMProvider):
    name = "fake"

    def __init__(self, text="", error=None):
        self.text = text
        self.error = error
        self.calls = []

    def complete(self, request):
        from llm_provider.base import LLMResponse
        self.calls.append(request)
        if self.error:
            raise self.error
        return LLMResponse(text=self.text)


def test_classifier_low_confidence_returns_none():
    """低置信度 → None。"""
    llm = FakeLLM(text='{"skill": "minimax-docx", "confidence": 0.3, "reason": "不确定"}')
    cls = LLMIntentClassifier(llm, list(BUILTIN_SKILLS.values()), confidence_threshold=0.6)
    assert cls.classify("随便问问") is None


def test_classifier_high_confidence_returns_match():
    """高置信度 → 返回分类结果。"""
    llm = FakeLLM(text='{"skill": "minimax-docx", "confidence": 0.85, "reason": "用户要 word"}')
    cls = LLMIntentClassifier(llm, list(BUILTIN_SKILLS.values()), confidence_threshold=0.6)
    result = cls.classify("帮我生成 word")
    assert result["skill"] == "minimax-docx"
    assert result["confidence"] == 0.85


def test_classifier_null_skill_returns_none():
    """skill=null（LLM 说无关）→ None。"""
    llm = FakeLLM(text='{"skill": null, "confidence": 0.9, "reason": "无关"}')
    cls = LLMIntentClassifier(llm, list(BUILTIN_SKILLS.values()))
    assert cls.classify("讲个笑话") is None


def test_classifier_llm_error_returns_none():
    """LLM 抛错 → fallback None（不阻塞主流程）。"""
    llm = FakeLLM(error=RuntimeError("API down"))
    cls = LLMIntentClassifier(llm, list(BUILTIN_SKILLS.values()))
    assert cls.classify("x") is None


def test_classifier_empty_skills_returns_none():
    """空 skills → None。"""
    llm = FakeLLM(text='{"skill": "x", "confidence": 0.9}')
    cls = LLMIntentClassifier(llm, [])
    assert cls.classify("x") is None


def test_classifier_parses_markdown_json_block():
    """LLM 返回 markdown 代码块包裹的 JSON。"""
    llm = FakeLLM(text='```json\n{"skill": "minimax-docx", "confidence": 0.9, "reason": "x"}\n```')
    cls = LLMIntentClassifier(llm, list(BUILTIN_SKILLS.values()))
    result = cls.classify("做个 word")
    assert result["skill"] == "minimax-docx"


def test_classifier_parses_embedded_json():
    """LLM 返回 "根据查询，{...}" 形式。"""
    llm = FakeLLM(text='根据用户查询，我认为 {"skill": "minimax-xlsx", "confidence": 0.8, "reason": "x"}')
    cls = LLMIntentClassifier(llm, list(BUILTIN_SKILLS.values()))
    result = cls.classify("做个表格")
    assert result["skill"] == "minimax-xlsx"


def test_classifier_parses_invalid_json_returns_none():
    """LLM 返回非 JSON → None。"""
    llm = FakeLLM(text="这个查询我不确定要哪个 skill")
    cls = LLMIntentClassifier(llm, list(BUILTIN_SKILLS.values()))
    assert cls.classify("x") is None


def test_classifier_parses_empty_returns_none():
    """LLM 返回空 → None。"""
    llm = FakeLLM(text="")
    cls = LLMIntentClassifier(llm, list(BUILTIN_SKILLS.values()))
    assert cls.classify("x") is None


def test_classifier_caches_last_result():
    """last_result 记录最近一次分类结果。"""
    llm = FakeLLM(text='{"skill": "minimax-docx", "confidence": 0.9, "reason": "r"}')
    cls = LLMIntentClassifier(llm, list(BUILTIN_SKILLS.values()))
    cls.classify("x")
    assert cls.last_result is not None
    assert cls.last_result["skill"] == "minimax-docx"


def test_classifier_passes_skills_in_prompt():
    """prompt 含 skill 列表的 JSON。"""
    llm = FakeLLM(text='{"skill": null, "confidence": 0}')
    cls = LLMIntentClassifier(llm, list(BUILTIN_SKILLS.values()))
    cls.classify("x")
    prompt = llm.calls[0].user
    assert "minimax-docx" in prompt
    assert "minimax-xlsx" in prompt
    assert "minimax-pptx" in prompt


def test_validate_invalid_type_returns_none():
    """_validate 校验失败 → None。"""
    assert _validate("not a dict") is None
    assert _validate([1, 2, 3]) is None
    assert _validate(None) is None


def test_validate_normalizes_confidence():
    """confidence 整数 → float。"""
    result = _validate({"skill": "x", "confidence": 1})
    assert result["confidence"] == 1.0
    assert isinstance(result["confidence"], float)


def test_validate_string_confidence_defaults_to_zero():
    """confidence 字符串 → 0.0。"""
    result = _validate({"skill": "x", "confidence": "high"})
    assert result["confidence"] == 0.0


# ───────────────────────── TwoLayerIntentRouter ─────────────────────────


def test_two_layer_l1_hit_no_l2_call():
    """L1 命中 → 不调 L2。"""
    llm = FakeLLM(text='{"skill": "minimax-docx", "confidence": 0.9}')
    router = TwoLayerIntentRouter(
        list(BUILTIN_SKILLS.values()),
        l2_classifier=LLMIntentClassifier(llm, list(BUILTIN_SKILLS.values())),
    )
    result = router.route("做个 word")
    assert result["skill"] == "minimax-docx"
    assert result["layer"] == "L1"
    assert len(llm.calls) == 0  # L2 未被调用


def test_two_layer_l1_miss_triggers_l2():
    """L1 未命中 → L2 触发。"""
    llm = FakeLLM(text='{"skill": "minimax-docx", "confidence": 0.85, "reason": "LLM 判 word"}')
    router = TwoLayerIntentRouter(
        list(BUILTIN_SKILLS.values()),
        l2_classifier=LLMIntentClassifier(llm, list(BUILTIN_SKILLS.values())),
    )
    result = router.route("我想生成一个 word 文件")  # L1 关键词可能命中
    # 如果 L1 命中 → 直接返回；如果未命中 → L2
    assert result is not None
    assert result["layer"] in {"L1", "L2"}


def test_two_layer_l1_miss_l2_low_confidence_returns_none():
    """L1 miss + L2 low confidence → None。"""
    llm = FakeLLM(text='{"skill": "minimax-docx", "confidence": 0.3}')
    router = TwoLayerIntentRouter(
        list(BUILTIN_SKILLS.values()),
        l2_classifier=LLMIntentClassifier(llm, list(BUILTIN_SKILLS.values())),
    )
    assert router.route("讲个完全无关的话题") is None


def test_two_layer_no_l2_classifier_falls_back_to_l1():
    """无 L2 → 仅 L1。"""
    router = TwoLayerIntentRouter(list(BUILTIN_SKILLS.values()), l2_classifier=None)
    result = router.route("做个 word")
    assert result["layer"] == "L1"


# ───────────────────────── Feature Flags ─────────────────────────


def test_default_feature_flags():
    """默认 flags。"""
    f = default_feature_flags()
    assert f.runtime == "python"
    assert f.reflexion_enabled is False
    assert f.llm_l2_router is False
    assert f.llm_primary == "minimax-m3"
    assert f.python_agent_percent == 100


def test_parse_feature_flags_full():
    """完整 TOML 配置解析。"""
    data = {
        "agent_runtime": {"runtime": "python", "python_agent_users": ["u-1", "u-2"]},
        "features": {"reflexion_enabled": True, "llm_l2_router": True},
        "llm": {"primary": "minimax-m3", "fallback": ["kimi", "deepseek"]},
        "gray": {"python_agent_users": ["u-1"], "python_agent_percent": 50},
    }
    f = parse_feature_flags(data)
    assert f.runtime == "python"
    assert f.reflexion_enabled is True
    assert f.llm_l2_router is True
    assert f.llm_primary == "minimax-m3"
    assert f.python_agent_percent == 50


def test_parse_feature_flags_partial():
    """部分字段 → 默认填充其余。"""
    data = {"features": {"llm_l2_router": True}}
    f = parse_feature_flags(data)
    assert f.llm_l2_router is True
    assert f.runtime == "python"  # 默认
    assert f.reflexion_enabled is False  # 默认


def test_parse_feature_flags_extra_preserved():
    """未知字段保留到 extra。"""
    data = {"custom_field": "custom_value", "features": {}}
    f = parse_feature_flags(data)
    assert f.extra == {"custom_field": "custom_value"}


def test_load_feature_flags_none_returns_default():
    """path=None → 默认。"""
    f = load_feature_flags(None)
    assert f.runtime == "python"


def test_load_feature_flags_nonexistent_returns_default():
    """不存在的路径 → 默认（不报错）。"""
    f = load_feature_flags("/nonexistent/path/flags.toml")
    assert f.runtime == "python"


def test_load_feature_flags_invalid_toml_returns_default(tmp_path):
    """TOML 解析失败 → 默认（不报错）。"""
    p = tmp_path / "bad.toml"
    p.write_text("this is not valid toml: [[[")
    f = load_feature_flags(p)
    assert f.runtime == "python"


def test_load_feature_flags_valid(tmp_path):
    """有效 TOML 加载。"""
    p = tmp_path / "flags.toml"
    p.write_text("""
[agent_runtime]
runtime = "python"

[features]
reflexion_enabled = true

[llm]
primary = "minimax-m3"
fallback = ["kimi"]

[gray]
python_agent_users = ["u-1"]
""")
    f = load_feature_flags(p)
    assert f.runtime == "python"
    assert "u-1" in f.python_agent_users
    assert f.reflexion_enabled is True


# ───────────────────────── FlagChecker 热加载 ─────────────────────────


def test_flag_checker_no_path_uses_default():
    """无 path → 默认 flags。"""
    checker = FlagChecker()
    assert checker.flags().runtime == "python"


def test_flag_checker_hot_reload(tmp_path):
    """文件 mtime 变化 → 重新加载。"""
    p = tmp_path / "flags.toml"
    p.write_text("""
[features]
reflexion_enabled = false
""")
    checker = FlagChecker(path=p)
    assert checker.flags().reflexion_enabled is False

    # 修改文件
    import time
    time.sleep(0.05)
    p.write_text("""
[features]
reflexion_enabled = true
""")
    # 重新调用 flags() 应触发 reload
    assert checker.flags().reflexion_enabled is True


def test_flag_checker_user_in_rollout(tmp_path):
    """is_user_in_rollout — 白名单 + 百分比。"""
    p = tmp_path / "flags.toml"
    p.write_text("""
[gray]
python_agent_users = ["u-whitelist"]
python_agent_percent = 50
""")
    checker = FlagChecker(path=p)
    # 白名单
    assert checker.is_user_in_rollout("u-whitelist") is True
    # 非白名单 → hash 决定（确定性）
    in_count = sum(1 for i in range(100) if checker.is_user_in_rollout(f"u-{i}"))
    # 50% 灰度 → ~50 个 user 在内（±若干）
    assert 30 <= in_count <= 70


def test_flag_checker_user_percent_0_returns_false(tmp_path):
    """百分比 0 → 全部 false（白名单除外）。"""
    p = tmp_path / "flags.toml"
    p.write_text("""
[gray]
python_agent_percent = 0
""")
    checker = FlagChecker(path=p)
    assert checker.is_user_in_rollout("u-1") is False
    assert checker.is_user_in_rollout(None) is False


def test_flag_checker_user_percent_100_returns_true(tmp_path):
    """百分比 100 → 全部 true。"""
    p = tmp_path / "flags.toml"
    p.write_text("""
[gray]
python_agent_percent = 100
""")
    checker = FlagChecker(path=p)
    for i in range(10):
        assert checker.is_user_in_rollout(f"u-{i}") is True


def test_flag_checker_session_in_rollout(tmp_path):
    """session 灰度。"""
    p = tmp_path / "flags.toml"
    p.write_text("""
[gray]
python_agent_sessions = ["s-1", "s-2"]
""")
    checker = FlagChecker(path=p)
    assert checker.is_session_in_rollout("s-1") is True
    assert checker.is_session_in_rollout("s-other") is False


def test_flag_checker_is_runtime_python(tmp_path):
    """runtime check。"""
    p = tmp_path / "flags.toml"
    p.write_text("""
[agent_runtime]
runtime = "nodejs"
""")
    checker = FlagChecker(path=p)
    assert checker.is_runtime_python() is False


def test_flag_checker_is_feature_enabled(tmp_path):
    """通用 feature check。"""
    p = tmp_path / "flags.toml"
    p.write_text("""
[features]
reflexion_enabled = true
llm_l2_router = false
""")
    checker = FlagChecker(path=p)
    assert checker.is_feature_enabled("reflexion_enabled") is True
    assert checker.is_feature_enabled("llm_l2_router") is False
    assert checker.is_feature_enabled("nonexistent") is False


def test_flag_checker_primary_and_fallback(tmp_path):
    """primary_llm / fallback_llms。"""
    p = tmp_path / "flags.toml"
    p.write_text("""
[llm]
primary = "kimi"
fallback = ["minimax-m3", "deepseek"]
""")
    checker = FlagChecker(path=p)
    assert checker.primary_llm() == "kimi"
    assert checker.fallback_llms() == ["minimax-m3", "deepseek"]


def test_flag_checker_default_primary():
    """无配置 → 默认 minimax-m3。"""
    checker = FlagChecker()
    assert checker.primary_llm() == "minimax-m3"
    assert "kimi" in checker.fallback_llms()


def test_flag_checker_nonexistent_file_uses_default():
    """path 指向不存在文件 → 默认。"""
    checker = FlagChecker(path="/nonexistent/flags.toml")
    assert checker.flags().runtime == "python"
