# Agent 功能全量审计报告（2026-10-21）

范围：wmessage 全部 agent 链路——模型循环、工具分发与闸门、工具体、Skill 链、
MCP 链、受管服务 / 进程 / 通知链、子 agent 编排、记忆体、执行痕迹、定时链、
问答链、前后端契约、审查链（门禁本身）、代码冗余与文档一致性。

审计对象是 `96546d71`（远程技能安装）落库后的工作区现状。基线：
`cargo nextest` **1916 例全绿**、`vitest` **973 例 / 92 文件全绿**、
`tests-audit` 对拍 26 例全绿、`cargo machete` 无未用依赖。

**结论**：基线健康，上一轮（2026-10-19）审计的发现项已全部修完且未回退。
本轮查出 **1 条 P0 + 15 条 P1**，其中 P0 是「文档与工具描述承诺的能力，
代码从不执行」的同类断裂——但方向相反：这次是**承诺了闸门却没有闸门**。

- **P0 1 条**：`create_skill` 无确认即改写**进程级全局路由表**，其 `intents`
  正则零宽度校验 → 一条 `(?s).*` 技能即可劫持此后每一条用户消息
- **P1 15 条**，五条主线：
  1. **端口闸采集失败被折叠成「判了没事」**（L1 安全边界在 lsof 失灵时静默失效）
  2. **Windows 上停止阶梯整体空转，却强制回报「已停止」**（进程活着，宿主以为已停）
  3. **工具层 60 处把拒绝/失败返回成 `Ok`** → 污染幻觉守卫的「成功凭证表」，
     **用户拒绝了删除，痕迹表仍记 `delete_task` 成功过**
  4. **编辑 MCP 服务器会静默删掉它的 keyring 机密**（`skip_serializing` 让前端
     回填不到 env）；**连接指纹对 env/headers 不敏感** → 改了机密不重连
  5. **MCP 的 HTTP 分支不做 DNS 解析** → 域名指向内网即放行（同一个应用里
     远程技能安装链**有**这道 DNS 级闸，MCP 链**没有**）
  另加：主聊天路径无 panic 隔离（子 agent 有）；子 agent 工具预算触达不落
  `BudgetExceeded`；`rollback: none` 不阻止回滚段执行；`clippy` 不在任何门禁里
  （已实证漏放一条从未运行的测试）
- **文档漂移 16 处**：架构文档散文数字错 24%、§6.4「完整列表」有 3 条幽灵条目
  与 26 条漏登记（自称「由脚本对拍」，脚本对散文与 §6.4 零校验）
- **真重复 8 处**（已漂移 3 处）、**死代码 3 处**、**跨语言无锁常量 2 处**

**本轮最能说明问题的一个形状**：`McpPanel.tsx:288-290` 那段注释
「args/env 留空会丢，因此编辑走 bot_get_config 拿完整 mcpServers」——
作者**明确知道**会丢，专门改了路径，而后端的 `skip_serializing` 正好让这条路
拿不到 env。**注释记录了意图，代码否掉了它**。这不是「没考虑到」，是
**两处设计各自正确、合起来错**——单审任一处都看不出来。

---

## 怎么审的

七个 `explore` 子代理并行，各查一个互不重叠的文件所有权面，全程只读；主会话
另做三件事：

1. **跑基线**：`cargo nextest` 全量 + `vitest` 全量 + `tests-audit` 三脚本 +
   `cargo machete` + `cargo clippy --all-targets`（clippy 是**唯一**能发现
   「测试从未运行」的信号——它报了 `dead_code`）
2. **独立复核高危条目**：本报告每条 P0 / P1 都由主会话逐条读过代码、给出行号
   与触发条件才记为事实。凡子代理标 NEEDS-VERIFICATION 或与文档口径冲突的，
   一律降级或改写（见 §四「复核时改过的地方」）
3. **跨链对拍**：前后端命令名单 / 事件名 / 类型镜像逐项比对；`TOOLS_TABLE`
   63 项的 `mutating` / `claims_patterns` 全量核对

| 角度 | 范围 | 结果 |
|---|---|---|
| A Skill 链 | `bot_skills/**` + `SKILL-*.md` | P0 1 / P1 6 / P2 8 / P3 5 |
| B MCP 链 | `bot/mcp/**` + `McpPanel` | 见 §三 |
| C 工具链 | `registry` / `dispatch` / `tools` / `hooks` / `tool_guard` | 见 §三 |
| D 服务·进程·通知链 | `bot/services.rs` / `bot/procs.rs` / `notifications.rs` | P1 3 / P2 9 / P3 4 |
| E 子 agent·记忆·痕迹链 | `bot_orchestrator` / `db/subagents` / `memory/**` / `trace*` | P1 1 / P2 6 / P3 7 |
| F 冗余·文档一致性 | 全仓重复实现 + `SPEC.md` + `docs/**` | P1 5 / P2 12 / P3 8 |
| G 前后端契约 + 审查链 | `src/**` ↔ `lib.rs` / `tests-audit/` / `scripts/` | 主会话直审 |

**五路发现的共同形状**：**契约写在文档或注释里，代码不认**。
`rollback: none` 说一套、`allowed-tools` 只在一条路径生效、`is_protected`
只管一条淘汰路径、`meta.rollback` 全文没人读、文档说「由脚本对拍」而脚本
对该段零校验——与上一轮「不变式只写在注释里」是同一个病根的另一半：
上一轮查的是「注释说的事实没被代码保证」，这一轮查的是「**文档说的事实
被代码否掉**」。

---

## 一、P0（1 条）

### P0-1 `create_skill` 无确认改写进程级全局路由表 → 提示注入 + 无 LLM 免审执行

**位置**：`src-tauri/src/bot_skills/manage.rs:597-673`（`tool_create_skill`）
· `manage.rs:653`（`rebuild_intent_routes`）· `src-tauri/src/intent_router.rs:224`
（`static ROUTES`）· `intent_router.rs:195-208`（`compile_rules`）·
`intent_router.rs:211-220`（`match_compiled`）· `src-tauri/src/bot_chat.rs:976`
（路由入口）· `bot_chat.rs:1076-1080`（正文进 system prompt）·
`src-tauri/src/bot/config/types.rs:331`（`bypass_llm_on_pre_step_hit: true` 默认）

**证据链（主会话逐段复核）**

1. **无确认**：`tool_create_skill`（`manage.rs:597-673`）全程**零**个
   `ask_user_confirm` 调用（`grep ask_user_confirm manage.rs` → 无输出）。
   对照：`install_skill`（`remote.rs:666`）有确认窗。**「从公网下载」要确认，
   「本地写一个」不要**——这个不对称不是设计意图能解释的。
2. **落盘即全局生效**：`manage.rs:653` 调 `rebuild_intent_routes(app)` →
   `intent_router::rebuild_routes` → 写入**进程级** `static ROUTES`
   （`intent_router.rs:224`）。技能目录里的 SKILL.md 变了，全局路由表就变，
   **跨会话、跨重启存活**。
3. **正则零宽度校验**：`compile_rules`（`intent_router.rs:200-204`）只做
   `Regex::new(p).ok()` —— 纯语法检查。`(?s).*` 语法合法、匹配任意输入。
   `validate_created_meta`（`manage.rs:433-451`）只查 description 非空 +
   `INTENT_BLACKLIST`（5 个中文短语：`全盘遍历`/`批量删除`/`无确认删除`/
   `遍历文件系统`/`清空所有`）——`(?s).*` 一条都不沾。
   `parse.rs:220-240` 收集 `intents` 时同样零形态校验。
4. **新技能默认启用**：`parse.rs:56` `enabled: true`（缺省即启用），
   `intent_rules_from_dirs`（`manage.rs:232`）只按 `enabled` 与 `intents` 非空过滤。
5. **命中即接管**：每条用户消息都过 `route_user_input`（`bot_chat.rs:976`），
   `match_compiled`（`intent_router.rs:214`）`re.is_match(text)` →
   `RouteAction::Skill`。`bypass_llm_on_pre_step_hit` **默认 true**
   （`types.rs:331`），路由结果被保留。
6. **正文进 system prompt**：`bot_chat.rs:1076-1080`
   `prompt.push(PromptSlot::SkillBody, active_skill)` —— 最多 50KB
   （`state.rs:11` `MAX_SKILL_BODY`）攻击者控制的文本进系统提示。
   `mode: auto` 的技能更直接：`apply_skill_route`（`bot_chat.rs:671-682`）→
   `run_skill_scheduler` → **整段 DSL 无 LLM 参与地执行**。
7. **模型可直接调**：`create_skill` 在 `TOOLS_TABLE`（`registry.rs:2141`），
   且在 general / coder / research 三个工具集里都有
   （`registry.rs:2962 / 3040 / 3104`）。
8. **提示词层无兜底**：`prompts/system.rs` 规则 26 只讲 `install_skill` /
   `search_skills`，**`create_skill` 根本没进系统提示词**；全仓无任何
   「intents 不得过宽」的规则。

**为什么这是 P0**：这不是「一个工具权限大了点」，而是**一个模型可调用的
工具能在无用户确认的情况下，永久改写进程级行为，把此后每一条用户输入
劫持进系统提示**。触发条件是模型被用户粘贴的文档 / 抓回的网页 / 任务卡标题
里的指令诱导调一次 `create_skill`。攻击面横跨提示注入与权限提升两道。

**修法建议**：① `validate_created_meta` 增加 intents 宽度闸——拒绝能匹配
空串或匹配 ≥N 个字符的模式（要求至少含一个长度 ≥2 的字面量片段）；②
`create_skill` 补 `ask_user_confirm`（与 `install_skill` 对齐），确认文案里
列出 intents 与 mode；③ 给 `create_skill` 生成的路由加「仅本会话有效」标记。

**顺带**：`skills_import`（`manage.rs:377-419`）**不跑** `validate_created_meta`
——与 `install_skill`（`remote.rs:656`）两套准入口径，用户从设置页导入一个
第三方技能文件夹即可绕过上面所有闸门（黑名单要到 `preflight` 启动时才拦，
拦的是「启动」不是「落盘」）。修 ① 时应把两条安装路径收敛到一个准入函数。

