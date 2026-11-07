# 实机冒烟测试方案：Agent 运行时改造批 5–8（工作单元 / 机器可判据 / 需要人处理聚合 / 定时作业预演档）

> **覆盖范围**：`3f801143..e0f23813` 共 4 个提交（11-04 07:50 → 08:26）——
> 批 5 工作单元 `work_items`（每会话一行，stage 二分）、批 6 机器可判定验收随计划落行、
> 批 7「需要人处理」聚合面（查询时聚合三源进通知中心）、批 8 定时作业预演档（`mode` 三态）。
> 方案全文见 `docs/AGENT-RUNTIME-REFIT-PLAN-2026-11-02.md`（§七/§八/§九 + 修订 8 的四项拍板）。
>
> **顺带回归**：前序轮两条修复——D-1 `at:` 一次性作业首跑宽限（`cd1bec19`）、
> D-3 失败重试/自动暂停策略落库（`e5af9aee`），各一张回归卡（§五 M161/M162）。
>
> **卡片**：M137–M162（26 张主卡）+ W19–W21（3 张 Windows 专章）；变异注入锚点
> **V145–V157**（13 条，编号接 11-03 方案的 V136–V144；卡片号 M137 接 M136）。
>
> **门禁前置**：本批基线 `cargo nextest + vitest + pytest 审计` 全绿
> （2026-11-04 实测：cargo nextest 全量通过、vitest 121 文件 / 1188 例通过、
> 审计 58 例通过）；实机前重跑 `bash scripts/test-all.sh` 确认仍绿。
>
> **编号口径**：Windows 专章从 W19 起（W16–W18 已被 11-03 方案占用），
> 变异编号从 V145 起（V136–V144 已被 11-03 方案占用）。
>
> **配套验收卡**：`docs/acceptance/cards-06-orchestration.md` 的 ACC-P-13（SPEC 6-118）
> 与 ACC-P-14（SPEC 6-119）。追踪矩阵 `docs/acceptance/matrix.csv` 里这两行当前状态是
> 「未验 / 无实机卡」——**本份就是它们等的实机卡**；执行完毕后回填矩阵状态。

---

## 〇、跑法与口径

- **基线**：`e0f23813`（或其后的修复提交）。跑前 `git log --oneline -1` 记录实际基线。
- **数据口径**：测试阶段老数据可删、不做迁移（用户拍板）。清数据直接删库文件或
  直写 sqlite 均可，**但时间戳列一律写毫秒整数**——写 ISO 字符串会触发启动即崩
  （`task_from_row` 对 INTEGER 列零容忍，历史坑在录）。
- **杀进程**：macOS 用 `pkill -x wmessage`（**禁 `pkill -f`**——会自匹配把自己环境
  一起杀，历史事故）；Windows 用 `taskkill /IM wmessage.exe /F`（见 W19）。
- **定位数据目录**：启动后 bot.log 首部的 `app_data_dir` 审计行（kv `path`）。
- **库证**：所有卡的事件判据 + 库证双验。sqlite3 常用查询见 §七。
- **通知授权**：macOS 系统设置 → 通知 → wmessage；Windows 见 W16（沿用 11-03 方案）。
  M159 需要能随时开关它——先确认入口可达。**11-03 轮已知 macOS 上通知横幅不弹
  （D-2）**，本批 M159 的横幅半可能同样取不到，库证侧（effect_queue）可照取。
- **产物目录**：机器判据相对 `AI_Gen_Files`（`db/paths.rs:31-37`），
  构造产物时别放错目录。
- **结果回填**：每卡 PASS / FAIL / 部分 + 证据（事件行原文 + 库证查询输出），
  填进 §十的回填位。FAIL 必须带变异锚点编号（有锚点的卡），证明「测试有牙齿」。
- **时间预算**：26 张主卡里 M146（阶段重估）依赖变异、M156–M157 依赖预演档现场、
  M141（窄缝卡）与 M142（缺陷卡）各需一次杀进程 + 一次重建；
> 13 条变异锚点各需一次重建 + 复跑。**全套跑完预留 2–3 小时**。

---

## 一、工作单元 work_items（批 5）

> 表结构 `db/work_items.rs:16-31`（老库打开即建空表，无迁移）；
> 开票 `bot_chat/flow.rs:653-681`；收尾 `flow.rs:761-775`；
> 挂起 `ask_engine.rs:516-532`；启动清扫 `lib.rs:582-605`；
> tick 重估 `bot_scheduler.rs:1102-1112`（阈值 **2 小时**）。
> 阶段二分：person 族 = `undelivered / awaiting_answer / awaiting_review / blocked / failed`
> （`db/work_items.rs:39-45`）；working 族 = `queued / idle / working`。
> **批 5 只产出 working / awaiting_answer / idle / failed 四态**，其余三态是预留词。

### M137 开票：一轮主聊天建一行（基础卡）
- **步骤**：主聊天发一句会干活的话（如「帮我总结一下今天的日程」），等它跑完。
- **判据**：
  1. Q1 查 `work_items`：该会话**恰好一行**（`kind=chat`、`stage=idle` 或 `working`、
     `objective` = 本轮输入摘要 ≤40 字）；
  2. `session_id` 与会话 id 一致；`origin_ref` 为空（主聊天不挂任务卡，现状如此）；
  3. bot.log 有 `app_data_dir` 但**无**开票失败告警（开票失败不阻断聊天主链，
     `flow.rs:668` 的 `let _ =` 吞错——所以本卡以库证为准，不以日志为准）。
- **变异锚点**：V145。

### M138 每会话一行：连发两轮不增行、objective 刷新
- **步骤**：同一会话连发两条消息（第二条在第一条收尾后），都等跑完。
- **判据**：
  1. Q1 该 `session_id` 仍**只有一行**（`id` 不变）；
  2. `objective` 刷新为**最新一轮**的输入摘要；
  3. `created_at` 不变、`updated_at` 递增（`upsert_for_session` 只在无行时建行，
     `db/work_items.rs:96-130`）。
- **变异锚点**：V145（对照组：把 upsert 改成恒插新行 → 行数随轮次增长）。

### M139 汇报轮不覆盖 objective
- **步骤**：主聊天派一个子 agent，等它回报（**Report origin** 的一轮）。
- **判据**：
  1. 子 agent 回报到达后，该会话的 `objective` **仍是用户原话**，没被回报内容覆盖
     （`flow.rs:660-664`：`TurnOrigin::Report` 传 `None` objective）；
  2. `updated_at` 仍会刷新（汇报轮也开票，只是不改目标）。
- **注**：本卡是「批 5 取 objective 的口径」的正向验收，不涉及子 agent 自身行为
  （子 agent 生命周期由 11-03 方案的 M125–M128 覆盖）。

### M140 轮收尾：成功 → idle、失败 → failed
- **步骤**：
  - **成功半**：正常问一句能答上来的，等它答完；
  - **失败半**：构造一个失败的轮（可用 `HTTPS_PROXY=http://127.0.0.1:9` 指向本地死端口
    让模型调用必败——11-03 轮用过这招，或让它调一个会被闸拦的工具）。
