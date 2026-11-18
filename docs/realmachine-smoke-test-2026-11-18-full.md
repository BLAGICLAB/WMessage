# 实机冒烟测试方案：上次冒烟结束至今全部改动（旅游计划 / 模型库 / 子 agent / 提醒 / 流断流 / 重复闸 / 审计修复四批）

> **覆盖范围**：上次实机冒烟**跑完**（`a92943ab`，11-15 那轮 16 PASS / 3 部分 / 1 FAIL /
> 2 受限）至今的**全部改动**，37 笔（18 笔代码 + 19 笔文档/方案），分两段：
>
> ① **v0.3.0 后七批 46 笔**（`1b102e90..ab30e13c`，2026-11-08 → 11-15）：
> 旅游计划修复（2h24m 空转根因链）/ 模型库抗抖 + 离线种子 / 子 agent 全链收官 /
> 定时提醒 / 流断流修复 / 重复调用死循环闸 —— 该批已有一份方案
> （`docs/realmachine-smoke-test-2026-11-15.md`，M182–M203 + W25–W27 + V172–V192）
> **但从未跑过**，本份把它的卡**并入**执行，不再跑两份。
>
> ② **11-16 ~ 11-18 的 18 笔代码改动**：MCP elicitation 能力声明与应答回声脱敏
> （`534dfc46` / `dd67a8cd`）、11-16 审计修复批（`ea108952..0b3d2265`：MCP 机密迁移前置 /
> OAuth 登出接进设置页 / 终稿自检 verdict 消费顺序 / 过期作业关开关期 Held /
> mutating 登记 / think 流去重 / 同步 IO 挪出 async worker / MCP 槽位自愈 /
> 树杀兜底 / 提醒广播 / 死代码清零）、审计四批修复 25 笔（`ddc00174..21c647f5`：
> OAuth 派生端点逐 URL 过闸 / 凭据文件 0600 / 公网闸 IPv6 过渡段收紧 /
> 重定向按 RFC 切方法 / 守卫出处池与证据池 / 工作项与 turn_record 时序 /
> executor 工具行动态渲染 / 树杀先拍树再收尸 / Windows Job 失败留痕）。
> 逐条处置状态见 `docs/agent-audit-2026-11-17.md`。
>
> **卡片**：M182–M230（49 张主卡）+ W25–W29（5 张 Windows 专章）；变异注入锚点
> **V172–V222**（51 条，编号接 11-07 方案的 V158–V171；卡片号 M182 接 M181）。
>
> **配套验收卡**：ACC-O-13（SPEC 6-128 重复闸）、ACC-P-22（SPEC 6-127 提醒接定时模块）、
> ACC-E-05（语义性失败不降级 + 重试按钮判据）；执行完毕回填
> `docs/acceptance/matrix.csv` 对应行状态。
>
> **门禁前置**：基线 `21c647f5` 上全量门禁绿（2026-11-18 实测：cargo test 2414 +
> vitest 1252 + pytest 审计对拍 58/58）；实机前重跑 `bash scripts/test-all.sh`
> 确认仍绿。**Mimosa 静态面**：本批 27 笔提交均未拿到完整扫描结论（钩子每次报
> `scanner_enobufs`），执行前建议补一次完整深扫。
>
> **编号口径**：Windows 专章从 W25 起（W22–W24 已被 11-07 方案占用），变异从
> V172 起，卡片从 M182 起。

---

## 〇、跑法与口径

- **基线**：`21c647f5`（或其后的修复提交）。跑前 `git log --oneline -1` 记录实际基线。
- **覆盖范围**：上次实机冒烟跑完（`a92943ab`）至今的全部改动，37 笔：
  ① **11-15 方案那批七批 46 笔**（`1b102e90..ab30e13c`）：旅游计划修复 / 模型库抗抖 +
  离线种子 / 子 agent 全链收官 / 定时提醒 / 流断流修复 / 重复调用死循环闸——
  该方案已落盘但**从未跑过**（M182–M203 + W25–W27 + V172–V192），本份并入执行；
  ② **11-16 ~ 11-18 的 18 笔代码改动**：MCP elicitation 能力声明与应答回声脱敏、
  11-16 审计修复批（`ea108952..0b3d2265`）、审计四批修复 25 笔
  （`ddc00174..21c647f5`，含 OAuth 派生端点逐 URL 过闸、凭据文件 0600、公网闸 IPv6
  过渡段收紧、重定向按 RFC 切方法、守卫出处池与证据池、工作项与 turn_record 时序、
  executor 工具行动态渲染、树杀先拍树再收尸）。
- **数据口径**：测试阶段老数据可删（用户拍板）；直写 sqlite 时间戳列一律毫秒整数
  （`reminders` 表的 `remind_at` / `fired_at` 都是 INTEGER）。
- **杀进程**：macOS `pkill -x wmessage`（禁 `pkill -f`）；Windows `taskkill /IM wmessage.exe /F`。
- **定位数据目录**：启动后 bot.log 首部 `app_data_dir` 审计行。判据落在 data_dir 下：
  `bot.log`（全部事件证）、`wmessage.db`（`work_items` / `subagents` / `bot_messages` /
  `reminders` / `scheduled_job_runs` 表）、子 agent 产物目录（`gen_dir/<id>/`）。
- **审计证**：本轮全部事件证在 **bot.log 文件行**（`[ts] LEVEL | event | k=v`），不在
  sqlite——grep 模板见 §十。事件名断言用 `grep -c` 计数。
- **断网构造**：macOS 关 Wi-Fi 即可；模型库种子卡（M188）需要**启动期断网**，
  恢复网络后走设置页手动同步。流断流卡（M197/M198）需要**回合中途断网**——
  发消息后数 2 秒断网即可触发读侧失败。
- **无灰度开关**：两批改动全部无 bot-config 开关（重复闸恒开、提醒恒开、OAuth 过闸
  恒开、公网闸 IPv6 收紧恒开、重定向切方法恒开、守卫口径恒开）——对照面靠豁免表、
  默认路径卡与「字面量级」负向卡背书。
- **重复闸误拦处置**：实机若发现合法工具被拦（轮询类不在豁免表），把工具名补进
  `src-tauri/src/bot/constants.rs` 的 `REPEAT_GATE_EXEMPT_TOOLS` 后重启，并在
  回填位备注——豁免原则=纯读/轮询/状态探测。
- **结果回填**：每卡 PASS / FAIL / 部分 + 证据，填 §九回填位；FAIL 带变异锚点编号。
- **时间预算**：49 张主卡约 8 小时（OAuth 章最慢，要起真 MCP 服务器）；变异复跑 51 条
  （各需一次重建 + 复跑）另计 9 小时；M190（真熔断构造）与 M192（续跑全链）单卡各
  预留 15 分钟。

---


## 一、旅游计划修复回归（2h24m 根因链的实机面）


> 根因链：check 阻塞 600s 整档睡眠跨 deadline（实等 19 分钟）→ 期间主回合零
> 流式事件 → 前端 dead-man 看门狗（`STREAM_DEAD_IDLE_MS` = 660_000）判死并自动
> /stop → 子 agent 跑完经汇报轮又续上。本域三卡逐环复验。

### M182 check 等待预算钳制 + 心跳不误杀（主卡）
- **步骤**：派一个耗时 ≥3 分钟的子 agent（如「调研 X 并写一份 500 字小结到产物」），
  主会话让它 `check_subagent` 且 `waitMs` 给足 600000；围观整个等待期不操作。
- **判据**：
  1. bot.log 的 `subagent_check_wait` 行：`waited_ms` **≤** `wait_ms`（钳制后上界
     封死；修复前实测 1156s/600000）；
  2. 等待期内每 60s 有一条 `bot-check-heartbeat` 广播（前端事件流可见；观感 =
     回合在途指示持续、无「已停止」横幅）——660s 看门狗不再判死；
  3. 等待期结束时子 agent 行转终态（`subagent-finished`），汇报轮回灌结果。
- **变异锚点**：V172、V173。

### M183 /stop 级联取消在跑子 agent
- **步骤**：同 M182 派长任务，在子 agent 在跑期间（主回合在 check 等待或已回话
  均可）对主会话点停止（`/stop`）。
- **判据**：
  1. 子 agent 行转 `cancelled`（右侧栏与聊天内嵌区同步）；Q2 查 `subagents` 表
     status 列佐证；
  2. 子 agent 产物目录不再增长（runner 令牌停机，不烧完整份预算）；
  3. `check_subagent` 等待期被 /stop 打断时等待立即返回（不被 10 分钟锁死），
     退出原因落 `subagent_check_wait`（`stopped`）；
  4. **其他会话**的在跑子 agent 不受影响（级联只认本会话）。
- **变异锚点**：V174。

### M184 汇报轮放开提问（半实机）
- **步骤**：派子 agent，目标里显式要求「完成后向用户确认一件事再收尾」——
  让汇报轮的模型有理由调 `ask_user`（如「调研完先问我要不要加附录再交」）。
- **判据**：
  1. 汇报轮（子 agent 终态后的自动续轮）里 `ask_user` **不被拦**——聊天页弹出
     问卡（修复前三连拒「当前会话未开放提问」）；
  2. 应答后汇报轮继续收尾，回报正文落库（`bot-chat-report` 定稿气泡）；
  3. 提问预算默认 3 不变（连问第 4 次被拦，属既有口径）。
- **诚实登记**：汇报轮是否调 ask 取决于模型判读，构造不出时标「环境受限」，
  L1 锚点（注册覆盖面 User | Report + 成对注销）背书。
- **变异锚点**：V175。

### M185 搜索引擎互斥单开
- **步骤**：设置页 → 搜索引擎，依次开 Tavily（此时 DeepSeek/Brave 关）→ 再开
  DeepSeek → 观察 Brave；手改 bot-config 制造双开残留后重启对照。
- **判据**：
  1. 开 DeepSeek 的**同一次保存**里 Tavily 自动变 false（UI 即时反映，无需手动关）；
  2. 任何时刻至多一个引擎开——运行期搜索不再报「多引擎双开」错；
  3. 残留双开（手改配置）重启后：保存任意一项即被自动互斥拉平；`conflictText`
     只作兜底提示且文案说明自动互斥行为；
  4. `deepseekFetchEnabled`（抓取通道）不受互斥影响，可与任一引擎并存。
- **变异锚点**：V176。

### M186 中栏最窄块换行不压字（视觉卡）
- **步骤**：窗口缩到最小宽度（950px），派一个带子 agent 行与工具折叠的回合，
  围观执行过程区；再切到有「停止」键在途的回合。
- **判据**：
  1. 子 agent 行 / 工具折叠标题 / 活体计时**块内不断行**、块整体折行——停止键
     不压字、Σ 总耗时不被截断；
  2. 折行后无横向滚动条；块与块之间留白正常。
- **无变异锚点**（纯前端排版，无头 Chrome 量排版法已录 L1）。

---


## 二、模型库抗抖 + 离线种子


### M187 同步失败的可诊断性（err_chain / 重试 / 60s）
- **步骤**：设置 → 模型库，断网（或 hosts 屏蔽 models.dev）点「同步」；观察
  失败提示文案与耗时；恢复网络再点一次。
- **判据**：
  1. 失败文案带**根因链**：`models.dev 请求失败：<source 链>`——DNS 失败 /
     TLS 握手 / 连接被重置三种断法文案可辨（修复前只有顶层 `error sending request`）；
  2. 请求失败自动退避重试 2 次（观感 = 失败前有明显等待；严格计时 L1 已钉）；
  3. 整请求超时 60s（慢路由半路不再被 30s 掐断——断网场景与超时场景文案分层）；
  4. 恢复网络后同步成功，厂商网格刷新。
- **变异锚点**：V177。

### M188 离线种子兜底（启动期断网也不空）
- **步骤**：**断网**状态下冷启动应用（先 `pkill -x wmessage`）；直接看模型库
  厂商网格与「从模型库添加」按钮；然后恢复网络点手动同步。
- **判据**：
  1. 断网启动后厂商网格**非空**（种子 235 厂商 8935 模型兜底落库），添加按钮
     在场——三症状（更新失败 + 网格缺 + 按钮消失）不再同现；
  2. bot.log 有**一条** `meta_seed_models_dev`（WARN，空表才播种种子的实锤）；
  3. 手动同步失败也兜底落库，错误文案注明「已用内置快照兜底」语义；
  4. 恢复网络手动同步成功后网格为在线数据（种子被刷新覆盖）；
  5. **非空表不重复播种**：正常重启（在线）无第二条 `meta_seed_models_dev`。
