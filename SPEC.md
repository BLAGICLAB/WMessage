# WMessage — SPEC v2（现行产品规格）

> **活文档**：本文件记录产品**现行**形态，功能进/改/删随同一提交更新
> 「功能规格」一节（开发基线有此约定）。v1 原始需求快照（2026-08-13 ~
> 08-18）原文保留于文末附录 A，不再修改。架构细节以
> `docs/rust-bot-architecture.md` 为准。

## 开发基线（单人口径，2026-10-08 收敛，同日按 vibe coding 通行要素进化）

本项目是单人 vibe-code，不跑团队流程。约定如下：

- **门禁只留一条**：pre-push `scripts/test-all.sh`（cargo nextest + vitest）；
  pre-commit `scripts/test-fast.sh`（fmt/check/tsc/knip/oxlint/vitest 智能跳过）。
  批次 spec 门禁（BATCH_SPEC / batch-verify.py / docs/batches/）已废除。
- **注释纪律**：工单号（W8-ATTACH、OCR C5、B0-1、AUDIT-*…）、日期决策记录、
  「拍板」过程**只进 DEVLOG，不进源码**；注释只写代码本身说不出的约束，
  不画横幅分隔线。test-fast 第 0 步机械拦截（误伤时行内加 `audit-ok`）。
- **DEVLOG 降级**：git log 是事实记录；DEVLOG 只写 git log 写不下的
  「为什么换方向」，一事一段，不写小作文。
- **SPEC 活文档**：功能进/改/删随同一提交更新「功能规格」一节；
  附录 A（v1 快照）冻结不动。
- **docs/ 口径**：顶层只放长期设计文档与手册；一次性审计/交接/冒烟/工单文档
  直接进 `docs/archive/` 或不落盘。
- **代码卫生（anti-slop）**：死代码随发现随删，自己孤儿化的一切随同一提交删除；
  不为「以后可能」预留抽象（trait/config/中间件）——第二个调用方出现时再抽象；
  不吞错误（空 catch / unwrap 绕错 / as any 逃逸）；密钥只走 keyring/环境变量。
- **提交纪律**：动手前工作区保持干净 commit（checkpoint）；小步提交，
  一个逻辑变更一个 commit；行为变更换测试，修 bug 先写复现测试；
  新增依赖给一行「stdlib/现有依赖为何不行」；提交前人眼看一遍 diff——
  门禁只懂规则，不懂意图。
- **代理协作**：AI 代理入口是根目录 `AGENTS.md`（一屏内，只放代理推不出的
  信息：命令、禁令、地图）；它落后于本节时以本节为准并同步修正它。

## 产品定位

单机个人 AI 任务助手：Todo 看板为骨架（单主窗口 + 托盘常驻，Windows 10/11 +
macOS），内置大模型机器人管理任务，向上长出子 Agent、任务图谱、
自进化观察态。无账号体系；多人协作 = 任务 JSON 导出汇总导入。

## 功能规格（现行，2026-10-15）

### 1. 看板与任务卡