- **判据**：
  1. 成功半：Q1 该行 `stage=idle`、`error` 为空；
  2. 失败半：`stage=failed`、`error` 非空（存失败原因，`finish` 的 `COALESCE` 语义
     ——**旧 error 不会被空值抹掉**，`db/work_items.rs:186-202`）；
  3. 失败半的行**立刻**出现在通知中心「待处理 → 后台待办」区块（person 族含 failed，
     交叉验 M152）。
- **变异锚点**：V146。

### M141 极短回合不留 working 残行（读码预判缺陷面，依赖变异）
- **说明**：开票与轮收尾都是 fire-and-forget 的 `spawn`
  （`bot_chat/flow.rs:653-681` 开票、`:761-775` 收尾），两者都按 `WHERE session_id = ?1` 写库。
  **若收尾的 UPDATE 早于开票的 INSERT 落地** → 收尾匹配不到行 →
  **静默 no-op**（`db/work_items.rs:194-201` 的 `execute` 返回 0 行不算错）→
  随后开票把行建出来，`stage` 停在 `working` → 直到 2h 后被 tick 重估收成 failed
  （`bot_scheduler.rs:1105`）。实践中开票比收尾早整整一个模型往返，几乎撞不上——
  **属窄缝，只能靠变异构造**。
- **步骤**：
  1. 打变异 V156（把开票的写库动作前插 `sleep 10s`，人为拉开开票与收尾的落地次序）
     → 重建 → 运行；
  2. 发一条**极短**请求（如「现在几点」，模型一轮就答完）；
  3. 等 60s 后查 Q1。
- **判据**：
  1. **变异态**：该行 `stage=working`（收尾被 no-op 吃掉，开票晚落地）——判据 FAIL 成立；
  2. **pristine 对照组**（同一场景不打变异）：该行 `stage=idle`（收尾生效），
     **不得**停在 working；
  3. pristine 下若也停在 working → 不是窄缝而是稳定缺陷，按 FAIL 记，并查 bot.log
     有无 `work_items_sweep_failed`（清扫出错会留痕）。
- **变异锚点**：V156（反向锚点：变异态暴露残行，pristine 不产生）。

### M142 崩溃后待答行不永久报警（**缺陷卡，现状预期 FAIL**）
- **说明**：启动清扫只收 `working` 行（`db/work_items.rs:220-228` 的 `WHERE stage='working'`），
  **`awaiting_answer` 不在其中**；而 `ask_user` 的 waiter 注册表在**内存**里
  （`app_state::question_waiters`）——进程一重启，等待器全部消失。
  于是：一轮挂在待答状态时进程崩溃 → 重启后那一行**永远停在 `awaiting_answer`**
  → 它是 person 族 → 「后台待办」**永久报警**，且用户没有「作废」入口能消掉它。
- **步骤**：让 bot 调 `ask_user` 提一个问题（**先不答**）→ 在挂起状态下杀进程
  → 等 10s 重启 → 看该行 stage 与「后台待办」。
- **判据**：
  1. 现状下**预期 FAIL**：该行仍为 `awaiting_answer`，且在「后台待办」里长期挂着
     （本卡就是来暴露这个缺陷的，记 FAIL 并附源码证据）；
  2. **修复方向已定**：启动清扫把 `awaiting_answer` 一并按中断收——进程重启后
     没有活着的 waiter，那个等待已无意义；修复后该行应为 `failed` 且带「进程重启」类 error；
  3. **不得**误伤**仍在活着的**待答行——未重启的应用里提问等待可达 24h 属合法长挂
     （见 M143 判据 2）。即清扫只在**启动**时做，**不进 tick**。
- **变异锚点**：V157（**修复型**：把启动清扫的 `WHERE stage='working'` 扩到
  `IN ('working','awaiting_answer')` → 该行转 failed、报警消失 → 判据转 PASS，
  证明本卡判据有牙齿）。

### M143 ask_user 挂起 → awaiting_answer → 回答后回 idle
- **步骤**：让 bot 调 `ask_user` 提一个问题；**先不答**（等 ≥30s，让 tick 跑过一轮）；
  再在提问卡上作答，把话聊完。
- **判据**：
  1. 提问后 Q1 该行 `stage=awaiting_answer`（`ask_engine.rs:516-532`）；
  2. **静置 30s+ 后仍是 awaiting_answer**——`sweep_stale_working` 只处理 `working`
     族（`db/work_items.rs:232-241`），提问等待可达 24h 属合法长挂，**不得**被误收；
  3. 回答并聊完后 `stage=idle`；
  4. 挂起期间该行出现在「后台待办」（awaiting_answer 是 person 族）。
- **变异锚点**：V146 变体（把 stage 常量写死 idle → 挂起不可见）。

### M144 失败行被新一轮唤醒 → working（重试语义）
- **步骤**：在 M140 的失败半之后，**同一会话**再发一条消息（让它干活），等它跑完。
- **判据**：
  1. 新一轮开票后 Q1 该行 `stage` 从 `failed` 回到 `working`
     （`upsert_for_session` 的 `stage = CASE WHEN stage='failed' THEN 'working'`，
     `db/work_items.rs:109`）；
  2. **error 保留**（不被新一轮抹掉——`upsert_for_session` 不碰 error 列；
     `finish` 用 `COALESCE` 也只在新值非空时覆盖）；
  3. 轮跑完后 `stage=idle`（成功）或 `failed`（再失败），`error` 为最新一轮的结果。
- **注**：这是「失败 → 重试」在**阶段语义**上的表现，不是子 agent 重派。

### M145 启动清扫：working 行随进程消失按中断收（核心卡）
- **步骤**：起一个**跑得久**的轮（如让它读多个文件再总结），在**执行中**杀进程，
  等 10s 重启。
- **判据**：
  1. 重启后 Q1 该行 `stage=failed`、`error` 含「进程重启，执行现场已不存在」
     （`sweep_all_working`，`db/work_items.rs:220-228`）；
  2. 被清扫的行**进了**「后台待办」（failed 是 person 族）；
  3. 清扫**不误伤** idle 行（idle 是已结束的活，不该被改成 failed）；
     **`awaiting_answer` 行的处置见 M142**——现状下它不在清扫范围（缺陷卡），
     本卡不把「awaiting_answer 被收」作为通过条件；
  4. bot.log **无** `work_items_sweep_failed`（`lib.rs:598-604` 只在出错时落审计）。
- **变异锚点**：V147。

### M146 tick 阶段重估：working 超 2h 无更新按中断收（依赖变异）
- **说明**：阈值写死 2 小时（`bot_scheduler.rs:1105`：`2 * 60 * 60 * 1000`），
> **pristine 下手工不可达**（要等 2 小时）。本卡**依赖变异**把阈值改小。
- **步骤**：打变异 V148（`stale_ms` 改 10s）→ 重建 → 起一个长轮（>10s 无更新）→
  静置 60s → 看该行 stage。
- **判据**：
  1. 变异态：该行 `stage=failed`、`error` 含「轮超时无更新，按中断收尾」
     （`db/work_items.rs:236`）；
  2. **对照组**（pristine，同一场景）：该行**不**被收成 failed，仍是 working
     ——证明差异来自阈值而非别的路径；
  3. 被打变异期间**不误伤** awaiting_answer 行（它不参与重估）。
