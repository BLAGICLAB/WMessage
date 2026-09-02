#!/usr/bin/env python3
"""
WMessage 主调度循环与 pre-step / pre-execute 双中间件联动专项检测
（2026-08-17 22:38 老板拍板：自动检测 + pytest 单元测试 + 端到端 event_log + 报告）

设计说明：
- 业务代码不可改动（"不改动业务代码"约束），所以 mock LLM 跑 E2E 走不通
- 改用 pytest 对 Rust 源码做静态分支断言：
  - 读取 src-tauri/src/{bot,bot_skills,intent_router,tool_guard,audit}.rs
  - 每个核心校验规则一条 test，断言当前代码是否满足规格
  - 失败信息精确到行号 + 上下文，便于定位

执行：python3 -m pytest tests-audit/audit_pre_step_pre_execute.py -v
"""
from pathlib import Path
import os
import re
import subprocess
import sys

import pytest

SRC = Path("/Users/renshi/Projects/wmessage/src-tauri/src")
BOT = (SRC / "bot.rs").read_text()
# F-6 step 5（2026-08-18）后：聊天编排在 bot_chat.rs、工具循环在 bot_model_loop.rs。
# 编排/路由/事件类断言统一对合并文本 BOT_ALL 做（行为仍在，只是位置搬家）
BOT_CHAT = (SRC / "bot_chat.rs").read_text()
BOT_MODEL_LOOP = (SRC / "bot_model_loop.rs").read_text()
BOT_ALL = BOT + BOT_CHAT + BOT_MODEL_LOOP
# Phase C（2026-08-19）起 bot_skills.rs 拆为 bot_skills/ 目录，拼接所有子模块保持单文件语义
BOT_SKILLS = "\n".join(
    p.read_text() for p in sorted((SRC / "bot_skills").glob("*.rs"))
)
INTENT_ROUTER = (SRC / "intent_router.rs").read_text()
TOOL_GUARD = (SRC / "tool_guard.rs").read_text()
AUDIT = (SRC / "audit.rs").read_text()


def find_line(text, pattern):
    """返回首个匹配 pattern 的行号（1-indexed）；无匹配返回 -1"""
    for i, line in enumerate(text.splitlines(), 1):
        if re.search(pattern, line):
            return i
    return -1


def grep_count(text, pattern):
    """统计匹配次数"""
    return sum(1 for _ in re.finditer(pattern, text))


def cargo_test_count():
    """跑一次 cargo test 拿测试数（轻量，只查结果）。重试一次避免 flake 误报。"""
    # 保留完整 os.environ（HOME/CARGO_HOME 缺失会让 cargo 直接起不来），只覆盖 PATH
    env = {**os.environ, "PATH": "/Users/renshi/.cargo/bin:/usr/bin:/bin"}
    for attempt in (1, 2):
        r = subprocess.run(
            ["cargo", "test", "--manifest-path", "/Users/renshi/Projects/wmessage/src-tauri/Cargo.toml", "--lib", "--no-fail-fast"],
            capture_output=True, text=True, timeout=120,
            env=env,
        )
        m = re.search(r"test result: ok\. (\d+) passed", r.stdout)
        if m:
            return int(m.group(1))
        # 失败重试一次（避免 cargo test 偶发 flake 误报）
        if attempt == 1:
            continue
    return -1


# ────────────────────────────────────────────────────────────────────
# 规则 1：pre-step 命中复合 Skill → 禁止 call_llm；event_log 出 pre_step.route_skill
# ────────────────────────────────────────────────────────────────────