- **任务页 = 单人周时间网格**（2026-10-10 改版）：列=周一~周日、行=8:00–18:00，日历惯例轴；三列看板（待办/今日/完成）退役，`column` 数据契约保留（归档/回收站/图谱/bot 不变）
- 排期：右侧毛玻璃任务池列出全部未完成任务（统一标题色块，色=首标签六色板），拖到网格写 `planStart`/`planEnd`（默认 60 分钟，30 分钟吸附）；块可移动/底缘拉伸/拖回池清除；时长溢出 18:00 折入次日渲染（物理跨午夜时间）；**双击**网格块/池项（含池底已完成区）开全局毛玻璃编辑浮窗（单击纯拖拽面防误触；计划时间编辑、标记完成、删除、子任务、备注、标签、截止）；⌘N 新建直开浮窗
- **任务编辑浮窗**（2026-10-13）：新建/编辑统一为全局毛玻璃上层窗口（`TaskEditorOverlay`），fixed 悬浮于导航栏右侧内容区之上、从左滑入/向左滑出（240ms），任何导航页打开都在**当前页上层**滑出——导航栏「新建任务」/⌘N/`quick-add` 不再切看板，⌘K/图谱/首页/聊天 `edit-task` 跳转也不切视图，浮窗开在原页面上；Esc/✕/删除关闭（带退场动画），浮窗开着可继续操作页面（双击其他块/池项原位换任务不重播动画）
- 计划块 v2（2026-10-12，小样 `docs/design-mock/task-card-density-mock.html` 为准）：首行 = 归属头像最左 + 完成对勾最右（顶隙 4px，12px 视觉 + 扩展热区），标题第二行起、行数按块高 1/2/4/6 封顶（矮块 <1 小时退化为「头像 · 标题 · 对勾」同一行）；时间行 = 时长「N 小时」在底部左侧（矮块与塌缩块不显示）；同列双卡重叠各占半区（narrow 紧凑排版）；同簇 3 卡以上整簇塌缩——所有卡只显头像（上）+ 对勾（下）上下排列（尺寸随块高 16/12/10 三档，无标题无时间，双击块身开详情），保持各自分栏几何；已完成任务原位保留在原时间区间（整块降饱和 + 对勾变绿，无划线），点勾恢复待办
- 已归档卡原位封存保留（2026-10-14）：网格准入 = 未删（归档不再从时间泳道消失），归档卡以封存态留在原时间区间——虚线边 + 红色「已归档」印章（薄块平贴、塌缩簇退化红点），降饱和高于已完成，无拖拽/无对勾/不可完成；悬停提示「已归档 · 双击去归档区查看」，**双击跳归档区定位该卡**：滚动居中 + 紫边框标记（`--plan-violet`，脉冲两拍），目标卡被搜索/标签筛选挡住时自动清筛选，定位态保留到下一次交互（点卡/改筛选/切页即清）
- 任务卡（归档/回收站沿用）：标题/备注/标签/子任务进度/文件绑定（多文件）/截止时间/折叠/归属头像（人完成→用户头像，交给机器人→机器人头像）；截止日=当天的待办自动进今日（数据规则不变，主窗口不再显示列）
- **文件绑定语义**（2026-10-16 统一内核 `upsert_files_to_task`，任务卡 UI/聊天右侧栏任务池行内「绑定文件」/bot 三渠道同规）：查重主键=路径（小写归一，同名不同路径视为不同条目）；**文件追加制**（同路径不重复）；**文件夹槽替换制**（每卡至多 1 个文件夹，绑新文件夹占旧槽原位，满员也可换槽不占新名额）；每卡总量 10 封顶。bot 渠道（edit_task files 参数 / link_file_to_task / 产物弹窗确认）**只增不减**——空 files=无操作，清空/解绑仅 UI；模型来源绑定的文件与文件夹都必须在 AI_Gen_Files 白名单内（canonicalize 校验），is_dir 一律以真实 metadata 校正；link_file_to_task 双分支：任务执行流程内登记→完成弹窗勾选落库，普通对话直接落卡（必须命中恰一张未删除卡：taskId 直查或 title 精确匹配唯一，不能新建卡，同名多卡改用 taskId）
- **子任务卡**：详情面板子任务行悬停「＋」一键生成为任务卡——新卡进待办、`parent_id` 记母卡 id（`tasks.parent_id` 列，普通卡 NULL；仅关联键，无级联/归属语义，不走 task_patch）；原行保留并换成「↗」跳转标记（点击打开该生成卡的编辑浮窗；生成卡被彻底删除后点击静默不动）；行尾「×」删除该行——已生成子任务卡时同时两边解除关联（child `parent_id` 清空）但**保留子卡**（退化为普通卡）；＋/↗/× 同排悬停显现；删除有存活子卡的母卡时弹询问——「一并删除子卡」（子卡同规则软删+清调度）/「仅删母卡」（子卡保留，parentId 悬挂不清理，母卡恢复后关联仍成立）；任务图谱为 parent 关联画连线（母→子，两端被过滤/悬空丢边，计入度数可救孤儿）
- 自动归档（天数可配置，默认 7 天）：归档页搜索+标签筛选+三列网格、只读可恢复；归档时大模型自动打标（≤3 个）
- 回收站：软删除；绑文件任务彻底删除时三选项弹窗（文件移废纸篓 / 只删卡 / 取消）。「移废纸篓」逐文件带卡 id 调 `delete_bound_file`：路径仍被别的卡片或工作区引用时拒绝并指名持有者（绑集是全局的，不判归属会把别人的引用一起变成死链）
- 多人汇总：任务导出 JSON 按 id 并集合并（updated_at 新者胜）；`owner_id` 归属字段；信封 profile 卡携带头像 ID（预设 avatarId），导入端落 people 表；任务页默认只看自己的任务，图谱与统计看全部
- 定时任务卡 ⏰：一次/每天/每周/每月四档，到点自动交给机器人执行，结果前置「⏰ 自动执行」写备注；一次性执行完自动清除，错过补跑 2h 时效窗口
- **首页 = 主窗口落地仪表盘**（2026-10-11）：吸顶收缩头部（姓名取设置个人资料用户名；滚动进度驱动问候/摘要/完成率进度条与 KPI 收成单行钉顶，spring 平滑且反向连续还原，`prefers-reduced-motion` 时免缓动；小字只统计今日计划/今日完成，进度条=今日完成率，实心段+斜纹余量）；首行 = 必应每日一图（16:9 比例整图显示；`bing_daily` 命令：cn.bing.com 官方 API，当天缓存于 `runtime/bing/` 只拉一次，保留 7 天，base64 data URL 展示，点击外跳大图，失败友好占位+重试）+ 其下「即将到期」单行横条（逾期优先，多条每 4s 轮播、悬停暂停，点行跳编辑态，高度补齐与周图列的缺口）｜本周计划/完成柱状图（跨两行，周一..周日两组柱，当天 accent 高亮）｜通知模块（半高不随行拉伸）；第二行 = 图谱速览（buildTaskGraph 同源数据 + FA2 布局 SVG，点节点跳编辑态）｜本周日程（任务页周网格迷你版，segsForWeek/layoutLanes 同源）｜任务卡分页器（映射任务池：本周 7 天各一页 + 未排期池共 8 页，‹ › 换页）｜机器人动态右列通栏；末行 = 归档脉搏（本周归档/自动打标率/归档阈值 + 标签分布 chips，映射归档页统计）｜最近归档（最近 10 张归档卡，按 updatedAt 倒序，点行跳归档页编辑态）｜机器人动态右对齐任务卡列；**Linked Hover 跨四组件**：共享 hover 状态（日期 + 任务卡），周图橙色竖向参考线、日程列参考线与 Dim、任务卡自动翻到该天页、图谱节点高亮/Dim，其余数据降透明；头部 KPI = 待办/完成/归档三池真实存量；数据 30s 轻轮询 + 通知变更广播
- 主窗口：左导航多视图（首页/任务/日程/归档/工作区/回收站/任务图谱/活动/通知）+ ⌘K 命令面板；主窗关闭=隐藏，托盘常驻

### 2. 机器人聊天入口（挂件已退役）

