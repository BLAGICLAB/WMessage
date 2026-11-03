# 实机冒烟测试方案：Agent 运行时改造（Effects 队列 + 写路径原子化）

> **覆盖范围**：`ba073c64..5cbf2c1b` 共 8 个提交——批 1 写路径原子化（with_tx /
> 作业收尾两笔写合并）、批 2 统一副作用队列（`db/effects.rs` + `bot/effects.rs`，
> 8 处通知发送点收编、reminder 删行入队同事务）、批 3 子 agent 终态/派发收编 +
> 作业并发闸（G7）、批 4 活动页 trace 标题、全面审计修复（panic 隔离 / 清扫同步
> 先于派发器 / 去重集 / next_due 滤超限行 / queued 行按事实收终态）。
> 方案全文见 `docs/AGENT-RUNTIME-REFIT-PLAN-2026-11-02.md`（含 2026-11-03 修订与拍板）。
>
> **卡片**：M116–M136（21 张主卡）+ W16–W18（3 张 Windows 专章）；变异注入锚点
> **V136–V144**（编号接 10-29 方案的 V97–V135；卡片号 M116 接 M115）。
>
> **门禁前置**：本批基线 `cargo nextest + vitest + pytest 审计` 全绿；
> 实机前重跑 `bash scripts/test-all.sh` 确认仍绿。
>
> **编号口径**：Windows 专章从 W16 起（W13–W15 已被 10-29 方案占用），
> 变异编号从 V136 起（V97–V135 已被 10-29 方案占用）——**不要沿用草案里的
> W13–W15 / V97–V105，那两段号已被占**。

---

## 〇、跑法与口径

- **基线**：`5cbf2c1b`（或其后的修复提交）。跑前 `git log --oneline -1` 记录实际基线。
- **数据口径**：测试阶段老数据可删、不做迁移（用户拍板）。清数据直接删库文件或
  直写 sqlite 均可，**但时间戳列一律写毫秒整数**——`deleted_at` 等列写 ISO 字符串
  会触发启动即崩（task_from_row 对 INTEGER 列零容忍，历史坑在录）。
- **杀进程**：macOS 用 `pkill -x wmessage`（**禁 `pkill -f`**——会自匹配把自己环境
  一起杀，历史事故）；Windows 用 `taskkill /IM wmessage.exe /F`（见 W16）。
- **定位数据目录**：启动后 bot.log 首部的 `app_data_dir` 审计行（kv `path`）。
- **库证**：所有卡的事件判据 + 库证双验。sqlite3 常用查询见 §七。
- **通知授权**：macOS 系统设置 → 通知 → wmessage；Windows 见 W16。B 组卡需要
  能随时开关它——先确认入口可达。
- **结果回填**：每卡 PASS / FAIL / 部分 + 证据（事件行原文 + 库证查询输出），
  填进 §八的回填位。FAIL 必须带变异锚点编号（有锚点的卡），证明「测试有牙齿」。
- **时间预算**：本批卡多数带退避与重试（M117/M124 各约 1 分钟静置），
> 全套跑完预留 **2–3 小时**（含变异注入 9 次的重建与复跑）。

---

## 一、通知队列投递（notify kind）

### M116 通知正常投递且成功删行
- **前置**：通知授权开启；effect_queue 为空（§七 Q1）。
- **步骤**：建一个 `at:+1m` 一次性定时作业（内容随意），到点等它跑完。
- **判据**：
  1. OS 通知「⏰ 定时任务完成：…」弹出（延迟应在秒级——派发器 kick 唤醒）；
  2. Q1 查 effect_queue：该作业的 `sched_done:{job_id}` 行**已删**（成功删行语义）；
  3. bot.log 有 `sched_job_done`。
- **变异锚点**：V136（成功路径不删行 → 行残留且通知反复弹）。

### M117 发送失败 → 退避重试 → give-up 审计（核心卡）
- **前置**：**关闭**系统通知授权；清空 effect_queue。
- **步骤**：同 M116 跑一个一次性作业；跑完后静置 **90 秒**。
- **判据**：
  1. 通知不弹（授权关了）；
  2. bot.log **没有** `effect_giveup` 直到约 30 秒后出现一次——退避序列是
     **2s → 4s → 8s → 16s**（`base_ms=2000`，`min(cap, base × 2^attempts)`，
     `cap_ms=60000`），第 5 次失败时 attempts 达 `max_attempts=5` 即 give-up，
     累计等待 2+4+8+16 = 30s（**不是 62s**——最后一次退避不再等待）；
  3. 重试期间 Q1 应观察到 `attempts` 逐次 1→5；
  4. give-up 后该行**留库**（`attempts=5`，`error` 非空）且不再投递（静置 5 分钟
     不会刷屏）；
  5. 全程无 panic、无 50ms 热循环（活动页/风扇手感 + `effect_queue` 不异常增长）。
