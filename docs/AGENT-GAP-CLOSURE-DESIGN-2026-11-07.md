# Agent 差距收编方案（D-1～D-7）

> **状态：方案，未实施。** 2026-11-07 大厂对比调研（Claude Code / Codex / Gemini CLI / Cursor /
> Cline / OpenHands）得出 7 项可借鉴差距（D-1～D-7），用户拍板全部做，本文是细化方案。
> 上游调研报告（差距全景、业界口径与来源链接）见当日会话记录；本文只放落地必需的
> 现状事实、设计与锚点。冲突时以本文为准。

## 〇、总览

| # | 项 | 一句话 | 性质 |
|---|---|---|---|
| D-1 | checkpoint 回合级快照回滚 | 快照层已有，扩覆盖面 + 补回合级三态恢复 + 补清扫 | 增量补齐 |
| D-2 | 用户指令文件层 | `instructions.md` 进 system 稳定段，用户可编辑 | 新增 |
| D-3 | 工具分组按需挂载 | 休眠组 + `activate_tools` 元工具，schema 缩面 | 复议旧拍板（轻量版） |
| D-4 | auto 档分类器兜底 | 白名单外弹窗前插一次侧调分类，三态判决 | 新增（灰度） |
| D-5 | eval 一键脚本 | `run-eval.sh` + 「改了必跑」写进测试文档 | 纯流程 |
| D-6 | MCP OAuth | HTTP 传输服务器支持 OAuth 2.1 授权码 + 刷新 | 新增 |
| D-7 | MCP elicitation | 服务器反问接 ask 卡，per-server 开关 | 新增 |

批次顺序与依赖见 §八；不做清单 §九；实施内待拍板 §十。

---

## 一、D-2 用户指令文件层

### 1.1 现状事实

- system 装配点：`bot_chat/flow.rs` 的聊天主链，用 `SystemPromptBuilder`（`prompt_builder.rs`）
  按 `PromptSlot` 声明顺序拼接；**`PromptSlot` 新变体只能追加在末尾**（prompt_builder.rs 顶部约定）。
- 现有槽序：Base（SYSTEM_PROMPT + MAIN_AGENT_ADDENDUM）→ VerifierRetryTail → GenDir →
  SkillCatalog → SkillBody → Recovery → Plan。system 非字节稳定（SkillCatalog 每轮按本轮
  query 嵌入重排、Plan 按需出现），缓存设计意图是 Base/GenDir/SkillCatalog 段命中前缀缓存。
- 记忆注入走 P1 机制并进本轮用户消息末尾（`bot_chat/inject.rs`），不占 system。
- 记忆面现状是 DB 型语义记忆体（检索注入），**没有用户可直接编辑、可 diff 的确定性指令文件**。
- 设置页「记忆」分区：`SettingsPage/MemorySection.tsx`（186 行，三张卡），是本项的 UI 挂载点；
  get/set 命令模式可照抄 `MemoryTuningCard`（`MemoryPanel.tsx` 的 `memory_tuning_get/set`）。

### 1.2 业界口径

Claude Code auto memory（4 类 markdown + 索引每会话加载，默认开）、Codex Memories
（`~/.codex/memories/` markdown、可 git diff、默认关）、Gemini `save_memory` 追加 GEMINI.md——
收敛结论：**文件式记忆是 coding agent 默认形态**，本质是「用户权威指令层 + agent 自动记忆层」
两层互补。本仓 DB 语义记忆对应第二层，缺第一层。

### 1.3 设计

- **文件**：`data_dir/instructions.md`（单文件 v1，不做多文件/分层）。
- **装配**：新槽 `PromptSlot::UserInstructions`，插在 VerifierRetryTail 之后、GenDir 之前。
  每回合读一次盘：内容没变则字符串等价、前缀缓存不破坏；用户改完下一回合生效（一次 cache
  bust，低频可接受）。文件缺失/为空 → 槽为空串，不加额外文案。
- **上限**：`INSTRUCTIONS_MAX_CHARS = 16_000`。保存命令超限拒（返回人话错误）；外部改超限
  时载入前 16_000 并尾部加「(已截断)」标注 + 审计 WARN。
- **命令**：`instructions_get`（返回内容 + 字数 + 上限）/ `instructions_set`（校验长度，
  `db::atomic_write` 落盘，审计 `instructions.updated`）。
- **前端**：`MemorySection.tsx` 加一张卡（与现有三卡平级）：文本域 + 字数/上限显示 + 保存
  按钮 +「打开文件」（`reveal_path` 语义）。
- **边界**：内容整体视为用户可信输入，不做 sanitize；不进子 agent system（子 agent 沿用
  profile 提示词，不注入）；不进任务卡执行 system（v1 只挂主聊天，行为面最小）。

