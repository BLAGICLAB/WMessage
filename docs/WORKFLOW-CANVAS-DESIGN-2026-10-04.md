# 工作流画布设计方案（Workflow Canvas SPEC）

> 2026-10-04 · 状态：**v1 设计定稿（讨论收口，待评审实施）**
> 目标：主窗口**左侧导航栏新增「工作流」栏目**（功能模块，非改主窗口结构）——点击栏目进入
> 工作流画布：用户一句话描述总目标 → AI 拆解成任务卡 DAG → 自动连线
> → 人工调整 → 保存 → 一键拓扑执行；工作流可 JSON 导入导出，作为可复用 SOP 模板。
> 现有看板/归档/工作区/回收站/设置五个栏目零改动。
> 关联：`docs/TASK-CHAT-EXECUTION-DESIGN.md`（执行=会话）、`docs/SUBAGENT-ORCHESTRATION-DESIGN-2026-09-27.md`
> （并发闸/预算/状态机）、`docs/SKILL-DSL.md`（step 链先例）、`docs/rust-bot-architecture.md`（现行架构）；
> 审计链：`docs/BATCH-SPEC-TEMPLATE.md`（批 spec/OCR 计划格式）、`docs/OCR-FOLLOWUPS-INDEX.md`（衍生债账本）、
> `docs/OCR-FIX-PLAN-2026-09-21.md`（基线账本）。

**讨论中已确认的决策**（2026-10-04 与老板对齐）：

1. 只保留「AI 拆解」一种生成模式，砍掉"从首页挑卡进画布"——入口 = 一个自然语言输入框，适合小白
2. 节点就是 Task 实体（不是独立的工作流节点类型），画布只是任务卡的另一种呈现形式；执行/持久化/事件管道全复用
3. 工作流任务卡默认不在看板/挂件显示（`origin` 过滤），bot 工具仍可见
4. 总目标卡 = 画布上的特殊标识节点（绑定工作流元数据，**不是 Task**），兼任运行入口，v1 不做自动验收卡
5. 生成阶段全在内存草稿，**点「保存」才落库**（防垃圾卡、防事件风暴）；「开始执行」未保存时置灰
6. 目标不满意/改了 → 整图重新生成（丢弃草稿重来），v1 不做增量重拆
7. 拆解提示词在设置页「工作流」分区可配置（两段式，见 §8.3），带恢复默认
8. 画布支持手动加卡/删卡，删卡连带清理所有相关连线
9. 整卡对整卡连线（无端口类型系统）；v1 连线语义 = "上游完成后才执行下游"
10. 画布库选型 @xyflow/react（React Flow v12，React 19 兼容，MIT）
11. 产出格式单一 JSON（不做 ComfyUI 式 UI/API 双格式），带 `version` 字段；导入 = 实例化（全新 id），与 `tasks_import` 的按 id 合并语义刻意分离

---

## 1. 竞品调研对比（2026-10-04 检索核实）

> 结论先行：各家的"引擎侧"复杂度都来自多节点类型 + 类型化端口 + 参数面板，我们单一节点类型
> 全部绕开；各家踩过的坑和成熟语义（n8n 连线键、Airflow 触发规则、LLMCompiler 调度）直接吸收。

