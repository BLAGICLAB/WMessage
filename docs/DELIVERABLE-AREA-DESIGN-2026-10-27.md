# 交付物区改造方案（2026-10-27）

> 本文是**改造方案**，不含代码改动。§十 的口径已于 2026-10-27 全部拍板，方案定稿。
> 起因：实机一轮 PPT 生成任务，机器人回复里给了交付物路径，但消息下方的「附件」
> 区显示的是 11 个执行过程文件（`.py` 脚本 + 中间稿），**最终的 `.pptx` 不在其中**，
> 且正文里的路径链接点击打不开。

---

## 一、症状与根因（现状账本）

### 1.1 实测复现

在仓库自身的 vitest 里跑真实路径（临时用例，已删）：

```
extractFilePaths("D:\Program Files\wmessage-portable-2026-10-27\AI_Gen_Files\绩效工资分配实施办法宣贯（职工代表大会）.pptx")
→ ["D:\\Program"]
```

同一条 `.md`（含 `（逐页）`）→ 也是 `["D:\\Program"]`。**整条路径被砍到第一个空格**，
只剩 9 个字符，点开必然报「路径不存在」。

### 1.2 四条根因

| # | 根因 | 位置 | 后果 |
|---|---|---|---|
| R1 | 路径正则把全角标点当硬边界（`（）【】，。；、！？` 全在排除集里） | `src/lib/openTarget.ts:28-30` | 中文文件名里的括号让带空格分支匹配失败 → 落到无空格分支 → 截成 `D:\Program` |
| R2 | `run_python` 不带落盘凭证，harvest 搬进 `AI_Gen_Files` 的产物不进 `file_changes` | `bot/tools/doc.rs:824`、`py/harvest.rs:11` | 脚本产出的 `.pptx` 对文件变更通道完全隐形 |
| R3 | 文件摘要区是**二选一**：`structured.length > 0 ? structured : fps` | `MessageList.tsx:515-516` | 有 11 条写盘凭证 → 正文里点名的 pptx 被整份丢弃，连正则兜底都不走 |
| R4 | 完成态正文只链接化「行内代码」和「markdown 链接」，裸路径是纯文本 | `MarkdownText.tsx:33-121` | 交付物那段裸路径不可点（本次**不修**，见 §九） |

### 1.3 同一个气泡三种显示（口径不稳）

`trace_sink.rs:7-9`：**trace 只在 `run_task_in_chat_with`（任务执行流程）里建**，主聊天没有 trace；
`bot/dispatch.rs:911` 的 `bot-file-changed` 发射是 `if let Some(trace_id) = trace_id_for_session(...)`，
没 trace 就不发。又因 `bot_messages` 表无文件列（`db/mod.rs:200-210`），刷新后 `m.fileChanges` 必为 undefined。

于是同一条回复的附件区会显示三种东西：

| 场景 | structured | 实际显示 |
|---|---|---|
| 执行流程 · 刚回复完（实时） | 11 条 | **11 个 `.py` 脚本** |
| 执行流程 · 刷新后 | 空 | 正文正则结果（修好 R1 后 = pptx + md） |
| 主聊天 | 空 | 正文正则结果 |

**结论：附件区不是"不准"，是"没有确定语义"**——它取决于有没有 trace、有没有刷新。

### 1.4 语义错位（本次改造的真正靶子）

「改动文件」在本仓库有**三处副本**，其中两处是同一份数据：

| 位置 | 数据源 | 口径 |
|---|---|---|
| 气泡下方 chips | `file_changes`（有）/ 正文正则（无） | 本轮 |
| 右侧栏「本会话文件改动」 | `file_changes` 表 | 会话累计 |
| 计划浮层「环境信息」 | `fileChanges`（同一份） | 会话累计 |

而**「交付物」这个概念在本仓库是空的**：`prompts/system.rs:61` 已经把契约写死——
「给用户的汇报里只列最终交付物：中间草稿、脚本、临时文件一律不列」——模型的行为是对的
（那轮正文只点名了 pptx + 设计说明 md，10 个 `.py` 一个没列），是 UI 把「正文交付物」
和「过程文件」混成了一锅。

---

## 二、业界口径（2026-10 调研摘要）