- 侧边磁吸挂件窗口整体退役（2026-10-15）：收起态商标方块、边缘吸附、贴边位置记忆、任务/工作区/聊天三视图全部下线；聊天能力收进主窗口，任务与工作区本来就只在主窗口有，不再重复一份
- 聊天改为左导航「Agent 能力」组下的「聊天」导航项，点开占满主内容区（与定时/活动/通知/程序同级）
- **聊天常挂载**：切视图只切可见性不卸载——`execute-task` / `chat-open-session` / `chat-focus-session` / `subagent-finished` 全靠组件内监听器驱动，条件挂载会让切走页面期间的事件无处可去
- 跨入口跳聊天统一走 `chat-focus-session`：带 `sessionId` 时 ChatPanel 切到该会话，空载荷只切视图（托盘「打开聊天」、任务卡 🤖 按钮、⌘K 命令面板共用）
- 托盘菜单「打开主窗口 / 打开聊天 / 退出」：聊天项把主窗口推到最顶层并发空载荷事件
- 聊天区不再有「选任务模式」与任务引用块（原依赖挂件任务列表选卡，主窗口聊天页无卡可选；把任务交给机器人走任务卡 🤖 按钮）
- 机器人总开关关闭时聊天页不给可用界面（ChatPanel 跳过全部对外 invoke），开关状态由 `bot_get_enabled` + `bot-changed` 广播驱动
- **聊天页两栏布局**（2026-10-15 改版）：左侧常驻会话栏（200px）+ 右侧聊天主体。会话栏取代原「顶部药丸按钮 + 点击展开下拉」——下拉一旦折叠，入口就退回成一个不起眼的控件，用户反馈「对话下拉选择框不明显」；常驻侧栏是 ChatGPT / Claude / Poe 的通行做法

### 3. 机器人聊天与模型

- 多会话聊天：流式回复、思考/工具调用折叠、Markdown 渲染、斜杠命令 /stop /compact /retry /clean、删除确认（60s 超时自动拒）、审计日志 bot.log
- **危险操作确认单一呈现者**：高危动作与文件访问授权只由 `ConfirmMap` 全窗弹（60s 倒计时 + 拒绝/确认；白名单外路径三选一含「始终允许该目录」）。确认可能在任何视图下发生，聊天窗只切 hidden 不卸载，弹窗放聊天窗里会不可见；两个呈现者会让同一请求弹两次并在其中一个答完后留下点不动的残影
- **会话栏交互**（2026-10-15 改版）：栏顶独立「新建对话」主按钮；当前会话用 `nm-inset` 底 + 品牌色竖条 + 实心图标三重标记（不只靠底色——色觉障碍与低对比屏下底色差异不足），`aria-current="page"`；历史会话带相对时间、悬停才让位给删除键（未显形时禁指针事件，列表不抖动也不占点击区）；栏底会话计数
- **会话栏可折叠**（2026-10-15）：折叠钮只在聊天头部左侧一个（侧栏收起即卸载，栏内再放一个既会消失又与头部钮构成双入口）；折叠态存 localStorage `chat-rail-open`，默认展开——收起来只为窄窗与专注阅读
- **右侧上下文栏**（2026-10-16）：占聊天主体的 1/3 宽（`flex-[2]` : `flex-1` 的 2:1 分剩余空间，min 240 / max 420 兜底极端窗口），默认收起（存 localStorage `chat-ctx-open`）。侧栏是**聊天主体的兄弟节点**，不是 header 的子节点——塞进横向 header 会变成 header 的 flex item 把整行拉垮。顶部**图标分类切换**三块工作面，切类别不打断对话（工作面在侧栏，主对话留在原位），遵循 Agent workbench 的「对话锚点 + 工作面」范式：
  - **任务**：未完成任务，排序复用 `sortPoolTasks`；**点标题 = 把卡引用进当前消息**（同附件口径：输入区上方出可移除芯片，发送时正文带 `[引用任务]` 块 + 消息挂 `refs` 渲染成可点的卡引用，已引用的项在侧栏标出），**不是**立刻让机器人去做——那是任务卡 🤖 的 `runWithBot`，两者的时机与代价完全不同；勾 ✓ = 切 `column: done` 走 `updateTask`
  - **文件**：当前会话 `bot-file-changed` 累积，按 path 去重取最近统计 + ±行数
  - **子 Agent**：按 `parent_session_id` 拉 `list_subagents`，在途转圈；六态分别显示（完成 / 失败 / 已取消 / 预算耗尽 / 排队中 / 执行中），在途行带停止钮（`cancel_subagent`，key 传 `sa_` 行 id——子会话不在会话列表里、停止键够不着它）；`subagent-finished` 到达即重拉（子 agent 行只有这两处可见，不重拉就一直停在 running）。行按 ZCode 单行摘要渲染：`状态图标 · 目标 · 已跑轮数/预算 · 运行时长`，聊天内嵌区与右侧栏共用 `SubagentRowLine`。轮数取 `list_subagents` 顺带回带（内存仪表 `tool_guard::subagent_ctx`，与 `check_subagent` 同口径，终态读不到即留空不报 0），因此在途时按 `SUBAGENT_POLL_MS`（3s）重拉以推进轮数；时长在前端算（`started_at`/`finished_at`，毫秒），在途每秒刷新、终态定死。点条目（聊天内嵌区或右侧栏列表）把**被点的那个** `sa_` 行 id 钉进右侧栏并切到只读详情（`SubagentDetailPanel`）：目标 / 状态 / profile / 起止 / 用时 / trace，以及 `result_json`、`acceptance_json`、`context_summary`、`error` 四块原文（长于 220 字默认折叠、标题恒显）。详情内不放干预入口——在途唯一的取消留在列表行上贴着状态图标；行 id 状态归聊天面板而非右侧栏内部，两个展示点因此能点开同一份详情
  - 「终端」「审查」未做：项目内无后端命令也无数据源，空壳入口会变成永远点不动的死 UI

