# Batch Spec: SUBA-1

## 目的

用户指令：以 docs/SUBAGENT-ORCHESTRATION-DESIGN-2026-09-27.md（方案 B 定稿）为开发基线实施 A 期第一批。
SUBA-1 = 设计 §11 A1「orchestrator 核心（服务端，不暴露 LLM 工具）」：
`subagents` 表 + 生命周期状态机 + spawn/check/cancel 内部实现；子卡创建（一子 agent 一子卡）
+ parent_task_id 关联 + acceptance 双写（子卡 note 人可见侧）；审计事件 subagent_spawned；
预算/并发字段落表。A2（工具暴露+提示词+runner）与 A3（预算强制+并发排队+前端）后续批实施。

## 修法（按层）

1. **`subagents` 表**（db/subagents.rs 新建，模板 skill_out.rs）：SubagentRow 对齐设计 §3
   字段（id/status/profile/model/parent_session_id/task_id/objective/budget_json/result_json/
   error/created_at/started_at/finished_at）+ 增列 session_id（子 agent 会话串链）、trace_id
   （贯穿日志）、acceptance_json（双写机器侧）。状态枚举六态 +
   `transition_allowed` 合法转移表：queued→running|cancelled；running→succeeded|failed|
   cancelled|budget_exceeded；终态一律禁移。
2. **orchestrator 核心**（bot_orchestrator.rs 新建）：spawn/check/cancel 三段式——
   纯 DB 核心 `_locked`（in-memory conn 可单测，同 task_patch_locked 先例）+ 异步包装
   （spawn_blocking + DB_WRITE_LOCK + 审计 + broadcast）。spawn：校验（objective/验收标准
   非空、预算钳制 turns≤50 硬顶）→ 缺 parent 卡 TaskNotFound → 建子卡（🧩 前缀标题、
   note=验收标准双写、column=doing、budget 上卡）→ 插入 subagents 行（status=queued）→
   audit `subagent_spawned`（结构化 + 文本行，带 subagent_id/task_id/parent_session_id/
   profile/trace_id）。check：按 subagent_id 或 task_id 双键解析（幂等，返回
   status/progress/result/error）；wait_ms 上限 5000 轮询在 A2 接 runner 后生效。cancel：
   queued/running → cancelled（error=原因、finished_at 打戳）；终态 no-op 带标记；
   未知 id InvalidArgument。
3. **任务卡三字段**（tasks.rs + db/mod.rs，走 TP-2 task_patch 既有通道）：Task 增
   assignee（Option<String>）/budget（Option<SubagentBudget>）/result（Option<JSON Value>）
   —— DB 列 assignee/budget/result TEXT（JSON 序列化），upsert/load_all 全链路补齐，
   apply_task_patch 白名单三臂（null=清空），open_db 幂等 ALTER，3 处测试建表同步。
   设计 §10.5「子 agent 任务卡主状态只读」由白名单机制天然满足：子 agent 工具白名单
   （A2）不含任何任务卡写工具，task_patch 非 LLM 工具；服务端代勾走内部写路径。

## 红线

- 零 LLM 行为变更：不注册 tauri 命令、不进 registry TOOLS_TABLE、不改 SYSTEM_PROMPT
  （A2 才暴露）；主仓 29 工具 schema/baseline 测试不动。
- RMW 契约：所有写路径持 DB_WRITE_LOCK，upsert_tasks 的 debug_assert 契约保持。
- 既有 tasks 表迁移幂等（ALTER 前查 PRAGMA table_info），老库无损升级。
- 子卡不设 bot_assigned（那是 🤖 执行语义）；执行防重入（ExecGuard）A2 接 runner 时上。

## 测试

- db/subagents.rs：转移矩阵全合法 + 非法拒绝；insert/load 往返；update_status 打戳
  started/finished；find_by_task。
- bot_orchestrator.rs：spawn 建子卡+行+双写 note；缺 parent 卡 TaskNotFound；
  objective/验收标准空串拒绝；预算钳制 50 硬顶；cancel 置 cancelled + 终态 no-op +
  未知 id InvalidArgument；check 双键 + 进度小计 + 未知 id InvalidArgument。
- db/tasks.rs：task_patch 三新键臂（设值/null 清空/未知键仍拒）。
- 既有全量回归（cargo nextest + vitest + test-all）。

## spec 起草后自查三条

1. expected_files 5（2 新建 + 3 修改，含架构文档模块树登记——audit_module_map.py 门禁）
2. budget：新文件 +850（两文件约 700 行含测试）；修改 +130/-15
3. fix 字段：全部新建模块内闭环；tasks.rs 仅追加字段/列/patch 臂（既有路径零语义变更）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "SUBA-1",
  "family": "subagent-orchestration",
  "expected_files": [
    "src-tauri/src/db/subagents.rs",
    "src-tauri/src/bot_orchestrator.rs",
    "src-tauri/src/db/mod.rs",
    "src-tauri/src/db/tasks.rs",
    "docs/rust-bot-architecture.md"
  ],
  "max_lines_added": 150,
  "max_lines_removed": 20,
  "max_new_files_lines": 900,
  "findings": [
    {"id": "SUBA-1", "file": "src-tauri/src/bot_orchestrator.rs", "line": 1, "fix": "orchestrator 核心：spawn/check/cancel 纯 DB 核心（_locked，单测锚点）+ 异步包装（spawn_blocking+DB_WRITE_LOCK）；spawn=校验+预算钳制+建子卡（🧩/note 双写验收/column=doing/budget 上卡）+插 queued 行+audit subagent_spawned；check 双键幂等；cancel queued/running→cancelled、终态 no-op；缺 parent 卡 TaskNotFound"},
    {"id": "SUBA-1", "file": "src-tauri/src/db/subagents.rs", "line": 1, "fix": "subagents 表持久化：六态状态枚举+transition_allowed 合法转移表（queued→running|cancelled；running→succeeded|failed|cancelled|budget_exceeded；终态禁移）+insert/load/find_by_task/update_status（打 started/finished 戳）"},
    {"id": "SUBA-1", "file": "src-tauri/src/db/mod.rs", "line": 105, "fix": "open_db 建表批新增 subagents 表（设计 §3 列 + session_id/trace_id/acceptance_json 增列）+ tasks 三新列幂等 ALTER（assignee/budget/result TEXT）"},
    {"id": "SUBA-1", "file": "src-tauri/src/db/tasks.rs", "line": 89, "fix": "Task 增 assignee/budget/result 三字段：struct+upsert SQL+load_all+apply_task_patch 白名单三臂（null=清空）+3 处测试建表同步；DB 存 JSON TEXT"}
  ],
  "assertions_min": {
    "src-tauri/src/bot_orchestrator.rs": 16,
    "src-tauri/src/db/subagents.rs": 8
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 600,
    "expected_max_comments": 6
  },
  "stop_conditions": ["compile_failure", "architecture_blocker", "family_heterogeneity", "new_high_different_root", "gate_fail"]
}
```

## 验证命令

```
BATCH_SPEC=docs/batches/SUBA-1.spec.md python3 scripts/batch-verify.py docs/batches/SUBA-1.spec.md
cd src-tauri && cargo fmt && cargo clippy-check: cargo check --tests
npx --no-install tsc --noEmit -p tsconfig.json
bash scripts/test-all.sh
```
