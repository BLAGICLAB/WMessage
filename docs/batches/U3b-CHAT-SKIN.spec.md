# Batch Spec: U3b-CHAT-SKIN

## 目的

UI 改造战役第三批第二步（依据 `docs/UI-REDESIGN-HANDOFF-2026-09-30.md` U3b 定义）：
**纯表现换肤**，对齐参考截图的对话形态。数据流零改动（U3a 已拆出三个表现组件，
本批只动它们的 JSX/class 与一处只读配置读取）。

1. **会话栈**（SessionList）：切换器加会话数 mono 徽章；下拉行改「活动圆点 +
   标题」，活动行圆点用品牌色。**相对时间未实现**：bot_sessions 表有 updated_at
   但 Session 载荷（{id,title}）不带，扩展需动 src-tauri 序列化——超出 UI 批
   红线，登记待后端批（见 findings）。
2. **消息形态**（MessageList）：
   - 助手消息 → **无边框富文本卡**（去 nm-inset 边框底色，近贴列左，全宽）；
   - 用户消息 → 右对齐**浅底气泡**（bg=--inset-bg、20px 圆角、无边框）；
   - 工具调用 → **mono pill 徽章行**（名称+✓/… 状态点，成功绿）+ 折叠
     **「进程 N/M」**详情（逐工具入参，默认收起；ToolBadges 自身 memo——文本
     tick 不改 tools 引用时跳过徽章行渲染）；
   - **文件变更摘要条**：内容中提取的文件路径（extractFilePaths 既有接口）
     ≤2 个平铺 pill、≥3 个折叠为「📄 N 个文件」摘要条（aria-expanded + 展开
     列表）。**+/- 行级统计未实现**：当前事件面（bot-tool-*）不携带 diff 行数，
     不接假数据，登记待后端批（见 findings）；
   - 💭 思考 / ⚠️ Skill 失败折叠行保留原样。
3. **输入卡**（InputArea）：圆角 16→20px；工具栏新增 **🛡 授权模式只读 pill**
   ——读 bot_get_config 既有载荷的 `permMode` 字段（BotConfigView
   `#[serde(rename_all = "camelCase")]`，None/非法值回 ask，与后端
   PermMode::from_cfg 一致），PERM_LABELS 本地化短标签进 pill、完整语义在
   title；设置页维护，面板不承载操作。圆形发送钮/模型选择器/推理强度/
   斜杠浮层保留。
4. types.ts +PermMode、constants.ts +PERM_LABELS（与 ReasoningLevel/EFFORT_LABELS
   同模式）；ChatPanel orchestrator 仅增 permMode state 读取（+10 行）。
5. ChatPanel.test.tsx 两处断言随表现更新（工具行 🔧 search → pill 文本
   "search" + 「进程 1/1」折叠），语义不变（工具名可见 + 可展开）。

## 红线核对

- 消息数据流/斜杠命令语义/Tauri 命令调用零改动：invoke 面不变（permMode 读
  既有 bot_get_config 载荷字段，非新命令/新字段——后端本就序列化该字段给设置页）；
  src-tauri/ 零触碰。
- 流式性能不回退：perf 测试三批数据 U3a 59×/61.6×/22.5× → 本批 72.7×/35.9×/
  **85.5×**（0.26ms vs 22.51ms），memo 边界与 ToolBadges/FileSummary 子 memo
  全部保持。
- 全部测试 348 绿；oxlint 0 warn；knip 0；tsc 通过。

## 验收记录（视觉）

`__dev_shim.html` 夹具升级（stub `__TAURI_INTERNALS__.invoke` 返回与
BotConfigView/会话/历史**真实数据形状一致**的罐装数据 + 挂载真实 ChatPanel），
web-gui-tester 截图三主题（`gui-test-screenshots/u3b_*.png`，不入库）：
- 深色：会话栈徽章/右对齐浅底气泡/无边框助手卡/mono pill/进程折叠/输入卡
  大圆角/🛡 弹授权 pill 全部到位（u3b_chat_dark + u3b_input_dark）；
- 浅色：同构验证（u3b_chat_light）；
- system：DOM 断言解析正确（prefersDark=true → dark，perm pill 在位）。
- 长会话滚动：memo 数据即滚动帧成本上界（单帧 0.26ms），60fps 预算内。
- 夹具踩坑记录：public/ 下 html 被 vite 原样直出不走转换，裸模块名（react）
  无法解析——夹具移到项目根后 html-proxy 正常；夹具提交前已删。

## ocr review 复审处置记录

14 条（2 high / 3 medium / 1 无级别 / 8 low）：

- **high 2 修**：`perm_mode` 线字段名错误——BotConfigView 是
  `rename_all = "camelCase"`，线字段为 `permMode`，原写法恒 undefined 恒
  ask（payload 类型声明 + 读取两处同步修正）。
- **medium 4 修**：PermMode 联合类型入 types.ts（消两处内联重复）；pill 文案
  本地化（PERM_LABELS，对齐 EFFORT_LABELS 惯例，完整语义在 title）；
  ToolBadges 包 memo（文本 tick 跳过徽章行）；FileSummary 统一 DOM 形状
  （≤2/>2 同为 w-full 容器）。
- **low 8**：修 3（会话删除钮 aria-label 带会话名、FileSummary 按钮
  aria-expanded、进程折叠 mono 字体），记 5（非空断言/onMouseEnter 重渲染/
  matches 无条件计算等既有微项，随 U4 打磨）。

## spec 起草后自查三条

1. expected_files = staged 全集 8 项（含本 spec）。
2. 预算：M 文件 numstat 合计 +149/-56；无新增文件（本 spec 为新文件计入
   new_files 预算）。
3. findings 留 2 条：会话相对时间（需 src-tauri 扩展 bot_sessions_load 载荷，
   超出 UI 批红线）；+/- 行级 diff 统计（事件面无数据源，不接假数据）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U3b-CHAT-SKIN",
  "family": "ui-redesign",
  "expected_files": [
    "docs/batches/U3b-CHAT-SKIN.spec.md",
    "src/components/ChatPanel.test.tsx",
    "src/components/ChatPanel/ChatPanel.tsx",
    "src/components/ChatPanel/InputArea.tsx",
    "src/components/ChatPanel/MessageList.tsx",
    "src/components/ChatPanel/SessionList.tsx",
    "src/components/ChatPanel/constants.ts",
    "src/components/ChatPanel/types.ts"
  ],
  "max_lines_added": 260,
  "max_lines_removed": 90,
  "max_new_files_lines": 150,
  "findings": [
    { "file": "src/components/ChatPanel/SessionList.tsx", "note": "会话相对时间未实现：需 src-tauri 扩展 bot_sessions_load 载荷（updated_at），超出 UI 批红线，登记待后端批" },
    { "file": "src/components/ChatPanel/MessageList.tsx", "note": "+/- 行级 diff 统计未实现：bot-tool-* 事件面无 diff 数据源，不接假数据；摘要条先用 extractFilePaths 真实文件数" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
npx --no-install vitest --run          # 348 绿
npx --no-install tsc --noEmit -p tsconfig.json
npx --no-install oxlint src            # 0 warn / 0 error
npx --no-install knip --no-progress    # exit 0
```
