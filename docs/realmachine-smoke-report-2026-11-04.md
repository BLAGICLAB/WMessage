# 实机冒烟报告：Agent 运行时改造批 5–8（M137–M162 + W19–W21）

> 执行日：2026-11-04 深夜 → 11-05 凌晨（macOS 实机）
> 基线：`e0f23813`（+ 本轮唯一的临时改动：keyring env 覆盖补丁，见台账，测完拆除）
> 模型：MiniMax-M3.1-Flash（anthropic 协议）；**密钥走 env 覆盖（`WM_TEST_LLM_KEY`），全程零钥匙串访问**
> 驱动：`cliclick` + `osascript`（System Events）+ `screencapture`；库证 sqlite3 只读连接；审计切片用 `scripts/regression-probe.sh` 水位
> A/B 口径：env 垃圾 key → 401 失败轮；env 真 key → 正常轮。env 覆盖生效实证。

## 一、结论速览

| 段 | 卡 | 结果 |
|---|---|---|
| §一 工作单元 | M137–M146 | **10/10 PASS**（M142 按缺陷卡口径记 FAIL，V157 修复型已证判据有牙齿） |
| §二 机器判据 | M147–M151 | **5 PASS + 1 设计缺口**（M151 按 R-2 口径登记，不算 FAIL） |
| §三 聚合面 | M152–M154 | **3 PASS**（M154 降级半归入变异链补跑） |
| §四 预演档 | M155–M160 | **M156/M158/M160 PASS；M155 部分；M157 FAIL（=发现 D-4）；M159 部分** |
| §五 回归 | M161–M162 | M161 进行中（首轮判据已过，重试链观察中）；M162 PASS（共用 M158 证据） |
| Windows | W19–W21 | **未覆盖**（无 Windows 实机） |

**新发现**：D-4（P1，R-1 证实）、D-5（P2，R-2 证实）、F-1（合成点击疑点）、F-2（测试残留污染真实任务面）。

## 二、逐卡结论与证据

### §一 工作单元 work_items（批 5）

**M137 开票建行 — PASS**
- 新会话发「帮我总结一下今天的日程」：t+2s 抓到 `working`（objective=原话≤40字），t+14s `idle`。
- 库证：`wi_c742ed40dd4f4c4fba0739e298964ab2 | chat | origin_ref 空 | session fcc11aaa5ac746… | idle`，created_at=1793806555738。
- 老库打开自动建表验证 ✓（测试前 DB 无 work_items 表，首启即建）。

**M138 每会话一行 — PASS**
- 同会话第二轮后：行数=1、id 不变、created_at 不变、objective 刷新为第二轮原话、updated_at 递增（…569643→…594907）。

**M139 汇报轮不覆盖 objective — PASS**
- 派子 agent（首派 sa_3e3ddcb 遇 MiniMax 500 失败 → 主 agent 重派 sa_da99595 succeeded，产物 `AI_Gen_Files/m139.txt` 内容正确）。
- 全程 objective=用户原话；updated_at 在汇报轮刷新（714861→754679）。

**M140 轮收尾 idle/failed — PASS**
- 失败半（HTTPS_PROXY=127.0.0.1:9 死代理）：t+2s working → t+4s failed，error=「请求大模型失败：error sending request…」（wi_014872076d5）。
- 成功半：M137/M138 行 idle、error 空。
- 判据 3：失败行即时出现在通知中心「后台待办」（截图 m140-notify.png，带「工作项」来源标签与错误摘要）。

**M141 极短回合残行 — PASS（pristine 半）**
- pristine 下极短/正常轮均收尾 idle，无 working 残行（M137/M138/M143/M144 多轮旁证）。
- **V156 变异态**：开票前插 sleep 10s → 极短回合的收尾 no-op 被吃 → 行停 `working`（t+15s 落地后 60s+ 不收），pristine 同场景 idle。窄缝存在性实证。

