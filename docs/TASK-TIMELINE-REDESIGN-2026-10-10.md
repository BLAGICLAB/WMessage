# 任务页改造方案：周时间网格

状态：**方向已拍板**（2026-10-10：坐标轴采用日历惯例——星期=横列、时刻=纵行；视觉按「高级感/现代感」升档）。剩余小项见文末确认清单。拍板后按「分批提交」小步落地，本文档随实现同步修订。

## 1. 目标

取消主窗口任务页的「待办 / 今日 / 完成」三列排列，改为**单人周时间网格**：

- 横轴 = 周一到周日（7 列，与 Google/Apple 日历等主流惯例一致）；纵轴 = 时刻 8:00–18:00（10 个小时行）；
- 未完成任务以统一大小的标题色块收进右侧**任务池**（毛玻璃、悬浮在网格层上、可右缘折叠）；
- 色块拖到网格即排期：自动写入 `planStart` / `planEnd`（默认 60 分钟）；
- 计划块可跨日（时长溢出 18:00 折入次日列顶继续渲染）；
- 新建任务入口在页面左上，点开后左侧滑出**任务卡详情**（毛玻璃，与任务池同设计），内含计划时间编辑。

> 轴向调研结论（2026-10-10）：个人日历品类（Google Calendar、Outlook、Apple Calendar、TickTick、Mobiscroll Time Grid）一律「星期=列、时刻=行」；原方案的「时刻=横轴」属于资源排期品类（Float、Resource Guru、Ganttic、Mobiscroll Timeline）的取向。按日历惯例翻转，换来默认 60 分钟块的标题可读性（日列 ~160px vs 小时列 ~105px）、与全人类日历一致的肌肉记忆、以及未来展示 8–18 之外时段的纵向伸展余地。

## 2. 范围

改：主窗口 `board` 视图（`KanbanBoard` 三列 → `TaskTimelinePage`）、任务数据模型（新增两个可选字段）、设计 token（新增 accent、玻璃、计划块色板三组）。

不改：

- `column` 数据契约（todo/doing/done）。挂件、归档、回收站、图谱、统计、bot 全部照旧读 column；本改造只换 board 视图的呈现，「完成」走详情面板按钮（复用 `set_task_column` 既有语义：写 completedAt、清 botAssigned 等）。
- 挂件（WidgetApp 用自己的 SortableTaskCard，不经过 KanbanBoard）。
- bot 工具 schema（v1 不暴露计划字段，见确认清单 7）。

## 3. 布局与线框

```text
┌ 导航栏 ┬────────────────────────── 任务页 ──────────────────────────────┐
│        │ ＋新建任务   ‹  10月6日 – 10月12日 ›  今天          [任务池 ⸬]   │
│ 新建   ├────────────────────────────────────────────────────────────────┤
│ 搜索   │      周一 6   周二 7  ◐周三 8  周四 9   周五 10  周六 11  周日 12│
│ …      │ 8:00  ┌───────┬───────┬───────┬───────┬───────┬─────┬───────┐ │
│        │ 9:00  │ ▓▓▓▓▓ │       │       │ ▓▓▓   │       │ ░░░ │       │ │
│        │ 10:00 │       │       │ ▓▓▓▓▓ │       │       │ ░░░ │       │ │
│ (详情  │ …     │       │ ▓▓    │       │       │ ▓▓▓▓▓ │     │       │ │
│  面板  │ 17:00 │       │       │       │       │ ▓▓ »  │ « ░ │       │ │
│  打开  │       └───────┴───────┴───────┴───────┴───────┴─────┴───────┘ │
│  时为  │                                               ┌────────────┐  │
│  左列) │                                               │ 任务池(毛玻璃)│  │
│        │                                               │ ▪ 写周报     │  │
│        │                                               │ ▪ 采购       │  │
│        │                                               └────────────┘  │
└────────┴────────────────────────────────────────────────────────────────┘
```

- 详情面板为**文档流左列**（挤压网格宽度，不遮挡时刻轴）；任务池为**悬浮层**（叠在网格右侧上方）。两者共用同一毛玻璃质感。
- 任务池折叠后缩为右缘竖条把手（图标 + 未完成计数角标），点击展开。
- 新建任务按钮在页头左上（全局 ⌘N 行为不变：切到任务页 + 打开详情面板新建卡）。
- `»«` 为跨日续接标记：周五 17:30 + 60min → 周五列底部一段 + 周六列顶部一段。

## 4. 交互规格

### 4.1 网格