- **不打变异的诚实结论**：pristine 侧只能验「2 小时窗口内不误收」。

---

## 二、机器可判定验收（批 6）

> 解析 `bot_plan.rs:254-283`（只收非空字符串、**最多 5 条**、缺字段不算计划非法）；
> 落行 `bot_chat/flow.rs:676-680`（`set_acceptance` 写 JSON 数组）；
> 判定 `db/work_items.rs:310-377`（复用 `eval_machine_criteria`，**fail-closed**）；
> 结论进 `result` JSON 的 `machineCheck.{passed,total}`，审计 `work_item.machine_check`。
> **判据只记录、不改判阶段**——改判推翻语义仍归子 agent 收尾契约。

### M147 Planner 声明的验收标准落行（模型依赖卡）
- **前置**：任务要**触发 Planner**——`needs_plan` 命中多步关键词（然后/之后/接着/批量/
  分别/依次/按步骤/分步/逐步/计划…）或「先…再…」或 ≥2 个附件
  （`bot_plan.rs:50-58`）。**单步问题不会触发**，别用「现在几点」试。
- **步骤**：发一个多步且**产出文件**的任务，如「先读一下今天的日程然后生成一份 报告.md 到
  AI_Gen_Files」。
- **判据**：
  1. bot.log 有 `plan.generate`（`bot_plan.rs:302-308`，含 `steps` 与 `acceptance` 两个 kv）；
  2. Q4 查该会话行的 `acceptance` 列：非空 JSON 数组，元素形如
     `file_exists:报告.md` / `file_nonempty:xxx.csv`；
  3. 数组长度 **≤5**（解析层上限，`take(5)`）。
- **模型不配合怎么办**：Planner 不给 acceptance → 本卡**按未覆盖记**并写明
  「模型未输出 acceptance」；改用 M148 的**直写库证路径**继续验判定管线
  （下一步给方法）。

### M148 轮收尾判定：产物在 → machineCheck 全过（核心卡）
- **步骤**（**绕开模型输出的确定性路径**，推荐）：
  1. 起一个会干活但不产文件的轮（如「帮我查一下今天星期几」）；
  2. **轮执行中**直写 sqlite，把该会话行的 `acceptance` 改成
     `["file_exists:冒烟产物.txt"]`；
  3. 在 `AI_Gen_Files` 下放一个非空文件 `冒烟产物.txt`；
  4. 等本轮收尾。
- **判据**：
  1. bot.log 有 `work_item.machine_check`，kv 含 `passed=1`、`total=1`；
  2. Q4 该行 `result` 列含 `"machineCheck":{"passed":1,"total":1}`；
  3. `stage` **不受判据影响**（成功轮仍是 idle）——判据只记录不改判；
  4. 产物为空文件时同一判据应判**不通过**（fail-closed，对照 M149）。
- **变异锚点**：V149。

### M149 fail-closed：产物缺失 / 空文件 → 判不通过，且不改判阶段
- **步骤**：同 M148 的直写路径，但把判据改成**不存在的文件名**
  （`file_exists:不存在的文件.txt`）；再跑一轮，判据改成**空文件**
  （`file_nonempty:空文件.txt`，先 `touch` 出 0 字节）。
- **判据**：
  1. 缺失半：`passed=0`、`total=1`，`work_item.machine_check` 的 `failed` kv
     列出未过的判据原文；`result` 里 `machineCheck.passed=0`；
  2. 空文件半：同样判不过（`file_nonempty` 对 0 字节文件判 false，
     `bot/subagent_digest.rs:336-340`）；
  3. **阶段不被改判**：轮成功收尾仍是 idle（判据不推翻模型自述的成功）；
  4. 判据路径按 `AI_Gen_Files` 相对解析——把文件放到别处（如桌面）应判不过。
- **变异锚点**：V150。

### M150 判据语法非法 / 空数组 → 不产生机器结论
- **步骤**：把该会话行的 `acceptance` 直写成 `["这不是判据语法"]`，跑一轮收尾；
  再写成 `[]` 跑一轮。
- **判据**：
  1. 非法语法半：bot.log **无** `work_item.machine_check`（解析不了就跳过，
     `subagent_digest.rs:325-327`），`result` 里**无** `machineCheck`；
  2. 空数组半：同样无 `machineCheck`（`db/work_items.rs:330-332` 提前返回）；
  3. 两种情况**阶段照常落**（idle/failed），聊天主链不受影响。
- **注**：这是「判据只认 file_exists:/file_nonempty:」的边界卡，也是「静默跳过」
> 的诚实性卡——跳过不报错，用户需要知道（结论写在 §十一）。

### M151 旧 acceptance 跨轮沿用（读码预判缺陷面）
- **说明**：`upsert_for_session` 的 UPDATE **不碰 acceptance 列**
> （`db/work_items.rs:106-119` 只改 objective/stage/updated_at），而
> `set_acceptance` 只在**本轮有计划且计划带 acceptance** 时才调用
> （`bot_chat/flow.rs:676-680`）。**推论：上一轮写下的 acceptance 会在新一轮
> 没有 acceptance 时继续生效**，机器判定可能对着过期判据下结论。
- **步骤**：M148 已写过 acceptance 的会话 → 再发一轮**不触发 Planner** 的问题
  （如「现在几点」）→ 等收尾。
- **判据**：
  1. 该轮收尾**仍**产生 `work_item.machine_check`（沿用旧判据）；
  2. 结论针对的是**旧**判据的文件名；
  3. Q4 确认 `acceptance` 列在本轮未被清空。
- **判定口径**：若 1 成立，这是**设计缺口**（跨轮沿用过期判据），登记为发现，
> 不算 FAIL——但要写进报告的发现清单，供拍板是否改。

---

## 三、「需要人处理」聚合面（批 7）

> 口径：**查询时聚合**（拍板），三源合并后按时间倒序截断——
> 工作项 person 族 / failed 与 budget_exceeded 的子 agent / **失败且启用**的定时作业
> （`db/work_items.rs:396-452`）。**不新增 notifications kind、不改 notifications 表**。
> 前端：通知中心「待处理」页签顶部「后台待办」区块
> （`NotificationsPage.tsx:467-500`，前端取 `limit=30`，`:393`）；
> 加载失败降级空表，不炸通知主列表（`:392-402`）。

### M152 通知中心「后台待办」区块渲染
- **步骤**：制造三源各一条（工作项 failed 见 M140；失败子 agent 见 M153；失败作业见 M153），
  打开通知中心 → 「待处理」页签。
- **判据**：
  1. 页签**顶部**出现「后台待办」区块（标题行 + 卡片列表）；
  2. 每条带**来源标签**（工作项 / 子 Agent / 定时作业）、时间、标题、错误摘要
     （有 `detail` 时）；
  3. 区块内条目**不含**通知表原有 4 类 kind（memory_proposal / artifact_bind /
     task_question / service_alert）——那是下面原有的 pending 列表，两者是并列关系；
  4. 切到「全部」页签时**不显示**该区块（只挂在 pending 上）。