- **构造备选**：不想断网可直写 sqlite 清空 `meta_provider` 表再冷启动（注意
  先备份 db）。种子只含解析器消费字段——个别厂商展示字段可能缺，判据只看
  厂商行数 > 0 与按钮在场。
- **变异锚点**：V178。

---


## 三、子 agent 全链收官（空转 / 冗余重派 / 熔断遗留的实机面）


> 本域是子 agent 全链审计（20 发现修 18）的实机面。构造成本从低到高排列，
> M190 的真熔断最难构造，可标受限。

### M189 verifier 交付件齐即收尾（正向停机）
- **步骤**：派一个带 3 条验收标准的子 agent（如「写产物 report.md + 逐条核验
  落 verified.json」），目标体量控制在 5–10 分钟能交付；围观收尾行为。
- **判据**：
  1. 验收报告（`checks` 数 ≥ 需自评判据数）落盘后，模型循环当轮末注入一次
     「立即收尾」提醒——bot.log 有**一条** `subagent_deliverable_nudge`（不刷屏）；
  2. 注入后子 agent 在数轮内交卷（`submit_result` 或合法收尾 JSON），不再
     「报告出完仍磨 17~20 分钟到人手动终止」；
  3. 报告判定只认**本轮开跑后**新写的名字含 verif 的 .json（产物目录与
     AI_Gen_Files 根两处探）——上一轮任务的陈年 verified.json 不触发；
  4. 提示词正向纪律生效：收尾语可见「交付件齐即收尾」语义（观感级）。
- **变异锚点**：V179。

### M190 熔断出口恢复链（剥标记 / 强制交卷 / 续跑）
- **步骤**：构造真熔断——派一个注定超 200 轮安全网的目标（如「逐个读取
  指定目录下 300 个文件并各写一行摘要」），等其撞 `budget_exceeded`；围观
  熔断后的收尾面；随后对熔断行调 `resume_subagent`（见 M192）。
- **判据**：
  1. 熔断瞬间若正文带熔断标记：恢复链自动起一轮恢复——bot.log 有
     `subagent_fuse_recovered`（带 `via`：`resumed` / `resumed_disk_verdict` /
     `resumed_forced_submit` / `resumed_fuse_tail` 之一）；
  2. 恢复成功后收尾正文是**可解析的交卷 JSON**（`result_json` 有结构化字段，
     非 800 字熔断残留）；恢复轮不进 usage 记账（水位条无异常跳动）；
  3. 恢复失败路径：正文交不出合法 JSON 时行仍 `budget_exceeded`，但
     `session_history.json` sidecar 已落（bot.log 有 `subagent_history_sidecar`），
     供 M192 续跑；
  4. 非熔断收尾的行：sidecar 被清（`subagent_history_sidecar_cleared`）——
     残留旧文件不让下一次同名误判。
- **诚实登记**：真熔断构造耗预算、慢；若超时可先用停滞强停行验判据 3/4
  （但 resume 只认 `budget_exceeded` 行，判据 1/2 需真熔断）。
- **变异锚点**：V180。

### M191 submit_result 交卷工具化
- **步骤**：M189 的子 agent 收尾时观察其是否走 `submit_result` 工具；另开
  主会话直接诱导模型调 `submit_result`（如「把刚才的结果用 submit_result 交了」）。
- **判据**：
  1. 子 agent 调 `submit_result` 成功即回合结束（无额外收尾文本轮）——行转
     `succeeded`，`result_json` 收到 status/summary/checks/artifacts 结构化字段；
  2. **主会话调用被拒**：「submit_result 仅子 agent 执行会话可用」（会话闸）；
  3. 工具描述含 hint 键位指引（verifier 专用 `checks` 键在 schema 描述里点名，
     模型交卷时用得上）；
  4. 交卷成功后 `check_subagent` 立即读到终态（无 running 残留窗口）。
- **变异锚点**：V181。

### M192 resume_subagent 跨进程续跑
- **步骤**：M190 拿到一个 `budget_exceeded` 行（sidecar 在场）；先试**非熔断行**
  （M189 的 succeeded 行）调 `resume_subagent` 对照；再对熔断行续跑，等完成后
  检查新会话行为与配置档。
- **判据**：
  1. succeeded / failed / cancelled 行续跑被拒（「只有 budget_exceeded 行可续跑」
     语义的人话错误）；在跑 / 排队行调用也被拒；
  2. 熔断行续跑：bot.log 有 `subagent_resume`（带历史消息条数与配置档名），
     行翻回 `queued`、同 id 重入派发管线；追加 10 轮预算（`RESUME_ROUNDS`）；
  3. **续跑行携带原会话配置档**（onion 元列搬运）：新会话的工具白名单收窄与
     首跑一致（validator 档的子 agent 续跑后仍无写权限——本条是审计最大缺口
     的实机面）；
  4. runner 见 sidecar 进续跑模式：装载全量历史继续（不是从零重派），完成交卷；
  5. 续跑完成后 sidecar 被清；再次 `resume_subagent` 报「没有可装载的续跑历史」。
- **变异锚点**：V182、V183（**V183 必须 KILLED**——配置档静默丢失是本轮最重缺口）。

### M193 终态真实回读 + 机器验收豁免收口
- **步骤**：M190 熔断构造期间，在熔断恢复 / cancel 抢跑窗口围观行终态；另派
  一个带机器可判定标准（`file_exists:<路径>`）的子 agent 正常收尾对照。
- **判据**：
  1. cancel 抢先窗口不再误报 `failed`——被取消的行终态是 `cancelled`（收尾
     闭包失败后回读**真实**终态，不猜）；
  2. 机器验收达成数只认干净行：任一熔断标记 / 停滞强停的行**不计入**验收
     统计（修复前预算残行冲低达成数）；
  3. 续跑引导提示词按行状态判据（不再按 error 串排除工具熔断行）——观感级：
     resume 前的引导文案对工具熔断行同样给出可续跑指引；
  4. 机器标准逐条通过时照常推翻 `result_json_unparseable` 判成功
     （`subagent.parse_failure_recovered` 在案，回归确认不破）。
- **变异锚点**：V184。

---


## 四、定时提醒


### M194 提醒页签端到端：新建 → 到点通知 → 一次性删行
- **步骤**：定时页切到「提醒」页签，新建一次性提醒（`at:` 串，2 分钟后）；
  等到点；查库与通知。
- **判据**：
  1. 页签切换正常：任务 / 提醒两组条目各自渲染（`ScheduleEntry` 判别联合——
     Job 与 Reminder 字段集各自成立，前端不串字段）；
  2. 到点弹**系统通知**（内容 = 提醒原文）；`schedule_overview` 里该条目的
     `nextRunAt` 来自库列 `remind_at`（不现算）；
  3. 一次性提醒触发即删：Q1 查 `reminders` 表该 id 消失；`firedCount` /
     `oneShot` 标记在触发前可见（一次性行没有「下次可暂停」语义）；
  4. 通知入队走 Effects 队列（`reminder:{id}` 键）——入队失败行还在、下轮
     tick 重做不丢提醒（`reminder.enqueue_failed` 只在真失败时出现）。
- **无变异锚点**（判据 3 的库证 + 通知观感已双验；写路径闸在 M195）。

### M195 周期提醒：暂停 / 改期 / 过期丢弃
- **步骤**：新建每日提醒（`daily:HH:MM`，取 3 分钟后的时刻）；触发一次后在
  UI 暂停它；再改一次时间（往后拨一天）；最后手工把库列 `remind_at` 改到
  过去很远（3 天前）再等一轮 tick。
- **判据**：
  1. 触发后 `fired_count` +1、`fired_at` 更新、`remind_at` 推进到明日同刻
     （advance 与通知入队**同一事务**——通知失败行不推进）；
  2. 暂停（`reminder_set_enabled` false）后不再触发；**重新启用时 `remind_at`
     重算**（不按暂停前的旧点补响）——RMW 基线保护：调度器期间推进过这条
     提醒的并发写被拒（改规则时下次触发时刻跟着重算）；
  3. 改期（`reminder_update`）后 `remind_at` 立即重算到新时刻；
  4. `remind_at` 落后超过一次性补提醒宽限（`ONE_SHOT_GRACE`）→ bot.log 有
     `reminder.expired`（丢弃不补），周期行照常推进不炸。
- **变异锚点**：V185。

### M196 页标题「定时」+ 通知未授权提示
- **步骤**：看定时页标题；macOS 系统设置里关闭本应用的通知授权，回应用点
  提醒页签的「重新检查」；再回系统设置开授权后重查。
- **判据**：
  1. 页标题显示「定时」（非「任务」/「调度」旧文案）；
  2. 通知未授权：**提醒页签顶部**出现提示条「系统通知未授权——提醒到点不会响」
     + 「重新检查」按钮；任务页签**不出现**该提示（作业不靠系统通知）；
  3. 授权后点「重新检查」→ 提示条消失；
  4. 提示条只在提醒页签挂载期检查（不阻塞页签切换）。
- **无变异锚点**（纯前端条件渲染，组件测试已钉）。

---


## 五、流断流修复（Windows 实机诊断问题的 mac 复验面）


> 三层叠加：传输层响应体截断（`Kind::Decode`）被 `to_string()` 吞成顶层文案；
> `emitted_any` 恒真使重发判据在开思考时死代码；读侧失败 blanket `Internal`
> 让用户只见「内部错误」。mac 侧复验行为面，Windows 专属面在 W25/W26。

### M197 断流文案分层 + err_chain 审计
- **步骤**：发一条长回复的话，回复进行到一半（正文已上屏）时断网；等失败；
  看气泡错误文案与 bot.log。
- **判据**：
  1. 用户文案分三层：**现象**（本轮中断）+ **成因**（读取超时 / 连接失败 /
     响应传输被截断之一；idle 超时单独「大模型长时间没有响应」）+ **本轮未完成**
     ——原始串（`reqwest` 报错文）与「内部错误」字样**不出现在**用户可见文案；
  2. 自动重试发生过时文案末尾带「已自动重试 N 次仍失败」；
  3. bot.log 的 `llm.stream_failed` 行带 `err` / `err_chain`（逐层展开 source
     链到 hyper 真因）/ `bytes` / `elapsed_ms` / `phase` 五个字段（诊断留痕）；
  4. 失败语义可恢复：气泡有重试入口（衔接 M199）、不是「不可自动重试；请反馈日志」。
- **变异锚点**：V186。

### M198 开思考时断流仍能重发 + 清场不叠影
- **步骤**：设置里**打开思考**（reasoning 开），重复 M197 的中断构造；失败后
  点重试按钮（若已跑 M199）或发新消息续轮。
- **判据**：
  1. 开思考时流中断**仍走自动重发**（修复前 `emitted_any` 被思考增量置恒真、
     `llm.stream_retry` 是死代码）——bot.log 有 `llm.stream_retry` 计数行；
  2. 重发前 `bot-turn-restart` 清场：前端思考段 / 工具 / 正文回到本轮起点
     （无叠影、无「半段思考 + 新半段」拼接错乱）；库证：Q2 该回合终稿行的
     thinking / tools 不含重发前残留；
  3. 重发成功后回合正常收尾；再失败则按 M197 文案分层落。
- **变异锚点**：V187、V188。

### M199 失败气泡重试入口
- **步骤**：连发两条消息（第一条成功、第二条断网失败）；先对**第一条成功气泡**
  长按 / 找入口对照；再对第二条失败气泡点「重试」。
- **判据**：
  1. 重试按钮只出现在**末条失败气泡**且其前有用户行（旧失败气泡、成功气泡、
     无前置用户行的失败气泡都不渲染）；
  2. 点击后走 `/retry` 截尾重放：失败气泡及其后内容被删、重放其前最近一条
     用户行（同 id 重放），重跑成功后历史连续；
  3. 重试过程沿用 M198 的清场（无叠影）；旧气泡重试不可用（不出现入口即是判据）。
- **变异锚点**：V189。

### M200 ort 预检 + 启动自检（mac 成功路径）
- **步骤**：正常启动应用（mac 上动态库恒可用），看通知中心与设置页记忆区；
  对照启动前后历史告警的收拢。
