# 真机全量测试报告：2026-10-24（M40–M74 全量行为卡）

对 `8887e50e`（10-24 10:51，工作区干净）这一版在 macOS 实机做的**全量**行为卡验收：按
`docs/realmachine-mutation-test-2026-10-24.md` 的 35 张 macOS 卡（M40–M74）逐卡跑行为判据。
**Windows 专章（W04–W06）按口径跳过**；**50 条变异注入全部未做**（本轮口径不改代码）；每张卡
的「变异注入复验」由 `scripts/mutation-trace-funnel.sh` 的单测层与既有守卫测试承担。

**结论：30/35 卡有实机结论（22 PASS · 7 部分/静态核对 · 1 已排除），4 卡未实机触发（守卫单测在钉）。
0 个本批回归缺陷；2 条真缺陷当天已修（D-9 `b5b996bf` Replan 解析兼容对象步骤、D-11 `c430bb7e` 脚本失败
原因进 stderr），1 条提示词纪律已补（`bef13ba3` 参数被拒禁止降级重试），2 处卡片已改（`118fc543`）；
1 条留观（run_python 可致解释器崩溃并弹系统崩溃窗，应用侧记账如实）。**

| 项 | 值 |
|---|---|
| 提交 | `8887e50e`（工作区 clean，与上一轮冒烟同一版） |
| 构建 | `npm run tauri dev`（stderr 可见），三轮启动（M48/M51 需要重启） |
| 数据目录 | `~/Library/Application Support/com.renshi.wmessage`（dev 兜底，`data_dir_fallback` 为既有口径） |
| 模型 | MiniMax-M3（anthropic 兼容端点），`permMode=ask`（确认弹窗全程生效） |
| 观测 | `regression-probe.sh` mark/report + bot.log 审计行 + 只读 sqlite（`file:...?mode=ro`）+ 截图 + cliclick/osascript |
| 驱动脚本 | `/tmp/mutest/wm-ui.sh`（置前校验 → 窗口坐标动态换算 → pbcopy+回车）；confirm 弹窗守望 `/tmp/mutest/m41-watch.sh`、`m64-watch.sh` |

---

## 一、逐卡结论（35 张）

### T0 档：安全边界与数据破坏

| 卡 | 结论 | 关键证据 |
|---|---|---|
| M40 敏感词 `pin` 整词匹配 | **PASS** | `ui_type` 实测：`Spinner`（含子串 pin）/`typing`/`shipping` 三框**都不被拒**且字符串真落框（Safari 截图地面真相）；整词 `PIN` 被拒——`拒绝操作：目标控件是敏感控件（PIN），不代填密码 / 验证码类内容。`；schema 副判据在 `registry.rs:370` 逐字在位 |
| M41 前台锁 | **PASS（部分）** | 前台闸拒绝原文**逐字命中**（12:58:13，Safari 不在前台时 `ui_type` 被拒：`目标窗口不在前台：键盘注入会落进当前正在使用的应用…`）；正向路径（activate_window → 置前成功 → 输入成功）成立；危险确认弹窗 60s 无响应默认拒绝 ×2（fail-closed 实证）；untitled 控件保守确认弹窗、界面已变复核闸（`控件…不在窗口…当前界面里`）均实拍。**未复现**：确认弹窗挂起期间切前台后再确认的锁内复检（模型编排不配合，弹窗竞态窗口 ~60s 内需三步联动）；洞二置前/注入并锁为并发形态——两半由单测 `injection_must_wait_for_the_target_window_to_come_to_front`（三态）与 `with_inject_lock` 结构钉住 |
| M42 程序 id 白名单 | **PASS** | `apps_create id="."` → `参数校验失败：id=.（id 只能是字母/数字/-/_（单个目录分量））`（WARN）；负向对照 `smoke-check-1024` 建成功，`apps/` 目录地面真相一致 |
| M43 工作区导入闸 | **部分（静态 PASS）** | 源码核对：`workspace_import`（workspace.rs:284-286）已接 `check_export_path_gate` + `read_import_bounded(MAX_IMPORT_BYTES)`。实机的导入文件对话框自动化未走通（native panel 与本驱动不睦），行为面未触发 |
| M44 标签双上限 | **部分（静态 PASS）** | 源码核对：`tasks.rs:1980` 注释与实现确认 UI 主写口收 `MAX_TAGS=10/MAX_TAG_LEN=30`（与 bot 口 :890-904 同常量），空数组清除语义同测。实机 UI 标签编辑器未驱动（11 连输需 ~22 次精确键盘编排） |