---

## 二、P1（15 条）

> P1-1 ~ P1-11 见本节；P1-12（主聊天路径无 panic 隔离）见 §3.1；
> P1-16 / P1-17（MCP 机密被删 / 指纹不认机密）与
> P1-18（MCP HTTP 分支无 DNS 级 SSRF 闸）见 §3.2。

### P1-1 端口闸的采集失败被折叠成「判了没事」

**位置**：`src-tauri/src/bot/procs.rs:570-571`（`run_lsof` 失败 → `continue`）·
`procs.rs:429-431`（Linux 侧 `if by_inode.is_empty() { return out; }`）·
`src-tauri/src/bot/services.rs:279-297`（`judge_sockets`）· `services.rs:284-286`

**证据**（主会话复核）：`run_lsof`（`procs.rs:588-608`）不可执行 / stdout 为空
时返回 `None` → `listening_sockets_lsof:570-571` `continue` → 返回**空 map**；
Linux 侧 `/proc/net/*` 读不到同样返回空 map。`judge_sockets(&[], …)` 的
`for` 循环不执行 → 直接落到 `GateVerdict::Ok`。而 `Unsupported` **只**在
`!listening_supported()`（编译期 `cfg!` 平台判定）时返回。

**为什么坏**：设计稿写死「`Unsupported` 的语义是『没判』，**不是**『判了没事』」，
但 `Unsupported` 只覆盖「平台没有采集路径」（Windows），**没覆盖「采集路径存在
但本次采空」**。「服务没监听端口」与「采集器坏了 / 权限不足」在当前数据流里是
**同一个空集合**，类型层就无法区分。后果：L1 这条被称为「真安全边界」的规则
在 lsof 失灵时**完全静默失效**——没有审计、没有通知、面板无变化。连带
`services.rs:617-621` 用宿主自身监听端口刷新 L2 动态保留集，采样器坏掉时
`host_ports` 也是空 → 动态保留集被**清空**，L2 一并静默失效。

**触发条件**：macOS 上 lsof 两个候选路径都不可用 / 系统文件表耗尽；Linux 上
`/proc/net/tcp*` 不可读（受限容器、`hidepid`）。

**修法**：采集层返回显式的「采样是否成功」信号，把「采空但采成功」与「采集失败」
分开，后者降级为 `Unsupported` 并写审计。最小可行版本是拿宿主自身已知在监听
的端口做**金丝雀**——`enforce_listening_gate` 已在采 `self_pid` 的 socket，
宿主明明在监听而采样结果为空，即判定采集器失灵。

### P1-2 Windows 上停止阶梯整体空转，却强制回报「已停止」

**位置**：`src-tauri/src/bot/procs.rs:681-700`（`send_signal` / `signal_group`
的 `#[cfg(not(unix))]` 分支）· `procs.rs:721-724`（`pid_exists` 恒 false）·
`src-tauri/src/bot/services.rs:1397-1401`（强写 `Exited`）· `services.rs:1419-1425`
（`stopped: true`）

**证据**（主会话复核）：非 unix 下 `send_signal` / `signal_group` 均 `false`，
`pid_exists` 恒 `false` → `wait_pid_exit` 立刻返回 true。但 `stop_entry`
（`services.rs:1397-1401`）无条件把 `g.state = ServiceState::Exited`，
`StopOutcome`（`:1419-1425`）无条件 `stopped: true`。

**为什么坏**：Windows 上 `stop_service` / 端口闸杀组 / 退出清理三条路径全部
只发了个寂寞，接口却返回「已停止」、登记表写「已退出」。服务进程继续跑、
继续占端口、继续烧 CPU，而宿主已把它从活动集合摘掉（`services.rs:1717` 按
`state.is_active()` 过滤）→ **退出时再也不会收它**。讽刺的是
`procs.rs:870-873` 的注释写着「宁可赌一次也不能让服务活着跑（否则 stop 报
『已停』而进程还在）」——**这句注释描述的场景在 Windows 上必然发生**，
而赌的那一次（`signal_group`）在 Windows 上是 no-op。

**修法**：给 `stop_tree` 一个显式的「本平台不支持发信号」出口，
`stop_entry` 据此**不**强写 `Exited`、**不**返回 `stopped: true`，而是返回
一条明确错误；Windows 的真正停止路径按设计稿自己写的那条实现。

### P1-3 服务层崩溃 / 强杀退出路径收不掉服务（无父进程看门狗）

**位置**：`src-tauri/src/bot/services.rs:1048-1053`（spawn 无看门狗）·
`src-tauri/src/lib.rs:767-778`（唯一清理入口是 `RunEvent::ExitRequested`）

**证据**：服务层 spawn 只有 `cmd.process_group(0)`，没有父进程死亡探测。
对照**沙箱层**：`py/runtime.rs:322` 有 `PARENT_WATCHDOG`（ppid ≤ 1 即自退），
`py/runtime.rs:511-514` 注入脚本——这是上一轮审计 G4 的修复手段。服务层没有。
`audit.rs:662-665` `install_panic_hook` 只记日志，不触发退出清理。

**为什么坏**：`process_group(0)` 让服务自成进程组、无控制终端、SIGHUP 不达。
宿主一旦非正常死亡（主线程 panic、OOM、SIGKILL、机器重启），受管服务全部存活
并继续监听端口——正是设计稿「服务层不能接受这条 trade-off：逃逸的 node 子进程
= 宿主管不到的孤儿」要防的那个孤儿。沙箱层已堵，服务层没堵，**两层口径不一**。

**修法**：服务层复用沙箱层看门狗思路（Unix `pre_exec` 里
`prctl(PR_SET_PDEATHSIG, SIGTERM)`），并在 `lib.rs` 加 `RunEvent::Exit` 兜底。

### P1-4 工具层 60 处把「拒绝 / 失败」返回成 `Ok` → 污染幻觉守卫的成功凭证表

**位置**：`src-tauri/src/bot/tools.rs` 等 11 个文件 · `src-tauri/src/bot/registry.rs:55-70`
（`ToolStatus::is_success`）· `src-tauri/src/bot/dispatch.rs:641`
（span 的 `ok` 来源）· `src-tauri/src/db/trace.rs:537-567`（`session_tool_names`）

**证据**（主会话用脚本量化 + 逐条复核）：全仓 **60 处** `ToolResult::ok(...)`
携带拒绝/失败语义（`ToolResult::ok` 字面量调用共 81 处）。按
`audit.rs:127-150` 的文本分类器口径分成两类：

| 类 | 数量 | 文本分类器判 | status 判 | 后果 |
|---|---|---|---|---|
| A | 12 | **失败** | 成功 | **同一个结果两个答案** |
| B | 48 | 成功 | 成功 | **静默假成功** |

A 类 12 处（含 `format!` 模板）：`tools.rs:607`「用户拒绝了删除，任务未删除」
（`UserDenied`）· `tools.rs:1147`「路径校验失败，拒绝绑定：{path}」
（`Keyword`）· `bot_manage_file.rs:252`「用户拒绝了删除，文件未删除」·
`bot_skills/remote.rs:675`「用户拒绝了安装，技能未安装」· `bot_ui.rs:290/463/559`
「激活窗口失败 / 点击失败 / 输入失败」· `bot_manage_file.rs:147/281`「{action} 失败」。

B 类 48 处：「缺少 X 参数」「未找到 id=X 的任务卡」「不能为空」——
`tools.rs` 15 处、`bot_fs.rs` 12 处、`bot_desktop.rs` 8 处、`bot_manage_file.rs` 2 处、
`bot_skills/runtime.rs` 1 处、`bot_skills/remote.rs` 6 处（含**远程技能安装的
全部失败路径**：下载失败 / 写临时文件失败 / 创建解包目录失败 / 解包失败 /
拒绝安装 / 搜索失败）。

**为什么坏**——这条是本轮**最值得注意的跨链断裂**：

- `audit.rs:169-172` 的注释白纸黑字写着「内置工具的成败判定走
  `registry::ToolStatus`（见 `dispatch::span_ok`），不要在这里给内置工具的成败
  兜底——两套口径同时判，早晚会给同一个结果两种答案：之前就发生过——5 处
  闸拦/拒绝/预算触顶的文案既不带「⚠️」也不带「失败：」，审计记 WARN 而
  Skill DSL 调度器把被拒步骤记成成功」。
  **那 5 处修了，但这 60 处是同一类问题的更大一片——注释写的是「之前发生过」，
  现在仍在发生。**
- `dispatch.rs:641` `let ok = result.status.is_success();` → trace span 的 `ok`
- `db/trace.rs:557-566` 按 `ok` 把工具名放进 `SessionTools.succeeded`
- `bot_model_loop.rs:2810-2829` 点名判据的判决口径是「**没成功过**」
- → **用户拒绝了删除，痕迹表里 `delete_task` 仍记为「成功过」**。
  幻觉守卫的证据源被污染：模型说「已删除任务卡」，守卫查凭证说「有，
  `delete_task` 成功过」——守卫不但不拦，还替它背书。
- 同一条对 Skill DSL 调度器（`bot_skills/scheduler.rs:499`
  `let failed = !tool_outcome.status.is_success();`）：一步参数写错的技能会被
  记为**成功**，最终汇总输出「✅ 技能「x」自动执行完成（N 步）」而实际零产物。
  `SKILL-DSL.md:173` 要求前端在失败未回滚时提示「已完成步骤未回滚，请人工核对」
  ——这条护栏永远不会触发。

**触发条件**：任何工具被调用时缺必填参数、目标不存在、或用户拒绝确认。

**修法**：① 工具层把「缺必填参数 / 目标不存在 / 用户拒绝」三类改成
`ToolResult::error` / `ToolResult::denied`（影响模型侧文案，需评估）；
② 或在调度器与 trace 采集处对 `Ok` 状态的文本做一次失败特征判定
（`audit::classify_tool_failure` 已有同类口径可复用，但它对「缺少 / 未找到」
不命中，需要补关键词）；③ 加一条守卫测试：扫描 `bot/**/*.rs`，禁止
`ToolResult::ok(` 携带拒绝语义文本——这类问题正是**单测抓不到、只有静态
扫描能抓**的，与上一轮审计的「源码扫描守卫」思路一致。

