# 实机冒烟报告：Agent 运行时改造（Effects 队列 + 写路径原子化）

执行时间 2026-11-03 19:35 → 21:47（macOS 本机，darwin 25.6.0 arm64；基线 HEAD `0c19c321`，
覆盖窗口 `ba073c64..5cbf2c1b`）。被测产物 = `npx tauri build --debug` 出的
`wmessage.app`（bundle id `com.renshi.wmessage`，走系统通知必须有 bundle id）。
驱动 = cliclick 坐标点击 + pbcopy/cmd+v 粘贴 + screencapture 截图 + **只读** sqlite3 库证。
方案与判据原文见 `docs/realmachine-smoke-test-2026-11-03.md`。

> **收尾状态**：`git status` 干净（仅剩用户自己的 7z 分卷未跟踪），全部注入补丁已还原并
> 重建 pristine 产物；本轮产生的测试数据（7 个定时作业、4 条提醒、2 行 effect_queue 残留）
> 已清空（台账见 §六）。

## 一、总览

| 档 | 结论 |
|---|---|
| 基线门禁 | **全绿**：cargo nextest 2461 passed / 11 skipped、vitest 1188 tests、pytest 58 passed（跑前 + 收尾各一轮） |
| 主卡 21 张（M116–M136） | **PASS 12 · 部分 PASS 6 · FAIL 1（M132）· 未覆盖 2（M120、M136 半）** |
| Windows 专章（W16–W18） | 未执行（本机 macOS；判定不适用） |
| 变异注入 9 条（V136–V144） | **7 条验证通过（V136/V137/V138/V139/V140/V141/V143 —— 注入后判据均 FAIL 成立 = 变异有牙齿）· V142 未跑（时间预算）· V144 触发条件在 UI 路径上不可达** |
| 新发现 | **3 条**：D-1（P1，`at:` 一次性作业永不变更为 Dispatch）、D-2（平台口径，macOS 通知失败路径不可达）、D-3（P1，失败重试/自动暂停策略从未落库） |

**卡结论一览**：M116 部分 · M117 PASS · M118 部分 · M119 部分 · M120 未覆盖 · M121 部分 ·
M122 PASS · M123 部分 · M124 PASS · M125 PASS · M126 部分 · M127 PASS · M128 PASS ·
M129 PASS · M130 PASS · M131 PASS · M132 **FAIL** · M133 部分 · M134 PASS · M135 PASS ·
M136 PASS · W16–W18 未执行。

## 二、逐卡结论（对应方案 §一–§五）

