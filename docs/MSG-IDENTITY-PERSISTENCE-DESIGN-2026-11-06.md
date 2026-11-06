# 聊天消息身份落库改造方案（快照协议 + 按 id upsert 混合）

状态：**已作废（2026-11-06 拍板），被
《BACKEND-TRUTH-PERSISTENCE-DESIGN-2026-11-06.md》取代**——不再走「前端快照
协议」，直接真源化。本文以下内容仍被新方案引用、继续有效：§二 现状事实与
消费方清单、§四 数据面（列/索引/created_at 换轨/老库口径）、§7.2 消息出生点
清单（新方案收缩为三类）、§八 turn_seq 联动规则、§十四 后续梯队与存量备查
登记表（含条带 A 方案待修）。§五/§六/§七 中与「前端发快照」相关的内核口径
已被新方案 §5.1 重新划定（snapshot-diff 仅限后端内部重建）。

## §一 背景与动机

三条线会诊到同一处：

1. **子 agent 条带漂移**（用户实机报告）：完成的子 agent 条带挂「最后一条助手回复」，
   每条新指令都再现。根因之一是消息无稳定身份，无法表达「这条回复属于哪一轮」。
2. **消息身份调研**：`bot_messages` 已有 `id`（AUTOINCREMENT）与 `created_at` 两列，
   但落库是全量覆写（DELETE 全部 + 重插），id 每轮重排、created_at 全批同刻——
   两列都是假身份，透传即误导。
3. **工作项重构联动**（`docs/WORK-ITEM-REDESIGN-PLAN-2026-11-04.md`）：其统一键
   `(session_id, turn_seq)` 若有消息行的身份列做同源对齐，「一条消息一条流水」的
   追踪矩阵才能落到位。

业界口径（调研结论，详见对话记录）：Claude Code / Codex 走 append-only 事件日志
（uuid + parentUuid），Cursor 走 SQLite 按消息 id upsert blob，LangGraph 的
`add_messages` reducer 把「同 id upsert、无 id append」做成默认语义，OpenAI
Conversations API 以 item id + 父指针表达派生。**共同纪律：身份出生即有、终身不变；
持久层是唯一事实。没有一家把 UI 当前数组当持久化载荷整段重写。**

本方案取 Cursor 派的 SQLite 落法，但保留 wmessage 快照协议的真源归属：**前端协议
不改（仍发全量快照），只把存储写法从覆写换成按 id upsert + 差集删除**。

## §二 现状事实（设计依据）

### 2.1 写路径

- 唯一写内核：`bot_history_save_inner`（`db/bot_history.rs:48`）——单事务内
  `DELETE WHERE session_id` 全删 + 逐行 INSERT，超 2000 条截最旧（`MAX_HISTORY_MSGS`），
  截断写审计 `chat.history_save_truncated`。
- 前端保存：`persistHistory`（`useChatHistory.ts:37`）发全量 `Msg[]`（滤掉 hint 行），
  正常一轮至少两次：回合开始存用户消息（`useChatSessionActions.ts:241`）、
  回合终稿存 done（`:285`），失败路径 `:324`。
- 后端全量保存（同一内核，行不带 id）：执行会话创建（`bot_chat/exec.rs:189`，
  1 行任务块）、执行回复落库（`exec.rs:239`，[任务块, 回复] 两行整体覆写）、
  子 agent 收尾 wrapper+digest 两行（`subagent_runner.rs:805`，覆写子会话）。
- 后端追加（不走覆写内核）：汇报轮 `bot_history_append_inner`
  （`db/bot_history.rs:159`，纯 INSERT，防「读回快照与并发落库互踩」），
  调用点 `bot_chat/report.rs:129`。
- 清空：`bot_history_clear`、会话删除连带 `DELETE FROM bot_messages`
  （`db/bot_sessions.rs:196`）。

### 2.2 读路径与消费方清单

| 消费方 | 排序依赖 | 改造影响 |
|---|---|---|
| `bot_history_load_inner`（`bot_history.rs:16`） | `ORDER BY id` | 换 `ORDER BY ord, id` |
| 记忆抽取 `memory/extract.rs:676` | `ORDER BY id DESC LIMIT N` | 换 `ord DESC`；只取 role/content，无列变更 |
| 汇报轮取数 `bot_chat/report.rs:98` | 经 load，只读 role/content | 无 |
| 旧库迁移检查 `db/mod.rs:334` | COUNT | 无 |

