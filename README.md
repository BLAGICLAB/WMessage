# WMessage

Tauri 2 + React 19 + TypeScript 的 Todo 看板，新拟态（Neumorphism）UI，带系统侧边磁吸挂件。跨 Windows 10/11 + macOS。

## 功能特性

- **三列看板**（待办 / 今日 / 完成），dnd-kit 列内排序 + 跨列拖拽（拖进「完成」自动记完成时间）
- **任务卡**：标题（折叠时单行截断）、备注、标签、子任务（勾选 + 进度 x/y）、文件绑定（打开 / 复制文件+标题）、截止时间（永远最底）
- **拖拽手柄**：标题左侧 ☰ 三横线（悬停卡片才显现，按住拖拽）；折叠开关在标题右侧、标题与对勾之间，展开才显示全部标题和设置
- **今日规则**：截止日期 = 当天的待办任务自动进「今日」列
- **自动归档**：完成满 7 天自动归档；归档页：搜索（标题+备注）+ 标签计数筛选（点击筛选/再点取消）+ 三列网格（按 order 从左到右依次填充）；卡片默认折叠、**不可拖拽**、**只读**（仅折叠开关 / 文件打开+复制 / ↩ 恢复可用）；↩ 恢复回看板
- **回收站**：软删除 + 三列网格；任务卡只读（除折叠/📂 打开/📋 复制/↩ 恢复/🗑 彻底删除外不可编辑）；**绑文件/文件夹时彻底删除**弹三选项弹窗（Portal 到 body 跳出 TodoCard transform 包含块 + nm-card）：🗑 全部删除（任务卡删除 + 绑定的本地文件 **移到废纸篓/回收站**，走 Rust `trash` crate 不是物理删除，误删可恢复，失败 alert 任务卡保留可重试）／📄 保留文件删除（只删任务卡）／取消（点 backdrop = 取消）；**未绑文件时**保留原两选项 `window.confirm`；支持恢复 / 彻底删除 / 清空
- **导入数据**（主窗口设置页「任务数据管理」）：选任务导出的 JSON 文件按 id 并集合并，同 id 保留 updated_at 更晚者（便携多机合并）
- **打勾圆圈**：标题右侧一键完成 / 取消完成
- **卡片折叠**：标题以下内容可折叠（状态持久化）
- **侧边磁吸挂件**（独立透明窗口，屏幕任意边缘）：
  - 触发条悬停展开 / 📌 锁定常驻；贴右/左为竖条（44×220），**贴顶自动变横条**（220×44）
  - 挂件 logo：#3 深蓝双方块（触发条 24px + 面板头部 20px）
  - 任务卡与主窗口显示一致；可打勾完成（完成后消失）、勾子任务、打开/复制文件
  - 新建任务（顶部大长条，创建后直接编辑标题，从顶部出现）
  - 点任务标题（**双击**）→ 主窗口弹出并进入该任务编辑态（2026-08-17 12:17 老板拍板；单击不再触发）
  - 自由拖动 + 贴边吸附（右/左/顶，24px 容差），圆角跟随边缘，位置自动记忆
