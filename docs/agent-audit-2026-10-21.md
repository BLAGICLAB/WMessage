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

### P1-1 端口闸的采集失败被折叠成「判了没事」（已修，见 §七）

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

### P1-4 工具层把「拒绝 / 失败」返回成 `Ok` → 污染幻觉守卫的成功凭证表（已修，见 §七）

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

#### P2-3 `screenshot` 实际写盘却标 `mutating: false`（已修，见 §七）

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

**SSRF 闸 —— ⚠️ 见 P1-18（已修，见 §七），**MCP 自己的 HTTP 分支**没有用 DNS 级闸**，
只有字面量级检查。远程技能安装链（`bot_skills/remote.rs:242`）才复用了
`bot_web::check_public_url`（DNS 解析 + 逐 IP 公网校验 + `resolve_to_addrs`
钉 IP，TOCTOU 已堵）。**两者是不同的闸**——初稿把这条写反了，见 P1-18。


**本链已确认的问题**（主会话逐条复核成立）：

#### P1-16 编辑 MCP 服务器会静默删掉它的 keyring 机密（env / headers）（已修，见 §七）

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

#### P1-17 连接指纹对 env / headers 不敏感 → 改了机密不重连（已修，见 §七）

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

#### P2-4 MCP 面板没有「本平台能不能采集」的能力标志（已修，见 §七）

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

#### P2-5 连接重建期间工具集瞬时消失，同前缀服务器会被错路由（已修，见 §七）

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

#### P1-18 MCP 的 HTTP 分支不做 DNS 解析 → SSRF 缺口（**本报告初稿写错，已更正**；已修，见 §七）

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

#### P2-7 `has_inline_secrets` 零生产调用：死函数 + 文档口径不一致（已修，见 §七）

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

---

## 七、修复落地记录

按 §六 优先级逐条落地。每条一个 commit，行为变更配测试（修 bug 先写复现测试）。

| 项 | 状态 | 落点 |
|---|---|---|
| P0-1 | 已修 | `intent_router::check_intent_pattern`（宽度闸）+ `tool_create_skill` 确认窗 + `skills_import` 补准入核 |
| P1-18 | 已修 | MCP HTTP 分支接 DNS 级 SSRF 闸 + 钉 IP（`manager::http_client_for_mcp`） |
| P1-16 | 已修 | 脱敏视图下发（`mcp_server_secrets_masked`）+ 掩码回传语义（`secrets::resolve_masked`） |
| P1-17 | 已修 | 指纹显式枚举字段，含 env/headers（`manager::fingerprint`） |
| P1-4 | 已修 | 194 处工具返回点改 `denied` / `error` + 静态扫描守卫重写（`audit.rs`） |
| P1-1 | 已修 | 采集层显式回报采样成败（`procs::ListeningSample`）+ 采样失败降级 `Unsupported` |
| P1-2 | 已修 | Windows 停止阶梯落地（`taskkill /T /F`）+ 停止失败不谎报「已停」+ pid 0/1 不许当树根（`procs::stop_tree` / `services::stop_entry`）|
| P1-12 | 已修 | 主聊天路径工具执行包 `catch_unwind`（`dispatch::run_tool_panic_isolated`）+ 接线守卫 |
| P2-6 | 已修 | stdio 读侧行长上限 1MB（`mcp/stdio_transport::LineCappedReader`）+ stderr 按字节块切行只留尾部 + 子进程生命周期自托管（`CappedChildProcess`）|
| P1-7 | 已修 | 失效测试补回 `#[tokio::test]` + `cargo clippy --all-targets -- -D warnings` 进快门禁（`scripts/test-fast.sh`）+ 16 处存量 lint 清零 |
| P1-6 | 已修 | 回滚段只在 `rollback: auto` 时执行（`scheduler`）+ `auto` 无回滚段降级为 `none`（`parse::normalize_rollback_meta`）|
| P1-5 | 已修 | 工具预算触达落 `BudgetExceeded`（收尾读 `ctx.used_tool_calls` + `classify_outcome` 认工具熔断标记）|
| P2-1 | 已修 | 「超预算」审计事件移到截断发生处（与 `effective_output_cap` 同源），dispatch 侧不再用 per-tool 预算当阈值 |
| P2-2 | 已修 | 溢出指针按文件规模说清可达范围（`read_text_file` 单次上限 100KB + grep_files 兜底路径）|
| P1-8 | 已修 | §6.4 清单按实际文件重建（155↔155，幽灵/漏登记归零）+ 脚本新增 §6.4 双向校验 |
| P1-9 | 已修 | 架构文档开头散文数字改为实测值 + 脚本新增散文数字校验 + 12 处行数标注刷新 |
| P1-10 | 已修 | SPEC 附录口径改成真话（v1 快照只读 / 机制详录活节并立真标题）|
| P1-11 | 已修 | 绑定内核收成一份（`taskFiles::applyBinding` + `lib/useTaskFiles.ts` hook）：卡片 / 详情面板 / 上下文栏三处共用，合并基线一律取最新 files |
| P1-3 | 已修 | 服务层父进程看门狗（unix 管道 EOF 助手 + Windows `KILL_ON_JOB_CLOSE` job）+ `RunEvent::Exit` 兜底 |
| P2-7 | 已修 | 迁移判据收成一份（`mcp::config::has_inline_secrets` 判**原始 JSON 条目**）+ `config/io.rs` 改调它 + 判据单源扫描守卫 |
| P2-3 | 已修 | `screenshot` 归变更类（`mutating: true` + 声称话术）+ 产出二进制档落盘凭证（`bot_desktop::screenshot_ok_result`）|
| P2-4 | 已修 | `McpServerStatus` 回带平台采集能力（`listeningSupported` / `resourceSupported`）+ 面板在 false 时明说「本平台不支持采集」（`procs::snapshot_supported`）|
| P2-5 | 已修 | 函数名走进程级登记簿（`mount::NameRegistry`）：名字 ↔ 服务器跨断连稳定；`find_mounted` 改查登记簿（离线服务器也能反查，走懒重连）|

### P0-1 的落地口径

