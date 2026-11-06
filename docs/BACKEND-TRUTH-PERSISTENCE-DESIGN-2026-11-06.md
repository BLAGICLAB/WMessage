# 聊天持久化真源化改造方案（持久层唯一事实、前端投影）

状态：**方案，未实施**。2026-11-06 拍板：取代同日
《MSG-IDENTITY-PERSISTENCE-DESIGN-2026-11-06.md》（快照混合方案作废；其数据面
设计、消费方清单、出生点清单、业界口径作为本文素材继续被引用，见旧文 §四/§七/§十四）。

## §一 目标与动机

三件事一起根治：

1. **汇报轮 stale 快照竞态**（旧方案 §2.3）：前端快照保存与后端 append 互踩，
   汇报行可能被冲掉。根因是「前端全量快照保存」这个写模式本身——真源化直接
   拆掉它，竞态不复存在，而不是被缓解。
2. **多窗口一致**：单一真源 + 变更事件扇出，任何窗口的投影都收敛到同一份事实。
3. **前端状态机减负**：`streamState` 的世代/纪元/回灌守卫群是为「快照与流式
   混搭」付出的复杂度；投影模式下可大幅收缩。

范式依据（调研在旧方案 §一，此处只留结论）：Claude Code / Codex 的转录由
agent 进程写、UI 只读投影；大厂没有一家让 UI 数组当持久化载荷。**仓内先例
更重要**：exec 会话（创建+回复全由后端写）、汇报轮 append、子 agent 收尾落库
三条链路后端本来就是真源——真源化 = 把既有模式推广到主聊天，不是引入新架构。

顺带收益：IPC 从每轮两次全量快照缩为小操作事件；trace_id / thinking / tools /
fileChanges 的落库权归产物所有者（后端）；busy 拒答不再遗留孤立用户消息
（现状缺陷，`useChatSessionActions.ts:259` 注释自认「历史里会留一条孤立用户消息」）。

## §二 现状事实（2026-11-06 核实）

### 2.1 写点与真源归属盘点

| 写点 | 现写者 | 真源化后 |
|---|---|---|
| 主聊天用户消息 | 前端（回合开始全量保存，`useChatSessionActions.ts:241`） | 后端（`bot_chat` 落行） |
| 主聊天回合终稿 | 前端（回合终稿全量保存，`:285`；失败 `:324`） | 后端（回合终结处落行） |
| 汇报轮回复 | 后端 append（`bot_chat/report.rs:129`） | 不变 |
| exec 会话创建/回复 | 后端整段覆写（`bot_chat/exec.rs:189/239`） | 不变（内核换身份版） |
| 子 agent 收尾 wrapper+digest | 后端整段覆写（`subagent_runner.rs:805`） | 不变（内核换身份版） |
| compact/retry/remove/clean | 前端改内存数组 + 全量保存 | 后端定向操作 |

### 2.2 事件契约现状

- 统一出口 `emit_recorded`（`bot/stream_log.rs:237`）：sessionId 信封盖章 +
  seq 记号 + 会话内缓冲，`bot_stream_replay` 按 seq 回放——**新事件走此出口
  即免费获得回放能力**。例外：`bot-file-changed` 自盖章不进缓冲（模块头有因）。
- 流式事件清单：`bot-chat-delta` / `bot-think-delta` / `bot-tool-running` /
  `bot-tool-done` / `bot-plan` / `bot-confirm` / `bot-turn-status` /
  `bot-chat-report` / `bot-skill-failed` / `bot-file-changed`。
- 前端 `streamState` 用这些事件累积 meta（thinking/tools/fileChanges/正文），
  回合终稿时**并流进全量保存**——聚合结构只存在于前端内存。

### 2.3 bot_chat 契约现状

`bot_chat(messages: Vec<ChatMsg>, session_id, reasoning_effort)`（`bot_chat.rs:182`）
——**完整历史由前端传入**；返回 `BotChatResult{text, task_refs, busy, trace_id}`。
会话级防重入 `ChatGuard` 已存在（busy 拒答语义靠它）。

### 2.4 后端读历史先例

`report_turn_history`（`bot_chat/report.rs:98`）已经从 DB 读上下文构造模型输入
——「后端读库建上下文」不是新能力，是推广。