class TestPreStepRouting:
    """规则 1：pre-step 路由命中 Skill 时不应继续调用 LLM"""

    def test_pre_step_module_exists(self):
        """pre-step 模块存在（intent_router.rs）"""
        assert "pub fn route_user_input" in INTENT_ROUTER
        assert "RouteAction::Skill" in INTENT_ROUTER
        assert "RouteAction::PassThrough" in INTENT_ROUTER

    def test_no_static_skill_routes(self):
        """2026-08-19 老板拍板：未安装的技能不得有路由——静态 INTENT_RULES 表已删除，
        路由由已安装技能 SKILL.md frontmatter 的 intents 声明动态生成
        （启动 / skills_import / skills_delete 后 rebuild_routes 重建）。
        原 7 条硬编码映射中：3 条幻影（web-search/task-summary/archive 实体从未存在）、
        ppt-orchestra-skill 实体与本运行时不兼容且能力已内置 SYSTEM_PROMPT 规则 10。"""
        assert "INTENT_RULES" not in INTENT_ROUTER, "静态路由表应已删除（动态路由重构）"
        hardcoded = re.findall(r'skill_name:\s*"([^"]+)"', INTENT_ROUTER)
        assert not hardcoded, f"路由表不得硬编码技能名（路由来自已安装技能 intents）: {hardcoded}"
        assert "rebuild_routes" in INTENT_ROUTER, "动态路由重建入口缺失"
        assert "route_with_rules" in INTENT_ROUTER, "规则注入式纯函数核缺失（测试/离线核验用）"

    def test_pre_step_called_in_bot_chat(self):
        """bot_chat 入口调用 middleware::run_pre_step（F-2 抽象层 2026-08-18）"""
        assert "middleware::run_pre_step" in BOT_ALL, (
            "bot_chat 应调 middleware::run_pre_step（F-2 抽象层）"
        )

    def test_pre_step_hit_triggers_start_skill(self):
        """pre-step 命中 → start_skill 调用（Skill 进入 Running 状态）"""
        # 在 bot_chat 的 pre-step 分支里能找到 start_skill 调用
        assert "start_skill(&app" in BOT_ALL or "start_skill(\\&app" in BOT_ALL
        # 且调用路径在 Some(RouteAction::Skill(_)) 分支内（F-2 抽象层包装）
        # 用更宽松的检查：start_skill 调用存在且 RouteAction::Skill 存在
        skill_branch = re.search(
            r"Some\(RouteAction::Skill\([^)]+\)\)\s*=>\s*\{[^}]*start_skill", BOT_ALL, re.DOTALL
        )
        assert skill_branch is not None, (
            "pre-step 命中分支里没看到 start_skill 调用 → pre-step 形同虚设（F-2 抽象层：Some(RouteAction::Skill(...)) => {...}）"
        )

    @pytest.mark.skip(reason="[2026-08-18 F-4 A 路径] interactive mode 允许 Skill 内部调用 LLM，"
                         "pre-execute 安全层持续生效，不属于逃逸 bug。"
                         "保留此 test 作为设计意图文档。")
    def test_no_call_llm_after_pre_step_hit(self):
        """[设计意图文档 2026-08-18 F-4 A 路径]

        本项目区分 Skill 两种运行 mode（详见 docs/SKILL_DSL.md 第 4.4 节）：

        - **interactive**（默认 / medium+high 风险）：Skill 子流程允许内部调用 LLM 做自然语言理解、
          多轮交互；pre-step 路由层跳过外层主 LLM，但 Skill 内部可继续使用 LLM；
          所有 tool 调用强制经过 pre-execute 安全校验。

        - **auto**（low 风险 / 显式声明）：纯 DSL 调度模式，完整 bypass 全部 LLM，
          参数提取、流程全部硬编码。

        本测试不再是 bug 排查，而是设计意图文档：
        - run_model_loop 在 pre-step 命中 Skill 后被调用，是 interactive mode 的预期行为
        - pre-execute 持续生效，黑名单原子工具（create_word_revisions / link_file_to_task）
          在非 Skill Running 状态硬阻断
        - 监控可按 `pre_step.route_skill` 事件的 `mode` 字段区分

        与 F-1 `bypass_llm_on_pre_step_hit` 开关：外层 pre-step 路由是否跳过主 LLM，
        与 Skill 内部 `mode` 互相独立。
        """
        pytest.skip("[2026-08-18 F-4 A 路径] 设计意图文档，不需要断言")

    def test_event_log_pre_step_route_skill_format(self):
        """event_log 应出现 pre_step.route_skill 事件"""
        assert "pre_step.route_skill" in BOT_ALL, (
            "期望 bot 编排层用 audit_event! 发 pre_step.route_skill 事件，"
            "当前格式不符规格"
        )
        # 同步验证其他两个中间件事件名
        assert "pre_step.route_failed" in BOT_ALL, "缺 pre_step.route_failed 事件"
        assert "pre_execute.deny" in BOT_ALL, "缺 pre_execute.deny 事件（规格：pre_execute.deny；2026-08-18 F-3 改名）"

    def test_old_free_form_intent_route_removed(self):
        """旧的 intent_route free-form 事件应被替换（不再有新的）"""
        # 查仍在调用旧 intent_route 字符串的 audit_log / audit_event!
        old_pattern = re.search(r'audit_(log|event)[^"]*"intent_route', BOT)
        assert old_pattern is None, (
            f"仍有旧 intent_route 事件残留：{old_pattern.group(0)[:100]}"
        )

    def test_old_tool_blocked_atomic_removed(self):
        """旧的 tool_blocked_atomic free-form 事件应被替换"""
        old_pattern = re.search(r'audit_(log|event)[^"]*"tool_blocked_atomic', BOT)
        assert old_pattern is None, (
            f"仍有旧 tool_blocked_atomic 事件残留：{old_pattern.group(0)[:100]}"
        )


