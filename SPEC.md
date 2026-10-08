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

单机个人 AI 任务助手：Todo 看板为骨架（主窗口 + 侧边磁吸挂件，Windows 10/11 +
macOS），内置大模型机器人管理任务，向上长出工作流编排、子 Agent、任务图谱、
自进化观察态。无账号体系；多人协作 = 任务 JSON 导出汇总导入。

## 功能规格（现行，2026-10-08）

### 1. 看板与任务卡

- 三列看板（待办/今日/完成），dnd-kit 拖拽；截止日=当天的待办自动进今日
- 任务卡：标题/备注/标签/子任务进度/文件绑定（多文件）/截止时间/折叠/归属头像（人完成→用户头像，交给机器人→机器人头像）
- 自动归档（天数可配置，默认 7 天）：归档页搜索+标签筛选+三列网格、只读可恢复；归档时大模型自动打标（≤3 个）
- 回收站：软删除；绑文件任务彻底删除时三选项弹窗（文件移废纸篓 / 只删卡 / 取消）
- 多人汇总：任务导出 JSON 按 id 并集合并（updated_at 新者胜）；`owner_id` 归属字段；看板/挂件默认只看自己的任务，图谱与统计看全部
- 定时任务卡 ⏰：一次/每天/每周/每月四档，到点自动交给机器人执行，结果前置「⏰ 自动执行」写备注；一次性执行完自动清除，错过补跑 2h 时效窗口
- 主窗口：左导航多视图（看板/日程/归档/工作区/回收站/工作流/任务图谱/活动/通知）+ ⌘K 命令面板；主窗关闭=隐藏，托盘常驻

### 2. 挂件

- 屏幕任意边缘磁吸（右/左竖条，贴顶自动横条）；悬停滑出 / 📌 锁定常驻；自由拖动+贴边吸附+位置记忆
- 任务/工作区/聊天三视图：打勾、勾子任务、开文件、新建任务、双击标题唤起主窗口进入编辑态；聊天区支持拖文件附件

### 3. 机器人聊天与模型

- 多会话聊天：流式回复、思考/工具调用折叠、Markdown 渲染、斜杠命令 /stop /compact /retry /clean、删除确认（60s 超时自动拒）、审计日志 bot.log
- **一次执行 = 一个新会话**：任务卡 🤖、定时任务、工作流节点执行统一走聊天会话
- 双协议：OpenAI 兼容 + Anthropic 兼容；模型中心：厂商分类、内置模型库、厂商级 key（系统 keyring）、每模型推理参数与推理强度、可用性门禁、聊天内快切模型
- 工具 30+：任务 CRUD/子任务/绑定、文档生成（Word 修订模式：.NET OpenXML 优先 + Python 兜底；Excel 公式注入过滤；PPT；PDF）、文件编辑（edit_file/write_file，Aider 式三级匹配）、文件读写+grep、本机 Python 沙箱（run_python，独立临时目录+60s 超时+产物回收）、联网（web_search：Tavily/Brave key 可配，未配置降级 Bing+百度抓取；fetch_url 公网白名单）、图片识字 ocr_image、截图直达模型视觉、电脑辅助 Tier1（reveal_path 等原生四件）、时间、记忆工具、use_skill、ask_user（执行中提问）
- **语义记忆体 v2**：bge-small-zh 本地嵌入+混合打分；事实+教训两类；自动抽取、参数可调（设置页记忆区「检索参数」卡：注入预算/topN/recentN/lessonN/容量/衰减/去重阈值 8 项，留空=默认）、总开关、管理面板、导入导出、写入冲突裁决（改口即更新）、定时整理（consolidation）、黄金集评估器（recall@5 锁死）
- **MCP 外部服务器**：stdio/HTTP 接入，env/headers 机密走系统钥匙串
- **技能系统**：Markdown+YAML frontmatter DSL（对齐 Agent Skills 开放标准），步骤推进状态机 + LLM 兜底，设置页运行结果徽章
- 产物统一落 `AI_Gen_Files`（启动预建、同名加 `(n)` 永不覆盖），流程结束产物登记表弹窗汇总

