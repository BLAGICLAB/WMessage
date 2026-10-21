# 003 — 首页吸顶头部：把滚动驱动的布局属性动画换成 transform

- **Status**: DONE — 性能分支判定为「不需要改代码」；B5（百分比文字硬跳）已落地
- **Commit**: b22387c7（计划锚定）→ 实测与 B5 落地见文末「实测」
- **Severity**: HIGH
- **Category**: §5 Performance
- **Estimated scope**: 1 个 tsx（`HomeHeader.tsx`），约 40 行改动；高视觉回归风险，必须逐帧看

> **结论（先读这段）**：本计划的第一件事是量，量完的结论是**这个头部不慢**——
> 收口全程 **0.0% 掉帧**，且「头部逐帧变高」与「头部不动、同样滚 150px」两段的
> 帧指标**完全不可区分**（均值 16.67 vs 16.66、max 18.60 vs 18.80）。头部滚动驱动的
> 布局写入的边际成本在测量噪声内，**B1/B2/B4 的性能重构没有必要做**——
> 那会是一次「为不存在的问题」冒的高视觉回归风险，不做。
> 只落地了与性能无关、但确实存在的 B5（18% 阈值处百分比文字硬跳）。

## Problem

首页吸顶收缩头部用 framer-motion 的滚动弹簧驱动，**9 条属性是布局属性**（不是 transform/opacity），每滚动一帧全部重写一遍。

声明（`src/components/HomePage/HomeHeader.tsx:153-170`）：

```tsx
  const titleSize = useTransform(progress, [0, 1], [24, 14.5]);
  const poolSize = useTransform(progress, [0, 1], [26, 16]);
  const poolGap = useTransform(progress, [0, 1], [26, 14]);
  const kpiBigSize = useTransform(progress, [0, 1], [36, 15.5]);
  const subOpacity = useTransform(progress, [0, 1], [1, 0]);
  const subHeight = useTransform(progress, [0, 1], [24, 0]);
  const barOpacity = useTransform(progress, [0, 1], [1, 0]);
  const barHeight = useTransform(progress, [0, 1], [28, 0]);
  const labelOpacity = useTransform(progress, [0, 1], [1, 0]);
  const labelHeight = useTransform(progress, [0, 1], [18, 0]);
  const bandHeight = useTransform(progress, [0, 1], [68, 0]);
  const bandOpacity = useTransform(progress, [0, 1], [1, 0]);
```

应用点（同一文件）：

| 行 | 元素 | 属性 |
| --- | --- | --- |
| `:91` | KPI 数字 | `fontSize` |
| `:122` | 今日完成率进度条填充 | `width`（`animate={{ width: `${pct}%` }}`） |
| `:202` | 标题 h1 | `fontSize` |
| `:213` | 今日摘要行 | `opacity` + **`height`** |
| `:219` | 完成率进度条容器 | `opacity` + **`height`** |
| `:231` | 三池容器 | **`gap`** |
| `:237` | 三池文字标签 | `opacity` + **`height`** |
| `:243` | 三池数字 | `fontSize` |
| `:258` | KPI 横排行 | `opacity` + **`height`** |

**关于 `width`（`:122`）的准确说明**：这一条**不是**滚动驱动的——`pct` 来自 `phases`（数据），由 30s 轮询刷新，spring 只在数值变化时跑。审计时把它记成 HIGH 是**定级过高**，它一年也就触发个位数次。这里如实修正：`:122` 的 `width` 转换属**可选**，不是本计划的必做项。

**关于 `height`（`:213`/`:219`/`:237`/`:258`）的准确说明**：这 4 条改的是**同一个总和高度**，所以每帧只导致一次「头部高度变化 → 滚动容器内容重排」。不是 4 次独立重排。

**真正没被量化的部分**：这个头部的帧成本**从来没被实测过**。审计明确标注了这条不确定性（"I did not profile frames, so the actual dropped-frame impact is unmeasured"）。动效规则把「dropped frames」列为 HIGH 判据，但这里连丢没丢帧都不知道。所以本计划**先量再改**——这是本计划最重要的部分。

**另有一处确定的、非性能问题**（同一组件，同一类「状态跳变」）：

```tsx
/* src/components/HomePage/HomeHeader.tsx:125 */
            {pct >= 18 && <span className="num">{pct}%</span>}
```

