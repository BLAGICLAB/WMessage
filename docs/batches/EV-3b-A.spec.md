# Batch Spec: EV-3b-A

## 目的

修 C5-EV-3b-A 中 3 条 silent-error-swallow（吞值→可见化，纯 warn 留痕，
零行为变化）：

1. **apply.rs:151**（high）：embedding 批量失败静默塌成 None 转发
   insert_item——lesson 无向量永不可语义召回，运维零信号。
2. **activation.rs:136**（high）：load_config_from_file /
   load_state_from_file 把 {IO 错（非 NotFound）/ JSON parse 失败 /
   块存在但 schema 不匹配 / 未知 state 字符串} 全部静默塌成 Default——
   配置损坏时以为在 calibrating 实际跑默认。
3. **mod.rs:55**（medium，triage 行号 :48 漂移）：`emit::app_handle()`
   返回 None 时整个候选池落盘块（含成功/失败审计）静默跳过——
   register_app_handle 未初始化/早调完全不可见。

## 簇拆分说明（family 内再分）

record.rs:211 + entry.rs:110（read_all 单行损坏全读失败）**移出本批**：
修法方向是 fail-closed（现状 Err 传播）vs fail-open（skip-and-warn）的
语义方向选择 → **B 类攒批第 17 项**，本批不动。

## 人类可读摘要

- family: silent-error-swallow（吞值→可见化）
- 覆盖 findings: 3（原簇 5 条 - 2 条转 B 类）
- 预估 diff: 3 files / +67/-12 lines 实测（A 类，budget 执行中校正 +45/-15 → **+75/-20**；
  activation 双 loader 每错误分支独立 eprintln + let-else→match 重排超估）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 4

## 核实的编译层事实（spec 前已核）

1. embed_text 签名 `pub fn embed_text(text: &str) -> Option<Vec<f32>>`
   （memory/embed.rs:156）——None = 空文本或推理失败，调用侧无法区分，
   留痕只能报数量不报错因（注明）。
2. apply.rs 嵌入已在持锁前批量算好（历史批已移出临界区），:151 处是
   collect 点；warn 加在 collect 后、持锁前。
3. activation.rs 两个 loader 均 lenient 设计（缺块=合法默认模式）——
   **只有「存在但坏」与「IO 错≠NotFound」才留痕**，缺块不 warn（否则
   首装/未配置场景刷噪声）。
4. mod.rs:55 `if let Some(app) = emit::app_handle()` 无 else 分支；
   emit_proposals / apply_from_consolidation 不依赖该块结果。
5. 既有 asserts：apply.rs 25 / activation.rs 42 / mod.rs 0（无测试模块，
   本批不加——纯 eprintln 留痕无可断言行为变化）。

## 修法

- **apply.rs**：embs collect 后统计 None 数，>0 时 eprintln
  `[evolution_apply] embedding 失败 {n}/{total}（lesson 将无向量不可召回）`。
- **activation.rs**：两个 loader 的三类「存在但坏」路径各加 eprintln
  `[evolution_activation]`（IO 错≠NotFound / JSON parse 失败 / 块或
  state 值存在但无效），缺块/缺 key 保持静默 default。
- **mod.rs**：`if let Some(app)` 加 else —— eprintln
  `[evolution] app_handle 未注册，候选池落盘跳过（{n} 条 proposals）`。

## 红线

- 零行为变化（只加 eprintln，默认值/返回值/控制流不变）
- 不动 record.rs:211 / entry.rs:110（B 类第 17 项）
- 不动 apply.rs:156 锁跨度（3b-G 簇）与 poison 条

## spec 起草后自查三条

1. `expected_files` = 3：apply.rs / activation.rs / mod.rs（全
   src-tauri/src/evolution/ 下）。无 ripple（无签名变更）。
2. budget = **A 类**：初估 +30/-8 → 实测 +67/-12（双 loader 每错误分支
   独立文案 + let-else→match 重排），校正 → **max +75/-20**。
3. 三条 findings 的 fix 字段均已写明无 ripple；转 B 类的 2 条不进
   机器可读 findings 数组（零代码处置不进 gate）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "EV-3b-A",
  "family": "silent-error-swallow",
  "expected_files": [
    "src-tauri/src/evolution/apply.rs",
    "src-tauri/src/evolution/activation.rs",
    "src-tauri/src/evolution/mod.rs"
  ],
  "max_lines_added": 75,
  "max_lines_removed": 20,
  "findings": [
    {"id": "C5-EV-3b-A-1", "file": "src-tauri/src/evolution/apply.rs", "line": 151, "fix": "embs collect 后统计 None 数，>0 时 eprintln [evolution_apply] 留痕（数量级，embed_text 不报错因）；行为不变，无 ripple"},
    {"id": "C5-EV-3b-A-2", "file": "src-tauri/src/evolution/activation.rs", "line": 136, "fix": "两个 loader 的「存在但坏」三类路径（IO≠NotFound / parse 失败 / 值无效）加 eprintln [evolution_activation]，缺块缺 key 保持静默 default；行为不变，无 ripple"},
    {"id": "C5-EV-3b-A-3", "file": "src-tauri/src/evolution/mod.rs", "line": 55, "fix": "app_handle None 加 else eprintln 留痕（含 proposals 条数）；行为不变，无 ripple"}
  ],
  "assertions_min": {
    "src-tauri/src/evolution/apply.rs": 25,
    "src-tauri/src/evolution/activation.rs": 42
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 4
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
