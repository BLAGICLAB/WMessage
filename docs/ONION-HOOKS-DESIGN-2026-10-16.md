# 多层洋葱键（Layered Hook Pipeline）实施方案

> 来源：外部设计稿《WMessage 多层洋葱键 · 完整开发提示词》（含 Lua Layer2 钩子与四份
> 子 Agent yaml）。本文件是它在本仓的**落地映射与改造方案**，不写代码。
> 与 `docs/AGENT-TRANSPARENCY-DESIGN-2026-10-06.md`、`docs/APPS-DESIGN-2026-10-15.md`
> 同类：模块与运行时的现行口径以 `docs/rust-bot-architecture.md` 为准。

已定的前置决定（本轮）：
1. **不引入脚本运行时**——先做原生钩子注册表，事件回调是 Rust 内部实现；
   设计稿的 Lua 脚本层推迟到 seam 稳定之后再议（见「脚本层的债」）。
2. **子 Agent 流水线非阻塞**——spawn 后立即返回，主循环轮询/等齐，
   不用设计稿的 `runSync()`。
3. 本轮只交本方案，不动工代码。

---

## 一、结论先说

设计稿的七个层里，**六个在本仓已经有对应缝隙**。钩子运行时本质不是「搭一条新管线」，
而是把已有的硬编码闸改成**可注册扩展点**。真正没有对应物的是 L0 `pre_ingest`。

单看这一点，第一切片比表面看起来小得多。但设计稿有三处与现行架构直接冲突、
一处命名根本对不上（见 §四、§五），不先解决就动手，落地出来的是第二套真相源。

---

## 二、层 → 现行实现映射

行号对应本文件提交时的 HEAD（`0109447`）。改动本仓后需重新校准。

| 设计层 | 现行实现（file:line） | 缝隙性质 | 改造量 |
|---|---|---|---|
| L0 `pre_ingest` | **无**。用户消息在 `bot_chat.rs` 里被读入后直接进入建 msgs 流程 | 真新增 | 新增一个前置函数 |
| L1 `ingest` | `bot_chat.rs:948-955` 历史预算解析 + 截断（`truncate_chat_history_with_summary`） | 硬编码步骤 | 接线 |
| L2 `assemble` | `bot_chat.rs:964-975` 记忆块注入（`build_memory_block`）；摘要消息 `bot_chat.rs:957` | 硬编码步骤 | 接线（**子 agent 调度落点**） |
| L3 `pre_execute` | `bot_model_loop.rs:1295` 请求 body 装配；`:1271-1281` `llm.request` 埋点 | 硬编码步骤 | 接线 |
| L4 `pre_tool` | **已是一套完整闸**：`bot/dispatch.rs:166` 规则表 deny/ask → `:270` 调用点；`:299` 子 agent 白名单；`:403` 入参 JSON 闸 | 三道闸串行 | 接线（**最高价值**） |
| L5 `post_tool` | `bot/dispatch.rs:477` `tool.return` 审计；输出钳制与 `skill_on_step_post` 在其后 | 硬编码步骤 | 接线 |
| L6 `post_turn` | `bot_orchestrator.rs:1551` 收尾判定（状态机 + 收尾 JSON + 机器验收） | 硬编码步骤 | 接线 |

### 各层钩子的现有等价物（能力对照）

- **L0**：无。设计稿要的「过滤/清洗输入、提前识别任务类型、拦截非法请求」在本仓
  **没有任何等价物**——用户输入直接进模型。这是设计稿里唯一真正的新能力。
- **L1**：`chat.history_truncated` 审计 + 摘要落库（`bot_chat.rs` 截断路径）。
- **L2**：记忆体 v2 的 `build_memory_block` 已经是「检索素材注入上下文」的现成实现，
  设计稿的「注入检索素材」不必重写，只要把它的结果作为钩子链的一环。
- **L3**：`llm.request` 审计已记 `msgs_count`/model/provider/reasoning；token 预算事件
  `loop.msgs.over_budget` 已有（只审计不截断）。
