# Batch Spec: NEW-2b-2

## 目的

落地 PHASE2-TRIAGE-NEW-2b（非 bot 域部分）：memory / evolution / api_handlers / py / db 共 21 处同形 silent `into_inner` 补 C3-1 留痕（NEW-2 同款机械形态）。

## 人类可读摘要

- family: poisoned-silent-recovery
- 覆盖: 21 处 / 6 文件（memory 8 + evolution 5 + py 4 + api_handlers 3 + db 1，全为生产代码）
- 预估 diff: 6 文件 / +84/-43（执行中校正 ×1：rustfmt 重排实测）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 3

## 逐条处置（用户拍板 2026-09-26）

统一修法（NEW-2 同款）。锁名映射：memory/{mod,consolidate}×8→DB_WRITE_LOCK；evolution/apply×2→DB_WRITE_LOCK；evolution/emit×3→emitted_map；api_handlers/handlers×3→API_RMW_LOCK；py/runtime×3→PY_CHILDREN；py/audit→BOT_LOG_LOCK；db/mod.rs:67→LEGACY_COPY_LOCK。

## 测试

- 无新测试：纯机械日志；grep 验证全仓 silent 形态归零 + test-all 担保

## 红线

- 只改 21 处；恢复语义不变；新日志串不引用审计批次号

## spec 起草后自查三条

1. expected_files 6（全路径已列）
2. budget +95/-25
3. 无签名/调用链变化；assertions_min 全 0

## 自主执行规则 / Stop 条件

同 NEW-2（全自动、stop 五条触发即停）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "NEW-2b-2",
  "family": "poisoned-silent-recovery",
  "expected_files": [
    "src-tauri/src/memory/mod.rs",
    "src-tauri/src/memory/consolidate.rs",
    "src-tauri/src/evolution/apply.rs",
    "src-tauri/src/evolution/emit.rs",
    "src-tauri/src/api_handlers/handlers.rs",
    "src-tauri/src/py/runtime.rs",
    "src-tauri/src/py/audit.rs",
    "src-tauri/src/db/mod.rs"
  ],
  "max_lines_added": 95,
  "max_lines_removed": 50,
  "findings": [
    {"id": "NEW-2b-2", "file": "src-tauri/src/memory/mod.rs", "line": 178, "fix": "非 bot 域 21 处 silent into_inner 补 eprintln（DB_WRITE_LOCK/emitted_map/API_RMW_LOCK/PY_CHILDREN/BOT_LOG_LOCK/LEGACY_COPY_LOCK，NEW-2 同形态）；ripple：无"}
  ],
  "assertions_min": {
    "src-tauri/src/memory/mod.rs": 0,
    "src-tauri/src/memory/consolidate.rs": 0,
    "src-tauri/src/evolution/apply.rs": 0,
    "src-tauri/src/evolution/emit.rs": 0,
    "src-tauri/src/api_handlers/handlers.rs": 0,
    "src-tauri/src/py/runtime.rs": 0,
    "src-tauri/src/py/audit.rs": 0,
    "src-tauri/src/db/mod.rs": 0
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
BATCH_SPEC=docs/batches/NEW-2b-2.spec.md python3 scripts/batch-verify.py docs/batches/NEW-2b-2.spec.md
```