- **判据**：
  1. mac 正常路径：无 `memory.embed_engine_unavailable`、无服务告警（预检恒
     Ok——预检只在 Windows 缺 dll 场景拦截，见 W25）；
  2. 启动自检**成功收掉历史告警**：此前残留的记忆服务告警（若有）被清；
  3. 加载侧 `catch_unwind` 兜底在位（代码面已 L1；实机只需确认记忆语义检索
     正常——问一条能命中记忆的问题，检索有结果）；
  4. 记忆 / OCR 两侧共用同一份预检结论（一侧确认不可用不拖死另一侧启动）。
- **变异锚点**：V190（失败路径变异，实机主要在 W25 执行）。

---


## 六、工具重复调用死循环闸（repeat gate）


> 闸语义：同一工具以**完全相同参数**（JSON 规范化按键排序）连续调用达 3 次
> （`REPEAT_GATE_MAX_IDENTICAL`）即拦截第 3 次，role=tool 拒文回填、不计数、
> 不进失败链；可提问语境先转问（`REPEAT_GATE_ASK_TIMEOUT_SECS` = 300s），
> 每回合至多一问。豁免表 20 工具（纯读/轮询/探测）。

### M201 同参 3 连调拦截 + 豁免对照
- **步骤**：诱导模型同参连调——发「用 create_task 建一条名为 X 的任务，建三次
  （内容完全一样）」之类诱导，或让其连调一个非豁免写工具 3 次；对照发
  「连查 5 次任务列表」（`query_tasks` 在豁免表）。
- **判据**：
  1. 第 3 次同参调用**未执行**，模型收到 `⛔ 死循环闸` 拒文（点名工具名、连击
     数、两条出路：换路或如实收尾）；bot.log 有 `repeat_gate`（WARN，kv 含
     `tool` / `consec=3` / `round` / `action=reject`）；
  2. 换参数（哪怕改一个键）即放行——闸只认完全相同参数；
  3. 豁免工具连查 5 次全部执行、无 `repeat_gate` 行（轮询合法语义不受拦）；
  4. 闸拒不进失败链：拒后模型正常换路收尾，回合不落 failed、无 fail_hint
     双重提示；
  5. 一次会话内跨提问自然清零：换个新问题重新连调，计数从头起。
- **变异锚点**：V191。

### M202 可提问语境转问三态
- **步骤**：主聊天（交互、在窗口前台）重复 M201 的诱导，第 3 次撞闸时**停留
  不答**超过 300 秒，观察问卡超时行为；重跑一轮这次**及时点「继续再试一次」**；
  再跑一轮点「就此打住」。
- **判据**：
  1. 第 3 次撞闸先弹**问卡**：问题点名工具与连击数，三选项「换方法重试 /
     继续再试一次 / 就此打住」，假设为「就此打住」——与 ask 卡行式 UI 一致；
  2. 答「继续再试一次」：重试放行（连击重置），bot.log `action=answered`，
     模型收到裁决原文回填；**同回合第二次撞闸不再问**（每回合至多一问），
     直接拒文；
  3. 300s 超时无人应答：按假设降级——模型收到「转问未获应答」拒文
     （`action=fallback`），回合不再挂 24 小时；
  4. 子 agent / 无人值守（定时作业）语境**不弹问卡**直接拒文（转问出口只在
     主聊天交互会话装配）。
- **变异锚点**：V192（**必须 KILLED**——超时回落若改放行原参，死循环闸形同虚设）。

### M203 卡住升级口径三处点名（观感级）
- **步骤**：诱导一个容易卡住的任务（如「反复尝试打开一个不存在的文件路径直到
  成功」），围观模型在连续失败 / 卡住时的自发行为。
- **判据**：
  1. 主提示词、任务卡执行提示词、相关工具描述三处均含「卡住就问 ask_user」
     口径（提示词面 grep 可验，实机看模型自发提问行为改善为观感级）；
  2. 模型在卡住时**有概率**主动发问卡而非静默磨（不强制判定成功——提示词
     引导是概率面，闸是硬面，两者互补）。
- **无变异锚点**（提示词面，无确定性注入点）。

---


## 七、Windows 专章


- **W25 ort 三道防线（真加载自检 + catch_unwind + 永久降级）**：Windows 上
  临时改名为 `onnxruntime.dll`（模拟缺失，跑完还原）→ 启动：预检真加载失败
  取 Win32 错误码（126 = 缺 VC++ 运行库 / 127 = 找不到指定程序），启动自检投
  **一次性**通知中心服务告警（`memory.embed_engine_unavailable`，带
  reason/startup 字段），bot.log 一条不刷屏；记忆/OCR 转**永久**关键词降级
  （二次调用不再尝试加载、不 panic、不毒化全局 Mutex——修复前 ort `expect`
  panic 会让记忆子系统整进程失效）；恢复 dll 后重启自愈。**缺 MSVC 运行库的
  真机**（无 VC++ Redistributable）场景同样只允许降级告警，不允许崩溃。
- **W26 流断流 Windows 面**：Windows 上复跑 M197/M198（断网卡）——重点验
  「响应传输被截断」成因归类（`Kind::Decode` 是 Windows 实机高发形态）在
  用户文案的呈现，以及失败气泡重试按钮（M199）在 Windows 窗口下的可用；
  换 exe 复验注意 SHA256 与路径两处记账（出包手册口径，本轮不主动出包）。
- **W27 提醒与通知授权（Windows）**：M194/M196 在 Windows 复跑——Windows
  通知授权语义与 macOS 不同（系统级「通知」开关），关授权后提醒页签提示条
  与「重新检查」行为一致；`reminders` 表在 `%APPDATA%` 数据目录下读写正常
  （中文用户名 / 长路径各试一次新建提醒）。

---


## 八、变异注入锚点总表（V172–V222）

前 21 条（V172–V192）来自 11-15 方案那批七批 46 笔；后 23 条（V193–V215）来自 11-16~11-18 的审计四批修复。

| 锚点 | 变异注入 | 必须被哪张卡逮住 |
| 锚点 | 变异注入 | 必须被哪张卡逮住 |
|---|---|---|
| V172 | check 等待步长钳制删除（`min(step_ms, 剩余预算)` 改回整档直睡） | M182（waited_ms ≈ 2× 预算，判据 1 红） |
| V173 | 心跳不发（`bot-check-heartbeat` 改恒 None） | M182（660s 看门狗判死并自动 /stop，判据 2 红） |
| V174 | stop 级联丢会话过滤（`load_live_subagents_by_session` 换全表扫描） | M183（他会话子 agent 被误取消，判据 4 红） |
| V175 | `ask_registered` 回退只认 User（摘掉 Report 分支） | M184（汇报轮 ask_user 三连拒） |
| V176 | 搜索引擎互斥删（toggle 不随同关其余引擎） | M185（双开保存成功、运行期搜索报错） |
| V177 | err_chain 改回 `to_string()`（丢 source 链） | M187（三种断法文案同形，判据 1 红） |
| V178 | 种子兜底删（空表直接在线刷新、失败即弃） | M188（断网启动网格空 + 按钮消失，三症状复活） |
| V179 | nudge 新鲜度判据删（陈年 verif*.json 也算本轮交付） | M189（旧报告触发「立即收尾」，判据 3 红） |
| V180 | 恢复链不剥熔断标记（原文交下游 classify） | M190（交卷判 failed / result_json 不可解析） |
| V181 | submit_result 成功不翻行终态（工具 Ok 行仍 running） | M191（check 读不到终态，判据 4 红） |
| V182 | 续跑成功不清 sidecar | M192（终态行残留 sidecar、二次 resume 误判可续跑，判据 5 红） |
| V183 | resume 派发待办丢 onion 元列 | M192（**续跑行配置档静默收窄丢失——审计最大缺口，必须 KILLED**，判据 3 红） |
| V184 | 机器验收豁免放宽（熔断标记 / 停滞强停行计入验收） | M193（残行冲低达成数，判据 2 红） |
| V185 | 暂停/启用不重算 `remind_at`（定点更新删） | M195（重启用后按旧点补响，判据 2 红） |
| V186 | `llm.stream_failed` 的 err 字段回退顶层串（err_chain 删） | M197（诊断面退化，判据 3 红） |
| V187 | 重发判据回退 `emitted_any`（思考增量毒化复活） | M198（开思考断流不重试直接失败，`llm.stream_retry` 消失） |
| V188 | `bot-turn-restart` 清场不删 thinking | M198（重发后思考段叠影，判据 2 红） |
| V189 | 重试按钮挂全部失败气泡（不判末条 + 前置用户行） | M199（旧气泡重试截掉新轮次，判据 1 红） |
| V190 | ort 预检删（回 `expect` panic 链） | M200/W25（记忆子系统整进程失效 / 全局 Mutex 中毒） |
| V191 | 拦截阈值 3→5（或豁免表清空） | M201（第 3 次照常执行 / 豁免轮询被拦，判据 1/3 红） |
| V192 | 转问超时回落改放行原参（fallback 分支删） | M202（**无人应答死循环继续——必须 KILLED**，判据 3 红） |
| 锚点 | 变异注入 | 必须被哪张卡逮住 |
|---|---|---|
| V193 | OAuth 发现第二跳 root 复用钉原 host 的 client（去掉逐 URL 过闸） | M204（登录行为变）/ M205（内网请求被发出，判据 2 红） |
| V194 | token endpoint 不再过闸（复用原 client 直发） | M205（请求打到内网，判据 2 红） |
| V195 | `authorization_endpoint` 的字面量级闸删（scheme/私网不拦） | M205（`file://` 或私网 authorize URL 被放行，判据 3 红） |
| V196 | 降级凭据文件回退 `atomic_write`（0644） | M206（权限不再是 0600，判据 2 红） |
| V197 | 登出回到 `let _ =` 吞错 + 无条件记成功 | M206（钥匙串故障时仍记成功、用户无提示，判据 3 红） |
| V198 | 重定向不分派方法（四种全保持） | M208（301/302/303 收到 POST + body，判据 1 红） |
| V199 | final_url 只裁查询串、不裁 userinfo | M209（bot.log 出现 `pass`，判据 1 红） |
| V200 | `sink_args_preview` 不给 fetch_url 特裁 | M210（入参预览带查询串，判据 1 红） |
| V201 | fetch_url 自己的 `audit_log` 不裁 URL | M210（日志带查询串，判据 1 红） |
| V202 | 公网闸 IPv6 过渡段判定移除 | M211（三条过渡段地址放行，判据 1 红） |
| V203 | 出处池不排除「【系统提示】」前缀 | M212（模型复述返工提示里的数即放行，判据 2 红） |
| V204 | 证据池只留返回文本头部（回到 `chars().take`） | M213（引用尾部数字被判无出处，判据 1 红） |
| V205 | 工作项闸去掉 origin（汇报轮也开票收尾） | M214（失败信号被唤醒又收成 idle，判据 2 红） |
| V206 | `turn_record::begin` 移回模型循环内部 | M215（失败行拼进上一轮汇报轮正文，判据 1 红） |
| V207 | `/clean` 不广播 `bot-msg-dropped` | M216（第二窗口不收敛，判据 2 红） |
| V208 | 槽位插入不核对启用快照 | M217（幽灵工具条目每轮出现，判据 1 红） |
| V209 | 行长闸恢复「一视同仁跳过」（不判 JSON-RPC） | M218（大响应被跳过 → 干等超时，判据 1 红） |
| V210 | 树杀顺序倒回（先 wait 收尸再树杀） | M219（node 孤儿占端口，判据 1/2 红） |
| V211 | elicit 解析失败文案回显用户输入 | M207（错误链把输入带进审计，判据 3 红） |
| V212 | 回声指纹不登记 JSON 转义形态 | M207（转义形态的应答值漏进审计，判据 1 红） |
| V213 | executor 工具行改回写死 writereditor 档 | M221（显式换档时清单与白名单脱节，判据 2 红） |
| V214 | 滑动窗桩子文案改回「完整结果已存档」 | M213（文案与实际钳制不符，判据 2 红） |
| V215 | Windows Job Object 创建失败不留痕 | W28（无失败审计，判据 4 红；需 Windows 实机） |
| V216 | 终稿自检 verdict 不消费（快照还原后重复消费） | M224（同一回合重复触发返工，判据 1 红） |
| V217 | 子 agent 成功终态不清残留 error | M224（成功行仍挂旧失败原因，判据 2 红） |
| V218 | 过期一次性作业在开关关闭期直接删（回到不看总开关的 Delete） | M225（作业凭空消失，判据 1 红） |
| V219 | `bot-think-delta` 去掉 noteSeq 去重闸 | M225（思考文本重复拼接，判据 3 红） |
| V220 | 子 agent 提示词工具清单与白名单脱节（回写死档案位） | M226（模型照着调被拒，白烧一轮） |
| V221 | 种子兜底成功分支不留痕（回到只有 Err 才打） | M229（无 `meta_seed_models_dev`，判据 1 红） |
| V222 | elicitation 桥接不声明 form 能力（回到只覆写 create_elicitation） | M228（服务器从不发起反问，判据 1 红——真实服务器从不反问即症状） |

