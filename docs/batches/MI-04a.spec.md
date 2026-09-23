# Batch Spec: MI-04a

## 目的

C5-MI-04a：journal_pending 无条件 INSERT 新 pending 行——同 (task_id, src) 重入/重试产生
多条 pending，find_pending 只取 id 最新一条，旧行成不可见孤儿，replay 语义被搅浑。
修法：持锁内 check-then-reuse（既有 pending 行刷新 op/dst/created_at 并复用 id），
不加 schema（不动既有 DB）。

## 人类可读摘要

- family: 无跨域 family（journal 完整性单点 bug；finding 原文建议二选一：UNIQUE partial
  index **或** 复用既有 id——本批取后者，零 schema 变更）
- 覆盖 findings: 1（C5-MI-04a，journal.rs:34 → 实际 :36 的 INSERT，行号漂移已核）
- 预估 diff: 2 files / +70/-18 lines（实测 +66/-3，added 超估 1 行 = 估算错，按 SOP 校正一次）（B 类：inner 函数体重写 + 既有测试块改写 + 新测试）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 3

## 红线

- **零 schema 变更**（不建 UNIQUE index——既有库可能已含重复 pending，CREATE UNIQUE
  INDEX 会失败且需数据迁移 = 数据破坏相关修法，超本批）
- **不清理既有孤儿 pending**（老库已存在的重复行不动；replay 仍见全部——登记 follow-up）
- 不改 `journal_pending_inner` / `journal_pending` 签名（返回仍是 row id，调用点
  run.rs:214/:333 零改动）
- 去重原子性依赖既有约定：所有 journal 写走 db_write_lock（NEW-B-2），find+act 在锁内
- 编译层事实必须核实，不许凭印象

## spec 起草后自查三条（APW-02a 2026-09-23 立）

1. `expected_files` 是否覆盖全部写入路径 → journal.rs（inner 去重逻辑）+
   migration/mod.rs（:393-416 旧行为测试块改写 + 1 个新测试）。无签名 ripple
   （run.rs:214/:333 调用点已核，语义兼容：返回值仍是可操作的有效 pending id）。✓
2. budget 是 A 类还是 B 类？→ B 类：journal.rs inner +24/-1；mod.rs 测试块改写
   +9/-10 + 新测试 +25 → 合计 +58/-11 → budget +65/-18。✓
3. findings 逐条 fix 字段是否显式列出 ripple 的文件 + 行号？→ 见 JSON；唯一 ripple =
   mod.rs:393-416 测试块编码了旧行为（两条 pending 同 key 取最新 id），改写为新不变量。✓

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
  "batch_id": "MI-04a",
  "family": "journal-integrity-dedup",
  "expected_files": [
    "src-tauri/src/migration/journal.rs",
    "src-tauri/src/migration/mod.rs"
  ],
  "max_lines_added": 70,
  "max_lines_removed": 18,
  "findings": [
    {"id": "C5-MI-04a", "file": "src-tauri/src/migration/journal.rs", "line": 36, "fix": "journal_pending_inner 先 SELECT 同 (task_id,src) 最新 pending，存在则 UPDATE 刷新 op/dst/created_at 并复用其 id（持锁内 check-then-act，原子性由 db_write_lock 保证），否则原样 INSERT。ripple：migration/mod.rs:393-416 测试块编码旧行为（同 key 双 pending 取 id 最大者），改写为新不变量（复用同 id + dst 刷新）；新增 journal_pending_dedups_same_key 测试（id 复用 + pending 行数恒 1 + op/dst 刷新）"}
  ],
  "assertions_min": {
    "src-tauri/src/migration/mod.rs": 122
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
fix(migration): MI-04a — journal_pending 同 (task_id,src) 去重复用（C5-MI-04a）

【family】无跨域 family（journal 完整性单点 bug）
【实修 1 条】
- journal.rs:36 journal_pending_inner：无条件 INSERT → 持锁内 check-then-reuse
  （同 key 既有 pending 刷新 op/dst/created_at 复用 id；否则原样 INSERT）
- ripple：mod.rs:393-416 测试块原编码旧行为（同 key 双 pending 取最新 id）→ 改写为
  新不变量断言；新增 journal_pending_dedups_same_key
【行为变更】同 (task_id,src) 重入/重试不再产生孤儿 pending 行（旧：id DESC 只认最新，
旧行不可见但仍被 replay 扫到）。既有库已存在的孤儿 pending 不清理（follow-up）。
【D2】行为断言：同 key 二次 pending 返回同 id 且 pending 行数恒 1、op/dst 刷新。
前置断言：异 key / committed / cleared 行为不变（既有测试全绿）。
反例断言：若去重失效，journal_pending_dedups_same_key 的行数断言挂。
【自测】cargo fmt / cargo check --tests / scripts/test-all.sh 全绿
【OCR】r1
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/MI-04a.spec.md python3 scripts/batch-verify.py docs/batches/MI-04a.spec.md
```

## 不在本批

- C5-MI-04b（journal.rs:126 find+act TOCTOU）：finding 建议 journal_try_claim 原子 helper =
  API 面扩张（新 pub(crate) 函数 + 调用方决策流改造），需先核 run.rs:139/:276 调用形态，
  独立批
- 既有库孤儿 pending 清理（数据迁移方向）：数据破坏相关，B 类候选攒批报人拍
- UNIQUE partial index（finding 的另一选项）：需 schema 迁移 + 既有重复行处理，不取