**宽度闸三道**（判据落在 `intent_router::check_intent_pattern`，纯函数）：
① 模式必须可编译——编译不了的模式在 `compile_rules` 里被 `.ok()` 静默丢弃，
装上却永不触发（哑弹技能），不如当场拒；
② 必须含 ≥2 字符的**连续字面量段**——`.*` / `(?s).*` / `^.*$` / `.{0,}` /
`[\s\S]*` 这类模式没有字面量，能匹配任意文本（含空串）；
③ 不得能匹配「不含其字面量的探针串」——`(?s)(润色|修订)?.*` 这类「关键词可选」
的写法含字面量却仍能匹配任意文本，同样过宽。

②选「字面量连续段 ≥2」而不是「禁 `.*`」：后者会误杀
`(?i)(润色|修订).{0,15}(word|文档)` 这类合法的复合模式（现役 docx 技能正是这批）。

**确认窗**：`tool_create_skill` 落盘前走 `ask_user_confirm`（与 `install_skill`
同一姿态），确认文案同时展示 **intents 触发词**与 **mode**——这两项决定「建成后
哪些话会被路由进来」和「命中后是否绕开 LLM 直接执行」，不给用户看就等于让用户盲批。
非交互 / 后台执行按 `ask_confirm_inner` 的既有口径默认拒绝。

**准入核收敛**：`create_skill` / `install_skill` / 设置页导入三条装入入口现在
走同一个 `validate_created_meta`（description 必填 + 危险词 + intents 宽度闸），
解析与名称一致性检查单源在 `prepare_created_meta`。

**未做的部分**：原「修法建议」③「给 `create_skill` 生成的路由加『仅本会话有效』
标记」**本轮未实现**——它要动的是进程级路由表的数据结构（`route_user_input`
得带 session 参数），与①②不是一个量级。①②已把「模型可在无用户确认的情况下
永久改写进程级行为」这条路径关掉，③留作后续加固。

### P1-18 的落地口径

**连接口两道闸**（`manager::http_client_for_mcp`）：
① **字面量级**（`config::http_url_is_public`）：scheme + host 字面量——拒绝
localhost 家族、环回、私网、链路本地、保留段、数字形式 IPv4 变体、mDNS
`.local`、IPv4-mapped / IPv4-compatible / 6to4 / Teredo / NAT64 / 文档段。
这一层比 `bot_web` 的字面量侧更全，**保留不动**。
② **DNS 级**（`bot_web::check_public_url`，判据单源，与抓取链同一份）：域名
解析出的**每个** IP 都必须是公网——内网域名、DNS rebinding、
`169.254.169.254.nip.io` 这类「公网域名解析到内网」在这一跳被拦。

**钉 IP**：`resolve_to_addrs` 把校验过的地址钉给 reqwest，校验与请求之间不再
二次解析（DNS TOCTOU 窗口关闭）。尾点变体（`example.com.`）两种形态都登记
——DNS 查的是去尾点的名字，reqwest 查表用的是 URL 原样 host。

**保存口不做 DNS 解析**：`config::http_url_is_public` 现在返回解析后的 URL
（连接口要拿它做 DNS 级校验），判据本身一字未改——保存时做 DNS 解析会让
设置页在网络异常时卡住，而 DNS 结果本来就该在**连接时**现查。

**测试**（`manager::http_gate`）：内网/环回/链路本地 URL 在发请求前被拒；
公网 IP 字面量过两道闸；钉 IP 后用 `.invalid` 域名连本地回环桩成功（证明
reqwest 没有二次解析、Host 头仍是原域名）；源码文本钉住「HTTP 分支只有
`http_client_for_mcp` 这一个闸入口」。

### P1-16 / P1-17 的落地口径

**根因同源**：`McpServerConfig.env/headers` 的 `skip_serializing` 同时造成
「前端拿不到机密」（P1-16）与「指纹看不见机密」（P1-17）。两处一起解。

**P1-16 —— 脱敏视图 + 掩码回传语义**：
- 新命令 `mcp_server_secrets_masked(id)` 下发**键名 + 掩码值**（`MASK`），
  明文不出宿主；前端编辑表单回填掩码，用户看得见「哪些变量已设置」，
  能逐项增删。
- 保存时掩码值原样回传 = **「这一项保留原值」**（`secrets::resolve_masked`
  与**当前已存**的真值对表——掩码过期也不会把旧值写回去）；非掩码值 = 替换；
  **没回传的键 = 用户删掉了这一项**。
- 前端本组不展示时（stdio 的 headers / http 的 env）按脱敏视图原样回传，
  不发空对象——空对象在旧口径下等于「清空这一组」。
- 取脱敏视图失败 → **不开表单**（fail-closed）：表单里的空对象一旦被当成
  「清空」，就是又一次静默删机密。
- 旧口径「空 = 清除条目」保留在 `store_server_secrets` 内部（真值集合为空
  → 删 blob），但**到达那里之前**掩码已被还原，不再有「前端没回填」这一路。

**P1-17 —— 指纹显式枚举字段**：`fingerprint` 不再用
`serde_json::to_value(cfg)`（那走同一个 `Serialize`，机密字段被剥掉），
改为逐字段哈希，**含 env/headers**：改了 API token / 环境变量即换指纹 →
重连 → 新凭据生效。

**测试**：`secrets::masked` / `resolve_masked`（保留 / 替换 / 删除 / 掩码过期）
与 `manager::fingerprint`（env/headers 变即换指纹、相同配置指纹稳定）。

### P1-1 的落地口径

**类型层分开「采空」与「采不到」**：采集层现在返回 `procs::ListeningSample`
（`by_pid` + `complete`），不再只回一个可能为空的 map。`complete = false`
= 采集路径存在但本次采不到——判定层据此返回 `Unsupported`（**没判**）而不是
`Ok`（**判了没事**）。

**各平台的「采不到」怎么认**（实测口径，不是猜的）：
- **macOS**：`lsof` **空 stdout 不等于失败**——没有匹配项时 lsof 退出码非 0、
  stdout 与 stderr 都是空的；真正报错（选项非法 / 系统文件表耗尽等）时 stderr
  有内容。旧实现把「stdout 为空」当失败，于是采集器一失灵，采样结果就被
  折叠成「没人监听」。现按 stderr 判：stderr 有内容或二进制起不来 = 采集失败。
- **Linux**：`/proc/net/{tcp,tcp6,udp,udp6}` **四张表一张都读不到** = 采集失败
  （受限容器 / `hidepid`）；读到但表里没有 socket = 采空但采成功。