- **变异锚点**：V137（attempts 不递增 → 永不过限、无 give-up）；
  V139（next_due 不滤超限行 → give-up 后派发器 50ms 空转，Q1 行 error 期间
  CPU/日志异常）。

### M118 授权恢复 → 同 key 再入队即复活
- **前置**：接 M117（give-up 行在库）。
- **步骤**：恢复通知授权；对**同一个作业**再触发一次完成事件（手动触发既有作业，
  程序页「立即运行」）——`at:` 作业用完即删，所以要事先建一个可重复触发的
  `daily` 作业。
- **判据**：
  1. 新通知正常弹出；
  2. `sched_done:{job_id}` **同 key** 再入队 → `attempts` 归零（enqueue 的
     `ON CONFLICT` 分支重置 attempts / error，`generation+1`）→ 投递成功、行删净；
  3. Q1 查不到该 key。

### M119 ask_user 门铃通知
- **步骤**：主聊天让 bot 调 `ask_user` 提一个问题。
- **判据**：OS 门铃通知弹出（标题「❓ …」，key 为 `ask:{qid}`）；应答后无残留
  （Q1 无 `ask:` 行）。

### M120 app 运行完成通知
- **步骤**：程序页跑一个已有 app（跑得完的）。
- **判据**：「程序「…」运行完成」通知弹出；`app_done:{run_id}` 行删净。

---

## 二、提醒（删行 + 入队同事务）

### M121 一次性提醒到点：弹一次、行删净
- **步骤**：主聊天让 bot 建 `+2m` 一次性提醒（`repeat=none`）。
- **判据**：通知「⏰ 提醒」弹出一次；Q2 查 `reminders` 表该行**已删**；
  Q1 无 `reminder:{id}` 残留；bot.log 有 `reminder.fired`。

### M122 触发后杀进程不重复轰炸（核心卡，裂缝 5 正解）
- **步骤**：同 M121，但在通知弹出的**瞬间**（±2s）`pkill -x wmessage`；等 10s 重启。
- **判据**：重启后**不**再弹同一条提醒；Q2 该行已不在；Q1 无残留投递。
  对照：改造前行为是「先发后删」，杀在两步之间会在重启后补一发。
- **变异锚点**：V138（reminder 删行与入队拆回两个事务 → 重启后重复弹）。

### M123 周期提醒：推进 + 下周期再弹
- **步骤**：建 `daily` 提醒，时刻设 2 分钟后；到点后观察，再等下一周期
  （次日到点可用拨系统时间验证，测完拨回）。
- **判据**：到点弹一次；Q2 该行 `remind_at` 已推进到明天、`fired_count` +1；
  Q1 行删净；次日到点再弹。

### M124 授权关闭下提醒：重试 + give-up（有账的丢失）
- **前置**：关闭通知授权。
- **步骤**：建 `+2m` 一次性提醒，到点后静置 60s。
- **判据**：bot.log 有 `reminder.fire_failed`（**约 30s 后**，与 M117 同一条
  退避序列 2+4+8+16s，第 5 次失败 give-up）；Q2 提醒行**已删**（一次性消费不回滚
  ——丢失有账、不再轰炸）；Q1 行 attempts=5 留库。
- **变异锚点**：V137（同 M117，attempts 不递增 → 无 give-up、无限重试）。

---

## 三、子 agent 派发与终态（subagent_dispatch / subagent_finalize）

### M125 正常生命周期（终态走队列落库）
- **步骤**：主聊天派一个轻量子 agent（如「用 check_subagent 可查的简短调研」）。
- **判据**：跑完后行终态 `succeeded`（Q3）；`subagent-finished` 事件照常；
  Q1 无 `subagent_finalize:` / `subagent_dispatch:` 残留（CAS 删净）；
  父会话回报段照常出现（回报链未受影响）。

