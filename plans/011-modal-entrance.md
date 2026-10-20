# 011 — 共享 Modal 壳加入场动效

- **Status**: TODO
- **Commit**: b22387c7
- **Severity**: MEDIUM
- **Category**: §8 Missed opportunities / §2 Easing & duration
- **Estimated scope**: 2 个文件（`Modal.tsx` + `main.css`），约 20 行改动

## Problem

全应用的弹窗都走同一个共享壳 `src/ui/Modal.tsx`，而这个壳**没有任何动效**——遮罩和面板都是瞬间出现。

```tsx
/* src/ui/Modal.tsx:40-61 —— 当前 */
  const panel = isValidElement<React.HTMLAttributes<HTMLDivElement>>(children)
    ? cloneElement(children, {
        onPointerDown: (e: React.PointerEvent<HTMLDivElement>) => {
          children.props.onPointerDown?.(e);
          e.stopPropagation();
        },
      })
    : children;

  const node = (
    <div
      {...rest}
      className={className}
      onPointerDown={closeOnOutside && onClose ? onClose : undefined}
    >
      {panel}
    </div>
  );
  // className 由调用方给全：z 档/padding 各异，且 Tailwind JIT 只扫描源码
  // 字面量，prop 变量拼出的类不会被生成
  return portal ? createPortal(node, document.body) : node;
```

九个渲染点全部继承了这个「瞬时出现」：

| 调用点 | 用途 |
| --- | --- |
| `src/ui/ErrorDialogHost.tsx:51` | 全局错误弹窗 |
| `src/components/ConfirmMap/ConfirmMap.tsx:104` | 批量确认地图 |
| `src/components/TodoCard/TodoCard.tsx:744` | 任务卡操作确认 |
| `src/components/TagConsolidateModal.tsx:109` | 标签合并（破坏性操作） |
| `src/components/SettingsPage/McpPanel.tsx:157` | MCP 配置 |
| `src/components/AppsPage/AppsPage.tsx:688` | 程序上架确认 |
| `src/components/TracePanel/TracePanel.tsx:245` | 执行轨迹 |
| `src/components/MigrationPanel.tsx:459` | 数据迁移 |
| （`DeleteConfirmDialog` 随自进化模块退役已删，不计入） | — |

弹窗在动效规则里属于「偶发」档，**标准做法就是给一段入场动画**。现在这些弹窗——包括「标签合并」这种高风险的破坏性确认——都是「啪」地一下糊在屏幕上。

## Target

遮罩淡入 + 面板从 0.96 缩放淡入。**在共享壳里做一次，九个调用点全部受益。**

**一、TSX 侧**：给遮罩和面板各挂一个类。

```tsx
/* 目标 —— src/ui/Modal.tsx:40-61 */
  const panel = isValidElement<React.HTMLAttributes<HTMLDivElement>>(children)
    ? cloneElement(children, {
        onPointerDown: (e: React.PointerEvent<HTMLDivElement>) => {
          children.props.onPointerDown?.(e);
          e.stopPropagation();
        },
        // 面板入场动效：与调用方自带的 className 合并，不覆盖
        className: children.props.className
          ? `${children.props.className} modal-panel`
          : "modal-panel",
      })
    : children;

  const node = (
    <div
      {...rest}
      className={className ? `${className} modal-backdrop` : "modal-backdrop"}
      onPointerDown={closeOnOutside && onClose ? onClose : undefined}
    >
      {panel}
    </div>
  );
```

**注意**：`cloneElement` 传 `className` 会**覆盖**子元素原有的 className，所以必须拼接，不能直接给值。`children.props.className` 在 `React.HTMLAttributes` 上是 `string | undefined`。

**二、CSS 侧**（加进 `src/ui/main.css` 的 `@layer components` 块）：

```css
  /* === 共享弹层（src/ui/Modal.tsx）：遮罩淡入 + 面板从 0.96 缩放淡入 ===
     弹窗是偶发档，给一段标准入场；transform-origin 用 center
     ——弹窗是居中出现的，从中心长是对的（浮层才需要从触发点长）。 */
  .modal-backdrop {
    animation: modal-fade var(--dur-pop) var(--ease-out);
  }
  .modal-panel {
    transform-origin: center;
    animation: modal-pop var(--dur-pop) var(--ease-out);
  }
  @keyframes modal-fade {
    from {
      opacity: 0;
    }
    to {
      opacity: 1;
    }
  }
  @keyframes modal-pop {
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

取值依据：

- `scale(0.96)` —— 落在 0.9–0.97 区间；弹窗用 0.96 比下拉稍大一点，因为它离触发点更远、不需要「贴着长出来」的感觉。
- `transform-origin: center` —— 弹窗例外，从中心出现是正确的（动效规则明确豁免弹窗的原点规则）。
- `var(--dur-pop)`（200ms）—— 动效规则给弹窗 200–500ms，取下限，因为这是工作型应用不是营销页。
- `var(--ease-out)` —— 进场曲线。

**三、依赖**：两个 token 由计划 001 建立。**若计划 001 尚未落地**，写字面量：

```css
    animation: modal-fade 0.2s cubic-bezier(0.23, 1, 0.32, 1);
    animation: modal-pop 0.2s cubic-bezier(0.23, 1, 0.32, 1);
