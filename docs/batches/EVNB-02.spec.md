# Batch Spec: EVNB-02 (v1)

## 目的

C3-1 mutex poison 日志约定补齐：两处 `unwrap_or_else(|e| e.into_inner())` 静默吞 poison 事件
（无 eprintln），违反 C3-1（lock_db_write 既有形态 = eprintln + into_inner 恢复）。
补 eprintln 使错误可见，不改变恢复行为。

## 人类可读摘要

- family: error-visible-non-blocking（poison 事件日志可见 + 不阻断；与 EVNB-01 / C5-AP-06 同形态）
- 覆盖 findings: 2（C5-DB-05 的 mutex 条 tasks.rs:469-471；C5-MI-01 journal.rs:19-21）
- 预估 diff: 2 files / +4/-4 lines（A 类）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 5

## 红线

- family 一致性：只含 C3-1 poison-log 形态（eprintln 可见 + 行为不变）
- **不改恢复语义**（仍 into_inner 恢复——传播 Err 会改 C3-1 既有约定，属 B 类，不在本批）
- 只动 triage 清单内的 2 站；其余 ~12 处同形静默站（不在 OCR 271 名单）登记 PHASE2-TRIAGE-NEW-2，不顺手扩

## Stop 条件

- compile_failure / architecture_blocker / family_heterogeneity / new_high_different_root / gate_fail

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "EVNB-02",
  "family": "error-visible-non-blocking",
  "expected_files": [
    "src-tauri/src/db/tasks.rs",
    "src-tauri/src/migration/journal.rs"
  ],
  "max_lines_added": 6,
  "max_lines_removed": 6,
  "findings": [
    {
      "id": "C5-DB-05-mutex",
      "file": "src-tauri/src/db/tasks.rs",
      "line": 469,
      "fix": "db_delete 内 3 行 raw DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner()) → super::lock_db_write()（mod.rs:295 已带 eprintln mutex_poisoned + HOLDING_DB_WRITE 标记；与同文件 db_upsert_for :450 / tasks_import :519 统一）。源 OCR finding tasks.rs:423 指名 db_upsert_for/db_delete/tasks_import 三处，另两处已合规，本批只补 db_delete。无 ripple。"
    },
    {
      "id": "C5-MI-01",
      "file": "src-tauri/src/migration/journal.rs",
      "line": 19,
      "fix": "db_write_lock() 的 unwrap_or_else(|e| e.into_inner()) 闭包改块：eprintln!(\"[mutex_poisoned] migration::journal DB_WRITE_LOCK: {e:?}\") 后 e.into_inner()。保持返回 MutexGuard（不用 lock_db_write 的 DbWriteGuard：journal 注释明记防重入死锁的短临界区设计，不动）。无 ripple。"
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

- `super::lock_db_write() -> DbWriteGuard`（db/mod.rs:295-302，内含 eprintln!("[mutex_poisoned] db::DB_WRITE_LOCK: {e:?}")）——db_delete 换用后 guard 类型变（DbWriteGuard），用法 `let _g =` 不变
- `journal.rs:19 db_write_lock() -> MutexGuard<'static, ()>` 保持原形，仅闭包内加 eprintln
- 源 finding（OCR-CODE-REVIEW-2026-09-21-fullscan.json tasks.rs:423）："db_upsert_for、db_delete、tasks_import"——实测 :450/:519 已走 lock_db_write，仅 db_delete :469-471 残留
- 无签名变化、无调用点变化 → 无 ripple 文件

## Budget 逐点算（A 类）

| # | 改动点 | 类 | + | - |
|---|---|---|---|---|
| 1 | tasks.rs:469-471 3 行 → 1 行（lock_db_write） | A | 1 | 3 |
| 2 | journal.rs:21 1 行闭包 → 4 行块（+eprintln） | A | 4 | 1 |
| **合计** | | | **5** | **4** |

**budget: max_lines_added: 6 / max_lines_removed: 6**（预估 +5/-4；执行时按 numstat 校正一次）

## 提交信息骨架

```
fix(db,migration): EVNB-02 — C3-1 mutex poison 日志补齐（db_delete + journal）

【family】error-visible-non-blocking（poison 事件 eprintln 可见 + 不阻断；恢复语义不变）
【实修 2 站】
- tasks.rs:469-471 — db_delete raw lock → super::lock_db_write()（统一同文件 :450/:519，
  自带 mutex_poisoned eprintln + HOLDING_DB_WRITE 标记）
- journal.rs:19-21 — db_write_lock 闭包加 eprintln!("[mutex_poisoned] migration::journal ...")
  保持 MutexGuard 返回（短临界区防重入设计不动）
【D2】行为断言：poison 发生时 stderr 有 [mutex_poisoned] 行；前置断言：锁获取与恢复行为不变
（仍 into_inner）；反例断言：若 poison 无日志输出，则该路径违反 C3-1——本批后两站不存在此路径。
【自测】cargo fmt / cargo check --tests / scripts/test-all.sh 全绿
【OCR】r1
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/EVNB-02.spec.md python3 scripts/batch-verify.py docs/batches/EVNB-02.spec.md
```

## 不在本批

- C5-DB-05 其余 3 条：paths.rs:50（copy_legacy_db open 失败 warn+continue → fail-closed 方向）、
  workspace.rs:241（import updated_at 缺省 policy）、bot_history.rs:52（批量同 now 时间戳）
  —— 均涉 policy / 修法方向选择，属 B 类候选，另行报拍
- PHASE2-TRIAGE-NEW-2：其余 ~12 处同形静默 into_inner（workspace.rs:173/:189/:278、
  bot_history.rs:89/:108、bot_sessions.rs:78/:100/:112、ops.rs:236/:246、
  config/audit.rs:26、config/mod.rs:581）不在 OCR 271 名单，登记后下一轮评估
- C5-MI-03（rules.rs:27 quarantine vs 签名扩 = B 类；rules.rs:121 CSV coerce 收紧）