前端 `rowsToMsgs`（`useChatHistory.ts:72`）把行映射成 `Msg`，消息无 id 字段；
`MessageList` 用数组序当 React key（`MessageList.tsx:660-661`，注释说明「合成 id
会改消息数据流」指的是**拆分搬移红线**——前端生成 id 的禁令语境，不是产品决定）。

### 2.3 已知竞态（本方案保持现状，不扩大不修复）

汇报轮 append 与前端快照互踩：前端在内存快照里没有汇报行时保存，汇报行被冲掉。
现状靠终态事件触发 `loadFinalText` 回读缓解。upsert + 差集删除的语义与覆写
**逐一对齐**（快照里没有 = 删），竞态面不变。

## §三 方案总则（四条不变式）

1. **快照协议不变**：前端仍发全量数组，`bot_history_save` 命令签名不破坏性变更；
   retry/compact/截断/删行的前端语义一行不动。
2. **单事务原子性不变**：upsert + 差集删除 + updated_at 推进包同一事务，
   中途失败全回滚（对应既有回归 `bot_history_save_rolls_back_on_insert_failure`）。
3. **真源归属不变**：前端内存数组仍是权威快照；持久层是它的镜像，只是镜像方式
   从「重画」变「按 id 对齐」。
4. **行为与覆写逐一对齐**：不引入任何「快照外保留」新规则，不修既有竞态。
   覆写语义下会发生的事（截断冲旧行、retry 冲失败回复、stale 快照冲汇报行）
   upsert 语义下一件不多一件不少。

## §四 数据面设计

### 4.1 列与索引

```sql
-- 迁移走 ensure_columns（db/mod.rs:86，幂等补列）：
ensure_columns(conn, "bot_messages", &[
    ("msg_id", "TEXT"),                    -- 消息身份；NULL = 旧库存量行
    ("ord",    "INTEGER NOT NULL DEFAULT 0"), -- 快照序（落库时按数组下标盖章）
    ("trace_id", "INTEGER"),               -- 拍板项 D2；None = 无 trace
]);
-- upsert 冲突目标需要的唯一索引（部分索引，兼容 NULL 存量行）：
CREATE UNIQUE INDEX IF NOT EXISTS idx_bot_messages_sid_msgid
    ON bot_messages(session_id, msg_id) WHERE msg_id IS NOT NULL;
-- 复合索引：现状 bot_messages 除主键外零索引，所有按 session_id 的读写全走
-- 全表扫；upsert 查找、差集读、DELETE WHERE session_id 与 load 排序四处受益
CREATE INDEX IF NOT EXISTS idx_bot_messages_sid_ord
    ON bot_messages(session_id, ord);
```

- 新装库：`db/mod.rs` 基线 DDL（`bot_messages` 建表处）同步加三列，建两索引；
  老库走 `ensure_columns` + 建索引（均幂等）。两处口径必须一致（同 `subagents`
  单源 DDL 的教训）。
- 复合索引 `(session_id, ord)` 随批 1 建：`bot_messages` 现状除主键外零索引
  （已核实），save 的 `DELETE WHERE session_id`、upsert 查找、差集读与 load
  排序四处受益；索引维护随 upsert 同批发生，无额外全表写。
- `created_at` 列已存在（NOT NULL），**语义换掉**：从「本批覆写时刻」改为
  「首插时刻」。实现上免费——upsert 的 `DO UPDATE SET` 不列 `created_at` 即保旧值，
  新行 INSERT 取当刻。

### 4.2 字段语义表

| 字段 | 生成方 | 出生时机 | 终身变化 |
|---|---|---|---|
| `msg_id` | 前端（前端写的行）/ 后端（后端写的行） | 消息对象出生（含流式占位气泡）；存量行 NULL | 永不变；同一逻辑消息跨轮、跨重启、跨前后端交接不变 |
| `ord` | 后端内核 | 每次保存按数组下标重盖 | 每次保存可变（快照序本来就随数组变） |
| `created_at` | 后端内核 | 首次 INSERT | 永不变（首插时刻） |
| `trace_id` | 前端透传（`runChat` 已持有 `full.traceId`） | 回合终稿保存时随行 | 随行覆盖（终稿以最后一轮为准） |

