# WMessage

Tauri 2 + React 19 + TypeScript 的 Todo 看板，新拟态（Neumorphism）UI，带系统侧边磁吸挂件。跨 Windows 10/11 + macOS。

## 功能特性

- **三列看板**（待办 / 今日 / 完成），dnd-kit 列间拖拽
- **任务卡**：标题、备注、标签、子任务（勾选 + 进度 x/y）、文件绑定（打开 / 复制文件+标题）、截止时间（永远最底）
- **今日规则**：截止日期 = 当天的待办任务自动进「今日」列
- **自动归档**：完成满 7 天自动归档；归档页支持搜索 + 标签筛选 + 恢复
- **回收站**：软删除，支持恢复 / 彻底删除 / 清空
- **打勾圆圈**：标题右侧一键完成 / 取消完成
- **卡片折叠**：标题以下内容可折叠（状态持久化）
- **侧边磁吸挂件**（独立透明窗口，屏幕任意边缘）：
  - 触发条悬停展开 / 📌 锁定常驻；贴右/左为竖条（44×220），**贴顶自动变横条**（220×44）
  - 挂件 logo：#3 深蓝双方块（触发条 24px + 面板头部 20px）
  - 任务卡与主窗口显示一致；可打勾完成（完成后消失）、勾子任务、打开/复制文件
  - 新建任务（顶部大长条，创建后直接编辑标题，从顶部出现）
  - 点任务标题 → 主窗口弹出并进入该任务编辑态
  - 自由拖动 + 贴边吸附（右/左/顶，24px 容差），圆角跟随边缘，位置自动记忆
- **按钮体系**：状态切换用 `nm-outset ↔ nm-inset`（凸↔凹）；动作按钮用 `.nm-btn`（默认凸起、按下瞬间凹陷）
- **主窗口关闭 = 隐藏**：挂件可随时唤起；Cmd+Q 正常退出
- **本地持久化**：SQLite 行级增量写入，主窗口与挂件实时同步

## 技术栈

- Tauri 2 + React 19 + TypeScript + Vite 7
- Tailwind CSS v3，新拟态组件类见 `src/ui/main.css`：`nm-card` / `nm-inset` / `nm-outset` / `nm-btn` / `nm-sidebar-panel`(+`-top`)
- @dnd-kit/core 拖拽
- **rusqlite 0.37（bundled）**：任务数据存 `wmessage.db`（WAL + busy_timeout 2s）
- Rust 命令：`copy_file_with_title`（macOS NSPasteboard；Windows CF_HDROP + CF_UNICODETEXT，已编译验证）、`db_load` / `db_upsert` / `db_delete`

## 架构

### 双窗口

- **主窗口**：`index.html` → `App`（看板 / 归档 / 回收站三视图）
- **挂件窗口**：`index.html#/widget` → `WidgetApp`（Rust 侧 `WebviewWindowBuilder` 创建，透明无边框置顶）

### 目录结构

```
src/
  main.tsx                    # 按 URL hash 分流渲染 App / WidgetApp
  App.tsx                     # 视图切换、今日/归档规则、mutate 统一变更出口、双窗口事件桥
  types.ts / storage.ts       # Task 类型 / SQLite 读写封装（loadTasksFromDb/upsertTasks/deleteTaskRows）
  format.ts                   # 主窗口与挂件共用格式化（basename/formatDue）
  ui/main.css                 # 新拟态样式体系（含 .nm-btn 按压态、.nm-sidebar-panel-top）
  assets/widget-logo.png      # 挂件 #3 深蓝双方块 logo（源 docs/logo/assets/3/）
  components/
    KanbanBoard.tsx           # 三列拖拽
    TodoCard.tsx              # 主窗口任务卡
    TaskCardContent.tsx       # 挂件任务卡（显示与 TodoCard 一致）
    WidgetApp.tsx             # 挂件窗口：悬停展开、拖动吸附、贴顶横条、新建任务
    ArchivePage.tsx           # 归档页
    TrashPage.tsx             # 回收站页
    DoneCircle.tsx            # 打勾圆圈（共享）
    FoldToggle.tsx            # 折叠按钮（共享）
src-tauri/
  src/lib.rs                  # copy_file_with_title、widget 窗口、主窗口关闭=隐藏
  src/db.rs                   # SQLite：open_db/upsert/delete/load + data.json 迁移
  capabilities/default.json   # main + widget 窗口权限
```