- **L4**：三道闸的语义正好覆盖设计稿的「权限校验 / 参数校验 / 阻断高危工具」。
  **这是设计稿里唯一「已有实现且值得开放扩展」的一层。**
- **L5**：`tool.return` + `tool.output.over_budget` + `skill_on_step_post`（熔断/暂停）。
- **L6**：收尾时的会话归档、`subagent_finish` 事件、指标统计（`usage_stats_*`）。

---

## 三、原生钩子注册表设计（不引脚本 VM）

### 3.1 形态

一个进程内注册表，键 = 层名，值 = 回调链。回调是 Rust trait 对象，不接受字符串路径。

```rust
pub enum HookLayer { PreIngest, Ingest, Assemble, PreExecute, PreTool, PostTool, PostTurn }

/// 钩子上下文：会话 id + 层名 + 可中断标志（原地修改，由调用方消费）
pub struct HookCtx<'a> {
    pub onion_key: &'a str,          // = session_id（见 §六的命名讨论）
    pub session_id: Option<&'a str>,
}

pub trait Hook: Send + Sync {
    /// 返回 None = 中断本次流转（等价设计稿的「返回 nil」）
    fn on_event(&self, ctx: &HookCtx<'_>, payload: &mut HookPayload) -> HookResult;
    fn name(&self) -> &'static str;
}
```

### 3.2 三条纪律

1. **顺序固定、不可跳层**：注册表只决定「同层内谁先谁后」（按注册序），
   不决定层序。层序由调用点的代码结构保证——Rust 里层序是函数调用栈，
   天然不可乱序，这比运行时配置更硬。
2. **钩子可单独禁用**：注册表提供 `enabled(layer) -> bool`，默认全开；
   对应设计稿「yaml 里配空串即禁用」。
3. **钩子不得吞错**：钩子 panic 或返回错误一律**记审计并放行**，
   不得让用户配置里的一个 bug 变成主链路中断。这条与仓内既有纪律一致。

### 3.3 注册粒度

- 进程级注册（全局一套）+ 会话级开关（按 session_id 覆盖）。
- **不做**按 yaml 文件注册的机制——那是脚本层的事（§七）。

---

## 四、与现行架构的三处冲突（必须先解决）

### 冲突 ①：`runSync()` 与现行非阻塞派发相反

- 现行：`spawn_subagent`（`bot_orchestrator.rs:329`）立刻返回 `SpawnAck`，
  并有 FIFO 闸：全局 3 / 每会话 2（`:937-938`）。
- 设计稿：Layer2 钩子里串行 `runSync()` 四次。
- 后果：按现行墙钟预算（`max_wall_seconds` 默认 600s），最坏一次钩子静默阻塞
  几十分钟，且主会话全程无任何增量输出。
- **已定方案**：非阻塞。详见 §八 流水线形态。

### 冲突 ②：`allowed_skills` / `denied_skills` 是权限的第二真相源

- 现行权限在**三处**强制，且互相印证：
  1. schema 过滤 `bot/registry.rs:1220` `tools_json_for`
  2. dispatch 闸 `bot/dispatch.rs:299`（白名单外一律拒，记 `subagent_whitelist_deny`）
  3. 递归禁用（spawn/check/cancel 对子 agent 一律拒）
- 设计稿再加一份 yaml `denied_skills`，就会与 `profile_whitelist`
  （`bot/registry.rs:1210`）分叉。**这正是本轮 P0 在 `BUILTIN_TOOL_COUNT` 上刚清掉的
  同类病**：两个真相源必然漂移，且没有东西会喊错。
- **建议**：若采纳 per-onion 配置，其 `allowed_skills` 必须**只能收紧**、
  不能放宽 profile 白名单（`effective = profile_whitelist ∩ yaml.allowed`）；
  `denied_skills` 直接删除——现行是「默认全放行 + 白名单」，加 deny 列表只会
  让同一条权限有两个表达法。

### 冲突 ③：四个子 Agent 与现行三段式重叠

- 现行 stage 机已有 `recon` / `executor` / `verifier`（`prompts/subagent.rs:27-31`
  的派发流程），分别对应设计稿的 Probe / WriterEditor / Validator。