| 产品 | 交付物在哪 | 正文里怎么表示 | 「改了什么」在哪 |
|---|---|---|---|
| **ChatGPT** | 左侧 **Library** 独立标签页。上传和**创建**的文件自动入库；可按「上传/生成」、文件类型筛选；多选批量下载；**删掉聊天不删 Library 文件** | 正文只叙述 + 路径 | 无 diff 面 |
| **Claude** | 侧栏 **Artifacts 标签页**（"Everything you make is saved to the Artifacts tab"）；产物在**右侧面板**渲染，可边看边聊；同一产物多版本（artifact id + version_id） | 正文引用产物 | 版本历史在 artifact 面板内 |
| **Gemini** | 右侧 **Canvas 面板**（文档/幻灯片/代码）；顶部「分享和导出」「上一版本/下一版本」；可导出到 Google Docs / Colab | 正文引用 Canvas | Canvas 面板内「代码 / 显示近期的更改」 |
| **Cursor** | 气泡末尾**一张卡**（Canvas），点开进侧面板 | 气泡末一张卡 | 气泡底部 **review pill 只显示改动数（+/-）**，点开进右侧 **Changes tab**（文件树 + diff） |

出处：OpenAI 帮助中心 `Using Library to manage files in ChatGPT`（article 20001052）；
Anthropic 帮助中心 `What are artifacts and how do I use them?`（article 17153992）；
Google 帮助中心 `利用 Canvas 创建文档、应用及其他内容`（gemini/answer/16047321）；
Cursor Agents Window 口径（review pill + Changes tab）。

**四条共同规律：**

1. **交付物是独立实体，有自己的容器**——Library / Artifacts / Canvas / Changes tab。
   没有一家是把一堆文件名塞进气泡当「附件列表」。
2. **气泡只放锚点，实体去面板。** Cursor 气泡底部的 pill **只显示改动数量**，文件清单在右侧 tab。
3. **「改了什么」与「交付了什么」从不混在一个列表。** Cursor 拆成 Changes tab + Canvas 卡；
   ChatGPT 在 Library 按「上传/生成」分类；Claude 的 artifacts 与 transcript（推理、工具调用、决策）分开。
4. **交付物可脱离消息存活。** ChatGPT 删聊天不删 Library 文件；Claude 的 artifact 有独立 id 和版本。

> 映射回本仓库：`AI_Gen_Files`（`db/paths.rs:31`）就是现成的 Library——启动预建、跨会话、
> 同名 `(n)` 不覆盖、按扩展名天然分型。差的只是浏览面。
> Cursor 的 review pill ≈ 我们的「执行轨迹」按钮；Changes tab ≈ 我们的右侧栏文件面板 + 轨迹面板。

---

## 三、目标与边界

### 3.1 本次要做

| # | 目标 | 落点 |
|---|---|---|
| G1 | 消息下方那个位置改成**交付物区**，只放最终产物，不放执行过程文件 | `MessageList.tsx` |
| G2 | 右侧栏「文件」页加**打开产物文件夹**按钮 + 两个子 tab：**改动（轨迹）** / **交付物（产物）** | `ContextRail.tsx` |
| G3 | 交付物清单口径 = **正文自报 ∪ 新产生兜底**，会话级与消息级同源 | 新前端纯函数模块 |
| G4 | 点交付物 → 跳右侧栏预览；文本直接预览；二进制给「用默认程序打开」；**不再二次跳转** | 复用既有 `openFilePreview` + `FilePreview` |
| G5 | 交付物路径必须准确：不截断、不挂打不开的条目 | 正则修正 + 存在性校验 |
| G6 | 删掉被这次改造孤儿化的旧设计 | 见 §七 |
| G7 | 交付物区为空但本轮确有写盘时，给一行提示 + 跳右侧栏「改动」tab | `MessageList.tsx`（Q3 已拍板） |

### 3.2 本次不做（明确）

- 不动 `MarkdownText` / `RichText` 的正文渲染（裸路径仍不可点）
- 不做跨会话的 Library 页（产物库浏览面）
- 不做卡片式 artifact 面板 / 产物预览编辑器
- 不扩展 `link_file_to_task` 到普通对话（模型显式登记 final 的口径留到下一轮）
- 不改提示词契约（`system.rs:61`「只列最终交付物」继续有效）

---

## 四、交互设计

### 4.1 三处 UI 的职责划分