**连带修 L2 的连带失效**：采样失败时**保留端口动态保留集不清空**。旧流程把
「宿主没在监听」当成事实去清空它——采集器一坏，L2（防抢占宿主端口）跟着一起
静默失效，两个闸一起废比一个闸废更糟。

**不再静默**：采样失败写审计 `service.listening_sample_failed`（只在
「成功 → 失败」时记一条，避免 30s 一轮刷屏）；设置页服务面板多一行
「最近一次监听端口采集失败——端口闸本轮未判」（`services_snapshot_json` 的
`listeningSampleOk`）。

**已知残留**：`lsof` 跑得通但**看不见别的用户的进程**时（权限不足），它既不
报错也不出行——这种「采空但其实是采不到」静态判不了。报告建议的**金丝雀**
（拿宿主自身已知在监听的端口当基准）本轮未做：宿主只在本地 HTTP API 开着时
才在监听，金丝雀的适用条件不成立，硬上会变成误报源。这条留作后续加固。

**测试**：`judge_sockets` 采样失败 → `Unsupported`（采空+采成功 → `Ok`）；
保留集在采样失败时原样保留；macOS 上 `lsof` 空输出 = 没匹配而非失败；
真采到子进程监听 socket 时采样状态必须是「成功」。

### P1-4 的落地口径

**规模**：本报告初稿按 `audit.rs` 的文本分类器口径数出 60 处（A 类 12 / B 类 48）。
按「失败语义」重新穷举（含 `format!` 模板、跨行调用、变量实参），**实际改写
194 处**，覆盖 13 个文件（`bot/tools.rs` / `bot_fs.rs` / `bot_ui.rs` /
`bot_desktop.rs` / `bot_manage_file.rs` / `bot_reminders.rs` / `bot_archive.rs` /
`bot_skills/remote.rs` / `bot_skills/runtime.rs` / `apps/manage.rs` /
`memory/mod.rs` / `py/ast.rs` / `ocr.rs`），另改 `bot/mcp/mount.rs` 2 处
（MCP 服务器被禁用 / 被删除 → `denied`）。**初稿的 60 处是下界**：文本分类器
只认「失败 / 错误」两类，「缺少 X 参数」「未找到」「不能为空」「拒绝」这些
参数闸与拒绝类文案它一条都不认。

**归属三档**（写进 SPEC「工具成败只由 status 承担」）：
① **参数闸**（缺必填参数 / 参数为空 / 参数非法 / 超上限 / 目标不存在或已存在 /
路径校验不过）→ `Denied`（工具没动手）；
② **门禁与用户拒绝**（白名单闸拦下、用户点否、确认超时、功能开关关闭）→
`Denied`；
③ **执行失败**（读写 / 下载 / 解包 / 删除 / 设置 / 取消 / 点击 / 输入 / 激活 /
探测失败）→ `Error`（动了没成）。

**两处按错误类型分流，不按文案**：
- `resolve_task` 的失败混着两种语义——`CommandError::DbError`（读库失败）是
  执行失败（`Error`），`DomainRule`（定位不到 / 参数缺失 / taskId 与标题打架）
  是工具没动手（`Denied`）。新增 `resolve_task_err` 按变体分，文案一字未改。
- `resolve_owner` 原先把「读成员表失败」和「同名歧义 / 未命中」塞进同一个
  `Err(String)`：前者是执行失败，后者是参数闸。**成员表改由调用方读**，
  `resolve_owner` 收窄成纯函数（`&[PeopleEntry] → Result<String, String>`）。

**`Warn` 也是成功**——所以失败文案连 `Warn` 都不能配。`ask_user` 的 7 处
`Warn` 是**设计内降级**（未获回答 / 队列故障 → 按模型声明的假设继续），
不是失败事件，按失败处理会让 Skill DSL 调度器误判步骤失败并触发回滚——
这类保留 `Warn`，并在守卫的良性清单里写明理由。

**记忆内核的失败信号从文案改成类型**：`memory::record_lesson_core` 原先返
`String`，失败靠「失败：」前缀传递，调用方再按前缀猜 status。改成返
`Result<String, String>`，四个调用点（`tool_record_lesson` /
`auto_lesson_on_task_failure` / `bot::corrections` / `bot_skills::runtime`）
按类型分。

**守卫重写**（`audit.rs` 的 `failure_texted_results_never_use_ok_or_warn`）：
上一版守卫**长期空转**——它用 `split_once("#[cfg(test)]")` 取后半段当「生产
代码」，切反了，等于只扫测试代码；且只认「失败：/错误：」两种形态。重写后：
- 判定对象是**构造器调用的首个字符串实参**（注释、代码、非首参都不参与）；
- 失败语 = 「失败 / 错误」后接分隔符 + 参数闸与拒绝类关键词
  （拒绝 / 缺少 / 不能为空 / 未找到 / 不存在 / 非法 / 无效 / 未获确认 /
  没有可用 / 不可用 / 禁用）；
- 良性清单两条，各带理由（「逐条接受/拒绝」是 Word 修订模式给用户的操作
  指引；「提问队列暂不可用」是设计内降级）；
- **自带正例夹具**（`ok_warn_failure_scanner_catches_violations`）：违例必须
  抓到、正常文案 / 注释 / 字面量里的同形文本必须放过——上一版没有自测，
  所以没人看出它什么也没扫。

**变量实参扫不到**（`ToolResult::ok(e, …)`）：那些点的成败由调用方按错误
类型给，文案不承载语义，静态扫描无解。这条局限写进守卫的注释里。

**顺带清掉的失效注释**：104 行「首字「X」非 error/warn 前缀 → ok」这类注释
记录的是「为什么这里用 ok」——理由是文案首字。这套理由在 status 改由语义
决定之后不再成立，留着会让人以为「文案首字决定 status」，故一并删除。


### P1-2 的落地口径

**Windows 上不再谎报「已停止」**：

- `procs::stop_tree` 的返回类型从 `TreeStop` 改成 `Result<TreeStop, String>`：
  `Err` = **本平台没有可用的停止手段**（不是「这个进程停不掉」），调用方必须
  区分这两者。unix 阶梯照旧；Windows 走 `taskkill /PID <pid> /T /F`
  （整树强杀，不给优雅退出的机会）；其余平台显式失败而不是空转。
