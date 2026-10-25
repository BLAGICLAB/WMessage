# 前端拆分方案（第三轮 · 前端篇）

> 状态：**待执行**（2026-10-25 出案）
> 关联：`docs/FILE-SPLIT-PLAN-2026-10-25.md`（第一轮 · Rust 18 条）、`docs/FILE-SPLIT-ROUND2-2026-10-25.md`（第二轮 · Rust 10 条）
> 交接：§十一 是**可直接粘贴到新对话任务的提示词**，新会话先读本文件再动手

---

## 一、为什么做这件事

Rust 侧两轮拆分已经把「>1300 行的文件」清到 **0 个**：`tests-audit/audit_file_size.py`
的 `WHITELIST` 现为 `{}`，门禁口径内最大文件 1265 行（`py/runtime.rs`）。

前端同一口径下还有 **4 个 2000+ 行文件**，但**前端至今不在任何行数门禁口径内**——
`audit_file_size.py` 的 `SCAN_ROOTS` 只有 `src-tauri/src|tests|examples`，docstring 里
明写着「前端要治是独立批次的事，不在本脚本混进来」。

前端最大的 `SettingsPage.tsx` 4068 行，比刚拆完的任何一个 Rust 巨型文件都大。
这批补的就是前端这一段。**Rust 侧的红线（纯搬移、零行为变化、每文件一 commit）
在前端同样适用**，但搬移手法和判定口径要换，见 §四 与 §六。

---

## 二、基线（2026-10-25 实测）

前端 `src/` 下 **249** 个 `.ts/.tsx`，共 **59 474** 行（不含 `node_modules`）。

| 分位 | p50 | p75 | p90 | p95 | p99 | max |
|---|---|---|---|---|---|---|
| 行数 | 122 | 234 | 498 | 687 | **2270** | **4068** |

**超 1500 行**（Rust 门禁的「新文件上限」口径）—— 4 个：

| 行数 | 文件 | 性质 | 相对 p99 |
|---|---|---|---|
| **4068** | `src/components/SettingsPage/SettingsPage.tsx` | 生产 | 1.8× |
| **3105** | `src/components/ChatPanel.test.tsx` | 测试 | 1.4× |
| **2985** | `src/components/SettingsPage.test.tsx` | 测试 | 1.3× |
| **2270** | `src/components/ChatPanel/ChatPanel.tsx` | 生产 | 1.0× |

**超 1200 行**（`docs/rust-bot-architecture.md` 里那条「每个文件 ≤ 1200」老约定）——
上面 4 个，外加 `src/components/GraphPage/GraphCanvas.tsx`（**1245**）。

前端不是「普遍偏大」：249 个文件里只有 **5** 根柱子，第 6 名往下是 1182
（`src/App.tsx`）。这 5 根是**四根柱子**——一个巨型设置页、一个巨型聊天面板、
两个配套的巨型测试文件，外加一个贴着线的图谱画布。

---

## 三、前端这边有什么门禁、没有什么门禁

**有**（pre-commit 快门禁 `scripts/test-fast.sh` 的 TS 链路，改 `src/` 即触发）：

| 步骤 | 命令 | 门禁强度 | 拆分时最容易炸的点 |
|---|---|---|---|
| [0/N] | 工单号防线 | 拦 | 新增行含工单号 → 需行尾 `audit-ok` |
| [3/N] | `npx tsc --noEmit` | 拦 | props 类型不匹配 / 导入路径错 |
| [3.5/N] | `npx knip --no-progress` | 拦 | **新文件的 export 无人消费 → 判死** |
| [3.6/N] | `npx oxlint src` | error 拦 | `react-hooks/rules-of-hooks` = error |
| [4/N] | `npx vitest --run --changed` | 拦 | 用例数变化 / mock 作用域 |

**没有**：

- **行数门禁**——`audit_file_size.py` 只扫 Rust，前端超长文件无人拦。
- **自动格式化器**——仓库无 prettier（`package.json` 无、`*.prettierrc*` 不存在），
  `lint` 只跑 oxlint。

第二条是本方案的关键前提：**前端没有格式化器会重排文件**，所以「搬移」=
字节级搬走 + 少量接线，判定可以比 Rust 侧更硬（Rust 侧要靠词法分类容忍
rustfmt 的重排，前端侧不需要）。

---

## 四、拆分原则

### 4.1 两类搬移

