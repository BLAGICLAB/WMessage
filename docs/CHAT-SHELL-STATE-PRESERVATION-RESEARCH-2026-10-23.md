# 设置页接管导致在途流式丢失 — 业界口径调研与方案评估

> 状态：**调研报告**（2026-10-23）。本文只出结论与方案取舍，**不含代码改动**。
> 触发：草稿跨切设置丢失（已修，`01ba711b`）时顺带发现的相邻缺陷——
> 设置页接管主树时，**在途回复的流式事件同样会丢**。
> 相关：`SPEC.md`（聊天区口径）、`src/App.tsx`（设置视图提前 return）、
> `src/components/ChatPanel/ChatPanel.tsx`（流式订阅与在途态）。

---

## 0. 一页结论

**现象**：回复进行中点进设置页，回来后这段回复的流式过程消失（在途正文、
工具行、思考过程都不再增长），最终文本要等后端落库后重看会话才出现。

**根因**：设置页在 `src/App.tsx:962` 是**提前 return**——整棵主树（含聊天面板）
被替换掉，聊天面板卸载 → 它的 `listen("bot-chat-delta")` 等 11 个订阅随之注销 →
**那段时间后端广播的增量事件没有任何接收者**。后端的回合照跑不误（Rust 侧是独立任务），
但「广播」没有回放，前端也拿不到「还有哪些没收到」的线索。

**业界口径**：这件事有三层解法，能力递增、成本也递增：

| 层 | 做法 | 能扛住 | 大厂/框架的代表 |
|---|---|---|---|
| ① 壳层保活 | 切走时**别卸载**，树留着、订阅留着 | 切页面/切分类 | React `<Activity>`、Angular `RouteReuseStrategy`、Vue `<KeepAlive>`、Next.js layout |
| ② 状态外置 | 把要紧的状态搬出组件（store/服务层） | 组件卸载 | React 官方「连接放服务层」、Zustand/Redux 类 store |
| ③ 协议可恢复 | **服务器当真相源**：run 归服务器，客户端凭游标重连补齐 | 重启/换设备/换窗口 | SSE `Last-Event-ID`、Agno `/resume`、OpenAI/Google background mode |

**我们现在的位置**：① 未做（设置页提前 return），② 只做了草稿那一小块
（`chatDrafts.ts`，模块级 store + localStorage），③ 完全没做（纯推送、无游标、无回放）。

**推荐**：**先做①**（把设置页从提前 return 改成叠层，主树不卸载）——
它一行改动量级就能覆盖绝大多数场景，且**我们代码里已经有这个模式的现成先例**
（聊天区自己就是「常挂载 + `hidden` 切换」）。③ 是正解但工程量大（要在后端加
事件序号 + 回放缓冲 + 在途 run 注册表），建议作为二期单独立项。

**一条重要的反直觉结论**：业界最新的 React `<Activity>`（19.2）**不适合我们这个场景**——
它隐藏子树时会**清理子树的 Effect**，订阅会停；而我们要的恰恰是「看不见但仍在收流」。
详见 §2.1.3，那是本次调研最值得记住的一条。

---

## 1. 问题定性（我们的现状）

### 1.1 事件链（逐段可查）

```
用户发消息
  └─ 后端起回合（Rust 独立任务，bot_model_loop.rs）
       ├─ 增量广播  bot-chat-delta / bot-think-delta / bot-tool* / bot-turn-status …（11 类事件）
       ├─ 回合收尾 → bot_history_save_audited（bot_chat.rs:1783，整轮 DELETE+INSERT 落库）
       └─ 收尾广播  bot-chat-report（bot_chat.rs:1445）
```

前端侧（`src/components/ChatPanel/ChatPanel.tsx`）：