侧栏折叠钮同样只有聊天头部右侧一个（`chat-ctx-open`），与左栏对称。
- **阅读带限宽**（2026-10-15 改版）：消息区与输入卡共用 760px 居中带——通栏时正文行长在宽窗口下会拉长到难以扫读
- **聊天头部**：当前会话标题常驻（切到历史会话后靠它确认自己在哪一段对话里，不靠侧栏高亮反推）+ 正在回复指示点
- **空会话引导**（2026-10-15 改版）：图标 + 说明 + 三枚可点击起始短语，点一下填进输入框不自动发送（措辞仍由用户过目）——空会话是唯一能低成本教用户上手的时刻
- **一次执行 = 一个新会话**：任务卡 🤖、定时任务执行统一走聊天会话
- 双协议：OpenAI 兼容 + Anthropic 兼容；模型中心：厂商分类、内置模型库、厂商级 key（系统 keyring）、每模型推理参数与推理强度、可用性门禁、聊天内快切模型
- **截断自愈**（2026-10-15）：Anthropic 模式响应撞 `max_tokens`（思维链吃光预算，正文与排在最后的工具调用一并被砍）时，主循环拿已生成正文作前缀自动续跑至多 2 轮并拼接成完整答复，配额用尽才退回「回复因长度限制被截断」提示；默认 max_tokens 8192 → 32768
- 工具（45 个，清单见「内置机器人聊天」一节）：任务 CRUD/子任务/绑定、文档生成（Word 修订模式：.NET OpenXML 优先 + Python 兜底；Excel 公式注入过滤；PPT；PDF）、文件编辑（edit_file/write_file，Aider 式三级匹配）、文件读写+grep、本机 Python 沙箱（run_python，独立临时目录+60s 超时+产物回收）、Python AST 静态探查（ast_inspect，标准库 ast + 零新增依赖，符号树/定义/引用/调用链四种模式；调用前先过 bot_fs 白名单闸，只支持 .py）、联网（web_search：Tavily/Brave key 可配，未配置降级 Bing+百度抓取；fetch_url 公网白名单）、图片识字 ocr_image、截图直达模型视觉、电脑辅助 Tier1（reveal_path 等原生四件）、时间、记忆工具、use_skill、ask_user（任务执行中提问：手动执行任务卡，通知中心异步问答 + 假设兜底 + 全局预算 1–5 默认 3；聊天直接文字提问不用此工具）
- **Word 模板双层口径**：设置页上传 .docx（管纸张/页边距/页眉页脚）+ 文字排版参数旁车 json（管正文/主标题/一二三级标题的字体字号行距缩进对齐，上传即从模板正文提取，启发式单点），参数记录可在面板编辑（含改名）；生成按参数构造版式，旁车缺失自动重提取；默认模板缺失时走内置版式
- **语义记忆体 v2**：bge-small-zh 本地嵌入+混合打分；事实+教训两类；自动抽取、参数可调（设置页记忆区「检索参数」卡：注入预算/topN/recentN/lessonN/容量/衰减/去重阈值 8 项，留空=默认）、总开关、管理面板、导入导出、写入冲突裁决（改口即更新）、定时整理（consolidation）、黄金集评估器（recall@5 锁死）
- **MCP 外部服务器**：stdio/HTTP 接入，env/headers 机密走系统钥匙串
- **技能系统**：Markdown+YAML frontmatter DSL（对齐 Agent Skills 开放标准），步骤推进状态机 + LLM 兜底，设置页运行结果徽章
- 产物统一落 `AI_Gen_Files`（启动预建、同名加 `(n)` 永不覆盖），流程结束产物登记表弹窗汇总

### 4. 编排与执行