- `procs::pid_exists` 在 Windows 上改用 `OpenProcess` + `GetExitCodeProcess`
  判存活——unix 的 `kill(pid, 0)` 在 Windows 上没有对应物，恒 false 会让
  「等进程退出」立刻判定「已退出」，停止阶梯因此整条空转，而上层照样回报
  「已停止」。
- `services::stop_entry` 等不到进程退出时**不写 `Exited`**、**不回报
  `stopped: true`**：状态放回停止前的那个（服务留在活动集合里，退出时还会
  再收一次），返回一条明确错误，并写审计 `service.stop_failed`。写 `Exited`
  等于告诉退出清理「这个不用收了」，而它明明还在跑。
- MCP 侧同样不再把「阶梯跑完」当成「进程没了」：收尾再核一次存活。

**顺带挖出来的宿主自杀（本次最要紧的一条）**：

新写的「停不掉」用例第一次跑就把**整台机器的用户进程** SIGKILL 掉了
（连带 4 个并发测试一起死，表现为一片 SIGKILL）。根因不在测试，在生产代码：
`stop_tree` 直接拿登记表的 pid 当树根，而登记表里 spawn 前的占位条目 pid
就是 0——`descendants(snap, 0)` 于是把 **launchd 的整棵子树**（= 机器上每个
用户进程）当成「我们的后代」，随后逐个 SIGKILL。

修法落在**树根这一层**（不是信号那一层）：`send_signal` / `signal_group`
早就各自挡了 0，但那挡的是「信号发出去会自杀」；这里漏掉的是**收集**这一步
——pid 0 是「调用方进程组」的哨兵，pid 1 是 init/launchd（全机进程的祖先）。
现在 `descendants` 与 `stop_tree` 都拒收 pid ≤ 1 的树根（返回空集合 /
`no_process`），并把「没有进程可停」如实回报给调用方。

**spawn 窗口的竞态**：`do_start` 在 spawn 成功、pid 写进登记表之后多看一眼
状态——若这期间已被 stop 过（那时 pid 还是 0，登记表记的是「没有进程」），
就把刚起来的进程树收掉并让这次启动失败，否则它会变成宿主管不到的孤儿。

**测试**：`stop_tree(0, …)` 不杀任何进程（哨兵进程事后仍活着）；`descendants`
拒收 pid 0/1；停不掉时报错且状态放回原样；没起来过的服务报「没有进程」。

### P1-12 的落地口径

**主聊天路径的 panic 隔离**：工具 future 现在包 `catch_unwind`
（`dispatch::run_tool_panic_isolated`），panic 归一成 `ToolResult::error`
回灌给模型——模型能读到失败原因并换策略，这一轮也不会整轮丢掉。此前工具
panic → future unwind → `bot_chat` 整轮崩：用户这一轮对话全丢（无部分输出、
无错误提示），前端停在「正在思考」。子 agent runner 早有同样的隔离
（`bot_orchestrator::run_subagent`），hooks 扩展点三层都做了 fail-open——
被扩展的本体反而没有。

**归一成失败而不是成功**：成功凭证表会被污染（幻觉守卫查不到真实成功记录，
反过来给模型背书）。

**接线守卫**：`dispatch` 源码扫描钉住「内置工具执行与 MCP 兜底路由都必须走
隔离层，隔离层必须真的 catch_unwind 且归一成 `ToolResult::error`」。

### P2-6 的落地口径

**不用 vendor rmcp，也不用加依赖**。rmcp 的 `transport-child-process` 特性
**已隐含开启** `transport-async-rw`（同一个 crate 的另一个模块），于是
`rmcp::transport::async_rw::AsyncRwTransport` 直接可用——它就是 rmcp 自己
stdio 传输的**内核**（`TokioChildProcess` 内部就是
`AsyncRwTransport::new(stdout, stdin)`），差别只在读侧那一层。

**读侧行长上限**（`mcp/stdio_transport::LineCappedReader`）：每行最多转发
1MB，超出的字节**直接丢弃**（不缓冲、不分配），换行符照常转发 → rmcp 的
`read_until` 拿到的 `line_buf` 有界（≤1MB + 1），那条超长行按 rmcp 既有的
「非法 JSON 逐行忽略」跳过，**会话不断**（服务器刷完长行仍能正常应答）。

**为什么不选「超限直接断链」**：断链会把外部服务器的一次坏输出升级成
「这条 MCP 服务器不可用」，还得靠懒重连兜；截断只丢那一条消息。代价是
超长行之后的内容一起丢——但那条消息本来就不完整。

**截断不静默**：每条连接第一次截断时打一行诊断（「服务器输出超长行已截断，
该行被跳过，后续消息照常处理」），只报一次，免得被刷屏。

**stderr 侧**：原来用 `BufReader::lines()`——**整行分配**（上界在分配之后，
本报告原文已指出）。现在按 8KB 字节块读，行缓冲用 `VecDeque` 只留**尾部**
4KB（超长行的末尾通常才是真正的错误信息；前面的进度条 / base64 没有诊断
价值）。用双端队列而不是 `Vec` + `drain`：后者会让一条超长行退化成 O(n²)。

**子进程生命周期自己托管**（`CappedChildProcess`）：不再用 rmcp 的
`TokioChildProcess`（它把读侧写死了）。关闭口径与 rmcp 原实现一致——先关
stdin（服务器看见 EOF 自己退）→ 3s 优雅退出窗口 → 超时强杀；传输被丢弃时
同步 `start_kill`（不用 rmcp 的 `tokio::spawn(kill)`：后者在没有运行时的
Drop 里会 panic）。

**测试**：
- `LineCappedReader` 单测：一条 100KB 的无换行长行 → 只转发 cap 字节；
  正常行原样转发、超长行被截后**后续行照常送达**。
- `harvest_stderr` 单测：100KB 超长行只留尾部且长度有界；环形缓冲满则挤掉
  最老的一行。
- e2e（真实 python3 桩）：桩先吐 5MB 畸形 JSON 行 → 连接与工具调用照常，
  且**客户端没有回响应**（无上限时那行会被当成「形状不对的 JSON-RPC」→
  rmcp 回 Invalid Request → 桩能在 stderr 里看到）。这条 e2e 对上限敏感：
  把 cap 临时放大到 100MB 实测会失败（标记 `client-reply` 出现）。

### P1-7 的落地口径