### 1.4 锚点

1. `instructions_set` 超限拒、合法落盘、审计有事件；文件不存在时 `instructions_get` 返回空。
2. system 装配含文件内容；会话内两回合文件未变则 system 该段字节相同；改文件后下一回合变化。
3. 文件被外部改成超长 → 载入截断带标注 + WARN 审计。

---

## 二、D-5 eval 一键脚本与「改了必跑」流程

### 2.1 现状事实

- `bot/eval.rs`：13 用例（E01-E12 + E07b），`#[tokio::test] #[ignore]` 名为
  `eval_prompt_discipline_report`；跑法
  `cargo test --manifest-path src-tauri/Cargo.toml --lib bot::eval -- --ignored --nocapture`；
  单跑 `WMESSAGE_EVAL_ONLY=E05,E11`；判据全机器可判定（`judge: fn(&CaseOut) -> Vec<String>`），
  无 LLM-as-judge；配置快照 `src-tauri/eval-bot-config.json`（gitignore），key 走 keyring。
- **模块头注释写「12 用例」与实际 13 条不符**（E07b 加入后没更新）——顺手修。
- memory 黄金集：`tests/memory_eval.rs` 的 `eval_recall_report`（recall@1/@5 分项）。
- 既有拍板：不引外部评估平台、不上 LLM-as-judge、eval 不进 CI/pre-push——本项不推翻，
  只补「手动一键 + 改动后必跑」的流程。

### 2.2 设计

- **脚本** `scripts/run-eval.sh`：
  1. 打印提示「将调用真实模型，消耗账户额度，按回车继续 / Ctrl-C 取消」；
  2. 跑 `bot::eval` 全量（透传 `WMESSAGE_EVAL_ONLY`）；
  3. 可选第二段：`--memory` 时追加跑 memory 黄金集；
  4. 末尾打印汇总与 tokens，并提示「把 PASS/FAIL 数字与偏差记入 DEVLOG」。
  不做 Windows 版（实机自动化不做 Windows 拍板在案）。
- **流程**：`docs/testing.md` 加一节「提示词 / 引擎行为改动后的 eval 口径」——什么算
  「提示词/引擎行为改动」（SYSTEM_PROMPT、prompts/ 目录、model_loop 行为、工具 schema 文案）、
  跑什么、怎么记账（DEVLOG 记 PASS 数对比）。

### 2.3 锚点

1. 脚本在取消输入时零执行；`WMESSAGE_EVAL_ONLY` 透传生效。
2. eval.rs 头注释用例数与 `all_cases().len()` 一致（顺手补一条断言式对拍：模块头不再写死数字）。

---

## 三、D-1 checkpoint 回合级快照回滚

### 3.1 现状事实（关键：快照层已存在，本项是补齐不是新建）

已有的全链：

- **快照层**：write_file/edit_file 修改已有文件时，把 before 全文快照到
  `data_dir/checkpoints/<uuid>`（`bot_fs/receipt.rs` 的 `write_before_snapshot`，原子写）；
  `file_changes` 表有 `before_ref/before_sha/after_sha`。
- **逐文件回滚**：`file_rollback` 命令（`db/trace.rs` 的 `file_rollback_core`）——只回 modify；
  漂移闸（当前 sha ≠ after_sha 拒回滚）+ 快照闸 + 符号链接拒 + 64MB 上限。
- **消息侧原语**：`bot_history_drop_after`（锚行保留、删其后全部，`/retry` 在用）与
  `bot_history_drop`（单行删）。
- **凭证面**：edit/write/manage_file copy/截图/archive/doc 族已发 `with_file_change` 凭证。

缺口（本项全部要补）：

1. **覆盖面**：saveAs（裸 `fs::write` 且无凭证）、apps_update_files（裸写无凭证）、
   manage_file move/delete（无凭证无快照）、copy（只有 after sha）；
   doc 族/archive/截图有凭证但 modify 场景无 before 快照。
2. **回合级聚合回滚**：只能逐文件回，没有「回到某轮」；消息侧原语已就绪。
3. **清扫**：`checkpoints/` 目录无任何保留期管理；`retire_traces_before` 删 change 行后
   快照文件成孤儿。
4. **忙时闸**：`ChatGuard` 只读查询是 `#[cfg(debug_assertions)]` only，生产无「会话是否忙」
   查询口。

### 3.2 业界口径

