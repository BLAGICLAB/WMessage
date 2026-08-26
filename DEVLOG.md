# WMessage 开发日志

> 面向开发者的里程碑记录。产品规格见 `SPEC.md`，项目说明见 `README.md`。

## 2026-08-26（周三·深夜 2）PREVR 第 1+2 层：失败换策略提示 + 复杂任务动态计划

**动因**：架构对比后老板拍板落地 PREVR（Plan-Execute-Verify-Replan）的前两层——rust bot 此前工具失败只会把错误抛给用户，不会自己换办法。

**第 1 层：执行器内验证（run_model_loop）**
- 工具结果复用审计分级（classify_text Warn/Error = 失败）做失败检测
- 单次失败 → 注入「换策略」提示（换参数/换工具/拆小步骤）；同工具连续 ≥2 次失败 → 禁止相同调用、要求如实告知用户
- 与 soft_warn 同一协议安全位：提示在本轮 tool 响应全部回填后才注入（不破坏 tool_calls→tool 序列）

**第 2 层：动态计划（新模块 bot_plan.rs）**
- `needs_plan` 保守启发式（多步关键词/「先…再…」/多附件）命中才触发 Planner，简单问答零额外成本
- Planner = 单次非流式 LLM 调用，输出 JSON 步骤列表；`parse_plan` 容忍包裹文字、上限 8 步；失败/解析不出 → 降级原自由循环不阻断聊天
- 计划注入 system prompt（【执行计划】块，含「走不通就调整并说明」指令）；仅聊天主路径启用，execute_task_core / exec_steps 目标单一不规划
- **Replan**：run_model_loop 内同工具连续失败 ≥2 且有计划 → 带失败原因重规划剩余步骤（≤2 次硬上限防死循环），新计划注入对话
- 计划纯提示词文本，执行仍走 execute_tool 全量安全网关（白名单/授权/确认/熔断），无绕过通道
- 审计：plan.generate / plan.skip / plan.replan / plan.replan_failed
- 测试：bot_plan 7 个新用例（needs_plan 命中/放过、parse 容错/截断/拒绝、plan 块渲染）；cargo 414 全绿；npm 110 全绿；tsc 通过

## 2026-08-26（周三·深夜）会话隔离修复：流式/Skill/确认/逐步执行全链路按会话归属

**动因**（老板要求审计「聊天会不会串」）：审计发现存储层（bot_messages 按 session_id）和前端切换（sessionIdRef 竞态守卫）是隔离的，但运行期有四个串线通道。

**修复**：
- **流式事件（P0）**：run_model_loop 的 bot-chat-delta/bot-think-delta/bot-tool* 全部收口到 `emit_stream` 闭包，只在交互实例（StopGuard.is_interactive）广播；后台定时任务（execute_task_core interactive=false）不再向挂件推流——此前定时任务到点触发时，其 LLM 输出会追加进用户当前会话的 streaming 气泡并被 persistHistory 当成本轮对话落库
- **Skill 按会话归属（P1）**：SkillRun 新增 session_id；start_skill/active_skill_run_for/skill_finish/skill_mark_paused/skill_confirm_result/skill_on_step(_post)/is_skill_active_for 全部按会话过滤——会话 A 暂停中的 Skill 不再被会话 B 的主循环推进、确认或收尾；start_skill 的「单活动技能切换」也只结束同会话技能
- **确认弹窗按会话归属（P1）**：ConfirmMap 条目记录归属会话，bot-confirm 事件带 sessionId，前端只弹当前会话的确认；后台执行（interactive=false）不弹窗直接拒绝（无人在场必超时，且会串进用户当前会话）；bot_confirm_response 从条目取回会话再恢复对应 Skill
- **逐步执行挂起按会话归属**：PendingExec 带 session_id，has_pending_for 按会话匹配——会话 A 挂起等确认时，会话 B 的消息不再被 resume 截胡
- 透传链：StopGuard::new(interactive, session_id) → bot_chat/bot_execute_task 命令收 sessionId 参数 → run_model_loop/execute_tool_impl/各工具；exec_steps 三个 StopGuard 同理
- 测试：后端 407 全绿（StopGuard/SkillRun 构造同步补字段）；前端新增「确认弹窗按会话过滤」用例（捕获 bot-confirm 监听器，验证别会话/无归属不弹、本会话弹）；110 全绿 + tsc 通过

## 2026-08-26（周三·夜）熔断上限 50 + 发送/停止一体键

- **熔断放宽**（老板拍板）：默认对话轮数 20 → **50**；单轮 Function 调用上限 10 → **50**，软警告 7 → 35（保持 ~30% buffer）。失控防护仍靠幻觉守卫 + 软警告 + 停止键
- **发送/停止一体键**（老板拍板）：聊天发出后发送键变为红框正方形 ■ 停止键（`text-[var(--danger)]`），点击即 `bot_stop` 立马中断本次运行；中断或回复结束自动变回发送键。输入框 placeholder 同步提示；/stop 斜杠命令保留可用
- 测试：熔断边界断言改 50/51 与 50 轮默认值；ChatPanel 新增「busy 时停止键点击调 bot_stop」用例
- 验证：`cargo test --lib` 407 全绿；`npm test` 109 全绿；tsc 通过

## 2026-08-26（周三·晚）任务卡绑定文件交互改版：点名直开 + chip 内「复制」字样

**动因**（老板指令）：📂 打开 / 📋 复制两个 emoji 按钮与具体文件不对应（多文件时 📂 还要弹选择列表），交互绕。

**改造**（主窗口 TodoCard + 挂件 TaskCardContent 同步）：
- 打开：删 📂 按钮与多文件选择列表 → **点绑定文件名/文件夹名直接打开**该文件（chip 名变 button）
- 复制：删 📋 按钮 → 每个 chip 在解绑 × 前加「**复制**」字样（逐文件 `copy_file_with_title`，复制文件+标题）
- 样式：chip 小一号字号（名称 11px / 复制 10px）+ `nm-inset` 凹陷底色与卡片底色区分
- 绑定操作行只留绑定类按钮（＋绑定文件 / 📁绑定文件夹 / ×解绑全部）
- WidgetApp 回调改 per-path：onOpenFilePath / onCopyFilePath（删除 onOpenFile/onCopyFile 整卡回调）
- `copy_files_with_title`（多文件复制）随改版整体下线：命令、macOS/Windows 平台 helper、invoke 注册全删
- 测试：TodoCard/TaskCardContent 的 📂 多选列表与 📋 用例改写为新交互；`npm test` 108 全绿 + tsc 通过

## 2026-08-26（周三）文件访问改造：白名单硬拦 → 执行前授权（Kimi CLI 风格）+ yolo 模式

**动因**（老板原话：「该读的不让读，还要绑定文件，流程繁琐」）：read_text_file/grep_files/list_files/extract_document 白名单外硬拒绝，要读其它文件得先绑定任务卡或改设置页，流程打断。

**改造**：
- `bot-config.json` 新增 `permMode`：`strict`（白名单外硬拒，旧行为）/ `ask`（**新默认**，白名单外弹授权窗）/ `yolo`（全放行不弹窗，文件工具 + run_python 免开关）；`PermMode::from_cfg` 非法值回退 ask
- `bot_fs::resolve_with_perm` 三分流：白名单命中静默放行；ask → 复用 ConfirmMap 弹三选一窗（允许一次 / 始终允许该目录 / 拒绝，60s 超时与挂件不可见默认拒绝）；yolo → 直接放行；全程记审计（`bot_fs.yolo_allow / ask_allow_once / ask_allow_always / ask_denied`）
- 「始终允许该目录」：文件取父目录、目录取自身，自动追加进 `allowedDirs` 落盘（`bot::add_allowed_dir`）
- **allowedDirs 语义修正**：旧「非空整体替换内置默认」→ 新「在内置默认（桌面/下载/文档 + 任务卡绑定文件夹）之上**追加**」——否则始终允许写入一个目录后默认目录反而失效
- extract_document / create_word_revisions 的 path 校验改走同一分流（任务卡绑定文件 / AI_Gen_Files 仍静默放行）
- run_python：yolo 模式跳过 py-enabled 开关（记 `py_exec | yolo_bypass`）；ask/strict 维持设置页开关门控
- 前端：ChatPanel 确认弹窗按 `kind="file_access"` 渲染三按钮（`bot_confirm_response` 加 `always` 参，老调用兼容）；SettingsPage 加授权模式三态选择器（yolo 带风险提示）+ 白名单文案改追加语义
- prompt 规则 19 / 安全红线 / TOOLS 描述同步「弹窗授权」措辞
- 测试：PermMode 解析/老配置兼容、`merge_raw_dirs` 追加语义；顺带修 eefa78f 遗留的两个前端测试（设置页出现两组「📤 导出/📥 导入」按钮导致 getByText 二义性，改取第一个 = 任务导入/导出）
- 验证：`cargo test --lib` 407 全绿；`npm test` 107 全绿

## 2026-08-13（周四）M1 脚手架 + M2 看板 + M3 起步

### M1 脚手架
- create-tauri-app react-ts 模板：Tauri 2 + React 19 + TypeScript + Vite 7 + Tailwind v3
- 依赖：@dnd-kit/core、@dnd-kit/utilities、postcss、autoprefixer
- 新拟态样式体系建在 `src/ui/main.css`（`@layer components`）

### M2 看板
- `KanbanBoard.tsx`：dnd-kit 三列（待办/今日/完成），PointerSensor distance:5，列间拖拽
- localStorage 持久化；新建任务自动进标题编辑态
- 卡片结构顺序（固定，别再改）：标题 → 备注 → 标签 → 子任务 → 文件按钮 → 截止时间（**截止永远最底**）
- 列头最终版：全宽 `nm-inset` 凹陷胶囊 + text-lg font-semibold（苹方/SF Pro），标签顶左、计数顶右
- **今日规则**：截止日期=当天 且 列=待办 → 自动进「今日」列；每分钟 + 设 due 时 + 启动时套用；完成列豁免

### M3 文件绑定（起步）
- Rust 命令 `copy_file_with_title(path, title)`：macOS 用 NSPasteboard（public.file-url + NSFilenamesPboardType + 标题文本），Windows 用 CF_HDROP（cfg-gated，未编译验证）
- 前端：📎 绑定文件 / 📁 绑定文件夹（plugin-dialog），📂 打开（plugin-opener），📋 复制文件+标题，× 解绑

## 2026-08-14（周五）M3 收尾 + 归档回收站 + M4 挂件 + 系列迭代

### M3 完成 + 归档 + 回收站
- `copy_file_with_title` 编译通过，粘贴到 Finder/飞书/微信可得文件，文本场景得标题
- **自动归档**：进完成列记 `completedAt`，满 7 天自动 `archived`（`ARCHIVE_AFTER_MS`，每分钟检查）
- 归档页：搜索（标题+备注）+ 标签筛选（计数降序）+ ↩ 恢复；回收站：软删除 + 恢复/彻底删除/清空
- 主窗口头部三视图切换：未选中 `nm-outset` 凸起 / 选中 `nm-inset` 凹陷，文字颜色统一

### M4 侧边磁吸挂件
- Rust 侧创建 `widget` 窗口：transparent + decorations(false) + always_on_top + skip_taskbar + visible_on_all_workspaces（macOS 透明窗口必须 `macOSPrivateApi: true`）
- URL hash 分流：`index.html#/widget` → WidgetApp
- 收起 = 44×220 触发条，悬停展开 320×560 面板，📌 锁定常驻
- **数据同步三保险**：localStorage 同源共享 + `tasks-changed` tauri 事件 + 5s 兜底轮询

### 挂件与主窗口一致性迭代（老板逐条验收）

| 时间 | 变更 |
|---|---|
| 05:42 | 挂件任务卡与主窗口显示一致：新增 `TaskCardContent` + `format.ts`（basename/formatDue 共用） |
| 05:50 | 标题右侧打勾圆圈：新增 `DoneCircle` 共享组件，主窗口+挂件都可一键完成/取消完成 |
| 05:57 | 标题以下折叠：新增 `FoldToggle`，`Task.collapsed` 字段持久化，两窗口同步 |
| 06:02 | 主窗口删除按钮移到截止日期右侧，改小 emoji 🗑️（回收站视图不重复显示） |
| 06:07 | 挂件子任务可勾选 + 📂/📋 文件按钮（与主窗口一致） |
| 06:10 | 头部切换按钮凸/凹按压态（新增 `.nm-outset`） |
| 06:14 | 任务卡标题字体统一 14px/500（新增 `.nm-task-title` 共用类） |
| 06:22 | 挂件圆角处阴影段修复：外阴影漏进透明切角 → 改内阴影 `inset -6px 0 10px -4px` |
| 06:31 | 挂件新建任务 + 标题编辑（新建自动进编辑态） |
| 06:40 | 挂件点标题 → 跳主窗口并进入该任务编辑态（`edit-task` 事件 + 主窗口 setEditingId） |
| 11:07 | 挂件 logo 定 #3 深蓝双方块（老板拍板）：触发条 🗂 表情 → 24px 透明 PNG，面板头部加 20px logo；资产 `src/assets/widget-logo.png`（源 `docs/logo/assets/3/`） |
| 11:12 | 贴顶时触发条竖变横（220×44，flex-row + 水平文字 + 上缘内阴影 `.nm-sidebar-panel-top`）；初始定位与 collapse 按 edge 选择横/竖尺寸 |
| 11:18 | 挂件头部「全部/今日/锁定」按压态：未选中 `nm-outset` 凸起胶囊 → 选中 `nm-inset` 凹陷胶囊（与主窗口头一致） |
| 11:22 | 文件打开/复制按钮（主窗口 TodoCard + 挂件 TaskCardContent）：新增 `.nm-btn` 动作按钮类，默认凸起、`:active` 按下瞬间凹陷（动作按钮用 momentary 按压态，不用常驻切换） |
| 11:24 | 绑定文件/文件夹按钮（📎/📁 幽灵文字 → `nm-btn` 凸起胶囊 + 按压态） |
| 11:27 | 剩余动作按钮统一 `nm-btn`：归档 ↩恢复、回收站 ↩恢复/🗑彻底删除（红字保留）、挂件 +新建任务长条（常驻 nm-inset 仅保留列头/输入框/标签芯片等非动作元素） |
| 11:32 | 出 Windows 包 v2（含 SQLite/挂件 logo/贴顶横条/按钮体系全套改动），飞书发送老板验收 |
| 11:36 | 文档更新：README 全面刷新（SQLite 架构、单写者同步、交叉编译说明、logo 文档指针）+ DEVLOG 待办刷新 |
| 06:47 | 主窗口关闭改为隐藏（CloseRequested prevent_close + Cmd+Q 走 ExitRequested destroy） |
| 07:09 | 挂件新建任务长条移到「全部/今日」下方，新任务从列表顶部出现 |
| 07:28 | 挂件打勾后任务消失（挂件只显示未完成：todo + doing） |
| 07:36 | 挂件自由拖动 + 贴边吸附：右/左/顶 24px 容差、圆角跟随边缘、锚点持久化（`wmessage-widget-pos`） |

### 存储落盘（方案2）+ Logo 定稿 + Windows 交叉编译验证（09:22-10:40）

- **Logo 定稿**：老板 5 张豆包渐变玻璃质感图，裁定以图片为准；去水印/透明底/多尺寸 → `docs/logo/assets/`；规范 `docs/logo/WMessage-LOGO-GUIDELINES.md`；#4 生成全套 Tauri 图标到 `src-tauri/icons/`
- **Windows 剪贴板修复**：`copy_file_windows` 首次编译验证（cargo check 交叉目标），windows 0.61 API 修正见踩坑记录
- **Windows 交叉编译**：产出 9.5MB 独立 exe（静态 CRT），Win10 实测可运行
- **存储落盘（方案2）**：任务数据 localStorage → 应用数据目录 `data.json`（Win: `%APPDATA%\com.renshi.wmessage\data.json`），防清理工具误删。Rust `load_data`/`save_data`（原子写）；单写者：主窗口统一落盘，挂件只读 + `tasks-updated` 携带数据上报；旧 localStorage 首次启动自动迁移；POS_KEY（挂件锚点）仍走 localStorage

### 存储换 SQLite（方案B，10:54 老板拍板）

- 弃 data.json 全量覆盖，上 rusqlite（bundled）行级增量：`db_load`/`db_upsert`/`db_delete` 三命令（`src-tauri/src/db.rs`），表 tasks 单表，tags/subtasks 存 JSON 文本列，WAL + busy_timeout 2s
- 前端统一变更出口 `mutate`（App.tsx）：计算新数组 → diff 出 upserts/deletes → 行级落盘 → 广播 `tasks-changed`；挂件 `applyAndSync` 同样 diff 后经 `tasks-updated` 上报 {upserts, deletes}，主窗口统一落盘（单写者不变）
- 迁移链：SQLite 空库时 Rust 侧自动导入方案2 的 data.json 并删除；更早的 localStorage 数据由主窗口首次启动导入后清除
- 规则（今日/归档）照旧在加载与每分钟重套，diff 后行级落盘

## 踩坑记录（避免重蹈）
- Tailwind `@apply` 不能引用自定义组件类（`.nm-card-hover { @apply nm-card }` 编译报错）
- TodoCard 的 useDraggable 在 DndContext 外会崩 → 归档/回收站页必须包空 `<DndContext>`
- 卡片内裸 `<button>` 是 inline 会并排 → 注意 display（块级化）
- macOS 透明窗口：`tauri.conf.json` 必须 `"macOSPrivateApi": true`
- `WebviewWindow.getByLabel` 返回 Promise，必须 await
- **矩形透明窗口里的圆角面板禁用外阴影**（直边被裁、圆角漏光，形成"一段阴影"）→ 用 inset 内阴影
- 主窗口关闭=销毁会让挂件失去唤起目标 → CloseRequested 拦截改隐藏；Cmd+Q 在 ExitRequested 里 destroy 主窗口
- Rust `get_webview_window` 需要 `use tauri::Manager;`
- Windows 交叉编译链路（macOS → exe）：`brew install llvm lld` + `cargo install cargo-xwin`，然后 `tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc`；缺 llvm-rc 会报 `tauri-winres NotAttempted("llvm-rc")`，缺 lld-link 链接阶段挂
- windows crate 0.61 API 大改：`GlobalAlloc/GlobalLock/GlobalUnlock/GMEM_MOVEABLE` 在 `System::Memory`；`CF_HDROP/CF_UNICODETEXT` 在 `System::Ole` 且为 `CLIPBOARD_FORMAT(u16)` 新类型（传给 `SetClipboardData` 用 `.0 as u32`）；`SetClipboardData` 第二参是 `Option<HANDLE>`（与 `HGLOBAL` 不同新类型，需 `HANDLE(h.0)`）；`GlobalLock` 返回裸指针不是 Result；`BOOL` 只有 `From<bool>`（用 `true.into()`）
- **localStorage 会被清理工具当缓存删**（EBWebView 目录），关键数据必须落盘到 app_data_dir 的 data.json（temp+rename 原子写）
- 落盘架构单写者：主窗口统一写文件，挂件只上报 `tasks-updated`（携带数据）；主窗口 persist 用 `loaded` 门控，否则启动瞬间 async 加载完成前会写空数据覆盖旧档
- 引入 C 依赖（rusqlite bundled）后，Windows 目标裸 `cargo check` 会挂（cc-rs 用宿主 cc 编 sqlite3.c 找不到 stdlib.h），必须 `cargo xwin check --target x86_64-pc-windows-msvc`（cargo-xwin 接管 C 编译器 + SDK 头文件）

## 后续待办

- M5 全局快捷键（已做 ✓）
- M6 打包：交叉编译 exe 已通 ✓（v2 已发验）；剩 NSIS 安装包 + 代码签名 + macOS dmg
- Logo 规范 2.3 单色托盘版（16/32px，现有渐变图缩到托盘尺寸会糊）
- 挂件窗口伸缩平滑动画（当前瞬时伸缩）polish
- ~~深色模式~~ ✓（23:30-23:55 完成，见下节）
- 数据备份/导出（可选；SQLite 文件本身可整体拷贝）

### 深色模式（23:30-23:55，老板逐条验收）