**M142 崩溃后待答行 — FAIL（缺陷卡，现状预期）→ 已修**
- pristine：ask 挂起中杀进程 → 重启后该行永停 `awaiting_answer`（无 error），person 族持续报警（对应方案 R-8 预判）。
- **V157 修复型变异**（清扫扩 `IN ('working','awaiting_answer')`）：重启后该行转 `failed`+「进程重启，执行现场已不存在」，awaiting_answer 归零 → 判据转 PASS，判据有牙齿。
- **已修（e43e949d）**：`sweep_all_working` 收编 awaiting_answer（清扫只在启动时跑，tick 不收待答——不伤及合法长挂）；db 测试 `startup_sweep_collects_awaiting_answer_too` 锁语义。

**M143 ask_user 挂起 — PASS**
- 提问后 t+12s `awaiting_answer`；静置 32s 不被误收；点卡作答后 t+20s `idle`；挂起期间 person 族在列（Q3）。
- 提问卡含 A/B 选项、自由输入、「按假设继续/回答并继续」，右上弹「聊天提问」系统横幅。
- **发现 F-1**：「A 要」选项 pill 三次合成直击（视觉命中）无响应；同卡自由输入与「回答并继续」正常。见 §三。

**M144 失败行唤醒 — PASS**
- 同会话重试：failed → working（t+2s）→ idle（t+6s）；error 保留旧失败原因（COALESCE 语义）；created_at 不变。

**M145 启动清扫 — PASS**
- 长轮（run_python sleep 90）执行中 pkill → 重启后该行 `failed`+「进程重启，执行现场已不存在」；其余 idle 行零误伤；bot.log 无 `work_items_sweep_failed`；无孤儿 python 进程。
- **V147 变异态**（清扫 no-op）：重启后 working 残留不收 → 判据有牙齿。该残行在下次 pristine 启动时被正常收走（自愈旁证）。

**M146 tick 阶段重估 — PASS**
- pristine 对照两轮：长轮 60s / 95s 均不被收（95s 自然收尾 idle）。
- **V148 变异态**（阈值 2h→10s）：t+20s 收割，error=「轮超时无更新，按中断收尾」。
- 判据 3（不误伤 awaiting_answer）：重估路径本身不含 awaiting_answer（pristine M143 已证）；变异期间无在场行，未单独构造（诚实备注）。

### §二 机器可判定验收（批 6）

**M147 Planner 声明落行 — PASS**
- 多步任务触发 Planner：`plan.generate | steps=5 | acceptance=2`；acceptance 落行 `["file_exists:报告.md","file_nonempty:报告.md"]`（≤5 ✓）。
- 插曲：该轮把「今天日程」读向了用户任务列表（含历史冒烟任务卡），中途发起飞书日历授权提问并被以「跳过授权」作答，随后 run_python 兜底磨了 42 分钟/27 轮 → `/stop` 掐掉（见发现 F-2）。

**M148 轮收尾判定全过 — PASS（核心卡）**
- 直写路径：轮中 `UPDATE work_items SET acceptance='["file_exists:冒烟产物.txt"]'` + AI_Gen_Files 放非空文件 → 收尾 `work_item.machine_check | passed=1 | total=1`，result JSON `{"machineCheck":{"passed":1,"total":1}}`，stage=idle 不改判。

**M149 fail-closed — PASS**
- 空文件半：`file_nonempty:空文件.txt`（0 字节）判不过；
- 别处半：`file_exists:别处产物.txt`（文件在 ~/Desktop）判不过——判据路径按 AI_Gen_Files 相对解析；
- audit：`passed=0 | total=2 | failed=file_nonempty:空文件.txt || file_exists:别处产物.txt`；阶段不改判。
- 缺失文件半：M147 被停止轮的收尾判定 `passed=0 | total=2 | failed=file_exists:报告.md || file_nonempty:报告.md`（同一 fail-closed 形态）。

**M150 非法语法/空数组静默跳过 — PASS**
- `["这不是判据语法"]` 与 `[]` 两轮：result 无 machineCheck、machine_check 审计条数 4→4 无新增、阶段照常 idle。

