# 006 — 拖拽幽灵改用 transform 定位，去掉逐帧布局写入

- **Status**: 已完成
- **Commit**: b22387c7（计划锚定）→ 落地于本次提交
- **Severity**: MEDIUM
- **Category**: §5 Performance
- **Estimated scope**: 2 个文件（`TaskTimelinePage.tsx` + `main.css`），约 20 行改动

## Problem

拖拽时跟随指针的「幽灵块」用 `left` / `top` 内联定位，而这两个是**布局属性**——每写一次就触发一次重排。

```tsx
/* src/components/TaskTimeline/TaskTimelinePage.tsx:406-417 —— 当前 */
      {ghost && (
        <div
          ref={ghostRef}
          className="drag-ghost"
          style={
            {
              "--c": ghost.color,
              left: ghost.x,
              top: ghost.y,
            } as React.CSSProperties
          }
        >
          <div className="drag-ghost-title">{ghost.title}</div>
          <div className="drag-ghost-label num">{ghost.label}</div>
        </div>
      )}
```

```css
/* src/ui/main.css:1042-1054 —— 当前 */
  .drag-ghost {
    position: fixed;
    z-index: 50;
    pointer-events: none;
    width: 180px;
    ...
  }
```

`pointermove` 里每次都写这两个值（`TaskTimelinePage.tsx:203` / `:212` / `:236` / `:247`），于是：

- 幽灵每帧触发一次重排（`left`/`top` 改动 → 布局 → 绘制 → 合成）。
- 同一帧里还**读**了 `ghostRef.current?.offsetWidth` / `offsetHeight`（`:180-181`）——读布局属性会强制同步布局，把上一帧的脏布局立刻算出来。读+写在同一帧里交替，正是最典型的布局抖动（layout thrash）成因。

幽灵是拖拽过程中**唯一在动**的元素，也是整个交互里最需要保持在 60fps 的东西。

> 说明：幽灵落点的提示框 `.plan-slot`（`WeekGrid.tsx:210-215`）同样每帧改 `top` / `height`，但它是**静态提示**（不跟随指针），只在落点变化时才需要重算，且不在本计划范围内。

## Target

幽灵改用 `transform: translate3d()` 定位——纯合成器操作，不触发布局。

**CSS 侧**：给 `.drag-ghost` 固定 `left: 0; top: 0`，让 transform 成为唯一的位移来源。

```css
/* 目标 —— src/ui/main.css:1042-1054 */
  .drag-ghost {
    position: fixed;
    left: 0;
    top: 0;
    z-index: 50;
    pointer-events: none;
    width: 180px;
    ...
  }
```

**TSX 侧**：用 transform 代替 left/top。

```tsx
/* 目标 —— src/components/TaskTimeline/TaskTimelinePage.tsx */
      {ghost && (
        <div
          ref={ghostRef}
          className="drag-ghost"
          style={
            {
              "--c": ghost.color,
              transform: `translate3d(${ghost.x}px, ${ghost.y}px, 0)`,
            } as React.CSSProperties
          }
        >
```

**坐标口径不变**：`.drag-ghost` 是 `position: fixed`，`ghost.x` / `ghost.y` 本来就是视口坐标（`getBoundingClientRect()` 与 `ev.clientX/clientY` 同源，见 `:150-156` 的 `clampToGrid` 与 `:177-186` 的幽灵钳制），所以 `left: 0; top: 0` + `translate3d(x, y, 0)` 与原来的 `left: x; top: y` **完全等价**。

**顺带修掉逐帧的布局读取**：`offsetWidth` / `offsetHeight` 缓存成 ref，只在第一次量到时记下来（幽灵尺寸是固定的：宽 180px，高度由两行固定内容决定，不会变）。

```tsx
/* 在组件里新增 */
  /** 幽灵尺寸缓存：内容固定（标题 + 标签两行），量一次就够；
      每帧读 offsetWidth 会强制同步布局，把上一帧的脏布局立刻算出来。 */
  const ghostSizeRef = useRef<{ w: number; h: number } | null>(null);
```