- **主题变量体系**（`src/ui/main.css`）：`:root` / `.dark` 两套独立变量——`--bg` 页面底（深色 #1f242b 中度深灰非纯黑，比卡片表面更暗拉层次）、`--surface` 卡片/按钮表面（#2d333e）、`--sh-low/high`（+strong/hover 档）双阴影（深色专属：更深暗阴影 #13161b + 偏亮弱高光 #4a5464，不复用浅色参数）、`--sh-top` 顶部 1px 内高光（深色玻璃边缘感，浅色 transparent 关闭）、`--edge`、`--input-bg`、`--hover-bg`、文字五级 `--t1..t6`（深色主文字 #f0f3f8 非纯白，逐级降亮但保证可读）、品牌色 `--brand/--brand-strong`（低饱和蓝紫灰，深色提亮 #a3b6dd）、`--success/--danger`
- **nm 组件全部走变量**：nm-card（凸起双阴影+顶部内高光）、nm-inset（内双阴影）、nm-outset、nm-btn（hover 阴影加深/active 内凹），hover/active 两套都有清晰反馈；主题切换平滑过渡（background-color/box-shadow 0.28s，body color 同步）
- **组件颜色全部改变量引用**（`text-[var(--tN)]` 等，脚本批量替换 9 个文件）：看板卡片、折叠卡、归档/回收站、挂件面板、按钮、输入框（含 placeholder 颜色）、checkbox accent 走品牌色
- **主题三态**（`src/theme.ts`）：light/dark/system，localStorage 持久化；system 用 matchMedia 监听系统外观实时切换（含旧 WebView addListener 兼容）；双窗口 storage 事件同步；index.html 预渲染脚本防闪屏；header/挂件按钮三态循环（☀️/🌙/🖥️），Rust 侧 Cmd+Ctrl+T 快捷键快速浅/深切换（emit toggle-theme 只发主窗口，挂件靠 storage 同步防双触发）
- **挂件深色适配**：深蓝 logo 提亮（`.dark .widget-logo` brightness 1.9）
- 验证：tsc/cargo check/npm run build 全过；dev 实例实测浅色正常、快捷键切深色后像素采样+vision 确认（背景中度深灰、卡片与背景层次分明、文字层级可读、双阴影可见、挂件同步变深）

### 平台窗口关闭行为（老板验收，11:42–11:52）

| 时间 | 变更 |
|---|---|
| 11:42 | 主窗口 board 视图「+ 新建任务」按钮 `nm-inset` → `nm-btn`（动作按钮类，凸起/按下凹陷） |
| 11:50 | Windows 关主窗口=隐藏进托盘（不再直接退出）；托盘图标用 **#3**（`docs/logo/assets/3/`）；右键菜单「打开主窗口 / 退出」，**退出才是真退出**（`app.exit(0)` → `ExitRequested` → destroy main）；左键单击/双击托盘恢复主窗口 |
| 11:52 | macOS 行为不变：关闭=隐藏，Cmd+Q 真退出；托盘仅 Windows（`#[cfg(target_os = "windows")]`） |
| 12:17 | 任务卡拖拽手柄：主窗口+挂件标题前拖拽区，悬停才显示 ✋ 小手图标；主窗口 listeners 从整卡移到手柄（只能从手柄拖）；挂件手柄仅展示不参与拖拽 |
| 12:17 | 挂件任务卡标题：单击 → 双击才打开主窗口编辑（openInMain） |
| 13:31 | Win10 缺 WebView2 Runtime 报 webview2loader.dll 找不到：便携包内附微软官方 Evergreen 安装器 MicrosoftEdgeWebview2Setup.exe + README.txt（先装一次再跑 exe；Win11 自带无需装） |
| 12:26 | 任务卡拖拽排序：主窗口三列内排序 + 跨列（@dnd-kit/sortable 多容器 SortableContext + DragOverlay），挂件列表排序（DndContext + SortableTaskCard）；Task 加 `order` 字段（SQLite `ord REAL` 列 + 老库 ALTER 迁移），排序用左右邻居中点（边界 ±1、间隙耗尽全量整数重排，`assignInsertOrder`）；挂件排序后拖拽 click 防误聚焦（200ms 守卫） |

- Cargo.toml tauri features 增加 `tray-icon`、`image-png`；托盘图标生成自 `3-1024.png` → `src-tauri/icons/tray-wm-32.png`（另备 16px）
- 托盘 API 验证：macOS `cargo check` 通过；托盘代码块临时去 cfg 门在 macOS 编译验证 0 错误后恢复
- Windows 交叉 check 被 `libsqlite3-sys` C 交叉编译卡住（本机无 Windows C 工具链），Windows 侧需实机 build 验证

### 便携模式 + 合并导入 + 任务卡交互大改 + 打包定案（13:50-17:25，老板逐条验收）

| 时间 | 变更 |
|---|---|
| 13:50 | **便携模式**：数据库随 exe 走（写探针检测 exe 目录可写性，不可写兜底 app_data_dir，首次启动自动迁移旧库），U 盘拷走数据随行 |
| 14:02 | **合并导入**：「导入数据库」选 wmessage.db 按 id 并集合并，同 id 保留 `updatedAt` 更晚者；Task 加 updatedAt（写路径自动打戳，老数据回填 0）；外部库只读打开、缺 ord/updated_at 列容忍 |
| 15:52 | 「🗂 导入数据库」改名「导入数据」，仅首页显示 |
| 16:00 | 任务卡交互：删隐藏 ✋ 改 ☰ 三横线拖拽手柄（悬浮标题左侧留白）；折叠 chevron 移到标题正下方居中细行 |
| 17:00 | ☰ 手柄太靠边/离标题太近 → 改标题左侧占位，标题行 gap-2 |
| 17:13 | 折叠 chevron 移到标题行内、标题与对勾之间，三角符号加大；标题折叠时单行 truncate（悬停 tooltip 全文），展开才显示全部标题和设置 |
| 17:19 | **拖拽 bug**：待办拖不进空完成列——松手时指针落在卡片自身新位置（over=active）早退丢草稿 → 改为草稿已跨列时按草稿提交 |

- **Windows 打包定案（16:46 老板确认，必须照此执行）**：`cargo clean` 全量重编 + `npx tauri build --target x86_64-pc-windows-gnu --no-bundle` 完整流程；**直接 `cargo build` 出的 exe 缺内置页面资源**（报「无法访问此页面」）
- 交叉编译链路换 mingw-w64 + `x86_64-pc-windows-gnu`（`CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc`，CC/CXX 同理）；NSIS 在 macOS 打不了（makensis 跨平台崩），发绿色版 zip
- **绿色包**：wmessage.exe + WebView2Loader.dll（必带，缺它报「找不到 webview2loader.dll」，与 WebView2 Runtime 无关）+ MicrosoftEdgeWebview2Setup.exe（备用）+ README.txt
- **zip 必须用 Python zipfile 打**：macOS `zip -j` 的 Unix 扩展字段让 Win 资源管理器解压报「位置不可用」

### 归档/回收站三列改造（22:15-22:38）

| 时间 | 变更 |
|---|---|
| 22:15 | 归档页：单列长卡 → **按标签分组的三列小窗口**（标签名作标题 + 计数，任务卡竖排窗口内；新标签自动新建窗口；无标签归「未分类」；搜索过滤隐藏空窗口） |
| 22:28 | 归档窗口标题加 # 前缀（代表标签）+ 窗口可拖拽换列（标题行手柄 + rectSortingStrategy），顺序存 localStorage 持久化；搜索过滤态禁拖 |
| 22:34 | 拖拽激活 bug：`onPointerDown={undefined}` 写在 listeners spread 之后把 dnd-kit 的 onPointerDown 覆盖没了 → 仅无手柄时覆盖 stop |
| 22:38 | 回收站页：单列 → 三列网格（grid-cols-3，从左到右依次填充） |
| 22:41 | 文档更新（本次） |

### M5 全局快捷键（22:53-23:00）

- 引入 `tauri-plugin-global-shortcut = "2"`（Rust 侧注册，无需前端 capabilities）
- **唤起/隐藏主窗口**：macOS `Cmd+Ctrl+W` / Win+Linux `Ctrl+Alt+W`（可见则隐藏，隐藏则 show+unminimize+focus）
- **快速新建任务**：macOS `Cmd+Ctrl+N` / Win+Linux `Ctrl+Alt+N`（唤起主窗口 + emit `quick-add`，前端切回首页 + addTask 进入标题编辑态）
- 键位选择避开冲突：macOS 不用 Cmd+Shift+N（Finder 新建文件夹）、Win 不用 Ctrl+Shift+N（浏览器隐身窗口）；Modifiers 用 cfg 按平台选 SUPER|CONTROL / CONTROL|ALT
- 踩坑：global-hotkey 0.8 `Shortcut::new` 返回 Self 不是 Result（不能 `?`）；`with_handler` 的 setup 闭包引用 hotkey_mods 需加 `move`
### 本地 HTTP API（外部机器人接口，00:06-00:50）

- 老板需求：可选本地 HTTP 接口（默认关闭），Bearer token 鉴权，REST CRUD + SSE，不动现有业务代码，禁止 0.0.0.0 与文件遍历。**明确不写任何大模型调用/function-call 逻辑**
- 实现（`src-tauri/src/api.rs`，tiny_http 0.12 + uuid）：
  - 端口 4763，只绑 127.0.0.1；全部端点 Bearer 鉴权（401 否则）；CORS 全开（本地友好）
  - GET /api/tasks、GET /api/tasks/:id、POST /api/tasks（title 必填，status/note/filePath/fileIsDir/due/tags 可选，status∈todo/doing/done 校验）、PUT /api/tasks/:id（部分更新，进 done 记 completedAt/出 done 清除）、GET /api/events（SSE）
  - 对外 JSON 形状：db::Task 字段 + `status` 别名（=column）
  - 任务变更后：SSE 广播 tasks-changed + emit_to("main","tasks-updated") → 看板自动刷新（复用挂件→主窗口既有通道）
  - token 持久化到数据目录 api-token.txt（uuid v4），`load_or_create_token`；api_start/api_stop/api_status 三个命令；ApiState 托管（默认关闭）
  - 数据访问抽象 TaskStore trait：生产 TauriStore（走 db_load/db_upsert），单测 MemStore
  - **单测**（cargo test --lib api，2 个全过）：401/CRUD/400/404 + SSE 收到 tasks-changed
  - 前端：SettingsPage.tsx（开关 nm-outset/nm-inset 胶囊 + 运行状态 + token 只读框 + 复制按钮 + 接口清单），App.tsx 加「设置」视图与 header 按钮
- **坑**：tiny_http `Response::new` 流式 reader + `req.respond` 会缓冲到连接结束才 flush，SSE 长连首帧出不去 → 改用 `req.upgrade("text/event-stream", resp)` 拿原始 ReadWrite 流直接 write+flush（15s 心跳 `: keepalive`）；`Shortcut`-式教训：tiny_http respond 与 upgrade 不可混用
- 验证：cargo check/test、tsc、npm run build 全过；dev 实例已重建运行，老板在设置页开启后可用 curl 验证

### 本地 HTTP API 升级（07:24-07:40，老板定 P0/P1/P2 全做）

- **P0 功能补齐**：PUT 支持 due/tags/archived/deleted（空串清 due、空数组清 tags）；DELETE /api/tasks/:id 软删进回收站（幂等）；GET /api/tasks 过滤（?status=todo|doing|done、?trash=1、?archived=1、?all=1，默认活跃任务）；开关持久化（api-enabled.flag，重启自动恢复 API）
- **P1 安全**：去 CORS 头与 OPTIONS 预检；请求体 1MB 上限（413）；token 轮换（api_rotate_token 命令 + 设置页「重新生成」按钮）；限流 120 次/分（429）
- **P2 可靠性**：SSE 事件 id + `?since=` 断线重放（EventHub：原子自增 id + 1000 条环形历史）；操作日志 data_dir/api.log；DB_WRITE_LOCK 静态锁包住 db_upsert/db_delete/db_merge（API 线程与主窗口并发写安全）；GET /api/health 免鉴权健康检查
- 测试：cargo test --lib api 2 个全过（覆盖过滤、软删幂等、恢复、due/tags 更新、非法 status 400、health）

### 桌面文件自动迁移清理（09:48-10:15）

- 老板需求：任务完成满 7 天进入归档后，按用户规则表自动迁移其绑定文件（移动到归档目录并更新 filePath，附件链接不断）；规则表可增删改/上传/模版下载，不改代码；保护看板任务附件；delete 规则默认关闭；手动触发 + 定时轮询；异常记日志不崩溃；Win/Mac 兼容
- 实现（`src-tauri/src/migration.rs`）：
  - 规则模型 MigrationRule { id, enabled, keywords, action(move|delete), archiveDir }；规则表存数据目录 cleanup-rules.json（serde camelCase）
  - 引擎 run_migration：阶段一归档到期任务（done 满 7 天 → archived=true，主窗口关闭时也照常）；阶段二对 archived 任务按规则顺序匹配关键字（大小写不敏感）执行 move/delete；成功后 db_upsert 更新 filePath 并 emit tasks-updated（source:"migration"）
  - 保护：仅 archived=true 且未删除的任务参与；看板/回收站附件绝不触碰
  - 归档目录 {year} 占位符；相对路径基于桌面（app.path().desktop_dir()），绝对路径原样；同名冲突自动 `名 (n).ext`
  - move：rename 优先，失败（跨卷）退化 copy+remove；delete：remove_file/remove_dir_all
  - 轮询：spawn_polling 后台线程（启动 60s 后首跑，此后每 10 分钟）；RUNNING AtomicBool 防手动/轮询重入
  - 日志：数据目录 migration.log（[YYYY-MM-DD HH:MM:SS] 行）
  - 命令：migration_rules_load/save/import/template_save/run/status（import 用 dialog 选 JSON，template_save 用 dialog 存模版）
- 前端：MigrationPanel.tsx 设置页新卡片——规则表逐行编辑（启用/关键字/动作/归档目录/删除行）、添加/保存/下载模版/导入规则表/立即执行、上次执行摘要 + 日志尾部展示；切换为 delete 动作默认关闭并显示 danger 提示；types.ts 加 MigrationRule/MigrationReport；App.tsx 监听器 source!=="migration" 才回写落盘
- **踩坑**：MigrationRule 最初没加 `#[serde(rename_all = "camelCase")]`，导入的规则表 archiveDir 字段反序列化成空串 → 迁移把文件移到了桌面根目录（实测发现）；加 rename_all + 专门的 serde 测试防回归。教训：JSON 模型字段名与前端约定必须显式对齐并测序列化
- 验证：cargo test 9 个全过（关键字匹配/大小写/空关键字/冲突命名/年份展开/规则校验/serde camelCase）；tsc + cargo check 过；运行时实测 4 场景——归档任务命中 move 规则被正确移动且 filePath 更新、无命中不动、delete 规则禁用不动、看板任务命中规则也不动（保护逻辑生效）

### 工作区（静态链接收藏，12:27-12:40）

- 老板需求：工作区按钮放主窗口「归档」「回收站」之间、挂件「今日」「锁定」之间；UI 类似任务卡（标题 + 折叠），展开后增删链接，可连接文件/文件夹/网址
- 实现：
  - db.rs：新表 workspace_items（id/title/collapsed/links JSON/ord/updated_at）+ WorkspaceLink/WorkspaceItem 结构（serde camelCase）+ workspace_load/workspace_upsert/workspace_delete 命令（DB_WRITE_LOCK）
  - WorkspacePage.tsx：主窗口视图——卡片标题内联编辑、FoldToggle 折叠、链接行（🔗/📄/📁 图标 + label + target + 悬停删除）、添加链接三入口（网址输入 / 文件对话框 / 文件夹对话框）、新建工作区大长条；链接打开：url → openUrl，file/folder → openPath
  - 挂件：view 加 "workspace"，按钮在今日与锁定之间；工作区视图只读展示卡片 + 链接点击打开；workspace-changed 事件 + 5s 轮询同步；工作区视图隐藏「+ 新建任务」
  - 事件：主窗口编辑后 emit("workspace-changed")；挂件 listen 后重读
- 踩坑：挂件 JSX 条件渲染改坏结构（div 重复）→ tsc 报错，修正；App.tsx 漏 import WorkspacePage → tsc 报 Cannot find name，补 import
- 验证：cargo check + tsc 过；dev 实例自动重建（12:33）；SQLite 探针插入/查询/删除 workspace_items 正常

### 内置机器人聊天（21:18-21:52，第一步：开关 + 聊天窗口本体）

- 老板确认 m-message = WMessage；机器人方案：设置页开关 + 挂件下方聊天区（原挂件窗口 1/2 高度）
- 实现：
  - bot.rs：bot_get_enabled/bot_set_enabled（bot-enabled.flag 持久化，套路同 api-enabled.flag）；bot_get_config/bot_set_config（bot-config.json：baseUrl/apiKey/model，默认 DeepSeek）；bot_chat——OpenAI 兼容流式（reqwest 0.13 + futures-util），delta 经 bot-chat-delta 事件推挂件窗口，带 4 个工具（list/create/complete/delete_task）多轮循环（≤6），工具进程内直改 SQLite，改后广播 tasks-changed + tasks-updated(source:"bot")
  - App.tsx：tasks-updated 监听器 source!=="bot" 才回写（同 api/migration 处理，防异步回写覆盖）
  - WidgetApp.tsx：botOn 状态（挂载读 + bot-changed 事件）；展开高度 = PANEL_H + CHAT_H(280=1/2)；聊天区挂任务列表下方（border-t 分隔）
  - ChatPanel.tsx：消息列表（用户 nm-outset 右 / 助手 nm-inset 左）+ 流式增量追加 + Enter 发送 + 清空对话 + 空态引导
  - SettingsPage.tsx：「设置」卡片改「机器人设置」；开关 + 开启后显示大模型 API 配置（Base URL/API Key/模型 + 保存）；开关切换 emit bot-changed 同步挂件窗口高度
- 验证：cargo check ✓、tsc ✓、dev 实例自动重建（21:51:37）；视觉验证未做（Mac 锁屏，截屏只见壁纸）
- 待验证：老板解锁后——①设置页开关与 API 配置保存 ②挂件展开出现聊天区、高度 840（560+280）③配好 API Key 后发消息流式回复 ④「新建任务/完成某任务」工具调用生效
- 待办：聊天记录持久化（目前仅内存）、文档处理（Excel/Word/PPT/PDF）、Windows 打包

### API Key 迁入系统凭据存储（22:32-22:38，老板拍板：用凭据管理）

- 背景：老板问 key 安全性（是否会流到外网）→ 答复只发给配置的 Base URL；风险点：本地明文文件。老板选 keychain/凭据管理方案
- 实现：
  - Cargo.toml + keyring crate（features: apple-native-keyring-store；windows-native-keyring-store 默认启用）——macOS 钥匙串 / Windows 凭据管理器统一 API
  - bot.rs：KEYRING_SERVICE="wmessage-bot" / USER="api-key"；read/write/has/clear 四个辅助；BotConfig 去掉 apiKey 字段（保留 Option 仅迁移用）；bot_get_config 返回 BotConfigView{baseUrl,model,hasApiKey}（不回传 key 本体）；bot_set_config 新签名（config + api_key: Option，非空才写凭据存储，留空不动旧 key）；新增 bot_clear_api_key；bot_chat 从凭据存储读 key
  - migrate_legacy_key：App 启动（lib.rs setup）+ 设置页读配置时兜底调用——老 bot-config.json 明文 key 迁入凭据存储后从文件清除（凭据存储已有 key 时不覆盖）
  - SettingsPage：key 输入框不回显（hasApiKey 显示 ✓ + placeholder"输入新 Key 可覆盖"）；保存只传输入框内容（空=不动）；「清除已保存的 Key」按钮（confirm 后 bot_clear_api_key）
- 验证：cargo check ✓ tsc ✓；dev 重建（46626, 22:36:49）；实测迁移——重启后 bot-config.json 只剩 baseUrl/model（明文 key 已清），security find-generic-password -s wmessage-bot -a api-key 能读到 key ✓
- 注意：keyring 在 macOS 未签名 dev 构建首次访问钥匙串会弹授权框（正常）；Windows 侧凭据管理器行为待老板 Win 机实测

### 机器人工具扩展：编辑/子任务/绑定文件（22:45-22:50）

- 老板点单：编辑任务、子任务、绑定文件夹
- bot.rs 新增 4 个工具：
  - edit_task：按标题关键词匹配未完成任务，可改 newTitle/note/due/column/tags；空串清字段；列变更补完成语义（进 done 记 completedAt、出 done 清除，与主窗口一致）；无有效字段返回"没有可修改的字段"
  - add_subtask：给任务追加子任务（uuid id、done:false）
  - toggle_subtask：按子任务内容关键词匹配，翻转 done
  - bind_file：isDir=true/false 弹系统文件/文件夹选择框（spawn_blocking + blocking_pick_file/pick_folder，避免卡异步运行时），结果写 filePath/fileIsDir；取消返回"用户取消了选择"
  - 重构：find_task_by_keyword 辅助函数（大小写不敏感 contains）；execute_tool 改 async（bind_file 需要 await spawn_blocking）
  - SYSTEM_PROMPT + TOOLS JSON 同步更新（职责描述 + 8 工具 schema）
