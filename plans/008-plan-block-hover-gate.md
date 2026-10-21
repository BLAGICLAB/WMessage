# 008 — 计划块 hover 动效加触屏门控，时长收到 100ms

- **Status**: 已完成
- **Commit**: b22387c7（计划锚定）→ 落地于本次提交
- **Severity**: MEDIUM
- **Category**: §6 Accessibility / §1 Purpose & frequency
- **Estimated scope**: 1 个 CSS，约 12 行改动

## Problem

周网格里的计划块（`.plan-blk`）在 hover 时同时启动三条动效，而且**所有 hover 规则都没有做触屏门控**。

```css
/* src/ui/main.css:689-692 —— 当前 */
  .plan-blk {
    ...
    transition:
      box-shadow 0.12s ease,
      transform 0.12s ease,
      background-color 0.12s ease;
  }
```

```css
/* src/ui/main.css:704-706 —— 当前 */
  .plan-blk:hover {
    transform: translateY(-1px);
  }
```

```css
/* src/ui/main.css:722-727 —— 当前 */
  .dark .plan-blk:hover {
    transform: translateY(-2px);
    box-shadow:
      0 8px 24px color-mix(in srgb, var(--c) 45%, transparent),
      inset 0 1px 0 rgba(255, 255, 255, 0.28);
  }
```

```css
/* src/ui/main.css:805-807 —— 当前 */
  .plan-blk:hover .plan-grip {
    opacity: 1;
  }
```

问题：

1. **触屏会误触发并粘住**。`.plan-blk` 明确是触屏可用的（`touch-action: none`，`main.css:685`；块本身可以拖拽/拉伸）。触屏上点一下块，浏览器会先派发 `:hover`——块就抬起来了并且**保持抬起**，直到用户点到别处。移动端/触屏笔记本上这是明确的视觉 bug。
2. **同时启动三条过渡**，其中 `box-shadow` 在暗色下要从 `0 4px 16px` 变到 `0 8px 24px`——这是最常被 hover 的元素，每 hover 一次就重绘一次阴影。
3. **时长 120ms 越过仓库自定的 chip 预算**（`src/ui/main.css:158`：「按钮/Chips 100ms」），且 120ms 这个数字只此一处，与 main.css 里其它 0.1s / 0.15s 都不重合。

## Target

**一、所有 `.plan-blk` 的 hover 规则包进 `@media (hover: hover) and (pointer: fine)`。**

```css
/* 目标 —— 替换 main.css:704-706 */
  @media (hover: hover) and (pointer: fine) {
    .plan-blk:hover {
      transform: translateY(-1px);
    }
  }
```

```css
/* 目标 —— 替换 main.css:722-727 */
  @media (hover: hover) and (pointer: fine) {
    .dark .plan-blk:hover {
      transform: translateY(-2px);
      box-shadow:
        0 8px 24px color-mix(in srgb, var(--c) 45%, transparent),
        inset 0 1px 0 rgba(255, 255, 255, 0.28);
    }
  }
```

```css
/* 目标 —— 替换 main.css:805-807 */
  @media (hover: hover) and (pointer: fine) {
    .plan-blk:hover .plan-grip {
      opacity: 1;
    }
  }
```

**同样要包进门控的两处**（审计时漏掉、本计划补上）：

```css
/* main.css:921-925 —— .plan-blk.is-archived:hover */
  @media (hover: hover) and (pointer: fine) {
    .plan-blk.is-archived:hover {
      transform: translateY(-1px);
      filter: saturate(0.5);
      box-shadow: 0 3px 8px color-mix(in srgb, var(--c) 14%, transparent);
    }
  }
```

```css
/* main.css:935-937 —— .dark .plan-blk.is-archived:hover */
  @media (hover: hover) and (pointer: fine) {
    .dark .plan-blk.is-archived:hover {
      box-shadow: 0 4px 14px color-mix(in srgb, var(--c) 18%, transparent);
    }
  }
```

**二、过渡时长收到 `--dur-chip`（100ms），并把 `box-shadow` 移出过渡列表。**

```css
/* 目标 —— 替换 main.css:689-692 */
  .plan-blk {
    ...
    transition:
      transform var(--dur-chip) var(--ease-out),
      background-color var(--dur-chip) var(--ease-out);
  }
```

`box-shadow` 移出过渡列表的意思是：暗色下 hover 时阴影**直接换到新值**（不补间），而位移仍然平滑。理由是阴影是一次「状态点亮」，不是一段运动；每帧重绘阴影是纯浪费。亮色模式下 `.plan-blk:hover` 本来就不改阴影（只有暗色改），所以移除后亮色行为完全不变。

**依赖**：两个 token 由计划 001 建立。**若计划 001 尚未落地**，写字面量，功能等价：

```css
    transition:
      transform 100ms cubic-bezier(0.23, 1, 0.32, 1),
      background-color 100ms cubic-bezier(0.23, 1, 0.32, 1);
```

## Repo conventions to follow

- **`@media (hover: hover) and (pointer: fine)` 是动效规则里给出的门控写法**，仓库其它地方目前没有先例，所以本计划是**新增**这个写法——第一次用就在这里。
- **`.plan-blk` 的几何定位是内联的**（`top`/`left`/`width`/`height`，见 `PlanBlock.tsx:87-95`），本计划不碰。
- **`main.css` 里 `@media` 块的写法先例**：`:596-600`（`@supports not (backdrop-filter: …)`）、`:991-995` 与 `:1026-1030`（reduced-motion 覆盖）都是独立成块。跟着这个风格写。
- **`.plan-grip` 的 `opacity` 过渡**（`main.css:799`）保留——`opacity` 是合成器属性，且它的显隐是「拉伸握把浮现」的信息性反馈。