### T1 档：静默失效与状态谎报

| 卡 | 结论 | 关键证据 |
|---|---|---|
| M45 due 三写口归一 | **PASS** | bot 口三连：空格格式原样传入被拒、半截 `2026-10` 被拒（`截止时间格式必须是 YYYY-MM-DDTHH:mm`（WARN 逐字）；合法 `T` 格式建卡成功；库里无脏格式行。schema 夹具：due 说明 = `YYYY-MM-DDTHH:mm（T 分隔，空格分隔不合法）`，无空格示例。**模型行为注意**：第一轮模型把空格自行归一成 T（闸没被喂到），换「不透明字符串」话术后才原样传入；被拒后模型又自行去掉 due 重试建卡（截止格式验证3 due=NULL 的来历）——闸没漏，但「模型贴心重试」值得产品侧知晓。HTTP 口（可选半）未测 |
| M46 非法列值 + 完成三件套 | **PASS** | `create_task column=archived` → `column 只能是 todo 或 doing（收到 "archived"）`（逐字）；`edit_task` 同 → `column 不合法：invalid task status: archived`；`complete_task` 后库中 `col=done` + `completed_at` 有值 + `bot_assigned` 清空。**archived 复位半不可达**：bot 工具域不含已归档卡（`未找到…的未完成任务`），该半为防御分支，单测钉 |
| M47 技能启用开关 | **PASS** | 关 ppt-generator：frontmatter 首键 `enabled: false`（其余键完好）、无 `.tmp`、审计 `skill.enabled_changed`；三闸全验——`use_skill` → `技能「ppt-generator」已被禁用`、意图路由不触发（无 skill.start）、行置灰+tooltip 形态。重开：`enabled: true`、`帮我做个PPT` 被**路由进技能**（`skill_step_ok` + `skill_completed steps: 2`）——恢复成立 |
| M48 内置退役清理 | **PASS** | 播种 `builtin_skills/pptx-design/` 残留 → 重启 → 目录清空 + `builtin_skill_retired | skill=pptx-design` + `skills/` 自建技能无损；二次重启幂等静默 |
| M49 超时清扫覆盖 Running | **PASS** | 60s 超时技能在跑：另一会话 `use_skill` 两次被拒（`正在另一个会话/任务中运行`）；超时点后第三次启动成功，且清扫与启动**同秒**出现：`skill_expired | name: sweep-test | 等待确认超时，已过期并释放占位` + `skill.start`。「真死会话」（模型循环 Err 退出）无法在不破坏 LLM 配置的前提下安全制造——用「真 Running + 超时」触发的是同一条清扫路径，如实记录 |
| M50 MCP 启用不拉起子进程 | **PASS** | 配置即启用 → 面板 `MiniMCP · stdio · 0 个工具 · 待发现工具`（**pending** 新五态），`ps` 无 mini-mcp 进程；面板说明文案即「打开开关不会启动子进程——服务器静默待命」 |
| M51 工具清单落盘快照 | **PASS** | 面板「工具」按需发现：`mcp.tools_discovered | tools=1`，进程即起即收（ps 0）；`mcp-tool-cache.json` 落盘（fingerprint + echo 工具定义，内容核对）；**重启后**新会话模型直接调 `mcp_minimcp_echo` 成功返回 `echo:快照挂载验证`（无连接挂工具）且面板保持 `1 个工具 · 静默待命`；删除服务器 → `mcp.server_deleted` + 快照 `servers` 变空（无孤儿条目） |
| M52 预算触顶收尾窗 | **PASS** | max_tool_calls=1 强制触顶：`subagent_budget_tool_calls_exceeded | used=1 | max=1`（WARN）→ 读被拒且文案**逐字一致**（`⚠️ 工具调用预算已触顶，收尾窗内只放行写产物工具（write_artifact_file / write_file / edit_file）与 read_own_card；…`，审计 `reason=budget_wrap_up_deny`）→ `write_artifact_file` 放行落盘（run-log.md 4340B）→ `edit_file` 追加成功 → key-lines2.md 六行齐全 → 收尾 JSON status=succeeded。第一轮（预算 3）模型用 grep_files 一发覆盖三文件没触顶——收尾窗只在真触顶时验证，如实记录 |
| M53 删除/导入广播 + 导入导出审计 | **部分（3/5 半 PASS）** | `tasks_export | count=58 | path=…`（带 count 与 path）✓；非法 JSON 被响亮拒绝（错误框 `不是有效的任务数据 JSON：data did not match any variant of untagged enum TasksImportPayload`，非静默）✓；同构文件重导「导入完成：本次写入 0 条任务卡」且**无** `tasks_import` 审计——「0 条不写审计不广播」口径 ✓×2。**未走通**：imported>0 的写入行（合并语义下两次 0 条，第二次的 bump 疑似落在 subtask 层）与对应的广播观察——native 对话框自动化不稳，broadcast 半由守卫单测钉 |
| M54 重读并入 mutate 链 | **未实测** | 依赖 imported>0 的导入流程（见 M53）；竞态半由 `reloadTasks` 链上化的守卫单测钉 |
| M56 读失败不伪装空库 | **已排除（卡片要改）** | 按卡片配方 `alter table app_runs rename to app_runs_tmp` 后重开「运行历史」——显示「还没有运行记录」。查库发现 **app_runs 已被原地重建为空表**：`open_db` 的每连接 DDL（CREATE TABLE IF NOT EXISTS）把改名遗留就地补齐 → 读取「成功」返回 0 行，空态呈现是**正确**的。文档预警过「库级改名」会被 DDL 否证，实测**表级改名同样被否证**——这条注入手段在本构建上不可用，判据回到 `AppsPage.test.tsx` 单测（`.catch(() => setLoadError(true))` 分支）。表已恢复（5 条运行记录完好） |
| M57 问答应答持写锁 | **未实测** | 需要真造长事务窗口 + 机器人 ask_user 提问的时序交叠，单驱动冒烟无法编排；写锁接线由 `ask_engine` 守卫单测钉 |