### 4. 编排与执行

- **工作流画布**：一句话总目标 → AI 拆解任务卡 DAG → 画布手调（加删卡/连线）→ 保存落库 → 一键拓扑执行；JSON 导入导出=可复用 SOP 模板（导入=全新 id 实例化）；工作流任务默认不在看板/挂件显示
- 工作流质量件：拆解前澄清（双层档案 + 澄清回注 + assumptions 折叠条）、执行中 ask_user 提问闭环、节点级验收 + run 级结构化审计、轻量评审模型、有界重试/返工环、导出路径闸门
- **子 Agent 编排**：生命周期/并发闸/预算上限/状态机/runner，结构化字段路由
- **任务图谱**：Sigma.js + FA2 worker 力导向（Obsidian 式交互：hover 邻接高亮、度数定大小、过滤器、局部放大）；多人归属着色/聚簇、依赖编辑、标签近义合并、视野自适应、节点大小双模式（连接度/耗时）、设置页七项图谱偏好
- **Agent 透明化**：执行详情面板（工具/耗时/成败/文件 diff 证据、error_class 分类、痕迹清理与导出）、画布实时高亮、定时执行透明、通知中心（三类决策事件消息化）、Agent 参数注册表 + 词元统计卡、主窗活动页

### 5. 治理与自进化（观察态）

- 设置页决策板：候选提案 Promote/Reject（即开关语义）、应用策略二档、影子观察（shadow）、执行痕迹与 lesson 落库；每张提案卡带**决策证据**——影子判定（采纳后会不会进 lesson top-3）、同目标冲突标注（池内/已生效）、回滚预警（观察窗回滚 ≥5 提示切手动档）；**提案派生门槛可调**（Merge/Distill/Contradiction 三个涉及条数门槛，按记忆总量给 4 档建议值，存 evolution.deriveThresholds）
- 文件治理：授权模式四档 strict 白名单硬拒 / ask 弹授权（默认）/ auto / yolo 全放行 + per-tool 权限规则表；绑定集合精确命中才放行（`..`/软链/前缀相似目录全拒）
- 密钥纪律：API key、搜索 key、MCP 机密全走系统凭据存储，不落明文配置

### 6. 数据、集成与平台

- SQLite（WAL + busy_timeout 2s）单写者行级增量读写；主窗 `mutate()` diff 落盘广播 `tasks-changed`，挂件 diff 上报 + 5s 兜底轮询；迁移链自动导入 data.json/localStorage 旧数据
- **本地 HTTP API**（tiny_http + SSE + token 鉴权）：启停/状态/换 token，供外部集成
- 便携模式：数据库随 exe 走（exe 目录锚定，不可写兜底 app_data_dir）；Windows 绿色 zip + macOS dmg（Intel/Apple Silicon）
- 桌面清理：CSV 规则表（模版下载/导入），`{year}` 占位，move/delete 两动作，10 分钟后台轮询，migration.log；看板/回收站附件绝不触碰
- 全局快捷键：唤起/隐藏主窗口（macOS `Cmd+Ctrl+W` / Win `Ctrl+Alt+W`）、快速新建任务（`Cmd/Ctrl+Alt+N`，唤起并进入标题编辑）、切换深浅色（`Cmd/Ctrl+Alt+T`）；注册失败只记日志不影响启动
- UI 体系：新拟态 nm 组件类 + 设计 token + 左导航壳 + lucide 图标 + a11y 基线 + 深浅色三态主题

## 技术栈（现行）

- Tauri 2 + React 19 + TypeScript + Vite 7 + Tailwind v3（nm 组件类）
- rusqlite（bundled）；reqwest 流式；keyring；ort + tokenizers（bge-small-zh 本地嵌入）；rmcp（MCP）；tiny_http（本地 API）
- 前端图形：sigma.js + graphology + FA2（任务图谱）、dagre（工作流画布布局）、@dnd-kit/core
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

