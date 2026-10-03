# Batch Spec: U20C-ICON1

## 目的

emoji → lucide 线性图标迁移第一批（方案见 `docs/UI-ICON-PLAN-2026-10-04.md`，
执行其中可立即做的批 1–3，约 45 处 DOM 符号）：任务卡家族（挂件+主窗双端同步）、
聊天面板、设置页/工作台/挂件。彩色 emoji 与文本符号（☰✓★）统一为 lucide
stroke 图标——跨平台渲染一致、跟随主题 token、读屏语义可控、与 nm 线性设计
语言归一。

## 改动范围（18 源文件 + 9 测试文件）

- **批 1 任务卡家族**：TodoCard / TaskCardContent（GripVertical 拖拽手柄、
  Paperclip/Folder 文件 chip、Bot 交给机器人、Clock 定时、Puzzle 收尾结果、
  TriangleAlert 未完成项、FileText 保留文件删除）+ DoneCircle（✓ → Check，
  挂件/主窗共用组件一处改双端生效）；
- **批 2 聊天面板**：InputArea（Shield 授权 pill、Image/Paperclip 附件 chip）、
  ChatPanel（📌 → Pin）、MessageList（MessageCircle 思考过程、TriangleAlert
  失败行、Check 复制反馈/工具行状态）、UserBubbleContent（附件 chip）；
- **批 3 设置页与工作台**：SettingsPage（Sun/Moon/Monitor 主题三档、
  Download/Upload 导出导入×4、Check 保存/复制反馈、TriangleAlert 双开冲突）、
  MigrationPanel（Check 规则状态、Download/Upload/ClipboardList、
  TriangleAlert 删除警告）、MemoryPanel（★ → Star 重要度）、SkillsPanel、
  ProfileRow、KanbanBoard（Archive 已归档）、WidgetApp / WorkspacePage
  （Lock/Pin 常驻、GripVertical、Link2/Folder/FileText 链接 chip、
  FileText/FolderOpen 选择文件/夹）、ErrorBoundary、ArtifactBatchDialog
  （ORIGIN_LABEL 改 ReactNode 内嵌 Bot/Clock/Archive）。

## 按方案有意保留（不换）

- 聊天**消息正文**里的 emoji（assistant content 的 ⚠️/📦 压缩与失败提示——
  内容纪律，同 LLM 输出）；addHint 字符串提示（string API，随批 4 弹窗组件化
  再议）；🧩 子任务会话标题契约（后端 `bot_orchestrator.rs` 发前缀 + 前端
  `ChatPanel.tsx:1170` `startsWith("🧩")` 判流 + 后端测试锁串——方案批 5）；
- 句内文本 ✓（「已存入系统凭据存储 ✓」等状态文案，单色文本字形非彩色
  emoji，保留）；errorHandler 原生弹窗 ❌💡🔁（批 4 前置：弹窗组件化）；
- 代码注释/测试描述里的 emoji（文档措辞）。

## 规范（方案 §4 落地）

按钮/工具栏图标 `size={12–13}`、行内 chip/徽标 `size={10–11}`；一律
`stroke=currentColor` 继承所在文字色（语义色仅 DeleteConfirmDialog 警告行显式
`--danger`）；行内对齐用 `inline-block align-[-1/-2px]`；可访问名不回归——
图标全部 `aria-hidden`，按钮 accessible name 由残留文字或既有 title 提供。

## 测试适配（9 个测试文件，断言跟随渲染内容走）

- 附件 chip 断言 `"📎 a.pdf"` → `"a.pdf"`（图标 aria-hidden 后文本节点即纯
  文件名，TodoCard ×8 / TaskCardContent ×7）；
- 按钮 accessible name 去 emoji：「📥 导入」→「导入」、「📤 导出」→「导出」、
  「📋 查看迁移日志」「⬆ 导入规则表」「⬆ 导入技能文件夹」「📁 dir」等；
- 主题三档「☀️ 浅色/🌙 深色/🖥️ 跟随系统」→ 纯文字断言；
- 「已保存 ✓」「★4」→ 「已保存」/ title 定位（`getByTitle("重要度 4/5")`）；
- 消息正文与契约断言（⚠️ 气泡、🧩 标题）**零改动**（对应生产代码未动）。

## 验证

- vitest 401/401 全绿（46 文件）；tsc --noEmit 过（清掉一个未使用 import）；
  test-fast.sh exit 0（knip/oxlint/桥审计/错误码审计）；改动 +233/-120。
- 残余 DOM emoji 24 行全部为方案保留项（终扫脚本核过：消息正文 7、契约 2、
  注释 9、句内 ✓ 5、errorHandler 3 项在批 4）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U20C-ICON1",
  "family": "ui-icon-migration",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/U20C-ICON1.spec.md",
    "src/App.test.tsx",
    "src/components/ArtifactBatchDialog.tsx",
    "src/components/ChatPanel.test.tsx",
    "src/components/ChatPanel/ChatPanel.tsx",
    "src/components/ChatPanel/InputArea.tsx",
    "src/components/ChatPanel/MessageList.tsx",
    "src/components/ChatPanel/UserBubbleContent.tsx",
    "src/components/DoneCircle.tsx",
    "src/components/EvolutionPanel/DeleteConfirmDialog.tsx",
    "src/components/KanbanBoard.tsx",
    "src/components/MigrationPanel.test.tsx",
    "src/components/MigrationPanel.tsx",
    "src/components/SettingsPage.test.tsx",
    "src/components/SettingsPage/MemoryPanel.test.tsx",
    "src/components/SettingsPage/MemoryPanel.tsx",
    "src/components/SettingsPage/ProfileRow.test.tsx",
    "src/components/SettingsPage/ProfileRow.tsx",
    "src/components/SettingsPage/SkillsPanel.test.tsx",
    "src/components/SettingsPage/SkillsPanel.tsx",
    "src/components/SettingsPage/SettingsPage.tsx",
    "src/components/TaskCardContent.test.tsx",
    "src/components/TaskCardContent.tsx",
    "src/components/TodoCard.test.tsx",
    "src/components/TodoCard/TodoCard.tsx",
    "src/components/WidgetApp/WidgetApp.tsx",
    "src/components/WorkspacePage.tsx",
    "src/ui/ErrorBoundary.tsx"
  ],
  "max_lines_added": 400,
  "max_lines_removed": 200,
  "max_new_files_lines": 150,
  "findings": [
    { "file": "src/components/TodoCard/TodoCard.tsx", "note": "批1 任务卡家族图标化（挂件侧 TaskCardContent 同步，老板一致性锁）；chip 内联图标 align-[-2px] 对齐" },
    { "file": "src/components/SettingsPage/SettingsPage.tsx", "note": "主题三档 Sun/Moon/Monitor（tuple 改对象数组 + LucideIcon 类型）；导出导入 Download/Upload 对齐 MemoryPanel 头部既有口径" },
    { "file": "src/components/ArtifactBatchDialog.tsx", "note": "ORIGIN_LABEL Record<string> → Record<ReactNode>，句内内联 11px 图标" },
    { "file": "src/components/ChatPanel/ChatPanel.tsx", "note": "仅 📌 Pin 一处；消息正文与 🧩 契约按方案保留" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
npx vitest --run                                  # 401/401
bash scripts/test-fast.sh                         # exit 0
python3 scripts/batch-verify.py docs/batches/U20C-ICON1.spec.md
```
