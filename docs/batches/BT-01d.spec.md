# BT-01d spec — C5-BT-01b 残余 + OCR-001：tools.rs DB 读失败信号收口（2 项）

## 背景（为什么有残余）

1e023e3（DB-01b-BT-01b，2026-09-23）修了 BT-01b.1（active_tasks 改 Result）+
BT-01b.2a/2b/2c（list/complete/delete 三 call site）。但 C5-BT-01b 第二条 finding 的
站点是 `tool_search_tasks`（triage 标 tools.rs:280，现 :286），不在该批 spec 覆盖内——
今日仍在 `db_load().await.unwrap_or_default()`：DB 读失败 → 空结果假象，与
「搜索无命中」不可区分。OCR 原文建议：mirror `tool_query_single_task` 模式。

PHASE2-TRIAGE-OCR-001（§4 follow-up 已登记，未修）：`resolve_task` 内
`find_task_by_keyword(app, &kw).await?`（:547/:571）把 active_tasks 的 Err(String)
经 `?` 落成 `CommandError::Internal`，同函数 :539-541 自己的 active_tasks 显式
`CommandError::DbError`——同一 DB 故障两条路径两种 code，前端 code 统计漏算。

## 目标 findings（2 项，同 family：error-not-propagated）

1. **tools.rs:286**（C5-BT-01b 残余，high）：tool_search_tasks 吞 DB load 失败返空 list。
   按 §1 判据该条实为信号丢失（只读搜索不破坏数据），family 归 error-not-propagated，
   与 1e023e3 commit 所标 family 一致。
2. **tools.rs:547 + :571**（PHASE2-TRIAGE-OCR-001，medium）：error code 不一致。

## 修法（照既有先例，零新设施）

1. tool_search_tasks：照 tool_query_single_task（:207-210）模式——
   `let Ok(tasks) = crate::db::db_load(app.clone()).await else { return ToolResult::ok("搜索失败：数据库读取错误".to_string(), Vec::new()); };`
   首字「搜」非「失败」开头 → 机械首字符判定 = ok（同 :208 注释理由）。
2. :547 / :571 两处 `.await?` →
   `.await.map_err(|e| CommandError::DbError(format!("按关键词查找失败：{e}")))?`，
   对齐 :539-541 既有 map_err 形态。

## 行为变更

- DB 读失败时 tool_search_tasks 从「返回空结果」变「返回显式错误文案」（fail-visible，
  不写空默认值假象）；正常路径不变。
- find_task_by_keyword 失败路径 error code Internal → DbError（与 resolve_task 主路径
  一致）；错误文案更具体。audit/前端按 code=DB_ERROR 统计口径变准（此前漏算）。

## spec 起草后自查三条

1. `expected_files` → `src-tauri/src/bot/tools.rs`（单文件）。无签名 ripple：
   tool_search_tasks / find_task_by_keyword / resolve_task 签名均不变。✓
2. budget → A 类（行内小改）：search fix +3/-1；两处 map_err +4/-2。合计 ≈+7/-3 →
   budget +12/-6。✓
3. findings fix 字段列 ripple → 均无 ripple（改动全在 tools.rs 内部）。✓

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
  "batch_id": "BT-01d",
  "family": "error-not-propagated",
  "expected_files": [
    "src-tauri/src/bot/tools.rs"
  ],
  "max_lines_added": 12,
  "max_lines_removed": 6,
  "findings": [
    {"id": "C5-BT-01b.R", "file": "src-tauri/src/bot/tools.rs", "line": 286, "fix": "tool_search_tasks 的 db_load().await.unwrap_or_default() → let-else 显式返 ToolResult::ok(\"搜索失败：数据库读取错误\")（照 tool_query_single_task :207-210 模式）。ripple：无"},
    {"id": "PHASE2-TRIAGE-OCR-001", "file": "src-tauri/src/bot/tools.rs", "line": 547, "fix": ":547 与 :571 两处 find_task_by_keyword(app, &kw).await? → .await.map_err(|e| CommandError::DbError(format!(\"按关键词查找失败：{e}\")))?，对齐 resolve_task :539-541 的 DbError 形态。ripple：无"}
  ],
  "assertions_min": {
    "src-tauri/src/bot/tools.rs": 0
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 4
  }
}
```