### P1-5 子 agent 的 `max_tool_calls` 触达不落 `BudgetExceeded`

**位置**：`src-tauri/src/bot_orchestrator.rs:790-818`（`classify_outcome`）·
`src-tauri/src/bot_model_loop.rs:653/658`（两个熔断常量）·
`src-tauri/src/bot/dispatch.rs:471-505`（工具预算强制）

**证据**（主会话复核）：`classify_outcome` 三条终态路径——`wall_exceeded` →
`BudgetExceeded`；`text.contains(ROUNDS_FUSE_MARKER)` → `BudgetExceeded`；
其余走 JSON 解析。工具调用熔断的正文用的是**另一个常量**
`FUSE_MARKER = "⏹ 已熔断"`（`bot_model_loop.rs:653`），而轮数熔断用
`ROUNDS_FUSE_MARKER = "⏹ 已熔断（轮数上限）"`（`:658`）。工具熔断正文
（`:805`）**不含** `ROUNDS_FUSE_MARKER` → 落不进 `BudgetExceeded` 分支。

**为什么坏**：触顶后落 `Succeeded`（模型照办并输出 `{"status":"succeeded"}`）
或 `Failed`（无收尾 JSON → `result_json_unparseable`，**原因字段指向「解析失败」
而真实原因是预算触顶**）。直接违反设计文档
`SUBAGENT-ORCHESTRATION-DESIGN-2026-09-27.md:139`「预算触达 → 状态
`budget_exceeded`」与 `:356`「预算三顶生效，超限 budget_exceeded」。
后果链：`prompts/subagent.rs:45` 教父 agent「failed / budget_exceeded /
cancelled：先读 error 与 summary，**决定重试**」——既然烧满预算的子 agent
报的是 `succeeded`，父 agent **不会重试**，产出实际残缺而无人知晓。

**修法**：收尾处读一次 `ctx.used_tool_calls`（收尾前守卫仍注册），与
`budget.max_tool_calls` 比较，达顶则强制 `status = BudgetExceeded`、
`error = "max_tool_calls"`；并让 `classify_outcome` 也认工具熔断标记。

### P1-6 `rollback: none` 不阻止回滚段执行

**位置**：`src-tauri/src/bot_skills/scheduler.rs:296`（`parse_skill_steps` 拆出
`rollback` 段）· `scheduler.rs:501-511`（失败无条件跑回滚）·
`scheduler.rs:130`（`rollback.is_empty()` 早退）· `src-tauri/src/bot_skills/parse.rs:205-212`

**证据**（主会话复核）：`scheduler.rs` 全文**零**处读 `meta.rollback`。
`meta.rollback` 只在 `runtime.rs:578` 被读（`if rollback == "auto" && …`）。
回滚段是否存在**只看正文有没有 `## Rollback` 段**。

**为什么坏**：`SKILL-DSL.md:153` 定义「`rollback_attempted = false` = rollback 段
根本没写（`rollback: none`）/ 跑挂 / 在跑之前就 panic 了」——明确把
`rollback: none` 与「不执行回滚」绑定。实现里 `rollback: none`（**默认值**）
只要正文留有 `## Rollback` 段（复制模板时最常见）**就会执行作者明确声明
不要执行的逆操作**（`delete_task` 等），可能删掉作者想保留的数据。
同一模块内两处对 `rollback` 的理解还互相矛盾——调度器照跑，`skill_finish`
却对 LLM 说「不提供回滚章节」。

**修法**：三处 `run_rollback_segment_core` 调用前加 `meta.rollback == "auto"`
判定；`parse_meta` 里做「`auto` 但无 `## Rollback` 段 → 降为 `none`」
（`SKILL-DSL.md:133` 已承诺这个降级，解析器没做）。

### P1-7 `clippy` 不在任何门禁里 → 已实证漏放一条从未运行的测试

**位置**：`AGENTS.md:19` 与 `SPEC.md:155` 声称 clippy 是 Rust lint 门禁 ·
`scripts/test-fast.sh` / `scripts/test-all.sh` / `.githooks/*` / `docs/testing.md`
**零出现 clippy**

**证据**（主会话复核）：全仓 grep `clippy` 只命中 `AGENTS.md:19` 与
`SPEC.md:155`。`AGENTS.md:22` 的节标题是「硬性禁令（**门禁会拦**，人也退）」，
第一条即「不留死代码」——但 Rust 侧**没有任何自动拦截**。

**已实证的后果**：`src-tauri/src/bot/mcp/manager.rs:1116` 的
`async fn missing_binary_marks_down_and_lazy_reconnects()` **没有
`#[tokio::test]`**（同文件 `:861/:950/:992/:1044/:1136/:1153` 六个兄弟测试
全都有）。它位于 `#[cfg(test)] mod e2e_stdio`（`:818` 起），被 clippy 的
`dead_code` 判死——`cargo clippy --all-targets` 唯一的 lib 内 `dead_code` 告警
就是它。`cargo nextest list | grep missing_binary` → **0 行**，从未注册。
它覆盖的「MCP stdio 指向不存在的脚本 → 置 Down + 错误可查 + 懒重连自动恢复」
是 MCP 连接可用性的核心行为。**这正是上一轮审计 L2（「一个回归测试被静默禁用
（缺 `#[test]`），从未被收集运行」）的复发**——上轮修过一处，这一轮又长出一处，
且**门禁没有能力拦住复发**。

**修法**：① 补回 `#[tokio::test]`；② 把
`cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`
接进 `scripts/test-fast.sh` 的 cargo 链路；③ 在 `docs/testing.md` 门禁表补一行。

### P1-8 架构文档 §6.4「完整列表」有 3 条幽灵条目 + 26 条漏登记，脚本看不见

**位置**：`docs/rust-bot-architecture.md:554/556` · `tests-audit/audit_module_map.py`

**证据**（子代理查 + 主会话复核脚本口径）：§6.4 有 132 条 `- \`path\`` bullet，
实际源文件 **155** 个 → 漏 26 个、幽灵 3 个（`eval/metrics.rs`、`eval/mod.rs`、
`prompts/reflection.rs` —— `src-tauri/src/eval/` 整个目录不存在）。
脚本 6 个用例**当前全绿**，却零反应，因为它只解析 §1 模块树的 `── x.rs` 格式
（`TREE_ENTRY_RE`），**没有任何用例解析 §6.4 的 `- \`path\`` 格式**；
而 `test_every_source_file_is_mentioned` 的判据是「整篇文档任意位置出现
该字符串」——26 个漏登记文件在正文散文里被提到，照样过审。

**为什么坏**：文档自称「由 `audit_module_map.py` 对拍」是**过度声称**——
脚本对拍的只是 §1 树，不是 §6.4 清单。读者把 §6.4 当导航会点到不存在的文件；
新增模块时按 §6.4 口径「同步登记」也做不到（清单本身已残缺 23 条）。

**修法**：`audit_module_map.py` 增设 §6.4 格式用例（每条能解析到真实文件 +
每个源文件都被 §6.4 列出），并把 `test_every_source_file_is_mentioned` 的判据
从「全文提到」收紧到「§6.4 列出」。

### P1-9 架构文档散文数字错 24%，且写下即错

**位置**：`docs/rust-bot-architecture.md:3`（166 个文件 / 约 8.9 万行）·
`:556`（实际 166 个 `.rs` 文件）

**证据**：实测 **155** 个 `.rs` / **110,621** 行 → 文件 −11（−6.6%）、
行数 **+21,621（+24.3%）**。溯源：写下这句的 commit `d2674e33`（2026-10-21）
当时是 **150 文件 / 103,189 行**——**写下时就是错的**，不是后续漂移。
另「全部在 `src-tauri/src/`」也不准（`src-tauri/tests/` 10 个、
`examples/` 1 个也在该 crate 内）。

**为什么坏**：架构文档的入口第一屏。文档别处用「≤1200 行」当拆分阈值、
用总行数估算评审范围——按 8.9 万行会系统性偏低 2.4 万行。
`audit_module_map.py` 的 22 处行数标注**全部准确**，唯独这两句散文零校验。

**修法**：把散文数字改为脚本生成（加一条用例：正文出现的「N 个文件 /
M 万行」必须等于实测值）。

### P1-10 SPEC 的「冻结附录 A」仍在被持续修改，且承载着唯一的机制详录

**位置**：`SPEC.md:3-5` / `:21` / 附录 A 头（`:159` 起）· `SPEC.md:111` · `:248`

**证据**：`SPEC.md:5`「原文保留于文末附录 A，**不再修改**」、`:21`「附录 A
（v1 快照）**冻结不动**」、附录头再声明一次。但 `git log -L 248,248:SPEC.md`
显示附录内的 `:248` 最近一次被 **`96546d71`（2026-10-21 远程技能安装）**改
（工具数 61→63 + install_skill/search_skills 段）；`8c0a2ab1`/`b95f8b8b`/
`8a71cbe9`/`c2554a62` 同类——**至少 5 次违反自订口径**。

**为什么坏**（结构问题，不只是措辞）：`SPEC.md:111`「工具（63 个，清单见
「内置机器人聊天」一节）」——**现行节不含工具清单与机制细节**，读者按
`SPEC.md:3`「现行形态以本文件前半部分为准」去找 `install_skill` / 端口闸 /
资源告警会找不到；唯一有这些内容的地方是被声明为「冻结、原文保留、
停止更新」的附录 A。文档对外宣称的结构（现行节=真相，附录=考古）与内容
实际分布**正好相反**。

**修法**：二选一并改口——(a) 承认附录 A 实际是「机制详录」的活节，把三处
「冻结」措辞改为「现行节给概览、本节给逐项细则，随功能更新」并在标题标注
最后更新日期；或 (b) 把 `:248` 的工具清单与最新机制条目迁回现行节，
附录只留 2026-08 原始快照。

