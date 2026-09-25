# Batch Spec: FUP-1

## 目的

落地四项修法明确的小挂起 follow-up（用户 2026-09-26 拍板「未执行完的按 SOP 执行」）：APW-02b-OCR-3（断言恒真）、APW-02b-OCR-5（staging 清理 4 组重复提 helper）、FRDV-01-OCR-1（DomainRule domain 值统一）、MI-04b-OCR-3（守卫等待双魔数抽 const）。

## 人类可读摘要

- family: misc-pending-followups（4 项独立改动，如实声明）
- 覆盖: 4 文件 / +35/-20
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 3

## 逐条处置（原文均已核）

**① db/mod.rs:538（APW-02b-OCR-3，low）**：`warns.is_empty() || warns.iter().any(...)` 恒真断言。**修法**：该测试（copy_legacy_db_writes_three_files_returns_warns）的 BUSY 场景确定性产生 checkpoint warn → 改精确断言 `!warns.is_empty()` + `any(contains("checkpoint"))`。
**② db/paths.rs:118-141（APW-02b-OCR-5，low）**：4 组 3 行 remove_file 清理近似重复。**修法**：提 `fn remove_staging(tmp_main, tmp_wal, tmp_shm)` helper，4 组改调（2b 组额外的 wal_sidecar 回滚行保留原位）。
**③ migration/rules.rs:177/:183/:197（FRDV-01-OCR-1，low×3 同根）**：行级错误 `domain: "csv"` 与模块域 "migration" 不一致。**修法**：三处统一 `domain: "migration"`。前端消费核验：grep src/ 无 domain 字段消费（前端按 error code 分流），安全。
**④ migration/run.rs:474-482（MI-04b-OCR-3，low）**：守卫等待 `0..60` + `from_secs(5)` 双魔数。**修法**：抽 `const GUARD_WAIT_ATTEMPTS: u32 = 60;` + `const GUARD_WAIT_STEP_SECS: u64 = 5;`。

## 测试

- 无新测试：①为既有测试断言收紧（确定性场景）；②为纯重构（既有 4 条 copy_legacy_db 测试走全部分支）；③④为常量/字面值变更

## 红线

- family 如实声明：4 项独立改动
- 不动 error type 结构（只改 domain 字面值）；不动 guard 状态机语义（只抽常量）
- 新注释不引用审计批次号

## spec 起草后自查三条

1. expected_files：db/mod.rs + db/paths.rs + migration/rules.rs + migration/run.rs = 4
2. budget：①+2/-4 ②helper +8/-12 ③+3/-3 ④+4/-2 ≈ +17/-21，上限 +40/-35；无新文件
3. fix 字段 ripple：无（全部文件内闭环）

## 自主执行规则 / Stop 条件

同 NEW-2（全自动、五条触发即停）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "FUP-1",
  "family": "misc-pending-followups",
  "expected_files": [
    "src-tauri/src/db/mod.rs",
    "src-tauri/src/db/paths.rs",
    "src-tauri/src/migration/rules.rs",
    "src-tauri/src/migration/run.rs"
  ],
  "max_lines_added": 40,
  "max_lines_removed": 35,
  "findings": [
    {"id": "APW-02b-OCR-3", "file": "src-tauri/src/db/mod.rs", "line": 538, "fix": "恒真断言收紧为 !is_empty + any(checkpoint)（确定性 BUSY 场景）；ripple：无"},
    {"id": "APW-02b-OCR-5", "file": "src-tauri/src/db/paths.rs", "line": 118, "fix": "4 组 3 行 remove_file 清理提 remove_staging helper（2b 组额外回滚行保留）；ripple：无"},
    {"id": "FRDV-01-OCR-1", "file": "src-tauri/src/migration/rules.rs", "line": 177, "fix": "行级错误 domain csv → migration ×3（前端无 domain 消费，grep 已核）；ripple：无"},
    {"id": "MI-04b-OCR-3", "file": "src-tauri/src/migration/run.rs", "line": 474, "fix": "守卫等待 60×5s 抽 GUARD_WAIT_ATTEMPTS/GUARD_WAIT_STEP_SECS const；ripple：无"}
  ],
  "assertions_min": {
    "src-tauri/src/db/mod.rs": 0,
    "src-tauri/src/db/paths.rs": 0,
    "src-tauri/src/migration/rules.rs": 0,
    "src-tauri/src/migration/run.rs": 0
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
BATCH_SPEC=docs/batches/FUP-1.spec.md python3 scripts/batch-verify.py docs/batches/FUP-1.spec.md
```