**失效的测试补回属性**：`bot/mcp/manager.rs` 的
`missing_binary_marks_down_and_lazy_reconnects` 补上 `#[tokio::test]`。它覆盖的
「MCP stdio 指向不存在的脚本 → 置 Down + 错误可查 + 懒重连自动恢复」此前
**从未被收集运行**（缺属性 → clippy 的 dead_code 判死 → `cargo nextest list`
查无此例）。补回后实测通过——与本报告初稿的判断一致：是写漏了属性，不是
行为不完整。

**clippy 进快门禁**：`scripts/test-fast.sh` 新增 `[2.4/N]` 步：

```
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

- `--all-targets` 是关键：只 lint lib 的话，测试目标里的死函数照样看不见。
- **clippy 没装 = 这道门禁直接失败**（不静默 skip）。静默 skip 等于门禁只剩
  一个名字——「文档说的事实被代码否掉」那个形状。
- 存量 lint 清零：`-D warnings` 一开就是 **16 处报错**（`io_other_error` ×7、
  `collapsible_if` ×2、`field_reassign_with_default` ×7、`if_same_then_else`、
  `redundant_closure`、`manual_is_multiple_of`、`single_char_add_str`、
  `too_many_lines`）。这 16 处此前**从未被任何门禁拦过**——clippy 不在门禁里
  的代价，现在一次结清。清零后 `cargo clippy --all-targets -- -D warnings`
  全绿。

**文档口径同步**：`AGENTS.md` 的命令条改成真话（clippy 带 `--all-targets
-- -D warnings`，且**已在快门禁内**）；`docs/testing.md` 门禁表补 `[2.4/N]`
一行，并写明它防的是哪一类问题——「测试压根没被收集运行」，那种 `cargo
nextest` 跑起来全绿、只有 clippy 知道真相的坑。

### P1-6 的落地口径

**`meta.rollback` 终于被读了一次**：回滚段只在 `rollback: auto` 时执行。
此前 `scheduler.rs` 全文**零处**读 `meta.rollback`——回滚段存不存在只看正文
有没有 `## Rollback` 那一节。于是复制模板时最常见的形态（frontmatter 写着
`rollback: none`、正文留着 `## Rollback` 段）会执行作者**明确声明不要执行**
的逆操作（`delete_task` 等），可能删掉作者想保留的数据。

- 执行闸落在调度器：解析出 `(steps, rollback)` 之后，按 `meta.rollback` 决定
  这段回滚步给不给执行——一处闸管住三个失败出口（状态机中止 / allowed-tools
  违规 / 步骤失败）。
- 降级也补上了：SKILL-DSL 承诺「`rollback: auto` 时必须含 `## Rollback` 段，
  否则 `rollback: none`」，解析器此前没做。frontmatter 单独解析时看不到正文，
  所以降级放在「meta 与正文同时在手」的那一层（`start_skill`），判据抽成
  纯函数 `parse::normalize_rollback_meta`。
- 判据单源：段落标题判定仍走 `is_rollback_heading`（`## Rollback` / `## 回滚`
  都认），不再各认一半。

**测试**：`rollback: none` + 正文有回滚段 → 回滚步**不执行**、`rollback_attempted`
为 false；对照组 `rollback: auto` → 照跑（别把 auto 也掐了）；`auto` 但正文没有
回滚段 → 降级为 `none`（中英文标题都认）。

### P1-5 的落地口径

**工具预算触达要落 `budget_exceeded`**。此前只有墙钟与轮数两条熔断落
`BudgetExceeded`，工具熔断的正文用的是另一个常量（`FUSE_MARKER`），
`classify_outcome` 不认 → 落进 JSON 解析分支：模型照办输出
`{"status":"succeeded"}` 就报**成功**，没输出收尾 JSON 则报 failed +
`result_json_unparseable`——真实原因「预算触顶」两次都被丢掉。

父 agent 的提示词教它「failed / budget_exceeded / cancelled：先读 error 与
summary，**决定重试**」——既然烧满预算的子 agent 报的是 succeeded，父 agent
**不会重试**，产出实际残缺而无人知晓。

两处一起补：
- `classify_outcome` 认工具熔断标记 → `BudgetExceeded` + `max_tool_calls`
  （轮数标记是工具标记的前缀，所以判在轮数之后）。
- 收尾处读一次 `ctx.used_tool_calls` 与 `budget.max_tool_calls` 比：触达即
  强制 `BudgetExceeded`——不依赖模型自觉汇报（dispatch 侧预算闸到顶后拒绝
  新调用，模型可能被告知「停止新探索」后照办收尾，也可能没来得及收尾）。
  部分结果照旧存进 `result_json.summary`，父 agent 判断「该不该原目标重试、
  还是放宽预算」要用。

### P2-1 / P2-2 的落地口径

**P2-1：审计阈值与真实截断 cap 同源**。「工具输出超预算」事件原先在
dispatch 侧按 **per-tool 预算**报，而实际截断用
`min(per-tool 预算, 全局 cap)`（bot_model_loop 的 `effective_output_cap`）：
默认配置下真实 cap 是 30000（全局），长输出工具的 per-tool 预算是 65536——
**输出 40000 字节时文本被截了，审计却不报**；运维据此查超预算会漏掉全部由
全局 cap 触发的截断。dispatch 那段注释写的正是它要防的分裂（审计阈值和真实
cap 必须是同一个数），但没防住。

事件移到**截断发生的地方**（bot_model_loop 的 execute_tool 闭包）：阈值就是
这次真正用的 cap。dispatch 侧不再发这条事件——它只知道 per-tool 预算，
用它当阈值就是两边各说各话。

**P2-2：溢出指针不许承诺读不到的东西**。指针写「已**完整**转存到本地文件
…用 read_text_file 配 offset + limit 分页读」，但 `read_text_file` 只读文件
**前 100KB**（offset 是在这 100KB 的行上取的）——超过 100KB 的文件分页读只能
读到前部，再往后 offset 直接撞「超出范围」，尾部永远读不回来。模型的反应是
「文件被截断了」。

指针现在按文件规模说清**可达范围**：超限时补一句「单次读取上限 100KB：分页
读只能读到文件前部，后半段用 grep_files 按关键词定位」（grep_files 的单文件
上限 2MB，覆盖得到）。文件本身的内容确实是完整的——不可达的是读法。

**已知残留**：根治做法是让 `read_text_file` 支持「从 offset 起读的行窗」（不受
100KB 读入上限约束），那要动 TOCTOU 防护的读路径，本轮未做——本轮先把承诺
改成与可达性一致。

