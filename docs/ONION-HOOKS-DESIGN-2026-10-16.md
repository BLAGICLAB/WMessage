# 多层洋葱键（Layered Hook Pipeline）落地方案 · 修订版

> 来源：外部设计稿《WMessage 多层洋葱键 · 完整开发提示词》（含 Lua Layer2 钩子与
> **四**份子 Agent yaml）。本文件是该设计稿在本仓的落地映射与**修订后的设计**，
> 只写方案、不写运行时代码。与 `docs/AGENT-TRANSPARENCY-DESIGN-2026-10-06.md`、
> `docs/APPS-DESIGN-2026-10-15.md` 同类；模块与运行时的现行口径仍以
> `docs/rust-bot-architecture.md` 为准。

## 本轮已定的五条（与原设计稿的差异集中在这里）

| # | 决定 | 对原设计稿的改动 |
|---|---|---|
| 1 | **Planner 归主 agent，只保留三个子 Agent**：Probe / WriterEditor / Validator | 四段流水线 → 三段 |
| 2 | **精准 AST 按需加载**，未挂载或语言不支持时降级 `grep_files` + `read_text_file` | `ast_inspect` 不再是「无对应的空名」，改为可选挂载能力 |
| 3 | **per-onion 配置分两层**：轻量元配置进 `bot_sessions`（1–2 列），完整洋葱 YAML 放文件系统，不入库 | 原稿「每洋葱一份 yaml」未定存储面，现已定 |
| 4 | **L4 `pre_tool` 只读**：只允许拦截/放行，禁止改写 `tool_call` 参数；payload 改写能力收到 L2/L3 | 原稿 L4 含「修改工具参数」，安全语义收紧 |
| 5 | **工具名一律对齐现有 44 个工具**；`denied_skills` 取消 | 原稿 8 个 Skill 名里有 5 个不存在 |

另有一条前置决定沿用：**不引入脚本运行时**（先原生钩子注册表，Lua 的债见 §七）。

---

## 一、结论先说

七层里六层在本仓已有缝隙（L4 甚至已是完整闸），只有 L0 `pre_ingest` 是真新增。
钩子运行时本质是把硬编码闸改成可注册扩展点。

三子 Agent 的修订让设计与现行架构的冲突面明显收窄：**Probe / WriterEditor /
Validator 正好一一对应现行 stage 机的 recon / executor / verifier**
（`prompts/subagent.rs:27-31`），Planner 归主 agent 后不再需要第 4 个 profile。

---

## 二、层 → 现行实现映射

行号对应本文件提交时的 HEAD（`d57c81f`）。改动本仓后需重新校准。

| 设计层 | 现行实现（file:line） | 缝隙性质 | 改造量 |
|---|---|---|---|
| L0 `pre_ingest` | **无**。用户输入在 `bot_chat.rs` 直接进建 msgs 流程 | 真新增 | 新增前置函数 |
| L1 `ingest` | `bot_chat.rs:948-955` 历史预算解析 + 截断（`truncate_chat_history_with_summary`） | 硬编码步骤 | 接线 |
| L2 `assemble` | `bot_chat.rs:957` 摘要消息；`:964-975` 记忆块注入（`build_memory_block`） | 硬编码步骤 | 接线（**子 agent 调度落点**） |
| L3 `pre_execute` | `bot_model_loop.rs:1295` 请求 body 装配；`:1271-1281` `llm.request` 埋点 | 硬编码步骤 | 接线（**payload 改写能力的收归处**） |
| L4 `pre_tool` | **已是一套完整闸**：`bot/dispatch.rs:166` 规则表 deny/ask（调用点 `:270`）；`:299` 子 agent 白名单；`:403` 入参 JSON 闸 | 三道闸串行 | 接线（**最高价值**，且**只读**） |
| L5 `post_tool` | `bot/dispatch.rs:477` `tool.return` 审计；其后输出钳制 + `skill_on_step_post` | 硬编码步骤 | 接线 |
| L6 `post_turn` | `bot_orchestrator.rs:1551` 收尾判定（状态机 + 收尾 JSON + 机器验收） | 硬编码步骤 | 接线 |

