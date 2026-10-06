# Agent 能力透明化改造方案（2026-10-06）

> 目标（老板原话拆解）：
> 1. **参数透明**——设置里各模块增加自定义选项，agent 参数更透明可调试。
> 2. **执行透明**——任务卡 / 工作流 / 定时任务执行不再是黑盒：调用了什么工具、改了什么文件、diff 可见。
>
> 本文先给现状调研结论（§1），再给总体设计（§2）、两大目标的分项设计（§3/§4）、前端改造（§5）、
> 分期路线（§6）与风险约束（§7）。现状引用均为 2026-10-06 工作区快照的 file:line。

---

## 1. 现状调研

### 1.1 核心结论：可观测信息分三路，互不相通

| 路径 | 现有内容 | 前端可见性 |
|---|---|---|
| **Tauri 事件**（只 emit 到 widget 窗口，bot_model_loop.rs:553-560） | `bot-chat-delta`/`bot-think-delta`（流式文本）、`bot-tool`/`bot-tool-name`/`bot-tool-done`（工具名+入参）、`chat-open-session`、`subagent-finished` | 聊天流有；但 `bot-tool-done` payload **只有 args，无结果/耗时/成败**（bot_model_loop.rs:1349-1352），前端 `ToolCall` 类型也只有 `{id,name,args,done}`（ChatPanel/types.ts:6） |
| **bot.log 审计**（audit.rs，纯文件） | `tool.call`（args 截 80 字，dispatch.rs:176）、`tool.return`（结果截 80 字+耗时+refs，dispatch.rs:349-384）、`llm.usage`、`sched_*`、`workflow_run*`、`bot_fs.edit_file`（路径+行数+匹配级别，bot_fs.rs:694-704） | 只有设置页「机器人审计日志」读尾部 200 行**纯文本**（SettingsPage.tsx:644-656）；审计不 emit 任何事件 |
| **DB / 轮询** | 任务行变更 `tasks-updated` 行级广播；工作流运行态靠前端 5s 轮询 `workflow_is_running`（WorkflowPage.tsx:538-558）；定时任务历史=⏰ 前缀会话人工找（bot_scheduler.rs:19-20） | 无节点级事件、无执行日志、无历史关联跳转 |

### 1.2 diff 缺口的根因（目标 2 的关键）

- `tool_edit_file`（bot_fs.rs:634-725）：三级匹配内核 `try_apply_edit`（:514-592）**手里有替换前后全文**，但写回后旧内容即丢弃——不留备份、不算 diff、不 emit 事件；审计只有「路径(截200) | exact\|ws | +N 行」一行（:694-704）。
- `tool_write_file`（bot_fs.rs:728-800）：覆盖已存在文件有确认流程，同样不留 before。
- 聊天里 `FileSummary` 是**从消息正文正则抽文件路径**的近似（MessageList.tsx:218-260），代码注释明言「+/- 行级统计当前事件面无数据源，不接假数据」（:215-217）。

### 1.3 参数面现状（目标 1 的关键）

- **已有配置但无 UI**（现成挂载点）：`memory_tuning` 8 参数（types.rs:181，注释明示「本期无设置页 UI——手改 bot-config.json 生效」）、全局 `max_tokens`（SettingsPage.tsx:420-422 注释明示无入口）、全局 `reasoning_effort`（只有数据通道，实际档位 UI 在挂件聊天输入栏 ⚡ 下拉且是会话级覆盖不回写）。
- **硬编码、不可配置的 agent 行为参数**（影响体验最大的一批）：

| 参数 | 当前值 | 位置 |
|---|---|---|
| 模型循环最大轮数 | 50 | bot_model_loop.rs:295（skill frontmatter 可覆盖，配置不可） |
| 单请求工具熔断 | 100（可配 maxFunctionCalls） | bot_model_loop.rs:315 |
| 工具结果回填消息栈 | **不截断** | bot_model_loop.rs:1390-1394（上下文膨胀风险，仅审计不截） |
| 会话历史预算 | 100,000 字 | bot_chat.rs:101 |
| 子 agent 预算 | turns 30 / tool_calls 100 / wall 600s / 并发 3 | db/subagents.rs:16-19、bot_orchestrator.rs:732 |
| 记忆注入 | 预算 4000 字 / top 5 / recent 3 / lesson 3 | memory/mod.rs:33,136-140（memoryTuning 可覆盖） |
| 搜索 | 条数 8 / 输出 6000 字 | bot_web.rs:16-18 |
| 调度扫描间隔 / 定时超时 | 30s / 30min | bot_scheduler.rs:1078/:68 |
| LLM 流 idle / 重试 | 120s / 1 次 1500ms | bot_model_loop.rs:445,323-324 |

- **词元统计**：设置页「词元统计」section 是「规划中」占位（SettingsPage.tsx:2819-2829），而 `llm.usage` 审计数据已在产生（bot_model_loop.rs:1152-1163）。

### 1.4 半成品基建（复用而非重建）

- `LoopTrace{turn_count, tool_calls[]}` 已在模型循环产出到手（bot_model_loop.rs:454-460），但 run_task_in_chat 处「暂无消费方」直接剥掉（bot_chat.rs:1486-1487）。
- `evolution/trace.rs` 的 `ExecutionTrace/ToolCallSummary`（name/success/duration_ms/error_kind）+ 采样判定成型，只写一行 `trace.completed` 审计，无落表无 UI。
- `confirm_requests`（bot_slash.rs:263-353）：后台执行时确认通道整条不可用（一律拒），无持久化待确认队列。

### 1.5 执行可见性缺口汇总（改造靶子）

1. 任务卡执行中只有头像换机器人（TaskCardContent.tsx:138），无当前动作/工具流/子任务实时勾选/「详情」入口。
2. 主窗口没有聊天视图（聊天只在挂件 WidgetApp），围观执行必须唤起挂件。
3. 工具调用只见名字+入参，无结果、无耗时、无成败、无 diff。
4. 工作流画布只有 running 布尔 + done/total 计数 + 三色描边（TaskNode.tsx:25-36），失败红环不解释原因，节点无会话跳转。
5. 定时任务执行历史无列表、无会话关联跳转；执行中无「正在跑哪张卡」的显示。
6. 审计日志非实时、入口深、纯文本。

---

## 2. 总体设计：一个执行痕迹（ExecTrace）底座 + 两个透明面

