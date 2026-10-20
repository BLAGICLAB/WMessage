# 016 — 归档定位的平滑滚动要听减少动效的话

- **Status**: TODO
- **Commit**: b22387c7
- **Severity**: MEDIUM
- **Category**: §6 Accessibility
- **Estimated scope**: 2 个文件（`ArchivePage.tsx` + 其测试），约 12 行改动

## Problem

「双击归档卡跳到归档区定位该卡」这个功能用 JS API 做平滑滚动：

```tsx
/* src/components/ArchivePage/ArchivePage.tsx:63-67 —— 当前 */
  useEffect(() => {
    if (highlightTaskId) {
      locateRef.current?.scrollIntoView({ behavior: "smooth", block: "center" });
    }
  }, [highlightTaskId, query, activeTag]);
```

而仓库的 reduced-motion 兜底块里有一条滚动守卫：

```css
/* src/ui/main.css:111-120 —— 当前 */
@media (prefers-reduced-motion: reduce) {
  *,
  *::before,
  *::after {
    transition-duration: 0.01ms !important;
    animation-duration: 0.01ms !important;
    animation-iteration-count: 1 !important;
    scroll-behavior: auto !important;   /* ← 这条管不到 JS API */
  }
}
```

**`scroll-behavior: auto` 管不住 `scrollIntoView({ behavior: "smooth" })`。** CSS 的 `scroll-behavior` 属性只影响**由 CSS 驱动**的滚动（锚点跳转、`scrollTo` 不带 behavior 时的默认行为）；而 `scrollIntoView` 的 `behavior` 选项是**显式指定**的，直接覆盖 CSS 属性。

所以：**开了「减少动效」的系统设置，归档定位仍然是平滑滚动的。**

这条守卫的意图是明确的——同一个功能里的另一个动效就有专门的 reduced-motion 处理：

```css
/* src/ui/main.css:991-995 —— 当前 */
  @media (prefers-reduced-motion: reduce) {
    .archive-locate {
      animation: none;
    }
  }
```

紫框脉冲被关掉了，滚动却还在动——**同一个功能的两半，减少动效只照顾了一半**。

## Target

在 JS 里读一次 `prefers-reduced-motion`，命中就用 `behavior: "auto"`。

```tsx
/* 目标 —— src/components/ArchivePage/ArchivePage.tsx:63-67 */
  useEffect(() => {
    if (highlightTaskId) {
      // 减少动效时不做平滑滚动：CSS 的 scroll-behavior: auto !important
      // （main.css:118）管不到 scrollIntoView 的 behavior 选项——它是显式指定，
      // 会直接覆盖 CSS 属性。所以这里必须在 JS 里判一次。
      const reduced =
        typeof window.matchMedia === "function" &&
        window.matchMedia("(prefers-reduced-motion: reduce)").matches;
      locateRef.current?.scrollIntoView({
        behavior: reduced ? "auto" : "smooth",
        block: "center",
      });
    }
  }, [highlightTaskId, query, activeTag]);
```

- `typeof window.matchMedia === "function"` 的守卫是必要的：老 WebView 或测试环境可能没有这个 API。
- `behavior: "auto"` 让浏览器用瞬时跳转（等价于 CSS `scroll-behavior: auto`），这正是 reduced-motion 用户想要的。

**测试环境说明**：`src/test/setup.ts` 已经把 `window.matchMedia` 补成 mock，且**所有查询都返回 `matches: false`**，所以现有那条断言（`behavior: "smooth"`）**不需要改**。本计划只需要**新增**一条 reduced-motion 的用例。

## Repo conventions to follow

- **仓库判断系统偏好已有先例**：`src/theme.ts` 的 `subscribeSystem` 用的就是 `window.matchMedia`（见 `src/test/setup.ts` 里那条 mock 的注释）。保持同样的 API 形状。
- **不要改** `main.css:111-120` 的兜底块，也不要改 `:991-995` 的 `.archive-locate` 覆盖。
- **不要改** 定位态的其它行为（紫框、保留到下一次交互、被筛选挡住时自动清筛选——那段在 `ArchivePage.tsx:55-62`）。
- **不要改** 滚动目标：`block: "center"` 保持。

## Steps

1. 改 `src/components/ArchivePage/ArchivePage.tsx:63-67`，按 Target 节的代码替换。
2. 在 `src/components/ArchivePage/ArchivePage.test.tsx` 的「ArchivePage 跳转定位」describe 块里**新增**一条用例：

   ```ts
   it("减少动效下定位不滚动（瞬时跳转）", () => {
     // 把 matchMedia mock 成「命中减少动效」
     const spy = vi
       .spyOn(window, "matchMedia")
       .mockImplementation(
         (query: string) =>
           ({
             matches: query.includes("prefers-reduced-motion"),
             media: query,
             onchange: null,
             addListener: vi.fn(),
             removeListener: vi.fn(),
             addEventListener: vi.fn(),
             removeEventListener: vi.fn(),
             dispatchEvent: vi.fn(),
           }) as unknown as MediaQueryList,
       );
     renderPage(tasks, { highlightTaskId: "b" });
     expect(Element.prototype.scrollIntoView).toHaveBeenCalledWith({
       behavior: "auto",
       block: "center",
     });
     spy.mockRestore();
   });
   ```

   `vi.restoreAllMocks()` 已在 `src/test/setup.ts` 的 `afterEach` 里全局调用，所以 mock 会被自动还原；显式 `mockRestore()` 只是保险。
3. 跑验证（见下）。

## Boundaries

- **不要改** `src/ui/main.css:111-120` 与 `:991-995`。
- **不要改** 现有的那条 `behavior: "smooth"` 断言（它测的是默认路径，依然成立）。
- **不要改** `block: "center"`。
- **不要动** 归档定位的紫框脉冲（`.archive-locate`）与它的 reduced-motion 覆盖。
- **不要改** `highlightTaskId` 变化时清筛选的逻辑（`:55-62`）。
- 不要新增依赖。

## Verification

- **机械**：
  ```bash
  npx tsc --noEmit      # 零错
  npx oxlint src        # 零报
  npm test              # 全绿
  ```
  新增的那条用例是主要验收点。

- **手感自检**：
  - 系统开启「减少动效」（macOS：系统设置 → 辅助功能 → 显示 → 减少动态效果；Windows：设置 → 辅助功能 → 视觉效果 → 动画效果关闭）后，从时间泳道双击一张归档卡跳到归档区：页面应当**瞬时跳到**目标卡，不能是缓慢滑过去。
  - 关掉「减少动效」再试一次：应当恢复平滑滚动（这是**默认路径的回归检查**，别改坏了）。
  - 定位后目标卡的紫框脉冲：在减少动效下**不出现**（`main.css:991-995` 的既有行为，不要被本计划改掉）；正常模式下出现两拍。
  - 目标卡被筛选挡住时自动清筛选的既有行为不受影响。
- **完成判据**：
  - `grep -n "prefers-reduced-motion" src/components/ArchivePage.tsx` 命中。
  - 新增用例通过，原有用例仍通过。

