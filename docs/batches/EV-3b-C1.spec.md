# Batch Spec: EV-3b-C1

## 目的

修 C5-EV-3b-C1 3 条（状态转移/条件判定错）：

1. **mapping.rs:37**（high）：to_change_record 合规时 approval_source 无条件
   Pending，entry.status 为 Expired/Rejected 时产出内部不一致记录
   （status=Rejected + approval_source=Pending = 「被拒却待批准」）。
   ApprovalSource::Pending 的文档自述「还没批准（Pending 状态时）」。
2. **status.rs:36**（high）：can_transition 无条件放行 Approved→Active 却
   注释「skip-canary 配置」——本模块无配置入参，硬约束强制责任在调用方，
   文档未写明 → 取 finding 的轻量选项：**doc 收紧为显式调用方契约**
   （加配置入参 = 签名改 = B 类，不做）。
3. **kill_switch.rs:39**（high）：should_auto_apply = !all_auto_apply，
   shadow_only=true 时仍返 true（自有测试注释已写明「实际语义：
   shadow_only=true → should_auto_apply=false」但回避了断言）——
   谓词改为 `!all_auto_apply && !shadow_only`。

## 人类可读摘要

- family: logic-condition（条件判定/派生一致性）
- 覆盖 findings: 3
- 预估 diff: 3 files / +40/-8 lines（A 类，budget +55/-15）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 4

## 核实的编译层事实（spec 前已核）

1. **kill_switch 两个谓词零生产调用方**（grep 全仓：仅 kill_switch.rs 自身
   + 测试；apply.rs 的 auto_apply_gate 不查 kill switch）——改谓词语义
   无生产 ripple。finding 所述「apply.rs 只看 should_auto_apply」当前
   不成立（无调用），是潜在 API 陷阱而非现役 bug。
2. to_change_record 生产调用点仅 1：panel/commands.rs:119（toggle ON，
   entry 通常 Pooled→Pending，本次修复对主路径零变化；Expired/Rejected
   边缘才变）。
3. ApprovalSource 无 Expired 对应变体——Expired 保持 Pending（从未批准
   也未被拒，无更合适变体），spec/代码注释声明。
4. status.rs 的 Active→Rejected 缺失（同 OCR low :43-45）确认**有意**
   （Active 的退出只有 RolledBack/Expired）——本批只在 doc 中一并写明，
   不加转移。
5. 既有 asserts：mapping 23 / status 34 / kill_switch 19。

## 修法

- **mapping.rs**：`approval_source = if !compliant { SystemRejected } else
  { match status { Rejected => SystemRejected, _ => Pending } }`（Expired
  保持 Pending + 注释）；新增测试 1（rejected entry → 两个字段一致）。
- **status.rs**：Approved→Active 行 doc 收紧——can_transition 是纯拓扑表，
  skip-canary 策略门（kill switch / 配置）由调用方负责；Active 无
  Rejected 出口为有意设计一并写明。零代码行为变化。
- **kill_switch.rs**：should_auto_apply 改 `!self.all_auto_apply &&
  !self.shadow_only` + doc 更新；测试 shadow_only_implies_no_real_apply
  补断言 `!k.should_auto_apply()`（把注释里的意图锁成断言）。

## 红线

- status.rs 不加配置入参（签名改 = B 类）；不加 Active→Rejected 转移
- kill_switch 不改 should_shadow_only / 不加 ApplyMode 枚举（YAGNI，
  零调用方）
- mapping.rs Expired 不变体新增（枚举扩展 = 协议面，不自决）

## spec 起草后自查三条

1. `expected_files` = 3：candidate/mapping.rs + change/status.rs +
   sandbox/kill_switch.rs。无 ripple（零/单调用点已核）。
2. budget = **A 类**：mapping +26/-3（含测试）、status +7/-2、
   kill_switch +7/-3：估 +40/-8，预留 → **max +55/-15**。
3. 三条 findings 的 fix 字段均已写明无 ripple。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "EV-3b-C1",
  "family": "logic-condition",
  "expected_files": [
    "src-tauri/src/evolution/candidate/mapping.rs",
    "src-tauri/src/evolution/change/status.rs",
    "src-tauri/src/evolution/sandbox/kill_switch.rs"
  ],
  "max_lines_added": 55,
  "max_lines_removed": 15,
  "findings": [
    {"id": "C5-EV-3b-C1-1", "file": "src-tauri/src/evolution/candidate/mapping.rs", "line": 37, "fix": "approval_source 由 resolved status 派生（Rejected→SystemRejected，余→Pending；Expired 保持 Pending 注释声明）+ 一致性回归测试；无 ripple"},
    {"id": "C5-EV-3b-C1-2", "file": "src-tauri/src/evolution/change/status.rs", "line": 36, "fix": "doc 收紧：can_transition=纯拓扑表，skip-canary 策略门属调用方契约；Active 无 Rejected 出口写明有意；零代码变化；无 ripple"},
    {"id": "C5-EV-3b-C1-3", "file": "src-tauri/src/evolution/sandbox/kill_switch.rs", "line": 39, "fix": "should_auto_apply 改 !all_auto_apply && !shadow_only + doc + 测试补断言（原测试注释已述意图未断言）；零生产调用方无 ripple"}
  ],
  "assertions_min": {
    "src-tauri/src/evolution/candidate/mapping.rs": 23,
    "src-tauri/src/evolution/change/status.rs": 34,
    "src-tauri/src/evolution/sandbox/kill_switch.rs": 19
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