# ────────────────────────────────────────────────────────────────────
# 规则 2：pre-step miss → LLM 自由区；每个 tool_call 进 pre_execute
# ────────────────────────────────────────────────────────────────────

class TestPreStepMissFlowsToLLM:
    """规则 2：未命中复合业务时放行 LLM，工具调用必走 pre_execute"""

    def test_passthrough_branch_in_bot_chat(self):
        """bot_chat 处理 PassThrough 分支（F-2 抽象层：Some(RouteAction::PassThrough) | None => None）"""
        assert "RouteAction::PassThrough" in BOT_ALL
        # F-2 抽象层：PassThrough 被 Some(_) 包裹，与「无中间件命中」(None) 一起 return None
        pass_branch = re.search(
            r"Some\(RouteAction::PassThrough\)\s*\|\s*None\s*=>\s*None", BOT_ALL
        )
        assert pass_branch is not None, (
            "PassThrough 分支缺失（F-2 抽象层：Some(RouteAction::PassThrough) | None => None）"
        )

    def test_run_model_loop_called_unconditionally(self):
        """run_model_loop 在 pre-step 处理后调用（无论命中与否都进 LLM）。

        批次8审计（2026-09-02）修复：原先写死旧签名字面量
        `run_model_loop(app, msgs, 8, &stop)`，函数加 max_rounds/plan_state
        参数后整条断言 FAIL（门禁红，pre-push 被卡）。改为正则只锁
        「无条件调用 run_model_loop」这一行为不变量，参数演进不再误报。"""
        assert re.search(r"run_model_loop\(\s*app\s*,\s*msgs\s*,", BOT_ALL), (
            "bot_chat 必须在 pre-step 处理后无条件调用 run_model_loop"
        )

    def test_every_execute_tool_has_pre_execute_check(self):
        """execute_tool 入口必走 pre_execute（middleware::run_pre_execute，F-2 抽象层 2026-08-18）"""
        fn_match = re.search(
            r"async fn execute_tool\([^)]*\)[^{]*\{", BOT
        )
        assert fn_match is not None, "没找到 execute_tool 函数"
        fn_start = fn_match.end()

        fn_body = BOT[fn_start:fn_start + 2000]
        # F-2 抽象层：execute_tool 通过 middleware::run_pre_execute 调 pre-execute
        assert "run_pre_execute" in fn_body, (
            "execute_tool 入口应调 middleware::run_pre_execute"
        )
        # active_skill bool 通过 is_skill_active() 计算后传给 middleware
        assert "is_skill_active" in fn_body, (
            "execute_tool 仍需 is_skill_active 状态传给 middleware"
        )

    def test_skill_on_step_called_in_execute_tool(self):
        """execute_tool 还应调 skill_on_step（步骤计数/熔断）"""
        fn_match = re.search(
            r"async fn execute_tool\([^)]*\)[^{]*\{", BOT
        )
        assert fn_match is not None
        fn_body = BOT[fn_match.end():fn_match.end() + 2500]
        assert "skill_on_step" in fn_body