## Steps

1. 确认 token 是否已存在：`grep -n -- "--ease-out\|--dur-chip" src/ui/main.css`。
   - 存在 → 用 token 形式；不存在 → 用字面量形式并在本文件顶部记一笔。
2. 改 `src/ui/main.css:689-692` 的 `.plan-blk` 过渡列表：按 Target 节的写法替换（去掉 `box-shadow` 腿，时长与曲线换 token）。
3. 把 5 处 hover 规则包进 `@media (hover: hover) and (pointer: fine)`：
   - `main.css:704-706`（`.plan-blk:hover`）
   - `main.css:722-727`（`.dark .plan-blk:hover`）
   - `main.css:805-807`（`.plan-blk:hover .plan-grip`）
   - `main.css:921-925`（`.plan-blk.is-archived:hover`）
   - `main.css:935-937`（`.dark .plan-blk.is-archived:hover`）
4. 改 `main.css:799` 的 `.plan-grip`：`transition: opacity 0.12s ease;` → `transition: opacity var(--dur-chip) var(--ease-out);`。
5. 跑验证（见下）。

## Boundaries

- **不要改** `.plan-blk` 的定位、尺寸、圆角、配色、层级。
- **不要改** hover 时的位移量（亮色 -1px / 暗色 -2px）与阴影值——门控和时长是本计划的全部内容。
- **不要动** `.plan-grip` 的显隐逻辑（`opacity: 0` → hover `opacity: 1`），只改它的过渡时长。
- **不要动** `.plan-blk.is-archived` / `.is-done` 的降饱和与虚线边框。
- **不要把 hover 规则从 `@media` 里放出来**——那正是本计划要修的 bug。
- 不要新增依赖。

## Verification

- **机械**：
  ```bash
  npx tsc --noEmit      # 零错
  npx oxlint src        # 零报
  npm test              # 全绿
  ```
  加两条静态锁（加进 `src/ui/main.css.test.ts`，照该文件现有写法）：

  ```ts
  it("计划块 hover 规则有触屏门控", () => {
    // 每个 .plan-blk:hover 变体都必须落在 hover: hover 媒体查询里
    const hoverRules = css.match(/@media \(hover: hover\) and \(pointer: fine\)\s*\{[\s\S]*?\n  \}/g) ?? [];
    expect(hoverRules.length).toBeGreaterThan(0);
    for (const rule of hoverRules) {
      expect(rule).toMatch(/\.plan-blk/);
    }
    // 门控之外不应再有裸露的 .plan-blk:hover 规则
    const bare = css.match(/^\s*\.plan-blk:hover\s*\{/gm) ?? [];
    expect(bare).toEqual([]);
  });

  it("计划块过渡列表不含 box-shadow，时长归到 chip 档", () => {
    const block = css.match(/\.plan-blk\s*\{([^}]*)\}/)?.[1] ?? "";
    expect(block).not.toMatch(/transition:[^;]*box-shadow/);
    expect(block).toMatch(/transition:[^;]*var\(--dur-chip\)/);
  });
  ```

  注意第二条的正则要能跨行（过渡列表是多行的），照文件里 `nm-card-hover` 那条测试的 `[\s\S]*?` 写法。

- **手感自检**：
  - 任务页，指针划过周网格里的计划块：块应当**平滑地抬起来一点**（不是硬跳），且抬升幅度仍是一像素级别的克制。
  - 暗色模式下重复：抬升仍平滑；阴影应当**瞬间换到**新值（不拖尾），不要出现阴影追着块慢慢变大的感觉。
  - 指针在块上停留 1 秒以上：块必须完全静止。
  - **触屏回归检查**（关键）：在触屏设备或 DevTools 的设备模拟（iPhone / Pixel）上点一下计划块——块**不能**保持抬起；点完必须回到原状。
  - 拉伸握把（`.plan-grip`）在指针靠近时应当浮现，且不粘住（移开即消失）。
  - 归档态的块（虚线边 + 红印章）hover 时也要有同样的抬升，且印章的降饱和不因门控而失效。
- **完成判据**：
  - `grep -n "^\s*\.plan-blk:hover" src/ui/main.css` 返回空（全部进了媒体查询）。
  - 上面两条测试全绿。


## 落地记录

**与 007 的合并**：007 刚给 `.plan-blk` 的过渡列表补了 `opacity` 一项（网格侧源块
变暗、`.is-done`/`.is-archived` 的透明度变化），本计划 Target 节的过渡列表只写了
`transform` + `background-color` —— 直接照抄会把 007 那条腿抹掉。合并后的最终形态：

```css
    transition:
      transform var(--dur-chip) var(--ease-out),
      background-color var(--dur-chip) var(--ease-out),
      opacity var(--dur-chip) var(--ease-out);
```

即：本计划只做它自己的事（**移出 box-shadow 腿** + 5 处 hover 规则加触屏门控），
不动 007 加的 opacity 腿。两条计划的静态锁同时成立。

**静态锁写法**：门控检查不能靠正则猜「这条规则在不在媒体查询里」——用括号配对
扫出 `@media (hover: hover) and (pointer: fine)` 的区间，再检查每条 `.plan-blk…:hover`
规则的起始位置落在哪个区间。撤掉任一处门控，锁立刻转红（已反向验证）。
