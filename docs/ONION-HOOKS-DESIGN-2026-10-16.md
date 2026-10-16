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
| 5 | **工具名一律以现行 `TOOLS_TABLE` 为准**，原稿那套名整条作废；`denied_skills` 取消 | 原稿 8 个 Skill 名里有 5 个不存在 |

第二轮（同一会话追加）：

| # | 决定 | 落点 |
|---|---|---|
| 6 | `RESEARCH_TOOLS` 增 `grep_files` 与 `extract_document`，不新增 profile | §3.3（已写明两处连带断言） |
| 7 | **AST 不走 MCP** —— 外部 MCP 工具进不了子 agent（`registry.rs:1237` 短路），上一版建议作废 | §3.6 |
| 8 | `onions/` 要设置页**导入 + 导出**；导出比现 Skill 机制多一步（Skill 只有导入） | §5.3 |
| 9 | **子 agent 不改名**，沿用 recon / executor / verifier；`role` 字段承担可读性 | §5.5 |
| 10 | 原稿工具名整体作废，不再做映射表 | §3.2 |

第三轮（同一会话追加）：

| # | 决定 | 落点 |
|---|---|---|
| 11 | AST 首批语言 rust / typescript / python / go —— **已被 #14 取代** | §3.6 |
| 12 | 编译策略 feature flag `ast-inspect` 默认关闭 —— **已被 #15 取代**（改走沙箱后无编译期） | §3.6 |
| 13 | **v1 严格只读**，符号级批量改写另开一轮（仍然有效） | §3.6 |

第四轮（同一会话追加）：

| # | 决定 | 落点 |
|---|---|---|
| 14 | **只保留 Python**，砍掉 rust / typescript(+/tsx) | §3.6 |
| 15 | **改走既有 `run_python` 沙箱 + stdlib `ast`**，不引 tree-sitter | §3.6 |

另有一条前置决定沿用：**不引入脚本运行时**（先原生钩子注册表，Lua 的债见 §七）。

---

## 一、结论先说

七层里六层在本仓已有缝隙（L4 甚至已是完整闸），只有 L0 `pre_ingest` 是真新增。
钩子运行时本质是把硬编码闸改成可注册扩展点。

三子 Agent 的修订让设计与现行架构的冲突面明显收窄：三个角色**正好一一对应现行
stage 机的 recon / executor / verifier**（`prompts/subagent.rs:27-31`），
Planner 归主 agent 后不再需要第 4 个 profile。名字也沿用这一套（§5.5），
不在设计稿再造第二套叫法。

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

| 运行时 stage（沿用） | yaml 模板名 | 角色 | profile | 是否新增能力 |
|---|---|---|---|---|
| `recon` | probe | 资料与代码探查，只读，产出结构化素材 | research | 是（代码检索/文档提取需扩白名单，见 §3.3） |
| `executor` | writereditor | 公文/PPT/代码撰写与编辑 | coder | 否 |
| `verifier` | validator | 合规校验、代码评审与静态检查 | coder（yaml 收紧掉写工具） | 部分（AST 视角可选） |

列名的分工：**stage 是运行时身份**（落进提示词与 runner 闭包），**模板名是配置名**
（`onions/<name>.yaml`，由 `bot_sessions.onion_config` 引用）。两套词各管各的域，
不互相翻译——理由见 §5.5。

**Planner 归主 agent 的理由**：现行主循环本身就有计划能力（`bot_plan`），
把它独立成子 agent 只是多一层身份与提示词，不带来新工具集。任务拆解的产物
（大纲/任务清单）由主 agent 在 L2 决定「是否需要三段流水线」时产出，
作为 WriterEditor 的输入。

### 3.2 工具名：原稿名作废，一律以现行工具表为准

原稿那套 `list_dir / read_file / shell / vector_search / ast_inspect / delete`
**整条作废**——它们不是「待映射的别名」，是写错了的名字。本仓工具的唯一真相源是
`bot/registry.rs` 的 `TOOLS_TABLE`（`BUILTIN_TOOL_COUNT` 个，见 `registry.rs:709`）；
yaml 里出现的任何工具名都必须能在那张表里查到，否则视为配置错误（期 0 单测）。

三条语义澄清（不是改名建议，是**别按字面理解**）：

