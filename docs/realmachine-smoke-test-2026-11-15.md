# 实机冒烟测试方案：v0.3.0 后批次（旅游计划修复 / 模型库种子 / 子 agent 全链收官 / 定时提醒 / 流断流 / 重复闸）

> **覆盖范围**：`1b102e90..ab30e13c`（2026-11-08 23:49 → 11-15 09:16，共 46 笔，
> 其中 docs / chore / 纯测试 9 笔无独立卡），七批——
> ① **旅游计划修复批**（`1b102e90..c3b88e18` + `6a004a6a`，10 笔）：2h24m 空转的
> 根因链修复——check 等待步长钳制、等待期心跳、/stop 级联取消子 agent、汇报轮
> 放开提问、验收面收敛、搜索引擎互斥单开、窄栏块换行；
> ② **v0.3.0 发布**（`c99cf21a`，版本号单源，无行为面不出卡）；
> ③ **模型库抗抖 + 离线种子**（`28376f97` + `ad720c37`）：err_chain 根因链 /
> 退避重试 / 60s 超时 / 725KB 种子编进二进制；
> ④ **子 agent 全链收官批**（`519cd635` / `7dffbe83` + `682a8e76..e1a2d589`，
> 15 笔）：交付件齐即收尾、熔断出口恢复链、submit_result 交卷工具化、
> resume_subagent 跨进程续跑、会话闸与豁免收口六修；
> ⑤ **定时提醒批**（`180753c2..265bf729`，7 笔）：reminders 表 + 四命令 +
> SchedulePage 提醒页签 + 通知授权提示；
> ⑥ **流断流修复批**（`0cefe322..3fbf2002`，7 笔）：Windows 实机诊断的断流三层
> 叠加修复——err_chain 留痕 / `LlmRequestFailed` 分层文案 / 思考不毒化重发 +
> `bot-turn-restart` 清场 / 失败气泡重试按钮 / ort 预检 + 启动自检；
> ⑦ **重复调用死循环闸批**（`e1050d6c` / `b5d0a385` / `bab37f1a`）：同参 3 连调
> 拦截 + 可提问语境转问 ask 卡 + 提示词三处点名 ask_user。
> 方案底稿：断流与 ort 见 `docs/STREAM-BREAK-RECOVERY-AND-WINDOWS-RUNTIME-FIX-2026-11-14.md`。
>
> **卡片**：M182–M203（22 张主卡）+ W25–W27（3 张 Windows 专章）；变异注入锚点
> **V172–V192**（21 条，编号接 11-07 方案的 V158–V171；卡片号 M182 接 M181）。
>
> **配套验收卡**：ACC-O-13（SPEC 6-128 重复闸，`cards-06-orchestration.md`）、
> ACC-P-22（SPEC 6-127 提醒接定时模块，`cards-08-platform.md`）、ACC-E-05
> （语义性失败不降级 + 重试按钮判据，`cards-04-guard.md`）；执行完毕回填
> `docs/acceptance/matrix.csv` 对应行状态。
>
> **门禁前置**：基线 `ab30e13c` 上全量门禁绿（2026-11-15 实测：cargo nextest
> 2590 + vitest 1251 + pytest 审计对拍 58）；实机前重跑 `bash scripts/test-all.sh`
> 确认仍绿。**Mimosa 静态面**：11-08 深扫 24 条与既有误报基线同构（依赖 0 命中），
> 此后无新扫——执行前建议补一扫再开跑。
>
> **编号口径**：Windows 专章从 W25 起（W22–W24 已被 11-07 方案占用），变异从
> V172 起，卡片从 M182 起。

---

## 〇、跑法与口径

- **基线**：`ab30e13c`（或其后修复提交）。跑前 `git log --oneline -1` 记录实际基线。
- **数据口径**：测试阶段老数据可删（用户拍板）；直写 sqlite 时间戳列一律毫秒整数
  （`reminders` 表的 `remind_at` / `fired_at` 都是 INTEGER）。