### M126 queued 窗口杀进程 → 自动补派（核心卡，变异依赖）
- **说明**：建行与派发待办已合并同事务（`subagent_spawn.rs:145-198`），
> 「有行无待办」窗口只剩「提交后 → 派发器消费前」的毫秒级缝隙，**手工无法
> 稳定命中**——本卡依赖变异。
- **步骤**：打变异 V140（建行事务与入队拆开 + 入队前 `sleep 10s` 拉长窗口）→
  重建 → 派子 agent → 3 秒内杀进程 → 重启。
- **判据**：重启日志 `subagent_zombie_sweep_done | count: 0 | exempted: 1`；
  该行**不被**置 cancelled，由派发器补派跑完（Q3 终态非 cancelled）；
  打 V141（清扫去豁免）对照组：exempted=0、行被清、补派不发生。
- **不打变异的诚实结论**：手工只能验证到「无异常路径」（M125/M127）。

### M127 running 窗口杀进程 → 按 failed 收（无豁免，符合设计）
- **步骤**：派一个跑得久些的子 agent（如「读三个文件再总结」），看到运行中后杀进程，
  重启。
- **判据**：行终态 `failed`、error 带「应用重启，执行现场已不存在（僵尸行清理）」；
  `subagent_zombie_sweep_done | count: ≥1`；父会话下一轮组装收到失败回报。

### M128 取消路径回归
- **步骤**：派子 agent → 运行中点取消。
- **判据**：行 `cancelled`（取消路径不经队列，行为不变）；无 `subagent_dispatch_giveup`；
  右侧栏状态即时更新。

---

## 四、作业调度（G7 并发闸 + 收尾事务）

### M129 双作业同时到点 → 两路并发（2 槽）
- **步骤**：建两个 `at:+2m` 作业（内容选能跑 1–2 分钟的），同时到点。
- **判据**：两个会话**同时**进入执行（bot.log 各有一条 `sched_job_run`，
  两张任务卡都进「执行中」）；完成后两个通知都到；Q4 两作业 `last_status=ok`。

### M130 第三作业排队不羊群（核心卡）
- **步骤**：在 M129 基础上再加第三个同时到点的作业。
- **判据**：第三个作业**不与前两个并发**——前一个完成后接续（槽位释放即起，
  信号量排队，不等 30s tick）；**全程无 `sched_job_skip` 审计**
  （去重集 `DISPATCHED_JOBS` 挡住 30s tick 的重复派发——改造前此场景会出现
  等待者堆积 + skip 噪声）；Q4 第三作业最终 ok。
- **变异锚点**：V142（去重集 insert 改恒真 → 等待期每 30s 一条 `sched_job_skip`）。

### M131 手动触发也过闸
- **步骤**：两个槽被长作业占满时，程序页手动「立即运行」第三个作业。
- **判据**：手动触发**排队**而非并发第三路（`scheduled_job_fire` 与到点触发
  共用同一 `JOB_RUN_SLOTS` 信号量，`:1006-1011`）——观察到它在前一槽释放后才开始；
  以**不出现三路并发**为准。

### M132 失败重试链路（正常路径回归）
- **步骤**：建一个注定失败的作业（如让它调一个会被拒的工具），retry_max=2。
- **判据**：失败后 Q4 `retry_at` 非空、`retry_count=1`、`last_status=fail` 徽标即时更新；
  5 分钟后自动重试（`RETRY_DELAY_MS = 5 × 60 × 1000`）；重试用尽后
  `sched_job_autopaused`（若开了 pause_on_failure）。
  `sched_job_finalize_skip` **不应**出现（它只在收尾写库失败时出现——见诚实清单）。

---

## 五、启动时序与展示

### M133 重启日志顺序：清扫先于派发（审计修复 P1 的守护观察）
- **步骤**：正常使用中杀进程重启（无僵尸行也行），抓 bot.log 启动段。
- **判据**：`subagent_zombie_sweep_done`（若有）时间戳**早于**任何
  `subagent_spawned`（派发器补派动作）；清扫是同步执行的（`lib.rs:580` 早于
  `lib.rs:592` 的 `spawn_dispatcher`），顺序由代码保证，本卡验实机日志确无交叠。

### M134 活动页标题显示输入摘要
- **步骤**：主聊天发「帮我看看今天的日程安排」；开活动页。
- **判据**：本回合 trace 行主标题显示「帮我看看今天的日程安排」（≤40 字截断，
  `truncate_for_log(content.trim(), 40)`），不再回落裸会话 id；
  定时作业行仍显示作业内容。