### P1-11 前端文件绑定两份实现，一份修过并发一份没修

**位置**：`src/components/TodoCard/TodoCard.tsx:134-136,139-190` ·
`src/components/TaskTimeline/TaskDetailPanel.tsx:220,222-270`

**证据**（子代理查 + 主会话复核）：`TodoCard` 有 `taskRef`（`:134-136`），
合并基线取 `taskFiles(taskRef.current)`（`:155`/`:174`），注释明写
「await 期间可能有并发绑定：以最新 files 为合并基线，别丢掉那批改动」。
`TaskDetailPanel` 用渲染期快照 `boundFiles = taskFiles(task)`（`:220`），
**无 taskRef**，而它的注释自称「**与 TodoCard 同通道**」。

**为什么坏**：同一业务规则两份实现且**已经漂移**。用户在 TaskDetailPanel
打开的同时另一路径（TodoCard / 上下文栏）绑定文件 → `await invoke("bind_files")`
返回后用陈旧基线 `mergeFiles` → 后到的那批被整列覆盖丢失。注释自称同通道
会强化读者「两边等价」的错觉。

**修法**：把「绑定 + 合并 + 截断提示」抽成一个共享 hook（内部持有 `taskRef`），
两处调用方零改动；顺带把 5 份 max-files 文案收进 `src/lib/taskFiles.ts`。

---

## 三、MCP 链 与 工具链

这两条链各有专属子审计在跑，**截至本文成稿仍在进行**。主会话已独立核过的
关键处结论如下（可独立成立，不依赖子审计）；子审计完成后应回补其余条目。

### 3.1 工具链

**单源真相的结构层面：全绿。** `SCHEMA_*` 常量 63 ↔ `TOOLS_TABLE` 63 ↔
`call_*` 适配器 63 严格 1:1；schema 内 `"name"` 与表项 `name` 63/63 一致；
**schema 声明的参数名与实现实际读取的参数名，闭包追踪全绿**（初筛出的
12 处「未读参数」经追查全是共享 helper 读取的假阳性——`parse_limit` /
`opt_filename` / `sanitize_image_paths_arg` / `collect_clip_paths` /
`resolve_task` / `resolve_task_key`）；`required` 字段与实现的必填校验逐项一致。

**闸门顺序（`bot/dispatch.rs:392` 起，主会话逐段核对）**

```
0.5  L4 pre_tool 钩子层      ← 唯一接线点，三道闸的入口
     per-tool 规则表          deny 硬拒 / ask 强制确认 / allow 到文件工具侧
     Skill 调度器步骤钩子      计数 / 熔断 / 动作记录
1.6  子 agent 会话白名单闸    白名单外一律拒（递归禁用 spawn 的服务端侧）
     预算强制                 max_tool_calls 原子 check-and-increment
     参数 JSON 闸
     → 工具执行 → L5 post_tool 钩子 → 审计
```

`dispatch.rs:1281-1325` 有源码文本守卫钉死这个顺序（含「返回值必须被消费」
与「早退必须紧跟」两条），7 处门禁早退全部 `ToolResult::denied`（`:1226-1240`
静态守卫禁止 `warn`），每条早退都配平 `tool.return` 事件。**没有发现绕过
某道闸的路径**。`use_skill` 在规则表与 skill 钩子两处显式豁免（豁免语义有
注释说明，不是漏网；子 agent 白名单闸与参数闸仍跑，无权限绕过）。

**本链的新发现**（子审计挖出、主会话独立复核成立）：

#### P1-12 主聊天路径没有 panic 隔离（子 agent 有）

**位置**：`src-tauri/src/bot/dispatch.rs:561`（`(t.call)(&ctx, args).await` 裸调用）
· `src-tauri/src/bot_model_loop.rs`（主聊天链路**零** `catch_unwind`）

**证据**（主会话复核）：`grep catch_unwind` 全仓 21 处，主聊天链路
`bot_chat.rs` → `bot_model_loop.rs` **零命中**。对照子 agent runner
（`bot_orchestrator.rs:2186`）：
```rust
std::panic::AssertUnwindSafe(run).catch_unwind(),
```
并有源码扫描守卫钉住「runner future 必须包 catch_unwind」。

**为什么坏**：工具 panic → future unwind → `bot_chat.rs:1216` 的
`run_model_loop` 直接崩 → **用户这一轮对话整轮丢失**（无部分输出、无错误提示），
且 `bot_model_loop.rs:3580-3588` 的 `turn_status` 收口被跳过，前端停在
「正在思考」。**同一份工具代码，两种健壮性**。讽刺的是 hooks 扩展点
（`hooks.rs:266/288/312`）三层**都**做了 fail-open——扩展点比被扩展的本体
更防护。

**修法**：`dispatch.rs:561` 改为
`AssertUnwindSafe((t.call)(&ctx, args)).catch_unwind().await`，panic 归一成
`ToolResult::error` 回灌模型（与子 agent 同范式）。

#### P2-1 审计阈值与实际回灌 cap 不是同一个数——而注释明说必须是同一个数

**位置**：`src-tauri/src/bot/dispatch.rs:602-618`（审计）·
`src-tauri/src/bot_model_loop.rs:562-568`（实际截断）

**证据**（主会话复核）：
```rust
// dispatch.rs:602-604
// 口径必须与回灌漏斗一致（字节）：审计阈值和真实 cap 是同一个数，否则会出现
// 「审计报了超预算、实际没截」或反之，两边各说各话。
if output_bytes > t.max_output_bytes { ... }   // ← per-tool 预算
```
```rust
// bot_model_loop.rs:562-568
.map(|t| t.max_output_bytes).unwrap_or(global_cap_bytes).min(global_cap_bytes)
```
`read_text_file` / `fetch_url` / `list_files` 的 per-tool = **65536**，
全局默认 `DEFAULT_TOOL_OUTPUT_CAP_BYTES` = **30,000**（`bot_model_loop.rs:551`）。

**为什么坏**：默认配置下真实 cap = min(65536, 30000) = 30000，而审计阈值
仍是 65536。**输出 40,000 字节时：实际被截断转存，审计却不报
`tool.output.over_budget`**。运维据此查超预算会漏掉全部由全局 cap 触发的截断。
`dispatch.rs:602` 的注释正好描述了它要防的分裂，**但没防住**——注释与实现
背离，方向与本轮其它文档漂移相反（代码比注释更宽松）。

**修法**：把审计事件移到 `bot_model_loop.rs` 的截断判定处（与
`effective_output_cap` 同源），或把全局 cap 传进 dispatch。

#### P2-2 溢出指针承诺「完整内容」，但超过 100KB 的文件尾部读不回来

**位置**：`src-tauri/src/overflow.rs:44-56`（指针文案）·
`src-tauri/src/bot_fs.rs:19`（`READ_MAX_BYTES = 100 * 1024`）·
`bot_fs.rs:1552`（`read_with_ino_check(..., READ_MAX_BYTES)`）

**证据**（主会话复核）：指针文案写「已**完整**转存到本地文件」+
「读法：read_text_file 配 offset（行号，从 1 起）+ limit（每页行数）分页读」。
而 `read_text_file` 走 `read_with_ino_check(&canonical, pre_ino, READ_MAX_BYTES)`
——**只读前 100KB**，offset 是在这 100KB 的行上取的。

**为什么坏**：一次 `run_python` 打出 200KB → 转存 200KB → 指针承诺「完整内容
在那儿，分页读」→ 模型读完前 100KB 后再 offset 读后半段时，
`bot_fs.rs:1590-1594` 判 `offset > total` 返回「offset 超出范围」——
**后半段永远读不到**。指针的承诺与实际可达性不符，模型会以为文件被截断了。

**修法**：指针文案补「单次读取上限 100KB」；或让 `read_text_file` 支持从
offset 起读的字节窗（当前是「先读前 100KB 再按行切」）。

#### P2-3 `screenshot` 实际写盘却标 `mutating: false`

**位置**：`src-tauri/src/bot/registry.rs:2220-2226`（`mutating: false`，
`claims_patterns: &[]`）· `src-tauri/src/bot_desktop.rs:396-404`（落盘）

**证据**（主会话复核）：`screenshot` 把 PNG 写到 `gen_dir`（AI_Gen_Files），
返回值是「已截屏：{path}…」，但 `mutating: false` + `claims_patterns` 为空。

**为什么坏**：变更声称闸只扫 `mutating==true` 工具的 patterns → 「已截屏/
已保存截图」不被拦。与 `copy_file_to_clipboard` 的「剪贴板不是文件变更」
论证**不同**——截图是真实落盘的文件产物，与 `create_word` 同性质。
更具体的连带后果：`screenshot` 不发 `file_changes` 凭证 → 模型若说
「已生成截图 /path/x.png」，落盘凭证闸查不到凭证 → **守卫误判为编造**，
反过来要求模型「重新生成这个文件」。

**修法**：`mutating: true` + `claims_patterns: &["已截屏", "已保存截图"]`；
并让 `screenshot` 发 `file_changes` 凭证（与 `create_word` 同路径）。

#### 工具链已核无问题

- `tools_index()` 有 HashMap 长度一致性守卫；`builtin_tool_count_matches_table`
  守卫存在。
- 意图路由：上次审计修的 ASCII 触发词回归（`download the report`/`cargo run`/
  `google 一下`/`running tests`/`domain model`）**全部有负例测试钉住**
  （`intent_router.rs:591-606`）；引用块只认消息头、正文伪引用行不收，均有测试。
- 参数表：`PARAMS_TABLE` 22 项的 min/max 与 resolver 实际钳制**逐项核对一致**
  （`loop.max_rounds` 5..200、`loop.max_function_calls` 1..500、
  `subagent.max_turns` 1..50、`search.max_results` 1..10、
  `tools.max_output_bytes` 0..200K、`tools.ask_budget` 1..5）；写侧
  `clamp_agent_params` 与读侧共用同一组 `clamp_*`；默认值由 resolver 在空配置下
  派生，非手写字面量。
