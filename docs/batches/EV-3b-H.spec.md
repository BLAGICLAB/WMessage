# Batch Spec: EV-3b-H

## 目的

修 C5-EV-3b-H 3 条（trace 双轨 / 静默丢），同文件（trace.rs）+
1 行 ripple（bot_chat.rs）：

1. **trace.rs:104**（high）：「aborted」双轨——`TraceOutcome::Aborted`
   变体 + 独立 `was_aborted: bool`，两个独立 setter 可矛盾
   （Success+was_aborted=true / Aborted+was_aborted=false），非良构
   状态机 API。修：**枚举作唯一事实源**——删 `was_aborted` 字段 +
   `with_aborted` setter，`Aborted` 语义由 outcome 承载。
2. **trace.rs:149**（high，同根）：采样规则静默丢用户 /stop——
   `Aborted` 只在调用方**同时**翻 was_aborted 旗标才记录；唯一生产
   调用方（bot_chat.rs:925）两轨同源翻牌（latent 非 active bug），
   但 API 形态允许丢失。随 ① 坍塌后：`should_record_trace` 删
   was_aborted 入参，`Aborted` 与 `Failure` 同待遇恒记录。
   **行为变更：用户中止的 trace 从「可能静默丢」变为恒记录**
   （audit 日志增量，非破坏）。
3. **trace.rs:193**（high）：emit 时 `tool_calls: Vec::new()` 硬编码——
   schema 广告宣传 name/success/duration_ms/error_kind 字段，实际
   恒空。修：TraceContext 加 `tool_calls: Vec<ToolCallSummary>` 字段 +
   `with_tool_calls` setter + forward 进 ExecutionTrace（audit payload
   的 `tool_calls` 计数 = vec.len() 变为真实值）。**producer 接线
   （run_model_loop 返回 per-tool 明细）= bot_model_loop 返回类型
   变更 = 跨域 ripple，不做，登记 follow-up**——本批通管道。

**同 family（双轨）同文件 medium :214 一并**：audit emit `outcome`
用 `format!("{:?}")`（Rust Debug 文本）而既有测试锁 serde
snake_case JSON 形态——同一字段两种序列化。修：emit 改
`serde_json::to_string(&trace.outcome)` 与锁定契约对齐。

## 人类可读摘要

- family: dual-track / silent-drop
- 覆盖 findings: 3（+1 同 family medium 一并）
- 预估 diff: 2 files / +30/-27 lines（A 类，budget +45/-35）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 3

## 核实的编译层事实（spec 前已核）

1. TraceContext/with_aborted 唯一生产 caller = bot_chat.rs:925-937，
   两轨同源（`.with_outcome(Aborted if aborted).with_aborted(aborted)`）
   ——latent API 陷阱，非 active bug。
2. `should_record_trace` 零外部 caller（仅 trace.rs 内 + 自身测试）——
   删入参无 ripple。
3. 既有测试 :328 锁的正是缺陷行为（`!should_record_trace(&Aborted,
   0, 100, false)`）——改断言并在 message/triage 写明（C1/C3 先例）。
   :371-372 坍塌后与 :328 冗余，合并。
4. ToolCallSummary 已 Serialize+Clone；ExecutionTrace.tool_calls 字段
   已存在于 schema（只是恒空）——forward 不改 jsonl schema。
5. serde_json 在 trace.rs 测试已用（:266）；lib 可用。
6. 既有 asserts：trace 25 / bot_chat 70。

## 修法

- **trace.rs**：删 was_aborted 字段/setter；should_record_trace 删第 4
  入参、规则 1 改 `Failure | Aborted`；TraceContext 加 tool_calls 字段
  + setter；maybe_record_trace forward vec + emit `aborted` 键改
  `matches!(trace.outcome, Aborted)` 派生 + outcome 键改 serde_json
  规范形；doc 同步（双轨不变量注释、tool_calls 管道注明 Phase 2 接线）。
  测试：:328 翻正（Aborted 恒记录）、:371-372 冗余合并、新增
  tool_calls forward 断言 + outcome serde 形态 emit 断言。
- **bot_chat.rs**：删 `.with_aborted(aborted)` 一行（唯一 ripple）。

## 红线

- 不接 producer（run_model_loop 返回类型 = 跨域，登记 follow-up）
- 不动 compute_trace_id 的 DefaultHasher（trace.rs:74 medium 不属本簇）
- 不动 :82/:418 lows（不属本簇行号）
- TraceOutcome 枚举变体不动（serde 形态是锁定契约）

## spec 起草后自查三条

1. `expected_files` = 2：trace.rs + bot_chat.rs。ripple = bot_chat
   删一行（已列）。
2. budget = **A 类**：trace +29/-26、bot_chat +0/-1：初估 +29/-27 →
   含浮动 **max +45/-35**。
3. 三条 findings 的 fix 字段均已写明 ripple。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "EV-3b-H",
  "family": "dual-track-silent-drop",
  "expected_files": [
    "src-tauri/src/evolution/trace.rs",
    "src-tauri/src/bot_chat.rs"
  ],
  "max_lines_added": 45,
  "max_lines_removed": 35,
  "findings": [
    {"id": "C5-EV-3b-H-1", "file": "src-tauri/src/evolution/trace.rs", "line": 104, "fix": "aborted 双轨坍塌为枚举唯一事实源：删 was_aborted 字段+with_aborted setter；ripple: bot_chat.rs 删 .with_aborted 一行（唯一 caller）"},
    {"id": "C5-EV-3b-H-2", "file": "src-tauri/src/evolution/trace.rs", "line": 149, "fix": "should_record_trace 删 was_aborted 入参，Aborted 与 Failure 同待遇恒记录（用户中止 trace 不再静默丢）；零外部 caller 无 ripple；锁旧缺陷行为的测试改断言"},
    {"id": "C5-EV-3b-H-3", "file": "src-tauri/src/evolution/trace.rs", "line": 193, "fix": "TraceContext 加 tool_calls: Vec<ToolCallSummary> + setter + forward 进 ExecutionTrace（audit payload 计数变真实值）；producer 接线（run_model_loop 返回类型）=跨域登记 follow-up 不做"}
  ],
  "assertions_min": {
    "src-tauri/src/evolution/trace.rs": 25,
    "src-tauri/src/bot_chat.rs": 70
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 3
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