- **子 Agent 编排**：生命周期/并发闸/预算上限/状态机/runner，结构化字段路由。子 agent 不建任务卡，契约（目标 / 验收标准 / 上下文摘要 / 预算 / 状态）全在 `subagents` 行上，取消信号是行状态 `cancelled`（子 agent 每轮可 `read_own_card` 重读）；**预算软收敛优先于硬顶**——轮数与工具调用两条预算都在用掉 7/10 时注入收尾提醒，撞顶后已跑完的那部分正文照样落进 `result_json`（部分结果不丢弃，父 agent 回灌拿得到 summary 而不是空壳）；**轮数口径单源**——「轮」一律由循环按真实轮次自报，不数 LLM 请求事件（一轮里可能重试 / 自动续写 / 终稿自检，数事件必然算大），请求数另存一列作成本口径；主聊天与子 agent 的失败轨迹也照常上报已跑出的轮数与 token（不因撞熔断而记成 0 轮）；`check_subagent` 的进度在子 agent 在跑时报「已跑 N / 预算 M 轮」，与验收进度并存——验收标准支持机器可判定写法：`file_exists:<相对路径>` / `file_nonempty:<相对路径>`（相对该子 agent 产物目录）由服务端核验并覆盖子 agent 自评，未通过项在最终正文点名；不带前缀的仍是自由文本、由 verifier 逐条自评；进度统计只认派发时声明过的标准，虚报或重复上报不计入达成数；用户取消后的收尾按行终态上报（记 `subagent_cancelled`、不沉淀「失败」教训）；进程重启会清扫上一世遗留的 running/queued 僵尸行（排队行按 cancelled 收、执行中的按 failed 收）；**per-onion 配置档**：`spawn_subagent` 的 `onion` 参数点名一份配置（内置 probe / writereditor / validator 三份，启动时播种进数据目录、用户改过的不会被覆盖），按「实际可用工具 = profile 白名单 ∩ 配置档 allowed（只收紧、无 deny 列表）」同时收窄 schema 层与 dispatch 白名单闸，并把角色与交集后的工具清单写进子 agent 的 system 提示词；档位优先级为「显式 profile > 配置档 > stage 派生」（配置档不得覆盖运行时档位，那等于放宽），配置档 `max_turn` 只收紧派发给的轮数上限；**子任务回报回灌**——父会话每次组装消息时检查自己派出的子 agent，终态（成功/失败/取消/超预算）的把结构化收尾打包成一段 system 消息追加在历史之后（父上下文只吸收 `result_json`，绝不注入子会话历史），在途的只报一句「N 个仍在进行中」；同一份结果只汇报一次（进程内去重，重启后会重复汇报一轮，见设计文档 §十）
- **输入规范化（L0）**：用户这一轮的输入在建 msgs 之前先过一遍规范化——删不可见垃圾字符（BOM / word-joiner / soft-hyphen）、零宽空格换成真空格、CRLF 统一为 LF、去首尾空白。不改可见内容、不改代码块、不折叠空行；ZWJ/ZWNJ 保留（前者拼合 emoji 序列、后者是某些语言的不成形标记，当噪音清掉会改坏用户原话）。有改动才记 `user.input_normalized` 审计。这层不进钩子注册表，per-onion 配置关不掉它
- **子 Agent 配置档管理**：设置页「机器人技能」同一区，含列表（名字 / 角色 / 档位 / 工具数 / 轮数上限，坏文件显形并说明坏在哪）、导入 `.yaml`、导出原样。导入要求**文件名与文件内 `onion_id` 一致**（`bot_sessions.onion_config` 存的是文件名），同名不静默覆盖；导出搬落盘原文以保留行内注释
- **per-onion 采样温度**：配置档的 `temperature` 覆盖模型条目自带的温度（采样参数不是权限，不适用「只收紧」；配置档是更具体的意图）。三个内置档里 validator 写的是 `temperature: 0`
- **任务图谱**：Sigma.js + FA2 worker 力导向（Obsidian 式交互：hover 邻接高亮、度数定大小、过滤器、局部放大）；多人归属着色/聚簇、依赖编辑、标签近义合并、视野自适应、节点大小双模式（连接度/耗时）、设置页七项图谱偏好
- **词元统计四档计价**（2026-10-17）：Anthropic 的 input token 分四档定价（cache read 0.1x / cache write 1.25x / 普通 input 1x / output），`exec_traces` 单列存 `cache_read_tokens` / `cache_write_tokens` 而**不折进 `prompt_tokens`**——折算会让数字同时失去「普通 input 量」与「等效成本」两种含义，长会话的缓存收益（工具循环里每轮重发全部历史，首轮之后几乎全是缓存命中）也就看不见了。按日 / 按模型 / 按模型×日期三个聚合口口径一致；trace 详情弹窗在 token 数后附缓存读/写两档（tooltip 说明单价差异），设置页词元统计面板仍只显示输入/输出两档不动
- **Agent 透明化**：执行详情面板（工具/耗时/成败/文件 diff 证据、error_class 分类、痕迹清理与导出；痕迹三表按保留期自动清理——启动一次 + 每天一次，数据管理可手动触发）、定时执行透明、通知中心（三类决策事件消息化）、Agent 参数注册表 + 词元统计卡、主窗活动页
- **程序（M1 运行态 + M2 开发态已上线，设计稿 `docs/APPS-DESIGN-2026-10-15.md`、代码规范 `docs/APP-DEV-CONVENTIONS.md`）**：大模型开发的确定性小程序（脚本 + 输入声明），文件夹/表单两种运行态，运行不连大模型；程序页是运行态（卡片/试跑/历史/产物，草稿徽章可试跑）+ 手工上架入口；卡上「删除」= **归档**（脚本/数据/版本快照/运行历史原地保留，从程序页移走，头部「已归档 N」里可恢复回草稿态；归档/恢复都是用户动作，bot 工具不受理，已归档程序不开放修改）；开发态 = 聊天即 IDE——bot 工具 `apps_list`（防重名）/ `apps_create`（落草稿+文件集落盘）/ `apps_update_files`（整文件集修改，用户满意后置 enabled 上架），脚本契约（校验段 + `##RESULT##` 哨兵）写进工具 schema；程序本体走 `app_versions` 快照表版本化（创建/更新/回滚自动存，滚动 100 版，回滚 = 旧内容整体写回且可再撤销）。旧工作流画布（任务卡 DAG + LLM 逐节点执行）已退役，遗留表与数据行由 open_db 一次性清扫删除（幂等，以 workflows 表存在为闸）

### 5. 治理与自进化（观察态）

- 设置页决策板：候选提案 Promote/Reject（即开关语义）、应用策略二档、影子观察（shadow）、执行痕迹与 lesson 落库；每张提案卡带**决策证据**——影子判定（采纳后会不会进 lesson top-3）、同目标冲突标注（池内/已生效）、回滚预警（观察窗回滚 ≥5 提示切手动档）；**提案派生门槛可调**（Merge/Distill/Contradiction 三个涉及条数门槛，按记忆总量给 4 档建议值，存 evolution.deriveThresholds）
- 文件治理：授权模式四档 strict 白名单硬拒 / ask 弹授权（默认）/ auto / yolo 全放行 + per-tool 权限规则表；绑定集合精确命中才放行（`..`/软链/前缀相似目录全拒）
- 密钥纪律：API key、搜索 key、MCP 机密全走系统凭据存储，不落明文配置

### 6. 数据、集成与平台

