# 001 — 建立动效 token 层，把曲线与时长收归一处

- **Status**: TODO
- **Commit**: b22387c7
- **Severity**: HIGH
- **Category**: §7 Cohesion & tokens / §2 Easing & duration
- **Estimated scope**: 22 files（1 个 CSS + 20 个 tsx + 1 个测试），约 120 行改动

## Problem

同一个「hover 一个控件」的动作，仓库里有两套曲线、两种时长，全靠手写。

**A 套：`src/ui/main.css` 里手写的 `transition:` 声明**——共 10 处，每处自带时长，用 CSS 关键字 `ease`：

```css
/* src/ui/main.css:236-238 —— .nm-btn */
  .nm-btn {
    ...
    transition:
      background-color 0.1s ease,
      border-color 0.1s ease;
  }
```

**B 套：tsx 里的 Tailwind 工具类**——共 38 处 `transition-*`，继承 Tailwind 3.4.19 的默认 `transition-timing-function: cubic-bezier(0.4, 0, 0.2, 1)` 与默认 150ms：

```tsx
/* src/components/HomePage/PoolPager.tsx:88 */
className="rounded-md px-2 py-0.5 text-xs text-[var(--t4)] transition-colors hover:bg-[var(--hover-bg)] hover:text-[var(--t2)]"
```

两套曲线的实际差别（按 cubic-bezier 求位移进度，25% 时间点）：

| 曲线 | 25% 时间 | 50% 时间 | 用在哪 |
| --- | --- | --- | --- |
| `ease` = `cubic-bezier(0.25, 0.1, 0.25, 1)` | 40.9% | 80.2% | main.css 全部 10 处 |
| `cubic-bezier(0.4, 0, 0.2, 1)`（Tailwind 默认） | **23.7%** | 77.6% | tsx 全部 38 处 |

Tailwind 那条是慢起曲线——按下去之后颜色变化「慢半拍才开始动」，这正是芯片/按钮 hover 显得滞后的成因。

**38 处里有 26 处是 `transition-colors`，其中 20 处没有 `duration-*` 类**，因此落在 Tailwind 默认的 150ms 上。而仓库自己在 `src/ui/main.css:158` 写着：

```css
   动效纪律：卡片 150ms，按钮/ Chips 100ms，无弹跳
```

按钮和 Chips 的预算是 100ms，这 20 处却跑 150ms——超出自定预算 50%。

**并且全仓没有任何 `--ease-*` / `--duration-*` token**：现有 10 条手写 `transition:` 各自带时长（0.1s / 0.12s / 0.15s），另有 6 条各自手写的曲线散在 `TaskPool.tsx`、`TaskEditorOverlay.tsx`、`main.css` 的 `@keyframes` 里。下一个要加动效的人没有归口，只能再手写一遍。

## Target

**一、`src/ui/main.css` 的 `:root` 里加一层动效 token**（唯一来源，代码里不再出现手写曲线）：

```css
  /* 动效 token（曲线 + 时长）——全仓唯一动效词汇表。
     时长阶梯对齐上方纪律（卡片 150ms，按钮/Chips 100ms，无弹跳）；
     曲线用强 ease-out / ease-in-out，不用 CSS 关键字 ease
     （cubic-bezier(0.25,0.1,0.25,1)），也不用 Tailwind 默认
     cubic-bezier(0.4,0,0.2,1)（慢起，hover 显滞）。 */
  --ease-out: cubic-bezier(0.23, 1, 0.32, 1);
  --ease-in-out: cubic-bezier(0.77, 0, 0.175, 1);
  --ease-drawer: cubic-bezier(0.32, 0.72, 0, 1);
  --dur-press: 160ms; /* 按压反馈 */
  --dur-chip: 100ms; /* 按钮 / Chips / hover 抬升 */
  --dur-card: 150ms; /* 卡片表面与选中态 */
  --dur-fade: 150ms; /* 淡入淡出 / 遮罩 */
  --dur-pop: 200ms; /* 下拉 / 浮层 */
  --dur-panel: 240ms; /* 面板 / 抽屉 */
  --dur-tooltip: 150ms; /* 提示条 */
```

**二、main.css 里的 10 条手写 `transition:` 全部换成 token 引用**：

```css
/* 改前 */
  transition:
    background-color 0.1s ease,
    border-color 0.1s ease;

/* 改后 */
  transition:
    background-color var(--dur-chip) var(--ease-out),
    border-color var(--dur-chip) var(--ease-out);
```

逐条对照表（改前 → 改后）：