| 方案 | 产出格式 | 值得抄的 | 明确不抄的 |
|---|---|---|---|
| **ComfyUI** | 双 JSON：UI 格式（nodes+links+坐标快照）与 API 格式（扁平执行图，`["节点id", 槽号]` 引用），互不可换 | ① `version` 字段保格式演进；② 把工作流 JSON 嵌入 PNG 元数据实现"拖图还原工作流"（记入远期） | 端口/类型系统、UI+API 双格式（我们逻辑图=执行图，一套就够）、Python 节点注册表 |
| **n8n** | 单 JSON：`nodes[]`（name/type/typeVersion/position/parameters）+ `connections`（按**节点名**做 key）+ `pinData` + `settings.executionOrder` | ① 导入时重新生成 id（与我们的实例化语义一致）；② AI 生成的 workflow 必须先校验再入库；③ `typeVersion`/`executionOrder` 说明执行语义要趁早版本化 | **connections 按节点名做 key**——重命名即断链，是公认设计债；我们用稳定本地 id。pinData 测试数据钉扎（v1 用单卡 🤖 试运行替代） |
| **Coze（扣子）** | 官方**无**工作流 DSL 导出（仅 Bot 复制分享），跨平台迁移靠第三方转换脚本 | 反面教材：不开放格式 = 锁死用户，我们坚持开放 JSON 导出 | 闭源格式 |
| **Dify** | 单 YAML DSL，可进 Git 做 diff 版本管理，被阿里云百炼等第三方兼容导入 | 版本化 DSL + 开放可导入形成生态（百炼主动兼容 Dify 就是证据） | YAML（本项目全链路 JSON 惯例 + tasks_export 先例，不引第二序列化格式） |
| **LLMCompiler**（UC Berkeley, ICML 2024） | 非文件格式，是执行架构：Planner 生成 DAG → Task Fetching Unit **依赖就绪即并行调度** → 失败触发局部 re-planning | ① "就绪即跑"调度模型（≠ 整层同步屏障）；② 失败局部重规划（记入 Phase 2）；③ 学术实证：LLM 实际执行会偏离声明计划 → 必须有校验环节，**我们的人审保存门就是校验环节** | 运行时自动 re-planning（v1 人工调整替代） |
| **Airflow**（trigger rules） | 非 JSON，触发语义行业标杆 | **失败下游传递跳过**：默认 `all_success`，上游失败 → 下游递归标 SKIPPED。v1 直接采用该默认语义 | trigger rule 变体（`all_done`/`one_failed` 等，记入远期） |
| **断点续跑先例**（各 DAG 引擎 checkpoint 模式） | 从 checkpoint 重建 DAG、跳过已完成节点续跑 | 我们的"任务卡即节点"天然自带 checkpoint：节点状态就在 Task 上，**断点续跑零成本**（§10.4） | — |

**对框架的净影响：零改动**。吸收的全部是语义细节与防坑决策（连线键、失败传播、调度模型、
校验门），总体框架（AI 拆解 → 自动连线 → 人工调整 → 保存 → 执行）保持不变。

---

## 2. 总体架构

```
┌─ 左侧导航栏新增「工作流」栏目 → 点击后主区域切换为工作流模块 ────────────┐
│                                                                          │
│  [空态] 居中输入框：一句话描述你想让 AI 做的工作流 …        [生成]      │
│                                                                          │
│  [画布态] React Flow 画布                                                │
│   ├ 总目标卡（特殊节点，绑定 workflows 行，非 Task）→ ▶ 运行整图       │
│   ├ 节点 = 任务卡（复用 TaskCardContent + nm-card 新拟态样式）          │
│   ├ 边 = dependsOn 的可视化（无独立边存储）                             │
│   └ 工具栏：＋加卡 ｜ 重新生成 ｜ 保存 ｜ ▶开始执行 ｜ 导入/导出 ｜ 切换工作流 ▾ │
└──────────────────────────────────────────────────────────────────────────┘

数据流：
  草稿（前端内存）--保存--> workflows 行 + tasks 行（事务，内容指纹 diff 保留已执行节点）
  执行：workflow_run → Rust 拓扑 ready-queue → 每节点 run_task_in_chat(origin=Workflow)
        → 复用 SubagentGate 并发闸 / 预算 / 30min 超时 / 会话即记录
  回显：复用 tasks-updated 事件管道，画布节点按 column/botAssigned/result 变色（零新事件）
```

新增模块落点：

- **导航集成（唯一触碰主窗口的点）**：`src/App.tsx:62` 的 `RailView` union 与
  `NAV_ITEMS`（App.tsx:63-68）各加一项 `"workflow"`，主区条件渲染（App.tsx:773）加一个
  分支——完全复用现有栏目切换模式，建议插在「看板」之后（核心功能前置，位置可调）；
  其余栏目组件零改动
- 前端：`src/components/WorkflowCanvas/`（Canvas / GoalCard / TaskNode / Toolbar / DecomposeDialog）
- Rust：`src-tauri/src/workflow_runner.rs`（拓扑调度）、`src-tauri/src/workflow_decompose.rs`（一次性 LLM 拆解）、`src-tauri/src/db/workflows.rs`
- 依赖：`@xyflow/react`、`@dagrejs/dagre`（自动布局，选型理由见 §9）

---

## 3. 数据模型

### 3.1 Task 新增字段（`src/types.ts:58` + `src-tauri/src/db/tasks.rs:90` 镜像）

```ts
Task {
  ...
  origin?: "user" | "workflow"      // 缺省 "user"，老库兼容
  workflowId?: string               // 所属工作流
  dependsOn?: string[]              // 上游任务 id 列表（执行语义，固有属性）
  canvasPos?: { x: number; y: number }  // 画布坐标；仅工作流卡使用
}
```