三家同构（Cline/Gemini shadow git、Claude Code 每轮快照 + `/rewind`）：**快照 + 漂移检测 +
「只回文件 / 只回对话 / 都回」三态恢复**是 table stakes；Cline 官方直言「checkpoints 让
auto-approve 可用」——廉价撤销是权限放开的底气。OpenHands V1 从架构侧印证：事件流 +
状态重放，checkpoint 只是截断点（本仓真源化已具备事件面）。

### 3.3 设计

**A. 覆盖面扩齐（写路径接快照）**

| 写路径 | 改法 |
|---|---|
| http_request saveAs（`bot/tools/http.rs`） | 裸写改 `db::atomic_write`；目标已存在时先 `write_before_snapshot`；接 `with_file_change` 凭证（kind=modify/create） |
| apps_update_files / apps_create（`apps/manage.rs` 的 `write_file_set`） | 逐文件：已存在先快照；接凭证。apps 产物在 `data_dir/apps/<id>/`，同样入账（改动页按 path 展示，不区分根） |
| manage_file delete | 删前把内容快照进 `checkpoints/`，发凭证 kind=delete（before_ref/before_sha 有值，after_sha=None） |
| manage_file move | 源发 kind=delete 凭证（含快照）、目标发 kind=create 凭证；跨卷实现不变 |
| manage_file copy | 补 kind=create 凭证（现状只有 sha 无凭证） |
| doc 族 / archive / 截图的 modify 场景 | 已有凭证的补 before 快照（与 edit.rs 同款调用） |

- create_skill / install_skill 不接（技能有自己的失败回滚与版本语义）——登记不做。
- 快照失败沿用现有降级：before_ref=None、不阻断业务、审计 `checkpoint.write_fail`。
- 超过 64MB（与 `file_rollback_core` 上限一致）不快照，凭证照发（可观测不可回滚，诚实）。

**B. file_rollback_core 扩 create/delete**

- kind=create 回滚 = 删除该文件（走 trash crate 系统回收站，与 manage_file delete 同语义）。
- kind=delete 回滚 = 从 `checkpoints/<before_ref>` 写回（`db::atomic_write`）；目标路径已被
  占用（存在且 sha 与快照不符）→ 拒，人话报告。
- 漂移闸沿用：目标当前 sha ≠ 凭证 after_sha → 跳过该条并如实报告（回合级聚合时不中断整批）。

**C. 回合级回滚命令 `bot_turn_rollback`**

- 入参：`sessionId`、`anchorMsgId`（用户消息行）、`files: bool`、`chat: bool`、
  `includeAnchorMsg: bool`。
- 忙闸：新增生产可用只读访问器 `ChatGuard::is_busy(session_id)`（把 debug-only 查询转正）；
  命令内先查，忙 → 拒「会话有在途回合」。前端入口在点击时预检 + 命令侧再查（双保险）。
- 文件侧范围：锚行的 `created_at` → 该会话 `exec_traces.created_at` 晚于锚行的
  `file_changes` 行（按 path 取最新一条决定终态），逐条走 B 的扩展回滚；产出
  `RollbackReport { restored, skippedDrift, failed }`。
- 消息侧：`chat=true` → `bot_history_drop_after(anchorMsgId)`（复用，锚保留）；`chat=true`
  且 `includeAnchorMsg=true` → 先 drop 锚行（`bot_history_drop`）再 drop_after 语义由
  drop 锚后自然覆盖（实现时合并为一次事务式删除，发 `bot-msg-dropped`）。
- 顺序固定：先文件后消息；消息删了文件侧就无法再按锚寻址（锚行时间戳是文件范围的键）。
- 审计：`turn_rollback` 事件（sessionId、scope、report）。
- 前端入口：用户气泡操作区加「撤回到此」（与既有重试入口并列）→ 小弹层三个选项
  （撤文件改动 / 撤回复保留我的消息 / 连我的消息一起撤，文件与对话可同时勾选）→
  确认弹窗 → 完成后 toast 报告 restored/skipped 数字，历史与改动页 refetch。

**D. 清扫**

- `retire_traces_before` 事务内：删除 change 行前收集 `before_ref` 清单，事务后删对应
  `checkpoints/` 文件（孤儿不再产生）。
- 存量孤儿：`spawn_retention_sweep` 同节奏（启动 + 每 24h）加一步——遍历 `checkpoints/`
  删除「无任何 change 行引用且 mtime > 30 天」的文件。常量
  `CHECKPOINT_RETENTION_DAYS = 30`（与 TRACE/OVERFLOW 对齐）。

### 3.4 锚点