### M153 三源各自的进入条件
- **步骤**：逐源构造并观察是否入列。
- **判据**：
  1. **工作项源**：person 族五种 stage 都入列（`undelivered` / `awaiting_answer` /
     `awaiting_review` / `blocked` / `failed`）；`working` / `idle` / `queued`
     **不入列**（working 族是「自己在推进」）；
  2. **子 agent 源**：`failed` 与 `budget_exceeded` 入列，`succeeded` / `cancelled`
     **不入列**（`db/work_items.rs:420-433`）；
  3. **定时作业源**：`last_status='fail'` **且** `enabled=1` 的作业入列
     （`db/work_items.rs:435-445`）；把该作业暂停（enabled=0）后**不再入列**；
     `last_status='ok'` 的作业不入列。
- **变异锚点**：V153（去掉子 agent 源 → 聚合面漏源）。

### M154 排序、截断与加载失败降级
- **步骤**：
  - **排序/截断半**：制造 ≥5 条不同时间的待办（可直写 sqlite 造时间差，注意
    `updated_at` 等时间列写**毫秒整数**），看区块内顺序；
  - **降级半**：临时把 `needs_attention_list` 弄失败（改命令名或让 DB 锁库），
  刷新通知中心。
- **判据**：
  1. 区块内条目按时间**倒序**（新的在上，`out.sort_by_key(Reverse(at))`）；
  2. 截断按 `limit`（前端取 30）——条目超限时取最新的 30 条，不报错；
  3. 降级半：通知**主列表照常显示**，只是「后台待办」区块消失；前端 console 有
     `[notifications] 后台待办加载失败:`（`NotificationsPage.tsx:400`）；
  4. 降级半**不产生**未捕获异常、不白屏。

---

## 四、定时作业预演档（批 8）

> `mode` 列（`db/schedule_jobs.rs:24-31` DDL + `:56-60` 补列，**老库自动补**）：
> `NULL`/`auto` = 到点直接执行（存量不变）、`preview` = 预演、`off` 沿用既有
> `enabled=false` 暂停语义。预演分支 `bot_scheduler.rs:540-584`：
> **建卡 + 广播照常**，然后只记一条 `preview` 执行历史 + 发「⏳ 定时任务预演」通知，
> **不执行**（不跑模型、不建 ⏰ 会话）。非法 mode 值明确拒绝（`:436-446`）。

### M155 切档持久化 + 非法值拒绝
- **步骤**：作业页找一个**内容型**作业（`kind='job'`），点行内「自动 → 预演」钮；
  再切回；另用开发者工具或直连 invoke 传一个非法 mode。
- **判据**：
  1. 按钮文案在「自动」/「预演」间切换（`SchedulePage.tsx:342-358`，
     `e.mode === 'preview'` 决定文案与高亮）；
  2. Q5 查 `scheduled_jobs` 表的 `mode` 列：切到预演后为 `preview`，切回后为 `auto`
     （**落库不是前端态**——`db/schedule_jobs.rs:212-232` 的 upsert 带 mode 列）；
  3. **非法值被拒**：`mode="dry_run"` 之类 → 报 `InvalidArgument`，文案含
     「mode 只接受 auto / preview」（`:436-446`），且**库里的 mode 不变**；
  4. 非内容型作业（app 型）**不显示**该按钮（`e.kind === 'job'` 才渲染）。

### M156 到点预演：建卡不执行 + 历史 + 通知（核心卡）
- **步骤**：建一个 `at:+2m` 的作业 → 切到「预演」档 → 等它到点。
- **判据**：
  1. 到点后**出现一张提案卡**（看板 Todo 列，卡片内容是作业内容）——预演档
     **照样建卡**，这是设计（「卡即提案」）；
  2. **没有** ⏰ 执行会话、**没有**模型调用痕迹（不跑模型、不跑工具）；
  3. 通知中心/系统通知有「⏳ 定时任务预演：…」（走 Effects 队列，key
     `sched_preview:{job_id}`，`:559-569`）；bot.log 有 `sched_job_preview`（`:579-582`）；
  4. Q6 执行历史多一条 `status='preview'` 的记录，summary 含「预演：已建提案卡，未执行」；
  5. 作业行**徽标不变**（预演不改 `last_status`——见 M160）。
- **变异锚点**：V151（删掉预演分支 → 到点直接执行，预演档形同虚设）。

### M157 「立即执行」确认真跑（**读码预判缺陷卡**，核心）
- **规格口径**（SPEC 6-119 / ACC-P-14 判据 2）：点行内「立即执行」→ **正常走完整
  执行链**（会话、结果写卡、完成通知），预演档**不拦**手动触发。
- **读码预判**：手动入口 `scheduled_job_fire`（`bot_scheduler.rs:1041-1068`）在
  `:1066` **直接调用 `run_scheduled_job(app, job)`**——与到点触发**同一个函数**；
  而预演闸在函数入口 `:542`（`job.mode == preview` → 记历史 + 通知后 `return`）。
  **推论：手动「立即执行」极可能被同一道预演闸拦下，只再建一张提案卡，不真跑。**
- **步骤**：在 M156 的预演档作业上点「立即执行」。
- **判据（按规格写，不按预判写）**：
  1. 应出现**真实执行**：⏰ 执行会话、模型调用、结果写卡、「定时任务完成」通知；
  2. 该轮**不应**再记一条 `preview` 历史、不应再发「⏳ 定时任务预演」通知。
- **若实测确被拦下**：本卡记 **FAIL**，按发现登记（附源码证据
  `bot_scheduler.rs:1066` → `:542`），并补跑**修复型变异** V152
  （手动路径绕过预演闸）确认判据可转 PASS——**变异能修好，说明判据本身有牙齿**。
- **注**：这是本批**风险最高**的一张卡，跑的时候盯紧。

### M158 存量 / NULL 作业到点照常执行（回归卡）
- **步骤**：找一个**没切过档**的存量作业（`mode` 为 NULL）到点。
- **判据**：
  1. 照常执行（模型调用、结果写卡、完成通知）——**存量行为不变**是本批的硬要求；
  2. **无**「⏳ 定时任务预演」通知、**无** `preview` 历史行；
  3. Q5 确认该作业 `mode` 仍为 NULL 或 `auto`，执行不因档位改变。

### M159 预演通知走副作用队列（失败不静默）
- **步骤**：关闭系统通知授权 → 预演档作业到点 → 静置 90s；再恢复授权。
- **判据**：
  1. Q7 查 `effect_queue` 有 `kind='notify'`、`key='sched_preview:{job_id}'` 的行，
     `attempts` 逐次 1→5；
  2. 约 30s 后 bot.log 出现 `effect_giveup`（退避 2→4→8→16s，第 5 次失败即 give-up，
     与 11-03 方案的 M117 同一条序列）；
  3. give-up 后该行**留库**（`attempts=5`、`error` 非空），**不刷屏**；
  4. 恢复授权后**同 key** 再入队 → `attempts` 归零（`generation+1`）→ 投递成功、删行。
- **变异锚点**：V154 可复用 M117 的注入（`WM_INJECT_NOTIFY_FAIL`），
> 但**本卡优先用真实授权开关**；macOS 横幅不可达时按 11-03 的 D-2 口径记。

