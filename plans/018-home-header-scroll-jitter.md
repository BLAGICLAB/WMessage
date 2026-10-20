# 018 — 首页吸顶头部滚动收缩时抖动（Windows 尤其明显）

- **Status**: TODO
- **Commit**: b22387c7
- **Severity**: HIGH
- **Category**: §4 Interruptibility / §5 Performance
- **Estimated scope**: 诊断为主；修复分三档，最小档 2 行（`main.css` 1 条 + `HomeHeader.tsx` 1 处）

> **本计划记录的是一条用户报告的真 bug**（不是审计里的规则违反），来源：用户在 Windows 上观察到「首页高吸头，轮动缩放时会抖动，Windows 执行时特别明显」。
> **它与计划 003 是同一段代码的两个不同症状**（003 讲布局属性带来的性能成本，本条讲反馈环带来的抖动），**018 必须先于 003 执行**——003 会重写头部内部结构。

## Problem

首页吸顶收缩头部在滚动收缩的过程中会**抖动**（画面来回晃），Windows 上尤其明显，macOS 上轻得多或看不到。

### 代码事实

**一、头部是滚动流里的 `sticky` 元素，高度每帧都在变。**

```tsx
/* src/components/HomePage/HomeHeader.tsx:185 */
    <motion.header className="sticky top-[-24px] -mt-6 z-20 -mx-6 bg-[var(--bg)] px-6 pt-6 pb-2.5">
```

`sticky` 元素**仍然占据文档流里的空间**（它只是被视觉上偏移）。所以头部高度一变，**它下面的内容就整体上移**。

而头部的高度每滚动一帧都在变——`HomeHeader.tsx:153-170` 声明了 4 条 `height` 轨道（`subHeight` / `barHeight` / `labelHeight` / `bandHeight`）加 3 条 `fontSize` 轨道，全部由滚动进度驱动。

**二、进度来自 `scrollY`，而 `scrollY` 会被浏览器的滚动锚定改写。**

```tsx
/* src/components/HomePage/HomeHeader.tsx:146-151 */
  const reduceMotion = useReducedMotion();
  const { scrollY } = useScroll({ container: scrollRef });
  const raw = useTransform(scrollY, [0, COLLAPSE_PX], [0, 1]);
  const spring = useSpring(raw, { stiffness: 320, damping: 34 });
  // 减少动效：跳过 spring，跟随滚动即时到值（still 连续可逆，只是不缓动）
  const progress = reduceMotion ? raw : spring;
```

滚动容器：

```tsx
/* src/App.tsx:992 */
        <main ref={mainRef} className="min-w-0 flex-1 overflow-y-auto p-6">
```

**三、Chromium 会用「滚动锚定」自动补偿内容上移，而补偿的方式是改 `scrollTop`。**

MDN 的说法（`overflow-anchor` 词条）：「Scroll anchoring adjusts the scroll position to compensate for the changes outside of the viewport… 也就是说，用户正在看的那个点在视口里保持不动，这可能意味着他的滚动位置实际上变了。」

于是形成**闭环**：

```
用户滚 1px
  → scrollY +1  → progress +ε  → 头部高度 -ΔH
  → 头部下方内容上移 ΔH（流内位置变了）
  → 滚动锚定把 scrollTop 改成 +1+ΔH（补偿，内容视觉上不动）
  → 下一帧读到的 scrollY 是 1+ΔH，不是 1
  → progress 跳得比预期多  →  头部再缩一点  →  补偿再多一点  →  …
```

**反馈环不稳定 → 抖动。**

**四、为什么 Windows 特别明显。** `overflow-anchor` 的浏览器支持（MDN 兼容表）：

| 浏览器 | 支持 | 起始版本 |
| --- | --- | --- |
| Chrome / Edge / Opera | 完整支持 | Chrome 56 / Edge 79 / Opera 43 |
| Firefox | 完整支持 | 66 |
| **Safari（macOS / iOS）** | **稳定版不支持**（Safari 27.0 才加入） | — |
| WebView on iOS | 不支持 | — |

