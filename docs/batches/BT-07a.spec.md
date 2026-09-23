# BT-07a spec — scheduler.rs:149 回滚复原 Drop 兜底（C5-BT-07 拆批 1/3）

## 拆批理由（family_heterogeneity 预拆，不触发 stop）

C5-BT-07 三条异质：
- **scheduler.rs:149**（本批）：panic 安全——rollback `execute_tool().await` panic 或
  restore 自身失败 → run 永久卡 Running，AtomicGuard 门禁被永久重开。修法 = RAII
  Drop 守卫，机械、零语义变更。
- bot_artifacts.rs:113：乐观并发校验缺——finding 称 `db_upsert` 不比
  `expected_updated_at`，与 tasks.rs:185 (i) 决策核验记录（upsert 有谓词校验，
  8 Some + 1 None）矛盾，**疑似 FP，需先核**（下批）。
- state.rs:164：`clear_terminal_skill_runs` 全局清理 vs 其余 helper 全部按会话——
  语义方向（接受全局语义 vs 加 session_id 参数 = 签名改），**B 类候选**。

## 目标 finding（1 条）

**scheduler.rs:149**（high）：`reopen_failed_run_for_rollback`（Failed→Running）与
`restore_failed_run_after_rollback`（复原 Failed）之间夹整个回滚段
（`execute_tool(...).await` 循环）。任一 panic → restore 跳过 → run 卡 Running，
AtomicGuard 放行窗口永久化。

## 修法（RAII Drop 守卫，scheduler.rs 私有）

```rust
struct RollbackRestoreGuard<'a, R: tauri::Runtime> {
    app: &'a tauri::AppHandle<R>,
    name: &'a str,
    session_id: Option<&'a str>,
    armed: bool,
}
impl Drop { armed → super::state::restore_failed_run_after_rollback(...) }
```

用法（run_rollback_segment_core）：
- `reopened` 后立即 `let mut restore_guard = … armed: reopened`；
- 正常路径保留原手动 restore（审计行顺序不变），其后 `restore_guard.armed = false`；
- panic 路径由 Drop 兜底。双重 restore 安全（restore 对非 Running 是 no-op，
  state.rs:124-125 注释钉死）。

## 行为变更

仅 panic/异常路径：run 不再卡 Running（复原 Failed 终态）。正常路径零变更
（手动 restore 在原位置执行，守卫 disarm）。

## 测试（scheduler.rs tests mod 新增 1 条）

`rollback_restore_guard_restores_failed_on_panic`：mock AppHandle（照 state.rs
test_handle 形态）+ 插入 Failed run → reopen 置 Running → catch_unwind 内构造守卫
+ panic → 断言 Drop 后 run 复原 Failed（+守卫 armed=false 不重复 restore 由
restore no-op 语义保证）。约 5 断言。

## spec 起草后自查三条

1. `expected_files` → `src-tauri/src/bot_skills/scheduler.rs`（单文件）。无签名
   ripple：guard 为私有 struct；run_rollback_segment_core 签名不变。✓
2. budget → A/B 混合：guard+impl ≈+20；用法 +3/-0；测试 ≈+35。合计 ≈+58/-0 →
   budget +70/-10。✓
3. findings fix 字段列 ripple → 无（全在 scheduler.rs 内部）。✓

## 自主执行规则

spec 被 reviewer 批准后:
- agent 全权执行至 commit,不中途报 status / exit / diff
- 例外只有 stop 条件触发
- 完成时一次报: hash + 批定性 + OCR 轮次 + comments 处置 + follow-up
- 过程证据落 ~/.openclaw/cache/<batch-id>/,不主动读回

## Stop 条件(触发即停,报 reviewer)

- compile_failure (无 X 形态退路)
- architecture_blocker (签名/调用点超预算)
- family_heterogeneity
- new_high_different_root (OCR r1 出的 high 非本批根因)
- gate_fail (batch-verify FAIL 且非 spec 声明调整可解)

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "BT-07a",
  "family": "state-machine-panic-safety",
  "expected_files": [
    "src-tauri/src/bot_skills/scheduler.rs"
  ],
  "max_lines_added": 70,
  "max_lines_removed": 10,
  "findings": [
    {"id": "C5-BT-07.3", "file": "src-tauri/src/bot_skills/scheduler.rs", "line": 149, "fix": "新增 RollbackRestoreGuard（Drop 兜底 restore_failed_run_after_rollback）；reopen 后 armed=reopened，正常路径手动 restore 后 disarm；panic 路径 Drop 复原 Failed。新增 panic 路径单测。ripple：无（私有 struct，签名不变）"}
  ],
  "assertions_min": {
    "src-tauri/src/bot_skills/scheduler.rs": 45
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 4
  }
}
```
