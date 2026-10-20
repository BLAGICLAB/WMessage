# 013 — framer-motion 的 y 简写改成完整 transform 串

- **Status**: TODO
- **Commit**: b22387c7
- **Severity**: MEDIUM
- **Category**: §5 Performance
- **Estimated scope**: 2 个 tsx，约 10 行改动

## Problem

全仓只有两个列表用了 framer-motion 的位移动画，两个都用了 `y` 简写。

```tsx
/* src/components/HomePage/PoolPager.tsx:116-124 —— 当前 */
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.div
            key={page}
            className="space-y-1"
            initial={reduceMotion ? false : { opacity: 0, y: 6 }}
            animate={{ opacity: 1, y: 0 }}
            exit={reduceMotion ? { opacity: 0 } : { opacity: 0, y: -6 }}
            transition={{ duration: 0.18 }}
          >
```

```tsx
/* src/components/HomePage/DueTicker.tsx:62-70 —— 当前 */
            <AnimatePresence mode="popLayout" initial={false}>
              <motion.span
                key={current.id}
                className="flex min-w-0 flex-1 items-center gap-2"
                initial={reduceMotion ? false : { opacity: 0, y: 8 }}
                animate={{ opacity: 1, y: 0 }}
                exit={reduceMotion ? { opacity: 0 } : { opacity: 0, y: -8 }}
                transition={{ duration: 0.25 }}
              >
```

动效规则点名了这件事：**Framer Motion 的 `x` / `y` / `scale` 简写不是硬件加速的**——它们在主线程上插值，负载下一掉帧。目标是**完整的 transform 串**：

```tsx
animate={{ transform: "translateX(100px)" }}
```

这两处都在**首页落地屏**上——最常打开、最需要保持顺滑的页面。

顺带两件事：
- 两个 `transition` 的时长不一样（0.18 / 0.25），做的是**同一类**「换页/换条的内容交叉淡入淡出」。
- 两个 `transition` 都没指定缓动，用的是 framer-motion 的默认。

## Target

两处都改成完整 transform 串，并统一到仓库的强 ease-out 曲线。

```tsx
/* 目标 —— src/components/HomePage/PoolPager.tsx:116-124 */
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.div
            key={page}
            className="space-y-1"
            initial={reduceMotion ? false : { opacity: 0, transform: "translateY(6px)" }}
            animate={{ opacity: 1, transform: "translateY(0px)" }}
            exit={reduceMotion ? { opacity: 0 } : { opacity: 0, transform: "translateY(-6px)" }}
            transition={{ duration: 0.18, ease: [0.23, 1, 0.32, 1] }}
          >
```

```tsx
/* 目标 —— src/components/HomePage/DueTicker.tsx:62-70 */
            <AnimatePresence mode="popLayout" initial={false}>
              <motion.span
                key={current.id}
                className="flex min-w-0 flex-1 items-center gap-2"
                initial={reduceMotion ? false : { opacity: 0, transform: "translateY(8px)" }}
                animate={{ opacity: 1, transform: "translateY(0px)" }}
                exit={reduceMotion ? { opacity: 0 } : { opacity: 0, transform: "translateY(-8px)" }}
                transition={{ duration: 0.25, ease: [0.23, 1, 0.32, 1] }}
              >
```

要点：

- `ease: [0.23, 1, 0.32, 1]` —— framer-motion 接受 4 元数组形式的 cubic-bezier，值取自动效规则的 `--ease-out`。
- **两处的时长保持各自原值**（0.18 / 0.25）：翻页（用户主动点 ‹ ›，要跟手）和即将到期轮播（4 秒自动轮换，可以更从容）是两种不同的交互节奏，**统一它们是本计划明确不做的事**。只统一曲线。
- `reduceMotion` 分支保持原样：reduced-motion 下入场直接挂载（`initial={false}`），退场只留淡出。

**依赖**：曲线值 `cubic-bezier(0.23, 1, 0.32, 1)` 就是 `--ease-out` 的字面值。framer-motion 的 `ease` 数组**不能**写 `var(--ease-out)`（它是 JS 值，不是 CSS），所以这里直接写字面量数组——**这是 framer-motion 的用法，与 CSS token 化不冲突**。

## Repo conventions to follow

