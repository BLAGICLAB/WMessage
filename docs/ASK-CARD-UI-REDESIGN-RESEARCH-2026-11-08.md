# ask_user 选择卡 UI 重设计 —— 大厂 UI 调研与方案（已落地）

> 2026-11-08。用户诉求原文：「ask user 工具聊天窗口弹出的选择窗口 ui 要重新设计，调研一下大厂的 ui 设计」。
> 本文先做 UI 层调研与方案对比；**U1–U9 已于同日按推荐拍板并落地**，见 §十 落地结果。
> 机制层调研见 `docs/ASK-USER-CHAT-MULTICHOICE-RESEARCH-2026-10-30.md`（方案 B 已落地，本文是其 UI 后续）。

## 一、结论先行

1. **业界在「选择题 UI」上已高度趋同**，可提炼为一套公认形状：**问题卡内嵌聊天区 +
   选项做成可点行/按钮 + 「其他」自由输入常驻 + 跳过/按假设继续路径显式 + 回答后卡
   片轻量留痕（问题 → 答案）**。没有一家强制结构化。
2. 我们 10-30 落地的卡**骨架方向是对的**（停靠输入区上方 + 点选 + 自由输入 + 按假设
   继续），与 claude.ai 的 docked popup、v0 的 composer 上方面板同流派；差距全在
   **视觉精细度与状态呈现**：选项是裸 chip 堆、选中态与全仓设计系统语义相反、主按钮
   无品牌强调、终态一行 emoji 敷衍、无任何过渡动画。
3. 推荐方案 B（§五）：**选项从 chip 改为单列全宽行 + radio 指示器**（带描述选项的
   业界默认形态，Claude Code 各端 GUI/Zed 同款），选中态回归设计系统「凹陷=选中」
   语义，自由输入改造为「虚拟选项行」（打字即勾上它，Gemini 口径），终态折叠为
   「问题 → 答案」单行留痕（Claude Code 终端/Devin Teams 口径）。**纯前端改造**，
   数据面零改动。
4. 二期的 `description`/`multiSelect` 数据增强**本轮不做**，但 B 方案的行式选项布局
   天然预留了描述副行与复选框位置，届时只改数据不改布局。

## 二、现状盘点

### 2.1 现有实现

- 组件：`src/components/ChatPanel/AskCard.tsx`（170 行）；契约测试
  `AskCard.test.tsx`；通知页姊妹卡 `TaskQuestionCard`
  （`src/components/NotificationsPage/NotificationsPage.tsx` L246 起）。
- 位置：**MessageList 之后、InputArea 之前**的停靠区（ChatPanel.tsx L916-939），
  多卡 `space-y-2` 纵向堆叠；resolved 卡保留到回合收尾 `clearResolved`。
- 结构：问题文本 → why 行（可选）→ assumption 行（可选）→ chip 横排（flex-wrap）→
  恒渲染单行 input（与点选互斥）→ 右对齐两按钮（按假设继续 ghost / 回答并继续 primary）。
- 数据：`askStore.ts` 的 `options: string[]`——无 label/description/multiSelect
  （Rust 侧 `ask_engine.rs` `ask_event_payload` 同构，方案 C 口径二期再做）。
- 回答后整卡替换为只读态：问题降为 t4 弱色 + 一行 `✅ 已回答：{answer}` 纯文本
  （emoji 当图标）；**当时选了哪个选项不留痕**（chips 整体消失）。

### 2.2 可辨识视觉问题清单