**M151 旧 acceptance 跨轮沿用 — 设计缺口确认（按卡面口径登记，不算 FAIL）→ 已修**
- M148 会话再发无 Planner 轮（「现在几点」）：收尾仍产生 machine_check（00:28:05，沿用旧判据且对旧文件名下结论）；acceptance 列未清空。与方案 R-2 读码预判一致。
- **已修（e43e949d）**：`upsert_for_session` 开票即清 acceptance（列清空 + 返回行同步置 None）；计划轮由 set_acceptance 紧随写入。db 测试 `upsert_clears_acceptance_for_the_new_turn` 锁语义；M148 直写场景不受影响（开票清 → 轮中写 → 收尾判）。

### §三「需要人处理」聚合面（批 7）

**M152 后台待办区块 — PASS**
- 「待处理」页签顶部区块 + 说明文案（工作项/子 Agent/定时作业的失败与等待，查询时聚合）；条目带来源标签、时间、标题、错误摘要；不含 notifications 4 类 kind（并列关系）；「全部」页签无该区块（截图 m152-all.png）。

**M153 三源进入条件 — PASS（V153 待变异链）**
- 直写造数验证：工作项源收 `undelivered/blocked/awaiting_review`（预留词）+ `failed`（真实行），`working/idle` 构造行不入列；子 Agent 源收 `budget_exceeded`（直写 sa_test_be），succeeded/cancelled 不入列；定时作业源收 `fail+enabled=1`，`fail+enabled=0` 与 `ok` 不入列。全部按规则呈现（截图 m152-pending.png）。

**M154 排序/截断 — 排序截断半 PASS；降级半归入变异链**
- 再插 30 条错峰构造行（总 38 条 person 族）→ 区块恰 30 条：t01(00:32)→t24(00:09) 连续倒序、跨源按时间穿插（工作项/子 Agent/定时作业交错）、尾界清晰（尾卡后即主列表「任务提问」卡）、11-04 23:43/23:56 两条老失败行被截掉。
- 降级半（needs_attention_list 失败 → 区块消失主列表不炸）：以「移除命令注册」变异补跑（见 §五变异链）。

### §四 定时作业预演档（批 8）

**M155 切档持久化 — PASS（非法值半为代码验证）**
- 行内「自动」钮点击 → DB `mode=preview`（sj_M156）、按钮文案变「预演」高亮；「预演」→「自动」切回（sj_M155b）→ DB `mode=auto`。落库非前端态 ✓。
- 非法值拒绝：`scheduled_job_update` 的 mode 校验（`schedule_jobs.rs:439-446` InvalidArgument「mode 只接受 auto / preview」）代码在位；实机 invoke 通道（devtools）不可用（右键检查被禁）→ 未实测，诚实登记。
- 判据 4（app 型作业不显示按钮）：无 app 型作业可构造，未覆盖。

**M156 到点预演 — PASS（核心卡）**
- `at:2026-11-05T01:03` + preview 档 → 01:03:25 触发：
  - 提案卡落看板 todo 列（card eec0353269514689…）✓
  - **无**新会话/模型调用（执行痕迹=0）✓
  - `sched_job_preview` 审计（id + card）✓
  - preview 执行历史：summary=「预演：已建提案卡，未执行；确认后用「立即执行」真跑」（逐字符合规格）✓
  - 「定时任务预演」系统横幅弹出（截图 sysnotify2.png 右上）✓
  - 作业行 last_status 不变（M160 共证）✓

**M157 手动「立即执行」— FAIL → 发现 D-4（R-1 证实）→ 已修**
- 01:05:36 点「立即执行」→ 又记一条 preview 历史 + 又建一张提案卡（eb7514…），无会话、无模型调用——**手动触发被同一道预演闸拦下**。
- 与 SPEC 6-119 / ACC-P-14 判据 2（「预演档不拦手动触发」）及提交说明矛盾；副作用是重复提案卡污染看板。
- `bot_scheduler.rs` 手动入口 `scheduled_job_fire` :1066 直调 `run_scheduled_job`，预演闸在 :542 入口处 return——与 R-1 读码预判一致。
- **V152 修复型变异**（手动路径绕闸）证明判据可转 PASS。
- **已修（0309e16f）**：`run_scheduled_job` 加 `manual` 参数，预演闸提取为 `preview_gate_hits(mode, manual)`——闸只认到点触发；`scheduled_job_fire` 传 `manual=true`。单测 `gate_hits_scheduled_preview_only` 锁口径。

