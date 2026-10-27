# 流式事件信封：会话号只在统一出口盖章（计划浮层不实时刷新的根因修复）

> 2026-10-27 · 现行实现口径见 `SPEC.md`「执行计划浮层」条。
> 本文只写「为什么这么切」与「哪些坑踩过」——代码结构与判据以源码为准。

## 一、现象与根因

**现象**：聊天窗口右上角的执行计划浮层不实时刷新。模型更新计划后，浮层还停在
旧状态；有时要关掉再点开，才看到计划走到哪里了。

**根因不是前端没订阅，是事件载荷少了一个字段**。

`bot-plan` 事件的广播路径：

```
update_plan 工具
  → plan_state::set_plan()          写会话计划表（后端权威状态）
  → plan_state::plan_payload()      组装业务载荷：steps / explanation / updatedAt / turnStartedAt
  → stream_log::emit_recorded()     写会话缓冲 + 广播给 main 窗口
  → 前端 usePlanState 监听 "bot-plan"
```

两处口径对不上：

| 位置 | 做了什么 |
|---|---|
| `plan_payload()` | 只组装业务载荷，**没有 `sessionId`** |
| `emit_recorded()` | 只往载荷里塞 `seq`；`session_id` 参数只用于写会话缓冲，**不回灌到广播载荷** |
| 前端 `usePlanState` | `const sid = p?.sessionId ?? null; if (!sid \|\| sid !== …) return;` |

于是**每一条实时 `bot-plan` 事件都在前端被无条件丢弃**（`sid` 恒为 `null`）。

浮层的数据只有两条来路：

1. 实时 `bot-plan` 事件 → **全被丢弃**；
2. 挂载 / 切会话时拉的 `bot_plan_state` 快照 → 这条是好的。

所以：打开浮层 → 拉到当下快照 → 模型继续 `update_plan` → 浮层纹丝不动 →
关掉再点开 → 重新挂载再拉一次快照 → 进度才出现。这就是「有时要重新点开」的成因。

### 1.1 为什么两侧测试都没抓到

- **前端** `PlanPanel.test.tsx` 手工造的 `bot-plan` 载荷**带了 `sessionId`**——
  测的是一份现实中不存在的载荷形状，天然测不出。
- **后端** `plan_state.rs` 的 `payload_carries_steps_and_timestamps` 只断言
  `steps / updatedAt / turnStartedAt`，也不断言信封字段。
- **回放路径**同样漏：`useChatHistory` 按 `handlers[ev.event]` 分发补齐事件，
  而处理器表里**没有 `bot-plan`**——重连 / 补齐也不会刷新计划面板。

### 1.2 本质：信封盖章权被拆散了

不只是「漏了一个字段」。会话号这个信封字段，此前由各发射点**各自手写**
（`shell.rs` / `flow.rs` / `report.rs` 各写一遍），`plan.rs` 忘了写 →
事件静默丢失，无告警、无测试能抓到。

这正是业界一致反对的反模式：**每个生产者自己拼信封**。

## 二、业界怎么做（调研）

| 规范 / 系统 | 做法 | 出处 |
|---|---|---|
| **ACP**（Agent Client Protocol） | `session/update` 通知把 `sessionId` 放在 **params 外层（信封）**，计划对象保持纯 `{type, planId, entries}`；且「Agent MUST send a complete list of all plan entries in each update，Client MUST replace the current contents of that plan completely」——整表替换是规范硬要求 | agentclientprotocol.com/protocol/v2/agent-plan |
| **CloudEvents**（CNCF） | 事件 = 信封属性（`id` / `source` / `type` / `time`）+ `data`（业务载荷）。信封标准化的目的就是让路由器 / 日志 / 去重工具**不解析业务载荷**就能路由与去重 | jsonkit.in/blog/cloudevents-json-standard；kihiujohn.github.io/blog/cloud-events-spec |
| **Kafka** | record = Metadata（offset / partition / timestamp / **headers**）+ Payload（key / value）。Confluent 把 schema-id 前缀**从 payload 挪进 header**，理由原文是「the payload stays untouched，which is what makes a topic readable by tooling that knows nothing about the registry」 | tpointtech.com/kafka-record；hupe1980.github.io/schemreg |
| **行业 AI 流式**（Spring AI + Redis Pub/Sub） | 消息信封定为 `{eventId, sessionId, messageId, sequence, type, content, timestamp, finished}`：会话号 / 序号 / 去重 ID 都是**信封级字段**，由发布层统一盖章；频道按 `sessionId` 哈希分片；断线补拉按 `(sessionId, lastSequence)` | kwkr.cn/news/587851 |
| **Vercel AI SDK**（对照，本项目用不上） | data stream 每条对话一条 SSE 流，消息是带 `type` 判别子的分段——靠**连接隔离**，不在每条消息里带会话号 | v6.ai-sdk.dev/docs/ai-sdk-ui/stream-protocol |