**L4 的现有三道闸语义已覆盖设计稿要的「权限校验 / 参数校验 / 阻断高危」**，
且它们本身就不改写参数——本轮决定 ④（只读）与现状一致，不需要把闸改成可写再收回。

---

## 三、修订后的设计稿

### 3.1 三个子 Agent（Planner 归主 agent）

| 子 Agent | 角色 | 对应现行 stage | 是否新增能力 |
|---|---|---|---|
| Probe | 资料与代码探查，只读，产出结构化素材 | recon | 是（代码检索需 grep_files，见 §3.3 缺口） |
| WriterEditor | 公文/PPT/代码撰写与编辑 | executor | 否 |
| Validator | 合规校验、代码评审与静态检查 | verifier | 部分（AST 视角可选） |

**Planner 归主 agent 的理由**：现行主循环本身就有计划能力（`bot_plan`），
把它独立成子 agent 只是多一层身份与提示词，不带来新工具集。任务拆解的产物
（大纲/任务清单）由主 agent 在 L2 决定「是否需要三段流水线」时产出，
作为 WriterEditor 的输入。

### 3.2 工具名映射（对齐现有 44 个工具）

| 原稿名 | 本仓实际 | 处置 |
|---|---|---|
| `list_dir` | `list_files` | 改名 |
| `read_file` | `read_text_file` | 改名 |
| `write_file` | `write_file` | ✅ 直接对上 |
| `shell` / `shell_sandbox` | `run_python`（受限解释器沙箱：独立临时目录 + 60s 超时 + 产物回收 + 开关默认关闭） | **不映射成 shell**；静态检查走受限脚本，不给裸 shell |
| `vector_search` | `recall_facts` | 语义记忆检索（语义记忆体 v2），非通用向量库查询 |
| `ast_inspect` | **按需挂载的新能力**（§3.6） | 默认不挂载 |
| `delete` | 无对应（`delete_task` 只删任务卡） | 删除该条目 |

### 3.3 两个 profile 缺口（本仓侧待改，不在本轮）

三份 yaml 的工具集要真正生效，前提是 profile 白名单覆盖它们。现状：

- `RESEARCH_TOOLS`（`bot/registry.rs:1179`）= `web_search / fetch_url / list_files /
  read_text_file / write_artifact_file / read_own_card` — **没有 `grep_files`**，
  而 Probe 必须检索代码。
- `CODER_TOOLS`（`:1187`）= `read_text_file / list_files / grep_files / edit_file /
  write_file / run_python / write_artifact_file / read_own_card` — **没有联网工具**。
- `GENERAL_TOOLS`（`:1197`）= 两者并集。

因此「探查资料 + 检索代码 + 只读」这件事**现行没有任何单个 profile 覆盖**。
两个选项，需拍板：

- **A（推荐）**：给 `RESEARCH_TOOLS` 加 `grep_files`。它本来就是只读 profile，
  加一个只读检索工具不扩大安全面，一行改动。
- **B**：新增第 4 个 profile `probe`。身份更清晰，但多一套提示词与白名单维护面。

### 3.4 权限模型：只有收紧，没有 deny 列表

原稿的 `allowed_skills` + `denied_skills` 会成为**权限的第二真相源**——
现行权限已在三处强制并互相印证：

1. schema 过滤 `bot/registry.rs:1220` `tools_json_for`
2. dispatch 闸 `bot/dispatch.rs:299`（白名单外一律拒，记 `subagent_whitelist_deny`）
3. 递归禁用（spawn/check/cancel 对子 agent 一律拒）

修订后的单一公式：

```
effective = profile_whitelist(profile) ∩ yaml.allowed
```

- 取消 `denied_skills`：默认拒绝已由 profile 白名单给出，再加一张拒绝表只会
  让同一条权限有两个表达法（本轮 P0 在 `BUILTIN_TOOL_COUNT` 上刚清掉的同类病）。
- `effective ≠ yaml.allowed` 时**记审计并在渲染给模型的包装里显式列出实际可用工具**
  ——「配置看着生效、实际落空」必须让模型和人都看得见。