## 九、结果回填位

上半部是 11-15 方案那批卡（M182–M203 / W25–W27），下半部是本轮新增卡（M204–M223 / W28–W29）。

| 卡 | 结果 | 证据（事件行 / 库证输出） | 备注 |
|---|---|---|---|
| M182 | PASS | `subagent_check_wait wait_ms=600000 waited_ms=196257 exit=terminal`（对比 11-08 旧行 1156579）；心跳源码面 orchestrator.rs:433；S0 等待跨 660s 无假死 | 判据 2 事件面=前端广播不落 bot.log |
| M183 | PASS | `sa_5d64d276\|cancelled\|用户停止`；check 等待 1s 即返；f1_note 未生成；race（他会话）跑到 194 轮 | 4/4 |
| M184 | 部分 | 主聊天 ask 卡端到端活体（`ask_answered action=answer` + 挂卡 3.5min + 回填续跑）；汇报轮场景未及构造（macau 子 agent 随 relaunch 链中止） | ask_registered 覆盖面 L1 背书 |
| M185 | PASS | 开 DeepSeek 同保存 Tavily 自动翻关 + 落盘对拍；双开残留识别 + conflictText 逐字 + 关一保一拉平（tavily=true/deepseek=false） | 抓取通道独立 ✓ |
| M186 | PASS | 950px 实况截图：子 Agent 行/计时块内不断行、停止键不压字、无横滚 | — |
| M187 | PASS | 失败文案根因链五层（…client error (Connect)：tunnel error：tcp connect error：Connection refused os error 61）+「重试 2 次后仍失败」尾注 | 判据 3 六十秒上限观感未占满（连接拒绝瞬时），L1 计时背书 |
| M188 | PASS | 清表 0 → 死代理冷启动种子落库 **235 厂商行**；网格满砖 + 更新按钮在场；手动同步失败库不破坏；在线重启 235→326；seed 事件全程 0 条 | 判据 2 口径出入：事件只在种子失败时打（lib.rs ensure_seed Err 分支），落库实锤=行数 |
| M189 | PASS | `subagent_deliverable_nudge round=29` 恰一条（本轮新写 checks 文件触发）；注入后交卷动作发生（JSON 截断→failed） | 判据 3 陈年文件反例未单独构造，L1 背书 |
| M190 | 受限 | 真熔断行两次构造未产出：MiniMax 并行批+父级拆分+收尾 JSON 截断三连绕开双轴（S5 对首只撞 ~400 工具墙但行被机器验收翻成功） | 恢复链 L1 + 11-08 旧 specimen 背书 |
| M191 | PASS | 子 agent 交卷即终态（13:39:30 submit → succeeded）；主会话拒绝点在**工具清单装配层**（模型自证无此工具）；check 立即读终态（13:28:53→13:29:00） | — |
| M192 | 受限 | 无 budget_exceeded 行可续 | resume 事务原子性 + onion 元列搬运 L1 背书 |
| M193 | PASS | 判据 1：cancelled 非误报 failed；判据 4 活体：`sa_b4dfe8ea succeeded \| result_json_unparseable:机器验收 1 项全通过，按产物判成功`；拒绝面：e613 验收不满足 → failed | 判据 2/3 L1 + 行证据间接 |
| M194 | PASS | `reminder.fired` + 系统通知真弹（通知中心截图）+ 触发即删（COUNT=0）+ 零异常事件 | 4/4 |
| M195 | PASS | 首响推进明日；暂停/恢复 `set_enabled` 双行 + remind_at 重算明日（不补响）；改期立即重算（非基准值）；过期 `reminder.expired 错过 259225s` + 行删；周期行推进不炸 | 4/4 |
| M196 | 部分 | 页标题「定时」✓、双页签渲染 ✓、切换不阻塞 ✓；未授权提示条出现/消失**环境受限**（System Settings 驱动超预算，组件测试 12 例钉渲染逻辑） | — |
| M197 | PASS | 判据 1 实证：失败文案「请求大模型失败：**大模型长时间没有响应**，本轮未完成，可点重试」四段齐、无原始串无内部错误；`llm.stream_idle_timeout bytes=10637 phase=body cause=idle` 诊断字段齐 | 判据 3 变体：RST 即断路径未捕获（切管器不传 RST，构造器局限）；send 层文案未分层 → F-新1 |
| M198 | PASS | 判据 1 实证：思考增量下 `llm.stream_retry attempt=1` 触发（emitted_body 判据活体）；重发撞仍切隧道 → 按 M197 分层落（判据 3 后半） | 判据 2 restart 前端事件不可观测，库证 failed 行无思考残留间接；判据 3 重发成功路径未捕获；596 同场景未触发 retry——重发预算语义登记观察 |
| M199 | FAIL→F-新2 | 读侧失败气泡**无重试按钮**（文案却自指「可点重试」）；根因面 MessageList.tsx retryIdx 判据（投影 turnStatus 未落 failed）；命令面 `/retry` 全链 PASS（截尾+同 content 重放+20s succeeded+库证 ord13 failed→completed 1319 字） | 401 类气泡有按钮（对照在案） |
| M200 | PASS | `memory.embed_engine_unavailable` 全程 0；记忆语义检索命中（教训全文复述截图）；判据 2 空集收敛登记 | — |
| M201 | PASS | `repeat_gate consec=3 action=answered` + 问卡三选项弹出；换参即放行活证（+column）；query_tasks ×5 全执行零闸（26 行日志）；回合 done 收尾 + record_lesson | — |
| M202 | PASS | 问卡三选项 + 假设就此打住；answered 放行 + 同回合二次撞闸 `action=reject` 直拒（10ms 双行）；30s 变异窗 `action=fallback`；子 agent 零 ask 卡 | 判据 3 用 30s 变异法（方案 §十二 点名允许，跑完还原） |
| M203 | PASS | 提示词三处点名 grep 在案；行为观感：一次读败即如实收尾（「循环重试无法成功，所以我停在这里」+三条出路+不伪造） | 概率面两可口径 |
| W25–W27 | 未覆盖 | 本机为 macOS | 环境受限延后 |

**变异执行台账**（V172–V192 补跑于 2026-11-15 晚 ~ 11-16 凌晨，两轮共 20 条，方法同前轮：单编辑→重建→复跑→还原）：

**总结果：20 条实跑 = 19 KILLED + 1 KILLED（经 L1）+ 0 SURVIVED**；另 1 条（V190）本机不可激活，登记随 W25 Windows 轮。

**第一轮（11-15 晚，12 条）**：

| 锚点 | 判定 | 一句话证据 |
|---|---|---|
| V176 搜索互斥删 | **KILLED** | 变异态双开落盘 tavily=T/deepseek=T；pristine 同保存自动翻关 |
| V177 err_chain 丢链 | **KILLED** | 变异态审计行纯顶层串；pristine 带五层链（Connect→tunnel→tcp→refused 61） |
| V178 启动种子兜底删 | **KILLED** | 清表+死代理冷启动 COUNT=0、厂商库网格消失；还原后种子回填 326 |
| V179 nudge 新鲜度判据删 | **KILLED** | gen 根陈年 verif*.json（mtime 退 24h）：变异态轮末注入「立即收尾」；pristine 同场景 0 条 nudge |
| V180 恢复链不剥熔断标记 | **KILLED** | 桩 fuse_submit（交卷 JSON 与「⏹ 已熔断」同轮）：pristine 行终态 **succeeded**；变异态 **budget_exceeded**（真实完成被误报熔断） |
| V181 交卷不立即终局 | **KILLED** | 双队列桩：子回合越过交卷跑到 text 步 → 行翻 failed+unparseable；pristine submit 即 succeeded |
| V182 续跑成功不清 sidecar | **KILLED** | 桩造熔断行→resume 续跑完成：变异态**无** sidecar_cleared 审计行 + 盘上 session_history.json 残留；pristine 明确有清理行 |
| V183 resume 丢 onion 元列（**必须 KILLED**） | **KILLED** | 桩 tools_vary 44.8s 造真熔断行+sidecar+onion=validator；pristine resume 保留 validator、变异态续跑会话 onion_config=**NULL**（配置档静默丢失） |
| V184 机器验收豁免放宽 | **KILLED** | 桩熔断残行（判据 file_exists:ok.txt 不存在）：pristine 残行 result_json **无 acceptance_check**；变异态**带 acceptance_check[{machine:true,met:false}]** |
| V185 提醒 grace 推进删 | **KILLED** | stale 行启用：pristine 静默推进明日零通知；变异态 22s 后 reminder.fired 补响 |
| V186 stream_failed 缺 err_chain | **KILLED** | 桩 break（bytes=298/801ms）：变异态五字段缺 err_chain；pristine 五字段齐 |
| V187 重发判据毒化复活 | **KILLED** | 桩 think_break（纯思考增量即断）：变异态无 stream_retry 单次即败；pristine retry attempt=1 触发 |
| V188 restart 清场不删 thinking | **KILLED（经 L1）** | 实机叠影观测窗秒级未命中（连拍 12 帧）；防线本体由 stream.test「bot-turn-restart 清场」用例锁定 |
| V189 重试按钮挂全部 | **KILLED** | pristine 401 旧气泡（非末条）无重试（裁剪图）；变异态同气泡出现「⟲ 重试」 |
| V191 拦截阈值 3→5 | **KILLED** | 桩同参 create_task ×5：consec 1-4 全放行（4 任务落库）、第 5 次 consec=5 才拦；pristine 第 3 次即拦 |
| V192 转问超时回落放行（**必须 KILLED**） | **KILLED** | 30s 变异窗：超时后 action=answered 放行原参、**第 3 任务落库**；pristine action=fallback 只 2 任务 |

**第二轮（11-15 夜 ~ 11-16 凌晨，8 条）**：

| 锚点 | 判定 | 一句话证据 |
|---|---|---|
| V172 check 等待步长钳制删 | **KILLED** | 桩 spawn 慢子 agent → check(waitMs=300)：pristine `waited_ms=305`（钳制生效）；变异 `waited_ms=1508`（≈5× 预算） |
| V173 心跳不发 | **KILLED** | 等价小阈值窗（看门狗 660s→30s、心跳 60s→5s，跑完还原）：变异态 30s 被 dead-man 判死并自动 /stop（bot_stop 审计行）；pristine 同场景无 bot_stop、等满 wait_ms=45005 |
| V174 stop 级联丢会话过滤 | **KILLED** | 桩双会话路由 + 真实在跑子 agent：A 会话 /stop → pristine A cancelled + **B 仍 running**；变异态 **A、B 双双 cancelled**（误伤） |
| V175 ask_registered 丢 Report 分支 | **KILLED** | 桩三队列（main/sub/report）：变异态汇报轮 ask_user 被拒「当前会话未开放提问」；pristine 放行（ask_user_asked 审计行） |
| V179 nudge 新鲜度判据删（第二轮复核） | **KILLED** | 同第一轮结论 |
| V180 恢复链不剥熔断标记（第二轮复核） | **KILLED** | 同第一轮结论 |
| V182 续跑成功不清 sidecar（第二轮复核） | **KILLED** | 同第一轮结论 |
| V184 机器验收豁免放宽（第二轮复核） | **KILLED** | 同第一轮结论 |
| V190 ort 预检删 | **不可激活（环境受限）** | 失败路径只在 Windows 缺 dll / 缺 MSVC 运行库场景可构造；macOS 动态库恒可用，删与不删行为一致——随 W25 Windows 轮验 |