### 2.5 终稿聚合缺口

thinking / tools / fileChanges 目前只活在前端 streamState meta（事件累积），
后端没有回合级聚合结构——**B2 需要新建聚合**，这是本方案最大的新代码面。

## §三 目标架构与不变式

```
DB 行（唯一真源） ──变更事件（msg-upserted / msg-dropped）──▶ 各窗口投影
     ▲                                                          │
     └────────── 后端写路径（回合/操作/汇报，全部单事务） ◀── 前端只发意图
```

四条不变式：

1. **真源唯一**：消息的一切持久变更由后端在事务内完成；前端不发全量快照、
   不持有可覆盖真源的写权。
2. **投影可重建**：任何时刻 `bot_history_load` 整段重读即恢复投影——事件丢失、
   窗口重开、版本升级都靠这条兜底，不做事件游标。
3. **乐观可见**：用户消息本地先显（带预生成 id），事件回显同 id 对账；对不上
   即整段 reload。
4. **写权收敛**：会话内同一时刻至多一个写者（ChatGuard 已有）；操作类写
   （drop/replace）与回合写同走 DB 写锁 + 变更事件。

## §四 数据面（继承旧方案，标注差异）

- **列与索引**：`msg_id TEXT` / `ord INTEGER NOT NULL DEFAULT 0` /
  `trace_id INTEGER` 三列 + 唯一索引 `(session_id, msg_id)` + 复合索引
  `(session_id, ord)`（含「现状除主键零索引」的核实结论）。唯一索引是**全列**
  而非旧方案 §4.1 的部分索引——偏离登记 §十五-1：SQLite 中 NULL 相异，存量
  NULL id 行彼此不冲突，无需 WHERE 谓词，也避开 ON CONFLICT 目标子句与部分
  索引 WHERE 匹配的版本兼容坑。
- **`created_at` 首插时刻语义**：同旧方案 §4.2（upsert 不列该列即保旧值）。
- **`turn_seq`**：规则同旧方案 §八（新用户行 MAX+1、助手行按 ord 继承、
  汇报 append 自成一轮），实现仍挂工作项重构窗口，本方案不落。
- **差异 1（id 生成权）**：旧方案前端十处出生点生成 id；真源化后——
  用户消息 id 由**前端预生成**（乐观对账用，随 `bot_chat` 传入），
  其余行一律**后端生成**。前端出生点收缩为三类（§八）。
- **差异 2（ord）**：仍由后端内核盖章；定向操作（drop/replace）后受影响区段
  重排。
- 老库口径：同旧方案 §4.3（不迁移，NULL 行首次保存自然换血）。

## §五 后端写路径

### 5.1 内核两入口（继承旧方案 §5.1，重新划定使用者）

- **upsert 行内核**：按 `(session_id, msg_id)` upsert 单行/少行，保
  `created_at`，盖 `ord`。使用者：用户消息落行、回合终稿落行、汇报 append
  （append 语义 = 无条件 INSERT，同旧方案）、定向操作的受影响行。
- **snapshot-diff 内核**：全量 upsert + 差集删除，**仅限后端内部整段重建**
  （exec 会话重跑、子 agent 会话收尾）——前端合约里它已死（§六），不构成
  误用面。旧方案对「部分快照误用」的担忧随前端合约退役消解。差集删除的
  id 清单经 `json_each` 单参数下发（偏离登记 §十五-2，不用临时表）。

### 5.2 回合落行时序（主聊天）

```
bot_chat(session_id, content, client_msg_id, ...):
  ChatGuard acquire（busy → 拒答，不落任何行）        ← 现状孤立用户消息缺陷在此消失
  ① 事务 A：upsert 用户行（id=client_msg_id, ord=MAX+1, created_at=now）
      → emit bot-msg-upserted（用户行）
  ② run_model_loop（流式事件照旧）
  ③ 回合终结处：聚合回合记录（§5.3）→ 事务 B：upsert 终稿行
      → emit bot-msg-upserted（终稿行）——恒在 bot-turn-status 终态事件之后
```

### 5.3 回合终稿聚合（B2 核心新代码）