- 一条硬约束（期 0 单测）：`yaml.allowed` 里的每个名字必须能在 `TOOLS_TABLE`
  里找到，否则视为配置错误。

### 3.5 修订后的三份 yaml

> `hooks` 各层为空串 = 不启用（对应「钩子可单独启用/关闭」）。
> `mounts` 是按需挂载能力（§3.6），不进 profile 白名单。
> `allowed` 是期望集，实际生效取交集（§3.4）。

**probe.yaml**
```yaml
onion_id: probe
role: 资料与代码探查，只读，产出结构化素材
profile: research
allowed: [web_search, fetch_url, list_files, read_text_file, grep_files,
          write_artifact_file, read_own_card]
mounts: [ast_inspect]        # 可选；未挂载/语言不支持时降级 grep_files + read_text_file
max_turn: 6
temperature: 0.1
hooks:
  post_turn: ""             # 素材落产物目录由 write_artifact_file 承担，无需钩子
```

**writereditor.yaml**
```yaml
onion_id: writereditor
role: 公文撰写、PPT 大纲与生成、代码编写与 diff 补丁
profile: coder
allowed: [read_text_file, list_files, grep_files, edit_file, write_file,
          run_python, write_artifact_file, read_own_card]
mounts: []
max_turn: 8
temperature: 0.2
hooks:
  post_tool: ""             # 预留：产物写后清洗/格式化
```

**validator.yaml**
```yaml
onion_id: validator
role: 公文合规校验、政策口径与指标一致性检查、代码评审与静态检查，只读
profile: coder              # 需要 run_python 跑检查；只靠 allowed 收紧去掉写工具
allowed: [read_text_file, list_files, grep_files, run_python,
          write_artifact_file, read_own_card]
mounts: [ast_inspect]
max_turn: 4
temperature: 0
hooks:
  pre_tool: ""              # 只读钩子：只能拒绝，不能改参数
```

注意 validator 用 `coder` profile 但把 `edit_file`/`write_file` 收紧掉——
这正是「只能收紧」模型的用例：profile 给的是上限，yaml 决定本次不用什么。

### 3.6 AST：按需挂载 + 降级

- **挂载方式复用现成的动态挂载先例**：MCP 外部工具就是「默认不在内置清单、
  连接后由 `tools_json_with_mcp`（`bot/mcp/mount.rs:285`）动态追加」。
  `ast_inspect` 走同一条路——**不进任何 profile 白名单**，只在会话 yaml 声明
  `mounts` 且该能力可用时进 schema。
- **实现选型待定**（不在本轮）：Rust 生态的 AST 能力大致两条路——
  内嵌 `tree-sitter`（新增依赖、含多语言 grammar、编译期成本明显），
  或走外部 MCP 服务器（零新增依赖，复用已有 mount 机制，但要求用户自己接一个）。
  倾向后者：它同时解决了「按需加载」与「不给默认清单加负担」。
- **降级是提示层行为，不是运行时拦截**：`mounts` 未声明 / 语言不支持时，
  渲染给模型的包装里改写探查指引为「用 `grep_files` 定位 + `read_text_file` 精读」。
  不做运行时拒绝——那只会把模型卡死在「工具不可用」上。

### 3.7 Layer2 调度（非阻塞，修订稿）

```
用户消息 → L0 清洗 → L1 入库 → L2 组装
L2：主 agent 判断是否需要三段流水线（关键词/规则），并产出任务大纲
  ├─ 需要 → 依次 spawn（不等待）：Probe → WriterEditor → Validator
  │        每个 spawn 立刻返回子 agent id；会话上下文记「流水线 id」
  │        下一轮 assemble 检查：齐了打包注入 payload；没齐直接放行，
  │        并在正文提示「流水线进行中，完成后自动汇总」
  └─ 不需要 → 直接放行
→ L3 校验/改写 payload → LLM → L4 只读闸 → 工具 → L5 → L6
```

要点：

1. **派发不阻塞**：沿用 `SpawnAck` + FIFO 闸（全局 3 / 每会话 2，
   `bot_orchestrator.rs:937-938`）。三段流水线的排队深度天然受闸约束。