**第二轮基础设施升级（桩）**：新增 ① `main_routes` 按 body 关键词分派（并发会话各拿一套剧本，V174 必需）；② `fuse_submit` 步型（交卷 JSON 与熔断标记同轮，V180 必需）；③ 探针请求（body < 2KB 非流式）不再消费剧本队列；④ 汇报轮路由特征（REPORT_TURN_PROMPT 首句）；⑤ `/stop` 斜杠命令需双回车（命令选择器吃第一次回车）。

**变异写法教训（两条，本轮踩到）**：
1. SQL 变异「去条件但保留 `?1` 绑定」→ rusqlite 报「Wrong number of parameters」中断级联——测到的是崩溃不是越权；等价全表扫描须写成 `... OR ?1 IS NOT NULL` 仍消费参数。
2. 等价小阈值变异（看门狗 660s→30s）必须**同步**调小被观测的另一个量（心跳 60s→5s），否则 pristine 也会判死、两态无差异。

| 锚点 | 判定 | 一句话证据 |
|---|---|---|
| V176 搜索互斥删 | **KILLED** | 变异态双开落盘 tavily=T/deepseek=T（pristine 同保存自动翻关）；conflictText 兜底提示同屏活体 |
| V177 err_chain 丢链 | **KILLED** | 变异态审计行纯顶层串；pristine 同行带五层链（Connect→tunnel→tcp→refused 61） |
| V178 启动种子兜底删 | **KILLED** | 清表+死代理冷启动 COUNT=0、厂商库网格消失（M188 三症状复活）；还原后种子回填 326 |
| V181 交卷不立即终局 | **KILLED** | 双队列桩：子回合越过交卷跑到 text 步 → 行翻 failed+result_json_unparseable（pristine submit 即 succeeded） |
| V183 resume 丢 onion 元列（**必须 KILLED**） | **KILLED** | 桩 tools_vary 44.8s 造真熔断行+sidecar+onion=validator；pristine resume 保留 validator、变异态续跑会话 onion_config=**NULL**——配置档静默丢失（M192 判据 3 红） |
| V185 提醒 grace 推进删（等价注入） | **KILLED** | stale 行启用：pristine 静默推进明日零通知；变异态 22s 后 reminder.fired 补响 |
| V186 stream_failed 缺 err_chain | **KILLED** | 桩 break（bytes=298/801ms）：变异态五字段缺 err_chain（pristine M197 PASS 五字段齐） |
| V187 重发判据毒化复活 | **KILLED** | 桩 think_break（纯思考增量即断）：变异态无 stream_retry 单次即败（pristine 同场景 retry attempt=1） |
| V188 restart 清场不删 thinking | **KILLED（经 L1）** | 实机叠影观测窗秒级+折叠默认收起未命中（连拍 12 帧）；防线本体由 stream.test「bot-turn-restart 清场」用例锁定（还原后绿） |
| V189 重试按钮挂全部 | **KILLED** | pristine 401 旧气泡（非末条）无重试（裁剪图）；变异态同气泡出现「⟲ 重试」——判据 1/3 被打破 |
| V191 拦截阈值 3→5 | **KILLED** | 桩同参 create_task ×5：consec 1-4 全放行（4 任务落库）、第 5 次 consec=5 才拦（pristine 第 3 次即拦） |
| V192 转问超时回落放行（**必须 KILLED**） | **KILLED** | 30s 变异窗+回落放行：consec=3 超时后 action=answered 放行原参、**第 3 任务落库**（pristine action=fallback 只 2 任务）——死循环闸「无人应答即拒」防线有牙 |

**基础设施革新（本轮方法论沉淀）**：钥匙串 ACL 每次重建弹密码框不可自动化 → env 假密钥补丁（keyring.rs 读点回退，值可假）+ **脚本化 LLM 桩**（stub-llm.mjs：剧本逐步消费/双队列路由 sub_match/tools_vary 熔断构造/break·think_break·delay_ms·http_error 步型）+ LocalStub 模型（本地 baseUrl 挂已验证厂商域）。真熔断行构造从「20-40 分钟真模型且被并行批绕开」变为「桩秒级确定性驱动」——V181/V183/V192 的判定面全部由此解锁。

**顺延 9 条**：V172/V173（check 等待钳制/心跳——桩 delay_ms 长等待场景，约 20 分钟/条）、V174（stop 级联双会话双子 agent）、V179（nudge 新鲜度 stale verif 注入）、V180/V182（恢复链剥标记/sidecar 不清——需 FUSE 标记正文形态的熔断行，tools_vary 已可造，顺延补跑）、V184（机器验收豁免放宽）、V175（汇报轮提问概率面，L1 背书）、V190（ort 预检——失败路径仅在 Windows 缺 dll 场景，本机 macOS 不可激活）。

---

| 卡 | 结果 | 证据（事件行 / 库证输出） | 备注 |
|---|---|---|---|
| M204 | 未跑 | 需要真 OAuth MCP 服务器 + 浏览器授权流程（用户在场），随变异轮补跑 | 基础设施已备（env key + 桩 + 探针） |
| M205 | 未跑 | 需恶意 OAuth 服务器（自建最小 HTTP 服务），随变异轮补跑 | — |
| M206 | 未跑 | 需钥匙串真实读写面（env 补丁拆除后补跑更真） | — |
| M207 | 未跑 | 需 official-elicit-demo 服务器重建，随变异轮补跑 | — |
| M208 | 部分 | 302 全证：POST→/redirect-to→/get echo 200 JSON（301/302/303→GET 丢体）+ /post 405+allow:POST 交叉验证；每跳过闸可见（来源行显最终落点）；127.0.0.1 被公网闸拒（既有口径顺手背书） | 301/303/307/308 纯函数单测全矩阵背书（tests.rs:791-819）——httpbin.org/httpbingo.org/postman-echo 全部 302 化、webhook.site 免费档不给挂 Location，实机登记环境受限 |
| M209 | FAIL→已修→复验 PASS | 修复前：tool 审计行 args 带原始 JSON（SECRET-pass 原文）+ 来源行带 userinfo（2 处泄漏）；163090ad 修复后复验：SECRET-pass 计数 0、args=POST httpbin.org/post、来源行 ***@ | F-新4（P2）：主循环 tool 审计行与技能步失败日志两处 sink 没接上；锚点测试 5 例 |
| M210 | PASS | SECRET-Q210/Q210B 全文计数 0（tool.call args_preview、fetch_url 自家审计行、tool.return preview、tool 事件四处全净）；host+path 可见；模型面未裁（/html 成功面，JSON content-type 被 fetch_url 拒是设计口径） | 成功+404 两个方向都跑过 |
| M211 | PASS | 三条过渡段（64:ff9b::c0a8:0101 / 2002:0a00:0001::1 / ::c0a8:0101）全被公网闸拒「已拒绝访问本机/内网地址」；2606:4700:4700::1111 未被字面量级拦（tcp connect error 65，网络层非闸） | 判据 3（MCP 侧同源）L1 背书（两闸单源 check_public_url） |
| M212 | PASS | hallucination_guard kind=声称变更但未调工具 ×1 → 【系统提示】注入（第二轮请求体实证）→ 逐字复述 → hallucination_guard.repeat ×1（返工提示未成为出处） | 复述文案必须与首轮同词形（变更类声称模式按子串命中） |
| M213 | PASS | loop.tool_window.applied chars=28183 trimmed=1 keep_recent=6；桩子全文含「存档原文按 16384 字节上限截断，超出部分不在存档里」（无「完整结果已存档」）；lookup_receipt(call_y1s8a0) 取回凭证+完整返回（16KB 档）；尾部引用（共 100 条事件 + 最后事件 id 27547393152）未被拦 | 大结果走回灌漏斗转存（conv-history 指针+头尾内联）先于滑动窗触发是预期分层 |
| M214 | PASS | 单回合 spawn+失败构造：wi 行 failed+error 非空 → trace 741 汇报轮跑完 → 行仍 failed 同 error；极短回合（trace 741 ms=13）无 working 残行 | work_items 每会话一行（重设计未实施）——判据用「无合法用户轮插入」的构造才可观察；spawn 缺 profile 被拒=工具闸正证 |
| M215 | PASS | 失败行 ord=10 content=「⚠️ 大模型 API 错误 500…（上游：stub-fail-215）」只含本轮错误，无上一轮汇报轮正文（ord 6/8 在案对照） | — |
| M216 | 部分 | 未执行完：/clean 命令选择器自动化 3 次（点击条目/双回车/键盘导航）均未执行，待用户 5 秒手动补跑 | Cmd+N=新建任务面板非多窗→判据 2（第二窗口同步）环境受限登记；自动化对抗中误建 1 个空任务待清理 |
| M220 | PASS | bot_onions_bound validator 8 工具含 run_python+ast_inspect；run_python 实际执行成功（m220-verifier-ok 156ms）；判据 2 静态面：research 字样只在 recon 段正确语境 | 判据 3（显式 profile=research 被拒）未单独构造，登记行为口径 |
| M221 | 部分 | 默认档 9 工具全点名无漏列、write_file 过白名单（路径语义错是另一回事）；stage=executor+profile=research 被 stage 默认盖掉（两档都绑 writereditor）——显式 profile 优先级观察项登记 | 判据 2 的 prompt↔生效集一致性达成 |
| M222 | 部分 | (a) PASS：reminder.set id=545749f0 at:2026-11-19T03:00 + 定时页页签计数/条目/nextRunAt（1 小时后）全渲染；(b) 作业不双跑未构造（需作业到点窗口+手动触发并发，登记顺延） | — |
| M223 | PASS | (a) 僵尸行（daily:broken-format + 过期 remind_at）reminder.fired 后删行（COUNT=1 仅剩 M222 行）+ 系统通知实弹（截图）；(b) 「已重命名」声称被拦 kind=声称变更但未调工具 + 诚实更正放行 | (a) 的删行无独立审计行（fired 事件即痕迹），登记观察 |
| M227 | PASS | 20 次 list_files Σ77ms 一次跑完，气泡「调用工具 20/20 · Σ 77ms」渲染流畅、事件成对（观感卡口径：macOS 本机记「未观察到卡顿」） | — |
| M228 | 未跑 | 需官方 SDK MCP 服务器（1.32.2 按会话复用），随变异轮补跑 | — |
| M229 | PASS | 清表后死代理冷启动 meta_seed_models_dev providers=235 models=8935（成功分支留痕实证）；库行数 0→235 | V221 修复面实机验证 |
| M230 | PASS | 判据 1：气泡「请求大模型失败：网络连接中断（连接失败），本轮未完成，可点重试」四段齐、无原始串无内部错误（截图）；判据 2：重试按钮在位且点击重放（同 content user.message 01:12:22）；判据 3：恢复后重试成功气泡翻转 | 杀桩构造（连接拒绝）——deadproxy 被 NO_PROXY 豁免不适用于 LocalStub 场景，已登记 |
**变异执行台账**（V193–V222 共 30 条待跑；2026-11-19 凌晨完成 M204–M230 行为卡后按用户节奏另开变异轮，基础设施已全部就绪——env 假密钥 + stub-llm 桩 + 剧本路由 + bodies.log 证据面，跑法同前轮「单编辑→重建→复跑→还原」）：

| 锚点 | 跑否 | 结论（KILLED / SURVIVED / 受限） | 证据 |
|---|---|---|---|
| V193 | | | |
| V194 | | | |
| V195 | | | |
| V196 | | | |
| V197 | | | |
| V198 | | | |
| V199 | | | |
| V200 | | | |
| V201 | | | |
| V202 | | | |
| V203 | | | |
| V204 | | | |
| V205 | | | |
| V206 | | | |
| V207 | | | |
| V208 | | | |
| V209 | | | |
| V210 | | | |
| V211 | | | |
| V212 | | | |
| V213 | | | |
| V214 | | | |
| V215 | | | |
| V216 | | | |
| V217 | | | |
| V218 | | | |
| V219 | | | |
| V220 | | | |
| V221 | | | |
| V222 | | | |

---

## 十、库证查询与日志证

上半部来自 11-15 方案那批，下半部是本轮新增面的查询模板。

### bot.log 事件证（grep 模板，`<data_dir>` 从 bot.log 首部 `app_data_dir` 行取）