模型循环终结处聚合一份回合记录 → `BotMsgRow`：

| 字段 | 来源 |
|---|---|
| content / task_refs | 循环已有（`BotChatResult`） |
| trace_id | 循环已有（`exec_traces` 行号） |
| thinking | `bot-think-delta` 发射点累积成段 |
| tools | `bot-tool-running/done` 发射点累积成 `ToolCall[]`（id/name/args/result/ms/ok） |
| fileChanges | `bot-file-changed` 发射点累积（注意其自盖章不进缓冲的例外，聚合要在发射侧挂） |
| plan/skillFailure | `bot-plan` / `bot-skill-failed` 有终态载荷的回合按需聚合 |

聚合必须覆盖全部发射点——漏一个发射点即静默丢字段，逐字段锚点测试钉住（§十二
批 3）。聚合结构只服务落库，不改变流式事件语义。

### 5.4 定向操作（新 tauri 命令，B3）

| 命令 | 语义 | 替代的现状 |
|---|---|---|
| `bot_history_drop(msg_id)` | 按行删（removeMessage） | 前端数组 filter + 全量保存 |
| `bot_history_drop_after(msg_id)` | 删该行之后所有行（retry 截尾；用户行复用同 id） | 前端 slice + 全量保存 |
| `bot_history_replace_summary(msg_ids[], summary_row)` | 摘要行换血（compact） | 前端构造摘要数组 + 全量保存 |
| `bot_history_clear` | 已有，不动 | /clean |

每个操作：单事务 + 受影响行 ord 重排 + `bot-msg-dropped` / `bot-msg-upserted`
事件。bot_chat 的 /retry 链路改为「先 op 再起回合」由前端编排。

## §六 bot_chat 契约改造（B1）

- 入参：`messages: Vec<ChatMsg>` 退役 → `content: String`（前端拼好的
  附件/引用块原文，意图路由解析不变）+ `client_msg_id: String` +
  `session_id` + `reasoning_effort`。
- 上下文来源：后端读 DB（§2.4 先例推广）；用户行先落（§5.2 ①）再读，顺序
  在后端内天然成立。
- 返回 `BotChatResult` 不变；终稿行的到达通知改由 `bot-msg-upserted` 事件
  承担，前端 done 路径从「用返回值拼终稿」改为「等事件收行」（返回值仍用于
  busy 判定与错误处理）。
- `bot_compact(messages)` 入参同步退役（读库 + 摘要内核复用）。
- 波及面核对清单：`inject.rs`（注入）、`intent_router`（意图块解析——content
  原文照传，理论零波及，回归测试钉住）、图片附件路径、`exec.rs` 执行轮。

## §七 事件契约

- 新增 `bot-msg-upserted`：载荷 = 整行投影（msgId/ord/role/content/refs/thinking/
  tools/turnStatus/traceId/createdAt），走 `emit_recorded`（进缓冲、有序号）。
- 新增 `bot-msg-dropped`：载荷 = msgId 数组，同上。
- **顺序约定**：终稿 upserted 恒在该回合 `bot-turn-status` 终态事件之后发出
  （投影替换占位气泡依赖此序）；用户行 upserted 恒在首个流式事件之前。
- 投影失步恢复 = 整段 reload（`bot_history_load`），**不做事件游标**——事件
  只是实时增量，正确性永远以 reload 为准（不变式 2）。
- `docs/acceptance/event-names.md` 白名单同步两个新事件名。

## §八 前端投影

### 8.1 投影机制

`messages` = 切会话时 load 一次 + 事件 reducer：
- `bot-msg-upserted`：按 msgId 替换；不存在则按 ord 插入。
- `bot-msg-dropped`：按 id 删。
- 本视图不活跃的会话不持有投影（现状同款），切换时 load + 回放缓冲尾部。

### 8.2 前端出生点收缩（旧方案 §7.2 十处 → 三类）

| 类 | 处置 |
|---|---|
| 乐观用户行 | send 时本地插入，id 预生成随 `bot_chat` 传；upserted 回显同 id 对账，busy 拒答则撤行 + hint |
| hint 行 | **显式降格声明：UI 装饰，永不入真源**；本地 id 仅作 React key |
| 流式占位气泡 | 事件驱动；终稿 upserted 按 §七顺序约定替换占位 |