### 4.3 老库口径

测试阶段不迁移（现行口径：旧数据不对齐、同类问题删旧数据）：存量行 `msg_id` NULL，
读回照常（前端给它们现场生成 id）；该会话**下一次保存**时，差集删除把 NULL id 行
清掉，内容以新 id 重插（首插时刻= 当刻，原值本就是批时刻无信息可丢）。永不保存的
会话保持 NULL 行，读路径无感。

## §五 写路径设计

### 5.1 内核改造（`bot_history_save_inner`）

```
fn save_snapshot(tx, sid, rows):
    now = now_ms()
    -- 1) 收集本批 id 进临时表（避开 SQLite 变量上限；2000 行远超旧默认 999）
    CREATE TEMP TABLE IF NOT EXISTS incoming(msg_id TEXT PRIMARY KEY);
    DELETE FROM incoming;
    for row in rows: 若 row.msg_id 为 None 则现场生成服务端 id 并回填
    INSERT INTO incoming 批量写入全部 id

    -- 2) upsert：新行取首插时刻，旧行保 created_at
    for (i, row) in rows:
        INSERT INTO bot_messages (session_id, msg_id, ord, role, content,
                                  refs, thinking, tools, turn_status,
                                  trace_id, created_at)
        VALUES (?sid, ?id, ?i, ...)
        ON CONFLICT(session_id, msg_id) WHERE msg_id IS NOT NULL DO UPDATE SET
            ord = excluded.ord, role = excluded.role, content = excluded.content,
            refs = excluded.refs, thinking = excluded.thinking,
            tools = excluded.tools, turn_status = excluded.turn_status,
            trace_id = excluded.trace_id
            -- 故意不列 created_at：保首插时刻

    -- 3) 差集删除：库里不在本批的（含 NULL id 存量行）清掉
    DELETE FROM bot_messages
     WHERE session_id = ?sid
       AND (msg_id IS NULL OR msg_id NOT IN (SELECT msg_id FROM incoming));

    -- 4) 截断口径不变：超 2000 条先裁最旧再进 upsert（被裁行不在本批 → 第 3 步自然删除）
    -- 5) UPDATE bot_sessions SET updated_at（现状保留）
    DROP TABLE incoming;（连接级临时表，事务内随回滚一并消失）
```

要点：

- **ord 由内核按数组下标盖章**，前端不发序号——快照序的唯一权威仍是前端数组，
  retry 砍尾、compact 换行、removeMessage 删行全靠「不在本批即删 + ord 重排」免费获得。
- 空快照 = 差集删除清空全会话，与现状 `/clean`、覆写空数组语义一致。
- `bot_history_append_inner`（汇报轮）不变更模型：纯 INSERT，行生成服务端 id、
  `created_at` 取当刻、`ord` 取 `MAX(ord)+1`。它不删任何行，差集不适用。
- `bot_history_clear`、会话删除不动。

### 5.2 后端写点的 id 口径（拍板项 D1）

后端三个全量保存点（exec 创建 / exec 回复 / subagent wrapper+digest）行不带 id，
内核现场生成服务端 id（推荐：与前端同一生成器语义的 uuid v4 字符串）。
代价：exec 会话每次重跑整段重建，行 id 每轮换——语义成立（exec 会话本就是
「每次执行一次重建的暂存转录」，任务块内容也随任务状态变）。

备选（不推荐）：exec 两行用确定式槽位 id（`exec:{task_id}:{origin}:user/reply`），
重跑原地更新不换 id。省一次重插，但引入「id 编码业务槽位」的特例口径，
子 agent wrapper/digest 没有等价槽位可编，两处口径分裂。

### 5.3 前后端 id 交接

