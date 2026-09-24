# BT-09b spec — dispatch.rs:248 重复查找收口 + :424 wontfix 登记（C5-BT-09 拆批 2/3+3/3）

## 目标（1 实修 + 1 登记）

1. **dispatch.rs:248**（实修）：:234 已 `tools_index().get(name)` O(1) 查到
   `&'static ToolDef`（含 max_output_chars），post-execute 却又
   `TOOLS_TABLE.iter().find(|t| t.name == name)` 线性扫——每次调用两次 name 查找。
   修：:234 的 lookup 结果存入 `tool_def` 复用；:248 改 `if let Some(t) = tool_def`；
   `TOOLS_TABLE` 从 dispatch.rs 的 use 移除（仅此一处用，registry 单源真相不变）。
2. **dispatch.rs:424**（wontfix-with-rationale 登记，零代码）：commit_and_report
   失败走 `ToolResult::ok` 是 dispatch.rs:402-404 注释钉死的设计意图（防 LLM
   severity classifier 把「完成任务失败：DB error」误判 fatal），与
   PHASE2-TRIAGE-OCR-003（tools.rs:160 同型）同处理。OCR 建议的 ToolResult::warn =
   状态语义变更，属 B 类方向，但有 OCR-003 先例判定在先——登记 wontfix，
   若产品日后加 ToolStatus::Fail 变体再一并改 handler 层。

## 行为变更

零（同一 ToolDef 同一 max_output_chars，查找方式从线性变索引复用）。

## spec 起草后自查三条

1. `expected_files` → `src-tauri/src/bot/dispatch.rs`（单文件）。✓
2. budget → A 类：+4/-3 → budget +6/-5。✓
3. findings fix 字段列 ripple → 无（dispatch 内部；TOOLS_TABLE 仍 pub 于 registry，
   其它使用方不动）。✓

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
  "batch_id": "BT-09b",
  "family": "redundant-lookup",
  "expected_files": [
    "src-tauri/src/bot/dispatch.rs"
  ],
  "max_lines_added": 6,
  "max_lines_removed": 5,
  "findings": [
    {"id": "C5-BT-09.1", "file": "src-tauri/src/bot/dispatch.rs", "line": 248, "fix": ":234 tools_index().get(name).copied() 结果存入 tool_def 复用；:248 改 if let Some(t) = tool_def；use 移除 TOOLS_TABLE。ripple：无（TOOLS_TABLE 仍 pub 于 registry）"},
    {"id": "C5-BT-09.2", "file": "src-tauri/src/bot/dispatch.rs", "line": 424, "fix": "wontfix-with-rationale 登记（与 OCR-003 同型：ToolResult::ok 失败文案是设计意图，dispatch.rs:402-404 注释钉死）。零代码。ripple：无"}
  ],
  "assertions_min": {
    "src-tauri/src/bot/dispatch.rs": 0
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 3
  }
}
```