- **设置页「不再自留一份硬编码数字」：真做到了。** 前端 `paramNumber` 从后端
  `bot_effective_params` 的 `paramViews` 取 min/max，页面无独立数字副本。
- 溢出字节口径：`byte_head_end`/`byte_tail_start` 严格不切破 UTF-8
  （`cap_tool_output_never_splits_a_multibyte_char` 对 cap=1..150 全遍历）；
  溢出目录恒在读白名单；转存路径由程序生成（UTC 日期目录 + uuid），不接受
  外部输入拼路径。
- 工具失败回灌模型：结果文本作 `role: tool` 回灌，失败时带 `【系统提示】`
  换策略引导，连续失败 2 次触发 Replan——**回灌机制本身健全，问题在 status
  语义（P1-4）**。
- 工具链 6 个文件零 `unwrap()`/`expect()` 逃逸（全部在 `#[cfg(test)]` 内）、
  零 `as any`、零空 catch。



### 3.2 MCP 链

**命名与撞名**：`mcp_tool_name`（`bot/mcp/config.rs:443`）→
`mcp_{前缀}_{工具}`，两者都净化成 ASCII、工具名小写、前缀最长 24 字节。
**已知边界被显式记录**（`config.rs:440-442`）：不同服务器净化后前缀相同
（「my tool」与「my.tool」）且工具同名时会产出同名 function——**由挂载层在
构建 schema 时全列表查重、追加序号消歧**（不在纯函数内处理）。这属于
「已知边界 + 有归属方处理」，不是缺陷。

**schema 体积**（`bot/mcp/mount.rs:41-43`）：`MAX_SCHEMA_BYTES = 8KB`。
超限时**降级参数 schema 为宽松空对象、保留工具**（`mount.rs:74-80`），
描述里标注降级。**不丢工具**——模型仍看得见、能调，服务器侧自己校验。
描述截断 512 字符（`:57`）。这个取舍是对的：丢工具会让模型看不到能力，
降级参数只是把校验责任推给服务器。

**反查**：`execute_mcp_tool`（`mount.rs:154-160`）先 `find_mounted(name)`
精确匹配挂载表，miss 直接 `ToolResult::error("未知工具：{name}")`——
编造的 `mcp_*` 名在路由内仍回「未知工具」。**不会误命中内置工具**。

**SSRF 闸 —— ⚠️ 见 P1-18，**MCP 自己的 HTTP 分支**没有用 DNS 级闸**，
只有字面量级检查。远程技能安装链（`bot_skills/remote.rs:242`）才复用了
`bot_web::check_public_url`（DNS 解析 + 逐 IP 公网校验 + `resolve_to_addrs`
钉 IP，TOCTOU 已堵）。**两者是不同的闸**——初稿把这条写反了，见 P1-18。


**本链已确认的问题**（主会话逐条复核成立）：

#### P1-16 编辑 MCP 服务器会静默删掉它的 keyring 机密（env / headers）

**位置**：`src-tauri/src/bot/mcp/config.rs:102`（`env` 的 `skip_serializing`）·
`src-tauri/src/bot/config/types.rs:528`（`BotConfigView.mcp_servers`）·
`src/components/SettingsPage/McpPanel.tsx:284-302`（`startEdit` 回填）·
`src-tauri/src/bot/mcp/secrets.rs:364-368`（空机密 = 删条目）·
`src-tauri/src/bot/mcp/commands.rs`（`mcp_server_save` 调 `store_server_secrets`）

**证据链**（主会话逐段复核）：

1. `McpServerConfig.env` / `headers` 带 `#[serde(skip_serializing)]`
   （`config.rs:102`），注释写明「值视为机密——`skip_serializing` 使任何写路径
   都写不回盘」。**读侧（反序列化）仍认老配置，但序列化一律不带这两个字段。**
2. `BotConfigView.mcp_servers: Vec<McpServerConfig>`（`types.rs:528`）——
   `bot_get_config` 返回的就是它，序列化时 env/headers **被剥掉**。
3. 前端 `startEdit`（`McpPanel.tsx:292-293`）拿 `getBotConfig()` →
   `full.env` → `undefined` → `envText` 初始化为**空串**。
4. 保存时 `buildPayload` 发 `env: {}` / `headers: {}`。
5. `mcp_server_save` → `store_server_secrets(app, id, &{}, &{})` →
   `secrets.is_empty()` → **`delete_backend(app, id)`**（`secrets.rs:364-368`，
   注释「空机密 = 清除条目」）→ **keyring 条目 `mcp:<id>` 被删**。

**为什么坏**：用户在设置页改一个 MCP 服务器的名字或超时（任何字段），
**它存着的环境变量 / 鉴权头就被静默删除**——用户看到的是一个**空白的
env 输入框**，不知道那下面原本有值。删除后重建连接时 `load_config` 水合不到
机密 → stdio 子进程**不带原环境变量启动**，HTTP 服务器**不带原 Authorization
头** → 401 / 连接失败。

**最能说明问题的是那段注释**（`McpPanel.tsx:288-290`）：
> 「表单回填需要原始配置字段——status 视图没有 args/env，…args/env 留空会丢：
> 因此编辑走 bot_get_config 拿完整 mcpServers」

作者**明确知道**「留空会丢」，专门为此改走了 `bot_get_config` 这条路——
而后端的 `skip_serializing` 正好让这条路拿不到 env。**注释记录了意图，
代码否掉了它**。这不是「没考虑到」，是**两处设计各自正确、合起来错**。

**修法**：① `mcp_server_save` 在「编辑且请求未带 env/headers」时**保留**原
机密（区分「用户清空」与「前端没回填」——前者用显式的 `clearSecrets` 标记）；
或 ② 后端在 `BotConfigView` 里返回 `hasEnv` / `hasHeaders` 布尔标志（不返回
明文），前端据此在未改动时不发空对象；或 ③ 前端编辑时**保留原值**，
只在用户真的动过该字段时才改写。

**连带**：同一条路径还有一个变体——**保存失败也会改写机密**。
`mcp_server_save` 里 `store_server_secrets` 在 `upsert_in_config`（校验）
**之前**执行；校验失败时配置文件不写，但对**已存在的**服务器**不**回滚
keyring → 「保存报错，但机密已被清空/替换」。

#### P1-17 连接指纹对 env / headers 不敏感 → 改了机密不重连

**位置**：`src-tauri/src/bot/mcp/manager.rs:168-173`（`fingerprint`）

**证据**（主会话复核）：
```rust
/// 配置指纹：任何影响连接/行为的字段变化都会换指纹（id 变化 = 换条目，天然换槽）。
pub fn fingerprint(cfg: &McpServerConfig) -> u64 {
    let value = serde_json::to_value(cfg).unwrap_or(serde_json::Value::Null);
    let mut h = DefaultHasher::new();
    value.to_string().hash(&mut h);
    h.finish()
}
```
`serde_json::to_value(cfg)` 走的是**同一个 `Serialize` 实现**——而 env/headers
是 `skip_serializing` → **不在指纹里**。

**为什么坏**：注释说「任何影响连接/行为的字段变化都会换指纹」，而 env/headers
**恰恰是影响连接行为的字段**，却**不换指纹**。于是：用户改了 MCP 服务器的
API token / 环境变量 → `mcp_server_save` 写盘成功 → `trigger_reload` →
`ensure_connected(cfg)` → 指纹与旧连接**相同** → **直接返回，保留旧连接**
→ 旧子进程继续带着**旧的环境变量**跑，HTTP 继续发**旧的 Authorization 头**。
**改动看起来生效了（配置已保存、面板已刷新），实际行为没变**，直到用户改了
别的字段（args/name/timeout）或重启应用。

**修法**：指纹计算不能用 `to_value`——应显式枚举「影响连接/行为的字段」
（`transport` / `command` / `args` / `url` / `env` / `headers` / `timeout_secs`）
构造哈希，或对 env/headers 单独算一个摘要拼进去。
（`timeout_secs` 是 `skip_serializing_if = "Option::is_none"`，Some 时会进
指纹——所以改超时**会**重连，改机密**不会**，这个不对称正好印证了问题。）

#### P2-4 MCP 面板没有「本平台能不能采集」的能力标志

**位置**：`src-tauri/src/bot/mcp/commands.rs`（`mcp_status` 返回结构）·
`src/components/SettingsPage/McpPanel.tsx`

**证据**：受管服务层有 `services.rs:1936` 的 `listeningSupported` 标志 →
`ServicesPanel.tsx:159-163` 明说「本平台暂不支持监听端口采集」。**MCP 侧的
`McpServerStatus` 没有对应字段**。

**为什么坏**：Windows 上 `bot/procs.rs` 的进程快照层本就退化（依赖 `ps`），
`listening_sockets` 返回空 map、`tree_sample` 返回 `None` → MCP 面板显示
「无监听端口 / 无资源占用」——**这是「没判」被显示成「判了没事」**。
`bot/procs.rs:352-357` 的模块注释自己写着这个原则：「判定层据此返回
Unsupported（没判）而不是 Ok（判了没事）——**安全闸在某个平台静默不生效
是缺陷，不是功能缺失**」。服务层守住了，MCP 层没守住。

**修法**：`McpServerStatus` 加 `listeningSupported` / `resourceSupported`
布尔字段，前端在 false 时显示「本平台不支持采集」而非空值。

#### P2-5 连接重建期间工具集瞬时消失，同前缀服务器会被错路由

**位置**：`src-tauri/src/bot/mcp/manager.rs`（`ensure_connected`：先
`slots.remove(&cfg.id)`，再 `close_old(old).await`，再 `spawn_service`）

**证据**：`close_old` 里的 `stop_stdio_tree` 含 SIGINT→100ms→SIGTERM→400ms
的等待，`spawn_service` 的握手超时最长 10s —— **在这段时间里该服务器的槽
不存在**。