2. **等齐靠轮询**：复用现有 `check_subagent`，不新增等待原语。
3. **中间产物不回传**：现行已满足（子 agent 上下文是独立 session 行，父上下文
   只在收尾拿 `result_json`）。实现钩子时须保证**父 payload 只注入 `result_json`，
   不注入子会话历史**。
4. **事件不需要新建**：`subagent_start/finish/error` 已有对应实现——
   `subagent-spawned`（`:1899`，queued 即发）、`subagent-finished`（`:1884`）、
   审计事件 `subagent_done / failed / cancelled`（`:1697-1700`）。

### 3.8 七层能力边界（L4 只读的连带影响）

| 层 | 可否改写内容 | 说明 |
|---|---|---|
| L0 | 可改写用户输入 | 唯一的输入侧改写点 |
| L1/L2 | 可改消息与 payload | 记忆块、摘要、子 agent 结果注入都在这里 |
| L3 | 可改 payload（含模型参数） | token 超限、敏感词、温度/上限调整 |
| **L4** | **只可拒绝或放行** | 禁止改写 `tool_call` 参数；需要规范化参数的动作必须放到 L2/L3 |
| L5 | 可改工具结果 | 清洗、结构化、脱敏 |
| L6 | 可改最终回复 | 归档、记忆沉淀、指标 |

L4 只读带来一条实现纪律：**钩子拿到的 `tool_call` 是 `&` 而非 `&mut`**——
在类型层面挡住改写，而不是靠文档约定。

---

## 四、原生钩子注册表设计（不引脚本 VM）

```rust
pub enum HookLayer { PreIngest, Ingest, Assemble, PreExecute, PreTool, PostTool, PostTurn }

pub trait Hook: Send + Sync {
    fn name(&self) -> &'static str;
    /// 返回 HookResult::Interrupt = 拒绝本次流转（对应原稿「返回 nil」）
    fn on_event(&self, ctx: &HookCtx<'_>, input: HookInput<'_>) -> HookResult;
}
```

`HookInput` 按层给不同只读/可写视图：L4 的 `tool_call` 视图是只读引用，
L1/L2/L3 的 payload 视图是可变引用。三条纪律：

1. **顺序固定、不可跳层**：注册表只决定同层内顺序（注册序）；层序由 Rust
   函数调用栈保证，比运行时配置更硬。
2. **可单独禁用**：注册表提供 `enabled(layer)`，默认全开；yaml 里空值即禁用。
3. **钩子不得吞错**：panic 或返回错误一律记审计并放行，不得让用户配置的 bug
   变成主链路中断（与仓内既有纪律一致）。

注册粒度：进程级注册 + 会话级开关覆盖。不做按文件注册（那是脚本层的事）。

---

## 五、两层配置（决定 ③）

### 5.1 轻量元配置 → `bot_sessions` 加两列

现有表只有 5 列（`db/mod.rs:199`：`id / title / created_at / updated_at /
is_subagent`），**没有任何配置列**。新增恰好两列：

```sql
ALTER TABLE bot_sessions ADD COLUMN onion_config TEXT;     -- 引用数据目录 onions/<name>.yaml
ALTER TABLE bot_sessions ADD COLUMN onion_max_turn INTEGER; -- 可选覆盖，NULL = 用 YAML/默认
```

**纪律：能放进 YAML 的就别进 DB**。数据库只存「指向哪份配置」和「一处确实需要
每会话不同的覆盖」，大文本（角色描述、提示词片段、钩子路径表）一律不入库——
理由是 SQLite 行会被整表读进内存（会话列表每次都要读），且备份/导出语义会变复杂。

### 5.2 完整洋葱 YAML → 文件系统

落点：`<数据目录>/onions/<onion_id>.yaml`。内容就是 §3.5 三份文件 + 主会话默认
一份。加载时机：会话创建/切换时读一次，缓存在进程内；文件变更不热重载
（与现有 `BotConfig` 一样是启动时读）。

### 5.3 身份命名

