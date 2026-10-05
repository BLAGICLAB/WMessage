# 图谱标签聚簇 / 依赖编辑 / 标签近义设计（G4-CLUSTER · G5-DEPEDIT · G6-SYNONYM，2026-10-05）

> 承接 `docs/TASK-GRAPH-DESIGN-2026-10-05.md`（图谱主设计）。渲染层已迁移
> Sigma.js + FA2 worker（G3-SIGMA）。本设计覆盖三批：
> ① 标签亲和聚簇 + 视野自适应；② 普通任务卡依赖编辑；③ 标签近义（嵌入引擎）。
> 拖线建边明确不做（老板拍板）。

## 0. 调研结论（方案依据）

- FA2 原生支持**边权重影响布局**：`edgeWeightInfluence`（0=忽略 / 1=按权重正比吸引），
  见 [ForceAtlas2 原论文（Jacomy et al. 2014）](https://pmc.ncbi.nlm.nih.gov/pmc/articles/PMC4243594/)；
  graphology 实现默认权重即参与计算。
- **团簇分离的最大杠杆是 `linLogMode`**（Noack LinLog 能量模型，社区拉紧、团间推远），
  社区常用配置 `linLogMode: true, edgeWeightInfluence: 0.5`
  （[ipysigma #214 实例](https://github.com/medialab/ipysigma/issues/214)）。
- 虚拟锚点（每标签一个不可见伪节点 + 星型边）是社区常用聚簇手法，文献少但实现简单、
  边数线性（对比同标签两两连边的 O(k²) 团爆炸）；代价是给布局引入少量"假节点质量"。
- 项目内嵌引擎已具备语义向量能力：`memory::embed::embed_text(text) -> Option<Vec<f32>>`
  （bge-small-zh，512 维 L2 归一化，同步阻塞接口，须在阻塞线程调用）。

## 1. G4-CLUSTER 标签亲和聚簇 + 视野自适应

### 1.1 虚拟锚点

- graph-adapter 建图时，为每个**标签组**创建一个锚点节点 `taggrp:<组ID>`：
  `kind: "anchor"`、`hidden: true`、`size: 0`（reducer 再兜底 hidden）。
- 标签组 = 同名标签天然同组；**同义组**（G6）通过并查集合并到同一锚点。
- 每张带该标签组任务卡连一条**星型边** `任务卡 → 锚点`，
  边属性 `weight: 0.35`（dep/member 为 1）；FA2 `edgeWeightInfluence: 1` 下
  弱弹簧把同标签卡自然拉拢，团簇"松散可辨"而非焊死。
- 只为 **≥2 张卡**的标签组建锚点（单卡标签无聚簇意义，省节点）。

### 1.2 隔离纪律（锚点不污染业务视图）

- 统计条 / collectTags / 成员 chips / 详情面板：数据源是 graph-build 结果，**不含锚点**（锚点在 adapter 层追加）。
- Sigma 渲染：reducer 对 `kind === "anchor"` 置 `hidden: true`（节点/标签不画）；
  edgeReducer 对两端含锚点的边置 `hidden: true`。
- 事件：enterNode / clickNode / downNode / doubleClickNode 对 `taggrp:` 前缀一律忽略。
- 标签资格（degree ≥6 等）：改用建图时写入的 `degree` 属性（不含锚点边），
  reducer 不再用 `g.degree()`（锚点边会虚增度数）。

### 1.3 视野自适应

- **重建图后**：计算存活节点包围盒 → `camera.animate` 对准中心、ratio 铺满视口
  （≈1s 缓动）——过滤后集合自动"散开铺满"，解决"缩在角落"。
- **收敛自动停机时**：再适配一次终态视野（FA2 停机前最后微调的位置可能略出框）。
- 复位过滤器 = 重建图 = 自动适配，无需专门按钮。

### 1.4 参数（集中在 graph-adapter 常量）

`TAG_EDGE_WEIGHT = 0.35`；`TAG_ANCHOR_MIN_CARDS = 2`。linLogMode 本版**不开**
（整体观感变化大，作为后续调参项留档）。

## 2. G5-DEPEDIT 普通任务卡依赖编辑

### 2.1 数据与语义

- `dependsOn` 字段与 `task_patch` 白名单通道本就全量任务可用（工作流画布只是
  唯一现存入口）。语义：`A.dependsOn 含 B.id` = B 是 A 的上游（B→A 有向边）。
- **本版范围**：只允许**本人卡**互设依赖（ownerId 均为空的卡）；外来卡只读不变。
  纯可视化 + 图谱呈现，**不**联动看板排序/今日规则/归档规则（语义扩展另立设计）。

### 2.2 环检测（纯函数 `wouldCreateDepCycle`）

- 入参 `(tasks, selfId, depId)`：self 的 dependsOn 增加 depId 是否成环。
- 判定：从 depId 沿 dependsOn 边 DFS，能到达 selfId ⇒ 成环（拒绝）；
  含 `depId === selfId` 自环拒绝。纯函数放 graph-build.ts（图论工具归属），单测覆盖
  直连/传递/无环/自环四类。

### 2.3 UI（详情面板）

- 本人任务节点详情面板新增「依赖」区：
  - 现有依赖列表（标题解析自全量 tasks；已删除的显示「已删除任务」可移除）；
  - 添加：搜索输入（候选 = 本人卡 ∧ 非自身 ∧ 未删除 ∧ 未已是依赖 ∧ 过环检测）+
    添加按钮；命中校验失败给出行内提示（环 / 不存在）。
- 写路径：`onPatchTask(id, { dependsOn })` → App 既有 `patchTask`（task_patch →
  RMW → tasks-updated 广播）→ tasks 变更 → 图谱重建自动出现连线。零新后端命令。
- 移除 = patch dependsOn 去掉该项（同一通道）。

## 3. G6-SYNONYM 标签近义（嵌入引擎）

### 3.1 Rust 侧 `tag_similar_pairs`

- 新模块 `tag_similar.rs`：命令 `tag_similar_pairs(app, tags: Vec<String>)
  -> Vec<TagPair { a, b, score }>`。
- 流程：去重 → `spawn_blocking` 内逐标签 `embed_text`（**进程内 LRU 缓存**
  tag→向量，跨调用复用；嵌入失败/引擎未加载的标签跳过）→ 两两余弦
  （已 L2 归一化，点积即余弦）→ `score ≥ 0.78` 的对返回，上限 200 对。
- **审计**：每次计算写 INFO 事件 `tag_synonyms`（输入标签数、输出对数）；
  引擎不可用写 WARN `tag_synonyms_unavailable`（一次/调用）。
- 注册 lib.rs invoke_handler + 桥一致性自动核对。

### 3.2 前端

- GraphPage：`tagList` 变化（去抖：仅当标签数或集合变化）→ invoke
  `tag_similar_pairs` → `synonymPairs` → 并查集把标签并成同义组 →
  `tagGroups: Map<tag, groupKey>` 传给 adapter（§1.1 锚点按组建）。
- 失败/引擎不可用 → 空对（退化为同标签聚簇），不打扰。
- 常量 `TAG_SIMILAR_THRESHOLD = 0.78`（bge 短文本经验值，可调）。

## 4. 审计与安全

- 新增审计：`tag_synonyms`（INFO，每次计算）/ `tag_synonyms_unavailable`（WARN）。
- 依赖编辑走既有 task_patch 通道（RMW 基线 + tasks-updated 广播），不新增写路径；
  环拒绝发生在前端（数据通道本就允许 dependsOn 任意值，与工作流画布一致）。
- 外来卡（ownerId 非空）依赖编辑入口不渲染（只读语义不变）。
- 锚点节点**不落库**（adapter 层即时构造，非持久数据），导出文件无锚点。

## 5. OCR 审计计划

- 三批各 1 轮 OCR 代码评审（批次 spec 内声明 ocr_plan），预期重点：
  - G4：锚点隔离是否泄漏（统计/度数/事件/导出四路径）；
  - G5：环检测边界（自环/传递/已删任务）与 patch 白名单一致性；
  - G6：嵌入调用线程合规（spawn_blocking）、缓存无界增长、阈值语义。
- OCR findings 处置沿用 C/H/M/L 分级入账惯例（r1 修 high+，中低缓期入账）。

## 6. 验收环节

- 自动化：adapter 测试（锚点构造/权重/同义组并查）、graph-build 环检测 4 类、
  Rust tag_similar 纯函数测试；vitest + cargo test + test-fast 全绿 + build。
- 浏览器压测 harness：1227 档验证聚簇形态 + 过滤后视野铺满 + 依赖连线出现。
- 人工冒烟：`MANUAL-SMOKE-ACCEPTANCE-TASK-GRAPH-2026-10-05.md` 增补 §12
  （聚簇/视野/依赖编辑/近义四节，含桌面演示数据步骤）。

## 7. 未来项

- linLogMode 调参开关；拖线建边；跨人依赖；近义阈值用户可调；依赖语义联动
  （上游完成提醒）。