1. **本仓不提供裸 shell**。`run_python` 是受限解释器沙箱：独立临时目录 + 60s
   超时 + 产物回收 + 开关默认关闭。原稿 `shell` / `shell_sandbox` 的位置由它承担，
   但它**不是** shell——没有进程管理、没有任意命令，静态检查只能写成受限脚本。
2. **没有通用向量检索工具**。`recall_facts` 是语义记忆体 v2 的记忆召回，
   不是可对任意语料做向量查询的检索层。
3. **`delete` 不存在**。`delete_task` 只删任务卡，不删文件；需要动文件时走
   `edit_file` / `write_file`（受 `bot_fs` 授权模式约束）。

### 3.3 profile 缺口：已定（本轮不改代码，仅记录精确改法）

三份 yaml 要真正生效，前提是 profile 白名单覆盖它们。现状：

- `RESEARCH_TOOLS`（`bot/registry.rs:1179`）= `web_search / fetch_url / list_files /
  read_text_file / write_artifact_file / read_own_card` — **没有 `grep_files`**，
  而 Probe 必须检索代码。
- `CODER_TOOLS`（`:1187`）= `read_text_file / list_files / grep_files / edit_file /
  write_file / run_python / write_artifact_file / read_own_card` — **没有联网工具**。
- `GENERAL_TOOLS`（`:1197`）= 两者并集，且断言单测
  `profile_whitelists_match_design_mapping`（`:1820`）锁死
  `general = research ∪ coder 去重`。

**已定**：给 `RESEARCH_TOOLS` 增 `grep_files` 与 `extract_document`（不新增 profile）。
两者都不扩大安全面：

- `grep_files` 只读检索；
- `extract_document` 有独立红线——「Office/PDF 文本层提取」，扫描件转图走
  **本地 OCR**（`ocr.rs:118`：字节全本地，不外传）。且它与 `run_python` 一样受
  `bot_fs` 文件授权模式管辖（strict 硬拒 / ask 弹窗 / yolo），白名单外不会静默读盘。

需要连带改的只有两处断言：

1. `registry.rs:1820` `profile_whitelists_match_design_mapping` 里 RESEARCH 的
   字面数组（加 `grep_files`、`extract_document`）；
2. 同测试末尾的 `general = research ∪ coder 去重` 断言——并集会随之多出这两项，
   但去重后仍是 research 的超集，改 RESEARCH 即自动满足（**改前必须重跑确认**，
   不能想当然）。

**在这次改动落地之前，Probe 的 yaml 不会完全生效**：交集计算（§3.4）会把
`grep_files` 剔掉。这是已知且有意的，不是遗漏。

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
> `allowed` 是期望集，实际生效取交集（§3.4）。
> AST 能力不进 yaml：`ast_inspect` 是普通内置工具，靠 profile 白名单约束（§3.6）。

**probe.yaml**
```yaml
onion_id: probe
role: 资料与代码探查，只读，产出结构化素材
profile: research
allowed: [web_search, fetch_url, list_files, read_text_file, grep_files,
          write_artifact_file, read_own_card]
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
max_turn: 4
temperature: 0
hooks:
  pre_tool: ""              # 只读钩子：只能拒绝，不能改参数
```

注意 validator 用 `coder` profile 但把 `edit_file`/`write_file` 收紧掉——
这正是「只能收紧」模型的用例：profile 给的是上限，yaml 决定本次不用什么。

### 3.6 Python AST：走既有 run_python 沙箱 + stdlib ast（零新增依赖）

**决定（第四轮）**：砍掉 tree-sitter，**只保留 Python**，实现走本仓已有的
Python 沙箱 `run_python` + 标准库 `ast`。

**依赖理由**：新增依赖 **0 个**。stdlib 的 `ast` 就是 Python 官方解析器，给出
精确语法树——比第三方 grammar 更贴 Python 语义（装饰器、推导式、海象、f-string
全认），不需要任何新东西。

#### 相对上一版能砍掉的

| 砍掉 | 原因 |
|---|---|
| tree-sitter 核心 + 5 套 grammar crate | 只留 Python，用 stdlib `ast` |
| Cargo `[features]` 段（本仓从未用过） | 不需要编译期开关 |
| Windows/macOS C 工具链风险 | grammar 是 C 代码，改走沙箱后消失 |
| **`MOUNT_ONLY_TOOLS` 类别** | 它唯一的用途就是挂 `ast_inspect`；工具改为普通内置工具后不再需要引入 |
| yaml 的 `mounts` 字段 | 没有了消费者。**不为未来预留**（MCP 有自己的动态挂载机制） |