| 卡 | 结论 | 关键证据 |
|---|---|---|
| M116 通知投递 + 成功删行 | **部分 PASS**（机制面过，横幅未取到） | 19:54:13 `sched_job_run` → 19:54:15 `sched_job_done`；Q1 投递成功后该 `sched_done:*` 行删净、无残留。**横幅未取到**：macOS 通知授权未开启（见 D-2），`show()` 恒 Ok 但横幅不弹 |
| M117 失败重试 + give-up | **PASS（机制面，受控故障注入）** | 原判据前提「关闭通知授权→发送失败」在 macOS 不成立（D-2），改用 env 门控故障注入复现：attempts 逐次 1→5；退避 2→4→8→16s；20:54:54 `effect_giveup \| kind=notify \| key=sched_done:5269857658fe458591df6524b6bbeac3`；行留库 attempts=5、error 非空；无 panic |
| M118 同 key 再入队复活 | **部分 PASS** | 同一作业二次触发 → 同 key 再入队：`generation` 1→2、attempts 由 5 **归零**后重新计次（ON CONFLICT 分支）。「投递成功、行删净」半因注入态下投递恒失败而不可验 |
| M119 ask_user 门铃 | **部分 PASS**（机制面过，横幅未取到） | 提问卡 → 「回答并继续」→ 20:10:01 `ask_answered action=answer`；Q1 无 `ask:` 残留。横幅未取到（同 D-2） |
| M120 app 运行完成通知 | **未覆盖** | 程序页无可运行的 app，构造不出「app 跑完」现场；`app_done:{run_id}` 行无法产生 |
| M121 一次性提醒弹一次删净 | **部分 PASS**（机制面过，横幅未取到） | 20:00:04 `schedule_reminder`（+2m / repeat=none）→ 20:02:29 `reminder.fired`；Q2 该行**已删**；Q1 无 `reminder:{id}` 残留 |
| M122 杀进程不重复弹 | **PASS** | 21:38:11.771 `reminder.fired` 后立即 SIGKILL → 重启后**同一 id 无第二条** `reminder.fired`；Q2 该行已不在；Q1 无残留投递。对照（V138 变异下）：21:28:29.808 → **21:29:31.595 同 id 再弹一次** |
| M123 周期提醒推进 | **部分 PASS** | daily:20:21 到点弹一次；Q2 `remind_at` 推进到 11-04 20:21、`fired_count=1`、`schedule=daily:20:21`。「次日再弹」未覆盖（需拨系统时钟，本轮未改系统时间） |
| M124 授权关闭下提醒 give-up | **PASS（机制面，受控故障注入）** | 20:49:58 首次失败 → 退避 2+4+8+16s → 20:50:26 `reminder.fire_failed \| id=70da7dcbfb4343d6b714054b24d8de12`；Q2 提醒行**已删**（一次性消费不回滚）；Q1 行 attempts=5 留库 |
| M125 子 agent 正常终态 | **PASS** | 子 agent `sa_f1eda01a…` 终态 `succeeded`；`subagent_done`（status=succeeded）；Q1 无 `subagent_finalize:` / `subagent_dispatch:` 残留；父会话收到结果 |
| M126 queued 窗口补派 | **部分 PASS（依赖变异）** | 手工路径不可命中（queued 窗口毫秒级）。V140 下观测到「行被清、无补派」、V141 下「exempted=0、行被清」→ 判据 FAIL 成立；pristine 的 `exempted=1` + 补派路径实机未取到 |
| M127 running 窗口收 failed | **PASS** | 杀进程后重启：行终态 `failed`，error「应用重启，执行现场已不存在（僵尸行清理）」；`subagent_zombie_sweep_done \| count: 1 \| exempted: 0`；父会话收到失败回报 |
| M128 取消路径回归 | **PASS** | 点行内取消方块 → `subagent_cancelled`（用户取消）；行 `cancelled`；无 `subagent_dispatch_giveup`；侧栏即时更新 |
| M129 双作业 2 槽并发 | **PASS** | 20:30:02.100 两条 `sched_job_run` **同毫秒**（A=60b9c4ee…、B=5571520d…）→ 两路并发；两作业最终 `last_status=ok` |
| M130 第三作业排队不羊群 | **PASS** | B 于 20:32:37.501 完成 → C 于 **20:32:37.508 起跑**（7ms，不等 30s tick）；**整个 bot.log 全文检索零 `sched_job_skip` / `sched_job_slot_error`**；C 最终 ok |
| M131 手动触发也过闸 | **PASS** | 两槽占满时点「立即执行」→ 按钮变灰（请求在途、后端等 `JOB_RUN_SLOTS`）；A 于 20:34:10.396 完成 → 手动触发的 D 于 **20:34:10.399 起跑**（3ms）；全程无三路并发 |
| M132 失败重试链路 | **FAIL** | 失败后 Q4 `retry_at` 空、`retry_count` 空、无 `sched_job_retry`、无 `sched_job_autopaused`；徽标 `fail` 正常更新。根因见 D-3（`upsert_scheduled_job` 不写 `retry_max`/`pause_on_failure` 两列 → UI 失败策略从未落库 → `will_retry` 恒 false） |
| M133 清扫先于派发 | **部分 PASS** | 启动段顺序确证：20:05:07.447 `app_data_dir` → 20:05:07.454 `subagent_zombie_cleared` → `subagent_zombie_sweep_done`（同步清扫早于派发器）。「queued 行被补派」的对比半未覆盖（本轮启动无 queued 行，已在 V140/V141 轮补强） |
| M134 活动页标题 | **PASS** | 首行主标题「帮我看看今天的日程安排」；旧行仍是裸 session id（对比明显）；定时作业行仍显示作业内容 |
| M135 换行与长文本 | **PASS** | 标题「实机冒烟 M135：这一条消息带换行\n第二行：…，用来…」——换行转义为字面 `\n`、40 字截断带「…」、单行无破版、无注入痕迹 |
| M136 队列不堆积 | **PASS** | Q1 中成功投递过的行**全部已删**，只剩 give-up 行（注入期产生）与进行中行；对照 V136（成功也留行）：行残留 + due 每 5s 前移 = 反复投递 |
| W16–W18 Windows 专章 | **未执行** | 本机 macOS，判定不适用 |

