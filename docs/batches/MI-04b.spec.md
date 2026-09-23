# Batch Spec: MI-04b

## 目的

C5-MI-04b：journal find+act TOCTOU。核后的真实竞态对：replay
（spawn_polling 启动路径，**无 MigrationGuard**）vs run_migration（有守卫）——
两者可在相同 (task_id, src) 上交错。修法：replay 纳入 MigrationGuard（等到拿到守卫再
replay），journal 行动者回到单写者语义。**不取** finding 的 try_claim 新状态方案
（状态机变更），**也不能**取 DB_WRITE_LOCK 包 find+act（act 内含 db_upsert 会重入死锁，
journal.rs:15-17 注释明示）。

## 人类可读摘要

- family: race/TOCTOU（单写者纪律收口）
- 覆盖 findings: 1（C5-MI-04b，journal.rs:126 find_pending 无锁 → 实际竞态对在 run.rs
  spawn_polling 的 replay 调用点，行号漂移已核）
- 预估 diff: 1 file / +26/-3 lines（B 类：插入守卫等待块；OCR r1 critical 采纳后 + 作用域限制块 + 等待上限，budget 校正一次 14→26）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 2

## 红线

- 不改 journal 状态机（无新 state、无 try_claim helper）
- 不动 replay 内部用 journal_committed_inner/cleared_inner（无 DB_WRITE_LOCK）的既有形态
  ——MigrationGuard 互斥后 journal 表只剩单行动者，无实际对手方；统一 inner/locked 形态
  属一致性洁癖，登记 follow-up
- 不改 MigrationGuard 本身（acquire/Drop 不动）
- 编译层事实必须核实，不许凭印象

## spec 起草后自查三条（APW-02a 2026-09-23 立）

1. `expected_files` 是否覆盖全部写入路径 → 仅 run.rs（spawn_polling 内插守卫等待块）。
   无签名 ripple（spawn_polling 签名不变）。✓
2. budget 是 A 类还是 B 类？→ B 类（插入块 ~12 行 + 注释）→ budget +14/-2。✓
3. findings 逐条 fix 字段是否显式列出 ripple 的文件 + 行号？→ 无 ripple；竞态对两端
   （run.rs:139/:276 guarded 侧 + run.rs:410 unguarded replay 侧）已在 fix 字段列明。✓

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
  "batch_id": "MI-04b",
  "family": "race-toctou-single-writer",
  "expected_files": [
    "src-tauri/src/migration/run.rs"
  ],
  "max_lines_added": 26,
  "max_lines_removed": 3,
  "findings": [
    {"id": "C5-MI-04b", "file": "src-tauri/src/migration/run.rs", "line": 410, "fix": "spawn_polling 启动 replay 前等待获取 MigrationGuard（5s 轮询 acquire），与 run_migration 互斥 → journal find+act 单写者化。竞态对两端：run.rs:139/:276（guarded run 内 find_pending→decide→committed）vs run.rs:410（unguarded replay 写 journal）。无 ripple：签名不变；Finding 原文所述 journal.rs:126 无锁 SELECT 本身保留（WAL 下读写不互斥，注释已述），关闭的是并发写者"}
  ],
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

## 提交信息骨架

```
fix(migration): MI-04b — replay 纳入 MigrationGuard，journal find+act 单写者化（C5-MI-04b）

【family】race/TOCTOU（单写者纪律收口）
【实修 1 条】
- run.rs:410 spawn_polling：启动 replay 前 5s 轮询等待 MigrationGuard——replay 是
  run_migration 外唯一另一个 journal 写者（且用 inner 无锁写），不挂守卫时与进行中
  的 run 在相同 (task_id,src) 上 find+act 交错
【不取方案】finding 的 try_claim 新状态 helper（状态机变更，超 family 最小修法）；
DB_WRITE_LOCK 包 find+act（act 内含 db_upsert 重入同锁 = 死锁，journal.rs 注释明示）
【行为变更】启动 60s 时若迁移正在进行，replay 等待而非并发执行（不丢 replay——
轮询直到拿到守卫）。
【D2】行为断言：replay 执行时必持 MigrationGuard（与 run 互斥）。前置断言：守卫空闲时
replay 行为与现状一致（获取成本一次原子 CAS）。反例断言：若 replay 不再持守卫，
spawn_polling 中 acquire 调用点消失（grep 可见）。
【自测】cargo fmt / cargo check --tests / scripts/test-all.sh 全绿
【OCR】r1
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/MI-04b.spec.md python3 scripts/batch-verify.py docs/batches/MI-04b.spec.md
```

## 不在本批

- replay 内 journal_committed_inner/journal_cleared_inner 无 DB_WRITE_LOCK 的既有形态：
  MigrationGuard 互斥后 journal 表单行动者，无实际竞态；统一走 locked 包装属一致性
  洁癖，登记 follow-up
- C5-DB-03（db 域 race/TOCTOU 2 条）：§3 已标"原子创建 vs 读侧同步两种设施，可能拆"，
  独立批
