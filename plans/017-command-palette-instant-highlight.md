# 017 — ⌘K 命令面板的键盘高亮改为瞬时

- **Status**: 已完成
- **Commit**: b22387c7（计划锚定）→ 落地于 `07526ffd`
- **Severity**: MEDIUM
- **Category**: §1 Purpose & frequency / §7 Cohesion
- **Estimated scope**: 1 个 tsx，1 行改动

## Problem

⌘K 命令面板的**开合**是零动效的——这一点完全正确：

```tsx
/* src/App.tsx:917 / :1079 —— 条件挂载，无动画 */
{open && <CommandPalette … />}
```

动效规则对「每天按 100 次以上的键盘操作」的判词是**永不动效**（Raycast 就一个动画都没有）。开合做对了。

**但面板内部唯一的一条动效，是键盘选中高亮的过渡：**

```tsx
/* src/components/CommandPalette.tsx:181-192 —— 当前 */
                <button
                  // 键盘导航集中在搜索框（↑↓/回车），结果行不进 Tab 序
                  tabIndex={-1}
                  data-cmd-active={i === active || undefined}
                  onClick={() => choose(row)}
                  onMouseEnter={() => setActive(i)}
                  className={`flex w-full items-center gap-2.5 rounded-[var(--r-sm)] px-2.5 py-2 text-left text-sm transition-colors duration-100 ${
                    i === active
                      ? "bg-[var(--hover-bg)] text-[var(--t1)]"
                      : "text-[var(--t3)]"
                  }`}
                >
```

后果有两个：

1. **高亮滞后于光标**。按 ↓ 键，焦点行应该**立刻**亮起来；但 100ms 的背景过渡意味着高亮在 100ms 后才追上。在这 100ms 里用户看到的是「旧的还亮着、新的还没亮」。
2. **双重曝光**。过渡窗口内，**旧行还在褪色、新行还在上色**，同一时刻两行都带着底色——这正是动效规则里点名的「crossfade that visibly double-exposes」。

而且这条过渡对**鼠标悬停**是有用的（鼠标划过时底色渐变很自然），对**键盘导航**则是负担。同一个 `active` 状态被两条路径共用（`onMouseEnter` 也调 `setActive`），所以没法靠状态区分。

**怎么办？** 键盘驱动的表面就该是**瞬时**的——把过渡整个去掉，鼠标悬停也跟着变成瞬时。这正是 Raycast 的做法：列表项的悬停底色是瞬间出现的，快到用户感觉不到「有过渡」。在一个键盘优先的搜索面板里，**瞬时反馈反而比渐变更跟手**。

## Target

去掉结果行的过渡类。

```tsx
/* 目标 —— src/components/CommandPalette.tsx:187 */
                  className={`flex w-full items-center gap-2.5 rounded-[var(--r-sm)] px-2.5 py-2 text-left text-sm ${
                    i === active
                      ? "bg-[var(--hover-bg)] text-[var(--t1)]"
                      : "text-[var(--t3)]"
                  }`}
```

只删 `transition-colors duration-100` 这两个类，**其余一个字不动**。

**执行顺序（重要）**：本计划**必须在计划 001 之后执行**。计划 001 会把这一行改写成 `transition-colors duration-[var(--dur-chip)] ease-[var(--ease-out)]`；本计划再把整组过渡类删除。如果顺序反了，计划 001 会把过渡加回来。

> 若想把两个计划合成一个改动（001 转换时直接跳过这一行），也可以——那样就跳过本计划。两种做法二选一，**不要既跳过又删除**。

## Repo conventions to follow

- **⌘K 面板的开合零动效是既定的正确做法** —— 面板由使用方条件挂载（`src/App.tsx:917`、`:1079`），每次打开都是全新实例。本计划不动这个机制。
- **面板的其它交互（↑↓/回车/Esc、滚动进可视区、输入变化后选中项回首位）都不动。**
- **不要给鼠标悬停单独加过渡** —— 那会把「键盘瞬时、鼠标渐变」的不一致引入回来；键盘优先的面板里两者都瞬时才是对的。

## Steps

1. 确认计划 001 已落地：`grep -n "transition-colors" src/components/CommandPalette.tsx`。
   - 命中（说明 001 已把它换成 token 形式）→ 继续第 2 步。
   - 未命中 → 两种可能：001 未执行（**先执行 001**），或 001 已在这一步跳过（**本计划可标记为已覆盖**）。**先确认再动手。**
2. 改 `src/components/CommandPalette.tsx:187`，把类串里的 `transition-colors duration-100`（或 001 换成的 token 形式）整组删掉：

   ```tsx
                  className={`flex w-full items-center gap-2.5 rounded-[var(--r-sm)] px-2.5 py-2 text-left text-sm ${
                    i === active
                      ? "bg-[var(--hover-bg)] text-[var(--t1)]"
                      : "text-[var(--t3)]"
                  }`}
   ```

3. 跑验证（见下）。

## Boundaries

- **不要改** 面板的开合机制（条件挂载）。
- **不要改** 选中态的视觉（`bg-[var(--hover-bg)] text-[var(--t1)]`）与未选中态的 `text-[var(--t3)]`。
- **不要改** 键盘导航逻辑（`:111-126` 的 `onKeyDown`）、`active` 状态的来源（`onMouseEnter` 也会 `setActive`）。
- **不要给 `CommandPalette` 加任何新的动效**（包括退场、结果行入场、遮罩淡入）——这一条面板的判词是「永不动效」。
- **不要动** `src/components/ChatPanel/` 里的任何东西。
- 不要新增依赖。

## Verification

- **机械**：
  ```bash
  npx tsc --noEmit      # 零错
  npx oxlint src        # 零报
  npm test              # 全绿
  ```
  加一条静态锁（加进 `src/components/CommandPalette.test.tsx`）：

  ```ts
  it("命令面板结果行没有过渡（键盘高亮必须瞬时）", () => {
    const src = readFileSync("src/components/CommandPalette.tsx", "utf-8");
    expect(src).not.toContain("transition-colors");
  });
  ```

  注意这条断言**只针对 `CommandPalette.tsx`**——面板里目前只有这一处 transition（`:187`），删掉后整个文件都不该再有。

- **手感自检**：
  - 按 ⌘K 打开面板，输入几个字，**按住 ↓ 连按十次**：高亮必须**死死跟住键盘**，不能有拖尾；旧的行必须**立刻**失去底色，不能残留半秒。
  - 鼠标在结果行上划过：底色瞬时出现/消失（不渐变）。这在键盘优先的面板里是对的，不要觉得"少了点什么"而去加回过渡。
  - 用 ↑ 往上翻：同上，高亮瞬时跟随。
  - 回车选中：行为不变。
  - Esc 关闭：仍然瞬时（无退场动画）——**别把面板的开合动效顺手加回来**。
  - 慢放看（DevTools → Animations 面板，playback 10%）：面板开合与高亮变化都应当**没有任何过渡条目**。
- **完成判据**：
  - `grep -n "transition" src/components/CommandPalette.tsx` 返回空。
  - 上面那条测试全绿。