**为什么坏**：挂载表在每次 dispatch miss 时**实时重建**（`mount.rs`
`find_mounted`），而模型看到的 schema 是**本轮请求开始时**构建的。两者之间
若发生重连：① 该服务器的工具从挂载表消失 → 模型按旧 schema 调用的名字
变成「未知工具」；② 更糟的是**同前缀碰撞时编号漂移**——两台服务器净化后
前缀相同且工具同名时，名字是 `mcp_p_read` / `mcp_p_read_2`；A 台重连期间
B 台的工具被重新编号成 `mcp_p_read` → **模型本想调 A 的 `mcp_p_read`
被路由到 B**。触发面窄（需同前缀 + 同工具名 + 重连在飞），但后果是
**跨服务器静默错调**。

**修法**：重连期间保留旧槽（先建新槽、成功后再换旧），或把工具名映射改成
按服务器 id 而非序号派生（序号只在构建时消歧，调用时按 id 反查）。

#### P1-18 MCP 的 HTTP 分支不做 DNS 解析 → SSRF 缺口（**本报告初稿写错，已更正**）

**位置**：`src-tauri/src/bot/mcp/manager.rs:369-373`（HTTP 分支发请求前的唯一闸）·
`src-tauri/src/bot/mcp/config.rs:294-295`（闸的实现与自述）

**证据**（主会话复核）：
```rust
// manager.rs:369-373
McpTransport::Http => {
    let url = cfg.url.as_deref().unwrap_or("").trim().to_string();
    if let Err(reason) = super::config::http_url_is_public(&url) {
        return Err(format!("URL 安全闸：{reason}"));
    }
```
```rust
// config.rs:294-295（闸的自述）
/// 字符串/字面量级校验，不做 DNS 解析（解析结果可变且 TOCTOU；DNS 级校验二期再议）。
pub fn http_url_is_public(u: &str) -> Result<(), String> {
```
全量检索：`grep -rn "check_public_url|bot_web|is_private_host" src-tauri/src/bot/mcp/`
→ **零命中**。`git log -S "check_public_url" -- src-tauri/src/bot/mcp/` → 无历史
——**MCP 链从未接过 DNS 级闸**。

**对照组存在且更强**：`bot_web.rs:891` 的 `check_public_url` 做 DNS 解析、
要求每个解析出的 IP 都是公网、并把 addr 钉给 reqwest。**远程技能安装链
复用了它**（`bot_skills/remote.rs:242` `crate::bot_web::check_public_url(&u)`）
——所以同一应用里**远程技能安装有 DNS 级 SSRF 防护，MCP HTTP 分支没有**。

**为什么坏**：MCP 的 HTTP 分支会把 **agent 上下文 + 用户配置的 Authorization 头**
发往该 URL。字面量闸只挡「URL 里写着内网地址」，挡不住「URL 里写着公网域名、
但该域名解析到内网」：
- `http://internal.corp/mcp`（内网域名）→ 放行 → 打到内网服务
- `http://evil.example/mcp`（DNS rebinding）→ 放行
- `http://169.254.169.254.nip.io/`（公网域名解析到链路本地）→ 放行
  （`is_reserved_ipv4` 只对**字面量 IP** 生效，域名分支根本不查）

**已挡住的不用重造**：`file://` 等非 http/https；`localhost` 家族与尾点变体；
数字/十六进制形式主机名（`2130706433`、`0x7f.0x0.1`）；`.local`；
`127.0.0.1` / 环回 / 私网 / 链路本地（169.254.169.254）；IPv6 保留段与转换前缀
（IPv4-mapped / IPv4-compatible / 6to4 / Teredo / NAT64 / 文档段）；
重定向穿透（`manager.rs:397` `redirect(Policy::none())`）；保存口 + 连接口双重把关。
**字面量 IPv4/IPv6 侧其实比 `bot_web` 更全**——缺口只在「域名 → IP」这一跳。

**修法**：HTTP 分支改用 `bot_web::check_public_url` 同款校验，并把解析出的
addr 用 `ClientBuilder::resolve` 钉给 reqwest（校验与请求之间不再二次解析）；
或把 `config.rs:294` 那句「二期再议」从注释升级为一条待办并明确负责人。

> **本报告初稿在这里写错了。** 初稿的 §3.2 写着「MCP 侧复用同一份
> `check_public_url`，DNS rebinding 被堵住」——那是我从**远程技能安装链**
> 的复用关系外推的，**没有核 MCP 自己的 HTTP 分支**。MCP 子审计完成后
> 才发现两者是**不同的闸**。这条留在 §四「复核时改过的地方」里作为
> 本轮审计自身的教训：**「A 复用了 X」不等于「B 也复用了 X」。**

#### P2-6 stdout / stderr 读取侧无行长上限 → 外部 MCP 服务器可撑爆宿主内存

**位置**：`src-tauri/src/bot/mcp/manager.rs:320-331`（stderr 读取）·
rmcp 3.5.0 `src/transport/async_rw.rs:137`（stdout 读取）

**证据**（主会话复核 rmcp 源码）：
```rust
// rmcp-3.5.0/src/transport/async_rw.rs:137
match self.read.read_until(b'\n', &mut self.line_buf).await {
```
`line_buf: Vec<u8>`，`read_until` **一直追加直到遇到 `\n`**——**没有长度上限**。
codec 的 `max_length` 对读侧不生效：`async_rw.rs:225` 把
`JsonRpcMessageCodec::new()` 的 `max_length` 设成 `usize::MAX`，且 codec
只用在**写侧**。stdio 客户端传输走这条路径（`child_process.rs:38,159`）。

stderr 侧注释自承同样的问题（`manager.rs:325-327`）：
> 「行级字节帽：子进程刷超长行时 `BufReader::lines` 会**整行分配**，
> 4k 字符截断只影响尾部诊断展示不影响判定」——**上界在分配之后**。

**为什么坏**：外部 MCP 服务器（或其依赖的 npm 包）打印一条**不带换行**的长行
（进度条、base64 blob、崩溃 dump）→ 宿主进程的 `line_buf` / `String` 无界增长
→ **宿主 OOM**。这是**外部输入驱动宿主内存**的路径，而 MCP 服务器本身是
不可信第三方代码。

**同时要澄清（不要误判为更严重）**：垃圾行 / 二进制**不会**杀死连接。
`async_rw.rs:164-172` 把非法 JSON 与非法 UTF-8（`InvalidUnicodeCodePoint`
属 Syntax 类）**逐行忽略**，会话继续；形状不对但 JSON 合法则回一个
`Invalid Request` 响应。**没有 panic 路径**。

**修法**：读侧加行长上限（如 1MB 截断，或超限直接断开该连接并记审计）；
stderr 侧改为「按字节流分块切行、只保留尾部 N 字节」，避免整行分配。

#### P2-7 `has_inline_secrets` 零生产调用：死函数 + 文档口径不一致

**位置**：`src-tauri/src/bot/mcp/config.rs:225-229`（定义）·
`src-tauri/src/bot/config/io.rs:288-289`（实现用的是**孪生的 Value 级判据**）

**证据**：全仓引用只有 5 处 = 定义 1 处 + `secrets.rs:418-426` 的**单测**
（该测试 mod 起于 `secrets.rs:383`）→ **生产调用点为 0**。真正的迁移实现用的是
```rust
let inline = !env.as_object().map(|m| m.is_empty()).unwrap_or(true)
    || !headers.as_object().map(|m| m.is_empty()).unwrap_or(true);
```
而设计文档把它列为迁移判据（`docs/MCP-KEYSLOT-MIGRATION-DESIGN-2026-09-29.md:35`
「`has_inline_secrets(&server) -> bool`（**迁移判据**）」、`:59`、`:82`）。

**为什么坏**：两处口径不一致——文档说「迁移的短路判据是 `has_inline_secrets`」，
实现用一个**平行的、针对原始 JSON Value 的复制品**。该函数本身成为**只被测试
引用的死函数**（AGENTS.md「不留死代码」）。它的注释（`config.rs:225`）「只用于
**迁移前对原始文件内容**判定」在事实上就与实现矛盾：实现判定的是**原始 JSON**，
而这个函数判定的是**水合后的类型化配置**（水合后所有服务器看起来都带机密
——正是它自己注释里说的「水合后的内存值不适用」）。

**修法**：二选一——把 `io.rs:288-289` 换成调用 `has_inline_secrets`（迁移读的是
含明文的原始文件，`skip_serializing` 不影响反序列化，技术上可行），或删掉该
函数与单测、并把设计文档三处表述改成「Value 级判据」。

#### 本链其它已确认项


- **P1-7**：`bot/mcp/manager.rs:1116` 的「缺二进制 → 置 Down + 懒重连」
  测试**从未运行过**（缺 `#[tokio::test]`），且只有 clippy 的 `dead_code`
  知道这件事——而 clippy 不在门禁里。
  **该测试是这条路径的唯一覆盖**：grep 全仓 `state == "down"` 的断言，
  其余三处（`manager.rs:1150/1170`）走的是 HTTP URL 闸 / DNS 闸路径，
  **不覆盖「spawn 失败 → Down → 懒重连恢复」这条状态迁移**。
  （成因判定：测试逻辑本身今天仍然成立——fixture `tests/fixtures/mcp_echo_server.py`
  存在，`expect_err` / `state == "down"` / `error.is_some()` / 换正确配置后
  `state == "connected"` 五个断言都应通过；**是写漏了属性，不是行为不完整**。）
- **P3**：`mount.rs:165-171` `spawn_blocking(...).await.unwrap_or_default()`
  —— 线程 join 失败被折叠成空列表，随后走「MCP 服务器已被删除」的 warn
  分支。业务上可接受（降级而非崩溃），但按 AGENTS.md「不吞错误」的口径
  应留痕（至少 `eprintln!` 一行）。
- **P3**：**外部 MCP 工具不进审计与幻觉守卫口径**。`dispatch.rs:563`
  `tools_index().get(name)` 对外部工具返回 `None` → `dispatch.rs:605-619`
  的 `tool.output.over_budget` **不产生**；`registry.rs:913` 的
  `claims_mutation` 只从 `TOOLS_TABLE` 派生 → 外部工具声称「我改了文件」
  **不会**被幻觉守卫打回（这是有意为之：外部工具行为宿主不可知）。风险在于
  「所有工具调用都有审计」这个直觉对 MCP 不成立——超长输出不留痕。
  修法：在 `execute_mcp_tool` 内按 MCP 侧的 `MAX_RESULT_BYTES`（30,000）
  单独发一条 `tool.output.over_budget`，并在架构文档写明既有口径。