- **Windows 走 WebView2 = Chromium → 一直有滚动锚定 → 闭环成立 → 抖。**
- **macOS 走 WKWebView → 旧版 WebKit 根本没有这个机制 → 没有闭环 → 不抖**（macOS 26 / Safari 27 起会开始有，届时这个 bug 会在 Mac 上也复现——**所以它是跨平台 bug，Windows 只是先暴露**）。
- 本仓库全仓**从未用过 `overflow-anchor`**（`grep -rn "overflow-anchor" src/` 为空），完全吃浏览器默认。

**五、弹簧本身是欠阻尼的，扰动来了不回稳。**

`stiffness: 320, damping: 34` → 阻尼比 ζ = 34 / (2√320) = **0.9503 < 1**，欠阻尼：任何扰动都会先过冲再回摆，而不是落定。临界阻尼需要 `damping = 2√320 = 35.78`。即使没有锚定环，光是用户滚轮的离散输入 + 欠阻尼弹簧，也会产生一点来回晃。

**六、字号插值到小数点，Windows 的分数 DPI 下光栅化会跟着抖。**

`titleSize` 24→**14.5**、`kpiBigSize` 36→**15.5**——每帧都在改字号 → 文本每帧重新整形（re-shape）→ 标题的**宽度**也在变 → 同一行里的日期 chip 跟着横向挪 → 加上 Windows 125%/150% 缩放下小数像素的光栅化差异，文字本身就会「呼吸」。

### 尚未确认的部分

**根因已在本地 Chromium 确认**（见文末「实测」一节）；**尚未在 Windows 真 app 上确认**——但用户报告的「Windows 抖、Mac 不抖」本身就是这个机制的独立佐证（Chromium 有滚动锚定，旧 WebKit 没有）。

## Target

> **诊断已完成——根因已确认，见文末「实测」一节。** 本地 Chromium 上的同构复现件测出确定数据：滚动锚定把 scrollTop 往回拉约 17px，形成**自持振荡的极限环**（progress 反复倒退 9 次）；关掉锚定后 `progress = scrollTop/150` 精确成立、零倒退。
>
> 所以：**第 2 档是主修法**（无条件做），**第 3 档降级为稳健改进**（做，但不是根因），**第 4 档撤销**（第 2 档已达成它想要的目标）。

### 第 1 步：诊断（已完成，结果见「实测」节）

在 Windows 上打开首页，DevTools → Elements 选中 `<main>` → Styles 面板加一条：

```css
overflow-anchor: none;
```

然后慢慢滚过头部收缩的那 150px，看抖动：

| 现象 | 结论 | 走哪档 |
| --- | --- | --- |
| **抖动消失**（预期，本地已验证） | 滚动锚定闭环是主因 | 第 2 档 |
| 抖动还在，但变成「一次到位不再晃」 | 闭环是主因，剩下的是弹簧回摆 | 第 2 档 + 第 3 档 |
| 抖动完全没变化 | 与锚定无关 | 停下来报告，回看本文件第 5、6 条（弹簧欠阻尼、字号小数重排） |

> 不想装 Windows 也能做这一步：macOS 上 Safari 27+ 已有滚动锚定，同一条 DevTools 规则能复现；没复现就说明这台 Mac 的 WebKit 还没这个机制，用 Windows 验。

**在真 app 上这一步已从「探案」降级为「确认」**：机制层面（滚动锚定 + 高度变化 + scrollY 反馈）与复现件完全一致，结论把握很高。若真 app 上关掉锚定抖动仍在，**停下来报告**，不要硬改。

### 第 2 档：切断锚定闭环

给滚动容器关掉滚动锚定。**加在 `main` 上，不是加在头部上**——头部不是锚定节点的祖先，只在头部上关不够。

CSS（加进 `src/ui/main.css` 的 `@layer components` 块）：

```css
  /* 主滚动容器（App.tsx 的 <main class="overflow-y-auto">）：
     关掉滚动锚定。吸顶头部的高度由滚动进度驱动、每帧都在变，
     头部下方内容因此每帧上移；Chromium 的滚动锚定会自动改 scrollTop
     补偿这个上移（而进度又来自 scrollY），形成反馈环 → 抖动。
     实测（本地 Chromium 同构复现件，详见 plans/018 的「实测」节）：
     开着锚定时 drift 恒为 −17px、progress 反复倒退 9 次（自持振荡）；
     关掉后 progress = scrollTop/150 精确成立、零倒退。
     锚定是浏览器替作者兜底的通用行为，这里由应用自己接管滚动语义。 */
  .app-scroll {
    overflow-anchor: none;
  }
```