- 验证：cargo check ✓ tsc ✓ dev 重建（47308, 22:49:17）
- 待老板实测：编辑/子任务/绑定弹框流程（尤其 macOS 弹框与钥匙串授权体验）

### 挂件内选任务操作（22:59-23:05）

- 老板反馈：打标题费时，想在挂件里直接选任务操作机器人
- 交互设计：聊天区加「🎯 选任务」按钮进入选择模式 → 点任务卡标题切换选中（ring-2 高亮，不再打开主窗口）→ 选中任务以 📌 引用块显示在输入框上方（可单删）→ 输入指令发送 → 消息自动附 [已选任务] 块（id+标题）→ 发送后清空选择退出模式
- Rust 工具侧：complete/delete/edit/add_subtask/toggle_subtask/bind_file 新增 taskId 参数（精确匹配优先），resolve_task 统一定位（taskId → title 关键词）；list_tasks 输出带 id 供模型引用；SYSTEM_PROMPT 加规则：消息含 [已选任务] 时强制用 taskId
- 验证：cargo check ✓ tsc ✓ dev 重建（48070, 23:04:47）运行中
- 待老板实测：选任务模式交互（点击选中/取消、引用块、发送后清理）

### 选任务模式修复（23:08-23:11）

- Bug：选任务模式点不了任务卡——标题绑 onDoubleClick（双击才开主窗口），单击无选中反应
- 修复：SortableTaskCard 加 selectMode/onSelect props；selectMode 下根 div onClick 整卡选中 + cursor-pointer；onTitleClick 传 undefined（暂停双击跳主窗口）；卡内交互按钮均 stopPropagation 不受影响
- 验证：tsc ✓ dev 重建（49146, 23:11:06）；老板实测 ✓ 可批量选中一次标记多个任务完成

### 搜索任务卡工具 search_tasks（23:14-23:16）

- 老板需求：机器人加搜索任务卡功能
- bot.rs 新增第 9 个工具 search_tasks：
  - query 关键词匹配标题/备注/标签/子任务（大小写不敏感 contains）
  - status 参数：active=仅未完成（默认），all=含已完成
  - 结果按 order 排序，带 id（供后续 taskId 精确操作）、状态列、截止时间、标签
  - 无命中返回"没有找到匹配…"
- SYSTEM_PROMPT 加规则：用户想找任务时调用 search_tasks；工具 schema 同步
- 验证：cargo check ✓ dev 重建（49372, 23:15:51）运行中

### 搜索结果可点击跳任务（23:21-23:25）

- 老板反馈：机器人回复的搜索结果想手动点进去
- 实现：
  - bot.rs：TaskRef{id,title} + BotChatResult{text,taskRefs}；execute_tool 返回 (文本, Vec<TaskRef>)，全部 9 个工具在成功时带上涉及的任务引用（list/search 带全部命中）；bot_chat 收集后按 id 去重返回
  - ChatPanel.tsx：invoke 解析 taskRefs，助手消息气泡下方渲染 📌 任务按钮（truncate + title 提示）；点击 → emit edit-task + 聚焦主窗口（主窗口打开该任务编辑态）
- 验证：cargo check ✓ tsc ✓ dev 重建（50331, 23:24:35）运行中
- 待老板实测：搜索后点 📌 按钮是否跳主窗口打开任务

### search_tasks 改为只搜归档（23:31-23:33）

- 老板裁定：搜索工具只搜归档文档，不要搜待办/今日/完成（活跃任务看板挂件直接可见，无需机器人搜）
- 实现：
  - bot.rs：tool_search_tasks 过滤条件 t.archived == Some(true)（原来排除归档，现只搜归档）；删掉 status 参数（schema + 实现同步）；无命中提示改「归档里没有找到匹配…」；SYSTEM_PROMPT 规则 4 改：活跃任务用 list_tasks、归档任务用 search_tasks
  - App.tsx：edit-task 监听判断任务 archived → 跳归档页而非看板（机器人搜归档任务点 📌 按钮落点正确）
- 验证：cargo check ✓ tsc ✓ dev 重建（50916, 23:32:57）运行中

### search_tasks 改回搜索全部任务卡（23:37-23:39）

- 老板再裁定：所有任务卡都能搜索最好（推翻 23:31 的"只搜归档"）
- 实现：
  - bot.rs：tool_search_tasks 过滤改为仅排除回收站软删（deleted_at.is_none()），覆盖待办/进行中/已完成/已归档；结果行对已归档任务加「（已归档）」标记；无命中提示改回「没有找到匹配…」；SYSTEM_PROMPT 规则 4 与 TOOLS schema 描述同步（搜所有任务卡）
  - 保留：App.tsx 点 📌 跳转逻辑（归档→归档页，其余→看板）；resolve_task 仍只定位未完成任务（操作类工具边界不变）
- 验证：cargo check ✓ tsc ✓ dev 重建（51495, 23:38:13）运行中

### 机器人安全优化（23:44-23:50，参考老板发的《Harness 安全网关需求》）

- 老板发来《WMessage 最终完整版 AI 助手 + Function 工具 + Harness 安全网关完整架构需求》，要求参考并优化机器人安全
- 对照七层网关做差距分析后落地 5 项（不打断聊天体验的部分）：
  - 审计日志（第 7 层）：数据目录 bot.log——用户原始指令（截 300 字）、工具名、入参（截 500 字）、执行结果（截 300 字）全留痕；truncate_for_log 按字符数安全截断防刷日志
  - HTTP 超时熔断（第 5 层）：reqwest connect_timeout 15s + 总超时 300s，杜绝请求挂起卡死聊天
  - 参数上限校验（第 3 层）：MAX_TITLE 200 / MAX_NOTE 5000 / MAX_KEYWORD 100 / MAX_SUBTASK_TEXT 200 / MAX_DUE 30 / 标签 ≤10 个且每个 ≤30 字；接入 create/edit/search/add_subtask/toggle_subtask
  - 提示词安全红线（第 1 层）：禁系统命令/修改系统设置、禁批量删除、禁全盘遍历、禁编造路径、禁猜测 id
  - 模块文档注释更新：安全机制清单
  - 已有：工具白名单（第 2 层）、6 轮调用上限（第 5 层）、keyring 凭据存储
- 未落地（待老板定）：第 4 层风险分级弹窗确认（会打断聊天流）、第 6 层文件沙箱（等文档生成功能）、多会话/Markdown 渲染（功能扩展非安全项）
- 验证：cargo check ✓ tsc ✓ dev 重建（52214, 23:49:09）运行中

### 聊天记录持久化（23:53-23:58）

- 老板问：聊天记录持久化怎么做 → 直接实现（SQLite 全量覆盖方案）
- 实现：
  - db.rs：新表 bot_messages（id AUTOINCREMENT/role/content/refs/created_at）；BotMsgRow 结构（role/content/refsJson）；三个命令——bot_history_load（按 id 顺序读全部）、bot_history_save（DELETE 后全量 INSERT，事务式简单可靠）、bot_history_clear
  - lib.rs：注册三命令
  - ChatPanel.tsx：挂载时 bot_history_load 恢复（含 refs JSON 反序列化，📌 按钮也持久化）；send 成功/失败后 persistHistory 全量写入（失败消息 ⚠️ 也存）；「清空对话」按钮同步 bot_history_clear
- 踩坑：edit 工具给 execute_batch 的 SQL 串多塞了 `","` 导致 19 个编译错（prefix REAL/INTEGER unknown）——sed 看现场定位后 python 修复
- 验证：cargo check ✓ tsc ✓ dev 重建（53235, 23:57:03）运行中；sqlite3 探针确认 bot_messages 表已建
- 待老板实测：聊几句 → 重启 App（Cmd+Q 再开）→ 聊天记录应恢复；清空对话 → 记录删除

### 多会话管理（00:00-00:04，老板拍板"可以加"）

- 需求：聊天记录多会话（新建/切换/删除对话，仿主流聊天应用）
- 实现：
  - db.rs：bot_messages 加 session_id 列 + 新表 bot_sessions(id/title/created_at/updated_at)；迁移——老库补列，无 session_id 的孤儿消息自动归入「默认对话」会话
  - 命令：bot_sessions_load（updated_at 倒序）、bot_session_create（uuid id，缺省「新对话」）、bot_session_delete（会话+消息级联删）、bot_session_rename；bot_history_load/save/clear 全部改为按 session_id 维度（save 顺带刷会话活跃时间）
  - lib.rs 注册 4 个新命令
  - ChatPanel.tsx 重写：头部会话切换器（nm-outset 按钮 + 下拉菜单：会话列表/当前高亮/🗑删除/＋新建对话，点击外部关闭）；切换会话重载消息；清空按钮改 🧹（只清当前会话）；首轮发送后「新对话」自动改名用户消息前 20 字；busy 时锁定切换/新建/删除
- 验证：cargo check ✓ tsc ✓ dev 重建（53901, 00:03:30）运行中；sqlite3 探针确认 bot_sessions 表已建（会话由前端挂载时惰性创建）
- 待老板实测：①重启后多个会话都在、各自消息独立 ②切换/新建/删除流程 ③老单会话数据应出现在「默认对话」里

### 文档处理 + Python 编程（00:16-00:31，老板拍板：本机 Python 方案）

- 老板拍板：调用本机 Python；文档处理全走 Python；润色重点是 Word，Excel/PDF/PPT 不润色、参照主流功能（提取/生成）
- 新增 bot_py.rs（Python 执行基础设施 + 固定脚本模板）：
  - 环境：detect_python（macOS python3/python；Windows python/python3/py -3）；py_env_check 返回版本 + openpyxl/docx/pptx/pypdf 可用性
  - 沙箱执行 run_python：独立临时目录 py-runs/<uuid>/、run.py + params.json 传参（永不拼 shell）、默认 60s 超时强杀、stdout/stderr 各截 64KB、双线程读输出防死锁、执行完清临时目录
  - 开关：py-enabled.flag（默认关）；py_exec 未开拒绝执行
  - 审计：py_audit 写 bot.log（脚本摘要/耗时/退出码/输出摘要）
  - 固定脚本 5 个：EXTRACT（docx/xlsx/pptx/pdf 按扩展名提取文本，含表格/工作表/幻灯片结构）、MAKE_DOCX（黑体标题+宋体正文 12pt）、MAKE_XLSX（=开头单元格写原生公式）、MAKE_PDF（reportlab STSong-Light 中文字体+自动折行分页）、MAKE_PPTX（封面+标题要点页）
  - 命令：py_exec、doc_extract（弹框选文件或给定路径）、doc_make_word/excel/pdf/ppt；gen_out_path 输出 AI_Gen_Files/<文件名>，同名自动加 (n) 序号永不覆盖；文件名只取 basename 防路径穿越
- bot.rs：TOOLS schema 加 6 工具（extract_document/create_word/create_excel/create_ppt/create_pdf/run_python）；execute_tool 分发 + 6 个桥接函数（提取文本截 30000 字防爆上下文）；SYSTEM_PROMPT 加文档规则 8-12（先提取→Word 润色后 create_word 新文件不覆盖原文件→公式 Excel→Python 编程→生成文件只落 AI_Gen_Files）
- SettingsPage：机器人设置卡片加「允许机器人执行 Python」开关（默认关、开启 confirm）+「本机 Python 环境」检查按钮（版本+四库状态）
- lib.rs 注册 9 个 bot_py 命令
- 踩坑：ChildStdout/ChildStderr 不能放同一数组循环（类型不同）→ 分开两个 thread；child.stdout.take() 进闭包 partial move → 先 take 再传；edit 误删 refreshBot/loadConfig 定义 → 补回
- 验证：cargo check ✓ tsc ✓；5 个 Python 脚本实测全过（提取 docx 文本 ✓、生成 docx 段落校验 ✓、xlsx 公式 ✓、pdf ✓、pptx ✓）；dev 重建（55633, 00:30:07）运行中
- 本机环境：系统 Python 3.9.6 + openpyxl 3.1.5 + python-docx 1.2.0 + python-pptx 1.0.2 + pypdf 6.10.2 + reportlab 4.5.1 全齐
- 待老板实测：①设置页开关+环境检查 ②让机器人提取 Word 并润色生成新文件 ③Excel 公式 ④run_python

### Word 修订模式（07:02-07:20，老板指令「做修订模式」）

- 需求：Word 润色增加修订模式——生成带修订标记（track changes）的文档，删除内容标删除线、新增标红色下划线，可在 Word「审阅」里逐条接受/拒绝
- 实现：
  - bot_py.rs：新增固定脚本 MAKE_DOCX_REVISIONS_SCRIPT（约 130 行 Python）——原文优先从 originalPath 回读文件（与 EXTRACT 同逻辑：非空段落 + 表格行），提取被截断时回退模型传的 original 行（长度对比判断，保证对比范围一致）；difflib 段落级对齐（autojunk=False）+ 替换段落内字符级 diff；w:ins/w:del XML（author=WMessage AI、date=UTC、id 自增），删除 run 用 w:delText + strike + 红色，新增 run 用 w:u + 红色；新命令 doc_make_word_revisions
  - bot.rs：doc_extract 返回改为 {path, text}（工具结果带 [文档路径] 头）；TOOLS 加 create_word_revisions（originalPath/original/revised/title/filename）；execute_tool 分发 + tool_create_word_revisions；SYSTEM_PROMPT 规则 9 加修订模式分支（含截断时必传 original 的兜底规则）
- 踩坑（重要）：TOOLS JSON 昨晚最后一版有两个括号 bug——extract_document 少一个 `}`、create_excel 多一个 `}`（`},"description"` 提前闭合了 sheets 对象），serde_json::from_str(TOOLS).unwrap() 会直接 panic，bot 聊天整个不可用。逐条解析 + 括号事件追踪定位修复
- 验证：cargo check ✓；Python 脚本单测两条路径全过——①文件回读路径：3 段原文 vs 3 段修订，字符级 diff 正确（del「很好」/ins「晴朗」、ins「三点」「会议」），ins/del 带 author/date/id 序号；②截断回退路径：原文 5 段 + 模型只传 3 段 → 用模型行对比，不产生尾部假删除；接受修订后文本正确；dev 重建（61918, 07:18）运行中
- 待老板实测：让机器人「润色这个Word，用修订模式」→ 打开生成文件看删除线/下划线标记 → 审阅里接受/拒绝全部修订

### 联网工具：web_search + fetch_url（07:46-07:56，老板指令「加最后两个工具，按最优方案」）

- 老板确认机器人配的是 MiniMax 接口 → 搜索首选 MiniMax 自带 web_search 触发 + 客户端执行
- 协议实测（curl 直连 MiniMax M3）：
  - 模型返回 tool_calls（name=web_search, args={query}），**服务端不透明执行**——回传 query 原样无效，模型反复换词重试
  - 客户端执行搜索、把真实结果（标题+链接+摘要文本）作为 tool 消息回传 → 模型正常读取继续（实测读到摘要里的日期信息）
  - 结论：MiniMax 只负责"何时搜、搜什么"，搜索执行在客户端
- 搜索后端选型实测：DuckDuckGo（lite/html 两个端点）从本机连不通（HTTP 000）；cn.bing.com 200 可用且 `<li class="b_algo">` 结构清晰 → 用 Bing 抓取
- 实现：
  - 新模块 bot_web.rs（联网工具，约 300 行）：
    - web_search：Bing 抓取（q + mkt=zh-CN、UA 头、connect 15s/总 30s），解析 b_algo 块（h2>a 标题 + href + p 摘要），去标签 + 实体解码（含数字实体 &#NNN;），最多 8 条、输出截 6000 字
    - fetch_text：URL 校验（url crate，仅 http/https）、本机/内网拦截（IP 字面量 is_private/is_loopback/is_link_local/is_unique_local + localhost/.local/.internal/.lan/.home.arpa 后缀）、2MB 上限、Content-Type 只收 html/xml/text、GB18030/GB2312/GBK/Big5 嗅探解码（encoding_rs）、html2text 转纯文本
  - bot.rs：TOOLS 加 web_search（MiniMax type=web_search 格式）+ fetch_url；execute_tool 两臂 + tool_web_search/tool_fetch_url（fetch 结果截 30000 字）；SYSTEM_PROMPT 规则 13-15（最新信息先搜、给链接用 fetch、来源标注）+ 红线补 fetch_url 只公网
  - Cargo.toml：+html2text 0.13 + encoding_rs 0.8 + url 2（rsproxy 镜像源可用）
- 踩坑：html2text::from_read 返回 Result 不是 String；测试里 futures-util 无 block_on → 用 tauri::async_runtime::block_on；数字实体解码漏吃分号（consumed 未含 ;）→ 修
- 验证：cargo check ✓；单元测试 4 个全过（Bing 解析真实 fixture / 内网拦截清单 / 实体解码 / 真实网络搜索+抓取——SEARCH OK 8 条结果带链接、FETCH OK example.com 175 字、127.0.0.1 和 file:// 被拒）✓；TOOLS 18 工具 JSON 全解析 ✓；dev 重建（64297, 07:55）运行中
- 待老板实测：问机器人实时问题（如"今天北京天气"）→ 应触发搜索并回答带链接；发个链接让总结 → fetch 正文

### 搜索双引擎：Bing + 百度（07:58-08:04，老板指令「再加一个搜索引擎百度」）

- 实测百度可抓（www.baidu.com/s 200、无验证页、22 个 result 容器）
- 百度链接是加密的 /link?url（经典替换表已失效、AES CBC/ECB 末 32 字符 key/iv 方案对齐不上）→ 链接原样给出（浏览器可打开跳转），标题+摘要照常
- 实现：web_search 改双引擎——futures_util::future::join 并行跑 Bing + 百度，按标题去重合并，最多 8 条，单引擎失败不影响另一个（全失败才报错）
- parse_baidu：逐 h3 找 baidu.com/link 标题 + extract_baidu_snippet（h3 后第一个 ≥12 字且无 JSON 垃圾的 span，截 200 字）
- 踩坑：切片 h3end+6000 字节可能切在汉字中间 → is_char_boundary 修
- 验证：单测 5 个全过（新增 parse_baidu_fixture：10 条 Bing + 5 条百度真实结果，摘要 48-200 字）✓；dev 重建（65300, 08:04）
- 待老板实测：中文问题搜索，结果里应混有百度来源；百度跳转链接浏览器可开

### 聊天体验三改（08:13-08:20，老板指令：流式回复效果不好）

- 老板三点：①思考过程做成下拉/折叠 ②工具调用折叠、只发结果 ③结果里的网页/文件给可点链接
- 实测 MiniMax M3 流式：reasoning 以 `<think>…</think>` 标签混在 content 里流式下发（无独立 reasoning 字段）
- 后端 bot.rs：
  - feed_think + tail_prefix_len：`<think>` 标签拆分状态机（标签跨流式块时缓冲前缀），思考 → bot-think-delta 事件，正文 → bot-chat-delta；回合结束冲刷残留并丢弃未闭合标签碎片
  - 工具事件三连：bot-tool（新调用）、bot-tool-name（名字补全）、bot-tool-done（执行完带 args）
- 前端 ChatPanel.tsx：
  - Fold 组件（▸/▾ 折叠块）：💭 思考过程默认收起；🔧 工具行默认收起（执行完标 ✓、展开看入参截 500 字）
  - RichText：正文渲染 http(s) 链接（openUrl）和绝对文件路径（/Users /home /Library 等常见前缀 + Windows 盘符路径，openPath）为可点链接，尾部标点裁剪
  - streamingMeta ref 存流式装饰（思考/工具行），bot_chat 完成后并入最终消息；历史持久化仍只存正文
- 踩坑：tail_prefix_len 比较方向写反（尾部反向对标签正向）→ 改 tag.starts_with(&s[len-k..]) + is_char_boundary 防切汉字；单测暴露
- 验证：cargo check ✓ tsc ✓ think 单测 4 个全过（标签跨块/整块/无标签/多块）✓；dev 重建（66061, 08:19）
- 待老板实测：问个需要思考+工具的问题（如「新建任务：买菜」），看 💭 和 🔧 折叠行；再问实时问题验证链接可点

### 折叠行持久化（08:27-08:28，老板指令：折叠行要留在对话框里，默认收起可点开）

