# 任务执行聊天化设计方案

> 2026-09-09 · 状态：**已实施**（2026-09-10 第一期落地，偏差见文末「实施偏差」节）
> 目标：任务卡「交给机器人」和定时任务不再是黑箱——每次执行都在聊天窗口新建一个对话，
> 在对话里流式执行，过程可见、可 /stop 叫停、有永久记录可回看。

**评审已确认的决策**：
1. 一次执行 = 一个新会话（执行质量优先，不带杂历史）
2. busy 时**执行不排队**（新会话立即开跑），只有"自动跳转查看"排队，忙完弹提示
3. 会话膨胀靠**手动删除**解决，不做自动清理；删除会话不影响已沉淀的记忆（mem_items 独立）
4. 不做任务卡上的「查看执行对话」入口和 last_session_id 关联——直接在聊天窗口按前缀找
5. 批量执行：每张卡一个独立新会话（单卡失败不污染其他卡记录）

## 1. 现状与问题

| 路径 | 现状 | 问题 |
|---|---|---|
| 🤖 交给机器人 | 在**当前**会话里执行（`bot_execute_task`，interactive=true），有流式 | 执行混入当前对话，污染上下文；连点只靠前端 busy 拦 |
| ⏰ 定时任务 | `bot_scheduler.rs:374` 调 `execute_task_core(interactive=false, session=None)`，**完全 headless**，不发任何流式事件 | 黑箱：看不到过程、无法干预、结果只写卡片备注 |
| 批量执行 | `chat_execute_tasks` 逐卡顺序在当前会话执行 | 同🤖，且多卡结果挤在一个对话里 |

关键事实：三条路径最终都进同一个 `run_model_loop`（bot_model_loop.rs），headless 只是
`stream_to_widget` 开关关掉 + `session_id=None`。多会话 UI、/stop 按会话取消、
系统通知插件全部现成。**改造成"每次执行 = 新建会话 + 会话内流式执行"没有架构障碍。**

## 2. 核心设计

```
执行触发（🤖按钮 / 定时器 / 批量）
   │
   ▼
后端 run_task_in_chat(task_id, origin)            ← 新统一入口（bot_chat.rs）
   │  1. 创建新会话（标题 = 来源前缀 + 任务标题）
   │  2. 任务块（build_task_block 现有格式）作为 user 消息写入 bot_messages
   │  3. run_model_loop(EXECUTE_SYSTEM_PROMPT, 该会话, 流式开启)
   │  4. assistant 回复写入 bot_messages（持久化从"前端负责"补一条后端路径）
   │  5. 结果写回任务卡（现有 edit_task/complete_task prompt 约定不变）
   ▼
前端收到 chat-open-session {sessionId} → 唤起 widget 并切到该会话（直播执行）
   └─ busy 时不打断：跳转排队，当前对话结束后提示「⏰/📋 xxx 已执行，点击查看」
定时任务完成/失败 → 系统通知（点击打开对应会话）
```

- **一次执行 = 一个会话**，标题前缀区分来源：`📋 任务：xxx`、`⏰ 定时：xxx`、「📦 批量」
- 会话即执行记录：聊天窗口天然支持回看，无需另做"执行回放"页
- 取消即 /stop：StopGuard 本就按会话隔离，新会话执行天然可被 ■ 按钮或 /stop 中断
- 记忆红利：走聊天路径后任务执行自动获得记忆块注入 + 截断摘要 + 历史管理

## 3. 各路径改造

### 3.1 🤖 交给机器人（交互路径）
- TodoCard 的 `emit("execute-task")` 改为直接 `invoke("bot_execute_task")`（或保留事件但语义改为"新建会话执行"）
- 后端创建新会话执行，广播 `chat-open-session`；前端 ChatPanel 新增 listen：唤起 widget + `switchSession(sessionId)`
- ≥2 未勾子任务仍走 exec_steps 逐步执行（在新会话内，用户在场可确认），逻辑不变

### 3.2 ⏰ 定时任务（核心受益路径）
- `bot_scheduler.rs:374` 改为调 `run_task_in_chat`，**绕开 exec_steps**（无人在场，直接整体执行）
- 流式事件照常发到 widget：用户若正开着聊天窗口可实时围观；不看也不影响
- 执行完成/失败发系统通知（复用 tauri-plugin-notification + due_notify 的原子写去重模式），点击通知打开 widget 对应会话
- "⏰ 自动执行摘要前置进 note"的兜底逻辑（bot_scheduler.rs:377-410）保留，随调用迁移

### 3.3 批量执行
- `chat_execute_tasks`：每张卡一个独立新会话（已定）——单卡失败不污染其他卡的执行记录，代价是一次多出 N 条会话，靠前缀识别 + 手动删除

### 3.4 取消与并发
- `bot_execute_task` 纳入 ChatGuard（当前只防 bot_chat，🤖路径后端无锁，是现存小漏洞，顺手补）
- 每张新会话天然隔离，多任务并发执行维持现状（调度器 spawn 不 await）
- ExecGuard（按 task_id 防重入）不动

## 4. 改动清单

**后端（Rust）**
1. `bot_chat.rs`：新增 `run_task_in_chat(app, task_id, origin) -> session_id`——创建会话、写 user 消息、调模型循环、写 assistant 回复、回写任务卡、返回 session_id；`execute_task_core` 重构为其内部步骤或被取代
2. `db.rs`：会话创建/消息写入抽 Connection 内核供后端直调（目前是前端命令 `bot_session_create` / `bot_history_save`，内核只在前端路径被用）；**不加 last_session_id**（评审决策 4）
3. `bot_scheduler.rs`：定时触发改调 `run_task_in_chat`；收尾加系统通知
4. `lib.rs`：注册新命令；无新插件
5. 通知：execute 收尾处 `app.notification().builder()...show()`（基建已在 lib.rs:280 注册）

