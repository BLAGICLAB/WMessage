# Batch Spec: P3-SEC-1

## 目的

Phase 3 首批：medium security 类 25 条中 5 条清晰修复（其余 20 条按红线/设计/FP skip 登记于 docs/PHASE3-MEDIUM-TRIAGE.md）。

## 人类可读摘要

- family: medium-security-fixes
- 覆盖: security 5 修 + 20 skip 登记
- 预估 diff: 5 文件 / +55/-8
- OCR 计划: r1（安全批必跑）, timeout 1800s, 期望 comments ≤ 3

## 逐条处置（用户授权自决 2026-09-26）

**① .gitignore**：明文凭据 sibling 防御模式补齐（bot-*-key.txt / secrets.json / *.pem.key）。
**② api_handlers/body.rs**：Transfer-Encoding 出现即 Malformed(400)——CL 预拒对 TE 请求无效（RFC 7230 §3.3.3 歧义优先拒绝）。
**③ ocr.rs**：scheme 前缀比较小写归一（RFC 3986 §3.1，`HTTP://` 原可绕过隐私红线拒绝）。
**④ db/paths.rs escape_for_log_inline**：与 audit::escape_for_log（NEW-1345 后全集）字符级对齐；不直接复用避免 db→bot/config 循环依赖。
**⑤ py/runtime.rs setrlimit**：返回值检查（≠0 → spawn 失败）——原静默吞失败 = 限额未生效时放行无上限子进程。

**Skip 20 条**（详见 docs/PHASE3-MEDIUM-TRIAGE.md）。

## 测试

- 既有 kv_value_pipe parity 测试担保 ④；test-all 全量担保

## 红线

- 权限模型 / capabilities / schema / 产品参数不动；API 响应形状不变

## spec 起草后自查三条

1. expected_files 5（全路径已列）
2. budget +60/-10
3. fix 字段：均为文件内闭环

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "P3-SEC-1",
  "family": "medium-security-fixes",
  "expected_files": [
    ".gitignore",
    "src-tauri/src/api_handlers/body.rs",
    "src-tauri/src/ocr.rs",
    "src-tauri/src/db/paths.rs",
    "src-tauri/src/py/runtime.rs"
  ],
  "max_lines_added": 60,
  "max_lines_removed": 10,
  "findings": [
    {"id": "P3S-0", "file": ".gitignore", "line": 66, "fix": "明文凭据 sibling 模式补齐"},
    {"id": "P3S-2", "file": "src-tauri/src/api_handlers/body.rs", "line": 46, "fix": "Transfer-Encoding 出现即 Malformed(400)"},
    {"id": "P3S-16", "file": "src-tauri/src/ocr.rs", "line": 37, "fix": "scheme 前缀小写归一比较"},
    {"id": "P3S-17", "file": "src-tauri/src/db/paths.rs", "line": 330, "fix": "escape_for_log_inline 与 audit 全集字符级对齐（parity 测试担保）"},
    {"id": "P3S-20", "file": "src-tauri/src/py/runtime.rs", "line": 462, "fix": "setrlimit 返回值检查，失败即 spawn 失败"}
  ],
  "assertions_min": {
    "src-tauri/src/db/paths.rs": 0
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
BATCH_SPEC=docs/batches/P3-SEC-1.spec.md python3 scripts/batch-verify.py docs/batches/P3-SEC-1.spec.md
```