#### 四项能力 → stdlib `ast` 的对应

| 能力 | `ast` 实现 |
|---|---|
| 符号树摘要 | `ast.walk` 收集 Module / ClassDef / FunctionDef + 行号 |
| 定位定义 | 同上，按符号名匹配 |
| 引用列表 | `ast.Name` / `ast.Attribute` 节点收集（按作用域过滤） |
| 调用链片段 | `ast.Call.func` 收集调用边——**跨文件需先建索引** |

**跨文件调用链是实现要点**：单文件 AST 只给语法不给语义，调用链必须基于一次索引
（扫目录下所有 `.py` 建「模块 → 符号」表，再收集跨模块调用边）。tree-sitter 同样
**不提供现成调用图**（它只有语法查询、没有语义解析），所以这条路并不比 tree-sitter
差，不是新负担。

#### 安全：必须由 Rust 侧先过授权闸（本轮发现的关键约束）

`run_python` 的 schema 自己写着「**无文件系统隔离**」（`bot/registry.rs:270`），
沙箱的 `stdin` 挂 `Stdio::null()`（`py/runtime.rs:588` / `:732`）不接受外部输入。
于是两条偷懒路都是错的：

- 让 ast 脚本自己 `open(path)` → 等于**教模型绕过 `bot_fs` 授权闸**读白名单外文件，
  strict 模式直接失效；
- 把文件内容内联进 `code` 参数 → 大文件撑爆参数，且等于把源码复制进对话历史。

**处置**：`ast_inspect` 自己先走
`resolve_with_perm(app, "ast_inspect", path, interactive, session_id)`
（与 `tool_read_text_file` 同款闸，`bot_fs.rs:1402`）：白名单外直接拒，白名单内才把
**canonical path** 交给沙箱脚本。授权判定留在 Rust 侧，沙箱侧不做判定——这正好补上
`run_python` 缺的那一层，也是这条路线必须写死的设计约束。

#### 可用性判定（替代原「编译/装配/语言」三重）

没有编译期了，剩下四层：

| 层 | 判据 | 不可用时的表现 |
|---|---|---|
| 开关 | 设置页「允许机器人执行 Python」，或 `perm_mode == yolo`（`py/document.rs:2003-2010`，yolo 跳过会记 `py_exec \| yolo_bypass` 审计） | 返回「Python 编程未开启…」 |
| 工具规则 | 规则表对 `run_python` / `ast_inspect` 的 deny/ask | 走现成 `tool_rule_gate` |
| 语言 | 目标文件必须是 `.py` | 提示层降级（§末） |
| 授权 | `resolve_with_perm` | strict 硬拒 / ask 弹窗 |

#### 归属：Validator 专有，Probe 不给

`RESEARCH_TOOLS`（`bot/registry.rs:1179`）里**没有 `run_python`**，而 yaml 只能
收紧不能放宽（§3.4）——所以 Probe 用不了 AST。建议**不给 research profile 加
`run_python`**：那会把「只读 profile」变成「可执行代码」，与已同意新增的
`grep_files` / `extract_document`（都是只读）性质完全不同。

分工因此更清晰：**Probe 做 grep 级检索**（已同意扩白名单），
**Validator 做 ast 级深审**（coder profile 本就有 `run_python`）。

#### 工具面与连带改动

- 作为内置工具进 `TOOLS_TABLE`（`BUILTIN_TOOL_COUNT` **44 → 45**），
  加入 `CODER_TOOLS` 与 `GENERAL_TOOLS`，**不进 `RESEARCH_TOOLS`**；
- 进主 agent 默认清单——主 agent 直接被问「梳理这个项目的调用链」是最常见场景，
  不该要求用户先配 mounts。这也是 **`MOUNT_ONLY_TOOLS` 与 yaml `mounts` 都不需要**
  的原因：本轮最大的机制简化；
- 连带要改的断言：`registry.rs:1820` 的 CODER / GENERAL 字面数组、
  `BUILTIN_TOOL_COUNT`、`tools_baseline.json` 基线（主 agent 清单多一个工具，
  字节会变）；
- **跨文件索引暂不缓存**：每次调用现扫，先量实际耗时再决定是否缓存——
  不为「以后可能要快」预留机制。