| 位置 | 装什么 | 口径 | 备注 |
|---|---|---|---|
| 气泡下方**交付物区** | **这一轮**的最终产物 | 单条消息 | 替换现有 chips 位置 |
| 右侧栏「文件」页 → **改动 tab** | 本会话动过的文件（±行/diff） | 会话累计 | 现有清单**一行不改** |
| 右侧栏「文件」页 → **交付物 tab** | **这个对话**的产物 | 会话累计 | 新增 |
| 执行轨迹面板 | 该轮工具调用 + 文件修改 + 回滚 | 单轮 | **不改**，兜住改动可达性 |
| 计划浮层「环境信息」 | 降级为一行汇总 + 跳右侧栏改动 tab | 会话 | 见 §七 清理项 C3 |

### 4.2 右侧栏「文件」页布局

```
┌─ 文件 ────────────────────────────┐
│ 📂 打开产物文件夹  AI_Gen_Files     │  ← 新按钮（链接按钮 + 路径 tooltip）
│ ┌────────┬────────┐                │
│ │ 改动 4 │ 交付物 2│               │  ← 子 tab
│ └────────┴────────┘                │
│ ▸ 绩效宣贯PPT生成脚本.py    +462 −0 │  ← 改动 tab（现有清单）
│ ▸ 宣贯PPT设计说明（逐页）.md +78 −0 │
│                                   │
│ （交付物 tab）                     │
│ ▸ 绩效工资分配实施办法宣贯（…）.pptx│
│ ▸ 宣贯PPT设计说明（逐页）.md         │
└───────────────────────────────────┘
```

- 「打开产物文件夹」用**链接按钮**样式（不是主按钮），点击在系统文件管理器里打开 `AI_Gen_Files`
- 子 tab 用与右侧栏一级 tab（`TABS`，`ContextRail.tsx:114-117`）一致的下划线式样
- 预览态（`previewPath` 非空）**整栏替换**成预览内容，子 tab 记住上次选择，返回时回对应清单

### 4.3 点击链路（一次到位，不二次跳转）

```
气泡交付物 chip
  └─> onPreviewFile(path)                     // MessageList → ChatPanel
        └─> openFilePreview(path)             // ChatPanel.tsx:177-182（已存在）
              ├─ setCtxOpen(true)             // 开侧栏
              ├─ setCtxTab("files")           // 切文件页
              ├─ setFileSubTab("deliverables")// 切交付物子 tab
              └─ setPreviewPath(path) + setPreviewMode("content")

右侧栏交付物条目
  └─> setPreviewPath(path) + setPreviewMode("content")   // 同一条链路，不重拉清单

预览态渲染（ContextRail.tsx:357 分支）
  ├─ previewMode === "content" → FilePreview
  │    ├─ 文本     → 直接渲染（.md 走 MarkdownText，其余 <pre>）
  │    └─ 二进制   → unsupported 态卡片 + 主行动「用系统默认程序打开」（已实现）
  └─ previewMode === "diff"    → FileDiffView（现有行为）
```

**「不要再次跳转」的落点：**

- 「用系统默认程序打开」按钮走 `openTarget(path)` → Rust `open_file_path`，
  **只开系统程序，不改 rail 状态、不切 tab、不重设 previewPath**（现状已满足，验收即可）
- 点交付物**只跳一次**：侧栏开 + 切页 + 设预览路径是**一次**状态更新，不是「先跳列表再跳预览」

### 4.4 一个必须解决的细节：预览默认视图由来源决定

现状 `ContextRail.tsx:358` 用 `hit = fileItems.find(f => f.path === previewPath)` 判断：
命中改动清单 → 显示 **FileDiffView（看改动）**，没命中 → 显示全文。

交付物 `.md` 恰好**也在**改动清单里（它是 `write_file` 写的）→ 从交付物区点进去会看到
diff 而不是内容，**与「文本直接预览」的要求冲突**。

→ 引入 `previewMode: "content" | "diff"`，由**点击来源**决定，不由「是否在改动清单里」决定。
改动 tab 点进来 = `diff`；交付物 tab / 气泡交付物区点进来 = `content`。

### 4.5 交付物区的空态（Q3 已拍板：提示一行 + 可跳转）

交付物区为空有两种情况，口径不同：