### M160 预演不改作业徽标
- **步骤**：M156 的预演作业跑完后，看作业行徽标与 Q5。
- **判据**：
  1. `scheduled_jobs` 表的 `last_status` 列**不被写成** `'preview'`——预演分支只写
     `scheduled_job_runs`（`record_job_run_in_tx`，`:546-557`），**不碰 job 状态**；
  2. 徽标显示的是**上一次真实执行**的结果（未执行过则为空）；
  3. 预演**不触发**失败重试计数、不触发自动暂停（`retry_at` / `retry_count` 不变）。
- **变异锚点**：V155（预演时把 `last_status` 也写成 `preview` → 徽标口径漂移，
> 且批 7 的作业源聚合（判 `last_status='fail'`）会跟着漂）。

---

## 五、前序轮修复回归（顺带覆盖）

### M161 D-3 回归：失败重试 / 自动暂停策略随 upsert 落库
- **背景**：11-03 方案的 M132 判 **FAIL**——`upsert_scheduled_job` 不写
  `retry_max` / `pause_on_failure` 两列，UI 上的失败策略**从未落库**
  （报告 D-3）。修复提交 `e5af9aee`。
- **步骤**：建一个注定失败的作业（让它调一个会被闸拦的工具），**并把失败策略设为**
  「重试 2 次 + 自动暂停」→ 到点失败 → 观察。
- **判据**：
  1. 失败后 Q5 该作业 `retry_max=2`、`pause_on_failure=1`（**列有值**）；
  2. `retry_at` 非空、`retry_count=1`、`last_status='fail'` 徽标即时更新；
  3. 5 分钟后自动重试（`RETRY_DELAY_MS = 5 × 60 × 1000`），bot.log 有 `sched_job_retry`；
  4. 重试用尽后 `sched_job_autopaused`（开了自动暂停时），作业被暂停。
- **对照**：11-03 轮同一场景 `retry_at` / `retry_count` 为空且无重试事件。

### M162 D-1 回归：`at:` 一次性作业到点不被过期清理抢先删除
- **背景**：11-03 轮新发现 D-1（P1）——`at:` 一次性定时作业**永远不会执行**，
> 到点即被判过期删除。修复提交 `cd1bec19`（恢复首跑宽限）。
- **步骤**：建一个 `at:+2m` 的一次性作业，等它到点。
- **判据**：
  1. 到点后作业**照常执行**（不是被删）；
  2. 执行后作业按一次性语义被清理（用完即删属正常，对照项）；
  3. 对照 11-03 轮的失效态：作业从未执行、到点即消失。

---

## 六、Windows 专章

### W19 工作单元启动清扫（对照 M145）
- Windows 杀进程：`taskkill /IM wmessage.exe /F`；其余步骤与判据同 M145
  （working 行 → failed、error 含「进程重启」、idle 行不被误伤；awaiting_answer 行的
  现状缺陷见 M142）。
- 附加观察：Windows 是硬杀（无信号语义），清扫判定与 macOS 一致。

### W20 预演档「立即执行」（对照 M157）
- 同 M157 步骤，判据同。
- **重点**：M157 若在 macOS 判 FAIL，Windows 大概率同因（共用 `run_scheduled_job`，
> 与平台无关）——记为**同源**而非独立缺陷；修复只需改一处。

### W21 通知中心「后台待办」+ 预演通知横幅（对照 M152 / M156）
- 「后台待办」区块渲染、来源标签、排序截断（对照 M152/M154）；
- 预演通知横幅在 Windows 的弹出与授权开关路径（设置 → 系统 → 通知 → wmessage），
  对照 M159 的队列侧结论。

---

## 七、库证查询（sqlite3 模板）

```bash
DB="<app_data_dir 路径>/wmessage.db"   # 路径从 bot.log 首部 app_data_dir 行取

# Q1 工作项全览（每会话一行的直观核对）
# 列：id / kind / origin_ref / session_id / objective / stage / error
sqlite3 "$DB" "SELECT substr(id,1,12), kind, IFNULL(origin_ref,'-'), substr(IFNULL(session_id,'-'),1,12), substr(IFNULL(objective,'-'),1,24), stage, IFNULL(error,'-') FROM work_items ORDER BY updated_at DESC LIMIT 20;"

# Q2 单行详情（stage / objective / acceptance / result / 时间戳）
sqlite3 "$DB" "SELECT stage, objective, IFNULL(acceptance,'-'), IFNULL(result,'-'), created_at, updated_at FROM work_items WHERE session_id='<sid>';"

# Q3 person 族（= 批 7 聚合面的工作项源；working 族不在此列）
sqlite3 "$DB" "SELECT stage, substr(IFNULL(objective,'-'),1,30), updated_at FROM work_items WHERE stage IN ('undelivered','awaiting_answer','awaiting_review','blocked','failed') ORDER BY updated_at DESC;"

# Q4 机器判据：acceptance 列与 result 里的 machineCheck
sqlite3 "$DB" "SELECT IFNULL(acceptance,'-') FROM work_items WHERE session_id='<sid>';"
sqlite3 "$DB" "SELECT IFNULL(result,'-') FROM work_items WHERE session_id='<sid>';"

# Q5 定时作业：执行档 + 失败策略（D-3 回归看 retry_max / pause_on_failure）
sqlite3 "$DB" "SELECT substr(content,1,16), IFNULL(mode,'NULL'), last_status, retry_max, pause_on_failure, retry_at, retry_count, enabled FROM scheduled_jobs;"

# Q6 执行历史（预演行 status='preview'；真实执行行 status='ok'/'fail'）
sqlite3 "$DB" "SELECT job_id, status, fired_at, IFNULL(summary,'-') FROM scheduled_job_runs ORDER BY id DESC LIMIT 10;"

# Q7 副作用队列（预演通知 key=sched_preview:{job_id}）
sqlite3 "$DB" "SELECT kind, key, attempts, due, IFNULL(error,'-') FROM effect_queue ORDER BY seq;"

# Q8 子 agent 状态（批 7 聚合面的子 agent 源：failed / budget_exceeded 入列）
sqlite3 "$DB" "SELECT id, status, substr(IFNULL(error,'-'),1,40) FROM subagents ORDER BY created_at DESC LIMIT 10;"

# Q9 看板任务卡（预演提案卡是否真的建出来了）
sqlite3 "$DB" "SELECT substr(content,1,24), status FROM tasks WHERE created_at > <毫秒时间戳> ORDER BY created_at DESC LIMIT 10;"

# Q10 本批落库的两张新表/新列是否存在（老库自动补列，应为 0 缺失）
sqlite3 "$DB" "PRAGMA table_info(work_items);"
sqlite3 "$DB" "PRAGMA table_info(scheduled_jobs);" | grep -w mode
```

> 直写 sqlite 清数据 / 造数：时间戳列（`updated_at` / `created_at` / `fired_at` / `due`）
> 必须毫秒整数，写 ISO 字符串会启动即崩（INTEGER 列零容忍）。
> **造 acceptance** 时注意它是一列 JSON 数组文本：
> `UPDATE work_items SET acceptance='["file_exists:冒烟产物.txt"]' WHERE session_id='<sid>';`