### P1-8 / P1-9 / P1-10 的落地口径

**P1-8：§6.4「完整列表」现在真被对拍了**。脚本原先只解析模块树的
`── xxx.rs` 格式，**没有任何用例解析 §6.4 的 `- \`path\`` 格式**——而 §6.4
自称「完整列表、由 audit_module_map.py 对拍」。里面躺着 3 条幽灵条目
（`eval/metrics.rs`、`eval/mod.rs`、`prompts/reflection.rs`——`src-tauri/src/eval/`
整个目录都不存在）与 26 条漏登记，脚本照样全绿。

- 清单按实际文件重建：155 条 ↔ 155 个源文件，幽灵 0、漏登记 0。
- 脚本新增两条用例：§6.4 每条必须指向真实文件；每个源文件必须在 §6.4 **列出**。
- `test_every_source_file_is_mentioned` 的判据从「全文任意位置提到」收紧到
  「§6.4 列出」——旧判据下那 26 个漏登记的文件在正文散文里被提到过，照样过审。

**P1-9：散文数字改为实测值，并让脚本盯住它**。文档开头原写「166 个文件、
约 8.9 万行，全部在 `src-tauri/src/`」——实测 155 个文件 / 112,456 行
（11.2 万行），文件数 −6.6%、行数 +24.3%，而且**写下时就是错的**。另
「全部在 src-tauri/src/」也不准（`src-tauri/tests/` 10 个、`examples/` 1 个
也在该 crate 内）。

新增用例 `test_prose_file_and_line_counts_match_reality`：解析开头那句里的
「共 N 个文件」「约 N 万行」，与实测值逐项比对。散文数字从此不再是「写下即错、
无人校验」。

顺带把模块树里 12 处过期的行数标注刷新到实测值（脚本本来就会报，只是这批
漂移还没被 push 门禁跑到）。

**P1-10：SPEC 附录的口径改成真话**。此前三处写着附录 A 是「v1 原始需求快照 ·
冻结不动 / 停止更新」，而它实际一直在改——内置工具清单从 61 改到 63 就是证据，
「冻结」这条自订口径被违反过至少 5 次。更麻烦的是结构：文档对外宣称
「现行节 = 真相、附录 = 考古」，而**唯一的机制详录（工具清单、端口闸、
资源告警）恰恰长在附录里**——读者按 `SPEC.md:3` 去现行节找 `install_skill`
找不到。

现在把两段的口径分开写清，并在附录里给活的那段立了个真标题
（「### 机制详录（活节，随功能更新）」）：
- 上半段 v1 原始需求：**历史快照，只读**；
- 下半段机制详录：**活节，随功能更新**（概览在现行节、细则在附录）。

### P1-11 的落地口径

**审计正文写「两份实现」，实际是三处**。它自己给的「5 份 max-files 文案」
这个数字就已经说明不止两处——第三处是聊天右侧栏（`ContextRail`）：它用
Rust 侧 `pick_files_dialog` 选文件（与前两处的 webview `dialog.open` 不是
同一条通道），合并基线同样是「点击时的快照」。三份里只有卡片侧修过并发，
另两处都还在丢并发绑定。

**一份内核，两个面**：
- `taskFiles::applyBinding(base, added, commit)`——合并 → 超限提示 → 落库。
  不依赖 React，判据单源（`mergeFiles` 的去重 / 文件夹槽替换 / 10 上限）。
- `useTaskFiles(task, commit)`（`src/lib/useTaskFiles.ts`）——React 面：内部持
  `taskRef`，`commit` 也走 ref（调用方多半传内联箭头，拿它当 `useCallback`
  依赖会让三个回调每帧换身份，memo 子组件白 memo）。

**合并基线一律取「await 之后的最新 files」——这条才是本项的真正内容**。
旧代码在点击那一刻就把 `boundFiles` 闭包进 `pickFile`：对话框挂起期间
另一路径（卡片 / 详情面板 / 上下文栏）绑进来的文件，会被后到的合并结果
**整列覆盖掉**（用户看到的是「另一个窗口绑的文件凭空消失」）。现在三处
都从 ref 取现值。

**上下文栏的选文件通道保持不变**（Rust 侧 `pick_files_dialog`）：那是
Rust `spawn_blocking` 弹框，聊天附件在用；卡片与详情面板走 webview 的
`dialog.open`。两条通道并存是既有事实，统一它是独立一条，不在本项范围。

**测试**：
- `TaskDetailPanel`：对话框挂起期间 `task.files` 被并发改动 → 合并基线取
  最新，两批文件都在。**这条已在旧实现上实测失败过**：旧实现把
  `/docs/concurrent.txt` 整列覆盖掉了（先写复现测试、再改代码）。
- `ContextRail`：同型复现（挂起期间 `tasks` 被并发改动）。
- `taskFiles`：两条源码扫描守卫——「每个任务最多绑定」文案全仓只有
  `lib/taskFiles.ts` 一份；组件层不直接调 `mergeFiles`（合并规则只在内核
  里一份）。这守卫防的是「下一个人又在一处复制一遍」。

**已知残留**：三处绑定的**选文件通道**仍未统一（webview `dialog.open` vs
Rust `pick_files_dialog`）；本项统一的是「合并 / 提示 / 落库」这一段。

### P1-3 的落地口径

**这一条原先不在 §六 的优先级表里**（审计正文有、优先级表漏了），本轮补在
最后落地。

**为什么没按审计建议的 `prctl(PR_SET_PDEATHSIG)` 做**：
一是它是 **Linux 专有**，macOS 根本没有——照建议做等于用户主平台一点没修到；
二是它按「父**线程**」算：受管服务是工具链用 **blocking 线程池** spawn 的
（`bot/tools.rs` → `spawn_blocking_map` → `do_start`），而 tokio 的 blocking
线程空闲一会儿就会退休——线程一退，服务就被内核 SIGTERM 掉。看门狗反倒成了
杀手。所以走「管道 EOF」这条跨平台的路。

**unix（macOS / Linux）**：每个服务起一个看门狗助手（`/bin/sh`，自成进程组）：
- 宿主持有管道**写端**，助手的 stdin 是**读端**；
- 宿主一旦非正常死亡（SIGKILL / OOM / 崩溃），内核关闭它的所有 fd → 写端随之
  关闭 → 助手的 `read` 收到 EOF → 它 `kill -TERM -<pgid>` 收掉服务整组；
