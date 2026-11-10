# 定时模块 · 提醒分组与提醒管理（落地方案）

> 2026-11-10 · 设计稿：`docs/design-mock/schedule-reminder-grouping.html`
> 状态：方案待拍板，未实施

## 0. 结论

- **分组形态：顶部标签切换**（`定时任务` / `提醒`，各带计数）。不新建导航项、不做独立页。
- **不用分页**：本仓 pagination 的既有语义是「换时间段」（`PoolPager` 的 8 页 = 本周 7 天 + 未排期池），作业与提醒之间没有时间关系。
- **范围**：把提醒接进已有的 `SchedulePage`，补上提醒的**暂停 / 改 / 删 / 建**四个前端入口。
  后端三个洞：① `reminders.enabled` 无写路径（周期提醒今天无法暂停）；② 提醒无前端列表入口；③ 提醒触发后前端无感知（无事件广播）。

## 1. 现状

### 1.1 两条链路（同一 30s tick 上并列）

| 维度 | 定时作业（job） | 提醒（reminder） |
|---|---|---|
| 语义 | 到点**做事**：建任务卡交机器人执行 | 到点**说话**：发一条系统通知 |
| 存储 | `scheduled_jobs` 表 | `reminders` 表（`db/reminders.rs`） |
| 定时串 | `daily:` / `weekly:D:` / `monthly:DD:` / `at:` | **同一套格式**（`bot_reminders::build_schedule`） |
| 触发 | `bot_scheduler::scheduler_tick` → `jobs_tick` | 同一 tick → `reminders_tick`（`bot_scheduler.rs:1123`） |
| 投递 | 建卡 → `run_task_in_chat` | Effects 队列 `KIND_NOTIFY` → 系统通知 |
| 前端入口 | 工具 + 4 个 Tauri 命令 + `SchedulePage` | **仅 3 个聊天工具**（`schedule_reminder` / `list_reminders` / `cancel_reminder`） |
| 生命周期语义 | 重试 / 自动暂停 / 执行档 / 执行历史 / missed | 无（提醒不做事，没有失败与历史） |

### 1.2 三个待补的洞

1. **`reminders.enabled` 是半死字段**：列存在、`load_due_reminders` 按 `enabled IS NOT 0` 过滤（`db/reminders.rs:106`）、`list_reminders` 渲染「已暂停」字样（`bot_reminders.rs:254`）——**但没有任何写路径能置 false**（插入恒为 `enabled: None`，`bot_reminders.rs:191`）。周期提醒今天无法暂停，只能取消重建。
2. **提醒无前端列表**：用户在 UI 里看不到自己设了哪些提醒，也改不了。
3. **提醒触发后前端无感知**：`reminders_tick` 只写审计 + 投递通知，**不发任何前端事件**；`SchedulePage` 只在挂载 / `tasks-changed` / `sched-status` 时刷新 → 一次性提醒触发后行不会消失、周期提醒的「已触发次数」不更新。

## 2. 设计决策

| 候选 | 结论 | 理由 |
|---|---|---|
| **顶部标签切换** | ✅ 采用 | 两类互斥、各一套动作、各自规模小（个位数到十几条）；一次点击切视角 |
| 分区列表（同屏两段） | ❌ | 下半段像「附录」；同屏出现两套按钮语义 = 噪音 |
| 混排一行一徽标 | ❌ | 行模板要在两套动作间分叉；顶部摘要失去单一含义 |
| 分页 | ❌ | 与本仓 pagination 语义（换时间段）冲突；量级用不上 |

**为什么标签上要带计数**：提醒不在当前屏时用户会忘。计数是唯一代价的兜底。
**为什么默认停在「定时任务」**：作业会自己做事，失败 / 错过需要人看；提醒只是响一声。默认 tab 不持久化（持久化会让用户每次进来都停在提醒 tab，作业的失败被漏看）。

## 3. 数据契约

### 3.1 后端：`ScheduleEntry` 改判别联合

现状是平铺结构 + `kind: "job"` 字面量，作业专属字段（`lastStatus` / `retryMax` / `mode` …）对提醒无意义。改成内部标签枚举：

