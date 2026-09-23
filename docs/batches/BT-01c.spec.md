# BT-01c spec — C5-BT-01a 余 2 条（error-not-propagated，log-and-continue 可见化）

## 目标

C5-BT-01a 剩余 2 条（前 3 条已在前批修复）：

1. **bot_skills/scheduler.rs:428**（high）：Done 路径 `let _ = skill_finish(...)`——OCR 原文称
   "skill_finish 返回 Err 则静默"，**核实后不成立**：skill_finish 返回 `String`（rollback hint），
   ok=true 路径恒为空串，丢弃无害。**真实缺口**：skill_finish 对"零迁移"（无 Running/Paused
   run 匹配该 session）完全静默——调用方以为状态机已收尾，实际什么都没发生，僵尸 Running
   泄漏恰恰在这个路径不可见。修复落在 runtime.rs skill_finish 内部（见下）。
2. **bot_scheduler.rs find_due_tasks**（high，:263 起）：两处 `db_upsert(...).await.is_ok()`
   只分支成功——乐观锁冲突（用户并发编辑使 expected_updated_at 过期）或 IO 失败时，
   stale at: 清理 / missed recurring 的 sched_last 更新静默丢失，零可见性。

## 修法

### 1. runtime.rs `skill_finish`（:368 起）

- 循环内加 `transitioned` 标记（session 匹配且状态 Running/Paused 的分支置 true）。
- 循环后、`return` 前：**仅 ok=true 且零迁移**时补一条审计——
  `skill_finish_no_transition | session: {} | reason: {}`（session_id None 渲染 `-`，
  reason 走 truncate_for_log 120）。
- **仅 ok=true 才记**：false 路径调用点（bot_model_loop 7 处错误收尾）在无活动技能时
  合法 no-op，全记会刷正常路径日志；ok=true 的两个调用点（scheduler.rs:431 done 收尾 +
  bot_model_loop.rs:594 AdvanceAction::Finish）都以"存在活动 run"为前提，零迁移恒为异常。
- 签名不变（仍返回 String），无 ripple（bot_model_loop 10 处调用点不动）。

### 2. bot_scheduler.rs find_due_tasks 两处

`.is_ok()` 分支 → `match`：Ok 广播逻辑原样；Err → `crate::bot::audit_log`：

- stale 清理失败：`sched_stale_cleanup_failed | ids: {} | err: {}`
- missed 标记失败：`sched_missed_mark_failed | ids: {} | err: {}`
- err 走 `truncate_for_log(&e.to_string(), 200)`（CommandError 有 Display）。
- **不重试**（重试是语义变更，挂 follow-up）；只加可见性，与本家族 log-and-continue 一致。

## spec 起草后自查三条

1. `expected_files` → `src-tauri/src/bot_skills/runtime.rs` + `src-tauri/src/bot_scheduler.rs`。
   无签名 ripple。✓
2. budget → B 类：runtime.rs transitioned 标记 + 审计块 +10/-0；bot_scheduler.rs 两处
   is_ok→match 各 +8/-1 ≈ +16/-2；注释微调 +4。合计 ≈+30/-2 → budget +40/-10。✓
3. findings fix 字段列 ripple → runtime.rs 签名不变无 ripple；finding 1 的源站
   scheduler.rs:428 本身代码行不动（修在被调函数内），fix 字段注明。✓

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
  "batch_id": "BT-01c",
  "family": "error-not-propagated",
  "expected_files": [
    "src-tauri/src/bot_skills/runtime.rs",
    "src-tauri/src/bot_scheduler.rs"
  ],
  "max_lines_added": 40,
  "max_lines_removed": 10,
  "findings": [
    {"id": "C5-BT-01a.4", "file": "src-tauri/src/bot_skills/runtime.rs", "line": 368, "fix": "skill_finish 加 transitioned 标记；ok=true 且零迁移时 audit_log_hook(skill_finish_no_transition)。源 finding 站 scheduler.rs:428 的 let _ = 本身无害（返回值 ok 路径恒空串），真实缺口=零迁移不可见，修在被调函数内。ripple：无（签名不变）"},
    {"id": "C5-BT-01a.5", "file": "src-tauri/src/bot_scheduler.rs", "line": 263, "fix": "find_due_tasks 两处 db_upsert .is_ok() → match：Ok 广播不变；Err → audit_log（sched_stale_cleanup_failed / sched_missed_mark_failed，err truncate 200）。ripple：无（函数签名不变）"}
  ],
  "assertions_min": {
    "src-tauri/src/bot_skills/runtime.rs": 57,
    "src-tauri/src/bot_scheduler.rs": 46
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
fix(bot): BT-01c — skill_finish 零迁移可见化 + find_due_tasks upsert 失败留痕（C5-BT-01a 余 2 条）

【family】error-not-propagated（log-and-continue 可见化，不重试不改语义）
【实修 2 条】
- bot_skills/runtime.rs skill_finish：ok=true 且零迁移（无 Running/Paused run 匹配 session）
  → audit_log_hook(skill_finish_no_transition)。源 finding 站 scheduler.rs:428 的 let _ =
  丢弃 ok 路径恒空串无害，真实缺口是零迁移静默 → 僵尸 Running 不可见。
- bot_scheduler.rs find_due_tasks 两处 db_upsert .is_ok() → match Err 臂 audit_log：
  sched_stale_cleanup_failed / sched_missed_mark_failed（乐观锁冲突/IO 失败不再静默丢清理）。
【行为变更】仅新增审计日志行；状态机、广播、返回值不变。
【D2】行为断言：现有 103 断言（runtime 57 + bot_scheduler 46）钉死状态机/调度语义不回归。
前置断言：Ok 路径广播逻辑逐字保留。反例断言：若 Err 臂被删，审计行消失 = 回到静默。
【自测】cargo fmt / cargo check --tests / scripts/test-all.sh 全绿
【OCR】r1
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/BT-01c.spec.md python3 scripts/batch-verify.py docs/batches/BT-01c.spec.md
```

## 不在本批

- find_due_tasks "每 tick 最多 3 次 db_load" 的 race 窗口收窄（架构/性能，finding 附带提及）：
  本批只加可见性，窗口收窄属设计变更 → 挂 follow-up 登记。
- bot_scheduler.rs 其余吞错站点（如有）：不在 OCR 271 名单本簇内，不顺手扩。