```
┌ 采集层 ──────────────────────────────────────────────────────┐
│ dispatch.rs(每工具) / bot_model_loop.rs(每轮+usage)            │
│ bot_fs.rs(文件diff) / bot_scheduler.rs(触发) / workflow_runner(节点) │
└──────┬──────────────────────────┬─────────────────────────────┘
       │ 落库(异步)                │ emit 事件(实时)
┌ 数据层 ▼ ────────────┐   ┌ 事件层 ▼ ───────────────────────────┐
│ exec_traces          │   │ bot-tool-done 扩展(+result/ms/ok)    │
│ exec_spans           │   │ bot-file-changed(新增)               │
│ file_changes         │   │ workflow-node-status / sched-status  │
└──────┬───────────────┘   └──────────────┬──────────────────────┘
       │ 查询命令 trace_*                  │
┌ 展示层 ▼ ───────────────────────────── ▼ ─────────────────────┐
│ 聊天 ToolBadges 增强 / 任务卡「执行详情」TracePanel             │
│ 工作流画布实时高亮 / 定时任务执行历史 / 设置页参数卡+词元统计    │
└──────────────────────────────────────────────────────────────┘
```

设计原则（对齐项目既有哲学）：

- **单一真相源查表**：参数走 `PARAMS_TABLE`（仿 bot/registry.rs TOOLS_TABLE），采集点只挂在 dispatch/model_loop/bot_fs 三处咽喉，不在 33 个工具里各改。
- **落库为查询面服务，事件为实时面服务**，两者同源不双写业务逻辑：span 写库和 emit 在同一采集函数里。
- **默认行为零变更**：所有新截断/新行为都有配置开关且默认关闭或保持现状；事件只加字段不改名（兼容 tests/fixtures/tools_baseline.json 锁测）。
- **隐私**：diff 与工具结果含用户文件内容，只落本地 SQLite，不外发；提供清除。

---

## 3. 目标一设计：参数透明化

### 3.1 PARAMS_TABLE 参数注册表（新增 `src-tauri/src/bot/params.rs`）

仿 `ToolDef`/`TOOLS_TABLE`（bot/registry.rs:335）模式，每个参数一条注册项：

```rust
pub struct ParamDef {
    pub key: &'static str,        // "loop.max_rounds"
    pub label: &'static str,      // "模型循环最大轮数"
    pub category: ParamCategory,  // Loop | Tools | Subagent | Memory | Search | Schedule | Advanced
    pub hint: &'static str,       // 一句话说明 + 调大的代价
    pub default: ParamValue,      // Int/String/Bool/Float
    pub clamp: (i64, i64),        // 数值钳制
    pub read: fn(&BotConfig) -> ParamValue,    // 生效值（含 None→default 解析）
    pub source_of: fn(&BotConfig) -> ParamSource, // Config | Default | ModelEntry | SessionOverride
}
pub static PARAMS_TABLE: &[ParamDef] = &[ /* … */ ];
```

- 新命令 **`bot_effective_params`** → `Vec<ParamView{key,label,category,hint,value,default,source,unit}>`。
- 写路径不新增命令，继续走 `bot_set_config` 整份替换写（项目惯例，SettingsPage.tsx:713-798）；参数卡改值 = 改对应 config 字段后整写。
- 价值：以后加参数 = 注册表加一条，设置页自动渲染；「可调试」的核心是**生效值+来源可见**（如 max_tokens 显示「条目覆盖 > 全局 > 默认 8192」）。

### 3.2 参数分批导出清单

**第一批（进 PARAMS_TABLE + 设置页 UI）**：

| key | 去向 | 说明 |
|---|---|---|
| `loop.max_rounds` | 新 config 字段 `max_rounds: Option<u32>`（默认 50，钳 5..=200） | 现硬编码 bot_model_loop.rs:295 |
| `loop.max_function_calls` | 已有 maxFunctionCalls，仅入表展示 | — |
| `loop.history_budget_chars` | 新字段（默认 100000） | bot_chat.rs:101 |
| `tools.max_output_chars` | 新字段 `max_tool_output_chars: Option<u32>`（**默认 None=不截断，保持现状**） | bot_model_loop.rs:1390-1394 的截断开关，P3 只出参数、P4 落行为 |
| `subagent.max_turns / max_tool_calls / max_wall_secs / max_running` | 新字段组（默认 30/100/600/3） | db/subagents.rs:16-19、bot_orchestrator.rs:732 |
| `search.max_results / search.output_chars` | 新字段（默认 8/6000） | bot_web.rs:16-18 |
| `model.max_tokens` / `model.reasoning_effort` / `memory.memory_tuning`×8 | **已有字段，只补 UI** | types.rs:135/142/181 |

**第二批（只读展示，不开放编辑）**：LLM idle 120s、重试策略、调度扫描 30s、记忆去重阈值等——入表展示生效值，`source=hardcoded`，避免用户调坏。后续按需再放开。

### 3.3 设置页落点

- 「机器人」section 新增**「Agent 运行参数」卡**：按 category 分组渲染 PARAMS_TABLE，每项显示 当前生效值 / 默认值 / 来源徽标（默认|配置|模型条目） / 说明 tooltip；高级参数折叠。
- 「记忆」section：memoryTuning 8 项补控件（预算/条数/衰减/去重阈值，钳制与 memory/mod.rs:154-148 一致）。
- 「模型」section：全局 maxTokens、全局 reasoningEffort 补控件（ModelRow 条目级已有）。
- 「词元统计」section 从占位实装：后端按天聚合 llm.usage（见 §4.2），前端出 每日 tokens 柱状 + 按 origin 分类 + 累计费用估算（单价可后置）。

---

## 4. 目标二设计：执行透明化

### 4.1 数据模型（新增 `src-tauri/src/db/trace.rs`，migrations.rs 幂等建表，仿 ensure_meta_tables）