```rust
#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ScheduleEntry {
    Job(JobEntry),
    Reminder(ReminderEntry),
}
```

`JobEntry` / `ReminderEntry` 各自持有自己的字段集，公共字段（`targetId` / `title` / `schedule` / `schedLast` / `nextRunAt` / `enabled`）提到共同基类。

> 序列化形状不变（仍是平铺的 `{"kind": "...", ...}`），前端现有 `e.kind === "job"` 的写法继续可用。

### 3.2 提醒条目字段映射

| 前端字段 | 来源 | 说明 |
|---|---|---|
| `kind` | 常量 `"reminder"` | |
| `targetId` | `reminders.id` | |
| `title` | `reminders.text` | 提醒内容（≤200 字） |
| `schedule` | `reminders.schedule` | 与作业同格式 → `formatSchedule` 直接可用 |
| `schedLast` | `reminders.fired_at` | **复用为「上次触发」**（作业那边是「上次执行」，语义同位） |
| `nextRunAt` | `reminders.remind_at` | **取持久化值，不现算**（见 §10 优化 3） |
| `enabled` | `reminders.enabled()` | |
| `firedCount` | `reminders.fired_count` | 新出口——这列早就落库了，只是没有地方显示 |
| `oneShot` | `reminders.one_shot()` | **后端算**（`schedule.starts_with("at:")`），前端不解析（见 §10 优化 2） |
| `missed` | 恒 `false` | 提醒不补跑，语义与作业的「已错过」不同，不复用该徽标 |

### 3.3 前端：`src/types.ts` 判别联合

```ts
interface ScheduleEntryBase {
  targetId: string; title: string; schedule: string;
  schedLast: number | null; nextRunAt: number | null;
  missed: boolean; enabled: boolean;
}
export interface JobEntry extends ScheduleEntryBase {
  kind: "job"; detail: string | null;
  lastStatus?: string | null; lastError?: string | null;
  retryMax: number; pauseOnFailure: boolean; mode: string | null;
}
export interface ReminderEntry extends ScheduleEntryBase {
  kind: "reminder"; firedCount: number | null; oneShot: boolean;
}
export type ScheduleEntry = JobEntry | ReminderEntry;
```

收益：TS 按 `e.kind` 自动收窄，**在提醒行上读 `retryMax` 直接编译报错**——不靠人记得判别。

## 4. 后端改动清单

### 4.1 `src-tauri/src/db/reminders.rs`

| 改动 | 说明 |
|---|---|
| `Reminder` 加 `expected_updated_at: Option<i64>` | `#[serde(default, skip_serializing)]`，与 `ScheduledJob` 同构（`db/schedule_jobs.rs:114`） |
| 新增 `upsert_reminder(conn, &Reminder)` | `INSERT … ON CONFLICT DO UPDATE`，带 RMW 基线检查（复用 `super::tasks::rmw_conflict` + `CONFLICT_ERR_PREFIX`） |
| 新增 `set_reminder_enabled(conn, id, enabled) -> bool` | **定点 UPDATE**（只动 `enabled` + `updated_at`），不携带整行快照、不撞 RMW（见 §10 优化 9）；返回是否命中行 |
| 新增 `update_reminder(conn, &Reminder)` | 改 `text` / `schedule` / `remind_at`，**带 RMW 基线**（编辑是真编辑，要防并发覆盖） |

> 现有文件头注释写着「不做 RMW 基线：提醒只由机器人工具建/取消，没有并发编辑入口」——**本次加 UI 编辑入口，这句注释的前提就不成立了**，需同步改写。

### 4.2 `src-tauri/src/bot_reminders.rs`

| 改动 | 说明 |
|---|---|
| 新增 `pub fn next_fire_at(schedule, now) -> Option<i64>` | 薄封装 `bot_scheduler::occurrence_after`（`bot_scheduler.rs:156`），**四种档位统一用它**（含 `at:` 未来时刻；`at:` 已过返回 None → 写入口拒绝） |
| `reminders_tick` 触发 / 删除后广播事件 | `app.emit("reminders-changed", …)`，见 §5.4 |
| 新增 `tool_*` 旁路：UI 建提醒的落库逻辑复用 `validate_reminder_text` | 内容校验（≤200 字）单一来源 |

