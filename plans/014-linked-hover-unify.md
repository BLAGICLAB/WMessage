# 014 — 首页联动 hover 三个组件统一成一种响应

- **Status**: 已完成
- **Commit**: b22387c7（计划锚定）→ 落地于本次提交
- **Severity**: MEDIUM
- **Category**: §7 Cohesion & tokens / §8 Missed opportunities
- **Estimated scope**: 3 个 tsx，约 10 行改动

## Problem

首页有「联动 hover」：鼠标停在某一天/某个任务上时，**周图柱状图、日程迷你网格、图谱速览**三个组件会一起响应——高亮那一项、压暗其余。这是首页最显眼的一组交互。

但三个组件的响应方式不一样：两个在淡入淡出，第三个是硬切。

```tsx
/* src/components/HomePage/WeeklyChart.tsx:165 与 :175 —— 有过渡 */
                style={{ opacity: cellOpacity(i), transition: "opacity .15s ease" }}
```

```tsx
/* src/components/HomePage/DayGridMini.tsx:114 —— 有过渡 */
                    transition: "opacity .15s ease",
```

```tsx
/* src/components/HomePage/GraphMini.tsx:174-179 —— 无过渡（连线） */
                <line
                  key={`${e.source}-${e.target}`}
                  x1={s.x}
                  y1={s.y}
                  x2={t.x}
                  y2={t.y}
                  stroke="var(--edge-strong)"
                  strokeWidth={hot ? 0.8 : 0.5}
                  opacity={activeTaskId ? (hot ? 0.9 : 0.12) : 0.6}
                />
```

```tsx
/* src/components/HomePage/GraphMini.tsx:196-204 —— 无过渡（节点） */
                  r={hot ? r + 0.8 : r}
                  fill={lit ? "var(--accent)" : fill}
                  opacity={dim ? 0.22 : 1}
                  stroke={hot ? "var(--accent)" : "var(--surface)"}
                  strokeWidth={hot ? 0.9 : 0.4}
```

所以：指针扫过周图时，柱子是**渐隐**的；同一时刻，指针扫过的图谱节点是**啪地跳**的（半径 +0.8px、描边 +0.5px、透明度全变）。同一次手势，三块区域给出三种反馈强度——其中两种在淡变、一种在闪。

而且这三处用的曲线/时长是各自手写的（`opacity .15s ease` 写了两遍，`GraphMini` 干脆没写），没有共同的来源。

## Target

**三个组件统一到同一条过渡**：`opacity` / `r` / `stroke-width` 一起补间，150ms，走仓库的强 ease-out。

```tsx
/* 目标 —— src/components/HomePage/WeeklyChart.tsx:165 与 :175 */
                style={{
                  opacity: cellOpacity(i),
                  transition: "opacity var(--dur-card) var(--ease-out)",
                }}
```

```tsx
/* 目标 —— src/components/HomePage/DayGridMini.tsx:114 */
                    transition: "opacity var(--dur-card) var(--ease-out)",
```

```tsx
/* 目标 —— src/components/HomePage/GraphMini.tsx:174-179（连线） */
                <line
                  key={`${e.source}-${e.target}`}
                  x1={s.x}
                  y1={s.y}
                  x2={t.x}
                  y2={t.y}
                  stroke="var(--edge-strong)"
                  strokeWidth={hot ? 0.8 : 0.5}
                  opacity={activeTaskId ? (hot ? 0.9 : 0.12) : 0.6}
                  style={{
                    transition:
                      "opacity var(--dur-card) var(--ease-out), stroke-width var(--dur-card) var(--ease-out)",
                  }}
                />
```