- **按钮体系**：状态切换用 `nm-outset ↔ nm-inset`（凸↔凹）；动作按钮用 `.nm-btn`（默认凸起、按下瞬间凹陷）
- **任务卡归属头像**：人完成 → 用户头像；点「交给机器人」→ 机器人头像（执行结束无论成败改回）；任务卡只显头像、悬停显姓名；用户/机器人头像与姓名可在设置页上传/修改
- **定时任务卡**：⏰ 到点自动交给机器人执行（一次 / 每天 / 每周 / 每月 四档），结果前置「⏰ 自动执行」写进备注；一次性执行完自动清除
- **内置机器人聊天**（挂件下方，设置页开关）：用大模型管理任务（新建/完成/删除/搜索/编辑/子任务/绑定文件）；29 个工具含文档处理（Word 润色修订模式/Excel 公式/PPT/PDF，产物统一落 AI_Gen_Files：启动预建、同名加序号永不覆盖，run_python 写相对路径的产物自动回收进该目录）、本机 Python 编程（默认关闭）、联网搜索（配置 Tavily/Brave key 走对应 API，未配置走 Bing+百度网页抓取）、网页抓取（公网白名单）、语义长期记忆（记忆体 v2）；多会话 + 历史持久化 + 折叠思考/工具行；斜杠命令 /stop /compact /retry /clean（2026-08-17 21:52 老板拍板移除 /help：补全面板已上浮，/help 还会污染 LLM 上下文）；删除任务弹确认（60s 超时自动拒绝）；审计日志 bot.log
- **工作区**（主窗口「归档」与「回收站」之间）：类似任务卡的静态链接收藏（网址/文件/文件夹），标题内联编辑 + 折叠 + 增删链接 + 拖拽排序；挂件只读展示可点击打开
- **桌面清理**（设置页）：任务完成满 7 天归档后按规则自动迁移其绑定文件（移动到归档目录并更新 filePath / 删除文件）；规则表用 CSV 表格管理（下载模版 → Excel/WPS 编辑 → 导入）；`{year}` 占位符；后台 10 分钟轮询；看板/回收站附件绝不触碰；迁移日志 migration.log（弹窗查看）
- **全局快捷键**：唤起/隐藏主窗口 macOS `Cmd+Ctrl+W` / Win `Ctrl+Alt+W`；快速新建任务 macOS `Cmd+Ctrl+N` / Win `Ctrl+Alt+N`（唤起并直接进入标题编辑态）；切换深浅色 `Cmd+Ctrl+T` / `Ctrl+Alt+T`（注册失败只记日志，绝不影响启动）
- **主窗口关闭 = 隐藏**：挂件可随时唤起；Cmd+Q 正常退出
- **本地持久化**：SQLite 行级增量写入，主窗口与挂件实时同步；便携模式：数据库随 exe 走（exe 目录不可写时兜底 app_data_dir；exe 旁已有 wmessage.db/AI_Gen_Files 时强制锚定 exe 目录；zip 内直接双击运行时不进系统 temp，退化 app_data_dir 并记 WARN）

## 技术栈

- Tauri 2 + React 19 + TypeScript + Vite 7
- Tailwind CSS v3，新拟态组件类见 `src/ui/main.css`：`nm-card` / `nm-inset` / `nm-outset` / `nm-btn` / `nm-sidebar-panel`(+`-top`)
- @dnd-kit/core 拖拽
- **tauri-plugin-global-shortcut**：全局快捷键（Rust 侧注册）
- **rusqlite（bundled）**：数据存 `wmessage.db`（WAL + busy_timeout 2s），行级增量读写
- 其他关键依赖：reqwest（流式 LLM 请求）+ keyring（API Key 系统凭据存储）+ csv（规则表导入）+ encoding_rs（CSV/网页编码兜底）+ html2text（网页转文本）+ tokio（oneshot 确认通道）+ chrono + base64 + uuid
- Rust 模块：`db.rs`（SQLite 全部读写）、`app_state.rs`（运行期全局状态单一入口）、`bot/`（`bot.rs` 门面 + `registry.rs` 工具单源真相 / `dispatch.rs` 分发 / `tools.rs` 实现 / `config.rs` 配置）+ `bot_artifacts.rs`（产物登记表：bot 流程结束汇总弹窗）+ `bot_chat.rs`/`bot_model_loop.rs`/`bot_scheduler.rs`/`bot_slash.rs`（机器人聊天编排/工具循环/定时调度/斜杠命令）、`bot_skills/`（Skill DSL 调度器）、`memory/`（语义记忆体 v2：embed/store/rank/consolidate）、`bot_py.rs`（本机 Python 沙箱 + 文档脚本模板）、`bot_web.rs`（搜索/抓取）、`migration.rs`（桌面清理）、`profile.rs`（头像资料）、`api.rs` + `api_server.rs`/`api_handlers.rs`/`api_auth.rs`（本地 HTTP API）、`ocr.rs`（图片识字）、`audit.rs`（审计日志）

## 架构

### 双窗口

- **主窗口**：`index.html` → `App`（看板 / 归档 / 工作区 / 回收站四视图 + 设置页）
- **挂件窗口**：`index.html#/widget` → `WidgetApp`（Rust 侧 `WebviewWindowBuilder` 创建，透明无边框置顶）