**调度器本身零改动**：暂停靠 `load_due_reminders` 的 `enabled IS NOT 0` 过滤自动生效。

### 4.3 新增 Tauri 命令（4 个）

| 命令 | 签名 | 说明 |
|---|---|---|
| `reminder_create` | `(app, text, schedule) -> Reminder` | 校验内容 + `validate_schedule`；`remind_at = next_fire_at(schedule, now)`，为 None（`at:` 已过）则 `InvalidArgument` |
| `reminder_update` | `(app, id, text?, schedule?) -> Reminder` | 改内容 / 时间；**改 schedule 必须重算 `remind_at`**（见 §10 优化 13）；带 RMW 基线 |
| `reminder_delete` | `(app, id) -> bool` | 取消提醒（删除行）；行不存在返回 false → 前端提示「提醒已不存在（可能刚刚触发）」 |
| `reminder_set_enabled` | `(app, id, enabled) -> bool` | 暂停 / 恢复；定点 UPDATE |

> **不复用 `schedule_set_enabled` 的 kind 分支**：该命令现有实现是「load + 改 + upsert + RMW」，与提醒的定点 UPDATE 语义不同，塞进同一个 kind 分支会让两条路径的分叉藏在函数体内。拆成独立命令，命令名自解释。
> 命名不与机器人工具冲突：工具走 registry，命令走 `invoke_handler`，两套命名空间（`audit_bot_namespace.py` 只查模块路径，不查命令名）。
> 错误码**不新增**：`InvalidArgument` / `TaskNotFound` 够用 → `error.rs` 与 `errorHandler.ts` 不用动。

### 4.4 `schedule_overview` 追加提醒条目

在现有作业循环之后追加 `load_reminders` 循环，产出 `ScheduleEntry::Reminder`。
`nextRunAt` 取 `remind_at`（持久化值），**不**用 `occurrence_after` 现算。

### 4.5 审计事件

新增 4 个事件名（跟随提醒族既有的点号风格 `reminder.set` / `reminder.cancel`）：

| 事件 | 时机 |
|---|---|
| `reminder.create` | UI 新建提醒 |
| `reminder.update` | UI 修改提醒 |
| `reminder.delete` | UI 取消提醒 |
| `reminder.set_enabled` | UI 暂停 / 恢复 |

同步进 `docs/acceptance/event-names.md`（该文档目前无门禁读取，但它是事件名总表，手工维护）。

## 5. 前端改动清单

### 5.1 `SchedulePage.tsx`：标签行

- 新增 `tab: "jobs" | "reminders"`，**默认 `"jobs"`**，不持久化。
- 标签行照 `NotificationsPage.tsx:406-432` 的 `role=tablist` 配方（选中态 `nm-inset`），两个标签各带计数徽标。
- 计数口径 = 该类总行数（**含暂停**）——暂停了也要看得见，否则用户以为没设过。
- **摘要条按当前 tab 取**（见 §10 优化 1）：
  - 作业 tab：`最近将执行：X，N 小时后`
  - 提醒 tab：`最近会提醒：X，N 分钟后`
- 新建按钮文案随 tab：`新建定时` / `新建提醒`。
- 空态随 tab：
  - 作业：`暂无定时任务` / `写下要做什么，到点自动新建任务卡执行。`
  - 提醒：`还没有提醒` / `设一个到点响一声的事——到点发系统通知，不建任务卡、不执行任何内容。`

### 5.2 `src/components/SchedulePage/ReminderRow.tsx`（新增）

提醒行模板，**独立文件**（`SchedulePage.tsx` 已 680 行，别让它继续膨胀到 900 行）：