```tsx
/* 目标 —— src/components/HomePage/GraphMini.tsx:196-205（节点） */
                  r={hot ? r + 0.8 : r}
                  fill={lit ? "var(--accent)" : fill}
                  opacity={dim ? 0.22 : 1}
                  stroke={hot ? "var(--accent)" : "var(--surface)"}
                  strokeWidth={hot ? 0.9 : 0.4}
                  style={{
                    transition:
                      "opacity var(--dur-card) var(--ease-out), r var(--dur-card) var(--ease-out), stroke-width var(--dur-card) var(--ease-out)",
                  }}
```

**关于 `r` 和 `stroke-width` 能不能过渡**：它们是 SVG 的**几何/描边属性**，在现代浏览器里是真正的 CSS 属性（SVG 几何属性规范），因此可以放进 `transition` 的属性列表。本应用的运行环境是 Tauri：Windows 走 WebView2（Chromium，SVG 几何属性自 Chrome 45 起支持）、macOS 走 WKWebView（Safari 15 起支持）。`opacity` / `stroke-width` 则是 CSS 1 时代就有的属性，处处可用。

**依赖**：`--dur-card` 与 `--ease-out` 由计划 001 建立。**若计划 001 尚未落地**，写字面量：

```tsx
transition: "opacity 0.15s cubic-bezier(0.23, 1, 0.32, 1)"
```

## Repo conventions to follow

- **联动 hover 的语义（高亮当前项、压暗其余）不动** —— 只统一「怎么变过去」的方式。
- **SVG 属性写法保持现状**（`r=` / `strokeWidth=` / `opacity=` 这些 presentation attribute 继续用属性写法），本计划只**追加** `style` 里的 `transition`，不改成 CSS 类。
- **`GraphMini.tsx:186-190` 的注释说明了图谱速览与图谱页是两套颜色体系**（"两边不共用色值不是遗漏，是两张不同的屏"）——本计划不碰配色。
- **`fill` 的变化不补间**：颜色变化不参与过渡，保持与其它两个组件一致（那两个也只过渡 opacity）。

## Steps

1. 确认 token 是否已存在：`grep -n -- "--ease-out\|--dur-card" src/ui/main.css`。
   - 存在 → 用 token 形式；不存在 → 用字面量形式并在本文件顶部记一笔。
2. 改 `src/components/HomePage/WeeklyChart.tsx:165` 与 `:175`：把 `transition: "opacity .15s ease"` 换成 token 形式（两处都要改）。
3. 改 `src/components/HomePage/DayGridMini.tsx:114`：同上（该处是 `style` 对象里的一个独立字段，注意上一行的 `opacity:` 写法）。
4. 改 `src/components/HomePage/GraphMini.tsx`：
   - `<line>` 元素加 `style={{ transition: "opacity …, stroke-width …" }}`；
   - `<circle>` 元素加 `style={{ transition: "opacity …, r …, stroke-width …" }}`。
5. 跑验证（见下）。

## Boundaries

- **不要改** 三个组件的联动逻辑（`cellOpacity` / `dimmed` / `hot` / `lit` 的判定）。
- **不要改** 任何颜色值、尺寸、布局。
- **不要给 `fill` 加过渡**。
- **不要动** `GraphPage/GraphPage.tsx` 的完整图谱页（那是另一块画布，本计划只管首页速览卡）。
- **不要动** `HomePage/WeeklyChart.tsx` 的 recharts Tooltip（那是计划 004 的地盘）。
- **不要动** `HomeHeader.tsx`（计划 003 的地盘）。
- 不要新增依赖。

## Verification

- **机械**：
  ```bash
  npx tsc --noEmit      # 零错
  npx oxlint src        # 零报
  npm test              # 全绿
  ```
  加一条静态锁（加进 `src/components/HomePage/HomePage.test.tsx` 或新建同名测试文件）：

  ```ts
  it("首页三个联动组件的过渡口径一致", () => {
    const graph = readFileSync("src/components/HomePage/GraphMini.tsx", "utf-8");
    expect(graph).toMatch(/transition:[^"']*r [^"']*stroke-width/);
    expect(graph).toMatch(/transition:[^"']*opacity/);
    // 周图与日历不应再留下手写的 .15s ease
    for (const f of [
      "src/components/HomePage/WeeklyChart.tsx",
      "src/components/HomePage/DayGridMini.tsx",
    ]) {
      expect(readFileSync(f, "utf-8")).not.toContain('opacity .15s ease');
    }
  });
  ```

