# 任务图谱模块设计（G1-OWNER / G2-AUTOTAG / G3-GRAPH，2026-10-05）

> 单机个人助手 × 多人数据汇总。个人看自己的看板；把多人的任务卡导出文件
> 汇总导入后，在图谱里看整个部门的任务关系与（后续）年终任务量统计。
> 图谱交互对齐 Obsidian 关系图谱：力导向布局、hover 邻接高亮、度数定大小、
> 过滤器侧栏、局部放大。工作流产生的任务卡（已有 dependsOn 父子/依赖关系）整体入图。

## 0. 背景与决策记录

- 单机应用，无账号体系；多人协作 = 各自导出 `wmessage-tasks-*.json`，交给汇总人导入。
- 设置页已有个人资料（用户名/头像，`profile.json`）。但导出不含资料、任务无归属人字段，
  导入后与本地任务混在同一张表——这是本设计要解决的核心缺口。
- 老板拍板：
  1. **看板/挂件/归档/回收站/⌘K 默认只显示自己的任务**；图谱与统计看全部。
  2. **归档纳入图谱**：归档 = 完成的历史任务，是统计主体，只排除回收站（软删）。
  3. 归档时大模型自动打标（≤3 个）——当前未实现，本设计一并落地。
- 联网调研结论：Obsidian 图谱 = d3-force 同款物理模型（斥力 + 连线弹簧 + 向心力，
  速度 Verlet 积分 + alpha 冷却）+ canvas 逐帧渲染 + hover 高亮邻接/其余淡出 +
  节点大小随连接度 + 过滤变更后暖启动重算。渲染器闭源，此为社区公认等效配方。

## 1. 数据模型（G1-OWNER）

### 1.1 tasks 表加 `owner_id TEXT`（第 29 列）

- `TASKS_DDL` 补列；open_db 幂等 ALTER 迁移补新清单 `OWNER_TASK_COLUMNS`
  （与既有列清单并列遍历，不动原清单语义）。存量行 NULL = 本人。
- `Task` 增 `ownerId?: string`（serde camelCase，缺省 None）。**库内 NULL 恒等于本人**：
  前端/机器人写入路径不感知该字段（serde default None → 写 NULL），零改动兼容。
- 前端 `src/types.ts` `Task.ownerId?: string`。渲染层约定：`ownerId == null` 即本人。

### 1.2 新表 `people`（成员注册表）+ `db/people.rs`

```sql
CREATE TABLE IF NOT EXISTS people (
  id         TEXT PRIMARY KEY,   -- personId（UUID，首次导出/导入时生成并固化）
  name       TEXT NOT NULL,
  avatar     TEXT,               -- 预留（v1 不填，信封不携带头像，见 §1.4）
  is_self    INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER
);
```

- `people_list` 命令：返回 `[{id, name, isSelf}]`；is_self 行的 name 读取时用
  profile 现值覆盖（改名后无需回写 people）。
- 本人 personId：`profile.json` 加 `personId` 字段（serde default），首次调用
  `profile::ensure_person_id` 时生成 UUID v4 固化，并 upsert `people(is_self=1)` 行。
  `profile_set_name` 只改 profile，people 的本人名由 people_list 读取时合并（单一事实源）。

### 1.3 导入归属归一（核心规则）

导入端把每条任务的归属解析为「有效 personId」：

```
effective = task.ownerId 非空 ? task.ownerId : envelope.profile.personId
存库: effective == 本人 pid ? NULL : Some(effective)
```

- v1 裸数组（旧版导出/本人旧文件）：全部视为本人（NULL），行为与旧版完全一致。
- 透明转发：张三的库里已有李四的卡（ownerId=李四），张三把全量导给我，我导入后
  李四的卡仍归属李四——`people` 注册表随信封同步流转，图谱按人聚合不串档。

### 1.4 导出格式 v2（信封）与向后兼容

```jsonc
{
  "version": 2,
  "exportedAt": 1759651200000,
  "profile": { "personId": "…uuid…", "name": "张三" },
  "people":  [ { "id": "…", "name": "李四" } ],   // 汇总人已知的其他成员（不含自己）
  "tasks":   [ /* Task[]，ownerId 已按 §1.3 显式盖章 */ ]
}
```

- 导出 = `tasks_export` 重写：读全量 → NULL owner 盖章为本人 pid → 组信封序列化。
  头像不入信封（dataUrl 动辄数百 KB，多人多次交换会滚雪球；图谱侧栏用名字 +
  首字母色块代替，`people.avatar` 列预留）。
- 导入 = serde untagged `{ v2: Envelope, v1: Vec<Task> }`，按序尝试；people 与 tasks
  同事务写入；people upsert 保留最新 name；信封未携带的 ownerIds 补占位行
  （name="未知成员"），图谱不出现悬空归属。