- SQLite（WAL + busy_timeout 2s）单写者行级增量读写；主窗 `mutate()` diff 落盘广播 `tasks-changed`，后端 diff 上报；迁移链自动导入 data.json 旧数据（localStorage 迁移已删）
- **本地 HTTP API**（tiny_http + SSE + token 鉴权）：启停/状态/换 token，供外部集成
- 便携模式：数据库随 exe 走（exe 目录锚定，不可写兜底 app_data_dir）；Windows 绿色 zip + macOS dmg（Intel/Apple Silicon）
- 桌面清理：CSV 规则表（模版下载/导入），`{year}` 占位，move/delete 两动作，10 分钟后台轮询，migration.log；看板/回收站附件绝不触碰
- **日志体系**：数据目录四个落盘日志——bot.log（全应用结构化审计，`[ts] LEVEL | event | k=v`）、error.log（WARN/ERROR 事件镜像账本，不被 Info 流水稀释）、api.log（HTTP API 访问+变更）、migration.log（迁移执行），全部 5MB 轮转保留两代（`.old`/`.old.1`）；全局 panic 钩子记 ERROR；前端 window error/unhandledrejection 与 CSP 违规经 frontend_event_report 上报（32 签名去重）；设置页「诊断」卡 = error.log 最近错误 + 一键复制全部日志（error 信号在前、流水在后，单文件超 256KB 尾部截断）
- 全局快捷键：唤起/隐藏主窗口（macOS `Cmd+Ctrl+W` / Win `Ctrl+Alt+W`）、快速新建任务（`Cmd/Ctrl+Alt+N`，唤起并进入标题编辑）、切换深浅色（`Cmd/Ctrl+Alt+T`）；注册失败只记日志不影响启动
- UI 体系：新拟态 nm 组件类 + 设计 token + 左导航壳 + lucide 图标 + a11y 基线 + 深浅色三态主题

## 技术栈（现行）

- Tauri 2 + React 19 + TypeScript + Vite 7 + Tailwind v3（nm 组件类）
- rusqlite（bundled）；reqwest 流式；keyring；ort + tokenizers（bge-small-zh 本地嵌入）；rmcp（MCP）；tiny_http（本地 API）
- 前端图形：sigma.js + graphology + FA2（任务图谱）、@dnd-kit/core
- 工具链：cargo nextest / clippy / machete、vitest / oxlint / knip / tsc；门禁见 `docs/testing.md`

---

# 附录 A：v1 原始需求快照（2026-08-13 ~ 2026-08-18）

> 项目起点的原始需求记录（含原始 UI 规范与逐条追加功能），停止更新；
> 现行形态以本文件前半部分为准。

# WMessage — SPEC v1

> 唯一依据。来源：老板 2026-08-13 发来的最终 OpenClaw 完整 Prompt（含样式说明）。
>
> **历史存档声明（2026-09-29）**：本 SPEC 是项目起点的原始需求快照。此后
> 功能已大幅演进（子 Agent 编排、自进化观察态、MCP 宿主、双协议等），现行
> 架构以 `docs/rust-bot-architecture.md` 为准，迭代记录见 `DEVLOG.md`；
> 本文件不再随功能更新。

## 项目定位

WMessage：Tauri + React + TailwindCSS 的 Todo 看板，带系统侧边磁吸挂件。

## 技术栈

- Tauri 2 + React 19 + TypeScript + Vite 7
- Tailwind CSS v3（`@layer components` 新拟态组件类）
- 拖拽：`@dnd-kit/core`
- 编译产出：Win10/Win11（exe 安装包）、macOS（dmg，Intel + Apple Silicon）

## UI 规范（新拟态 nm 体系）

- 基色 `#f4f7fa`；凸卡片圆角 `rounded-2xl`
- 凸卡片 `.nm-card`：`box-shadow: 6px 6px 12px #d8dee6, -6px -6px 12px #ffffff`
- hover 上浮 `.nm-card-hover`：`translateY(-3px) scale(1.01)` + 阴影增强
- 内凹 `.nm-inset`：inset 阴影，用于按钮/列头/触发条，与凸卡片对比
- 侧边面板 `.nm-sidebar-panel`：左侧圆角 + 单侧投影
- 先浅色主题；深色模式后续扩展

## 功能需求

1. 主看板：卡片列 + 拖拽（列间移动）
2. 系统侧边磁吸挂件：默认贴边隐藏，鼠标悬停滑出单列清单；视图切换（全部任务 / 今日专注）
3. 任务绑定本地文件：打开本地文件；一键复制文件 + 附带任务标题文本
4. 本地持久化存储
5. 全局快捷键
6. 挂件常驻锁定开关
7. 挂件 UI 与主看板视觉统一

## 里程碑

- M1 脚手架 + 样式体系 + 看板静态 ✓
- M2 拖拽看板（dnd-kit）+ localStorage 持久化
- M3 文件绑定：打开 / 复制（Tauri opener + clipboard + fs）
- M4 侧边挂件多窗口（磁吸、悬停滑出、锁定常驻）
- M5 全局快捷键
- M6 打包（NSIS exe + dmg universal）

## Logo 规范

- 视觉使用规范 V1.0（正式版）：`docs/logo/WMessage-LOGO-GUIDELINES.md`
- Logo 资产：`docs/logo/assets/`（已处理：去水印、透明底、多尺寸）

## 目录约定

- 样式入口：`src/ui/main.css`
- 组件：`src/components/`
- 类型：`src/types.ts`

## 功能需求（追加 2026-08-15：桌面文件自动迁移清理）

