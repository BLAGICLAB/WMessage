# 任务图谱多角度审计报告（2026-10-19）

范围：`src/components/GraphPage/` 全套图谱前端 + 它依赖的 `src/lib/graphPrefs.ts`、
`src/components/HomePage/GraphMini.tsx`、`src/lib/homeKpi.ts`。审计对象是工作区现状
（含当轮未提交的改动），不是某个 commit 的快照。

**结论**：四个角度共 34 条发现。高危 4 条已全部修复并有单测或变异测试钉住；另有
4 条在审计过程中顺手修掉。**中危 11 条、低危 17 条未动**，清单在下面，每条都带
文件行号与证据。

---

## 怎么审的

四个 `verifier` 子代理并行，各查一个互不重叠的角度，全程只读、临时脚本只写 `/tmp`：

| 角度 | 范围 | 条数 |
|---|---|---|
| A 力场数学 | `graph-force3d.ts` / `graph-scene.ts` / 调用点 | 5（高 1 / 中 2 / 低 2） |
| B 渲染生命周期 | `GraphCanvas.tsx` 渲染循环、GPU 资源、拾取 | 11（高 2 / 中 2 / 低 7） |
| C 状态流与数据契约 | GraphPage ↔ GraphCanvas 的状态与 props | 8（阻断 1 / 高 1 / 中 4 / 低 2） |
| D 测试与文档一致性 | 测试覆盖、SPEC/DEVLOG 与代码是否对得上 | 10（中 4 / 低 6） |

高危条目由主会话逐条复核过代码，没有只转述子代理结论。

**四条高危的共同形状**：声明的意图与实际接线脱节——注释、函数名或架构约定说着一套，
运行时做的是另一套。这类缺陷单测抓不到，因为测试压根没覆盖那条路径。

---

## 一、已完成

| # | 问题 | 严重度 | 状态 | 提交 |
|---|---|---|---|---|
| C#1 | 速览卡连线一条都画不出来 | 阻断 | 已修 | `bead226` |
| C#3 | 松散度搬迁写失败仍删旧键（丢用户唯一存档） | 高 | 已修 | `3b20ed4` |
| B#1 | 收敛后切着色维度，星体不变色 | 高 | 已修 | `a459999` |
| B#2 | 铭牌 declutter 每帧 O(M²) 且无预算 | 高 | 已修 | `a459999` |
| C#2 | 改一条依赖整场重建，相机被弹回定稿机位 | 高 | 已修 | `a459999` |
| A#1 | 拖拽松手后力场永久停机 | 高 | 已修 | `a459999` |
| D#1 | 星轨宽度整档错位（实际只有标称值一半，全落亚像素） | 中 | 已修 | `a459999` |
| — | 星轨测试盖不住着色器里的换算系数（DEVLOG 声称过强） | 中 | 已修 | `a459999` |
| — | `graph-declutter.ts` 零测试文件 | 中 | 已修 | `a459999` |
| — | `GraphSettingsPanel.test.tsx` 对新文案零断言 | 中 | 部分 | `3b20ed4` |
| — | 死代码：两个没人调用的 `setPixelRatio` | 低 | 已修 | `6f53cfd` |
| — | 注释/用例名与现状不符三处 | 低 | 已修 | `6f53cfd` |
| — | 设置页六项偏好的文案与实际行为不符（松散度、存档优先级） | — | 已修 | `3b20ed4` |

### 每条怎么修的

**C#1 速览卡连线全丢**（`GraphMini.tsx`）
graphology 的端点在 `forEachEdge` 回调的第 3、4 个参数里，边属性对象只装 `addEdge`
时传进去的东西（这里没传 = `{}`）。原来从 `attrs.source/target` 取端点拿到的是
`undefined`，每条边都被 `byId.get(undefined)` 静默丢掉：节点照排、连线全没、不报错。
先加复现测试（预期 3 条边、实得 0）再改。

**C#3 搬迁写失败仍删旧键**（`graphPrefs.ts`）
原来 `write()` 吞掉配额异常后仍无条件删旧键——写失败就把用户唯一的选择弄丢了。
改成确认新键写成功才删旧键，单测注入 `QuotaExceededError` 钉住时序。

**B#1 星体不变色**（`GraphCanvas.tsx`）
`needsUpdate` 只在 `pushGeometry()` 里设，而它只在力场跑动的帧被调用；收敛后外观全靠
`refreshAppearance()` 上传，那条路标了边、漏了星体。标记收进 `markAppearanceDirty()`，
星体与星轨同一条规则，并断言它不越界去标 `position`。

