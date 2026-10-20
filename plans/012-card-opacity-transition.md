# 012 — 整卡透明度切换补上 opacity 过渡腿

- **Status**: TODO
- **Commit**: b22387c7
- **Severity**: MEDIUM
- **Category**: §3 Physicality & origin / §8 Missed opportunities
- **Estimated scope**: 2 个文件（`main.css` + `TagConsolidateModal.tsx`），约 6 行改动

## Problem

有三处用「整卡降到 60% 不透明度」表达状态变化，但过渡机制没跟上——**整张卡在两帧之间硬切**。

**前两处是 `.nm-card`**，而 `.nm-card` 的过渡列表里根本没有 `opacity` 这一腿：

```css
/* src/ui/main.css:162-171 —— 当前 */
  .nm-card,
  .nm-card-hover {
    border-radius: var(--r-lg);
    background-color: var(--surface);
    border: 1px solid var(--edge);
    box-shadow: var(--shadow-sm);
    transition:
      background-color 0.15s ease,
      border-color 0.15s ease;
  }
```

```tsx
/* src/components/SchedulePage/SchedulePage.tsx:252-256 —— 当前 */
      <div
        key={key}
        data-testid={`schedule-row-${key}`}
        className={`nm-card p-3 ${e.enabled ? "" : "opacity-60"}`}
      >
```

```tsx
/* src/components/NotificationsPage/NotificationsPage.tsx:426-430 —— 当前 */
              <article
                key={n.id}
                className={`nm-card p-4 ${n.status !== "pending" ? "opacity-60" : ""}`}
                aria-label={`${meta.label}：${n.title}`}
              >
```

所以：把一条日程**暂停**，整张卡瞬间变灰；一条通知**状态变更**，整张卡瞬间变灰。元素上明明已经架好了过渡机制（同级的 background-color / border-color 都在补间），只是**少声明了 opacity 这一条腿**。

**第三处**是标签合并弹窗里的簇条目，它连 `.nm-card` 都不是，是个裸 `<li>`，压根没有任何过渡声明：

```tsx
/* src/components/TagConsolidateModal.tsx:158-166 —— 当前 */
                  <li
                    key={c.members.join("|")}
                    className={`rounded-lg p-3 border ${
                      sel.enabled
                        ? "border-[var(--bd)] bg-[var(--bg)]"
                        : "border-dashed border-[var(--bd)] bg-transparent opacity-60"
                    }`}
                  >
```

勾选/取消勾选时，边框（实线 ↔ 虚线）、底色（实色 ↔ 透明）、不透明度**三项同时瞬切**。

## Target

**一、给 `.nm-card` 的过渡列表补上 `opacity` 一腿**（一处改动，修好两处调用点）：

```css
/* 目标 —— src/ui/main.css:168-170 */
    transition:
      background-color var(--dur-card) var(--ease-out),
      border-color var(--dur-card) var(--ease-out),
      opacity var(--dur-card) var(--ease-out);
```

**依赖**：token 由计划 001 建立。**若计划 001 尚未落地**，写字面量：

```css
    transition:
      background-color 0.15s ease,
      border-color 0.15s ease,
      opacity 0.15s ease;
```

**二、给标签合并弹窗的 `<li>` 补上过渡**（覆盖它同时变化的三条属性）：

```tsx
/* 目标 —— src/components/TagConsolidateModal.tsx:158-166 */
                  <li
                    key={c.members.join("|")}
                    className={`rounded-lg p-3 border transition-[opacity,border-color,background-color] duration-[var(--dur-card)] ease-[var(--ease-out)] ${
                      sel.enabled
                        ? "border-[var(--bd)] bg-[var(--bg)]"
                        : "border-dashed border-[var(--bd)] bg-transparent opacity-60"
                    }`}
                  >
```

（`transition-[opacity,border-color,background-color]` 是 Tailwind 的任意属性列表；`duration-[var(--dur-card)]` 与 `ease-[var(--ease-out)]` 是任意值类。**若计划 001 尚未落地**，写成 `transition-[opacity,border-color,background-color] duration-150 ease-[cubic-bezier(0.23,1,0.32,1)]`。）

## Repo conventions to follow