```tsx
/* 目标 —— 替换 :180-181 的逐帧读取 */
        const gs = ghostSizeRef.current ?? {
          w: ghostRef.current?.offsetWidth ?? 180,
          h: ghostRef.current?.offsetHeight ?? 54,
        };
        if (!ghostSizeRef.current && gs.w) ghostSizeRef.current = gs;
        const gw = gs.w;
        const gh = gs.h;
```

（`ghostSizeRef` 要在拖拽开始时（`startDrag`）重置为 `null`，让不同任务重新量一次——不同标题长度可能让高度差一像素。）

## Repo conventions to follow

- **`.drag-ghost` 的材质（颜色、投影、圆角、字号）不动**，本计划只改定位方式。
- **拖拽几何口径集中在 `TaskTimelinePage.tsx` 的 `clampToGrid`（`:149-156`）和幽灵钳制（`:177-186`）**——这两处的计算结果原样喂给 transform，不要动算法。
- **`--c` 颜色变量照旧内联注入**（`style={{ "--c": ghost.color }}`），它走的是 CSS 变量但只影响背景色，不涉及位移。
- **`.plan-slot`（`WeekGrid.tsx:210-215`）不在本计划范围**。

## Steps

1. 改 `src/ui/main.css:1042-1054` 的 `.drag-ghost`：加 `left: 0;` 和 `top: 0;`（放在 `position: fixed;` 之后）。
2. 改 `src/components/TaskTimeline/TaskTimelinePage.tsx:406-417`：把 `left: ghost.x, top: ghost.y` 换成 `transform: \`translate3d(${ghost.x}px, ${ghost.y}px, 0)\``。
3. 在 `TaskTimelinePage.tsx` 里加 `ghostSizeRef`（`useRef`，需确认 `useRef` 已在 import 里；没有就加）。
4. 把 `:180-181` 的逐帧 `offsetWidth` / `offsetHeight` 读取换成读缓存（首次量后写入）。
5. 在 `startDrag`（`TaskTimelinePage.tsx` 内）里把 `ghostSizeRef.current = null` 重置。
6. 跑验证（见下）。

## Boundaries

- **不要改** `clampToGrid`（`:149-156`）与幽灵钳制（`:177-186`）的算法——坐标口径不变。
- **不要改** `.drag-ghost` 的宽高、颜色、投影、圆角、内层结构。
- **不要动** `.plan-slot`（`WeekGrid.tsx:210-215`）。
- **不要动** `WeekGrid` 的 `PlanBlockView` 渲染与 `plan-blk` 的内联 `top`/`left`/`width`/`height`——那些是**布局定位**，本来就该是布局属性。
- **不要做** `React.memo` / `useCallback` 改造。审计里提到「每次 pointermove 整棵时间轴树重渲染（`setGhost`/`setSlot`/`setDragEvade` 触发）」，那是一个**独立且更大的重构**（需要先稳定回调引用），本计划明确不碰。
- 不要新增依赖。
- 如果 `ghost.x` / `ghost.y` 在别处也被读作 CSS 像素值（而不只是写进 style），**停下来报告**。

## Verification

- **机械**：
  ```bash
  npx tsc --noEmit      # 零错
  npx oxlint src        # 零报
  npm test              # 全绿
  ```
  加一条静态锁（读源文件，照 `src/ui/main.css.test.ts` 的写法）：

  ```ts
  it("拖拽幽灵用 transform 定位，不用 left/top", () => {
    const src = readFileSync("src/components/TaskTimeline/TaskTimelinePage.tsx", "utf-8");
    const ghostBlock = src.match(/className="drag-ghost"[\s\S]*?style=\{([\s\S]*?)\}/)?.[1] ?? "";
    expect(ghostBlock).toContain("translate3d");
    expect(ghostBlock).not.toContain("left: ghost.x");
    expect(ghostBlock).not.toContain("top: ghost.y");
  });
  ```