- **变异锚点**：V143（`begin_trace` 的 title 传 None → 回落会话 id）。

### M135 标题的换行与长文本
- **步骤**：发一条带换行 + 200 字的消息。
- **判据**：标题单行截断显示（后端 `truncate_for_log` → `escape_for_log`
  按字符数截到 40 + 省略号，前端 `t.title || t.sessionId` 渲染），无破版、
  无注入痕迹；换行被转义为 `\n` 字面量（后端 escape 已做，前端 React 自动转义）。

### M136 成功删行 → 队列不堆积（长期观察）
- **步骤**：正常使用半天（若干通知 / 提醒 / 子 agent / 作业）。
- **判据**：Q1 中**成功投递过的行全部已删**，表内只剩 give-up 行（授权关闭期间
  产生的）与进行中的行；行数与「后台待办」直觉一致。

---

## 六、Windows 专章

### W16 通知授权关闭的等效路径（对照 M117/M124）
- Windows 关通知：设置 → 系统 → 通知 → 关 wmessage 开关（或开「勿扰模式」）。
- **判据**：与 M117/M124 同——重试退避、give-up 审计、不刷屏。
- 附加观察：Windows 通知走 `taskkill /IM wmessage.exe /F` 杀进程（M122 的
  Windows 版），重启后不重复弹。

### W17 子 agent 杀进程补派 / 收尸（对照 M126/M127）
- 同步骤用 `taskkill /IM wmessage.exe /F`；判据同（exempted 计数、failed 收尾）。
- 注意 Windows 下杀进程是硬杀（无信号语义），running 行收 failed 的判定
  与 macOS 一致。

### W18 活动页标题渲染（对照 M134/M135）
- WebView2 下的截断与换行转义显示；中文 40 字截断按字符数（不是字节）——
  40 个汉字应完整截在第 40 字。

---

## 七、库证查询（sqlite3 模板）

```bash
DB="<app_data_dir 路径>/wmessage.db"   # 路径从 bot.log 首部 app_data_dir 行取

# Q1 effect_queue 全览（行数应与「后台待办」直觉一致）
# 列：kind / key / payload / seq / generation / attempts / due / since / error
sqlite3 "$DB" "SELECT kind, key, attempts, due, IFNULL(error,'-') FROM effect_queue ORDER BY seq;"

# Q2 提醒行
sqlite3 "$DB" "SELECT id, remind_at, fired_count, IFNULL(schedule,'one-shot') FROM reminders;"

# Q3 子 agent 行
sqlite3 "$DB" "SELECT id, status, substr(IFNULL(error,'-'),1,40) FROM subagents ORDER BY created_at DESC LIMIT 5;"

# Q4 作业状态
sqlite3 "$DB" "SELECT substr(content,1,16), last_status, retry_at, retry_count, enabled FROM scheduled_jobs;"

# Q5 派发待办与行的对账（不应出现「queued 且无待办」的活行）
sqlite3 "$DB" "SELECT s.id, s.status FROM subagents s WHERE s.status='queued' AND NOT EXISTS (SELECT 1 FROM effect_queue e WHERE e.kind='subagent_dispatch' AND e.key='subagent_dispatch:'||s.id);"

# Q6 本批新增的 effect_queue kind（只有这三种，其余为历史行）
sqlite3 "$DB" "SELECT DISTINCT kind FROM effect_queue;"
```

> 直写 sqlite 清数据：时间戳列（due/retry_at/created_at/…）必须毫秒整数，
> 写 ISO 字符串会启动即崩（INTEGER 列零容忍）。

---

## 八、变异注入锚点总表（V136–V144）

> 编号接 10-29 方案的 V97–V135。**每条变异 = 改一个字符/一行 → 重跑对应卡 →
> 判据应当 FAIL**。
> **注入前置**：改之前 `git stash` 或建 checkpoint 分支；**改完必须还原**
> （`git status` clean 才算收尾干净）。
> **注入位置以本表列出的 `file:line` 为准**——行号是 `5cbf2c1b` 现状，
> 别的提交动过同一文件后要按符号名重新定位。

