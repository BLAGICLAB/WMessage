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
| 06:47 | 主窗口关闭改为隐藏（CloseRequested prevent_close + Cmd+Q 走 ExitRequested destroy） |
| 07:09 | 挂件新建任务长条移到「全部/今日」下方，新任务从列表顶部出现 |
| 07:28 | 挂件打勾后任务消失（挂件只显示未完成：todo + doing） |
| 07:36 | 挂件自由拖动 + 贴边吸附：右/左/顶 24px 容差、圆角跟随边缘、锚点持久化（`wmessage-widget-pos`） |

## 踩坑记录（避免重蹈）

- Tailwind `@apply` 不能引用自定义组件类（`.nm-card-hover { @apply nm-card }` 编译报错）
- TodoCard 的 useDraggable 在 DndContext 外会崩 → 归档/回收站页必须包空 `<DndContext>`
- 卡片内裸 `<button>` 是 inline 会并排 → 注意 display（块级化）
- macOS 透明窗口：`tauri.conf.json` 必须 `"macOSPrivateApi": true`
- `WebviewWindow.getByLabel` 返回 Promise，必须 await
- **矩形透明窗口里的圆角面板禁用外阴影**（直边被裁、圆角漏光，形成"一段阴影"）→ 用 inset 内阴影
- 主窗口关闭=销毁会让挂件失去唤起目标 → CloseRequested 拦截改隐藏；Cmd+Q 在 ExitRequested 里 destroy 主窗口
- Rust `get_webview_window` 需要 `use tauri::Manager;`
- 本机 Rust 工具链 PATH 未持久化：`export PATH="$HOME/.cargo/bin:$PATH"`

## 后续待办

- M5 全局快捷键
- M6 打包（Win exe / macOS dmg；Windows 侧 `copy_file_windows` 未编译验证）
- 挂件窗口伸缩平滑动画（当前瞬时伸缩）polish
- 深色模式