### 目录结构

```
src/
  main.tsx                    # 按 URL hash 分流渲染 App / WidgetApp
  App.tsx                     # 视图切换、今日/归档规则、mutate 统一变更出口、双窗口事件桥（含 workspace-updated 代理落盘）
  types.ts / storage.ts       # Task/WorkspaceItem 类型 / SQLite 读写封装
  format.ts                   # 主窗口与挂件共用格式化（basename/formatDue/formatSchedule/scheduleToDatetime）
  profile.ts / theme.ts       # 头像资料缓存订阅 / 三态主题
  ui/main.css                 # 新拟态样式体系（含 .nm-btn、.md-body Markdown 样式）
  assets/widget-logo.png      # 挂件 #3 深蓝双方块 logo（源 docs/logo/assets/3/）
  components/
    KanbanBoard.tsx           # 三列拖拽
    TodoCard.tsx              # 主窗口任务卡（含 ⏰ 定时面板）
    TaskCardContent.tsx       # 挂件任务卡（显示与 TodoCard 一致）
    ChatPanel.tsx             # 机器人聊天（多会话/流式/斜杠命令/删除确认弹窗）
    MarkdownText.tsx          # 助手回复 Markdown 渲染
    WidgetApp.tsx             # 挂件窗口：悬停展开、拖动吸附、贴顶横条、任务/工作区/聊天三视图
    SettingsPage.tsx          # 设置：个人资料/主题/任务数据管理/机器人/API/桌面清理
    WorkspacePage.tsx         # 工作区（静态链接收藏）
    MigrationPanel.tsx        # 桌面清理面板（CSV 规则表 + 迁移日志弹窗）
    ArchivePage.tsx           # 归档页（标签分组三列窗口，可拖拽换列）
    TrashPage.tsx             # 回收站页（三列网格）
    DoneCircle / FoldToggle / ActorAvatar   # 共享小组件
src-tauri/
  src/lib.rs                  # 窗口管理、快捷键（容错注册）、托盘、命令注册
  src/app_state.rs            # 运行期全局状态单一入口 AppState（8 张执行期表，lib.rs 注入）
  src/db.rs                   # SQLite 全部表读写 + 迁移链 + 日志轮转
  src/bot.rs                  # 机器人门面：跨模块 re-export + 子模块声明
  src/bot/registry.rs         # 工具单源真相：29 个 schema + TOOLS_TABLE → TOOLS/MUTATING_TOOLS/dispatch
  src/bot/dispatch.rs         # execute_tool 分发（查表）+ pre_execute/skill 钩子 + tool.return 审计
  src/bot/tools.rs            # 28 个 tool_* 实现（任务卡 CRUD/子任务/文档生成/联网/时间/记忆转发）
  src/bot/config.rs           # BotConfig/ApiProvider/PermMode/KeySlot（key 走系统 keyring）+ audit_log
  src/bot_py.rs               # 本机 Python 执行 + 文档脚本模板（EXTRACT/MAKE_DOCX 等）
  src/bot_web.rs              # 联网搜索（Tavily/Brave 可配置，未配置走 Bing+百度）/ 网页抓取（公网白名单）
  src/migration.rs            # 桌面清理引擎（规则匹配/文件迁移/轮询）
  src/profile.rs              # 头像资料
  src/api.rs                  # 本地 HTTP API（tiny_http + SSE）
  capabilities/default.json   # main + widget 窗口权限
```

### 数据存储与同步（SQLite，单写者）

- **数据源**：`wmessage.db`（便携模式在 exe 同目录；否则 app_data_dir：Win `%APPDATA%\com.renshi.wmessage\wmessage.db`，macOS `~/Library/Application Support/com.renshi.wmessage/wmessage.db`；AI 产物目录 `AI_Gen_Files` 与数据库同目录，启动即预建）
- 表 `tasks`（`tags`/`subtasks` JSON 文本列，含 schedule/sched_last 定时列、bot_assigned 归属列、ord 排序列、updated_at 合并导入列）；另有 `workspace_items`（工作区）、`bot_messages`/`bot_sessions`（聊天多会话）；**行级增量读写**（无全库覆盖写）
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