1. saveAs 覆盖已存在文件 → 产生 before 快照与 modify 凭证，`file_rollback` 能回。
2. apps_update_files 同上；manage_file delete → delete 凭证 + 快照，扩后的回滚能从快照写回。
3. create 凭证回滚 → 文件进系统回收站；delete 回滚时目标被占 → 拒且报告。
4. 回合级：modify+create+delete 混合回合，`files=true` 全复原、漂移文件跳过并进报告；
   `chat=true` 只删消息不动文件；先文件后消息的顺序断言。
5. 忙时拒：ChatGuard 持有时命令拒，空闲放行。
6. 清扫：retire 后对应快照文件消失；孤儿 + 超期文件被 24h sweep 收走。
7. 既有逐文件 `file_rollback` 回归（漂移闸语义不变）。

---

## 四、D-3 工具分组按需挂载（旧拍板的轻量复议）

### 4.1 现状事实

- `bot/registry/schemas.rs` 1226 行 / 约 97K 字符；主聊天默认清单 = 64 条 schema
  ≈ 6.5 万字符（约 16K token 级）随每请求发送。「13 组」只是 FACT-SHEET 文档归类，
  **代码里没有组结构**。
- 装配链：`tools_json_with_mcp(session_id)`（schemas.rs）→ `tools_json_for` 按会话现算
  （Cow）：子 agent 走 profile ∩ allowed 白名单交集（唯一现成的按会话缩面机制），其余会话
  全量；MCP 工具尾部 splice。每轮 `prepare_round` 调用——按会话换清单有现成缝。
- dispatch 兜底：子 agent 白名单是「schema 层过滤 + dispatch 闸双保险」——按组缩面必须
  同样双保险，否则模型幻觉调未挂载工具会查表命中照跑。
- 拍板在案：CONTEXT-TOOLCALL 方案 §七「不做 Programmatic Tool Calling、Tool Search Tool」。
  本项是**轻量复议**：不做语义搜索、不做写代码调工具；只做「固定 13 组 + 休眠/激活」，
  触发条件是 66 工具 + MCP 挂载增长后 schema 常驻成本持续上涨。

### 4.2 业界口径

Anthropic Tool Search Tool（工具 defer_loading，只留 ~500 token 搜索器）与 PTC 已成
2026 工具爆炸解法三件套之二；本仓「技能渐进披露」（目录层先行、正文按需）已是同构思路，
工具面照抄其哲学。

### 4.3 设计

- **组常量**：新文件 `bot/registry/groups.rs`（避免 schemas.rs 膨胀，行数棘轮注意）：
  `TOOL_GROUPS: &[(&str, &[&str])]`（组名 → 工具名，13 组照 FACT-SHEET 口径）、
  `DEFAULT_GROUPS`（默认挂载）、`DEFERRED_GROUPS`（休眠）。`mutating_tools` 等既有函数不动。
- **量化先行**：`#[test]`（`--nocapture`）打印全量与各组序列化字符量——数字落 DEVLOG，
  并回填本文 §4.5；默认/休眠集以数字 + 行为风险微调（实施内拍板，见 §十 P-1）。
  推荐初版：休眠「桌面系统（含窗口/UI/截图/open_url/clipboard）+ 受管服务 + 程序 apps」
  三组，主聊天 64 → ~50 条。
- **激活工具** `activate_tools`（进 TOOLS_TABLE 且 ∈ 默认组）：入参 `groups: string[]`
  （只接受休眠组名）；把组标记写进 `AppState` 的会话级
  `Mutex<HashMap<session_id, HashSet<&'static str>>>`；返回已激活清单。休眠组在 system
  加一行固定文案（GenDir 槽附近）：「另有桌面系统/受管服务/程序三组工具默认休眠，
  需要 `activate_tools` 启用」——让模型知道能力存在（防「不知道所以不用」）。
- **装配**：`tools_json_for` 主聊天分支按「默认组 + 该会话已激活组 + subagent-only 过滤」
  拼装；MCP 尾挂不变。缓存影响如实登记：激活瞬间 tools 参数变化，Anthropic 缓存链
  （tools→system→msgs）从 tools 层起重算一次，本回合 input 上涨，之后恢复稳定前缀——
  低频动作，摊销可接受。
- **dispatch 兜底闸**（新一级，排在 per-tool 规则表之前）：工具 ∈ 休眠组 且 该会话未激活 →
  Denied + 人话提示「该工具休眠中，先调用 activate_tools」。子 agent 白名单链路不受影响
  （narrowed 交集先发生，子 agent 无 MCP、其白名单与组常量正交）。
- **范围**：只挂主聊天。任务卡执行/定时作业/exec 会话维持全量（执行面是最依赖工具面的
  场景，缩面行为风险大）——登记不做。
- 开关：bot-config `deferredToolGroups: bool` 默认 false（灰度，默认关=行为零变化）。

### 4.4 锚点