---

## 八、变异注入锚点总表（V145–V157）

> 编号接 11-03 方案的 V136–V144。**每条变异 = 改一个字符/一行 → 重跑对应卡 →
> 判据应当 FAIL**。
> **注入前置**：改之前 `git stash` 或建 checkpoint 分支；**改完必须还原**
> （`git status` clean 才算收尾干净）。
> **注入位置以本表列出的 `file:line` 为准**——行号是 `e0f23813` 现状，
> 别的提交动过同一文件后要按符号名重新定位。

| # | 卡 | 注入点（file:line） | 改动 | 重跑后应 FAIL 的现象 |
|---|---|---|---|---|
| V145 | M137 / M138 | `db/work_items.rs:96-130` `upsert_for_session` | 把「有行则 UPDATE」分支改成「恒 INSERT 新行」 | 同会话行数随轮次增长（每轮多一行），`session_id` 唯一索引报错或行重复 |
| V146 | M140 / M143 | `db/work_items.rs:193` `finish` | `let stage = if ok { idle } else { failed }` → `let stage = idle` | 失败轮不落 failed（行停在 idle），「后台待办」不出现该条 |
| V147 | M145 | `db/work_items.rs:220-228` `sweep_all_working` | 改成 no-op（不 UPDATE） | 重启后 working 行残留（stage 仍 working），不按中断收 |
| V148 | M146 | `bot_scheduler.rs:1105` | `2 * 60 * 60 * 1000` → `10 * 1000`（阈值改小，验证重估生效） | **反向锚点**：变异态该行被收成 failed（error「轮超时无更新」），pristine 不收——用来证明重估路径存在 |
| V149 | M148 | `db/work_items.rs:334` | `eval_machine_criteria` 调用改成返回空 vec | 无 `machineCheck` 结论、无 `work_item.machine_check` 审计 |
| V150 | M149 | `bot/subagent_digest.rs:333-341` | fail-closed 反向：目录 / 空文件也判 `met=true` | 缺失产物与空文件都判通过（passed=1），错误结论被当成真 |
| V151 | M156 | `bot_scheduler.rs:542` | 删掉整个预演分支（`if job.mode == preview { ... }`） | 到点直接执行、无预演通知、无 preview 历史——预演档形同虚设 |
| V152 | M157 | `bot_scheduler.rs:1041-1068` `scheduled_job_fire` | **修复型变异**：手动路径绕过预演闸（把 mode 临时置 None 再调 `run_scheduled_job`） | 手动「立即执行」真跑成功 → 证明 M157 的判据有牙齿、失败确由预演闸拦下所致 |
| V153 | M153 | `db/work_items.rs:420-433` | 去掉子 agent 源（循环体不 push） | 聚合面漏源：失败子 agent 不出现在「后台待办」 |
| V154 | M159 | `bot/effects.rs` `record_failure` | `attempts = attempts + 1` → `attempts = attempts` | 预演通知永不过限、无 give-up、无限重试 |
| V155 | M160 | `bot_scheduler.rs:546-557` | 预演记历史时**额外**把 `scheduled_jobs` 的 `last_status` 列写成 `'preview'` | 作业徽标显示 preview，且批 7 作业源聚合口径漂移 |
| V156 | M141 | `bot_chat/flow.rs:668-681`（开票写库处） | 在开票的写库动作前插 `sleep 10s`，人为拉开开票与收尾的落地次序 | 极短回合后该行停在 `working`（收尾 no-op 被吃掉）；pristine 同场景为 `idle` |
| V157 | M142 | `db/work_items.rs:220-228` `sweep_all_working` | **修复型变异**：清扫条件从 `stage='working'` 扩到 `IN ('working','awaiting_answer')` | 崩溃后待答行转 `failed`、永久报警消失 → 判据转 PASS（证明本卡有牙齿） |

---

## 九、本批事件名速查（判据引用）

工作项：`work_item.machine_check`（kv：`session` / `passed` / `total` / `failed`）·
`work_items_sweep_failed`（只在清扫出错时出现，正常路径不应见）
调度：`sched_job_preview`（kv：`id` / `card`）· `sched_job_done` · `sched_job_err` ·
`sched_job_retry` · `sched_job_autopaused` · `sched_job_skip` ·
`notify_enqueue_failed`（预演通知入队失败时）
计划：`plan.generate`（kv：`steps` / `acceptance`）· `plan.replan`
队列：`effect_giveup` · `subagent_dispatch_giveup`

> **本批新增的两条事件名已登记进 `docs/acceptance/event-names.md`**（`sched_job_preview`、
> `work_item.machine_check`，白名单总数 258 → 260），`audit_acceptance_matrix.py` 对拍绿。
> 跑卡时若 bot.log 里 grep 不到这两条——**先查源码再判缺陷**（`work_item.machine_check`
> 只在行上有 acceptance 且判据非空时才落，见 M150）。

---

## 十、结果回填位

> 执行者按卡逐条填。**PASS / FAIL / 未覆盖 三选一**，不要留空。**未覆盖必须写清
> 为什么**（环境不具备 / 卡与卡之间没来得及复位 / 模型不配合 / 构造不出来）。

### 10.1 逐卡结论

> 全量证据与细节见 `docs/realmachine-smoke-report-2026-11-04.md`（截图 /tmp/smoke-2026-11-04/）。
> 基线 e0f23813；测试期密钥走 env 覆盖（临时补丁已拆），全程零钥匙串访问。