### 测试与门禁

- **提交时自动跑** pre-commit（`scripts/install-hooks.sh` 装一次）→ `scripts/test-fast.sh`：按改动文件智能跳过，`cargo fmt --check` / `cargo check` / `tsc` / `vitest --changed`，外加四道防回潮门禁（详见 `docs/testing.md`）：审计批次号防线、cargo machete（未使用 Rust 依赖）、Tauri 桥一致性（命令注册↔前端 invoke、emit↔listen）、knip（前端死代码/依赖）
- **手动快速验证**：`cargo test --lib`（Rust lib 637 例）、`npm test`（前端 209 例）、`bash scripts/test-fast.sh`
- **全量验证**（push 前）：`bash scripts/test-all.sh`（cargo nextest 全量 + tests-audit 一致性检查 + vitest）；集成测试 `cargo test --test llm_integration` 等；bge 模型真实推理冒烟 `cargo test --lib memory::embed -- --ignored`
- 维护手册：`docs/testing.md`（各门禁防什么、失败了怎么修、误伤豁免方式）

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
- **绿色包四件套**：wmessage.exe + WebView2Loader.dll（必带，缺它报「找不到 webview2loader.dll」）+ MicrosoftEdgeWebview2Setup.exe（Win10 备用）+ README.txt
- **.NET 侧车（Word 修订）**：`npm run publish:docx-dotnet`（= `scripts/publish-docx-dotnet.sh [输出目录]`）publish win-x64 self-contained 并归位到便携包 `dotnet/`（macOS 可 cross publish；Windows 用 Git Bash 跑）
- **zip 用 Python zipfile 打**：macOS `zip -j` 的 Unix 扩展字段会让 Win 资源管理器解压报「位置不可用」
- NSIS 安装包在 macOS 打不了（makensis 跨平台崩），发绿色版 zip；NSIS + 代码签名待做（M6）
- 老链路备查（cargo-xwin + MSVC）：`tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc --no-bundle`（llvm/lld + cargo-xwin 环境）

## 上线验收清单（Windows 10 / 11 实机）

> 每项发布前在 Win10 测试机（已装 WebView2 Evergreen runtime）+ 任意 Win11 机器各过一遍。

- [ ] 首次启动：种子任务出现、看板三列正常、无报错弹窗
- [ ] WebView2 依赖：Win10 未装 runtime 时便携包附带的安装器可补救；Win11 免装直接可跑
- [ ] 便携模式：exe + WebView2Loader.dll 同目录启动；任务数据随目录走（U 盘换机器数据仍在）；exe 目录只读（如 Program Files）时兜底 app_data_dir；zip 内不解压直接双击运行时数据退化 app_data_dir（bot.log 有 WARN），AI_Gen_Files 不建到系统 temp
- [ ] 挂件：贴右/左/顶三边吸附、悬停滑出、📌 锁定、拖动换边、圆角跟随边缘；重启后位置记忆
- [ ] 全局快捷键：Ctrl+Alt+W 唤起/隐藏、Ctrl+Alt+N 快速新建、Ctrl+Alt+T 切主题（与浏览器/输入法等无冲突；被占用时 App 照常启动）
- [ ] 托盘：左键恢复主窗口、右键「打开主窗口/退出」；退出后进程完全结束
- [ ] 剪贴板：任务卡 📋 复制文件+标题 → 粘贴到资源管理器得文件、粘贴到微信/飞书输入框得标题文本
- [ ] 文件操作：绑定文件/文件夹、打开、解绑；路径含中文与空格
- [ ] CSV 规则表：下载模版 → Excel/WPS 编辑中文 → 导入成功（GBK/UTF-8 均可）；桌面清理按规则移动/删除已归档任务的绑定文件
- [ ] 任务数据管理：导出 JSON → 导入合并（同 id 保留最新修改）
- [ ] 机器人（若配置 API）：聊天流式回复、工具调用（新建/完成任务）、删除任务弹确认、60s 超时自动拒绝
- [ ] 挂件工作区/定时/头像功能与主窗口同步正常

