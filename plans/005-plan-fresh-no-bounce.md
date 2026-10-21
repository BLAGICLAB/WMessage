# 005 — 计划块落位弹入：去掉过冲曲线，回到「无弹跳」纪律

- **Status**: 已完成
- **Commit**: b22387c7（计划锚定）→ 落地于本次提交
- **Severity**: MEDIUM
- **Category**: §7 Cohesion & tokens / §3 Physicality & origin
- **Estimated scope**: 1 个 CSS，约 6 行改动

## Problem

新排期块落位时播放 `.plan-fresh` 弹入动画，用的曲线**带过冲**：

```css
/* src/ui/main.css:1012-1015 —— 当前 */
  /* 落位弹入（批 3：新排期块） */
  .plan-fresh {
    animation: plan-pop 0.22s cubic-bezier(0.2, 1.4, 0.4, 1);
  }
```

```css
/* src/ui/main.css:1031-1040 —— 当前 */
  @keyframes plan-pop {
    0% {
      transform: scale(0.88);
      opacity: 0;
    }
    100% {
      transform: scale(1);
      opacity: 1;
    }
  }
```

两个问题：

1. **曲线过冲**：控制点 `y = 1.4` > 1，块会先放大过头再缩回来。这直接违反仓库自己写在 `src/ui/main.css:158` 的纪律：

   ```css
      动效纪律：卡片 150ms，按钮/ Chips 100ms，无弹跳
   ```

   「无弹跳」是这个产品的性格基线（扁平、Linear 式、克制的仪表盘），而这是全仓**唯一**一条带过冲的曲线。

2. **起点缩放比过小**：`scale(0.88)` 低于动效规则给出的区间（`scale(0.9–0.97)` + `opacity: 0`）。0.88 意味着块从 88% 大小弹进来，视觉上更像「从远处飞来」而不是「落位就位」。

顺带一个观察（不在本计划的必修范围，但验收时要看）：`@keyframes` 在 CSS 级联里的优先级高于普通作者声明，所以这 220ms 期间 `.plan-blk:hover` 的 `transform: translateY(-1px)`（`main.css:704-706`）会被压住——块弹入的这段时间里它不响应 hover。缩短时长能减轻这个症状。

## Target

用仓库的强 ease-out 曲线、0.94 的起点缩放、200ms 时长。

```css
/* 目标 */
  /* 落位弹入（批 3：新排期块）：无弹跳，起步 0.94 缩放 + 淡入 */
  .plan-fresh {
    animation: plan-pop var(--dur-pop) var(--ease-out);
  }
```

```css
/* 目标 */
  @keyframes plan-pop {
    0% {
      transform: scale(0.94);
      opacity: 0;
    }
    100% {
      transform: scale(1);
      opacity: 1;
    }
  }
```

三个值的依据：

- `scale(0.94)` —— 落在 `scale(0.9–0.97)` 区间中段。比 0.88 收敛，比 0.97 更有「落位感」。
- `var(--dur-pop)`（200ms）—— 落位弹入与下拉/浮层同属「一次性出现」类，预算 150–250ms。原值 220ms 也在预算内，改成 200ms 是为了对齐 token 阶梯。
- `var(--ease-out)`（`cubic-bezier(0.23, 1, 0.32, 1)`）—— 进场用 ease-out，起步快。**无过冲**，符合「无弹跳」纪律。

**依赖**：两个 token 由计划 001 建立。**若计划 001 尚未落地**，本计划写成字面量，功能等价：

```css
  .plan-fresh {
    animation: plan-pop 0.2s cubic-bezier(0.23, 1, 0.32, 1);
  }
```

两条计划都落地时以 token 形式为准，不要两个都留。

## Repo conventions to follow

- **纪律写在 `src/ui/main.css:157-158`**，改任何曲线前先读它——「无弹跳」是既定的产品性格，不是本计划要挑战的对象。
- **`@keyframes` 统一放在 `@layer components` 块内**（`plan-pop` 已在其中，`:1031`），不要挪到文件顶层。
- **`animation` 简写里可以直接放 `var()`**——`animation: plan-pop var(--dur-pop) var(--ease-out)` 是合法 CSS。
- **`.plan-fresh` 的挂载时机不动**：`markFresh(drag.task.id)`（`TaskTimelinePage.tsx:300`）在落位后给块加上这个类，触发一次性入场。类名、挂载点、时长节奏都不属于本计划。