### 1.5 渲染层 owner 过滤（G1-OWNER 前端部分）

- App.tsx `visibleTasks`：在现有 origin 过滤上叠加 `!t.ownerId`（看板/归档/回收站/⌘K
  四处已统一走该选择器，一处改动全覆盖）。
- WidgetApp 任务读取处同规则补过滤。
- 导入/删除人数据的清理：不做「删除某人」入口（v1）；外来卡可进回收站但默认不可见，
  无残留风险。

## 2. 归档自动打标（G2-AUTOTAG）

- 触发点：前端观测「已加载任务从未归档 → 归档」转变（App.tsx 用 prev/next 快照比对，
  覆盖每分钟规则 tick、reload、远端合并三条路径），对 `tags` 为空的命中卡
  fire-and-forget 调用 `task_autotag`。首屏加载（prev 为空）不触发——防止启动时
  对历史归档卡批量调用；批量补打标走独立入口（本版不做，入 §9）。
- 后端 `task_autotag(id)` 守卫链（不满足静默返回 Ok(skip)）：
  行存在 ∧ col=done ∧ archived ∧ 未软删 ∧ tags 空 ∧ **owner_id IS NULL**（只给自己的卡打标）。
- LLM 调用复用 `bot_chat::summarize_messages` 一次性推理样板（同 `workflow_decompose`：
  不开会话、不进聊天记录、失败带错误重试 1 次）。模型 = 全局 active 模型——
  统一词表比个性化更重要，按卡模型覆盖**有意不用**。
- 提示词两段式：指引段（角色+取材：标题/备注/子任务）+ 硬契约段
  `{"tags":["…"]}` ≤3 个、每个 2~12 字、去重。校验链在服务端：fences 剥离 → JSON →
  数量/长度/去重/全量 trim，超限截断不整包拒（打标是增强功能，宁缺毋炸）。
- 持久化走 `task_patch`（tags 在白名单）→ 复用 RMW 基线 + `tasks-updated` 事件广播。
- 失败静默：前端 console.error + 后端审计 `task_autotag outcome=failed`，不打扰用户。

## 3. 任务图谱（G3-GRAPH）

### 3.1 数据构建（纯函数 `graph-build.ts`）

- 输入：全量 tasks（含归档、**排除 `deletedAt`**）、workflows、people、过滤器状态。
- 节点：任务节点 + **工作流 hub 节点**（`wf:{workflowId}`，Obsidian 的文件夹锚点角色——
  把无依赖的松散工作流也串进星系）。
- 边两类：
  - `dep`：`dependsOn`（有向，画箭头；悬空引用/被过滤端点 → 丢边）；
  - `member`：工作流卡 → 所属 hub（无向细边；hub 弹簧刚度低于任务间依赖，让团簇松散可分）。
- 度数 = dep 边数 + member 边数（hub 自身度数 = 成员数），驱动节点半径。
- 过滤器：状态三开关（todo/doing/done，默认全开）、成员 chips（默认全选）、
  标签多选（按使用计数排序）、工作流多选、年份（按 `completedAt`，缺省全部）、
  孤立节点开关（默认开）、已完成淡化（默认开，仅视觉）。归档随 done 走。
- 统计口径：一切按 `completedAt`（完成时间）归年归月，**不用归档时间**（完成 7 天后
  才归档，跨年会把账记错年份）。

### 3.2 力导向物理（`physics.ts`，零新依赖）

对齐 Obsidian/d3-force 等效模型，手写实现（d3-force 不在依赖树，避免幻影依赖 + 离线安装风险）：

- 每帧：网格分桶短程斥力（O(n·k)，cutoff ≈ 斥力半径，替代 Barnes-Hut——节点规模
  ≤数千足够）+ 连线弹簧（目标距离 dep 90 / member 130，刚度 0.06 / 0.02）+
  向心弱引力（0.03）+ 碰撞分离（半径和 + padding）。
- 积分：速度 Verlet，速度衰减 0.6；alpha 1 → 0.02（×0.985/帧）冷却后停物理循环，
  交互（拖拽/过滤/缩放Change）暖启动到 0.5。
- 半径：任务 `3 + sqrt(deg)·2`，hub `7 + sqrt(deg)`。初始位置：按序螺旋铺点。
- 斥力/弹簧常数是集中配置常量（顶部 const），验收调参不改逻辑。

### 3.3 画布渲染与交互（`GraphCanvas.tsx`）

- Canvas 2D + devicePixelRatio 缩放；相机 = 平移 + 滚轮缩放（指针为锚，0.15~4 夹紧）。
- 配色读 CSS 变量（`--bg/--t*/--brand/--success/--danger/--edge`），MutationObserver
  监听 `html.dark` 切换重读——双主题自动适配，不硬编码色值。
