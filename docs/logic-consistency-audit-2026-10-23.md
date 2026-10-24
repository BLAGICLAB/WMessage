# wmessage 全量逻辑与一致性审计（2026-10-23）

范围：全仓再次审计，切法与 10-22 分模块轮不同——按四个增量价值面铺五路并行只读审计：
**批 A 修复批次回归**（d79207c2..87e4acc2 共 22 个代码提交逐 diff + 现码全读）、
**批 B 跨层契约一致性**（命令/事件/类型/schema/常量双端全量对拍）、
**批 C 文档↔代码账实**（SPEC/架构文档/门禁文档/实机文档/两轮审计账本/DEVLOG/代码注释）、
**批 D Rust 横切接缝**（锁与事务纪律、广播完整性、审计口径、状态机、async 纪律、时间与常量）、
**批 E 前端逻辑**（竞态/串行链/三渲染器口径/日期工具单源/监听清理/错误链）。

分级沿用 10-21 口径：P0 破坏安全/数据边界；P1 功能断裂或承诺与代码背离；P2 局部缺陷（有实际破坏面）；P3 打磨。
**P1 全部与 P2 中的 11 条由主会话逐条读现码复核后才记为事实**（复核清单见各批标注）；其余 P2/P3 为子代理报告（均附双侧行号）。全程只读，未改任何代码。

基线说明：
- 起始基线 87e4acc2，`test-all.sh` 全量门禁 exit 0（nextest + vitest 全绿）。
- **审计期间并行会话将 HEAD 推进到 cd060bcd**（24 个提交：流式回灌、子 agent 预算治理、context-rail 文件改动面等，与本轮发现面基本不重叠；工作区另有并行会话未提交的 AGENTS.md / tests-audit 改动与未跟踪的 mutation 文档）。本文全部代码面发现按 cd060bcd 现码复核，未受推进影响；cd060bcd 之后未重跑全量门禁。
- 10-22 轮的 F 组 11 处待定性与未修 P3 存量（~70 条）本轮**未重查、未计入**，已知清单原样有效。

---

## 批 A · 修复批次回归（22 提交：20 个修法成立且有测试锁，2 处遗留问题如下）

### [P1-A1] ui_type 前台闸被确认弹窗自身打穿——判定与注入之间隔着最长 60s 的 await，且互斥范围没闭合（主会话复核）

- 位置：`src-tauri/src/bot_ui.rs:645`（前台校验）→ `:660-671`（gate_dangerous，确认弹窗最长等 60s）→ `:672-686`（截图 + 窗口复核 + 注入，**无前台复检**）；`ui/macos.rs` focus_node=AXPress 不激活应用。
- 触发：`detail_is_dangerous` 命中即走确认——detail 含**要打的文本本身**（前 60 字），打字助手的典型输入（「把报告发送给老板」/英文 "send"/"ok" 整词）经常命中。用户必须点 wmessage 的确认窗 = **wmessage 必然成为前台**；确认返回后只复核窗口标题（:673 注释自认「确认弹窗可能等了很久」却只补了身份复核），HID 键盘事件路由给当时的前台应用——最可能是 wmessage 自己（文本打进机器人聊天框）。
- 关联缺口：`INJECT_LOCK` 只罩 focus+type 两行（:680-683），`activate_window`（:406）不进锁、前台校验也在锁外——并发会话 B 的 activate_window(Y) 可落在 A 的「校验通过→注入」之间，A 的文本进 Y 的聚焦控件。非确认路径同样有窗口期（截图 + 全量窗口枚举 + 锁排队期间用户切应用）。
- 定级依据：d61707df 的承诺「注入前校验目标窗口在前台，判不出来也不注入」在确认路径上结构性不成立——承诺与代码背离。
- 修法：前台判定移进 `with_inject_lock`、在 focus 之后注入之前复检一次；activate_window 与注入段同锁；判定材料与注入之间不允许存在任何 await 点。

### [P2-A2·账实] M4 P2-1（validate_app_id 放行 `"."`）未修，账本却称「P2 二十条全部落地」（主会话复核）