8. 文件整理（绑定整理 + 扫描整理，共用一张全局规则表与同一份整理记录 migration.log）：
   - 规则表 `cleanup-rules.json`（数据目录）：每条含 启用开关 / 文件名关键字 / 动作 / 归档目录；用户可随时上传规则表（JSON / CSV 双格式），无需改代码；支持模版下载
   - 归档目录支持 `{year}` 占位符（展开为当年年份）；相对路径，基于「归档基准」解析——绑定整理按文件所在地盘取基准（落在哪个整理来源内用该源的归档根，都不在用桌面），扫描整理用该来源的归档根（空 = 来源文件夹本身）
   - 整理来源表 `cleanup-sources.json`（数据目录）：预设「桌面」一条（路径永不落盘，运行时经已知文件夹 API 解析真实位置，规避 Windows 重定向/OneDrive 劫持/显示名本地化）；自定义来源由用户经文件夹选择器添加（绝对路径），可启用/停用/移除/设归档根
   - 绑定整理：任务完成满归档天数进入归档列表后，同轮即按规则迁移其全部绑定文件（最多 10 个，逐文件按文件名匹配规则，可命中不同规则/目录；绑定物可为任意地址）：移动到归档目录 → 自动更新该文件绑定地址 → 写整理记录；已落在归档目录内的绑定不再重搬（防同名冲突后缀逐轮膨胀）。触发 = 后台轮询（启动 60s 后首跑，之后每 10 分钟）+ 设置页手动按钮
   - 扫描整理：仅手动触发（点击按钮，不定时执行）；对各启用来源的顶层文件按规则归类，**任务卡绑定物一律跳过（含回收站/归档卡）**——有主文件只由绑定整理管理，连接永不断
   - 保护：未归档（看板中）任务的附件绝不移动/删除（绑定整理只处理归档卡；扫描整理排除一切绑定物）；回收站任务不处理
   - 动作：move（移动归档）；delete（删除文件，默认关闭，需用户显式启用）
   - 异常：源文件不存在 / 权限不足 / 同名冲突 → 记日志跳过，不崩溃、不覆盖、不强制删除；同名冲突自动加 ` (n)` 后缀
   - Windows 优先兼容，同时适配 Mac（rename 失败退化为 copy+remove）

## 功能需求（追加 2026-08-15：工作区静态链接）

9. 工作区（静态链接收藏）：
   - 主窗口「工作区」视图按钮位于「归档」与「回收站」之间；挂件「工作区」按钮位于「今日」与「锁定」之间
   - UI 类似任务卡：每条工作区有标题（点击内联编辑）、折叠开关；展开后可增删链接
   - 链接三类：网址（🔗，openUrl 打开）、文件（📄）、文件夹（📁），点击打开（openPath）
   - 数据存 SQLite workspace_items 表（id/title/collapsed/links JSON/ord/updated_at）；主窗口编辑、挂件只读（workspace-changed 事件 + 5s 轮询同步）

## 功能需求（追加 2026-08-16：内置机器人聊天）

10. 内置机器人（设置页开关，默认关闭）：
   - 挂件下方聊天区（展开高度 840 = 560 + 280）；多会话（新建/切换/删除/自动改名）、历史持久化 SQLite
- 内置工具 45 个（清单以 `bot::registry::TOOLS_TABLE` 为准；其中 read_own_card/write_artifact_file 两个仅子 agent 可见，不进主 agent 清单）：任务管理（query_tasks/query_single_task/create_task/complete_task/delete_task/edit_task/add_subtask/toggle_subtask/remove_subtask/link_file_to_task）+ 文件读写（list_files/read_text_file/grep_files/edit_file/write_file）+ 文档处理（extract_document/create_word/create_word_revisions/create_excel/create_ppt/create_pdf）+ 图片识字（ocr_image）+ 本机 Python（run_python，独立临时目录在系统 temp 的 wmessage-py-runs/<uuid>/ + 60s 超时，开关默认关闭；子进程注入 WM_GEN_DIR/WM_TMP_DIR 环境变量，写相对路径的产物跑完自动回收进 AI_Gen_Files）+ Python AST 探查（ast_inspect：走同一 run_python 沙箱 + 标准库 ast，零新增依赖；op=symbols/defs/refs/callgraph 四模式，callgraph 跨文件现扫不缓存，单次上限 200 文件/2MB、触及即报 truncated；授权判定留在 Rust 侧——先过 bot_fs 白名单闸再把 canonical path 交给沙箱，目录内指向外部的 symlink 一律排除；仅 .py，非 .py 明确拒绝并指向 grep_files）+ 联网（web_search：配置 Tavily 或 Brave key 走对应 API、双开报错，未配置走 Bing+百度网页抓取 / fetch_url 仅公网）+ 长期记忆（remember_fact/recall_facts/record_lesson，语义记忆体 v2）+ 时间（get_current_time）+ Skill（use_skill/create_skill）+ 子 agent（spawn_subagent/check_subagent/cancel_subagent）+ 电脑辅助（reveal_path/open_url/clipboard_write/screenshot）+ 程序（apps_list/apps_create/apps_update_files）+ 问答（ask_user）
   - Word 润色默认修订模式（track changes，w:ins/w:del，author=WMessage AI）；产物统一落 AI_Gen_Files（`db::gen_dir` 中心函数解析、启动即预建、同名 (n) 序号永不覆盖；系统提示词硬性约束产物/临时文件落点）
   - 流式回复；思考过程（<think>）与工具调用折叠行可展开；Markdown 渲染回复
   - 斜杠命令：/stop /compact（≤300 字摘要）/retry /clean（清空当前对话）
   - 安全：删除任务弹确认（60s 超时自动拒绝）；审计日志 bot.log；参数上限；API Key 存系统凭据存储（keyring）；文件访问授权模式（2026-08-26）：strict 白名单硬拒 / ask 白名单外弹授权（默认，允许一次/始终允许该目录/拒绝）/ yolo 全放行（文件+Python）；extract_document path 校验（任务卡绑定文件 / AI_Gen_Files 静默放行，其余走授权分流）；上下文超长（400 context_length_exceeded / prompt is too long 等形态）自动降级：砍掉最旧的会话消息后重发，至多两次；仍超长则回「请 /compact 或新建会话」的可操作报错
   - 挂件选任务模式（🎯 整卡单击选中，📌 引用块随消息发送）；任务卡 🤖 按钮一键执行（卡片显示机器人归属头像）

