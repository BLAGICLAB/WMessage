# Batch Spec: RE1-MP02-LAND（前会话遗留工作树收口）

## 目的

用户指令 2026-09-27：以 SUBAGENT 设计文档为基线走全自动批循环。开工时发现工作树
残留前一会话的**已完成未提交**改动：RE-1（推理强度档位）+ MP-02（模型下拉双协议
同列）两特性在 ChatPanel.tsx 内交织、无法忠实拆分为两个家族提交。本批 = **原样
落地**该遗留树（零逻辑改动），为 SUBA-2 解锁 run_model_loop 签名依赖（RE-1 给
run_model_loop 增 reasoning_override 参数，SUBA-2 的 runner 调用必须基于该签名）。
本批非新开发批：findings 由原会话拥有，此处仅收口登记；family 异质为落地批固有
属性（stop 条件豁免，理由在案）。

## 内容（原样落地，逐文件归属标注）

- **RE-1 推理强度**（family: reasoning-effort）：
  - bot/reasoning.rs（新，356 行含 8 测试）：EffortLevel 抽象档位 → 按模型族/
    协议映射 ReasoningWire（GLM ≥5.3 effort / 旧版 thinking 开关、OpenAI
    reasoning_effort、Anthropic thinking.budget_tokens 按模型钳制），unknown 模型不发字段
  - config/types.rs：BotConfig.reasoning_effort 后台默认（None=medium）+ View 透传
  - bot_model_loop.rs：LlmHttp.reasoning + run_model_loop 增 reasoning_override
    参数 + 请求体注入（OpenAI 字段 / Anthropic thinking 块）+ llm.request 审计带档位
  - bot_anthropic.rs：apply_anthropic_thinking 注入函数
  - bot_chat.rs：bot_chat 命令增 reasoning_effort 单次覆盖参（会话级，不回写）
  - exec_steps.rs / run_task_in_chat：后台链路传 None（按全局默认）
  - SettingsPage：推理强度后台默认四档切换；ChatPanel：⚡ 会话级覆盖 + 生效档位显示
- **MP-02 模型下拉双协议同列**（family: model-picker-dual-protocol）：
  - config/commands.rs：apply_active_model_switch 跨协议命中连协议一起切
  - ChatPanel：🧠 下拉双协议分组展示 + 跨协议同步本地协议态 + 🧠 迁至输入卡底栏
- docs/MANUAL-SMOKE-ACCEPTANCE-2026-09-26.md：冒烟清单补勾（DEC-1/MP-01 回归项）
- docs/rust-bot-architecture.md：bot/reasoning.rs 模块树登记（module_map 门禁）
- 其余为两特性配套测试（llm_integration +94 / ChatPanel.test +142 / SettingsPage.test
  +49 / WidgetApp.test +7）

## 落地时本会话仅有的两处机械修正

1. cargo fmt 对 reasoning.rs 的纯格式化（原会话漏跑 fmt，diff 一处）
2. 架构文档补 reasoning.rs 条目（原会话遗漏 module_map 登记）

## 测试

- 三层全绿（本会话实测）：cargo fmt/check + tsc + test-all（Rust 1202 含 RE-1 新增
  reasoning 8 测试 + llm_integration 集成 + ChatPanel/SettingsPage/WidgetApp vitest）
- spec 自查：expected_files 18 = git status 全量；assertions_min 按 reasoning.rs
  实测 30；budget 按 numstat 实测（落地批按实申报，不设上限截断）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "RE1-MP02-LAND",
  "family": "pending-work-landing",
  "expected_files": [
    "docs/MANUAL-SMOKE-ACCEPTANCE-2026-09-26.md",
    "docs/rust-bot-architecture.md",
    "src-tauri/src/bot.rs",
    "src-tauri/src/bot/reasoning.rs",
    "src-tauri/src/bot/config/commands.rs",
    "src-tauri/src/bot/config/mod.rs",
    "src-tauri/src/bot/config/types.rs",
    "src-tauri/src/bot_anthropic.rs",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/bot_model_loop.rs",
    "src-tauri/src/exec_steps.rs",
    "src-tauri/tests/llm_integration.rs",
    "src-tauri/tests/task_chat_exec.rs",
    "src/components/ChatPanel.test.tsx",
    "src/components/ChatPanel/ChatPanel.tsx",
    "src/components/SettingsPage.test.tsx",
    "src/components/SettingsPage/SettingsPage.tsx",
    "src/components/WidgetApp.test.tsx"
  ],
  "max_lines_added": 1200,
  "max_lines_removed": 250,
  "max_new_files_lines": 400,
  "findings": [
    {"id": "RE1-MP02-LAND", "file": "src-tauri/src/bot/reasoning.rs", "line": 1, "fix": "原样落地：推理强度档位→线上参数映射（GLM/OpenAI/Anthropic 三协议），8 单测"},
    {"id": "RE1-MP02-LAND", "file": "src-tauri/src/bot_model_loop.rs", "line": 399, "fix": "原样落地：run_model_loop 增 reasoning_override 参数 + 请求体注入 + 审计"},
    {"id": "RE1-MP02-LAND", "file": "src/components/ChatPanel/ChatPanel.tsx", "line": 1, "fix": "原样落地：⚡ 会话级推理覆盖 + 🧠 双协议同列下拉（两特性交织，不可拆）"}
  ],
  "assertions_min": {
    "src-tauri/src/bot/reasoning.rs": 25,
    "src-tauri/tests/llm_integration.rs": 30
  },
  "ocr_plan": {
    "rounds": 0,
    "timeout_seconds": 0,
    "expected_max_comments": 0
  },
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
BATCH_SPEC=docs/batches/RE1-MP02-LAND.spec.md python3 scripts/batch-verify.py docs/batches/RE1-MP02-LAND.spec.md
bash scripts/test-all.sh
```