### 数据存储与同步（SQLite，单写者）

- **数据源**：`wmessage.db`（app_data_dir 下；Win：`%APPDATA%\com.renshi.wmessage\wmessage.db`，macOS：`~/Library/Application Support/com.renshi.wmessage/wmessage.db`）
- 表 `tasks` 单表，`tags`/`subtasks` 为 JSON 文本列；**行级增量读写**（无全库覆盖写）
- **单写者**：只有主窗口写库（`db_upsert` / `db_delete`），挂件只读（`db_load`）
- 主窗口 `mutate()`：计算新数组 → diff 出 upserts/deletes → 行级落盘 → 广播 `tasks-changed`
- 挂件 `applyAndSync`：diff 后经 `tasks-updated` 携带 `{upserts, deletes}` 上报主窗口；主窗口落盘后广播，挂件回读（JSON 相等守卫防回声循环）
- 兜底：挂件每 5s 轮询 `db_load`
- **迁移链**：SQLite 空库 → Rust 侧自动导入方案2 的 data.json 并删除；更早的 localStorage 数据由主窗口首次启动导入后清除
- 挂件锚点位置存 localStorage（非关键 UI 状态，`wmessage-widget-pos`）

## 开发

```bash
export PATH="$HOME/.cargo/bin:$PATH"  # 本机 Rust 工具链 PATH 未持久化
npm run tauri dev                     # 开发：前端 HMR + Rust 改动自动重编译
npm run build                         # tsc + vite build
npm run tauri build                   # macOS 打包
```

- 网络：cargo 走 rsproxy 镜像（`~/.cargo/config.toml`）；npm 慢时可加 `--registry=https://registry.npmmirror.com`
- macOS 透明窗口依赖 `tauri.conf.json` 的 `"macOSPrivateApi": true`（已配置）

### Windows 交叉编译（macOS → exe）

```bash
brew install llvm lld                # llvm-rc + lld-link（一次性）
cargo install cargo-xwin             # 一次性
export PATH="/opt/homebrew/opt/llvm/bin:/opt/homebrew/opt/lld/bin:$HOME/.cargo/bin:$PATH"
npm run tauri -- build --runner cargo-xwin --target x86_64-pc-windows-msvc --no-bundle
# 产物：src-tauri/target/x86_64-pc-windows-msvc/release/wmessage.exe（静态 CRT，独立运行）
```

- 验证 Windows 目标必须 `cargo xwin check --target x86_64-pc-windows-msvc`（引入 C 依赖后裸 `cargo check` 会挂）
- NSIS 安装包与代码签名待做（M6）

## 文档

- `SPEC.md` — 产品规格（唯一依据）
- `DEVLOG.md` — 开发日志（里程碑 + 踩坑记录）
- `docs/logo/` — Logo 规范 V1.0（`WMessage-LOGO-GUIDELINES.md`，含老板裁定「以图片为准」）+ 5 版处理资产（去水印/透明底/多尺寸）

## 已知待办

- M5 全局快捷键
- M6：NSIS 安装包 + 代码签名 + macOS dmg（交叉编译 exe 已通 ✓）
- Logo 规范 2.3 单色托盘版（16/32px，现有渐变图缩到托盘尺寸会糊）
- 挂件窗口伸缩平滑动画（当前瞬时伸缩）
- 深色模式
- 数据备份/导出（可选；SQLite 文件本身可整体拷贝）