**归纳出的三条共识**：

1. 会话标识属于**信封层**，不属于业务载荷（ACP / CloudEvents / Kafka 一致）；
2. 信封**只能有一个盖章处**——「每个生产者自己拼信封」是公认的反模式；
3. 客户端拿信封字段**路由与去重**，不解析业务载荷。

**为什么不能抄「连接隔离」那条**：Vercel AI SDK 每条对话一条流、Redis 一会话一频道，
都要求「一个消费端只收自己那条流」。本项目是**单个 main webview 收全部会话**，
连接级隔离用不上，只能走「信封 + 客户端按会话过滤」——这也正是 ACP 的做法。

## 三、本次的口径

### 3.1 信封单点盖章

`stream_log::emit_recorded` 是会话级流式事件的**唯一出口**，信封字段在这里盖：

```
载荷顶层 = 信封 + 业务字段
  信封（出口统一盖章，调用方不许自带同名键）
    sessionId   会话号——客户端按会话过滤靠它
    seq         会话内事件序号——补齐与实时去重靠它
  业务字段（各事件自带）
    steps / explanation / updatedAt / turnStartedAt   （bot-plan）
    text / thinking / …                              （流式增量）
```

调用方不再自带 `sessionId`：`shell.rs` / `flow.rs` / `report.rs` 三处手写注入
同提交删除（`shell.rs` 的 `emit` 闭包保留「只给交互实例发流」的闸门，只删盖章两行）。

> 口径写在 `stream_log.rs` 的函数注释里：信封只此一处盖章，业务字段不许自带同名键。

### 3.2 前端：状态外置 + 两条来路写同一份

计划的**变化流**（实时事件）与**状态源**（命令快照）此前各写各的组件 state，
晚到的那份能把新的盖掉；浮层又是条件挂载的，关着时发生的更新没人收。

改为模块级 store（`planStore.ts`，与 `streamState.ts` 同款做法）：

- **按会话归位**：每会话一份，别的会话的计划天然不会显示（切会话不闪旧数据）；
- **单调闸**：命令回包可能比先到的更新更旧（拉取途中模型又改了一次），
  按 `updatedAt` 比新旧，旧的不许盖新的；
- **两条来路写同一份**：实时订阅与「回放补齐」共用**同一批处理器**
  （`useChatStream` 的 `handlers` 表），回放出来的计划事件也落到浮层；
- **快照为 null 不回写**：回包在途中时模型又更新过一次，这时回包是旧的——
  拿它清空就把刚到的更新抹掉了。

面板读 store 用 `useSyncExternalStore`（外部 store 的标准读法，
不在 effect 里同步 `setState` 派生渲染值）。

### 3.3 刻意不做

- **不做两级信封**（`{sessionId, data: {...}}`）：前端十来个事件处理器都读顶层
  `sessionId` / `seq`，改成嵌套是一次纯搬移的大改，收益只有可读性。
  当前口径是「载荷顶层即信封」，与 `shell.rs` 既有写法一致。
- **不给计划加序号闸**：计划是整表状态，按 `updatedAt` 判新旧更贴切；
  序号闸是给流式增量用的（内容判重做不到那里）。