- `dependsOn` 存任务本身，不存画布——不在画布上依赖依然生效（将来看板可显示"有 N 个前置"徽标）
- SQLite：tasks 表幂等 `ALTER` 加 4 列（`origin TEXT DEFAULT 'user'`、`workflow_id TEXT`、
  `depends_on TEXT`（JSON 数组，同 tags/subtasks 惯例）、`canvas_x REAL`、`canvas_y REAL`），
  老库自动补列（SUBA-1 迁移模式）
- **4 处同步点**（types.ts 头部警示）：TS 类型 ／ Rust 镜像 struct ／ open_db 列约束 ／
  bot 工具注册表 task schema，逐处过审（§17.A）
- 工作流卡约束：不参与 ⏰ 定时扫描（bot_scheduler 扫描条件排除 `origin='workflow'`，
  防双触发）；DAG 内执行**绕开 exec_steps 逐步执行**（同定时任务 headless 路径，
  避免逐步 pending 卡死整图）

### 3.2 新表 workflows（`src-tauri/src/db/workflows.rs`）

```sql
CREATE TABLE IF NOT EXISTS workflows (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,            -- 列表展示名
  goal TEXT NOT NULL,            -- 用户原始自然语言目标
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
```

- 总目标卡绑定此行（不是 Task），解决"执行到总卡时干什么"的语义死结
- 多工作流天然支持（模板导入即新建），画布顶栏下拉切换 + 新建/重命名/删除（删除需确认，
  连带删该 workflowId 全部任务卡）
- 支撑拆解的工作流执行进度（done/total）由前端从任务列实时推导，**零新事件**

### 3.3 过滤规则

- 看板三列 / 挂件「全部任务」/「今日专注」：排除 `origin === 'workflow'`（设置页给
  「显示工作流任务」开关，默认关）
- bot 工具（list_tasks / query_single_task / search_tasks）**不过滤**——下游节点执行
  agent 靠它查上游结果（§11 数据传递）

---

## 4. 工作流文件格式 v1（导入导出 SOP 模板）

```json
{
  "version": 1,
  "generator": "wmessage 0.1.0",
  "exportedAt": "2026-10-04T12:00:00+08:00",
  "name": "周报生成流水线",
  "description": "可选的一句话说明",
  "goal": "用户原始自然语言目标",
  "nodes": [
    { "id": "n1", "title": "收集本周任务记录", "note": "产出：本周任务清单",
      "tags": [], "dependsOn": [], "pos": [240, 80] },
    { "id": "n2", "title": "汇总成周报", "note": "引用 n1 的结果",
      "tags": [], "dependsOn": ["n1"], "pos": [240, 320] }
  ]
}
```

要点（吸收自对比调研）：

- `version` 必填（ComfyUI 格式演进不炸老文件的关键）；后续加分支/条件节点时升 `version` 写迁移
- `id` 为**文件内本地 id**（n1/n2），导入时重映射为真实任务 id → 同一模板可反复实例化
- **AI 拆解输出不含 `pos`**——坐标由前端布局算好后填入再落盘；LLM 排坐标是浪费 token 且难看
- 导入 = **实例化**：全新 workflow 行 + 全新任务 id（对照：`tasks_export/import`
  （App.tsx:510、db/tasks.rs）的按 id + updatedAt 合并语义，两个入口两种用途，不混用）
- 导入校验（n8n 经验：AI 生成物必须校验后再入库）：version 已知、nodes ≤ 30、id 唯一、
  dependsOn 引用存在、无环无自环、title 非空 ≤ 80 字、note ≤ 500 字；违规整包拒绝并报具体条目
- 入口：画布工具栏「导入 / 导出」，走 Tauri dialog 存取 `.wflow.json` 文件

---

## 5. 工作流模块 UI（点击导航栏「工作流」栏目后的主区域）

### 5.1 状态机：空态 → 草稿态 → 已保存态 → 执行态

- **空态**：居中大输入框（placeholder：如"帮我整理本周任务并生成一份周报文档"）+「生成」+
  「从模板导入」；顶栏工作流下拉（空则只有新建）
- **草稿态**：生成结果直接成图（总卡 + 节点 + 自动布局的边）；所有编辑只改内存；
  「保存」高亮，「开始执行」置灰