## 三、新发现（3 条）

### D-1（P1）`at:` 一次性定时作业**永远不会执行**——到点即被判过期删除

- **现象**：前端「定时」页建 `at:2026-11-03T19:52` 的作业 → 19:52:13
  `sched_job_stale | id=2abe254178284adc8fa26e8edc6b5256 | 过期一次性作业，删除`；
  另一条经库构造的 at: 作业同样在 19:45:13 被删。
- **根因**（`bot_scheduler.rs`）：`classify_job` 判定顺序是
  `if at_expired(sched, sched_last, now) { return Delete }` → **再** `classify_due`。
  而两者要求同一个条件 `occ <= now`：`at_expired`（`sched_last=None` 时）返回
  `occ <= now` 即真 → Delete 抢先命中；而派发所需的 `classify_due → Run` 同样要
  `occ <= now`（`occurrence_after` 对 at: 只在 `occ > after` 时返回 Some）。
  两个分支互斥，Delete 恒先命中 → **at: 作业永远走不到 Dispatch**。
  `occ > now` 时 `classify_due` 判 NotDue → 也不跑。两条路都不通。
- **归属**：由 `633e7111`（2026-10-23「机器人总开关关闭时到点作业不再每 30s 空转一轮」，
  引入 `classify_job` 的 Held/Delete 判定顺序）引入，**非本批 8 提交引入**。
- **影响面**：M116（`at:+1m`）、M129–M132（方案里都用 `at:` 一次性作业）全部受影响。
  本轮改用 `daily` 作业替代（构造偏离，已登记），并记录在案：一次性定时能力当前**整体不可用**。

### D-2（平台口径）macOS 上「通知发送失败」不可达——M117/M124 的原判据前提不成立

- **根因**：`tauri-plugin-notification 2.4.0` 的桌面端 `show()`
  （`src/desktop.rs:185-215`）把真正的投递放进 `tauri::async_runtime::spawn` 并
  **丢弃结果**（`let _ = notification.show();`），函数恒返回 `Ok(())`；
  `notify_rust` 的 macOS 后端本身也没有错误返回。
- **后果 1（对本批卡）**：M117/M124 的「关闭通知授权 → 发送失败 → 退避重试 → give-up」
  在 macOS 上**无法触发**。本轮改用 **env 门控的受控故障注入**
  （`WM_INJECT_NOTIFY_FAIL=1` 时 `deliver_notify` 直接返回 Fail）复现退避/give-up 语义，
  注入已还原并登记（§六）。
- **后果 2（产品面，值得跟进）**：通知真正发不出去时（用户未授权 / 勿扰 / 横幅被系统吞），
  effect_queue 记的是「成功」→ 行被删除 → **通知静默丢失、无重试、无审计**。
  即方案里「有账的丢失」保证在 macOS 上不成立：丢失时**没有账**。

### D-3（P1）定时作业的「失败重试次数」「失败自动暂停」**从未落库**

- **现象**：M132 全程失败。UI 上「失败重试=2 次 + 失败自动暂停」勾选后保存成功，
  但库里 `retry_max` / `pause_on_failure` 恒为 NULL（6 个作业行全 NULL），
  再次「修改」保存后仍是 NULL。后果：作业失败后不排重试（`retry_at` 空、
  `retry_count` 空、无 `sched_job_retry`），重试配额形同虚设，自动暂停永不触发。
- **根因**（`db/schedule_jobs.rs:205-220`）：`upsert_scheduled_job` 的
  `INSERT INTO scheduled_jobs (id, content, schedule, sched_last, enabled, created_at, updated_at)`
  **不包含** `retry_max` / `pause_on_failure`；`ON CONFLICT(id) DO UPDATE SET` 也不更新这两列。
  `scheduled_job_create` / `scheduled_job_update` 把值塞进结构体后交给它 → **写库时被丢弃**。
  `update_job_run_state`（调度器专用定点更新）同样不写这两列 → 全仓**没有任何 SQL 写这两列**。
- **归属**：随 `35e8338b`（T1 定时作业模块）引入，**非本批 8 提交引入**；属长期潜伏缺陷。
- **建议**：`upsert_scheduled_job` 的 INSERT 与 ON CONFLICT 补上这两列（`Option<i64>` → NULL 语义），
  并补一条落库往返单测（现无任何测试覆盖这两列的持久化）。