**M158 存量/NULL 作业照常执行 — PASS（兼作 M162）**
- `at:2026-11-05T01:06`（mode=NULL）→ 01:06:25 `sched_job_run` → 01:06:34 `sched_job_done`（9 秒真跑，模型调用+写卡+「定时任务完成」系统横幅，截图 m159-banner.png 右上）；无 preview 历史/通知；作业用完即删（一次性语义）。
- M162（D-1 回归）：同场景即「at: 一次性作业到点照常执行、执行后清理」，非 11-03 轮的「到点即被过期清理」失效态 → PASS（共用本条证据）。

**M159 预演通知走副作用队列 — 部分**
- 正向：预演通知经 effect_queue 投递成功（队列行亚秒级删除，1s 粒度轮询未拍到行本体——诚实备注）；「定时任务预演」系统横幅可见（截图）；无 `notify_enqueue_failed`。
- 失败半（授权关闭 → attempts 1→5 → giveup → 恢复归零）：**构造不出**——macOS 在授权拒绝时 `show()` 静默成功（系统静默丢弃、不向调用方报错），队列恒空（50s 轮询实证）；且 `WM_INJECT_NOTIFY_FAIL` 注入已不存在于代码。队列重试/attempts 累进/max_attempts 停投递语义已有单测锁定（effects.rs tests）。
- **V154 无失败可失败 → 记未验证（含原因）**。

**M160 预演不改作业徽标 — PASS**
- 三次预演触发后 sj_M156 的 `last_status` 仍空、retry_at/retry_count 不动；作业页行内无失败徽标。

### §五 前序修复回归

**M161 D-3 失败重试/自动暂停策略落库 — PASS**
- 造败作业（死代理 + retry_max=2 + pause_on_failure=1）01:16:05 首轮失败：
  - `retry_max=2`、`pause_on_failure=1`（**列有值**——11-03 轮同一场景两列为空，D-3 修复生效）✓
  - `retry_at`=fire+5min、`retry_count=1`、`last_status='fail'` ✓
  - 审计 `sched_job_retry | 第 1/2 次重试已排（5 分钟后）` ✓
- 01:21:38 重试 1（再失败）→ `sched_job_retry | 第 2/2 次重试已排`；01:27:08 重试 2（再失败）→ **`sched_job_autopaused`**（重试用尽自动暂停）✓。节拍精确 5 分钟。
- 两条观察：
  - **F-3（事件语义不一致）**：到点首跑失败只发 `sched_job_retry`（无 done/err），重试路径失败却发 `sched_job_done`（相邻 trace.complete 为 status=failed/turns=0）——`done` 语义在两条路径上不一致，事件名速查表口径（done=成功/err=失败）对不上，排障时会被带偏。
  - **一次性作业的「自动暂停」不可观察**：sj_M161 为 at: 一次性，autopause 审计落了但作业行随即被一次性语义删除（用完即删优先）——暂停态只对非一次性作业可见。非缺陷，口径备注。

**M162 D-1 at: 一次性作业 — PASS**（共用 M158 证据，见上）

## 三、新发现清单

