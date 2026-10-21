# 019 — 任务编辑浮窗的内联手写过渡收归 token

- **Status**: 已完成
- **Commit**: 落地于本次提交
- **Severity**: MEDIUM
- **Category**: §2 Easing & duration / §7 Cohesion & tokens
- **Estimated scope**: 3 个文件（`TaskEditorOverlay.tsx` + `main.css` + 其测试），约 12 行改动

> 这条**不在原 18 条计划里**，是审计收尾时发现的缺口：`TaskEditorOverlay.tsx` 的入场过渡是
> 全仓最后一处没进 token 层的动效，原计划清单没有覆盖它。

## Problem

全局任务编辑浮窗（毛玻璃侧浮窗）用**内联样式**写着一条过渡，两个属性两条腿各写各的：

```tsx
/* src/components/TaskTimeline/TaskEditorOverlay.tsx:71 —— 改前 */
transition: `transform ${SLIDE_MS}ms cubic-bezier(0.05, 0.7, 0.1, 1), opacity 180ms ease`,
```

三处问题：

1. **`opacity` 用了 CSS 关键字 `ease`** —— 仓库自己的动效 token 注释点名禁用它：
   `main.css` 的 token 块写着「曲线用强 ease-out，**不用 CSS 关键字 ease**
   (cubic-bezier(0.25,0.1,0.25,1))」。
2. **两条腿不同步** —— 位移 240ms、透明度 180ms，曲线也不同。实测两腿的进度差得很远：

   | 绝对时刻 | 旧位移 | 旧透明度 | 差距 |
   |---|---|---|---|
   | 30ms | 67.6% | 22.1% | **45.5 个百分点** |
   | 60ms | 83.2% | 57.6% | 25.6 个百分点 |
   | 90ms | 90.7% | 80.2% | 10.5 个百分点 |

   也就是说浮窗**已经滑到位了、还剩四成透明**——面板在「半透明地飘进来」。
3. **手写曲线与裸时长不在词汇表里** —— `cubic-bezier(0.05, 0.7, 0.1, 1)` 与 `180ms`
   都是一次性写死的值，库里再出现一个抽屉就会再来一条分叉。

## Target

两条腿共用同一个时长 token 与同一条曲线，且那条曲线取自仓库既有的 `--ease-out`。

**一、CSS 侧**（`src/ui/main.css` 的 `:root` 动效 token 块）：

```css
  --dur-drawer: 240ms; /* 抽屉 / 侧浮窗滑入（任务编辑浮窗） */
```

**二、TSX 侧**（`src/components/TaskTimeline/TaskEditorOverlay.tsx`）：

```tsx
        transition:
          "transform var(--dur-drawer) var(--ease-out), opacity var(--dur-drawer) var(--ease-out)",
```

**三、TS 侧的 `SLIDE_MS` 保留**，但只用来算退场兜底：

```tsx
/** 滑入时长（ms）：与 main.css 的 --dur-drawer 同值。过渡本身走 CSS token，
 *  这个常量只用来算退场兜底；两处同值由静态锁钉住。 */
const SLIDE_MS = 240;
/** 退场卸载兜底：略大于滑入时长（transitionend 事件在 jsdom/降级路径不触发） */
const EXIT_MS = SLIDE_MS + 40;
```

`EXIT_MS` 从魔法数字 280 改成 `SLIDE_MS + 40`，那条「略大于滑入时长」的注释从此由代码
保证，而不是靠人记。

## 取值依据

- **`--ease-out`（`cubic-bezier(0.23, 1, 0.32, 1)`）而不是新建 `--ease-drawer`**：
  动效规则里「进场/退场 → ease-out」是硬规则，而仓库的 token 层只有这一条曲线在用
  （79 处消费）。审计规则目录里的 `--ease-drawer: cubic-bezier(0.32, 0.72, 0, 1)`
  是「iOS-like drawer curve」，但仓库 001 已经因为「零消费」把它删掉了；为了一处
  消费方再造一条曲线，违反 `AGENTS.md` 的「第二个调用方出现前不抽象」。

  三条候选曲线实测只差几个百分点，视觉上分不出：

  | 曲线 | 25% 时间 | 50% | 75% |
  |---|---|---|---|
  | 现状 `cubic-bezier(0.05, 0.7, 0.1, 1)` | 83.2% | 95.0% | 99.1% |
  | iOS drawer `cubic-bezier(0.32, 0.72, 0, 1)` | 77.9% | 95.5% | 99.2% |
  | **`--ease-out` `cubic-bezier(0.23, 1, 0.32, 1)`** | 77.5% | 96.6% | 99.7% |

  `--ease-out` 的尾巴反而最紧（75% 时间已走完 99.7%），240ms 的滑入不会被拖出黏滞感。