## 功能需求（追加 2026-08-16：定时任务卡 + 归属头像）

11. 定时任务卡：卡片 ⏰ 按钮设定时（一次 at: / 每天 daily: / 每周 weekly:D: / 每月 monthly:DD: 四档），到点自动交给机器人执行（30s 扫描调度器）；结果前置「⏰ 自动执行」写进备注；一次性执行完自动清除；错过的一次性任务不补执行
12. 任务卡归属头像：人完成 → 用户头像；交给机器人 → 机器人头像（执行结束无论成败清除）；用户/机器人头像与姓名在设置页修改。头像为**内置预设 16 选 1**（不支持上传自定义图片）：头像图打包前端 assets，profile.json / 导出信封 / people 表全链路只存稳定 ID（avatar-01~16，Rust `AVATAR_IDS` 与前端 `src/avatars.ts` 注册表双端对齐），渲染按 ID 解析本地资源；设置页点头像弹网格选择器（即点即换，可恢复默认占位）；导出信封 profile 卡携带 `avatarId`，导入端白名单过滤后落 `people.avatar`——跨设备按同一 ID 显示同一张头像

## 功能需求（追加 2026-08-17：移除 /help 斜杠命令）

13. 移除机器人聊天 `/help` 命令（老板 2026-08-17 21:52 指令）：
    - 缘由：机器人面板已上浮为补全面板（输入 `/` 弹出 4 条命令候选，无需 /help 再讲一遍），`/help` 文本还会被作为用户消息送进 LLM 上下文白白占 token
    - 行为：用户输入 `/help` 不再被本地拦截，由前端 `send()` 当普通文本发给模型；模型若仍能调用工具则按需执行，否则仅回一句普通回复
    - 保留：`/stop` `/compact` `/retry` 三个有副作用的本地命令不动（2026-09-08 起增补 `/clean` 清空当前对话，现共 4 条）
    - 影响面：仅 `src/components/ChatPanel.tsx`（SLASH_COMMANDS 列表 + runSlashCommand 分支 + 占位文案）


## 功能需求（追加 2026-08-18：Skill DSL 调度器 + 监控 UI + 跨平台打包）

### Skill DSL 调度器（C 路径）
- **DSL 格式**：Markdown + YAML frontmatter + `## Step N: 标题` + `tool_name({...})` + 可选 `## Rollback` 段
- **变量替换**：单段 `${stepN.result/id}` + `${prev.result/id}` + 嵌套路径 `${stepN.path.to.field}`（沿 `serde_json::Value` 路径取值）
- **状态机**：5 个 DslAdvanceAction（Run / Finish / AwaitUser / FailWithRollback / Terminate），每 step 前查 `advance_dsl` 决策
- **LLM 兜底**：FailedButRecoverable 把 `format_completed_summary(ctx)` 注入 system prompt，LLM 决策下一步；Terminate 不接管
- **返回类型**：`Result<DslOutcome, DslFailure>` —— `DslOutcome { Done, AwaitUser, FailedButRecoverable { reason, completed_summary, rollback_attempted } }` + `DslFailure { Terminated { reason } }`

### Skill 监控 UI（D 路径）
- **持久化**：每次 Skill 跑完写 SQLite `skill_outcomes` 表（skill_name PK + kind / reason / completed_summary / rollback_attempted / last_at_ms）
- **扫描集成**：`scan_skills` 调用 `load_all_skill_outcomes` 填充每个 Skill 的 `last_outcome` 字段
- **设置页徽章**：4 种状态 4 种配色（绿/蓝/黄/红 对应 done / await_user / failed_recoverable / terminated），title 含 reason + completed_summary

### 跨平台打包
- **Windows 绿色包**：Mac 上 mingw 交叉编译 `x86_64-pc-windows-gnu` + Python zipfile 打绿色 zip（不用 macOS `zip` 避免 Unix 扩展字段）
- **必备文件**：wmessage.exe + WebView2Loader.dll（160KB，缺它必报「找不到 webview2loader.dll」）
- **便携模式**：数据库放 exe 同目录随 U 盘走，exe 目录不可写时兜底 `app_data_dir`；exe 旁已有 `wmessage.db` 或 `AI_Gen_Files` 时强制便携锚定（写探针失败也不翻转）；从 zip 内直接双击运行（exe 落系统 temp）时不在 temp 建数据，退化 `app_data_dir` 并在 bot.log 记 WARN——全机只允许一个 AI_Gen_Files
- **完整流程**：`cargo clean --target x86_64-pc-windows-gnu` → `npx tauri build --target x86_64-pc-windows-gnu --no-bundle` 带 mingw env（不能直接 cargo build）

### Skill 路径解析
- **dev 模式**：扫 `target/debug/skills/` + `app_data_dir/skills`，数据目录优先（用户已导入版本覆盖 dev mock）
- **release 模式**：只扫 `app_data_dir/skills`（`#[cfg(debug_assertions)]` 条件编译排除 dev_skills_dir）
- **纯函数拆分**：`scan_skill_dirs(dirs)` 不依赖 AppHandle，单测可独立测

### 工具调用模型
- **白名单**（单点工具，非 Skill 状态可直接调用）：list_tasks / query_single_task / search_tasks / create_task / edit_task / complete_task / delete_task / add_subtask / toggle_subtask / remove_subtask / bind_file / read_text_file / grep_files / list_files / ocr_image / extract_document / create_word / create_word_revisions / create_excel / create_ppt / create_pdf / run_python / web_search / fetch_url / get_current_time / remember_fact / recall_facts / record_lesson / use_skill
- **黑名单**（Skill 内部专用）：link_file_to_task —— 非 Skill 状态禁止裸调（create_word_revisions 已去 Skill 化，聊天里直接可用）