- 老板：思考/工具折叠行要一直留在对话框（换会话、重启后还在），默认折叠、点开可看
- 实现：
  - db.rs：bot_messages 加 thinking/tools 两列（CREATE TABLE + PRAGMA table_info 迁移，沿用 session_id 模式）；BotMsgRow 加 thinking/toolsJson；bot_history_load/save 读写新列
  - ChatPanel.tsx：persistHistory 带 thinking/toolsJson；rowsToMsgs 统一回填（挂载/切会话/删会话三处）；流式监听里 streamingMeta ref 的副作用移出 setMessages updater（StrictMode 下 updater 跑两遍会把思考文本重复累积进 ref——最终消息思考会翻倍）
- 验证：cargo check ✓ tsc ✓；dev 重建（66956, 08:28）
- 待老板实测：问带思考+工具的问题 → 换会话再换回来 / 重启 App → 💭 和 🔧 折叠行还在，点开能看

### 任务卡交给机器人执行（08:47-08:55，老板批准两阶段方案，先落地阶段一）

- 老板拍板：「按照你的意思做」→ A（卡片按钮一键发起）+ B（🎯选卡+聊天说「完成它」）都做，同一执行循环
- 后端 bot.rs：
  - 大重构：bot_chat 的工具循环抽出 run_model_loop(app, msgs, max_rounds)——配置/Key 检查、流式（思考拆分+工具折叠事件）、进程内工具执行全部共用；bot_chat 轮数 6→8
  - 新命令 bot_execute_task(taskId)：任务卡（标题/备注/子任务/截止/绑定文件）组装成 [任务卡执行] 指令块 + EXECUTE_SYSTEM_PROMPT（先读卡→工具执行→link_file_to_task 绑产物→edit_task 写执行摘要→complete_task；线下事务诚实拒绝不标完成），10 轮工具循环；已完成/已归档卡片拒绝执行
  - TOOLS +1（19 个）：link_file_to_task（taskId+path，路径必须真实存在防编造）；extract_document 加可选 path 参数（直读绑定文件，不再只能弹框）
  - SYSTEM_PROMPT 加规则 16：聊天说「完成/执行」带 [已选任务] 引用块 → 同一执行语义
- 前端：
  - TodoCard（主窗口）+ TaskCardContent（挂件）：🤖 交给机器人按钮（截止时间上方，与两处展示一致）
  - 主窗口按钮：emit execute-task 事件 + 唤起挂件窗口；挂件按钮：botOn 时 emit 同一事件（bot 关闭时隐藏）
  - ChatPanel：监听 execute-task → executeTask(taskId)（busyRef 防重入、executeTaskRef 防旧闭包）→ 聊天区显示「🤖 执行任务卡：标题」→ bot_execute_task 流式执行 → 结果消息含 💭/🔧 折叠行 + 📌 任务引用，持久化同普通消息；首轮后会话改名为任务名
- 验证：cargo check ✓ tsc ✓ think 单测 4/4 ✓ TOOLS 19 工具 JSON 合法 ✓；dev 重建（68163, 08:54）
- 待老板实测：①新建一张能机器完成的任务卡（如「搜索一下XX并整理要点」）→ 点 🤖 看全流程 ②🎯 选卡 + 说「完成它」 ③线下任务卡（如「取快递」）→ 应诚实拒绝不标完成 ④产物文件应绑回卡片

### 聊天附件交互（09:06-09:07，老板指令：弹框选文件交互不好，改 ➕ 预添加和消息一起发）

- 老板流程：➕ 添加文件 → 聊天窗口写「润色」→ 一起发送
- 实现（前端为主）：
  - ChatPanel：输入行左侧 ➕ 按钮 → dialog 选文件（multiple，挂件窗口已有 dialog 权限）；已选文件显示为 📎 芯片行（basename + ×移除）
  - send()：附件组装成 [附件文件] 块附在消息前（与 [已选任务] 同模式），发送后清空附件
  - 历史消息：splitAttachments 从内容解析附件块 → UserBubbleContent 渲染 📎 芯片 + 正文（重启/切会话后芯片仍在）
  - 占位提示随附件变化（「输入指令，如：润色这个文件」）；只加附件没文字也可发送
- 后端：SYSTEM_PROMPT 加规则 17——消息带 [附件文件] 块时用 extract_document 的 path 参数直读，不再弹系统选择框
- 验证：cargo check ✓ tsc ✓；dev 重建（68751, 09:07）
- 待老板实测：➕ 加 Word → 输入「润色」→ 发送 → 机器人直读文件润色（不弹框）

### 修订模式链接打不开修复（09:19-09:27，老板反馈：修订完链接打不开、默认程序打不开）

- 排查（两个真凶叠加）：
  1. **老板机器只有 Pages 没有 Word**（/Applications 只有 Pages.app），Pages 打不开 Word track changes 的 docx——原修订模式用 w:ins/w:del 生成，Pages 必然失败；聊天记录里机器人自己也诊断过「Pages 打不开修订标记」
  2. **RichText 正则吞 markdown 反引号**：机器人消息里路径包在反引号里（`/path/docx`），正则字符类没排除反引号 → 链接点出去的是带尾反引号的路径 → openPath 静默失败
- 修复：
  - bot_py.rs MAKE_DOCX_REVISIONS_SCRIPT 重写：track changes（w:ins/w:del）→ **可见修订格式**（删除=红色+删除线、新增=红色+下划线，普通 run；文档开头灰色说明行）——Pages/WPS/Word 通用
  - ChatPanel RichText：路径/URL 字符类排除反引号和 *；openPath 失败兜底 revealItemInDir（Finder 定位，不再静默）
  - bot.rs 文案：工具描述和结果消息去掉「Word 审阅逐条接受/拒绝」，改为「红色删除线=删除、红色下划线=新增，Pages/WPS/Word 通用」
- 验证：新脚本单测（w:ins/w:del 计数 0、strike ×3、下划线 ×3、textutil 正常解析）✓；cargo check ✓ tsc ✓；dev 重建（70090, 09:27）
- 待老板实测：重新用修订模式润色 → 点链接 → Pages 直接打开可见修订对照版

### 修订模式改回 Word 原生 track changes（09:32-09:33，老板裁定：Pages 能打开 Word 修订模式）

- 老板拍板：所有润色/修改都用 Word 原生修订模式（w:ins/w:del，author=WMessage AI），撤销 09:27 的「可见修订」格式
- 恢复：MAKE_DOCX_REVISIONS_SCRIPT 回退 track changes 版本（w:ins/w:del + w:delText + strike/underline 显示 + 自增 id + author/date）；工具描述和结果文案回退「可在 Word 审阅里逐条接受/拒绝」
- 提示词升级：SYSTEM_PROMPT 规则 9 —— Word 润色**默认**用 create_word_revisions 修订模式，用户明确要纯文本版才用 create_word；EXECUTE_SYSTEM_PROMPT 规则 2 同步（任务执行里 Word 修改也走修订模式）
- 保留 09:27 的独立修复：RichText 正则排除反引号（链接吞尾反引号导致点不开的真凶）+ openPath 失败兜底 revealItemInDir
- 验证：脚本单测（w:ins ×3 / w:del ×3 / author=WMessage AI / textutil 解析）✓ cargo check ✓；dev 重建（70711, 09:33）
- 待老板实测：修订模式润色 → 点链接 → Pages 打开 track changes 修订

### 图片附件多模态（09:38-09:41，老板问题：MiniMax 能读图，为什么发的图片提取不出文字）

- 原因：机器人只把图片路径（[附件文件] 块）发给了模型，MiniMax 收到的是路径不是图片本体，自然读不了
- 实现（纯后端，前端无改动）：
  - bot.rs attach_images：解析 [附件文件] 块，图片扩展名（png/jpg/jpeg/webp/gif/bmp）读文件转 base64 data URL，消息 content 变成多模态数组 [text, image_url×N]；无图片时保持纯文本字符串
  - 限制：单张 ≤3MB（base64 后约 4MB）、每条消息最多 4 张、文件不存在/过大静默跳过
  - 范围：最近两条 user 消息的图片附加（追问「再仔细点」时上一张图还在上下文里），更早历史保持纯文本省 token
  - SYSTEM_PROMPT 规则 17 更新：图片附件直接出现在消息里，用视觉能力读取，不要用 extract_document 处理图片；文档附件才走 extract_document
- 新依赖 base64 0.22（rsproxy 镜像）
- 验证：单测 3 个全过（图片→data URL / 非图片→纯文本 / 不存在→跳过）✓；dev 重建（71057, 09:41）
- 待老板实测：➕ 发一张带文字的图片 → 说「提取图片里的文字」→ 应直接读出文字

### 收官两件套：删除确认 + 审计日志入口（09:49-09:51，老板拍板「先做1和3」）

- ① 删除任务弹确认（安全网关第 4 层，只对删除）：
  - bot.rs：CONFIRMS 静态表 + tokio oneshot 通道；ask_user_confirm 发 bot-confirm 事件给挂件并等待，**60s 超时默认拒绝**（安全兜底）；bot_confirm_response 命令回填；tool_delete_task 改 async 先确认后删除（顺手升级为 resolve_task 支持 taskId）；审计日志记录 confirm 请求
  - ChatPanel：bot-confirm 监听 → 挂件内弹窗（⚠️ 机器人要删除任务「XXX」+ 允许/拒绝 + 60 秒自动拒绝提示）
- ③ 审计日志查看入口：
  - bot.rs：bot_log_read 命令（倒序最新在前，默认 200 行、上限 2000）
  - SettingsPage：机器人设置卡加「查看日志」按钮 + 弹窗（pre 滚动显示、刷新/关闭）
- 新依赖 tokio（sync+time，oneshot + timeout；tauri 本就带 tokio 无额外成本）
- 验证：cargo check ✓ tsc ✓ 26 个单测全过 ✓；dev 重建（71717, 09:51）
- 待老板实测：①聊天说「删除任务XXX」→ 挂件弹确认 → 允许/拒绝/不理会（60s 自动拒）②设置页「查看日志」看审计记录

### 定时任务卡（阶段二）（09:58-10:07，老板指令：卡片加「⏰ 定时执行」，到点自动跑）

- 数据：tasks 表加 schedule/sched_last 两列（PRAGMA 迁移）；Task 结构加字段（field-level serde default，老前端数据兼容）；upsert/load/load_external 同步
- 定时格式：daily:HH:MM（每天）/ weekly:D:HH:MM（D=1..7 周一起）/ at:YYYY-MM-DDTHH:MM（一次性，执行完自动清除）
- 后端 bot.rs：
  - bot_execute_task 拆出 execute_task_core（命令与调度共用）
  - start_scheduler：30s tick 扫描到点任务（未删/未归档/未完成，sched_last < 触发点 ≤ now，漏执行会补跑一次）
  - run_scheduled：SCHED_RUNNING 防重入 → 先记 sched_last 防 30s 内重复触发 → 执行核心 → 结果前置「⏰ 自动执行 HH:mm」写进备注（失败也记）；一次性执行完清 schedule；审计日志 sched_run/sched_done
  - occurrence_after 纯函数 + 单测 5 个（daily/weekly/at/非法格式，2026-08-16 是周日基准）
- lib.rs：setup 里 start_scheduler 启动
- 前端：TodoCard（主窗口）+ TaskCardContent（挂件）🤖 旁加 ⏰ 按钮；已定时时显示徽标文案（formatSchedule：每天 09:00 / 每周一 09:00 / 08-17 10:00 一次）；点击展开面板：4 个预设 + 自定义 datetime-local 一次性 + 取消定时；挂件经 onSetSchedule 回调 applyAndSync
- 编译踩坑：容器级 #[serde(default)] 要求 Task: Default（改用字段级）；DateTime.weekday() 要 use chrono::Datelike；MIN_UTC 类型歧义（改 epoch from_timestamp_millis(0)）
- 验证：cargo check ✓ tsc ✓ 30 单测全过（含 sched 5 个）✓；dev 重建（73249, 10:07）
- 待老板实测：给一张卡设「每天 09:00」（或自定义一分钟后的定时一次）→ 到点自动执行 → 备注出现「⏰ 自动执行」记录

### 任务卡归属头像 + 个人资料（10:15-11:05，老板多轮确认规则）

- 规则：人完成 → 用户头像；点「交给机器人」→ 机器人头像（执行结束**无论成败**改回用户头像）；机器人也可上传头像+改名；任务卡只显头像、悬停 tooltip 显姓名
- 模型：一个 `bot_assigned` 布尔（不设 completed_by）；tasks 表 +bot_assigned 列（ALTER 迁移）；execute_task_core 进出 set_bot_assigned（手动 🤖 与 ⏰ 定时共用）；open_db 启动 Once 清残留
- profile.rs 新模块：profile.json 存数据目录、头像文件拷 profile/ 子目录；头像读返回 base64 data URL（绕开 asset protocol/CSP）；profile-changed 事件广播双窗口
- 前端：profile.ts 单例缓存+订阅；ActorAvatar 共享组件（bot 默认 main-logo、用户默认首字圆形）；TodoCard + TaskCardContent 标题行末尾；人完成清 botAssigned；SettingsPage「个人资料」卡片（ProfileRow 用户/机器人两行）
- 验证：cargo check + tsc + vite build + 30 单测；dev 重建 76198

### 聊天斜杠命令四件套（11:20-11:35）

- /stop：StopGuard 全局注册表 + bot_stop 命令；run_model_loop 轮次顶部/流式每 chunk/工具循环前检查，置位提前返回「⏹ 已停止」
- /compact：bot_compact 命令（单次非流式、无工具、60s 超时、≤300 字摘要），历史替换为「📦 上下文已压缩」单条并持久化
- /retry：找最后一条 user 消息，砍掉其后内容重跑；send 核心抽成 runChat(history, renameText) 共用
- /copy：最后一条非空助手回复写剪贴板 + 1.5s「已复制 ✓」（后续按老板要求删除）
- /help：列出全部命令
- 验证：cargo check + 30 单测 + npm build；dev 重建 78025

### 助手回复 Markdown 渲染 + 逐条复制（11:50-12:00）

- react-markdown v10 + remark-gfm + remark-breaks（单换行断行）；MarkdownText.tsx 自定义 a 保留原点击行为（URL→openUrl、绝对路径→openPath 失败 revealItemInDir）；react-markdown 默认转义原始 HTML
- ChatPanel：流式中 RichText（增量纯文本），完成态切 MarkdownText；每条完成回复下 📋 复制按钮 + 🗑 移除按钮
- main.css：md-body 全套样式（段落/标题/列表/行内代码/代码块/引用/GFM 表格/分隔线/任务复选框），主题变量深浅色自适应
- bundle 351→511KB（桌面应用无碍）；dev 重建 79217

### 机器人模块审计两轮清零（13:20-14:00，老板拍板「全修」）

- **第一轮 14 项**（P0×2 / P1×2 / P2×4 / P3×6）：
  - P0-1 一次性定时结果被回滚：清理 schedule 用旧快照 upsert 把执行期间修改整体回滚 → db_load fresh 合并只改目标字段
  - P0-2 API 响应 choices[0] 索引 panic：安全访问 + 流式坏行跳过；连锁修调度器每张卡 spawn 隔离（单卡 panic 不杀调度器）
  - P1 run_python 假沙箱文档诚实化 + kill_tree（Unix 进程组 -PGID / Windows taskkill /T）+ 管道读 take() 硬截断
  - P1 SSRF 三洞（bot_web.rs）：DNS 解析校验（防重绑定）、整数/十六进制/八进制 IPv4 字面量识别、重定向逐跳校验（5 跳上限）
  - P2 TOOLS 单测当场抓到真 bug（web_search 工具定义格式错误，模型侧该工具一直是坏的）+ StopGuard 实例隔离（/stop 不影响后台定时）+ 前端双发 busyRef + 删除确认挂件不可见直接拒绝
  - P3 六项：锁毒恢复、链式 unwrap 消除、api.rs update_task 长度校验、http_client 不退化无超时、bot_compact 20 万字符上限、bot.log/api.log 5MB 轮转
- **第二轮 7 项**（P1×2 / P2×2 / P3×3）：
  - P1 定时 panic 后 sched_running 永久残留 → SchedGuard RAII（Drop 清理）
  - P1 extract_document 可读任意文件 → extract_path_allowed 白名单（任务卡绑定文件 / AI_Gen_Files 目录，canonicalize 防 ../）；create_word_revisions 的 originalPath 同规则
  - P2 机器人开关只管 UI 不管后端 → bot_chat/execute_task_core/run_scheduled 三入口都查 bot_get_enabled（定时跳过时 audit 留痕 sched_skip）
  - P2 api.rs create_task 长度校验补齐（与 update/bot 侧三路对齐）
  - P3：/compact 进度占位、profile 换头像 save 失败回删孤儿文件、executeTask 复用 runChat（execTaskId 参数化，删 80% 重复）、http_client OnceLock 全局复用连接池、sessionIdRef 收尾竞态防护
- 验证：cargo check + 31 单测 + npm build；dev 重建 84239
- 已知边界：run_python 本质仍是用户权限执行，真隔离需 OS 级沙箱

### 设置页「检查环境」无反馈修复（14:30-14:36）

- 根因：py_env_check 是同步 tauri 命令，主线程跑 5 次 python 子进程冻结 UI 几秒；前端 catch 只 console.error 毫无反馈
- 修复：py_env_check 改 async + spawn_blocking；库检测合并单进程一次探测全部 5 库（补上漏掉的 reportlab）；py_exec 拆 py_exec_sync（工具链直调）+ async 命令走 spawn_blocking；前端加 pyEnvErr 可见错误文案
- dev 重建 86605

### 定时面板改造：datetime-local 直输 + 四档周期（15:28-15:32）

- 后端：schedule 新增 `monthly:DD:HH:MM`（当月无该日如 2 月 31 顺延）；occurrence_after 加 monthly 分支（逐月扫描 ≤12）；单测 +2
- 前端（TodoCard + TaskCardContent 两面板同步）：删预设按钮，改 datetime-local 整行输入 + 四档「定时一次/每天/每周/每月」+ 取消；打开回填（at→原值、daily→今天、weekly→最近目标星期、monthly→本月/下月该日）；format.ts 新增 scheduleToDatetime 回填 + formatSchedule monthly 显示
- 验证：32 单测 + npm build；dev 重建 91428

### 定时面板 NaN 链死循环事故（15:36-15:45，老板实测「点每月卡死 App」）

- 数据库实锤坏数据 `monthly:Na:TNaN:`，根因链四层：手动输入不完整 datetime → Invalid Date → weekly 写 `weekly:NaN:...` → 回填 NaN 字符串 → monthly 写坏 → `while (date.getDate() !== NaN)` 死循环卡死
- 四层防御：①面板点档位前校验日期有效性（源头堵死）②scheduleToDatetime 全分支校验+回退今天 09:00+monthly 顺延 for 上限 12 次 ③formatSchedule 坏数据原样返回不展开 ④SQL 清理已有坏数据
- 教训（铁律）：**datetime-local 值不可直接信任；while 循环必须带上限；写库前必须校验日期有效性**
- 验证：32 单测 + npm build；dev 重建 92414

### 定时健壮性全修（15:58-16:00）+ 快捷命令全修（16:16-16:18）

- 定时 3 项：P1 错过的一次性任务不再补执行（at_expired：从未执行且已过期 → 放弃清 schedule）；P2 执行期间用户改定时不被误清（收尾用执行后 fresh.schedule 判断）；P3 记 sched_last 失败放弃执行（防 30s 重复触发）
- 快捷命令 4 项：P2 enterBusy/exitBusy 同步镜像 busyRef（同一帧连按两次 /compact 并发）；P3 三个静默场景加 addHint 本地提示（不持久化不污染上下文）；/help 快照统一；switchSession 用 busyRef
- 33 单测全过；dev 重建 94175、95587

### 桌面清理改造：下载模板卡死修复 + 只读规则表（16:22-16:25）

- **卡死根因**：migration_rules_template_save 是同步命令，主线程调 blocking_save_file() → NSSavePanel 无法弹出死锁。改 async + spawn_blocking（migration_rules_import 同修）
- UI：MigrationPanel 删全部行内编辑，规则表改只读展示；操作区仅「⬇ 下载规则模版」「⬆ 导入规则表」
- 教训：**Tauri 主线程绝不能跑 blocking 对话框**（弹框类命令一律 async + spawn_blocking）
- dev 重建 96166

### 桌面清理：CSV 表格模版 + 迁移日志弹窗（16:32-16:37）