```sql
CREATE TABLE IF NOT EXISTS exec_traces (        -- 一次执行 = 一条（run_task_in_chat / 主聊天 / 技能 run）
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL,
  task_id INTEGER,                              -- 可空（纯聊天）
  origin TEXT NOT NULL,                         -- Manual/Scheduled/Batch/Workflow/Main/Skill（复用 TaskExecOrigin 语义）
  title TEXT,                                   -- 任务标题快照
  status TEXT NOT NULL DEFAULT 'running',       -- running/done/failed/stopped/timeout
  started_at INTEGER NOT NULL, finished_at INTEGER,
  turn_count INTEGER DEFAULT 0, tool_calls INTEGER DEFAULT 0, files_changed INTEGER DEFAULT 0,
  prompt_tokens INTEGER DEFAULT 0, completion_tokens INTEGER DEFAULT 0,
  error TEXT
);
CREATE INDEX IF NOT EXISTS idx_traces_task ON exec_traces(task_id);
CREATE INDEX IF NOT EXISTS idx_traces_session ON exec_traces(session_id);

CREATE TABLE IF NOT EXISTS exec_spans (         -- 每次工具调用 = 一条
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  trace_id INTEGER NOT NULL, turn INTEGER NOT NULL, tool_call_id TEXT,
  name TEXT NOT NULL, args TEXT, result TEXT,   -- JSON/文本，各钳 SPAN_TEXT_MAX（16KB）
  ok INTEGER NOT NULL DEFAULT 1, duration_ms INTEGER, created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_spans_trace ON exec_spans(trace_id);

CREATE TABLE IF NOT EXISTS file_changes (       -- 每次文件落盘类修改 = 一条
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  trace_id INTEGER NOT NULL, span_id INTEGER,
  path TEXT NOT NULL, kind TEXT NOT NULL,       -- create/modify/delete
  added INTEGER NOT NULL DEFAULT 0, deleted INTEGER NOT NULL DEFAULT 0,
  diff TEXT,                                    -- unified diff 文本，钳 MAX_DIFF_LINES(2000) 行
  truncated INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_changes_trace ON file_changes(trace_id);
```

- 保留期：随任务卡归档策略对齐——默认 30 天定期清理 + 单 trace 上限（复用「数据管理」的清理风格，archive_after_days 先例 types.rs:152）。
- 定时任务/工作流的执行历史因此自然有表可查：`trace_list(origin=Scheduled)` 即历史页数据源，`exec_traces.session_id` 即 ⏰ 会话跳转锚点（补上 bot_scheduler.rs:19-20 缺的关联）。

### 4.2 采集点接线（只动三处咽喉 + 两处旁路）

1. **dispatch.rs（工具层，主采集点）**：`execute_tool_impl` 前后已有 ToolCallTrace（session_id/turn/tool_call_id 已下传，dispatch.rs:25-48）和 tool.call/tool.return 审计——在同一处补：
   - span 落库（args/result 走 `SPAN_TEXT_MAX` 钳制）+ 异步写；
   - emit 扩展后的 `bot-tool-done`；
   - **文件类工具（edit_file/write_file）返回的 `FileChangeReceipt`（新增返回附件，不改工具 schema 文本）** → file_changes 落库 + emit `bot-file-changed`。
2. **bot_model_loop.rs（轮层）**：`LoopTrace` 本就有 turn_count + 每工具摘要（:454-460）——把 `llm.usage` 的 tokens 累计进 LoopTrace（:1152-1163 处顺手），trace 收尾时写 exec_traces 汇总行。
3. **bot_fs.rs（diff 生成点）**：`try_apply_edit` 返回的 `(替换后全文, mode, 行delta)` 处已有 before/after 全文在手——用 **`similar` crate**（纯 Rust 零依赖，加入 Cargo.toml）生成 unified diff；`tool_write_file` 覆盖前读旧内容算 diff（新建=全 + 行）。diff 钳 2000 行，截断置位。
4. **bot_chat.rs（收尾）**：run_task_in_chat 剥 LoopTrace 处（:1486-1487）改为写 trace 收尾（status/turn_count/tool_calls/files_changed/tokens/finished_at）——半成品基建就此闭环。
5. **bot_scheduler.rs / workflow_runner.rs（旁路打点）**：sched_run/sched_done 处 emit `sched-status`；workflow run_controller 节点状态流转处 emit `workflow-node-status`（nodeId=taskId、sessionId 随 run_task_in_chat 返回值带上——run_controller 已在 NodeOutcome 收流，:305-481，加字段即可）。

### 4.3 事件协议（只加不改）

| 事件 | 变更 | payload |
|---|---|---|
| `bot-tool-done` | **扩展字段**（兼容） | 原有 `{id,name,args,sessionId}` + 新增 `result:string|null, ms:number|null, ok:boolean` |
| `bot-file-changed` | 新增 | `{sessionId, traceId, path, kind:'create'|'modify'|'delete', added, deleted}` |
| `workflow-node-status` | 新增（全窗口广播） | `{taskId, status:'running'|'done'|'failed'|'skipped', sessionId}` |
| `sched-status` | 新增（全窗口广播） | `{taskId, phase:'started'|'done'|'failed'|'skipped', sessionId}` |

emit 范围：流式三件维持只推 widget；新增三事件 `app.emit` 全窗口广播（主窗口 TracePanel/画布/定时页要用；量级=工具调用频次，16KB 内文本事件无压力）。

### 4.4 查询命令（新增，挂 lib.rs invoke_handler）

- `trace_list({task_id?|session_id?|origin?|limit?})` → 摘要行数组（不含 span 正文）。
- `trace_detail(trace_id)` → trace 汇总 + spans（含 args/result 全文，已被钳）+ file_changes（含 diff）。
- `trace_clear_before(ts)` → 保留期清理（数据管理 section 挂按钮）。
- `usage_stats_daily(days)` → 词元统计卡数据源（从 exec_traces 聚合，llm.usage 不再需要解析 bot.log）。

---

## 5. 前端改造

### 5.1 聊天（挂件 ChatPanel）
- `ToolCall` 类型加 `result/ms/ok`（types.ts:6），`ToolBadges`（MessageList.tsx:178-213）pill 加耗时与成败色，展开区从「只有入参」变「入参 + 结果（mono 折叠）」。
- `FileSummary`（MessageList.tsx:218-260）数据源换成 `bot-file-changed` 事件聚合（按文件去重、显示 +N/-M、失败回退现有正则抽取），点击展开 unified diff 视图（简单行着色解析器，~40 行，不引前端库）。

