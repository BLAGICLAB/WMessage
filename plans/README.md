# 动效改进计划（improve-animations 审计产出）

全仓动效审计（`src/`，147 个 tsx + `src/ui/main.css`）的落地计划。**19 条高置信发现，其中 17 条落成计划**（2 条 LOW 未落计划：ContextRail 的 `nm-inset` 过渡随状态增删、LiveIndicator 内联 `animation` 压过定向 reduced-motion 覆盖）。

- **审计基线**：`5f8d82e7`（起审时）→ 复核于 `b22387c7`（审计期间仓库被并发提交，EvolutionPanel 已删）
- **各计划锚定的 commit**：`b22387c7`
- **行号已漂移，按内容定位**：001 加了 12 行 token 块、018 加了 11 行 `.app-scroll`，加上审计期间的并发提交，本清单里的 `file:line` 一律**不再可靠**（实测 `.plan-blk` 已下移 9 行、`.plan-fresh` 22 行、`.nm-popover` 25 行）。执行任何一条计划时按**选择器 / 类名 / 文案**找位置，别照着行号改。
- **来源技能**：`improve-animations`（判据来自 Emil Kowalski 的动效哲学）

## 计划清单

| # | 标题 | 严重度 | 类别 | 依赖 | 状态 |
| --- | --- | --- | --- | --- | --- |
| [001](001-motion-token-layer.md) | 建立动效 token 层，把曲线与时长收归一处 | HIGH | §7 / §2 | — | 已完成 `0af87dd4` |
| [002](002-taskpool-exit-easing.md) | 任务池退场曲线改掉 ease-in 慢起 | HIGH | §2 | 001（token） | TODO |
| [003](003-home-header-transform-scroll.md) | 首页吸顶头部：滚动驱动的布局属性动画换 transform | HIGH | §5 | 001（token，B5 步） | TODO |
| [004](004-weeklychart-tooltip-duration.md) | 周图 Tooltip 的 400ms 过渡收到 150ms | HIGH | §2 | 001（token） | TODO |
| [005](005-plan-fresh-no-bounce.md) | 计划块落位弹入：去掉过冲曲线 | MEDIUM | §7 / §3 | 001（token） | TODO |
| [006](006-drag-ghost-transform.md) | 拖拽幽灵改用 transform 定位 | MEDIUM | §5 | — | TODO |
| [007](007-pool-lift-transition.md) | 池内拖起补上过渡 | MEDIUM | §3 / §7 | 001（token）+ 001 先于本条改 `.plan-blk` 过渡 | TODO |
| [008](008-plan-block-hover-gate.md) | 计划块 hover 加触屏门控，时长收 100ms | MEDIUM | §6 / §1 | 001（token）+ 001 先于本条改 `.plan-blk` 过渡 | TODO |
| [009](009-press-feedback.md) | 给高频控件补上按压反馈 | MEDIUM | §3 | **必须晚于 001** | TODO |
| [010](010-popover-entrance.md) | 设置页下拉浮层加入场动效 | MEDIUM | §3 / §2 | 001（token） | TODO |
| [011](011-modal-entrance.md) | 共享 Modal 壳加入场动效 | MEDIUM | §8 / §2 | 001（token） | TODO |
| [012](012-card-opacity-transition.md) | 整卡透明度切换补上 opacity 过渡腿 | MEDIUM | §3 / §8 | 001（token）+ 001 先于本条改 `.nm-card` 过渡 | TODO |
| [013](013-framer-motion-transform-string.md) | framer-motion 的 y 简写改成完整 transform 串 | MEDIUM | §5 | — | TODO |
| [014](014-linked-hover-unify.md) | 首页联动 hover 三个组件统一成一种响应 | MEDIUM | §7 / §8 | 001（token） | TODO |
| [015](015-reduced-motion-activity-signal.md) | 减少动效下保住「还在跑」的活动信号 | MEDIUM | §6 | — | TODO |
| [016](016-archive-locate-reduced-motion.md) | 归档定位的平滑滚动要听减少动效的话 | MEDIUM | §6 | — | TODO |
| [017](017-command-palette-instant-highlight.md) | ⌘K 命令面板的键盘高亮改为瞬时 | MEDIUM | §1 / §7 | **必须晚于 001** | TODO |
| [018](018-home-header-scroll-jitter.md) | 首页吸顶头部滚动收缩时抖动（Windows 尤其明显） | HIGH | §4 / §5 | **必须先于 003**（同改头部内部） | 已完成 `dcc330f2` |

## 推荐执行顺序

### 第 0 批 — 地基（先做，其它计划引用它的 token）

**001** 一次改动覆盖 22 个文件，但全是机械替换：给 `src/ui/main.css` 的 `:root` 加 `--ease-*` / `--duration-*` token，把 10 条手写 `transition:` 和 3 处 `animation:` 换成 token，把 38 处 Tailwind `transition-*` 工具类统一成 token 类串，并给 `src/ui/main.css.test.ts` 加静态锁。