**v1 边界不变**：只读，不做符号级批量改写（属独立工具与独立权限面，另开一轮）。

#### 降级

非 `.py` 文件（或开关/授权未通过）时：工具不可用，模型只能看到
`grep_files` + `read_text_file`，提示层把探查指引写成「本文件走文本检索」。
**不做运行时替换**——把 `ast_inspect` 的结果换成 grep 的结果，模型会分不清自己
调的是哪个工具、参数语义也对不上。

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

### 5.3 设置页导入 / 导出（与 Skill 目录同机制，但多一步导出）

现有 Skill 目录的管理面只有四个命令：`skills_list` / `skills_open_dir` /
`skills_import`（`bot_skills/manage.rs:377`，选文件夹 → 拷贝进数据目录）/
`skills_delete`——**没有导出**。前端同款面板在 `SkillsPanel.tsx:95`。

onions/ 目录要**导入 + 导出**（本轮已定）。与 Skill 机制的差异只有一处，
且是有意多出来的：导入复用 `skills_import` 的同一套交互（选文件夹 → 拷进
`<数据目录>/onions/`），**导出需要新写一个** `onions_export`（把一份 yaml
复制到用户选的位置）。

为什么要导出：洋葱 yaml 是要被人手改的（钩子路径、`allowed` 收窄、role 描述），
只进不出的话，改好的配置无法带走、也无法分享给同事——而 Skill 正文是只读的
执行指令，导入一次就够。

**导出粒度已定：单份 yaml**（不做整目录打包）——与「一次导出一个配置」的直觉
一致，也不需要打包实现。落地时还要一起定的两件事：
2. **文件名与 `onion_id` 的一致性**：导出到别处后再导入，别名与 `onion_id` 不一致
   时以哪个为准？倾向导入时以**文件名**为准并回写 `onion_id`，否则 `bot_sessions`
   里存的那份引用会指错（§5.1 的 `onion_config` 存的就是文件名）。
3. **导入时的校验**：名字必须在字符集白名单内（防路径穿越，与 Skill 同款闸），
   且 `allowed` 里的工具名必须能在 `TOOLS_TABLE` 查到（§3.4 的硬约束前置到导入时，
   报错要指出是哪一行哪个名字）。

### 5.4 身份命名

沿用 `session_id` 作为洋葱唯一键，**不再引入 `onion_key` 第二套标识**——
与本轮 P0 把工具数收敛到单一常量的同一原则。yaml 里的 `onion_id` 只是
**配置模板名**，不是运行时身份。

### 5.5 子 Agent 要不要改名：不要，沿用 recon / executor / verifier

原稿的 `Probe / WriterEditor / Validator` 与现行 `SubagentStage`
（`bot_orchestrator.rs:41`：Recon / Executor / Verifier）是**一对一**的三段。
同时保留两套叫法，等于凭空多一张永久映射表——这正是本轮 P0 在工具数上刚清掉的
同类病（两个真相源必然漂移）。

改名成本核对过，结论是「改名反而更贵」：

- `SubagentStage` 是 **in-memory 枚举，不落库**（spawn 时随闭包进 runner，
  注释见 `bot_orchestrator.rs:342`），所以它对用户**不可见**，没有迁移负担；
- 但**提示词里已经全是这套词**（`prompts/subagent.rs:27-31` 的派发流程、
  `PROFILE_RECON` / `PROFILE_EXECUTOR` / `PROFILE_VERIFIER` 三段提示），
  换成 Probe/WriterEditor 要连带改三处提示常量 + 全部单测断言。