### 5.2 任务卡「执行详情」TracePanel（新组件 `src/components/TracePanel/`）
- 入口：TodoCard / TaskCardContent 状态行加「执行详情」按钮（有 trace 才显示，附 span 数徽标）。
- 弹层内容：摘要头（状态/耗时/轮数/工具数/改文件数/tokens）→ 时间线列表（每 span：图标+工具名+耗时+成败，展开入参/结果）→ 文件 diff 内嵌（红绿行）。
- 主窗口从 SQLite 查（trace_detail），挂件里也可看正在跑的 trace（span 实时事件也可选接，P2 先查询刷新）。

### 5.3 工作流画布
- 监听 `workflow-node-status` 实时高亮 running 节点（保留 5s 轮询作兜底，WorkflowPage.tsx:538-558）。
- 节点点击 → TracePanel（按 task_id 查 trace_list）；失败红环 hover 显示 error 摘要。
- 顶栏执行中显示「当前节点 N/M」替代纯 done/total。

### 5.4 定时任务页
- SchedulePage 新增「执行历史」列表：`trace_list(origin=Scheduled)`，行=时间/任务/状态/耗时/工具数，点行跳转对应 ⏰ 会话（session_id 关联，补齐 bot_scheduler.rs:19-20 缺口）。
- 监听 `sched-status`：执行中任务行内显示「执行中…」。

### 5.5 设置页
- §3.3 的参数卡 / memoryTuning / 全局推理参数 / 词元统计卡。

### 5.6 （可选 P4）主窗口「执行活动」
- 不搬聊天进主窗（架构不动），左导航 Agent 能力组可加「活动」聚合页：running traces 实时 + 最近完成列表 + 一键唤起挂件围观。列 P4 视 P1-P3 反馈决定。

---

## 6. 分期路线

| 期 | 内容 | 验收标准 |
|---|---|---|
| **P1 执行透明·后端根基** | db/trace.rs 三表 + migrations；dispatch/model_loop/bot_fs 采集接线；similar 生成 diff；LoopTrace 收尾闭环；bot-tool-done 扩展 + bot-file-changed；trace_* 查询命令 | 手动执行一张含文件编辑的任务卡：`trace_detail` 能看到每个工具调用的入参/结果/耗时/成败，文件修改有 unified diff；工具 schema 基线（tools_baseline.json）测试不动 |
| **P2 执行透明·前端呈现** | TracePanel + 任务卡入口；ToolBadges/FileSummary 增强 + diff 视图；workflow-node-status/sched-status + 画布实时 + 定时历史页 | 三大黑盒场景验收：手动卡、定时卡、工作流各跑一次，全程（工具流/文件 diff/节点状态/执行历史）在界面可见，不再需要翻 bot.log |
| **P3 参数透明** | bot/params.rs PARAMS_TABLE + bot_effective_params；设置页运行参数卡 + memoryTuning UI + 全局 maxTokens/reasoningEffort 控件 + 词元统计卡 | 设置页可见全部第一批参数的生效值/默认值/来源；词元统计 section 实装非占位 |
| **P4 增强（按需）** | max_tool_output_chars 截断行为落地（默认关）；trace 保留期+清理 UI；主窗口活动页；trace 接 evolution 采样（error_kind 分类器补全 bot_model_loop.rs:609 占位） | 各自单列验收 |

每期遵守项目硬约束：不改 TOOLS schema / 事件名 / prompt 常量 / 命令名（只增不改）；`cargo test --lib` 全绿；新增模块在 rust-bot-architecture.md §6.4 登记。

## 7. 风险与对策

1. **SQLite 写放大**：span 异步批量写（channel + 定时 flush），单 trace span 数天然被 max_function_calls 钳制；span 文本 16KB 上限。
2. **diff 体积**：2000 行截断 + truncated 标记；二进制/超大文件（>1MB 读上限已有，edit_file_sync:610-631）跳过 diff 只记行数。
3. **事件频率**：新增三事件只在工具调用/节点流转粒度（非 token 粒度），频率与现有 bot-tool 同级。
4. **隐私**：diff/结果只落本地库；trace_clear_before + 数据管理清空入口；不进任何上报管道（evolution trace 的「只存计数不存原文」纪律不受影响，§4.2 的 evolution 接线仍走其采样判定）。
5. **兼容**：bot-tool-done 只加字段；前端类型同步更新；旧会话无 trace 时前端入口隐藏。
6. **行为零变更**：不截断默认、不弹窗默认、工具结果回灌 msgs 逻辑不动（截断是 P4 显式开启项）。

## 8. 测试计划

- Rust：trace 落库单测（mock ModelLoopDeps 注入，仿 tests/mock_llm 模式）；diff 生成纯函数单测（exact/ws 两级匹配 × create/modify/delete）；span 钳制单测；migrations 幂等。
- 前端：ToolBadges/FileSummary/TracePanel 组件测试（项目已有 *.test.tsx 体系，Vitest）。
- 集成：llm_integration.rs 加一条「任务执行 → trace 断言」；tools_baseline.json 锁测保持绿。
- 手工冒烟：按 §6 各期验收标准出 MANUAL-SMOKE-ACCEPTANCE-EXEC-TRACE 文档（项目惯例）。

---

## 9. 大厂对标调研与方案改进（2026-10-06 增补）

> 对标对象：Claude Code（官方 docs）、OpenAI Codex CLI（官方 docs）、ZCode（本机 client 实测 + 配置指南）、
> Gemini CLI（官方 docs）。本节是 §1-§8 的增量层：先给对标摘要，再给借鉴清单（标注落点分期），§9.3 列对前文分期的具体增补。

### 9.1 对标摘要