- 着色模式（侧栏单选）：按状态（todo=中性灰 / doing=brand / done=success 淡化）或
  按成员（本人=brand，外来成员按序取固定调色盘，图例即成员 chips 本身）。
- hover：邻接子图全亮、其余淡出至 0.12（Obsidian 招牌交互）；点击选中 → 右侧详情面板；
  空白拖拽 = 平移；节点拖拽 = 固定该点（fx/fy）并暖启动。
- 标签文本：缩放 × 阈值以上渐显；高连接度节点常显。
- 详情面板（nm-card）：标题/状态/截止/成员（头像或首字母块）/标签/note（截断）/
  result.summary（工作流卡）；「在看板打开」仅本人卡可见（外来卡只读，杜绝跨人 RMW 写）；
  工作流卡提供「打开工作流」切视图。
- 搜索框：标题/note 匹配节点描金环，回车选中第一个命中。
- 顶栏统计条：`N 任务 · M 依赖 · K 工作流 · P 成员`（图谱定性，年终数字报表入 §9）。

### 3.4 导航接入

- `RailView` 增 `"graph"`，`NAV_ITEMS` 增「图谱」（lucide `Waypoints`，工作流之后）。
- App.tsx 渲染分支：`<GraphPage tasks={tasks} … />`（**传全量**，图谱自带过滤）；
  组件目录 `src/components/GraphPage/`（GraphPage / GraphCanvas / graph-build / physics + 测试）。

## 4. 分批账本

| 批 | 内容 | 触碰 |
|---|---|---|
| G1-OWNER | owner_id 列迁移 + people 表/命令 + 导出 v2/导入兼容 + 渲染层 owner 过滤 | db/tasks.rs、db/people.rs（新）、db/mod.rs、profile.rs、lib.rs、types.ts、storage.ts、App.tsx、WidgetApp.tsx、rust-bot-architecture.md（模块地图） |
| G2-AUTOTAG | task_autotag 命令 + 前端归档转变钩子 | db/tasks.rs、task_autotag.rs（新）、App.tsx |
| G3-GRAPH | graph-build/physics/GraphCanvas/GraphPage + 导航 + 单测 | src/components/GraphPage/*（新）、App.tsx |

批间依赖：G3 依赖 G1（people/owner）；G2 独立可先行。设计红线：
`origin` 语义不动（owner 与画布归属正交）；`db_load` 仍返全量（过滤只在渲染层，
图谱与未来统计都需要全量）；SQLite 事务边界/busy_timeout 纪律不动。

## 5. 验收环节

### 5.1 自动化门禁（提交前必须全绿）

- `bash scripts/test-fast.sh`：审计批次号防线 / cargo fmt+check / pytest 桥一致性
  （新增 people_list、task_autotag 必须 lib.rs 注册）/ 错误码一致性 / 模块地图对拍
  （db/people.rs、task_autotag.rs 必须写入 rust-bot-architecture.md）/ tsc / knip / oxlint / vitest。
- 全量：`cargo nextest run`（含新增 Rust 单测）+ `npx vitest run` 全绿 + `npm run build`。

### 5.2 单测锚点

- Rust：信封 v2 解析 / v1 兼容 / 坏 JSON 拒绝；owner 归一（外来盖 Some、本人归 NULL）；
  导出信封含 profile.people 且 owner 显式；people upsert 最新名覆盖；autotag 校验链
  （fences/超限截断/≤3 去重）。
- 前端：graph-build（建图/丢悬空边/过滤器/度数/排除软删含归档）；physics（弹簧收敛
  ≈目标距离、斥力分离重叠、alpha 冷却停止）；GraphPage 冒烟（渲染 + 过滤器交互）。

### 5.3 人工冒烟

`docs/MANUAL-SMOKE-ACCEPTANCE-TASK-GRAPH-2026-10-05.md`（另建，⭐ 快速子集），
隔离数据目录验收方式：`WMESSAGE_TEST_DATA_DIR=/tmp/wm-graph-test npm run tauri dev`，
不污染真实库。

### 5.4 视觉验收

图谱空态 / 力导向成图 / hover 淡出 / 详情面板 / 过滤器侧栏 / 双主题截图过
visual-judge（nm 新拟态统一）。

## 6. 性能边界

- 数千节点可交互（网格短程斥力 O(n·k)；物理冷却后零 CPU，仅渲染）；
- 首帧布局 ≤ 1s（千节点暖启动）；过滤器切换暖启动 ≤ 0.5s；拖帧不阻塞事件循环
  （物理在 rAF 内，每帧限步）。

## 7. 未来（不在本版）

- 年终统计视图（按人 × 月 × 状态 GROUP BY，导出 Excel/飞书表格）；标签共现边；
  局部图谱模式（选中节点 N 度邻域）；存量归档卡补打标按钮；信封携带头像；
  跨人任务依赖（dependsOn 已全局唯一，天然支持）。