TSX（`src/App.tsx:992`）：

```tsx
        <main ref={mainRef} className="app-scroll min-w-0 flex-1 overflow-y-auto p-6">
```

**副作用评估（已实测澄清）**：关掉锚定后，**用户的滚动不再被吃掉**——原本设 60px 实际只走 44px，现在精确走 60px。头部下方内容让位给收缩的头部，是收缩头部的**正常语义**，**不是失控的「跑得比滚轮快」**。**无需产品拍板。**

### 第 3 档：让弹簧不回摆（零成本的稳健改进，非根因）

把弹簧改成**临界阻尼**，任何扰动都直接落定，不回摆。

```tsx
/* src/components/HomePage/HomeHeader.tsx:149 —— 当前 */
  const spring = useSpring(raw, { stiffness: 320, damping: 34 });
```

```tsx
/* 目标：damping 36 → ζ = 36/(2√320) = 1.0062（过阻尼，无过冲、无回摆） */
  const spring = useSpring(raw, { stiffness: 320, damping: 36 });
```

`damping: 35.78` 是数学上的临界值，取整到 **36**（略微过阻尼，落定稍稳，不会来回晃）。

**诚实说明**：实测显示，闭环断掉后阻尼比 ζ=0.95 **本身不会产生可见振荡**（探针 D 倒退 0 次）。所以这一档**不是根因修复**，是「不再依赖别的条件恰好成立」的稳健化。**做了更好，不做也不会有抖动。**

**次要嫌疑：字号小数。** 若第 2 档在真 app 上做完仍有残留抖动、且确定与字号重排有关，再考虑把插值端点取整（`24 → 15`、`36 → 15.5 → 16`）。**注意**：取整会让字号在插值过程中出现约 1px 的**台阶式跳变**，比现在的小数插值更难看。**本计划默认不改字号端点**——除非诊断明确指向它。

### 第 4 档：撤销

原计划的第 4 档（把 `progress` 上移、用 transform 显式补偿内容位移）**不做**。理由：第 2 档落地后，内容的屏幕位置已经是滚动位置的纯函数（实测 `progress = scrollTop/150` 精确成立），第 4 档想达成的目标已被达成；而它要动组件边界、算准高度差、和计划 003 强耦合——代价大、收益为零。

## Repo conventions to follow

- **规格已写明这个设计**（`SPEC.md:56`）：「吸顶收缩头部（…滚动进度驱动问候/摘要/完成率进度条与 KPI 收成单行钉顶，spring 平滑且反向连续还原，`prefers-reduced-motion` 时免缓动…）」。**动效形态（滚动驱动 + 弹簧 + 可逆）是既定决策**，本计划只修「实现方式导致的抖动」，不改形态。
- **`reduceMotion` 分支要保留**（`:151` 的 `const progress = reduceMotion ? raw : spring;`）——减少动效时跳过弹簧、跟随滚动即时到值。本计划的弹簧改动**只影响非 reduced-motion 分支**。
- **`main` 的滚动容器身份不要改**（`overflow-y-auto p-6`、负边距配合头部的 `-mt-6`/`top-[-24px]`）——`App.tsx:992` 的 class 是布局口径，改动会影响头部吸附对齐。
- **CSS 类归 `src/ui/main.css`**（仓库约定：颜色/圆角/字号/动效 token 都放那儿），不在 `App.tsx` 里写内联样式。

## Steps

1. **第 2 档（主修法，先做）**：
   - 在 `src/ui/main.css` 的 `@layer components` 块加 `.app-scroll { overflow-anchor: none; }`（带上面那段注释，含实测数据）。
   - 改 `src/App.tsx:992`，给 `<main>` 的 class 加上 `app-scroll`。
2. **第 3 档（稳健化）**：改 `src/components/HomePage/HomeHeader.tsx:149` 的弹簧阻尼为 `damping: 36`。
3. **Windows 真 app 上确认**（30 秒）：DevTools → `<main>` 看不到 `overflow-anchor` 之外的异常；滚一遍确认抖动消失。**这是「确认」不是「探案」**——根因已在本地 Chromium 的同构复现件上确认（见「实测」节）。若真 app 上抖动仍在，**停下来报告**，不要硬改。
4. **记录观感**：滚动收缩期间，下方内容让位给收缩的头部是否自然（这本来就是收缩头部的语义）。
5. 跑验证（见下）。**若在真 app 上关掉锚定后抖动仍在**，停下来报告并附上「抖动仍在 / 锚定已关」的事实——那说明还有别的机制（字号小数重排、framer-motion 的 scroll 测量），不要硬改第 3、4 档来凑。