- 位置：代码 `src-tauri/src/apps/manage.rs:129-138`；账本 `docs/module-audit-2026-10-22.md:627`。
- 事实：现码仍只拒 空串/`/`/`\`/`..`——单个 `"."` 全不命中照常放行；`db/paths.rs` `apps_root.join(".")` 折叠到 apps 根；归档后「彻底删除」走 `remove_dir_all` 先递归删光**全部**程序目录再对 `.` 报错。bot 三件工具复用同一弱校验，LLM 可独立触达 create→delete 链。修复表实列 18 条 + 授权模型另计 = 19，账本却写 20 全落地——**账实不符**。
- 修法：id 收窄为 `[A-Za-z0-9_-]+` 白名单 + 一条 `id="."` 用例；账本行更正为 19/20。

### [P3] 其余回归面

- **死会话的 Running 技能仍永久占位**（`bot_skills/runtime.rs:70-80` Running 无条件拦截、`:51-62` sweep 只扫 Paused）——b1ac8348 只修了 Paused 一半：会话循环带 Err 退出后同名技能被永久拒绝直到重启。修法：Running 同套超时判定，sweep 扫一切超时非终态。
- **633e7111 panic 兜底可吞掉已承诺的重试**（`bot_scheduler.rs:725-760`，retry_at 落库后 panic 窗口极窄）——方向可接受的取舍，建议注释如实声明。
- **bot_test_connection 无厂商路径报错文案误导**（`bot/config/commands.rs:858-866` `unwrap_or_default()` 空厂商名拼进提示）。
- storage.test.ts:298 格式（b2ad135a 顺手弄乱一行，接入 prettier 会红）。

已查证无问题（20/22 修法成立且攻击未穿透）：bac7f803 事务真原子（merge/rollback 都走 with_write_conn + 同事务，回滚凭据不丢）；2a4d5fd1 守卫置位/清位覆盖 unwind；Held 水位语义分毫未变；2h 清扫=4×1800s 无误扫且四条失败路径全覆盖；48e96dad 全部 key 读取点收口、无带鉴权探活残留；5c5b6e93 安置不了就中止写盘；afe7f47b 活服务返回内存 token 不被顶掉；e07b7a0d 渐进 JPEG/多帧拦得住；992d7c5c 双闸闭合；stableStringify 键序无关实现正确；c23bc5d7 十口子零命中常驻守卫；1c9ea780 其余各点（flag 0600、小写+整词、角色判定、虚拟桌面换算、无标题 fail-closed）独立成立；d020b6c7 正反路径正确；48b53c9b/8572b5d3 双端对拍有测试锁。

---

## 批 B · 跨层契约一致性

### [P1-B1] 截止时间（due）契约三写口三口径，schema 教的格式恰好是全链路唯一不认的（主会话复核）

- 位置：schema `bot/registry.rs:192`「YYYY-MM-DD 或 **YYYY-MM-DD HH:mm**」（空格分隔）；唯一校验 `db/tasks.rs:839-854` `validate_due` 只认 `YYYY-MM-DD` / `YYYY-MM-DDTHH:mm`（T 分隔）；消费端 `due_notify.rs:316` 空格分隔解析为 None 静默不发通知；bot 写口 `bot/tools.rs:479/:792` 只 `check_len(MAX_DUE=30)`；HTTP `api_handlers/handlers.rs:252/:564` 同样只限长。
- 后果：模型**照 schema 传空格格式 → 落库 → 到点通知静默不发**——正是 SPEC §任务「写入即校验……解析不了的截止会被当成无截止」写明要防的事故。同一个值经 UI 的 task_patch 通道被 400 拒：同字段两写口行为相反。92769b45 修了 task_patch 却没跟 bot/HTTP 两个写口，schema 反过来在教错。
- 修法：bot create/edit_task 与 HTTP create/update 复用 `validate_due`（或先把空格归一成 T 再校验）；schema 文案改 T 格式。

### [P2-B2] query_tasks / query_single_task schema 仍承诺已退役的「工作流/定时」输出（主会话复核）

- `bot/registry.rs:182`「输出行带（工作流：名称）标记」、`:188`「创建时间/**定时**/**所属工作流**」——实现（`bot/tools.rs:263-281/:361-456`）早在 ccac16e2（10-15 退役画布工作流）就不产出这些字段，schema 没同步。模型被系统提示词告知可用不存在的标记回答工作流问题 → 得出「无工作流」的错误结论或凭空编造。`tools.rs:286` 注释同样残留。
- 修法：两个 SCHEMA 常量 + 注释删掉工作流/定时承诺。

### [P2-B3] tags 上限三写口三口径：task_patch（UI 主写口）完全不校验（主会话复核）

- bot 写口有 `MAX_TAGS=10`/`MAX_TAG_LEN=30`（`bot/tools.rs:803-821`），HTTP 引用同常量；**UI 主写口 `db/tasks.rs` `"tags" => set_from(...)` 零校验**，前端也只有 TagConsolidateModal 一处校验。UI 写入超长标签后：该行再经 bot 提交新 tags 被同一常量拒绝；选它做整理主标签必被拒。`tags_consolidate.rs:273-274` 注释「写路径按 MAX_TAG_LEN 拒」与事实不符。
- 修法：apply_task_patch 的 tags 键补双上限校验，三写口归一。

### [P3]

- **B4** bot complete_task / edit_task(column→done) 不清 `bot_assigned`、不设 archived=false（`bot/tools.rs:560-564/:842-844`），与 task_set_column（`db/tasks.rs:719-725`）和 HTTP（`handlers.rs:469-474`，注释明说三件套语义）口径不一致——执行收尾有独立清理兜底，仅并发窗口残留机器人头像。
- **B5** bot 侧 column 非法枚举静默吞：create `"done"`→todo（`tools.rs:494-497`），edit `.parse().ok()` 跳过（`:835-837`）——模型以为建成完成卡。
- **B6** sched-status 前端类型含后端从不发射的 `"skipped"`（`SchedulePage.tsx:99`）；missed 只发系统通知不发事件——「已错过」徽标要等 reload。
- **B7** 敏感词 `"pin"` 子串误伤（Spinner/shipping/typing 被当密码框拒输）——危险词侧 "ok" 有整词匹配，pin 没享受同等待遇（`bot_ui.rs:76-85/:270`）。
- **B8** delete_task 确认、ui_click 无标题 fail-closed 确认的行为 schema 未声明（方向安全，模型缺上下文）。

### 已查证对齐面（完整对过、零漂移——引用价值）

命令契约 **151 项双向全对齐**（无「调了未注册」、无「注册即死」）；事件 **17 种 Rust emit 全有前端监听**；`db::Task` 26 字段 ↔ types.ts 逐字段一致（wire 形状有测试锁）；ScheduleEntry/ProfileView/WorkspaceItem 双端一致；头像 16 枚举双端一致；bot UI 五闸 schema↔实现全对齐（拒换行/小写整词/角色判定/前台要求/窗口复核）；tags 整理契约（NEW_MAIN_PLACEHOLDER、30 上限）双端一致有对拍测试；数值口径抽查全对齐（limit/行数/条数/超时/截断/clamp 一串）；HTTP 广播链闭合（emit tasks-updated source=api → App 合并）；五源广播（main/bot/api/migration/initial-load）形状一致；标完成三件套在 task_set_column 与 HTTP 同口径。

---

## 批 C · 文档↔代码账实（按 cd060bcd 对拍）

### P2（关键事实错）

| # | 文档侧 | 代码/事实侧 |
|---|---|---|
| C1 | `rust-bot-architecture.md:102/:490` 列 `prompts/reflection.rs` | 文件不存在，全仓无 `REFLECTION_SYSTEM_PROMPT`（幽灵两处；对拍正则只认标准树格式，缩进行漏网） |
| C2 | 同文档 `:364/:464/:572` 写 `apply_reflection` | memory 门面只有 `save_summary`（`memory/mod.rs:790`） |
| C3 | 同文档 `:592`「实际 157 个 .rs」 | 实际 159，其下清单即 159 条，与 `:3` 自相矛盾 |
| C4 | 同文档 §2「关键类型位置」表（:388-402） | `db.rs`/`bot/config.rs` 已拆分不存在；IntentRule/TaskStore/ToolDef/TOOLS_TABLE/AppState 等行号全漂——整表失效 |
| C5 | 同文档 `:237` subagents 表 `parent_task_id` | 实为 `parent_session_id`（`db/subagents.rs:176/:206`） |
| C6 | 同文档 `:13/:407`「8 张执行期表」 | AppState 实 14 字段，自家表只列 10 行 |
| C7 | `realmachine-regression-test.md` 附录 D services.rs 行号靶点 10 处 | 全部漂移（`note_sample_status` :667→:670、RSS :1710→:1857 等）；**27ccf452 的「校准」漏了 services.rs 整族**（70b15cff 时全准） |
| C8 | `realmachine-overflow-test.md:286` `py/runtime.rs:762` | 实在 :900/:915——**从写下即错**，非后来漂移 |

### P3（措辞/数字漂移）

- SPEC.md 会话栏宽 200px（:80/:94）与 224px（:66 及实码 `SessionRail.tsx:33`）新旧并存。
- 架构文档 perm_mode 写三分支（:296-297）实四档含 Auto；web_search 只写 Bing（:309）实 Tavily/Brave 可配 + Bing/百度兜底。
- testing.md：工单号防线口径不全（实为 staged+unstaged+untracked 三源扫描）；测试例数快照过期（1539/857 vs 现 ~1922/~1049·99 文件）。
- test-all.sh:21 自报「独立守卫脚本 2 个」实跑 1 个（另一个已随 2325d9fc 退役）。
- AGENTS.md 两处计数：rerun「先跑修过的那 6 项」实 7 项；「32 项场景卡」实 34 张 S 卡。
- DEVLOG「接在四个地方」实列三组（代码 7 个调用点）。
- **注释漂移新 5 条**：memory/mod.rs:126「无设置页 UI」——MemoryPanel 已有检索参数卡；bot/tools.rs:1「20 个 tool_*」实 22；api_handlers 头「5 个命令」实 4；migration/commands.rs:3「7 个」实 11；bot.rs:5「6 个 re-export」实 7。

### 已核对一致（抽样面）

SPEC 数值契约群（工具数 63、输出上限、token 32768、端口 4763、cosine 阈值、apps 滚动 100 版、Python 30 分钟、图谱 bloom 参数、周网格参数等数十项全中）；§6.4 清单 159↔159 双向零幽灵零漏登记、23 条行数标注全对；testing.md 步骤/豁免表与脚本对上；三份实机文档引用路径全存在、非 services 靶点全中；agent-audit-10-21 的 24 项「已修」抽查 14 项全在位；agent-audit-10-19「未修 0 条」自洽；DEVLOG 抽查 4/5 逐字节一致。

---

## 批 D · Rust 横切接缝

### P2

- **D1 ask_engine 两处锁外直写 DB**（主会话复核）：`ask_engine.rs:111-115`（问答应答 respond_core）、`:352-361`（提问通知落库）——spawn_blocking 里 open_db 直写 notifications 表，不取 DB_WRITE_LOCK。主窗导入/大事务持锁超 2s busy timeout 时：**用户点了选项但写入静默失败**，引擎按假设继续。修法：闭包内补 `lock_db_write()`。
- **D2 workspace.rs 三处裸锁未随 2a4d5fd1 改造**（主会话复核）：`:172/:187/:292` 仍 `lock_or_recover(&DB_WRITE_LOCK)` 不置 HOLDING_DB_WRITE 线程标记——同文件其余写口都已改 `lock_db_write()`，唯 workspace 三处漏网；当前闭包只写 workspace 表未炸，标记纪律已破。修法：三处换 `lock_db_write()`。
- **D3 workspace_import 缺路径闸门 + 无大小上限**（主会话复核）：`:283-296` path 直达 `read_to_string`，无 `..` 拒绝、无符号链检查、无 64MB 上限——export 侧拒的路径 import 侧照读（`check_export_path_gate` 全仓仅 3 调用方，注释声称五处共用不实）。修法：入口补闸门 + bounded reader。
- **D4 app_runs 全部写口不持 DB_WRITE_LOCK**（主会话复核 grep 证实）：`apps/mod.rs:104/195/231/282` 与 `apps/commands.rs:219-221`（读接口内嵌清扫 UPDATE）。持锁冲突时 finish 失败 → **程序跑成功却对用户报错**，行被 2h 清扫标成「进程中断（未回写结果）」——历史里说谎。修法：四处写口进锁，读与清扫分离。
- **D5 SchedGuard 拒绝路径零留痕 + 24h ask 叠加吞 daily 周期**（主会话复核）：`bot_scheduler.rs:409-411` `else { return; }` 无审计——同函数其他全部跳过分支都有审计。叠加 ask 等待上限 24h：daily 作业触发后 ask 挂起期间守卫不释放，**次日同作业被静默吞掉**，用户以为定时坏了查无痕迹。修法：补 `sched_job_skip | 上一次执行仍在进行` 审计行。
- **D6 apps 读侧 async 命令直接同步 DB IO**：`apps/commands.rs:26/:36/:145/:162/:225` 五处在 async 体内 open_db（每次全套 DDL+PRAGMA）+同步文件操作——同文件写侧全是 spawn_blocking+锁，读写纪律分裂，tokio worker 被磁盘 IO 阻塞。
- **D7 trace 清扫与 trace_sink writer 在 async 任务体跑同步 DB 全套**：`db/trace.rs:973-992`、`trace_sink.rs:344-411`——持锁期间整个 worker 阻塞。修法：loop 体改 spawn_blocking。

### P3

- **D8 暂停作业恢复瞬间立即触发过期重试**（`bot_scheduler.rs:829-831` + `:791-800`）：重试无保鲜窗口（对照正常到点有 CATCHUP_WINDOW 2h），暂停数日恢复即立刻跑一次本该 5 分钟内重试的执行。
- **D9 睡眠双时钟错位**：运行超时用 Instant（睡眠不走）、清扫判龄用墙钟 `started_at`——合盖 >2h 再开盖，活跑被 recent_runs 清扫误标「进程中断」，脚本完成后又覆盖回 ok，先假失败后自愈。修法：清扫前校验 pid 仍活。NEEDS-VERIFICATION：mach_absolute_time 睡眠行为依 OS 版本需实机验证。
- **D10 db_delete / tasks_import 是仅有的不发 tasks 事件的 Rust tasks 写命令**（`db/tasks.rs:1087-1102/:1396-1429`）——当前前端调用方自己补发闭环成立，协议上「后端落盘必广播」在此两处断裂，未来任何后端复用即静默漏发。
- **D11 tasks_import/export 与 workspace 全部写口无审计事件**——同为数据面变更（导入可一次写入任意量、导出整库外流）却无「谁何时导入了什么」可查痕迹。

### 已查证无问题（攻过守住）

with_write_conn 全部 21 处闭包无重入；33 处事务体内无全局写锁再取、无嵌套事务；锁序恒为 DB_WRITE_LOCK→BOT_LOG_LOCK 单向无死锁；journal 裸锁有边界注释支撑；无跨 .await 持锁；upsert 系契约断言调用方全走 lock_db_write；受管服务双停并发全交错推演无 Stopping 卡死；技能冲突闸不永久占位（Paused 已修，Running 见 A 批 P3）；ui_automation_toggle/tag_merge 新审计事件转义口径正确；onion 导入名字白名单不可注入；audit_log 164 调用点抽查用户可控值均过转义。

---

## 批 E · 前端逻辑

### P2

- **E1 reloadTasks 绕过 mutate 串行链**（主会话复核）：`App.tsx:789-796` 直接 loadTasksFromDb→setTasks，不并入 mutating.chain（SYNC-1 注释自述合并后「全窗口严格串行」——reloadTasks 是仍未并入的第二条路径，与已知 applyRemoteRows 同族）。导入完成时若链上有在途 mutate 停在 await 窗口：mutate 恢复后用**导入前旧基线**无条件回写 tasksRef——弹出「导入完成 N 条」但界面无新卡（DB 未丢，UI 回退，自愈时机不定）。修法：主体并入 chain；顺带 reloadTasks 套规则不落盘与 handler 的 ruleChanged 落盘路径不一致，同因收口。
- **E2 聊天侧栏任务池漏滤 deletedAt**（主会话复核）：`App.tsx:313` 过滤条件 `column !== "done" && !archived` 无 `!t.deletedAt`——**回收站软删卡仍出现在聊天「待办任务」**，可被引用进对话、勾完成（把回收站里的卡置 done）、绑文件。任务页任务池用 isActiveTask（含 !trashed）——同语义两个入口口径不同。修法：换 isActiveTask 单源谓词。
- **E3 TaskDetailPanel 全局 Esc 无层级裁决**（主会话复核 handler 与冒泡路径）：`TaskDetailPanel.tsx:192-198` window keydown 无条件关浮窗，`:306-311` 标题 Esc 还原分支不 stopPropagation。三组必现：⌘K 开命令面板再按 Esc → 浮窗同时被关（saveOnClose 开=草稿意外提交；关=onDiscard 把浮窗期间已落库改动回滚到打开快照）；标题编辑态按 Esc 想只还原标题 → 整个浮窗连带提交 note/tags 被关；浮窗内开 TracePanel 按 Esc → 直达 window 关掉宿主浮窗。修法：Esc handler 加层级裁决 + textarea 分支 stopPropagation。

### P3

- **E4 首页「今日排期」完成判据绕过 isTaskCompleted 单源**（`homeData.ts:33-41` 用 completedAt，同页池计数/KPI 用 column）——遗留 done 无 completedAt 卡同屏两处归属互斥（今日排期算未完成、池存量算完成），恰是该单源 docstring 要防的第三处漏网。
- **E5 done 无 completedAt 卡三渲染器矛盾**：TodoCard:242 对勾（column 判）vs :659-661 状态行「未完成」（completedAt 判）同卡自相矛盾；PlanBlock/WeekGrid 用 column——三渲染器对同一行结论不一致。
- **E6 导出文件名日期后缀两套时区**：MemoryPanel 用 UTC toISOString（UTC+8 晚 8 点后日期大一天）vs 任务/工作区用本地 dayKey。
- **E7 GraphPage 详情卡绕过 format 单源**：due 裸串原样、completedAt 用无参 toLocaleDateString——同字段任务卡是格式化呈现、图谱是 ISO 原串。
- **E8 AppsPage 两处子列表失败伪装空态**（`AppsPage.tsx:492/:555` catch→[]）——违背全仓「失败≠空库」纪律（loadTasks/Schedule/Activity 都有错误态），运行记录/版本读失败显示「还没有」。
- **E9 坏 due 读侧三口径**：详情面板严格校验回退空串 / 老卡 includes("T") 原样塞 datetime-local / 首页 NaN 侥幸滤掉静默消失——同一行坏数据三处三种表现（写入口修好后此项自动降为纵深防御）。
- **E10 杂项**：homeData.ts:10 引用已删除的 localDateStr；原生 alert 三处与 ErrorDialogHost 两套视觉语言；TaskPool DoneCircle 恒 false 硬编码无注释。

### 已查证无问题（攻过守住）

mutating.chain 模块作用域跨 render 共享、StrictMode 不失效；mutate 失败不分叉；tasks-updated handler 已并链 + source 协议宁多写不丢；useTauriListen 全调用点无 stale closure；GraphCanvas 全套监听/RAF/Observer 清理配对；拖拽 AbortController 统一清理；TaskEditorOverlay 退场竞态防重开；discardEditor 只回写差异且 botAssigned 不回滚（不可逆外发）；useTaskFiles 合并基线取 await 后现值；Schedule/Notifications/Activity 竞态令牌防慢旧覆盖；ChatPanel 子 agent 轮询终态自停+序号去重闸；归档天数前后端同钳制；parsePlanDT 构造回读校验拒非法日历；DueTicker 取模兜底；isDueToday 三处同函数；无任何 HTML 拼接 DOM 口子；avatars 未知 ID 回退；useConfirm 不可用默认拒。

---

## 汇总与拍板清单

**总计：P0 0 / P1 2 / P2 21 / P3 29**（P2 含账实专项 8 条文档项）。10-22 轮的 F 组 11 处待定性与 ~70 条未修 P3 存量不变，未计入。

主会话复核覆盖：P1 2/2、P2 11/21（A2/B2/B3/D1-D5/E1-E3；C 组 8 条为文档对拍双侧行号即证据、D6/D7 为 grep 级证据）；P3 为子代理报告附行号。

修复建议按收益分组：

**A. 两条 P1 + 一行修（最优先）**
① ui_type 前台复检移进注入锁内、activate_window 并锁（A1）；
② due 三写口归一（bot/HTTP 补 validate_due 或归一化空格）+ schema 文案改 T 格式（B1，顺带 E9 降级）；
③ validate_app_id 白名单 + `id="."` 用例 + 账本行更正 19/20（A2）。

**B. 锁纪律四连（D1/D2/D4 + D3 闸门）**——ask_engine 两处、workspace 三处裸锁、app_runs 全写口、workspace_import 路径闸与上限。全是「主窗长事务期间静默丢写/说谎」同族。

**C. 调度与后台纪律（D5/D6/D7/D8）**——SchedGuard 拒绝补审计（叠加 24h ask 吞 daily 是真用户可见事故）、apps 读侧/trace 清扫 spawn_blocking、重试保鲜窗口。

**D. 前端三连（E1/E2/E3）**——reloadTasks 并链、任务池换 isActiveTask、Esc 层级裁决。

**E. schema/口径订正（B2/B3 + B4-B8）**——工作流残留承诺、tags 三写口归一、complete_task 三件套对齐、column 响亮拒绝、sched-status 死枚举、pin 整词、确认行为入 schema。

**F. 文档账实一次收（C1-C8 + P3 群）**——架构文档四幽灵/过期表、回归文档 services.rs 行号族、溢出文档行号、SPEC/testing/AGENTS/DEVLOG 计数订正、注释漂移 5 条；可合并一两个 docs commit。

**G. P3 择优**——死会话 Running 技能超时、D9 清扫前验 pid、D10/D11 广播与审计补口、E4/E5 完成判据单源化、E6-E8。

---

## 修复批次 · 2026-10-23（当天闭环）

按基线口径逐条修：先落复现用例（红）→ 修 → 变异复验（改坏→红→改回）→ 单独提交（fmt 解堵提交 c2298582 除外——上一批遗留的 rustfmt 漂移挡住了所有提交）。

| 编号 | 缺陷 | 修法 | 提交 |
|---|---|---|---|
| A1 P1 | 前台校验排在确认弹窗之前，确认后不复检；activate_window 不进注入锁 | 前台判定移进 with_inject_lock、focus 后注入前复检；置前与注入同锁 | `3efa1e25` |
| B1 P1 | due 三写口三口径，schema 教的空格分隔格式全链唯一不认 | 判据提为 db::tasks::due_format_error 三口共用；bot 侧 due_param_denied、HTTP 侧 check_due 接线；schema 两个文案改 T 格式；fixture 重生成 | `ec0e25c7` |
| A2 P2 | validate_app_id 放行 "."（目录折叠回程序根） | 收白名单 `[A-Za-z0-9_-]+` + `id="."` 与黑白名单用例；10-22 账本更正 19/20 并注明 | `009e811f` |
| D1 | ask_engine 两处锁外直写 notifications | 闭包内补 lock_db_write | `e3604e71` |
| D2 | workspace 三处裸锁不置线程标记 | 换 lock_db_write | `d6766145` |
| D3 | workspace_import 无路径闸、无大小上限 | 接 check_export_path_gate + read_import_bounded（从 tasks_import 提出共用，上限参数化+用例） | `7d66f8a6` |
| D4 | app_runs 四写口不持锁（回写失败=跑成功却报错+历史说谎） | 四处进锁 | `25ed736b` |
| D5 | SchedGuard 拒绝零留痕（24h ask 吞次日作业） | 补 sched_job_skip 审计行 | `edf3c763` |
| D6 | apps 六读命令同步 IO 跑 tokio worker | 全部包 spawn_blocking（含清扫） | `f2ee3f7b` |
| D7 | trace 清扫与 sink writer 同步 DB 全套跑 async 任务体 | loop 体/批写段包 spawn_blocking，连接随闭包进出 | `07280b91` |
| D8 | 暂停数日恢复瞬间立刻跑过期重试 | classify_job 增 RetryStale（保鲜窗=CATCHUP_WINDOW），消费时清 retry_at+审计；边界用例+变异复验 | `6f06f6ed` |
| B2 | query schema 残留工作流/定时承诺 | 两 SCHEMA + 注释删除；fixture 重生成 | `6b441c16` |
| B3 | task_patch tags 零校验（三写口三口径） | 补 MAX_TAGS/MAX_TAG_LEN 双上限 + 空数组=清除（与 bot/HTTP 约定归一）；用例 | `d86656c1` |
| B4+B5 | complete_task/edit 缺三件套、column 非法值静默吞 | Done 补 archived=false+清 bot_assigned（两处）；create/edit column 响亮拒绝 | `20187e41` |
| B6 | sched-status 无 skipped、missed 不发事件 | Missed 分支补发 `{jobId, phase:"skipped"}`（前端 handler 原生兼容） | `b88238a5` |
| B7+B8 | pin 子串误伤、确认行为 schema 未声明 | SENSITIVE_WHOLE_WORDS 整词豁免（用例）；ui_click/ui_type schema 写明确认行为；fixture 重生成 | `d2efb15f` |
| E1 | reloadTasks 绕 mutate 串行链（导入行从 UI 消失） | 主体并入 chain（SYNC-1 同款双分支推进） | `af1d959c` |
| E2 | 聊天任务池漏滤 deletedAt | 换 isActiveTask 单源谓词 | `286720db` |
| E3 | 浮窗 Esc 无层级裁决 | escCtxRef 裁决（trace>palette 让位>关浮窗）+ 标题 Esc stopPropagation + escBlocked 透传 | `a808778c` |
| Running 技能 | 死会话 Running 永久占位 | sweep 覆盖超时 Running + 冲突闸放行超时项（更新既有用例+新用例） | `921f3477` |
| D10+D11 | db_delete/tasks_import 不广播；导入导出无审计 | 落库补 tasks-changed；import/export 补 Info 审计（count+转义 path） | `20165f0e` |
| E4+E5 | 完成判据三处绕单源（同卡同屏矛盾） | planPhases/TodoCard 状态行/详情面板状态行全换 isTaskCompleted；测试更新 | `0bdbebfb` |
| E6+E7 | 导出日期 UTC/本地双实现、图谱详情裸 ISO | MemoryPanel 换 dayKey；GraphPage 换 formatDue/formatCompletedAt | `42262c9f` |
| A-P3 ① | 探活文案拼空厂商名、指错方向 | 无厂商/厂商两分支文案 | `331f7ad2` |
| A-P3 ②③ | panic 清重试取舍未声明、storage.test 格式 | 注释写明取舍；格式复原 | `3cf2c02d` |
| E8 | 运行/版本读取失败伪装空态 | 失败态与空态分离（loadError） | `607e0fe6` |
| E10 部分 | TaskPool 对勾恒 false 无说明 | 注释写明口径 | `cb72cd50` |
| C1-C6 | 架构文档幽灵/错名/计数/类型表漂移 | 删 reflection 两幽灵、apply_reflection→save_summary、160 计数（并行会话 MCP 批 +1 文件）、类型位置表行号校准、parent_session_id、14 张表、perm_mode 四档、web_search 全描述 | `aa2e2b70` |
| C7+C8 | 回归文档 services.rs 行号族 10 处、溢出文档错靶 | 全部按现码校准 | `4eb341ad` |
| C-P3 群 | SPEC 200px、testing.md 例数/工单口径、test-all 自报、AGENTS 两计数、DEVLOG 一计数、注释漂移 5 条 | 全部对齐现码（Rust 1965 例/前端约 1050 例 100 文件） | `47d93b61` |

**未修（拍板项，非遗漏）**：
- **D9 睡眠双时钟**（活跑被清扫误标「进程中断」）——正解要 pid 出 run_python（现有 PyRunResult 无 pid）或 app_runs 加列，超出 P3 择优范围且审计标 NEEDS-VERIFICATION（mach_absolute_time 睡眠行为依 OS 版本）；建议随下次动 py/runtime 时一并拍。
- **E9 坏 due 读侧三口径**——写口修后仅纵深防御，等有存量脏数据再统一读侧。
- **E10 alert 三处统一**——视觉语言变更，需拍方向。
- **F 组 11 处待定性**（10-22 轮遗留）与 **C 组缓存基线**等存量照旧。
