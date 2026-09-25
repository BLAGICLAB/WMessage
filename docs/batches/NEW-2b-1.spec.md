# Batch Spec: NEW-2b-1

## 目的

落地 PHASE2-TRIAGE-NEW-2b（bot 域部分，用户 2026-09-26 拍板「未执行完的按 SOP 执行」）：bot 域 24 处同形 silent `into_inner` 补 C3-1 留痕（NEW-2 同款机械形态）。

## 人类可读摘要

- family: poisoned-silent-recovery（与 NEW-2 同族延续）
- 覆盖: 24 处 / 7 文件（bot_skills 7 + bot_slash 7 + bot_chat 5 + exec_steps 3 + bot_scheduler 2，其中 1 处测试代码）
- 预估 diff: 7 文件 / +100/-24
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 3

## 逐条处置（用户拍板 2026-09-26）

统一修法（NEW-2 同款）：`unwrap_or_else(|e| e.into_inner())` → 闭包内 `eprintln!("[mutex_poisoned] <site>: {e:?}")` 后 `into_inner()`。恢复语义不变。锁名映射：bot_skills/{runtime,state,scheduler}→skill_runs；bot_slash×7→confirms；bot_scheduler:93→SchedGuard running / :828→tests sched_running（测试）；bot_chat:531→ChatGuard running / :544→chat_running / :1179,:1250,:1300→DB_WRITE_LOCK；exec_steps×3→pending_map。

## 测试

- 无新测试：纯机械日志；grep 验证 bot 域 7 文件 silent 形态归零 + test-all 担保

## 红线

- 只改 24 处；恢复语义不变；新日志串不引用审计批次号

## spec 起草后自查三条

1. expected_files 7（全路径已列）
2. budget +110/-30（24×+4/-1 + fmt 余量）
3. 无签名/调用链变化；assertions_min 全 0

## 自主执行规则 / Stop 条件

同 NEW-2（全自动、stop 五条触发即停）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "NEW-2b-1",
  "family": "poisoned-silent-recovery",
  "expected_files": [
    "src-tauri/src/bot_skills/runtime.rs",
    "src-tauri/src/bot_skills/state.rs",
    "src-tauri/src/bot_skills/scheduler.rs",
    "src-tauri/src/bot_slash.rs",
    "src-tauri/src/bot_scheduler.rs",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/exec_steps.rs"
  ],
  "max_lines_added": 110,
  "max_lines_removed": 30,
  "findings": [
    {"id": "NEW-2b-1", "file": "src-tauri/src/bot_skills/state.rs", "line": 231, "fix": "bot 域 24 处 silent into_inner 补 eprintln（skill_runs/confirms/running/pending_map 各按锁名，NEW-2 同形态）；ripple：无"}
  ],
  "assertions_min": {
    "src-tauri/src/bot_skills/state.rs": 0,
    "src-tauri/src/bot_skills/runtime.rs": 0,
    "src-tauri/src/bot_skills/scheduler.rs": 0,
    "src-tauri/src/bot_slash.rs": 0,
    "src-tauri/src/bot_scheduler.rs": 0,
    "src-tauri/src/bot_chat.rs": 0,
    "src-tauri/src/exec_steps.rs": 0
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 3
  },
  "stop_conditions": ["compile_failure", "architecture_blocker", "family_heterogeneity", "new_high_different_root", "gate_fail"]
}
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/NEW-2b-1.spec.md python3 scripts/batch-verify.py docs/batches/NEW-2b-1.spec.md
```