| 类别 | 含义 | 判定 |
|---|---|---|
| **A 类 · 纯搬移** | 整段代码原样移走，不改一个字符（只可能减少缩进） | 被搬块**逐行 lstrip 后完全相同** |
| **B 类 · 搬移 + 接线** | 被搬块本身逐字不变，但在**搬走之外**新增接线代码 | 被搬块逐行相同 **且** 接线代码符合 §4.2 白名单 |

A 类候选：纯函数、常量、类型、`describe` 块、整个子组件文件。
B 类候选：JSX 区块抽成子组件、hook 段抽成自定义 hook。

### 4.2 「接线」允许形态白名单（B 类新增代码只能是这些）

1. 新文件顶部的 `import` 语句；
2. props / 参数的类型声明；
3. 组件签名里的解构（`{ a, b }: Props`）——**目的就是让被搬块内的标识符保持原样**；
4. 调用处把原来的内联 JSX 换成 `<NewComponent ... />`；
5. 自定义 hook 的返回值解构；
6. 文件顶部的 `export` 关键字。

**白名单之外的一切新增/修改都要停下来问人。** 例如：改文案、改类名、调顺序、
加 `useMemo`、改 props 形状的语义——这些都不是搬移。

### 4.3 硬性禁令

- **不改行为**：零功能变化、零文案变化、零样式类名变化。
- **不改公开导入路径**：`import { SettingsPage } from "./components/SettingsPage"` 照旧可用
  （`src/components/SettingsPage/index.tsx` 透传）。
- **不改渲染语义**：SettingsPage 的 16 个 `hidden={activeSection !== "xxx"}` 区块
  是**同时挂载**的（靠 `hidden` 切可见性），抽成子组件后必须**仍然同时挂载**，
  不能改成条件渲染。
- **不改 hook 调用顺序**：只有**连续**的 hook 段才能抽成自定义 hook，抽取后
  调用位置 = 原第一行位置。
- **不动 mock 语义**：见 §八.1。
- **不顺手重构**：本批只做拆分，看到的其它问题记进 DEVLOG，不在这里修。
- **每文件一个 commit**，不合并批次。
- **不主动出 Windows 包**（前端无 exe，但同理：不动打包流程）。

### 4.4 目标线

- 每个文件 **≤ 1500 行**（门禁线）——**硬目标**。
- 理想 **≤ 1200 行**（老约定）——尽量达成，不为此硬拆。
- 拆分后**不引入新的超长文件**（例如别把 600 行再拆成两个 400 行 + 一个 300 行
  的三件套，纯属噪音）。

---

## 五、逐文件方案

> 行号锚点基于 **2026-10-25 的 HEAD**（`e0da2564`）。动手前重新核对——
> 文件结构若已变，以「结构」描述为准，锚点仅作导航。

### F1 · `src/components/SettingsPage/SettingsPage.tsx`（4068 行）

**现状结构**（实测）：

| 行段 | 内容 |
|---|---|
| 1–96 | 头部注释 + imports |
| 97–382 | 类型与常量：`PresetModel` `ProviderPreset` `PROVIDER_PRESETS` `API_FORMAT_OPTIONS` `AUTO_EXTRACT_MODES` `isAutoExtractMode` `KNOWN_PROTOCOL_URLS` `CURATED_PROVIDER_KEYS` `metaModelToEntry` `hostOf` `Props` `SectionKey` `SECTIONS` `AGENT_PARAM_ROWS` `AGENT_PARAM_FALLBACK_LABELS` `U32_MAX` |
| 383 | `export function SettingsPage({...}: Props) {` |
| 384–1850 | 组件体前半：state / hooks / handlers / 内联箭头子组件 |
| 1873–2064 | 组件体后半：更多 handlers + 一个 `useEffect`（1958 挂载即 `refresh()`） |
| 2066–4068 | **主 return：2003 行 JSX**，16 个 `hidden` 区块 |

`hidden` 区块行段（实测）：

| 行段 | 区块 | 行数 |
|---|---|---|
| 2114–2243 | `general` | 129 |
| 2243–2463 | `data` | 220 |
| 2463–2811 | `bot`（第 1 段） | 348 |
| 2811–2964 | `memory` | 153 |
| 2964–3578 | `model`（最大） | 614 |
| 3578–3733 | `bot`（第 2 段） | 155 |
| 3733–3856 | `mcp` | 123 |
| 3856–3992 | `bot`（第 3 段） | 136 |
| 3992–4068 | skills / mcp / services / graph / desk / tokens / opsHistory / logs | 76（已用面板组件，别动） |