- 现行 profile 只有 3 个（`db/subagents.rs:80` research / coder / general）。
- 新增 4 个身份，若不配新工具集，提示词与权限面翻倍而能力不增。
- **建议**：先补「Planner 缺的能力」（任务拆解/大纲产出，目前由主 agent 的 plan
  能力承担），而不是先加身份。若确要独立 Planner，给它一个**新 profile**
  （只读 + 结构化输出），而不是复用 research 再靠提示词区分。

---

## 五、命名对不上（照抄 yaml 会大面积落空）

设计稿 yaml 里的 Skill 名与本仓 44 个工具的对照：

| 设计稿名 | 本仓实际工具 | 处置建议 |
|---|---|---|
| `list_dir` | `list_files` | 改名对齐 |
| `read_file` | `read_text_file` | 改名对齐 |
| `write_file` | `write_file` | ✅ 唯一直接对上 |
| `shell` | 无（`run_python` 是受限解释器沙箱） | **不要映射成 shell**；Validator 需要的静态检查应走 `run_python` 或新增受限检查工具 |
| `shell_sandbox` | `run_python` | 语义接近但不同（无 shell、无进程管理） |
| `vector_search` | `recall_facts` | 语义记忆检索，非通用向量检索 |
| `ast_inspect` | **无对应** | 需新增，或明确降级为 `grep_files` + `read_text_file` 组合 |
| `delete` | **无对应**（`delete_task` 只删任务卡） | 从 deny 列表删掉，否则是误导性配置 |

**最危险的一条**：Validator 的 `allowed_skills: [read_file, shell_sandbox]` 按现有
工具名会**两个都不存在**。若权限实现只判「在不在 allowed 里」，后果是
**只读约束形同虚设**（要么全拒、要么全放行，取决于未命中时的默认分支）。
这类「配置看着生效、实际全落空」的失败模式，必须在实现期用一条单测兜住：
**所有 allowed_skills 里的名字必须能在 `TOOLS_TABLE` 里找到**。

---

## 六、命名与身份：onion_key 与现有 session 的关系

设计稿的「洋葱实例」与本仓的「会话」是同一个东西的不同叫法：

- 本仓已有 `bot_sessions`（`db/mod.rs:199`）：`id / title / created_at / updated_at /
  is_subagent`。**没有 per-session 配置列**。
- 本仓已有 `onion_key` 概念吗？有——设计稿的 `onion_key.spawnChildOnion` 与本仓
  的 `tool_guard::subagent_ctx(session_id)` + `SubagentRow` 是同一套东西的两种表述。
- 因此 **per-onion yaml 配置需要新的存储面**（DB 新表或 per-session 文件），
  以及一条配置解析链。这是实打实的新增工作量，不在「接线」范围内。

建议：沿用 `session_id` 作为洋葱唯一键，别再引入 `onion_key` 第二套标识——
本轮 P0 刚把工具数收敛到单一常量，同一原则适用于身份标识。

---

## 七、脚本层的债（本轮明确不还）

设计稿的 Lua 层要求引入脚本 VM 到主进程。评估结论与前置决定：

- 现状：`src-tauri/Cargo.toml` **无任何 Lua 依赖**。引入即新增依赖，
  按仓内纪律需给「stdlib / 现有依赖为何不行」的理由。
- 安全面：脚本钩子可触达 `payload`（含用户消息与工具结果），等于把应用进程
  的一个可执行扩展点交给用户目录下的文件。沙箱边界（能否读文件、能否起进程、
  能否调 Tauri 命令）需要单独设计，不是引入依赖就完事。
- 建议路径：先把注册表 + 内部钩子跑通（§三、§九），用**内置钩子**覆盖设计稿
  想要的能力；等 seam 稳定、权限面清楚，再评估是否需要脚本层。
  若最终要做，优先考虑「钩子 = 受限命令模板」而非通用脚本。

---

## 八、子 Agent 流水线形态（非阻塞版）

设计稿的同步串行改成：