- 管道两端都带 `FD_CLOEXEC`：助手绝不能继承写端（继承了 EOF 永远不来，
  看门狗就是个摆设）；
- 助手自成进程组：收它时不会连带宿主所在的组（宿主可能在别人的组里）；
- 服务停止 / 自己退出 → 看门狗从登记表移除 → 助手被 kill + **wait** 回收
  （不 wait 会留僵尸：助手是我们的子进程）。

**Windows**：复用沙箱层那套 Job Object（同一份实现，`py::runtime::win_job`），
加一个只要 `KILL_ON_JOB_CLOSE` 的构造——长驻 dev server 不该被内存限额误杀，
但宿主崩了必须跟着走：把服务进程塞进 job，宿主一死 job 句柄随进程关闭，内核
杀掉整组。

**退出兜底**：`lib.rs` 原来只在 `RunEvent::ExitRequested` 清理——那条路不是
每次都走（托盘直接退出、启动期失败等只发 `Exit`）。现在 `RunEvent::Exit`
也收一次，用一次性门闩保证「两条路只清一次」（重复清理会把已停的服务再报
一遍、审计也写两条）。

**测试**（`bot/services/watchdog.rs`，macOS/Linux 上可实测）：关掉写端
（= 宿主死亡）→ 服务进程组被收掉；宿主还活着 → 看门狗**不许**动服务；
登记表的 attach / disarm 配平。

判据上踩过一个坑：判「服务死了」**不能**用 `pid_exists`——服务是本进程的子
进程，被杀后未回收前是**僵尸**，而 `kill(pid, 0)` 对僵尸照样返回成功，会把
「已经死了」读成「还活着」。必须用 `try_wait()`（它顺手回收）。

**已知残留**：Windows 侧的 Job Object 路径**只做了交叉编译验证，行为未实测**
（本机无 Windows）；unix 侧依赖 `/bin/sh`（macOS / 主流 Linux 都有；缺了则
看门狗装不上，服务照跑，只是不带这层兜底——装不上不拦启动）。

### P2-7 的落地口径

**选了哪条路**：审计给的两条里取「让生产调它」，但**改的是函数的判据层级**——
`has_inline_secrets` 从「判类型化 `McpServerConfig`」改成「判**原始 JSON 条目**」，
`config/io.rs` 的迁移段改调它。

**为什么不按字面改成「io.rs 调类型化版」**：类型化版判的是**水合后**的配置，而
水合会把 keyring 的值灌进每台服务器的 `env`/`headers`——用它判，恒为 true，迁移
永远认为「文件里还有明文」，会反复迁移（且判不出「已剥离」这个幂等条件）。审计
正文自己也点破了这点（注释说「水合后的内存值不适用」）。所以真正的修法是**把判据
移到正确的层级**：`skip_serializing` 只挡序列化，不挡反序列化，原始文件里的
`env`/`headers` 读得出来，判据因此可以落在原始 JSON 上。

**语义逐条对齐（行为零变化）**：新判据与原来 io.rs 里那段孪生逻辑逐情况等价——
缺字段 / `null` / 空对象 / 非对象形态（写成字符串、数组）都**不算**内联机密；
任一段是非空对象即算。迁移行为与修前完全一致，这条是纯一致性修复，不改任何
用户可见行为。

**单测跟着搬家**：`secrets.rs` 里那条单测判的是类型化配置，签名一改就失去意义，
随判据一起搬到 `config.rs` 的测试模块（函数住哪，测试住哪），改成判原始 JSON
条目；顺带把「非对象形态不算内联机密」这一档也钉住。

**判据单源守卫**：新增一条扫描守卫，钉住「`config/io.rs` 的生产段必须调
`mcp::config::has_inline_secrets`，且不得再出现 `as_object().map(|m| m.is_empty())`
这种平行的 Value 级复制品」。扫描前先切掉 `#[cfg(test)]` 尾段——不切的话，把调用
点挪进 io.rs 的测试模块就能骗过守卫（同 `bot_model_loop.rs` 那条守卫的做法）。
**变异实测**：把 io.rs 的生产实现退回孪生判据，守卫如期变红（报「迁移判据必须调
mcp::config::has_inline_secrets」），确认这条守卫是承重的、不是空断言。

**设计文档三处口径**（`docs/MCP-KEYSLOT-MIGRATION-DESIGN-2026-09-29.md:35`、`:59`、
`:82`）改成「原始 JSON 条目级判据」——原先「`has_inline_secrets` 就是迁移判据」
这句话现在才真正成立。

**已知残留**：`secrets.rs` 水合路径里的 `s.env.is_empty() && s.headers.is_empty()`
**不是**迁移判据（它问的是「内存里有没有值 → 要不要回填 keyring」，判的是水合
后的类型化配置，方向相反），本轮不动。

### P2-3 的落地口径

**两条后果，方向相反**：① 变更声称闸只扫 `mutating == true` 工具的声称话术，
`screenshot` 不在其中 → 模型**不调工具**也能报「已截屏」，闸看不见；② 落盘凭证
闸查「这个路径被任何工具写过吗」，而 `screenshot` **不产 `file_changes`** → 模型
如实说「已生成截图 /path/x.png」时，路径在磁盘上（存在性那关过了）却查不到凭证，
闸反过来把真截出来的图判成凭空声称、要求模型重截。**同一个「写盘却当只读」的
根因，一边漏拦、一边误拦。**

**改法两处**：`registry.rs` 的 `screenshot` 改 `mutating: true` + 声称话术；
`bot_desktop.rs` 的截图成功路径改为产出**二进制档**凭证（路径 + 内容哈希、无行
diff——PNG 不是 UTF-8 文本，按行 diff 只会产乱码，与生成类文档同档）。

**声称话术为什么不收「已截图」**：`claims_mutation` 的第二阶段是「已」+ 8 字窗口
里找**剥掉「已」后的词干**——「已截图」的词干是「截图」，于是「**已查看截图内容**」
（用户传图让模型读图，模型用 `ocr_image` 看完后的如实说法）也会命中变更声称，
凭空把这一轮打回去。所以只收词干不含裸名词的三个：「已截屏」（工具自己的返回
文案）、「已保存截图」、「截图已保存」。这条有负例测试钉住。

