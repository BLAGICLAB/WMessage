# Batch Spec: PR5-MCP-DUPKEY

## 目的

OCR NEEDS-HUMAN F62：MCP env/headers 原始键 trim 后重名时 normalize 会静默合并
丢配置——按拍板「拒绝整个保存」在 save 入口 normalize 之前加前置查重闸。

## 人类可读摘要

- family: ocr-needs-mcp-dupkey
- 预估 diff: 3 files / +68/-0
- 测试：trim_colliding_raw_keys_are_rejected_with_both_keys_listed（先红后绿）

## 红线

- normalize 本身无失败路径，不改签名（spec 的 normalize->Result 分支不需要）
- 大小写不同的键不算重名（normalize 只 trim 不折叠）
- 拍板记录：冲突拒绝整个保存；错误列出原始键与规范化键

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "PR5-MCP-DUPKEY",
  "family": "ocr-needs-mcp-dupkey",
  "expected_files": [
    "docs/batches/PR5-MCP-DUPKEY.spec.md",
    "src-tauri/src/bot/mcp/commands.rs",
    "src-tauri/src/bot/mcp/config.rs"
  ],
  "max_lines_added": 118,
  "max_lines_removed": 50,
  "max_new_files_lines": 400,
  "findings": [
    {
      "id": "F62",
      "file": "src-tauri/src/bot/mcp/config.rs",
      "line": 234,
      "fix": "find_trim_collisions 前置闸：env/headers 原始键 trim 后同键即拒绝整个保存（拍板=拒绝，错误列原始键+规范化键）"
    }
  ],
  "assertions_min": {},
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 0
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