- **P3**：**MCP 面板无轮询**（`McpPanel.tsx:244-279` 只有挂载时与
  `mcp-status-changed` 两个刷新点，全文无 `setInterval`）→ RSS / CPU% /
  uptime 长期停在打开那一刻的值；而服务面板有 5s 轮询
  （`ServicesPanel.tsx:104`）。连带：5s TTL 缓存**几乎无调用压力**
  （`manager.rs:127` 注释说「`mcp_status` 可能被前端轮询，每次都拍一张全机
  `ps`（~50ms）就重了」——当前前端根本不轮询，TTL 缓存形同虚设）。
  另外 `listening_sockets` **每次调用都现做**（`manager.rs:473`，不进缓存），
  macOS 上每次一趟 lsof。
- **P3**：`mount.rs:240` 注释称 MCP 结果上限「fetch_url 同口径」，实际
  30,000 vs 内置 `fetch_url` 的 65,536——**并非同口径**（MCP 侧更严）。
  方向安全（更小的上限不会撑爆上下文），但注释会误导后续维护者去对齐成 64KB。
- **P3**：机密迁移的明文回滚网 `bot-config.backup-mcp-keys.json`
  （`bot/config/io.rs:303-334`）**按设计含明文**且 0600，但 Windows 上无 ACL
  限制、且文件永不清理 → 明文长期驻留。属**设计取舍**（口径见
  `docs/MCP-KEYSLOT-MIGRATION-DESIGN-2026-09-29.md` §5），但残留敞口
  未在任何面板/文档里提示（建议补一句「迁移成功后可手动删除该备份文件」）。
- **P3**：MCP 面板前端类型注释仍写「stdio：环境变量（**明文存本机
  bot-config.json**）」（`src/components/SettingsPage/types.ts:112/116`）——
  机密早已迁到 keyring，注释停留在旧口径。
- **P3**：`config.rs:26-31` 的上限都是**单项**的（单参数 4096 / 32 参数 /
  32 env 项）→ 总量理论可达 ~256KB，越过 `ARG_MAX` 时 spawn 失败；
  若失败落在握手阶段，用户看到的是「初始化握手失败」，归因错位。
- **P3（设计取舍，已报备）**：`STDIO_LAUNCH_ALLOWLIST` 含 `docker`/`podman`
  （可挂宿主任意目录如 `-v /:/host`）与 `python*`/`node`（可执行任意脚本）
  → 实际是任意代码执行面。缓解：只有能改设置页的本地用户能配 + 确认弹窗 +
  保存口校验；且 `manager.rs:269-289` 用**逐臂字面量构造**（源码层面不存在
  「运行时变量 → 进程名」路径）。取舍本身合理，但设置页应写清
  「docker/podman 启动器可挂载宿主目录」。
- **待核**（子审计未完成，主会话未独立确证）：stdio 子进程的 stdout 遇到
  垃圾行 / 超大行 / 二进制时会不会 panic 或无界增长；5s TTL 缓存**没有覆盖
  socket 枚举**（`listening_sockets` 每次调用都现做一次 lsof），
  macOS 上每次两趟 lsof ≈ 100–400ms，而 `mcp_status` 走 `spawn_blocking`
  故不阻塞 runtime 但仍偏慢。**这几条在子审计回补前，不应记为已确认。**



### 3.3 技能链与 MCP 链的交叉面

`bot_skills/**` 里**零** `service` 相关引用——技能 DSL 步骤调
`start_service` / `stop_service` 走的是 `registry → bot/tools.rs →
bot/services.rs` 的**同一份**受管服务实现，并经过同一条 `execute_tool`
规则闸。**无职责重叠、无双份实现**。
但技能 DSL **目前没有任何服务声明字段**（`parse.rs` 无 `services` 之类 key），
所以「skill DSL 声明服务」这个能力还不存在——要加的话直接指向
`bot/services.rs` 即可，不存在双份实现的障碍。

---

## 四、复核时改过的地方

子代理的结论不是直接抄进本报告的。以下几条**主会话复核后改了定性或严重度**：

| 原结论 | 复核后 | 依据 |
|---|---|---|
| **本报告初稿自己**：「MCP 侧复用同一份 `check_public_url`，DNS rebinding 被堵住」 | **改判为 P1-18：MCP 的 HTTP 分支根本不做 DNS 解析** | 初稿从**远程技能安装链**的复用关系外推，没核 MCP 自己的 HTTP 分支。实况：`manager.rs:371` 走 `config.rs::http_url_is_public`（注释自承「字符串/字面量级校验，**不做 DNS 解析**」），`mcp/` 目录对 `check_public_url`/`bot_web` **零引用**。见 P1-18 与 §三 3.2 的更正说明 |
| 「记忆整理链可让 LLM 删掉受保护条目」定 P2 | 降为 **P3**，且**不算缺陷** | `SPEC.md:136` 明写「`is_protected` 只管容量淘汰，**不豁免 merge**」；设计文档 `:59` 的「不可淘汰」语境就是容量淘汰。实现与口径**一致**。只保留一条 P3 备注：SPEC 给的不豁免理由是「豁免会让两条近义记忆并存」，而整理链的 `Contradiction` op 裁决的是**矛盾**不是近义，该理由不迁移 |
| 「`bot_manage_file.rs:271` 两分支相同 = 平台差异丢失」 | 修正为**冗余分支**，差异没丢 | `Cargo.toml:97-98` 的 `trash = "5"` 跨平台，macOS→「废纸篓」的区分仍在；该段自引入起就是两个同值分支，非后续漂移 |
| 「`allowed-tools` 在 `use_skill` 路径失效」定 P1 | 降为 **P2** | `allowed-tools` 是**作者自限**手段，不是对抗恶意模型的安全边界；失效率是契约问题不是权限突破 |
| 「远程技能零完整性校验」定 P1 | 降为 **P2** | 已有闸确实到位（SSRF 逐跳重校验 + DNS 钉 IP + zip slip/软链/炸弹四重闸 + 路径穿越防护），缺的是供应链完整性一层，属加固项 |
| 「`SKILL-DSL.md` 工具白名单表列 `list_tasks`/`bind_file`」定 P2 | 保留 **P2**，但归因改写 | 属实（`list_tasks` 已合并进 `query_tasks`，`bind_file` 已删，示例里的 `move_task`/`delete_file` 从不存在），但注册表侧有 `retired_tool_names_absent_from_schemas` 编译期锁——**锁住的是「不得出现在 schema 描述里」，没锁住文档表**。作者手册仍会误导 |
| 「记忆抽取限频表无界 + 重启失效」定 P3 | 保留 **P3**，但明确**不是缺陷** | 限频是**成本闸**不是正确性闸，重启后立刻抽一次是设计可接受的 |
| 「MCP 面板无能力标志」 | 由主会话在 MCP 子审计**之前**独立发现并写入 → 子审计独立复核**一致** | 两路独立得出同一条，可信度高 |

---

## 五、查过且没问题的部分（点名，避免漏审误读）

**前后端契约（主会话直审，全量对拍）**

- **命令面**：150 个注册命令 ↔ 151 个前端 `invoke` 名（多出的一个是
  `plugin:autostart|is_enabled`，插件调用）。**前端调了未注册的：0 个；
  注册了但前端从不调的：0 个**（无死命令）。
- **事件面**：24 个后端 emit ↔ 29 个前端 listen。**听而无发：0；
  发而无听：0**。脚本能解析 `emit(CONST_NAME)` 常量形式与注释剥离。
- **类型镜像**：`src/types.ts` 的 `Task` 与 Rust `db/tasks.rs::Task` 逐字段
  对齐（camelCase 约定，`expectedUpdatedAt` 的 `skip_serializing` 两侧一致）。
- **错误码**：`tests-audit/audit_error_codes.py` 锁 Rust 枚举 ↔ TS 联合类型的
  集合与**声明顺序**。
- **工具注册表**：`BUILTIN_TOOL_COUNT = 63` = `TOOLS_TABLE` 实际 63 条 =
  `SPEC.md:248`「内置工具 63 个」。**33 个 `mutating: true` 的工具全部有
  `claims_patterns`，没有 `mutating: false` 却带 `claims_patterns` 的** ——
  变更声称闸的覆盖无缺口。

**MCP 链（MCP 子审计逐条核过为绿的部分）**

- **端到端闭环完整**：`config` → `secrets`（keyring `mcp:{id}`）→ `manager`
  （槽位/握手/工具发现）→ `mount`（schema 段 + 反查）→ `dispatch`（兜底路由）
  → 回灌 → 前端。5 个 tauri 命令在 `lib.rs:692-696` 全部注册；`lib.rs:244`
  退出调 `shutdown_all`，`lib.rs:341-345` 启动调 `reload_from_config`。
  **未发现孤儿函数**（除 P2-7 的 `has_inline_secrets`）。
- **工具命名与撞名**：`mcp_{前缀}_{工具}` 恒定，前缀与工具名都净化成 ASCII 小写，
  前缀 24 字节 / 函数名 64 字节硬上限 + 6 位哈希尾。**内置 ↔ 外部当前无真撞名**
  ——63 个内置工具名**无一以 `mcp` 开头**（唯一的 `mcp_fake_x` 是测试夹具）；
  dispatch 侧内置优先、MCP 兜底；`find_mounted` 精确匹配，编造的 `mcp_*` 名
  回「未知工具」，**不会误命中内置工具**。消歧用 `used` 集合，后缀撞真实工具名
  也继续递增（含「A 有 read/read 2、B 有 read」的评审场景）。