| 维度 | Claude Code | Codex CLI | ZCode | Gemini CLI | wmessage 现状 |
|---|---|---|---|---|---|
| 配置分层 | 5 层（managed > CLI 参数 > 项目 local > 项目共享 > user），键级优先级，allow 列表跨层合并；`/status` 显示生效来源，`claude doctor` 列被拒条目 | `config.toml` + **profiles**（命名档案：模型+approval+sandbox 捆绑，一键切换） | user（`~/.zcode/cli/config.json`）+ workspace（`.zcode/config.json`）两层；Settings GUI 面板（Skills/Subagents/MCP/插件） | `settings.json` + `/settings` 对话框 | 单层 bot-config.json 整份替换写（本方案 PARAMS_TABLE 增强） |
| 权限模型 | **allow/ask/deny 规则表** `Tool(specifier)`（如 `Bash(npm run test *)`、`Read(./.env)`、`Edit(/src/**/*.ts)`），评估序 deny>ask>allow；模式 default/**acceptEdits**/plan/auto/dontAsk/bypassPermissions；「始终允许」自动写回 local 文件；`/permissions` 可视化管理 | **双轴解耦**：`approval_policy`（何时问：untrusted/on-failure/on-request/never）× `sandbox_mode`（能做什么：read-only/workspace-write/danger-full-access + `network_access` 开关） | 四档 permissionMode（**build**=改动前确认 / **edit**=自动应用 / **plan**=只读 / **yolo**）+ 7 个 hook 事件（PreToolUse/PostToolUse…） | autoAccept / yoloMode | 单轴 perm_mode 三档 + 目录白名单；ask 档「始终允许该目录」写回 allowed_dirs |
| 执行痕迹 | Ctrl+O verbose transcript 总开关；**OTel 遥测规格**：span 树 `interaction → llm_request / tool`，tool span 带 `duration_ms/result_tokens/tool_use_id/file_path`，diff/正文走**内容门控**（`OTEL_LOG_TOOL_CONTENT` 等 5 个开关，60KB 截断）；`error_class` 分类 | **rollout JSONL 会话文件**（`~/.codex/sessions/` 按日分区，`resume --last` 全量回放，含工具轨迹），`/status` 显示 session id | hooks 把 7 类事件暴露给外部脚本；挂件聊天流 | — | bot.log 审计纯文本（本方案 P1 落库三表 ≈ 其 tool span） |
| 文件回滚 | **checkpointing**：每个 prompt 自动快照，**只追踪 AI 文件编辑工具**（bash 改动不管），`/rewind` 三选（回代码/回对话/双回），保留 100 份，`cleanupPeriodDays` 默认 30 天 | git 脏检查警告，无快照 | — | checkpointing **默认关**，`/restore` 手动恢复 | 无（本方案 P1 只记 diff） |
| 用量观测 | `/context` 上下文水位、`/cost`、`claude_code.token.usage` 指标、statusline 可自定义显示 token/费用 | `/status` 剩余上下文 | — | settings.json 内 telemetry 配置 | 词元统计占位（本方案 P3 实装，数据已有） |

### 9.2 借鉴清单（按价值排序，标注落点）

1. **per-tool 权限规则表**（Claude Code rules）——落 **P3**。
   现状 perm_mode 三档对所有文件工具一刀切。借鉴：BotConfig 增 `tool_rules: Vec<{tool, action: allow|ask|deny}>`，
   评估序 deny > ask > allow > 全局 perm_mode 兜底；不做路径 glob（路径维度已有白名单，避免 Claude Code 自己文档都承认的 Bash 规则可绕过问题）。
   设置页授权卡增加规则表可视化；执行中「始终允许」确认写回规则（现状只写目录白名单）。
2. **acceptEdits 中间档**（Claude Code acceptEdits ≈ Codex on-request 的组合语义）——落 **P3**。
   PermMode 增第 4 档 `auto`：**写白名单内（AI_Gen_Files + 绑定文件夹 + allowed_dirs）自动放行，白名单外降级 ask**。
   直接解决现有黑盒痛点之一：定时/工作流后台执行时 ask 档无人应答一律拒（bot_slash.rs:275-301），写文件类任务必失败——auto 档让无人值守在白名单内畅通、白名单外留痕拒绝。
3. **文件改动可回滚**（Claude Code checkpointing / Gemini /restore）——**P1 预留字段，P2 出按钮，P4 完整化**。
   edit_file/write_file 时 before 内容已在手：P1 的 file_changes 表加 `before_ref`（before 全文落
   `data_dir/checkpoints/<before_ref>`，uuid 发号非 DB row id——row id 落库前不可知；16KB 钳制不适用于快照，
   快照即全文）；TracePanel 文件行加「回滚到此修改前」。
   边界对齐 Claude Code：只覆盖 AI 编辑工具，run_python 的文件副作用明确不追踪（产品文案写清）。
   保留期与 trace 一致（30 天）。
4. **verbose 三档全局开关**（`/config verbose` + Ctrl+O）——落 **P2**。
   设置「执行过程详细度」：简洁（只工具名 pill）/ 详细（+入参结果展开，现状）/ 调试（+轮次号+tokens+耗时）。
   前端一个全局档位控制 ToolBadges 默认展开级别，单条仍可手动展开（Ctrl+O 的「临时全展开」语义 → 聊天工具栏一个切换钮）。
5. **内容门控/隐私开关**（OTel 5 个内容开关的本地版）——落 **P4**。
   span 结果/diff 正文默认落库（本地隐私面可控），但提供设置开关：关闭正文只存元数据（名称/耗时/成败/行数）。
   超限截断 60KB→我们 16KB 的对齐确认（Claude Code 也用 60KB 上限，量级一致）。
6. **error_class 错误分类**（OTel `tool.execution.error_class`）——**P1 字段预留，P4 接线**。
   exec_spans 加 `error_class TEXT`；evolution trace 的占位分类器（bot_model_loop.rs:609 None）补全后同源复用。
7. **执行档案 Profiles**（Codex profiles）——落 **P4**。
   在 PARAMS_TABLE 之上加命名预设：「保守」（strict + 50 轮 + py 关）、「标准」、「放开」（auto/yolo + 200 轮），
   一键整体切换写 bot_set_config。避免用户逐项调 10 个参数。
8. **配置诊断提示**（claude doctor）——落 **P3 小项**。
   bot_set_config 已有钳制与非法值回退（schema.rs），前端不展示。参数卡对「值被钳制/非法回退/来源覆盖」给角标提示，
   与 §3.1 的 source 徽标同一处渲染。
9. **上下文水位**（/context）——落 **P4**。会话水位条（已用 tokens / 上下文窗口 %），数据 llm.usage 已有，
   挂件输入栏与任务卡执行详情各放一处；超阈值（如 80%）提示 /compact。
10. **trace JSONL 导出**（Codex rollout 的本地文件化思想）——落 **P4**。`trace_export` 命令导出单次执行为 JSONL
   （ spans + diffs + usage），便于贴给开发者/存档；不引入外部上报通道。
11. **明确不借**：多层设置文件（桌面单用户应用无团队共享诉求，单层 + PARAMS_TABLE + source 徽标已覆盖其可诊断性价值）；
    hooks 外部脚本执行（攻击面大，通知中心 + 系统通知已覆盖「执行结束告知」）；bare-name deny 移出工具上下文
    （我们仅 33 工具，无上下文瘦身诉求）；OTel 外发遥测（隐私定位不符）。

