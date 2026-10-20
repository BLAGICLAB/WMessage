# 010 — 设置页下拉浮层加入场动效，并从触发点长出来

- **Status**: TODO
- **Commit**: b22387c7
- **Severity**: MEDIUM
- **Category**: §3 Physicality & origin / §2 Easing & duration
- **Estimated scope**: 1 个 CSS + 4 个 tsx，约 14 行改动

## Problem

设置页的四个下拉浮层都是条件挂载的 `nm-popover`，**没有任何入场动效，也没有 transform-origin**——瞬间出现、瞬间消失，不知道从哪来。

```tsx
/* src/components/SettingsPage/ApiProviderSelect.tsx:105-111 —— 当前 */
      {open && (
        <div
          id={listboxId}
          role="listbox"
          className="nm-popover absolute z-50 mt-1 w-full p-1 space-y-0.5"
        >
```

```tsx
/* src/components/SettingsPage/AvatarPicker.tsx:148 —— 当前 */
          className="nm-popover absolute left-0 top-full z-50 mt-2 w-60 p-3"
```

```tsx
/* src/components/SettingsPage/SettingsPage.tsx:3054 —— 当前 */
                                      className="nm-popover absolute right-0 z-50 mt-1 w-40 p-1 space-y-0.5"
```

```tsx
/* src/components/SettingsPage/SettingsPage.tsx:3217 —— 当前 */
                                          className="nm-popover absolute right-0 z-50 mt-1 max-h-56 w-64 space-y-0.5 overflow-y-auto p-1"
```

```css
/* src/ui/main.css:221-229 —— 当前 */
  /* === 浮层面板（下拉/菜单容器）：实色悬浮底 + 大投影，无 hover 换底 ===
     不能用 nm-outset 当浮层底：其 hover 态切半透明 --hover-bg（按钮语义），
     鼠标移入浮层选选项时整个面板会变透明，透出下方内容（API 格式下拉 bug） */
  .nm-popover {
    border-radius: var(--r-md);
    background-color: var(--surface-raised);
    border: 1px solid var(--edge);
    box-shadow: var(--shadow-lg);
  }
```

这四处都是**鼠标触发**的下拉（不是 ⌘K 那种键盘触发的面板——⌘K 不做动效是对的）。动效规则给下拉/浮层的预算是 150–250ms，并要求「浮层从它的触发点长出来，而不是从中心」。

四个浮层还各自锚在不同方向上（`w-full` 居中 / `left-0` 左对齐 / `right-0` 右对齐），但 `.nm-popover` 是共用类——所以**变换原点必须按调用点分别指定**。

## Target

**一、`.nm-popover` 加一条入场动画**（挂载即播，一次性）。

```css
/* 目标 —— src/ui/main.css:221-229 */
  /* === 浮层面板（下拉/菜单容器）：实色悬浮底 + 大投影，无 hover 换底 ===
     不能用 nm-outset 当浮层底：其 hover 态切半透明 --hover-bg（按钮语义），
     鼠标移入浮层选选项时整个面板会变透明，透出下方内容（API 格式下拉 bug）
     入场：从触发点长出来（transform-origin 由调用点用 origin-* 指定），
     缩放起点 0.96 + 淡入，走 ease-out 200ms。 */
  .nm-popover {
    border-radius: var(--r-md);
    background-color: var(--surface-raised);
    border: 1px solid var(--edge);
    box-shadow: var(--shadow-lg);
    animation: popover-in var(--dur-pop) var(--ease-out);
  }
  @keyframes popover-in {
    from {
      opacity: 0;
      transform: scale(0.96);
    }
    to {
      opacity: 1;
      transform: scale(1);
    }
  }
```

- `scale(0.96)` 落在「不凭空出现」的区间（0.9–0.97）里。
- `var(--dur-pop)`（200ms）落在下拉/浮层预算（150–250ms）里。
- `var(--ease-out)` 是进场曲线。

**二、四个调用点各自指定变换原点**（Tailwind 的 `origin-*` 工具类）：

| 调用点 | 锚定方向 | 加的类 |
| --- | --- | --- |
| `ApiProviderSelect.tsx:109` | 触发钮正下方、宽度撑满 | `origin-top` |
| `AvatarPicker.tsx:148` | 触发钮正下方、左对齐 | `origin-top-left` |
| `SettingsPage.tsx:3054` | 触发钮正下方、右对齐 | `origin-top-right` |
| `SettingsPage.tsx:3217` | 触发钮正下方、右对齐 | `origin-top-right` |

```tsx
/* ApiProviderSelect.tsx:109 —— 目标 */
          className="nm-popover absolute z-50 mt-1 w-full p-1 space-y-0.5 origin-top"
```

```tsx
/* AvatarPicker.tsx:148 —— 目标 */
          className="nm-popover absolute left-0 top-full z-50 mt-2 w-60 p-3 origin-top-left"
```

**三、依赖**：`--dur-pop` 与 `--ease-out` 由计划 001 建立。**若计划 001 尚未落地**，写字面量：

```css
    animation: popover-in 0.2s cubic-bezier(0.23, 1, 0.32, 1);
```

## Repo conventions to follow