- **已保存态**：一切照常编辑，编辑后回到"未保存修改"提示态
- **执行态**：运行中节点 🤖 徽标 + 描边色（复用 botAssigned 驱动）；顶栏进度
  `done/total`；「停止」按钮（对未启动节点生效，运行中会话走既有 /stop）

### 5.2 节点与连线

- 节点内容复用 `TaskCardContent`（`src/components/TaskCardContent.tsx`）+ `.nm-card` 样式，
  保证与看板视觉统一；节点状态色：todo 灰 / doing 蓝+🤖 / done 绿+✔ / 失败红（result.status）
- 连线：从节点右缘 handle 拉到目标节点左缘（整卡对整卡，无端口）；贝塞尔曲线
- 连线校验（React Flow `isValidConnection`）：禁止自环、禁止重复边、**禁止成环**（实时
  DFS 判可达性），非法时 handle 变红并 toast 原因
- 手动加卡：工具栏「＋」→ 在视图中心新建空白卡进草稿；删卡：节点悬浮删除按钮 →
  连带清掉它指向别人的边和别人指向它的边（`dependsOn` 双向清理，§决策 8）
- 小白兜底：画布缩放按钮 + 「适应视图」；拖动/框选走 React Flow 默认交互，不自研

### 5.3 重新生成

- 按钮在**草稿态与已保存态均可用**：已保存时弹确认（"当前画布将被新生成结果替换，已保存的
  版本保留至你再次保存"）；确认后回输入框（预填原 goal 可改）→ 重新拆解 → 新草稿
- v1 无增量重拆（LLMCompiler 局部 re-planning 记入 Phase 2）

---

## 6. AI 拆解（workflow_decompose）

### 6.1 流程

```
用户 goal（≤500字）+ 用户可编辑提示词指引段
   │  一次性 LLM 调用（不开会话、不写 bot_messages、不进聊天记录；
   │  复用 bot 现有模型客户端与 keyring 配置）
   ▼
JSON 输出 → schema 校验 + 环检测 + 上限校验（任一违规 → 整体拒绝，报错可读，不部分入库）
   ▼
本地 id 分配 → @dagrejs/dagre 分层布局 → 生成草稿渲染画布
```

- 模型参数上限：输出节点数硬顶 30（契约要求 ≤ 20，留余量）；解析重试 1 次
  （附错误信息让模型自修），再失败即报错
- 拆解过程有 loading 态 + 可取消（悬空请求 abort）

### 6.2 输出契约（代码硬拼，模型只见到完整提示词）

```json
{ "subtasks": [ { "title": "≤80字", "note": "≤500字，说明做什么/产出什么", "dependsOn": [0, 2] } ] }
```

- `dependsOn` 用**数组下标**引用（比让模型编 id 更稳），前端转本地 id
- 校验项：JSON 可解析、subtasks ≥ 1 ≤ 30、title 非空限长、dependsOn 下标合法且 <
  自身下标（强制前向引用，结构上杜绝环）、无重复 title（重复自动加后缀）

### 6.3 提示词两段式（设置页可配的是第一段）

- **指引段（用户可编辑，存设置）**：角色设定、拆解风格、粒度要求、领域偏好；
  设置页 textarea + 「恢复默认」
- **契约段（代码硬拼，用户不可见不可改）**：输出 JSON 格式说明 + 字段约束 + few-shot
  示例 + "只输出 JSON" 硬指令——用户怎么改指引段都破坏不了契约（§决策 7）
- 拆解与执行注入系统均声明当前日期等基础上下文，与其他路径一致

---

## 7. 草稿与保存（workflow_save）

- 草稿仅在前端内存（React state）：生成、拖动、改卡、连断线都不落库、不发事件
- 「保存」= 单个 Tauri command 事务：
  1. workflows 行 upsert（name 取自输入框生成时的自动命名，可改）
  2. **内容指纹 diff**：对每个节点算指纹 `(title + note + dependsOn序列)`，与库中该
     workflowId 现有任务比对——指纹相同者**保留原任务 id**（连带 column/result/budget/
     子任务等全部执行痕迹），新增者建新卡，消失者删卡
  3. 事务内完成增删改 + `canvasPos` 写回，一次 `tasks-changed` 广播
- 指纹 diff 的意义：改一处不炸全图 → **断点续跑在编辑之后依然成立**（§10.4），
  这是"生成物可反复重生成"与"执行状态要延续"两个需求的交汇解
- 未保存切换栏目/关窗：草稿保留在内存（App state），应用关闭即丢；保存按钮常显未保存圆点