- 列头：`周一 10/6` 式两段标签（周几小字 + 日期大数字），**周一为一周首日**；今天列：日期数字套 accent 实心圆、列底 accent 5% tint；周六/周日在亮暗主题下都降一档存在感（底色压暗 2%、标签用弱文字级）。
- 行头：`8:00`–`18:00` 共 11 档刻度（10 个小时行），tabular-nums；小时实线、半点只用更淡的虚线提示吸附位。
- 周导航：`‹ ›` 翻周，`今天` 快捷回当前周；非当前周不画「现在时刻线」。
- 现在时刻线：当前周且时刻落在 8–18 内时，在今天列画 2px accent 横线贯穿该列（左端发光圆点）；打开页面与拖拽结束时校准，不做秒级定时器。
- 计划块：绝对定位于所在日列内，`top = (start−8:00)/10h`，`height = duration/10h`；圆角 8px，块内显示标题（≤2 行截断）+ 起止时间小字（高度 ≥48px 时显示）。
- **跨日折行渲染**：`planEnd` 越过 18:00 时，块拆为多段——当日贴列底段（下角开口）+ 次日自列顶起段（上角开口），两段同色并在衔接缘画 `»` / `«` 续接标记；跨到下周的部分在翻周后可见，本周列底画溢出指示。起于 8:00 前的既有数据（导入/编辑产生）在列顶贴边截断 + `«` 指示。
- 点击块 = 选中（accent 环）并打开左侧详情面板。

### 4.2 任务池

- 列出**全部未完成**的本人的用户任务（`column != done`、未删除、未归档、`origin=user`、本人——与现有 visibleTasks 过滤同口径，仅去掉 done）。
- 块统一大小（高 32px、宽撑满池内宽）：左侧 3px 色条 + 标题单行截断。已排期的任务也在池中，右端加时间角标（`二 9:00`）。
- 排序：已排期按 planStart 升序在前，未排期按 updatedAt 降序在后。
- 拖拽方向是双向的：池 → 网格写入计划；网格 → 池清除计划（`planStart`/`planEnd` 置 null）。
- 折叠状态存 localStorage（键 `wm-task-pool-open`），挂件面板折叠先例同思路。

### 4.3 拖拽（一个 DndContext 覆盖三个来源）

| 手势 | 结果 |
|---|---|
| 池块 → 网格某位置 | `planStart = snap30(落点日+时刻)`，`planEnd = planStart + 60min` |
| 网格块拖动 | 保持时长，`planStart` 跟随落点（30 分钟吸附，可跨列穿日） |
| 网格块底缘手柄纵向拉伸 | `planEnd = snap30(指针时刻)`（可越过 18:00 折入次日；最短 30 分钟） |
| 网格块 → 池 | 清除 `planStart` / `planEnd` |

- 落点解析用指针相对网格原点的偏移（dnd-kit `activatorEvent` + 网格 rect 反解），落出网格边界时钳制到最近合法格。
- 拖拽过程显示占位虚影（DragOverlay 渲染同款块），目标格 accent 8% 高亮。
- `planEnd > planStart` 恒成立（拖动/拉伸路径钳制），存前校验，非法不写。

### 4.4 任务卡详情面板（左）

- 打开时机：点网格块 / 点池块 / 新建任务。关闭：X、Esc。
- 内容复用 `TaskCardContent` 的编辑字段（标题、备注、标签、文件、子任务、due），新增**计划时间**区块：
  - 开始：`datetime-local` 输入（`YYYY-MM-DDTHH:mm`，与 due 同格式）；
  - 时长：30 分钟步进器（`− 60min +`），快捷 30/60/90/120；
  - 「清除计划」按钮；planStart 与 planEnd 联动校验同 4.3。
- 底部操作：`标记完成`（set_task_column done）、`删除`。完成语义完全走既有服务端命令。
- 视觉与任务池同一毛玻璃面板规格（同圆角、同边线、同模糊），仅结构不同。

## 5. 视觉体系（高级感升档）

基调一句话：**安静的高级来自层次而不是装饰**——网格下沉成一口「仪表井」，计划块是井里唯一的色彩，任务池和详情面板是两块悬浮毛玻璃，accent 只出现在「现在」和「选中」上。参照 Linear 的三条纪律适配本仓库 token：亮度分层表达层级（已有）、边框走半透明、唯一彩色只给激活态。

### 5.1 新增 token（main.css，亮暗各一套）