```bash
# 本轮新增/改动事件逐条计数（判据引用）
grep -c "subagent_check_wait"      <data_dir>/bot.log   # M182 等待计时
grep -c "subagent_deliverable_nudge" <data_dir>/bot.log # M189 收尾提醒（判据：恰好 1）
grep -c "subagent_fuse_recovered"  <data_dir>/bot.log   # M190 恢复链（带 via）
grep -c "subagent_history_sidecar" <data_dir>/bot.log   # M190/192 sidecar 落/清（_cleared 后缀单算）
grep -c "subagent_resume"          <data_dir>/bot.log   # M192 续跑（带历史条数+配置档）
grep -c "repeat_gate"              <data_dir>/bot.log   # M201/202 拦截（kv 含 action）
grep -c "llm.stream_retry"         <data_dir>/bot.log   # M198 重发（开思考场景）
grep -c "llm.stream_failed"        <data_dir>/bot.log   # M197 断流（kv 含 err_chain/bytes/phase）
grep -c "memory.embed_engine_unavailable" <data_dir>/bot.log  # M200/W25（成功路径应为 0）
grep -c "meta_seed_models_dev"     <data_dir>/bot.log   # M188 种子（判据：恰 1 条）
grep -cE "reminder.(enqueue_failed|expired|scan_failed|advance_failed)" <data_dir>/bot.log  # M194/195（正常路径应为 0，expired 仅过期场景）
```

### sqlite 证（时间戳列一律毫秒整数）

```sql
-- Q1 本轮核心表：reminders（M194/M195）
SELECT id, text, schedule, remind_at, enabled, fired_at, fired_count,
       datetime(created_at/1000,'unixepoch','localtime') AS created
FROM reminders ORDER BY created_at;

-- Q2 子 agent 行：状态 / 预算 token / 续跑面（M190–M193）
SELECT id, status, profile, parent_session_id, session_id,
       length(result_json) AS result_len, error,
       prompt_tokens, completion_tokens, cache_read_tokens,
       datetime(created_at/1000,'unixepoch','localtime') AS created,
       datetime(started_at/1000,'unixepoch','localtime') AS started,
       datetime(finished_at/1000,'unixepoch','localtime') AS finished
FROM subagents ORDER BY created_at DESC LIMIT 20;

-- Q3 模型库种子面（M188）：厂商行数（空表 → 种子后应 ≥ 数百）
SELECT COUNT(*) AS vendors FROM meta_provider;

-- Q4 汇报轮落库（M184）：子 agent 终态后的自动续轮行（无用户输入、带 bot-chat-report 定稿）
SELECT ord, role, turn_status, length(content) AS len,
       datetime(created_at/1000,'unixepoch','localtime') AS at
FROM bot_messages WHERE session_id = '<主会话id>' ORDER BY ord DESC LIMIT 10;
```

（表列名以 `PRAGMA table_info(<表>)` 为准；`meta_provider` 列名以现行 schema
为准——种子只写解析器消费字段，Q3 只数行数不点名列。）

---

```bash
# 通用：定位数据目录（bot.log 首部的 app_data_dir 审计行）
head -50 "$APPDATA/wmessage/bot.log" | grep -m1 app_data_dir

# ── OAuth 面（M204/M205/M206）──────────────────────────
grep -c "mcp.oauth.login"   "$LOG"      # 登录成功计数（应为 1）
grep -c "mcp.oauth.logout"  "$LOG"      # 登出计数
grep -Ei "127\.0\.0\.1|169\.254\.|10\.[0-9]|\[2002:|\[64:ff9b" "$LOG" | head   # 内网请求痕迹（应为空）

# ── 公网闸与 http 面（M208/M209/M210/M211）──────────────
grep -c "bot.http_request" "$LOG"
grep -E "来源：https?://" "$LOG" | head        # 「来源：」行的 userinfo 应是 ***@
grep -c "fetch_url | url:" "$LOG"              # 该行应只见 host+path

# ── 守卫与上下文面（M212/M213）─────────────────────────
grep -c "hallucination_guard" "$LOG"
grep -c "hallucination_guard.repeat" "$LOG"
grep -c "【系统提示】" "$LOG" | head -1        # 返工提示存在性

# ── 会话落库面（M214/M215/M216）───────────────────────
sqlite3 "$DB" "SELECT session_id, stage, error FROM work_items ORDER BY updated_at DESC LIMIT 5;"
sqlite3 "$DB" "SELECT msg_id, substr(content,1,80) FROM bot_messages WHERE content LIKE '%错误%' ORDER BY ord DESC LIMIT 5;"

# ── MCP 生命周期面（M217/M218/M219）───────────────────
grep -c "mcp.tools_discovered" "$LOG"
grep -c "mcp.tools_discovered_but_disabled" "$LOG"
pgrep -f "<你的 MCP server 名>" || echo "无残留进程（期望）"

# ── 提醒与作业面（M222/M223）──────────────────────────
sqlite3 "$DB" "SELECT id, remind_at, fired_count FROM reminders ORDER BY updated_at DESC LIMIT 5;"
sqlite3 "$DB" "SELECT COUNT(*) FROM reminders WHERE remind_at IS NULL;"   # 期望 0（M223 判据 1）
sqlite3 "$DB" "SELECT job_id, COUNT(*) FROM scheduled_job_runs GROUP BY job_id;"  # M222 判据 2
```

**事件名速查**（本轮判据引用，源码里均为真实事件名）：

| 事件名 | 出处 | 判据卡 |
|---|---|---|
| `mcp.oauth.login` / `mcp.oauth.logout` | `bot/mcp/oauth.rs` | M204 / M206 |
| `mcp.tools_discovered` / `mcp.tools_discovered_but_disabled` | `bot/mcp/manager.rs` | M204 / M217 |
| `onion.stage_attached` | `bot/mcp/manager.rs` | M220 / M221 |
| `tool.output.over_budget` / `tool.output.spilled` | `bot_model_loop/output.rs` | M213 |
| `hallucination_guard` / `hallucination_guard.repeat` | `bot_model_loop/guard.rs` | M212 / M223 |
| `bot-msg-upserted` / `bot-msg-dropped` | `bot_chat.rs`（统一出口盖章） | M216 |
| `bot-ask-resolved` | `bot/mcp/elicit.rs` | M207 |
| 自由格式串：`reminder.set \| ...` / `reminder.cancel \| ...` | `bot_reminders.rs` | M222 |
| 自由格式串：`bot.http_request` | `bot/tools/http.rs` | M208 |

---

## 十一、事件名速查（判据引用）

上半部来自 11-15 方案那批，下半部是本轮新增面。

流式/回合：`llm.stream_failed`（kv 增 `err_chain` / `bytes` / `elapsed_ms` /
`phase`）、`llm.stream_retry`（开思考场景复活）、`bot-turn-restart`（重发前
清场）、`bot-check-heartbeat`（check 等待期心跳，60s 一拍，只喂前端看门狗）。
子 agent：`subagent_deliverable_nudge`（交付件齐收尾提醒，一次）、
`subagent_fuse_recovered`（带 via 四值）、`subagent_history_sidecar` /
`subagent_history_sidecar_cleared` / `subagent_history_sidecar_failed`、
`subagent_resume`（带历史条数 + 配置档）、`subagent_check_wait`（等待时长 +
退出原因 terminal/timeout/stopped）、`subagent.parse_failure_recovered`（回归
确认在案）。闸：`repeat_gate`（WARN，kv `tool/consec/round/action`，action ∈
reject/answered/fallback）。模型库：`meta_seed_models_dev`（WARN，空表播种）。
提醒：`reminder.enqueue_failed` / `reminder.expired` / `reminder.scan_failed` /
`reminder.advance_failed`；ort：`memory.embed_engine_unavailable`（reason/
startup）。前端事件：`reminders-changed`（提醒写路径广播）、`bot-chat-report`
（汇报轮定稿）、`subagent-finished`（终态刷新）。
命令面新增：`reminder_create` / `reminder_update` / `reminder_delete` /
`reminder_set_enabled`；bot 工具新增：`submit_result`、`resume_subagent`
（`schedule_overview` 改判别联合返回）。

---

- 前轮 `docs/realmachine-smoke-test-2026-11-15.md` 覆盖 v0.3.0 后七批 46 提交
  （M182–M203 + W25–W27 + 变异 V172–V192）。**本轮覆盖的是那之后的四批 25 笔**
  （审计报告 `docs/agent-audit-2026-11-17.md` 的处置批次），两轮卡号与变异编号接续不重叠。
- 本轮修的东西大半是**闸与判定口径**，卡位偏「负向」：M205 / M211 是明确的负向卡
  （不该通过的必须被拦），其余以「行为面 + 审计面」双判据为主。
- 本轮**无灰度开关**：OAuth 逐 URL 过闸、公网闸 IPv6 收紧、重定向切方法、守卫口径
  全部恒生效——对照面只能靠「字面量级」与「单测锚点」两条背书。

## 十二、读码预判的风险面（提前预警，不代替实测）


1. **M201 误拦面**：非豁免工具的合法同参重试（第 2 次是合法的参数没改对重试，
   第 3 次才拦）——实机若遇合法场景被拦，往豁免表加名（§〇 处置口径），
   并在回填位备注工具名与场景。
2. **M202 的 300s 等待**：超时窗口内问卡占住主回合——测试时别在等待期发新
   消息（busy 拒答是既有语义，不是缺陷）；等不起可临时改小
   `REPEAT_GATE_ASK_TIMEOUT_SECS`（变异法，跑完还原）。
3. **M190 真熔断构造贵**：300 文件读目标 ≈ 300+ 轮、约 20–40 分钟与对应
   token——建议挑最小可熔断体量；造不出来就按卡内诚实登记降级。
4. **M188 种子字段面**：种子只含解析器消费字段（235 厂商 8935 模型的子集），
   个别厂商展示信息可能薄；判据只看网格非空 + 按钮在场，别拿种子数据验收
   在线数据的全量字段。
5. **M184/M203 概率面**：汇报轮自发提问与卡住自发提问都是提示词引导——
   模型不问不算 FAIL（两可），`ask_registered` 的闸行为（判据 1）才是硬判据。
6. **M198 清场与前端缓存**：`bot-turn-restart` 清的是本轮权威副本；若前端
   正在渲染增量流，清场瞬间的视觉跳变属预期（观感 = 回到起点重写），不算
   叠影缺陷——叠影指**清场后旧内容仍可见**。
7. **M195 直写库**：手工改 `remind_at` 到过去是构造过期场景的快捷法——
   注意毫秒整数纪律；改完等一轮 tick（调度器间隔）再看 `reminder.expired`。
8. **提醒 Effects 队列**：通知经 Effects 统一副作用队列——若队列里堆积了
   旧的 due_notify 待办，提醒通知会排队晚响（先进先出），判据只看「响没响」
   不卡秒级时延。

---


## 十三、与前几份的关系 + 未覆盖诚实清单

- **与前几份**：11-07 方案（M163–M181）覆盖真源化批 + 差距收编批，本份不重复；
  11-15 方案（M182–M203）覆盖 v0.3.0 后七批 46 笔，本份把它的卡**并入**执行
  （那份方案写了但从未跑过）。本份起点 `1b102e90` 正接 11-07 的基线 `16895019`。
  M199 重试按钮复用 M166（11-07）的 /retry 截尾链路——那条已验过，本份只验入口与
  末条判定。
- **11-08 白天四批仍不在本份**（用户口径 11-09 起）：三档退役 `61db05ee` /
  底栏逐级图标化 `6282ef2f` / 授权四档并两档 `4d0c156d..a58fd3ec` /
  ask 卡行式重塑 `0b463e59+754a9551`——这四批**没有任何一份冒烟卡覆盖过**，
  实机验收缺口在案；如需补验另出补卡（ask 卡 UI 面 M202 可部分顺带覆盖）。

---


## 十四、MCP OAuth 全链（审计 K-1 / K-2 / K-9 —— 本轮风险最高）


> 本轮把 OAuth 的派生端点（发现第二跳 root / DCR / token endpoint）全部改成「逐 URL 过
> 闸 + 钉当跳 IP」，并把降级凭据文件改 0600。改错的后果是「能登录但登不掉」或
> 「登录时把内网打一遍」——两个方向都要真链验证。

### M204 OAuth 登录全链（主卡）
- **步骤**：配置一台 HTTP 传输的 MCP 服务器（走 OAuth 那套，需支持
  `/.well-known/oauth-protected-resource` + `/.well-known/oauth-authorization-server`
  + DCR），点设置页「登录」，走完浏览器授权 → 回调 → 换 token。