### 8.3 streamState 收缩清单（诚实标注：收缩而非消灭）

- **退役**：turn-start / done / failed 三处全量保存的并流收尾（`persistHistory`
  及 `bot_history_save` 命令断代删除）；mergeInflight / resumeInflight 里
  「权威副本并入终稿」的整段逻辑。
- **保留**：inflight 标记、流式 meta（正文/思考/工具行在途累积仍服务占位气泡）、
  `bot_stream_replay` 接回、终态对账的世代闸（迟到终态仍需防旧轮污染新轮——
  范围从「保护快照」缩到「保护占位替换」，闸保持但职责变轻）。

## §九 多窗口一致（C 段）

- 事件扇出：`emit_to("main")` → 广播（`emit_all` 或窗口注册表）；投影每窗口
  独立，靠同一事件流收敛。
- **本方案只做「扇出就绪」**：多窗口的 UI（窗口管理、每窗会话路由、焦点与
  滚动语义）不在本方案，另行立项。
- 单窗口今天就受益：投影与真源永不背离（现状 loadFinalText 的各处补偿逻辑
  从「必需品」降为「兜底」）。

## §十 迁移与断代

- 列/索引迁移：同旧方案 §4.1/§4.3（`ensure_columns` + 基线 DDL 同步）。
- **断代删除**：`bot_history_save` 命令、`persistHistory`、`bot_compact`
  的 messages 入参——不留兼容层（测试阶段口径）。旧文档（快照混合方案）
  批 3 的「前端出生点带 id」大部分随之作废，仅乐观行与 hint 保留。

## §十一 不做清单

- **append-only 全量事件溯源**：真源化 ≠ 事件溯源——DB 行仍是状态存储，
  不做 replay-everything 的日志层（Codex/Cursor 爆盘教训仍有效）。
- 消息树 / parentUuid 分支。
- 流式增量逐字落库（仍回合终稿一次性落行；中途崩溃丢失在途正文为现状同款）。
- hint 行入真源。
- exec 会话多轮化（每次执行重建语义不变）。
- 事件游标 / 投影增量同步协议（reload 兜底已够，见不变式 2）。
- 多窗口 UI。

## §十二 批次切分与锚点测试

| 批 | 内容 | 锚点 |
|---|---|---|
| 批 1 数据面 | 三列 + 两索引 + 迁移；内核两入口（upsert 行内核 / snapshot-diff 内核） | ① upsert 保 created_at；② snapshot-diff 清快照外行与 NULL 行；③ 事务回滚整体性；④ 截断审计口径；⑤ append 取 MAX(ord)+1；⑥ 索引存在且迁移幂等 |
| 批 2 契约切换（B1） | `bot_chat` 新入参；用户行后端落行；`bot-msg-upserted`（用户行）；前端乐观行对账；busy 不落行 | ⑦ 新契约拒绝旧 messages 入参（编译期）；⑧ 用户行按 client_msg_id 落行且事件回显同 id；⑨ busy 拒答零落行（现状孤立用户消息缺陷转绿）；⑩ 意图路由回归（附件/引用块原文照传）；⑪ 乐观行对不上触发整段 reload |
| 批 3 终稿聚合（B2） | 回合终结聚合 thinking/tools/fileChanges/trace_id；终稿落行；终稿 upserted 恒在 turn-status 后 | ⑫ thinking 逐段聚合；⑬ tools 行逐字段（id/name/args/result/ms/ok）；⑭ fileChanges 聚合（含自盖章事件）；⑮ 无工具/无思考回合聚合空值口径；⑯ 事件顺序断言（upserted 晚于终态）；⑰ 重启后 thinking/tools 从库还原 |
| 批 4 定向操作（B3） | 四命令 + msg-dropped；persistHistory/bot_history_save 删除；compact 入参改 | ⑱ drop 单行；⑲ drop_after 截尾复用用户行 id；⑳ replace_summary 换血；㉑ 每操作事件载荷正确；㉒ **汇报竞态复现测试由红转绿**（后端 append 与「前端保存」并发的构造不再可能）；㉓ 全仓无 bot_history_save 调用残留 |
| 批 5 内部重建收口 | exec/子会话收尾换 snapshot-diff 身份版内核；投影失步恢复口径统一为 reload | ㉔ exec 重跑行序与事件流；㉕ 子会话收尾 wrapper/digest 带 id；㉖ replay 后整段 reload 兜底路径 |
| 批 6 扇出就绪（C） | emit_to→广播改造；投影多窗口收敛验证 | ㉗ 双投影收敛（同事件流两 reducer 终态一致）；㉘ 扇出不破坏现有单窗口过滤 |

