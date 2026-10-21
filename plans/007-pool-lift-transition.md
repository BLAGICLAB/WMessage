# 007 — 池内拖起补上过渡，别再单帧瞬时缩放

- **Status**: 已完成
- **Commit**: b22387c7（计划锚定）→ 落地于本次提交
- **Severity**: MEDIUM
- **Category**: §3 Physicality & origin / §7 Cohesion
- **Estimated scope**: 2 个文件（`TaskPool.tsx` + `main.css`），约 10 行改动

## Problem

从任务池里把任务拖起来时，被拖的池项会加一个 `.pool-lift` 类：

```tsx
/* src/components/TaskTimeline/TaskPool.tsx:55-60 —— 当前 */
  const itemCls = (t: Task) =>
    `flex h-[34px] touch-none items-center gap-2 rounded-lg border pr-1.5 ${
      liftedId === t.id
        ? "pool-lift relative z-10"
        : "hover:border-[var(--edge-strong)]"
    }`;
```

```css
/* src/ui/main.css:1070-1077 —— 当前 */
  /* 池内拖拽源块悬浮：升起 + 同色光晕（拖住即悬浮，落位/取消才收回） */
  .pool-lift {
    transform: scale(1.04) translateY(-1px);
    box-shadow:
      var(--shadow-lg),
      0 0 0 2px color-mix(in srgb, var(--accent) 45%, transparent);
    cursor: grabbing;
  }
```

**`.pool-lift` 没有任何 `transition` 声明**，而池项本身的类串里也没有 `transition-*` 工具类（见 `itemCls`——只有 `hover:border-[var(--edge-strong)]`）。所以：

- 拖起时，`transform` 与 `box-shadow` 在**一帧内**从原值跳到目标值。缩放 1.04 是瞬间发生的，读起来像「闪」而不是「被拿起来」。
- 松手时同样瞬时收回。

这跟同一次拖拽里网格侧的块形成了割裂：网格侧的源块只做 `opacity-35` 变暗（`PlanBlock.tsx:86`），变暗本身有淡入淡出（`opacity` 有 transition 腿的话），而池侧是硬切。同一个手势，两个源的「被拿起来」反馈强度和形态完全不同。

## Target

**给 `.pool-lift` 补上 transform 过渡**，让拖起/收回都变成一段可读的动画。

```css
/* 目标 —— src/ui/main.css:1070-1077 */
  /* 池内拖拽源块悬浮：升起 + 同色光晕（拖住即悬浮，落位/取消才收回） */
  .pool-lift {
    transform: scale(1.04) translateY(-1px);
    box-shadow:
      var(--shadow-lg),
      0 0 0 2px color-mix(in srgb, var(--accent) 45%, transparent);
    cursor: grabbing;
    /* 拖起/收回走 transform 过渡，不再单帧瞬切。
       box-shadow 不进过渡列表：阴影动画是逐帧重绘，
       而拖起这块是「瞬间抬起」的语义，不需要阴影跟着补间。 */
    transition: transform var(--dur-chip) var(--ease-out);
  }
```

三个设计决定：

1. **只过渡 `transform`** —— 动效规则要求优先动 `transform` / `opacity`。`box-shadow` 那一大圈投影 + 光晕如果也补间，每帧都要重绘这一小块；让它在拖起瞬间直接到位（不补间）更省，也符合「阴影是状态不是运动」的直觉。
2. **时长用 `--dur-chip`（100ms）** —— 拖起是一个「立刻发生」的高频反馈，100ms 足够读出、不会拖慢操作。
3. **曲线用 `--ease-out`** —— 起步快，块「弹」离池面的感觉立刻建立。

**依赖**：两个 token 由计划 001 建立。**若计划 001 尚未落地**，直接写字面量，功能等价：

```css
    transition: transform 100ms cubic-bezier(0.23, 1, 0.32, 1);
```

**另外补一处**：网格侧源块的变暗（`PlanBlock.tsx:86` 的 `dimmed ? " opacity-35" : ""`）目前是硬切。`.plan-blk` 的过渡列表是 `box-shadow / transform / background-color`（`main.css:689-692`），**不含 `opacity`**，所以透明度变化也是瞬时的。给 `.plan-blk` 的过渡列表补上 `opacity` 一项即可：

```css
/* 目标 —— src/ui/main.css:689-692 */
  .plan-blk {
    ...
    transition:
      box-shadow var(--dur-chip) var(--ease-out),
      transform var(--dur-chip) var(--ease-out),
      background-color var(--dur-chip) var(--ease-out),
      opacity var(--dur-chip) var(--ease-out);
  }
```

这一处也顺带覆盖了 `.plan-blk.is-done`（`opacity: 0.68`）与 `.plan-blk.is-archived`（`opacity: 0.88`）的透明度变化——它们此前同样是瞬时的。

