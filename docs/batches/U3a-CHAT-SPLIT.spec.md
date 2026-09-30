# Batch Spec: U3a-CHAT-SPLIT

## 目的

UI 改造战役第三批第一步（依据 `docs/UI-REDESIGN-HANDOFF-2026-09-30.md` U3 定义，
与已登记的 B5-2/3 ChatPanel 拆分合并执行）：**行为等价拆分，只拆不换肤**。

1. **四文件拆分**（ChatPanel.tsx 1862 行 → orchestrator 1213 行 + 四个子文件）：
   - `SessionList.tsx`（129 行）：🤖 会话切换器 + 会话下拉（切换/删除/新建）+ 🎯 选任务钮；
   - `MessageList.tsx`（252 行）：消息列表 + **MsgBubble React.memo**（B5-2 流式
     性能项）+ `MessageListUnmemoized` 对照导出（仅供 perf 测试）；
   - `InputArea.tsx`（388 行）：斜杠 picker + 输入卡（附件/textarea/模型下拉/
     推理强度/发送-停止一体键）；
   - `useChatUi.ts`（75 行）：useDropdownTop / useOutsideClose / useAutoGrow
     三个纯视图 hook（原三处 outside-click effect 去重为一处实现）。
   orchestrator 保留全部数据流：流式六事件监听、runChat/send/runSlashCommand、
   会话管理、DRAFT-1 草稿隔离——**消息数据流/斜杠命令语义/Tauri 命令调用零改动**，
   JSX 逐字搬移（拆分期间仅有的渲染层改动：type="button"、aria 三处、空 picker
   守卫，见下 3）。
2. **MsgBubble memo 前后对比数据**（B5-2 验收，`MessageList.perf.test.tsx` 用
   React.Profiler actualDuration 度量，30 气泡 × 50 次流式更新）：
   **memo=0.28ms vs 无 memo=16.43ms（≈59×）**，三次运行 22.5×/61.6×/59.0×，
   倍率结论稳定；配套 useCallback 稳句柄（copyMessage/openTaskInMain/removeMessage
   经 messagesRef/switchSession 经 sessionIdRef 依赖最小化，防句柄抖动击穿 memo）。
3. **oxlint 存量 32 warn 清零**（B5-1 登记项）：`.oxlintrc.json` 关
   `oxc/no-map-spread`（9 处 `.map(t=>({...t,x}))` 为本仓 React 不可变更新惯用法，
   配置注释说明）；其余 23 处逐点处置——6 处真实修复（artifact key 用 path、
   MigrationPanel effect 后移、artifact-batch-ready 监听统一切 expandFnRef、
   InputArea 解构改为顶层 ref props、空斜杠容器守卫、嵌套三元提取
   placeholderOf/bubbleBody），17 处 `oxlint-disable-next-line`/`eslint-disable`
   带理由（render 期 latest-ref 既有模式 ×4、挂载期外部数据同步 ×8、
   props→state 受控同步 ×3、防重复标签撞 key ×2、顺序删除 ×1、
   setSize 依赖省略 ×1）。
4. **a11y 补齐**（随拆分顺手，不涉行为）：SessionList 触发钮
   aria-haspopup/aria-expanded、下拉 role="menu"+menuitem+aria-current、
   🎯/🗑 aria-label；InputArea 各钮 type="button"+aria-label（＋/停止/移除附件）。
5. types.ts +ReasoningLevel/ModelItem、constants.ts +EFFORT_LABELS/
   PROVIDER_LABELS（自 ChatPanel.tsx 迁入，纯类型/常量搬迁）。

## 红线核对

- 消息数据流/斜杠命令语义/Tauri 命令调用零改动：监听器、invoke 面、
  runChat/runSlashCommand/send 函数体逐字保留（useCallback 包装不换执行语义）。
- 换肤零改动：气泡/输入卡 class 与结构逐字搬移（U3b 另批）。
- 双窗口/事件桥零改动：chat-focus-session（U2 增量）语义不变。
- ChatPanel.test.tsx **零改动全绿**（DOM 结构不变，869 行断言按语义查询）；
  WidgetApp 折叠不丢聊天回归绿。
- 全部测试 348 绿（44 文件，+1 perf 测试）；oxlint **0 warn**（32→0，本批顺清
  B5-1 登记项）；knip 0；tsc 通过。

## ocr review 复审处置记录