| # | 问题 | 位置 | 严重度 |
|---|---|---|---|
| V1 | **选中态语义反转**：`picked` 用 `nm-outset`（抬升）、未选用 `nm-inset`（凹陷），与设计系统「凹陷=选中/按压」（main.css L232-250）及全仓选择器（ArchivePage.tsx:165、TagConsolidateModal.tsx:225、UsageStatsCard.tsx:800、SettingsPage.tsx:293）**相反**；通知页 TaskQuestionCard（NotificationsPage.tsx:289-290）同样反着用，而同文件 L453 用法正确——是笔误不是有意为之 | AskCard.tsx L114-118 | 高（对全仓约定） |
| V2 | 未选中 chip 无 hover 反馈、无 focus-visible 环、无 transition | AskCard.tsx L114-118 | 高 |
| V3 | 选项纯文字胶囊堆叠，无 radio/勾选指示器，选中只靠底色互换 | AskCard.tsx L107-129 | 高 |
| V4 | 数据结构无 description 概念，选项无副行说明 | askStore.ts L27 | 中（二期范围） |
| V5 | 主按钮是中性 `nm-btn`，没用「页面唯一主操作」的 `nm-btn-brand`，与 ghost 按钮视觉权重几乎持平 | AskCard.tsx L32-33, L142-166 | 中 |
| V6 | 终态敷衍：✅ emoji 当图标、所选选项不留痕、无折叠 | AskCard.tsx L37-55, L62-69 | 中 |
| V7 | 无入场/状态过渡（设计系统有现成 `nm-popover` 动画与 `--dur-chip` token 未用） | ChatPanel.tsx L933-938 | 中 |
| V8 | 输入件简陋：无 Enter 提交、无聚焦环强调、点选↔输入互斥清空完全静默 | AskCard.tsx L130-141 | 中 |
| V9 | 卡底与未选 chip 同为 `nm-inset`，凹陷面上放凹陷件，层级弱 | AskCard.tsx L97, L117 | 低 |
| V10 | busy 态只有换文案，无 spinner | AskCard.tsx L164 | 低 |
| V11 | `mcp_elicit` 源卡与普通卡视觉完全无法区分（仅按钮文案分叉） | AskCard.tsx L72-84 | 低 |
| V12 | resolved 卡全尺寸留置到回合收尾，多问时堆叠区迅速变高 | ChatPanel.tsx L916-939 | 中 |

## 三、大厂 UI 调研

> 证据标注：【一手】=官方仓库源码/快照/官方文档直读；【实机抓屏】=第三方项目
> AltanS/collie 逐键驱动真实 Claude Code 存下的终端缓冲区原文（v2.1.201→2.1.314）；
> 【二手】=博客/转述。查不到的如实标「未查到」。

### 3.1 位置光谱（先定「长在哪」）

业界五个流派，从激进到保守：

| 流派 | 产品 | 说明 |
|---|---|---|
| 输入框本体（prompt form 内） | v0（2026-06 改版）「clarifying questions now appear in the prompt form」，含单选/多选/skip/行内自定义 | 最新最激进：问题与待发 prompt 同框 |
| **输入框上方停靠** | **claude.ai 网页（docked question popup）**、v0 权限审批「grouped into one panel above the composer」、**wmessage 现状** | 不打断消息流阅读，紧贴用户手边 |
| 消息流内嵌卡 | Lovable question card、Replit plan 卡、Claude Desktop Cowork「card materializes in the transcript」、Cursor | 成为对话历史的一部分 |
| 消息末尾按钮组 | Bolt.new quick action buttons（response 末尾） | 轻量，接近「建议回复」 |
| 终端全屏接管 | Claude Code CLI / Codex CLI / Gemini CLI（浮层覆盖输入区） | TUI 特有 |

**claude.ai 的一手 issue 描述**（anthropics/claude-ai-mcp#1120）：弹层停靠输入框上方，
含选项 + 「Something else」（自由输入）+「Skip」（跳过，点击开启新回合）。
→ 我们的停靠位与 claude.ai 同款，**位置不动**（改消息流内嵌会与真源化持久化的
「后端独占写权」投影模型相抵触：内嵌卡要么进消息投影要么另建机制，收益不值）。

### 3.2 Claude Code —— AskUserQuestion（定义者，参考价值最大）

**终端参考实现**【实机抓屏 v2.1.314】：

```
────────────────────────────────────────────── ← 全宽分隔线与对话隔开
 ☐ Colour                                      ← header chip（黑字淡紫底 #b1b9f9）
Which colour do you prefer?                    ← 问题粗体白
❯ 1. Red                                       ← 指针紫、选项名染紫、无底色
     A warm colour, common in flowers…         ← description 下一行灰 #999 缩进对齐
  2. Green
     The colour of most leaves…
  3. Blue
  4. Type something.                           ← 自由输入=列表内行内字段
──────────────────────────────────────────────
  5. Chat about this                           ← 逃生行（=放弃作答转聊天）
Enter to select · ↑/↓ to navigate · Esc to cancel
```

- **选项形态：编号列表行 + description 副行**，不是 pill。
- **已选回看**：选项名变绿（#4eba65）+ 末尾 `✔`。
- **推荐项**：label 加 `(Recommended)` 后缀 + **置顶**（changelog 2.0.62 一手；指针
  默认停第 1 行，即推荐项默认高亮）。
