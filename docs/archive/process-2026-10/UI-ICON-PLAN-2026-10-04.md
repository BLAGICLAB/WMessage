# UI 图标系统方案：emoji 全面替换为 lucide 线性图标（2026-10-04，只方案不动代码）

## 0. 一句话结论

DOM 里正在使用的 emoji/文本符号约 **95 行、20 类符号**，全部可归入既有的
lucide-react 线性图标家族（依赖已在，`^1.49.0`；IconButton/`nm-icon-btn`
家族在 U20A 已确立口径）。建议分 5 批迁移，其中 3 批可立即做（约 60–70 处），
2 批有前置依赖（原生弹窗组件化、后端 🧩 标题契约）需单独评估。

## 1. 现状盘点（实证）

全量扫描（排除注释行后）DOM 中的 emoji/文本符号共 95 行。注意：源码 grep 出的
300+ 处「emoji」**大部分是注释里的文档措辞**（如「🧠 下拉」「⚠️ 必须与…一致」），
不影响 UI，本方案不动它们。真实渲染在界面上的按用途聚类：

| 用途 | 现用符号 | 出现位置（文件） | 约处数 |
|---|---|---|---|
| 附件/文件 chip | 📎 📁 📄 🖼 | TodoCard、TaskCardContent、ArtifactBatchDialog、UserBubbleContent | ~20 |
| 交给机器人 / 定时 | 🤖 ⏰ | TodoCard、TaskCardContent（挂件+主窗双份） | ~6 |
| 完成态 | ✓（文本勾） | DoneCircle、MessageList、ProfileRow | ~7 |
| 拖拽手柄 | ☰（文本符号） | TodoCard、TaskCardContent | 4 |
| 导出/导入按钮 | 📤 📥 | SettingsPage（任务导出/导入 + 工作区导出/导入） | 4 |
| 迁移面板 | ⬇ ⬆ 📋 ⚠️ | MigrationPanel | 5 |
| 聊天状态行 | 💭(思考) 📦(压缩) ⚠️(失败前缀) | ChatPanel、MessageList | ~10 |
| 输入栏 pill/附件 | 🛡(授权模式) ➕(附件) | InputArea | 2 |
| 子任务/收尾 | 🧩(收尾结果) 🗄(已归档) | TaskCardContent、ChatPanel | 2 |
| 链接 | 🔗 | ArtifactBatchDialog 等 | 2 |
| 重要度徽标 | ★（文本星） | MemoryPanel | 2 |
| 错误弹窗文案 | ❌ 💡 🔁 | lib/errorHandler.ts（**原生 alert/confirm**） | 3 |
| 工具行状态 | ✓ / … | MessageList（mono pill 徽章） | 若干 |

已有的正确先例（无需动）：🧠 模型下拉 DOM 里**已经是** lucide `Brain size=13`
（InputArea:258，注释里的 🧠 只是文档措辞）；厂商 logo 用 lobe 的 SVG 资源
（providerLogoMap），本就不是 emoji；MemoryPanel 头部导出/导入/刷新已是
IconButton + Download/Upload/RefreshCw。

## 2. 为什么换（技术理由）

1. **跨平台渲染不一致**：emoji 走系统字体（macOS Apple Color Emoji / Windows
   Segoe UI Emoji / Linux Noto），同一 app 在三端观感、字号、基线都不同；截图
   风格的彩色 emoji 与 nm 设计系统的线性语言冲突。
2. **无法跟随主题**：emoji 是固定彩色位图，不能继承 `currentColor`，暗色模式下
   彩色块突兀，也无法用 `--t3/--danger/--success` 等 token 表达语义。
3. **小尺寸发糊**：任务卡 chip、pill 里 emoji 以 10–11px 渲染，细节糊成一团；
   ☰ ★ ✓ 这类文本符号基线对不齐、字重不可控。
4. **无障碍**：读屏器把 ⚠️ 读成「警告符号」、🤖 读成「机器人脸」，语义不可控；
   lucide + aria-label 可精确命名（IconButton 家族已确立 aria+title 规范）。
5. **既有能力已就位**：lucide-react 已是依赖（全仓已 import 数十个图标），
   IconButton 基件 + `nm-icon-btn` 材质已存在，迁移是「收口」而非「新建」。

## 3. 方案选型

| 方案 | 评估 |
|---|---|
| **A. lucide-react 全面化（推荐）** | 依赖/基件/口径全就位，线性 stroke 风格与 nm 材质同语言，`stroke=currentColor` 自动跟主题；成本仅为逐点替换 + 少量测试适配 |
| B. 自绘 SVG sprite | 对单机桌面应用成本过高、无收益，不取 |
| C. 保留 emoji + CSS grayscale/滤镜统一观感 | hack：色彩可压但跨平台字形差异、基线、读屏问题全在；不取 |

## 4. 统一规范（与 U20A 确立的家族口径对齐）

- **尺寸**：按钮/工具栏图标 `size={13}`；行内徽标/chip `size={10–11}`；
  DoneCircle 内 `size={12}`。
- **颜色**：一律 `currentColor`（lucide 默认），自动继承所在元素的 `--tN`
  文字色；语义色在使用处 className 指定（如 `--danger`、`--success`）。
- **材质**：可点击的纯图标操作 → `IconButton`（`nm-icon-btn`）；带文字的
  按钮 → `nm-btn` 内 `<Icon size={13} /> 文字`（gap-1）；纯展示徽标 →
  `inline-flex items-center gap-1`。
