# Batch Spec: U2-NAV

## 目的

UI 改造战役第二批（依据 `docs/UI-REDESIGN-HANDOFF-2026-09-30.md` U2 定义）：
主窗口信息架构升级——左侧窄导航栏 + ⌘K 命令面板。

1. **左侧导航栏**（App.tsx，w-44 窄栏，Linear 式）：品牌行（logo+h1）→ 新建任务
   （⌘N 提示芯片）→ 搜索（⌘K 提示芯片）→ 分隔线 → 视图切换（首页/归档/工作区/
   回收站，lucide 图标 15px，选中走 U1 的 nm-inset 凹陷语义 + aria-current）→
   底部设置入口。顶部工具条职能**全部迁移**后解散（内容区获得整屏高度，
   `h-screen flex` + `main overflow-y-auto`，子页面无滚动假设不受影响）。
2. **⌘K 命令面板**（新组件 CommandPalette.tsx，条件挂载无 open prop）：
   任务标题 + 会话标题聚合搜索（任务按 updatedAt 降序取 8 / 会话取 6），
   键盘 ↑↓ 选择 + 回车跳转 + Esc 关闭 + 遮罩点击关闭，空态文案，
   结果行带位置徽标（待办/今日/完成/归档/回收站）与「挂件聊天」标注。
   材质：surface-raised 提亮层 + --edge 边框 + --shadow-lg 投影（U1 预告的
   U2 消费点，main.css 本批引回两行）。
3. **⌘N/⌘K 快捷键**（纯前端 window keydown，不碰 Rust 全局注册）：
   ⌘/Ctrl+N = 关面板+切看板+新建任务进编辑态（与既有 quick-add 事件共用
   startNewTaskRef）；⌘/Ctrl+K = 面板开合。isComposing 跳过（中文输入法
   组合期不触发）。
4. **会话跳转机制**：新增前端事件 `chat-focus-session`（既有事件桥零改动，
   纯增量）——App 发出；WidgetApp 监听→展开挂件面板（ref 写走 effect，避开
   render 期写 ref 的新 lint）；ChatPanel 监听→复用 switchSession 完整切换
   （仅 setSessionId 不会加载历史）。任务跳转 = 按任务所在位置切视图 + 进编辑态
   （viewForTask 与既有 edit-task 监听共用一套映射）。
5. App.test.tsx 同步三处断言（新建任务按钮文案迁移 / 设置入口由 title 查询改
   文本查询），语义不变。

## 红线核对

- KanbanBoard.tsx **零改动**（三列拖拽核心交互不动；KanbanBoard.test 8 绿）。
- `src-tauri/` 零改动；既有双窗口事件桥（tasks-changed/tasks-updated/edit-task/
  quick-add/toggle-theme/workspace-*）语义与实现零改动，仅新增 chat-focus-session。
- oxlint 32 warn 存量持平（复审期间一度 39，新增 7 条已全部修回）；0 error；
  knip exit 0；tsc 通过；vitest 347 全绿（+6 CommandPalette 新测试）。
- 四档字号机制、主题三态机制（theme.ts 零改动）、U1 token 体系不动。

## 测试与验收

- 定向：vitest 347 绿（43 文件）/ tsc / oxlint 32 持平 / knip 0。
- 视觉（web-gui-tester 黑盒 + dev shim 夹具，截图在 `gui-test-screenshots/`，
  不入库）：浅色看板+导航栏、深色看板+导航栏、深色工作区+导航栏三张实截图；
  ⌘K 面板 DOM 断言全过（开合/aria-modal/自动聚焦/空态/面板取 --surface-raised
  提亮层/投影 --shadow-lg 实测生效——期间抓出 Tailwind `shadow-[var(--x)]`
  被解析为阴影颜色的陷阱，改内联 style 消歧）；Esc 关闭实测。
  **受限项**：①面板展开时截图必超时（IAB 对全屏遮罩的合成限制，已定位规律，
  面板视觉由 DOM 断言 + 6 条单测背书，结果行的选中态渲染随 U3 在 Tauri 环境
  补截图）；②浏览器模式下任务/会话数据为空（U1 已登记的存量限制），面板
  聚合搜索的实数据形态在 Tauri 环境复核；③⌘N/⌘K 为浏览器保留键，IAB 内
  无法触发，keydown 实现待 Tauri 实测（导航栏按钮提供等价 GUI 路径）。
