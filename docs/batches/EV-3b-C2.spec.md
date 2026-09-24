# Batch Spec: EV-3b-C2

## 目的

修 C5-EV-3b-C2 2 条（双侧逻辑不一致 / 契约名实不符）：

1. **ttl.rs:23**（high）：mark_expired 只转 Pooled→Expired（软淘汰保历史），
   evict_expired 却删**任何**过期条目不论 status——过期 Promoted/Rejected
   被硬删丢审计轨迹，两 helper 不可安全组合。
2. **change/derive.rs:34**（high）：`hard_constraint_compliance` 只检约束
   5/6 却名/doc 声称「满足所有 9 条」——调用方望名生义会跳过下游复核。
   取 finding 轻量选项：**重命名为 passes_auto_apply_gate** + doc 写明
   部分检查语义（扩大检查到 9 条 = 语义变更方向，不做）。同根 medium
   （:35 开放式否定 `!matches!(impact, Low)` 对未来变体静默放行）一并改
   显式白名单 `High | Medium`。

## 人类可读摘要

- family: contract-consistency（双侧不一致/名实不符）
- 覆盖 findings: 2（+1 同根 medium 一并）
- 预估 diff: 3 files / +30/-12 lines（A 类，budget +42/-18）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 3

## 核实的编译层事实（spec 前已核）

1. evict_expired **零生产调用方**（grep 全仓：仅 ttl.rs 自身测试 +
   candidate/mod.rs re-export）——改谓词无 ripple。
2. hard_constraint_compliance fn 零外部调用方（仅 derive.rs from_proposal
   内部 + 自身测试 5 处 + change/mod.rs re-export）——重命名 ripple =
   change/mod.rs 一行 + 测试改名。**ChangeRecord 的持久化字段
   hard_constraint_compliance 不动**（jsonl schema）。
3. ImpactLevel 恰为 Low/Medium/High 三变体——白名单与现谓词今日等价，
   未来新增变体编译期可见（这就是目的）。
4. apply.rs auto_apply_gate（apply.rs:26）与本谓词同语义（MemoryHint +
   非 Low）——双侧一致已核。
5. 既有 asserts：ttl 17 / derive 26。

## 修法

- **ttl.rs**：evict_expired 谓词改「过期 **且** status ∈ {Pooled, Expired}
   才删」（Promoted/Rejected 永不硬删）；文件头 + fn doc 同步修订
  （软淘汰语义统一）；新增测试 1（过期 Promoted/Rejected 存活）。
- **derive.rs**：fn 重命名 hard_constraint_compliance →
  passes_auto_apply_gate + doc 写明「仅约束 5/6（自动应用门槛），其余
  由 apply 入口校验」；谓词改显式白名单；文件头字段 8 注释同步；
  测试 5 处改名。
- **change/mod.rs**：re-export 名同步。

## 红线

- ChangeRecord 字段名 / jsonl schema 不动
- 不扩大合规检查范围（9 条全覆盖 = 语义方向，B 类不自决）
- derive.rs 其它 low（clone 性能 / derive_layer 覆盖）不动

## spec 起草后自查三条

1. `expected_files` = 3：candidate/ttl.rs + change/derive.rs +
   change/mod.rs。ripple = change/mod.rs re-export 一行（已列）。
2. budget = **A 类**：ttl +15/-4、derive +13/-7、mod +1/-1：估 +29/-12，
   预留 → **max +42/-18**。
3. 两条 findings 的 fix 字段均已写明 ripple。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "EV-3b-C2",
  "family": "contract-consistency",
  "expected_files": [
    "src-tauri/src/evolution/candidate/ttl.rs",
    "src-tauri/src/evolution/change/derive.rs",
    "src-tauri/src/evolution/change/mod.rs"
  ],
  "max_lines_added": 42,
  "max_lines_removed": 18,
  "findings": [
    {"id": "C5-EV-3b-C2-1", "file": "src-tauri/src/evolution/candidate/ttl.rs", "line": 23, "fix": "evict_expired 谓词加 status 门（仅 Pooled/Expired 可硬删，Promoted/Rejected 保历史）+ doc 修订 + 回归测试；零生产调用方无 ripple"},
    {"id": "C5-EV-3b-C2-2", "file": "src-tauri/src/evolution/change/derive.rs", "line": 34, "fix": "fn 重命名 passes_auto_apply_gate + doc 写明仅检约束 5/6 + impact 谓词改显式白名单 High|Medium；ripple: change/mod.rs re-export 一行 + 测试改名；ChangeRecord 字段/jsonl schema 不动"}
  ],
  "assertions_min": {
    "src-tauri/src/evolution/candidate/ttl.rs": 17,
    "src-tauri/src/evolution/change/derive.rs": 26
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