后端 append / 后端保存的行带服务端 id 落库 → 前端 `loadFinalText` / 会话切换
`bot_history_load` 读回时**保留**该 id（`rowsToMsgs` 透传）→ 用户下次发消息，
快照里带着同一 id → upsert 命中旧行不换身份。交接零特殊逻辑，靠「读回保留」达成。

### 5.4 审计事件补字段（搭批 2）

`subagent_report.persist_failed`、`subagent_report.history_load_failed`、
`task_exec.persist_failed`、`memory.summary_save_failed` 四个事件现只带 `error`
（已核实），补 `session_id`（各调用点作用域内现成）。事件名不变——
`docs/acceptance/event-names.md` 白名单只认名字，无门禁影响。
`chat.history_save_truncated` 已带 dropped/total 条数，不动。

## §六 读路径设计

- `bot_history_load_inner`：SELECT 增加 `msg_id, created_at, trace_id`，
  排序 `ORDER BY ord, id`（`id` 只作 ord 雷同的次级稳定键，兜住 DEFAULT 0 的存量行）。
- `BotMsgRow` 增 `msg_id: Option<String>`、`created_at: Option<i64>`、
  `trace_id: Option<i64>`，全部 `#[serde(default)]`——保存方向（前端→后端）只有
  `msg_id` 有意义，`created_at`/`trace_id` 入参忽略（服务端所有）；读取方向
  （后端→前端）三值照发。老调用方/老测试零改动编译。
- `memory/extract.rs` 排序换 `ord DESC`（取近 N 条再反转的时间正序语义不变）。

## §七 前端设计

### 7.1 id 生成器

新增 `src/lib/msgId.ts`：`newMsgId()` 优先 `crypto.randomUUID()`（Tauri
WKWebView / WebView2 均支持），异常环境退化为时间戳+随机数拼接。单点封装，
出生点不直接摸 crypto。

### 7.2 消息出生点清单（全部补 id）

| 出生点 | 位置 |
|---|---|
| 用户消息 | `useChatSend.ts:212`（send 构造） |
| 流式占位气泡 | `useChatSessionActions.ts:220`（runChat） |
| 回合终稿 | `useChatSessionActions.ts:268`（done） |
| 回合失败行 | `useChatSessionActions.ts:319`（failed） |
| 回灌占位气泡 | `useChatHistory.ts:191`（resumeInflight） |
| 合并在途占位 / 补用户消息 | `ChatPanel.tsx:232-242`（mergeInflight；userMsg 是同一 Msg 对象引用，id 天然继承） |
| /compact 进度占位 / 摘要行 / 失败行 | `useChatSend.ts:118 / 126 / 134` |
| hint 行 | `ChatPanel.tsx:280`（addHint；不落库，id 只为 React key） |
| 读回映射 | `useChatHistory.ts:83`（rowsToMsgs：库里有 id 保 id，缺失现场生成） |

### 7.3 契约与渲染

- `Msg` 类型增 `id: string` 与 `createdAt?: number`（仅展示用，落库不回传
  created_at）；`persistHistory` 映射增 `msgId: m.id`。
- `MessageList` React key 从数组序换 `m.id`（读回必有 id：存量行读回时已现场
  生成；hint 行出生即有）。memo 命中率改善由 perf 测试校准
  （`MessageList.perf.test.tsx` 的 memo 对照口径同步核）。
- 流式事件挂靠（bot-tool-running/done、file-changed 挂最后一条 assistant）
  **本批不改**——挂靠按「尾部寻址」继续走数组序，事件带 id 精确落行是后续
  信封扩展，不在本方案。

### 7.4 消息时间展示（批 5，可选能力）

`createdAt`（首插时刻）随批 2 读回后，气泡 hover / 操作行显示生成时间——
「这条回复是几点生成的」目前库里查不到，读回即有能力。展示形态（hover 角标
还是常显小字）由实现批定，不加新命令、不动事件契约，组件测试锁读回值的
渲染口径即可。

## §八 与工作项方案的联动

本方案**不**给 `bot_messages` 加 `turn_seq`，规则先定、实现挂工作项重构窗口
（`WORK-ITEM-REDESIGN-PLAN-2026-11-04` 批 A 动 DDL 时同窗口落列）：

