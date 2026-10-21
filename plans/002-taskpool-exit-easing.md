# 002 — 任务池退场曲线改掉 ease-in 慢起

- **Status**: 已完成
- **Commit**: b22387c7（计划锚定）→ 落地于 `92f79eb6`
- **Severity**: HIGH
- **Category**: §2 Easing & duration
- **Estimated scope**: 1 个 tsx（+ 可能 1 处 CSS token），约 8 行改动

## Problem

任务池（`TaskPool.tsx`）的滑出/让位用的是一条 **ease-in 家族曲线**：两个 y 控制点都小于 1（`y1=0`、`y2=0.15`），所以动画开头极慢、末尾才窜出去。

```tsx
/* src/components/TaskTimeline/TaskPool.tsx:116-124 —— 当前 */
      style={{
        transform:
          hidden || evaded ? "translateX(calc(100% + 16px))" : "translateX(0)",
        transitionProperty: "transform, opacity",
        transitionDuration: hidden || evaded ? "150ms" : "240ms",
        transitionTimingFunction: hidden || evaded
          ? "cubic-bezier(0.3, 0, 0.8, 0.15)"
          : "cubic-bezier(0.05, 0.7, 0.1, 1)",
      }}
```

按 cubic-bezier 求位移进度，这条曲线的推进速度是：

| 曲线 | 25% 时间 | 50% 时间 | 75% 时间 |
| --- | --- | --- | --- |
| 现退场 `cubic-bezier(0.3, 0, 0.8, 0.15)` | **3.5%** | **15.4%** | 40.6% |
| 现入场 `cubic-bezier(0.05, 0.7, 0.1, 1)` | 83.2% | 95.0% | 99.1% |
| 目标 `--ease-out` `cubic-bezier(0.23, 1, 0.32, 1)` | 77.5% | 96.6% | 99.7% |

也就是说：**退场的前 40% 时间里，池子几乎没动**（只滑了 15% 的路），然后突然窜出去。看起来像卡了一下。

同一个组件的两个方向差了一个数量级（3.5% vs 83.2%），所以「滑出」和「滑回」像两个不同的物体。

触发路径有两条，都很频繁：

1. **拖拽让位**（`evaded`）：从池里把任务块拖到周网格时，整池要滑开让出视野；指针回到池上时又滑回。拖拽是连续手势，池子每帧都在等用户——延迟启动的退场正好卡在这个最需要即时反馈的时刻。
2. **折叠任务池**（`hidden`）：右上角 `⇥` 按钮，一天点若干次。

## Target

进场和退场都用 `--ease-out`（`cubic-bezier(0.23, 1, 0.32, 1)`）——进场/退场按动效规则就该是 ease-out（起步快、跟着手）。

```tsx
/* 目标 */
      style={{
        transform:
          hidden || evaded ? "translateX(calc(100% + 16px))" : "translateX(0)",
        transitionProperty: "transform, opacity",
        transitionDuration: hidden || evaded ? "150ms" : "240ms",
        transitionTimingFunction: "var(--ease-out)",
      }}
```

`transitionDuration` 的两个值**保持不变**（150ms 退场 / 240ms 入场）：150ms 退场是「让开视野，别挡路」——应当比入场快，这是有意的非对称时序；240ms 入场在抽屉/面板的 200–500ms 预算内。

**依赖**：目标值 `var(--ease-out)` 来自动效 token 层（计划 001）。

- 如果计划 001 已落地：直接用 `var(--ease-out)`。
- 如果还没落地：在本计划里一并把这一行加进 `src/ui/main.css` 的 `:root` 块（位置在 `--fs-lg: 16px;` 之后），内容完全一致：

  ```css
    /* 动效 token（曲线 + 时长）——全仓唯一动效词汇表。
       时长阶梯对齐纪律（卡片 150ms，按钮/Chips 100ms，无弹跳）；
       曲线用强 ease-out / ease-in-out，不用 CSS 关键字 ease
       （cubic-bezier(0.25,0.1,0.25,1)），也不用 Tailwind 默认
       cubic-bezier(0.4,0,0.2,1)（慢起，hover 显滞）。 */
    --ease-out: cubic-bezier(0.23, 1, 0.32, 1);
    --ease-in-out: cubic-bezier(0.77, 0, 0.175, 1);
    --ease-drawer: cubic-bezier(0.32, 0.72, 0, 1);
    --dur-press: 160ms;
    --dur-chip: 100ms;
    --dur-card: 150ms;
    --dur-fade: 150ms;
    --dur-pop: 200ms;
    --dur-panel: 240ms;
    --dur-tooltip: 150ms;
  ```

  两条计划都落地时，001 会做同一次添加——以先落地的那次为准，不要加两遍。

