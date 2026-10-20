# 009 — 给没走 nm-btn 家族的高频控件补上按压反馈

- **Status**: TODO
- **Commit**: b22387c7
- **Severity**: MEDIUM
- **Category**: §3 Physicality & origin
- **Estimated scope**: 3 个 tsx，约 8 行改动

## Problem

仓库有一套成熟的按压反馈：`.nm-btn:active:not(:disabled) { background-color: var(--inset-bg); }`（`src/ui/main.css:244-246`）、`.nm-icon-btn:active`（`:270-272`）、`.nm-btn-brand:active`（`:1131-1133`）。**颜色那一半是有的。**

但全应用的主开关和聊天输入栏整排按钮**绕开了 nm-\* 家族**，用的是裸 Tailwind 类——它们既没有颜色按压反馈，也没有形状按压反馈：

```tsx
/* src/components/Toggle/Toggle.tsx:41-56 —— 当前 */
      className={[
        "relative inline-flex shrink-0 items-center rounded-full transition-colors duration-150",
        dims,
        checked ? "bg-[#ef4444]" : "bg-[var(--border)]",
        disabled ? "opacity-50 cursor-not-allowed" : "cursor-pointer",
      ].join(" ")}
    >
      <span
        aria-hidden
        className={[
          "inline-block transform rounded-full bg-white shadow transition-transform duration-150",
          size === "sm" ? "h-4 w-4" : "h-5 w-5",
          checked ? knobChecked : "translate-x-0.5",
        ].join(" ")}
      />
```

```tsx
/* src/components/ChatPanel/InputArea.tsx:530-536 —— 当前 */
            <button
              type="button"
              className="shrink-0 w-8 h-8 rounded-full flex items-center justify-center bg-[var(--t1)] text-[var(--bg)] hover:opacity-85 transition-opacity"
              title="发送（Enter 发送，Shift+Enter 换行）"
              aria-label="发送"
              onClick={send}
            >
```

实测：整个 `src/components/ChatPanel/`、`CommandPalette.tsx`、`FoldToggle.tsx` 里，`active:` 变体**零命中**（除 `active:cursor-grabbing`）。也就是说聊天输入栏的每一个按钮——发送、停止、附件 `+`、模型选择、思考档位——按下时**什么反馈都没有**。

发送键尤其突出：它是整个聊天界面按得最多的控件，而它唯一的反馈是 `hover:opacity-85`——一个鼠标用户**按下之前就已经触发过**的 hover 效果。按下的瞬间，按钮不给任何回应。

## Target

给这几处补上**形状按压反馈**：按下时微缩到 0.97，松手回到 1。

```tsx
/* Toggle 旋钮 —— 目标 */
          "inline-block transform rounded-full bg-white shadow transition-transform duration-150 active:scale-[0.97]",
```

```tsx
/* 发送键 —— 目标 */
              className="shrink-0 w-8 h-8 rounded-full flex items-center justify-center bg-[var(--t1)] text-[var(--bg)] hover:opacity-85 transition-[opacity,transform] duration-150 active:scale-[0.97]"
```

**为什么 `active:scale-[0.97]` 不会破坏 Toggle 旋钮的位移**：Tailwind v3 的 `scale-*` 写的是 CSS 变量（`--tw-scale-x/y`），而 `transform` 属性由 `.transform` / `.translate-x-*` 共同拼成同一个表达式：

```css
/* 编译产物（已实测） */
.active\:scale-\[0\.97\]:active {
  --tw-scale-x: 0.97;
  --tw-scale-y: 0.97;
  transform: translate(var(--tw-translate-x), var(--tw-translate-y)) rotate(...) skewX(...) skewY(...) scaleX(var(--tw-scale-x)) scaleY(var(--tw-scale-y))
}
```

所以旋钮的 `translate-x-[22px]`（`--tw-translate-x`）与按压时的 `scale(0.97)`（`--tw-scale-x`）**共存于同一个 transform 串**——旋钮不会跳回原位。

**发送键为什么要把 `transition-opacity` 换成 `transition-[opacity,transform]`**：`transition-opacity` 的 `transition-property` 只有 `opacity`，按压时的 transform 变化不会被过渡（会瞬切）。`transition-[opacity,transform]` 把两个属性都纳进去。

**应用范围**（本次只改这两处最要紧的；其余同族按钮见 Boundaries）：

1. `src/components/Toggle/Toggle.tsx:52` —— 旋钮加 `active:scale-[0.97]`（`transition-transform duration-150` 保持不变，它已经过渡 transform）。
2. `src/components/ChatPanel/InputArea.tsx:530-536` —— 发送键加 `active:scale-[0.97]`，并把 `transition-opacity` 换成 `transition-[opacity,transform]`。

**顺带**：`src/components/ChatPanel/InputArea.tsx` 里另外 4 个同族按钮（`:320` 附件、`:338` 权限 chip、`:405` 模型、`:470` 思考档位、`:523` 停止键）目前也都没有按压反馈。它们是**同一条工具栏**，一起改才不割裂——**本计划一并处理**（这 5 处的改法完全相同，逐处替换类串即可）。

## Repo conventions to follow