- **`.nm-popover` 的材质（实色底 + 大投影 + 无 hover 换底）一个字都不许改** —— 注释里写明了原因：换成半透明 hover 底会让浮层变透明、透出下方内容（API 格式下拉的老 bug）。本计划只加 `animation`。
- **`@keyframes` 写在 `@layer components` 块内**（`plan-pop`、`hover-tip-in` 都在那里），不要挪到文件顶层。
- **`animation` 简写里可以放 `var()`**。
- **reduced-motion 已自动生效**：全局块 `src/ui/main.css:111-120` 把 `animation-duration` 压成 `0.01ms !important`，浮层在减少动效下是瞬时出现的（正确行为）。不要为此另写规则。

## Steps

1. 计划 001 已落地：`--ease-out` 存在，`--dur-pop` **还没有**（001 只定义有消费方的 token）。
   先把 `--dur-pop: 200ms; /* 一次性出现（下拉 / 浮层 / 落位弹入） */` 加进 `src/ui/main.css` 的
   `:root` 动效 token 块，再用 token 形式。**不要**回退到字面量形式。
2. 改 `src/ui/main.css:221-229` 的 `.nm-popover`：按 Target 节的写法加 `animation` 一行并更新注释；同时在 `@layer components` 块内加 `@keyframes popover-in`（放在 `.nm-popover` 规则之后即可）。
3. 改四个调用点，各加一个 `origin-*` 类：
   - `src/components/SettingsPage/ApiProviderSelect.tsx:109` → `origin-top`
   - `src/components/SettingsPage/AvatarPicker.tsx:148` → `origin-top-left`
   - `src/components/SettingsPage/SettingsPage.tsx:3054` → `origin-top-right`
   - `src/components/SettingsPage/SettingsPage.tsx:3217` → `origin-top-right`
4. 跑验证（见下）。

## Boundaries

- **不要改** `.nm-popover` 的 `background-color` / `border` / `box-shadow` / `border-radius`。
- **不要给浮层加退场动画** —— 退场需要「挂载/卸载握手」（监听 animationend 再卸载），会把这四个调用点的结构都改掉，超出本计划范围。进场动画已经解决了「不知道从哪来」的问题。
- **不要改** 四个浮层的定位（`absolute` / `z-50` / `mt-1` / `mt-2` / `left-0` / `right-0` / `w-full` / `w-40` / `w-60` / `w-64`）与内层结构。
- **不要动** `ApiProviderSelect` 的下拉箭头旋转（`:100` 的 `transition-transform` + `rotate-180`）——那是独立的一条动效。
- **不要动** `AvatarPicker` 的相机角标（`group-hover:flex`）。
- **不要给聊天输入栏的下拉做本计划** —— 那是另一个区域（`InputArea.tsx:352/417/485`），需要单独评估（它们是聊天里最热的控件，动画预算要更保守）。
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
  it("浮层有入场动效且缩放起点在 0.9–0.97", () => {
    const popover = css.match(/\.nm-popover\s*\{([^}]*)\}/)?.[1] ?? "";
    expect(popover).toMatch(/animation:\s*popover-in/);
    const kf = css.match(/@keyframes popover-in\s*\{[\s\S]*?scale\(([\d.]+)\)/)?.[1];
    expect(parseFloat(kf ?? "0")).toBeGreaterThanOrEqual(0.9);
    expect(parseFloat(kf ?? "0")).toBeLessThanOrEqual(0.97);
  });
  ```

  再加一条调用点检查（读 tsx 源文件）：

  ```ts
  it("四个浮层调用点各自指定了变换原点", () => {
    for (const [f, origin] of [
      ["src/components/SettingsPage/ApiProviderSelect.tsx", "origin-top"],
      ["src/components/SettingsPage/AvatarPicker.tsx", "origin-top-left"],
      ["src/components/SettingsPage/SettingsPage.tsx", "origin-top-right"],
    ] as const) {
      expect(readFileSync(f, "utf-8")).toContain(origin);
    }
  });
  ```

- **手感自检**：
  - 设置页 → 模型厂商下拉（`ApiProviderSelect`）：浮层应当**从触发钮正下方长出来**，而不是从自己中心胀开。视觉上浮层上边缘应当"长"在按钮下边缘上。
  - 设置页 → 头像选择器（`AvatarPicker`）：浮层从**左上角**（贴着头像钮的左边缘）长出来。
  - 设置页里两个右对齐下拉（`SettingsPage.tsx:3054` / `:3217`）：从**右上角**长出来。
  - 打开/关闭反复点：每次打开都应有一段一致的入场，不能第一次有、后面没有。
  - DevTools → Animations 面板，playback 调到 10%：浮层应当**从 0.96 缩放 + 淡入**，起点不是 0（不能凭空出现）。
  - 鼠标移入浮层内部（选选项）：浮层底色**不能变透明**（这是 `.nm-popover` 注释里点名过的老 bug，回归检查）。
  - DevTools → Rendering → `Emulate prefers-reduced-motion: reduce`：浮层应瞬时出现，无过渡。
- **完成判据**：
  - `grep -n "popover-in" src/ui/main.css` 命中 2 处（规则 + keyframes）。
  - 四个调用点各自带上了正确的 `origin-*`。
  - 上面两条测试全绿。