- 订阅写在 **Effect** 里（`listen("bot-chat-delta")` 在 855 行一带，共 11 路）；
- 在途态存在 **内存 ref/state**：`inflightRef` / `inflightSids` / `streamingMetaMapRef`（387–392 行）；
- `mergeInflight()`（392 行）负责「重建视图时接回在途回合」——但它读的
  `inflightRef.current` 与 `streamingMetaMapRef.current` **同样只在内存**。

### 1.2 丢的是什么 / 没丢的是什么

| 项 | 切设置后 | 说明 |
|---|---|---|
| 流式增量（正文/思考/工具行） | **丢** | 广播即焚，没有游标、没有回放缓冲 |
| 在途气泡的占位与已读正文 | **丢** | `mergeInflight` 依赖内存 ref，面板一挂载就是空 |
| 后端回合本身 | **不丢** | Rust 侧独立任务照跑完 |
| 最终全文 | **不丢，但要重看才见** | 收尾时整轮落库；前端此刻没有订阅者，不会自动刷新视图 |
| 工具调用痕迹 | **不丢** | 落 `file_changes` / trace 表，面板重载能看到 |

所以准确说法是：**「过程」丢了，「结果」还在，只是不会自己长出来。**

### 1.3 为什么这条现在才显形

主树里其它页面（首页/看板/图谱…）是条件渲染、互相关换，但**聊天区是唯一的常挂载位**
（`src/App.tsx:1100` 注释：「聊天常挂载… 条件挂载会让切走页面期间的事件无处可去
（挂件时代的『折叠不丢聊天』诉求，同解）」）。**设置页的提前 return 绕过了这条约定**——
它不是「切一个页面」，是「把主树整体换掉」。这是我们的设计里唯一一处破例。

---

## 2. 业界口径：三层

> 每条都标了出处与「对我们的适用性」。二手来源（博客）已注明，不与官方文档混用。

### 2.1 第一层 · 壳层保活：切走时别卸载

#### 2.1.1 Vue `<KeepAlive>`（官方文档，中文）

> 「`<KeepAlive>` 是一个内置组件，它的功能是在多个组件间动态切换时**缓存被移除的组件实例**。」
> —— 默认行为是「组件实例被替换掉后会被销毁…会丢失其中所有已变化的状态」；
> `<KeepAlive>` 让它被缓存而非销毁。配套 `include` / `exclude` / `max`（**LRU 淘汰**），
> 以及 `onActivated` / `onDeactivated` 两个生命周期钩子。

出处：<https://cn.vuejs.org/guide/built-ins/keep-alive.html>

**对我们的意义**：`onActivated` / `onDeactivated` 这对钩子就是「隐藏时暂停工作、
回来时恢复工作」的**行业标准形状**。对应我们的 `ChatPanel`：`deactivated` 时要不要停
子 agent 轮询、`activated` 时要不要补刷一次——先例就在这。

#### 2.1.2 Angular `RouteReuseStrategy`（官方文档）

> 「默认情况下，Angular 在离开路由时**销毁**组件实例，并在返回时建立新实例。」
> 要保留就实现自定义复用策略：`shouldDetach` / `store` / `shouldAttach` / `retrieve` /
> `shouldReuseRoute` 五个方法，核心概念是「**分离的路由句柄**（detached route handle）」——
> 离开时把组件实例、子组件与全部状态留在内存里，回来时重新挂回。

官方点名的适用场景：**表单状态保留 / 昂贵数据保留 / 滚动位置维护 / 类标签页界面**。
官方同时提醒：自定义策略增加复杂度，管理不当对内存有性能影响。

出处：<https://dev.angular.tw/guide/routing/customizing-route-behavior>

**对我们的意义**：这就是「设置页从提前 return 改成叠层」的**同构解**——
Angular 的做法本质上也是「离开时把组件留在内存，回来时挂回」。

#### 2.1.3 React `<Activity>`（19.2 官方文档）——**本次调研最反直觉的一条**

官方原文（<https://react.dev/reference/react/Activity>，已抓正文）：

