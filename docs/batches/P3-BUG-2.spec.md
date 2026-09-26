# Batch Spec: P3-BUG-2

## 目的

Phase 3 bug 第二批：6 条用户可触发 panic/行为错（B49/B50/B56/B57/B60；B86 判 stale）。其余 141 条 bug 登记分拣账本（DEFER/SKIP）。

## 人类可读摘要

- family: medium-bug-fixes-r2
- 覆盖: bug 5 修 + B86 stale + 141 条登记
- 预估 diff: 5 文件 / +40/-8
- OCR 计划: 跳过（parse/展示截断类，gate+测试担保；用户指令省 token）

## 逐条处置（用户授权自决 2026-09-26）

**① parse.rs:74（B49）**：YAML scalar 单引号与双引号同权剥离（`'foo-bar'` 原带引号进值）。
**② parse.rs:319（B50）**：步骤编号补连续性检查（1 起连续递增，空洞静默落空 ${stepN.*} 引用）。
**③ bot_slash.rs:322/:404/:432（B56/57）**：字节切片 `&s[..8]` 对多字节字符 panic → `chars().take(8)`（:322 顺带补丢失的逗号——前次 sed 语法错，本批自愈）。
**④ db/bot_sessions.rs:129（B60）**：bot_session_rename 检查受影响行数，0 行返 InvalidArgument（原静默 Ok）。
**stale ⑤ B86**：synthetic window_days as u32 modulo-by-zero——validate() 已在 generate 入口拒 <1（EV-3b-F 落地），消费点不可达。

## 测试

- bot_slash / bot_sessions / parse 既有 29 测试 + test-all 担保

## 红线

- 不动 parser 语法面（仅值预处理与校验增严）；错误变体用既有 InvalidArgument

## spec 起草后自查三条

1. expected_files 5（全路径已列）
2. budget +45/-10
3. fix 字段：全部函数内闭环

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "P3-BUG-2",
  "family": "medium-bug-fixes-r2",
  "expected_files": [
    "src-tauri/src/bot_skills/parse.rs",
    "src-tauri/src/bot_slash.rs",
    "src-tauri/src/db/bot_sessions.rs"
  ],
  "max_lines_added": 45,
  "max_lines_removed": 10,
  "findings": [
    {"id": "B49", "file": "src-tauri/src/bot_skills/parse.rs", "line": 74, "fix": "YAML scalar 单引号剥离（trim_matches 双臂）"},
    {"id": "B50", "file": "src-tauri/src/bot_skills/parse.rs", "line": 320, "fix": "步骤编号连续性检查（1 起递增，空洞 Err）"},
    {"id": "B56", "file": "src-tauri/src/bot_slash.rs", "line": 404, "fix": "字节切片 → chars().take(8) ×2 + :322 逗号修复"},
    {"id": "B57", "file": "src-tauri/src/bot_slash.rs", "line": 322, "fix": "同上（含在 B56）"},
    {"id": "B60", "file": "src-tauri/src/db/bot_sessions.rs", "line": 129, "fix": "rename 检查 rows==0 → InvalidArgument"}
  ],
  "assertions_min": {},
  "ocr_plan": {
    "rounds": 0,
    "timeout_seconds": 0,
    "expected_max_comments": 0
  },
  "stop_conditions": ["compile_failure", "architecture_blocker", "family_heterogeneity", "new_high_different_root", "gate_fail"]
}
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/P3-BUG-2.spec.md python3 scripts/batch-verify.py docs/batches/P3-BUG-2.spec.md
```