### 9.3 对前文分期的增补汇总

| 期 | 增补项 | 来源 |
|---|---|---|
| P1 | exec_spans 加 `error_class`；file_changes 加 `before_ref` + checkpoints 落盘（保留期 30 天） | §9.2-3/6 |
| P2 | TracePanel 文件级「回滚」按钮；verbose 三档全局开关 + 聊天临时全展开钮 | §9.2-3/4 |
| P3 | `tool_rules` per-tool 规则表 + 授权卡可视化 + 「始终允许」写回；PermMode 第 4 档 `auto`（acceptEdits 语义）；配置钳制/来源诊断角标 | §9.2-1/2/8 |
| P4 | 内容门控开关；Profiles 三预设；上下文水位条；trace JSONL 导出；error_class 接 evolution 分类器 | §9.2-5/6/7/9/10 |

> 对标后不改变 P1/P2 是「执行透明」主线、P3 是「参数透明」主线的判断；增补项多为低成本高价值的小切口，
> 其中 §9.2-3（回滚）与 §9.2-2（auto 档）是两个真正改变产品能力的借鉴点。

---

# 第二部分：开发执行手册（细化 §6 分期表为可执行批次）

> 节奏对齐项目惯例：**一批 = 一次提交序列 + 一条 DEVLOG 批次条目 + 一次本地 OCR 复审**。
> 门禁实现见 `docs/testing.md` 与 `scripts/test-fast.sh` / `test-all.sh`，本部分只写「本改造新增什么、怎么验、怎么记账」。

## 10. 批次卡（P1–P4 细化到文件级改动）

### P1 执行透明·后端根基（4 批）

**P1-a 数据层 `db/trace.rs`**
- 改动：新增 `src-tauri/src/db/trace.rs`（三表 DDL + CRUD + 保留期清理）；`db/migrations.rs` 幂等建表（仿 `ensure_meta_tables` 先例）；`db/mod.rs` re-export；架构文档 §6.4 登记（`audit_module_map.py` 强制）。
- DDL 补充（对 §4.1 的细化）：`file_changes` 增 `before_ref`（快照文件名=change_id）、`before_sha`/`after_sha`（P2 回滚防漂移比对）；span/diff 钳制常量 `SPAN_TEXT_MAX=16KB`、`MAX_DIFF_LINES=2000` 进 `db/trace.rs` 模块头。
- 测试：建表幂等（连跑 ×2）、三表 CRUD 往返、钳制截断置位、保留期只删过期、索引存在。
- 审计：`trace.retired`（清理条数）。

**P1-b diff 生成 + bot_fs 接线**
- 改动：`Cargo.toml` 加 `similar`（machete 门禁自动覆盖）；`bot_fs.rs` 的 `tool_edit_file`/`tool_write_file` 产出 `FileChangeReceipt{path, kind, added, deleted, diff, before_ref, before_sha, after_sha, truncated}`；`try_apply_edit`（:514-592）与 write 覆盖前取 before 全文。
- 测试（新增 ~12 例，风格对齐 bot_fs 白名单锁测试）：exact/ws 命中 × create/modify/delete 矩阵、CRLF 免疫、多处命中不产 diff 只记行数、>1MB 跳过 diff、非 UTF-8 跳过、before 快照写盘失败**降级不 fail 工具**。
- 安全：快照落 `data_dir/checkpoints/<change_id>`，文件名内部发号非用户路径——杜绝路径注入面。
- 审计：`file.change`（Info）、`checkpoint.write_fail`（Error）。

**P1-c dispatch 采集接线**
- 改动：`bot/dispatch.rs` `execute_tool_impl` 前后挂 span 落库（异步 channel + 批量 flush，不阻塞工具执行）+ emit 扩展后 `bot-tool-done{result,ms,ok}` + 新 `bot-file-changed`；`ToolStatus→ok/error_class` 映射；`bot_model_loop.rs` LoopTrace 增 tokens 累计（`llm.usage` 产生点 :1152-1163 顺手）。
- 测试：mock ModelLoopDeps 下 span 字段断言（name/turn/args 钳/result 钳/ok/duration/error_class）、事件 payload 断言、早退路径（pre_execute deny 也成 span）。
- 硬约束：`bot-tool-done` 仅加字段不改名；`tools_baseline.json` 与 `registry_tests` 不动。

**P1-d 收尾闭环 + 查询命令**
- 改动：`bot_chat.rs` run_task_in_chat 剥 LoopTrace 处（:1486-1487）改写 exec_traces 收尾（status/turns/tools/files/tokens/finished_at）；`bot_scheduler.rs` sched_run/sched_done 与 `workflow_runner.rs` run_controller 节点流转处打点 + emit `sched-status`/`workflow-node-status`；新命令 `trace_list`/`trace_detail`/`trace_clear_before`/`usage_stats_daily` 注册 lib.rs invoke_handler；error.rs 视需要加 `TraceNotFound`（双端同步，见 §12.2）。
- 测试：llm_integration 新增「任务执行 → trace_detail 断言」用例；收尾状态矩阵（done/failed/stopped/timeout）；`audit_tauri_bridge.py` 绿（P1 阶段命令注册未调用=warn 属预期）。
- 验收门：sqlite3 手查三表 + `trace_detail` JSON 与真实文件 diff 人工比对（§14.1 P1 清单）。

### P2 前端呈现（3 批）

**P2-a TracePanel + 任务卡入口 + 回滚**
- 改动：新目录 `src/components/TracePanel/`；`TodoCard`/`TaskCardContent` 加「执行详情」入口；后端新命令 `file_rollback`（after_sha 比对防漂移 → `db::atomic_write` 回写 → `file.rollback` 审计）。
- 测试：TracePanel.test.tsx、回滚冲突拒绝用例（文件已被手改 → 提示冲突不覆盖）、knip 绿（目录被引用）。

**P2-b 聊天增强**
- 改动：`ToolCall` 类型 +result/ms/ok（types.ts:6）；ToolBadges 展开区显示结果（mono 折叠）；`FileSummary` 数据源换 `bot-file-changed` 事件聚合（正则抽取降级保留）；unified diff 行着色解析（~40 行，不引前端库）；verbose 三档设置 + 聊天工具栏「临时全展开」钮（Ctrl+O 语义）。
- 测试：ToolBadges/FileSummary/verbose 切换组件测试；tsc/knip/vitest 绿。