> 「An Activity boundary hides its content by setting `display: none` on its children
> and **cleaning up any of their Effects**.」
>
> 「When an Activity is 'hidden', **all its children's Effects are cleaned up**.
> Conceptually, the children are unmounted, but React **saves their state** for later.
> This is a feature of Activity because it means **subscriptions won't be active
> for hidden parts of the UI**.」

即：**`<Activity>` = 保状态，但停副作用**。官方给的「想要隐藏期间继续收数据」修法是：

> 「如果业务要求页面隐藏期间仍持续接收并缓存消息，就不能把数据生命周期完全交给这个页面
> Effect。更合理的做法是**把连接和消息缓存放到页面之外的服务层**，页面 Effect 只负责
> 展示层订阅。」

**结论（对我们）**：我们的流式订阅正是 `ChatPanel` 的 Effect。
用 `<Activity>` 包聊天区 → 隐藏时订阅被清理 → **流式照样丢**。
所以「上 `<Activity>`」是**错的方向**；我们要的是它的反面：
**保留挂载 + 保留 Effect 跑**（CSS `display:none` 变体）。

顺带两条硬约束（写进方案选型）：

- 我们当前是 **react 19.1**（`package.json`：`react: ^19.1.0`），`<Activity>` 是 **19.2** 才有的
  → 即便要用也得先升 React；
- React 版本页导航显示官方文档当前为 **react@19.3**。

（调研中遇到一篇第三方博客称「Activity 隐藏时不会清理 Effect」，与 react.dev 原文
**直接矛盾**；以官方文档为准。）

#### 2.1.4 CSS `display:none` 变体——**我们代码里已经在用**

不卸载、不清理 Effect，只是不显示。代价：隐藏期间仍在跑副作用与重渲染。

> 这正是 `src/App.tsx:1104` 对聊天区的做法：
> `<div className={view === "chat" ? "h-full" : "hidden"}>` —— 切页面只切可见性，
> 「messages 与监听器全程存活」。

**关键对照**：`<Activity>`（保状态、停订阅）与 `display:none`（保状态、**继续订阅**）
是同一个问题的两种解，**选哪个取决于隐藏期间要不要继续收数据**。我们要继续收 → 选后者。

#### 2.1.5 Next.js App Router：layout / template / parallel + intercepting

官方文档：

> 「A layout is UI that is shared between multiple routes. **On navigation, layouts
> preserve state, remain interactive, and do not re-render.**」而 template 相反：
> 「templates create a new instance for each of their children on navigation… state is not
> preserved in Client Components, and effects are re-synchronized.」

出处：<https://next.nodejs.cn/docs/app/building-your-application/routing/layouts-and-templates/>
（Next.js 官方文档中文镜像）

而「弹窗盖在页面之上、**底下的页面上下文不丢**」这个和我们「设置页叠在聊天之上」
**几乎同构**的诉求，Next.js 给的是 **Parallel Routes（`@slot`）+ Intercepting Routes
（`(.)`）**：

> 「拦截路由允许你在当前布局中加载应用程序另一部分的路由…例如，当在信息流中点击一张照片时，
> 你可以在模态框中显示该照片，**覆盖在信息流上**。」

出处：<https://nextjscn.org/docs/app/api-reference/file-conventions/intercepting-routes>

**对我们的意义**：设置页做成「叠在主树之上」在 Next.js 里是一等公民模式（有官方范式），
不是野路子。Next.js 的额外提醒值得抄：**底下的页面（layout）保持存活、保持交互**。

---

### 2.2 第二层 · 状态外置：把要紧的东西搬出组件

- React 官方 `<Activity>` 排查条目给的就是这条：连接与消息缓存放**页面之外的服务层**，
  页面 Effect 只做展示层订阅（见 §2.1.3 引文）。
- Angular 官方：组件实例与状态留在内存的 detached handle 里（§2.1.2）。
- Vue 官方：`<KeepAlive>` 缓存组件实例与全部状态（§2.1.1）。