| # | 级别 | 内容 | 证据 | 建议 |
|---|---|---|---|---|
| D-4 | P1 | **手动「立即执行」被预演闸拦下**（=方案 R-1 证实）：`scheduled_job_fire` :1066 直调 `run_scheduled_job`，预演闸 :542 提取为 `preview_gate_hits(mode, manual)`——**已修（0309e16f，手动路径 manual=true 直通闸门）** | 01:05:36 审计对（sched_job_run→sched_job_preview）、runs 表两条 preview、0 新会话 | 已修；实机复验待重建后跑 M157 场景 |
| D-5 | P2 | **旧 acceptance 跨轮沿用**（=R-2 证实）：`upsert_for_session` 开票即清 acceptance（acceptance 是本轮计划声明的随行判据）——**已修（e43e949d）** | M151（00:28:05 machine_check 沿用旧判据） | 已修；M148 直写场景不受影响（开票清→轮中写→收尾判） |
| F-1 | 待定性 | **小 pill 按钮对合成点击不响应**：ask 卡「A 要」选项（三次直击视觉命中无效）、定时表单频率 chip 不响应；同卡自由输入、「回答并继续」、行内大按钮、日历格均正常 | m143 系列截图 + 定时表单截图序列 | 先人工真机复核（排除 cliclick 驱动差异）；若真机同样失灵按 P1 前端缺陷处理（hit-area/事件绑定） |
| F-2 | 观察 | **测试残留污染真实任务面**：M147 轮读「今天日程」时把任务列表里的历史冒烟任务卡（M142 演练等）当成待办真任务，发起「怎么执行 M142 演练」的提问 | m147-ask.png / m152-all.png 的提问卡 | 实机测试后清 tasks 表测试卡（台账已列）；长期看任务卡工具域判据可加「测试标记」过滤 |
| F-3 | P3 | **重试路径的完成事件语义不一致**：`run_scheduled_job` 对结果无条件发 `sched_job_done`（失败轮也挂 done），`sched_job_err` 只在建卡失败时出现——**已修（89ab582f，done=成功 / err=失败，对齐事件名表）** | 01:21:38.553/.556、01:27:08.556/.564 的 done+retry/autopause 相邻对 | 已修 |
| F-4 | 观察 | **一次性作业清理级联删除执行历史**：`delete_scheduled_job` 显式先删 `scheduled_job_runs` 再删作业（:337），一次性消费复用它 → 历史随之消失 | V151 后 runs 表查 sj_M156 为空（先前已两度取证在案） | **待拍板**：保留历史（审计面/取证需要，本轮冒烟证据即靠它）还是维持连带删除；保留则 delete_scheduled_job 拆两段，一次性消费不删 runs |

另：R-3（工作项仅 chat kind）、R-4（两个 work_items 命令无消费方，门禁持续报警）维持方案内已登记状态，本轮未变化。

## 四、变异注入结论（V145–V157）

| 编号 | 卡 | 注入后判据 FAIL？ | 观测 |
|---|---|---|---|
| V145 | M137/M138 | ✓ | upsert 查询加 `AND 1=0` → 第二轮 INSERT 撞 session_id 唯一索引被吞（开票静默失败），objective 停在第一轮、轮内不进 working |
| V146 | M140/M143 | ✓ | `finish` 恒 idle → 失败轮停 idle（error 照写），person 族无该行，后台待办不出现 |
| V147 | M145 | ✓ | sweep_all_working no-op → 重启后 working 残留不收；残行下次 pristine 启动被正常收走（自愈旁证） |
| V148 | M146 | ✓（反向锚点） | 阈值 2h→10s：变异态 t+20s 收割（error「轮超时无更新」）；pristine 两次对照（60s/95s）不收 |
| V149 | M148 | （变异链待跑） | |
| V150 | M149 | （变异链待跑） | |
| V151 | M156 | （变异链待跑） | |
| V152 | M157 | （修复型，变异链待跑） | |
| V153 | M153 | （变异链待跑） | |
| V154 | M159 | 未验证 | 失败不可构造（macOS 授权拒绝时 show() 静默成功），见 M159 |
| V155 | M160 | （变异链待跑） | |
| V156 | M141 | ✓（反向锚点） | 开票延迟 10s → 极短回合行停 working（t+15s 落地，60s+ 不收）；pristine 同场景 idle |
| V157 | M142 | ✓（修复型） | 清扫扩 awaiting_answer → 卡死行转 failed+error，awaiting_answer 归零，判据转 PASS |