| 情况 | 判据 | 表现 |
|---|---|---|
| **这轮确实没产出文件** | declared 与 produced 都空 | 什么都不显示（气泡区不留空行） |
| **这轮写了文件但模型没在正文点名** | produced 非空、declared 空 | 一行提示：「本轮有 N 个产物未在回复中列出」→ 可点，跳右侧栏「改动」tab |

- 「N 个产物」= produced 的条数（含被形态过滤掉的脚本）——用户要的是**总数**，
  不是"交付物形态筛选后剩几个"：过滤是展示口径，提示里的数字要如实反映"有东西没给你"
- 跳转落点：右侧栏文件页 → **改动** tab（不是交付物 tab——那里是空的）
- 主聊天里 produced 恒空（无 trace，见 §5.4）→ 那种情况下没有提示，只有真正的空态

---

## 五、数据模型：交付物清单怎么算

### 5.1 条目形状

```ts
type Deliverable = {
  path: string;        // 绝对路径（模型给的原样，用于打开）
  name: string;        // basename（显示）
  source: "declared" | "produced";  // 正文自报 / 本轮新产生
  kind: "text" | "binary" | "dir";  // 决定预览走哪一支
  exists: boolean;     // 存在性校验结果
};
```

### 5.2 两个来源

| 来源 | 取法 | 形态过滤 | 说明 |
|---|---|---|---|
| **declared（正文自报）** | 从消息正文抽绝对路径 | **不过滤** | 模型说它是交付物就信——与 `system.rs:61` 的契约一致。**Q1 已拍板：配套说明文档也算交付物**（模型点名了就挂上，不做「主/辅」分档） |
| **produced（新产生）** | `file_changes` 路径 ∩ 交付物形态白名单 | **过滤** | 脚本/中间产物形态不进交付物区 |

- **形态白名单复用 `openTarget.ts` 的 `FILE_EXTS`**：含 `pptx/docx/xlsx/pdf/md/png/zip/…`，
  **不含 `.py/.js/.sh/.bat`** → 那 10 个生成脚本自然被排除，不另发明一套规则
- **去重**：路径小写归一后比对（Windows 路径大小写不敏感，与 `taskFiles.ts` 同口径）
- **排序**：declared 按正文出现顺序在前；produced 追加在后（时间序）

### 5.3 存在性校验（新增命令）

```
bot_probe_paths(paths: Vec<String>) -> Vec<PathProbe>
  PathProbe { path, exists, is_dir, size }
```

- **存在的才进交付物区**：挂一个打不开的条目正是最初那个诉求，不能只靠点击时报错
- 同步 syscall（`exists` / `canonicalize` / `metadata`）**必须包 `spawn_blocking`**——
  仓库纪律明确（`bot/tools/doc.rs:14-18`）且有源码守卫断言：跑在 async runtime 线程上会卡住整个 app 的事件循环
- 复用现成探测逻辑：`bot/tools/doc.rs:25` 的 `probe_paths_sync`（当前是 `pub(super)`，需提升可见性）

### 5.4 两个作用域怎么拿数据

| 作用域 | declared 来源 | produced 来源 | 计算时机 |
|---|---|---|---|
| **消息级**（气泡交付物区） | 该条消息正文 | `m.fileChanges`（本轮） | 渲染时 `useMemo` |
| **会话级**（右侧栏交付物 tab） | 全会话助手消息正文 | 会话 `file_changes` | 切到该子 tab / `sessionId` 变 / 文件变更 tick 变 时算一次并缓存 |

- 会话级需要「全会话正文路径」→ `ChatPanel` 本来就持有 `messages`，`ContextRail` 只在侧栏打开时挂载
  （`ChatPanel.tsx:832-843`），per-token 重渲染是既有现状，不新增负担
- **计算必须 gate 在「交付物子 tab 打开时」**，否则每来一个 token 就重扫一遍全会话正文
- `m.fileChanges` 只在有 trace 的会话里有值（`useChatStream.ts:337-373` 累积）→ **主聊天里
  produced 那半边天然为空，只有 declared**。这是诚实的缺口：主聊天无 trace，脚本产物拿不到凭证

---

## 六、逐项改造清单

### 6.1 后端