- **多选**：`[ ]`/`[✔]` 复选框行，chip 由 `☐` 变 `☒`，列表内粗体 `Submit` 行
  （v2.1.283 抓屏）。
- **多问**：顶部步进条 `← ☐A ☐B ✔ Submit →`，Tab/左右键切换，只显示当前问的选项；
  已答 `☒` 未答 `☐`；2.0.55 起单问答完自动提交不出审阅页。
- **回答后历史形态**（全灰折叠成一条）：
  ```
  ● User answered Claude's questions:
    ⎿  · 问题 → 答案
  ● User declined to answer questions        ← 拒答/取消的终态也留痕
  ```
- **VS Code 扩展（GUI 参照）**：Other 输入框多行（Shift+Enter 换行）；2.1.261 加
  折叠按钮（可不关卡看后面对话）；2.1.286 答完后对话里出「Questions」行逐问列所选项；
  弹层「dims/greys out content above it」（issue #26078 一手）。
- **claude.ai 网页**：docked popup + Something else + Skip；模型先写的正文被吞成
  caption 是官方在追的渲染缺陷（#1120）——**别让提问卡吞掉助手正在说的话**。

### 3.3 OpenAI Codex CLI —— request_user_input（源码级一手）

浮层快照：

```
Question 1/1 (1 unanswered)            ← dim；可带红色倒计时
Choose an option.                      ← 未答时强调色
› 1. Option 1         First choice.    ← 描述与选项名同行第二列对齐（不同于 Claude）
  2. Option 2         Second choice.
  4. None of the above  Optionally, add details in notes (tab)
tab to add notes | enter to submit answer | esc to interrupt
```

- header 字段**不渲染**；无多选；`None of the above` 自动追加，Tab 开 notes 输入框
  （未选任何项时占位 `Select an option to add notes`）。
- **回答后**：`• Questions 1/1 answered` + 逐问 `answer: 值`（值用强调蓝）；
  打断态 `Questions 0/1 answered (interrupted)`。秘密答案显示 `••••••`。
- 带未答题提交有确认页（Proceed / Go back）。

### 3.4 Gemini CLI —— ask_user（npm 包源码还原，一手）

- 绿色圆角框，粗体标题 `Answer Questions`；多问时 tab 条 `← □ Auth │ ✓ Database │
  ≡ Review →`（□ 未答 ✓ 已答 ≡ Review，当前 tab 绿粗下划线）。
- **高亮行整行有底色** + 行首 `●`；描述下一行次要色；已选回看选项名变绿 + ` ✓`。
- 多选：复选框 + 自动追加 `All of the above` + 末尾粗体 `Done` 行。
- **自由输入：choice 题永远附一行行内输入作为最后一个选项**（占位 `Enter a custom
  value`），**在里面打字会自动勾上这一项**——「输入=一个虚拟选项」的鼻祖口径。
- 回答后：`**User answered:**` + `header: 问题` / `→ 答案`。

### 3.5 IDE 系：Cursor / Cline / Roo / Kiro / VS Code / Zed

- **Cursor（AskQuestion）**：聊天流内嵌卡，2–4 选项/题；2.4 起非阻塞（agent 边干边
  等答案）；2.6 加多行回答。选项控件像素形态**未查到**（changelog 无图注）；论坛
  一直在要「Custom Answer / Other」（说明它自由输入弱）。教训：2025-11 有 bug
  「点任一选项整卡自动提交，无法多选」——**点选与提交的边界要清楚**。
- **Roo Code（Suggested Responses，文档最全）**：问题正下方一排可点按钮，**点击即
  作为回复发送**（不落输入框）；Shift+点或铅笔图标把建议文本**复制进输入框**编辑后
  发送（v3.11 一手）；提问期有倒计时条（自动继续），因「无法停止」被报 bug
  （issue #11810）——**倒计时必须配 Stop**。
- **Cline 系**：`follow_up` 建议渲染为问题下方一排可点 suggestion chips，点击即发送。
- **Kiro**：现行版把评审做成**文档批注 + 单流转按钮**（floating Review button +
  「Send N to Kiro」），不是多选按钮组。
- **VS Code MCP elicitation**（den.dev 实装记一手）：原生 UI 风格对齐 **Command
  Palette**（编辑器级弹层非聊天气泡）；三动作 **Accept / Decline / Cancel**；枚举
  渲染形态官方未写明。
