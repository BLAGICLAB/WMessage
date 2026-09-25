# Batch Spec: HANDLERS

## 目的

C5-AP-06 OCR r1 遗留的 handlers.rs:342/496/584 silent-discard 收口：`after_change` 内部 `store.notify_change` 失败时 Err 被吞，调用方不知道。本批走 after_change 内部 log 化（不传播、不改签名），让运维 stderr 可见。

## 人类可读摘要

- family: error-visible-non-blocking（与 C5-AP-06 一致；命名与 PHASE2-TRIAGE.md §1 对齐）
- 覆盖 findings: 1（util.rs:90 after_change 内部 log 化）
- 覆盖 findings: 3（handlers.rs:342/496/584 三处 silent-discard，同一根因指向 after_change:95；一处修复覆盖三处显现）
- 预估 diff: 1 file / +3/-1 lines
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 3

## 红线

- family 一致性：本批只含 error-visible-non-blocking，不混入 error-not-propagated
- 0 FP / 0 family 异质
- after_change 签名不变（仍 `()`）；不传播 Err；handler 签名不变（仍 `()`）
- 不改 handlers.rs：3 call site `after_change(...);` 保持不变

## Stop 条件（触发即停，报 reviewer）

- compile_failure
- architecture_blocker
- family_heterogeneity
- new_high_different_root
- gate_fail

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "HANDLERS",
  "family": "error-visible-non-blocking",
  "expected_files": [
    "src-tauri/src/api_handlers/util.rs"
  ],
  "max_lines_added": 5,
  "max_lines_removed": 1,
  "findings": [
    {
      "id": "HANDLERS.0",
      "file": "src-tauri/src/api_handlers/util.rs",
      "line": 95,
      "fix": "after_change body 第 95 行 store.notify_change(op, task); 改 if let Err(e) = store.notify_change(op, task) { super::ratelimit::log_line(log, &format!(\"[after_change] notify_change failed: op={op} err={e}\")); }；后续 log_line + emit_fn 不变；签名仍 ()，3 handler 调用点不变"
    }
  ],
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

## 签名核（执行前已验）

`src-tauri/src/api_handlers/util.rs:90-100` 当前 after_change：
```
pub(crate) fn after_change(
    store: &Arc<dyn TaskStore>,
    task: &db::Task,
    op: &str,
    emit_fn: &Option<Arc<dyn Fn(&db::Task) + Send + Sync>>,
    log: &Option<PathBuf>,
) {
    store.notify_change(op, task);  // 改这行（HANDLERS.0）
    super::ratelimit::log_line(log, &change_log_line(op, task));
    if let Some(f) = emit_fn {
        f(task);
    }
}
```

3 handler 签名 + call site 全部不变：
- `handlers.rs:188 fn create_task(mut req, store, emit_fn, log) -> ()`；line 342 `after_change(store, &task, "created", emit_fn, log);`
- `handlers.rs:346 fn update_task(mut req, store, id, emit_fn, log) -> ()`；line 496 `after_change(store, &t, "updated", emit_fn, log);`
- `handlers.rs:546 fn delete_task(req, store, id, emit_fn, log) -> ()`；line 584 `after_change(store, &t, "deleted", emit_fn, log);`

## 提交信息骨架

```
fix(handlers): HANDLERS — after_change 内部 log 化（error-visible-non-blocking family）

【family】error-visible-non-blocking（与 C5-AP-06 一致，命名与 PHASE2-TRIAGE.md §1 对齐）

【实修 1 处】
- util.rs:95（HANDLERS.0）：
  store.notify_change(op, task); 改 if let Err(e) = ... { log_line("[after_change] notify_change failed: op={op} err={e}") }
  后续 log_line + emit_fn 不变；签名仍 ()；3 handler 调用点不变

【自测】cargo fmt / cargo check --tests / scripts/test-all.sh 全绿
【OCR】r1：exit ? / ? comments / ?s（期望 ≤ 3）
【family】error-visible-non-blocking（成员 +1；与 C5-AP-06 同 family）
【红线】after_change 签名不变 / handler 签名不变 / 3 call site 不变
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/HANDLERS.spec.md python3 scripts/batch-verify.py docs/batches/HANDLERS.spec.md
```