**P2-c 工作流/调度事件 + 页面**
- 改动：WorkflowPage 监听 `workflow-node-status` 实时高亮（5s 轮询保留兜底）；节点点击 TracePanel、失败红环 hover 原因；SchedulePage「执行历史」列表（`trace_list(origin=Scheduled)`）+ 行内「执行中」+ 点行跳 ⏰ 会话。
- 验收：§14.2 P2 三大黑盒场景冒烟——**本阶段核心门槛，三场景任一不过不进 P3**。

### P3 参数透明（3 批）

**P3-a PARAMS_TABLE + 新 config 字段**
- 改动：新增 `src-tauri/src/bot/params.rs`（ParamDef/PARAMS_TABLE/bot_effective_params 命令）；BotConfig 新增全 Option 字段（`max_rounds`、`history_budget_chars`、`max_tool_output_chars`、subagent 预算 4 项、search 2 项）——**None=现默认，无需 schemaVersion bump**（沿用 v0→v1 只补版本号先例）；`resolve_max_rounds`（bot_model_loop.rs:303-305）、db/subagents.rs、bot_web.rs 读点改走 config 解析链。
- 测试：表完整性行数锁（注册表防漏登记）、clamp 边界、source 解析链（ModelEntry 覆盖 > 全局 > 默认）。

**P3-b 设置页参数卡**
- 改动：机器人 section「Agent 运行参数」卡（按 category 分组 + 来源徽标 + 钳制角标）；记忆 section memoryTuning 8 项控件；模型 section 全局 maxTokens/reasoningEffort 控件；「词元统计」占位实装（usage_stats_daily）。
- 测试：SettingsPage 参数卡渲染/保存/回写测试；`bot-config-changed` 广播链回归。

**P3-c tool_rules + auto 档**
- 改动：BotConfig 增 `tool_rules: Vec<ToolRule{tool, action}>`；`PermMode` 增 `Auto` 变体；`resolve_writable`/`resolve_with_perm`（bot_fs.rs:396-487/:210-320）三分支改四分支 + 规则表前置评估（deny > ask > allow > 全局档兜底）；ask 确认「始终允许」写回 tool_rules。
- **安全锁（本批强制）**：auto 档白名单内放行/白名单外降级 ask 正反用例、规则首中即停、Strict 仍硬拒、Yolo 语义不变、`..`/软链穿越用例在 auto 档下重跑全绿——并入 bot_fs 白名单锁测试序列（§11 安全锁表）。
- 审计：`tool_rule.hit`（Info）、`perm.auto_degraded`（Info）、`params.clamped`（Info）。

### P4 可选增强（按需拉批）
保留期设置 UI / 内容门控开关 / Profiles 三预设 / 上下文水位条 / `trace_export` JSONL / error_class 接 evolution 分类器 / 主窗口活动页——每项独立小批，先 OCR 复审后验收，不设固定顺序。

## 11. 全量测试门禁（每批必过）

| 层 | 命令 | 基线 / 预期 |
|---|---|---|
| 提交门禁 | git hook → `scripts/test-fast.sh` | fmt / cargo check / machete / pytest collect / 桥一致 / 错误码 / 模块地图 / tsc / knip / vitest --changed 全绿 |
| 全量（push 前） | `bash scripts/test-all.sh` | nextest 全量 + tests-audit 四脚本 + vitest |
| Rust lib | `cd src-tauri && cargo test --lib` | 当前 958 通过，**只增不减** |
| Rust 集成 | `cargo test --test llm_integration` / `task_chat_exec` / `skill_e2e` / `memory_v2_degraded` | 全绿；P1-d 起新增 exec_trace 断言 |
| 前端 | `npm test` | 209 例基线只增不减 |
| ignored 冒烟 | `cargo test --lib memory::embed -- --ignored` | 真实推理冒烟（P3-b 改参数注入时必跑） |

三把既有协议锁是本改造高危区，每批必跑：

1. **`tools_baseline.json` + `registry_tests`**——本改造全程不碰工具 schema；P1-c 事件扩展只动 emit payload。
2. **`audit_tauri_bridge.py`**——新命令（trace_list/trace_detail/trace_clear_before/usage_stats_daily/file_rollback/bot_effective_params）与新事件（bot-file-changed/workflow-node-status/sched-status）自动纳入双向校验；P1 注册未调用=warn 预期，P2 完成配对后必须绿。
3. **`audit_module_map.py`**——新模块（db/trace.rs、bot/params.rs、TracePanel/）当批登记架构文档 §6.4，防模块地图失真。

## 12. 审计规范

### 12.1 审计埋点（audit_event! → bot.log）

新增事件清单（命名对齐既有点分小写风格；KV 500 字符截断、`err =>` 宏臂、`AuditLevel::from_tool_status` 全部沿用）：

| 事件 | 级别 | 时机 | 关键 KV |
|---|---|---|---|
| `trace.start` | Info | 建 trace（run_task_in_chat / 主聊天） | trace_id / session_id / origin / task_id |
| `trace.complete` | Info | 执行收尾 | turns / tools / files / prompt_tok / completion_tok / ms / status |
| `trace.span_overflow` | Warn | args/result 超 16KB 被钳 | tool / len |
| `file.change` | Info | 落盘类修改成功 | path / kind / added / deleted / truncated |
| `checkpoint.write_fail` | Error | before 快照写盘失败 | path / err |
| `file.rollback` | Warn | P2 回滚执行 | change_id / after_sha_match |
| `trace.retired` | Info | 保留期清理 | removed |
| `params.clamped` | Info | P3 配置被钳制/非法回退 | key / requested / effective |
| `tool_rule.hit` | Info | P3 规则表命中 | tool / action / rule |
| `perm.auto_degraded` | Info | auto 档白名单外降级 ask | path |
| `trace.gated` | Info | P4 内容门控关闭时跳过正文落库 | trace_id |

### 12.2 一致性审计（tests-audit/）

- 桥一致性 / 错误码 / 模块地图三脚本由门禁自动覆盖（§11）；本改造新增错误码只有 `TraceNotFound` 一个候选，`error.rs` 与 `errorHandler.ts` 同批同步（`audit_error_codes.py` 卡漂移）。
- 安全锁新增用例（P3-c 四分支）**必须**并入 `bot_fs.rs` 白名单锁测试序列（`allowlist_rejects_traversal_and_prefix_similar_dir` 同风格），在 docs/testing.md §安全相关锁表登记条目。