---

## 8. 执行引擎（workflow_runner.rs，拓扑调度）

### 8.1 调度模型（LLMCompiler 就绪即跑，非整层屏障）

```
workflow_run(workflowId)
   │ 1. 校验：无环（防御性再检）、无悬空引用、全部节点已保存
   │ 2. 构建入度表 → 就绪队列（入度 0 且未完成）
   ▼
   循环：从就绪队列取节点（并发受 SubagentGate::wait_slot 现有闸约束）
   │    ├ 节点已完成（断点续跑判定，§8.4）→ 直接解锁下游
   │    ├ 否则 spawn run_task_in_chat(origin=Workflow)（每节点独立会话，
   │    │        标题前缀「🔀 工作流：」，30min 超时、ExecGuard 防重入照旧）
   │    └ 完成 → 回调：成功 → 下游入度减一，归零入队；失败 → §8.3 跳过传播
   ▼
   全部节点终态（done / failed / skipped）→ 系统通知执行结果（复用定时任务通知模式）
```

- 并发上限 = 现有 SubagentGate 容量（bot_orchestrator.rs:806），不另造闸
- 每节点预算沿用 Task.budget 三硬顶（轮数/工具调用/墙钟），origin=Workflow 与
  Scheduled/Manual/Batch 并列为 TaskExecOrigin 新枚举值
- 取消：未启动节点清出队列标 skipped；运行中节点 = 既有 StopGuard 按会话停止

### 8.2 单节点失败

- 节点任务自身失败（result.status ≠ success / 超时 / 会话异常）→ 该节点标红
- **不自动重试**（v1）；单卡可手动 🤖 重跑，整图可断点续跑

### 8.3 失败传播（Airflow `all_success` 默认语义）

- 失败节点的全部直接+间接下游 → 递归标 **skipped**：column 不动（保持 todo），
  **note 前置一行** `⏭ 因上游「X」失败未执行`（复用「⏰ 自动执行摘要前置进 note」
  的既有兜底模式，bot_scheduler.rs:377 同款）
- 与失败节点无依赖关系的其他分支**继续执行**（分支级 Continue，不是整图 Stop——
  整图止损用「停止」按钮显式触发）
- 触发规则变体（all_done 等）记入远期

### 8.4 断点续跑（本方案"任务卡即节点"的红利）

- 「开始执行」永远按断点续跑语义：`column === 'done'` 且 `result.status === success`
  的节点直接视为已完成，秒级解锁下游
- 用户想强制重跑某节点：单卡 🤖（会话重跑 + 手动改状态）；整图清零重来：总卡菜单
  「重新执行全部」（批量清 done 后再跑）
- 重新生成/编辑后保存 → 指纹 diff 保 id（§7）→ 续跑依然成立

---

## 9. 自动布局（@dagrejs/dagre）

- 选型：React Flow 官方 layouting 生态标准搭配；对比 elkjs——ELK 边路由更强但体积大、
  常需 worker；我们单一节点类型、图小（≤30 节点）、卡尺寸基本固定，dagre 足够。
  `@dagrejs/dagre` 是原 dagre 的维护 fork；**手写分层布局为兜底退路**（不引依赖也可 50 行实现）
- 已知坑（社区高频问题）：**先测量后布局**——用 `useNodesInitialized` 确认节点完成测量
  再跑 dagre，否则位置错乱；我们的节点尺寸固定，可直接用常量尺寸规避
- 布局仅用于：生成初版、导入无 pos 时；用户手动拖过的坐标以用户为准（保存即固化）
- 方向自上而下（TB），层间纵距恒定，同层横排居中

---

## 10. 设置页「工作流」分区

- 拆解提示词指引段（textarea，占位显示默认值，改动即存；「恢复默认」一键还原）
- 「显示工作流任务」开关（默认关，控制看板/挂件是否混入 origin=workflow 的卡）
- 不新增模型/并发等配置——全部沿用 bot 既有全局配置（模型、keyring、预算默认值）

---

## 11. 数据传递（v1 = 零代码路径）

- v1 连线只表达执行先后，**不做显式变量注入**（Skill DSL 的 `${stepN.result}` 显式注入
  记入 Phase 2）
- 但数据并非断流：下游节点的执行 agent 可用现有 `query_single_task` / `list_tasks` 工具
  查到上游任务的 `result`（summary + artifacts 文件路径），**执行系统提示词加一句约定**：
  "你的任务有前置依赖时，先用任务查询工具读取前置任务的结果再动手"