## 四、变异注入结论（对应方案 §八）

| 编号 | 卡 | 注入后判据 FAIL？ | 观测到的具体现象 |
|---|---|---|---|
| V136 | M116 / M136 | **是 ✓** | 「成功也留行」：投递成功后行不删，`due` 每 5s 前移（1793710758035→…763047→…768060→…773072）→ 通知每 5s 反复投递、行长期堆积 |
| V137 | M117 / M124 | **是 ✓** | `attempts` 恒 0（21:32:35→21:33:29 共 11 次采样全为 0）、due 每 2s 前移（base 退避）→ 无限重试、**无** `effect_giveup` 审计 |
| V138 | M122 | **是 ✓** | 删行与入队拆两事务：同一提醒 id 在重启后**再弹一次**（21:28:29.808 → 21:29:31.595 两条 `reminder.fired`），pristine 下同场景只有一条 |
| V139 | M117 | **是 ✓** | 去掉 attempts 过滤后，派发器被 give-up 行（due 停在过去）逼成 50ms 空转：**10s 墙钟烧 2.1s CPU（≈21%）**；同一队列在 pristine 产物下 **10s 只烧 0.05s（0.0%）** —— 同库同行的 A/B 对照 |
| V140 | M126 | **是 ✓** | 建行与入队拆开 + 10s 窗口：窗口内杀进程 → 重启后 21:11:56.953 `subagent_zombie_cleared \| was: queued` + `sweep_done count: 1 \| exempted: 0`，行终态 `cancelled`，Q1 **无** `subagent_dispatch:` 待办 → 无补派（判据要求的 `exempted=1` + 补派跑完不成立） |
| V141 | M126 | **是 ✓** | 清扫去掉 queued 豁免：造出「queued 行 + 派发待办仍在队列」现场后杀进程 → 重启 21:18:51.085 `sweep_done count: 1 \| exempted: 0`、行被清为 `cancelled` → 补派不发生（pristine 同现场应 `exempted=1`） |
| V142 | M130 | **未跑** | 时间预算未覆盖：需「两槽被长作业占满 + 第三个到点作业排队 ≥90s」才能在槽位释放瞬间看到 N−1 条 `sched_job_skip`；M130 的 pristine 侧判据已实测成立（全库零 skip） |
| V143 | M134 | **是 ✓** | 活动页首行标题回落为会话 id `5709e7d9166d4b348f3022e0a0821382`（pristine 下同输入显示「帮我看看今天的日程安排」） |
| V144 | M126 变体 | **未覆盖（触发条件不可达）** | 让 queued 行收到 failed 终态的唯一生产路径是「行不存在」的幽灵收尾（`subagent_runner.rs:106`），UI 造不出「行已删」；其余终态入队点都在行已转 running 之后 → 状态机允许 queued→running→failed，够不到 V144 的分支 |

## 五、未覆盖（诚实清单）

| 卡 / 项 | 为什么没跑 |
|---|---|
| M116 / M119 / M121 / M123 的 **OS 横幅**半 | macOS 通知授权未在系统设置里开启 → `show()` 返回 Ok 但横幅不弹（D-2）。已请用户在「系统设置 → 通知 → wmessage」开启，本轮未获回执。库证侧（行删净、无残留、审计事件）全部取到 |
| M117 / M124 的**原判据**（关闭授权→失败） | 平台不可达（D-2），改用 env 门控受控故障注入复现机制面语义；注入已还原 |
| M120 app 运行完成通知 | 程序页无可运行的 app，构造不出「app 跑完」现场 |
| M123 的「次日到点再弹」半 | 需拨系统时钟验证，本轮未改系统时间（避免影响用户环境） |
| M126 的 pristine 侧（`exempted=1` + 补派跑完） | queued 窗口毫秒级，手工无法稳定命中（方案已注明本卡依赖 V140/V141）。已用两条变异分别构造出「行被清 / exempted=0」的对照组 |
| M133 的「queued 行被补派」对比半 | 本轮启动时无 queued 行；已在 V140/V141 轮补强该对比 |
| M136 的「give-up 行自然产生」半 | 本轮 give-up 行来自故障注入；真实授权关闭场景不可达（D-2） |
| V142 变异 | 时间预算未覆盖（见 §四） |
| V144 变异 | 触发条件在 UI 路径上不可达（见 §四） |
| W16 / W17 / W18 | 本机 macOS，判定不适用；需 Windows 实机 |
| 方案 §十一列出的方案层缺口 | panic 隔离路径、`sched_job_finalize_skip` / `sched_job_slot_error` 分支、服务资源告警通知等——本轮未新增覆盖 |