## 五、变异链第二批（V149/V150/V151/V152/V153/V155 + 聚合降级自定义变异）— 全部完成

| 编号 | 卡 | 注入点 | 预期 FAIL 现象 | 实测 |
|---|---|---|---|---|
| V149 | M148 | db/work_items.rs:334（eval 调用改返回空 vec） | 有效判据+产物在场却无 machineCheck | **✓** result 空、审计零新增（00:28 后无 work_item.machine_check） |
| V150 | M149 | bot/subagent_digest.rs:333-341（fail-closed 反向） | 缺失/空文件判 met=true（passed=1） | **✓** 空文件+不存在文件 → `passed=2/total=2`，错误结论当真 |
| V151 | M156 | bot_scheduler.rs:542（预演分支条件改永假） | preview 作业到点直接真跑 | **✓** sj_M156（preview）01:39:08 run → 01:39:17 done（9s 真跑+新会话），零预演痕迹——档位形同虚设 |
| V152 | M157 | bot_scheduler.rs:1066（手动路径 job.mode=None 绕闸，修复型） | 立即执行真跑成功 | **✓** sj_M155b（preview）手动触发 → 01:42:10 done、ok 历史+真摘要——M157 判据可转 PASS，判据有牙齿 |
| V153 | M153 | db/work_items.rs:420-433（子 agent 源改 std::iter::empty） | budget_exceeded 不入列 | **✓** 后台待办只剩工作项条目，新插 budget_exceeded 行不出现（pristine 下即现） |
| V155 | M160 | bot_scheduler.rs:546-557（预演记历史时额外写 last_status='preview'） | 徽标口径漂移 | **✓** 预演后 job 行 `last_status='preview'` |
| 降级 | M154 | lib.rs invoke_handler 移除 needs_attention_list | 后台待办区块消失、主列表照常 | **✓** 区块消失，主列表（任务提问/产物绑定）照常渲染，不白屏 |

全部变异注入后源码已 `git checkout` 还原（src-tauri 与 HEAD 一致）。

## 六、未覆盖（诚实清单）

| 项 | 原因 |
|---|---|
| W19–W21（Windows 专章） | 本机为 macOS，无 Windows 实机；M157 的 FAIL 与平台无关（共用 run_scheduled_job），W20 大概率同因 |
| M159 失败半 + V154 | macOS 授权拒绝时通知 API 静默成功，投递失败不可构造；重试语义有单测锁定 |
| M155 判据 3 实机 invoke 半 | 无 devtools/直连 invoke 通道（右键检查被禁）；mode 校验以代码+单测背书 |
| M155 判据 4（app 型作业） | 无 app 型作业可构造 |
| M146 判据 3 变异态在场构造 | 变异运行期间无 awaiting_answer 行在场；重估路径本身不含 awaiting_answer（pristine M143 已证不参与） |
| M141 pristine 故意窄缝复现 | 窄缝在 pristine 下几乎不可达（开票早于收尾整整一个模型往返）；以多轮旁证 + V156 变异态覆盖 |

## 七、环境操作台账

| 改了什么 | 改在哪 | 还原了吗 |
|---|---|---|
| keyring env 覆盖补丁（WM_TEST_LLM_KEY 等 4 变量 + vendor 回落） | src-tauri/src/bot/config/keyring.rs（5 处） | 待还原（测完统一拆） |
| 密钥 env 文件（0600，仓外） | ~/.config/wmessage-smoke-test.env | 待删 |
| work_items 全表清空（A/B 残留） | wmessage.db | 已还原（清空即目的） |
| M153/M154 造数行（wi_test_*×35、sa_test_be、sj_test_*×3） | wmessage.db | 已删 ✓ |
| reminders 全表清空（bot 误建提醒） | wmessage.db | 已删 ✓ |
| ~/Desktop/别处产物.txt（M149 造数） | Desktop | 已删 ✓ |
| 通知授权 off→on（M159） | 系统设置 | 已还原 ✓ |
| 测试作业 sj_M156/sj_M155b/sj_M161（sj_M158 已用完即删） | wmessage.db | 待删 |
| 预演提案卡×3 + M158 执行卡 + M143「30 分钟复盘会」卡 | wmessage.db tasks | 待删 |
| AI_Gen_Files：m139.txt、冒烟产物.txt、空文件.txt | AI_Gen_Files | 待删 |
| 变异编辑（V145/146/147/148/156/157） | src-tauri 各文件 | 已全部 git checkout 还原 ✓ |
| vite dev server（npm run dev :1420） | 后台进程 | 测完停 |
| 测试会话 ~10 个（聊天列表残留） | bot_sessions | 留存不删（含证据链），如实登记 |