**B#2 铭牌避让无预算**（`graph-declutter.ts` / `GraphCanvas.tsx`）
每块铭牌都要和已占位矩形逐一比对：3200 块实测 15ms/帧、12800 块 68ms/帧。加 4ms
预算，整段摆位提到纯函数 `declutterPass()`（时钟可注入）。预算耗尽后剩下的铭牌隐一帧
并保住上次偏移，下一帧接着摆；摆位顺序是锚点优先、任务按连接度降序，所以预算永远
先给重要的牌。

**C#2 改依赖整场重建**（`GraphCanvas.tsx` / `GraphPage.tsx`）
没做增量边更新（边缓冲区、邻接表、铭牌资格都要跟着改，风险远大于收益），改为认出
「同一批节点、同一外径」：节点 id 逐位一致且 R 相同就接着上次的坐标与机位走。判据
`canReuseLayout()` 有单测。换松散度、增删任务、动过滤器、离开图谱页都照旧重来。

**A#1 拖拽松手力场停机**（`GraphCanvas.tsx`）
`settled`/`running` 的赋值点散在 6 处，停机只有一处（收敛），唤醒有三条而拖拽松手那条
从来没接上，注释还写着「松手后从新位置继续平衡」。所有唤醒路径收进一个
`resumeForce()`：停机只有一处、唤醒也只该有一处。

**D#1 星轨宽度**（`GraphCanvas.tsx` + 小样）
着色器漏了 NDC↔像素的 2 倍系数（窄带法线写成 `perp / uRes`），深度系数又一直钉在
下限（`clamp(340/depth, 0.45, 1.15)` 里的 340 是旧机位遗留，按小样机位
2.882×R258 ≈ 744 算系数只有 0.457）。两处相乘，「细」档最终只剩 0.23 像素。补回系数、
参考纵深校准到 744（该纵深上系数正好为 1，档位值字面成立）、下限抬到 0.5。

---

## 二、未修 · 中危 11 条

| # | 问题 | 位置 | 证据 / 影响 | 建议 |
|---|---|---|---|---|
| M1 | `autoLayout=false` 时同步跑 700 步 + `separate()` O(N²)，与「已时间切片」的注释相悖 | `GraphCanvas.tsx:794` | 大图首帧卡死 | 静态路径也走时间切片，或订正注释 |
| M2 | 旋转松手误触发点选（无位移阈值） | `GraphCanvas.tsx:709` | 从空白处旋转、松手压在节点上就选中 | 记录按下点，超过 ~4px 位移不视为点击 |
| M3 | `nodeRadius` 是绝对值、不随 `R = k√N` 缩放 | `graph-scene.ts:88`、`:247` | N 从 2 到 10000：R 涨 70×，星点直径 32.6px → 0.46px（亚像素）；`minGap` 同理，换档相对间距变 2.1 倍 | 按 R 比例给（同 `REST_RATIO` 的做法） |
| M4 | `separate()` 只补一半缺口 | `graph-force3d.ts:416` | 代数上 `d' = (d+minGap)/2 < minGap`；实测 n=2 两张同标签卡 8.373 → 8.630（目标 8.89），注释却写「硬分离」 | 去掉那个 `* 0.5`，或对 `d < minGap` 的对迭代 2~3 轮 |
| M5 | 「已完成」两处口径不一致 | `graph-build.ts:130`（按 `t.column`）vs `homeKpi.ts:121,157`（按 `completedAt != null`） | 遗留 done 无 completedAt 的卡，两边判相反 | 定一个口径，另一个引用它 |
| M6 | `getGraphLooseness()` 在渲染期写 localStorage | `graphPrefs.ts:95` | 迁移副作用，功能幂等但违反渲染纯度 | 改 `useState(getGraphLooseness)` 或显式 migrate |
| M7 | 首页速览卡主线程同步跑 80 轮 FA2，节点无上限 | `GraphMini.tsx:58` | 每次任务写入重跑 | 节点上限 + 降轮数 |
| M8 | 画布不可聚焦、无 `role`/`aria-label`、注释承诺的 Esc 清除选择未实现、键盘完全不可导航 | `GraphCanvas.tsx` 文件头与组件尾部 | 键盘用户用不了 | 补 `tabIndex` + Esc + 方向键 |
| M9 | `GraphSettingsPanel.test.tsx` 对三档标签与全部新文案零断言 | `GraphSettingsPanel.test.tsx` | 文案回退无人察觉 | 补断言（`3b20ed4` 只补了档位标签） |
| M10 | `hubPull` 位置修正绕过温度封顶 | `graph-force3d.ts:327` | 违反文件头声明的「位移永不超 temp」；300 步实测违反 5 次，全在锚点上 | 位移计入 `vel`，或改注释明确排除锚点 |
| M11 | 星轨/构图缓存的接线在 WebGL effect 内，jsdom 无上下文无法单测 | `GraphCanvas.tsx` | 仓库既有约定：画布整块 mock 掉 | 已知边界，抽出的纯函数已有单测 |