49 条（5 high / 16 medium / 28 low，LLM 复审两轮）：

- **high 5 修**：removeMessage deps 含 messages 致流式期 memo 边界每帧击穿
  （messagesRef 化，deps=[]）；useOutsideClose deps 含 close/refs 致下拉打开期
  每帧重订阅（ref 转发恢复「仅 open 翻转挂/摘」的原节拍）；MessageList 嵌套
  三元（bubbleBody 提取）；SessionList 🎯 缺 aria-label；InputArea 按钮缺
  type="button"。
- **medium 12 修/驳**：TaskCardContent artifact key 补 path 可选 fallback；
  4 处 disable 理由注释失实（按 lint 真实触发机制重写）；SessionList 下拉
  aria 完整化（menu/menuitem/aria-current/删除钮 label）；InputArea ＋/停止/
  移除附件 aria-label；placeholder 嵌套三元提取；InputArea ref 顶层 props 化
  （嵌对象触发 react/refs 全对象标记）；空斜杠容器守卫。**驳回 4**：
  会话删除「无确认」不实（deleteSession 内有 window.confirm）；isImagePath
  迁 format.ts（跨文件重构留 U3b）；附件 title 硬编码限制文案（与 Rust 侧
  对齐属独立任务）；Tab 劫持为斜杠补全拍板交互。
- **low 28**：修 8（注释措辞/冗余、空容器、hasContent 提升、aria 补齐），
  记 20（非空断言、tuple 字面量、onMouseEnter 重渲染、ResizeObserver 观察面
  等既有模式/微优化，均随原码搬移不属本批行为面，登记 U4 打磨）。

## spec 起草后自查三条

1. expected_files = staged 全集 22 项（含本 spec）。
2. 预算：M 文件 numstat 合计 +211/-660（ChatPanel.tsx +132/-649 为拆分主体，
   其余 15 文件均为 ≤16 行的 lint 处置）；新文件 6 个共 1061 行（五个源/测试
   文件 934 行 + 本 spec 127 行——初稿预算 990 漏计 spec 自身，B6 自查条 ②
   同款失误，提交门禁拦截后修正为 1100）。
3. findings 留 1 条：MsgBubble memo 前后对比数据随 perf 测试入库
   （MessageList.perf.test.tsx），三次运行 22.5×/61.6×/59.0×。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U3a-CHAT-SPLIT",
  "family": "ui-redesign",
  "expected_files": [
    ".oxlintrc.json",
    "docs/batches/U3a-CHAT-SPLIT.spec.md",
    "src/components/ArtifactBatchDialog.tsx",
    "src/components/ChatPanel/ChatPanel.tsx",
    "src/components/ChatPanel/InputArea.tsx",
    "src/components/ChatPanel/MessageList.perf.test.tsx",
    "src/components/ChatPanel/MessageList.tsx",
    "src/components/ChatPanel/SessionList.tsx",
    "src/components/ChatPanel/constants.ts",
    "src/components/ChatPanel/types.ts",
    "src/components/ChatPanel/useChatUi.ts",
    "src/components/EvolutionPanel/EvolutionPanel.tsx",
    "src/components/MigrationPanel.tsx",
    "src/components/SettingsPage/McpPanel.tsx",
    "src/components/SettingsPage/ProfileRow.tsx",
    "src/components/SettingsPage/SettingsPage.tsx",
    "src/components/SettingsPage/SkillsPanel.tsx",
    "src/components/TaskCardContent.tsx",
    "src/components/TodoCard/SubtaskRow.tsx",
    "src/components/TodoCard/TodoCard.tsx",
    "src/components/WidgetApp/useDragCleanup.ts",
    "src/components/WidgetApp/WidgetApp.tsx"
  ],
  "max_lines_added": 1250,
  "max_lines_removed": 780,
  "max_new_files_lines": 1100,
  "findings": [
    { "file": "src/components/ChatPanel/MessageList.tsx", "note": "MsgBubble memo 前后对比数据随 perf 测试入库：memo=0.28ms vs 无memo=16.43ms（≈59×，三次运行 22.5×/61.6×/59.0×）" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
npx --no-install vitest --run          # 348 绿（含 perf 数据输出）
npx --no-install tsc --noEmit -p tsconfig.json
npx --no-install oxlint src            # 0 warn / 0 error
npx --no-install knip --no-progress    # exit 0
```