- **schema 体积与裁剪**：`MAX_SCHEMA_BYTES = 8KB` 超限时**降级为宽松空参数
  对象 + 描述标注、保留工具**——不丢工具（丢了模型就看不见能力），取舍正确；
  描述截断 512 字符；单服务器工具数上限 128（保序截尾 + 留痕）；结果侧
  `MAX_RESULT_BYTES = 30_000`，structuredContent 先钳后入栈、总长再钳并标注
  原字节数——**无空洞截断**。
- **机密存储的其余部分**：`skip_serializing` 覆盖 `env`/`headers` **两个**字段，
  配置永不明文落盘成立（`secrets.rs:430-455` 有单测锁）；keyring 条目名
  `mcp:{id}` 的 id 由后端 uuid 生成 → **无撞名**；Linux 降级单文件 0600 +
  tmp+rename 原子写；blob 上限 2048 字节；水合读失败**不缓存**（下次重试）；
  `mcp_server_delete` 会 purge 机密。
- **URL 闸的字面量侧**：非 http/https scheme、`localhost` 家族与尾点变体、
  数字/十六进制形式主机名、`.local`、环回/私网/链路本地（169.254.169.254）、
  IPv6 保留段与转换前缀（IPv4-mapped / IPv4-compatible / 6to4 / Teredo /
  NAT64 / 文档段）**全部已拦**；重定向穿透已拦（`Policy::none()`）；
  保存口 + 连接口**双重把关**。**字面量 IPv4/IPv6 侧其实比 `bot_web` 更全**——
  缺的只是「域名 → IP」这一跳（P1-18）。
- **stdout 协议健壮性**：垃圾行 / 非法 UTF-8 / 二进制**被逐行忽略，不会 panic
  也不会断链**（rmcp `async_rw.rs:164-172`）；形状不对但 JSON 合法则回
  `Invalid Request` 响应。stderr 侧有 20 行 × 4000 字符的环形上界
  （诊断输出不丢）——注意这个上界在**整行分配之后**（P2-6）。
- **子进程生命周期（unix）**：MCP 与 services **共用同一份**
  `bot::procs::stop_tree`（`manager.rs:754` vs `services.rs:1372`），
  **不是重复实现**，差别仅在等待回调；`close_old` 先阶梯后取消令牌，顺序正确
  且有注释说明；unix 下 `cmd.process_group(0)` 自成进程组；退出时 `shutdown()`
  drain 全部槽位；`reload_from_config` 对不再启用的 id 主动 `disconnect` 并记
  `mcp.disconnected` 审计。
- **资源采样**：并发下 `sample_cache` 用 `std::sync::Mutex` **全程持锁** →
  **不会重复采样**；`mcp_status` 已把它挪到 `spawn_blocking`，不卡 runtime；
  CPU% 公式与 services **共用** `procs::cpu_usage_pct`（除零已处理）；采样口径
  是整棵进程树合计，与架构文档一致。
- **白名单构造**：逐臂**字面量**构造，源码层面不存在「运行时变量 → 进程名」
  的路径；前端下拉与后端同表。

**Rust 侧结构性健康**

- **子 agent 状态机写口唯一**：`UPDATE subagents SET status` 全仓仅
  `db/subagents.rs:550` 一处，所有终态路径都过 `transition_allowed`；
  终态不可复活。
- **预算三顶都在生产路径强制**：`max_turns` → 模型循环 `rounds_budget`；
  `max_tool_calls` → `dispatch.rs:465` 执行前原子扣减 + `bot_model_loop.rs:1724`
  单轮熔断；`max_wall_seconds` → `tokio::time::timeout`（排队不计时）。
- **幂等键服务端派生**（上轮 U8 修复）：父会话 + 档位 + 目标 + 验收标准 +
  指定模型，只认活着行；查表 + 插入在同一把 `DB_WRITE_LOCK` 内，无并发窗口。
- **Rust 死代码**：编译期 `dead_code` 权威，全仓**仅 1 处**（P1-7 那条失效测试）；
  12 处 `#[allow(dead_code)]` 逐条看过，均为平台/测试条件，无掩盖真死代码。
- **SSRF 闸单源**：`bot_skills/remote.rs` 复用 `bot_web::check_public_url`，
  私网判据只有一份（`bot_web.rs` 的 `ipv4_is_private`/`ipv6_is_private`）；
  逐跳重定向各自重过闸；`resolve_to_addrs` 钉 IP 堵 DNS TOCTOU。
- **解包限额单源**：新文件复用 `bot_archive::Limits::unpack()`。
- **信号阶梯单源**：`services.rs:1372` 复用 `bot::procs::stop_tree`，
  阶梯时长只有一份；MCP stdio 子进程走同一套。
- **路径白名单判定核单源**：`bot_fs.rs:462 is_within_allowlist` 按分量比较，
  `/tmp/ab` 不误吞 `/tmp/abc`。
- **定时串格式单源**：只有 `bot_reminders.rs:126` 一处 builder。
- **定时链 tick**：作业与提醒共用 scheduler 的同一个 30s tick；`due_notify`
  是独立 ticker 但扫的是不同数据源，无重复触发。
- **记忆抽取限频真在生效**（30min/会话，进程内 check-and-set）；
  容量 500 单层上限兜住降级模式下的重复插入。
- **notify 幂等键无跨类撞键**：`memory:` / `artifact:` / `wfq:` / `service:`
  四段前缀互不为前缀。
- **`file_paths_with_receipt` 的索引真的用上了**：`EXPLAIN QUERY PLAN` 实测
  → `SEARCH file_changes USING COVERING INDEX idx_changes_path (path=?)`。
- **工具链**：63 个工具的 schema 名与 `ToolDef.name` 一一对应（编译期锁）；
  `tools_index` 覆盖全部表名；退役工具名不得出现在任何 schema 描述里。

**审查链**

- `tests-audit/` 四个脚本**形态区分正确**（3 个 pytest + 1 个独立守卫，
  逐个点名跑，避开了「pytest 收不到用例却退出 0」那个坑），26 例全绿。
- `audit_tauri_bridge.py` 的事件提取能处理常量形式与注入闭包，且**先剥注释**。
- 70 处 `include_str!` 源码扫描守卫质量合格：限定生产代码切片
  （`split_once("#[cfg(test)]")`）、needle 缺失即 panic、函数体内查缺席。
  **没有发现空转守卫**。
- 前端有 `knip`（死文件/死 export/死依赖）、`oxlint`、`tsc` 三道；
  Rust 侧有 `fmt` / `check` / `machete`，**缺的只有 clippy**（P1-7）。

**冗余**

- `escape_for_log` 是**单源**（`audit.rs` 用 `pub(crate) use` 重导出
  `bot/log_sanitize.rs`）——不是三份副本。
- `keyword_hit`（`intent_router.rs` 命令首词 vs `bot/tools.rs` 任务字段）、
  `extract_json`（返回 `&str` vs 返回 `Value`）是**同名异义**，
  `dt` 是各测试模块的**局部 helper**——三者都不是重复实现。
- 服务层与沙箱层**职责边界清晰、无双份实现**：服务层直接 spawn、不继承
  rlimit、stdout 落文件；沙箱层才设 `RLIMIT_AS`/`RLIMIT_CPU`、才连坐杀组。

---

## 六、修复优先级建议

| 序 | 项 | 一句话 | 性质 |
|---|---|---|---|
| 1 | P0-1 | `create_skill` 补 intents 宽度闸 + 确认窗；`skills_import` 补准入校验 | 安全边界 |
| 2 | P1-18 | MCP HTTP 分支接 DNS 级 SSRF 闸 + 钉 IP | 安全边界 |
| 3 | P1-16 / P1-17 | MCP 编辑别删机密；指纹要把 env/headers 算进去 | 数据丢失 |
| 4 | P1-4 | 工具层「拒绝/失败」不再返回 `Ok`；补静态扫描守卫 | 跨链断裂 |
| 5 | P1-1 / P1-2 | 采集失败显式 `Unsupported`；Windows 停止路径不再谎报 | 安全边界 |
| 6 | P1-12 | 主聊天路径补 `catch_unwind`（与子 agent 同范式） | 健壮性 |
| 7 | P2-6 | rmcp 读侧加行长上限，防外部服务器撑爆宿主内存 | 资源 |
| 8 | P1-7 | 补 `#[tokio::test]` + clippy 进门禁 | 门禁（防复发） |
| 9 | P1-6 / P1-5 | `meta.rollback` 被真正读一次；工具预算触达落 `BudgetExceeded` | 契约 |
| 10 | P2-1 / P2-2 | 审计阈值与截断 cap 同源；溢出指针别承诺「完整」 | 口径 |
| 11 | P1-8 / P1-9 / P1-10 | 架构文档 §6.4 + 散文数字；SPEC 附录口径 | 文档一致性 |
| 12 | P1-11 | 抽共享绑定 hook | 真重复（已漂移） |

第 4、8 项的性质值得单说：**这两类问题单测抓不到**——
「工具把拒绝报成成功」只有静态扫描能抓，「测试没被收集」只有 clippy 能抓。
上一轮审计建的「源码扫描守卫」这条路是对的方向，应该把它扩到这两类。

另外 **P1-16 与 P1-17 有一个共同的根因**：`McpServerConfig.env/headers` 的
`skip_serializing` 同时造成了「前端拿不到机密」（P1-16）与「指纹看不见机密」
（P1-17）。**修一处 `skip_serializing` 的用法能同时解掉两条**——比如
给 `Serialize` 写一个「脱敏视图」：指纹用**完整值**算、对外下发用**脱敏值**
算。这是本轮性价比最高的一个修复点。

**最后一条关于方法本身**：P1-18 是**本报告初稿自己写错**的一条
（初稿断言「MCP 侧复用同一份 SSRF 闸」）。它是这么来的：从**远程技能安装链**
的复用关系外推到 MCP 链，没核 MCP 自己的 HTTP 分支。MCP 子审计完成后才
发现两者是**不同的闸**。教训是：**「A 复用了 X」不等于「B 也复用了 X」**——
跨链的复用关系必须逐条落到调用点，不能从一个调用点外推。这也说明
「子代理 + 主会话复核」这个双层结构确实拦住了东西：如果只有子代理，
这条会被原样写进报告。