## Repo conventions to follow

- **`.pool-lift` 的视觉（缩放 1.04、位移 -1px、光晕环）不动**——那是设计定的「被拿起来」语汇，本计划只给它补上过渡。
- **过渡类写在 CSS 类里，不写在 tsx 类串里** —— `.pool-lift` 已经是 CSS 类，加 `transition` 声明最自然。
- **`transition` 属性清单要覆盖「变化的全部属性」**——`src/ui/main.css.test.ts` 里的 `nm-card-hover 过渡对称` 测试就是这个规则的现成范例（hover 改了哪些属性，基类 transition 就必须声明哪些）。
- **网格侧源块是否也要「抬起」**（而不只是变暗）是产品视觉决策，**不在本计划范围**（见 Boundaries）。

## Steps

1. 确认 token 是否已存在：`grep -n -- "--ease-out\|--dur-chip" src/ui/main.css`。
   - 存在 → 用 token 形式。
   - 不存在 → 用字面量形式（`transition: transform 100ms cubic-bezier(0.23, 1, 0.32, 1);`），并在本文件顶部记一笔「计划 001 落地后请替换为 token 形式」。
2. 改 `src/ui/main.css:1070-1077` 的 `.pool-lift`：在 `cursor: grabbing;` 之后加

   ```css
       /* 拖起/收回走 transform 过渡，不再单帧瞬切。
          box-shadow 不进过渡列表：阴影动画是逐帧重绘，
          而拖起这块是「瞬间抬起」的语义，不需要阴影跟着补间。 */
       transition: transform var(--dur-chip) var(--ease-out);
   ```

3. 改 `src/ui/main.css:689-692` 的 `.plan-blk`：在过渡列表末尾补 `opacity var(--dur-chip) var(--ease-out),` 一项。
4. 跑验证（见下）。

## Boundaries

- **不要改** `.pool-lift` 的 `transform` 值（`scale(1.04) translateY(-1px)`）、`box-shadow`、光晕颜色。
- **不要改** `TaskPool.tsx:55-60` 的 `itemCls` 类串结构——本计划不改它的 class 组成。
- **不要给网格侧源块加「抬起」效果**（transform 缩放/位移）——只补 `opacity` 的过渡腿。让两侧的拾起语汇完全对称是产品设计决策，需要单独拍板。
- **不要改** `.plan-blk` 的 `box-shadow` / `transform` / `background-color` 三项既有过渡的时长与曲线。
- **不要动** `PlanBlock.tsx:86` 的 `dimmed ? " opacity-35" : ""` 判定逻辑。
- 不要新增依赖。

## Verification

- **机械**：
  ```bash
  npx tsc --noEmit      # 零错
  npx oxlint src        # 零报
  npm test              # 全绿
  ```
  加一条静态锁（加进 `src/ui/main.css.test.ts`，照该文件现有写法）：

  ```ts
  it("池内拖起有 transform 过渡，网格块有 opacity 过渡腿", () => {
    const poolLift = css.match(/\.pool-lift\s*\{([^}]*)\}/)?.[1] ?? "";
    expect(poolLift).toMatch(/transition:[^;]*transform/);
    const planBlk = css.match(/\.plan-blk\s*\{([^}]*)\}/)?.[1] ?? "";
    expect(planBlk).toMatch(/transition:[^;]*opacity/);
  });
  ```

  注意 `.plan-blk` 的规则里有嵌套写法，正则要能跨行匹配（照文件里 `nm-card-hover` 那条测试的 `[\s\S]*?` 写法）。

- **手感自检**：
  - 任务页，从池里按住一个任务并开始拖：块应当**平滑地抬起**（微微放大 + 上浮），不能是「啪」地跳一下。
  - 把块拖回池里（取消拖拽）：块应当**平滑地收回**原位，不能硬切。
  - 拖起后停住不动：块必须**完全静止**（不能有细微的抖动或呼吸感）——过渡必须结束在终值上。
  - 连续拖起-放下 5 次，每次的抬起观感必须一致。
  - 网格侧：从网格里拖起一个已排期的块，源块应当**平滑变暗**（不再瞬切）。
  - 网格侧：把一个块标记完成（`.is-done`，`opacity: 0.68`）与归档（`.is-archived`，`opacity: 0.88`），透明度变化应当是淡入淡出而不是瞬间变淡。
  - 回归检查：块在网格里的 hover 抬升（`translateY(-1px)`）仍然正常，且不会被新加的 `opacity` 过渡拖慢。
- **完成判据**：
  - `grep -n -A 8 "^  .pool-lift {" src/ui/main.css` 能看到 `transition:` 含 `transform`。
  - `grep -n -A 8 "^  .plan-blk {" src/ui/main.css` 能看到 `transition:` 含 `opacity`。
  - 上面那条测试全绿。