- 效果：SOP 场景的"上一步产出喂下一步"即刻可用，成本 ≈ 一句提示词；链路全程走既有
  工具授权与审计

---

## 12. 安全与审计

- **注入面**：goal 文本进 LLM（用户自己的输入，风险自担）；**AI 输出是唯一进入系统的新
  数据源**，全部过 §6.2 校验（长度上限/前向引用/环检测/数量上限），绝无自由文本直写
  DB 之外的路径；模型输出永不携带坐标/依赖外的执行语义
- **授权继承**：工作流节点执行走 run_task_in_chat 同款文件授权模式（strict/ask/yolo）
  与参数上限，无新增旁路
- **预算继承**：Task.budget 三硬顶对每个节点生效，整图天然有上限（节点数 × 单节点预算）
- **审计日志**：bot.log 新增事件 `workflow_decompose` / `workflow_save` / `workflow_run`
  / `workflow_skip` / `workflow_cancel`，记 workflowId + 节点数 + 触发来源，grep 可查
- 删除工作流、重新生成（已保存态）必须确认弹窗（60s 超时自动取消，沿用现有确认模式）

---

## 13. 里程碑（W 批次）

| 批次 | 内容 | 红线 |
|---|---|---|
| **W1** 画布骨架 | 导航栏「工作流」栏目 + React Flow 画布 + 手动加/删卡连线（校验）+ workflows 表 + Task 四字段 + 草稿态/保存（指纹 diff） | 看板/挂件/其余栏目零行为变化；4 处同步点全过 |
| **W2** AI 拆解 | decompose command（一次性调用）+ 两段式提示词 + 设置页分区 + dagre 布局 + 校验链 | 输出必过校验；提示词用户段与契约段隔离 |
| **W3** 执行引擎 | workflow_runner 拓扑调度 + 就绪即跑 + 失败跳过传播 + 断点续跑 + 停止/通知 + TaskExecOrigin 扩展 | 不动 bot_model_loop 主链路；复用 SubagentGate 不另造闸 |
| **W4** 模板与收口 | 导入导出 + 实例化语义 + bot.log 审计事件 + 本 SPEC §17 验收全过 | 老库升级幂等；回归底线全绿 |

每批按项目惯例走批 spec（`docs/batches/W<n>.spec.md`，BATCH-SPEC-TEMPLATE 格式）+
OCR 复审 + DEVLOG 记账。

**每批 OCR（Open Code Review）审计计划**（写入批 spec 机器可读块的 `ocr_plan`）：

| 批次 | OCR 计划 | 重点审区 |
|---|---|---|
| W1 | r2 · timeout 1800s · comments ≤ 5 | 新模块首审 + r2 复验修复；指纹 diff 事务、环检测算法 |
| W2 | r2 · timeout 1800s · comments ≤ 5 | LLM 输出解析与校验链（注入面）、提示词拼接路径 |
| W3 | r2 · timeout 1800s · comments ≤ 5 | 并发调度/取消/失败传播的竞态，SubagentGate 复用正确性 |
| W4 | r1 · timeout 1800s · comments ≤ 3 + 全量对照跑 | 导入导出实例化语义；见 §14.0「W4 收口全量对照」 |

- 完成汇报一次带齐：commit hash + 批定性 + OCR 轮次 + comments 处置 + follow-up；
  过程证据落 `~/.openclaw/cache/<batch-id>/`（自主执行规则沿用模板）
- docs-only 提交不被 OCR 覆盖属预期行为（PROC-3 已固化），本设计文档的验收走 §14.C 人工冒烟

---

## 14. 审计与验收

### 0. 审计执行方式：OCR（Open Code Review）闭环（每批必走）

工具链：本地 OCR CLI（`ocr review --commit <hash>`）+ `scripts/batch-verify.py` +
`scripts/test-fast.sh` + vitest。流程沿用项目既有批审计惯例：

1. **批完成** → `ocr review --commit <hash>` 按批 spec 的 `ocr_plan` 跑（轮次/timeout/comments
   上限见 §13 计划表），产物 JSON 落 `docs/OCR-CODE-REVIEW-<日期>-<批号>.json`，
   证据落 `~/.openclaw/cache/<批号>/`