### T2 档：可感知语义

| 卡 | 结论 | 关键证据 |
|---|---|---|
| M58 契约卡实时余量 | **PASS** | 子 agent 首轮 `read_own_card` 返回 JSON 完整落进产物：`budgetUsage{roundsMax:50, roundsUsed:0, toolCallsMax:100, toolCallsUsed:1, wallRemainingSecs:3599}` 五字段齐全且是**已用量**；**顶层无 `budget` 键** |
| M59 动作流实时上屏 | **PASS** | 子 agent 运行中右侧栏实拍：`✓ grep_files /private/tmp/… 7ms`、`✓ read_text_file /private/… 6ms`——一条调用一行、工具名可见、绿勾收口（return 不另起一行）。列表恒持 6 条的滚动截断由 `subagentActivity.test.ts` 钉 |
| M60 子 agent 行工具调用进度 | **PASS** | 三处实拍：`1/50 轮 2/100 次 28s`、`0/30 轮 0/3 次 23s`、`1/30 轮 2/3 次 58s`——轮数与工具调用**并排同报**，预算传参（max_tool_calls=3 / =1）准确生效 |
| M61 token 用量三列 | **PASS** | 本批新子 agent 行三列**有值**（completion_tokens=7708/9061, cache_read_tokens=76881/32963；prompt_tokens=0——该厂商把输入全记缓存列，非 NULL 即记账生效，如实记录）；老行（7 条）三列全 NULL，未倒填 |
| M62 预算纪律三条红线 | **PASS** | 行为面：`read_text_file args={"path":…}` **不带 limit** 一次读完（bot_fs.read | lines 1-3/3）；schema 夹具含「一次读完」。spawn 回执格式与 §1.6 一致（`生效预算：墙钟 600s / 轮数 30 / 工具调用 3`）。附带韧性证据：授权干扰下子 agent 用 grep_files 三段自取全文并交叉校验，产物零缺失 |
| M63 重试保鲜窗口 | **PASS** | DB 拨针（retry_at=3h 前）→ `sched_job_retry_expired | id: m63stale001 | 重试到点已超 2h 保鲜窗口，丢弃重试回到正常排期`（**逐字**）；retry_at/retry_count 清零、后续无重复噪声。**nuance**：丢弃 30s 后该作业跑了一次——`classify_due` 对 never-run（sched_last NULL）作业是「首跑即补」语义，与代码一致、非缺陷；卡片「回到正常排期」的语境是有基线的作业 |
| M64 防重入拒绝补审计 | **PASS** | DB 拨针在运行中再触发：`sched_job_skip | id: m64reentry001 | 上一次执行仍在进行，本次到点不触发` ×2（**逐字**），运行结束后才放行 |
| M65 错过作业补发 sched-status | **PASS** | DB 拨针（daily:09:00 + sched_last 昨天）→ `sched_job_missed | id: m65missed002 | schedule: daily:09:00 | 到点已超 2h 补跑窗口，跳过不补跑`（**逐字**）；定时页**未刷新**即出现新作业行与最新「上次」时间（sched-status 事件生效）。**badge nuance**：「已错过」徽标的 missed 标志由 Rust 端按查询时点计算，而消费发生在下一个 tick（≤30s）——实拍时消费已完成、missed 已回落 false，故无徽标；徽标窗口设计上就是瞬态，渲染逻辑由 `SchedulePage.test.tsx` 钉 |
| M66 完成判据归一 | **PASS（部分）** | 外部置 `col=done, completed_at=NULL` 遗留卡：KPI 完成率把它计为完成（37%→39.6% 联动）、任务池 29 不含它（isTaskCompleted 一致）；卡片状态行未实机点开（完成卡不入待办池，需经图谱/归档路径点开），该半由 `homeData.test.ts` 专门用例钉 |
| M67 导出文件名 + 图谱呈现 | **PASS** | 实拍导出对话框默认名 `wmessage-memories-20261024`（记忆）/`wmessage-tasks-2026-10-24`（任务）——均为**本地日期**；源码 `MemoryPanel.tsx:136` 用 `dayKey(new Date())`，`GraphPage.tsx:567` 用 `formatDue`。图谱键盘游标选中节点实拍：详情卡「完成于 完成 2026-10-19 05:2…」为格式化呈现（非裸 ISO）。**时段限制**：实机时 UTC+8 白天，本地日期==UTC 日期，时区差分不可辨别——但 dayKey 接线有源码+实拍双证 |
| M68 探活文案两条 | **部分** | 源码核对：`bot_test_connection` 按 `vendor.as_deref()` 分两支（无厂商 → 指路全局主 Key；有厂商 → 指路厂商页），`unwrap_or_default()` 拼空名已不存在。实机：加了无 key 的 Bailing 厂商（Ling-1T），连接测试的 toast 转瞬即逝未被捕获（盲点点击 + toast 生命周期 ~2s）。「无厂商老条目」当前 UI 已不可造（legacy 形态）。**残留**：Bailing 厂商留在配置里（可在厂商页删除） |
| M69 运行记录写口持锁 | **PASS（正常路径）** | 程序页跑 `冒烟检查`：绿横幅「运行完成 0.1s」+ 磁贴「上次成功·刚刚」+ `app_runs` 行 `status=ok` 终态（不悬 running）。锁竞态半（长事务窗口内回写）不可人工复现，由写锁守卫单测钉 |
| M70 六读命令阻塞线程 | **观察级** | 全程（含重日志、大列表、多页签切换）界面无冻结、输入即时；「点程序页卡死全应用」的反例形态未出现。`spawn_blocking` 守卫单测钉 |
| M71 清扫/批量落库阻塞线程 | **观察级** | 高强度对话 + 定时作业连发期间 trace 照常落库（bot.log 大量 trace 记录）、应用不卡；丢弃路径的 stderr 诊断在 dev 下可见性保留。守卫单测钉 |
| M72 schema 无工作流承诺 | **PASS** | 夹具全文 0 处「工作流」；`query_tasks` 输出行无标记（实机 48 行核验）；模型被问「列出带工作流标记的任务」时未编造、以实查回答 |
| M73 聊天任务池过滤单源 | **PASS** | 外部播种软删卡（deleted_at 有值）+ 遗留 done 卡后：任务页任务池与聊天右侧栏均保持 **29 个未完成**（计数不涨）、软删卡标题不在列表——isActiveTask 单源生效 |