沿用 `session_id` 作为洋葱唯一键，**不再引入 `onion_key` 第二套标识**——
与本轮 P0 把工具数收敛到单一常量的同一原则。yaml 里的 `onion_id` 只是
**配置模板名**，不是运行时身份。

---

## 六、与现行架构的冲突与处置（全部已定）

| 冲突 | 处置 |
|---|---|
| 原稿 `runSync()` 与现行非阻塞派发相反（最坏静默阻塞几十分钟） | **非阻塞**，见 §3.7 |
| 原稿 `allowed/denied_skills` 是权限第二真相源 | **只留 `allowed`，取交集**，见 §3.4 |
| 原稿四子 Agent 与现行三段式重叠 | **改三个**，正好对齐 recon/executor/verifier，见 §3.1 |
| 原稿 L4 可改写参数 | **L4 只读**，改写能力收到 L2/L3，见 §3.8 |
| 原稿每洋葱一份 yaml 无存储面 | **两层**，见 §五 |

---

## 七、脚本层的债（本轮明确不还）

- 现状：`src-tauri/Cargo.toml` **无任何 Lua 依赖**。引入脚本 VM 等于把应用进程的
  一个可执行扩展点交给用户目录下的文件，沙箱边界（能否读文件/起进程/调 Tauri
  命令）需单独设计，不是引入依赖就完事。
- 建议路径：先用**内置原生钩子**覆盖设计稿想要的能力；seam 稳定、权限面清楚后，
  再评估是否需要脚本层。若最终要做，优先「钩子 = 受限命令模板」而非通用脚本。

---

## 八、分期路线

| 期 | 交付 | 为什么这个顺序 |
|---|---|---|
| 0 | 工具名映射表做常量单测（§3.2 + §3.4 的硬约束） | 不写运行时代码就挡住「配置看着生效、实际落空」 |
| 0.5 | profile 缺口二选一（§3.3 A/B） | Probe 的 yaml 要真正生效，前提是 research 有 `grep_files` |
| 1 | **L4 `pre_tool` 原生注册表**：三道闸收进统一入口，钩子只读插在闸前后 | 唯一「已有实现且真有扩展价值」的一层，改动可控 |
| 2 | L5 `post_tool` + L6 `post_turn` | 与 L4 同一条链，复用注册表与顺序纪律 |
| 3 | L1/L2/L3：记忆块/摘要/埋点成为钩子链一环 | L4 的注册表与只读语义已稳定后再放宽 |
| 4 | 两层配置落地（§五） | 需要前几期的 seam 稳定 |
| 5 | L0 `pre_ingest`：真新增能力（输入清洗、任务类型预判） | 设计稿唯一的新能力，但完全独立，可后置 |
| 6 | 三子 Agent 流水线接线（§3.7） | 需要 0.5 与注册表都就位 |
| 7 | AST 按需挂载（§3.6） | 独立能力，选型定了再动 |

每期一个提交、配单测、跑 `scripts/test-fast.sh`——与仓内既有纪律一致。

---

## 九、仍需拍板

1. **§3.3 的 profile 缺口**：给 `RESEARCH_TOOLS` 加 `grep_files`（推荐），
   还是新增第 4 个 profile？
2. **§3.6 的 AST 选型**：走 MCP 外部服务器（零新增依赖，推荐），
   还是内嵌 `tree-sitter`（新增依赖 + 多语言 grammar 编译成本）？
3. **yaml 的存放与分发**：数据目录下的 `onions/` 目录是否需要设置页的导入/导出
   （与 Skill 目录的导入机制一致），还是只支持手放文件？

---

## 十、本轮未验证 / 未做的事

- 未写任何运行时代码。§二 的行号在后续改动后需重新校准。
- 未验证钩子能否在现有 async 生命周期里安全挂载——期 1 的第一步应是
  「在 L4 挂一个内置钩子跑通全链」，而不是先设计注册表 API。
- 未评估 `tree-sitter` 的实际依赖体积与多语言 grammar 的编译成本（§3.6 已建议
  走 MCP 路线规避，但未实测）。
- §3.3 的 profile 缺口意味着 **Probe 的 yaml 在改代码前不会完全生效**——
  这是已知且有意的，不是遗漏。