## Repo conventions to follow

- 动效 token 住在 `src/ui/main.css` 的 `:root` 块（第 11–53 行），每组 token 前带 `/* … */` 说明块。参照已有的圆角/字号阶梯写法。
- 纪律注释在 `src/ui/main.css:157-158`：「动效纪律：卡片 150ms，按钮/ Chips 100ms，无弹跳」——本计划不动时长，这条不受影响。
- TaskPool 的这组内联样式是**因为位置/位移要用 JS 算**才内联的（`transform` 的 `calc(100% + 16px)`），保持内联，只换曲线的取值方式。
- CSS 变量可以直接写在 React 内联样式里：`style={{ transitionTimingFunction: "var(--ease-out)" }}`——React 不会转义 `var()`，浏览器直接解析。

## Steps

1. 确认 `var(--ease-out)` 是否已存在：`grep -n -- "--ease-out" src/ui/main.css`。
   - 不存在 → 按上面 Target 节的说明把整个动效 token 块加进 `src/ui/main.css` 的 `:root`。
2. 改 `src/components/TaskTimeline/TaskPool.tsx:121-123`：把三元的两段曲线换成单一值。

   ```tsx
        transitionTimingFunction: "var(--ease-out)",
   ```

   整块改完后是：

   ```tsx
        style={{
          transform:
            hidden || evaded ? "translateX(calc(100% + 16px))" : "translateX(0)",
          transitionProperty: "transform, opacity",
          transitionDuration: hidden || evaded ? "150ms" : "240ms",
          transitionTimingFunction: "var(--ease-out)",
        }}
   ```

3. 跑验证（见下）。

## Boundaries

- **不要改** `transitionDuration` 的两个值（150ms / 240ms）——时长是有意的不对称，不要顺手"统一"。
- **不要改** `transform` 的位移目标值（`translateX(calc(100% + 16px))`）——那是布局口径，与本计划无关。
- **不要动** `src/components/TaskTimeline/TaskTimelinePage.tsx` 里 `setDragEvade` 的触发逻辑（那是计划 007/008 的地盘）。
- **不要改** `src/ui/main.css` 里 `.plan-blk` 等其它 transition 声明——那是计划 001 的地盘。
- **不要改** `transitionProperty: "transform, opacity"` —— 两个属性都是合成器属性，保留。
- 不要新增依赖。
- 如果代码与本计划描述的现状对不上（行号漂移、三元结构不同），**停下来报告**。

## Verification

- **机械**：
  ```bash
  npx tsc --noEmit      # 零错
  npx oxlint src        # 零报
  npm test              # 全绿
  ```
  另加一条断言（加进 `src/components/TaskTimeline/TaskPool.test.tsx` 或新建同名测试文件，按仓库现有测试文件风格）：

  ```ts
  it("任务池退场用 ease-out（不是 ease-in 慢起曲线）", () => {
    // 池子 aside（hidden/evaded）时不得再出现 ease-in 家族曲线
    const style = /* 渲染池子并读其 style */;
    expect(style.transitionTimingFunction).toContain("var(--ease-out)");
    expect(style.transitionTimingFunction).not.toContain("0.3, 0, 0.8");
  });
  ```

  若读不到内联 style，退一步用静态锁（`src/ui/main.css.test.ts` 同款写法，读源文件）：

  ```ts
  const src = readFileSync("src/components/TaskTimeline/TaskPool.tsx", "utf-8");
  expect(src).not.toContain("cubic-bezier(0.3, 0, 0.8, 0.15)");
  expect(src).toContain("var(--ease-out)");
  ```

- **手感自检**：
  - 打开任务页，从右侧任务池里拖一个任务块到周网格：池子滑开时**必须立刻开始动**（不能先卡 40ms 再窜出去）。这是本计划的核心。
  - 把指针拖回池子上，池子滑回；再拖出去，反复几次——两个方向的手感必须一致（现在两个方向差 22 倍）。
  - 点右上角 `⇥` 折叠任务池、点 `⇥` 再展开：滑出与滑入的手感应当对称。
  - 在 DevTools → Animations 面板把 playback 调到 10%，确认池子位移是**匀速可读的推进**（能看到它在头 1/4 时间内已经走了大半），而不是「先不动、后暴冲」。
  - 切换深浅主题：池子的玻璃背景与投影在滑出过程中的观感不应改变（本次只动曲线，不动配色）。
- **完成判据**：
  - `grep -n "cubic-bezier" src/components/TaskTimeline/TaskPool.tsx` 返回空。
  - `grep -n "var(--ease-out)" src/components/TaskTimeline/TaskPool.tsx` 命中 1 次。
  - 上面那条测试全绿。