- **判据**：
  1. `mcp.oauth.login` 事件 1 条（server id 对）；
  2. 登录成功后该服务器的工具出现在模型清单里（`mcp.tools_discovered` 1 条），
     且能实际调用成功；
  3. bot.log 全文 grep **无内网地址**（`127.0.0.1`、`169.254.`、`10.`、`192.168.`）——
     派生端点过闸后不该有任何指向内网的请求；
  4. 回调服务器只绑 127.0.0.1:0 随机端口（授权完成后该端口已关闭）。
- **变异锚点**：V193、V194、V195。
- **环境依赖**：需要一台真 OAuth MCP 服务器（官方 SDK 1.32.2 那套可复用前轮的手记）。
  没有则登记「环境受限」，判据 1/4 用纯函数面背书。

### M205 派生端点逐 URL 过闸（负向，主卡）
- **步骤**：用一台**恶意** OAuth MCP 服务器（自建最小 HTTP 服务）：保护资源元数据里回
  `authorization_servers: ["http://169.254.169.254/"]`，点「登录」。
- **判据**：
  1. 登录**失败**，错误文案指向「URL 安全闸」而不是「连不上」；
  2. bot.log 无任何指向 `169.254.169.254` 的请求记录（用审计里的 host 字段核对）；
  3. 同样把 `authorization_servers` 指到 `http://10.0.0.1/` 与 `file:///etc/passwd`
     各测一次，均被同一道闸拦下。
- **变异锚点**：V193、V194、V195。
- **注**：V193/V194/V195 三条变异必须**逐条**跑（分别对应第二跳 / token / authorize
  三个点的闸），不是跑一条代表全部。

### M206 凭据文件权限与登出不再假成功
- **步骤**：(a) macOS 观察 OAuth 令牌落点（System keyring 后端，不落文件）；(b) 若环境
  有 Linux 无 keyring 档，查 `bot-mcp-oauth.json` 权限；(c) 在**钥匙串不可用**的条件下
  点「登出」（如在受管服务里临时禁用钥匙串访问，或用 `keychain` 锁定态）。
- **判据**：
  1. macOS：令牌在钥匙串里（`security find-generic-password -s wmessage` 能查到
     `mcp-oauth:<id>` 条目），**不在** data_dir 的任何 json 里；
  2. Linux 降级档：`bot-mcp-oauth.json` 权限 **0600**（`stat -c %a`），同目录其他
     文件不是 0600（对照组，证明闸真的生效）；
  3. 钥匙串不可用时点登出 → **命令报错**（前端有提示），**没有** `mcp.oauth.logout`
     成功事件；钥匙串恢复后再登出 → 成功事件 1 条且令牌条目消失。
- **变异锚点**：V196、V197。
- **环境依赖**：判据 2 需 Linux 降级档（本机 macOS 受限则登记）；判据 3 需构造钥匙串
  故障（macOS 钥匙串锁定态可近似）。

### M207 elicitation 应答回声面（凭据脱敏）
- **步骤**：起一台声明 `elicitation.form` 能力的 MCP 服务器（per-server
  `allowElicitation=true`，**默认 false**，须显式开），触发一次反问，在应答里输入一段
  含引号/反斜杠的文本（如 `a"b-secret`），观察服务器回显。
- **判据**：
  1. bot.log 全文 grep **无** `b-secret`（应答值与转义形态都不该出现）；
  2. `bot-ask-resolved` 的 answer 恒为占位符 `[MCP 应答，值不落日志]`；
  3. 在**数字字段**里输入非数字（如误粘一段凭据）→ 错误文案**不含**你输入的内容，
     只说「输入不是数字，请重新输入」。
- **变异锚点**：V211、V212。

---


## 十五、公网闸与 http_request 行为（审计 H-1 / H-2 / H-4 / H-6）


> 本轮把公网闸的 IPv6 判据与 MCP 闸**并成一份**（方向是收紧），并让 http_request
> 的重定向按 RFC 9110 切方法（303/301/302 的 POST 改 GET 丢体）。两条都会改变真实
> 联网行为，必须实测。

### M208 重定向按 RFC 切方法（主卡）
- **步骤**：本地起一个最小 HTTP 服务，提供 4 个端点：`/301`（301 → `/target`）、
  `/302`、`/303`、`/307`，`/target` 回显收到的 method 与 body。分别用 `http_request`
  发 POST（带 body）打这四个端点。
- **判据**：
  1. 301/302/303 三条 → `/target` 收到的 method 是 **GET** 且**无 body**；
  2. 307 → 收到的 method 仍是 **POST** 且 body 原样；
  3. 每跳都过了目的地闸（本地地址走的是「用户配置目的地」路径，若被拒则本卡改用
     公网可达的测试服务，或登记受限）。
- **变异锚点**：V198。

### M209 userinfo 不进审计面
- **步骤**：`http_request` 打一个带 userinfo 的 URL（如 `https://user:pass@<host>/x`）
  ——用一个会 404 的地址即可（错也要看审计）；另测一个 302 到带 userinfo 的 Location。
- **判据**：
  1. bot.log 全文 grep **无** `pass`（userinfo 不该出现在 tool.return preview、子 agent
     动作流、执行痕迹 span 任何一处）；
  2. 「来源：」行保留 scheme://host/path，userinfo 被替换成 `***@`；
  3. 正文节选里的第三方链接**未被裁**（对照：同一条日志里页面正文里的链接原样在）。
- **变异锚点**：V199。

### M210 fetch_url 查询串不进审计面
- **步骤**：`fetch_url` 调一个带查询串的 URL（`https://<host>/p?q=secret`），成功与失败
  各一次（失败用会 404 的路径）。
- **判据**：
  1. bot.log 全文 grep **无** `secret`（入参预览、失败文案、`fetch_url | url:` 那条
     自家审计三处都不得带查询串）；
  2. 日志里该 URL 仍可见 host + path（信息没丢，只是凭据段被裁）；
  3. 模型可见面（对话气泡里的结果）**仍带**完整 URL（模型需要知道最终落点）——这条
     是口径的分界，裁错方向算 FAIL。
- **变异锚点**：V200、V201。

### M211 IPv6 过渡段收紧（负向）
- **步骤**：`http_request` / `fetch_url` 各打一次这些字面量地址：
  `http://[64:ff9b::c0a8:0101]/`（NAT64 → 192.168.1.1）、`http://[2002:0a00:0001::1]/`
  （6to4 → 10.0.0.1）、`http://[::c0a8:0101]/`（IPv4-compatible）。
- **判据**：
  1. 三条全被公网闸拒（错误文案指向公网闸）；
  2. **回归**：普通公网 IPv6（如 `https://[2606:4700:4700::1111]/`）仍**未被字面量级
     拦下**（放行与否取决于 DNS 解析，但不该被「IPv6 保留段」这条判据拒）；
  3. MCP HTTP 服务器的同一类地址同样被拒（两闸已单源）。
- **变异锚点**：V202。
- **注**：本卡是**收紧方向**的回归卡——若用户环境是 IPv6-only 且走 NAT64，联网工具会
  受影响，跑完在回填位注明（这是有意取舍，不是缺陷）。

---


## 十六、守卫与上下文面（审计 D-1 / D-2 / E-1a）


### M212 系统提示不算出处（幻觉守卫二次防线）
- **步骤**：让模型报一个**没有出处**的数（如「这次处理了 424242 条记录」，而本次工具
  返回里没有这个数），观察守卫拦下并注入返工提示；然后让模型**照抄返工提示里的数**
  再报一次。
- **判据**：
  1. 第一次：`hallucination_guard` 事件 1 条（kind=unsourced_value 之类），本轮不
     直接放行；
  2. 第二次（模型复述同一个数）：**仍被拦**（`hallucination_guard.repeat` 或再次
     Continue）——修复前这里会因「返工提示里的数被当出处」而放行；
  3. bot.log 里能看到返工提示文本（`【系统提示】` 前缀），但该提示**不在**下一轮的
     出处判定里（这是本卡的实质）。
- **变异锚点**：V203。

### M213 证据池头尾对齐 + 滑动窗桩子披露
- **步骤**：(a) 让模型读一个 >64KB 的文件并引用**尾部**的一个数（如让子 agent 统计一个
  大 CSV 的末行数值）；(b) 让一轮内工具结果累计超 24K 字符触发滑动窗，看模型看到
  什么。
- **判据**：
  1. 引用尾部数字**不被判「无出处」**（修复前会误拦真话）；
  2. 滑动窗桩子文案**含钳制披露**（「存档原文按 N 字节上限截断，超出部分不在存档里」），
     **不含**「完整结果已存档」这种超出事实的说法；
  3. 模型在桩子情况下调 `lookup_receipt` 取回的内容与披露的上限一致（拿回的是被
     截过的那份，模型不误以为拿到全文）。
- **变异锚点**：V204、V214。

---


## 十七、会话落库与工作项一致性（审计 L-2 / L-3 / L-4 / L-5 / L-8）


### M214 工作项只跟用户轮（失败信号不被抹）
- **步骤**：(a) 制造一次用户轮失败（如让模型调一个必然被拒的工具后失败）；(b) 在
  该会话派一个子 agent，等它收尾触发汇报轮；(c) 看「需要人处理」聚合面。
- **判据**：
  1. 用户轮失败后，`work_items` 表该行 stage=failed、error 非空；
  2. 汇报轮跑完后，该行**仍是 failed**（stage 不被唤醒回 working 再收成 idle）——
     这是本卡的实质（修复前失败信号当场消失）；
  3. 极短回合（模型一轮就返回）不留 stage=working 的残行（开票与收尾次序已钉）。
- **变异锚点**：V205。
- **库证**：`SELECT stage, error, updated_at FROM work_items WHERE session_id=?;`

### M215 回合事实命令层清场（失败行不串台）
- **步骤**：(a) 让子 agent 收尾触发一次**汇报轮**（它会把正文流进该会话）；(b) 紧接着
  让用户轮在**密钥失效**条件下失败（设置页清空 API key 后立刻发消息）。
- **判据**：
  1. 失败行的 content **只含本轮的错误**，不含上一轮汇报轮的正文（修复前会拼在一起
     落库）；
  2. 库里 `bot_messages` 里该 failed 行的 content 与界面显示一致；
  3. 正常路径（密钥有效）不受影响：失败行只含错误。
- **变异锚点**：V206。

### M216 /clean 广播删除投影
- **步骤**：单窗口跑 `/clean`；再开第二个窗口看同一会话。
- **判据**：
  1. `/clean` 后 `bot-msg-dropped` 事件 1 条（载荷含被删的 msgIds 列表）；
  2. **第二窗口**的会话视图同步清空（不刷新页面的情况下）——这是本卡的实质（修复前
     第二窗口仍显示旧消息）；
  3. 前端改为「落库成功再清视图」：invoke 失败时**视图不清空**且有提示（可用改库锁
     或断连构造失败）。
- **变异锚点**：V207。

---


## 十八、MCP stdio 生命周期与工具发现（审计 K-3 / K-5 / K-4 / K-10 / K-11）


### M217 工具发现与槽位自愈
- **步骤**：(a) 起一台 stdio MCP 服务器，等工具发现完成；(b) 在**发现进行中**（最长约
  20s）于设置页禁用该服务器；(c) 再看模型清单；(d) 另测：握手超时后槽位自愈。
- **判据**：
  1. 情形 (b) 之后模型清单里**不再出现**该服务器的工具（修复前是幽灵条目，每轮都
     出现，直到下一次 MCP 配置写才消失）；
  2. 出现 `mcp.tools_discovered_but_disabled` 事件（不生效时有留痕）；
  3. 情形 (d)：停滞的 Connecting 槽位按 40s 判为 down，发现路径能重新走起来
     （`mcp.tools_discovered` 再来一条，面板不再永远「待发现」）。
- **变异锚点**：V208。

### M218 超长响应行快速失败
- **步骤**：让 MCP 服务器在一次工具调用里返回**单行 >1MB** 的 JSON-RPC 响应（如
  返回一个 1.5MB 的 base64 字段）。
- **判据**：
  1. 调用**快速失败**（秒级，不是等满超时）——修复前要干等到 600s；
  2. 错误可读（提到响应超单行上限）；
  3. **回归**：服务器刷一条超长**噪声行**（如进度条）后，仍能正常应答下一条请求，
     会话不断。
