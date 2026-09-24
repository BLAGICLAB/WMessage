# Batch Spec: EV-3b-C3

## 目的

修 C5-EV-3b-C3 2 条（设计约定未强制）：

1. **routing.rs:21**（high）：canary 与 A/B 复用同一 bucket——canary 群
   60% A / 40% B，联合分布破坏，效应无法归因单变量（spec R3 把两条写成
   并列维度，暗示正交）。修法：A/B 改独立 hash（加盐 fnv1a），canary 桶
   不变（成员稳定）。同位置 low（bucket pub 暴露实现细节）一并：
   bucket 降 pub(crate) + 移出 re-export（零外部调用方已核）。
2. **kill_switch.rs:15**（high maintainability）：三布尔允许矛盾组合
   （all_auto_apply=true + shadow_only=false），契约只靠
   should_shadow_only 的 `||` 兜底。修法：load_from_file 边界归一化
   （all_auto_apply=true 时强制 shadow_only=true + eprintln 留痕）。
   注：finding 所述「should_auto_apply 对该组合返 true」已被 EV-3b-C1
   修复（谓词改 !all && !shadow_only），本批补边界归一化。

## 人类可读摘要

- family: convention-enforcement（约定 → 强制/正交化）
- 覆盖 findings: 2（+1 同位置 low 一并）
- 预估 diff: 3 files / +35/-8 lines（A 类，budget +50/-15）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 3

## 核实的编译层事实（spec 前已核）

1. bucket 零外部调用方（grep 全仓：仅 routing.rs 测试 + sandbox/mod.rs
   re-export）；pub(crate) 化 + 移出 re-export 无 ripple。
2. is_ab_a 零生产调用方（grep 仅 re-export + 测试）——A/B 换独立 hash
   不改任何现役行为（无消费者在读），是契约修正。
3. is_canary 调用方：shadow.rs:355（测试）；canary 桶不动 → 成员稳定。
4. kill_switch load_from_file 是唯一反序列化边界；直接构造 struct
   （pub 字段）仍可造矛盾组合——类型级禁绝 = 私有字段/枚举重构，
   YAGNI 不做，边界归一化 + doc 足够（finding 针对「config writers
   persist 矛盾组合」）。
5. 既有 asserts：routing 11 / kill_switch 20（C1 后）。
6. A/B 独立 hash 用加盐 `fnv1a("ab:" + session_id)`（复用既有 fnv1a，
   跨进程稳定契约保持）。

## 修法

- **routing.rs**：新增 `fn ab_bucket(session_id)`（加盐 fnv1a % 100），
  is_ab_a 改用它；bucket 降 pub(crate) + doc 注明「实现细节勿依赖数值」；
  新增测试 2（canary∩A 比例在 [0.3,0.7]——确定性 hash 非统计 flaky；
  ab_bucket 与 bucket 对同输入不同值分布独立）。
- **sandbox/mod.rs**：re-export 移除 bucket。
- **kill_switch.rs**：load_from_file 反序列化后归一化（all_auto_apply
  → shadow_only=true + eprintln `[evolution_kill_switch]`）；新增测试 1
  （矛盾组合被归一化）。

## 红线

- canary 桶/hash 不动（成员稳定是契约）
- 不做类型级禁绝（私有字段重构 YAGNI）
- fnv1a 本体不动（已知值测试锁死）

## spec 起草后自查三条

1. `expected_files` = 3：sandbox/routing.rs + sandbox/kill_switch.rs +
   sandbox/mod.rs。ripple = mod.rs re-export 一行（已列）。
2. budget = **A 类**：routing +20/-5、kill_switch +13/-2、mod +1/-1：
   估 +34/-8，预留 → **max +50/-15**。
3. 两条 findings 的 fix 字段均已写明 ripple。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "EV-3b-C3",
  "family": "convention-enforcement",
  "expected_files": [
    "src-tauri/src/evolution/sandbox/routing.rs",
    "src-tauri/src/evolution/sandbox/kill_switch.rs",
    "src-tauri/src/evolution/sandbox/mod.rs"
  ],
  "max_lines_added": 50,
  "max_lines_removed": 15,
  "findings": [
    {"id": "C5-EV-3b-C3-1", "file": "src-tauri/src/evolution/sandbox/routing.rs", "line": 21, "fix": "A/B 改独立加盐 hash（ab_bucket），与 canary 桶正交；bucket 降 pub(crate) 移出 re-export；canary 不动；ripple: sandbox/mod.rs re-export 一行"},
    {"id": "C5-EV-3b-C3-2", "file": "src-tauri/src/evolution/sandbox/kill_switch.rs", "line": 15, "fix": "load_from_file 边界归一化矛盾组合（all_auto_apply→强制 shadow_only + eprintln 留痕）+ 回归测试；类型级禁绝 YAGNI 不做；无 ripple"}
  ],
  "assertions_min": {
    "src-tauri/src/evolution/sandbox/routing.rs": 11,
    "src-tauri/src/evolution/sandbox/kill_switch.rs": 20
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