- **framer-motion 在本仓库只出现在 3 个文件**（`HomeHeader.tsx`、`PoolPager.tsx`、`DueTicker.tsx`），全部走 `useReducedMotion` + `useScroll`/`useSpring`/`useTransform`。不要引入第二种用法。
- **两个文件都已经接了 `useReducedMotion`**（`PoolPager.tsx:33`、`DueTicker.tsx:22`），`initial={reduceMotion ? false : …}` 的写法要保留。
- **`mode="popLayout"` 与 `initial={false}` 保持不变** —— 它们管的是「退场元素浮在入场元素之上」和「首次挂载不播入场」，与本计划无关。
- **`HomeHeader.tsx` 不在本计划范围**（它的动效由计划 003 处理）。

## Steps

1. 改 `src/components/HomePage/PoolPager.tsx:120-123`：

   ```tsx
            initial={reduceMotion ? false : { opacity: 0, transform: "translateY(6px)" }}
            animate={{ opacity: 1, transform: "translateY(0px)" }}
            exit={reduceMotion ? { opacity: 0 } : { opacity: 0, transform: "translateY(-6px)" }}
            transition={{ duration: 0.18, ease: [0.23, 1, 0.32, 1] }}
   ```

2. 改 `src/components/HomePage/DueTicker.tsx:66-69`：

   ```tsx
                initial={reduceMotion ? false : { opacity: 0, transform: "translateY(8px)" }}
                animate={{ opacity: 1, transform: "translateY(0px)" }}
                exit={reduceMotion ? { opacity: 0 } : { opacity: 0, transform: "translateY(-8px)" }}
                transition={{ duration: 0.25, ease: [0.23, 1, 0.32, 1] }}
   ```

3. 跑验证（见下）。

## Boundaries

- **不要改** 两处的时长（0.18 / 0.25）。
- **不要改** `mode="popLayout"`、`initial={false}`、`reduceMotion` 的任何分支。
- **不要动** `HomeHeader.tsx`。
- **不要把 `y` 简写改回去**，也不要「顺手」把 `animate` 的 `y: 0` 写成 `transform: "none"` —— `translateY(0px)` 与 `none` 在合成层行为不同。
- **不要改** `PoolPager` 的分页逻辑或 `DueTicker` 的 4 秒轮播逻辑。
- 不要新增依赖。

## Verification

- **机械**：
  ```bash
  npx tsc --noEmit      # 零错
  npx oxlint src        # 零报
  npm test              # 全绿
  ```
  加一条静态锁（加进 `src/components/HomePage/HomePage.test.tsx` 或新建同名测试文件，照仓库现有风格）：

  ```ts
  it("列表动画用完整 transform 串，不用 y 简写", () => {
    for (const f of [
      "src/components/HomePage/PoolPager.tsx",
      "src/components/HomePage/DueTicker.tsx",
    ]) {
      const src = readFileSync(f, "utf-8");
      expect(src).not.toMatch(/\by:\s*-?\d/);
      expect(src).toMatch(/transform:\s*"translateY\(/);
    }
  });
  ```

- **手感自检**：
  - 首页任务卡分页器：点 ‹ › 翻页，旧内容应**淡出并上滑**，新内容**淡入并从下滑上来**。两个元素的交接不能出现「同时全在」的重影。
  - 首页「即将到期」：等它自动轮播到下一条（或鼠标悬停暂停后切走），旧标题**淡出上滑**、新标题**下滑淡入**。
  - 快速连点 ‹ ›：每次翻页的动画都必须独立播完，不能互相打断或错位。
  - DevTools → Elements 选中动画中的元素，Computed 的 `transform` 应是一个完整的 `matrix(...)`（含 translate 分量），而不是空。
  - **回归检查（关键）**：`mode="popLayout"` 的退场行为——退场元素应当浮在退场位置（不再被推着走）。若改完发现退场元素突然跟着新元素跑，说明 `transform` 串与 popLayout 的定位投影打架了，**停下来报告**。
  - DevTools → Rendering → `Emulate prefers-reduced-motion: reduce`：翻页应当**直接切换**（无入场），退场只留淡出。
- **完成判据**：
  - `grep -n "y: " src/components/HomePage/PoolPager.tsx src/components/HomePage/DueTicker.tsx` 返回空。
  - 上面那条测试全绿。

