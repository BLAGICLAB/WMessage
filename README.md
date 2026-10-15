# WMessage

Tauri 2 + React 19 + TypeScript 的个人任务与 AI 代理工作台，新拟态（Neumorphism）UI。跨 Windows 10/11 + macOS。

## 功能特性

- **任务页**：任务池 + 周时间网格（计划块排期、子任务卡、归档卡原位封存）；任务卡含标题/备注/标签/子任务/绑定文件/截止/验收标准，支持折叠、拖拽排序、完成圆圈一键勾
- **内置机器人聊天**（左导航 Agent 能力组）：用大模型管理任务与文档（新建/完成/删除/搜索/编辑/子任务/绑定文件）；42 个主可见工具含文档处理（Word 润色修订模式/Excel 公式/PPT/PDF，产物统一落 AI_Gen_Files）、本机 Python 编程（默认关闭）、联网搜索（Tavily/Brave 可配置，未配置走 Bing+百度网页抓取）、网页抓取（公网白名单）、语义长期记忆（记忆体 v2）、技能（skill）系统；常驻会话侧栏 + 头部上下文 + 右侧上下文栏（任务池/文件/子 Agent 三面板）；斜杠命令 /stop /compact /retry /clean；删除任务弹确认（60s 超时自动拒绝）；审计日志 bot.log
- **受管子 agent**：主 agent `spawn_subagent` 派发长任务（research/coder/general 三档 + recon/executor/verifier 三段式），预算三硬顶（轮数/工具调用/墙钟）+ 并发闸，执行契约与状态全落 `subagents` 表（不建任务卡）；聊天内嵌子 agent 区域在途可见、行上可取消；verifier 拒绝触发精准修复重试闭环
- **程序（apps）**：零 LLM 的确定性 Python 小程序——程序页画廊上架/试跑/运行历史/版本快照回滚/归档恢复；脚本契约（`##RESULT##` 哨兵 + argv 目录约定）见 `docs/APP-DEV-CONVENTIONS.md`；AI 走 bot 工具「聊天即 IDE」（apps_list/apps_create/apps_update_files 落草稿，用户试跑满意再上架）
- **自动归档 + 桌面清理**：完成满 7 天自动归档（归档页搜索/标签筛选/三列网格）；归档后按 CSV 规则表自动迁移绑定文件（`{year}` 占位符、后台轮询、迁移日志 migration.log）
- **回收站**：软删除 + 恢复/彻底删除；绑文件时彻底删除弹三选项（移废纸篓/仅删卡/取消，走 Rust `trash` crate 不物理删除）
- **首页**：问候 + 三池 + KPI 栏（8 选 4）、到期轮播、周柱图、通知中心、图谱/日程/任务卡联动
- **图谱/日程**：任务关系图谱（部门全量）；日程页周视图（定时任务 ⏰ 到点自动交给机器人执行）
- **工作区**：静态链接收藏（网址/文件/文件夹），内联编辑 + 拖拽排序
- **标签整理**：相似标签聚簇合并（tag_merge_history 可追溯）
- **按钮体系**：状态切换用 `nm-outset ↔ nm-inset`（凸↔凹）；动作按钮用 `.nm-btn`
- **任务卡归属头像**：人完成 → 用户头像；机器人执行 → 机器人头像；设置页可上传/修改
- **全局快捷键**：唤起/隐藏主窗口 macOS `Cmd+Ctrl+W` / Win `Ctrl+Alt+W`；快速新建 `Cmd+Ctrl+N` / `Ctrl+Alt+N`；切深浅色 `Cmd+Ctrl+T` / `Ctrl+Alt+T`（注册失败只记日志，绝不影响启动）
- **主窗口关闭 = 隐藏**（托盘可唤起）；Cmd+Q 正常退出
- **本地持久化**：SQLite（WAL）行级增量写入 + 单写者行级广播；便携模式数据库随 exe 走
- **本地 HTTP API**（默认关闭）：tiny_http + SSE + token 鉴权，供外部集成（见下）