| # | 文件 | 改动 |
|---|---|---|
| B1 | `src-tauri/src/bot_skills/files.rs` | 新增 `bot_probe_paths` 命令：批量探测路径 → `exists/is_dir/size`；同步 syscall 包 `spawn_blocking`；复用 `probe_paths_sync`（需从 `bot/tools/doc.rs` 提升到 `pub(crate)`，或在新模块重写一份等价实现） |
| B2 | `src-tauri/src/bot_skills/files.rs` | 新增 `bot_open_gen_dir` 命令：解析 `db::gen_dir(app)`（`db/paths.rs:31`）→ 调 `open_file_path` 内部逻辑打开文件夹 → 返回解析后的路径串（给前端做 tooltip 显示） |
| B3 | `src-tauri/src/lib.rs` | 注册 B1 / B2 两个命令 |
| B4 | `src-tauri/src/bot/tools/doc.rs:824`、`py/harvest.rs:11`、`py/runtime.rs:1135` | **可选批**（P2）：`run_python` 回传 harvest 搬运清单 → 逐个包 `build_binary_file_receipt` → 脚本产物进 `file_changes`。收益：执行流程里右侧栏「改动」tab 也能看见 pptx。**注意**：只在有 trace 的会话里生效（`dispatch.rs:911`） |

### 6.2 前端

| # | 文件 | 改动 |
|---|---|---|
| F1 | `src/lib/openTarget.ts:28-30` | **R1 修正**：带扩展名分支的路径字符集放开 `（）【】`（文件名里的括号）；**无扩展名分支保持收紧**（它是贪婪匹配，放开会把整句吞进路径）；`,。；、` 仍作边界 |
| F2 | `src/lib/deliverables.ts`（新） | 纯函数模块（可单测）：`extractDeliverablePaths(content)`、`pickDeliverableShaped(paths)`、`mergeDeliverables(declared, produced)`、`normalizePathKey(path)` |
| F3 | `src/lib/deliverables.ts` | 形态白名单直接复用 `openTarget.ts` 导出的 `FILE_EXTS`（不另立规则） |
| F4 | `src/components/ChatPanel/MessageList.tsx:459-540` | `FileSummary` → 改名「交付物区」：数据源 = `extractDeliverablePaths(m.content)` ∪ `pickDeliverableShaped(m.fileChanges?.paths)`；chip 去掉 ±行数，改「图标 + 文件名 + 全路径 tooltip」；点击 → `onPreviewFile`（跳右侧栏预览）；空态两态（§4.5） |
| F5 | `src/components/ChatPanel/MessageList.tsx:112` | `showActions` 条件里的 `fps.length > 0` 改成「有交付物」口径 |
| F6 | `src/components/ChatPanel/ContextRail.tsx:353-463` | 文件页重构：顶部「打开产物文件夹」按钮 + 子 tab（改动 / 交付物）；`改动` tab = 现有 `fileItems` 清单（不动）；`交付物` tab = 会话级清单；预览态按 `previewMode` 分流（§4.4） |
| F7 | `src/components/ChatPanel/ContextRail.tsx:104-112` | props 增补：`messages`（会话级 declared 来源）、`fileSubTab` / `onFileSubTabChange`、`previewMode` / `onPreviewModeChange` |
| F8 | `src/components/ChatPanel/ChatPanel.tsx:176-182` | `openFilePreview` 增补：设 `previewMode="content"` + 切 `fileSubTab="deliverables"`；`messages` 传给 `ContextRail` |
| F9 | `src/components/ChatPanel/FilePreview.tsx` | **大概率不改**：unsupported 态 + 主行动「用系统默认程序打开」（`:159-180`）与标题栏 ExternalLink 图标（`:136-144`）已满足 §4.3；只在验收发现「±行数在交付物预览里碍眼」时再摘（`:127-133`） |
| F10 | `src/components/ChatPanel/PlanPanel.tsx:225-268` | 「环境信息」段降级为一行汇总 + 跳转右侧栏「改动」tab（清单已在右侧栏，不重复渲染） |

### 6.3 文档

| # | 文件 | 改动 |
|---|---|---|
| D1 | `SPEC.md` | 补产品口径：交付物区的定义（最终产物）、右侧栏文件页双子 tab、附件位语义变更；**`SPEC.md:135` 计划浮层「环境信息」那一条同步改**（C3/Q4 降级后不再是三段之一）（随提交更新） |
| D2 | `DEVLOG.md` | 记「为什么把附件位换成交付物区」——R3 的二选一口径 + 业界口径调研（只记方向变更，不记实现细节） |

---