- CSV 模版（老板要求编程小白会用）：csv crate；四列表头「启用/文件名关键字/动作/归档目录」+ 两行示例；UTF-8 BOM（Excel/WPS 双击中文不乱码）
- CSV 导入：宽容表头匹配、关键字中英文逗号/顿号/分号分隔、启用识别 是/true/1、动作识别 移动归档/删除文件 + move/delete；编码 UTF-8 → GBK 兜底；旧 JSON 兼容
- 迁移日志弹窗：migration_log_read 命令（尾部行最新在前，与 bot_log_read 同模式）；📋 查看迁移日志按钮 + 70vh 弹窗
- dev 重建 97119

### 桌面清理审计全修（16:46-16:49）+ 全面审计 11 项（17:12-17:25）+ 最严格审计 9 项（17:30-17:43）

- 桌面清理 10 项：RUNNING 改 MigrationGuard RAII（panic 不锁死）；轮询线程 catch_unwind；migration.log 5MB 轮转；load_rules 坏 JSON 记日志；阶段一归档不参与本轮迁移补注释；CSV/面板标注「规则顺序即优先级」；源文件消失 → 解绑附件不再每轮记 skip 噪音；CSV 编码链补 UTF-16 LE/BE；删死命令 migration_rules_save 与 MigrationStatus 死字段
- 全面审计 11 项（我亲自读全部 13000+ 行）：P1 WorkspacePage 不监听 workspace-changed（挂件改工作区主窗口不刷新）、db.rs load_external 丢新字段 → 动态探测列；P2 formatSchedule at: 分钟丢失、ChatPanel busyRef 统一、挂件工作区直写库改 workspace-updated 上报主窗口代理落盘（单写者架构）；P3 greet 死命令/TrashPage 空壳 DndContext/api 日志时间戳可读/due trim/theme 监听清理/monthly 范围校验
- 最严格审计 9 项（4 类交叉核对：命令注册 × invoke、事件 emit × listen、unwrap 全量、边界精读）：P2 快捷键 `shortcut.register(...)?` 改容错（键位被占不再让 App 启动失败）；P3 bot-chat-done 死事件删、死注册 6 命令删（py_exec/doc_*，工具链走函数直调）、mergeDb 死函数删、Header unwrap 改 expect、sched_last_dt 防御、find_due_tasks O(n) 开库改批量、percent_decode + → 空格、SSE 客户端锁中毒 into_inner 恢复
- 累计四轮审计 42 项全部清零；33 单测 + cargo check 无警告 + npm build；dev 重建 98166、1648、3317

### PPT 技能移植（18:36-18:41，老板：PPT 做得不好，发挥大模型能力）

- 背景：OpenClaw 的 MiniMax 文档技能因架构冲突否决，知识可移植；机器人 PPT 原是固定脚本（封面+标题+要点）
- **双移植**：①提示词技能——SYSTEM_PROMPT 规则 10 重写（大纲先行/每页一个观点/标题即结论/bullets 精炼/数据用表格/主题按场合/页数宁少勿多）；②脚本版式引擎——MAKE_PPTX_SCRIPT 升级为六版式（cover/toc/section/content/table/closing）+ 三主题（blue 商务蓝/dark 深色/green 清新绿），16:9 手工画布（色块+文本框+页码）、bullets 自适应字号（≤5 条 20pt、6-8 条 16pt、8+ 自动双栏）、表格页表头加粗底色
- create_ppt 工具 schema 升级（slide.type + subtitle/items/rows + theme）；doc_make_ppt 加 theme 参数
- 验证：脚本本地 + 嵌入 Rust 后提取端到端两轮测试；cargo check + 33 单测 + npm build；dev 重建 12181
- 踩坑：Python runs 解包（str 列表 vs 元组）、封面 continue 逻辑丢封面（重写主流程）、提取脚本切片偏移

### PPT skill 文档完整移植（18:51-18:53，老板点醒：文档作为提示词可行）

- 结论修正：此前否决 MiniMax PPT skill 只针对 SKILL.md 执行机制（agent 指令 + .NET 依赖），文档内容作提示词完全可行
- 通读 `~/.openclaw/workspace/skills/ppt-orchestra-skill/SKILL.md` 原文：SYSTEM_PROMPT 规则 10 重写为排版手册（版式定位/内容页子类型/数据表格化/主题按场合/生成后自查循环）
- 引擎去 AI 痕迹：删标题下装饰横线（skill Avoid 清单点名）、字号对齐规范（内容页标题 32pt、封面 48pt）
- 验证：端到端 5 页 + cargo check + npm build；dev 重建 13192
- 教训：否机制 ≠ 否内容，skill 知识要读原文移植

### 技能批量移植（19:04-19:07）— 12 个 OpenClaw 技能评估，4 个知识移植

- 评估 12 个技能：移植 4 个知识、否决 8 个（机制冲突/能力重复/需外部 API）
- **已移植**：
  - color-font-skill：18 套配色精选 10 套进 PPT 引擎（blue/navy/teal/forest/wine/sky/plum/coral/dark/green），theme 参数 + TOOLS schema 同步，提示词加「按场合选色」表
  - slide-making-skill：排版纪律（正文不粗体、配色只用所选主题、无渐变）
  - minimax-xlsx：派生值必须写公式不硬编码
  - minimax-docx：Word 按文档类型排版（公文/提案/首行缩进）
- **否决**：OpenXML/.NET 与 XML 直改机制（架构冲突，仅取知识）、vision-analysis（已有多模态读图）、gif-sticker-maker 与 music 系列（需外部生成 API + ffmpeg）、mmx CLI（已直连 MiniMax API）、design-style-skill（面向组件化 PptxGenJS，文字引擎用不上）
- 验证：cargo check + 33 单测 + forest 主题端到端 + npm build；dev 重建 13932

### 技能移植变动审计（19:19-19:23）— 9 项发现全修

- 审计对象：PPT 脚本引擎 + THEMES 10 套配色 + 提示词排版手册（三批移植的产物）
- **P2 对比度/正确性**：navy 深底主题斑马纹硬编码浅灰看不清 → THEMES 加 alt 键；table 表头/section 背景/toc 标题条从 accent 改 band（navy 的 accent 是黄色，黄底白字）；render_cover 此前读 slides[0]，cover 非首位时封面标题错 → 传参
- **P2 提示词与引擎不一致**：提示词承诺 Word 首行缩进但脚本没做 → MAKE_DOCX 补 Pt(24) 缩进；提示词「两个 bullet 组」引擎不支持分组 → 措辞改分条目
- **P3**：textbox runs 非 str 防御；lxml import 死代码清理
- 端到端回归（navy+cover 非首位+数字 bullet+表格）6 页全对；dev 重建 14774
- 教训：提示词与引擎能力必须对齐；深色主题配色对比度逐处验证

### 机器人技能系统（19:39-19:45）— 轻量版 Agent Skills，技能可安装

- 需求：WMessage 机器人能力此前全部硬编码，无法安装技能。调研后照 Anthropic Agent Skills 规范实现轻量版
- bot_skills.rs：技能 = 数据目录 skills/<name>/SKILL.md（frontmatter + 正文）；progressive disclosure——提示词只注入「名称+描述」清单，新工具 use_skill 按需读全文（50KB 上限）；技能名白名单防路径穿越；import 递归拷贝跳符号链接、重名拒绝
- SettingsPage「机器人技能」卡片：导入文件夹 / 删除 / 打开目录 / 列表
- 已把 OpenClaw 的 12 个技能导入 WMessage 数据目录（PPT/文档/配色/视觉分析系列）
- 验证：cargo check + 37 单测 + npm build；dev 重建 16491

### Skill 调度器实现（20:53-20:57）— 运行模型 v1.0

- 设计文档 docs/SKILL-RUNTIME.md（生命周期+状态机+流水线+两种模式+回滚+禁止项+示例）
- 实现范式：调度器监督 + 模型执行——use_skill 即启动（预审→Running），execute_tool 每步过 skill_on_step（计数/超步熔断/超时熔断/暂停拒绝/动作记录），run_model_loop 六出口接 skill_finish，/stop 联动 skill_terminate_all
- 元数据字段全落：risk_level/mode/max_steps/timeout_secs/rollback/enabled/intents（clamp + 非法回退 + high 强制 interactive）；preflight 黑名单拒绝
- 审计：skill_start/skill_failed/skill_completed/skill_terminated
- 单测 +11 → 48 全过；dev 重建 22894

### Skill 调度器优化（21:08-21:11）— 3 个 gap 全修

- resumable 字段全链落地（解析/镜像/审计，默认 false）
- Paused 状态真实入口：ask_user_confirm 联动 skill_mark_paused，bot_confirm_response 联动 skill_confirm_result（恢复/拒绝/超时默认拒）；resumable=false 暂停即终止语义落地
- 回滚建议务实版：失败+可回滚+有动作 → 提取「## 回滚」章节生成建议文本，run_model_loop 六出口拼接回复，模型询问用户后按章节执行逆操作
- 审计 +skill_paused/skill_confirm；单测 48→52；dev 重建 24037

### Skill 调度器核验（21:20-21:24）— 3 项修复

- 老板核验清单驱动，发现并修复：
  - P1 状态机 bug：start_skill 后停在 Loaded 未转 Running，步骤计数/熔断/动作记录全部静默失效 → 预审通过直接 Running
  - P1 单轮 Function 5 次熔断缺失（只有轮数限制，并行 tool_calls 可绕过）→ function_calls_total 独立计数熔断 + 审计
  - P2 歧义意图兜底：技能 ≥3 时提示先列候选向用户确认
- 核验通过：调度器仅编排不可绕过、暂停确认联动、审计、双安全域、preflight
- dev 重建 24037

### Windows 绿色版 v1.0.0 打包（21:43-21:47）

- 元数据：作者张晓峰（Cargo.toml authors + tauri bundle.publisher + package.json + README.txt）；版本 0.1.0 → 1.0.0
- 清理：.DS_Store、旧 zip（WebView2 安装器保留作 Win10 备用）
- mingw 全量重编（cargo clean + tauri build --no-bundle）→ exe 42.8MB；四件套 zip（Python zipfile）→ wmessage-win-x64-v1.0.0.zip 14.7MB
- 验证：exe 内版本/作者/CSP 字符串确认；zip 完整性 OK；飞书交付

### 截止时间改造：日历 + 时间方向键 + 无确认键（22:26-22:40，老板指令）

- 老板需求：看板首页截止日期时间设置不要确认键；日期用日历选，时间手动输入或方向键调整
- 实现：
  - 新组件 `src/components/DuePicker.tsx`：日期按钮（显示 MM-DD）→ 自定义日历弹层（周一开头、月切换 ‹ ›、今天高亮、选中 nm-inset 高亮、「今天」快捷按钮）；点日期**即选即存**并收起
  - 时间输入框：手动输 HH:mm（1-2 位时 + 2 位分，>23:59 忽略）或 **↑↓ 方向键 ±1 分钟**（Shift ±1 小时），即改即存；maxLength 5、placeholder HH:mm、blur 回退已提交值
  - × 移除截止时间；点外部或 Esc 收起编辑态；全部操作**无确认键**
  - TodoCard 截止编辑块：`datetime-local` 整行输入 → DuePicker（归档/回收站页复用 TodoCard 自动生效；挂件截止仅展示不变）
- 防御（沿用定时面板 NaN 事故教训）：parseDueParts/buildDue 严格正则解析组装，无效时间不写库；日历日期由年月日数字直接拼装，不经过 Date 解析，杜绝 Invalid Date → NaN 链
- 兼容：旧数据 date-only（"YYYY-MM-DD"）与 datetime（"…T18:00"）均正常解析；formatDue 已支持两种显示
- 验证：tsc + vite build 过；dev 实例 HMR 已生效

### 截止时间回退 datetime-local + 写库防御（22:41，老板拍板）

- 老板对比新旧后拍板：**回旧版**（datetime-local 单控件，紧凑），放弃 DuePicker 日历+时间框方案（组件已 trash）
- 保留新方案里的关键防御（这是老板最初痛点与历史 NaN 事故的根源）：
  - format.ts 新增 `isValidDateTimeLocal`：格式完整（`^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$`）+ 时 ≤23 / 分 ≤59 + `new Date` 往返日期一致（防 2026-02-31 静默进位）
  - TodoCard 截止 onChange：空串→清 due；完整合法→写库；**不完整/非法值一律不写库**，blur 回退已提交值
- 效果：旧版交互（一行控件、原生渲染）+ 新版安全（坏数据进不了库）
- 验证：tsc + vite build 过；dev 实例 HMR 生效

### 打勾圆圈：完成时间展示 + 取消完成按日期回列（22:55，老板指令）

- 老板需求：完成时间自动生成并**显示在截止日期右边**；截止日期的 × 离近一点腾位置；完成列再点 ✓ → 完成时间的日期是今天 → 回「今日」，否则回「待办」，完成时间删除
- 实现：
  - format.ts：`formatCompletedAt(ms)` → 「完成 MM-DD HH:mm」；`isToday(ms)` 本地时区判今天（模块级 pad 上提，顺带消除局部重复）
  - TodoCard 截止行：due 按钮 + × 收紧为同一 span（gap 0、× 宽 w-4）；完成/归档卡在右侧显示完成时间（column===done 且 completedAt 存在）；取消完成时列回退按 `isToday(completedAt)` 决定 doing/todo，并清 completedAt/archived
  - WidgetApp toggleDone 同步同规则（挂件虽只显示未完成任务，保持两窗口逻辑一致）
- 验证：tsc + vite build 过；dev 实例 HMR 生效

### 完成时间居中显示（23:01，老板指令）

- 完成时间改为 ×（截止移除）与 🗑️（删除任务卡）之间**居中**（flex-1 + text-center + whitespace-nowrap），不再紧跟截止日期；todo/doing 卡布局不变

### 看板功能多角度审计（23:12，老板指令：Section 一验收通过后全面审计）

- 审计范围：主看板 17 项功能（数据一致性/竞态/边界/跨窗口/注入安全）
- 修复 2 项：
  - P3 看板「已归档 N」计数把回收站里的归档卡也算进去 → 过滤条件补 `!t.deletedAt`
  - P2 标题/备注/标签/子任务/挂件标题输入框 Enter 提交无 IME 组合态防护 → 全部加 `!e.nativeEvent.isComposing`（中文输入法回车确认候选词不会误提交；macOS 无感，Windows 发布包重点回归）
- 核对无问题：mutate 单写者同步链无竞态；assignInsertOrder 浮点中点/边界 ±1/间隙耗尽重排正确；拖进空完成列草稿 fallback 正确；完成时间生成/清除/显示条件一致；due 三层校验；React 转义防注入；拖拽 5px 激活与点击不冲突；归档规则老数据补 completedAt 保证完成卡都有完成时间
- 待老板拍板：取消完成回「待办」的任务若 due=今天，今日规则会在 60s 内再拉回「今日」列（既有规则行为，是否需要豁免）

### 取消完成回列判断改截止日期（23:23，老板纠正）

- 老板纠正：完成列再点 ✓ 的回列判断依据是**截止日期**（due 日期部分是今天 → 回「今日」；否则回「待办」），此前误实现为完成时间日期
- 修正：format.ts `isToday(ms)` 替换为 `isDueToday(due)`；TodoCard/WidgetApp toggleDone 同步改；App.tsx 本地 isDueToday 删除、统一 import format.ts 版本（消除三处重复）
- 附带收益：之前审计提出的「取消完成回待办但 due=今天会被今日规则再拉回今日」的拍板问题自然消解——due=今天时本来就直接回「今日」列
- 验证：tsc + vite build 过；dev 实例 HMR 生效

### 归档窗口拖拽换列功能取消（23:37，老板指令）

- 老板裁定：归档页窗口拖拽换列功能取消，归档任务卡不可拖拽（仅展示）
- 代码现状核对：ArchivePage 早已是「搜索 + 标签计数筛选 + 三列网格（order 从左到右填充）」实现，无 DndContext/窗口拖拽/顺序持久化残留——**无代码改动，仅文档同步**
- 同步：README 功能特性行、验收清单二节（删除窗口拖拽项，补充「归档任务卡不可拖拽」）

### 归档卡只读化（23:42，老板指令）

- 老板裁定：归档卡除折叠开关 ▸/▾、绑定文件/文件夹打开（📂，桌面清理迁移后核验）、↩ 恢复外，其余一律不可编辑不可点击
- 实现（TodoCardView 按 archived 分支）：
  - 标题/备注：去掉点击编辑与 cursor-text；「+ 备注」隐藏
  - 标签：× 移除隐藏；「+ 标签」隐藏
  - 子任务：checkbox disabled；× 删除隐藏；「+ 添加子任务」隐藏
  - 文件：只留 📂 打开（📋 复制 / × 解绑隐藏）；无绑定文件时不显示绑定入口
  - 🤖 交给机器人 / ⏰ 定时面板：整体隐藏
  - 截止日期：改只读 span（无点击编辑/× 移除）；「+ 截止时间」隐藏
  - 🗑️ 删除：隐藏（归档卡只能恢复，不能删）
  - 标题自动编辑态（autoEdit）加 !archived 守卫（📌 跳归档只展开不编辑）
  - 保留：FoldToggle、完成时间展示、ActorAvatar、↩ 恢复
- 坑：`{!archived && (` 包裹两个兄弟 JSX 元素报语法错 → 加 fragment `<>`
- 验证：tsc + vite build 过；dev 实例 HMR 生效
- 文档：README 功能行、验收清单二节（新增只读条目）+ 六节 📌 跳转描述同步

### 归档卡保留 📋 复制文件（23:59，老板补充）

- 归档只读基础上，绑定文件区保留 📂 打开 + 📋 复制文件+标题；× 解绑仍隐藏（解绑属编辑）
- 文档同步：验收清单二节只读条目、README 功能行

## 2026-08-17（周一）远程手工验收 polish

> 老板今日通过飞书远程验收，要求「想好了再改，改完同步项目文档、开发文档、MANUAL-ACCEPTANCE.md」。本节为验收过程中通过代码 review 发现并修复的 6 项 polish，全部为前端 TS 代码改动，不动 Rust。

### ChatPanel.tsx 三处修复

1. **聊天输入框 Enter 加 IME 组合态防护**（P2 缺陷）
   - 现状：TodoCard 全部输入框（标题/备注/标签/子任务）已加 `!e.nativeEvent.isComposing`；ChatPanel 聊天输入漏了
   - 影响：中文输入法回车确认候选词会误触发 send，把半成品文本发给模型
   - 修复：与 TodoCard 一致 — `if (e.key === "Enter" && !e.nativeEvent.isComposing) send()`

2. **`messages.map` IIFE + mutation 渲染反模式重构**（P2 缺陷）
   - 现状：原代码在 JSX 渲染条件里用 IIFE `( () => { const fps = extractFilePaths(m.content); (m as Msg & { _fps?: string[] })._fps = fps; return fps.length > 0; } )()`，并把 `m._fps` mutation 放在渲染阶段
   - 影响：违反 React 渲染纯函数原则；StrictMode 下渲染双跑会覆盖 `_fps`；下方 `.map` 又读 `m._fps ?? extractFilePaths(...)` 做兜底，逻辑分散难读
   - 修复：把 `messages.map` 回调从箭头函数 `(m, i) => (...)` 改成块体 `(m, i) => { const fps = ...; const showActions = ...; return (...); }`，顶部一次性算 `fps` + `showActions`，去掉 IIFE 与对 `m` 的 mutation

3. **`/help` 命令改本地提示不持久化**（P3 缺陷）
   - 现状：原代码 `/help` 把「可用快捷命令…」push 到 messages 并 persistHistory，每次发 `/help` 历史里堆一条
   - 影响：与 `/stop`（无进行中）/ `/retry`（无 user 消息）/ `/compact`（消息太少）三个分支用的 `addHint` 模式不一致——这三条只本地提示不持久化
   - 修复：`/help` 也走 `addHint` 不持久化，纯本地展示。理由：help 文本属「参考信息」而非「对话内容」，重启后历史里堆 5-6 条 help 是噪音
   - 注意：实际命令（`/compact` 压缩结果、`/retry` 重跑结果）仍然持久化，行为不变

### WorkspacePage.tsx 三处修复

| 位置 | 触发 | 修复 |
|---|---|---|
| 编辑链接 displayName 输入框 | `commitEditLink()` | 加 `!e.nativeEvent.isComposing` |
| 编辑链接 targetUri 输入框 | `commitEditLink()` | 同上（含中文/空格的路径） |
| 添加链接 targetUri 输入框 | `commitAddLink(it.id)` | 同上（拿半输入 URL 进库） |

### 验收扫描

