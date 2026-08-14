# WMessage 开发日志

> 面向开发者的里程碑记录。产品规格见 `SPEC.md`，项目说明见 `README.md`。

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

- M5 全局快捷键
- M6 打包：交叉编译 exe 已通 ✓（v2 已发验）；剩 NSIS 安装包 + 代码签名 + macOS dmg
- Logo 规范 2.3 单色托盘版（16/32px，现有渐变图缩到托盘尺寸会糊）
- 挂件窗口伸缩平滑动画（当前瞬时伸缩）polish
- 深色模式
- 数据备份/导出（可选；SQLite 文件本身可整体拷贝）