1. 量化测试输出全量/分组字符量（数字 ≥ 6 万字符级）。
2. 默认关时 `tools_json_for` 返回与现状逐字节相同（快照对比）。
3. 开启后：休眠组工具不在 tools 数组；模型调休眠工具 → dispatch 拒且文案含 activate_tools；
   activate_tools 激活后下一轮 schema 含该组且 dispatch 放行。
4. activate_tools 传休眠清单外的组名 → 拒；子 agent 会话调 activate_tools → 拒。
5. MCP 工具不受组影响（激活前后 MCP 尾挂一致）。

### 4.5 量化数字回填位

（实施批 4 第一步回填：全量 ___ 字符 / 默认集 ___ 字符 / 休眠三组 ___ 字符。）

---

## 五、D-7 MCP elicitation 接 ask 卡

### 5.1 现状事实

- MCP 工具链路：dispatch 查表 miss → `execute_mcp_tool` → `execute_mcp_direct`（mount.rs）→
  `McpManager::call_tool`（per-server `call_locks` 串行 → `ensure_connected` 按需拉起 →
  `invoke_tool` → `retire` 调用完即回收）。
- **服务器主动发来的带 id request（elicitation/create、sampling/createMessage）现在零处理**：
  宿主用默认 `ClientHandler for ()`（rmcp 3.5.0），elicitation 默认自动 Decline、sampling 返回
  method_not_found——不会挂死，是静默吞掉。要接 elicitation 就是实现自定义 ClientHandler。
- ask 卡全链现成：`ask_engine.rs` 的 `engine_ask_user`（`bot-ask` 事件 + `question_waiters`
  oneshot + `wait_ask_reply` 24h 上限）→ 前端 AskCard → `ask_respond` 回程；
  无人值守/汇报轮不注册（`ask_registered` 闸）；前端看门狗对未答 ask 豁免。
- MCP 机密走 keyring：`keyring_entry(id)` = service `wmessage.bot.v1` + user `mcp:{id}`。

### 5.2 业界口径

MCP elicitation（服务器向用户反问）规范已稳定（2026-07-28 版含 URL-mode）；大厂客户端
（Claude Code / Claude Desktop）支持 form 式 elicitation；安全共识：**对不可信 server 默认关**、
内容按不可信渲染。本仓 ask_user 内嵌选择题卡（方案 B 已落地）形态完全同构，缺的只是协议接线。

### 5.3 设计

- **per-server 开关**：MCP 配置加 `allowElicitation: bool` **默认 false**（不可信 server 默认
  关，对齐业界共识与「默认行为=现状」）；McpPanel 加开关。开启的服务器才走自定义 handler，
  否则沿用默认 Decline（等价现状）。
- **handler**：`spawn_service` 的 HTTP 分支按开关注入实现 `elicit()` 的 ClientHandler（拿
  app handle）；`call_tool` 入口登记在途调用 `ACTIVE_MCP_CALLS: Mutex<HashMap<server_id,
  (session_id, stop)>>`（`call_locks` 已保证同 server 串行，登记天然单条），elicit 到达时
  反查会话；无在途调用 → 直接 Decline（SEP-2260 关联校验由 rmcp 内部兜底）。
- **接卡**：v1 只支持单字段 schema（`string` 自由文本 / `boolean` / `enum` 单选）——映射进
  现有 AskCard 结构（message=问题、enum→选项、bool→确认/拒绝）；多字段表单登记不做。
  卡面必显示来源 server 名（不可信内容提示）+ 字段名，内容按纯文本渲染。
- **等待**：60s 超时（对齐 ConfirmMap 默认拒节奏，不等 ask 的 24h——elicitation 是工具调用
  内同步等待，长等占住 `call_locks`）；`/stop` → 立即 Decline；无人值守（会话非
  TurnOrigin::User）或主窗口不可见 → 立即 Decline；等待期间 `retire` 语义核对：等待在
  `invoke_tool` 之前完成（elicitation 是服务器在工具执行中发来的请求，宿主侧等待发生在
  call_tool 的 future 内部）——实现时验证 rmcp 长等待不断连，若断连按现有 Down 标记
  报错重试路径走。
- **安全**：应答字段值不进审计、不进日志（可能含凭据）；字段数与长度钳制（单字段 ≤ 2KB）；
  elicitation 请求的 message 以 server 名为前缀渲染防误导。
- 范围：只做 HTTP 传输（stdio server 的 elicitation v1 不接——本仓 MCP HTTP 只连公网、
  stdio 是本地进程， elicitation 价值集中在远程 SaaS server）——登记不做。

### 5.4 锚点