- 其他组件扫描（MigrationPanel / KanbanBoard / ArchivePage / TrashPage）：无同类 Enter 反模式
- tsc + vite build 全过；改动纯前端，Rust 不动

### 同步

- README.md / SPEC.md 不动（无功能/规格变更，仅内部 polish）
- MANUAL-ACCEPTANCE.md 末尾追加「本次验收过程同步修复（2026-08-17）」小节列明 6 项

### 回收站页只读化 + 彻底删除绑文件弹窗（10:42，老板指令）

**需求**
- 1. 主窗口回收站任务卡：除折叠/📂打开/📋复制/↩恢复/🗑彻底删除外，一律不可编辑不可点击（与归档卡同模式）
- 2. 彻底删除时若绑定了文件/文件夹，弹窗询问是否一并删除本地文件

**Rust 新增 `delete_bound_file` 命令**（`src-tauri/src/bot_skills.rs`，注册到 `lib.rs` invoke_handler）
- 签名 `delete_bound_file(path: String, is_dir: bool) -> Result<(), String>`
- 按 is_dir 分流 `remove_file` / `remove_dir_all`；路径不存在视为成功（幂等，已删则跳过）
- 错误透传给前端（权限不足/路径异常）→ 前端 alert 失败原因，任务行保留在回收站，用户可重试
- 与现有 `open_file_path` / `pick_files_dialog` 同文件就近平铺（文件操作就近原则）

**TodoCardView 改造（`src/components/TodoCard.tsx`）**
- 沿用归档卡模式：所有 `archived` 守卫扩到 `archived || trashed`，包括：
  - `useState(autoEdit && !archived)` → `... && !trashed`（📌 跳回收站只展开不编辑）
  - 标题 cursor + onClick + title 三处守卫
  - 备注 onClick + cursor + 「+ 备注」按钮隐藏
  - 标签 × 移除 + 「+ 标签」按钮隐藏
  - 子任务 checkbox `disabled={archived || trashed}`、× 移除 + 「+ 添加子任务」按钮隐藏
  - 文件 × 解绑 + 「📎/📁 绑定文件/文件夹」按钮隐藏
  - 截止编辑/×移除/「+截止时间」按钮隐藏（保留只读 span）
  - 🤖 + ⏰ 整段隐藏（`{!archived && !trashed && ...}`）
- 保留：折叠开关、📂 打开、📋 复制、↩ 恢复、🗑 彻底删除、完成时间展示、ActorAvatar
- 🗑 彻底删除按钮 onClick 改造（弹窗 + invoke）：
  - 有 `filePath`：confirm「任务卡「X」绑定了文件「Y」。完整路径：/.../Y。是否一并删除本地文件？此操作不可撤销。」
  - 无 `filePath`：confirm「确定彻底删除任务「X」？此操作不可撤销。」
  - 取消：不动
  - 确认 + 有 filePath：先 `invoke('delete_bound_file', { path, isDir })`，成功后再 `onDelete(task.id)`；失败 alert 中止
  - 确认 + 无 filePath：直接 `onDelete(task.id)`

**未动**
- TrashPage / App.tsx 无需改：confirm + invoke 全部封在 TodoCard 内部，`onDelete` 仍是原来的 `hardDeleteTask`（删 SQLite 行），流程干净
- 「清空回收站」不变：老板只说单卡彻底删除，全弹窗体验差不实施

**验证**
- `tsc --noEmit` 过；`cargo check` 过（dev profile 25.56s）
- 同步：MANUAL-ACCEPTANCE.md 三节末尾追加只读化条目 + 彻底删除弹窗条目；README 功能行补一句

### 挂件圆角规则改下半句（11:07，老板指令）

**原规则**（裁定 2026-08-15 21:09）：贴边（右/左/顶）→ 四角全直角贴合屏幕边缘；悬浮 → 四边全 `rounded-2xl`

**新规则**（2026-08-17 11:07）：**贴屏侧直角 + 对侧 `rounded-2xl`**；悬浮四边全 `rounded-2xl`（不变）

老板原话：「贴左缘挂件左边直角右边 `rounded-2xl` 圆角，贴右缘挂件右边直角左边 `rounded-2xl` 圆角，贴顶缘挂件上边直角下边 `rounded-2xl` 圆角」

**动机**：原「贴边全直角」让挂件看起来贴死在屏幕上；新规则让对侧圆角，挂件视觉上「浮起」感更强

**实现**（`src/components/WidgetApp.tsx`）
- 原：`const edgeClass = edge === "float" ? "rounded-2xl" : "";`
- 新：
  ```ts
  const edgeClass =
    edge === "right" ? "rounded-l-2xl" :
    edge === "left" ? "rounded-r-2xl" :
    edge === "top" ? "rounded-b-2xl" :
    "rounded-2xl";
  ```
- 触发条（collapsed）与展开面板（expanded）共用同一 `edgeClass` —— 两者视觉一致
- `.nm-sidebar-panel` / `.nm-sidebar-panel-top` 不设 `border-radius`，Tailwind 工具类完全可控

**验证**
- `tsc --noEmit` 过

**同步**
- MANUAL-ACCEPTANCE.md 五.2 描述按老板原话改写
- README.md 无需改（"圆角跟随边缘" 措辞笼统，新规则仍属跟随边缘）
- MEMORY.md ## Standing Decisions 挂件圆角规则条目：旧规则保留时间戳 + 标注「改下半句」 + 新规则描述 + 实现

### 唤起主窗口强制置顶规则（11:31，老板指令）

**原行为**：WidgetApp `focusMain` / ChatPanel `openTaskInMain` 仅 `main.show() + main.setFocus()`。Windows 上 `setFocus` 不一定把窗口推到 z-order 最顶层（其他窗口抢焦点时主窗口被遮住）

**新规则**（老板 2026-08-17 11:31）：双击唤起后主窗口**必须出现在桌面屏幕最顶层**才能看见

**实现**
- 新增 `src/focus.ts` — 共享 `focusMainWindow()` 工具：
  ```ts
  export async function focusMainWindow(): Promise<void> {
    const main = await WebviewWindow.getByLabel("main");
    if (!main) return;
    await main.show().catch(() => {});       // 防隐藏
    await main.unminimize().catch(() => {});  // 防最小化（Windows 必需）
    await main.setFocus().catch(() => {});    // macOS/Linux 推到 z-order 最前
    await main.setAlwaysOnTop(true).catch(() => {});  // Windows 强制置顶兜底
    await new Promise((r) => setTimeout(r, 80));
    await main.setAlwaysOnTop(false).catch(() => {}); // 立即恢复，不长驻
  }
  ```
- WidgetApp 双击标题 `openInMain` → `emit("edit-task") + focusMainWindow()`
- ChatPanel 点任务引用 `openTaskInMain` → 同样改 `focusMainWindow()`（一致性）
- 删除两处内联的 `WebviewWindow` + `show/setFocus` 重复代码（约 10 行）

**为什么不长期驻顶 alwaysOnTop**：会干扰用户正常使用电脑（盖住其他窗口）；短暂闪烁只在唤起那一瞬生效

**验证**：tsc 过（移除两处未使用的 WebviewWindow import 顺手清掉）

**同步**
- MANUAL-ACCEPTANCE.md 五.5 加「唤起后主窗口必须出现在桌面屏幕最顶层」子项
- README.md 不动（widget 功能行「点任务标题 → 主窗口弹出并进入该任务编辑态」措辞笼统仍适用）

### 唤起主窗口强制置顶 — Rust 端单一真相（11:43，老板追问）

**问题**：之前只在 JS 侧（`src/focus.ts` + `WidgetApp.openInMain` + `ChatPanel.openTaskInMain`）走了 alwaysOnTop 闪烁逻辑；Rust 侧 4 处内联调用仍是裸的 `show + unminimize + set_focus`：

- `lib.rs` line 155-157 全局快捷键 Cmd+Ctrl+W 的 show 分支
- `lib.rs` line 162-164 全局快捷键 Cmd+Ctrl+N（快速新建）
- `lib.rs` line 275-277 托盘菜单「打开主窗口」
- `lib.rs` line 289-291 托盘图标左键/双击

老板 11:43 追问「那程序在 Windows 的托盘里也能跳出主窗口」 — 理论上能（`show` 会取消隐藏），但 Windows 上不一定在 z-order 最顶层（与之前 setFocus 同样的问题）

**改造**
- `src-tauri/src/lib.rs`：新增 `pub fn bring_main_to_front(window: &WebviewWindow)` helper（show + unminimize + set_focus + setAlwaysOnTop 闪烁 80ms）+ `#[tauri::command] fn focus_main_window(app: AppHandle)` 命令包装（供 JS invoke）
- 4 处 Rust 调用点全部改走 `bring_main_to_front(&w)`（删除内联的 show/unminimize/set_focus 三行，约 12 行）
- 注册 `focus_main_window` 到 invoke_handler
- `src/focus.ts`：删掉 JS 端重复实现，改为 `await invoke("focus_main_window")`（单一真相在 Rust）

**为什么 Rust 做单一真相**
- 4 处 Rust + 2 处 JS 调用点如果各做各的，闪烁时长/策略改了要改 6 处
- Rust 端可以用 `std::thread::sleep` 同步阻塞 80ms，JS 端通过 invoke 异步等结果
- 同步 vs 异步：sync 命令在 Tauri 主线程跑，80ms 可接受（用户点击瞬间的小延迟）

**同步阻塞 80ms 的取舍**
- 不长驻 alwaysOnTop 是必须的，否则盖住所有窗口干扰用户
- 短暂闪烁必须有，否则 Windows 上 setFocus 推不到 z-order 最顶层
- 80ms 是经验值，足够 OS 应用 alwaysOnTop 状态再恢复（可在老板实测后调整）

**验证**
- `tsc --noEmit` 过
- `cargo check` 过（1.20s 增量编译）

**同步**
- MANUAL-ACCEPTANCE.md 五.5 子项更新：标注 6 处调用点（2 JS + 4 Rust）都走 `bring_main_to_front` 单一真相
- README.md 不动（措辞已涵盖）

### 挂件任务卡 🤖 + ⏰ 折叠到「操作」下拉键（12:06，老板指令）

**问题**：挂件「全部/今日」视图下，新建任务（`+ 新建任务` 大长条触发，addTask prepend 到列表顶部）会直接空显 `🤖 交给机器人` + `⏰ 定时` 两个按钮，标题还是「新任务」、没填任何内容时就露在外面——视觉杂乱

**老板指令**：改为点下拉键才显示这些操作

**实现**（`src/components/TaskCardContent.tsx`，仅改挂件，主窗口 TodoCard 不动）
- 新增 `adminOpen` state（默认 `false`）
- 把原来的「🤖 交给机器人 + ⏰ 定时 + 定时面板」三段整体包到 `adminOpen && (onBotExecute || onSetSchedule)` 折叠段
- 在折叠段上方加「▸ 操作」按钮（chevron + label），点击切换 `adminOpen`
- ⏰ 按钮点击逻辑补充：`if (!schedOpen) setAdminOpen(true)` —— 点 ⏰ 打开定时面板时自动展开「操作」段，否则面板藏在折叠态看不见（关闭时不强制展开，避免与用户主动收起冲突）

**主窗口 TodoCard 暂不动**
- 老板原话「在挂件」明确指 widget
- 主窗口横向空间大，inline 显两个按钮问题不大
- 后续若要统一改可参照本实现

**验证**
- `tsc --noEmit` 过
- 不动 Rust

**同步**
- MANUAL-ACCEPTANCE.md 五.6（挂件任务卡）补「🤖 + ⏰ 默认折叠在下拉键后面」子项
- README.md 不动（挂件任务卡显示描述是功能级，polish 不需列）

### 挂件任务卡标题永远单行截断（12:14，老板指令）

**问题**：widget 标题 h3 的 `truncate` 类只在 `task.collapsed === true` 时应用，展开态可换行。挂件面板宽 320px - 32px padding - 24px card padding = 264px，标题行内还要扣除 ☰/折叠/打勾/头像 ≈ 96px，标题实际可用宽度仅 ~168px——长标题必换行撑高卡片

**老板原话**「缩略显示和主窗口一样，任务卡保持一行」

**实现**（`src/components/TaskCardContent.tsx`，仅 widget，主窗口 TodoCard 未动）
- 标题 h3 `className` 去掉 `task.collapsed ? "truncate" : ""` 条件，改为永远 `truncate`
- `title` 属性简化为永远 `task.title`（鼠标悬停看完整标题）
- 保留 `cursor-text`（onTitleClick 时）+ 双击唤起主窗口的 onDoubleClick 行为

**主窗口 TodoCard 同样问题没改**（待老板拍板）
- 主窗口 `src/components/TodoCard.tsx` line 207-209 同样的 `${task.collapsed ? "truncate" : ""}` 条件
- 老板原话「和主窗口一样」——但实际两边都只在 collapsed 时截断，主窗口横向空间更大（撑高问题不那么明显）所以视觉上更不易察觉
- 两种处置：
  - A. 仅改 widget（已做）：严格按老板字面要求，但 widget 和主窗口不一致
  - B. 一起改：widget 和主窗口都永远 `truncate`，两边统一；但改了主窗口是额外动作
- 等老板晚上核对时决定

**验证**
- `tsc --noEmit` 过
- 不动 Rust

**同步**
- MANUAL-ACCEPTANCE.md 五.6 挂件任务卡补「标题永远单行截断」子项
- README.md 不动（属于 polish 行为而非功能）

### 挂件标题溢出检测 + 折叠键（12:24，老板细化指令）

**上一轮改法**：上一轮把 widget 标题改为永远 `truncate`，无条件截断
**老板反馈**「不动主窗口，只改挂件，新建任务还是要判断标题名称是否超过了一行，超过一行用缩略显示，增加折叠键，展开态会显示全部标题」

**修正后设计**（`src/components/TaskCardContent.tsx`，仅 widget）
- **JS 运行时检测溢出**：`useLayoutEffect` 测量 `scrollHeight` vs `lineHeight`，超出一行才设 `titleOverflow=true`；短标题保持原样
- **条件截断**：仅 `titleOverflow === true` 且未展开时 `truncate`，避免短标题被无谓截断
- **条件折叠键**：仅 `titleOverflow === true` 时在标题右侧显示 ▾/▴ 按钮，溢出才露控件
- **展开态切换**：点 ▾ → 标题完整显示 + 切换为 ▴；再点 ▴ 回到截断态
- **标题变化重置**：useLayoutEffect 依赖 `task.title`，标题改了重新检测 + 重置 `titleExpanded=false`

**为什么用 `useLayoutEffect` 而不是 `useEffect`**
- useLayoutEffect 在 DOM 更新后、浏览器绘制前同步运行
- 可避免「先渲染完整标题 → useEffect 后检测 → 重渲染截断」的闪烁
- 短标题场景：首次渲染无 truncate → useLayoutEffect 确认无溢出 → 保持无 truncate，无闪烁
- 长标题场景：首次渲染无 truncate → useLayoutEffect 检测溢出 → 第二次渲染加 truncate + 折叠键，可能一帧闪烁（可接受）

**主窗口 TodoCard 未动**（老板明确「不动主窗口」）

**验证**
- `tsc --noEmit` 过
- 不动 Rust

**踩坑（已修）**
- 上一轮手抖把输入框的 `nm-task-title` 误改成 `nm-task-name`（类不存在，会丢失字体样式）—— 已回退

**同步**
- MANUAL-ACCEPTANCE.md 五.6 「标题永远单行截断」条目改写为「标题溢出检测 + 折叠键」（描述新行为 + 主窗口未动）
- README.md 不动

### 挂件任务卡折叠设计回滚到单一 FoldToggle（12:33，老板拍板）

**上一轮设计被否定**：
- 12:06 加 `▸ 操作` 折叠键 + `adminOpen` state 包裹 🤖/⏰/schedule panel
- 12:24 又给标题加溢出检测 + 新折叠键（titleRef/useLayoutEffect/titleOverflow/titleExpanded）

**老板原话**「没有改对，不用重新设计折叠窗口，挂件新建任务时，应该每个都带下拉键折叠窗口，下来后显示交给机器人和定时，这时候会显示标题全名吧」

**真意**：
- 不要新增任何折叠机制，复用现有 FoldToggle
- 每个挂件任务卡（包括新建空任务）都永远显示 FoldToggle
- 折叠态只露标题，展开态露标题完整 + 🤖 + ⏰ + schedule panel + 其他内容

**回滚改造**（`src/components/TaskCardContent.tsx`，仅挂件，主窗口未动）
- 删除 `adminOpen` state（连同其 JSX 引用 + `setAdminOpen` 调用）
- 删除 `titleRef` + `titleOverflow` + `titleExpanded` + `useLayoutEffect` 整套溢出检测
- 删除 `useLayoutEffect` + `useRef` import（仅留 `useEffect` + `useState`）
- 删除 `hasBelow` 常量（已无用处）
- FoldToggle 去掉 `hasBelow &&` 门控，改为 `{onToggleCollapsed && ...}` 永远显示
- 标题 h3 回到原始 `task.collapsed ? "truncate" : ""` 模式（含 `title` 属性 conditional）
- 🤖 + ⏰ 行恢复为水平 flex 布局（去掉 `flex-col` 包装 + `self-start`）
- ⏰ onClick 去掉「点开时确保 admin 段展开」逻辑（adminOpen 不存在了）
- schedule panel 移到独立 block（与 🤖/⏰ 行平级，不再嵌在折叠态里）

**结果**
- 挂件任务卡（包括新建空任务）永远显示一个 FoldToggle（▸/▾）
- 折叠态：只露标题（单行截断）
- 展开态：标题（完整多行）+ 备注/标签/子任务/文件 + 🤖 + ⏰ + 定时面板（点 ⏰ 弹出）+ 截止时间
- 没新增任何折叠机制

**验证**
- `tsc --noEmit` 过
- `grep adminOpen` 0 残留
- 不动 Rust

**同步**
- MANUAL-ACCEPTANCE.md 五.6 补「统一复用现有 FoldToggle」条目，记录两轮设计被否、回滚到单一 FoldToggle 的最终设计
- README.md 不动

### 斜杠命令 autocomplete picker + /help 单一真相（15:51，下午开工）

**老板指令**：聊天窗口用户输入第一个字「/」时，向上浮出所有快捷命令（自动补全面板）

**实现**（`src/components/ChatPanel.tsx`）
- 抽 `SLASH_COMMANDS` 常量（模块顶部，Props 类型后）：
  ```ts
  const SLASH_COMMANDS = [
    { cmd: "/stop", description: "停止当前回复" },
    { cmd: "/compact", description: "压缩对话上下文" },
    { cmd: "/retry", description: "重新生成上一条回复" },
    { cmd: "/help", description: "显示本帮助" },
  ];
  ```
- 加 `slashIdx`（当前选中下标）+ `slashDismissed`（Esc 后不再自动浮出，直到下次输入）
- 派生 `pickerOpen` = `!slashDismissed && input.startsWith("/") && !input.includes(" ")` —— 第一个字是 / 且没空格
- Picker UI（`{!slashDismissed && input.startsWith("/") && !input.includes(" ") && (...)}`）渲染在 input 行**上方**（包裹 input 行的 `<div>` 改成 `flex-col`，picker 是 sibling）
  - 每个命令一行：`<button>` 含命令名 + 描述，hover 高亮 / 选中态 `nm-inset`
  - 前缀过滤：`SLASH_COMMANDS.filter(c => c.cmd.startsWith(input))` —— 输入 `/` 显全部，`/s` 只显 /stop，`/h` 只显 /help，无匹配自动隐藏
- input 行 onKeyDown 新增拦截（picker 开着时）：
  - `ArrowDown` / `ArrowUp`：循环切换选中下标
  - `Tab` / `Enter`：补全为 `cmd + " "`（**注意：picker 开着时 Enter 也走补全**，避免输入半截命令误发送）
  - `Escape`：设 `slashDismissed=true` 关掉 picker（输入保持原样）
- input 行 onChange 同步重置 `slashIdx=0` + `slashDismissed=false`（任何输入都重开 picker）
- `/help` 命令 handler 改用 `SLASH_COMMANDS.map((c) => \`${c.cmd} ${c.description}\`).join("\n")`（单一真相，autocomplete 与 /help 共用同一清单）

**为什么 Tab 和 Enter 都走补全**
- picker 开着意味着用户输入不完整（只有命令名、还没参数），直接发送会变成「/s」被当普通消息发出去——无效
- Enter 走补全 + 关 picker，用户看到自动补的命令名后再按 Enter 才真正发送（命令名 + 空格，会被 runSlashCommand 处理）