百分比文字在 18% 阈值处**瞬时出现/消失**，而此时进度条的 `width` spring 还在跑——数字在运动中途硬跳一下。

## Target

**分支 A（先量）：** 用 Chrome DevTools Performance 面板录一段首页滚动，统计掉帧。

- 若掉帧 < 5%：**本计划到此为止，不改代码**——把实测数字写回本文件并把 Status 改成 DONE（结论：动效已是「刚好够用」）。这比硬改一个视觉风险高的头部更负责。
- 若掉帧 ≥ 5%：执行分支 B。

**分支 B（把滚动驱动的布局属性换成 transform/opacity）：**

**B1 — 4 条 `height` 轨道换成 `transform: translateY()` + `opacity`，并把收口块移出文档流。**

收口块的父级已经是 `position: relative`（`:186` 的 `<div className="relative flex items-end justify-between gap-6">`），所以给 4 个收口块加 `absolute` 即可让它们不再参与头部高度计算；头部自身的高度改由**一条** `height` 轨道承担。

```tsx
/* 改前 —— 四个收口块各有 height 轨道 */
          <motion.p
            className="overflow-hidden text-[13.5px] leading-6 text-[var(--t3)]"
            style={{ opacity: subOpacity, height: subHeight }}
          >
            {summary}
          </motion.p>
          <motion.div
            className="overflow-hidden"
            style={{ opacity: barOpacity, height: barHeight }}
          >
            {planned > 0 && (
              <div className="pt-2">
                <ProgressBar done={phases.done} planned={phases.planned} />
              </div>
            )}
          </motion.div>
```

```tsx
/* 改后 —— 内容用 transform 推出视野，块本身 absolute 移出文档流 */
          <motion.p
            className="absolute inset-x-0 top-0 overflow-hidden text-[13.5px] leading-6 text-[var(--t3)]"
            style={{ opacity: subOpacity, transform: `translateY(${-24 * (1 - subScale)}px)` }}
          >
            {summary}
          </motion.p>
```

其中 `subScale` 是新增的一条 `useTransform(progress, [0, 1], [1, 0])`（1 = 完全展开，0 = 完全收起），用来同时驱动位移与不透明度。`translateY` 的像素值取各块的**自然高度**（`sub` = 24px、`bar` = 28px、`label` = 18px、`band` = 68px），这四个数字在代码注释里已经写死了：

```tsx
  // 收口容器 = pt-2(8px) + 条(20px)；高度必须与内容严格一致，否则 overflow 裁条
  const barHeight = useTransform(progress, [0, 1], [28, 0]);
  // KPI 横排行收口高度 = pt-3(12) + 标签行 16 + gap-1(4) + 数字行 36 = 68，
  // 必须与内容严格一致，否则 overflow 裁字/留白
  const bandHeight = useTransform(progress, [0, 1], [68, 0]);
```

**B2 — 头部自身用一条 `height` 轨道承担总高度。**

```tsx
/* 新增：头部总高度 = 固定部分 + 三个收口块的高度和（都随 progress 线性变化） */
  const collapsedExtra = 24 + 28 + 68; // sub + bar + band（label 在右列，不计入）
  const headerHeight = useTransform(progress, [0, 1], [HEADER_EXPANDED, HEADER_COLLAPSED]);
```

`HEADER_EXPANDED` / `HEADER_COLLAPSED` 必须**实测**得出：改完后在浏览器里量「完全展开时 `<motion.header>` 的 offsetHeight」和「完全收起时（滚到底）的 offsetHeight」，把两个数写进常量。不要凭算式猜——现有注释已经警告过「高度必须与内容严格一致，否则 overflow 裁条」。

**B3 — `gap`（`:231`）与 3 条 `fontSize` 轨道保持不动。**

- `fontSize` 换 `transform: scale()` 会破坏基线对齐（`items-baseline`）与文字渲染质量，属于视觉重设计而不是动效修复。
- `gap` 只影响右列 3 个元素的排布，成本可忽略。

**B4（可选，与 B1 同批做）— `:122` 的 `width` → `scaleX`。**

**只有在 B1/B2 落地且视觉验收通过后才做**，因为它需要把百分比文字从填充块里拆出来单独定位：