## 七、多余设计删除清单

「删」指**本次改造直接孤儿化**的旧设计，不是无关遗留的顺手重构（AGENTS.md 纪律）。

| # | 位置 | 删什么 | 理由 |
|---|---|---|---|
| C1 | `MessageList.tsx:493-514` | `structuredPillOf`（±行数 chip） | 交付物区不再展示轨迹文件；±行数是轨迹语义，且二进制没有行 |
| C2 | `MessageList.tsx:515-516` | `structured.length > 0 ? structured : fps` 二选一 | 这正是 R3；改成 declared ∪ produced 并集 |
| C3 | `PlanPanel.tsx:225-268` | 「环境信息」的完整文件清单（`MAX_FILE_ROWS` 那段） | 与右侧栏「改动」tab 同一份数据；降级为一行汇总 + 跳转。清清单重复渲染（**Q4 已拍板：降级**） |
| C4 | `types.ts:71-73` | `fileChanges` 字段注释里「FileSummary 优先用，空时回退正文正则抽取」 | 口径已变（改为并集，且 produced 半边只作兜底） |
| C5 | `useChatStream.ts:337-338` | 注释「收尾并入最终消息给 FileSummary 用」 | 改为「并入交付物区的 produced 半边」 |
| C6 | `openTarget.ts` | 仅服务旧 chips 的死代码 | **先查再删**：`extractFilePaths` 仍被 `RichText`（正文内联链接）与 `MarkdownText` 用，**必须保留**；只删确认无引用的部分 |

> C3 会改变计划浮层的既有形态（`SPEC.md:135` 把「环境信息」列为计划浮层三段之一），
> **Q4 已拍板：降级**。实施时必须同步改 `SPEC.md:135`（D1 覆盖），
> 否则文档与实现不一致——这一条是本批次的硬验收项。

---

## 八、分批实施与验收口径

每个批次一个 commit，行为变更配回归测试（AGENTS.md 纪律）。

### 批次 1｜路径识别修正（R1）

- 改动：F1
- 测试：`src/lib/openTarget.test.ts` 补用例
  - `D:\Program Files\wmessage-portable-2026-10-27\AI_Gen_Files\绩效工资分配实施办法宣贯（职工代表大会）.pptx` → **原样返回**
  - `...宣贯PPT设计说明（逐页）.md` → **原样返回**
  - 反例不变：`见 /Users/x/会议纪要 草稿版 已完成` → 仍截到首个空格（无扩展名分支未放开）
  - 反例不变：`...（职工代表大会）.pptx 生成完毕` → 只取到 `.pptx` 处，不吞后文
- 门禁：`bash scripts/test-fast.sh`（vitest + clippy）

### 批次 2｜交付物数据层（F2/F3/B1）

- 改动：F2、F3、B1
- 测试：
  - `src/lib/deliverables.test.ts`：`mergeDeliverables` 去重（小写归一）/ 排序 / 形态过滤（`.py` 被排除、`.pptx` 保留）
  - Rust：`bot_probe_paths` 单测（存在 / 不存在 / 目录 / 大小）
- 门禁：`bash scripts/test-fast.sh`

### 批次 3｜气泡交付物区（F4/F5 + 存在性校验接线）

- 改动：F4、F5
- 验收（可判定）：
  1. 气泡下方显示「📎 交付物 2」，条目 = pptx + 设计说明 md
  2. **11 个 `.py` 一个都不出现**
  3. chip 只有图标 + 文件名，无 ±行数
  4. 点 chip → 右侧栏打开、切到文件页、进入预览态（**一次跳转**）
  5. 刷新页面后显示**完全一致**（不随 trace/刷新漂移）
  6. 不存在的路径不进交付物区
  7. 空态两态可分：模型没点名但本轮有写盘 → 显示「本轮有 N 个产物未在回复中列出」+ 可点跳右侧栏「改动」tab；确实没产出 → 不留空行
- 测试：`MessageList.test.tsx` / `ChatPanel.test.tsx` 补气泡用例

### 批次 4｜右侧栏双子 tab（B2/F6/F7/F8）

