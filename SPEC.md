# WMessage — SPEC v1

> 唯一依据。来源：老板 2026-08-13 发来的最终 OpenClaw 完整 Prompt（含样式说明）。

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
   - 30 个工具：任务管理（list_tasks/query_single_task/search_tasks/create_task/edit_task/complete_task/delete_task/add_subtask/toggle_subtask/remove_subtask/bind_file/link_file_to_task）+ 文档处理（extract_document/create_word/create_word_revisions/create_excel/create_ppt/create_pdf）+ 文件读写（list_files/read_text_file/grep_files）+ 图片识字（ocr_image）+ 本机 Python（run_python，独立临时目录 + 60s 超时，开关默认关闭）+ 联网（web_search：配置 Tavily 或 Brave key 走对应 API、双开报错，未配置走 Bing+百度网页抓取 / fetch_url 仅公网）+ 长期记忆（remember_fact/recall_facts/record_lesson，语义记忆体 v2）+ 时间（get_current_time）+ Skill（use_skill）
   - Word 润色默认修订模式（track changes，w:ins/w:del，author=WMessage AI）；产物只落 AI_Gen_Files 同名 (n) 序号永不覆盖
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
- **便携模式**：数据库放 exe 同目录随 U 盘走，exe 目录不可写时兜底 `app_data_dir`
- **完整流程**：`cargo clean --target x86_64-pc-windows-gnu` → `npx tauri build --target x86_64-pc-windows-gnu --no-bundle` 带 mingw env（不能直接 cargo build）

### Skill 路径解析
- **dev 模式**：扫 `target/debug/skills/` + `app_data_dir/skills`，数据目录优先（用户已导入版本覆盖 dev mock）
- **release 模式**：只扫 `app_data_dir/skills`（`#[cfg(debug_assertions)]` 条件编译排除 dev_skills_dir）
- **纯函数拆分**：`scan_skill_dirs(dirs)` 不依赖 AppHandle，单测可独立测

### 工具调用模型
- **白名单**（单点工具，非 Skill 状态可直接调用）：list_tasks / query_single_task / search_tasks / create_task / edit_task / complete_task / delete_task / add_subtask / toggle_subtask / remove_subtask / bind_file / read_text_file / grep_files / list_files / ocr_image / extract_document / create_word / create_word_revisions / create_excel / create_ppt / create_pdf / run_python / web_search / fetch_url / get_current_time / remember_fact / recall_facts / record_lesson / use_skill
- **黑名单**（Skill 内部专用）：link_file_to_task —— 非 Skill 状态禁止裸调（create_word_revisions 已去 Skill 化，聊天里直接可用）
