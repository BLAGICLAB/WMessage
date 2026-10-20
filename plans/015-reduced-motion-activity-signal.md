# 015 — 减少动效下保住「还在跑」的活动信号

- **Status**: 已完成
- **Commit**: b22387c7（计划锚定）→ 落地于本次提交
- **Severity**: MEDIUM
- **Category**: §6 Accessibility
- **Estimated scope**: 1 个 CSS + 8 个 tsx，约 30 行改动

## Problem

全局的 reduced-motion 兜底块把**所有**动画时长和迭代次数都压成 0.01ms：

```css
/* src/ui/main.css:110-120 —— 当前 */
/* 动效克制（U4）：系统声明减少动效时，过渡/动画一律瞬时化 */
@media (prefers-reduced-motion: reduce) {
  *,
  *::before,
  *::after {
    transition-duration: 0.01ms !important;
    animation-duration: 0.01ms !important;
    animation-iteration-count: 1 !important;
    scroll-behavior: auto !important;
  }
}
```

这条策略本身是**既定的产品决策**（「动效克制（U4）」），本计划不推翻它。问题出在它的**副作用**：

`animation-duration: 0.01ms` + `animation-iteration-count: 1` 意味着旋转类动画**播一次就停在终态**。Tailwind 的 `animate-spin` 的终态是 `rotate(360deg)`，也就是回到起点——于是**转子停成一个静止的图标**。

全仓有 **12 处 `animate-spin`**（加载/运行中的指示器）：

```
src/components/TagConsolidateModal.tsx:139
src/components/TagConsolidateModal.tsx:263
src/components/SettingsPage/UsageStatsCard.tsx:825
src/components/SettingsPage/OperationHistoryTab.tsx:74
src/components/SettingsPage/OperationHistoryTab.tsx:120
src/components/AppsPage/AppsPage.tsx:334
src/components/ChatPanel/FilePreview.tsx:129
src/components/ChatPanel/LiveIndicator.tsx:46
src/components/ChatPanel/SubagentRowLine.tsx:19
src/components/ChatPanel/MessageList.tsx:326
src/components/ChatPanel/MessageList.tsx:376
src/components/ChatPanel/ContextRail.tsx:347
```

外加 2 个「还在动」的小圆点（`ContextRail.tsx:176`、`ActivityPage/index.tsx:152`，都是 `animate-pulse`）。

**这个信号是承载信息的，不是装饰。** 代码自己的注释就写明了：

```ts
/* src/components/ChatPanel/MessageList.tsx:322-323 —— 原文 */
// 静态「…」在 300s 级的工具里与「已结束」在视觉上无区别——
// 这是用户看不出 agent 在不在跑的主因
```

```ts
/* src/components/ChatPanel/LiveIndicator.tsx:3-6 —— 原文 */
// 原因需要它：原先 header 只有一个 animate-pulse 小圆点 + 「正在回复」四个字，
// 它由 viewedBusy 驱动、整个 turn 恒为 true —— 于是它在工具执行期**确实亮着
// 但完全不变**，300s 的 pip install 与 0.1s 的 list_files 长得一模一样。
// 「亮着」不等于「有信息」，静默一样会让人以为卡死。
```

也就是说：**减少动效的用户（以及任何把动画时长压到 0 的环境）看到的是一个静止的图标**，「正在跑」和「已经结束」长得一模一样——恰好是这段注释在费劲解决的问题。

动效规则说得很直接：**减少动效是「更少、更轻」的动画，不是「零」——保留帮助理解的反馈，去掉位移。** 旋转是位移类（它让元素动起来），所以该去掉；但「这个指示器代表还有事情在发生」是**可理解性**的一部分，该保留。

## Target

**给活动指示器一个自己的类**：正常情况旋转；减少动效时**换成不涉及位移的透明度呼吸**——信号还在，但不再有「转」这个动作。

**一、CSS 侧**（加进 `src/ui/main.css` 的 `@layer components` 块）：