- **Zed / ACP（1.17.2 起）**：Agent Panel 内表单——**单选=圆点行（RadioButton）、
  多选=复选框行、整行可点、长选项换行不截断**（PR #63885/#63656 一手）；
  Claude Code 经 ACP 接入时带 Other 文本字段。→ 「整行可点 + radio 圆点」是 GUI
  端共识控件。

### 3.6 Web 系：Lovable / Devin / Replit / ChatGPT

- **Lovable（信息量最大的对照）**：
  - Question card 内嵌消息流，**一卡最多 4 问**，可**前后翻题改答案再统一提交**，
    草稿存浏览器刷新不丢。
  - **Skip / Skip all** 显式跳过；2026-09 专门改口径：「skip ≠ 批准它建议的额外
    工作，只是用合理默认值继续已请求的工作」——**跳过语义必须写清楚**（我们的
    「按假设继续」有 title 提示，但 assumption 行仅在存在时显示）。
  - 设计题选项做成**内容预览**（色板/字体样张/线框）且**预选第一项**。
  - 等待态：后续排队消息在聊天底部灰显。
  - 确认卡自带**状态回写**：点 Build 卡面变「Switched to Build mode」。
- **Devin**：Teams 里 ≤3 个选项的简单问题渲染为 **one-tap reply buttons**，
  「**问题留在聊天里，你的答案显示在其下方**」（回答后形态唯一查到实据的留痕方式）；
  长文本选项降级为**列表 + Submit**（Slack 口径，按长度自适应）；30 秒倒计时自动
  放行（可点 Wait for my approval 拦住）；transcript 把跳过记为 skipped。
- **Replit Agent**：plan 卡（Revise / Cancel / Build 动作）；论坛抱怨**强制单选缺
  Other**——自由输入兜底是刚需。
- **ChatGPT**：agent/deep research 澄清基本纯文本；GPT-6 Intelligent UI 走消息体内
  渲染按钮/表单（控件就地改内容不发新消息）；第三方归纳的 inline 卡规则值得抄：
  **主操作限两个（一主一次）、不做 tab/深层导航、避免内部滚动区**。

### 3.7 共识提炼（八条）

1. **选项控件两形态**：带描述的选项 → **单列全宽列表行**（Claude Code 各端/Zed/
   Codex）；短选项（≤3 个、文本短）→ pill/按钮（Devin Teams、claude.ai popup）。
   GUI 端行式控件的标配是 **radio 圆点/复选框 + 整行可点 + 长文本换行**。
2. **「其他」自由输入必配**，且最佳形态是**融入选项列表的最后一行**（Gemini 的
   「打字自动勾上该行」、Claude Code 的 Type something. 行），而非孤立输入框。
3. **跳过/按假设路径显式给按钮**，且语义要写清（Lovable 的 skip≠批准教训；
   Roo 的倒计时必须可停）。
4. **回答后轻量留痕**：「问题 → 答案」折叠成一行（Claude Code 终端历史、Devin
   Teams、Codex `Questions 1/1 answered`）；所选选项要可见，不是只写「已回答」。
5. **多问用步进/tab**（Claude Code stepper、Gemini tab、Lovable 卡内翻题），
   已答/未答状态在导航器上可见。
6. **主操作限两个**（一主一次，ChatGPT inline 卡规则）；推荐项置顶加标注
   （Claude Code (Recommended)+置顶、Lovable 预选第一项）。
7. **点选=选中≠提交**（除 Roo/Cline 的点击即发送流派外，Claude/Devin/Lovable 都
   有显式提交步），多问/多选时必须如此。
8. **过渡与状态反馈**：弹层不打断阅读（claude.ai 的吞正文是反面教材）、入场动画、
   选中/提交的即时反馈、提交中 spinner。

### 3.8 反模式清单（各家踩过的真坑）

| 坑 | 出处 | 对我们的映射 |
|---|---|---|
| 弹层抢焦点/压暗内容惹抱怨 | claude-code#87807、#26078 | 停靠卡不遮消息流，仅入场动画 |
| 提问卡吞掉助手前置正文 | claude.ai #1120、CLI #67475 | bot 文本照常上屏，卡片追加在其后，不替换 |
| 点任一选项整卡误提交 | Cursor 2025-11 bug | 点选只改选中态，提交走显式按钮 |
| 强制单选缺 Other | Replit 论坛 | 自由输入常驻 |
| 倒计时不可停 | Roo #11810 | 24h+assumption 兜底保持，不引入自动倒计时 UI |
| 超时吞后续请求 | Cursor AskQuestion timeout | 已有 24h+看门狗豁免（hasUnresolvedAsk），不动 |
| 跳过被解读为批准额外工作 | Lovable 2026-09 改口径 | 「按假设继续」的 title 必须写明按哪条假设 |
| 远程端不渲染致静默卡死 | claude-code#73012 | 通知中心双写兜底已有，保留 |