## Boundaries

- **不要改** `useScroll` / `useTransform` 的接线（`:147-148`）、`COLLAPSE_PX = 150`、进度的映射区间。
- **不要改** `reduceMotion` 分支的行为。
- **不要改** 头部的 sticky 定位与负边距（`:185`）。
- **不要改** 字号插值端点（`24 → 14.5`、`36 → 15.5`、`26 → 16`）——除非诊断明确指向它，且改了必须说明台阶跳变的观感变化。
- **不要动** `src/components/HomePage/` 之外的任何文件（`App.tsx` 的 `<main>` class 除外）。
- **不要做**原计划的第 4 档（`progress` 上移 + transform 补偿）——已撤销，理由见 Target 节。
- **不要动** 计划 003 的产物——**018 必须先于 003**（003 会重写头部内部结构，两者改同一段代码）。
- 不要新增依赖。

## Verification

- **机械**：
  ```bash
  npx tsc --noEmit      # 零错
  npx oxlint src        # 零报
  npm test              # 全绿
  ```
  加两条静态锁（加进 `src/components/HomePage/HomePage.test.tsx`）：

  ```ts
  it("吸顶头部弹簧是临界/过阻尼（不回摆）", () => {
    const src = readFileSync("src/components/HomePage/HomeHeader.tsx", "utf-8");
    const m = src.match(/useSpring\(raw,\s*\{[^}]*stiffness:\s*(\d+)[^}]*damping:\s*(\d+)/);
    expect(m).not.toBeNull();
    const [, k, c] = m!;
    // 阻尼比 ζ = c / (2√k) 必须 ≥ 1，否则欠阻尼会回摆
    expect(Number(c) / (2 * Math.sqrt(Number(k)))).toBeGreaterThanOrEqual(1);
  });
  ```

  ```ts
  it("主滚动容器关掉了滚动锚定（避免吸顶头部高度变化形成反馈环）", () => {
    const css = readFileSync("src/ui/main.css", "utf-8");
    expect(css).toMatch(/\.app-scroll\s*\{[^}]*overflow-anchor:\s*none/);
    const app = readFileSync("src/App.tsx", "utf-8");
    expect(app).toMatch(/<main[^>]*className="[^"]*app-scroll/);
  });
  ```

- **手感自检（**在 Windows 上做，这是本计划的主战场**）**：
  - 打开首页，**用滚轮慢慢滚过**头部收缩的那 150px，眼睛盯住头部与下方内容的交界：不能有来回晃、不能有高频闪动。
  - 再**快速滚一下**（滚轮一格一格往下拨）：头部收缩过程必须**平滑单向**，不能反弹。
  - **反向滚回顶部**：收缩/展开必须连续可逆，不能有卡顿或二次抖动。
  - 盯住标题右侧的**日期 chip**：它在第一行里，字号变小时 chip 的横向位置不能左右来回跳（字号重排引起的横向漂移）。
  - 盯住标题**文字本身**：不能有「呼吸感」的忽大忽小。
  - 关掉「减少动效」再验一遍（`prefers-reduced-motion: reduce` 时走 `raw` 不走弹簧，**本来就不该抖**——如果 reduced-motion 下也抖，说明问题不在弹簧，去查锚定）。
  - DevTools → Rendering → `Emulate prefers-reduced-motion: reduce` 对照一次。
  - 其它页面（任务页、设置页、聊天页）**不能被牵连**：`.app-scroll` 只在主容器上，但确认一下。
- **完成判据**：
  - Windows 上滚动收缩过程**肉眼无抖动**。
  - 诊断结论写在本文件的「实测」一节里。
  - 上面两条测试全绿。

## 实测

**根因已确认（2026-10-20，在本地 Chromium/Electron 里用同构复现件验证）。**