```
用户消息 → L0/L1/L2 组装
L2 判定复杂任务（关键词/规则）
  ├─ 命中 → 依次 spawn（不等待）：Probe → Planner → WriterEditor → Validator
  │        每个 spawn 立刻返回子 agent id；把「流水线 id」记在会话上下文
  │        下一轮 assemble 钩子检查：齐了就把结果打包注入 payload；没齐就
  │        直接放行，并在正文提示「流水线进行中，完成后自动汇总」
  └─ 未命中 → 直接放行
→ L3 → LLM → L4 → 工具 → L6
```

要点：

1. **派发不阻塞**：沿用现有 `SpawnAck` + FIFO 闸（全局 3 / 每会话 2）。
   四段流水线的排队深度天然受闸约束，不会一次吃掉全部并发。
2. **等齐靠轮询**：现有 `check_subagent`（`bot_orchestrator.rs`）已经是轮询式，
   钩子里复用它即可，不新增等待原语。
3. **子 agent 中间产物不回传父上下文**：这一点现行已满足——子 agent 上下文是
   独立 session 行，父上下文只在收尾时拿到结构化 `result_json`。
   设计稿的「仅返回压缩后摘要」不需要额外机制，但**要在钩子实现里保证
   父 payload 只注入 `result_json`，不注入子会话历史**。
4. **事件**：设计稿的 `subagent_start/finish/error` 已有对应实现——
   `subagent-spawned`（`:1899`，queued 即发）、`subagent-finished`（`:1884`）、
   以及审计事件 `subagent_done` / `subagent_failed` / `subagent_cancelled`
   （`:1697-1700`）。**不需要新事件体系**，只需让钩子能监听既有 emit 点。

---

## 九、分期路线（建议）

| 期 | 交付 | 为什么先做这个 |
|---|---|---|
| 0 | 本方案落盘 + 命名映射表（§五）作为常量单测 | 不写运行时代码就把「名字对不上」这类失败模式挡住 |
| 1 | **L4 `pre_tool` 原生注册表**：把现有三道闸收进统一入口，钩子可插在闸前后 | 唯一「已有实现且真有扩展价值」的一层，改动可控、收益直接 |
| 2 | L5 `post_tool` + L6 `post_turn`：输出整形、收尾前加工 | 与 L4 同一条链，复用注册表 |
| 3 | L1/L2/L3：记忆块/摘要/埋点成为钩子链一环 | 依赖 L4 的注册表与顺序纪律 |
| 4 | L0 `pre_ingest`：真新增能力（输入清洗、任务类型预判） | 设计稿唯一的新能力，但独立，可后置 |
| 5 | per-onion 配置存储（若确需要） | 需要先确认 §六 的身份命名不再引入第二套 |

每期一个提交、配单测、跑 `scripts/test-fast.sh`——与本仓既有纪律一致。

---

## 十、未决问题（需拍板）

1. **Planner 是否独立成子 agent**：还是由主 agent 的既有 plan 能力承担？
   涉及是否新增第 4 个 profile（§四 冲突 ③）。
2. **`ast_inspect` 真做还是降级**：真做要新增工具（AST 解析依赖），
   降级则用 `grep_files` + `read_text_file` 组合并从 yaml 里删掉这个名字。
3. **per-onion 配置是否需要**：若只是「每类任务一套钩子」，用进程级注册 +
   会话级开关即可，不必上存储面。
4. **钩子是否允许改 payload**：设计稿允许各层改 payload；但 L4 的参数闸一旦
   可被钩子改写，安全语义就变了。建议 L4 只读（可拒绝、可放行），不改参数。

---

## 十一、本轮未验证/未做的事

- 未写任何运行时代码；§二 的行号在后续改动后需重新校准。
- 未评估 Lua 依赖的具体选型与其沙箱边界（§七 已说明推迟理由）。
- 未做端到端可行性验证（钩子能否在现有 async 生命周期里安全挂载），
  期 1 的第一步应当是「在 L4 挂一个内置钩子跑通全链」，而不是先做注册表 API。