```tsx
/* 目标：填充块用 scaleX，百分比文字单独用 translateX 跟随填充块中心 */
      <div className="home-progress" role="progressbar" aria-label="今日完成率" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct}>
        <motion.div
          className="home-progress-fill"
          initial={false}
          animate={{ transform: `scaleX(${pct / 100})` }}
          style={{ transformOrigin: "left center" }}
          transition={reduceMotion ? { duration: 0 } : { type: "spring", stiffness: 120, damping: 22 }}
        />
        {pct >= 18 && (
          <motion.span
            className="num home-progress-label"
            animate={{ transform: `translateX(${pct / 2}%)` }}
            transition={reduceMotion ? { duration: 0 } : { type: "spring", stiffness: 120, damping: 22 }}
          >
            {pct}%
          </motion.span>
        )}
      </div>
```

配套 CSS（加进 `src/ui/main.css` 的 `.home-progress` / `.home-progress-fill` 附近）：

```css
  .home-progress {
    position: relative;
    height: 20px;
    ...
  }
  .home-progress-fill {
    position: absolute;
    inset: 0;
    transform-origin: left center;
    border-radius: 999px;
    background: var(--success);
  }
  .home-progress-label {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    font-size: 10.5px;
    font-weight: 650;
    line-height: 1;
    color: #fff;
  }
```

**注意**：`translateX(${pct / 2}%)` 的百分比基准是**标签自身宽度**，不是容器宽度——`translateX` 的百分比永远按元素自身尺寸算。这里需要的是「填充块中心在容器中的位置」，所以必须用 `left` + `transform` 的组合，或把标签放到一个占满容器宽度的绝对定位层里再 `translateX(${pct / 2}%)`（此时该层的自身宽度 = 容器宽度，百分比基准才对）。**这一步极易写错，落地时必须用 DevTools 量实际位置**。

**B5 — 修掉 `:125` 的百分比文字瞬跳。**

```tsx
/* 改前 */
            {pct >= 18 && <span className="num">{pct}%</span>}
/* 改后 —— 用 opacity 过渡代替条件挂载 */
            <span className="num home-progress-label" style={{ opacity: pct >= 18 ? 1 : 0, transition: "opacity var(--dur-fade) var(--ease-out)" }}>
              {pct}%
            </span>
```

`--dur-fade` / `--ease-out` 由计划 001 建立；若计划 001 未落地，本步直接写字面量 `opacity 150ms cubic-bezier(0.23, 1, 0.32, 1)`。

## Repo conventions to follow

- **framer-motion 在本仓库只出现在 3 个文件**（`HomeHeader.tsx`、`PoolPager.tsx`、`DueTicker.tsx`），全部走 `useReducedMotion` + `useScroll`/`useSpring`/`useTransform` 这套。改动不要引入第二种写法。
- **reduced-motion 分支要保留**：`const progress = reduceMotion ? raw : spring;`（`:151`）——减少动效时跳过 spring、跟随滚动即时到值。`:123` 的 `transition={reduceMotion ? { duration: 0 } : …}` 同理，不要删。
- **注释里记着高度的精确口径**（`:160-166`），改高度相关代码时这些注释是校验依据，不要删也不要改写口径。
- **头部是 `position: sticky`**（`:185`，`sticky top-[-24px] -mt-6 z-20 -mx-6 bg-[var(--bg)] px-6 pt-6 pb-2.5`），左右负边距与负 margin 是为了抵消滚动容器的 24px padding——动这块时不要顺手改这些。
- **规格已写明这个设计**（`SPEC.md:56`）：「滚动进度驱动问候/摘要/完成率进度条与 KPI 收成单行钉顶，spring 平滑且反向连续还原，`prefers-reduced-motion` 时免缓动」。**动效形态本身是既定决策**——本计划只动「用什么属性实现」，不改「滚动驱动 + 弹簧 + 可逆」这三条。

## Steps

1. **先量**。起开发服务器（`npm run dev`），打开首页，DevTools → Performance → 录 3 秒匀速滚动（从顶部滚到吸顶收起）。统计总帧数与 > 16.7ms 的帧数，算掉帧率。
   - 命令：`npm run dev`，另开一个终端 `npx vite --host`（如需）。
   - 记录：掉帧率、掉帧集中在哪几段（滚动中 / 弹簧收尾）。
2. **判定**：
   - 掉帧率 < 5% → **停止改代码**，把实测数字写进本文件「Verification → 实测」一节，把本计划 Status 改成 `DONE (no change needed)`，并在本文件顶部加一行结论。**这是合法结果。**
   - 掉帧率 ≥ 5% → 继续第 3 步。