```css
  /* 活动指示器（加载 / 运行中）：默认旋转。
     减少动效时换成不涉及位移的透明度呼吸——旋转是位移，该去掉；
     但「还有事情在发生」这个信号必须留下（§6：保留帮助理解的反馈）。 */
  .wmsg-activity {
    animation: wmsg-spin 1s linear infinite;
  }
  @keyframes wmsg-spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .wmsg-activity {
      animation: wmsg-breathe 1.6s ease-in-out infinite;
    }
    @keyframes wmsg-breathe {
      0%,
      100% {
        opacity: 0.35;
      }
      50% {
        opacity: 1;
      }
    }
  }
```

**关键点**：`.wmsg-activity` 的选择器特异性（0,1,0）高于全局兜底块里的 `*`（0,0,0），且它用 `animation` 简写一次性覆盖 `animation-duration` 与 `animation-iteration-count`——所以它在 reduced-motion 下能赢过兜底块。

**二、TSX 侧**：12 处 `animate-spin` 换成 `wmsg-activity`。

```
src/components/TagConsolidateModal.tsx:139        className="animate-spin"           → className="wmsg-activity"
src/components/TagConsolidateModal.tsx:263        className="inline animate-spin mr-1" → className="inline wmsg-activity mr-1"
src/components/SettingsPage/UsageStatsCard.tsx:825 className={loading ? "animate-spin" : ""} → className={loading ? "wmsg-activity" : ""}
src/components/SettingsPage/OperationHistoryTab.tsx:74  className="animate-spin"    → className="wmsg-activity"
src/components/SettingsPage/OperationHistoryTab.tsx:120 className="animate-spin"    → className="wmsg-activity"
src/components/AppsPage/AppsPage.tsx:334            className="animate-spin"           → className="wmsg-activity"
src/components/ChatPanel/FilePreview.tsx:129        className="animate-spin"           → className="wmsg-activity"
src/components/ChatPanel/LiveIndicator.tsx:46       className="animate-spin text-[var(--brand)]" → className="wmsg-activity text-[var(--brand)]"
src/components/ChatPanel/SubagentRowLine.tsx:19     className="shrink-0 animate-spin text-[var(--brand)]" → className="shrink-0 wmsg-activity text-[var(--brand)]"
src/components/ChatPanel/MessageList.tsx:326        className="animate-spin"           → className="wmsg-activity"
src/components/ChatPanel/MessageList.tsx:376        className="animate-spin"           → className="wmsg-activity"
src/components/ChatPanel/ContextRail.tsx:347       className="shrink-0 animate-spin text-[var(--brand)]" → className="shrink-0 wmsg-activity text-[var(--brand)]"
```

**三、两个活动小圆点**同样换成呼吸版（`animate-pulse` 本身就是透明度动画，被压成 0.01ms 后同样冻结）：

```tsx
/* src/components/ChatPanel/ContextRail.tsx:176 —— 目标 */
                    className="h-1.5 w-1.5 shrink-0 wmsg-activity rounded-full bg-[var(--brand)]"
```

```tsx
/* src/components/ActivityPage/index.tsx:152 —— 目标 */
                        className="mr-1 inline wmsg-activity"
```

**注意**：`ContextRail.tsx:176` 的圆点用 `wmsg-activity` 时，尺寸/圆角/颜色不变，只是动画从「旋转」变成「呼吸」——圆点旋转是不可见的（正圆），所以给它一个专门的活动类更合适。

## Repo conventions to follow

- **全局 reduced-motion 兜底块（`main.css:110-120`）不动** —— 那是既定的产品策略（注释写着「动效克制（U4）」）。本计划是在它**之下**加一个更精确的例外，不是改它。
- **`.wmsg-*` 是本仓库给应用自有动画的命名前缀**（现成先例：`.wmsg-live-label`、`@keyframes wmsg-shimmer`，见 `main.css:1018` 与 `LiveIndicator.tsx:47`）。新类沿用这个前缀。
- **图标本身、尺寸、颜色一律不动** —— 只换动画类。
- **`BingCard.tsx:87` 的 `animate-pulse` 不在本计划范围** —— 那是必应图的**骨架屏**（加载占位），冻结成静态骨架是可以接受的（骨架本来就该是静态的），不是「活动信号」。

## Steps

1. 在 `src/ui/main.css` 的 `@layer components` 块里加 `.wmsg-activity` 规则 + `@keyframes wmsg-spin` + reduced-motion 下的 `@keyframes wmsg-breathe`。
2. 按上面清单，把 12 处 `animate-spin` 换成 `wmsg-activity`（逐个文件、逐行改，保留各自的其它类）。
3. 把 `ContextRail.tsx:176` 与 `ActivityPage/index.tsx:152` 的 `animate-pulse` 换成 `wmsg-activity`。
4. 跑验证（见下）。