**切法三段**（建议按 S1 → S2 → S3 三个 commit，也可合成一个文件的单 commit，
但**一次只动一段**，每段独立过门禁）：

**S1（纯搬移，风险最低）**：97–382 的类型与常量 → 并入同目录既有的
`types.ts`（196 行）与 `constants.ts`（18 行）。
- 搬移后 `SettingsPage.tsx` 从 4068 → ~3770。
- 已被 `types.ts` / `constants.ts` 拥有的同名符号**不要重复定义**（先读这两个文件）。
- `Props` / `SectionKey` 若被外部引用 → 保持在 `types.ts` 并 `export`；
  仅本文件用 → 搬进 `types.ts` 但**不 export**（knip 会判死 export，见 §八.4）。

**S2（搬移 + 接线）**：主 return 里 **8 个大 `hidden` 区块** → 8 个 section 组件，
放在 `src/components/SettingsPage/` 下，命名沿用现有约定
（`XxxSection.tsx` / `XxxPanel.tsx`，同目录已有 `XxxPanel.test.tsx` 的先例）：

| 新文件 | 来源行段 | 预估 |
|---|---|---|
| `GeneralSection.tsx` | 2114–2243 | ~135 |
| `DataSection.tsx` | 2243–2463 | ~230 |
| `BotCoreSection.tsx` | 2463–2811 | ~360 |
| `MemorySection.tsx` | 2811–2964 | ~165 |
| `ModelSection.tsx` | 2964–3578 | ~630 |
| `BotApiSection.tsx` | 3578–3733 | ~165 |
| `McpSection.tsx` | 3733–3856 | ~135 |
| `BotToolsSection.tsx` | 3856–3992 | ~145 |

搬移手法（保住被搬块逐字不变）：
```tsx
// ModelSection.tsx
import { ... } from "react";

export type ModelSectionProps = { /* ... */ };

export function ModelSection({ models, activeModelId, onPick, ... }: ModelSectionProps) {
  return (
    /* ← 被搬块从这里开始，逐字照抄 */
  );
}
```
- 父组件里的 `hidden={activeSection !== "model"}` **留在父组件**（它控制挂载与可见性，
  抽走会改变语义）→ 父组件写成：
  `<div hidden={activeSection !== "model"}><ModelSection ... /></div>`。
- ⚠️ **`useEffect` / `useState` 绝不能跟着 JSX 走**：区块内如有 hook，必须留在父组件
  或整体搬进 section 组件并接受 effect 顺序变化（§八.2）。**动手前逐块扫
  `use[A-Z]`**，有一处就先停下来问。
- ⚠️ **DOM ref**：区块内若有 `ref={xxxRef}` 传给父组件的 input/ref，必须一起搬
  （连 ref 对象一起进 props），否则受控输入失焦。

**S3（搬移 + 接线，可选）**：组件体 384–2064 的 state / handlers → 自定义 hook 文件
（沿用 `src/components/ChatPanel/useChatUi.ts` 的先例，命名 `useSettingsXxx.ts`）：
- `useSettingsState.ts`：全部 `useState`；
- `useSettingsHandlers.ts`：`invoke` 链路类 handlers（`clearVendorKey` `toggleAutostart`
  `refresh` `toggle` `runExport` `runImport` `runExportWs` `runImportWs`
  `copyToken` `rotateToken` `consolidateNow` …）；
- ⚠️ hook 段必须**连续**，且抽取后调用位置 = 原第一行位置（§4.3）。
- ⚠️ handler 依赖 `useCallback` 的 deps 数组，搬动后 deps 不变（依赖的是同一批 state）。

**S1+S2+S3 做完的预估终态**：`SettingsPage.tsx` ≈ **900–1100 行**（门面：state +
handlers + 16 个区块的挂载骨架），8 个 section 文件每个 ≤ 630 行。

---

### F2 · `src/components/ChatPanel/ChatPanel.tsx`（2270 行）

**现状结构**（实测）：

| 行段 | 内容 |
|---|---|
| 1–75 | imports |
| 76–124 | 纯函数与常量：`parseJsonField` `TURN_STATUS_VALUES` `parseTurnStatus` `STICK_BOTTOM_PX` `Props` |
| 125–2097 | 组件体：10 个 `useState`、3 个 `useRef`、17 个 `useEffect`、17 个 `useCallback`、1 个 `useMemo`、1 个 `useLayoutEffect`，以及大量内联 handler |
| 2098–2270 | JSX（**已经是子组件**：`SessionRail` `UsageMeter` `MessageList` `InputArea` `ContextRail` `TracePanel`） |