- **变异锚点**：V209。

### M219 树杀兜底先拍树再收尸（孤儿进程）
- **步骤**：起一个 `npx` 包装的 stdio MCP 服务器（npx → node 两层），在中途强杀
  wrapper（或触发握手超时），随后查进程表。
- **判据**：
  1. 真实服务器进程（node）**不留活**（`pgrep -f "<server-name>"` 为空）——修复前
     npx 死了 node 活着占端口；
  2. 端口不被占（`lsof -i :<port>` 空）；
  3. 不误杀同组无关进程（对照组：另起一个同 prefix 的无关进程，确认它还在）。
- **变异锚点**：V210。
- **注**：本卡的进程名/端口取你实测时的实际值，填回填位。

---


## 十九、技能、提示词与提醒/作业（审计 C-1 / C-3 / E-1a / K-9 / L-6 / L-7 / L-9 / D-4 / D-5）


### M220 验收段档位口径
- **步骤**：让主 agent 派一个 verifier 段子 agent（按提示词只传 `stage=verifier`、不传
  profile），让它用 `run_python` 做一次只读复核。
- **判据**：
  1. verifier 段**实际可用** `run_python` 与 `ast_inspect`（不被 dispatch 白名单闸拒）；
  2. 主 agent 的提示词里**不含**「验收段用 research profile」这类与运行时相反的
     指示（grep 源码与实际下发的 system prompt 两处）；
  3. 若父 agent 自行传了 `profile=research` → 该次调用会被白名单闸拒（属既有口径：
     显式档位覆盖 yaml 档），不算 FAIL，但要在回填位记下实际行为。
- **变异锚点**：无（源码面已锁）。
- **注**：判据 2 可直接 grep 源码：`prompts/subagent.rs` 里不应再出现「用 research
  profile」。

### M221 executor 工具行随生效集渲染
- **步骤**：派一个 executor 段子 agent（`stage=executor`，默认挂 writereditor 档），
  让它改一个文件；再派一个**显式给别的配置档**的 executor 段。
- **判据**：
  1. 默认路径：提示词里的工具行与实际可用工具一致（edit_file / write_file /
     run_python 可用且能真调通）；
  2. 显式换档时：提示词里的工具行**只列该档实际生效的工具**（不列被收掉的）——
     模型不该照着一个被收掉的工具名去调（那会撞白名单闸白烧一轮）；
  3. 模型全程无「工具不可用」类失败（对照判据 2 的后果面）。
- **变异锚点**：V213。

### M222 提醒广播与作业不双跑
- **步骤**：(a) 聊天里让模型建一条提醒（+2 小时），切到「定时」页；(b) 同一作业，
  先让它到点排队（占满并发闸），再手动点「立即执行」。
- **判据**：
  1. 情形 (a)：**不用刷新页面**，定时页就出现新提醒（`reminders-changed` 事件带
     1 条；修复前模型建的提醒不广播，页面停在旧快照）；
  2. 情形 (b)：手动触发**不与到点派发重复跑**（同一 occurrence 只有一次执行——
     对照 `scheduled_job_runs` 的行数与 token 消耗）。
- **变异锚点**：无。

### M223 僵尸提醒退休与变更话术
- **步骤**：(a) 建一条 schedule 串损坏或极难的周期提醒（可手改 reminders 表的排期串列），
  让它触发一次；(b) 让模型声称「已把 a.txt 重命名为 b.txt」但**没有**真调工具。
- **判据**：
  1. 情形 (a)：触发后该行**被删**（不是留一条 `remind_at` 为 NULL 的僵尸行——它既不
     触发也无人清扫）；`SELECT COUNT(*) FROM reminders` 核对；
  2. 情形 (b)：该声称被幻觉守卫拦下（`hallucination_guard` 1 条，kind=mutation 类）——
     修复前「已重命名」不在任何工具的 claims_patterns 里，守卫完全看不见。
- **变异锚点**：无。
- **注**：情形 (a) 的 schedule 串要能被 `occurrence_after` 算出「无下次」——若当前实现
  永远算得出（生产不可达），本卡登记为「不可激活」并记明理由。

---


## 二十、Windows 专章（本机 macOS 跑不了，登记延后）


### W28 Windows Job Object 限额与拆分后行为
- **步骤**：Windows 上跑 run_python 与 apps（长脚本 + 内存 hog 脚本各一）。
- **判据**：
  1. 内存超限的脚本**被限额拦住**（Job Object PROCESS_MEMORY 生效）；
  2. 宿主退出时子进程**跟着死**（KILL_ON_JOB_CLOSE）；
  3. **拆分后回归**：`py/win_job.rs` 拆出后行为与拆分前一致（限额、kill-on-close、
     `create_kill_on_close` 三条都在）；
  4. Job Object 创建失败时**留有审计**（本轮新加的留痕行）。
- **变异锚点**：V215。

### W29 F-2 真补评估（是否补 ACTIVE_PROCESS）
- **步骤**：Windows 上跑一个 fork 风暴脚本（`multiprocessing` 大量起进程），观察系统
  是否被拖垮。
- **判据**：
  1. 当前行为记录在案（无进程数上限，只有内存/CPU/超时约束）；
  2. 给出「补 `JOB_OBJECT_LIMIT_ACTIVE_PROCESS`」的实测结论——半生效的限额比没有更
     危险，本轮只把注释写准，能力补齐与否留给实机结论。
- **变异锚点**：无。
- **注**：本卡是**评估卡**不是验收卡，结论写回填位即可。

---


## 二十一、补漏改动的回归面（M224–M230）

> 这批是 11-16 ~ 11-18 里**前一份增量方案没写卡**的改动——它们同属审计修复批，
> 但落点分散在守卫终稿消费、子 agent 终态、定时作业、模型库种子、elicitation
> 能力声明与前端失败气泡六处，单开一节成卡便于跑完后统一对账。

### M224 终稿自检 verdict 消费顺序 + 子 agent 成功终态清残留 error
- **步骤**：(a) 让模型在终稿前触发一次终稿自检（模型自己回答「是否达到目标」）；(b) 派一个
  子 agent，先让它失败（带 error），再让它重跑成功。
- **判据**：
  1. 自检的 Y/N 判定被**消费一次**：判过之后同一回合不再重复触发返工（第二次判定
     不再回灌同一份提示）；
  2. 子 agent 成功终态后，它的行**不再残留上一轮的 error**（库证
     `SELECT status, error FROM subagents WHERE id=?` 与右侧栏显示一致）——
     修复前成功终态会把旧失败原因带在行上。
- **变异锚点**：V216、V217。

### M225 过期作业不静默删除 + 变更声称登记 + 思考流去重
- **步骤**：(a) 建一条一次性定时作业（2 分钟后到点），**在到点前把定时总开关关掉**，
  等它到点后 2 分钟；(b) 让模型用 `http_request`（saveAs 落盘）与 `update_plan` 后
  如实说「已保存/已更新」；(c) 开思考模式跑一轮，看思考文本。
- **判据**：
  1. 情形 (a)：那条作业**保留**在定时页（开关关闭期间判 Held，不派发也不清理），
     不静默消失；恢复开关后才被清理，且清理时有用户可见通知；
  2. 情形 (b)：「已保存/已更新」这类声称**不被守卫判成凭空变更**（不再推回重跑 →
     不出现重复下载）；
  3. 情形 (c)：思考文本**不重复拼接**（回灌重放与实时重叠时同一段思考只出现一次）。
- **变异锚点**：V218、V219。

### M226 提示词与白名单对齐 + 服务面板可见性闸
- **步骤**：(a) 派三个档位的子 agent各一次（recon/executor/verifier），看提示词里的
  工具清单；(b) 打开设置页的服务面板，对照运行中的受管服务。
- **判据**：
  1. 各档提示词里点名的工具**都在该档实际生效集内**（清单里没有会被服务端拒绝的
     工具）；反之生效集里的工具没有被漏列；
  2. 服务面板显示的受管服务与实际运行的一致（不可见的服务不出现，出现的可停）。
- **变异锚点**：V220。

### M227 同步 IO 挪出 async worker（性能面）
- **步骤**：在一轮里连续调 20 次工具（含 http_request 与读文件），全程观察界面响应。
- **判据**：
  1. 工具调用密集时**界面不卡顿**（流式增量照常刷新，主线程不被同步文件 IO 阻塞）；
  2. 事件顺序与改前一致（无回退：工具开始/结束事件仍成对出现）。
- **注**：本卡是**观感卡**，不设硬指标——改前后的差别在慢盘/杀软环境下才明显；
  macOS 本机多数情况观察不到差异，记「未观察到差异」即可。

### M228 elicitation 能力声明（真反问能发生）
- **步骤**：起一台官方 SDK 的 MCP 服务器（1.32.2 那套，需**按会话复用**），声明
  `elicitation.form` 能力并在工具处理里反问一次；再把该服务器的 elicitation 开关关掉。
- **判据**：
  1. 开关开：服务器**真的发起反问**，聊天区弹出问题卡，答完工具收到应答值；
  2. 开关关：能力集为空，服务器**拒发**反问，工具秒失败（零行为变化）；
  3. 反问的应答值**不进 bot.log**（与 M207 判据 1 同一道脱敏面）。
- **变异锚点**：V222。
- **环境依赖**：需官方 SDK 服务器；**必须按会话复用**（无状态接法会让处理
  tools/call 的实例从未 initialize，能力集恒 null——这个坑 11-16 已踩过）。

### M229 种子兜底成功也留痕
- **步骤**：清空模型库表（`meta_provider` 等），**断网**启动应用，让它走随包快照兜底。
- **判据**：
  1. 兜底**成功**也有审计事件 `meta_seed_models_dev`（带 providers / models 计数）——
     修复前只有失败分支才打，事后分不清「本地本来有种子」与「这次刚播的种」；
  2. 恢复网络后手动同步正常。
- **变异锚点**：V221。

### M230 send 层失败文案分层 + 失败气泡重试按钮
- **步骤**：断网（或指向一个不可达地址）后发一条消息，观察气泡；再点气泡上的重试按钮。
- **判据**：
  1. 气泡文案是**分层**的（「网络连接中断（成因）」+ 重试指引），**不含** reqwest 原始串
     （原始串只进审计）；
  2. 失败气泡**有重试按钮**且点它能重试（截尾重放同一条消息）；
  3. 重试成功后气泡转成功态。
- **注**：M199 已验过 /retry 命令面与末条判定，本卡只验**入口可见**与文案分层。

---

## 二十二、与前轮的关系 + 未覆盖诚实清单

- **与前轮**：11-15 方案（M182–M203 + W25–W27 + V172–V192）覆盖 v0.3.0 后七批
  46 提交；本份把它并入执行（那份方案写了但从未跑过），并追加 11-16~11-18 的
  18 笔代码改动（审计四批修复 + elicitation 两笔）。卡号与变异编号两段接续不重叠。
- **本轮新增面全是「闸与判定口径」**，卡位偏负向：M205（恶意 OAuth 服务器不得把宿主
  变成内网跳板）、M211（IPv6 过渡段必须被拒）是明确的负向卡；其余以「行为面 +
  审计面」双判据为主。
- **本轮无灰度开关**：OAuth 逐 URL 过闸、公网闸 IPv6 收紧、重定向切方法、守卫口径
  全部恒生效——对照面只能靠「字面量级」与「单测锚点」两条背书。
- **仍未覆盖（诚实清单）**：
  ① 11-08 白天四批（见 §十三）；
  ② Linux 降级档（M206 判据 2 需 Linux 无 keyring 环境）；
  ③ Windows 三张专章（W25–W27 + W28/W29，本机 macOS 跑不了）；
  ③' CSS 悬空变量清零（`096f5d9d`）是纯样式修复（五个变量引用改真变量），
     视觉面由前端测试背书，本份**不出实机卡**——若要人工过一眼，看设置页
     Toggle 轨道底色、输入框浮层边框、Word 模板行 hover 底色三处即可；
  ④ M223 判据 1 若 `occurrence_after` 在当前实现下永远算得出下次（生产不可达），
     则登记为「不可激活」并记明理由；
  ⑤ V190（真 ort 熔断行）与 V215（Windows Job 失败留痕）本机不可激活。