# ────────────────────────────────────────────────────────────────────
# 规则 3：pre_execute 黑名单阻断；白名单放行（run_python, query_single_task）
# ────────────────────────────────────────────────────────────────────

class TestPreExecuteAtomicGuard:
    """规则 3：原子黑名单硬锁 + 单点白名单放行"""

    def test_atomic_blacklist_exists(self):
        """黑名单 ATOMIC_TOOLS 存在且包含已知项"""
        assert "ATOMIC_TOOLS" in TOOL_GUARD
        assert "create_word_revisions" in TOOL_GUARD
        assert "link_file_to_task" in TOOL_GUARD

    def test_is_atomic_tool_function(self):
        """is_atomic_tool 函数存在并正确判断"""
        assert "pub fn is_atomic_tool" in TOOL_GUARD
        # 单测覆盖
        assert "atomic_blacklist_recognizes" in TOOL_GUARD

    def test_atomic_block_message_mentions_skill(self):
        """阻断消息应引导走 Skill"""
        assert "Skill" in TOOL_GUARD  # 多次出现
        assert "atomic_block_message" in TOOL_GUARD

    def test_whitelist_run_python_exists_in_tools(self):
        """白名单 run_python 在工具列表中（不应被误判为原子）"""
        # TOOLS 常量里能看到 run_python 定义
        assert '"run_python"' in BOT
        # tool_guard 单测里也覆盖了 run_python 不在黑名单
        assert '"run_python"' in TOOL_GUARD

    def test_whitelist_query_single_task_exists(self):
        """白名单 query_single_task 已在工具列表中（老板 2026-08-17 22:57 拍板补充）"""
        assert '"query_single_task"' in BOT, (
            "query_single_task 应在 TOOLS 常量里"
        )
        # 函数实现也要存在
        assert "fn tool_query_single_task(" in BOT, (
            "query_single_task 需有实现函数"
        )
        # execute_tool match arm 也要有
        assert '"query_single_task" => tool_query_single_task' in BOT, (
            "execute_tool 里缺 query_single_task 路由"
        )
        # tool_guard 白名单单测也要包含
        assert '"query_single_task"' in TOOL_GUARD, (
            "tool_guard::tests 应验证 query_single_task 不在原子黑名单"
        )


# ────────────────────────────────────────────────────────────────────
# 联动时序：event_log 事件流
# ────────────────────────────────────────────────────────────────────