> 这一批是**唯一会大面积触碰源码的改动**。建议单独一个 commit，落地后其余计划都只改局部。

### 第 1 批 — 手感崩点（001 之后立刻做）

| 顺序 | 计划 | 为什么这个顺序 |
| --- | --- | --- |
| 1 | **002** | 最高杠杆的单点修复：任务池退场从「前 40% 时间几乎不动」改成 ease-out，改一条曲线即可。 |
| 2 | **004** | 一行改动，周图 tooltip 从 400ms 收到 150ms。 |
| 3 | **017** | **必须晚于 001**（001 会把这一行改成 token 形式，017 再整组删掉）。 |
| 4 | **018** | **用户报告的真 bug**（滚动收缩时抖动，Windows 尤其明显）。必须先于 003——两者改同一段头部代码，003 会重写内部结构。诊断优先：先在 Windows 上用 DevTools 加 `overflow-anchor: none` 确认是不是滚动锚定的反馈环。 |
| 5 | **003** | 风险最高的一条，**先量后改**——它自带「量完发现没丢帧就不改代码」的合法出口。建议在别的计划都落地、界面稳定之后再动。 |

### 第 2 批 — 一致性收口

**005 → 007 → 008 → 012 → 014 → 009**

- 005/007/008/012 都要改 `main.css` 的过渡声明，其中 007 与 008 碰 `.plan-blk` 的过渡列表、012 碰 `.nm-card` 的过渡列表——**001 必须先于它们**。
- 009（按压反馈）**必须晚于 001**：它会覆盖 001 在 `InputArea.tsx:530-536` 发送键上的产出。

### 第 3 批 — 补上缺失的动效（增量的：有 vs 没有）

**010**（浮层入场）→ **011**（弹窗入场）

这两条是纯增量：现在的下拉和弹窗是「啪」地出现，改完有入场。011 落在共享壳 `src/ui/Modal.tsx` 上，一次改动九个弹窗受益。

### 第 4 批 — 性能与无障碍

**006**（拖拽幽灵 transform）→ **013**（framer-motion transform 串）→ **015**（减少动效活动信号）→ **016**（归档定位滚动守卫）

这四条彼此独立，可并行。它们的风险最低、边界最清楚。

## 依赖关系图

```
001 (token 层)
 ├─ 002  003(B5步)  004  005  007  008  010  011  012  014
 ├─ 009  ← 必须晚于 001（会覆盖 001 在 InputArea:530-536 的产出）
 └─ 017  ← 必须晚于 001（会删除 001 在 CommandPalette:187 的产出）

018 (吸顶头部抖动)  ← 必须先于 003（同改 HomeHeader 内部）
独立（不依赖 001）：006  013  015  016
001 必须先于：007  008  012（都改 main.css 的过渡列表）
```

## 每条计划都包含

- **Problem** —— 现状代码逐字引用 + `file:line` + 为什么不对
- **Target** —— 精确到 cubic-bezier 数值、时长毫秒、类名字符串；取值全部来自 `improve-animations` 的 `AUDIT.md`，不凭记忆近似
- **Repo conventions to follow** —— 本仓库的既有约定（token 放 `main.css` 的 `:root`、纪律注释在 `main.css:157-158`、静态锁测试的写法……）
- **Steps** —— 一步一改，含文件、位置、替换前后的代码
- **Boundaries** —— 明确不许碰的东西，以及与其它计划的边界划分
- **Verification** —— 机械检查（`tsc` / `oxlint` / `vitest` / 新增的静态锁断言）+ **手感自检**（逐条可观察的现象）+ 完成判据

## 执行时注意

1. **001 先行**。其余计划大量引用 `var(--ease-out)` / `var(--dur-chip)` 等 token；单独执行某条计划时，它自己写了「若 001 未落地则用字面量」的备选写法——两套都落地时**以 token 形式为准**。
2. **静态锁测试是硬门禁**。`src/ui/main.css.test.ts` 里已有若干结构锁（例如 `nm-card-hover` 的过渡对称性正则），001 与 012 都会改到那张表——**改了过渡声明就要同步改断言**，否则 `npm test` 会红。
3. **手感自检不能跳**。动效的机械正确不等于看着对。每条计划的 Verification 里都写了可观察的现象（弹层从哪个角长出来、池子是不是立刻开始动、reduced-motion 下转子还在不在动），逐条对着看。
4. **003 允许不改代码**。它先量掉帧率，量下来没丢帧就到此为止并把数字写回计划文件——这是合法结果，不要硬改一个视觉风险高的吸顶头部。
5. **提交粒度**：一条计划一个 commit（`AGENTS.md`：「一个逻辑变更一个 commit」）。001 虽然动 22 个文件，但它是一个逻辑变更。