| 卡 | 结论 | 证据 | 备注 |
|---|---|---|---|
| M137 开票建行 | PASS | working@t+2s→idle@t+14s；wi_c742ed40dd4 单行 chat/origin_ref 空 | 老库首启自动建表 ✓ |
| M138 每会话一行 | PASS | 行数 1、id/created_at 不变、objective 刷新、updated_at 递增 | |
| M139 汇报轮不覆盖 objective | PASS | 首派 500 重派成功（m139.txt 落盘）；objective 保持原话；updated_at 汇报轮刷新 | MiniMax 500 偶发 |
| M140 轮收尾 idle/failed | PASS | 死代理失败行 failed+error；失败行即时入后台待办（截图） | |
| M141 极短回合不留 working 残行 | PASS | pristine 多轮旁证全 idle；V156 变异态行停 working | pristine 窄缝不可达，靠变异 |
| M142 崩溃后待答行不永久报警 | FAIL（预期） | 崩溃后永停 awaiting_answer；V157 修复型转 PASS | 缺陷卡；**已修 e43e949d** |
| M143 ask_user 挂起 | PASS | awaiting_answer@t+12s；静置 32s 不误收；作答后 idle；person 族在列 | +F-1 选项 pill 点击疑点 |
| M144 失败行唤醒 | PASS | failed→working→idle；error 保留（COALESCE） | |
| M145 启动清扫 | PASS | working 行转 failed+「进程重启」；idle 零误伤；无 sweep_failed | V147 变异态残留不收 ✓ |
| M146 tick 阶段重估 | PASS | pristine 60s/95s 不收；V148 阈值 10s 变异态 t+20s 收「轮超时无更新」 | |
| M147 acceptance 落行 | PASS | plan.generate steps=5 acceptance=2；落行 2 条 file_exists/nonempty | 轮次失控插曲（F-2） |
| M148 machineCheck 全过 | PASS | 直写 acceptance+产物 → passed=1/total=1 + 审计 + 阶段不改判 | V149 变异零审计 ✓ |
| M149 fail-closed | PASS | 空文件/别处文件/缺失文件全判不过，failed 列原文；阶段不改判 | V150 反向 passed=2/2 ✓ |
| M150 非法判据静默跳过 | PASS | 非法语法/空数组无 machineCheck，审计 4→4 | |
| M151 旧 acceptance 跨轮沿用 | 设计缺口 | 无 Planner 轮沿用旧判据再判定、列不清空 | 登记 D-5（=R-2）**已修 e43e949d** |
| M152 后台待办区块 | PASS | 待处理顶部区块+来源标签；全部页签无区块 | |
| M153 三源进入条件 | PASS | person 族含预留词入列/working·idle 不入列；budget_exceeded 入列；fail+enabled 入列 | V153 砍源漏源 ✓ |
| M154 排序截断 + 降级 | PASS | 38 条截 30 条、跨源倒序穿插、老行被截；降级变异区块消失主列表照常 | |
| M155 切档持久化 + 非法值拒绝 | 部分 | 自动↔预演双向切档 DB 落库 ✓；非法值=代码验证（实机 invoke 通道无）；app 型按钮未构造 | |
| M156 到点预演 | PASS | 提案卡 todo 列 + 无执行 + preview 历史 + 审计 + ⏳ 横幅 + last_status 不变 | |
| M157 立即执行真跑 | FAIL | 被预演闸拦：再记 preview + 再建卡，无会话 | **D-4（=R-1）已修 0309e16f**；V152 修复型转 PASS |
| M158 存量作业照常执行 | PASS | 01:06:25 run→01:06:34 done 真跑；无预演痕迹；用完即删 | 兼作 M162 |
| M159 预演通知走队列 | 部分 | 正向：横幅可见+队列亚秒清空+无 enqueue_failed；失败半构造不出（macOS show() 静默成功） | V154 未验证有因 |
| M160 预演不改徽标 | PASS | last_status 空、retry 列不动 | V155 写 preview ✓ |
| M161 D-3 回归 | PASS | retry_max/pause_on_failure 落库；retry 1/2→2/2 节拍 5min；autopaused 审计 | +F-3 事件语义、一次性删除观察 |
| M162 D-1 回归 | PASS | at: 一次性到点执行+用完即删（共用 M158 证据） | 非 11-03 失效态 |
| W19 Windows 启动清扫 | 未覆盖 | 无 Windows 实机 | |
| W20 Windows 立即执行 | 未覆盖 | 同上（D-4 与平台无关，共用 run_scheduled_job） | |
| W21 Windows 后台待办 + 横幅 | 未覆盖 | 同上 | |

### 10.2 变异注入结论

| 编号 | 卡 | 注入后判据 FAIL？ | 观测到的具体现象 |
|---|---|---|---|
| V145 | M137 / M138 | ✓ | upsert 查询加 AND 1=0 → 第二轮 INSERT 撞唯一索引被吞，objective 停在第一轮、轮内不进 working |
| V146 | M140 / M143 | ✓ | finish 恒 idle → 失败轮停 idle（error 照写），后台待办不出现该条 |
| V147 | M145 | ✓ | sweep no-op → 重启后 working 残留不收；残行下次 pristine 启动被收（自愈旁证） |
| V148 | M146 | ✓（反向锚点） | 阈值 10s：变异态 t+20s 收（「轮超时无更新」）；pristine 60s/95s 不收 |
| V149 | M148 | ✓ | 判定改空 vec → result 空、work_item.machine_check 审计零新增 |
| V150 | M149 | ✓ | fail-open → 空文件+不存在文件 passed=2/total=2，错误结论当真 |
| V151 | M156 | ✓ | 预演分支条件永假 → preview 作业到点 9s 真跑（新会话+done），零预演痕迹 |
| V152 | M157 | ✓（修复型） | 手动路径 mode=None → preview 作业立即执行真跑成功（ok 历史+真摘要），判据转 PASS |
| V153 | M153 | ✓ | 子 agent 源置空 → budget_exceeded 造数行不入列（pristine 即现） |
| V154 | M159 | 未验证 | 失败不可构造：macOS 授权拒绝时 show() 静默成功（系统静默丢弃），队列恒空（50s 实证）；重试语义有单测锁定 |
| V155 | M160 | ✓ | 预演写 last_status → job 行 last_status='preview'，徽标口径漂移 |
| V156 | M141 | ✓（反向锚点） | 开票延迟 10s → 极短回合行停 working（t+15s 落地 60s+ 不收）；pristine 同场景 idle |
| V157 | M142 | ✓（修复型） | 清扫扩 awaiting_answer → 卡死行转 failed+error、awaiting_answer 归零，判据转 PASS |
| 降级 | M154 | ✓（自定义） | lib.rs 移除 needs_attention_list → 后台待办区块消失，主列表照常渲染 |

### 10.3 未覆盖（诚实清单）

> **跑了什么、没跑什么，必须如实列**。空着不填 = 这份报告不可信。

| 卡 / 项 | 为什么没跑 |
|---|---|
| W19–W21（Windows 专章） | 本机为 macOS，无 Windows 实机；D-4 与平台无关（共用 run_scheduled_job），W20 大概率同因 |
| M159 失败半 + V154 | macOS 授权拒绝时通知 API 静默成功，投递失败不可构造；WM_INJECT_NOTIFY_FAIL 已不存在于代码 |
| M155 判据 3 实机 invoke 半 | 无 devtools/直连 invoke 通道（右键检查被禁）；mode 校验以代码+单测背书 |
| M155 判据 4（app 型作业不显示按钮） | 无 app 型作业可构造 |
| M146 判据 3 变异态在场构造 | 变异运行期间无 awaiting_answer 行在场；重估路径本身不含 awaiting_answer（pristine M143 已证不参与） |
| M141 pristine 窄缝复现 | 窄缝 pristine 下几乎不可达（开票早于收尾一个模型往返）；多轮旁证 + V156 变异态覆盖 |

### 10.4 环境操作台账

> 所有临时改动（直写库的数据、注入的 bug、临时开关）**逐条登记 + 还原**。
> 收尾必须 `git status` clean + 干净启动验证通过。