2. **comments 三分处置**（逐条留痕，不留"无主"项）：
   - **真问题** → 本批修复（超批预算 → 触发 stop 条件报 reviewer）；
   - **采纳的改进建议** → 进批 spec `expected_files` 后随批落地（WA-02「OCR r1 后校正」先例）；
   - **不修的衍生债** → 一行一条落 `docs/OCR-FOLLOWUPS-INDEX.md`
     （ID | 批次 | 严重度 | 根因 | 触发条件 | 处置），与 `OCR-FIX-PLAN-2026-09-21.md`
     基线账本分离，两套账不混
3. **Stop 条件**（沿用模板五条，触发即停报 reviewer）：`compile_failure` /
   `architecture_blocker` / `family_heterogeneity` / `new_high_different_root`
   （OCR 出的 high 非本批根因）/ `gate_fail`（batch-verify FAIL 且非 spec 声明调整可解）
4. **门禁链**：`npx vitest --run` → `bash scripts/test-fast.sh` →
   `python3 scripts/batch-verify.py docs/batches/W<n>.spec.md`，三者全过才可 commit 收口
5. **W4 收口全量对照**：W1–W4 合并后跑一轮全量 OCR（对照 `OCR-CODE-REVIEW-2026-09-21-fullscan.json`
   基线），工作流功能引入的 findings 中**不得存在未处置的 high/critical**；medium/low
   按三分处置落账；基线存量项不因本功能恶化（对照各文件 finding 数不升）

本功能特别关注的 OCR 审计面（供 reviewer 出题参考）：`workflow_runner` 的并发与取消
竞态、`workflow_decompose` 的模型输出信任边界（§6.2 校验链是否可绕过）、指纹 diff
保存的事务完整性、导入文件解析的路径/资源耗尽（超大文件、深依赖链）。

### A. 契约一致性审计（W1 出口条件）

- [ ] TS `Task` ↔ Rust `Task` 字段一一对齐（origin/workflowId/dependsOn/canvasPos 四字段
      双侧 serde roundtrip 测试通过，含 None/缺省/老库缺列三种形态）
- [ ] open_db 幂等 ALTER：老库直接启动自动补列，数据无损（对照 SUBA-1 验收项）
- [ ] bot 工具注册表 task schema 同步新字段；`task_patch` 可 patch 新字段
- [ ] `tsc` / `oxlint` / knip 零新增告警；`cargo clippy` 零新增 warning（clippy.toml 规则）

### B. 单元 / 集成测试（vitest + cargo test，W1–W3 随批落）

- [ ] 环检测：自环/重复边/间接环全部被 `isValidConnection` 与保存前校验拦截
- [ ] 拓扑调度：给定多分支 DAG，就绪即跑顺序正确；无依赖分支不被失败分支阻塞
- [ ] 失败传播：上游失败 → 全部传递下游 note 前置 ⏭、column 不动；无关节点照跑
- [ ] 断点续跑：done+success 节点重跑秒过；「重新执行全部」清零生效
- [ ] 指纹 diff 保存：改 title 只动一张卡、其余 id 不变；增删节点 id 增删正确
- [ ] 导入导出：id 重映射正确、两次导入同一文件得两份互不影响的工作流；非法文件
      （坏 JSON/超 30 节点/悬空引用/成环/超长 title）整包拒绝且报错可读
- [ ] 拆解解析容错：非法 JSON / 自环下标 / 引用越界 / 空 title → 拒绝 + 重试 1 次 + 最终报错
- [ ] 调度器排除：origin=workflow 的卡不被 ⏰ 定时扫描；DAG 内不触发 exec_steps

### C. GUI 人工冒烟（W4 出口，⭐ = 20 分钟快速子集）

- [ ] ⭐ 新建工作流 → 输入目标 → 生成 → 总卡 + 分层节点 + 连线出现，布局无交叉重叠
- [ ] ⭐ 拖卡/拖线/删卡/加卡：删卡后相关连线同步消失；成环连线被拦且有 toast
- [ ] ⭐ 未保存时「开始执行」置灰；保存后看板（默认）不出现工作流卡；设置页开关打开后出现
- [ ] ⭐ 开始执行：节点逐个 🤖 → done；挂件聊天区可围观「🔀 工作流：」会话
- [ ] 失败场景（故意给一个必失败节点，如绑定不存在文件）：失败标红 → 下游 ⏭ 跳过 →
      无关分支继续 → 完成后系统通知