- **仓库的按压反馈是「颜色 + 形状」两半**：颜色半边已有（`.nm-btn:active` 的 inset 底），本计划补的是形状半边。**不要删掉或替换已有的 `:active` 颜色规则。**
- **`.nm-btn` / `.nm-icon-btn` / `.nm-btn-brand` 不需要改** —— 它们已经有颜色按压反馈，符合仓库既有约定。本计划只管**没走 nm-\* 家族**的控件。
- **按压缩放量的取值来自动效规则**：0.95–0.98 之间取 0.97，不要更小。
- **不要用裸 CSS `transform: scale(0.97)` 写进类里** —— 那会覆盖元素自己的 transform（旋钮的 translateX 会被抹掉，旋钮跳回原位）。必须用 Tailwind 的 `scale-*`（走 CSS 变量合成）。

## Steps

0. **确认计划 001 已落地**（它会先把这几行的 `transition-*` 工具类换成 token 形式）：
   `grep -n -- "--dur-chip\|--ease-out" src/ui/main.css` 应当有命中。
   - 未落地 → **先执行计划 001**，否则本计划第 4 步会把 001 的 token 形式覆盖回字面量。
   - 已落地 → 本计划第 4 步里凡是「原本是 `transition-colors` / `transition-opacity`」的地方，**保留 001 换好的 token 形式**（`duration-[var(--dur-chip)] ease-[var(--ease-out)]`），只把属性列表从 `transition-colors` / `transition-opacity` 换成能覆盖 transform 的版本，时长与曲线沿用 001 的 token。
1. 改 `src/components/Toggle/Toggle.tsx:52`：在类串里加 `active:scale-[0.97]`。

   ```tsx
          "inline-block transform rounded-full bg-white shadow transition-transform duration-150 active:scale-[0.97]",
   ```

2. 改 `src/components/ChatPanel/InputArea.tsx:530-536`（发送键）：类串改为

   ```tsx
   className="shrink-0 w-8 h-8 rounded-full flex items-center justify-center bg-[var(--t1)] text-[var(--bg)] hover:opacity-85 transition-[opacity,transform] duration-150 active:scale-[0.97]"
   ```

3. 改 `src/components/ChatPanel/InputArea.tsx` 的其余 4 个同族按钮（`:320`、`:338`、`:405`、`:470`、`:523`）：同样加 `active:scale-[0.97]`，并把它们的 `transition-colors` / `transition-opacity` 换成能覆盖 transform 的属性列表：
   - 原本是 `transition-colors` 的 → `transition-[color,background-color,border-color,transform]`（或 `transition-colors` + `transition-transform` 两个类并用，前者管颜色、后者管 transform；**二选一，别两个都用 `transition-[...]` 又加 `transition-transform`**）
   - 原本是 `transition-opacity` 的 → `transition-[opacity,transform]`
   - 原本没有任何 transition 的 → 加 `transition-transform duration-150`
4. 跑验证（见下）。

## Boundaries

- **不要改** `.nm-btn` / `.nm-icon-btn` / `.nm-btn-brand` 的 `:active` 规则。
- **不要改** `Toggle` 的配色（`bg-[#ef4444]` / `bg-[var(--border)]`）、尺寸（`dims`）、旋钮的位移值（`knobChecked`）。
- **不要改** `InputArea` 里各按钮的 `hover:` 效果、图标、尺寸。
- **不要给 `CommandPalette` / `FoldToggle` / `SessionRail` / `ContextRail` 加按压反馈** —— 那些是键盘驱动或低频控件，本计划不碰。
- **不要用 `transition-all`**。
- **不要写裸 CSS `transform: scale(...)` 覆盖元素自身的 transform**。
- 不要新增依赖。

## Verification

- **机械**：
  ```bash
  npx tsc --noEmit      # 零错
  npx oxlint src        # 零报
  npm test              # 全绿
  ```
  加一条静态锁（照仓库现有测试风格）：

  ```ts
  it("Toggle 与聊天输入栏按钮有按压反馈", () => {
    const toggle = readFileSync("src/components/Toggle/Toggle.tsx", "utf-8");
    expect(toggle).toContain("active:scale-[0.97]");
    const input = readFileSync("src/components/ChatPanel/InputArea.tsx", "utf-8");
    expect(input).toContain("active:scale-[0.97]");
    // 发送键的过渡必须覆盖 transform
    expect(input).toMatch(/transition-\[opacity,transform\]/);
  });
  ```

- **手感自检**：
  - 设置页里任意一个 Toggle：按住不放，旋钮应当**微微缩一下**（约 3%），松手弹回。旋钮的位置（开/关）**不能**因为按压而跳变。
  - 聊天界面按住发送键：按钮应当有轻微的按下去的感觉；松手回弹。
  - 输入栏其它 4 个按钮同样按一遍，确认每个都有反馈且不打架。
  - 快速连点发送键：每次按压都要有独立反馈，不能只响应第一次。
  - 键盘操作（Tab 聚焦 + Enter）不应触发按压缩放（`:active` 只在指针按下时成立，这是 CSS 原生行为，确认一下即可）。
  - DevTools → Elements 选中旋钮，Computed 的 `transform` 在按压时应为 `matrix(0.97, 0, 0, 0.97, 22, 0)` 一类（缩放 + 原有位移同时存在）。
- **完成判据**：
  - `grep -rn "active:" src/components/ChatPanel/InputArea.tsx src/components/Toggle/Toggle.tsx` 每处都含 `scale-[0.97]`。
  - 上面那条测试全绿。

