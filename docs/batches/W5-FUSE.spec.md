# Batch Spec: W5-FUSE

## 目的

用户实测反馈（工作流第 5 步被熔断卡住）：①全域熔断上限调成 100 且可配置；
②熔断后恢复路径可发现——节点卡写 ⚠️ 归因备注，工具栏出现「继续执行」。

## 人类可读摘要

- family: workflow-canvas
- 预估 diff: ~5 files / +220/-25 lines
- OCR 计划: r1, timeout 1800s, comments ≤ 5

## 背景（熔断机制现状）

`bot_model_loop.rs`：非子 agent 会话熔断上限 = 硬编码 `MAX_FUNCTION_CALLS_PER_REQUEST = 50`
（子 agent 走预算 max_tool_calls）。熔断后循环优雅返回「⏹ 已熔断…」消息（进会话），
**节点卡无标记**；工作流控制器判该节点失败 → 下游 ⏭ 跳过。恢复 = 单卡 🤖 + 再点
开始执行（断点续跑），但不可发现且 50 上限重跑大概率再熔断。

## 修复设计

1. **全域上限可配 + 默认 100**：
   - BotConfig 新增可选字段 `maxFunctionCalls`（serde skip_serializing_if None，
     老配置零影响）；None = 默认 100（用户拍板值），有值则用之（钳 ≥1）
   - 派生链（core 熔断处）：子 agent 预算 → `deps.fuse_cap_override`（薄壳
     run_model_loop 从 config 读入传入）→ 默认 100
   - `MAX_FUNCTION_CALLS_PER_REQUEST` 50→100，软警阈值随 cap 动态推导（已动态）；
     熔断测试断言同步
2. **熔断归因**：控制器节点任务结果解析——`!ok && result.text.contains("已熔断")` →
   在该节点卡 note 前置 `⚠️ 已熔断：调用工具达上限，可调高「调用工具上限」后单卡 🤖
   重跑或整图继续`（mark_note_prefix 复用 ⏭ 同款 RMW 模式，mark_skipped 更名）
3. **恢复可发现**：工具栏执行按钮——工作流存在已完成节点且未在跑时显示
   **「继续执行」**（同一 action，title 说明已完成 N 个将跳过）；总目标卡进度照旧

## 红线

- family 一致：只动熔断上限/归因/续跑可发现性，不碰调度结构
- 子 agent 预算路径字节级不变（派生链第一优先级）
- 老配置（无 maxFunctionCalls 字段）行为 = 新默认 100，无迁移

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W5-FUSE",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/OCR-FOLLOWUPS-INDEX.md",
    "docs/batches/W5-FUSE.spec.md",
    "src-tauri/src/bot/config/commands.rs",
    "src-tauri/tests/llm_integration.rs",
    "src-tauri/tests/task_chat_exec.rs",
    "src/components/SettingsPage/SettingsPage.tsx",
    "src/components/WorkflowCanvas/WorkflowPage.tsx",
    "src-tauri/src/bot/config/types.rs",
    "src-tauri/src/bot_model_loop.rs",
    "src-tauri/src/workflow_runner.rs"
  ],
  "max_lines_added": 260,
  "max_lines_removed": 40,
  "findings": [
    {"id": "W5-1", "file": "src-tauri/src/bot_model_loop.rs", "line": 1, "fix": "默认上限 100 + Deps.fuse_cap_override + 派生链"},
    {"id": "W5-2", "file": "src-tauri/src/bot/config/types.rs", "line": 1, "fix": "BotConfig.maxFunctionCalls 可选字段"},
    {"id": "W5-3", "file": "src-tauri/src/workflow_runner.rs", "line": 1, "fix": "熔断检测 + ⚠️ 归因备注（mark_skipped 更名 mark_note_prefix）"},
    {"id": "W5-4", "file": "src/components/WorkflowCanvas/WorkflowPage.tsx", "line": 1, "fix": "继续执行按钮态"},
    {"id": "W5-5", "file": "src/components/SettingsPage/SettingsPage.tsx", "line": 1, "fix": "工作流分区加上限输入（镜像 pythonTimeoutSecs 模式）"}
  ],
  "assertions_min": {
    "src-tauri/src/bot_model_loop.rs": 8
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 5
  },
  "stop_conditions": [
    "compile_failure",
    "architecture_blocker",
    "family_heterogeneity",
    "new_high_different_root",
    "gate_fail"
  ]
}
```