### 12.3 安全审计

- 每阶段收尾可跑 Mimosa 深度安全扫描（`security_scan`，focusFiles 指向本批改动文件）；触发条件=老板明确要求全量安全审计时。
- 本改造新增攻击面自查清单（每批 OCR 时人工过一遍）：
  diff/快照内容只落 `data_dir/` 本地、不执行不外发；回滚写回走 `db::atomic_write` + `after_sha` 比对 + 软链/穿越拒绝（对齐 `is_within_allowlist` 判定核）；trace_* 查询命令全只读；checkpoints 文件名用内部 change_id 防路径注入；`file_rollback` 纳入 perm_mode 语义（等价一次 write_file）。

## 13. 本地 Open Code Review（OCR）审计流程

- **工具**：本地 `ocr review --commit <sha>`（工作区模式亦可），产物 JSON + HTML。
- **节奏**：每批合并前跑本批 diff 复审；每阶段（P1/P2/P3）收尾加跑一次阶段全量复审。
- **产物命名**：`docs/OCR-CODE-REVIEW-<date>-<batch>.json/.html`；本改造批次前缀用 `et`（et1…et10，对齐 w/u 系列惯例），如 `OCR-CODE-REVIEW-2026-10-07-et1.json`。
- **处置门禁**：critical/high 当批修复，或书面 wontfix 拍板登记 `OCR-FOLLOWUPS-INDEX.md`；medium 本阶段内处置；low 顺手修或登记。**未清账不得进下一批。**
- **假阳性**：按 PROC-4 惯例登记（C3-r1-H1 先例：实证 false positive 附证据），累计 3-4 条评估调 OCR prompt/过滤规则。
- **衍生债**：修复引入的新问题一行一条登记 `OCR-FOLLOWUPS-INDEX.md`（ID | 批次 | 严重度 | 根因 | 触发条件 | 处置），与基线 finding 两套账本分离。
- **DEVLOG 句式**：复审条目写明「ocr 复审（N 条 XH/YM/ZL，json 于 docs/OCR-CODE-REVIEW-…json）」。

## 14. 验收流程

### 14.1 每批验收——DEVLOG 批次条目模板（对齐 R7 惯例）

```markdown
## 2026-10-XX（周X）P1-c dispatch 采集接线
### 改动清单
### 关键设计点
### 单测清单（cargo test --lib <module>，新增 N 例）
### 集成测试（cargo test --lib 全量 N/N 通过；test-all 绿）
### 硬约束遵守
1. ✅ 不改 prompt / TOOLS schema / 命令名 / 错误码；bot-tool-done 为加字段兼容
2. ✅ 新增表/命令/事件清单：…（audit_tauri_bridge 配对状态：warn 预期 / 绿）
3. ✅ 默认行为零变更：…（截断默认关 / 回滚需显式点击 / …）
### OCR 复审（N 条 XH/YM/ZL，json 于 docs/OCR-CODE-REVIEW-…json）
### 验收（对照 §14.1 P1 清单 / 批次验收门）
### 待办（移交下一批 / OCR-FOLLOWUPS-INDEX 登记）
```

### 14.2 阶段验收——MANUAL-SMOKE-ACCEPTANCE 文档（项目命名惯例）

- **P1** `docs/MANUAL-SMOKE-ACCEPTANCE-EXEC-TRACE-P1.md`（开发者视角）：
  ① 跑一张含 edit_file 的任务卡 → `sqlite3` 三表行数与内容抽查；
  ② `trace_detail` 返回的 diff 与真实文件人工比对（含一处 CRLF 用例）；
  ③ 挂件 devtools 抓 `bot-tool-done` 断言 result/ms/ok 字段；
  ④ bot.log 出现 `trace.start`/`trace.complete`/`file.change` 且 KV 完整。
- **P2** `docs/MANUAL-SMOKE-ACCEPTANCE-EXEC-TRACE-P2.md`（用户视角，**三大黑盒场景逐条回收**）：
  - 手动卡（含文件编辑+联网+python）：卡上「执行详情」→ 工具时间线含 args/result/耗时/成败；文件 diff 红绿；「回滚」生效、手改后回滚给出冲突提示；
  - 定时任务（`at:` +2min）：SchedulePage 行内「执行中」→ 执行历史新行 → 点行跳 ⏰ 会话 → TracePanel 全程——**全程不翻 bot.log**；
  - 工作流（3 节点含 1 故意失败）：画布 running 节点实时高亮 → 失败红环 hover 出原因 → 节点点击看 trace → 下游 skipped 灰显；
  - 证据：截图入库 + 浏览器 harness 真机验收（r2/r5 先例，commit ccca7ce 惯例）。
- **P3** `docs/MANUAL-SMOKE-ACCEPTANCE-EXEC-TRACE-P3.md`：
  参数卡逐项对照生效值（改 max_rounds=10 → 执行第 10 轮熔断在 trace 可见）；memoryTuning 改注入条数 → prompt 组装变化；`tool_rules` deny edit_file → 执行被拒 + `tool_rule.hit` 审计；auto 档白名单内自动成功、白名单外留痕拒绝；填 max_rounds=9999 → 落 200 + `params.clamped` + 钳制角标；词元统计卡与 llm.usage 抽样一致。
- **终验**：三阶段冒烟全过 + `test-all.sh` 绿 + 阶段 OCR 清账 + 架构文档/DEVLOG 收尾。

### 14.3 完成定义（DoD，每批打勾）

- [ ] `cargo test --lib` 只增不减；`test-all.sh` 绿
- [ ] tests-audit 四脚本绿（桥 / 错误码 / 模块地图 / pre_step）
- [ ] `tools_baseline.json` 锁测绿（全程不碰 schema）
- [ ] 新审计事件按 §12.1 落地且 bot.log 抽查可见
- [ ] 当批 OCR 复审无未处置 critical/high；衍生债已登记 FOLLOWUPS-INDEX
- [ ] DEVLOG 批次条目（模板 §14.1）+ 阶段 MANUAL-SMOKE 文档 + 截图证据
- [ ] 新增源码文件已在架构文档 §6.4 登记
