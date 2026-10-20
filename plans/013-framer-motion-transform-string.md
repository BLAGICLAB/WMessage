# 013 — framer-motion 的 y 简写改成完整 transform 串

- **Status**: 已完成
- **Commit**: b22387c7（计划锚定）→ 落地于本次提交
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


## 落地记录：计划的性能前提被实测推翻，只落地缓动那一半

本计划的核心动作（`y` 简写 → 完整 transform 串）**没有做**，因为它依赖的前提
「framer-motion 的 `x`/`y`/`scale` 简写不是硬件加速的」**在 framer-motion 上不成立**。
计划里真正成立的那一半（缓动口径统一）**做了**。

### 一、`y` 简写本来就是 CSS transform

**源码依据**（framer-motion 14.1.0 / motion-dom 14.1.0）：
`node_modules/motion-dom/dist/es/render/html/utils/build-transform.mjs` 里
`buildTransform` 把 `y` 映射成 `translateY`，和其它 transform 分量拼成一条
transform 串：

```js
const translateAlias = { x: "translateX", y: "translateY", z: "translateZ", ... };
...
transformString += `${transformName}(${valueAsType}) `;
```

也就是说 `y: 6` 最终落到 DOM 上就是 `style.transform = "translateY(6px)"`——
一个 GPU 可合成的 transform，不是布局属性，也不是「主线程插值的非硬件加速路径」。

**运行期实测**（jsdom + framer-motion，逐帧读 `style.transform`，180ms 时长）：

| 时刻 | A：`y: 6` 简写 | C：`transform: "translateY(6px)"` |
|---|---|---|
| 13ms | `translateY(5.2956px)` | — |
| 21ms | — | `translateY(5.0377px)` |
| 176ms | `translateY(0.0053px)` | — |
| 178ms | — | `translateY(0.0015px)` |
| 结束后 | `none` | `translateY(0px)` |

两种写法的动画轨迹、时长、最终状态**几乎逐帧一致**。改写它换不来任何性能。

### 二、改写还会带来两个退步

1. **静止态多一条 transform**：A 播完回到 `none`，C 停在 `translateY(0px)`。一条常驻的
   transform 会让元素**成为 fixed 子元素的包含块**——页面里任何 fixed 定位的后代都会被
   改成相对这个元素定位。
2. **不能再与其它 transform 分量组合**：`y` / `scale` / `rotate` 可以共存于一条串里
   （`buildTransform` 负责拼），手写 `transform` 字符串则独占这个属性。

### 三、真正成立的那一半：缓动没指定

计划里「两个 transition 都没指定缓动，用的是 framer-motion 的默认」是**真的**。默认缓动
在 `motion-dom/dist/es/animation/utils/default-transitions.mjs`：

```js
const ease = { type: "keyframes", ease: [0.25, 0.1, 0.35, 1], duration: 0.3 };
```

它既不是仓库的 `--ease-out`（`cubic-bezier(0.23, 1, 0.32, 1)`），也不是动效纪律里
「进场/退场 → ease-out」的那条曲线。实测两者的差别很大——同一段 180ms 动画：

| 时刻 | 默认缓动 `[0.25,0.1,0.35,1]` | `--ease-out` `[0.23,1,0.32,1]` |
|---|---|---|
| 13ms | 5.296px（已走 88%） | — |
| 19ms | — | 3.496px（已走 42%） |
| 176ms | 0.005px（结束） | — |
| 162ms | — | 0.0008px（结束） |

改成 `--ease-out` 后，位移在头 1/4 时间里就走掉大部分（起步快、落点准），与仓库其它
入场动效（chip / 卡片 / 浮层 / 弹窗）是一条曲线。

落地写法：`transition={{ duration: 0.18, ease: [0.23, 1, 0.32, 1] }}`。
framer-motion 的 `ease` 是 **JS 值**，不能写 `var(--ease-out)`；类型上
`Easing = EasingDefinition | EasingFunction`，而 `BezierDefinition = readonly
[number, number, number, number]`，四元数组是合法写法（`tsc` 零错）。

**时长保持 0.18 / 0.25 不动**：翻页是用户主动操作（要跟手），轮播是自动的（可以更从容），
两种节奏不该被统一——这是计划里明确划的边界，也站得住。

### 四、静态锁三条（`src/components/HomePage/HomePage.test.tsx`）

- 两处过渡都显式写了 `ease`（不靠库的默认曲线）。
- 两处的 ease 数组与 `main.css` 里 `--ease-out` 的字面值**逐字相同**（写死的那份由测试
  钉在 token 上防漂移，与计划 004 对 recharts `animationEasing` 的处理同一手法）。
- 两处时长都在 UI 预算内（≤300ms）。

前两条做过反向验证：撤掉 `ease` → 两条红。

### 五、规则原文的适用边界

`improve-animations` 的 `AUDIT.md` §5 写的是「Framer Motion `x`/`y`/`scale` shorthands
are not hardware-accelerated」。这条规则在**别的动画库**（React Spring、裸 DOM 动画）
上成立，但对 framer-motion 不成立——它的简写就是 transform 的语法糖。审计时把这条规则
直接套到 framer-motion 上，是这条计划走偏的根源。§5 的另一条（「只动 transform 与
opacity」）当前代码已经满足，没有可改的。