## Boundaries

- **不要改** `src/ui/main.css:110-120` 的全局 reduced-motion 兜底块。
- **不要改** `main.css:991-995`（`.archive-locate { animation: none; }`）与 `:1026-1030`（`.wmsg-live-label { animation: none; }`）这两条已有的定向覆盖——它们是别的功能（定位脉冲、流光文字）的专门处理，与活动指示器无关。
- **不要改** `BingCard.tsx:87` 的骨架屏 `animate-pulse`。
- **不要改** 任何图标的 `size`、颜色、其它类名。
- **不要**给 `.wmsg-activity` 加 `transform`（减少动效下只剩 `opacity` 变化，旋转是位移，要去掉）。
- 不要新增依赖。

## Verification

- **机械**：
  ```bash
  npx tsc --noEmit      # 零错
  npx oxlint src        # 零报
  npm test              # 全绿
  ```
  加两条静态锁（加进 `src/ui/main.css.test.ts`）：

  ```ts
  it("活动指示器有 reduce 下的呼吸态（信号不能消失）", () => {
    expect(css).toMatch(/\.wmsg-activity\s*\{[^}]*animation:\s*wmsg-spin/);
    expect(css).toMatch(/@media \(prefers-reduced-motion: reduce\)\s*\{[\s\S]*?\.wmsg-activity\s*\{[^}]*wmsg-breathe/);
    // 呼吸态只动 opacity，不动 transform
    const breathe = css.match(/@keyframes wmsg-breathe\s*\{([\s\S]*?)\n  \}/)?.[1] ?? "";
    expect(breathe).not.toContain("transform");
  });
  ```

  ```ts
  it("没有残留的 animate-spin（活动指示器统一走 .wmsg-activity）", () => {
    const files = listFiles("src").filter((f) => f.endsWith(".tsx") && !f.includes(".test."));
    const offenders: string[] = [];
    for (const f of files) {
      const src = readFileSync(f, "utf-8");
      if (src.includes("animate-spin")) offenders.push(f);
    }
    expect(offenders).toEqual([]);
  });
  ```

- **手感自检**：
  - DevTools → Rendering → `Emulate prefers-reduced-motion: reduce`，然后触发任意一个加载态（设置页运行历史、程序页、聊天发消息）：**指示器必须仍然在动**（变淡→变亮→变淡），不能是一个静止的图标。
  - 关掉 reduced-motion 模拟：指示器应当恢复**旋转**。
  - 聊天界面发一条会触发工具调用的消息：头部「正在思考」旁的转子在减少动效下应当**呼吸**而不是旋转；而它旁边的**每秒刷新的耗时数字必须继续跳动**（那本来就是信息）。
  - **回归检查（关键）**：正常模式下所有 12 处指示器仍然是旋转的，不能因为换了类而变成不转或转得更快/更慢（旋转周期仍是 1s）。
  - 减少动效下，所有**非活动**动效（hover 过渡、弹窗入场、进度条）仍应保持「瞬时化」——本计划不能意外把它们放开了。
- **完成判据**：
  - `grep -rn "animate-spin" src/ --include=*.tsx` 返回空。
  - `grep -n "wmsg-activity" src/ui/main.css` 命中 2 处以上（规则 + reduced-motion 覆盖）。
  - 上面两条测试全绿。


## 落地记录：计划给的 CSS 会被兜底块压死，加 `!important` 才成立

计划的核心判断（减少动效下转子冻结成静止图标，「还在跑」和「已经结束」长得一样）是对的，
但**计划给的 CSS 写法不生效**——这一点是实测出来的，不是推测。

### 一、计划原版为什么不行

计划的写法是：

```css
@media (prefers-reduced-motion: reduce) {
  .wmsg-activity {
    animation: wmsg-breathe 1.6s ease-in-out infinite;
  }
}
```

计划自己写了一句「`.wmsg-activity` 的特异性（0,1,0）高于全局兜底块里的 `*`（0,0,0），
所以它能赢过兜底块」——**这句话漏了 `!important`**。全局兜底块是：