**这条文件的性质与 F1 相反：它是「逻辑重、JSX 轻」。** JSX 只占最后 172 行，
前面 1973 行全是状态与副作用。所以切法是**抽 hook，不是抽组件**。

**切法**：

**C1（纯搬移）**：76–124 的纯函数与常量 →
- `parseJsonField` / `parseTurnStatus` → 并入既有 `src/components/ChatPanel/types.ts`
  或新建 `parse.ts`（同目录已有 `streamState.ts` `stickyBubbleWidth.ts` 这类工具文件的先例）；
- `TURN_STATUS_VALUES` → 并入既有 `constants.ts`；
- `STICK_BOTTOM_PX` → 并入既有 `constants.ts`；
- ⚠️ `Props` 类型：被 `ChatPanel/index.tsx` 或别处引用 → 留原处并 export；否则搬走不 export。

**C2（搬移 + 接线）**：组件体按**连续段**抽成自定义 hook 文件：
| 新文件 | 抽什么 |
|---|---|
| `useChatSessions.ts` | `sessions` `sessionId` `messages` `drafts` `patchDraft` + 相关 effect |
| `useModelPicker.ts` | `models` `apiProvider` `activeModelId` `modelMenuOpen` `modelBtnRef` `modelDropdownRef` `modelLabel` + 下拉开合 |
| `useEffortMenu.ts` | `effortBase` `effortBySession` `effortMenuOpen` `effortBtnRef` `effortDropdownRef` |
| `usePermMenu.ts` | `permMode` `permModeRef` `permMenuOpen` `permBtnRef` `permDropdownRef` `selectPermMode` |
| `useChatRail.ts` | `railOpen` `toggleRail` `ctxOpen` `ctxTab` `ctxOpen` `subagentStripOpen` `subagentDetailId` `openSubagentDetail` |
| `useTracePanel.ts` | `openTrace` `copiedIdx` `stopNotice` `pendingExecRef` |

- ⚠️ **hook 调用顺序**：每段内部顺序原样保留，段与段之间的相对顺序原样保留；
  抽取后 `useChatSessions()` 调用在原来 `useState(sessions)` 的位置，以此类推。
- ⚠️ **闭包新鲜度**：handler 读 state 走的是同一次 render 的闭包；抽成 hook 后
  若用 `useRef` 缓存旧值会引入陈旧读取——**不要新增 ref 缓存**。
- ⚠️ **`useEffect` 的 cleanup 顺序**：React 按声明顺序跑 cleanup，抽取后若把两个
  effect 段合并/重排，cleanup 顺序会变 → **段的相对顺序不许动**。
- 自定义 hook 必须以 `use` 开头（oxlint `rules-of-hooks` = error）。

**预估终态**：`ChatPanel.tsx` ≈ **700–900 行**（门面：组装 + JSX），6 个 hook 文件每个 ≤ 250 行。

---

### F3 · `src/components/ChatPanel.test.tsx`（3105 行）

**现状**（实测）：3 个顶层 `describe` —— `ChatPanel`（163）、`聊天窗滚动（流式期）`（2696）、
`在途回合（切走再切回不丢消息）`（2885），内部还有大量嵌套 `describe`。

**切法（A 类 · 纯搬移）**：按**嵌套 describe 主题**切到同目录，
命名沿用现有先例（`MessageList.test.tsx` `SubagentDetailPanel.test.tsx` 等）：

| 新文件 | 内容 |
|---|---|
| `ChatPanel.test.tsx`（留） | 顶层 `describe("ChatPanel")` 的主干 + 共享 setup |
| `ChatPanel.scroll.test.tsx` | `聊天窗滚动（流式期）` |
| `ChatPanel.inflight.test.tsx` | `在途回合（切走再切回不丢消息）` |
| `ChatPanel.<主题>.test.tsx` | 顶层 describe 内的大块嵌套 describe，按主题再切 2–4 个 |

- ⚠️ **`vi.mock` 必须每个新文件自带**：`vi.mock("@tauri-apps/api/core")` 这类
  模块 mock 是**按文件作用域**生效的，搬走不补 = 该文件里 mock 失效 → 真调 Tauri。