## 技术栈

- Tauri 2 + React 19 + TypeScript + Vite 7
- Tailwind CSS v3，新拟态组件类见 `src/ui/main.css`：`nm-card` / `nm-inset` / `nm-outset` / `nm-btn` / `nm-sidebar-panel`(+`-top`)
- @dnd-kit/core 拖拽；recharts（首页图表）
- **tauri-plugin-global-shortcut**：全局快捷键（Rust 侧注册）
- **rusqlite（bundled）**：数据存 `wmessage.db`（WAL + busy_timeout 2s），行级增量读写
- 其他关键依赖：reqwest（流式 LLM 请求）+ keyring（API Key 系统凭据存储）+ csv + encoding_rs + html2text + tokio + chrono + base64 + uuid

## 架构

### 单窗口

- 主窗口：左侧导航 rail（首页/任务/图谱/日程/归档/工作区/回收站/定时/程序 + Agent 能力组），`index.html` → `App` 单根组件切视图
- 聊天（ChatPanel）常挂载：子 agent/会话事件监听器不随切视图卸载，只切可见性

### 目录结构（要点）

```
src/
  App.tsx                     # 视图切换、今日/归档规则、mutate 统一变更出口、事件桥
  types.ts / storage.ts       # Task/WorkspaceItem 类型 / SQLite 读写封装
  lib/                        # 跨页共享逻辑（taskScope 口径谓词、botConfig、homeKpi、apps.ts…）
  components/
    ChatPanel/                # 机器人聊天（会话侧栏/流式/斜杠命令/子 agent 条带/右侧上下文栏）
    TodoCard/                 # 归档/回收站只读卡
    TaskTimeline/             # 任务页周时间网格 + 详情面板（编辑主战场）
    TaskDetailPanel 相关       # task/（TaskFileChips/SubtaskRow 等共享件）
    AppsPage/                 # 程序页（画廊/上架/历史/版本/归档）
    HomePage/ GraphPage/ SchedulePage/
    ArchivePage/ TrashPage/ WorkspacePage/
    SettingsPage/             # 资料/主题/机器人/API/桌面清理/模型供应商…
    ConfirmMap/ Toggle/ …     # 共享组件
src-tauri/
  src/lib.rs                  # 窗口/托盘/快捷键/命令注册 + 启动清扫
  src/db/                     # 全部表读写 + 迁移链 + 日志轮转（paths.rs 数据目录单一入口）
  src/bot/                    # registry 工具单源真相 / dispatch / tools / config（key 走 keyring）/ mcp
  src/bot_chat.rs bot_model_loop.rs bot_scheduler.rs bot_slash.rs   # 聊天编排/工具循环/定时/斜杠
  src/bot_orchestrator.rs     # 子 agent 编排（spawn/check/cancel/并发闸/runner/启动清扫）
  src/apps/                   # 程序模块（commands/manage/runner：契约校验/版本快照/归档）
  src/bot_py.rs + src/py/     # 本机 Python 执行门面（runtime/io/document/env）
  src/bot_skills/ src/bot_web.rs src/migration.rs src/profile.rs
  src/api*.rs                 # 本地 HTTP API（tiny_http + SSE + 鉴权）
  src/evolution/ src/memory/ src/ocr.rs src/audit.rs
  capabilities/default.json   # main 窗口权限
```

### 数据存储与同步（SQLite，单写者）

