# Batch Spec: NEW-2

## 目的

落地 PHASE2-TRIAGE-NEW-2（用户拍板立项，单独一批）：db / migration / bot.config 三域 12 处同形静默 `into_inner` 补 C3-1 留痕（eprintln poisoned 日志，EVNB-02 同款形态）。本批为纯机械同形修复——12 处模式完全统一（都是 `unwrap_or_else(|e| e.into_inner())` 无日志），不拆簇；12 处超 SOP ≤10 上限系用户拍板明示（「它自己就够一批」）。

**范围声明（重要）**：本批只修 NEW-2 登记的 12 处。实施时全仓 grep 实测另有 47 处同形 silent into_inner（bot_slash/memory/bot_chat/exec_steps/py 等域——NEW-2 登记注记「其他域多数已有 eprintln」与现状不符），**不越权扩**，登记 NEW-2b 待拍板。

## 人类可读摘要

- family: poisoned-silent-recovery（溶解家族复活：12 成员同形）
- 覆盖: 12 处（11 生产 + 1 测试代码 bot/config/mod.rs:581，与 NEW-1 测试面同口径）
- 预估 diff: 6 文件 / +50/-12（无新文件）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 4

## 逐条处置（原文均已核 + 用户拍板 2026-09-26）

统一修法（EVNB-02 / db::lock_db_write / journal.rs C3-1 同款）：`.unwrap_or_else(|e| e.into_inner())` → `.unwrap_or_else(|e| { eprintln!("[mutex_poisoned] <site>: {e:?}"); e.into_inner() })`。恢复语义不变（into_inner 照旧），仅补可见性。

| # | 文件:行（现值） | 站点标签 |
|---|---|---|
| 1-3 | db/workspace.rs:173/:189/:293 | db::workspace DB_WRITE_LOCK |
| 4-5 | db/bot_history.rs:94/:113 | db::bot_history DB_WRITE_LOCK |
| 6-8 | db/bot_sessions.rs:78/:100/:120 | db::bot_sessions DB_WRITE_LOCK |
| 9-10 | migration/ops.rs:315/:325 | migration::ops move_remove_fail_counts |
| 11 | bot/config/audit.rs:31 | bot::config::audit BOT_LOG_LOCK |
| 12 | bot/config/mod.rs:581（测试） | bot::config::mod BOT_LOG_TEST_LOCK (test) |

**ripple**：无（纯日志，行为不变）。

## 测试

- 无新测试：纯机械日志修复，零行为变更；test-all 全量编译+回归担保
- 收口验证：grep 确认 6 文件无残留 silent 形态（`unwrap_or_else(|e| e.into_inner())` 计数归零）

## 红线

- family 一致性：12 处同形机械修复（用户拍板超 ≤10 上限）
- 只改 12 处，其他 47 处不碰（NEW-2b 待拍板）
- 恢复语义不变（into_inner 保持）
- 新注释/日志串不引用审计批次号

## spec 起草后自查三条

1. expected_files：db/workspace.rs + db/bot_history.rs + db/bot_sessions.rs + migration/ops.rs + bot/config/audit.rs + bot/config/mod.rs = 6（全路径已列）
2. budget：每站点 +4/-1 × 12 = +48/-12，上限 +70/-45（执行中校正 ×1：rustfmt 对多行闭包重排后实测 -36 超 -20 估）；无新文件
3. fix 字段：无签名/调用链变化；assertions_min 全 0（纯日志无新断言，vitest/Rust 同理由 grep 验证 + test-all 担保）

## 自主执行规则

spec 经用户拍板（NEW 全立项，2026-09-26）后：
- agent 全权执行至 commit，不中途报 status / exit / diff
- 例外只有 stop 条件触发
- 完成时一次报：hash + 批定性 + OCR 轮次 + comments 处置 + push 两值
- 过程证据落 ~/.openclaw/cache/NEW-2/，不喊人

## Stop 条件（触发即停，报用户）

- compile_failure（无 X 形态退路）
- architecture_blocker（签名/调用点超预算）
- family_heterogeneity
- new_high_different_root（OCR r1 出的 high 非本批根因）
- gate_fail（batch-verify FAIL 且非 spec 声明调整可解）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "NEW-2",
  "family": "poisoned-silent-recovery",
  "expected_files": [
    "src-tauri/src/db/workspace.rs",
    "src-tauri/src/db/bot_history.rs",
    "src-tauri/src/db/bot_sessions.rs",
    "src-tauri/src/migration/ops.rs",
    "src-tauri/src/bot/config/audit.rs",
    "src-tauri/src/bot/config/mod.rs"
  ],
  "max_lines_added": 70,
  "max_lines_removed": 45,
  "findings": [
    {"id": "NEW-2.ws", "file": "src-tauri/src/db/workspace.rs", "line": 173, "fix": "3 处 silent into_inner 补 eprintln [mutex_poisoned] db::workspace DB_WRITE_LOCK（C3-1/EVNB-02 形态）；ripple：无"},
    {"id": "NEW-2.bh", "file": "src-tauri/src/db/bot_history.rs", "line": 94, "fix": "2 处同上（db::bot_history DB_WRITE_LOCK）；ripple：无"},
    {"id": "NEW-2.bs", "file": "src-tauri/src/db/bot_sessions.rs", "line": 78, "fix": "3 处同上（db::bot_sessions DB_WRITE_LOCK）；ripple：无"},
    {"id": "NEW-2.ops", "file": "src-tauri/src/migration/ops.rs", "line": 315, "fix": "2 处同上（migration::ops move_remove_fail_counts）；ripple：无"},
    {"id": "NEW-2.audit", "file": "src-tauri/src/bot/config/audit.rs", "line": 31, "fix": "1 处同上（bot::config::audit BOT_LOG_LOCK）；ripple：无"},
    {"id": "NEW-2.cfgmod", "file": "src-tauri/src/bot/config/mod.rs", "line": 581, "fix": "1 处同上（测试代码 BOT_LOG_TEST_LOCK）；ripple：无"}
  ],
  "assertions_min": {
    "src-tauri/src/db/workspace.rs": 0,
    "src-tauri/src/db/bot_history.rs": 0,
    "src-tauri/src/db/bot_sessions.rs": 0,
    "src-tauri/src/migration/ops.rs": 0,
    "src-tauri/src/bot/config/audit.rs": 0,
    "src-tauri/src/bot/config/mod.rs": 0
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

## 提交信息骨架

```
fix(arch): NEW-2 — poisoned-silent-recovery（12 处 silent into_inner 补 C3-1 留痕）

【family】12 处同形机械修复（用户拍板单独一批，超 ≤10 上限系明示）
【实修 12 处】6 文件逐处 eprintln [mutex_poisoned] <site>（恢复语义不变）
【范围声明】全仓另有 47 处同形 silent into_inner（bot_slash/memory 等）不越权扩 → NEW-2b 待拍板
【自测】grep 归零验证 + cargo fmt/check + tsc + test-all 全绿
【OCR】r1：<N> comments <处置>
【基线】D2: files=6(+A/-R) asserts=0→0 tests=test-all 全绿
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/NEW-2.spec.md python3 scripts/batch-verify.py docs/batches/NEW-2.spec.md
```