- 徽标「提醒」（`kind.rem` 样式）+ 标题 + `已暂停` 徽标。
- meta 行：`formatSchedule(schedule)` / `untilTime(nextRunAt)` / `已触发 N 次`（`firedCount > 0` 且非一次性时才显示）。
- 动作：**暂停 或 恢复** / **修改** / **取消**（两步确认，沿用现有 `cancelArmed` 范式）。
- **一次性提醒（`oneShot`）不显示「暂停」**——它触发即删，没有下次可暂停。
- **不出现**：立即执行 / 预演 / 执行历史 / 痕迹 / 会话跳转。

### 5.3 `src/components/SchedulePage/ScheduleEntryEditor.tsx`（由 `JobEditor` 迁改）

一个组件两种形态（`variant: "job" | "reminder"`），避免复制第二份编辑器：

| 部位 | job | reminder |
|---|---|---|
| placeholder | `要做什么？（500 字以内，到点自动新建任务卡交给机器人）` | `提醒我什么？（200 字以内，到点发一条系统通知）` |
| 失败重试 / 失败自动暂停 两个旋钮 | 显示 | **不显示**（提醒不会失败） |
| preview | `每天 09:00，到点自动新建任务卡并交给机器人执行` | `每天 11:00，到点发一条系统通知提醒你（只说话，不做事）` |
| 提交参数 | `content, schedule, retryMax, pauseOnFailure` | `text, schedule` |

频率面板 `ScheduleEditorPanel` **直接复用**（四档完全同构），回填走 `scheduleToDatetime`（已支持四种串）。

### 5.4 刷新链路：新增 `reminders-changed` 事件

| 侧 | 动作 |
|---|---|
| 后端 | `reminders_tick` 里一次性提醒消费后 / 周期提醒推进后 / 过期丢弃后 → `app.emit("reminders-changed", …)` |
| 前端 | `useTauriListen("reminders-changed", () => void reload())` |

否则：一次性提醒触发后行不消失、周期提醒的「已触发次数」不涨、UI 改完提醒也不刷新。

### 5.5 通知权限提示（建议同批做，见 §10 优化 5）

提醒 tab 顶部：`isPermissionGranted()` 为 false 时显示一条提示条 + 「授权」按钮。
文案要点：macOS 未授权系统通知时提醒**静默失败**（不是「响得晚」，是根本不响）。

## 6. 测试策略

### 6.1 Rust 单测（`src-tauri/src/db/reminders.rs`）

| 用例 | 断言 |
|---|---|
| `set_reminder_enabled` 生效 | enabled 变 false；行不存在返回 false |
| **暂停后不再触发** | 暂停后 `load_due_reminders` 不再返回该行（这是「暂停」的全部价值，必须锁） |
| `upsert_reminder` RMW 基线不匹配 | 拒绝写（复用 `rmw_conflict`） |
| `next_fire_at` 四档 | `daily` / `weekly` / `monthly` / `at:`（未来）各返回正确时刻；`at:` 已过返回 None |

### 6.2 前端 vitest（`src/components/SchedulePage/`）

| 用例 | 断言 |
|---|---|
| 默认 tab | 显示作业行 |
| 切到提醒 tab | 显示提醒行；标签计数正确 |
| 提醒行无作业动作 | 无「立即执行」/「历史」按钮 |
| 一次性提醒 | 无「暂停」按钮 |
| 暂停态 | 行变灰、按钮变「恢复」；invoke `reminder_set_enabled` |
| 新建提醒 | invoke `reminder_create` 且参数为 `{text, schedule}` |
| 修改提醒 | invoke `reminder_update` |
| 取消提醒 | 两步确认 → invoke `reminder_delete` |
| 空态 | 两个 tab 文案不同 |
| 摘要条 | 提醒 tab 显示「最近会提醒」，不显示「最近将执行」 |

### 6.3 门禁对拍