（另：M51 的「调用完即收」即 W04 的 macOS 侧等价证据——进程拉起 → 用完 → `ps` 0；Windows 树杀半按口径未测。）

---

## 二、发现与观察

| # | 现象 | 级别 | 状态/建议 |
|---|---|---|---|
| D-9 | `plan.replan_failed | reason=内部错误：Planner 输出无法解析为计划：{...}`——Replan 模型把步骤写成对象（tool/parameters/note），`parse_plan` 只认字符串数组全滤掉 → 空计划必然降级 | 真缺陷 | **已修（`b5b996bf`）**：非字符串步骤按紧凑 JSON 原样入计划，null/空对象丢弃；四条新测试（含实机样本）钉住 |
| D-10 | 机器人 `run_python` 执行模型自写的 ctypes/AX 脚本可让 **Python 解释器崩溃**（SIGTRAP on `AXUIElementCopyAttributeValue`），并弹出 macOS「"Python" 意外退出」系统崩溃窗打扰用户；应用侧如实记账（`py_exec done exit=1`、error.log ERROR「子进程异常终止（被信号杀死）」），宿主不受影响 | 观察级 | 既有能力面（非本批回归）。可议：对含 AX/FFI 特征的 run_python 代码加提示或软闸；系统崩溃窗的打扰无法由应用侧消除 |
| D-11 | `extract_document` 对不可提取类型（.txt）报 `提取失败：内部错误：提取失败：`——脚本把「不支持的格式」打到 stdout、stderr 为空，Rust 侧拼出无因错误 | 真缺陷（既有） | **已修（`c430bb7e`）**：四处脚本失败分支统一走 stderr（与 pdf 渲染脚本既有写法对齐）+ 源码级回归锁防新增分支回流 + `script_fail_err` 空 stderr 显式说明；真 python e2e 复现测试在位 |
| O-1 | M56 的实机注入手段（表改名）被 `open_db` 每连接 DDL 否证（表被原地重建为空表） | 已排除 | **卡片已改（`118fc543`）**：M56 卡注明库级/表级改名双双不可用，判据回单测 |
| O-2 | 模型「贴心重试」会绕过参数闸的原始意图（due 空格式被拒后自行去参重试建卡；M52 第一轮用 grep 一发合并绕开触顶） | 观察 | **底座提示词已补（`bef13ba3`）**：参数被闸拒绝时转告原因并问用户，禁止换参/丢参降级重试。写实机卡仍需「不透明字符串/禁止重试」外壳 |
| O-3 | 前端 `ResizeObserver loop` js_error ×28（重 UI 操作时段） | 噪声 | 已知浏览器伪影，不计 |
| O-4 | 授权弹窗（文件访问/工具确认）60s 无响应即拒绝——本轮两次因编排超时吃到，行为正确但对**自动化驱动**不友好（人手节奏需 <60s） | 观察 | fail-closed 符合预期；复跑手册可提示「弹窗出现后 60s 内必须响应」 |
| D-12 | 气泡原样显示 `⚠️ 大模型 API 错误 402：{"type":"error","error":{"type":"insufficient_balance_error","message":"insufficient balance (1008)"},"request_id":"072bb75e…"}`——上游错误体（给开发者看的 JSON）直接进用户界面，看不出该充值还是换模型 | 真缺陷 | **已修（`721aaff9`）**：LLM API 错误正文改「状态码指引 + 上游自己说的那句」——402 说「账户余额或额度不足：请到厂商控制台充值，或在设置页切换其他大模型」，原始 JSON 不再外泄（前端兜底提示也不再说「稍后重试」，余额类错误重试无用） |
| O-5 | 上面这条 402 的成因是 **MiniMax Token Plan 额度耗尽**（1008），不是应用缺陷：紧邻的 trace 177（PPT 素材稿）单轮烧掉 26079 completion tokens 后配额见底 | 账户条件 | 当时激活模型是 MiniMax-M3（anthropic 协议）；应用侧能做的已做（说人话 + 不误导重试），恢复靠充值或在设置页切 DeepSeek V4 Flash（Bailing/Ling-1T 无 key 且未过连接测试，厂商可用性闸会拦） |