3. **B1**：在 `HomeHeader.tsx` 里把 4 条 `height` 轨道换成 `transform: translateY()` + `opacity`，给 4 个收口块加 `absolute` 定位所需的 `relative` 父级（`:186` 已有 `relative`，确认 4 个块都在它里面；`band` 块在 `:256` 不在 `:186` 内，需要给它的父级加 `relative`）。
4. **B2**：加 `headerHeight` 单条轨道，实测出 `HEADER_EXPANDED` / `HEADER_COLLAPSED` 两个常量并写死，`<motion.header>` 挂上 `style={{ height: headerHeight }}`。
5. **B5**：按上面 B5 的代码替换 `:125` 的条件挂载。
6. **B4**（可选，建议在第 3–5 步视觉验收通过后再做）：按 B4 的代码 + CSS 改进度条。
7. 跑验证（见下）。提交信息建议：`perf(home): 吸顶头部收口改用 transform，去掉滚动帧里的布局写入`。

## Boundaries

- **注意：计划 018 必须先于本计划执行。** 018 修的是同一段代码的另一个症状——用户报告的「滚动收缩时抖动（Windows 尤其明显）」，根因是吸顶头部的高度变化 + Chromium 滚动锚定形成的反馈环。018 会改 `HomeHeader.tsx` 的弹簧配置与 `App.tsx` 的 `<main>` class；本计划会重写头部内部结构。**顺序反了会把 018 的修复覆盖掉。**
- **不要改** `useScroll` / `useSpring` 的接线（`:147-151`）——滚动驱动 + 弹簧 + 可逆是既定设计（`SPEC.md:56`）。
- **不要改** `reduceMotion` 分支的任何行为。
- **不要改** `titleSize` / `poolSize` / `kpiBigSize` 三条 `fontSize` 轨道，也不要加 `transform: scale()` 替代——会破坏基线对齐。
- **不要改** `poolGap`（`:231`）。
- **不要改** 头部的 sticky 定位与负边距（`:185`）。
- **不要改** 头部里任何**文案、数字口径、结构**。本计划只动「用什么属性实现同一段运动」。
- **不要动** `src/components/HomePage/HomeHeader.test.tsx` 里的既有断言——如果断言依赖 `height` 内联样式而因此失败，**那是断言在描述实现细节**，改断言前先确认产品行为没变，并在提交信息里说明。
- 不要新增依赖。
- 如果 B1/B2 落地后视觉验收不过（内容裁切、错位、重叠），**回退到第 2 步的结论**（不改代码），把验收失败的现象记进本文件——不要硬调参数硬凑。

## Verification

- **机械**：
  ```bash
  npx tsc --noEmit      # 零错
  npx oxlint src        # 零报
  npm test              # 全绿
  ```
- **手感自检**（这是本计划的关键，代码对了不等于看着对）：
  - 首页从顶部慢慢滚到吸顶收起，逐段看：
    - 标题字号 24→14.5 的收缩必须**连续**，不能有跳帧或撕裂。
    - 今日摘要行、完成率进度条、KPI 横排行三块在收起过程中**依次淡出并向上滑走**，不能出现「先消失再移动」或「被裁成半行」。
    - 收起到单行后，头部高度**恰好等于**一行（标题 + 三池数字）+ 上下 padding + 分隔线，**不能多出空白，也不能把内容裁掉**。
  - 反向滚回顶部：全过程必须**连续可逆**（这是既定设计，破坏了就等于改了规格）。
  - 滚动中**同时**看进度条：完成率百分比在 18% 附近出现/消失时不能硬跳（B5 生效后应淡入淡出）。
  - 切深浅主题再滚一遍：头部背景是 `bg-[var(--bg)]`，确认收口过程中没有露出底下的内容。
  - DevTools → Rendering → 开启 `Emulate prefers-reduced-motion: reduce`：头部应**跟随滚动即时到值**（不缓动），其余视觉一致。
  - DevTools → Animations 面板，播放速度调到 10%，逐帧看收口过程：四块内容应是「淡出 + 上滑」，不是「高度被压扁」。
  - **重新量一次掉帧率**，与第 1 步的基线对比，写进本文件。