复现件与 app 同构：同样的 `sticky top-[-24px] -mt-6` 头部、同样的 4 条 `height` 轨道 + 3 条 `fontSize` 轨道、同样的 `stiffness 320 / damping 34` 弹簧、同样的 `overflow-y-auto` 滚动容器。探针用**同步强制布局**（写样式后读 `offsetHeight` 逼浏览器结算布局，锚定补偿就在布局阶段发生），不依赖 rAF。

### 探针 A/B：等步进滚动（设定 scrollTop，等步进，记录实际值）

| 设定 | 锚定开启：实际 / drift | `overflow-anchor: none`：实际 / drift |
| --- | --- | --- |
| 20 | 10.5 / **−9.5** | 20 / 0 |
| 40 | 26 / **−14** | 40 / 0 |
| 60 | 44 / **−16** | 60 / 0 |
| 80 | 63.5 / **−16.5** | 80 / 0 |
| 100 | 83 / **−17** | 100 / 0 |
| 120 | 102 / **−18** | 120 / 0 |
| 140 | 122.5 / **−17.5** | 140 / 0 |
| 160 | 142.5 / **−17.5** | 160 / 0 |

**drift 恒为负且稳定在 −16 ~ −18px**：浏览器把 scrollTop 往回拉了。用户滚 60px，实际只滚了 44px。

### 探针 C/D：设 scrollTop=60 后**放手不动** 120 帧

| | 锚定开启 | `overflow-anchor: none` |
| --- | --- | --- |
| 末 scrollTop | **32**（设定 60，自己掉了 28） | 60（纹丝不动） |
| 末 progress | **0.2133**（期望 0.4） | 0.4（精确） |
| progress 倒退次数 | **9 次** | **0 次** |
| progress 轨迹 | `0.0191 0.2085 0.2183 0.2133 0.2129 0.2132 0.2133 …` | 单调收敛 |

### 结论

**`ROOT-CAUSE-CONFIRMED`。**

- 滚动锚定**确实在改写 scrollTop**（drift ≈ −17px），形成闭环。
- 闭环不是发散，而是收敛到**一个自持振荡的极限环**：progress 冲过 0.2183 又回摆到 0.2133，**反复 9 次**。
- 这精确对应用户看到的现象——是**抖动**（小幅高频来回颤），不是跳动或乱飞。
- 关掉锚定后，`progress = scrollTop / 150` **精确成立**（0/0.133/0.267/0.4/0.533/0.667/0.8/0.933/1），零漂移、零倒退。**第 2 档的修法成立。**

### 对原分析的修正

1. **原来说「滚动被放大」不准确**——实测是滚动被**吃掉**（drift 为负）。方向反了，但闭环本身成立。
2. **原来说「关掉锚定后内容会跑得比滚轮快」也不准确**——关掉之后进度与滚动位置精确对应，下方内容让位给收缩的头部是收缩头部的**正常语义**（不是失控）。这个副作用不成立，第 2 档没有需要产品拍板的观感代价。
3. **第 3 档（弹簧阻尼）从「可能有用」降级为「零成本的稳健改进，不是根因」**——探针 D 里阻尼仍是 ζ=0.95（`damping: 34`），但闭环断掉后**倒退 0 次**，说明单独的欠阻尼不会产生可见振荡。仍建议做（临界阻尼落定更稳、不依赖其它条件），但它**不能替代第 2 档**。
4. **第 4 档（显式 transform 补偿）可以不做**——第 2 档已经让「内容的屏幕位置 = 滚动位置的纯函数」，即第 4 档想达成的目标。**第 2 档 + 第 3 档足够，第 4 档撤销。**

### 仍需在真 app 上确认的部分

复现件是**同构模拟**，不是真 app。两处差异：

1. 真 app 用 framer-motion 的 `useScroll({ container })`，它对容器滚动的测量/校正可能与 `scroller.scrollTop` 直读略有差异。
2. 真 app 的 `HomeHeader` 用 `useSpring`（framer-motion 内置），与复现件的手写积分在数值上略有差异。

**结论的把握**：机制层面（滚动锚定 + 高度变化 + scrollY 反馈）两边完全一致，且**用户报告的「Windows 抖、Mac 不抖」本身就是这个机制的独立佐证**（Chromium 有锚定、旧 WebKit 没有）。把握很高，但**落地前仍需在 Windows 真 app 上跑一次 30 秒确认**——也就是本计划第 1 步的诊断，只是它已经从「探案」降级为「确认」。