```css
/* 强调色：仅现在时刻线 / 今天标识 / 选中环 / 新建主按钮四处使用。
   取 --brand 同族但饱和一档的靛蓝紫，暗色提亮保对比 */
:root  { --accent: #5b64d8; }
.dark  { --accent: #8a91f0; }

/* 毛玻璃面板（任务池 / 详情面板共用） */
:root  { --glass-bg: rgba(255, 255, 255, 0.66); }
.dark  { --glass-bg: rgba(27, 31, 41, 0.60); }
.glass {
  background: var(--glass-bg);
  backdrop-filter: blur(20px) saturate(1.5);
  border: 1px solid var(--edge);
  border-radius: var(--r-lg);           /* 16px */
  box-shadow: var(--shadow-lg);
}
@supports not (backdrop-filter: blur(1px)) {
  .glass { background: var(--surface); }  /* 退化不透明，保对比 */
}

/* 计划块色板（低饱和六色；标签哈希取色，无标签用 slate） */
--plan-blue: #5b7fe8;  --plan-teal: #2fa8a0; --plan-green: #4caf7d;
--plan-amber: #d9a13f; --plan-violet: #9a6fe0; --plan-slate: #7a86a0;
```

### 5.2 网格材质

- 网格整体沉在一口大圆角「井」里：`nm-inset` 材质（`--inset-bg` 叠底 + 1px 边线 + 16px 圆角），与悬浮的毛玻璃面板形成「下陷 ↔ 上浮」的层次对话。
- 时刻结构线：小时线 `color-mix(var(--edge) 55%, transparent)` 1px 实线；半点线 30% 透明虚线（仅提示吸附位，弱到不抢内容）。
- 今天列：`color-mix(var(--accent) 5%, transparent)` 整列铺底 + 列头日期套 accent 实心圆（反白数字）。
- 周末列：底色压暗一档（`--inset-bg` 40%）、列头标签降为 `--t5`——工作日是主舞台。
- 现在时刻线：2px accent 横线 + 左端 6px 圆点带 8px 半透明光晕（`box-shadow: 0 0 0 4px color-mix(var(--accent) 20%, transparent)`）——整页唯一会「发光」的元素。
- 时刻标签：10.5px tabular-nums `--t5`；列头「周几」11px `--t5`、「日期」20px/600 tabular，今天反白。

### 5.3 计划块解剖

- 底色 `color-mix(色板色 14%, var(--surface-raised))`，1px 边框 `color-mix(色板色 26%, transparent)`，左侧 3px 实色竖条，圆角 8px。
- 标题 12px/500 `--t1` 最多两行；高度 ≥48px 时右下角加起止时间 10px tabular `--t4`。
- 状态：hover 底色提到 18% 并上浮 1px（`translateY(-1px)` + `--shadow-sm`）；选中换 accent 1.2px 边 + `--focus-ring` 同款光环；拖拽幽灵 = 同款块 `scale(1.02)` + `--shadow-lg` + 90% 不透明度。
- 同列重叠（用户手动排重不做禁止）：按重叠数横向分栏（Google Calendar 同款），每栏内边距 2px。

### 5.4 动效纪律

- 只给动作反馈，不给氛围动画：面板滑入 200ms `cubic-bezier(0.16, 1, 0.3, 1)`（expo-out，高级感的来源是收尾不弹跳）、池折叠 200ms、块 hover 120ms、落格高亮 120ms 淡出。
- 拖拽幽灵 1:1 跟指针（无惯性延迟）；现在时刻线不呼吸不闪烁。
- `prefers-reduced-motion` 已有全局降级，新动效自动覆盖。

### 5.5 字重与数字

- 沿用系统字体栈（macOS PingFang SC / Windows 雅黑），不引网络字体：层级靠字重与灰阶（600 宣告 / 500 强调 / 400 阅读），数字一律 `tabular-nums`（时刻轴、日期、时长、计数）。
- 页头周区间「10月6日 – 10月12日」18px/600 是页面最大字号，其余全部 ≤15px——尺寸的克制就是高级感。

## 6. 数据模型与后端

### 6.1 字段

`Task` 新增两个可选字段（`src/types.ts` + `src-tauri/src/db/tasks.rs` 同步，serde camelCase）：

```ts
/** 计划开始 "YYYY-MM-DDTHH:mm"（与 due 同格式）；缺省 = 未排期 */
planStart?: string;
/** 计划结束，恒 > planStart；缺省 = 未排期 */
planEnd?: string;
```

### 6.2 Rust 侧改动（tasks.rs）

1. `Task` 结构体加 `plan_start: Option<String>` / `plan_end: Option<String>`（`#[serde(default)]`，老数据/老导入信封零改动兼容）。
2. 新列常量（沿 OWNER_/ACCEPTANCE_ 先例各自独立成组）：

   ```rust
   pub(crate) const PLAN_TASK_COLUMNS: [(&str, &str); 2] =
       [("plan_start", "TEXT"), ("plan_end", "TEXT")];
   ```

   注册进 open_db 幂等 ALTER 迁移；legacy 迁移测试 fixture 同步。