- **数据源**：`wmessage.db`（便携模式在 exe 同目录；否则 app_data_dir；AI 产物目录 `AI_Gen_Files` 同目录，启动即预建）
- `tasks`（tags/subtasks JSON 列 + schedule/bot_assigned/ord/updated_at）、`workspace_items`、`bot_messages`/`bot_sessions`、`subagents`、`apps`/`app_runs`/`app_versions`、`schedule_jobs` 等；**行级增量读写**（无全库覆盖写）
- 主窗口 `mutate()`：diff 出 upserts/deletes → 行级落盘 → 广播 `tasks-changed`；后端改动走 diff 上报同通道
- **迁移链**：空库自动导入旧版 data.json（更早的 localStorage 迁移已随其占位键删除）
- 运行期全局状态：`app_state.rs` 单一入口

## 开发

```bash
export PATH="$HOME/.cargo/bin:$PATH"  # 本机 Rust 工具链 PATH 未持久化
npm run tauri dev                     # 开发：前端 HMR + Rust 改动自动重编译
npm run build                         # tsc + vite build
npm run tauri build                   # macOS 打包
```

- 网络：cargo 走 rsproxy 镜像（`~/.cargo/config.toml`）；npm 慢时可加 `--registry=https://registry.npmmirror.com`

### 测试与门禁

- **提交时自动跑** pre-commit（`scripts/install-hooks.sh` 装一次）→ `scripts/test-fast.sh`：按改动文件智能跳过——工单号防线（工单号/拍板记录只进 DEVLOG，不进源码）、`cargo fmt --check` / `cargo check` / cargo machete / `tsc` / knip / oxlint / `vitest --changed`（详见 `docs/testing.md`）
- **手动快速验证**：`cargo test --lib`（Rust lib 1400+ 例）、`npm test`（前端 670+ 例）
- **全量验证**（push 前）：`bash scripts/test-all.sh`（cargo nextest 全量 + vitest）；bge 模型真实推理冒烟 `cargo test --lib memory::embed -- --ignored`
- 维护手册：`docs/testing.md`；开发基线约定见 `SPEC.md`「开发基线」

### Windows 交叉编译（macOS → exe，mingw-w64 链路）

```bash
brew install mingw-w64               # 一次性
rustup target add x86_64-pc-windows-gnu   # 一次性
export PATH="$HOME/.cargo/bin:$PATH"
(cd src-tauri && cargo clean --target x86_64-pc-windows-gnu)   # 全量重编（增量产物会缺资源）
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc \
CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++ \
npx tauri build --target x86_64-pc-windows-gnu --no-bundle
# 产物：src-tauri/target/x86_64-pc-windows-gnu/release/wmessage.exe
```

- **必须走 `npx tauri build` 完整流程**：直接 `cargo build` 出的 exe 缺内置页面资源，报「无法访问此页面」
- **绿色包四件套**：wmessage.exe + WebView2Loader.dll（必带）+ MicrosoftEdgeWebview2Setup.exe（Win10 备用）+ README.txt
- **.NET 侧车（Word 修订）**：`npm run publish:docx-dotnet` publish win-x64 self-contained 并归位到便携包 `dotnet/`
- **zip 用 Python zipfile 打**：macOS `zip -j` 的 Unix 扩展字段会让 Win 资源管理器解压报「位置不可用」
- NSIS 安装包在 macOS 打不了（makensis 跨平台崩），发绿色版 zip

## 上线验收清单（Windows 10 / 11 实机）

> 每项发布前在 Win10 测试机（已装 WebView2 Evergreen runtime）+ 任意 Win11 机器各过一遍。