| 门禁 | 约束 | 方案如何满足 |
|---|---|---|
| `audit_tauri_bridge.py` | 前端 invoke 的命令必须在 `lib.rs` 注册（**硬门禁**）；注册未调用的只 warn | 4 个新命令都有前端调用点 |
| `audit_error_codes.py` | Rust / TS 错误码双向一致 | 不新增错误码 → 两侧不动 |
| `audit_acceptance_matrix.py` | **SPEC 功能 ID 集合 ≡ 矩阵行集合**，每行有卡 | SPEC 加一条 → 必须同步矩阵行 + 卡（§7） |
| `audit_file_size.py` | 前端 .ts/.tsx ≤1500 行 | `ReminderRow.tsx` / `ScheduleEntryEditor.tsx` 独立成文件 |
| `audit_doc_claims.py` | 文档声明对拍 | 新增文档时按现有格式写 |

## 7. 文档同步（不可省）

| 文件 | 改动 |
|---|---|
| `SPEC.md` | 功能规格 §6 加一条 bullet → **功能 ID `6-120`**（当前末条是 6-119） |
| `docs/acceptance/matrix.csv` | 加一行 `6-120,6 数据、集成与平台,定时模块提醒分组与提醒管理,D6,ACC-P-15,—,无,<自动化覆盖路径>,L1+L2,未验,<缺口说明>` |
| `docs/acceptance/cards-06-orchestration.md` | 加 `### ACC-P-15 · 定时模块提醒分组` 卡：**SPEC**：6-120 + 步骤 + 判据（可观测 / 库证） |
| `docs/acceptance/event-names.md` | 补 4 个新事件名 |
| `docs/rust-bot-architecture.md` | 模块树若列了 `bot_reminders` 的能力面，同步加「前端入口」 |

> 矩阵的「自动化覆盖」列里的路径必须**真实存在**（门禁会逐条 `exists()` 检查）。

## 8. 批次切分（一个逻辑变更一个 commit）

| 批 | 内容 | 验收 |
|---|---|---|
| 1 | DB 层：`upsert_reminder` / `set_reminder_enabled` / `update_reminder` / `next_fire_at` + 单测 | `cargo test --lib` |
| 2 | 命令层：4 个 Tauri 命令 + `schedule_overview` 追加提醒 + 审计事件 + `event-names` | `cargo test --lib` + 门禁 |
| 3 | 前端类型判别联合（`src/types.ts`） | `npx tsc --noEmit`（**先拆类型、后改 UI**，让编译器把旧代码里的错误用法全揪出来） |
| 4 | 前端 UI：标签行 + `ReminderRow` + `ScheduleEntryEditor` + 事件刷新 + 空态/摘要文案 + vitest | `npm test` + `npx oxlint src` |
| 5 | 文档同步：SPEC / 矩阵 / 验收卡 / 架构文档 | `python3 -m pytest tests-audit/audit_acceptance_matrix.py -q` |

提交前跑 `bash scripts/test-fast.sh`；全量 `bash scripts/test-all.sh`。

## 9. 不做清单

- 不新建导航项 / 不做独立提醒页
- 不做提醒的「立即执行」「预演档」「执行历史」「执行痕迹」「会话跳转」
- 不做提醒的失败 / 重试语义（提醒不会失败）
- 不做提醒的拖拽排序
- 不做「系统通知已被用户看到」的回执（后端也不真知道用户看没看到）
- 不做邮件 / 移动端推送等多通道通知
- 不动 `due_notify`（任务卡截止提醒）链路
- 不动提醒工具组 `schedule_reminder` / `list_reminders` / `cancel_reminder`（聊天入口继续有效，与 UI 共用同一张表，不构成两个真相源）

## 10. 优化点自评

写完方案后回看，发现 13 处需要在实现时特别当心，其中 4 处是**不写就会出错**的：

### 必须处理（否则功能是错的）

1. **摘要条必须按 tab 取**——`upcoming` 现在从全部 entries 里取第一条启用项。提醒混进来后会出现「最近将执行：站起来活动一下，42 分钟后」——文案撒谎（提醒不执行）。两个 tab 各用各的动词。
2. **`oneShot` 必须后端算**——前端解析 `"at:"` 前缀是自找 `weekly:NaN` 那类漂移（`SchedulePage.tsx:16-17` 的注释就是在讲这件事）。后端已有 `Reminder::one_shot()`，直接传给前端。
3. **`nextRunAt` 取持久化的 `remind_at`，不现算**——作业的 `nextRunAt` 是 `occurrence_after` 现算，提醒的是存储值。两者在「错过窗口」下语义等价，但暂停时 `remind_at` 保留旧值 → 前端沿用作业的既有约定（`e.enabled && e.nextRunAt !== null` 才显示「下次」），暂停即不显示。
4. **提醒触发后必须广播 `reminders-changed`**——否则 UI 永远不刷新：一次性提醒触发后行不消失、周期提醒的「已触发次数」不涨。这是「做了前端」才暴露的新缺口。

