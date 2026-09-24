# Batch Spec: EV-3b-E

## 目的

修 C5-EV-3b-E 3 条（死代码 / no-op），同根同文件（**triage 行号标注
observe/shadow.rs 系笔误，实为 sandbox/shadow.rs**，已按内容定位核实）：

1. **sandbox/shadow.rs:0**（high）：`WithDummy` trait + impl 是纯 no-op
   （返回 self），存在唯一目的是压一个 unused-variable 警告——为压警告
   在生产模块加 trait 是反模式。
2. **sandbox/shadow.rs:87**（high）：`candidate_importance` 计算后只被
   no-op 消费，值丢地上——疑似未完成特性。
3. **sandbox/shadow.rs:163**（high）：删 no-op trait 本体（链式调用移除后
   即成死代码）。

修法取 finding 给的**删除选项**：删变量 + 删链式调用 + 删 trait/impl。
另一选项「接入决策逻辑」（importance 影响 Pass/Fail/note）= **语义变更
方向，B 类不做**——candidate 的 importance 已通过 `hypothetical.push(
candidate.clone())` 参与 top-3 排序，决策路径不缺输入；单独的
`candidate_importance` 局部变量确为纯死代码。

## 人类可读摘要

- family: dead-code（no-op  trait / 死变量）
- 覆盖 findings: 3（同根）
- 预估 diff: 1 file / +2/-15 lines（A 类，budget +10/-20）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 2

## 核实的编译层事实（spec 前已核）

1. `WithDummy` / `with_dummy_for` / `candidate_importance` 全仓 grep
   仅 sandbox/shadow.rs 自身——零外部使用，删除无 ripple。
2. candidate importance **仍参与决策**：candidate.clone() 进
   hypothetical 排序（shadow.rs:98-99）；被删的只是 :88 的冗余局部拷贝。
3. trait 是模块私有（无 pub），删除不影响模块对外表面。
4. 既有 asserts：sandbox/shadow.rs 27（删除不涉及测试）。

## 修法

- 删 :88 `let candidate_importance = ...`（变量）
- 删 :143-144 尾注释 + `.with_dummy_for(candidate_importance)` 链式调用
- 删 :175-183 `WithDummy` trait + impl + 注释

## 红线

- 不把 importance 接进 Pass/Fail 决策（语义方向，B 类）
- 不动同文件其它 finding（:136 catch-all 分桶 / lows）——不属本簇
- ShadowOutcome / ShadowInput 结构不动（持久化/schema 安全）

## spec 起草后自查三条

1. `expected_files` = 1：sandbox/shadow.rs。无 ripple（已核 grep）。
2. budget = **A 类**：删 13 行 + 尾括号行改动：初估 +2/-15 →
   **max +10/-20**。
3. 三条 findings 同根同 fix，fix 字段合并描述。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "EV-3b-E",
  "family": "dead-code",
  "expected_files": [
    "src-tauri/src/evolution/sandbox/shadow.rs"
  ],
  "max_lines_added": 10,
  "max_lines_removed": 20,
  "findings": [
    {"id": "C5-EV-3b-E-1", "file": "src-tauri/src/evolution/sandbox/shadow.rs", "line": 0, "fix": "删 no-op WithDummy trait + impl（存在唯一目的=压 unused 警告）；零外部使用无 ripple"},
    {"id": "C5-EV-3b-E-2", "file": "src-tauri/src/evolution/sandbox/shadow.rs", "line": 87, "fix": "删死变量 candidate_importance（importance 仍经 hypothetical 排序参与决策；接入决策=语义方向不做）"},
    {"id": "C5-EV-3b-E-3", "file": "src-tauri/src/evolution/sandbox/shadow.rs", "line": 163, "fix": "同根：链式调用移除后 trait 成死代码，一并删"}
  ],
  "assertions_min": {
    "src-tauri/src/evolution/sandbox/shadow.rs": 27
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 2
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