```css
@media (prefers-reduced-motion: reduce) {
  *,
  *::before,
  *::after {
    animation-duration: 0.01ms !important;
    animation-iteration-count: 1 !important;
  }
}
```

`!important` 的声明在任何特异性、任何层之前都赢。所以 `.wmsg-activity` 的
`animation-duration: 1.6s` 与 `animation-iteration-count: infinite` 会被兜底块压成
`0.01ms` 与 `1`——动画只播一帧就冻结，等于没改。

**Chromium 实测**（`--force-prefers-reduced-motion --dump-dom`，读 computed style）：

| 写法 | animation-name | duration | iteration-count |
|---|---|---|---|
| 计划原版（无 `!important`） | `wmsg-breathe` | `1e-05s` | `1` ← 冻结 |
| 修正版（带 `!important`） | `wmsg-breathe` | `1.6s` | `infinite` ← 正常 |

顺带确认层结构不是变量：Tailwind v3.4 把 `main.css` 里手写的 `@layer components`
整块展平了（编译产物里 `@layer` 出现 0 次），兜底块与新规则同为无层，
所以这里只比特异性与 `!important`，不必再考虑层的优先级。

### 二、端到端实测（真实编译产物 + 强制减少动效）

编译 `src/ui/main.css` 后在 Chromium 里读 computed style：

| 元素 | 正常模式 | 减少动效 |
|---|---|---|
| `.wmsg-activity`（转子） | `wmsg-spin` 1s infinite | **`wmsg-breathe` 1.6s infinite** |
| `.wmsg-activity-pulse`（圆点/状态图标） | `wmsg-pulse` 2s infinite | **`wmsg-breathe` 1.6s infinite** |
| `.plan-fresh`（落位弹入） | `plan-pop` 0.2s 1 次 | `1e-05s` 1 次 ← 仍被掐断 |
| `.animate-pulse`（必应骨架屏） | `pulse` 2s infinite | `1e-05s` 1 次 ← 仍被掐断 |

活动指示器的信号保住了，非活动动效照旧瞬时化——本计划没有意外放开别的动画。

### 三、拆成两个类：正常模式的形态必须原样

计划说「两个活动小圆点也换成 `wmsg-activity`」。实测下来有两个问题：

1. **`ActivityPage/index.tsx` 的那个不是圆点**，是 lucide 的 `Activity` 图标（心跳线），
   旋转它会变成另一个视觉；而 `ContextRail` 的圆点是正圆，旋转看不见。
2. 两个元素正常模式用的是 `animate-pulse`（透明度呼吸），换成旋转类会让它们**在正常
   模式下丢掉可见的呼吸**——正圆的呼吸是看得见的。

所以拆成两个类，各自保留正常模式原样的动画，只在减少动效时汇合到同一个呼吸态：

- `.wmsg-activity` —— 12 处转子（`animate-spin` → 1s linear infinite，与 Tailwind 一致）
- `.wmsg-activity-pulse` —— 2 处呼吸元素（`animate-pulse` → 2s，与 Tailwind 的周期一致；
  曲线走 `--ease-pulse` token，不手写 Tailwind 默认曲线）

减少动效时两者都换成 `wmsg-breathe`（1.6s ease-in-out，只动 opacity）。

**必应图的骨架屏 `animate-pulse` 不动**——骨架本来就该是静态的，不是活动信号。

### 四、静态锁四条（`src/ui/main.css.test.ts`）

- 活动指示器在减少动效下换成 `wmsg-breathe`，**且那条覆盖必须带 `!important`**
  （这是计划原版失效的那个点，锁住它）。
- 呼吸态的 keyframes 只动 opacity，不含 transform。
- `src/**/*.tsx`（测试文件除外）里不再有 `animate-spin`。
- `animate-pulse` 只剩下 `components/HomePage/BingCard.tsx` 一处（骨架屏）。

四条都做过反向验证：撤掉 `!important`、给呼吸态加 transform、把一处转子改回
`animate-spin`、把圆点改回 `animate-pulse`——各对应一条转红。

扫类名时会先剥掉注释：`LiveIndicator.tsx` 的注释里提过 `animate-pulse`（讲的是改之前
的历史），那是注释不是类名。