- 改动：B2、F6、F7、F8
- 验收：
  1. 文件页顶部有「打开产物文件夹」按钮，点击在系统文件管理器打开 `AI_Gen_Files`
  2. 子 tab「改动 / 交付物」可切；改动 tab 内容与现状逐字一致
  3. 交付物 tab = 会话级清单（这一轮那 2 个）
  4. 从交付物点进 `.md` → **文本预览**（不是 diff）；从改动 tab 点进 `.py` → **看改动**（现状不变）
  5. 点 pptx → unsupported 态 + 主行动「用系统默认程序打开」；点它 → 开系统程序，**侧栏不跳转、不切 tab**
- 测试：`ContextRail.test.tsx` 补双子 tab 与预览来源用例

### 批次 5｜清理与文档（C3/C4/C5 + D1/D2）

- 改动：C3、C4、C5、D1、D2
- 验收：`npx tsc --noEmit` + `npx oxlint src` + `bash scripts/test-fast.sh`；
  计划浮层「环境信息」只剩一行汇总 + 跳转（清单不再重复渲染）；
  `SPEC.md:135` 与 `SPEC.md` 产品口径已同步、DEVLOG.md 已记方向变更

### 批次 6（可选）｜`run_python` 产物凭证（B4）

- 改动：B4
- 验收：执行流程里右侧栏「改动」tab 能看到脚本产出的 pptx；二进制档（路径 + 内容哈希，无行 diff）

---

## 九、风险与回滚

| 风险 | 影响 | 处置 |
|---|---|---|
| R1 放开全角括号后误吞正文 | 路径把后文带进去 | 带扩展名分支是**惰性匹配 + 扩展名锚定**，天然停在扩展名处；无扩展名分支保持收紧。批次 1 有反例用例兜底 |
| 模型没在正文写路径 | 交付物区为空 | 提示词契约已要求写全路径（`system.rs:60`）。**Q3 已拍板：给一行提示**「本轮有 N 个产物未在回复中列出」+ 跳右侧栏改动 tab（§4.5）；注意提示里的 N 取 produced 总数（含被形态过滤的脚本），不是过滤后剩几个 |
| `bot_probe_paths` 的 IPC 开销 | 每条消息一次调用 | 每条消息 memo 一次；会话级一次批量。同步 syscall 已包 `spawn_blocking` |
| 主聊天里 produced 半边恒空 | 脚本产物挂不上 | 已知缺口：主聊天无 trace（`trace_sink.rs:7-9`）。批次 6 只在有 trace 时生效；彻底解决要另开产物登记通道（下一轮） |
| 交付物区与正文不一致 | 交付物列表 ≠ 正文提到的 | 两条来源同源于正文 + 写盘凭证；两边合并去重后一致 |

**回滚**：批次逐个独立 commit，`git revert` 单批即可。前端批次可只回前端（Rust 命令留着不调用）；
后端批次回滚后前端调用会失败 → **回滚顺序必须后端先、前端后**（或前端先加降级：命令失败 → 交付物区显示原文路径不校验）。

---

## 十、已拍板结论（2026-10-27）

| # | 问题 | 结论 | 影响章节 |
|---|---|---|---|
| Q1 | 正文点名的配套说明文档（如「宣贯PPT设计说明（逐页）.md」）算不算交付物？ | **算，全都挂上**——模型点名了就挂，不做「主/辅」分档 | §5.2 |
| Q2 | 交付物区放气泡下方还是右侧栏？ | **两个都要**：气泡 = 这一轮；右侧栏 = 这个会话 | §4.1 |
| Q3 | 交付物区为空但本轮有写盘时要不要提示？ | **提示一行 + 可跳转**右侧栏「改动」tab | §4.5（G7） |
| Q4 | 计划浮层「环境信息」清单是否降级（C3）？ | **降级为一行汇总 + 跳转**；`SPEC.md:135` 同步改 | §7 C3、§8 批次 5 |

**这四条定下来的直接后果：**

1. declared 那半**不做形态过滤**——正文点名什么就挂什么，包括 `.md` 这类配套文档
2. 交付物区有两种空态，必须能分辨「这轮没产出」与「有产出但模型没点名」（§4.5）
3. 右侧栏「改动」tab 成为**改动文件的唯一完整清单处**；气泡 chips 与计划浮层都不再重复渲染它
4. 实施顺序上，`SPEC.md` 的产品口径改动（D1）是**批次 5 的硬验收项**，不是可选项

---

## 十一、偏离登记

（实施过程中如有偏离本文口径，在此登记；空 = 无偏离。）
