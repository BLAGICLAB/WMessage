# BT-09a spec — scheduler.rs:283 Finish 早退假完成收口（C5-BT-09 拆批 1/3）

## 拆批理由

C5-BT-09 三条异质：
- **scheduler.rs:283**（本批）：`advance_dsl` 返 Finish 中途 break 后，收尾块
  (a) summary 标题用 `steps.len()`（计划步数）冒充实执步数；(b) 无条件
  `skill_finish(app, true, "done")`——run 已被外部信号置 Completed，此调用是
  无迁移调用且带非空 reason，经 BT-01c 闸门会误记 skill_finish_no_transition；
  (c) `skill_dsl_done` 审计行 steps_ok 同样用 steps.len()。
- dispatch.rs:248：post-execute 用 `TOOLS_TABLE.iter().find` 线性查 max_output_chars，
  上方已是 O(1) `tools_index()`——纯性能机械修（下批 BT-09b）。
- dispatch.rs:424：`commit_and_report` 失败走 `ToolResult::ok`——dispatch.rs:402-404
  注释钉死的**设计意图**（防 LLM severity classifier 误判 fatal），与
  PHASE2-TRIAGE-OCR-003（tools.rs:160 同型）同处理 = **wontfix-with-rationale**（下批登记）。

## 目标 finding（1 条）

**scheduler.rs:283**（high）：Finish 早退路径假完成。注意 Finish 的语义 =
**外部信号已把 run 置 Completed**（step-check 熔断 / 外部完成），调度器 break 是
「承认外部终态」，不是「自己跑完全部步骤」。

## 修法（scheduler.rs run_skill_scheduler_core，机械）

1. Finish 分支置 `finish_signal = true` 再 break。
2. summary 标题改 `results.len()`（实执步数）；`finish_signal` 时追加一行
   「（外部完成信号介入：计划 {steps.len()} 步，已执行 {results.len()} 步，剩余跳过）」。
3. `skill_finish` 仅在 `!finish_signal` 时调（finish_signal 路径 run 已是 Completed，
   无僵尸泄漏——:451-454 注释的泄漏约束只适用于自然跑完路径）。
4. `skill_dsl_done` 审计 steps_ok 改 `results.len()`。
5. outcome kind 保持 "done" / DslOutcome::Done——外部信号已完成 run，终态属实；
   不实的是步数与重复 finish，不是 done 本身。

## 行为变更

仅 Finish 早退路径：summary 步数从「计划步数」变「实执步数」+ 介入说明行；
不再误记 skill_finish_no_transition；审计 steps_ok 变实执数。自然跑完路径零变更。

## spec 起草后自查三条

1. `expected_files` → `src-tauri/src/bot_skills/scheduler.rs`（单文件）。无签名
   ripple（run_skill_scheduler_core 签名不变）。✓
2. budget → A 类：flag + 分支 + 文案 ≈+12/-3 → budget +18/-8。✓
3. findings fix 字段列 ripple → 无。✓

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
  "batch_id": "BT-09a",
  "family": "state-machine-false-completion",
  "expected_files": [
    "src-tauri/src/bot_skills/scheduler.rs"
  ],
  "max_lines_added": 18,
  "max_lines_removed": 8,
  "findings": [
    {"id": "C5-BT-09.3", "file": "src-tauri/src/bot_skills/scheduler.rs", "line": 283, "fix": "Finish 分支置 finish_signal；summary 标题与 skill_dsl_done 审计改 results.len()；finish_signal 时 summary 追加介入说明行且跳过 skill_finish（run 已被外部置 Completed，无僵尸泄漏）。ripple：无"}
  ],
  "assertions_min": {
    "src-tauri/src/bot_skills/scheduler.rs": 47
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 4
  }
}
```
