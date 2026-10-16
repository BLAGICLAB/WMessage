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

### 3.6 AST：内嵌 tree-sitter（已定）+ 按需挂载 + 降级

**选型已定：内嵌 tree-sitter。** 先记一条被排除的：上一版推荐「AST 走 MCP 外部
服务器」，这条路在本仓**走不通**——外部 MCP 工具**根本进不了子 agent**：
`tools_json_with_mcp`（`bot/registry.rs:1237`）对子 agent 会话直接短路返回
profile 白名单，注释写明「外部 MCP 工具不进子 agent，§5.1 边界不破」。
而 AST 的使用方正是 Probe / Validator 两个**子 agent**，走 MCP 等于给它们配一个
永远拿不到的工具。

**依赖理由**（仓内规矩：新增依赖先写清 stdlib / 现有依赖为何不行）：
- stdlib 没有源码解析能力；
- 现有依赖里没有任何解析器——`regex`（关键词匹配）与 `grep_files`（行级检索）
  拿不到语法树，只能做文本级近似；
- native 依赖不是新风险类别：本仓已有 `rusqlite`（bundled，C）、
  `ort`（ONNX Runtime，build 期 download-binaries）、`image`（C 解码）三个先例。

#### 「按需加载」在内嵌方案下的真实含义

grammar 是 **C 代码，编译进二进制**，不是运行时加载。所以按需只有两种真实现：

1. **Cargo feature flags 按语言开**——构建期决定，产物只含启用的 grammar；
2. **全量编译**——一次编完，语言随用随加。

运行时 dlopen grammar 等于回到动态库/外部进程路线，已排除。因此「按需」的落点是
**构建期**而非运行期——这一条必须写进设计，否则实现期会误解成运行时加载。

#### 挂载面：需要新增 `MOUNT_ONLY_TOOLS` 类别（本轮定的实现约束）

现状：`tools_json()`（`bot/registry.rs:1153-1164`）遍历 `TOOLS_TABLE`，
**只跳过 `SUBAGENT_ONLY_TOOLS`**。也就是说——**任何进了 TOOLS_TABLE 的工具都会
自动出现在主 agent 的默认清单里**，这与「按需加载、不进默认清单」直接矛盾。

处置：新增第二个排除类别 `MOUNT_ONLY_TOOLS`，语义是「谁在 yaml 里声明 `mounts`
谁才看得到」：

```rust
/// 按需挂载工具：不进任何静态清单（含主 agent 默认清单），
/// 只在会话 yaml 的 mounts 声明时由 tools_json_with_mcp 形态拼装进去。
/// 与 SUBAGENT_ONLY_TOOLS 的区别：后者是「只有子 agent 可见」，
/// 前者是「谁显式声明谁可见」——主会话也可以挂。
pub const MOUNT_ONLY_TOOLS: &[&str] = &["ast_inspect"];
```

这条不是可选项：不加它，`ast_inspect` 会以第 45 个工具的身份出现在每个主会话的
schema 里，与本轮 P0「工具数单一常量、清单按需裁剪」的方向相反。

#### v1 的工具面（四项都是只读）

| 能力 | 输入 | 输出 | 降级替代 |
|---|---|---|---|
| 符号树摘要 | 文件或目录 | 类/方法/顶层函数列表 + 行号 | `grep_files` 抓 `fn`/`def`/`class` 行 |
| 定位定义 | 符号名 | 定义位置列表 | `grep_files` 精确匹配 |
| 引用列表 | 符号名 | 引用点（文件:行） | `grep_files` 精确匹配 |
| 调用链片段 | 符号名 + 深度 | 上游调用者 / 下游被调 | 无（这正是原稿「梳理调用链」要的能力） |

前三项有等价降级，**第四项没有**——这是 AST 相对文本检索的净增量，也是它值得
一份编译成本的唯一理由。

#### 工程纪律

- **解析必须走 `spawn_blocking`**：大文件语法树构建是 CPU 密集同步操作，
  直接在 async 线程跑会占住 runtime worker。先例在 `bot_chat.rs` 的图片加载
  （同理由、同解法）。
- **输出体积复用现成机制**：`ToolDef` 已有 `max_output_chars` 字段，
  dispatch 已有 `tool.output.over_budget` 审计——符号树很容易超限，
  按既有机制钳制即可，不新造一套。