- **杀进程**：macOS `pkill -x wmessage`（禁 `pkill -f`）；Windows `taskkill /IM wmessage.exe /F`。
- **定位数据目录**：启动后 bot.log 首部 `app_data_dir` 审计行。本轮判据落在
  data_dir 下：`bot.log`（全部事件证）、模型库 sqlite（`meta_provider` 表）、
  子 agent 产物目录（`gen_dir/<id>/session_history.json` sidecar）。
- **审计证**：本轮全部事件证在 **bot.log 文件行**（`[ts] LEVEL | event | k=v`），
  不在 sqlite——grep 模板见 §十。事件名断言用 `grep -c` 计数。
- **断网构造**：macOS 关 Wi-Fi 即可；模型库种子卡（M188）需要**启动期断网**，
  恢复网络后走设置页手动同步。流断流卡（M197/M198）需要**回合中途断网**——
  发消息后数 2 秒断网即可触发读侧失败。
- **无灰度开关**：本轮七批全部无 bot-config 开关（重复闸恒开、提醒恒开）——
  对照面靠豁免表与默认路径卡（M201 判据 3）。
- **重复闸误拦处置**：实机若发现合法工具被拦（轮询类不在豁免表），把工具名补进
  `src-tauri/src/bot/constants.rs` 的 `REPEAT_GATE_EXEMPT_TOOLS` 后重启，并在
  回填位备注——豁免原则=纯读/轮询/状态探测。
- **结果回填**：每卡 PASS / FAIL / 部分 + 证据，填 §九回填位；FAIL 带变异锚点编号。
- **时间预算**：22 张主卡约 3 小时；变异复跑 21 条（各需一次重建 + 复跑）另计
  3–4 小时；M190（真熔断构造）与 M192（续跑全链）单卡各预留 15 分钟。

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

## 八、变异注入锚点总表（V172–V192）

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

---

## 九、结果回填位

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

**变异执行台账**（V172–V192 逐条 KILLED / SURVIVED / 受限）：

**全部未跑**（预算账如实登记：22 卡 + 熔断双构造 + 断流多轮重跑占满本轮；
每条变异需单编辑+重建+复跑+还原约 10–25 分钟，21 条另计 3–4 小时；口径同
11-07 轮「卡片先跑、变异补跑另起」）。V183（resume 丢 onion 元列）与
V192（超时回落放行）两条必须 KILLED 级随 M192 受限面与 M202 PASS 面顺延至
补跑轮；M202 超时窗补跑可直接复用本轮 30s 变异法（§十二 点名允许）。

---

## 十、库证查询与日志证

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

## 十一、本轮事件名速查（判据引用）

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
  本份起点 `1b102e90` 正接其基线 `16895019` 之后的首批行为变更。M199 重试
  按钮复用 M166（11-07）的 /retry 截尾链路——那条已验过，本份只验入口与
  末条判定。
- **11-08 白天四批不在本份**（用户口径 11-09 起）：三档退役 `61db05ee` /
  底栏逐级图标化 `6282ef2f` / 授权四档并两档 `4d0c156d..a58fd3ec` /
  ask 卡行式重塑 `0b463e59+754a9551`——这四批**没有任何一份冒烟卡覆盖过**，
  实机验收缺口在案；如需补验另出补卡（ask 卡 UI 面 M202 可部分顺带覆盖）。
- **无独立卡的 9 笔**：`c99cf21a`（版本号单源）、`e44f88c3`（验收面收敛提示词，
  已并入 M189 判据 4）、`d4b07ccc` / `c3b88e18` / `314fc59a` / `e1a2d589` /
  `7c468604` / `4da4e812`（docs / 注册表注释 / 死代码清理——行为面由各自宿主
  卡覆盖）、`76a8f4cf` / `ab30e13c`（门禁修复：测试脚本与模块树登记，无生产
  行为面）。
- **L1 已钉不重复验**：wait 钳制计时、resume 事务原子性（翻行+待办同事务）、
  submit_result 会话闸、RMW 基线拒写、搜索引擎互斥保存原子性——实机卡只验
  行为观感与事件落档。
- **双窗 / 多窗面**：本轮无新窗口面（`emit_recorded` 扇出就绪已在上轮验过），
  不含。