| # | 卡 | 注入点（file:line） | 改动 | 重跑后应 FAIL 的现象 |
|---|---|---|---|---|
| V136 | M116 / M136 | `bot/effects.rs:257` `finish_ok` | 改成「成功也留行」（去掉 DELETE，只更新 seq） | 通知反复弹、行堆积在 Q1 |
| V137 | M117 / M124 | `db/effects.rs:190` `record_failure` | `attempts = attempts + 1` → `attempts = attempts` | 永不过限、无 give-up、无限重试 |
| V138 | M122 | `bot_reminders.rs:372-375` | 删行与入队拆回两个事务（先 enqueue 后 delete，删失败不回滚） | 杀进程重启后重复弹同一条提醒 |
| V139 | M117 | `db/effects.rs:145` `next_due` | 去掉 `attempts < ?1` 过滤 | give-up 后派发器 50ms 空转，CPU/日志异常 |
| V140 | M126 | `bot/subagent_spawn.rs:145-198` | 建行事务与入队拆开 + 入队前 `sleep 10s` | 建行后杀进程 → 重启时行被清（无补派） |
| V141 | M126 | `bot_orchestrator.rs:602-613` | 清扫去掉 queued 豁免（`has_pending` 恒 false） | exempted=0、行被清、补派不发生 |
| V142 | M130 | `bot_scheduler.rs:725` `spawn_job_run` | 去重集 `set.insert` 改恒真返回 | 等待期每 30s 一条 `sched_job_skip` |
| V143 | M134 | `bot_chat/flow.rs:645` | `begin_trace` 的 title 传 `None` | 活动页回落会话 id |
| V144 | M126 变体 | `bot/effects.rs:471-477` `finalize_subagent` | queued 行写终态改回直写 failed（去掉改投 cancelled 的分支） | queued 行写终态必败 → `subagent_terminal_state_write_failed` 出现、行永挂 queued |

---

## 九、本批事件名速查（判据引用）

投递侧：`effect_giveup` · `notify_enqueue_failed` · `reminder.enqueue_failed` ·
`reminder.fired` · `reminder.fire_failed` · `subagent_dispatch_giveup` ·
`subagent_terminal_state_write_failed` · `ask_notify_enqueue_failed`
调度侧：`sched_job_finalize_skip`（仅在收尾写库失败时出现，正常路径不应见）·
`sched_job_slot_error`（仅并发闸异常时）· `sched_job_skip` · `sched_job_run` ·
`sched_job_done` · `sched_job_err` · `sched_job_retry` · `sched_job_autopaused` ·
`sched_job_panic`
子 agent：`subagent_zombie_sweep_done`（格式：`count` + `exempted`）·
`subagent_zombie_cleared` · `subagent_spawned`
提醒：`reminder.expired` · `reminder.set` · `reminder.cancel`
其它：`due_notify` · `due_notify_fail` · `service.resource_warning`

> **本批事件名与白名单的差口**：`sched_job_finalize_skip` / `sched_job_slot_error` /
> `subagent_zombie_sweep_done` 三条**尚未登记进 `docs/acceptance/event-names.md`**
> （该白名单由 `audit_acceptance_matrix.py` 对拍）。本批不涉及 SPEC 功能条目变更，
> 故不改矩阵；**但这三条事件名在跑卡时要能在 bot.log 里 grep 到**——
> 见到「文档说有、bot.log 里没有」先查源码再判缺陷。

---

## 十、结果回填位

> 执行者按卡逐条填。**PASS / FAIL / 未覆盖 三选一**，不要留空。**未覆盖必须写清
> 为什么**（环境不具备 / 卡与卡之间没来得及复位 / 模型不配合 / 构造不出来）。

### 10.1 逐卡结论

| 卡 | 结论 | 证据 | 备注 |
|---|---|---|---|
| M116 通知投递 + 成功删行 | | | |
| M117 失败重试 + give-up | | | |
| M118 同 key 再入队复活 | | | |
| M119 ask_user 门铃 | | | |
| M120 app 完成通知 | | | |
| M121 一次性提醒弹一次删净 | | | |
| M122 杀进程不重复弹 | | | |
| M123 周期提醒推进 | | | |
| M124 授权关闭下提醒 give-up | | | |
| M125 子 agent 正常终态 | | | |
| M126 queued 窗口补派 | | | |
| M127 running 窗口收 failed | | | |
| M128 取消路径回归 | | | |
| M129 双作业 2 槽并发 | | | |
| M130 第三作业排队不羊群 | | | |
| M131 手动触发也过闸 | | | |
| M132 失败重试链路 | | | |
| M133 清扫先于派发 | | | |
| M134 活动页标题 | | | |
| M135 换行与长文本 | | | |
| M136 队列不堆积 | | | |
| W16 Windows 通知授权 | | | |
| W17 Windows 子 agent 杀进程 | | | |
| W18 Windows 活动页标题 | | | |

