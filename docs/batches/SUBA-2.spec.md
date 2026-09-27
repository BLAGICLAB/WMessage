# Batch Spec: SUBA-2

## 目的

用户指令：以 SUBAGENT-ORCHESTRATION-DESIGN-2026-09-27.md 为基线实施 A 期第二批。
SUBA-2 = 设计 §11 A2「工具暴露 + 提示词」：spawn/check/cancel 三工具进 registry +
递归双保险（子 agent 工具清单断言无 spawn + 服务端身份校验）+ 任务包装渲染 +
四提示词资产（docs/prompts/subagent/ 固化为后端常量）+ 主 agent 系统提示接入 +
子 agent runner（独立会话 + 工具循环 + ChatGuard）+ 收尾 JSON 解析落库 +
SSRF 复核补测试。前置：RE1-MP02-LAND 已落地（run_model_loop 现签名）。

## 修法（按层）

1. **提示词资产**：prompts/subagent.rs 常量（MAIN_AGENT_ADDENDUM / SUBAGENT_BASE /
   PROFILE_RESEARCH / PROFILE_CODER / PROFILE_GENERAL / RESULT_SCHEMA_HINT），镜像
   docs/prompts/subagent/*.md 四件；主 agent 段在 bot_chat 装配处追加 Base 槽位
   （同槽 push 顺序 = 拼接顺序，零新 slot）。
2. **工具白名单**（设计 §5.1 → 本仓工具映射，勘误登记：本仓无泛型 write_file/
   read_file，映射为 read_text_file/list_files/grep_files + 新增 write_artifact_file
   仅限产物目录内写）：research = web_search/fetch_url/list_files/read_text_file/
   write_artifact_file/read_own_card；coder = read_text_file/list_files/grep_files/
   run_python/write_artifact_file/read_own_card；general = 并集去重。read_own_card
   为内部只读工具（卡即契约：读自己子卡 note/subtasks/deletedAt）。
3. **registry**：TOOLS_TABLE 尾部追加 spawn_subagent/check_subagent/cancel_subagent/
   write_artifact_file/read_own_card（主可见 32 = 29+3；write_artifact_file/
   read_own_card 为 subagent-only 不进默认 schema）；`tools_json_for(session)` 按
   tool_guard 子 agent 注册表选 profile 白名单 schema；测试更新（baseline 前缀比对）。
4. **递归双保险**：① schema 层——子 agent 会话的 tools 数组按白名单生成，无 spawn；
   ② 服务端——spawn 工具实现内校验调用者身份（子 agent 会话调 spawn → error）+
   dispatch 白名单闸（白名单外工具一律拒，早退事件配平 tool.return）。
5. **runner**（bot_orchestrator）：spawn 异步包装末尾 tokio spawn runner（非阻塞）；
   runner：ExecGuard（子卡防重入）→ 状态 queued→running → 建会话（🧩 标题）+ 包装
   user 消息落库 + set_session/assignee 回填 + register tool_guard ctx + ChatGuard +
   StopGuard(interactive=true 围观流式) → run_model_loop(max_turns=预算轮数) →
   收尾：结果 JSON 解析（最后一个顶层对象）→ 状态映射 → 产物目录文件 bind 回子卡 +
   result 写卡 + 代勾 subtask（结果 subtasks 对账）→ 历史落库 → 审计
   subagent_done/failed + widget 事件。软删 watcher：2s 轮询子卡 deletedAt /
   cancel 状态 → force_stop（设计 §4.3 硬停）。cancel 包装叠加 force_stop 直停。
6. **SSRF**：bot_web 已有禁重定向 + 逐跳公网校验 + DNS 解析校验；本批补边界用例
   （169.254 link-local / 0.0.0.0 / ::1 / fc00::/7 / fe80::/10 显式断言）。

## 红线

- 既有 29 工具 schema 字节不动（baseline 前缀测试锁）；新工具只追加表尾。
- 主 agent 行为零回归：tools_json() 对非子 agent 会话输出 = 29+3，软警告阈值 35
  行为对主会话字节级不变（cap*7/10 公式在 cap=50 时等值）。
- 子 agent 会话事件全带 sessionId（PAR-1 前端按会话隔离）；包装由 orchestrator
  渲染（不接受 LLM 自由拼接 system 段，设计 §10.6）。

## 测试

- registry：32 主可见 + baseline 前缀 + 白名单清单断言（research 无 spawn/任务写；
  general = 并集；read_own_card/write_artifact_file 仅白名单可见）。
- orchestrator：extract_last_json_object（纯文本夹 JSON/多 JSON 取末/解析失败）；
  收尾映射（成功/failed/result_json_unparseable）；包装渲染字段齐；
  spawn 非阻塞集成（mock app，runner 后台失败 → check 见 failed）。
- bot_web：SSRF 边界补测。
- 全量回归 test-all。

## spec 起草后自查三条

1. expected_files 16（2 新代码文件 + 4 提示词镜像 + 10 修改）
   【校正 ×1：补 app_state.rs（subagent_stops 表）/bot_slash.rs（StopToken::stop）】
2. budget：新文件 +280；修改 +1700/-100（OCR r1 采纳后实测 +1621/-45：事务化/
   竞态闸/trace_kv 补全/watcher 只读化/filename 校验等修复行数计入）
   【校正 ×1：补 app_state/bot_slash】【校正 ×2：实测行数】【校正 ×3：OCR r1 采纳】
   【校正 ×4：OCR r2 采纳后实测 +1736/-45】
3. fix 字段：runner 白名单/递归闸集中在 tool_guard+dispatch+registry 三点；其余各自闭环

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "SUBA-2",
  "family": "subagent-orchestration",
  "expected_files": [
    "src-tauri/src/prompts/subagent.rs",
    "src-tauri/src/bot_orchestrator.rs",
    "docs/prompts/subagent/main_agent.md",
    "docs/prompts/subagent/subagent_base.md",
    "docs/prompts/subagent/profiles/research.md",
    "docs/prompts/subagent/profiles/coder.md",
    "src-tauri/src/prompts/mod.rs",
    "src-tauri/src/bot/registry.rs",
    "src-tauri/src/bot_model_loop.rs",
    "src-tauri/src/tool_guard.rs",
    "src-tauri/src/bot/dispatch.rs",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/bot_web.rs",
    "src-tauri/src/app_state.rs",
    "src-tauri/src/bot_slash.rs",
    "docs/rust-bot-architecture.md"
  ],
  "max_lines_added": 1800,
  "max_lines_removed": 100,
  "max_new_files_lines": 320,
  "findings": [
    {"id": "SUBA-2", "file": "src-tauri/src/bot_orchestrator.rs", "line": 420, "fix": "runner：ExecGuard+queued→running+建会话+包装落库+tool_guard 注册+ChatGuard+StopGuard(围观流式)+run_model_loop(预算轮数)+软删 watcher+收尾(JSON 解析/产物 bind/代勾/result 写卡/审计/widget 事件)+cancel 叠加 force_stop"},
    {"id": "SUBA-2", "file": "src-tauri/src/bot/registry.rs", "line": 449, "fix": "三编排工具+write_artifact_file+read_own_card 进表（主可见 32）；tools_json_for 按 profile 白名单出 schema；baseline 前缀比对测试"},
    {"id": "SUBA-2", "file": "src-tauri/src/tool_guard.rs", "line": 45, "fix": "SUBAGENT_SESSIONS 注册表（subagent_id/task_id/profile/artifact_dir）；白名单闸与 spawn 身份校验的数据源"},
    {"id": "SUBA-2", "file": "src-tauri/src/bot/dispatch.rs", "line": 188, "fix": "子 agent 白名单闸：白名单外工具 pre_execute 拒绝 + subagent_progress 事件 + 早退事件配平"},
    {"id": "SUBA-2", "file": "src-tauri/src/bot_model_loop.rs", "line": 520, "fix": "tools 按会话选取（tools_json_for）+ 熔断上限按会话预算（cap*7/10 软警，主会话 50/35 行为不变）"}
  ],
  "assertions_min": {
    "src-tauri/src/bot_orchestrator.rs": 20,
    "src-tauri/src/bot/registry.rs": 25
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 600,
    "expected_max_comments": 8
  },
  "stop_conditions": ["compile_failure", "architecture_blocker", "family_heterogeneity", "new_high_different_root", "gate_fail"]
}
```

## 验证命令

```
BATCH_SPEC=docs/batches/SUBA-2.spec.md python3 scripts/batch-verify.py docs/batches/SUBA-2.spec.md
cargo fmt --check && cargo check && npx tsc --noEmit && bash scripts/test-all.sh
```