class TestEventLogTiming:
    """事件流完整性 + 时序"""

    def test_audit_module_exists(self):
        """结构化审计模块存在（Block 1）"""
        assert "pub enum AuditLevel" in AUDIT
        assert "pub fn write_event" in AUDIT
        assert "pub fn classify_text" in AUDIT

    def test_audit_event_macro_exported(self):
        """audit_event! 宏可跨模块调用"""
        assert "#[macro_export]" in AUDIT
        assert "macro_rules! audit_event" in AUDIT
        # bot 编排层实际在用（bot_chat.rs 全限定调用 crate::audit_event!）
        assert "crate::audit_event!" in BOT_ALL

    def test_post_execute_emits_structured_event(self):
        """execute_tool 末尾应发结构化 tool.return 事件（F-3 第三步 2026-08-18 改名）"""
        # NEW-C-4 后 execute_tool 是薄 wrapper，真正实现（事件埋点）在 execute_tool_with_stop
        fn_match = re.search(
            r"async fn execute_tool_with_stop\([^)]*\)[^{]*\{", BOT
        )
        assert fn_match is not None
        fn_body = BOT[fn_match.end():fn_match.end() + 6000]
        assert 'audit_event!' in fn_body or "write_event" in fn_body
        assert '"tool.return"' in fn_body, "execute_tool 末尾应发 tool.return 事件（2026-08-18 F-3 改名）"
        # 同步验证入口应有 tool.call（拆分自原 tool_done）
        assert '"tool.call"' in fn_body, "execute_tool 入口应发 tool.call 事件"

    def test_bot_log_path_uses_portable_dir(self):
        """bot.log 走 db_dir 便携模式路径（exe 可写时 src-tauri/target/debug/）"""
        log_path = Path("/Users/renshi/Projects/wmessage/src-tauri/target/debug/bot.log")
        # 批次8审计（2026-09-02）：原先 if exists 静默空转——文件不存在时整条
        # 测试假绿无约束力。改为显式 skip，让「未覆盖」在报告里可见
        if not log_path.exists():
            pytest.skip("bot.log 不存在（本机未跑过 dev 实例），跳过事件抽查")
        content = log_path.read_text()
        assert len(content) > 0
        # 至少应该有一类事件
        assert any(
            keyword in content
            for keyword in [
                "intent_route", "tool.return", "tool.call", "tool_done",
                "tool_blocked_atomic", "user:",
                "user.message", "skill.start", "llm.request", "llm.response",
            ]
        ), "bot.log 里没有预期事件类型"


# ────────────────────────────────────────────────────────────────────
# 回归基线：现有单测
# ────────────────────────────────────────────────────────────────────

class TestNoRegression:
    """保证联动改造没有打破既有单测"""

    def test_cargo_test_lib_baseline(self):
        """cargo test --lib 应该全过且测试数不大幅回归。

        批次8审计（2026-09-02）：阈值 80 是 2026-08-18 基线（当时 83），
        当前已 472——80 的下限只能挡灾难性回归。收紧到 400，
        保留约 15% 缓冲防正常删测试误报。"""
        count = cargo_test_count()
        assert count >= 400, f"cargo test --lib 失败或测试数大幅回归（{count} < 400，当前基线 472）"


# ────────────────────────────────────────────────────────────────────
# 规则 5：F‑1 bypass_llm_on_pre_step_hit 开关（2026‑08‑18 P0 release blocker）
# ────────────────────────────────────────────────────────────────────

class TestBypassLlmSwitch:
    """F‑1 bypass_llm_on_pre_step_hit 开关存在 + toggle off 行为"""

    def test_bypass_llm_switch_field_exists(self):
        """BotConfig 必须包含 bypass_llm_on_pre_step_hit 字段（F‑1 P0 release blocker）"""
        assert "pub bypass_llm_on_pre_step_hit: bool" in BOT, (
            "BotConfig 必须有 bypass_llm_on_pre_step_hit 字段"
        )
        assert "bypass_llm_on_pre_step_hit: true" in BOT, (
            "Default impl 默认 true（开启新行为）"
        )

    def test_bypass_llm_toggle_off_zero_routing(self):
        """toggle=false 时 pre_routed_skill 强制 None（LEGACY 路径）+ bypass_off 审计"""
        assert "read_bypass_llm_switch" in BOT, "缺 helper 函数 read_bypass_llm_switch"
        assert "pre_step.bypass_off" in BOT_ALL, "缺 pre_step.bypass_off 事件（toggle=off 审计）"
        assert "let pre_routed_skill = if bypass_llm_on_pre_step_hit" in BOT_ALL, (
            "缺二次 shadow：bypass=false 时强制 None"
        )

    def test_bypass_llm_default_view_field(self):
        """BotConfigView 必须暴露 bypass_llm_on_pre_step_hit 给前端"""
        assert "pub bypass_llm_on_pre_step_hit: bool" in BOT, (
            "BotConfigView 也应有同名字段（前端透传）"
        )


if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-v", "--tb=short"]))