- **降级是提示层行为，不是运行时拦截**：按扩展名判定语言；不支持或未挂载时，
  渲染给模型的包装里把探查指引改写为「用 `grep_files` 定位 + `read_text_file`
  精读」。不做运行时拒绝——那只会把模型卡死在「工具不可用」上。

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
| 0 | 工具名硬约束单测（§3.2 + §3.4：yaml 里的名字必须能在 `TOOLS_TABLE` 查到） | 不写运行时代码就挡住「配置看着生效、实际落空」 |
| 0.5 | **`RESEARCH_TOOLS` 增 `grep_files` + `extract_document`**（§3.3 已定）并同步 `registry.rs:1820` 断言 | Probe 的 yaml 要真正生效，前提是 research 覆盖代码检索与文档提取 |
| 1 | **L4 `pre_tool` 原生注册表**：三道闸收进统一入口，钩子只读插在闸前后 | 唯一「已有实现且真有扩展价值」的一层，改动可控 |
| 2 | L5 `post_tool` + L6 `post_turn` | 与 L4 同一条链，复用注册表与顺序纪律 |
| 3 | L1/L2/L3：记忆块/摘要/埋点成为钩子链一环 | L4 的注册表与只读语义已稳定后再放宽 |
| 4 | 两层配置落地（§五），含设置页 `onions_export` 与导入校验（§5.3） | 需要前几期的 seam 稳定；导出比现 Skill 机制多一步，是有意为之 |
| 5 | L0 `pre_ingest`：真新增能力（输入清洗、任务类型预判） | 设计稿唯一的新能力，但完全独立，可后置 |
| 6 | 三子 Agent 流水线接线（§3.7） | 需要 0.5 与注册表都就位 |
| 7 | AST 按需挂载（§3.6） | 独立能力，选型定了再动 |

每期一个提交、配单测、跑 `scripts/test-fast.sh`——与仓内既有纪律一致。

---

## 九、仍需拍板

1. **§3.6 的首批语言集**：内嵌已定，但 grammar 是逐语言编译进来的，
   支持哪几种直接决定构建时间与体积。建议首批 **rust / typescript / python / go**
   （本仓自身栈是 Rust + TS，python 是用户自动化最常见的目标语言）。
2. **§3.6 的构建策略**：Cargo feature flags 按语言开（产物小、构建要配置），
   还是全量编译（一次编完、语言随用随加）？倾向 feature flags——
   与 `tokenizers` 关 default-features 瘦身是同一思路。
3. **AST 是否要支持结构化改写**：v1 定的是四项只读能力。若要给 WriterEditor
   提供「按符号改写」的能力（如批量改调用点），那是第二个工具、另一套权限，
   应单开一轮，不要塞进 v1。

（前面几轮的待拍板均已定：profile 增 `grep_files` + `extract_document`；
AST 走内嵌 tree-sitter 不走 MCP；导入 + 导出都要，导出粒度为单份 yaml。）

---

## 十、本轮未验证 / 未做的事

- 未写任何运行时代码。§二 的行号在后续改动后需重新校准。
- 未验证钩子能否在现有 async 生命周期里安全挂载——期 1 的第一步应是
  「在 L4 挂一个内置钩子跑通全链」，而不是先设计注册表 API。
- **未实测 tree-sitter 各语言 grammar 的实际编译时间与体积增量**。§九 的语言集
  与构建策略两个问题正因此悬着——文档没给数字，只给了「按语言数线性增长」的判断。
- 未验证 §3.3 那次白名单改动的连带影响：断言单测
  `profile_whitelists_match_design_mapping`（`registry.rs:1820`）锁死了
  `general = research ∪ coder 去重`，改 RESEARCH 后并集与断言都要重跑确认——
  文档只标了必须改，没实测改完是否仍然成立。
- 未验证 `MOUNT_ONLY_TOOLS` 与既有 `tools_baseline.json` 的关系：新增该类别后，
  主 agent 清单的字节不该变，但基线断言是否还成立没跑过。
- **Probe 的 yaml 在期 0.5 落地前不会完全生效**（交集计算会把 `grep_files` 剔掉）。
  这是已知且有意的，不是遗漏。
- 子 agent 的中间产物不回传父上下文这一条，依赖现有 session 行隔离；未新增
  机制，但也未新增测试锁它（若期 6 接线时动了 msgs 组装顺序，需补测）。