1. 开关默认 false：开启 elicitation 的 server 调用到达 → 仍自动 Decline（行为与现状一致）。
2. 开关 true + 交互会话：工具调用中出现 elicitation → AskCard 弹出（含 server 名），
   应答 string/bool/enum 三型各自正确回传 ElicitResult；超时 60s → Decline。
3. `/stop` 立即 Decline；无人值守不弹卡直接 Decline；字段值不出现在审计/日志。
4. 无在途工具调用时 server 主动发 elicitation → Decline。
5. stdio server 发 elicitation → Decline（范围外，行为与现状一致）。

---

## 六、D-4 auto 档分类器兜底

### 6.1 现状事实

- auto 档当前语义（消费点仅文件闸 + write_file 覆盖确认）：读/写路径白名单外与 Ask 同分支
  弹授权窗（`bot_fs.rs` 的 `ask_path_confirm` / `bot_fs/edit.rs` 的 `ask_user_confirm`）；
  auto 独有 = write_file 白名单内覆盖跳确认。**非文件工具不走 perm_mode**（web 已有目的地
  闸、MCP 无 perm 语义）。
- 无人值守快速拒已有多处：`bot_slash.rs` 的 `ask_confirm_inner`（`!interactive` 直接拒）、
  子 agent 会话 ask 直接拒、`tool_open_url` 后台拒。
- 侧调通道现成：`summarize_http`（`bot_chat/summary.rs`，双协议、60s per-request 超时、
  不进 llm.usage 记账、不发 llm.request）；配置走 `selfcheckModelId`
  （解析失败/厂商未过连接测试回落 None 的先例在 `decide_selfcheck`）。

### 6.2 业界口径

2026 大厂共同收敛：Claude auto mode（分类器模型审动作，连拦 3 次退回人工）+ Codex
Auto-review（审批先过审查 agent）——「模式档位 + 白名单 + 分类器 + OS 沙箱」四层叠加。
本仓已有档位/白名单/沙箱三层，缺分类器层。

### 6.3 设计

- **判决位置**：`ask_path_confirm` 与 `ask_user_confirm` 的弹窗分支**之前**插
  `auto_gate_decide`。触发条件四条全满足才触发：`perm_mode == Auto`、interactive、
  bot-config `autoGateClassifier == true`、`selfcheckModelId` 已配置且可用。
- **三态判决**：allow → 放行（跳过弹窗）；deny → 直接拒（工具结果 Denied + 人话理由，
  模型可换路）；unsure（模型不可用/超时/输出不可解析）→ 走原弹窗流程——
  **fail-safe 不 fail-open**：任何异常都绝不变成放行。
- **调用**：复用 `summarize_http`，超时参数化出 8s 档（现 60s 硬编码改入参，默认值不变）；
  输入 = 工具名 + 目标路径/参数摘要 + 本回合用户原话前 400 字；输出 max_tokens 8、
  temperature 0，解析 allow/deny/unsure。提示词常量放新文件 `bot/gate/auto_classifier.rs`
  （闸 + 提示词 + 解析同文件，≤ 300 行）。
- **连拦退回**：会话级计数（进 AppState 现有会话 Map 结构），连续 3 次 deny → 本回合起
  停用分类器（回归全弹窗），对齐 Claude auto mode 口径。
- **审计**：`auto_gate.classifier { decision, latency_ms, tool }`——不含路径参数原文之外的
  敏感面；不进 llm.usage（沿侧调口径）。
- **范围**：v1 只挂文件读/写两个闸点；不挂 MCP / web（目的地闸已覆盖）；**无人值守不启用**
  （保持快速拒现状，行为零变化）——登记不做。开关默认 false（灰度）。

### 6.4 锚点

1. 开关关 / 非 auto 档 / 非交互 → 行为与现状逐字节一致（既有弹窗测试回归）。
2. 分类器 allow → 跳过弹窗直接放行；deny → Denied + 理由；unsure → 走弹窗。
3. 侧调超时/网络失败/输出乱文 → unsure（fail-safe）。
4. 连续 3 deny → 第 4 次起不再调用分类器、直接弹窗。
5. 分类器调用不出现在 llm.usage，`auto_gate.classifier` 审计在案。

---

## 七、D-6 MCP OAuth（HTTP 传输）

### 7.1 现状事实

- MCP HTTP 鉴权现状：配置 `headers: BTreeMap<String,String>` 全部作为 reqwest
  default_headers 挂上每个请求（`bot/mcp/manager.rs` 的 `http_client_for_mcp`）；
  机密由配置加载时 `hydrate_mcp_servers` 从 keyring（`mcp:{id}` 条目）注入——静态、无刷新。