**前端（React/TS）**
6. `ChatPanel.tsx`：新增 `chat-open-session` 事件 listen → 非 busy 直接 switchSession；busy 时记录待跳转会话，当前轮结束后 addHint 提示「已执行，点击查看」
7. **busy 锁不动**：一期不放宽切换限制，跳转只排队不打断（评审决策 2）
8. ~~任务卡「查看执行对话」入口~~ 不做（评审决策 4）
9. 定时任务执行中卡片显示「执行中…」状态（复用 set_bot_assigned 头像切换，或加小 spinner）

## 5. 边界情况

- **busy 时新执行到达**（评审决策 2）：**执行本身不排队**——后端立即在新会话开跑（会话隔离，现状就支持并发）；只有"自动跳转查看"排队，busy 结束后 hint 提示，用户自行点击切换
- **应用未运行时定时任务到点**：现状就是应用不在则不执行（调度器在应用内），本方案不改变这一点
- **执行中用户在新会话里插话**：同会话 ChatGuard 会拒绝（"上一轮进行中"）——一期不做运行中插话，/stop 已可叫停
- **widget 窗口未打开**：emit_to("widget") 无人接收不报错；历史已落库，事后打开会话即可看到完整记录
- **用户删除执行会话**：会话+消息原子删除（bot_session_delete 现有行为），执行记录随之消失——用户主权，合理；已沉淀的记忆（mem_items）不受影响
- **exec_steps 的会话绑定**：pending 状态按 session 停放，新会话执行时从干净状态开始，无串扰

## 6. 测试计划

- 后端：run_task_in_chat 全链路（mock LLM）——会话创建、user/assistant 消息落库、任务卡回写；定时路径创建会话且绕开 exec_steps；ChatGuard 覆盖 bot_execute_task
- 回归：memory_regression 17 例、llm_integration 38 例全绿；exec_steps 现有用例不破坏
- 前端：chat-open-session 非 busy 切会话、busy 时排队提示（vitest 现有 ChatPanel 测试模式）

## 7. 实施分期

- **第一期**（本方案全部）：统一 `run_task_in_chat` + 🤖/定时/批量三路径切换 + chat-open-session（busy 排队提示）+ 系统通知
- **第二期**（以后再说，本期不做）：busy 锁放宽（可切换查看执行直播）、执行中插话、执行会话分组/折叠 UI、卡片↔会话双向关联

## 8. 风险

- **会话膨胀**：每次执行一个会话，定时任务频繁时会话列表变长 → 已定：手动删除解决（评审决策 3），不做自动清理
- **busy 跳转排队被忽略**：提示只是一条 hint，用户可能没注意 → 配合系统通知（定时路径）兜底
- **模型不写回任务卡**：依赖 prompt 约定的老问题，本方案不加重（会话记录本身就是兜底证据）

## 9. 实施偏差（2026-09-10 第一期落地记录）

1. **无新 Tauri 命令**：方案改动清单预列「lib.rs 注册新命令」，实施时复用既有
   `bot_execute_task`（语义改为新会话执行，`session_id` 参数废弃保留兼容），
   `chat-open-session` 是事件不是命令——前端无新 invoke 需求，故未新增命令。
2. **`execute_task_core` 被取代而非保留**：`run_task_in_chat`（生产薄壳）+
   `run_task_in_chat_with`（注入模型循环的泛型内核，mock runtime 可测）接管全部逻辑；
   为让 mock runtime 测试能全链路驱动，给 `open_db` / `db_load_for` / `db_upsert_for` /
   `bot_enabled` / `scan_skills` / `build_skill_block` / `broadcast_after_mutation` /
   `injection_block` / `auto_lesson_on_task_failure` 加了泛型 Runtime 接缝
   （命令签名不变，行为不变）。
3. **定时路径的 interactive 语义**：新会话执行统一 `StopGuard::new_task_exec(true, sid)`
   ——定时任务现在同样流式推 widget（按 sessionId 过滤不串台）、可被该会话的 /stop
   中断；副作用：挂件可见时，定时执行中的危险操作会弹确认窗（60s 超时默认拒绝兜底
   不变；挂件不可见时仍自动拒绝）——评审视为「用户在场可干预」特性而非回归。
4. **通知点击不跳会话**：tauri-plugin-notification 的点击动作需要额外注册，一期只发
   通知文本（标题含任务名），用户点击拉起应用后按 ⏰ 前缀会话回看；设计中的「点击
   通知打开对应会话」留到二期。
5. **TodoCard 保留 emit("execute-task")**：未改为直接 invoke——事件语义改为「新会话执行」，
   ChatPanel 监听器改为直接 `invoke("bot_execute_task")`（不再经 runChat/当前会话），
   改动面更小且去重表（execTaskDedup）原样保留。
6. **busy 提示按钮**：hint 消息加了 `actionSessionId` 字段渲染「💬 查看执行对话」按钮
   （点击 switchSession），不是纯文本提示。
7. **失败也落库**：模型循环失败时 assistant 行以「⚠️ 执行失败：…」写入执行会话
   （会话即执行记录，留证可回看）——方案未明说，实施时补上。