## 四、推荐形态（方案 B 的卡片解剖）

### 4.1 布局（ASCII 示意）

```
┌──────────────────────────────────────────────────┐  ← 卡底 nm-inset，rounded r-md
│ ⌖ 需要你决定 ·  Chat 提问                [问题 1/2]│  ← 头部：来源标识 + 多卡序号
│                                                  │     （elicit 来源显示 MCP · elicitation）
│ 要用哪种部署方式？                    （text-sm t1 semibold）
│ 问这个：两个方案成本差 3 倍            （11px t5） │  ← why 行保留
│                                                  │
│ ◉ 部署到云服务器                                  │  ← 未选：nm-outset + hover 提亮
│     按现有配置直接上线                            │     （有 description 时才有此副行，二期）
│ ○ 本地打包                                        │  ← 选中：nm-inset + 品牌色描边
│                                                  │     + 实心圆点，transition --dur-chip
│ ✎ 其他想法…                          [输入行]     │  ← 自由输入=虚拟选项行：
│                                                  │     聚焦/有文字时圆点亮起
│                          [按假设继续] [回答并继续] │  ← ghost + nm-btn-brand 主钮
│ 按假设继续 = 未回答时按「沿用现状」继续   （title） │
└──────────────────────────────────────────────────┘
```

**回答后折叠**（resolved，同容器收成两行）：

```
│ ✓ 要用哪种部署方式？                              │  ← 问题 t4 + Check 图标（lucide）
│     → 本地打包                                    │  ← 答案 t2，选项名可见
```

assume/expired/dismiss 对应：`→ 已按假设继续（沿用现状）` / `→ 未获回答，已按 AI
假设继续` / `→ 已忽略`，图标区分（Check / Clock / X）。

### 4.2 关键决策与依据

| 决策 | 依据 |
|---|---|
| 位置保持「输入区上方停靠」 | 与 claude.ai docked popup 同款；消息流内嵌与真源化投影模型冲突；不动后端 |
| 选项改单列全宽行 + radio 圆点，整行可点 | 40 字长选项 chip 会折行破碎；Zed/Claude GUI 的行式是 GUI 共识；天然预留 description 副行与 multiSelect 复选框位 |
| 选中态回归「凹陷=选中」（nm-inset + 品牌色描边 + 实心圆点） | 修 V1 反转；对齐 main.css L232 语义与全仓 chip 约定 |
| 自由输入改为列表内虚拟选项行（铅笔图标 + 占位「其他想法…」），聚焦/有文字=亮起 | Gemini「打字自动勾上」口径；修 V8 静默互斥——互斥变可视 |
| 点选只改选中态，提交走主按钮；Enter 在输入行内可直接提交 | Cursor 误提交教训；Claude/Lovable 两步流派 |
| 主按钮 `nm-btn-brand`，次按钮 ghost；全卡主操作就两个 | 修 V5；ChatGPT inline 卡「一主一次」规则 |
| resolved 折叠为「问题 → 答案」单行对 + lucide 图标，所选选项可见 | Claude Code 终端历史 / Devin Teams / Codex `answered` 口径；修 V6/V12（折叠后多问堆叠不再爆高） |
| 入场用 `nm-popover` 动画，chip/行过渡用 `transition-colors duration-[var(--dur-chip)]`，提交中 Loader2 spinner | 修 V7/V10；全部用现有 token，不引新依赖 |
| 头部加来源标识行；`mcp_elicit` 显示「MCP · elicitation」 | 修 V11；ACP 规范要求「clearly identify the requesting agent」 |
| 多卡时头部显示「问题 n/N」徽标 | Lovable 多问导航的轻量替身；不做 tab（主操作限两个原则） |
| 通知页 TaskQuestionCard 同步对齐（至少修 V1 反转） | 两处一致地错，只改一处会制造分叉 |

### 4.3 不改的东西

- 后端零改动：payload 结构、事件对、ask_respond/AskReply、24h+assumption 兜底、
  预算闸、通知双写全不动。
- 数据面零改动：`options: string[]` 保持；description/multiSelect 是二期数据增强
  （届时 schema/registry/ask_engine/前端四处一起动，见 10-30 文档方案 C）。