**M3 与 M4 是一对**，改了会让 150 任务的观感再变一次，需要先拍板。

---

## 三、未修 · 低危 17 条

- **数据口径**：自环/重复边污染度数统计（`graph-build.ts:220`，后端 `tasks.rs` 的
  `dependsOn` 无自环校验，子 agent 可写）；`graphPrefs.ts:160` 多余键透传；
  `GraphPage.tsx:721`「重置过滤器」把默认值写进存档，长期遮蔽「只看我的」。
- **注释/文档失真**：`graphPrefs.ts:9` 引用已不存在的 `R_MAX`；`graph-scene.ts:85`
  示例「度25≈8.4」实为 8.12；`docs/testing.md:52` 写「当前 209 例」（实测 810）；
  代码与 `docs/rust-bot-architecture.md` 交叉引用指向已退役的
  `TASK-GRAPH-DESIGN-2026-10-05.md`（§3.2 的 `physics.ts` 不存在）。
- **资源与生命周期**：`renderer.forceContextLoss()` 缺失；`will-change` 常驻；
  `pixelRatio` 只在建场取一次（缩放窗口不跟随）；无 WebGL2 能力探测；
  `cloudsDirty` 每次 hover 都重采样。
- **可访问性/动效**：`prefers-reduced-motion` 只关 autoRotate，云噪声、布局动画、
  阻尼照动；铭牌宿主 `pointer-events:none` 之外的键盘路径缺失（同 M8）。
- **图形参数**：`NODE_VERT` 点径无上限钳制。

---

## 四、已核验、确认没问题的部分

别再查了，这些都经过严格核验：

**力场内核**（角度 A）
- 网格 27 邻桶覆盖完整：7 档规模与暴力 O(N²) 对拍，截断内配对数逐一相等，漏检 0、误检 0。
- 斥力严格反对称：逐对核验 44850 对，`max |F_ij + F_ji| = 0`（牛三守恒）。
- 温度收敛是结构保证：585 步到下限，早于 `maxSteps: 700`；`KINETIC_EPSILON` 判据成立。
- 任务节点位移封顶生效：300 步实测违规 0 次（锚点例外见 M10）。
- 无 NaN / 无发散：5/50/400/1200 节点 × 三档共 12 组，`repairCount = 0`。
- `normalize()` 同步缩放 hints（历史 bug 已修）：归一后锚点零漂移。
- 确定性：`mulberry32` 同种子 10 万次逐位一致；同一份数据三次构造坐标 maxDiff = 0。

**渲染层**（角度 B）
- cleanup 逐项核过：geometry/material/bloom/composer/renderer/controls/事件/RO 全释放，
  `RenderPass` 不 dispose 是对的。
- rAF 不叠加；拾取排除相机背后的点且不是每帧调用；铭牌宿主单份，
  `pointer-events: none` 不挡拾取。
- `bestD * 0.7` 的择近规则经 17.64 万组枚举无缺陷。
- 粒子数有界：星点 1100、云团锚点 × 320。

**规格与工具链**（角度 D）
- `SPEC.md:111` 的 34 条规格断言与代码逐值一致（Bloom 1.35/0.82/0.42、1100 星点、
  锚点权重 3/2/1.5、separate ≤1200、P93 分位、时间切片 10ms/400 步等）。
- knip 零死文件/死 export；私有常量零孤儿。
- 生产与小样的着色器口径逐值同步。

---

## 五、验证记录

星轨那条做过**变异测试**：把着色器里的换算系数改回 `1.0`（即缺 2 倍系数的原始故障态），
测试变红；还原后全绿。证明新测试不是空转的。

```
npx tsc --noEmit         ✓
npx knip --no-progress   ✓
npx oxlint src           ✓（exit 0，仅既有 warning）
npx vitest run           82 文件 / 810 用例全绿
```

新增测试：`graph-edge-width.test.ts`（6）、`graph-declutter.test.ts`（10）、
`graph-appearance-dirty.test.ts`（2）、`graph-layout-cache.test.ts`（5），
以及 `graphPrefs.test.ts` 的搬迁时序用例、`GraphMini.test.tsx` 的复现用例。

---

## 六、遗留

- **`ChatPanel.test.tsx > 流式事件按会话过滤` 偶发红**（8 次全量跑里 2 次）。与图谱无关：
  `src/test/scrollNoop.ts` 的 `scrollTo` stub 是 scoped 的（`beforeEach` 装、
  `afterEach` 删），React 的 passive effect 在 teardown 之后才 flush 就撞上窗口期。
  一行修法（stub 提到全局 setup，或延到 unmount 后再删），当时未在本批处理。
- **中低危 28 条**待拍板，M3/M4 建议一起做（都是尺度体系，改完 150 任务的观感会变）。