- **不接轮询**：事件到了就刷新，不需要定时问后端。

## 四、落地

### 批 1 · 后端信封收口

| 文件 | 改动 |
|---|---|
| `src-tauri/src/bot/stream_log.rs` | `emit_recorded` 统一注入 `sessionId`（盖章发生在写缓冲之前，缓冲与广播同源）；加回归测试 |
| `src-tauri/src/bot_model_loop/shell.rs` | 删手写注入，保留 `stream_to_main` 闸门 |
| `src-tauri/src/bot_chat/flow.rs` | 删载荷里的 `"sessionId"` |
| `src-tauri/src/bot_chat/report.rs` | 删载荷里的 `"sessionId"` |
| `src-tauri/src/bot/tools/plan.rs` | 回归测试：`bot-plan` 载荷必须带 `sessionId` |

### 批 2 · 前端分发补齐

| 文件 | 改动 |
|---|---|
| `src/components/ChatPanel/planStore.ts` | 新增：模块级 store（按会话 + 单调闸 + 订阅） |
| `src/components/ChatPanel/usePlanState.ts` | 改读 store（`useSyncExternalStore`）；快照回包写回 store |
| `src/components/ChatPanel/useChatStream.ts` | 注册 `bot-plan` 处理器（实时与回放补齐共用一份） |
| `src/components/ChatPanel/PlanPanel.tsx` | 类型改从 `planStore` 引 |
| `src/components/ChatPanel/useChatSessionActions.ts` | 删会话时一并清掉该会话的计划 |

## 五、验收口径

1. **后端信封**：`stream_log` 的 `emit_recorded_stamps_session_id_onto_payload`
   与 `plan.rs` 的 `writes_snapshot_records_event_and_acks_model`——
   撤掉盖章即红（已实测红→绿）。
2. **前端实时刷新**：`ChatPanel.plan.test.tsx` 的
   「bot-plan 事件到达 → 浮层当场刷新（不用关掉再点开）」——
   断掉 store 通知即红（已实测红→绿）。
3. **回放补齐**：`ChatPanel.plan.test.tsx` 的
   「在途回灌：回放里的 bot-plan 事件也要落到浮层」。
4. **口径单源**：`planStore.test.ts` 覆盖单调闸、按会话归位、
   快照为 null 不回写、删会话清理。

## 六、踩坑记录

- **测试造出来的载荷形状 ≠ 真实载荷形状**：前端测试手工造的 `bot-plan` 载荷带了
  `sessionId`，于是「载荷契约」这一层没有任何测试罩住。修好后两端各补一条
  断言信封字段的用例（后端断言真实载荷，前端断言事件到达即刷新）。
- **删掉手写注入 ≠ 删掉闸门**：`shell.rs` 的 `emit` 闭包兼着「只给交互实例发流」
  的会话隔离闸门——那部分必须留，删的只是盖章两行。
- **`streamState.dropSession` 不能顺手挂计划清理**：它在**每轮收尾**时调用
  （清 meta / 游标 / 在途标记），挂上去会让计划每轮结束就消失。
  计划的清理挂在**删会话**那条路上。

## 七、落地结果

| 批次 | 内容 | commit |
|---|---|---|
| 批 1 | 后端信封收口（`emit_recorded` 统一盖章 + 删三处手写注入）+ 两处回归测试 | `4cc39fe7` |
| 批 2 | 前端 store 外置 + 回放分发补齐 + 三处测试 | `800b1f7f` |

**门禁**：快检（`scripts/test-fast.sh`）与全量（`scripts/test-all.sh`：cargo nextest +
vitest 117 文件 / 1138 用例 + tests-audit 41 项对拍）均通过。

**未覆盖 / 留给实机**：本轮只改事件载荷与前端分发，**未做实机复验**——计划浮层
的实时刷新要在真机上跑一次多步任务（模型边做边 `update_plan`）才算端到端验过；
自动化用例守的是「事件到达 → 浮层当场变」这一段链路。