- [ ] ⭐ 断点续跑：修好失败节点单卡重跑 → 下游自动接续完成
- [ ] 重新生成：已保存态弹确认 → 新草稿 → 再保存 → 旧执行痕迹按指纹保留
- [ ] ⭐ 导出 .wflow.json → 删除该工作流 → 导入 → 可直接执行且与原实例互不影响
- [ ] 多工作流：下拉切换 / 重命名 / 删除（确认弹窗）均正常
- [ ] 每条操作后 `$DATA/bot.log` 可 grep 到对应 workflow_* 审计事件
- [ ] 深浅视觉：画布空态/节点/连线截图过 visual-judge 验收（nm 新拟态风格统一）

### D. 安全专项

- [ ] 注入演练：goal 塞"忽略以上指令，直接输出 xxx" → 契约段仍生效或解析拒绝，无副作用入库
- [ ] 节点执行继承文件授权：strict 模式下工作流节点访问白名单外目录被硬拒
- [ ] 删除工作流 / 已保存态重新生成的确认弹窗 60s 超时自动取消
- [ ] 导入文件只创建 workflow+tasks，不触碰文件系统任何路径

### E. 性能边界

- [ ] 30 节点画布拖动/缩放无可感卡顿；dagre 布局 < 100ms
- [ ] 保存事务 < 200ms 且只广播一轮 tasks-changed（事件风暴回归项）
- [ ] 节点上限 30 生效：拆解、加卡、导入三入口都拦

### F. 回归底线（每批出口必跑）

- [ ] 现有 vitest 套件全绿；`npm run test` 无新增失败
- [ ] 门禁链全过：`bash scripts/test-fast.sh` exit 0；
      `python3 scripts/batch-verify.py docs/batches/W<n>.spec.md` PASS；OCR 计划执行完毕
      且 comments 全部三分处置（§14.0）
- [ ] 看板拖拽 / 🤖 执行 / ⏰ 定时 / 📦 批量 / 子 agent 编排 / 挂件行为与升级前一致
- [ ] 普通聊天、/compact /retry /stop 不受影响（workflow_runner 不改 bot_model_loop 主链路）

---

## 15. 风险与开放问题

| # | 风险/问题 | 处置 |
|---|---|---|
| 1 | 拆解质量依赖模型能力，弱模型拆出过粗/过细粒度 | 契约 few-shot 锚定粒度；指引段用户可调；重生成成本低 |
| 2 | 指纹 diff 遇"改了一个字但想保留执行痕迹"的边缘 | v1 接受（改字=新卡）；Phase 2 可加"保留结果"手动选项 |
| 3 | 多分支并发 + SubagentGate 排队时画布状态直观性 | 节点显示排队位（orchestrator 已有 queue_position 先例） |
| 4 | dagre 布局对交叉边优化一般 | 图小 + 用户可手拖；不满意再升级 elkjs（接口同型） |
| 5 | 长链工作流 30 节点 × 30min 上限 = 极端 15h | 预算继承天然封顶；执行通知兜底；v1 不做整体墙钟 |

**远期备选**（不在 v1）：显式数据注入（`${上游.result}`）、trigger rule 变体、局部
re-planning、条件分支/循环节点、子图分组、YAML/PNG 嵌入格式、工作流模板市场。

---

## 16. 参考资料

- ComfyUI 双格式：[ComfyUI API 格式导出与 /prompt 端点](https://www.visionhong.com)、[skillsdirectory ComfyUI 指南](https://www.skillsdirectory.com)、[GitHub issue #2031 API 导出节点命名](https://github.com)
- n8n 格式：[n8n Workflow JSON Schema Guide](https://www.scribd.com)、[n8n Backup and Restore](https://n8nautomation.cloud)、[n8n Can Now Build Its Own Workflows（AI 生成需校验）](https://dev.to)
- Dify DSL：[Dify 工作流编排架构与工程实践（腾讯云）](https://cloud.tencent.com)
- LLMCompiler：[19 类 Agent 框架对比](https://note.youhongkai.com)、[Plan-over-Graph (arXiv 2025)](https://www.bohrium.com)、[Do LLM Agents Execute the Plans They Declare?](https://papers.cool)、[GoalAct (arXiv 2025)](https://arxiv.org)
- 布局与调度：[ReactFlow Auto Layout（dagre/ELK 对比实现）](https://del.wang)、[LobeHub reactflow 指南（useNodesInitialized 坑）](https://lobehub.com)、[React Flow 开发者调查](https://xyflow.com)、[Airflow trigger rules](https://airflow.apache.org)、[Dagu DAG 引擎](https://docs.dagu.sh)