- **手感自检**：
  - 任务页，从池里拖一个块进周网格：幽灵必须**精确贴住指针**（与改动前逐像素对齐，不能有 1px 偏移——尤其注意 `left: 0; top: 0` 之后 transform 的原点）。
  - 把块拖到网格**边缘**：幽灵的钳制行为不变（整块矩形钳在泳道内，见 `:177-186`）。
  - 拖到网格**外面**（游标出网格区）：幽灵的表现与改动前一致。
  - 快速来回甩鼠标（连续 pointermove）：幽灵不能出现「追不上」或「抖动」。
  - DevTools → Performance → 录一段拖拽，勾上 Paint flashing：改动后应**看不到幽灵区域的 layout/paint 闪烁**（改动前每帧一次）。
  - DevTools → Elements 选中 `.drag-ghost`，看 Computed：`transform` 应为 `matrix(1, 0, 0, 1, X, Y)`，`left`/`top` 均为 `0px`。
  - 回归检查：块的落位提示（虚线框 `.plan-slot`）位置必须仍与幽灵一致。
- **完成判据**：
  - `grep -n "left: ghost.x" src/components/TaskTimeline/TaskTimelinePage.tsx` 返回空。
  - 上面那条测试全绿。
  - Performance 面板里拖拽期间没有因幽灵引起的 layout 事件。


## 落地记录

落地与本计划一致，无偏差。两处与计划写法不同，理由如下。

**一、尺寸缓存改成「量到才缓存」。** 计划给的写法是
`const gs = ghostSizeRef.current ?? { w: ghostRef.current?.offsetWidth ?? 180, ... }`，
再 `if (!ghostSizeRef.current && gs.w) ghostSizeRef.current = gs;`——但那样第一次
pointermove 时幽灵元素**还没挂上**（`ghost` 状态是这次 move 才 set 的），读到的
`?? 180` / `?? 54` 是兜底值，且因为 `gs.w` 为真（180）而**被当成实测值缓存下来**，
之后再也不量。改法是只有 `ghostRef.current` 真的在时才量、才写缓存；元素不在就用
兜底值过这一帧，下一帧再量。行为与改动前一致（改动前也是元素挂上后才读到真实值）。

**二、`.drag-ghost` 的高度确实是固定的**，缓存是安全的：标题 `white-space: nowrap;
overflow: hidden; text-overflow: ellipsis`（单行），标签也是单行，宽度固定 180px。
拖拽过程中标签文案会变（「至 18:30」/「松手取消」/「拖到时间轴上排期」），但都是
单行，高度不变；标题在整次拖拽内不变。所以每次拖拽量一次即可——`startDrag` 里把缓存
置 `null`，让下一次拖拽重新量（不同任务标题可能让高度差一像素）。

**坐标等价性实测**（Chromium，两个 `position: fixed` 元素分别用两种定位方式落在
413,287）：`getBoundingClientRect()` 的 x/y/width/height **完全一致**（dx=0 dy=0
dw=0 dh=0）。`left: 0; top: 0` + `translate3d(x, y, 0)` 与原来的 `left: x; top: y`
是像素级等价，不存在计划里担心的 1px 偏移。

**静态锁四条**：

- DOM 级（`TaskTimelinePage.test.tsx`）：拖起来之后幽灵的 `style.transform` 形如
  `translate3d(Xpx, Ypx, 0)`，且 `style.left` / `style.top` 为空串。DOM 断言比扫
  源码文本强——源码写了 transform 但没真正落到元素上，DOM 断言照样红。
- 源码级（`main.css.test.ts`）：幽灵的 style 块里有 `translate3d`、没有
  `left: ghost.x` / `top: ghost.y`。
- 源码级：`.drag-ghost` 的 CSS 里 `left: 0` / `top: 0` 都在（transform 是唯一位移源）。
- 源码级：幽灵尺寸的读取必须落在「缓存为空且元素在」的守卫里且读完写回缓存，
  不允许出现 `ghostRef.current?.offsetWidth` 这种每帧裸读。

四条都做过反向验证：撤掉 transform → 2 条红；撤掉 CSS 的 `left/top: 0` → 1 条红；
退回每帧裸读 → 1 条红。

**没做的**：`.plan-slot`（落点提示框）仍每帧改 `top` / `height`——它是静态提示，不跟随
指针，只在落点变化时才需要重算，按计划边界不动。整棵时间轴树在 pointermove 里重渲染
（`setGhost` / `setSlot` / `setDragEvade`）也没碰——那要先稳定回调引用，是独立且更大的
重构。