**我们已有的同构先例**：草稿那次的修复（`src/components/ChatPanel/chatDrafts.ts`）——
模块级 store + localStorage，跨挂载存活。那是「状态外置」的最小形态。

**但要认清它的边界**：状态外置能扛住「组件被卸载」，**扛不住「订阅被注销」**——
存下来的只是**已经收到的东西**，收不到的那段时间依然是空白。草稿能救，是因为草稿
本来就是「用户写下的静态内容」；流式回复不是。

---

### 2.3 第三层 · 协议可恢复：服务器当真相源

这是能扛住**重启 / 换设备 / 换窗口**的层，业界正在往这个方向收敛。

#### 2.3.1 SSE `Last-Event-ID`：重连 ≠ 补齐

协议口径：SSE 的 `id:` 字段会记在客户端状态里，断线重连时浏览器自动把非空的
last event id 放进 `Last-Event-ID` 请求头；**服务端必须保留并重发游标之后的事件**。
EventSource **不会**自动补齐——重连只恢复连接，补齐是服务端的活。

对客户端的配套要求（二手来源，多处一致）：

- 用**稳定的事件 id / 序号**去重，或让状态更新幂等（重放可能与实时流重叠）；
- 游标过期/未知时：**从权威快照重建**，再从对应游标续上，并在补齐前**显式暴露
  「重连中 / 陈旧」状态**，不能把不完整的状态当实时展示；
- 事件带 `retry:` 控制重连间隔（标准允许用户代理加指数退避）。

出处（均为二手技术文，结论与 WHATWG SSE 规范一致）：
<https://www.server-sent-events.com/sse-protocol-fundamentals-architecture/event-id-retry-mechanism-design>、
<https://n4n.ai/blog/reconnecting-dropped-sse-streams-with-last-event-id>、
<https://pcnmobile.com/sports-odds-sse-recovery-reconnect-without-losing-updates/>

#### 2.3.2 Agno：可恢复流式的完整实现样本

Agno（agent 框架）的 Background Execution 文档把这件事拆成四档，档位差别就是
「客户端断线后能不能接回来」：

| background | stream | 行为 |
|---|---|---|
| false | true | 默认流式。**客户端断线 = 取消本次运行** |
| false | false | 非流式，返回完整响应 |
| true | false | fire-and-forget，立即返回 PENDING，**轮询取结果** |
| true | true | **可恢复流式**：后台任务与客户端解耦，事件按 `event_index` 编号缓冲；客户端断线时记下 `last_event_index`，重连调 `/resume` 从该序号之后**重放** |

要点：

- 回合的**状态**（RUNNING 等）持久化在 DB（所以要挂 `db=`）；
- 事件**缓冲**在内存，每 run 保留最近 10000 条；
- 落后太多（超出保留窗）的客户端**拿不回被裁掉的事件**——文档明说这点；
- 重连的口径：服务端必须从游标之后按序重放，再与新事件无缝衔接。

出处：<https://agno.link/fc5Mmtd32>（Agno 官方文档 Background Execution）

#### 2.3.3 方向性信号

Google 新的 Interactions API 与 OpenAI 的 background mode，都是同一个转向：
**agent 的执行状态归服务器**，客户端只是它的一个观察者（可轮询、可重连、可换设备继续）。
对我们的意义：wmessage 的回合本来就跑在 Rust 侧（已经是「服务端执行」），
只是**没有把「这一轮还在跑、跑到哪了」做成可查询/可重放的接口**。

---

## 3. 三层对我们各自能解什么