3. `task_patch` 白名单加 `planStart` / `planEnd`：字符串校验 `YYYY-MM-DDTHH:mm` 格式（同 due 的宽松校验路径），null = 清空落库，缺键 = 保留——语义与现有可选字段一致。
4. 导出/导入信封：serde 自动带出新字段，无需额外处理。

### 6.3 前端接线

- `updateTask`（task_patch 通道）原样携带新键；拖拽落点只发 `{planStart, planEnd}` 两键的最小 patch。
- RMW `expectedUpdatedAt` 防覆盖机制不变。

## 7. 组件拆分

新目录 `src/components/TaskTimeline/`：

| 文件 | 职责 |
|---|---|
| `TaskTimelinePage.tsx` | 页面骨架：页头（新建/周导航/池开关）、左详情、网格、池，装配 DndContext |
| `WeekGrid.tsx` | 网格渲染 + 计划块定位 + 现在时刻线 + 重叠分栏；落点↔时刻换算 |
| `PlanBlock.tsx` | 计划块（含跨日分段渲染、底缘拉伸手柄、选中态、重叠分栏） |
| `TaskPool.tsx` | 毛玻璃任务池 + 折叠把手 + 计数角标 |
| `TaskDetailPanel.tsx` | 左侧毛玻璃详情（内嵌 TaskCardContent + 计划时间区块 + 完成按钮） |
| `week.ts` | 纯函数：周一起始、snap30、addMinutes、跨日分段切分、越界钳制、时间↔坐标换算 |

退役：`KanbanBoard.tsx` + `KanbanBoard.test.tsx`（含 `spliceMove` 导出）在收尾批随同删除（硬性禁令：孤儿化同提交删除）。`App.tsx` board 分支换 `TaskTimelinePage`，`addTask`/`editingId`/⌘N 流程改指详情面板。

视觉小样：`docs/design-mock/task-timeline-mock.html`（纯静态单文件，暗/亮切换、池折叠、详情开关可交互），实现期作为像素级对照。

## 8. 测试计划

- Rust（tasks.rs 测试模块，沿 task_set_column_tests 先例）：patch 白名单设值/清空/非法格式拒写；迁移建列幂等；序列化 roundtrip；老信封缺字段反序列化兼容。
- `week.ts` 纯函数：周一起始、snap30、跨日分段（含跨周）、钳制、分钟↔px 换算往返一致。
- 组件：页头周导航与今天回跳；池过滤口径与排序、折叠记忆；拖拽落点 → 最小 patch 内容（沿 KanbanBoard.test.tsx 已有 dnd 测试写法）；详情面板计划时间编辑 → patch、完成按钮走 set_task_column；⌘N → 新卡编辑态面板。
- 行为变更测试随批次更新（App.test.tsx 里「待办/今日/完成」列头断言在退役批改写）。

## 9. 分批提交（每批过 test-fast）

1. **数据层**：types.ts + Rust struct/迁移列/patch 白名单 + Rust 测试（无 UI 变化，wire 全绿）。
2. **视觉 token + 网格只读**：main.css 三组 token、week.ts、WeekGrid + PlanBlock 渲染已排期数据（暂无写入路径）。
3. **拖拽写回 + 任务池**：DndContext、池/折叠、池→网格、网格移动/拉伸、拖回清除。
4. **详情面板**：左侧面板 + 计划时间编辑 + 完成入口；⌘N/新建接线。
5. **退役三列看板**：删 KanbanBoard*、App.test 更新、SPEC.md 与本文档同步。

批 2 起每批可运行查看，中途叫停不留半接线状态。

## 10. 已拍板与待确认

已拍板（2026-10-10）：

1. 坐标轴采用日历惯例：星期=横列、时刻=纵行（调研结论见 §1 引文）。
2. 「跨日穿透」= 时长溢出 18:00 自动折入次日列顶渲染（分段 + 续接标记）。
3. 吸附粒度 30 分钟，默认时长 60 分钟。
4. 周一为一周首日。
5. 「今日/doing」失去专属视觉（数据契约不动，混在池和网格里）。
6. 计划块新增六色低饱和色板，按首个标签取色、无标签中性色。
7. bot 工具 schema v1 不动（人工排期先行）。

待确认（不阻塞批 1–2，批 3 前给答复即可）：

- A. accent 靛蓝紫是否接受（替代品牌色承担「现在/选中/新建」三处激活语义；品牌色 --brand 保持原用途不变）。
- B. 周末列压暗 + 今天列 accent tint 的存在感分级是否接受（小样里可看效果）。