- 新用户行（首插）：`turn_seq = MAX(turn_seq) + 1`（同事务，与工作项同习语）。
- 新助手行：按 `ord` 继承最近前驱用户行的 `turn_seq`；无前驱（汇报轮 append）
  取 `MAX+1`（自成一轮）。
- retry 复用同一用户行（同 msg_id upsert 命中，turn_seq 不变），新一轮助手行
  继承同号——「同一回合的多次尝试」在消息面与工作项面同号对齐。

有了消息行的 `turn_seq`，子 agent 行（`subagents`）后续可挂 `spawn_turn_seq`，
条带锚定派发轮从「UI 口径」升级为「数据归属」——那是 A 方案之后如果有二期的路，
本文不承诺。

## §九 不做清单

- **append-only 事件日志**（Claude Code/Codex 式）：需配套 GC、体积治理、会话文件
  管理；单用户本地 SQLite 场景自找复杂度。Codex 多 GB 会话 / Cursor 30GB 库是
  缺治理的下场实录。
- **消息树 / 分支 / parentUuid**：retry/compact 的「截尾重存」语义不保留旧分支；
  需要时再立项。
- **后端增量事件流**（前端改发 delta）：破坏快照协议，不做。
- **turn_seq 落消息行**：挂工作项窗口（§八），本批不做。
- **修复汇报轮 stale 快照竞态**：保持现状对齐（§2.3）。
- **exec 会话多轮化**：每次执行重建的暂存语义不变。
- **事件精确挂靠**（bot-tool-* 带 msgId 落行）：信封方案后续扩展，本批不做。

## §十 拍板项

- **D1 后端行 id 口径**：推荐「内核现场生成服务端 uuid」（§5.2 备选为确定式
  槽位 id）。推荐理由：单点生成、口径统一；exec 会话 id 每轮换无害（本就整段重建）。
- **D2 trace_id 是否随本批落列**：推荐「随本批」（一列 + payload 透传，代价一档；
  收益是消息→执行痕迹直达，types.ts 里「traceId 暂不落库」的欠账一并清）。独立拍板
  是因为它是能力新增而非常量改写，可拆。
- **D3 老库不迁移确认**：按测试阶段现行口径（删旧数据不迁移），NULL 行首次保存
  自然换血（§4.3）。若要保 `created_at` 原值，需在差集删除前把 NULL 行的
  created_at 迁给同内容新行——不值，确认不迁移即默认放弃。

## §十一 批次切分

| 批 | 内容 | 锚点测试 |
|---|---|---|
| 批 1 | 数据面：基线 DDL 三列 + 两索引 + `ensure_columns` 迁移；写内核 upsert + 差集删除 + 临时表；截断/审计/updated_at 口径保留 | ① upsert 命中保 `created_at`（首插时刻不漂）；② 差集删除清掉快照外行与 NULL 行；③ 空快照清空会话；④ 事务中途 INSERT 失败全回滚（对齐既有回归）；⑤ 截断删最旧 + 审计计数；⑥ 后端 append 取 `MAX(ord)+1` 与首插时刻；⑦ `(session_id, ord)` 索引存在且迁移幂等重跑不炸 |
| 批 2 | 读路径：SELECT/ORDER BY ord、`BotMsgRow` 三字段（serde default）、memory/extract 换序；后端三个全量写点无 id 行服务端生成；§5.4 四审计事件补 session_id | ⑧ load 返回 id/created_at/trace_id 且序随快照；⑨ 2000 行差集删除无变量上限炸裂；⑩ exec 重跑后行序与任务块内容更新；⑪ 汇报 append 后前端读回带 id；⑫ persist_failed/history_load_failed 审计载荷带 session_id |
| 批 3 | 前端：`newMsgId` + §7.2 十处出生点 + `persistHistory` 带 msgId + `rowsToMsgs` 保 id/补缺失 + `Msg` 契约 | ⑬ 出生点全带 id（构造即有，非渲染时补）；⑭ 读回 id 保留、缺失行生成且保存后稳定；⑮ retry 复用用户行 id、失败回复行被差集删除；⑯ compact 摘要行换新 id、旧行清空 |
| 批 4 | 渲染：React key 换 `m.id` + perf 对照校准 + hint 行 key | ⑰ 中间行删除不触发全列表重挂载（key 稳定断言）；⑱ perf 测试两入口口径更新 |
| 批 5 | trace_id（若 D2 拍板做）：payload 透传 + `Msg.traceId` 读回保留 + 详情跳转接读库值；§7.4 消息时间展示 | ⑲ 终稿行带 trace_id、重启读回仍可跳执行痕迹；⑳ 读回 createdAt 的气泡按 §7.4 口径渲染时间 |