## 三、测试残留与状态

- **库**：新增 3 条调度测试作业已删（count=0）；测试任务卡（截止格式验证×3、列值验证系列、导入更新卡、定时作业产物、M66 遗留卡、M73 软删卡等）按「测试阶段不做数据迁移」口径保留同库；`subagents` 表三列迁移为结构性变更。
- **文件系统**：`skills/sweep-test`（M49 靶）、`apps/smoke-check-1024`（M42/M69 靶）保留；`builtin_skills/` 已被 M48 清空；`mcp-tool-cache.json` servers 已空；`AI_Gen_Files/` 下新增子 agent 产物与 wm-ui 前后截图；仓库目录的导出文件已删，`git status` 仅剩两份报告文档。
- **配置**：`allowedDirs` 增 `/private/tmp`（授权弹窗「始终允许该目录」所加）；新增 Bailing 厂商（无 key，可在厂商页删）。
- **应用**：dev 进程仍在跑（后台任务 exec_1d531525…），数据管理页留着一枚 OK 弹窗未关。
- **error.log**：193 → 270 行。增量 = ResizeObserver×28 + extract_document 空 detail×3 + run_python 信号杀×1（即 D-10）+ wait_for_element 超时×1 + 各类预期 WARN（闸拒绝留痕）。零 panic、零应用层错误。
- **PRAGMA integrity_check** = ok。

## 四、未覆盖（诚实清单）

1. **W04–W06**（Windows 专章）——按口径跳过；W04 的 macOS 侧「调用完即收」已由 M51 顺带覆盖。
2. **50 条变异注入**——口径不改代码；单测层变异由 `scripts/mutation-trace-funnel.sh` 与各守卫测试承担。
3. **M54 / M55 / M57 实机面**——分别依赖「import>0 导入流」「浮窗+命令面板叠层的精确 GUI 编排」「ask_user 与长事务交叠」，本轮驱动方式够不着；三者的核心分支均有守卫单测（`reloadTasks` 链上化、`TaskDetailPanel` Esc 裁决两条、`ask_engine` 写锁）。
4. **M41 锁内复检的弹窗竞态形态 + 洞二并锁**——单驱动无法稳定复现并发/竞态（见该卡行）。
5. **M45 HTTP 写口、M66 卡片状态行、M68 实机 toast、M53 import>0 广播**——见各卡行注记。