- ⚠️ **`vi.hoisted` 共享 mock 变量不可跨文件共享**——每个新文件各自 `vi.hoisted`
  一份。副作用：**跨 describe 的 mock 调用历史断言会失效**（如「第二个测试开始时
  invoke 已被调用 N 次」）。拆分前先 grep 断言里有没有依赖调用计数的。
- ⚠️ **拆完 vitest 用例总数必须不变**——这是前端版「行为没变」的硬判据。
- ⚠️ `src/test/setup.ts`（vitest `setupFiles`）是全局的，不用动。

---

### F4 · `src/components/SettingsPage.test.tsx`（2985 行）

**现状**（实测）：**只有 1 个顶层 `describe("SettingsPage")`**（317 行起），
2985 行几乎全在一个 describe 里，顶部有 ~317 行的
`vi.hoisted` + `mocks.invokeMock.mockImplementation(...)` 大 switch
（按命令名返回各配置）。

**切法（A 类 · 纯搬移 + 接线）**：
- 先把顶部的 mock 装置（`mocks` + `mockImplementation` 大 switch）抽成
  **共享模块** `src/components/SettingsPage/__testMocks.ts`：
  - 导出 `mocks`（`vi.hoisted` 仍在使用方文件里声明，**工厂体**放进共享模块）
  - ⚠️ `vi.hoisted` 的工厂函数体可以放共享模块，但**调用 `vi.hoisted(...)` 本身
    必须发生在使用方文件的顶层**（hoist 语义）。
  - 备选：每个新测试文件各写一份 mock（复制 300 行 × 4 = 不可接受）→ 走共享模块。
- 再按**被测区块**切 4–6 个测试文件：
  `SettingsPage.model.test.tsx` / `.bot.test.tsx` / `.memory.test.tsx` /
  `.data.test.tsx` / `.mcp.test.tsx` / `.general.test.tsx`，
  每个文件 `import { mocks } from "./__testMocks"` 后自带
  `beforeEach(() => { mocks.invokeMock.mockReset(); ... })`。

---

### F5 · `src/components/GraphPage/GraphCanvas.tsx`（1245 行）——**可选，最后做**

**现状结构**（实测）：已经是「常量 + 材质工厂 + 布局缓存 + 工具函数」的形状，
比 F1/F2 健康得多。1245 行贴着 1200 老约定，但**没破 1500 门禁线**。

**切法（只做纯搬移，不做重构）**：
- 把 glsl 材质常量（`NODE_VERT` 77 / `NODE_FRAG` 89 / `EDGE_VERT` 121 / `EDGE_FRAG` 144）
  与材质工厂（`createNodeMaterial` 110 / `createEdgeMaterial` 157）搬到
  `src/components/GraphPage/graph-materials.ts`（或同目录 `materials.ts`）；
- 搬完后 `GraphCanvas.tsx` ≈ 1000 行。
- ⚠️ 这条是**锦上添花**：只做纯搬移、不拆逻辑；若判定投入产出不划算，可整条不做。

---

## 六、纯搬移核对（前端版）

Rust 侧的 `scripts/verify_pure_move.py` 是**词法级**的（要分辨 code / comment /
literal 三类，因为 rustfmt 会重排）。**前端没有格式化器，不需要这套复杂度**——
用**逐行 lstrip/rstrip 比对**即可，且判定更硬。

**新脚本**：`scripts/verify_pure_move_tsx.py`（照 `verify_pure_move.py` 的命令行
形状写，但实现简化）。

```
用法：python3 scripts/verify_pure_move_tsx.py <原文件> --manifest <清单> --base-dir <目录>
清单每行：<相对路径>:<起>-<止>      # 行号指原文件，1-based 闭区间
可选：--allow-chunk 'rel:a-b'      # 声明该段是获准的接线改动
```

判定口径：
1. **被搬块比对**：把原文件 `[起-止]` 行与目标文件对应区间，逐行
   `lstrip()` 后比较 → **必须完全相同**（去缩进差异，不容忍内容差异）。
   B 类的「接线」不在被搬块内，所以不受影响。
2. **覆盖检查**：原文件里**不在清单内**的行，必须全部是
   `import` / `export` / 注释 / 空行 / 闭合括号——出现实体代码行就报「未登记的改动」。
3. **接线检查**：`--allow-chunk` 声明的段，逐行内容允许不同，但**行数必须对齐**，
   且改动行数 ≤ 声明段的 1/3（防「声明了但其实大改」）。