- **符号 → 图标映射表**（实现时以此为准，lucide ^1.49 的确切导出名以
  node_modules 为准，注意新旧命名如 TriangleAlert/AlertTriangle）：

| 现用 | lucide（候选） | 场景 |
|---|---|---|
| 🤖 | Bot | 交给机器人按钮 |
| ⏰ | Clock | 定时执行 |
| 📎 | Paperclip | 附件 chip / 绑定文件 |
| 📁 / 📂 | Folder / FolderOpen | 绑定文件夹 / 目录 |
| 📄 | FileText | 文件 chip |
| 🖼 | Image | 图片 chip |
| ☰ | GripVertical | 拖拽手柄 |
| ✓ | Check | 完成态（DoneCircle、工具行状态） |
| 📤 / ⬆ | Upload | 导出（对齐 MemoryPanel 头部既有口径） |
| 📥 / ⬇ | Download | 导入 |
| 📋 | ClipboardList | 查看迁移日志 |
| 💭 | MessageCircle | 思考过程折叠行 |
| 📦 | Archive | 上下文压缩 / 批量执行 |
| ⚠️ / ⚠ | TriangleAlert | 失败/警告前缀 |
| 🛡 | Shield | 授权模式 pill |
| ➕ | Plus | 添加附件 |
| 🧩 | Puzzle | 子任务/收尾结果 |
| 🗄 | Archive | 已归档 |
| 🔗 | Link | 链接 chip |
| ★ | Star | 重要度徽标 |
| ❌ 💡 🔁 | CircleX / Lightbulb / RotateCcw | 错误弹窗（见批 4 前置） |

## 5. 分批路线图（按风险与依赖排序）

### 批 1：任务卡家族（~30 处，可立即做）
TodoCard + TaskCardContent + DoneCircle：📎📁📄🖼🤖⏰☰✓。
**约束**：TaskCardContent.tsx:14 注释锁「挂件与主窗口显示一致（老板要求）」——
两份实现必须同批同步改，改完过一遍挂件/主窗截图对比；DoneCircle 是共用组件，
改一处即双端生效（✓ → Check size=12）。

### 批 2：聊天面板（~15 处，可立即做）
InputArea 的 🛡 pill → Shield size=10、➕ → Plus size=13；ChatPanel/MessageList
的 💭 → MessageCircle、📦 → Archive、⚠️ 前缀 → TriangleAlert size=11
（失败/警告行的语义色随行内既有 `--danger/--warn` token）。

### 批 3：设置页与工作台（~20 处，可立即做）
SettingsPage 📤📥×4 → Download/Upload（与 MemoryPanel 头部同款 IconButton 或
nm-btn 内嵌图标，视按钮形态）；MigrationPanel ⬇⬆📋⚠️；MemoryPanel ★ →
Star size=10（或保留文本星，倾向 Star 统一）；SkillsPanel/ProfileRow 的 ✓。

### 批 4：错误弹窗（前置依赖：errorHandler 弹窗组件化）
errorHandler.ts 用的是**原生 window.alert/confirm**——原生弹窗无法富文本，
emoji（❌💡🔁）在里面是唯一的视觉语言。替换前提是把错误确认/提示迁到应用内
Dialog 组件（`--danger` 语义色 + lucide 图标）。这是一个独立小项目，建议
单独立批，不混进图标批。

### 批 5：后端聊天流 emoji（前置依赖：跨端契约改造）
- `bot_orchestrator.rs:147/1088` 发的会话标题前缀「🧩 子任务：」——前端
  `ChatPanel.tsx:1169` 以 `title.startsWith("🧩")` **判定子 agent 会话并分流
  停止键**，且后端有测试锁前缀串（bot_orchestrator.rs:1917）。这是**功能契约**
  不只是装饰：替换需两端同批（改前缀串或引入结构化 is_subagent 字段），收益
  仅是观感，建议等真有跨端渲染诉求再做；
- `⚠️ 执行失败：`（bot_orchestrator.rs:1301）等进入聊天消息正文的 emoji 属于
  **消息内容**，同「LLM 输出/用户输入」一类——按内容纪律永不替换。

## 6. 不换的部分（明确排除）

- 代码注释、测试描述里的 emoji（文档措辞，不影响 UI，全仓占绝大多数）；
- 聊天消息正文、LLM 输出、用户输入中的 emoji（内容，不是 UI）；
- 厂商 logo（lobe SVG 资源，已是正确方案）；
- 后端发进聊天流的消息文案（归批 5/内容纪律）。

## 7. 验证与回归策略（每批同款）

1. 改前 grep 测试目录中的对应 emoji/文案断言（现存测试对 emoji 断言极少，
   U20A 已验证走正则/testid 的习惯）；
2. vitest 全量 + tsc + knip（新 import 的图标不可留未使用）；
3. 挂件 + 主窗截图对比（repo 已有 gui-test-screenshots 惯例），重点任务卡
   chip 行、聊天输入栏、迁移面板；
4. 暗色主题过一遍（emoji 时代的彩色块 → 线性图标应更协调，这是验收观感项）。

## 8. 工作量估计

- 批 1–3：合计约 60–70 处 DOM 替换 + 少量测试适配；单批改动面 3–5 文件，
  每批一个会话可完成（含回归）；
- 批 4：依赖弹窗组件化，独立立批（估中等）；
- 批 5：两端联动小批（或无限期挂起，观感收益低、契约成本高）。