**为什么把结果构造抽成 `screenshot_ok_result`**：截图要真屏幕 + `AppHandle`，
测试里跑不了；抽成纯函数后凭证能被直接断言（临时 PNG → 断言 `file_changes` 长度
1、路径一致、带内容哈希）。这属于「函数住哪、测试住哪」的对齐，不是为将来预留
的抽象——就这一个调用方 + 一个测试方。

**老实说副作用**：归入变更类之后，截图成功会置 `mutation_done`（模型如实说
「已截屏」不再被拦——这正是要的），并按变更类工具的既有节流触发一次自检（每 5 轮
最多 1 次，问模型「这次截图符合预期吗 Y/N」）。自检是既有机制、节流照旧，代价
是截图场景偶尔多一轮模型调用。

**测试**（3 条新用例）：截图在工具表里必须是变更类且话术能命中「已截屏 / 已保存
截图」；截图的声称话术**不得**命中「已查看截图内容」；截图结果必须带一条落盘凭证
（路径 + 哈希）。另：`mutating_tools_matches_baseline` 这条基线表随截图归类同步
补上 `screenshot`（这张表的作用是「变更类工具集合不许悄悄变」，不是把截图钉死在
只读档）。

### P2-4 的落地口径

**症状**：Windows 上 MCP 面板对 stdio 服务器显示「无监听端口 / 无资源占用」——而真相
是**没判**：`bot/procs.rs` 的进程快照层在非 Linux 平台走 `ps`，Windows 上没有这个
命令 → 采回来是空表 → `tree_sample` 返回 `None` → 内存 / CPU / 监听全空。
**空不等于没占用**：面板把「没判」显示成了「判了没事」。

**服务层早守住了，MCP 层没守住**：`services_snapshot_json` 早就带
`listeningSupported`（平台能力）+ `listeningSampleOk`（本轮采样成败），面板明说
「本平台暂不支持监听端口采集」；MCP 的 `McpServerStatus` 没有对应字段——同一件
事，两层两套口径。

**改法**：① `bot/procs.rs` 新增 `snapshot_supported()`（进程快照层能不能用——
Linux 走 `/proc`、macOS 走 `ps`、Windows 两者皆无），与既有的
`listening_supported()` 同一形状；② `McpServerStatus` 加 `listening_supported` /
`resource_supported` 两个布尔，`mcp_status` 从上面两个判据取值；③ 前端面板在
false 时显示「本平台暂不支持进程资源采集 / 监听端口采集」。

**标志挂在每台服务器上，面板只展示一次**：能力是**平台级**的，但 `mcp_status`
返回的是数组（每台一条）。后端给每台都挂同一份标志（省掉「面板级」那条另立契约
的改动），前端从首台取一次、面板顶部展示一次——每台服务器下面都挂一遍同样的提示
是噪音。

**边界（明说）**：这条只解决「**平台根本不支持采集**」。另一档「平台支持但**本轮
采集失败**」（macOS 上 `ps` 起不来）在 MCP 侧仍不可见——服务层有 `listeningSampleOk`
兜着，MCP 侧没有对应标志，本轮**没做**（不在审计给的修法里，且要动 manager 的采样
返回形状）。也就是说：Windows 上现在会说「本平台不支持采集」，但 macOS 上如果
`ps` 恰好起不来，面板仍会显示空值。

**测试**：Rust 侧一条**契约测试**（`McpServerStatus` 序列化出的键名必须是
`listeningSupported` / `resourceSupported`——改名会让提示静默消失，前端拿到
undefined 一律当「支持」）；前端两条（false 时显示提示、true 时不显示）。两侧合
起来把「后端改名」与「前端读错键」都钉住。

### P2-5 的落地口径

**根因不在「重连窗口」本身，而在「名字按在场顺序分配」**：挂载表每次 dispatch
miss 都**实时重建**（`mount::build_mounted_tools`），消歧序号（`_2`）是按**当前
在场顺序**给的。连接一断（重连窗口里槽被撤、或连接失败转 `Down`），这台服务器
就从集合里消失 → 后面的服务器序号前移 → **模型按旧 schema 调的名字落到了另一台
服务器头上**。触发面窄（同前缀 + 同工具名 + 断连在飞），后果是跨服务器静默错调。

**改法：给函数名加一本进程级登记簿**（`mount::NameRegistry`）：
- `(服务器 id, 工具名) → 函数名` **一经分配就不再变**；新分配时避开所有已用名
  （含别的服务器已占用的、以及别的工具恰好叫 `read 2` 这种真实名）；
- 名字一经分配即**占用**，不再释放给别的归属（服务器改名后旧名仍归它）——免得
  「旧名被别人接管 → 模型的旧调用落到别人头上」；
- **schema 构建与 dispatch 反查同用一本**（`mcp_tools_json_body` / `find_mounted`
  都走登记簿），两边不会分家。

**顺带修掉「工具集瞬时消失 → 未知工具」**：`find_mounted` 改为**先查登记簿**——
服务器当前离线也照样能按名字定位到它，随后 dispatch 走 `ensure_connected` 懒
重连（连上了就继续用这次调用），而不是把模型按旧 schema 调的名字报成「未知工具」。
为此把反查返回值从 `McpMountedTool`（带 schema）拆成 `McpToolRef`（不含 schema）
——路由本来就不需要 schema，而离线服务器也没有 schema 可给。

**没做的那条**：审计给的另一条修法「重连期间保留旧槽（先建新槽、成功后再换旧）」
本轮**没做**——同 id 的新旧两代进程会短暂并存（HTTP 传输可能撞端口、stdio 可能
双跑一份），代价大于收益。名字稳定性由登记簿解决后，「工具集瞬时消失」也不再
导致错路由：调用会落到**正确**的那台服务器上（离线则当场懒重连）。

**测试**：
- `names_survive_a_server_dropping_out`（核心回归）：同前缀两台，A 断连后 B 的
  序号**不得**漂移，旧名仍归 A。**变异实测**：把构建逻辑退回旧实现（按在场顺序
  重新编号），该用例如期变红（left `mcp_p_read` / right `mcp_p_read_2`）——确认
  这条守卫承重、不是空断言；
- `lookup_resolves_a_tool_of_an_offline_server`：服务器当前不在线时，仍能按名字
  反查到归属（离线也能定位）；
- 既有消歧用例（撞名、撞真实工具名、同输入同结果）全部保留，改走登记簿。