| 位置 | 选择器 | 改前 | 改后 |
| --- | --- | --- | --- |
| `main.css:94-96` | `body` | `background-color 0.15s ease, color 0.15s ease` | `background-color var(--dur-card) var(--ease-out), color var(--dur-card) var(--ease-out)` |
| `main.css:168-170` | `.nm-card, .nm-card-hover` | `background-color 0.15s ease, border-color 0.15s ease` | `background-color var(--dur-card) var(--ease-out), border-color var(--dur-card) var(--ease-out)` |
| `main.css:202-204` | `.nm-inset` | `background-color 0.1s ease, border-color 0.1s ease` | `background-color var(--dur-chip) var(--ease-out), border-color var(--dur-chip) var(--ease-out)` |
| `main.css:212-214` | `.nm-outset` | 同上 | 同上 |
| `main.css:236-238` | `.nm-btn` | 同上 | 同上 |
| `main.css:262-264` | `.nm-icon-btn` | `background-color 0.1s ease, color 0.1s ease` | `background-color var(--dur-chip) var(--ease-out), color var(--dur-chip) var(--ease-out)` |
| `main.css:289-291` | `.nm-input` | `background-color 0.1s ease, border-color 0.1s ease` | `background-color var(--dur-chip) var(--ease-out), border-color var(--dur-chip) var(--ease-out)` |
| `main.css:689-692` | `.plan-blk` | `box-shadow 0.12s ease, transform 0.12s ease, background-color 0.12s ease` | `box-shadow var(--dur-chip) var(--ease-out), transform var(--dur-chip) var(--ease-out), background-color var(--dur-chip) var(--ease-out)` |
| `main.css:799` | `.plan-grip` | `opacity 0.12s ease` | `opacity var(--dur-chip) var(--ease-out)` |
| `main.css:1122-1125` | `.nm-btn-brand` | `background-color 0.1s ease, border-color 0.1s ease, filter 0.1s ease` | 三项各换 `var(--dur-chip) var(--ease-out)` |

**三、三处 `animation:` 简写换成 token**（`animation` 简写里可以直接放 var()）：

```css
/* main.css:978 —— .archive-locate */
  animation: archive-locate-pulse 0.85s var(--ease-out) 2;

/* main.css:1096 —— .hover-tip */
  animation: hover-tip-in var(--dur-tooltip) var(--ease-out);

/* main.css:1235 —— .graph-plaque */
  transition: opacity var(--dur-fade) linear;
```

**四、38 处 Tailwind 工具类统一成 token 类串**。已实测 Tailwind 3.4.19 会把 `duration-[var(--x)]` / `ease-[var(--y)]` 输出在 `.transition-*` **之后**（同特异性，后来者胜），所以三件套可直接覆盖 Tailwind 默认值：

```tsx
/* 改前 */
className="rounded-md px-2 py-0.5 text-xs text-[var(--t4)] transition-colors hover:bg-[var(--hover-bg)] hover:text-[var(--t2)]"

/* 改后 */
className="rounded-md px-2 py-0.5 text-xs text-[var(--t4)] transition-colors duration-[var(--dur-chip)] ease-[var(--ease-out)] hover:bg-[var(--hover-bg)] hover:text-[var(--t2)]"
```

38 处的完整清单（`file:line`），逐条改：

```
src/App.tsx:162
src/components/AppsPage/AppsPage.tsx:475
src/components/ArchivePage.tsx:126
src/components/ArchivePage.tsx:149
src/components/ChatPanel/InputArea.tsx:320
src/components/ChatPanel/InputArea.tsx:338
src/components/ChatPanel/InputArea.tsx:405
src/components/ChatPanel/InputArea.tsx:470
src/components/ChatPanel/InputArea.tsx:523
src/components/ChatPanel/InputArea.tsx:532
src/components/ChatPanel/SessionRail.tsx:109
src/components/CommandPalette.tsx:187
src/components/FoldToggle.tsx:20
src/components/GraphPage/GraphPage.tsx:366
src/components/HomePage/BingCard.tsx:78
src/components/HomePage/DayGridMini.tsx:75
src/components/HomePage/DayGridMini.tsx:93
src/components/HomePage/GraphMini.tsx:142
src/components/HomePage/HomePanels.tsx:43
src/components/HomePage/HomePanels.tsx:67
src/components/HomePage/HomePanels.tsx:212
src/components/HomePage/PoolPager.tsx:88
src/components/HomePage/PoolPager.tsx:98
src/components/HomePage/PoolPager.tsx:108
src/components/HomePage/PoolPager.tsx:144
src/components/NotificationsPage/NotificationsPage.tsx:400
src/components/SettingsPage/ApiProviderSelect.tsx:100
src/components/SettingsPage/AvatarPicker.tsx:171
src/components/SettingsPage/AvatarPicker.tsx:191
src/components/SettingsPage/SettingsPage.tsx:1969
src/components/SettingsPage/SettingsPage.tsx:2812
src/components/TodoCard/TodoCard.tsx:260
src/components/Toggle/Toggle.tsx:42
src/components/Toggle/Toggle.tsx:52
src/components/WorkspacePage.tsx:350
src/components/task/SubtaskRow.tsx:109
src/components/task/SubtaskRow.tsx:121
src/components/task/SubtaskRow.tsx:133
```