批次依赖：批 1 → 2 → 3 → 4，批 5 依赖批 2（列已在）可与其并行。全程
`cargo test --lib` + `npm test` + 行数门禁 + 验收对拍门禁（若 SPEC 功能条目
因此变更，同步 `docs/acceptance/` 三件套）。

## §十二 风险清单

1. **差集删除误伤**：并发期后端 append 的行不在前端快照 → 被删（§2.3 竞态，
   现状同款）。验收时盯汇报轮 + 立即发送的组合场景。
2. **upsert 冲突目标与部分索引**：SQLite 要求 `ON CONFLICT` 目标子句与部分唯一
   索引的 WHERE 匹配（`WHERE msg_id IS NOT NULL` 两处都要写），写错即编译期
   SQL 错误，批 1 测试直接兜住。
3. **临时表与连接池**：`lock_db_write` 串行写下临时表生命周期安全；若未来引入
   连接池需换成 CTE/子查询形态——在内核注释里写明该约束。
4. **`created_at` 语义换轨**：库内旧消费方若按批时刻语义读过（无——现状无消费方
   读该列），grep 已核；换轨后「全批同刻」变「逐行首插」，展示侧若直接用会看到
   更真实的时间，属改善非回归。
5. **msgId 泄漏进模型上下文**：`bot_chat` 的 messages 映射只取 role/content
   （`useChatSessionActions.ts:248`），id 不进模型；persistHistory 只进 DB。
   无泄漏面。

## §十三 偏离登记

（实施时逐条登记：偏离点 / 原因 / 影响面 / 是否回写本文）

## §十四 后续梯队与存量备查登记（只记录，不动手）

2026-11-06 方案评审时摸出，均**不在本方案范围**；动工前各自走拍板。

### 第二梯队（身份落地后的立项候选）

1. **事件精确挂靠**：`bot-tool-running/done`、`bot-file-changed` 等流式事件带
   `msgId` 精确落行，替代「挂最后一条 assistant」的尾部寻址——尾部寻址是
   `streamState` 一堆竞态守卫（termEpoch / 世代 / resuming 闸）的根源，这是
   身份层最大的健壮性红利；需动事件契约（流式信封方案的延伸），单独立项。
2. **`subagents.spawn_turn_seq`**（§八二期）：子 agent 行挂派发轮号，条带锚定
   从 UI 口径升级为数据归属；顺带 `list_subagents` 可按轮过滤。
3. **后端真源化**（远期）：Claude Code 式「持久层唯一事实、前端投影」——根治
   汇报轮 stale 快照竞态（§2.3）、多窗口一致。大手术，等身份层用稳再议。

### 存量备查（与身份方案无关，调查途中顺带发现）

1. **子 agent 条带 A 方案未修**：完成的条带挂「最后一条助手回复」随每条新指令
   再现（用户实机报告，业界口径推荐「无在途即退场」）。独立小修，不等本文；
   修时一并纠正 `MessageList.tsx` §247 与 §635 那对互相矛盾的挂载注释。
2. **`subagentActivity` 终态不清**：动作流按子 agent id 累积直到切会话
   （`useChatSubagents.ts` §61），终态事件到达时清该 id 即可，会话内小驻留。
3. **`list_subagents` 终态行无 GC**：仅删会话清理（`bot_sessions.rs` §202），
   长寿会话展开条带越拉越长（上限 20）；可加「展开只显最近 N」或终态滚动上限。

### 已核不动项

- 子 agent 在途轮询 3s（`SUBAGENT_POLL_MS`）、`persistHistory` 每轮双写全量
  payload（2000 行上限封顶最坏体积）——核过，维持现状。