- URL 三道闸（字面量 + DNS 全解析 IP + 钉 IP）在 `http_client_for_mcp` 调用侧。
- tiny_http 已有双实例可行性：`api_server.rs` 用 `Server::http(("127.0.0.1", port))`，
  测试里有 `127.0.0.1:0` 随机端口先例；浏览器打开现成 `app.opener().open_url`。

### 7.2 业界口径

Claude Code `claude mcp login`（OAuth + CIMD 自动发现）、MCP 官方 Registry 与 OAuth 2.1
Resource Server 语义（2026-07-28 规范）：授权码 + PKCE + resource 参数是标配，多数服务器
要求动态客户端注册（DCR，RFC 7591）。

### 7.3 设计

- **新模块** `bot/mcp/oauth.rs`（登录流程 + token 存储 + 刷新），命令三件：
  `mcp_oauth_login(serverId)` / `mcp_oauth_logout(serverId)` / `mcp_oauth_status(serverId)`。
  仅对 HTTP 传输服务器可用（stdio 返回明确错误）。
- **登录流程**（用户手动触发 command，非工具调用，浏览器打开不走 `tool_open_url` 的
  interactive 闸，直接 `app.opener().open_url`）：
  1. 发现：GET `/.well-known/oauth-protected-resource`（RFC 9728）→
     `/.well-known/oauth-authorization-server`（RFC 8414）；404 时直接对 server URL 根试
     授权服务器元数据（兼容简化实现）。
  2. 注册：DCR（RFC 7591）POST 注册客户端；服务器不支持 DCR → 报人话错误（提示手动
     填 client_id——MCP 配置加可选 `oauthClientId/oauthClientSecret` 手动档）。
  3. 授权码：PKCE S256 + state；loopback 回调服务器 `tiny_http::Server::http(("127.0.0.1", 0))`
     随机端口、只收一个请求、5 分钟超时；redirect_uri = `http://127.0.0.1:<port>/callback`；
     授权 URL 带 `resource` 参数（RFC 8707，MCP 规范要求）。
  4. 换 token：authorization_code → access_token + refresh_token；校验 state。
  5. 存储：进 keyring（`mcp:{id}` 条目扩字段或新条目 `mcp-oauth:{id}`——实现时按
     `store_server_secrets` 的 blob 结构扩容，避免双条目迁移）；client_secret 同存。
     state/PKCE verifier 只在内存，不落盘。
- **连接**：`http_client_for_mcp` 对配置了 OAuth 的 server 改挂
  `Authorization: Bearer <access_token>`（从 keyring 读，替换/优先于静态 headers）。
- **401 处理**：`invoke_tool` 捕获 401/403 → 尝试 refresh 一次（grant_type=refresh_token）
  → 重建 client 并重连一次（复用 `retire` + `ensure_connected` 现有路径）→ 仍失败 →
  人话错误「授权已过期，请重新登录该 MCP 服务器」；refresh 也 401 → 清 token、要求重登。
- **安全**：token/secret 不进审计、不进日志（错误消息只含状态码）；回调服务器只绑
  loopback、只读一个请求的 code/state 两个参数；凭据处理对齐 HTTP-REQUEST 方案口径
  （不回显/不进审计/不进产物）。
- **范围**：只做 authorization_code + refresh_token；不做 device flow、client_credentials、
  token 交换（RFC 8693）——登记不做。

### 7.4 锚点

1. 非 HTTP 传输调 `mcp_oauth_login` → 明确错误。
2. 登录全流程（mock 授权服务器）：发现→DCR→PKCE→回调→换 token→keyring 落库→
   status 可查；state 不匹配 → 拒。
3. 连接后请求带 Bearer；token 失效 401 → refresh 成功自动重连成功一次。
4. refresh 失败 → token 清除、错误人话；凭据不出现在任何审计/日志事件。
5. 静态 headers 服务器（未配 OAuth）行为逐字节不变。

---

## 八、批次切分与顺序

串行推进，每批一个逻辑变更一个 commit；批内第一步先落量化/现状数字（有数字再动刀）。

| 批 | 内容 | 关键前置 | 验收 |
|---|---|---|---|
| 1 | D-2 指令文件（命令 + 槽 + 设置页卡） | 无 | §1.4 三锚点 |
| 2 | D-5 eval 脚本 + testing.md 流程 + 头注释修正 | 无 | §2.3 |
| 3 | D-1 checkpoint（A 覆盖面 → B create/delete → C 回合级 → D 清扫，四段四个 commit） | 无 | §3.4 七锚点 |
| 4 | D-3 分组挂载（量化 → groups.rs → activate_tools + dispatch 闸 → 前端开关） | 量化数字回填 §4.5 | §4.4 |
| 5 | D-7 elicitation（开关 + handler + 接卡） | 无 | §5.4 |
| 6 | D-4 分类器（summarize_http 超时参数化 → auto_classifier.rs → 双闸点接线 → 灰度开关） | 无 | §6.4 |
| 7 | D-6 OAuth（oauth.rs → 连接改挂 → 401 刷新 → 命令 + McpPanel UI） | 批 5 的 handler 注入缝 | §7.4 |