| 场景 | ① 壳层保活 | ② 状态外置 | ③ 协议可恢复 |
|---|---|---|---|
| 切设置页 | ✅ 完全解决 | 只能保住已收到的 | ✅ |
| 切别的分类/页面 | ✅（我们已是这样） | 部分 | ✅ |
| 聊天面板真被卸载 | ❌ | ✅（已收到的部分） | ✅ |
| 窗口/应用重启 | ❌ | ✅（落盘的部分） | ✅ |
| 换设备/换窗口接着看 | ❌ | ❌ | ✅ |
| 工程成本 | 低（App 壳层改动） | 中 | 高（后端事件序号 + 回放缓冲 + 在途 run 注册表 + 前端重连） |

---

## 4. 候选方案

### A. 设置页从提前 return 改成叠层（第一层，推荐先做）

**做法**：删掉 `src/App.tsx:962` 的提前 return，改成在主树**之上**渲染一层
`fixed inset-0` 的设置壳（z-index 高于任务编辑浮窗 z-40 / 命令面板 z-50）。
主树不卸载 → 聊天面板存活 → 订阅存活 → 流式不断。

**收益**：设置页期间的在途回复**全程可见**（连工具行、思考过程都在）；
顺带修掉设置页期间**所有**事件丢失（不止流式：`execute-task` / `chat-open-session` /
`subagent-finished` 这几路监听器也在聊天面板里）。

**代价与风险**：

| 风险 | 说明 | 处置 |
|---|---|---|
| 主树在设置页期间仍挂载 | 首页 30s 轮询、聊天轮询等后台任务继续跑 | 已经在跑（切别的分类也这样），可接受；必要时按可见性停 |
| 叠层层级 | 设置壳要盖住 z-40 的任务浮窗、z-50 的命令面板 | 设 z-[60]；命令面板在设置壳内渲染一次（避免双份） |
| ErrorDialogHost 行为变化 | 现在设置分支没挂宿主 → 设置页报错走原生 alert；改叠层后走应用内弹窗 | 属「统一到应用内弹窗」，是改进，但要写进验收 |
| 焦点/滚动 | 叠层盖住时主树仍可聚焦 | 设置壳 `fixed inset-0` + 自身滚动容器；必要时加 `inert` |

**风险等级**：中。改的是 App 壳层，视觉要实机看一眼；逻辑面改动集中、可测。

### B. 在途态与流式缓冲外置（第二层，补充而非替代）

把「已收到的流式内容 + 在途会话集合」也搬出组件（模块级 store），
让**面板重挂载后能重建视图**。

**但请注意它的天花板**（§2.2）：搬出去的是「已经收到的」，**收不到的那段仍然是空白**。
它能把「回来后看到半截」变成「回来后看到已有的半截」，但补不齐中间。

**结论**：B 值得做（作为 A 的兜底：万一哪天面板还是被卸载，至少不空白），
但**不能替代 A**。

### C. 后端做可恢复流式（第三层，正解但工程量大）

后端补三件套：

1. **事件序号**：所有 `bot-*` 事件带单调递增的 `seq`（按 session 维度即可）；
2. **回放缓冲**：每个在途 session 保留最近 N 条事件（内存或落库）；
3. **在途 run 注册表**：`bot_sessions_load` 或新增 `bot_inflight_list` 返回
   「哪些 session 正在跑」——现在 `BotSession`（`db/bot_sessions.rs:24`）没有这个字段，
   重挂载的客户端**根本不知道有在途回合**，所以 `mergeInflight` 才会失效。

前端加「重连时带 `last_seq` 补齐」的逻辑。

**代价**：后端状态机 + 缓冲生命周期 + 前端重连逻辑 + 一致性（补齐与实时的交接）
——**这是一个独立批次的工作量**，不是顺手能带上的。

### D. 升级 React 到 19.2+ 用 `<Activity>`（**明确不做**）

见 §2.1.3：`<Activity>` 隐藏时清理 Effect → **订阅会停 → 解决不了流式丢失**，
反而制造「以为保住了其实没保住」的假象。

---

## 5. 推荐路径

**一期（推荐先做）**：方案 A —— 设置页改叠层。

