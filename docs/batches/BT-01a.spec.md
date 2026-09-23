# Batch Spec: BT-01a (v3)

## 目的

C5-BT-01a 剩余 1 条（bot* 域，error-not-propagated family）：`let _ = std::fs::remove_file(...);` 静默丢弃 Err。修法 = 显式传播（`?` / 显式 `return Err`）。

**预核 5 条，实入 1 条**（1a/1b + #2 退 → error-visible-non-blocking family；#4/#5 退 → 非本 family）。

## 人类可读摘要

- family: error-not-propagated
- 覆盖 findings: 1（预核 5）
- 预估 diff: 1 file / +4/-1 lines（A 类行内小改）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 5

## 红线

- family 一致性：只含 error-not-propagated（1 条）
- `Err(NotFound) => {}` 是**承认语义（目标已达成）**，非吞错 —— spec 明写，预防 OCR r1 挑 family
- 不修 keyring / commands.rs（已退批，见 §不在本批）

## Stop 条件

- compile_failure / architecture_blocker / family_heterogeneity / new_high_different_root / gate_fail

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "BT-01a",
  "family": "error-not-propagated",
  "expected_files": [
    "src-tauri/src/bot_slash.rs"
  ],
  "max_lines_added": 7,
  "max_lines_removed": 1,
  "findings": [
    {
      "id": "C5-BT-01a-3",
      "file": "src-tauri/src/bot_slash.rs",
      "line": 462,
      "fix": "所在函数 pub fn bot_set_enabled(app: AppHandle, enabled: bool) -> CommandResult<bool>（返回 Result，? 允许）。call site :462 let _ = std::fs::remove_file(bot_flag_path(&app));。修法：match std::fs::remove_file(bot_flag_path(&app)) { Ok(()) => {} Err(e) if e.kind() == std::io::ErrorKind::NotFound => {} Err(e) => return Err(e.into()), }。【承认语义】Err(NotFound) => {} 是承认语义（要删的 flag 本就不在 = 目标已达成），不是吞错 / error-visible-non-blocking —— 预防 OCR r1 挑 family。【From 核】CommandError: From<std::io::Error>（error.rs:371 → IoError(e.to_string())），与同函数上方 std::fs::write(...)? 隐式转换同 code（IoError），一致。"
    }
  ],
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

## 签名核（执行前已验）

- `CommandError` 有 `From<std::io::Error>`（error.rs:371 → `IoError(e.to_string())`）
- #3 所在 fn：`-> CommandResult<bool>`（bot_slash.rs:456）
- 被删对象 `bot_flag_path(&app)`（bot_slash.rs:439）：flag 文件不存在属正常（禁用时可能从未创建）

## 调用点影响清单

| # | file:line | 现状 | 目标 | ? 允许 |
|---|---|---|---|---|
| 3 | bot_slash.rs:462 | `let _ = std::fs::remove_file(bot_flag_path(&app));` | `match std::fs::remove_file(...) { Ok/NotFound => {}, Err(e) => return Err(e.into()) }` | ✓ |

## Budget 逐点算（SOP §5：A 类行内小改 +1/-1 per 改动点）

| # | 改动点 | 类 | + | - |
|---|---|---|---|---|
| 3 | bot_slash.rs:462 match 展开（5 行 / 旧 1 行） | A | 4 | 1 |
| **合计** | | | **4** | **1** |

**budget: max_lines_added: 7 / max_lines_removed: 1**（A 类实测预期 ≈ +4/-1；执行时按 numstat 校正一次）

## 提交信息骨架

```
fix(bot): BT-01a — bot_slash flag 删除错误丢弃收口（error-not-propagated family）

【family】error-not-propagated（C5-BT-01a，预核 5 条实入 1 条）
【实修】（1 文件）
- bot_slash.rs:462 — bot_set_enabled flag 删除：let _ 丢弃 → match + NotFound 承认语义 + 真错误传播
  NotFound => {} = 承认语义（要删的 flag 本就不在 = 目标达成），非吞错。
  From<io::Error> 同 code IoError，与上方 std::fs::write(...)? 一致。
【预核 vs 实入】5 → 1（1a/1b commands.rs + #2 keyring.rs 退 → error-visible-non-blocking family，
  登记 C5-BT-01a-1 / C5-BT-01a-2；#4 skills/scheduler.rs:431 skill_finish 返 String 非 Result；
  #5 bot_scheduler.rs:265 fn 返 Vec + unwrap_or_default 属 failure-recovery-default-value）。
【自测】cargo fmt / cargo check --tests / scripts/test-all.sh 全绿
【OCR】r1
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/BT-01a.spec.md python3 scripts/batch-verify.py docs/batches/BT-01a.spec.md
```

## 预核 vs 实入（5 预核，1 实入）

| # | doc ref | 实核 file:line | 判 | 理由 |
|---|---|---|---|---|
| 1a/1b | commands.rs:30 | commands.rs:30 + :31 | **退** | (b) log-and-continue → family error-visible-non-blocking → 登记 **C5-BT-01a-1** |
| 2 | keyring.rs:263 | keyring.rs:272-276 | **退** | 修法 (d)（原顺序 + 双 Error 分流）→ family error-visible-non-blocking → 登记 **C5-BT-01a-2**；(c) 逆向半成功陷阱已否 |
| 3 | slash.rs:456 | bot_slash.rs:462 | **入** | fn -> CommandResult<bool>，`?` 允许 + NotFound 承认语义 |
| 4 | skills/scheduler.rs:428 | scheduler.rs:431 | **退** | `skill_finish -> String`（非 Result）→ `let _` 丢 String 非错误 |
| 5 | scheduler.rs:0（无效） | bot_scheduler.rs:263/:265 | **退** | fn `find_due_tasks -> Vec`（非 Result）+ `unwrap_or_default()` 属 failure-recovery-default-value |

## 不在本批

- **C5-BT-01a-1**（commands.rs:30/:31）：修法 (b) log warn + 继续 → family = error-visible-non-blocking（同 C5-AP-06）。已登记 PHASE2-TRIAGE §3.5。
- **C5-BT-01a-2**（keyring.rs:272-276）：修法 (d) 原顺序 + 双 Error 分流 → family = error-visible-non-blocking。已登记 PHASE2-TRIAGE §3.5。
- #4（skills/scheduler.rs:431）：`skill_finish -> String` 非错误丢弃（退 triage）
- #5（bot_scheduler.rs:265）：归 failure-recovery-default-value（BT-01b 同类），fn 返 Vec 需先改签名
- **C5-EV-3b-A**：退 triage 重做（不在本轮）
- **§2 簇清单 version / last-checked 标注**：下次 docs commit 带
