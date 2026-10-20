# 004 — 周图 Tooltip 的 400ms 过渡收到 150ms

- **Status**: TODO
- **Commit**: b22387c7
- **Severity**: HIGH
- **Category**: §2 Easing & duration
- **Estimated scope**: 1 个 tsx，1 行改动

## Problem

本周计划/完成柱状图的 tooltip 没配动画参数，recharts 3.10.1 的默认值直接生效：

```tsx
/* src/components/HomePage/WeeklyChart.tsx:157 —— 当前 */
          <Tooltip cursor={false} content={(p) => <WeekTip {...p} />} />
```

默认值在 `node_modules/recharts/es6/component/Tooltip.js:47-48`：

```js
  animationDuration: 400,
  animationEasing: 'ease',
```

`TooltipBoundingBox.js:15-24` 把它拼成一条 **transform 过渡**：

```js
function resolveTransitionProperty(args) {
  if (args.prefersReducedMotion && args.isAnimationActive === 'auto') {
    return undefined;
  }
  if (args.isAnimationActive && args.active) {
    var easing = typeof args.animationEasing === 'string' ? args.animationEasing : 'ease';
    return "transform ".concat(args.animationDuration, "ms ").concat(easing);
  }
  return undefined;
}
```

即 `transition: transform 400ms ease` 打在 tooltip 容器上。

问题有三层：

1. **400ms 是 tooltip 预算（125–200ms）的 2–4 倍**，也超出 UI 动画 300ms 的上限。
2. **它过渡的是 `transform`**——tooltip 的位置是跟着指针走的。指针从一根柱子移到下一根时，提示框要用 400ms 慢慢滑过去，**视觉上拖在指针后面**。
3. **同一张图里，动画口径不一致**：两根 `Bar` 都显式关掉了入场动画（`isAnimationActive={false}`，见 `:159` 和 `:169`），只有 tooltip 是开着的，而且是唯一一个时长失控的。

## Target

显式传 `animationDuration` 与 `animationEasing`，把 tooltip 的移动收进 tooltip 的时长预算，并换成仓库的强 ease-out 曲线（起步快、落点准，正好符合「提示框要跟住指针」的诉求）。

```tsx
/* 目标 */
          <Tooltip
            cursor={false}
            content={(p) => <WeekTip {...p} />}
            animationDuration={150}
            animationEasing="var(--ease-out)"
          />
```

- `animationDuration={150}` —— 落在 tooltip 预算（125–200ms）的中段。
- `animationEasing="var(--ease-out)"` —— `var()` 在内联 `transition` 简写里合法，recharts 会原样拼成 `transform 150ms var(--ease-out)`。曲线值 `cubic-bezier(0.23, 1, 0.32, 1)`。
- **`var(--ease-out)` 的依赖**：该 token 由计划 001 建立。**若计划 001 尚未落地**，本计划直接把曲线写死，同样合法、同样只改这一行：

  ```tsx
            animationEasing="cubic-bezier(0.23, 1, 0.32, 1)"
  ```

  两条计划都落地时以 token 形式为准，不要两个都留。

**reduced-motion 已经没问题，不要动**：recharts 的 `isAnimationActive` 默认是 `'auto'`，`resolveTransitionProperty` 第一条判断就是 `prefersReducedMotion && isAnimationActive === 'auto'` → 直接返回 `undefined`，不产生过渡。应用层 `src/ui/main.css:111-120` 的全局块也会把 `transition-duration` 压成 `0.01ms !important`（important 声明压过内联简写里的时长）。两重保险都在。

## Repo conventions to follow

- **图表动画口径的现成先例就在同一文件里**：两根 `Bar` 都写了 `isAnimationActive={false}`（`:159`、`:169`），说明这个仓库对 recharts 的默认动画是有意识的逐个开关，不是放任默认。
- **不动 `WeekTip` 组件本身**（`:55-90` 附近的 `WeekTip`）——本计划只改 `<Tooltip>` 的两个 props。
- **不动 `cursor={false}`**——不要顺手加十字准线，那是产品视觉变更，不是动效修复。

## Steps

1. 确认 token 是否已存在：`grep -n -- "--ease-out" src/ui/main.css`。
   - 存在 → 用 `animationEasing="var(--ease-out)"`。
   - 不存在 → 用 `animationEasing="cubic-bezier(0.23, 1, 0.32, 1)"`，并在本文件顶部记一笔「计划 001 落地后请替换为 token 形式」。
2. 改 `src/components/HomePage/WeeklyChart.tsx:157`：

   ```tsx
          <Tooltip
            cursor={false}
            content={(p) => <WeekTip {...p} />}
            animationDuration={150}
            animationEasing="var(--ease-out)"
          />
   ```

3. 跑验证（见下）。

## Boundaries

- **不要改** `WeekTip` 组件的内容、结构、样式。
- **不要改** `cursor={false}`。
- **不要动** 两根 `Bar` 的 `isAnimationActive={false}`——它们是对的。
- **不要动** `src/ui/main.css:111-120` 的 reduced-motion 全局块——recharts 自己已经处理了。
- **不要新增依赖**。
- 如果 recharts 版本变化导致这两个 prop 名字不同（`animationDuration` / `animationEasing`），**停下来报告**（它们的定义在 `node_modules/recharts/es6/component/Tooltip.js:47-48`，可自行核对）。

## Verification

- **机械**：
  ```bash
  npx tsc --noEmit      # 零错
  npx oxlint src        # 零报
  npm test              # 全绿
  ```
  加一条静态锁（可放 `src/components/HomePage/WeeklyChart.test.tsx` 或新建同名测试文件，照仓库现有测试文件风格）：

  ```ts
  it("周图 tooltip 的动画时长在 tooltip 预算内（≤200ms）", () => {
    const src = readFileSync("src/components/HomePage/WeeklyChart.tsx", "utf-8");
    expect(src).toMatch(/<Tooltip[\s\S]*animationDuration=\{(?:1[0-9]{2}|200)\}/);
    expect(src).not.toMatch(/animationDuration=\{400\}/);
  });
  ```

- **手感自检**：
  - 打开首页的「本周计划/完成」柱状图，指针从周一慢慢划到周日：提示框必须**跟住指针**，不能拖在后面。
  - 在一根柱子上停留，提示框不能有残余的滑动或抖动（到位后必须静止）。
  - 快速来回划（周一→周日→周一）：提示框不能有「追不上 / 抖一下」的感觉。
  - DevTools → Elements 选中 tooltip 容器，看 Computed 样式里的 `transition-duration` 应为 `0.15s`、`transition-timing-function` 应为 `cubic-bezier(0.23, 1, 0.32, 1)`。
  - DevTools → Rendering → `Emulate prefers-reduced-motion: reduce`：提示框应**直接到位**，无过渡（这条是回归检查，别改坏它）。
- **完成判据**：
  - `grep -n "animationDuration" src/components/HomePage/WeeklyChart.tsx` 命中且值为 150。
  - Computed 的 `transition-duration` = 0.15s。