因此：**运行时与提示词继续用 recon / executor / verifier**；想让提示词对模型
更好读，用 `role` 字段的自由描述（「只读探查，产出结构化素材」），而不是新增枚举名。
yaml 里的 `onion_id`（probe / writereditor / validator）是**配置模板名**，
不是运行时身份——两套词各管各的域，靠 §5.4 的 `onion_config` 引用连起来。

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
| **独立线 A** | **`ast_inspect`（Python，走 run_python + ast，零新增依赖）** | **不依赖任何钩子期**：它只是一个新内置工具 + profile 条目 + 一道授权闸。可与下面的主线并行，甚至先做 |
| 0 | 工具名硬约束单测（§3.2 + §3.4：yaml 里的名字必须能在 `TOOLS_TABLE` 查到） | 不写运行时代码就挡住「配置看着生效、实际落空」 |
| 0.5 | **`RESEARCH_TOOLS` 增 `grep_files` + `extract_document`**（§3.3 已定）并同步 `registry.rs:1820` 断言 | Probe 的 yaml 要真正生效，前提是 research 覆盖代码检索与文档提取 |
| 1 | **L4 `pre_tool` 原生注册表**：三道闸收进统一入口，钩子只读插在闸前后 | 唯一「已有实现且真有扩展价值」的一层，改动可控 |
| 2 | L5 `post_tool` + L6 `post_turn` | 与 L4 同一条链，复用注册表与顺序纪律 |
| 3 | L1/L2/L3：记忆块/摘要/埋点成为钩子链一环 | L4 的注册表与只读语义已稳定后再放宽 |
| 4 | 两层配置落地（§五），含设置页 `onions_export` 与导入校验（§5.3） | 需要前几期的 seam 稳定；导出比现 Skill 机制多一步，是有意为之 |
| 5 | L0 `pre_ingest`：真新增能力（输入清洗、任务类型预判） | 设计稿唯一的新能力，但完全独立，可后置 |
| 6 | 三子 Agent 流水线接线（§3.7） | 需要 0.5 与注册表都就位 |
| ~~7~~ | ~~AST 按需挂载~~ → **并入独立线 A** | 改为零依赖的内置工具后不再需要挂载机制 |

每期一个提交、配单测、跑 `scripts/test-fast.sh`——与仓内既有纪律一致。

---

## 九、仍需拍板

只剩两条，都是 Python-only 路线带来的新问题：

1. **要不要给 `RESEARCH_TOOLS` 加 `run_python`**？加了就让 Probe 也能做 AST，
   但那会把「只读 profile」变成「可执行代码」——与已同意新增的
   `grep_files` / `extract_document`（都是只读）性质完全不同。
   倾向**不加**：Probe 做 grep 级检索、Validator 做 ast 级深审，分工已经够清晰。
2. **跨文件索引要不要缓存**：每次调用现扫目录（安全、简单），还是建一次索引复用
   （快，但要处理文件变更失效）。倾向**先不缓存**，量过实际耗时再定——
   不为「以后可能要快」预留机制。

（此前的待拍板均已定：profile 增 `grep_files` + `extract_document`；
AST 只留 Python，走 `run_python` + stdlib `ast`，不引 tree-sitter、无 feature flag、
无 `MOUNT_ONLY_TOOLS`、yaml 无 `mounts`；导入 + 导出都要，粒度为单份 yaml；
v1 只读。）

---

## 十、本轮未验证 / 未做的事

- 未写任何运行时代码。§二 的行号在后续改动后需重新校准。
- 未验证钩子能否在现有 async 生命周期里安全挂载——期 1 的第一步应是
  「在 L4 挂一个内置钩子跑通全链」，而不是先设计注册表 API。
- **跨文件调用的实际耗时未测**：每次调用现扫目录 `.py` 建索引，大仓库下可能很慢。
  这是「要不要缓存」那条待拍板的依据所在——没有数字就不该预先设计缓存。
- **沙箱内跑 ast 脚本的真实开销未测**：每次调用起一个 Python 进程，叠加
  `run_python` 默认 60s 超时；大项目多文件调用是否够快未验证。
- **ast_inspect 的输出体积未估**：符号树在大模块上可能爆 `max_output_chars`，
  实际钳制阈值要等实现后量。
- 未验证 §3.3 那次白名单改动的连带影响：断言单测
  `profile_whitelists_match_design_mapping`（`registry.rs:1820`）锁死了
  `general = research ∪ coder 去重`，改 RESEARCH 后并集与断言都要重跑确认——
  文档只标了必须改，没实测改完是否仍然成立。
- 未验证「新增 ast_inspect 后 `tools_baseline.json` 基线要重新生成」的具体影响：
  按理主 agent 清单会多一个工具的 schema 字节，但基线文件比对的是全文还是逐项、
  重生成流程是什么，都没跑过。
- **Probe 的 yaml 在期 0.5 落地前不会完全生效**（交集计算会把 `grep_files` 剔掉）。
  这是已知且有意的，不是遗漏。
- 子 agent 的中间产物不回传父上下文这一条，依赖现有 session 行隔离；未新增
  机制，但也未新增测试锁它（若期 6 接线时动了 msgs 组装顺序，需补测）。