- 互斥语义不变（输入优先），只是把互斥从静默变为可视。
- 「按假设继续」的位置与行为不变（title 已含 assumption 文案），仅视觉升为 ghost
  按钮标准态。

## 五、方案选项

### 方案 A —— 最小修（只修 bug 不重塑）

修 V1（选中态反转）、V2（hover）、V5（品牌主钮）、V6（终态留痕）、V10（spinner）；
chip 布局与卡片结构保持。约半天。
缺点：chip 承载 40 字选项依然破碎（V3），无描述位、无虚拟选项行，二期 description
落地时还得再改一遍布局——一次返工。

### 方案 B —— 行式选项重塑（推荐）

§四 全量：行式选项 + radio、虚拟选项行、品牌主钮、终态折叠、入场动画、来源标识、
多卡徽标；顺带修 V1/V2/V9。纯前端 `AskCard.tsx` 重写 + 契约测试更新 + 通知页姊妹卡
对齐。约一天。
优点：一步到位对齐业界 GUI 共识，布局为二期的 description/multiSelect 预留到位。

### 方案 C —— B + 数据增强一次做

B 之上加 `options → {label, description?}[]`、`multiSelect`、（可选）单次多问。
联动 schema/registry/ask_engine/askStore/事件契约/通知中心/测试，规模翻倍；且 10-30
文档 P5 已拍板「二期」——小模型栈下 schema 越复杂工具调用越易碎。**不推荐本轮做**。

## 六、待拍板清单

| # | 拍板项 | 选项 | 推荐 |
|---|---|---|---|
| U1 | 方案选型 | ① A 最小修 ② B 行式重塑 ③ C 含数据增强 | ② B |
| U2 | 选项控件 | ① 单列全宽行 + radio（Claude GUI/Zed 口径） ② 保持 chip 横排只修语义 | ① |
| U3 | 回答后形态 | ① 折叠「问题 → 答案」单行对（Claude/Devin 口径） ② 保持全卡 + 仅高亮所选 ③ 现状一行小字 | ① |
| U4 | 主按钮 | ① `nm-btn-brand` 实底品牌色 ② 保持中性 nm-btn | ① |
| U5 | 自由输入 | ① 虚拟选项行（Gemini 口径，打字亮起） ② 保持孤立输入框只加 Enter 提交 | ① |
| U6 | 来源标识 | ① 头部标识行（普通=「需要你决定」，elicit=「MCP · elicitation」） ② 不区分 | ① |
| U7 | 多卡徽标「问题 n/N」 | ① 加 ② 不加 | ① |
| U8 | 通知页 TaskQuestionCard | ① 同步对齐（至少修 V1） ② 只改聊天卡 | ① |
| U9 | 入场动画与过渡 | ① 用现有 nm-popover/--dur-chip token ② 无动画 | ① |

## 七、不做清单

- **消息流内嵌卡**：与真源化「后端独占写权」投影模型冲突，claude.ai 也是停靠派；
  除非将来把 ask 卡纳入消息投影，否则不动位置。
- **单次多问 / 步进条 / tab 导航**：引擎一次一问，Lovable 式卡内翻题是给「一次打包
  4 问」的形态服务的，我们没有这个数据面。
- **倒计时自动继续 UI**：Roo 因不可停被报 bug；我们有 24h+assumption 兜底，够了。
- **内容预览型选项**（Lovable 色板/线框）：需要 preview 数据面，属 Claude Code
  preview 同级重武器，10-30 文档已列不做。
- **透传 dim 上方内容**（VS Code Claude Code 扩展行为）：停靠卡不遮消息流，不需要。

## 八、实施备注（拍板后用）

- 改动面：`src/components/ChatPanel/AskCard.tsx`（重写）+ `AskCard.test.tsx`
  （契约同步：输入优先/aria/回调参数不变）+ `src/components/NotificationsPage/`
  TaskQuestionCard（U8 若过）+ 可能抽一两个共享类。
- 设计系统锚点：`nm-inset`=选中/按压、`nm-outset`=未选中（main.css L232-254）、
  `nm-btn-brand`（L1223-1256）、`nm-popover` 入场（L261-277）、`--dur-chip`。
  **顺手纠正两处反转**（AskCard + TaskQuestionCard）。
- 图标走 lucide-react（仓库既有，61 个文件在用）：Check / Clock / X / Pencil（或
  PenLine）/ MessageCircleQuestion（来源标识）；提交中 spinner 沿用仓库惯例
  `<Loader2 className="wmsg-activity" />`（TagConsolidateModal.tsx:173），不用
  `animate-spin`。