**Esc 后怎么重开 picker**
- 任何输入（onChange）都会 `setSlashDismissed(false)`，重新激活自动显示
- 用户可以从空输入框开始重新输入 `/` 触发

**验证**
- `tsc --noEmit` 过（修了一个 map 第三参数 `arr` 未使用的 TS6133）

**同步**
- MANUAL-ACCEPTANCE.md 六.8 斜杠命令条目补「/help 不持久化」+「/ 斜杠命令 autocomplete picker」两段
- README.md 不动

### Skill 调度器 Step 1：后置拦截（原子黑名单硬锁，18:14 老板拍板）

**老板拍板**（Q1-Q4 完整决策）：
- Q1 复合业务清单保留（任务汇总/归档迁移/批量导出Excel/PPT生成/Word修订/文档生成/联网搜索/Python/任务卡执行 等）
- Q2 启用禁止裸调原子 Function 黑名单
- Q3 设立单点白名单，run_python 归入白名单
- Q4 开发顺序：先做后置拦截（小事、见效快）→ 再做前置预路由（核心硬锁）

**本节：Step 1 后置拦截落地**

**新增 `src-tauri/src/tool_guard.rs`**（独立模块）
- `ATOMIC_TOOLS: &[&str]` 黑名单：`create_word_revisions`（Word修订Skill内专用）+ `link_file_to_task`（Skill末尾绑产物专用）
- `is_atomic_tool(name) -> bool` 黑名单识别
- `atomic_block_message(name) -> String` 拦截错误文案（含「请走对应 Skill」引导）
- `is_skill_active() -> bool` 委托 `bot_skills::is_skill_active()`（穿透 Mutex 访问）

**改 `src-tauri/src/bot_skills.rs`**
- 新增 `pub fn is_skill_active() -> bool`：遍历 SKILL_RUNS 全局表查 `state == SkillState::Running`
- Running → 放行原子工具；其他状态（Loaded/Finished/Terminated/Failed/Paused）→ 阻断

**改 `src-tauri/src/lib.rs`**
- 注册 `mod tool_guard;`

**改 `src-tauri/src/bot.rs::execute_tool`**
- 入口加黑名单硬锁（先于 skill_on_step 步骤钩子，避免误计数）：
  ```rust
  if crate::tool_guard::is_atomic_tool(name) && !crate::tool_guard::is_skill_active() {
      let msg = crate::tool_guard::atomic_block_message(name);
      audit_log(app, &format!("tool_blocked_atomic | {name} | 硬锁拦截，提示走对应 Skill"));
      return (msg, Vec::new());
  }
  ```
- 现有 skill_on_step 钩子保留（步骤计数/熔断/动作记录）

**单测**（`tool_guard::tests`，6 个全过）
- `atomic_blacklist_recognizes_create_word_revisions` ✓
- `atomic_blacklist_recognizes_link_file_to_task` ✓
- `single_point_tools_are_not_atomic`（遍历 18 个非原子工具确保不被误判）✓
- `atomic_block_message_mentions_skill`（拦截文案必须引导走 Skill）✓
- `atomic_block_message_unknown_tool_fallback`（未知工具名兜底文案）✓
- `is_skill_active_false_with_no_skills`（无 Skill 时返回 false）✓

**验证**
- `cargo check` 过（0.80s）
- `cargo test --lib tool_guard` 6/6 过
- 整体 `cargo test --lib` 应仍全过（已有 52 个测试）

**黑名单初定 vs 待补**
- 当前：仅 2 个原子（Word修订 + 绑产物）。后续新加原子工具时同步扩 `ATOMIC_TOOLS`
- 没动单点白名单实现（run_python 等保持原有直接调用）

**下一步（Step 2）待老板下令启动**
- 前置预路由：bot_chat 入口前加意图分类；命中复合业务 → 直接 start_skill；未命中 → 放行 LLM
- 意图识别主力：关键词正则 L1；LLM 语义 L2 可选兜底
- 双轨触发：保留 `use_skill`（LLM-driven 兼容）+ 新增预路由直接 start_skill

### Skill 调度器 Step 2：前置预路由（L1 正则硬锁，18:14 老板拍板）

**老板拍板**（Q1）：复合业务清单保留，意图识别主力 = L1 关键词/正则硬锁；LLM 语义 L2 可选兑底（初期不上）

**本节：Step 2 前置预路由落地**

**新增 `src-tauri/src/intent_router.rs`**（独立模块 + 11 单测）
- `RouteAction` 枚举：`Skill(String)` | `PassThrough`
- `INTENT_RULES` 常量：7 条复合业务 → Skill 名映射
  - ppt-orchestra-skill、minimax-docx（修订模式）、minimax-xlsx、minimax-pdf、minimax-web-search、minimax-task-summary、minimax-archive
  - Python 脚本不在规则里（单点白名单 run_python，老板 Q3）
- 正则模式比关键词更灵活：允许中英文混杂 + 中间夹字（如「做一份 XX主题的PPT」中间有「XX主题的」也能命中）
- `once_cell::sync::Lazy` 进程启动时一次性编译所有正则
- 11 单测全过：7 个正向路由 + 4 个负向不路由

**Cargo.toml 加依赖**
- `regex = "1"`（关键词正则匹配）
- `once_cell = "1"`（Lazy 静态编译）

**改 `src-tauri/src/lib.rs`**
- 注册 `mod intent_router;`

**改 `src-tauri/src/bot.rs::bot_chat`**
- 在 `msgs` 构造系统提示词时插入预路由检查：
  - 取用户首条消息文本（`messages.last()`）调 `route_user_input(text)`
  - 命中 `RouteAction::Skill(name)` → 调 `bot_skills::start_skill` 启动 Skill（state → Running，原子黑名单自动放行）
  - 把 Skill body 拼到系统提示词末尾（`## Active Skill: {name}\n\n{body}`）→ LLM 看到 Skill 指令直接执行
  - 写 `bot.log`：`intent_route | {name} | 预路由命中，直接加载 Skill（跳过 LLM 选 Skill）`
  - Skill 未安装 / 加载失败 → 写 log 后放行 LLM（不阻断聊天）
- 命中 `RouteAction::PassThrough` → 原 LLM 路径不变
- **关键**：Skill 选择由路由器决定，LLM 不参与；LLM 仅在 Skill 内部执行时参与工具调用

**与 Step 1 联动**
- pre-router 调用 `start_skill` → state 进入 Running → `is_skill_active()` 返回 true
- Skill 内 LLM 调 `create_word_revisions` / `link_file_to_task` 等原子工具 → 后置拦截放行
- 链路打通：路由命中 → Skill 启动 → 原子工具放行 → Harness 全程监控 → Skill 结束

**整体单测状态**
- 总数 68 passed / 0 failed（原 52 + Step 1 加 6 + Step 2 加 10 — 部分测试并到 intent_router module）
- `cargo test --lib` 全过（api / bot / bot_py / bot_skills / bot_web / db / intent_router / migration / tool_guard 9 个 module）

**预路由 INTENT_RULES 表**（精简版）

| 用户输入关键字（正则） | 加载 Skill |
|---|---|
| 做/生成/搞 + ppt/幻灯片/演示 | `ppt-orchestra-skill` |
| 润色/修订 + word/文档，用修订模式 | `minimax-docx` |
| 做/生成 + excel/xlsx/表格 | `minimax-xlsx` |
| 做/生成 + pdf | `minimax-pdf` |
| 搜/搜索/查 + 联网 | `minimax-web-search` |
| 任务/事项 + 汇总/总结 | `minimax-task-summary` |
| 归档/迁移/清理 + 文件/桌面 | `minimax-archive` |

**未来扩展**（老板 Q1 拍板「L2 可选兑底」）
- 加 LLM 语义判断（用更小模型专门做 intent classification，置信度 < 阈值时回退到 LLM）
- 关键词表扩充（更多业务类型、更多同义口语变体）
- Skill 命名映射（实际 skill 名可能跟 INTENT_RULES 不一致，需要 import 时校对）

**下一步（Step 3，待老板下令）**
- 流程完整闭环：预路由 → Skill 启动 → 原子工具 → Harness 监控 → Skill 结束
- 待补：Skill 结束 / 中途失败的 fallback 策略、调度器审计报告、用户视角的「为什么这次没走 LLM 选 Skill」解释

---

## 2026-08-17（周日）回收站彻底删除·绑文件三选项弹窗

### 改动动机
原弹窗用 `window.confirm` 两选项（确定=文件+任务卡一起删；取消=只删任务卡），歧义大；
老板裁定改为三选项：🗑 全部删除 / � 保留文件删除 / 取消。

### 改动
- `src/components/TodoCard.tsx`
  - 新 state `purgeOpen` / `purgeBusy`
  - 彻底删除按钮 onClick 分支：有 `task.filePath` → `setPurgeOpen(true)` 弹三选项；
    无文件时保持原两选项 `window.confirm`
  - card 主体 return 末尾新增 `{purgeOpen && task.filePath && <div fixed inset-0 z-50 ...>}` 自定义 dialog
- 三按钮语义：
  - 🗑 全部删除（红色）：先 `invoke("delete_bound_file", { path, isDir })`，成功后再 `onDelete(id)`；
    失败 alert（任务卡保留在回收站，可重试或手动删除后再试）
  - 📄 保留文件删除：直接 `onDelete(id)`，本地文件不动
  - 取消：仅 `setPurgeOpen(false)`
- 防误点：`purgeBusy` 锁住三按钮 + 关闭 backdrop 的点击关闭（避免删除中途关掉导致 state 不一致）
- 点 backdrop = 取消（不执行删除）；点 dialog 内 = 阻断冒泡

### 验证
- `npx tsc --noEmit` 零错误
- `npx vite build` 通过（827ms）
- DEVLOG 同步

### Bug fix：回收站彻底删除弹窗 hover 任务卡时显示不正确（21:10 老板报）

- **症状**：鼠标放在任务卡上点 🗑 彻底删除 → 弹窗被裁缩到卡片边界内，标题「🗑 彻底删除任务卡」不可见
- **根因**：TodoCard 容器有两层 transform 创建 CSS 包含块，使 `position: fixed` 子元素不再相对视口定位
  1. `nm-card-hover:hover` 触发 `transform: translateY(-3px) scale(1.01)`（`src/ui/main.css` L96）
  2. dnd-kit `useSortable` 写的 `style={{ transform: CSS.Transform.toString(transform), transition }}`（TodoCard.tsx L791）—— 即使 transform 为 null，`CSS.Transform.toString(null)` 返回 `""`，写成 `transform: ""` 仍是非 none 值
- **修法**：dialog 外层用 `createPortal(<div ...>, document.body)` 渲染到 body 下，彻底脱离 TodoCard DOM 子树；z-index `z-50` → `z-[100]` 覆盖 hover shadow 提升
- **踩坑教训**：任何用了 dnd-kit / framer-motion / CSS hover transform 的卡片，内部弹窗必须 Portal，否则 fixed 失效被裁

### 21:17 老板拍板「删除绑定的文件 → Mac 废纸篓 / Windows 回收站」

- **动机**：原 `std::fs::remove_file` / `remove_dir_all` 是物理删除、不可恢复；改为跨平台 trash crate，误删可救（Mac 废纸篓默认 30 天 / Win 回收站永久直到清空）
- **Cargo.toml**：加 `trash = "5"`（2024 仍在维护，跨平台：macOS NSFileManager.trashItemAtURL / Windows SHFileOperation FOF_ALLOWUNDO / Linux gio trash）
- **src-tauri/src/bot_skills.rs::delete_bound_file**：
  - 不再按 `is_dir` 分流 `remove_file` / `remove_dir_all`，统一 `trash::delete_all(p)`
  - 错误文案按平台 cfg 分发："移到废纸篓失败" / "移到回收站失败" / "移到垃圾箱失败"
  - 错误消息追加「文件可能仍在原位置，任务卡保留可重试」提示
- **src/components/TodoCard.tsx**：
  - alert 错误文案调整：`${e}\n\n任务卡保留在回收站，可重试或手动从废纸篓/回收站清理后再试。`
  - 「全部删除」按钮副标题：`任务卡删除，并把绑定的本地文件夹/文件移到废纸篓/回收站`
- **三类文档同步**：
  - MANUAL-ACCEPTANCE.md L55-56 描述加「+ 21:17 移到废纸篓/回收站」后缀，明示走 trash crate
  - README.md L12 回收站 bullet 改写，明确「移到废纸篓/回收站」+「不是物理删除，误删可恢复」
  - SPEC.md 无相关条目，无需改
- **验证**：
  - `cargo check`：trash v5.2.6 拉下，编译零错（1.85s）
  - `cargo test --lib`：68 passed / 0 failed（既有单测未破）
  - `tsc --noEmit` 零错；`vite build` 827ms 通过
- **风险与边界**：
  - 网络盘文件 `trash` 可能失败 → 错误透传 + 让用户重试或手动处理（沿用 alert 路径）
  - Mac 上若 app 走沙盒会需要 entitlement（本项目 desktop app 不走沙盒，无影响）
  - 「不是物理删除」对审计/合规场景需注意；未来若要「真·不可恢复」，加「绕过废纸篓」勾选开关

## 验收过程 polish（21:42 / 21:52）

### 21:42 修 bug：挂件双击唤起主窗口后标题输入框显示旧值

**症状**：挂件新建任务并改名为「测试」→ 立即双击标题 → 主窗口唤起后那张卡的 input 框显示「**新任务**」而不是「测试」；挂件自己显示正常

**根因**：主窗口 `TodoCardView` 的标题草稿 `const [draft, setDraft] = useState(task.title)`，React useState 初值只在 mount 时算一次。挂件发 `tasks-updated` 后主窗口 `setTasks` 重渲染，但 `<SortableTodoCard key={t.id}>` 的 key 不变 → TodoCardView 不重挂载 → `draft` 停在第一次渲染时的「新任务」。双击唤起后 `edit-task` 事件 → `setEditingId(id)` → autoEdit 由 false→true → useEffect 触发 `setEditing(true)` 但**没同步 draft** → 渲染 `<input value={draft}>` 显示「新任务」

**对比挂件没踩的原因**：`TaskCardContent.tsx` line 47-49 useEffect 有 `if (editingTitle) setDraft(task.title)`，会在切换到编辑态那一瞬同步草稿；主窗口 TodoCard.tsx 缺这一行

**修法**（`src/components/TodoCard.tsx`）：用 `prevAutoEdit` ref 锁住「autoEdit 由 false→true 那一瞬」同步 `setDraft(task.title)`，避免 editing 期间因 `task.title` 进依赖而覆盖用户输入；editing 期间 task.title 不会外部变化（commit 才改，且同时 setEditing(false)），所以 task.title 进依赖安全
```tsx
const prevAutoEdit = useRef(autoEdit);
useEffect(() => {
  if (autoEdit && !archived && !trashed) {
    if (!prevAutoEdit.current) setDraft(task.title);
    setEditing(true);
  } else if (!autoEdit) {
    setEditing(false);
  }
  prevAutoEdit.current = autoEdit;
}, [autoEdit, archived, trashed, task.title]);
```

**三类文档同步**：
- `SPEC.md`：本 bug 暂未单独列功能需求条目（属于既有交互的边界修复，不改 SPEC 的功能列表；MANUAL-ACCEPTANCE 五.挂件新增验收子项「双击唤起后编辑框标题与挂件同步」）
- `README.md`：功能特性挂件 bullet 注明「**双击**」（强调单击不再触发，2026-08-17 12:17 老板拍板；本 bug 修复后行为）
- `docs/MANUAL-ACCEPTANCE.md`：
  - 五.挂件加新子项「双击唤起后编辑框标题与挂件同步」
  - 末尾「本次验收过程同步修复」追加 21:42 段落（症状/根因/修法/关联验收项）

**验证**：
- `tsc --noEmit` 零错
- `vite build` 848ms 通过
- dev 实例手动跑流程：挂件新建任务 + 改名「测试」+ 双击唤起 → 主窗口 input 显示「测试」（不是「新任务」）✅

**教训**：
- React `useState(initial)` 的初值只算一次，prop 后续变化不会重算 → 用 key 强制重挂是重置初值的唯一方法；用 useEffect 同步是另一种（用 ref 锁住「切换瞬间」防覆盖用户输入）
- 主窗口 vs 挂件两边都要写草稿同步逻辑时，记得对齐；这次的根因就是「两边只对齐了 useEffect 进编辑态，没对齐草稿同步」

### 21:52 老板拍板移除机器人聊天 /help 命令

**症状**：`/help` 现在只在本地弹一条提示（addHint 模式，不发后端），但补全面板已上浮（输入 `/` 弹出全部候选命令），`/help` 这个命令本身冗余；老板认为上浮面板已能起到「查命令」作用，`/help` 留着是潜在上下文污染与冗余 UI 入口

**改造**（`src/components/ChatPanel.tsx`）：
- `SLASH_COMMANDS` 列表删除 `{cmd:"/help", description:"显示本帮助"}` 条目；注释改写为「autocomplete picker + runSlashCommand 共享」并加 21:52 移除原因 + 老板指令引用
- `runSlashCommand` 删除 `if (cmd === "/help")` 分支（含原 9 行 addHint + SLASH_COMMANDS.map 拼字符串的代码）
- 空状态占位文本「（/help 查看快捷命令）」→「（输入 / 看可用命令）」——指引用户用 `/` 浮出补全面板查命令

**保留**：`/stop` `/compact` `/retry` 三个本地命令不动（均有副作用：停流/压缩/重试，行为不变）

**三类文档同步**：
- `SPEC.md`：第 10 条「斜杠命令：/stop /compact（≤300 字摘要）/retry」去掉 /help；末尾新增第 13 条「移除 /help 命令」，记录缘由 + 行为变化（用户输入 /help 落到 send() 当普通文本发模型）+ 影响面（仅 src/components/ChatPanel.tsx）
- `README.md`：内置机器人聊天 bullet 里「斜杠命令 /stop /compact /retry /help」→「斜杠命令 /stop /compact /retry」，加 2026-08-17 21:52 移除原因注脚
- `docs/MANUAL-ACCEPTANCE.md`：
  - 六.内置机器人聊天里整条斜杠命令验收重写（去掉 /help 列命令、/help 不持久化；保留 /stop /compact /retry；picker 现在剩 3 条）
  - 末尾「本次验收过程同步修复」追加 21:52 段落（症状/改造/影响面/关联验收项）

**验证**：
- `tsc --noEmit` 零错
- `vite build` 820ms 通过（js 体积从 521.48 kB 降到 521.33 kB，减 150 字节）
- `grep "/help" src/components/ChatPanel.tsx`：只剩注释里的移除原因，无业务代码
- `grep "help\|/help" src-tauri/src/`：Rust 端无 /help 处理（无需改）

**风险与边界**：
- 用户输入 `/help` 现在会被发到模型 → 模型按普通指令回复（可能瞎编或问「你能帮我做什么」）；若老板想加兜底可在 `send()` 入口识别 `/` 开头且不在白名单的斜杠文本时弹本地提示，但当前不做（老板说「直接当普通文本走模型」即可）
- autocomplete picker 的 4 → 3 变化需老板重启 dev 验一遍 UI（之前用过的用户可能不知道 picker 现在少了一条）


## 2026-08-18（周二）C 路径 Phase 1-5（DSL 调度器生产化）+ Phase 5（4 项生产化）

### C 路径概览
DSL 调度器从「解析 + 单次顺序执行」演进到「全链路生产可用 + 可监控 + 可调试」：
- **Phase 1**（07:35 前）：DSL 解析器（`parse_step_heading` / `parse_tool_call` / `parse_skill_steps`）+ `run_skill_scheduler` 入口
- **Phase 2**（07:35）：变量替换 `${stepN.result}` / `${stepN.id}` / `${prev.result}` / `${prev.id}` 接入
- **Phase 3**（05:30-06:12）：13 个 mock Skill 写入（11 真业务 + 2 样板），覆盖 Q1 复合业务清单
- **Phase 4 第 1 项**（06:20）：`dsl_advance_action` 状态机主循环，每 step 前 `advance_dsl` 决策 5 分支
- **Phase 4 第 2 项**（06:25）：嵌套路径 `${stepN.task.id}` / `${prev.list.0.title}` 支持 + `CompletedStep.parsed` 字段
- **Phase 4 第 3 项**（06:36）：端到端 `run_dsl_loop_sync` helper + 4 个 e2e 测试（Run/Finish/AwaitUser/FailWithRollback/Terminate 5 分支）
- **Phase 4 第 4 项**（07:09）：mock Skill 路径迁移，`dev_skills_dir` / `skill_search_paths` / `scan_skill_dirs` 拆分纯函数
- **Phase 4 第 5 项**（07:20）：LLM 兜底路径，`DslOutcome` / `DslFailure` 枚举 + `format_completed_summary` + bot.rs auto-mode 分支 match outcome 注入 system prompt