**类名选型规则**（按该处过渡的属性性质选，不是按行号）：

- 过渡的是颜色/背景/边框（`transition-colors`）→ `duration-[var(--dur-chip)] ease-[var(--ease-out)]`
- 过渡的是透明度（`transition-opacity`）→ 同上（hover 浮现的控件是 chip 级反馈）
- 过渡的是位移/旋转（`transition-transform`：`Toggle.tsx:52` 的开关旋钮、`ArchivePage.tsx:126` 的 `group-open:rotate-90`、`ApiProviderSelect.tsx:100` 的下拉箭头）→ `duration-[var(--dur-chip)] ease-[var(--ease-out)]`
- `AvatarPicker.tsx:171` 的 `transition-[box-shadow]` → `duration-[var(--dur-chip)] ease-[var(--ease-out)]`

**五、把 token 层锁进测试**（`src/ui/main.css.test.ts` 已有静态结构锁的先例，照它的写法加）。

## Repo conventions to follow

- **token 放 `src/ui/main.css` 的 `:root` 块**（第 11–53 行），每组 token 前用 `/* … */` 注释块说明用途与口径。参照现有的圆角/字号阶梯写法：

  ```css
    /* 圆角梯度与字号阶梯（新组件取 var；存量 Tailwind 圆角与字号类不动） */
    --r-sm: 8px;
    --r-md: 12px;
  ```

- **组件类放 `@layer components`**，用 `var(--token)` 引用，不写字面量。
- **CSS 变量 + Tailwind 任意值的写法仓库已经在用**：`rounded-[var(--r-sm)]`、`bg-[var(--hover-bg)]`、`text-[var(--t3)]`——`duration-[var(--dur-chip)]` / `ease-[var(--ease-out)]` 是同一套写法。
- **纪律注释在 `src/ui/main.css:157-158`**，改时长前先读它，改完保持一致。
- **静态锁测试的写法参照 `src/ui/main.css.test.ts`**：它用 `readFileSync("src/ui/main.css", "utf-8")` 读源文件（vitest 配置 `css: false` 会把 CSS 导入吞成空串），再用 `expect(css).toMatch(...)` 断言结构。

## Steps

1. 在 `src/ui/main.css` 的 `:root` 块末尾（`--fs-lg: 16px;` 之后）加入上面第 Target 节给的动效 token 块（含注释）。
2. 按 Target 的逐条对照表，把 main.css 里 10 条 `transition:` 声明换成 token 引用。
3. 把 `main.css:978`、`:1096`、`:1235` 三处 `animation:` / `transition:` 换成 token 引用。
4. 按上面 38 条清单，把 tsx 里的 `transition-*` 类串改成 `transition-* duration-[var(--dur-chip)] ease-[var(--ease-out)]`。每改一处，确认原有 `duration-100` / `duration-150` 已被新类串取代（不要两个 duration 类并存）。
5. 扩展 `src/ui/main.css.test.ts`，加三组断言（见 Verification 的「机械」节）。
6. 跑一遍验证（见下）。提交信息建议：`refactor(ui): 动效曲线与时长收归 token 层`。

## Boundaries

- **不要改** `src/components/TaskTimeline/TaskPool.tsx` 的内联 `transitionDuration` / `transitionTimingFunction`（第 116–124 行）——那是计划 002 的地盘。
- **不要改** `src/components/HomePage/HomeHeader.tsx` 的 framer-motion 接线——计划 003 的地盘。
- **不要改** `src/ui/main.css:1013-1015` 的 `.plan-fresh` 与 `@keyframes plan-pop`——计划 005 的地盘（它会同时改用 token）。
- **不要改** `src/components/HomePage/WeeklyChart.tsx` 的 recharts Tooltip——计划 004 的地盘。
- **注意两个后续计划会覆盖本计划在这两处的产出**（执行顺序见 `plans/README.md`）：
  - `src/components/CommandPalette.tsx:187` —— 计划 017 会把这一行的过渡类**整组删除**（⌘K 面板不该有键盘高亮过渡）。本计划照常转换即可，**017 必须在 001 之后执行**。
  - `src/components/ChatPanel/InputArea.tsx:530-536`（发送键）—— 计划 009 会把它改成 `transition-[opacity,transform] … active:scale-[0.97]`。本计划照常转换即可，**009 必须在 001 之后执行**。