- **`.nm-card` 的过渡列表有静态结构锁测试**：`src/ui/main.css.test.ts` 的「main.css nm-card-hover 过渡对称」那组断言用正则匹配 `transition: background-color 0.15s ease, border-color 0.15s ease;`——**加了一条 `opacity` 之后那条正则会失配，必须一起更新**（见 Verification）。
- **该测试的意图**（注释原文）：「hover 块改了哪些属性，基类 transition 必须同样声明，否则鼠标进入平滑、离开瞬间弹回」。本计划是同一条规则的延伸：`.nm-card` 的子类会改 `opacity`，基类就必须声明 `opacity`。
- **`.nm-card` 的视觉（圆角、底色、边框、投影）一个字都不许改。**
- **不用 `transition-all`。**

## Steps

1. 确认 token 是否已存在：`grep -n -- "--ease-out\|--dur-card" src/ui/main.css`。
   - 存在 → 用 token 形式；不存在 → 用字面量形式并在本文件顶部记一笔。
2. 改 `src/ui/main.css:168-170` 的 `.nm-card` 过渡列表，按 Target 第一节替换（加 `opacity` 腿）。
3. 改 `src/components/TagConsolidateModal.tsx:158-166` 的 `<li>` 类串，按 Target 第二节替换。
4. **更新 `src/ui/main.css.test.ts` 里那条已存在的正则**，让它容纳新增的 `opacity` 腿。原断言：

   ```ts
     expect(css).toMatch(
       /transition:\s*background-color 0\.15s ease,\s*border-color 0\.15s ease;[\s\S]*?\.nm-card-hover:hover/,
     );
   ```

   改成（用 token 形式的话）：

   ```ts
     expect(css).toMatch(
       /transition:\s*background-color var\(--dur-card\) var\(--ease-out\),\s*border-color var\(--dur-card\) var\(--ease-out\),\s*opacity var\(--dur-card\) var\(--ease-out\);[\s\S]*?\.nm-card-hover:hover/,
     );
   ```

   若计划 001 未落地（仍是字面量），改成：

   ```ts
     expect(css).toMatch(
       /transition:\s*background-color 0\.15s ease,\s*border-color 0\.15s ease,\s*opacity 0\.15s ease;[\s\S]*?\.nm-card-hover:hover/,
     );
   ```

   **注意**：这条测试同时在验证「hover 块没有引入 transform / box-shadow」，加 `opacity` 腿不影响那两条断言。
5. 跑验证（见下）。

## Boundaries

- **不要改** `.nm-card` 的 `background-color` / `border` / `box-shadow` / `border-radius`。
- **不要改** `SchedulePage.tsx:255` 与 `NotificationsPage.tsx:429` 的 `opacity-60` 判定逻辑和条件——本计划只补过渡，不改语义。
- **不要改** `TagConsolidateModal.tsx` 的簇选择逻辑、复选框行为、合并流程。
- **不要动** `.nm-card-hover:hover` 的规则（它只改 background-color 和 border-color，不改 opacity，**不要**给它加 opacity 腿）。
- **不要动** `main.css.test.ts` 里「hover 块不含 transform / box-shadow」那两条断言。
- 不要新增依赖。

## Verification

- **机械**：
  ```bash
  npx tsc --noEmit      # 零错
  npx oxlint src        # 零报
  npm test              # 全绿
  ```
  加一条静态锁（加进 `src/ui/main.css.test.ts`）：

  ```ts
  it("nm-card 的过渡覆盖 opacity（整卡降透明度要补间）", () => {
    const block = css.match(/\.nm-card,\s*\.nm-card-hover\s*\{([^}]*)\}/)?.[1] ?? "";
    expect(block).toMatch(/transition:[^;]*opacity/);
  });
  ```

- **手感自检**：
  - 日程页：把一条日程**暂停**（切 `enabled`），整张卡应当**平滑地淡到 60%**，不能瞬间变灰。再恢复，同样平滑。
  - 通知页：把一条通知的状态改成非 pending，整张卡应当平滑变淡。
  - 标签合并弹窗：勾选/取消一个簇，边框（实线↔虚线）、底色、不透明度应当**一起平滑过渡**，不能三项各自瞬切。
  - **回归检查（关键）**：`.nm-card-hover` 的 hover 效果仍然只有 background-color 与 border-color 在补间，hover 块仍然不含 transform / box-shadow（那条既有测试就是守这个的）。
  - 深色模式下重复上面的操作：卡片变淡时不能出现「文字突然看不清」的突兀——60% 不透明度在深色下本来就更低，确认这是既有观感、不要顺手调数值。
- **完成判据**：
  - `grep -n -A 10 "^  .nm-card," src/ui/main.css` 能看到 `opacity` 出现在过渡列表里。
  - `npm test` 全绿（含更新后的那条正则）。