- **手感自检**：
  - 首页，指针扫过周图的某一天：柱状图的 dim 与图谱速览的 dim **必须同步淡变**，不能再一个渐变一个硬切。
  - 指针停在某个任务卡上（图谱速览里高亮对应节点）：节点的半径与描边应当**平滑长大**，不能跳。
  - 指针快速来回扫：三个组件的高亮与压暗应当**成组地**一起变化，不能出现「柱子已经暗了、图谱还没亮」的错位感。
  - DevTools → Elements 选中图谱节点，Computed 样式里应能看到 `transition` 包含 `r` 与 `stroke-width`；把 playback 调到 10%，半径应当是**连续补间**的。
  - 回归检查：图谱节点的**点击拾取**（点节点跳编辑态）必须仍然正常——`r` 的补间不能影响命中区域判定。
  - 回归检查：周图 tooltip（计划 004 改过）与图谱速览互不干扰。
- **完成判据**：
  - `grep -rn "opacity .15s ease" src/components/HomePage/` 返回空。
  - `GraphMini.tsx` 的 `<line>` 与 `<circle>` 都带上了 `style.transition`。
  - 上面那条测试全绿。


## 落地记录：**SVG presentation attribute 变化不触发 CSS 过渡**

本计划 Target 节给 `GraphMini` 的 `<line>` / `<circle>` 加 `style.transition`，但
**照那样写，过渡一条都不会生效**。Chromium 实测（本地探针，逐帧采样 computed 值）：

| 属性 | 改法 | 是否补间 |
| --- | --- | --- |
| `r` | `setAttribute("r", …)` | **否**（全程终值，`getAnimations()` 为空） |
| `r` | `style.r = "24px"` | **是**（20 → 20.83 → 40px） |
| `opacity` | `setAttribute("opacity", …)` | **否**（全程终值） |
| `opacity` | `style.opacity = "0.3"` | **是**（1 → 0.967 → 0.3） |
| `stroke-width` | `setAttribute("stroke-width", …)` | **否**（1px → 6px 直跳） |

即：**只有走 CSS 属性（style）的变化才补间；presentation attribute 变化不补间**。
`CSS.supports("transition-property", "r")` 是 true（属性被识别），但这不等于
attribute 变化会触发过渡——所以「浏览器支持 SVG 几何属性」这个前提本身没错，
错在**值的来源**。

React 把 JSX 上的 `r={…}` / `opacity={…}` / `strokeWidth={…}` 渲染成**属性**，
所以这三项必须搬到 `style` 里：

```tsx
                <circle
                  ...
                  style={{
                    r: `${hot ? r + 0.8 : r}px`,
                    strokeWidth: `${hot ? 0.9 : 0.4}px`,
                    opacity: dim ? 0.22 : 1,
                    transition:
                      "opacity var(--dur-card) var(--ease-out), r var(--dur-card) var(--ease-out), stroke-width var(--dur-card) var(--ease-out)",
                  }}
```

（`fill` / `stroke` 不参与过渡，保持属性即可。）

**连带改了两条既有断言**：`HomePage.test.tsx` 与 `GraphMini.test.tsx` 里用属性
选择器 `[opacity='0.22']` / `getAttribute("opacity")` 找降透明的节点。产品行为
没变（无关节点仍然降透明到 0.22），变的只是「这个值放在哪」——断言跟着改成读
`style.opacity`。静态锁同时钉住「这三个值必须在 style 上、不得退回属性」，
退回属性等于静默失效（过渡没了，但代码看着是对的）。