批次依赖：1 → 2 → 3 → 4 → 5 → 6 严格串行（每批退役一段前端写权）。全程
`cargo test --lib` + `npm test` + 行数门禁；批 2/4 动事件名与命令面，同步
`docs/acceptance/` 三件套（event-names / matrix / cards）与 SPEC 功能条目。

## §十三 拍板项

- **T1 exec/子会话行 id 口径**：推荐服务端生成（每轮重建换 id，投影反正整段
  reload）；备选槽位 id（`exec:{task_id}:{origin}:user/reply`）原地更新不换 id。
- **T2 `bot_history_save` 断代删除确认**：推荐删（不留兼容层，测试阶段口径）。
- **T3 批 6 范围**：推荐只做「扇出就绪」（emit 广播化），多窗口 UI 另立项——
  多窗口一致的数据面就绪 ≠ 多窗口产品面开做。
- **T4 失步恢复一律整段 reload**：推荐确认（不做事件游标，理由见 §七）。

## §十四 风险清单

1. **终稿聚合漏字段**：发射点枚举不全即静默丢——批 3 逐字段锚点 + 聚合处
   注释列全发射点清单（代码注释只写约束，不写日期决策）。
2. **事件顺序约定被打破**：终稿 upserted 早于终态事件会破坏占位替换——顺序
   断言测试钉住（⑯）。
3. **契约切换波及面**：inject/intent/图片附件/exec 轮四路径，批 2 回归清单
   逐项跑。
4. **前端测试大改**：ChatPanel 系列数千行 mock 从「save/load 快照」换「事件
   投影」口径——工作量主体在批 2-4，预估与运行时改造批 1-8 同量级。
5. **汇报竞态复现测试构造**：现状竞态窗口靠时序，红转绿测试要先能稳定构造
   红态（批 4 ㉒ 的前置功课；构造不出红态则改为白盒断言「前端写权已不存在」）。
6. **单副本事实显性化**：真源化后 DB 是聊天记录唯一副本（现状其实也是）——
   无恶化，但「投影可重建 ≠ 数据可重建」，备份责任写明。

## §十五 偏离登记

- **偏离 1（批 1，已回写 §四）**：`(session_id, msg_id)` 唯一索引由部分索引
  改全列索引。原因：SQLite 中 NULL 相异，存量 NULL id 行彼此不冲突，全列索引
  天然兼容老库迁移；部分索引要求 `ON CONFLICT` 目标子句带匹配 WHERE 谓词，
  是纯增的版本兼容坑。影响面：零（语义等价，锚点⑥ 已钉 NULL 存量共存）。
- **偏离 2（批 1，已回写 §5.1）**：差集删除的 id 清单走 `json_each`
  单参数下发，不用旧方案 §5.1 伪码的临时表。原因：rusqlite 0.37 bundled（SQLite
  ≥3.45）`json_each` 为核心功能，单参数天然无变量上限；临时表的生命周期约束
  （旧方案风险清单的「临时表与连接池」条目）随之整体消失。影响面：批 1 锚点
  2000 行差集进出已实测通过。
- **批 1 附带（口径统一）**：`bot_messages` 建表 DDL 收为单源常量
  `bot_history::BOT_MESSAGES_DDL`（基线与测试建表共用），消灭 7 处手写
  schema 的漂移面；`bot_history_load_inner` 读序换 `ORDER BY ord, id`（存量
  行 ord 恒 0 落回 id 序，行为不变）、SELECT 带回 `msg_id`（读回保留是
  前后端身份交接的达成机制，提前到数据面批次一并落）。