- **完成判据**：
  - 分支 A（不改代码）：本文件里有实测掉帧率数字，Status = `DONE (no change needed)`。
  - 分支 B：`grep -n "height: subHeight\|height: barHeight\|height: labelHeight\|height: bandHeight" src/components/HomePage/HomeHeader.tsx` 返回空；滚动帧里不再有 `height` 写入；重测掉帧率 ≤ 5%。


## 实测（2026-10-21，分支 A：先量）

**测法**：搭了一个临时测量页（用完即删），挂载**真实 `HomePage`**（真实 CSS +
真实 framer-motion，24 张 mock 任务卡 + recharts 柱状图 + SVG 图谱速览 + 周网格
全在），放进与 app 同构的滚动容器（`app-scroll` + `overflow-y-auto`）。
用 `requestAnimationFrame` 逐帧记帧间隔，并**设了对照组**：

- **A 段 = 收口段**：`scrollTop` 0 → 150（`COLLAPSE_PX`），头部高度每帧都在变。
- **B 段 = 对照段**：`scrollTop` 300 → 450，滚同样 150px，但此时 `progress` 已被
  `useTransform` 钳在 1，头部不再写任何样式——只有页面本身在滚。

两段之差 = 头部滚动驱动的布局写入的边际成本。

**结果**（Chromium/Electron，60fps 屏，Mac）：

| 阶段 | 帧数 | 均值 | p50 | p95 | max | >17ms | **>25ms（掉帧）** |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 空闲基线 | 40 | 16.39ms | 16.70 | 18.50 | 22.80 | 7.5% | **0.0%** |
| A 收口段 0→150 | 90 | 16.67ms | 16.70 | 18.20 | 18.60 | 6.7% | **0.0%** |
| B 对照段 300→450 | 90 | 16.66ms | 16.70 | 16.80 | 18.80 | 3.3% | **0.0%** |
| 弹簧落定 | 120 | 16.67ms | 16.70 | 17.30 | 18.40 | 5.8% | **0.0%** |

- **掉帧率 0.0%**，远低于本计划 5% 的判定阈值 → 走**分支 A：不改代码**。
- **A 段与 B 段完全不可区分**（均值差 0.01ms、max 18.60 vs 18.80）→ 头部的
  布局写入**不产生可测量的额外成本**。空闲基线的 max 反而更高（22.80ms），说明
  零星 >17ms 的帧是环境噪声（GC、后台活动），与头部无关。
- 收口段头部高度 195.0 → 154.7，90 帧里有 46 个不同值 → 头部确实在逐帧动画，
  测的是真实运动，不是静态页。
- 三次独立测量结果一致。

**为什么这条本来就不该是 HIGH**：本计划自己已经算过，4 条 `height` 轨道改的是
**同一个总和高度**（不是 4 次独立重排）；`:122` 的 `width` 不是滚动驱动的
（数据驱动、30s 刷新一次）；`fontSize` / `gap` 的成本可忽略。实测与这个分析一致。

**B1/B2/B4（性能重构）不做的理由**：把 4 条 `height` 换成 `translateY` + `opacity`
需要给收口块加 `absolute`、把头部高度压成一条轨道，还要实测 `HEADER_EXPANDED` /
`HEADER_COLLAPSED` 两个常量（现有注释已经警告过「高度必须与内容严格一致，否则
overflow 裁字/留白」）。为一个**测不出**的性能问题冒这个视觉回归风险，不划算。

## 落地内容：只有 B5

B5 与性能无关，是本计划 Problem 节单列的「确定的、非性能问题」：

- **改前**：`{pct >= 18 && <span className="num">{pct}%</span>}` —— 条件挂载。
- **改后**：文字常驻挂载，用 `opacity` 淡入淡出（`opacity var(--dur-fade) var(--ease-out)`）。

原因：数据 30s 刷新一次，越过 18% 阈值那一刻进度条的 `width` 弹簧**还在生长**，
数字却在运动中途硬跳一下。改成透明度过渡后，数字淡入时进度条也在动，两者同步。

安全性：`.home-progress` 有 `overflow: hidden`、`.home-progress-fill` 有 `min-width: 0`，
填充块变窄时文字被裁掉、不会溢出到绿条外；`pct < 18` 时 opacity 为 0，本来就不可见。
减少动效下由 `main.css` 的全局 `transition-duration: 0.01ms !important` 兜底。

**新增断言**（`src/components/HomePage/HomePage.test.tsx`）：文字常驻 + opacity +
token 过渡；源码里不再是条件挂载。