## Steps

1. 计划 001 已落地：`--ease-out` 存在，`--dur-pop` **还没有**（001 只定义有消费方的 token，死 token 不进）。
   先把这一行加进 `src/ui/main.css` 的 `:root` 动效 token 块：
   `--dur-pop: 200ms; /* 一次性出现（下拉 / 浮层 / 落位弹入） */`
   然后本计划用 token 形式。**不要**回退到字面量形式（那等于又漏出一处手写值）。
2. 改 `src/ui/main.css:1013-1015` 的 `.plan-fresh` 规则：

   ```css
     /* 落位弹入（批 3：新排期块）：无弹跳，起步 0.94 缩放 + 淡入 */
     .plan-fresh {
       animation: plan-pop var(--dur-pop) var(--ease-out);
     }
   ```

3. 改 `src/ui/main.css:1031-1040` 的 `@keyframes plan-pop`：`0%` 的 `transform: scale(0.88)` 改成 `transform: scale(0.94)`。其余不动。
4. 跑验证（见下）。

## Boundaries

- **不要动** `plan-pop` 的 keyframe 名字、关键帧百分比结构（`0%` / `100%`）、`opacity` 腿。
- **不要动** `.plan-fresh` 的挂载逻辑（`TaskTimelinePage.tsx:300` 的 `markFresh`）。
- **不要动** `.plan-blk` 的常规样式（`main.css:676-693`）——那是计划 008 的地盘。
- **不要给这条动画加弹跳**（比如把 y 控制点调到 1.1 以上）。「无弹跳」是纪律。
- **不要新增依赖**。

## Verification

- **机械**：
  ```bash
  npx tsc --noEmit      # 零错
  npx oxlint src        # 零报
  npm test              # 全绿
  ```
  加一条静态锁（可加进 `src/ui/main.css.test.ts`，照该文件现有写法——它用 `readFileSync("src/ui/main.css", "utf-8")` 读源文件）：

  ```ts
  it("落位弹入无过冲（遵守「无弹跳」纪律）", () => {
    expect(css).toMatch(/\.plan-fresh\s*\{[^}]*animation:\s*plan-pop[^}]*\}/);
    // 曲线里 y 控制点不得 > 1
    const block = css.match(/\.plan-fresh\s*\{([^}]*)\}/)?.[1] ?? "";
    const curve = block.match(/cubic-bezier\(([^)]*)\)/)?.[1];
    if (curve) {
      const [, y1, y2] = curve.split(",").map((n) => parseFloat(n));
      expect(y1).toBeLessThanOrEqual(1);
      expect(y2).toBeLessThanOrEqual(1);
    }
    // 起点缩放比落在 0.9–0.97
    const kf = css.match(/@keyframes plan-pop\s*\{[\s\S]*?scale\(([\d.]+)\)/)?.[1];
    expect(parseFloat(kf ?? "0")).toBeGreaterThanOrEqual(0.9);
    expect(parseFloat(kf ?? "0")).toBeLessThanOrEqual(0.97);
  });
  ```

- **手感自检**：
  - 任务页，从右侧池里拖一个块到周网格落位：块应当**干脆就位**，不能先胀大再缩回。
  - 慢放看（DevTools → Animations 面板 playback 调到 10%）：缩放曲线必须**单调上升**到 1，不存在超过 1 再回落的那一段。
  - 连落 3–4 个块，每次的观感必须一致，且与整页其它动效（无弹跳）风格统一。
  - 落位后立刻把指针悬在块上：hover 抬升应当立刻恢复响应（时长从 220ms 缩到 200ms 后这个窗口也短了一点）。
  - 对比落位弹入与拖拽中的 `.drag-ghost` 跟随：两者不应有「一个弹一个不弹」的割裂感。
- **完成判据**：
  - `grep -n "cubic-bezier(0.2, 1.4, 0.4, 1)" src/ui/main.css` 返回空。
  - `grep -n "scale(0.88)" src/ui/main.css` 返回空。
  - 上面那条测试全绿。