## 文档

- `SPEC.md` — 产品规格（唯一依据）
- `DEVLOG.md` — 开发日志（里程碑 + 踩坑记录）
- `docs/testing.md` — 测试与门禁手册（日常提交流程 / 各门禁防什么 / 全量验证 / 豁免方式）
- `docs/logo/` — Logo 规范 V1.0（`WMessage-LOGO-GUIDELINES.md`，含老板裁定「以图片为准」）+ 5 版处理资产（去水印/透明底/多尺寸）

## 深色模式

- **三态主题**：浅色 ☀️ / 深色 🌙 / 跟随系统 🖥️，主窗口 header 与挂件面板均有循环切换按钮；选择持久化 localStorage（`wmessage-theme`），双窗口 storage 事件同步；跟随系统用 matchMedia 实时响应；启动无闪屏（index.html 预渲染脚本）
- **全局快捷键**：macOS `Cmd+Ctrl+T` / Win `Ctrl+Alt+T` 快速浅↔深切换（Rust 侧 emit `toggle-theme` 只发主窗口，挂件靠 storage 同步防双触发）
- **主题变量**（`src/ui/main.css` `:root` / `.dark` 两套独立）：`--bg` 页面底（深色中度深灰 #1f242b 非纯黑，比卡片表面更暗）、`--surface` 表面、`--sh-low/--sh-high`（+strong 档）双阴影（深色专属更暗阴影+弱高光，不复用浅色参数）、`--sh-top` 顶部 1px 内高光（深色玻璃边缘）、`--t1..t6` 文字五级、`--brand` 品牌色（深色提亮）、`--success/--danger`、`--input-bg/--hover-bg/--edge`
- 组件颜色全部 `text-[var(--tN)]` 式引用，nm-card/nm-inset/nm-outset/nm-btn hover/active 两套反馈，主题切换平滑过渡 0.28s

## 本地 HTTP API（外部机器人接口）

- 设置页开关（默认**关闭**），开启后监听 `http://127.0.0.1:4763`（只绑本机，不监听公网）；关闭立即停止
- 鉴权：所有端点要求 `Authorization: Bearer <token>`（token 在设置页展示/复制，存数据目录 api-token.txt）
- 端点：`GET /api/tasks`（?status=todo|doing|done、?trash=1、?archived=1、?all=1）、`GET /api/tasks/:id`、`POST /api/tasks`、`PUT /api/tasks/:id`（含 due/tags/archived/deleted）、`DELETE /api/tasks/:id`（软删幂等）、`GET /api/events`（SSE，?since=断线重放）
- 外部修改任务后：看板自动刷新（tasks-updated 通道）+ SSE 客户端实时收到 tasks-changed 事件
- 安全：无文件读取/遍历端点；请求体 1MB 上限；限流 120 次/分；字段长度校验；token 轮换；开关自动记忆（退出时开启则重启自启）；操作日志 api.log

## 已知待办

- ~~M5 全局快捷键~~ ✓（Cmd+Ctrl+W/N/T，macOS；Ctrl+Alt+W/N/T，Windows/Linux）
- M6：NSIS 安装包 + 代码签名 + macOS dmg（交叉编译 exe + 绿色包流程已通 ✓）
- Logo 规范 2.3 单色托盘版（16/32px，现有渐变图缩到托盘尺寸会糊）
- 挂件窗口伸缩平滑动画（当前瞬时伸缩）
- ~~深色模式~~ ✓（三态主题：浅色/深色/跟随系统，见「深色模式」章节）
- ~~任务数据备份/导出~~ ✓（设置页「任务数据管理」：全量 JSON 导入导出，含归档/回收站，按 id 合并保留最新修改）
- run_python 真隔离沙箱（当前是用户权限执行 + 独立临时目录 + 60s 超时；真隔离需 OS 级 sandbox，如 macOS Seatbelt / Windows 受限 token）。**定性（2026-08-16 上线前审计）**：保持现状——默认关闭 + 设置页开启确认 + 风险文案诚实化，首版不做每次执行确认
- macOS dmg 首版不做（2026-08-16 定），先发 Win 绿色包