- 拖拽回归：KanbanBoard 零 diff + 单测绿；双窗口同步回归：theme.ts/事件桥
  零 diff + theme.test 11 绿。

## ocr review 复审处置记录

24 条意见（1 critical / 3 high / 10 med / 10 low）：

- **critical** CommandPalette 残留 `if (!open) return null` 行（open 未声明，
  TS 解析到全局 window.open 恒真——地雷行，条件挂载下永远不可达但属未定义
  行为）——当场删除。
- **high** 3 条：ChatPanel 仅 setSessionId 不加载历史（改复用 switchSession
  完整切换）；RailButton 嵌套三元（改 if/else）；快捷键在输入编辑态仍触发
  （部分处理：补 isComposing 守卫防输入法组合期误触；全局快捷键语义本身
  为拍板要求，输入框内可用性 Linear 同款，驳回完全禁用方案）。
- **med** 8 条修：复用 useTauriListen 抽象、emit 失败补 console.error、
  品牌 h1 回归、PaletteSession 保留本地声明（**驳回**：ChatPanel/types 跨
  目录耦合在 U3a 拆分前不引入，注释已说明）、aria-modal、结果行
  tabIndex={-1}（键盘导航集中于搜索框的 combobox 模式）、会话加载
  async/await + console.warn、viewForTask 与 edit-task 共用去重。
- **med** 2 条部分/驳回：命令面板 scrollIntoView 命令式查询保留（React 无
  声明式等价物，能力守卫注释说明）；快捷键与 quick-add 双入口去重已做。
- **low** 修 5：userAgent 替代废弃的 navigator.platform、kbd 垂直内边距
  对齐、title="设置" 冗余删除（测试同步改文本查询）、⌘N 与 quick-add 共用
  startNewTaskRef、TaskIcon 与视图映射复用。**记 2**：WidgetApp 新监听与
  artifact-batch-ready 形态差异（旧监听是 render 期 ref 写——新 lint 下
  不可再写，统一重构涉既有代码留 U4 挂件批）；scrollIntoView 同前。

## spec 起草后自查三条

1. expected_files = staged 全集 8 项（**含 spec 文件本身**）。
2. 预算：M 文件 numstat 合计 +251/-110（App.tsx +216/-104 超出交接预估的
   ±120——导航栏壳 + RailButton 去重组件 + 快捷键/跳转处理器 + 工具条解散
   同文件内完成，净 +112；其余四文件均 ≤14 行增量）；新文件 CommandPalette
   203 行 + 测试 127 行 + spec 本文件。
3. findings 留 2 条：面板展开态截图受 IAB 合成限制（DOM+单测背书，U3 补）；
   浏览器模式数据为空致聚合搜索实数据形态未截图（存量限制，Tauri 复核）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U2-NAV",
  "family": "ui-redesign",
  "expected_files": [
    "src/App.test.tsx",
    "src/App.tsx",
    "src/components/ChatPanel/ChatPanel.tsx",
    "src/components/CommandPalette.test.tsx",
    "src/components/CommandPalette.tsx",
    "src/components/WidgetApp/WidgetApp.tsx",
    "src/ui/main.css",
    "docs/batches/U2-NAV.spec.md"
  ],
  "max_lines_added": 420,
  "max_lines_removed": 180,
  "max_new_files_lines": 480,
  "findings": [
    { "file": "src/components/CommandPalette.tsx", "note": "面板展开态截图受 IAB 遮罩合成限制，DOM 断言+单测背书，U3 在 Tauri 环境补截图" },
    { "file": "src/App.tsx", "note": "⌘N/⌘K 为浏览器保留键无法在 IAB 触发，keydown 实现待 Tauri 实测；导航栏按钮为等价 GUI 路径" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
npx --no-install vitest --run          # 347 绿
npx --no-install tsc --noEmit -p tsconfig.json
npx --no-install oxlint src            # 32 warn（存量持平）/ 0 error
npx --no-install knip --no-progress    # exit 0
```
