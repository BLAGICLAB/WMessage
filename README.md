# WMessage

Tauri 2 + React 19 + TypeScript 的 Todo 看板，新拟态（Neumorphism）UI，带系统侧边磁吸挂件。

## 功能特性

- **三列看板**（待办 / 今日 / 完成），dnd-kit 列间拖拽
- **任务卡**：标题、备注、标签、子任务（勾选 + 进度 x/y）、文件绑定（打开 / 复制文件+标题）、截止时间（永远最底）
- **今日规则**：截止日期 = 当天的待办任务自动进「今日」列
- **自动归档**：完成满 7 天自动归档；归档页支持搜索 + 标签筛选 + 恢复
- **回收站**：软删除，支持恢复 / 彻底删除 / 清空
- **打勾圆圈**：标题右侧一键完成 / 取消完成
- **卡片折叠**：标题以下内容可折叠（状态持久化）
- **侧边磁吸挂件**（独立透明窗口，可在屏幕任意边缘）：
  - 触发条悬停展开 / 📌 锁定常驻
  - 任务卡与主窗口显示一致；可打勾完成（完成后从挂件消失）、勾子任务、打开/复制文件
  - 新建任务（顶部大长条，创建后直接编辑标题，从顶部出现）
  - 点任务标题 → 主窗口弹出并进入该任务编辑态
  - 自由拖动 + 贴边吸附（右/左/顶），圆角跟随所在边缘，位置自动记忆
- **主窗口关闭 = 隐藏**：挂件可随时唤起；Cmd+Q 正常退出
- **本地持久化**：localStorage，主窗口与挂件实时双向同步

## 技术栈

- Tauri 2 + React 19 + TypeScript + Vite 7
- Tailwind CSS v3，新拟态组件类见 `src/ui/main.css`：`nm-card` / `nm-inset` / `nm-outset` / `nm-task-title` / `nm-sidebar-panel`
- @dnd-kit/core 拖拽
- Rust 命令：`copy_file_with_title`（macOS NSPasteboard；Windows CF_HDROP，cfg-gated）

## 架构

### 双窗口

- **主窗口**：`index.html` → `App`（看板 / 归档 / 回收站三视图）
- **挂件窗口**：`index.html#/widget` → `WidgetApp`（Rust 侧 `WebviewWindowBuilder` 创建，透明无边框置顶）

### 目录结构

```
src/
  main.tsx                    # 按 URL hash 分流渲染 App / WidgetApp
  App.tsx                     # 视图切换、今日/归档规则、双窗口事件桥
  types.ts / storage.ts       # Task 类型 / localStorage 键
  format.ts                   # 主窗口与挂件共用格式化（basename/formatDue）
  ui/main.css                 # 新拟态样式体系
  components/
    KanbanBoard.tsx           # 三列拖拽
    TodoCard.tsx              # 主窗口任务卡
    TaskCardContent.tsx       # 挂件任务卡（显示与 TodoCard 一致）
    WidgetApp.tsx             # 挂件窗口：悬停展开、拖动吸附、新建任务
    ArchivePage.tsx           # 归档页
    TrashPage.tsx             # 回收站页
    DoneCircle.tsx            # 打勾圆圈（共享）
    FoldToggle.tsx            # 折叠按钮（共享）
src-tauri/
  src/lib.rs                  # copy_file_with_title 命令、widget 窗口、主窗口关闭=隐藏
  capabilities/default.json   # main + widget 窗口权限
```

### 数据同步

- 单一数据源：localStorage（`storage.ts` 导出的键，两窗口同源共享）
- 主窗口改动 → persist effect 写 localStorage + emit `tasks-changed`
- 挂件改动 → `applyAndSync` 写 localStorage + emit `tasks-changed`；主窗口 listen 回读（JSON 相等守卫防回声循环）
- 挂件点标题 → emit `edit-task`；主窗口切看板并 `setEditingId` 进入编辑态
- 兜底：挂件每 5s 轮询 localStorage

## 开发

```bash
export PATH="$HOME/.cargo/bin:$PATH"  # 本机 Rust 工具链 PATH 未持久化
npm run tauri dev                     # 开发：前端 HMR + Rust 改动自动重编译
npm run build                         # tsc + vite build
npm run tauri build                   # 打包（M6 待完善）
```

- 网络：cargo 走 rsproxy 镜像（`~/.cargo/config.toml`）；npm 慢时可加 `--registry=https://registry.npmmirror.com`
- macOS 透明窗口依赖 `tauri.conf.json` 的 `"macOSPrivateApi": true`（已配置）

## 文档

- `SPEC.md` — 产品规格（唯一依据）
- `DEVLOG.md` — 开发日志（里程碑 + 踩坑记录）

## 已知待办

- M5 全局快捷键；M6 打包（Windows 侧 `copy_file_windows` 未编译验证）
- 挂件窗口伸缩平滑动画（当前瞬时伸缩）
- 深色模式