8. 桌面清理（任务绑定文件的自动迁移）：
   - 规则表 `cleanup-rules.json`（数据目录）：每条含 启用开关 / 文件名关键字 / 动作 / 归档目录；用户可随时上传规则表（JSON / CSV 双格式），无需改代码；支持模版下载
   - 归档目录支持 `{year}` 占位符（展开为当年年份）；相对路径基于桌面，绝对路径原样使用
   - 主规则：任务完成满 7 天进入归档列表后，按规则迁移其绑定文件：移动到归档目录 → 自动更新任务 filePath → 写迁移日志 migration.log
   - 保护：未归档（看板中）任务的附件绝不移动/删除；回收站任务不处理
   - 动作：move（移动归档）；delete（删除文件，默认关闭，需用户显式启用）
   - 触发：设置页「桌面清理」手动执行按钮 + 后台轮询（启动 60s 后首跑，之后每 10 分钟）
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
   - 30 个工具：任务管理（list_tasks/query_single_task/search_tasks/create_task/edit_task/complete_task/delete_task/add_subtask/toggle_subtask/remove_subtask/bind_file/link_file_to_task）+ 文档处理（extract_document/create_word/create_word_revisions/create_excel/create_ppt/create_pdf）+ 文件读写（list_files/read_text_file/grep_files）+ 图片识字（ocr_image）+ 本机 Python（run_python，独立临时目录在系统 temp 的 wmessage-py-runs/<uuid>/ + 60s 超时，开关默认关闭；子进程注入 WM_GEN_DIR/WM_TMP_DIR 环境变量，写相对路径的产物跑完自动回收进 AI_Gen_Files）+ 联网（web_search：配置 Tavily 或 Brave key 走对应 API、双开报错，未配置走 Bing+百度网页抓取 / fetch_url 仅公网）+ 长期记忆（remember_fact/recall_facts/record_lesson，语义记忆体 v2）+ 时间（get_current_time）+ Skill（use_skill）
   - Word 润色默认修订模式（track changes，w:ins/w:del，author=WMessage AI）；产物统一落 AI_Gen_Files（`db::gen_dir` 中心函数解析、启动即预建、同名 (n) 序号永不覆盖；系统提示词硬性约束产物/临时文件落点）
   - 流式回复；思考过程（<think>）与工具调用折叠行可展开；Markdown 渲染回复
   - 斜杠命令：/stop /compact（≤300 字摘要）/retry /clean（清空当前对话）
   - 安全：删除任务弹确认（60s 超时自动拒绝）；审计日志 bot.log；参数上限；API Key 存系统凭据存储（keyring）；文件访问授权模式（2026-08-26）：strict 白名单硬拒 / ask 白名单外弹授权（默认，允许一次/始终允许该目录/拒绝）/ yolo 全放行（文件+Python）；extract_document path 校验（任务卡绑定文件 / AI_Gen_Files 静默放行，其余走授权分流）
   - 挂件选任务模式（🎯 整卡单击选中，📌 引用块随消息发送）；任务卡 🤖 按钮一键执行（卡片显示机器人归属头像）

## 功能需求（追加 2026-08-16：定时任务卡 + 归属头像）

11. 定时任务卡：卡片 ⏰ 按钮设定时（一次 at: / 每天 daily: / 每周 weekly:D: / 每月 monthly:DD: 四档），到点自动交给机器人执行（30s 扫描调度器）；结果前置「⏰ 自动执行」写进备注；一次性执行完自动清除；错过的一次性任务不补执行
12. 任务卡归属头像：人完成 → 用户头像；交给机器人 → 机器人头像（执行结束无论成败清除）；用户/机器人头像与姓名在设置页上传/修改（profile.json + profile/ 头像文件，base64 data URL 返回）

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