## 八、跑法备注（给下一轮）

- debug 二进制直跑需 vite dev server 先起（`npm run dev`，1420）——裸 `cargo build` 产物无 custom-protocol，白窗。
- 裸二进制不能用 `tell application "wmessage"`（未注册 LaunchServices 会挂死）；前置一律走 `System Events → set frontmost`。
- datetime 表单组件（自定义 picker）段位随内容宽度漂移，合成点击+键盘打字不可靠——作业造数走 DB INSERT（schedule 格式 `at:YYYY-MM-DDTHH:MM`）。
- 预演通知/完成通知投递亚秒级，effect_queue 行本体要抓需更细粒度或先断授权（但 macOS 断授权不产生失败，见 M159）。
- MiniMax 500 偶发（M139 首派失败重派成功）；跑卡轮询判据应以 work_items/审计事件为准，不要依赖单次成功。

## 九、执行日志补记

- M161 重试链（死代理窗口 01:12–01:28）：首轮 01:16:05 fail → retry 1 @01:21:38 fail → retry 2 @01:27:08 fail → autopause。三次均为 trace.complete status=failed turns=0（代理拒连 ~3.1s）。
- 变异链顺序：V149(01:33) → V150(01:36) → V151(01:39) → V152(01:41) → V155(01:45) → V153(01:48) → 降级(01:49)，全部单编辑单重建，源码逐个 git checkout 还原。
- 01:39 V151 变异真跑后 sj_M156 连同其 runs 厲史被一次性清理级联删除（F-4）。
- 01:50 源码全还原 + pristine 重建，env 文件删除，应用回到钥匙串路径（重建后首次读 key 出现钥匙串授权框，属预期——正是本轮 env 模式规避的重建成本）。

## 十、修复批次（2026-11-05，全部实机变异先行证明形状）

| 发现 | 修复 | 提交 | 测试 |
|---|---|---|---|
| D-4 手动「立即执行」被预演闸拦 | `run_scheduled_job` 加 `manual` 参数；预演闸提取 `preview_gate_hits(mode, manual)` 只认到点触发 | 0309e16f | `gate_hits_scheduled_preview_only` |
| M142 崩溃后待答行永久报警 | `sweep_all_working` 收编 awaiting_answer（仅启动清扫，tick 不收） | e43e949d | `startup_sweep_collects_awaiting_answer_too` |
| D-5 旧 acceptance 跨轮沿用 | `upsert_for_session` 开票即清 acceptance（列 + 返回行），计划轮 set_acceptance 紧随写入 | e43e949d | `upsert_clears_acceptance_for_the_new_turn` |
| F-3 失败轮挂 done | `run_scheduled_job` 收尾按结果分流：成功 done / 失败 err | 89ab582f | 全量 lib 回归（2262 绿） |

**未修（有意）**：F-1 待人工真机复核；F-2 流程卫生（测试后清卡）；F-4 待拍板（`delete_scheduled_job` 显式连带删 runs，保留历史与否是产品口径——建议保留）。

实机复验提示：重建后首次读 key 会再弹一次钥匙串授权（新二进制签名），点「始终允许」即可；M157 场景可用任意 preview 档作业点「立即执行」验证真跑。