**落地状态（2026-10-23，已实现）**：设置页改为 `fixed inset-0` 叠层，主树不卸载；
主树在设置期间整棵 `inert`——不可点、不可聚焦，但**不暂停 JS 监听器**（在途流式要继续收）。
实现中查出两处配套，一并修掉：①命令面板在设置期间**只挂一份**（挂设置壳内，主树那份让位）；
②全局确认/错误弹窗必须 **portal 到 body**——它们原先挂在主树里，主树一 `inert` 就跟着
点不动（弹窗看得见却按不动）。二期（B + C）仍未立项。

- 覆盖：切设置页期间流式全程可见 + 设置页期间所有事件不再丢；
- 附带修掉「设置页提前 return 违反『聊天常挂载』既有约定」这条设计破例；
- 验收好写（见 §6）。

**二期（独立立项）**：方案 B（兜底）+ 方案 C（正解）打包做协议层可恢复。

- 理由：C 的价值只在「客户端可能真的不在场」时兑现（重启、换窗口、换设备）；
  一期做完 A 之后，日常路径已无「客户端不在场」的场景，所以 C 可以排在后面，
  按真实需求（要不要支持多窗口同时看同一会话）再启动。

**落地状态（2026-10-23，已实现）**：二期按 B + C 打包做完，落在两个提交上——
后端 `323d4daf`（序号 + 回放缓冲 + 在途登记）、前端 `bafb57e9`（在途态外置 +
回放补齐 + 终态自取）。实现中定下的几条口径：

- **序号跨轮次连续**（进程内不重置）：客户端的游标是「本会话已应用到的序号」，
  重置会让旧游标落到新一轮头部之后，补齐直接变成空；
- **缓冲只收在途轮次**：回合外的事件（技能路由的失败行早于 `begin`、汇报轮的
  收尾行晚于 `finish`）不记也不占序号——记进去回放会掺进不属于这一轮的内容；
- **两个事件不进缓冲**：`bot-usage-delta` 是累计型（重放会重复计数），
  `bot-file-changed` 发射点在工具执行侧且前端有两个订阅者（聊天窗 + 右侧栏）；
- **前端三份状态同源同生共死**：内容（meta）、在途集合、序号游标必须在一处——
  游标跑在内容前面，补齐就会跳过本该重建的那一段（接出来的是半截）；
- **实时与回放共用同一批处理函数**：两套口径迟早分家（回放出来的正文与实时
  出来的对不上，用户看到的是「重说了一遍」）；
- **回灌接回的那一轮，终态要自取最终正文**：发起它的窗口已经没了，没有 invoke
  结果可等；终态比落库早到一点点，最后一条还是用户消息就等一下再取。

**边界（写清楚，别当 bug）**：本轮做到的是「**客户端不在场而宿主仍在场**」可补齐
（webview 重载/崩溃、第二窗口）。**整个应用退出接不回**——后端与前端同进程，
进程一死这一轮也一起死（退出时 2s 宽限内能收尾的，半截以最终态落库，回来看到
的是最终态的半截，不是活的流式）。要「退出后还能续跑」得把模型循环挪出进程
（守护进程化），那是另一个量级的工程，本轮不做。另：主窗关闭是**隐藏**不是销毁
（webview 存活），那种情况面板一直挂着、订阅一直在，本来就不丢——本轮修的是
「面板真的没了」的那一半。

**不做**：D。

---

## 6. 验收口径（建议写进实机回归卡）

**A 期验收（设置页叠层）**

1. 聊天里发一条会跑工具的长回复 → **回复进行中**点「设置」→ 设置页停留 ≥ 10s →
   点「返回」→ **流式气泡仍在增长**（不是停在离开那一刻的半截）；
2. 同上，但中途在设置页里**改一个设置**（如切换主题）→ 返回后流式**未中断**；
3. 设置页期间后台又起一个新回合（定时任务/任务卡 🤖）→ 返回后**能立刻看到**那条会话
   （验证 `chat-open-session` / `execute-task` 监听器没丢）；