## 六、环境操作台账

| 改了什么 | 改在哪 | 还原了吗 |
|---|---|---|
| **故障注入（env 门控）**：`deliver_notify` 在 `WM_INJECT_NOTIFY_FAIL` 存在时直接返回 `EffectError::Fail`（macOS 通知失败不可达，用它复现退避/give-up） | `src-tauri/src/bot/effects.rs`（`deliver_notify` 函数头，6 行） | **已还原**（`git checkout`）+ 重建 pristine 产物 |
| **V136**：`finish_ok` 调用点改为 `reschedule`（成功也留行） | `src-tauri/src/bot/effects.rs` | 已还原 |
| **V137**：`record_failure` 的 SQL 按 `cfg!(test)` 分叉（非测试构建 attempts 不自增） | `src-tauri/src/db/effects.rs` | 已还原 |
| **V138**：提醒删行与入队拆两事务，中间 `sleep 10s` | `src-tauri/src/bot_reminders.rs` | 已还原 |
| **V139**：`read_next_due` 的 attempts 闸放开为 `i64::MAX` | `src-tauri/src/bot/effects.rs` | 已还原 |
| **V140**：`spawn_subagent_locked` 建行与入队拆开，中间 `sleep 10s` | `src-tauri/src/bot/subagent_spawn.rs` | 已还原 |
| **V141**：清扫的 queued 豁免改为 `WM_SWEEP_EXEMPT` 环境变量门 | `src-tauri/src/bot_orchestrator.rs` | 已还原 |
| **V143**：`begin_trace` 的 title 恒 `None` | `src-tauri/src/bot_chat/flow.rs` | 已还原 |
| **V142 / V144** | — | 未注入 |
| **运行期环境变量**：`WM_INJECT_NOTIFY_FAIL=1`（仅故障注入窗口）、`HTTPS_PROXY=http://127.0.0.1:9`（仅 M132 失败构造窗口，指向本地死端口让模型调用必败） | 被测应用进程启动参数 | 已随重启去除；当前进程无上述变量 |
| **测试数据**：7 个定时作业（M116 / M129-A / M129-B / M130-C / M130-D / M132 / V137）、4 条提醒（M121 / M123 / M124 / M122 用）、2 行 give-up 的 effect_queue 残留 | 应用数据目录 `~/Library/Application Support/com.renshi.wmessage/wmessage.db` | **已清空**：作业经 UI「取消定时」逐个删除、提醒经聊天 `cancel_reminder` 取消、2 行 effect_queue 残留经 `DELETE FROM effect_queue WHERE error LIKE '%注入故障%'` 删除（无 UI 入口，方案允许直写 sqlite 清数据） |
| **代码库** | `src-tauri/src/**` | `git status` 干净（仅剩用户自己的 7z 分卷未跟踪），HEAD 仍为 `0c19c321`；收尾 `bash scripts/test-all.sh` 全绿 |

## 七、驱动坑（方法论）

1. **`tauri build` 会杀掉正在运行的 app**：重签 bundle 时系统终止旧进程 → 变异流水线必须
   「先构建、再启动」，不能边跑边构建（第一轮就这么丢了一次正在跑的作业）。
2. **钥匙串弹窗只点「允许」，且 AX 按按钮名点比像素坐标稳**（坐标点在某些态下会切进输密码态
   且后续点击失效）。全程未代输任何密码。
3. **cliclick 的首击常被窗口聚焦吞掉**：每个点击前先 `m:` 悬停，必要时点两次。
   另外**必须先 `set frontmost of process "wmessage" to true`**——否则点击落到 Finder，
   表现为「点了没反应」。
4. **`tauri-plugin-notification` 的 `show()` 在 macOS 恒 Ok**：任何「通知失败」类判据在 macOS
   上都要靠注入复现（D-2）。
5. **改系统时间验证次日提醒**这类会污染用户环境的手段本轮一律没做，宁可记未覆盖。
6. **`sqlite3` 只读查询要 `mode=ro`**：用默认连接读 WAL 库会偶发 `database is locked`，
   监听脚本一度因此漏掉 queued 窗口（后改用 Python + 只读 URI + 2ms 轮询才命中）。