- 契约红线：`useSyncExternalStore` 空态必须共享模块级实例（askStore 踩过 76 例
  连坐坑，有契约测试锁死）；`respondTaskQuestion`/`elicit_respond` 调用参数不变；
  `hasUnresolvedAsk` 看门狗豁免逻辑不碰。
- 回放链路不碰：`bot-ask`/`bot-ask-resolved` 事件与 seq 去重原样；UI 只换皮。
- 验收锚点建议：① 未选行 hover 有反馈 ② 选中行凹陷+品牌描边+实心点 ③ 输入文字
  时虚拟行亮起且所选选项同步取消（可视互斥） ④ Enter 提交 ⑤ resolved 折叠含
  所选选项文本 ⑥ 提交中 spinner ⑦ elicit 卡来源标识可辨 ⑧ 多卡徽标正确。

## 九、参考链接

- Claude Code：[CHANGELOG](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md) ·
  [Agent SDK user-input](https://code.claude.com/docs/en/agent-sdk/user-input) ·
  [collie 实机抓屏（WIZARD_NOTES/ASK_FIELD_NOTES/fixtures）](https://github.com/AltanS/collie) ·
  [#26078 VS Code 弹层](https://github.com/anthropics/claude-code/issues/26078) ·
  [#12609 GUI 提案](https://github.com/anthropics/claude-code/issues/12609) ·
  [#87807 焦点](https://github.com/anthropics/claude-code/issues/87807)
- claude.ai：[claude-ai-mcp#1120 docked popup](https://github.com/anthropics/claude-ai-mcp/issues/1120) ·
  [#70648 移动端 bottom sheet](https://github.com/anthropics/claude-code/issues/70648) ·
  [#73012 web viewer 不渲染](https://github.com/anthropics/claude-code/issues/73012)
- Codex：[request_user_input 源码](https://github.com/openai/codex/tree/main/codex-rs/tui/src/bottom_pane/request_user_input) ·
  [history_cell](https://github.com/openai/codex/blob/main/codex-rs/tui/src/history_cell/request_user_input.rs)
- Gemini CLI：[ask-user 文档](https://geminicli.com/docs/tools/ask-user) ·
  [AskUserDialog.tsx](https://github.com/google-gemini/gemini-cli/blob/main/packages/cli/src/ui/components/AskUserDialog.tsx)
- Cursor：[changelog 2-1](https://cursor.com/changelog/2-1) · [2-4](https://cursor.com/changelog/2-4) ·
  [custom answer 请求](https://forum.cursor.com/t/allow-custom-user-options-in-plan-mode-follow-up-questions/148395)
- Roo/Cline 系：[Suggested Responses](https://roocodeinc.github.io/Roo-Code/features/suggested-responses) ·
  [v3.11 可编辑建议](https://roocodeinc.github.io/Roo-Code/update-notes/v3.11) ·
  [#11810 倒计时不可停](https://github.com/RooCodeInc/Roo-Code/issues/11810) ·
  [Kilo follow-up 文档](https://kilo.ai/docs/code-with-ai/agents/chat-interface)
- Zed/ACP：[releases](https://zed.dev/releases/stable) · [PR #63885 RadioButton](https://github.com/zed-industries/zed/pull/63885) ·
  [PR #63656 换行修复](https://github.com/zed-industries/zed/pull/63656) · [ACP elicitation](https://agentclientprotocol.com/protocol/v2/elicitation)
- Web 系：[Lovable chat 文档](https://docs.lovable.dev/features/projects/chat) ·
  [Lovable changelog（skip 口径）](https://docs.lovable.dev/changelog) ·
  [v0 changelog](https://v0.app/changelog) ·
  [Devin release notes 2026](https://docs.devin.ai/release-notes/2026) ·
  [Replit plan mode](https://docs.replit.com/features/agent/plan-mode) ·
  [ChatGPT agent 帮助](https://help.openai.com/en/articles/11752874-chatgpt-agent) ·
  [GPT-6 Intelligent UI](https://openai.com/index/gpt-6-for-everyone) ·
  [sunpeak inline 卡规则](https://sunpeak.ai/blogs/chatgpt-app-display-mode-reference)
- VS Code elicitation：[den.dev 实装记](https://den.dev/blog/vscode-mcp-elicitations-stop-guessing/) ·
  [VS Code 1.102](https://code.visualstudio.com/updates/v1_102)

## 十、落地结果（同日实施）

U1–U9 按推荐执行，纯前端改造，后端与数据面零改动；落地提交即调研文档之后的下一笔。
前端 1226 例全绿（`AskCard.test.tsx` 8→14 例、`NotificationsPage.test.tsx` +1 例）；
tsc 干净、oxlint 0 错误（改动文件 0 警告）；行数 / 验收对拍 / 文档对拍 / 桥接门禁 35 例绿。

**落地形态**：

- 共用作答区 `src/components/AskAnswerFields.tsx`（`useAskAnswer` 状态钩子 +
  `AskAnswerFields` 组件）：选项为 `role=radio` 单选行（radiogroup，整行可点、圆点指示器、
  长文本换行）；末行「其他回答」输入行（铅笔图标，有字即亮起并取消点选，Enter 提交、
  输入法组词中不触发，聚焦显示焦点环）。选中 `nm-inset` + 品牌色描边、未选
  `nm-outset`（自带 hover）——设计系统「凹陷=选中」语义回正。
- 聊天提问卡 `AskCard.tsx`：卡头来源标识（问号图标「需要你决定」/ 插头图标「MCP 输入
  请求」）+ 多卡时「第 n 问」；问题加粗并保留换行；主按钮 `nm-btn-brand` + 提交中
  `Loader2` 转圈；入场复用 `popover-in` 关键帧（从底部长出）；终态收成两行「图标 +
  问题（最多两行，悬停看全文）/ → 答案」，作答绿色对勾、按假设 SkipForward、超时 Clock。
- 通知页问题卡 `TaskQuestionCard` 改用同一作答区（同款选中态反转随之修掉）。

**验收锚点对照**（§八）：①②③④⑤⑦⑧ 由 `AskCard.test.tsx` 钉住（选中态类名断言、
输入取消点选、Enter 提交、组词中 Enter 不提交、终态文本、MCP 来源标识、多卡序号）；
⑥ 转圈与 hover / 动画属视觉项，用无头 Chrome 渲染编译后的 Tailwind + main.css 亮暗两套
核对过，**未在 Tauri 实机（WKWebView）里看过**。

**偏离登记**（对照 §四 / §六）：

1. U7「问题 n/N」改为「第 n 问」：模型在预算内还可能再问，总数 N 事先不可知，写成
   n/N 会暗示「这是最后一问」。
2. U6 卡头文案：§4.1 草图的「需要你决定 · Chat 提问」只留「需要你决定」（AskCard 聊天
   与任务执行会话共用，「Chat 提问」在执行会话里是错的）；MCP 来源写「MCP 输入请求」而非
   「MCP · elicitation」，与现有文案「应答 MCP 输入请求」「拒绝该输入请求」一致。
3. U8 通知页主按钮保持中性 `nm-btn`、不加转圈：该页每张卡都有主操作，`nm-btn-brand`
   按设计系统约定留给页面唯一主操作；两张卡只共用作答区。
4. 计划外修正（MCP 输入请求的终态语义）：原卡把拒绝 / 超时显示成「已按假设继续」、
   作答显示后端占位串「[MCP 应答，值不落日志]」；elicitation 没有假设兜底（未答即
   Decline），改为「已拒绝该输入请求」「未获应答，已按拒绝处理」「已提交（应答值不回显）」。
5. 计划外修正（未定义的颜色变量）：旧卡 `border-[var(--border)]` 引用的 `--border`
   全仓库没有定义（计算期无效，边框色退回 currentColor——旧卡边框其实是文字色）；新卡改用
   `nm-inset` 自带的 `--edge` 边框。同一未定义变量还在 `Toggle.tsx:48`（未开启态底色）与
   `InputArea.tsx:276`（附件预览边框）使用，不在本次范围，登记不修。
6. 问题文本加 `whitespace-pre-line`：MCP 问题里后端拼的「\n（字段：…；类型：…）」原先
   被折成一行。
7. 聊天视图左栏「新对话」也是 `nm-btn-brand`，提问卡挂起期间同屏两枚品牌色按钮；取提问卡
   强调（它是当下唯一要推进的动作），实机若嫌抢眼，退回中性只改 AskCard 一处类名。

**实机待验**：亮暗主题观感、hover 与焦点环、入场动画、中文输入法下的 Enter（判据与聊天
输入框同为 `nativeEvent.isComposing`）、一轮多问时停靠区高度。