- 收尾（随每批）：SPEC 加功能条目 → 同步 `docs/acceptance/matrix.csv` 与对应卡
  （`audit_acceptance_matrix.py` 三方对拍）；DEVLOG 记「为什么」。
- 批 3 最大，实施时先做 A/B（文件侧自洽可独立验收），C 依赖 A/B，D 独立可穿插。

---

## 九、不做清单（本方案内明确不做）

- shadow git 全仓快照——沿用 `checkpoints/` 增量快照，不引 git 依赖。
- 消息树 / fork 分支——BACKEND-TRUTH 拍板不做在案。
- D-1：delete 的「系统回收站内恢复 UI」（应用内恢复走快照写回；系统回收站仍是用户兜底）；
  create_skill / install_skill 接快照。
- D-3：PTC / 语义工具搜索 / 执行会话缩面 / 多字段激活编排；只做固定组 + 单元工具激活。
- D-4：无人值守启用分类器；MCP / web 工具挂分类器。
- D-5：eval 进门禁 / Windows 版脚本。
- D-6：device flow / client_credentials / token 交换；stdio 传输 OAuth。
- D-7：多字段 form 表单；stdio 传输 elicitation；sampling/createMessage（维持 rmcp 默认
  method_not_found）。
- 业界对齐但明确不追：云端 agent、agent teams、OTel 外发遥测（调研结论支持不做，登记备查）。

---

## 十、实施内待拍板

| # | 项 | 推荐 |
|---|---|---|
| P-1 | D-3 休眠组初版集合（桌面系统/受管服务/程序三组） | 按推荐做，量化数字出来后允许微调 |
| P-2 | D-1 回合回滚入口位置 | 用户气泡操作区（与重试并列） |
| P-3 | D-3 / D-4 灰度开关默认值 | 都默认 false（关=行为零变化） |
| P-4 | D-6 手动 client_id 档要不要做 | 做（MCP 配置两个可选字段，成本两行） |

---

## 十一、风险与偏离登记（实施时回填）

| # | 偏离 / 风险 | 处置 |
|---|---|---|
| 1 | D-3 激活瞬间破坏缓存前缀（tools 层起重算） | 如实登记，低频摊销；若实测 input 上涨不可接受 → 收缩到只对执行会话缩面 |
| 2 | D-7 rmcp 长等待期间连接断开语义未验证 | 实现时先写验证测试；断连按现有 Down 标记走报错重试 |
| 3 | D-4 分类器误拦好操作 | unsure 弹窗兜底 + 3 连拦退回 + 默认关灰度 |
| 4 | D-6 rmcp 重建 client 的重连语义 | 复用 retire/ensure_connected 路径；实现时验证测试 |
| 5 | （空位） | |
| 6 | D-1 saveAs 无 modify 场景（重名不覆盖、恒追加时间戳）——设计假设「覆盖已存在文件」与实现不符 | 只接 create 凭证 + 原子写；modify 快照不适用 |
| 7 | D-1 apps_update_files 不接快照凭证 | apps 已有版本快照表（app_versions 滚动 100 版）+ 回滚 UI，同一文件面再接 file_changes 回滚是双轨冗余 |
| 8 | D-1 doc 族 / archive 的 modify 场景补快照不做 | 凭证已有；modify 场景罕见（doc 产出基本 create），逐工具接线分散；逐文件回滚入口仍在 |
| 9 | D-1 前端「撤回到此」挂助手气泡操作区（设计原文写用户气泡——用户气泡无操作区基建）；三态弹层照设计落地 | 入口语义等价：锚点=该回复向上最近的用户行 |
| 10 | D-1 漂移分类按错误文本子串（is_drift_error）计数 | 分类错只影响 restored/skippedDrift 计数口径，fail 方向恒为「不回滚」；三支闸文案同文件维护 |
| 11 | 批 3 教训：ChatPanel 传给 MsgBubble 的新回调必须 useCallback 且依赖不得含每 render 变化的普通函数（addHint）——否则 MsgBubble memo 全体失效，实机表现为点击后整区重渲染异常（MessageList 头注释的硬约束，testing-lib 全量测试逮住） | addHint 走 ref 间接化；后续加气泡回调先过这条 |