- [ ] 首次启动：任务页空池（无演示数据、无报错弹窗）；新建一张任务卡后正常上池
- [ ] WebView2 依赖：Win10 未装 runtime 时便携包附带的安装器可补救；Win11 免装直接可跑
- [ ] 便携模式：exe + WebView2Loader.dll 同目录启动；任务数据随目录走；exe 目录只读时兜底 app_data_dir；zip 内直接双击运行数据退化 app_data_dir（bot.log 有 WARN）
- [ ] 全局快捷键：Ctrl+Alt+W 唤起/隐藏、Ctrl+Alt+N 快速新建、Ctrl+Alt+T 切主题（被占用时 App 照常启动）
- [ ] 托盘：左键恢复主窗口、右键「打开主窗口/退出」；退出后进程完全结束
- [ ] 剪贴板：任务卡 📋 复制文件+标题 → 粘贴到资源管理器得文件、粘贴到聊天输入框得标题文本
- [ ] 文件操作：绑定文件/文件夹、打开、解绑；路径含中文与空格
- [ ] CSV 规则表：下载模版 → Excel/WPS 编辑中文 → 导入成功（GBK/UTF-8 均可）；桌面清理按规则迁移已归档任务的绑定文件
- [ ] 任务数据管理：导出 JSON → 导入合并（同 id 保留最新修改）
- [ ] 机器人（若配置 API）：聊天流式回复、工具调用（新建/完成任务）、子 agent 派发与取消、删除任务弹确认、60s 超时自动拒绝
- [ ] 程序页：上架脚本 → 试跑 → 版本快照 → 归档/恢复

## 文档

- `AGENTS.md` — AI 代理工作约定入口（一屏：地图 / 命令 / 硬性禁令 / 工作方式）
- `SPEC.md` — 现行产品规格（功能规格全景 + 开发基线；v1 原始快照存文末附录 A）
- `docs/rust-bot-architecture.md` — Rust 侧架构与模块树（现行架构以此为准）
- `docs/APP-DEV-CONVENTIONS.md` — 程序（apps）脚本与 manifest 契约
- `docs/TASK-CARD-RENDERERS.md` — 任务卡两渲染器功能矩阵
- `DEVLOG.md` — 开发日志（只记「为什么换方向」）
- `docs/testing.md` — 测试与门禁手册
- `docs/archive/` — 过程性文档归档（历史审计/交接/评审/已退役设计）

## 深色模式

- **三态主题**：浅色 ☀️ / 深色 🌙 / 跟随系统 🖥️，header 循环切换；持久化 localStorage（`wmessage-theme`）；跟随系统用 matchMedia 实时响应；启动无闪屏（index.html 预渲染脚本）
- **全局快捷键**：macOS `Cmd+Ctrl+T` / Win `Ctrl+Alt+T` 快速浅↔深切换
- **主题变量**（`src/ui/main.css` `:root` / `.dark` 两套独立）：`--bg`、`--surface`、`--sh-low/--sh-high`、`--t1..t6`、`--brand`、`--success/--danger` 等；组件颜色全部 `text-[var(--tN)]` 式引用，切换平滑过渡 0.28s

## 本地 HTTP API（外部机器人接口）

- 设置页开关（默认**关闭**），开启后监听 `http://127.0.0.1:4763`（只绑本机）；关闭立即停止
- 鉴权：`Authorization: Bearer <token>`（token 在设置页展示/复制，存数据目录 `runtime/flags/api-token.txt`）
- 端点：`GET /api/tasks`（?status=todo|doing|done、?trash=1、?archived=1、?all=1）、`GET /api/tasks/:id`、`POST /api/tasks`、`PUT /api/tasks/:id`、`DELETE /api/tasks/:id`（软删幂等）、`GET /api/events`（SSE，?since=断线重放）
- 安全：无文件读取/遍历端点；请求体 1MB 上限；限流 120 次/分；字段长度校验；token 轮换；开关自动记忆；操作日志 api.log

## 已知待办

- M6：NSIS 安装包 + 代码签名（交叉编译 exe + 绿色包流程已通 ✓；macOS dmg 打包链路已通）
- Logo 规范 2.3 单色托盘版（16/32px，现有渐变图缩到托盘尺寸会糊）
- run_python 真隔离沙箱（当前是用户权限执行 + 独立临时目录 + 60s 超时；真隔离需 OS 级 sandbox）。**定性（2026-08-16 上线前审计）**：保持现状——默认关闭 + 设置页开启确认 + 风险文案诚实化