> ⚠️ 该脚本**不检查 JSX 文本的空白语义**——JSX 里 `<p> hello </p>` 的空格有意义，
> 逐行 lstrip 会放过这类改动。**所以人眼过 diff 仍是必检项**，脚本只是防大改。

---

## 七、每批收尾清单（每个 commit 前）

```bash
cd /Users/renshi/Projects/wmessage

# 1. 类型
npx tsc --noEmit -p tsconfig.json

# 2. 死代码（最容易炸的一关）
npx knip --no-progress

# 3. lint
npx oxlint src

# 4. 单测（拆分前后用例总数必须一致）
npm test                      # 记下 files / tests 计数
npx vitest run src/components/SettingsPage src/components/ChatPanel   # 相关子集

# 5. 纯搬移核对
python3 scripts/verify_pure_move_tsx.py <原文件> --manifest <清单> --base-dir <目录>

# 6. 人眼过 diff（脚本不懂意图）
git diff --stat && git diff
```

- **用例总数不变**是前端版的硬判据（Rust 版是 nextest 2191 不变）——
  拆完 vitest 报出来的 `tests` 计数必须与拆分前**完全相同**。
- `bash scripts/test-fast.sh` 由 pre-commit 自动跑（前 4 步已含在上面的命令里）。
- **不跑 Rust 链路**：本批只动 `src/`，`test-fast.sh` 会自动跳过 cargo。

---

## 八、风险与已知坑（前端特有）

1. **`vi.mock` 按文件作用域生效**——拆测试文件后必须每个文件自带 mock 声明，
   否则 mock 失效 → 测试真去调 Tauri → 全红或假绿。
2. **effect 顺序会变**——把带 `useEffect` 的 JSX 区块抽成子组件后，子组件的 effect
   在父组件 effect **之后**执行。SettingsPage 有「挂载即 `refresh()`」这类 effect，
   顺序变了就是行为变了。**规则：逻辑 effect 留在父组件，只搬纯 UI 区块。**
3. **`hidden` 区块是同时挂载的**——抽成子组件后必须仍然同时挂载，不能改成条件渲染。
   （与 §4.3 呼应，这是最容易「顺手优化」掉的地方。）
4. **knip 判死 export**——新文件里导出但无人消费的符号（类型、helper）会被 knip
   拦。**只 export 被消费的符号**；仅本文件用的类型不 export。
5. **rules-of-hooks 是 error**——自定义 hook 必须 `use` 开头；hook 只能在组件/
   自定义 hook 顶层**无条件**调用；不能把 hook 塞进条件分支或循环。
6. **`useCallback` / `useEffect` 的 deps 数组**——搬动后 deps 内容不变，但 oxlint 的
   `exhaustive-deps` 是 warn（不拦）——搬完若新增一批 warn，**不修**（那是噪音，
   修它就不是纯搬移了），记 DEVLOG。
7. **受控输入的 `ref`**——区块内若有 `ref` 传给父级 input，ref 对象要连同 props 一起搬。
8. **跨 describe 的 mock 调用计数断言**——拆测试文件后每个文件的 mock 独立，
   依赖「上一个测试调了几次」的断言会失效。拆前 grep 断言里的调用次数比较。
9. **测试顺序**——vitest 不保证文件/describe 顺序，但拆分改变了文件内的相对顺序；
   若有 describe 间共享的模块级状态（`let counter = 0`），会坏。拆前 grep。
10. **工单号防线**——新增的 `.ts/.tsx` 行禁含工单号（`批次N审计` `P0-12` `T1-3`
    `AUDIT-*` 「拍板」等）；确需引用历史口径加行尾 `audit-ok`。
11. **knip 的 `entry` 只有 `scripts/sync-lobe-icons.mjs`**——新文件只要被 `src/` 下
    任何文件 import 就算入口可达；但**只被测试文件 import** 的文件可能被判「死文件」。
    拆分时确认每个新文件的 import 方。

---

## 九、批次顺序与依赖