### 10.2 变异注入结论

| 编号 | 卡 | 注入后判据 FAIL？ | 观测到的具体现象 |
|---|---|---|---|
| V136 | M116 | | |
| V137 | M117 | | |
| V138 | M122 | | |
| V139 | M117 | | |
| V140 | M126 | | |
| V141 | M126 | | |
| V142 | M130 | | |
| V143 | M134 | | |
| V144 | M126 变体 | | |

### 10.3 未覆盖（诚实清单）

> **跑了什么、没跑什么，必须如实列**。空着不填 = 这份报告不可信。

| 卡 / 项 | 为什么没跑 |
|---|---|
| | |

### 10.4 环境操作台账

> 所有临时改动（常量、配置键、白名单、注入的 bug）**逐条登记 + 还原**。
> 收尾必须 `git status` clean + 干净启动验证通过。

| 改了什么 | 改在哪 | 还原了吗 |
|---|---|---|
| | | |

---

## 十一、未覆盖诚实清单（方案层面的已知缺口）

> **下面是方案设计上的缺口，不是本轮没跑**——跑卡时遇到别误判成缺陷。

- **panic 隔离路径**（catch_unwind → 按失败计次）：`bot/effects.rs:178-191` 已有
  panic 兜底，但**未编号变异**（补测时从 V145 起编）。手工也难注入——
  handler panic 要改生产代码。
- **服务资源告警通知**（`service_alert:{service_id}:{kind}:{scope}`）：需构造
  RSS/CPU 超阈值的受管服务，人工难触发；口径由 M116–M118 同代码路径覆盖
  （同一 `enqueue_notify` 入口），告警本体已有 `service.resource_warning` 审计。
- **`sched_job_finalize_skip` 分支**：只在收尾写库失败时出现，需锁库/拔盘注入，
  冒烟不覆盖（单测已锁同事务回滚语义）。
- **`sched_job_slot_error` 分支**：只在信号量被 close 时出现（编程错误），
  正常路径不可达。
- **observe.rs 自动暂停通知**（`sched_pause:{job_id}`）：长链路（重试用尽 +
  pause_on_failure），可用 M132 的作业加 pause 配置顺带观察，不单列。
- **effect_queue 长期性能/堆积**：非冒烟范畴；保留期策略随批 7 健康面板定。
- **批 1 的 upsert_tasks 契约断言**：debug_assert，release 实机不可见（单测覆盖）。
- **工作单元 work_items / 面板聚合 / 干跑模式**：批 5–8 未实施，无卡。
- **M126 的手工路径**：queued 窗口是毫秒级缝隙，手工稳定命中不了——
  本卡**依赖 V140/V141 变异**，不打变异只能验「无异常路径」。

---

## 十二、与前几份的关系

| 文档 | 覆盖窗口 | 卡片 | 状态 |
|---|---|---|---|
| `docs/realmachine-smoke-test-2026-10-25.md` | 10-24 10:51 → 10-25 15:36 | M75–M92 + W10–W12 | 已执行 |
| `docs/realmachine-smoke-test-2026-10-29.md` | 10-28 20:54 → 10-31 16:27 | M93–M115 + W13–W15 | 已执行 |
| **`docs/realmachine-smoke-test-2026-11-03.md`（本份）** | **11-02 05:31 → 11-03 08:42** | **M116–M136 + W16–W18** | **待执行** |
| `docs/AGENT-RUNTIME-REFIT-PLAN-2026-11-02.md` | 方案全文（批 1–8） | — | 批 1–4 已落地；批 5–8 待推 |
| `docs/ACCEPTANCE-PLAN-2026-10-30.md` | 按**功能条目**索引（全库 114 条） | `docs/acceptance/cards-*.md` | 与本份**互补** |

> **两条验收面的关系**：`ACCEPTANCE-PLAN-2026-10-30.md` 按 SPEC 功能条目建卡
> （回答「这条功能验没验过」），本份按改动窗口建卡（回答「这批改动对不对」）。
> 本批**不改 SPEC 功能条目**（批 1–4 是内部机制改造），故验收矩阵不动。