- **`--dur-drawer: 240ms`** —— 抽屉属于动效规则的「Modals, drawers 200–500ms」档，
  保持现状的 240ms 不动（240 是实测过的手感值，没理由改）。

## Repo conventions to follow

- **token 只定义有消费方的** —— `--dur-drawer` 的唯一消费方就是这条过渡，同步落地。
- **曲线与时长的唯一来源是 `main.css` 的 `:root` 块** —— TSX 里不再出现裸的
  `240ms` / `cubic-bezier(...)`（`SLIDE_MS` 只用于算退场兜底，且由静态锁与 token 同值）。
- **只动 transform 与 opacity** —— 动效规则的硬要求（其它属性会触发布局）。
- **`AGENTS.md` 的「复写一次就是一次静默错位」** —— `SLIDE_MS` 与 `--dur-drawer` 是
  两处同值，必须有锁。

## Boundaries

- **不要改** 浮窗的尺寸、位置（`LEFT`）、面板内容。
- **不要改** 退场握手的结构（`closing` → 定时器 → `onClosed`）。
- **不要改** 入场的两帧机制（挂载帧在屏外，下一帧滑入）。
- **不要改** `TaskEditorOverlay` 的 props 接口。
- 不要新增依赖。

## Verification

- **机械**：
  ```bash
  npx tsc --noEmit      # 零错
  npx oxlint src        # 零报
  npm test              # 全绿
  ```
- **静态锁三条**：
  - `src/ui/main.css.test.ts`：token 块里有 `--dur-drawer: 240ms`（并入已有的
    「定义了动效曲线与时长 token」那条）。
  - `src/components/TaskTimeline/TaskEditorOverlay.test.tsx`：过渡字符串逐字等于
    `transform var(--dur-drawer) var(--ease-out), opacity var(--dur-drawer) var(--ease-out)`，
    且不含 `cubic-bezier` 与裸 `NNms`。
  - 同文件：`--dur-drawer` 的 CSS 值与 TS 的 `SLIDE_MS` 同值（跨文件常量守卫）。
- **手感自检**：
  - 任务页双击一张任务卡 → 编辑浮窗从左侧滑入：面板应当**边滑边变清晰**，到位时已经
    完全实心，不能出现「已经滑到位但还半透明」。
  - 反复开关 5 次，每次一致。
  - 关闭时滑出方向与原来一致（同一套过渡，不分进出方向——那是本次范围外）。
  - DevTools → Rendering → `Emulate prefers-reduced-motion: reduce`：浮窗应当瞬时出现
    （全局兜底块会把过渡压成 0.01ms）。
- **完成判据**：
  - `grep -n "cubic-bezier" src/components/TaskTimeline/TaskEditorOverlay.tsx` 返回空。
  - 上面三条锁全绿，且都做过反向验证。

## 落地记录

落地与本计划一致。三条锁都做过反向验证：撤掉 token → token 锁与同值锁转红；退回手写
过渡串 → 过渡锁转红。

**新旧两条腿的进度对照**（绝对时刻，两条腿各自的完成度）：

| 时刻 | 旧位移 | 旧透明度 | 新两条腿（统一 240ms ease-out） |
|---|---|---|---|
| 30ms | 67.6% | 22.1% | 48.2% |
| 60ms | 83.2% | 57.6% | 77.5% |
| 90ms | 90.7% | 80.2% | 91.0% |
| 120ms | 95.0% | 92.5% | 96.6% |
| 240ms | 100% | 100% | 100% |

旧的两条腿在头 100ms 里差了最多 45 个百分点（面板半透明地飘进来），新的两条腿全程
同进度——到位即实心。