| 改了什么 | 改在哪 | 还原了吗 |
|---|---|---|
| keyring env 覆盖补丁（WM_TEST_LLM_KEY/TAVILY_KEY/BRAVE_KEY/DEEPSEEK_KEY + vendor 回落，5 处） | src-tauri/src/bot/config/keyring.rs | ✓ git checkout 还原 |
| 变异编辑 V145/V146/V147/V157/V153（db/work_items.rs）、V148/V151/V152/V155（bot_scheduler.rs）、V156（bot_chat/flow.rs）、V149（db/work_items.rs）、V150（subagent_digest.rs）、降级（lib.rs） | src-tauri 各文件 | ✓ 全部 git checkout 还原（src-tauri 与 HEAD 一致） |
| 密钥 env 文件（0600 仓外） | ~/.config/wmessage-smoke-test.env | ✓ 已删 |
| work_items 全表清空（A/B 残留 + 测试行；表本身本轮才建） | wmessage.db | ✓（清空即目的） |
| M153/M154 造数行（wi_test_*×35、sa_test_be、sj_test_*×3） | wmessage.db | ✓ 已删 |
| reminders 全表清空（bot 误建提醒） | wmessage.db | ✓ 已删 |
| 测试任务卡 10 张（M155/M156×3/M158/M161×3/V155/复盘会） | wmessage.db tasks | ✓ 已删 |
| 测试作业 sj_M156/sj_M155b/sj_M158/sj_M161/sj_V155 | wmessage.db scheduled_jobs | ✓ 已清（sj_M158/sj_M156/sj_M161/sj_V155 被一次性语义自动删） |
| AI_Gen_Files：m139.txt、冒烟产物.txt、空文件.txt | AI_Gen_Files | ✓ 已删 |
| ~/Desktop/别处产物.txt（M149 造数） | Desktop | ✓ 已删 |
| 通知授权 off→on（M159 失败半尝试） | 系统设置 | ✓ 已还原（终态 ON） |
| vite dev server（:1420，debug 二进制依赖） | 后台进程 | ✓ 已停 |
| 测试会话 ~10 个（含 401/失控轮证据链） | bot_sessions | 留存不删（聊天列表可见，如实登记） |

---

## 十一、读码预判的风险面（提前预警，不代替实测结论）

> 下面是**写方案时读码发现**的疑点，**不是已证实的缺陷**——按卡实测后再定性。
> 它们的用处是让跑卡的人盯住这几处，别把已知疑点当新发现，也别把疑点当结论。
> 其中 **R-7 / R-8 已各自成卡（M141 / M142）**，卡里写明了预期方向：R-8 现状**预期 FAIL**
> （那是缺陷卡），M141 靠变异构造、pristine 侧预期 PASS。

| # | 疑点 | 依据 | 对应卡 |
|---|---|---|---|
| R-1 | **手动「立即执行」极可能被预演闸拦下**——`scheduled_job_fire`（`bot_scheduler.rs:1041-1068`）在 `:1066` 直接调 `run_scheduled_job`，与到点触发同函数；预演闸在 `:542` 拦下即 `return`。与提交说明「该入口不受预演档拦截」及 ACC-P-14 判据 2 **不一致** | `bot_scheduler.rs:1066` → `:542` | M157（V152 修复型变异配套） |
| R-2 | **旧 acceptance 跨轮沿用**——`upsert_for_session` 不清 acceptance 列，新一轮无 acceptance 时旧判据继续生效，机器判定可能对着过期判据下结论 | `db/work_items.rs:106-119` | M151 |
| R-3 | **工作项只有主聊天一种 kind**——方案 §7.2 设计的 `kind=task / scheduled` 与 `origin_ref` 关联**尚未实现**（`upsert_for_session` 只在 `flow.rs:668` 被调用，`kind="chat"`、`origin_ref=None`） | `bot_chat/flow.rs:668` | 不列卡（设计未落地，非缺陷） |
| R-4 | **两个工作项命令无消费方**——`work_items_list` / `work_items_needing_human` 已注册为 Tauri 命令（`lib.rs:778-780`），但前端只调 `needs_attention_list`；批 7 撤掉了前端导出，命令留在 IPC 面上 | `lib.rs:778-780`；`src/lib/workItems.ts`；**仓库自己的门禁已报警**：`tests-audit/audit_tauri_bridge.py` 输出「已注册但前端未调用的命令：work_items_list, work_items_needing_human」 | 不列卡（死代码，AGENTS 禁令「不留死代码」，建议后续清理或接线） |
| R-5 | **超长轮会被 tick 误收**——`sweep_stale_working` 阈值 2h，长轮期间 `updated_at` 不刷新，理论上可被收成 failed，收尾又改回 idle/failed | `bot_scheduler.rs:1105` | 归入 §10.3 观察项 |
| R-6 | **判据语法错就静默跳过**——解析不了的判据直接 `continue`，用户看不到任何提示（机器结论整条缺席） | `bot/subagent_digest.rs:325-327` | M150（卡里已写明「跳过不报错」） |
| R-7 | **极短回合可能留下 `working` 残行**——开票与轮收尾都是 fire-and-forget 的 spawn，收尾的 UPDATE 若先于开票的 INSERT 落地，匹配不到行 → 静默 no-op → 行建出来停在 `working`，直到 2h 后被 tick 收成 failed | `bot_chat/flow.rs:653-681` / `:761-775`；`db/work_items.rs:194-201` | M141（V156 反向锚点） |
| R-8 | **崩溃后待答行永久报警**——启动清扫只收 `working`，`awaiting_answer` 不在其中；而 ask waiter 注册表在内存里，进程一重启等待器全失 → 该行永远停在 `awaiting_answer` → 永久进「后台待办」，用户无「作废」入口 | `db/work_items.rs:220-228`；`app_state::question_waiters` | M142（**现状预期 FAIL**，V157 修复型锚点） |

---

## 十二、与前几份的关系

| 文档 | 覆盖窗口 | 卡片 | 状态 |
|---|---|---|---|
| `docs/realmachine-smoke-test-2026-10-25.md` | 10-24 10:51 → 10-25 15:36 | M75–M92 + W10–W12 | 已执行 |
| `docs/realmachine-smoke-test-2026-10-29.md` | 10-28 20:54 → 10-31 16:27 | M93–M115 + W13–W15 | 已执行 |
| `docs/realmachine-smoke-test-2026-11-03.md` | 11-02 05:31 → 11-04 06:5x | M116–M136 + W16–W18 | 已执行（报告 `docs/realmachine-smoke-report-2026-11-03.md`：PASS 12 / 部分 7 / FAIL 1（M132，D-3 已修）/ 未覆盖 1；新发现 D-1 / D-2 / D-3） |
| **`docs/realmachine-smoke-test-2026-11-04.md`（本份）** | **11-04 07:50 → 08:26（批 5–8）+ D-1/D-3 修复回归** | **M137–M162 + W19–W21** | **待执行** |
| `docs/AGENT-RUNTIME-REFIT-PLAN-2026-11-02.md` | 方案全文（批 1–8） | — | 批 1–8 **全部落地**；批 7 偏离：活动页状态筛选延后（单独小批再做） |
| `docs/ACCEPTANCE-PLAN-2026-10-30.md` | 按**功能条目**索引（全库 116 条） | `docs/acceptance/cards-*.md` | 与本份**互补** |

> **两条验收面的关系**：`ACCEPTANCE-PLAN-2026-10-30.md` 按 SPEC 功能条目建卡
> （回答「这条功能验没验过」），本份按改动窗口建卡（回答「这批改动对不对」）。
> 本批**新增两条 SPEC 功能条目**（6-118 工作单元、6-119 作业执行档与预演），
> 追踪矩阵 `docs/acceptance/matrix.csv` 已同步，两行的「未验 / 无实机卡」状态
> **待本份执行完毕后回填**。