4. 设置页正常盖住主树：不可见主树的任何元素**点不到**（z-index / 命中测试）；
5. 设置页里报错 → 弹**应用内**错误弹窗（不是原生 alert）——这是行为变化，明确写进预期。

**已自动化覆盖的部分**（App 层单测）：切设置后主树仍在 DOM、`bot-chat-delta` 订阅未被
注销、设置期间到达的增量照样渲染进气泡、主树 `inert`、全局弹窗不在 `inert` 子树内
（还能点）、命令面板不双挂。**仍需实机过的部分**：真实浏览器里的视觉与命中（盖住期间
主树元素点不到、流式气泡肉眼可见地在长）、设置页内改设置不打断流式、设置页期间新起
回合能立刻看到。

**C 期验收（已立项并实现，见 §5 落地状态）**

6. 回复进行中**重启应用** → 回来后能重连该回合（至少拿到最终全文 + 工具痕迹），
   且界面明确显示「已重连 / 内容已补齐」，不把补齐前的状态当实时。

**已自动化覆盖的部分**（Rust 单测 7 例 + 前端 11 例）：序号单调与跨轮次连续、
按游标回放、过期游标报截断、缓冲有界、在途登记起止；前端在途态外置（内容 /
在途集合 / 游标同源）、序号去重（补齐与实时重叠不重复叠加）、回灌补进气泡、
已收尾不补、挂载时接回在途列表里的其他会话。**仍需实机过的部分**：真机上
「回复进行中重启应用 → 回来接回」这条完整链路（后端缓冲的真实事件流、重启后
终态自取最终正文、回合秒表起点对齐）。实机卡见 `docs/realmachine-regression-test.md`
的 **S34**。

---

## 7. 不做清单（避免范围蔓延）

- 不做「多窗口同时看同一会话」的实时同步（C 期若做，也只保证**最终一致 + 可重连**，
  不保证两窗口逐字同步）；
- 不把 `bot-*` 事件改成 SSE/长连接形态——Tauri 的窗口内广播够用，缺的是**序号 + 回放**，
  不是传输层；
- 不为「隐藏期间不跑后台任务」而引入 Activity/KeepAlive 类抽象——我们的诉求恰好相反
  （隐藏期间**要**继续跑）；
- 不动会话历史的落库口径（整轮 DELETE+INSERT 落库是既有设计，不在本次范围）。

---

## 8. 出处

**官方文档（已读正文）**

- React `<Activity>`：<https://react.dev/reference/react/Activity>
- Vue `<KeepAlive>`（官方中文）：<https://cn.vuejs.org/guide/built-ins/keep-alive.html>
- Angular 自定义路由复用行为（官方文档）：<https://dev.angular.tw/guide/routing/customizing-route-behavior>
- Next.js 布局与模板（官方文档中文镜像）：<https://next.nodejs.cn/docs/app/building-your-application/routing/layouts-and-templates/>
- Next.js 拦截路由（官方文档中文镜像）：<https://nextjscn.org/docs/app/api-reference/file-conventions/intercepting-routes>
- Agno Background Execution（官方文档）：<https://agno.link/fc5Mmtd32>

**二手来源（SSE 恢复机制，与 WHATWG SSE 规范口径一致）**

- <https://www.server-sent-events.com/sse-protocol-fundamentals-architecture/event-id-retry-mechanism-design>
- <https://n4n.ai/blog/reconnecting-dropped-sse-streams-with-last-event-id>
- <https://pcnmobile.com/sports-odds-sse-recovery-reconnect-without-losing-updates/>

**冲突记录**：一篇第三方博客（jsmanifest）称 React `<Activity>` 隐藏时**不会**清理
Effect，与 react.dev 原文相反。**以 react.dev 为准**：会清理。这条恰好是本次结论的关键，
故单列在此。