```

## Repo conventions to follow

- **共享壳的「只做一次、九处受益」是本仓库的既定做法** —— `Modal.tsx` 的注释写明了它的存在意义：「收口各处 modal 的 backdrop + stopPropagation 样板」。本计划必须落在共享壳里，**不要在九个调用点各写各的**。
- **`className` 由调用方给全**（注释：z 档/padding 各异，且 Tailwind JIT 只扫描源码字面量）——所以遮罩的 `className` 不能替换，只能追加。追加的 `modal-backdrop` 是**纯 CSS 类**，不涉及 Tailwind JIT 的字面量扫描问题。
- **`cloneElement` 只被允许改 `onPointerDown` 与 `className` 两个 prop**，不要往里塞别的。
- **`portal` 的语义不要改**：注释说明「卡片容器有 transform 包含块（fixed 被裁缩到卡片边界）时必须开」。`TodoCard.tsx:739` 正是这个场景。

## Steps

1. 计划 001 已落地：`--ease-out` 存在，`--dur-pop` **还没有**（001 只定义有消费方的 token）。
   先把 `--dur-pop: 200ms; /* 一次性出现（下拉 / 浮层 / 落位弹入） */` 加进 `src/ui/main.css` 的
   `:root` 动效 token 块，再用 token 形式。**不要**回退到字面量形式。
2. 改 `src/ui/Modal.tsx:40-61`：按 Target 节的代码，给 `panel` 拼 `modal-panel`、给遮罩拼 `modal-backdrop`。
3. 在 `src/ui/main.css` 的 `@layer components` 块内加 `.modal-backdrop` / `.modal-panel` 两条规则和两个 `@keyframes`。
4. 跑验证（见下）。

## Boundaries

- **不要改** `Modal.tsx` 的 props 接口、`onPointerDown` 停止传播逻辑、`portal` 语义、`closeOnEsc` / `closeOnOutside` 行为。
- **不要改** 九个调用点里任何一个的 className、尺寸、z 档、文案。
- **不要给弹窗加退场动画** —— 退场要「播完再卸载」的握手（`onClosed` 回调 + 定时器），会改 `Modal` 的接口并波及九个调用点。超出本计划范围。
- **不要给遮罩加 transform**（遮罩只淡入，缩放的是面板）。
- **不要动** `DeleteConfirmDialog`（已随自进化模块退役删除）。
- 不要新增依赖。

## Verification

- **机械**：
  ```bash
  npx tsc --noEmit      # 零错
  npx oxlint src        # 零错
  npm test              # 全绿
  ```
  加两条静态锁：

  ```ts
  // src/ui/Modal.test.tsx 里加（或新建）
  it("共享弹层给遮罩和面板都挂了动效类", () => {
    const src = readFileSync("src/ui/Modal.tsx", "utf-8");
    expect(src).toContain("modal-backdrop");
    expect(src).toContain("modal-panel");
    // className 必须是拼接而不是覆盖
    expect(src).toMatch(/children\.props\.className\s*\?/);
  });
  ```

  ```ts
  // src/ui/main.css.test.ts 里加
  it("弹层有遮罩淡入与面板缩放入场", () => {
    expect(css).toMatch(/\.modal-backdrop\s*\{[^}]*animation:/);
    expect(css).toMatch(/\.modal-panel\s*\{[^}]*animation:/);
    expect(css).toMatch(/\.modal-panel\s*\{[^}]*transform-origin:\s*center/);
  });
  ```

- **手感自检**：
  - 设置页 → 触发任意一个弹窗（例如标签合并）：遮罩淡入、面板从 0.96 缩放到 1，应当**干脆利落**，不能拖泥带水。
  - 任务卡 → 打开确认弹窗：同样的一段入场。**九处弹窗的表现必须一致**（这是共享壳的意义）。
  - 弹窗打开后，面板内容（文字、按钮、滚动区）必须**清晰无模糊**——缩放动画结束后应停在 `scale(1)`，不留残余的 transform。
  - 反复开关同一个弹窗 5 次：每次入场一致，不能第二次就没动画了。
  - DevTools → Elements 选中面板，Computed 的 `transform` 在动画结束后应为 `matrix(1,0,0,1,0,0)` 或 `none`（不能停在 0.97）。
  - **回归检查（关键）**：任务卡里的弹窗（`TodoCard.tsx:744`）走 `portal` 路径，确认它**没有**被任务卡的 transform 裁剪，且入场动画正常。任务卡本身 hover 时有 `translateY(-3px) scale(1.01)`（见 `TodoCard.tsx:739` 的注释）。
  - DevTools → Rendering → `Emulate prefers-reduced-motion: reduce`：弹窗应瞬时出现，无过渡。
- **完成判据**：
  - `grep -n "modal-backdrop\|modal-panel" src/ui/Modal.tsx` 命中。
  - `grep -n "modal-fade\|modal-pop" src/ui/main.css` 命中 4 处（两条规则 + 两个 keyframes）。
  - 上面两条测试全绿。