| 序 | 批次 | 为什么排这个位置 | 预估终态行数 |
|---|---|---|---|
| 1 | **F3** `ChatPanel.test.tsx` 拆 3–4 个 | 最纯（A 类），先拿它验证搬移手法与核对脚本 | 每个 ≤ 1200 |
| 2 | **F4** `SettingsPage.test.tsx` 拆 4–6 个 | 同上，但要处理共享 mock 模块 | 每个 ≤ 1200 |
| 3 | **F1-S1** SettingsPage 常量/类型外移 | 纯搬移 | 3770 |
| 4 | **F1-S2** SettingsPage 8 个 section 外移 | B 类，本批技术风险最高 | 门面 ~1000 |
| 5 | **F2-C1+C2** ChatPanel 逻辑外移 | B 类 | 门面 ~800 |
| 6 | **F5** GraphCanvas 材质外移 | 可选，最后做 | ~1000 |
| 7 | **收尾**：把 `.ts/.tsx` 纳管进 `audit_file_size.py` | 拆完再纳管，避免中途白名单反复 | — |

**第 7 步（纳管门禁）的具体做法**（照 Rust 侧的棘轮模式）：

- `tests-audit/audit_file_size.py` 的 `SCAN_ROOTS` 增加前端根 `src`；
  上限沿用 1500；**口径从「Rust 行数」改成「前后端统一行数」**。
- 存量超限文件先登记进 `WHITELIST`（路径 → 当前行数），只许变小不许变大；
  拆到 ≤1500 就销条目、收紧棘轮。
- ⚠️ `audit_file_size.py` 的 docstring 现在写着「前端 `.ts/.tsx` 不在本门禁口径内」
  ——纳管时**必须同步改这段**，否则又是事实漂移（第一轮踩过：docstring 与实现漂移，
  commit `65d292f6` 修的正是这个）。
- ⚠️ 若 `audit_module_map.py` 或 `audit_doc_claims.py` 里有对前端文件数的断言，
  同步更新（`audit_doc_claims.py` 命中过 `src/`，需查）。
- 纳管后跑：`python3 -m pytest tests-audit/audit_file_size.py -q`。

**每批一个 commit**（批次号用 F1/F2/… 区分，不要用数字批次号——
**源码里禁出现工单号**，但 commit message 可以）。

---

## 十、验收判据（整批做完时）

- [ ] 前端**无文件 > 1500 行**；目标 **无文件 > 1200 行**（`GraphCanvas` 可豁免到 1000 档）。
- [ ] `npx tsc --noEmit` ✓
- [ ] `npx knip --no-progress` ✓（零新增死文件/死 export）
- [ ] `npx oxlint src` ✓（无新增 error）
- [ ] `npm test` **用例总数与拆分前完全一致**（当前基线：100 files / 1065 tests）
- [ ] `python3 scripts/verify_pure_move_tsx.py` 每个文件全绿
- [ ] 人眼过每个 diff：无文案变化、无类名变化、无顺序变化
- [ ] 每个文件一个 commit，commit message 讲清「搬了什么、边界在哪」
- [ ] DEVLOG 记「为什么换方向」（如选了这个切法而不是那个），**不记工单号进源码**
- [ ] 纳管门禁后 `audit_file_size.py` 通过，`WHITELIST` 只剩允许的存量条目

---

## 十一、提示词（复制给新对话任务）

> 下面整段复制粘贴到一个新的对话任务里即可。它是自包含的：新会话不知道
> 本仓库历史，靠这段 + 本文件开工。

