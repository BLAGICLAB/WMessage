# Batch Spec: P3-BUG-1

## 目的

Phase 3 bug 类首批：medium bug 190 条中 6 条清晰低风险修复（B20/B24/B27/B29/B40/B42），其余登记 docs/PHASE3-MEDIUM-TRIAGE.md。

## 人类可读摘要

- family: medium-bug-fixes-r1
- 覆盖: bug 6 修
- 预估 diff: 6 文件 / +71/-20（执行中校正 ×1：ratelimit match 重排实测）
- OCR 计划: r1（含 API 数据完整性项）, timeout 1800s, 期望 comments ≤ 3

## 逐条处置（用户授权自决 2026-09-26）

**① handlers.rs:39（B20）**：json_ok 序列化失败静默 2xx+`{}` → 显式 500 + 错误体 + eprintln。
**② ratelimit.rs:47（B24）**：访问日志 open/writeln 双静默 → match + eprintln（含路径）。
**③ util.rs now_ms（B27）**：时钟异常 unwrap_or(0) → 留痕后取 0（行为不变）。
**④ bin/eval_run.rs（B29）**：flag 值吃 flag（`--config --db`）→ arg_value helper 拒绝 `--` 开头值 ×4 flag。
**⑤ bot_chat.rs:1432（B40）**：register_exec_session 移到 ChatGuard::acquire 之后——acquire 失败早退不再泄漏登记。
**⑥ bot_model_loop.rs:680/:692（B42）**：LLM 重试 sleep 前响应 stop.stopped()（镜像 :571 主循环口径）×2。

## 测试

- test-all 全量担保（B40/B42 行为路径既有测试覆盖；B20 新 500 分支编译担保）

## 红线

- 不动 API 契约形状（新增 500 仅在原静默错误路径）；不改 StopGuard 语义

## spec 起草后自查三条

1. expected_files 6（全路径已列）
2. budget +75/-15
3. fix 字段：均为函数内闭环

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "P3-BUG-1",
  "family": "medium-bug-fixes-r1",
  "expected_files": [
    "src-tauri/src/api_handlers/handlers.rs",
    "src-tauri/src/api_handlers/ratelimit.rs",
    "src-tauri/src/api_handlers/util.rs",
    "src-tauri/src/bin/eval_run.rs",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/bot_model_loop.rs"
  ],
  "max_lines_added": 75,
  "max_lines_removed": 25,
  "findings": [
    {"id": "B20", "file": "src-tauri/src/api_handlers/handlers.rs", "line": 39, "fix": "序列化失败显式 500 + 错误体（原静默 2xx+{}）；ripple：无"},
    {"id": "B24", "file": "src-tauri/src/api_handlers/ratelimit.rs", "line": 47, "fix": "访问日志 open/writeln 失败 eprintln 可见化；ripple：无"},
    {"id": "B27", "file": "src-tauri/src/api_handlers/util.rs", "line": 16, "fix": "now_ms 时钟异常留痕后取 0；ripple：无"},
    {"id": "B29", "file": "src-tauri/src/bin/eval_run.rs", "line": 27, "fix": "arg_value helper 拒绝 flag 值吃 flag ×4；ripple：无"},
    {"id": "B40", "file": "src-tauri/src/bot_chat.rs", "line": 1432, "fix": "register_exec_session 移到 ChatGuard 成功后（早退不泄漏登记）；ripple：无"},
    {"id": "B42", "file": "src-tauri/src/bot_model_loop.rs", "line": 680, "fix": "重试 sleep 前 stop.stopped() 早退 ×2（镜像 :571 口径）；ripple：无"}
  ],
  "assertions_min": {},
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
BATCH_SPEC=docs/batches/P3-BUG-1.spec.md python3 scripts/batch-verify.py docs/batches/P3-BUG-1.spec.md
```