### Phase 5（4 项生产化）
- **A. 11 个真业务 Skill 端到端 smoke test**（07:30）：新增 `generic_mock_executor` 覆盖 17 个工具 + `smoke_all_real_skills_run_dsl_loop_with_mock_executor` 扫所有 13 个 mock Skill 跑通
- **B. Windows release 打包**（07:35）：Mac 上 mingw 交叉编译 `x86_64-pc-windows-gnu` + Python zipfile 打绿色包 13.19 MB（wmessage.exe 42.4 MB + WebView2Loader.dll 157 KB），修 `unused_mut` warning
- **C. SKILL_DSL.md 编写文档**（07:52）：8941 字节作者视角实操指南，10 节覆盖（概述 / 目录结构 / frontmatter / 步骤语法 / 变量替换 / 状态机 / LLM 兜底 / 4 个完整示例 / 调试测试 / 10 个 FAQ）
- **D. Skill 监控 UI + SQLite 持久化**（08:00）：`skill_outcomes` 表 + `PersistedSkillOutcome` struct + `persist_outcome_quiet` 在 6 个 return 点 + `SettingsPage` 加 `SkillOutcomeBadge`（4 色对应 4 种状态）

### 关键决策
- **变量替换模式**：单段 `${stepN.result/id}` / `${prev.result/id}` + 嵌套 `${stepN.path.to.field}`，4 种模型 + 嵌套路径
- **数据目录优先**：dev 模式 scan_skills 合并 `target/debug/skills/` + `app_data_dir/skills`，数据目录在前（同 name 数据目录版本覆盖 dev mock）
- **LLM 兜底**：FailedButRecoverable 把 `format_completed_summary(ctx)` 注入 system prompt 决策下一步，Terminated（用户 /stop）不接管
- **持久化策略**：bot_skills 每次 run 完写 SQLite `skill_outcomes` 表，重启后历史可追溯

### 踩坑记录（追加）
- **Python parser 对 box-drawing chars（`─` U+2500）有 SyntaxError**：heredoc 写文件失败。绕道：用 ASCII anchor 或 `─` 逃逸
- **edit 工具对 `${...}` 字面量 JSON 解析有 bug**：被嵌套解析成多层对象。绕道：Python 脚本 str.replace 直接改文件
- **Windows 打包增量编译产物问题**：`cargo build` 直出 exe 报「无法访问此页面」（缺内置页面资源嵌入），必须 `npx tauri build --target x86_64-pc-windows-gnu --no-bundle` 完整流程
- **macOS `zip -j` 的 Unix 扩展字段让 Win 资源管理器解压报「位置不可用」**：用 Python `zipfile` 打绿色包
- **`db::open_db` 是 private**，bot_skills 调用 E0603，加 `pub` 修复

### 测试
- cargo test --lib 从 83（Phase 1 起点）→ 132（Phase 5 完工），0 regression
- 端到端覆盖：13 个 mock Skill 跑 run_dsl_loop_sync + 通用 mock executor 验证 parse + 变量替换 + 状态机 + 嵌套路径

## 2026-08-19（周三）Phase 7 Kimi 五批 + 任务卡多文件绑定改造

### Phase 7 五批（Kimi code 串行 5 跑）
- **7a 审计/日志安全**（P2-1/2/15/16/34/35）：token ct_eq、变更日志 title escape、append_line 不 panic、kv 转义、App.tsx 写路径 catch、errorHandler 空 msg 兜底
- **7b DB 事务/迁移**（P2-4/5/6/7/8/19）：老库拷 -wal/-shm、workspace upsert/delete 事务包裹、claim_dst_name TOCTOU、migrate 触发条件 json_len > db_len、save_rules atomic_write、probe_log_dir 三合一
- **7c API/Middleware**（P2-3/13/14/27/28/30）：SSE 通道载荷事件 id 去重、middleware panic catch_unwind、pre_execute 空注册审计、CommandError platform 字段、TaskInvalidState 变体、opener scope 收敛
- **7d 前端 state bug**（P2-17/20/21/22/23/33）：entry_view 头像 5MB cap、diffTaskRows 统一、mutate 落盘后才写 tasksRef、TrashPage 排序、useInlineEdit hook、WidgetApp 5s 兜底轮询弹 alert
- **7e 资源+跨平台**（P2-24/25/26/29/31/32）：ExitRequested cleanup_on_exit、spawn 失败清理、bring_main_to_front 后台化、Cargo.toml 单一版本源、托盘三平台、Linux secret-service 探测
- 累计 30 commit + 5 docs commit；cargo test --lib 314 → 358 pass（+44），vitest 55 → 85 pass（+30）

### 任务卡多文件绑定改造（上限 10）
- 老板 18:37 反馈：单文件绑定不够用，再选覆盖；老板 18:40 拍板直接 Kimi 做、上限 10 个
- **commit `f6fcc17`** 一改到底：
  - **Schema**：`Task.files: Array<{path, isDir}>`，保留 `filePath`/`fileIsDir` 双写过渡；`open_db` 启动迁移：老单绑定自动回填 files（幂等，老列不清）
  - **Rust**：`bind_files`/`bind_file` 命令（`fs::metadata` 判 `isDir`、去重保序、超 10 截断）；LLM `create_task`/`edit_task` 扩展 `files` 字段（Rust 侧硬上限截断+警告）；`tool.return` 审计补 `files_count`/`truncated` kv
  - **文件操作**：`copy_files_with_title`（多文件复制，命名 `{title}-{basename}`，单文件行为不变）；`openFile` 多文件弹 UI 列表选
  - **UI**：TodoCard/TaskCardContent/WidgetApp 多 chip 列表（📁/📎 + basename + 单独 ×）；超 5 折叠「还有 N 个」；`pickFile` 多选追加去重；文件夹仍单选独占（已绑文件夹弹提示不加不替换）
- 老板拍板撤掉修法 B（模块级 chatBusyRef + collapse() busy 检查）—— 当时是 17:56 挂件折叠 bug fix 的深度防御，老板选保持单一修改面，最终 commit `8c6c9d7`

### 测试
- cargo test --lib: 314（Phase 7 起） → 358（Phase 7 末） → 360+（多文件改造后）；新增迁移 + 多文件 + 上限 + 双写过渡 4 类测试
- vitest: 81 → 85（修复 7c/7d/7e + 挂件折叠修法 A）→ 90+（多文件 chip 列表 + × 移除 + 上限 UI）
- tsc --noEmit 零错
- Windows 绿色包 14:27 已发布（43.97 MB → 13.65 MB 第二次重打）供老板压测

### 晚间会话（Kimi code，20:45–23:10）：绑定互斥解除 + 机器人可靠性三连修 + 逐步执行模式
- **绑定文件/文件夹解除互斥**（老板反馈）：`TodoCard` 删掉「已绑文件夹不能再加文件」拦截；`pickFolder` 改追加不替换；绑定区补「📁 绑定文件夹」入口；文件夹仍单选。前端 100 测试全绿
- **「Command bind_files not found」**：不是代码问题——运行中的 app 是凌晨 00:54 起的旧二进制（`bind_files` 19:10 才编译进去），手动 nohup 起的进程不归 tauri dev 管不会自动重启；重启解决
- **Phase A 架构改造**（docs/ARCH-REFACTOR-PLAN.md 勾选清单）：`MutationOrigin` 枚举消灭 source 字符串约定（bot/api/migration 三处 emit 编译期锁死）；前端 `mutationOrigin.ts` 类型守卫，非法 source WARN + 按未落盘处理。顺手修两个预存问题：middleware 加 AppHandle 首参后 11 处集成测试调用点失配、`error.rs` doctest 伪代码块（标 `rust,ignore`）
- **机器人幻觉汇报三连修**（bot.log 实锤：模型零工具调用却回复「已添加子任务」「已移至回收站 🗑️」）：
  1. prompt 点名工具（add_subtask/toggle_subtask/remove_subtask/delete_task 等，M3 对没点名的工具不可靠）+ 规则 8 禁止无工具调用时声称完成
  2. **幻觉守卫**（`bot_model_loop.rs` 循环出口确定性拦截）：声称变更（「已」+8 字窗口内含变更动词，覆盖「已彻底删除」变体）但本轮 0 次变更工具调用 → 注入系统提醒补一轮（每次对话最多一次），记 `hallucination_guard` 审计
  3. **新增 `remove_subtask` 工具**（此前无删除子任务工具，模型无从下手）；`resolve_task` 加 taskId/标题交叉校验防跨卡张冠李戴
- **子任务显示**：主窗口+挂件子任务列表 `divide-y` 分隔线分行 + 长文本单行截断 hover 显示全文；prompt 约束子任务为 ≤15 字动宾短句
- **子任务逐步执行模式**（老板拍板：一个一个做、确认后勾选、不满意重做、全部做完不勾卡完成、定时执行跳过确认）：新模块 `exec_steps.rs` 挂起-恢复链路，每步从 DB 重读重建上下文；「继续」由系统直接落库勾选（不经 LLM）；bot_chat 入口挂起优先路由；/stop 清挂起。状态内存态，重启丢
- **熔断上限 10 → 30 全局**（老板拍板）：22:56「列计划+新增子任务」复合任务真触发熔断实锤 10 不够；软警告 7 → 20
- 测试：cargo 374 pass（+14：mutation/guard/classify），vitest 100 pass，tsc 零错

### 机器人工具升级 Phase 1/2（docs/BOT-TOOLS-UPGRADE-PLAN.md，全部验收通过）
- **Phase 1 本地文件工具**（把「不开」改成「可控地开」）：
  - 新模块 `bot_fs.rs`：`read_text_file`/`grep_files`/`list_files` 三工具 + `resolve_allowed` 白名单守卫（canonicalize 后 starts_with 判定，堵 `..` 逃逸/软链逃逸）；默认白名单 `~/Desktop`/`~/Downloads`/`~/Documents` + 任务卡绑定文件夹；每次访问记审计
  - `BotConfig` 加 `allowed_dirs: Vec<String>`（空=默认），设置页白名单 textarea；`bot.rs::load_config` 改 pub(crate)
  - 红线从「一律拒绝本地文件」改为「白名单外一律拒绝」，prompt 规则 19 约束模型不得擅自换目录
  - 顺手修：`extract_document` 的 `extract_path_allowed` 也过 bot_fs 白名单（之前 docx 附件在绑定文件夹内仍被拒）
  - 只读工具不进 MUTATING_TOOLS；tool_guard 非原子清单同步纳入
- **Phase 2 搜索/网页工具升级**：
  - `bot_web.rs::extract_main_content`：去 script/style/nav/footer 噪声块 → article/main → 语义 class/id 最大 div → 兜底全文；结果 <100 字自动回退整页转换；30KB 截断保留
  - `web_search` 结果清理（空白压缩 + 结尾省略号去除）+ 同域名最多 2 条（百度跳转豁免）+ 来源标注 [Bing]/[百度]/[Tavily]
  - `web_search_with_config(app, query)`：配了 `tavilyKey` 走 Tavily API，失败回退抓取记审计 `web_search.tavily_fallback`；设置页加 Tavily key 输入框（生效需老板申请 key）
- 测试：cargo 377（Phase 1）→ 382（Phase 2）pass，vitest 100 pass，tsc 零错；两阶段手动冒烟均老板验收通过

## 2026-08-20（周四）凌晨：Phase 3 模型可切换 + 聊天窗模型快速切换

### Phase 3（docs/BOT-TOOLS-UPGRADE-PLAN.md，全部验收通过）
- **3.2 兼容性代码核对**：payload（model/messages/tools/stream + Bearer）、SSE 解析（`delta.content` / `delta.tool_calls` 增量拼接）、tool 回填（assistant.tool_calls + role:tool + tool_call_id）全是标准 OpenAI 规范，Kimi/DeepSeek 官方兼容，零代码改动
- **3.3 提供商预设**：`src/lib/providerPresets.ts` 共享常量（设置页按钮组 + 聊天窗菜单共用）；命中判定 baseUrl+model 双匹配（DeepSeek Flash/Pro 同 baseUrl，单靠地址无法区分）
- **聊天窗模型快速切换**（老板提的新需求）：ChatPanel 头部 🧠 按钮（显示当前提供商 label / 自定义显示模型名）→ 菜单选预设即切换，整体回写配置保留白名单/Tavily 字段，apiKey 传 null 不动 keychain；设置页保存广播 `bot-config-changed` 同步刷新标签
- **预设跟随官方更新**：Kimi K2 下线 → K3（`kimi-k3`）；DeepSeek 旧名 `deepseek-chat` 2026-07-24 已废弃 → V4 双档（`deepseek-v4-flash` / `deepseek-v4-pro`），Rust 默认值同步换 v4-flash；菜单加「✏️ 自定义…」内联表单自由填任何 OpenAI 兼容端点
- **踩坑**：切 Kimi 报 401 Invalid Authentication —— 不是代码问题，keychain 里存的还是旧 MiniMax key（设置页 API Key 留空保存 = 保持原 key）；教训：跨提供商切换必须同步换 key，菜单底部已加提示文案
- 测试：cargo 382 pass、vitest 103 pass（+3：设置页预设、聊天窗预设切换、自定义表单）、tsc 零错；Kimi K3 幻觉场景冒烟老板验收通过

### Phase 4 低成本高价值工具（03:05，全部验收通过）
- **`get_current_time`**：返回「现在：yyyy-MM-dd HH:mm:ss 星期X」；prompt 规则 20 强制模型做相对日期判断前先调，杜绝凭训练数据猜日期
- **长期记忆**（`bot_facts` 表：key 主键 upsert / 空 value 删除 / 200 条上限）：
  - 模型主动式设计——不每轮自动注入全量记忆（防白烧 token），靠 prompt 规则 21 引导该回忆时调 `recall_facts`；观察点：模型遵循度不够则下一步改系统提示注入「已有 N 条记忆」提示
  - DB 操作抽成 `fact_upsert`/`fact_delete`/`fact_list`（接受 `&Connection`），内存库单测覆盖全逻辑（open_db 全链路依赖 AppHandle，同 bot_fs 先例不测）
  - `remember_fact` 计入 MUTATING_TOOLS：幻觉守卫覆盖「已记住」话术；`validate_fact_kv` 纯函数管入参边界（key ≤50 字 / value ≤500 字）
- 测试：cargo 382 → 387（+5），vitest 103，tsc 零错；老板冒烟：「记住我喜欢简洁的回复」→ 新会话问偏好，跨会话回忆成功

### 工具对比 Kimi Code 后的四项改进（05:10）
- **create_word 加 tables 参数**：可选表格列表 [{title?, rows}] 追加在段落之后，首行表头加粗（python-docx Table Grid）；schema + doc_make_word 透传（Option<Value> 向后兼容）
- **create_excel 多 sheet 核实无需改**：schema（sheets: [{name, rows}]）+ 脚本循环 + tool 透传早已支持，此前对比误判
- **run_python 超时可配**：三层优先级——工具参数 timeoutSecs（schema 新增，模型按需调）> 设置页「Python 默认超时」（BotConfig.python_timeout_secs）> 内置 60s；硬钳 300s 沿用 resolve_timeout；修复 ChatPanel 模型切换回写丢 pythonTimeoutSecs 的隐患（回写字段补齐）
- **fetch_url Jina Reader 回退**：正文提取 <100 字（JS 渲染 SPA 空壳）时回退 https://r.jina.ai/<url>（服务端渲染返回 markdown，免费无 key）；过 check_public_url + 2MB 上限；失败静默不影响主路径
- **textutil 方案否决**（老板问）：macOS 私有、只覆盖 Word、Windows 无等价物；现 Python 子进程方案跨平台四格式通吃，保留；远期可迁 Rust 原生解析（calamine/lopdf/解 zip）干掉 Python 依赖，单独立项
- 踩坑：TOOLS schema 手写 JSON 嵌套括号错一位（tables items 多关一层），tools_schema_parses 测试当场抓住——这个测试就是干这个的
- 测试：cargo 387 → 388（+1 jina_reader_url），vitest 103，tsc 零错
- **create_ppt customColors**（05:20，老板拍板「骨架固定、皮肤开放」）：可选 {bg/accent/text/sub/band/bandtext/alt} 6 位 hex 覆盖主题配色，脚本侧正则校验非法值忽略保底；版式骨架仍固定（信息架构不开放）
- **extract_document 分页读**（05:20）：30K 硬截断 → offset/limit 字符级分页（默认 30000、硬钳 60000），头部 [位置] offset–end/共 N 字符 + 尾部续读提示；offset 越界明确提示。prompt 规则 10 截断标记措辞同步更新
- **extract_document 代码审查结论**（老板问「有没有问题」）：无功能性 bug；三个已知局限——docx 表格抽在段落后（顺序失真）、xlsx data_only 对未保存过的公式读出空、pptx 漏表格内容
- 测试：cargo 388 → 390（+2 分页测试，jina +1 上一批），vitest 103，tsc 零错
- **extract_document pptx 表格漏读修复**（05:26）：walk 递归组合形状（shape_type 6=GROUP，须先判组再碰 has_table——GroupShape 无该属性），GraphicFrame 表格按行输出「单元格 | 分隔」；本机实测：文本框+表格均提取成功

### 架构改造 Phase A-C + 技能路由动态化（晚间，Kimi Code 执行）
- **Phase A（source 标记类型化）**：`mutation.rs` `MutationOrigin` 枚举取代 `"bot"/"api"/"migration"` 裸字符串；前端 `lib/mutationOrigin.ts` 类型守卫，非法值 WARN 不静默吞
- **Phase B（共享常量单一来源）**：新 `consts.rs` `app_consts` 命令下发 `max_task_files/max_title/max_note/image_exts`；前端 `lib/consts.ts` 启动拉取 + 失败回退硬编码；`MAX_TASK_FILES` 改活绑定 re-export（调用方零改动），ChatPanel 图片扩展名走 `imageExtSet()`
- **Phase C（bot_skills.rs 3091 行拆分）**：纯移动零逻辑改动 → `bot_skills/{mod,files,manage,parse,state,vars,runtime,scheduler}.rs`，全部 ≤1200 行；mod.rs 只留 re-export；仅 7 处可见性放宽到 pub(crate)；cargo 421 测试前后一致
- **技能路由动态化（老板拍板：未安装的技能不得有路由）**：
  - 删除静态 `INTENT_RULES` 7 条硬编码映射——3 条幻影（web-search/task-summary/archive 实体从未存在，功能均有内置替代）、ppt-orchestra-skill 实体与本运行时不兼容（subagent/node compile.js）且能力已内置 SYSTEM_PROMPT 规则 10
  - `intent_router` 重写：路由表 = 已安装技能 SKILL.md frontmatter `intents` 声明；启动 + `skills_import`/`skills_delete` 成功后 `rebuild_routes` 重建；空表恒 PassThrough（fail-open）
  - `parse_meta` intents 支持多行 YAML 列表（单行逗号分隔会切碎 `{0,15}` 量词）；`manage.rs` 新增 `intent_rules_from_dirs`（enabled=false / 无 intents 不产生路由）
  - minimax-docx 实体安装到 dev（target/debug/skills）+ release（App Support）两处，frontmatter 补 intents（含「润色/修订 + .doc/.docx 附件」上下文模式 `(?is)...[\s\S]*\.docx?`——附件以 [附件文件] 块嵌消息文本，无需改 middleware 签名）
  - 冒烟测试语义修正：parse/scheduler 两个扫真 skills 目录的 smoke 跳过非 auto 技能（interactive 技能无 DSL Step 是合法状态，不是解析失败）
  - tests-audit 审计脚本修复：BOT_SKILLS 改读拆分后目录；8 条编排层断言跟 F-6 拆分搬家到 BOT_ALL（bot.rs+bot_chat.rs+bot_model_loop.rs）；cargo_test_count 环境变量缺 HOME 导致恒 -1 的预存 bug 一并修
  - 遗留：xlsx/pdf 技能实体在 ~/.openclaw/workspace/skills 但未安装（要用就装，不用路由自然没有）；「机器人对话里安装技能」入口当前不存在，规则实现后任何安装入口自动生效
- 验证：cargo 421 全绿（lib 392 + 集成 29）、cargo check --release 通过、vitest 105、tsc 零错、pytest 审计 24 过 1 跳