### 应当处理（质量项）

5. **通知权限提示**——macOS 未授权系统通知时提醒**静默失败**。今天用户只能通过「提醒没响」反推；做了前端就该把这事说清。`isPermissionGranted()` 前端已有依赖（`App.tsx:388`）。
6. **判别联合而非平铺可选字段**——平铺方案（给 `ScheduleEntry` 加 `firedCount?` / `oneShot?`）改动更小，但 TS 不会拦住「在提醒行读 `retryMax`」。判别联合多改一个文件，换来编译期兜底。
7. **提醒行独立成 `ReminderRow.tsx`**——`SchedulePage.tsx` 已 680 行，加提醒分支会到 900 行。仓库有明确的拆大文件文化（`docs/FILE-SPLIT-PLAN-*`），新增行模板独立成文件比事后拆便宜。
8. **编辑器复用而非复制**——`JobEditor` 与 `ReminderEditor` 差 2 个旋钮 + 2 段文案。用 `variant` prop，不复制第二份 80 行组件（AGENTS.md：第二个调用方出现前不抽象——此刻正好是第二个调用方）。
9. **暂停用定点 UPDATE，编辑才用 RMW**——暂停是单列操作（`enabled`），携带整行快照 + RMW 是过度设计，还会让「UI 显示旧值 → 用户改内容 → 把 enabled 覆盖回去」成为可能。编辑（内容/时间）才是真编辑，必须带 RMW。
10. **不扩展 `schedule_set_enabled` 的 kind**——两个 kind 的落库语义不同（RMW upsert vs 定点 UPDATE），塞进同一命令会让分叉藏在函数体里。拆 `reminder_set_enabled`，命令名自解释。
11. **提醒的 `enabled` 字段需要新的写路径，但 `reminders.rs` 文件头注释要同步改**——那句「不做 RMW 基线：没有并发编辑入口」的前提被 UI 编辑入口推翻了，注释留在原地就是骗人。
12. **`reminder_delete` 返回 false 要有话可说**——行不存在通常意味着「刚刚触发了」。前端应提示「提醒已不存在（可能刚刚触发）」，而不是笼统的「删除失败」。

### 可以再想想的（本期不做，记录在案）

13. **计数口径与 tab 记忆**——标签计数含暂停（暂停了也要看得见）；tab 不持久化（持久化会让用户每次进来停在提醒 tab，作业的失败被漏看）。若实机反馈「老忘切 tab」，再考虑记忆 tab + 失败徽标单独提级。
14. **「已触发 N 次」的对比价值**——`fired_count` 有出口了，但「每天 11:00 已触发 12 次」这个数字本身对用户有多大意义？若无实机反馈，可以再想想是否该换成「上次触发：昨天 11:00」（`schedLast` 已经有了，可能 `firedCount` 反而是冗余的）。

## 11. 风险

| 风险 | 缓解 |
|---|---|
| `SchedulePage` 改动面大（680 行 + 新分支） | 提醒行与编辑器独立成文件；类型先拆（批 3）让编译器兜底 |
| 门禁「SPEC ≡ 矩阵 ≡ 卡」是硬对拍，漏一处即红 | §7 三件套一起改，同一提交内 |
| 新命令只注册不调用会被 bridge 门禁点名（warn 不 fail，但历史上因此删过命令——`23e6b380`） | 4 个命令都在同一批接上前端，不留半截 |
| macOS 系统通知授权是系统限制，开发构建可能不弹横幅 | 权限提示文案如实说明；不假装「提醒一定准时响」 |
