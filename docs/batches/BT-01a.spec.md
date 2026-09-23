# Batch Spec: BT-01a (v2)

## 目的

C5-BT-01a（bot* 域，error-not-propagated family）：`let _ = <Result 返回调用>;` 静默丢弃 Err。修法 = 显式传播（`?`，必要时 `map_err` 补上下文）。

**预核 5 条，实入 2 条**（1a/1b 退 → family 变更；#4/#5 退 → 非本 family）。

## 人类可读摘要

- family: error-not-propagated
- 覆盖 findings: 2（预核 5）
- 预估 diff: 2 files / +8/-4 lines（A 类行内小改）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 5

## 红线

- family 一致性：只含 error-not-propagated
- 2 条统一 `?` 显式传播
- keyring (c) 顺序改动 = **设计决策**（语义变更：遗留清理从「顺带」变「主删除前提」），commit message 显式标「设计决策，非 bug 修复」
- bot_slash `Err(NotFound) => {}` 是**承认语义（目标已达成）**，非吞错 —— spec 明写，预防 OCR r1 挑 family

## Stop 条件

- compile_failure / architecture_blocker / family_heterogeneity / new_high_different_root / gate_fail

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "BT-01a",
  "family": "error-not-propagated",
  "expected_files": [
    "src-tauri/src/bot/config/keyring.rs",
    "src-tauri/src/bot_slash.rs"
  ],
  "max_lines_added": 10,
  "max_lines_removed": 6,
  "findings": [
    {
      "id": "C5-BT-01a-2",
      "file": "src-tauri/src/bot/config/keyring.rs",
      "line": 276,
      "fix": "所在函数 pub(crate) fn delete_api_key_at(backend: KeyBackend, file: &Path, slot: KeySlot) -> CommandResult<()>（返回 Result，? 允许）。修法 (c) 顺序改动：把 v0 遗留条目清理（现 :272-276 if let Ok(old) = key_entry_for_service(LEGACY_KEYRING_SERVICE, slot) { let _ = old.delete_credential(); }）挪到主删除（let r = key_entry(slot)?.delete_credential()...）之前；清理失败改显式传播 map_err(|e| CommandError::KeyringError(format!(\"清除 v0 遗留 API Key 失败：{e}\")))?。【设计决策】遗留清理从「主删除后顺带」改为「主删除的前置条件」——失败则整体 abort，无副作用（主 key 未删），用户看到明确「删除失败」可重试；避免半成功陷阱（主 key 已删却返 Err → 重试 NoEntry）。【依据】原文注释 :273-274「顺带清 v0 遗留条目：否则下次读取会把它当作「可迁移的旧 key」搬回新条目，用户「清除」后 key 复活」。"
    },
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

- `CommandError` 有 `From<std::io::Error>`（error.rs:371）+ `From<String>`（error.rs:392）
- #2 所在 fn：`-> CommandResult<()>`（keyring.rs:263）
- #3 所在 fn：`-> CommandResult<bool>`（bot_slash.rs:456）
- #2 被清对象 `delete_credential()` 返回 `Result<(), keyring::Error>`（map_err 包装为 KeyringError，与同函数主删除同 code）

## 调用点影响清单

| # | file:line | 现状 | 目标 | ? 允许 |
|---|---|---|---|---|
| 2 | bot/config/keyring.rs:272-276 | 主删除先算 `let r = ...` → 遗留清理 `let _ = old.delete_credential();` → `r` | 遗留清理**挪到主删除前** + `map_err(...)?` | ✓ |
| 3 | bot_slash.rs:462 | `let _ = std::fs::remove_file(bot_flag_path(&app));` | `match ... { Ok/NotFound => {}, Err(e) => return Err(e.into()) }` | ✓ |

## Budget 逐点算（SOP §5：A 类行内小改 +1/-1 per 改动点）

| # | 改动点 | 类 | + | - |
|---|---|---|---|---|
| 2 | keyring 顺序改动（清理块上移 + 传播） | A | 4 | 3 |
| 3 | bot_slash:462 match 展开（5 行 / 旧 1 行） | A | 4 | 1 |
| **合计** | | | **8** | **4** |

**budget: max_lines_added: 10 / max_lines_removed: 6**（A 类实测预期 ≈ +8/-4；执行时按 numstat 校正一次）

## 提交信息骨架

```
fix(bot): BT-01a — bot* 域 let _ = 错误丢弃 2 条收口（error-not-propagated family）

【family】error-not-propagated（C5-BT-01a，预核 5 条实入 2 条）
【实修】（2 文件）
- bot/config/keyring.rs:272-276 — delete_api_key_at v0 遗留清理：顺序改动（挪到主删除前）+ map_err + ?
  【设计决策】遗留清理从「主删除后顺带」改为「主删除前提」：失败则整体 abort，无副作用，
  避免半成功陷阱（原 let _ 丢弃 → 主 key 已删却静默）。
- bot_slash.rs:462 — bot_set_enabled flag 删除：match + NotFound 承认语义 + 真错误传播
  （From<io::Error> 同 code IoError，与上方 std::fs::write(...)? 一致）
【预核 vs 实入】5 → 2（1a/1b 退 → error-visible-non-blocking family；#4 skills/scheduler.rs:431
  skill_finish 返 String 非 Result；#5 bot_scheduler.rs:265 fn 返 Vec + unwrap_or_default 属
  failure-recovery-default-value）。退批项见 spec §预核 vs 实入。
【自测】cargo fmt / cargo check --tests / scripts/test-all.sh 全绿
【OCR】r1
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/BT-01a.spec.md python3 scripts/batch-verify.py docs/batches/BT-01a.spec.md
```

## 预核 vs 实入（5 预核，2 实入）

| # | doc ref | 实核 file:line | 判 | 理由 |
|---|---|---|---|---|
| 1a/1b | commands.rs:30 | commands.rs:30 + :31 | **退** | 修法 (b) log-and-continue → family = **error-visible-non-blocking**（同 C5-AP-06）；登记 **C5-BT-01a-1**，未来该 family 成批时并入 |
| 2 | keyring.rs:263 | keyring.rs:272-276 | **入** | (c) 顺序改动（设计决策）；fn -> CommandResult<()>，`?` 允许 |
| 3 | slash.rs:456 | bot_slash.rs:462 | **入** | fn -> CommandResult<bool>，`?` 允许 + NotFound 承认语义 |
| 4 | skills/scheduler.rs:428 | scheduler.rs:431 | **退** | `skill_finish -> String`（非 Result）→ `let _` 丢 String 非错误 |
| 5 | scheduler.rs:0（无效） | bot_scheduler.rs:263/:265 | **退** | fn `find_due_tasks -> Vec`（非 Result）+ `unwrap_or_default()` 属 failure-recovery-default-value |

## 不在本批

- **C5-BT-01a-1**（commands.rs:30/:31 migrate_legacy_key / migrate_search_keys）：修法 (b) log warn + 继续 → family = error-visible-non-blocking（与 C5-AP-06 同 family）。已登记 PHASE2-TRIAGE §3.5，未来该 family 成批时并入。
- #4（skills/scheduler.rs:431）：`skill_finish -> String` 非错误丢弃；同文件真丢弃 :34 在 `fn persist_outcome_quiet -> ()`，`?` 不允许（退 triage）
- #5（bot_scheduler.rs:265）：归 failure-recovery-default-value（BT-01b 同类），fn 返 Vec 需先改签名
- **C5-EV-3b-A**：退 triage 重做（不在本轮）
- **§2 簇清单 version / last-checked 标注**：下次 docs commit 带
