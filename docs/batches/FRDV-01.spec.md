# Batch Spec: FRDV-01

## 目的

C5-MI-03 的 rules.rs:121 条：CSV 导入两处静默默认值收紧为 fail-closed——空 `动作` 单元格不再
静默 coerce 为 "move"；`启用` 列不再把白名单外任意 token 静默 coerce 为 false。

## 人类可读摘要

- family: failure-recovery-default-value（错误被替换成默认值 → 用户以为写了 A 实际跑了 B）
- 覆盖 findings: 1（C5-MI-03.2 = rules.rs:121；C5-MI-03.1 = rules.rs:27 quarantine 方向为 B 类候选，不在本批）
- 预估 diff: 2 files / +60/-5 lines（B 类：match arm 整块替换 + 新测试块）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 3

## 红线

- family 一致性：本批只含 failure-recovery-default-value，不混入其他 family
- 0 FP / 0 family 异质
- 编译层事实必须核实，不许凭印象
- **不改 `parse_rules_csv` 签名**（已返 `Result<RulesFile, CommandError>`，fail-closed 走既有 Err 通道，
  caller commands.rs:75 已 `?` 传播，无 ripple）
- 动作别名白名单（移动/移动归档/move/Move/删除/删除文件/delete/Delete）不动——OCR 附注的
  "移动" 单列别名 footgun 属行为收窄，超出 finding 主诉求，留 follow-up

## spec 起草后自查三条（APW-02a 2026-09-23 立）

1. `expected_files` 是否覆盖全部写入路径（含签名 ripple 的 caller + 新测试文件）
   → rules.rs（修复）+ migration/mod.rs（新测试，:764 起测试模块）。无签名变更，无 caller ripple。✓
2. budget 是 A 类还是 B 类？两类公式分开算？
   → B 类（match arm 整块替换 + 新测试函数块）：rules.rs 删 2 行旧 arm / 加 ~19 行新 arm；
   mod.rs 加 ~30 行（3 个新测试）。合计 +49/-2 → budget +60/-5（余量 ~20%）。✓
3. findings 逐条 fix 字段是否显式列出 ripple 的文件 + 行号？→ 见 JSON findings[0].fix。✓

自查未过不许发审。补完再审。

## 自主执行规则

spec 被 reviewer 批准后:
- agent 全权执行至 commit,不中途报 status / exit / diff
- 例外只有 stop 条件触发
- 完成时一次报: hash + 批定性 + OCR 轮次 + comments 处置 + follow-up
- 过程证据落 ~/.openclaw/cache/<batch-id>/,不喊人

## Stop 条件(触发即停,报 reviewer)

- compile_failure (无 X 形态退路)
- architecture_blocker (签名/调用点超预算)
- family_heterogeneity
- new_high_different_root (OCR r1 出的 high 非本批根因)
- gate_fail (batch-verify FAIL 且非 spec 声明调整可解)

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "FRDV-01",
  "family": "failure-recovery-default-value",
  "expected_files": [
    "src-tauri/src/migration/rules.rs",
    "src-tauri/src/migration/mod.rs"
  ],
  "max_lines_added": 60,
  "max_lines_removed": 5,
  "findings": [
    {"id": "C5-MI-03.2", "file": "src-tauri/src/migration/rules.rs", "line": 121, "fix": "动作 match 删 `other if other.is_empty() => \"move\"` 静默 arm 改行级 Err（含行号）；启用 match `_ => false` 改白名单（否/false/False/FALSE/0 => false）+ 未知/空 token 行级 Err。无签名 ripple（commands.rs:75 已 ? 传播）；新测试 3 个落 src-tauri/src/migration/mod.rs:764 测试模块（rejects_empty_action / rejects_unknown_enabled / accepts_explicit_disabled_variants）"}
  ],
  "assertions_min": {
    "src-tauri/src/migration/mod.rs": 116
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

## 提交信息骨架

```
fix(migration): FRDV-01 — CSV 导入静默默认值收紧 fail-closed（C5-MI-03.2）

【family】failure-recovery-default-value（错误被替换成默认值 → 用户以为写了 A 实际跑了 B）
【实修 1 条 / 2 处】
- rules.rs:124：空动作 `other if other.is_empty() => "move"` 静默 arm → 行级 DomainRule Err
  （"第 N 行动作为空（应填 移动归档 或 删除文件）"）
- rules.rs:136-139：启用 `_ => false` → 白名单 否/false/False/FALSE/0 => false +
  未知/空 token 行级 DomainRule Err（含行号与无效值原文）
【D2】行为断言：空动作行 / 未知启用 token → Err 含行号；显式 否/false/0 → enabled=false。
前置断言：官方模版 + 别名变体解析结果不变（既有 8 测试全绿）。
反例断言：若空动作仍静默 coerce 为 move，则 parse_rules_csv_rejects_empty_action 挂。
【自测】cargo fmt / cargo check --tests / scripts/test-all.sh 全绿
【OCR】r1
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/FRDV-01.spec.md python3 scripts/batch-verify.py docs/batches/FRDV-01.spec.md
```

## 不在本批

- C5-MI-03.1（rules.rs:27 load_rules 解析失败静默 fallback default → 用户规则可能被空 ruleset
  覆写）：修法方向 quarantine vs 返回结构化 Err = B 类决策，攒批报人拍
- 同 family 其他 B 类候选：workspace.rs:241（import updated_at 缺省 policy）、
  paths.rs:50（copy_legacy_db open 失败 fail-closed 方向）、bot_history.rs:52（批量同 now 时间戳，
  family 归属存疑）——均攒批报人拍
- 动作别名收窄（"移动"/"删除" 单词别名）：OCR 附注，留 follow-up