- **不要动** `tailwind.config.js` 的 `theme.extend`：token 归 `main.css` 是本仓库的既有约定（颜色/圆角/字号都这么放的），不要新开第二条 token 来源。
- **不要改任何布局/结构/文案**——只改过渡属性与类名。
- **不要新增依赖**。
- 如果某处代码与本计划描述的现状对不上（行号漂移、类名不同），**停下来报告**，不要即兴发挥。

## Verification

- **机械**：
  ```bash
  npx tsc --noEmit                                   # 零错
  npx oxlint src                                     # 零报
  npm test                                           # 全绿（含新增断言）
  npx tailwindcss -i src/ui/main.css -o /tmp/tw-check.css --content "./src/**/*.{ts,tsx}"
  ```
  断言在 `src/ui/main.css.test.ts` 里加：

  ```ts
  describe("动效 token 层（main.css）", () => {
    it("定义了动效曲线与时长 token", () => {
      expect(css).toMatch(/--ease-out:\s*cubic-bezier\(0\.23,\s*1,\s*0\.32,\s*1\)/);
      expect(css).toMatch(/--ease-in-out:\s*cubic-bezier\(0\.77,\s*0,\s*0\.175,\s*1\)/);
      expect(css).toMatch(/--ease-drawer:\s*cubic-bezier\(0\.32,\s*0\.72,\s*0,\s*1\)/);
      expect(css).toMatch(/--dur-chip:\s*100ms/);
      expect(css).toMatch(/--dur-card:\s*150ms/);
    });

    it("main.css 里不再有手写 transition 时长/曲线字面量", () => {
      // 只允许 token 形式；裸 0.1s ease / 0.15s ease 一律不留
      expect(css).not.toMatch(/transition:[^;]*\b0\.\d+s ease/);
    });

    it("tsx 里的 transition-* 工具类都带 token 时长与曲线", () => {
      const tsx = listFiles("src").filter((f) => f.endsWith(".tsx") && !f.includes(".test."));
      const offenders: string[] = [];
      for (const f of tsx) {
        const src = readFileSync(f, "utf-8");
        for (const m of src.matchAll(/transition-(?:colors|opacity|transform|\[[^\]]+\])(?![^\s"'`]*duration-)/g)) {
          offenders.push(`${f}: ${m[0]}`);
        }
      }
      expect(offenders).toEqual([]);
    });
  });
  ```

  `listFiles` 用 `node:fs` 的 `readdirSync(..., { recursive: true })` 或 `globSync` 实现即可（vitest 环境可用 `node:fs`；若 `recursive` 选项不可用，改成手写递归）。

  再确认编译产物里覆盖链成立：

  ```bash
  grep -n "^\.transition-colors\|^\.duration-\\\[var\|^\.ease-\\\[var" /tmp/tw-check.css
  ```
  期望 `duration-[var(...)]` 与 `ease-[var(...)]` 的行号**大于** `.transition-colors` 的行号（否则 Tailwind 默认值会盖住 token）。

- **手感自检**：
  - 打开首页、设置页、聊天页、任务页，依次 hover：卡片（`nm-card`）、按钮（`nm-btn`）、图标按钮（`nm-icon-btn`）、芯片（首页 `PoolPager` 的 ‹ ›、设置页的 tab pill）、输入框（`nm-input`）。
  - 期望：这五处 hover 的「响应快慢」听起来/看起来是**同一个**；不存在某个控件颜色变化明显慢半拍。
  - 在 DevTools → Animations 面板把 playback 调到 10%，确认 hover 过渡的属性是 `background-color` / `border-color` / `color`（不是 `all`），且时长是 100ms 或 150ms。
  - 切换深浅主题，body 背景与文字颜色过渡仍在（150ms），没有瞬间跳变。
  - 逐个点开 38 处清单里每个组件，确认它的过渡没有变慢或变快到失手（尤其 `Toggle.tsx:52` 的开关旋钮、`ArchivePage.tsx:126` 的折叠箭头、`ApiProviderSelect.tsx:100` 的下拉箭头）。
- **完成判据**：
  - `grep -rn "transition-colors" src/ --include=*.tsx | grep -v duration-` 返回空。
  - `grep -n "0\.[0-9]*s ease" src/ui/main.css` 返回空（除 `@keyframes` 内部不涉及）。
  - 上面三条测试全绿。