```text
我在 /Users/renshi/Projects/wmessage 这个仓库（Rust/Tauri 后端 src-tauri/ + React 前端 src/）里要执行「前端文件拆分」批次。

【必读，按顺序】
1. AGENTS.md（仓库工作约定：硬性禁令、工作方式、门禁命令）
2. docs/FILE-SPLIT-FRONTEND-2026-10-25.md（本次方案全文：基线、逐文件切法、
   纯搬移核对器规格、风险清单、验收判据）
3. docs/FILE-SPLIT-ROUND2-2026-10-25.md §六（上一轮 Rust 拆分学到的教训，
   思路通用；前端换了一套核对器）

【背景】
Rust 侧两轮拆分已把 >1300 行的 Rust 文件清到 0 个（tests-audit/audit_file_size.py
的 WHITELIST 现为 {}）。前端还剩 5 个大文件，是本批的目标：
  src/components/SettingsPage/SettingsPage.tsx      4068
  src/components/ChatPanel.test.tsx                  3105
  src/components/SettingsPage.test.tsx               2985
  src/components/ChatPanel/ChatPanel.tsx             2270
  src/components/GraphPage/GraphCanvas.tsx           1245（可选，最后做）

【本批性质：纯搬移，零行为变化】
- 每拆一个文件一个 commit，不合并批次，不在 commit 里夹带别的改动。
- 被搬走的代码块必须逐字不变（只允许减少缩进）。允许新增的只有「接线」：
  import / props 类型声明 / 组件签名解构 / 调用处替换 / export 关键字。
  方案 §4.2 是完整白名单，白名单之外的改动一律停下来问我。
- 禁止：改文案、改类名、改渲染顺序、改 hook 调用顺序、把 hidden 区块改成条件渲染、
  顺手重构、动 mock 语义、源码里写工单号（注释也不许；要引用历史口径加行尾 audit-ok）。

【技术要点】
- 前端没有 prettier，搬移是字节级的，不需要 Rust 那套词法分类。
- 需要新写一个纯搬移核对器 scripts/verify_pure_move_tsx.py（方案 §六 给了
  规格：逐行 lstrip 比对 + 覆盖检查 + --allow-chunk 接线段声明）。
  命令行形状照 scripts/verify_pure_move.py，但实现可以大幅简化。
- 测试文件拆分要特别注意 vi.mock 按文件作用域生效、vi.hoisted 不可跨文件共享
  （方案 §八.1、§八.8）。SettingsPage.test.tsx 顶部 ~317 行的 mock 大 switch
  要抽成共享模块，vi.hoisted(...) 的调用仍要留在使用方文件顶层。
- SettingsPage 的 16 个 hidden={activeSection !== "xxx"} 区块是同时挂载的，
  抽成子组件后必须仍然同时挂载；带 useEffect 的逻辑要留在父组件，不要跟着 JSX 走
  （方案 §八.2、§八.3）。
- ChatPanel 是逻辑重 JSX 轻（127-2097 全是 state/effect/callback），要抽自定义 hook，
  不是抽组件；hook 段必须连续、抽取后调用位置 = 原第一行位置。

【执行顺序（一次只做一个文件，做完一个再问我要不要继续下一个）】
1. src/components/ChatPanel.test.tsx        → 拆 3-4 个
2. src/components/SettingsPage.test.tsx     → 拆 4-6 个（先做共享 mock 模块）
3. src/components/SettingsPage/SettingsPage.tsx → S1 常量/类型外移 → S2 8 个 section 外移
4. src/components/ChatPanel/ChatPanel.tsx   → C1 helper 外移 → C2 抽 6 个 hook
5. src/components/GraphPage/GraphCanvas.tsx → 可选，只搬 glsl 材质常量
6. 收尾：把 .ts/.tsx 纳管进 tests-audit/audit_file_size.py（WHITELIST 棘轮 + 同步 docstring）

【每个 commit 前的收尾清单】
  npx tsc --noEmit -p tsconfig.json
  npx knip --no-progress
  npx oxlint src
  npm test            # 用例总数必须与拆分前一致（基线 100 files / 1065 tests）
  python3 scripts/verify_pure_move_tsx.py <原文件> --manifest <清单> --base-dir <目录>
  人眼过 git diff
pre-commit 会自动跑 scripts/test-fast.sh（TS 链路）；本批不涉及 Rust，不要跑 cargo。

【开工前先做两件事】
1. git status / git log --oneline -3，确认工作区干净、当前 HEAD；
   仓库里可能有一个不属于本批的未跟踪文件
   docs/CONTEXT-TOOLCALL-OPTIMIZATION-DESIGN-2026-10-25.md —— 不要动它。
2. 重新核对方案 §五 里各文件的行号锚点（行号基于 2026-10-25 的 HEAD e0da2564，
   若已漂移以「结构」描述为准）。

先读文档，然后告诉我你打算先做哪个文件、打算怎么切，等我确认后再动手。
```

---

## 附：与前两轮 Rust 拆分的对应关系

| Rust 侧做法 | 前端对应 |
|---|---|
| `pub(super)` 可见性翻译 | TS 的 `export` / 不 export（knip 判死 export） |
| 门面 `pub use` 逐项转出 | 子组件文件 `export function Xxx`，门面 `import` 后使用 |
| `scripts/verify_pure_move.py`（词法分类） | `scripts/verify_pure_move_tsx.py`（逐行 lstrip，更简单更硬） |
| nextest 用例数 2191 不变 | vitest 用例数 1065 不变 |
| `audit_file_size.py` 白名单棘轮 | 同款，纳管 `.ts/.tsx` 后启用 |
| 冻结守卫白名单按路径登记 | knip 的死 export / 死文件，路径敏感 |
| 纯搬移红线的例外白名单 | §4.2 接线白名单（6 条） |
