# 子 agent 全链路深度审计报告（2026-11-12）

- 基线：`5add9495`（工作区干净，HEAD）
- 范围：子 agent 全链路——spawn / 并发闸 / runner / 熔断恢复链 / 交卷与续跑 / check / cancel / digest / 汇报轮 / 白名单与 dispatch 闸 / 状态机与 Effects / 前端事件契约
- 方法：SOP（`docs/agent-audit-sop-zcode.md`）四段式的体检 → 深度审计 → 复核；每条发现带 file:line Receipt，关键结论全部经主审计者亲读源码回验（探查代理产出仅作线索）
- 门禁状态：9 个 `tests-audit/audit_*.py` 对拍全绿（含 SOP 基线对拍、常量单一源、验收矩阵、事件名白名单）；cargo/vitest 全量本会话未跑（只读审计）
- 结论速览：**0 阻断 / 1 中 / 1 中低 / 10 低 / 8 极低**（其中高置信死代码 4 件、高置信重复 5 件）。整体判断：这条链路的状态机、竞态闸、幂等与审计覆盖是全仓最厚的部分之一，近期五笔改造（正向停机 / 运行时探测 / 熔断恢复链 / submit_result / resume_subagent）质量高；发现集中在「三处新接缝的口径没对齐」与「改版遗留物」。

---

## 一、发现清单

### 发现 #1（中）主会话可直呼 submit_result，且会以其参数终止主回合

- 位置：
  - `src-tauri/src/bot/registry/schemas.rs:1080`——`pub const SUBAGENT_ONLY_TOOLS: &[&str] = &["write_artifact_file", "read_own_card", "submit_result"];`（对主会话只做 **schema 隐藏**）
  - `src-tauri/src/bot/registry/handlers.rs:465`——`pub(super) fn call_submit_result<'a>(_ctx: &'a ToolCtx<'a>, args: &'a str)`（**工具体不读 ctx，无会话校验**）
  - `src-tauri/src/bot_model_loop/core.rs:1031-1033`——`if crate::bot::registry::is_submission_tool(name.as_str()) && !failed { return Ok(DispatchOutcome::Submitted(result.clone())); }`（不分会话类型）
  - `src-tauri/src/bot/dispatch.rs:614`——1.6 白名单闸 `if let Some(ctx) = crate::tool_guard::subagent_ctx(session_id)` 只对**子 agent 会话**生效
- 问题：三个 SUBAGENT_ONLY 工具里，`write_artifact_file`（subagent_tools.rs:336-338）与 `read_own_card`（subagent_tools.rs:443-445）在工具体内都有 `subagent_ctx` 校验，唯独 `submit_result` 没有——「仅子 agent」的强制层在它身上只有 schema 不可见这一道。
- 触发条件：主会话模型幻觉出 `submit_result` 工具名（表内存在、dispatch 查 `tools_index()` 全表命中，规则表无特殊规则）→ 各闸放行 → `DispatchOutcome::Submitted` 让**主回合**以这段收尾 JSON 为终稿结束。
- 修复建议：`call_submit_result` 里补 `is_subagent_session` 检查，与同组另两个工具对齐（一处改动，锚点测试一条）。
- 反证：若主张「schema 隐藏即足够」，无法解释同组另两个工具为何都加了工具体闸——三者不对称指向遗漏而非设计。

### 发现 #2（中低）提示词教 resume 只认 `error=max_turns`，实际 reopen 只认 status

- 位置：
  - `src-tauri/src/prompts/subagent.rs:69-72`——「check_subagent 返回 status=budget_exceeded **且 error=max_turns 时**，优先调 resume_subagent」
  - `src-tauri/src/db/subagents.rs:575-584`——`reopen_subagent_for_resume` 的 match **只看 `row.status == BudgetExceeded`**，不读 error
- 问题：工具熔断触顶的行（error=`max_tool_calls`）同样可续跑且有 sidecar，但主 agent 按提示词会把它排除在续跑之外、走重派——sidecar 里已跑的全程历史被白扔，恰是 resume 要省的那笔成本。
- 触发条件：子 agent 工具调用达 400 熔断 → status=budget_exceeded / error=max_tool_calls → 主 agent 查提示词不满足「error=max_turns」→ 重派。
- 修复建议：提示词放宽为「status=budget_exceeded 时优先 resume（无续跑历史时工具会响亮报错，再走重派）」。
- 反证：无——reopen 的实际语义比提示词宽，提示词收紧没有任何机制收益。

### 发现 #3（低）resume 续跑轮绕过配置档 max_turn 钳制

- 位置：
  - `src-tauri/src/bot/subagent_runner.rs:141-147`——`max_rounds` 只由 `onion` 参数参与 min
  - `src-tauri/src/bot/subagent_tools.rs:293-298`——resume 的派发 payload `stage/onion` 恒 null
- 问题：续跑行 `max_rounds=200 → min(RESUME_ROUNDS)=10`，原配置档 `max_turn<10` 的「只收紧」钳制在续跑时失效（工具白名单交集不受影响——bind_session 从会话元数据重挂，但轮数上限这个权限维度丢了）。
- 触发条件：配置 `max_turn=5` 的子 agent 预算熔断 → resume → 提示词说「追加 10 轮」、实际拿 10 轮（配置只批 5）。
- 修复建议：续跑路径从 `bound_onion` 读 max_turn 参与取小。
- 反证：RESUME_ROUNDS=10 本身是小额度上限，突破幅度有限；且 resume 是显式决策。故定低不定中。

### 发现 #4（低）机器验收的熔断豁免只挡轮数标记，工具熔断与停滞行仍会被算验收

- 位置：`src-tauri/src/bot/subagent_runner.rs:832-845`
  ```rust
  let fused_on_rounds = matches!(&outcome, Ok((text, _, _))
      if text.contains(crate::bot_model_loop::ROUNDS_FUSE_MARKER));
  let machine_verdicts = if outcome.is_ok() && !fused_on_rounds { ... }
  ```
- 问题：注释（838-840 行）说「预算触顶/异常退出的行没有验收可言」，但守卫只认轮数标记：工具熔断 NotRecovered 的行（正文带 `FUSE_MARKER` 不带轮数标记）与 stalled 行 outcome 均为 Ok → `eval_machine_criteria` 照跑，`apply_machine_verdicts` 还会往 budget_exceeded/stalled 行正文追加「服务端机器验收未通过 N 项」。
- 影响：无翻盘风险（`reconcile_machine_override` 只推翻 `result_json_unparseable`，subagent_digest.rs:472-490 已亲验），属语义噪音 + 白算。
- 修复建议：豁免条件扩为「任一熔断标记或 stalled 即跳过」。

### 发现 #5（低）resume 注释宣称「新一轮时长口径从续跑起算」，但 started_at 不清

- 位置：`src-tauri/src/db/subagents.rs:572-573`（注释「finished_at 清空（新一轮时长口径从续跑起算）」）vs :587-589（`UPDATE subagents SET status='queued', finished_at=NULL`——started_at 不动）
- 影响：前端时长 = finishedAt − startedAt，续跑完成的行时长**含熔断前的旧运行段**，注释与行为不符。
- 修复建议：要么续跑时同时清 started_at，要么改注释为「时长跨段累计」。

### 发现 #6（低）finalize 闭包失败 + cancel 抢先的窗口里，事件口径回落 classify

- 位置：`src-tauri/src/bot/subagent_runner.rs:990-1016`（闭包失败 → `finalized=None`）→ :1023 `effective_status(None, status)` → 已被 cancel 置终态的行可能发 `subagent_failed` 事件
- 影响：罕见竞态下的前端文案不准，无数据损坏（行状态本身正确）。

### 发现 #7（极低）恢复链审计注释与事件内容不符

- 位置：`src-tauri/src/bot/subagent_runner.rs:664-666` 注释「trace 欠记的轮数在恢复审计事件里有轮数记录」——实际 `subagent_fuse_recovered` 事件（:702-705）只有 `via`，无轮数。恢复轮与补答轮的 token 也不进 usage 记账（:898-905 只取原始 trace）。
- 修复建议：注释改为如实描述（或在事件里补轮数字段）。

### 发现 #8（低）「追加 10 轮」4 处出现、2 处硬编码

- 位置：常量 `RESUME_ROUNDS=10`（subagent_repair.rs:253）用于 runner:616 与 repair:378；**硬编码**在 `subagent_tools.rs:321`（resume 工具回执「追加 10 轮预算」）与 `schemas.rs:350`（SCHEMA_RESUME_SUBAGENT description）。
- 影响：改常量这两处必漂移（schema description 是静态串引不了常量）。
- 修复建议：两处文案去数字化（「追加一小笔轮数预算」），或加对拍测试锁 `RESUME_ROUNDS==10` 与文案同步。

### 发现 #9（低）submit_result 字段契约三份同文，宣称的去重没做干净

- 位置：`subagent_runner.rs:1128`（任务包装）≈ `prompts/subagent.rs:117-118`（SUBAGENT_BASE 第 4 条）≈ `schemas.rs:340`（SCHEMA description）——status/summary 必填、acceptance_check 逐条、artifacts、blockers 的字段级契约重复三份；而 runner:1127 注释宣称「RESULT_SCHEMA_HINT 的字段说明随工具 schema 走，任务包装不再嵌一份」。
- 影响：目前三份口径一致（亲验），纯漂移风险。
- 修复建议：包装只留一句「按 submit_result 工具 schema 提交」，字段说明交给 schema 与 SUBAGENT_BASE。

### 发现 #10（低）【死代码】SubagentGate.notify 是零等待者的死字段

- 位置：`src-tauri/src/bot/subagent_spawn.rs:318`（字段）；:369（wait 循环）、:408（GateTicket::drop）两处 `notify_one()`
- 证据：`wait_slot_cancellable` 的等待靠 :370 的 50ms sleep 轮询推进（:312-315 注释自述从 `notified().await` 改为轮询的沿革）；全仓唯一 `.notified()` 消费属 `finish_notify()`（bot_orchestrator.rs:338-341，另一个 Notify）。gate 的 Notify 无任何等待者，两处 `notify_one` 是 no-op（最多存一个没人取的 permit）。
- 修复建议：删字段 + 两处 notify_one（保留轮询语义不动）。

### 发现 #11（极低）【死代码】MAX_GLOBAL_RUNNING / MAX_PER_SESSION_RUNNING 的 orchestrator re-export 零消费

- 位置：`bot_orchestrator.rs:25-26`；全仓活代码 0 引用（仅 `.mimosa` 旧快照命中）。常量本体在 spawn.rs:344/349 活着。

### 发现 #12（低）【死代码】CancelAck.ok 恒 true 且零读者

- 位置：`bot_orchestrator.rs:221`（定义）、:298/:307（两处写死 `true`）；Rust 侧只读 `already_terminal`（:513、subagent_tools.rs:225）；前端丢弃 cancel 返回值。属序列化 API 面，删除时同步前端契约即可。

### 发现 #13（极低）【冗余】resume 前置校验双重判

- `subagent_tools.rs:37-40`（`load_resume_history` 返回 Some 已保证 `len>=2`）vs :273（`Some(n) if n >= 2` guard 恒真）。30 行内同一不变式写两遍。

### 发现 #14（极低）【重复】「(n) 序号」产物去重循环全仓 4 份

- `bot/subagent_tools.rs:411-417`（上限 1000）/ `py/harvest.rs:92-104`（无上限）/ `py/document.rs:492-498`（无上限）/ `migration/ops.rs:230-239`（上限 100）。同 scheme 三套上限口径；已达「第二个调用方」门槛的抽象候选，但跨四子系统，收敛收益有限——登记不动。

### 发现 #15（极低）【冗余】「live」状态集合 SQL 字面量两处

- `db/subagents.rs:344`、`:513` 各写一遍 `status IN ('queued','running')`，与 `transition_allowed`/`is_terminal` 无编译期联动；新增状态时三处需同步。

### 发现 #16（低）check_subagent 缺第二层递归禁用，四件套双保险不对称

- 位置：`src-tauri/src/bot/subagent_tools.rs:180-203`（`tool_check_subagent` 无 `is_subagent_session` 检查）vs 同文件 :90-95（spawn）、:211-213（cancel）、:251-253（resume）三处均有。
- 问题：递归禁用的「双保险」在 check 上只剩 dispatch 1.6 白名单闸单层（check_subagent 不在任何 profile 白名单，亲验）。check 是幂等只读工具，越权后果仅限读状态，故低。
- 修复建议：补一行 `is_subagent_session` 检查，与同文件三兄弟对齐（或注释说明 check 只读故豁免第二层——二选一，现状是「无说明的不对称」）。

### 发现 #17（低）RESULT_SCHEMA_HINT 缺 `checks` 键，与两处教法不一致

- 位置：`src-tauri/src/prompts/subagent.rs:220-226`（hint 键集：status/summary/acceptance_check/artifacts/blockers，**无 checks**）vs SUBAGENT_BASE 第 4 条（:118「verifier 的逐条结论放 checks」）与 SCHEMA_SUBMIT_RESULT description（schemas.rs:340「verifier 的逐条验收数组放 checks」）。
- 触发条件：verifier 段子 agent 走到强制交卷轮或补答轮（熔断恢复链 / 补答链）——这两轮的提示词就是 RESULT_SCHEMA_HINT——收不到 checks 字段示例，可能改报 acceptance_check 形态。
- 影响：分类器与进度统计对两键都认（classify 结构识别 / `check_subagent_locked` 双键读取，亲验），故仅口径漂移不致错判；但 verifier 的「checks 结构 → 独立判定分支」（classify_verifier_outcome）在补答轮产出上会失活。
- 修复建议：hint 的 acceptance_check 行旁加一行注释式 checks 示例（或注明「verifier 用 checks」）。

### 发现 #18（极低）dispatch.rs 过期注释把子 agent 白名单闸说成 0.55

- 位置：`src-tauri/src/bot/dispatch.rs:581`——「子 agent 在 0.55 白名单闸已被拒，到不了这」。实际 0.55 是技能 allowed-tools 闸；子 agent 白名单闸是 1.6（:610-614），且执行顺序在 0.57 deferred_gate **之后**——子 agent 调休眠工具恰恰**到得了**这里，由 deferred_gate 或 1.6 拒。
- 影响：注释与闸序双双过期（闸序本体与 SOP §5.1 B 登记一致，无行为问题）。`groups.rs:204` 有同款注释。

### 发现 #19（极低）SUBAGENT_ONLY_TOOLS 注释只列两个工具，常量有三个

- 位置：`src-tauri/src/bot/registry/schemas.rs:1072-1073`（注释「write_artifact_file / read_own_card」）vs :1080（常量含 `submit_result`）。公式 `BUILTIN - len()` = 69-3=66 本身没错，错在枚举漏了第三个。与发现 #1 同根：submit_result 加入时注释没跟上。

### 发现 #20（极低）misc 组列着两个永远被过滤的子 agent 专属工具

- 位置：`src-tauri/src/bot/registry/groups.rs:111-112`（TOOL_GROUPS "misc" 含 write_artifact_file / read_own_card）+ :264/:283（两处拼装闭包把它们二次剔除，亲验）。
- 问题：组表里放着挂载结果恒为空的两项，机制靠双闸兜住但结构上易让后来者以为 misc 挂载会把它们带给主 agent（发现 #1 的诱因面之一）。组覆盖测试只豁免 submit_result 与 resume_subagent（groups.rs:335-336）。
- 修复建议：从 misc 组删这两项，或给组覆盖测试补「组内不得含 SUBAGENT_ONLY_TOOLS」断言。

### 流畅性备忘（不成发现，供下轮维护者）

- `subagent_runner.rs:692-771`——熔断分支 Recovered 提前 return，NotRecovered 携带标记落穿到 :768 的 `repair_final_json_once` 是必然 no-op（函数首行标记检查即退）。控制流正确，读者要绕一下才能确认。
- `bot_orchestrator.rs:435-441`——check 心跳事件 payload 字段名 `subagentId` 装的是 check key（可能是 taskId 口径）；前端只刷活跃戳不读该字段，无实际错配。
- 事件通道形态不一：`subagent-spawned/finished/activity` 走 `emit_to("main")` 定向，`bot-check-heartbeat` 走 `emit_recorded` 广播——符合真源化批 6「投影事件广播、非投影仍定向」的登记口径；**多窗口扇出立项时这三个事件要重新过一遍**。
- `bot/effects.rs` dispatch 消费里「spawn run_subagent 后立即 finish_ok 删待办」与「runner 写 Running」之间有一个窄窗口：进程恰在此间崩溃 → 重启后 queued 行无 pending 待办 → 启动清扫按 cancelled 收（恢复语义在该窗口退化为取消）。at-least-once 语义的已知边界，登记备查。

---

## 二、复核记录（SOP §7）

全部发现按「回读定位 / 反向找反例 / 找调用方」三件套过了一遍，关键动作：

| 发现 | 回读 | 反例/调用方 | 结论 |
|---|---|---|---|
| #1 | 亲读 schemas.rs:1080 / handlers.rs:465 / core.rs:1031 / dispatch.rs:614 | grep 规则表：submit_result/resume_subagent 无特殊规则；同组另两工具的工具体闸亲读确认 | 成立（中） |
| #2 | 亲读 prompts/subagent.rs:69-72 / subagents.rs:575-584 | reopen 无 error 判断 | 成立（中低） |
| #3 | 亲读 runner:141-147/619 与 tools:293-298 | resume payload 恒 null 亲验 | 成立（低） |
| #4 | 亲读 runner:832-845 + digest.rs:472-490 | reconcile 只推翻 unparseable，无翻盘路径 | 成立（低） |
| #5 | 亲读 subagents.rs:565-592 | 前端时长算式由探查代理带行号收证 | 成立（低） |
| #6 | 亲读 runner:990-1093 | emit 出口唯一（:1093）确认 | 成立（低） |
| #8 | 亲读四处原文 | RESUME_ROUNDS 消费点全列 | 成立（低） |
| #9 | 亲读三份原文 | 三份口径当前一致 | 成立（低，纯漂移风险） |
| #10 | 亲读 spawn.rs 全文 | grep `.notified()` 全仓唯一命中属 finish_notify | 成立（高置信死代码） |
| #11 | grep 全仓（排除 .mimosa） | 0 活引用 | 成立 |
| #12 | grep `ack.ok` 全仓 0 命中 | 前端丢弃返回值 | 成立 |
| #13-#15 | 亲读 | — | 成立（极低） |
| #16 | 亲读 subagent_tools.rs:180-203 与三处兄弟检查 | grep `is_subagent_session` 生产 4 处（含 dispatch 规则闸） | 成立（低） |
| #17 | 亲读 prompts/subagent.rs:220-226 | SUBAGENT_BASE:118 与 schemas.rs:340 双处教 checks；digest/classify 双键兼容亲验 | 成立（低） |
| #18-#20 | 亲读 dispatch.rs:581 / schemas.rs:1072-1080 / groups.rs:105-120 | — | 成立（极低） |

探查代理报告中被我**撤销/修正**的候选：
- 「收尾窗轮数用尽文案用 FUSE_MARKER 会被误归 max_tool_calls」——回查 `wrap_up_rounds_exhausted` 只由**工具轴**进窗触发（entered_round 仅工具轴写），`max_tool_calls` 归因自洽，**撤销**。
- 「停滞行会被机器验收翻盘成 succeeded」——reconcile 只推翻 `result_json_unparseable`，stalled 的 error 是 "stalled"，无翻盘；修正为 #4 的语义噪音。
- 「事件契约有幽灵事件」——四个后端事件（subagent-spawned / subagent-finished / subagent-activity / bot-check-heartbeat）逐一有前端消费者，无幽灵，撤销。

## 三、「设计如此」核对清单（未报缺陷）

- research 白名单含 `http_request`：目的地闸体系（2026-10-31 整合方案）的既定面。
- stage 角色提示词的工具列表比运行时白名单窄（如 verifier 提示词禁联网、research 白名单含联网）：正常路径由 stage 默认 onion（probe/validator/writereditor）交集收窄对齐；挂载失败「降级不挂 + 审计留痕」是文档化行为（subagent_tools.rs:46-48）。
- runner 的 `list_subagents` 等读路径持 `lock_db_write` 而 check/watcher 只读不持：watcher 注释明说不持锁防多子 agent 写锁互饿（runner:538）——仓库内既有口径分歧，单独不成发现。
- 恢复链 refs/trace 不并回、补答/恢复轮 token 不进 usage：注释已登记取舍（runner:664-666）。
- 双计数器口径（dispatch `used_tool_calls` 不回退 vs 引擎 `function_calls_total` 对 Denied 回退）：dispatch.rs:706-710 注释已登记（仪表守「实际执行」）。
- 早退出口漏报所在轮（`on_round` 轮末才报）：core.rs:620-624 注释自洽（「没跑完的一轮」不上报更贴近真实完成量）。
- 子 agent 无墙钟、唯一时间维度是 600s 停滞看门狗：2026-10-28 预算简化既定口径。

## 四、按风险排序的修复清单（待拍板，本轮不动代码）

| 序 | 发现 | 等级 | 一句话修法 | 预估 |
|---|---|---|---|---|
| 1 | #1 主会话可直呼 submit_result | 中 | call_submit_result 补 is_subagent_session 检查 + 锚点测试 | 0.5h |
| 2 | #2 resume 提示词口径过窄 | 中低 | MAIN_AGENT_ADDENDUM 改为 status 判据 | 0.2h |
| 3 | #4 机器验收豁免面过窄 | 低 | 豁免条件扩为任一熔断标记或 stalled | 0.3h |
| 4 | #8「追加 10 轮」硬编码 | 低 | 文案去数字化 + 对拍测试 | 0.3h |
| 5 | #9 字段契约三份 | 低 | 包装只留指路一句 | 0.2h |
| 6 | #10/#11/#12 死代码三件 | 低/极低 | 删 Notify 字段、删两 re-export、删 ok 字段（同步前端契约） | 0.5h |
| 7 | #3 resume 绕过 max_turn | 低 | 续跑从 bound_onion 读 max_turn 参与取小 | 0.3h |
| 8 | #16 check_subagent 补第二层 | 低 | 工具体补一行（或注释豁免理由） | 0.1h |
| 9 | #17 RESULT_SCHEMA_HINT 补 checks | 低 | hint 加一行示例 | 0.1h |
| 10 | #5/#6/#7/#18/#19/#20 注释与口径 | 低/极低 | 注释改如实 / 组表清项 / 补断言 | 0.4h |

---

## 五、体检附录

- 模块地图全部对上（spawn 439 / runner 1356 / digest 580 / repair 954 / tools 566 / orchestrator 757 / dispatch 997 / schemas 1295 / fuse 163 / tool_guard 344 / prompts/subagent 522），无地图漂移。
- 状态机（`transition_allowed`：queued→running|cancelled；running→4 终态；终态禁移 + 同态短路幂等）与 Effects（PK(kind,key) upsert、generation CAS、5 次退避后 give_up 审计兜底）闭环完整；启动顺序「清扫先于派发器」由 lib.rs:589-593 注释与调用序锁住。
- 递归禁用：dispatch 1.6（ctx.tools 交集闸，spawn/check/cancel/resume 不在任何白名单）+ 工具体内 `is_subagent_session`——spawn/cancel/resume 双层齐全，check_subagent 仅 dispatch 单层（见发现 #16）；另规则闸 Ask 分支对子 agent 直接拒（dispatch.rs:382-404，防后台会话等确认）。
- 早退点 8 处与 SOP 基线一致（逐处对号）：:336 L4 Interrupt / :374 规则 deny / :402 规则 ask×子 agent / :446 规则 ask×用户拒 / :570 技能 allowed-tools / :607 skill 熔断 / :642 子 agent 白名单 / :704 入参 JSON，全部 Denied 且 tool.return 配平（:634-641 白名单闸段亲验）。
- 事件契约：后端 4 个子 agent 事件全部有前端消费者，无幽灵事件；`subagent-finished` 是唯一收尾事件出口并挂 `finish_notify` 唤醒，与 check 的等待核同源。
- 权限收窄链亲验：三档白名单（research 10 / coder 12 / general 16 工具）无越档；research 无写/执行类、coder 无联网类，与 SOP §C 判据一致；submit_result 三档全含（交卷出口是运行时契约）且 onion 交集豁免有测试锁定。
- 子 agent 链路 8 文件 + 测试目录内 TODO/FIXME/待拍板标记：0 处。

---

## 六、修复落地记录（2026-11-12，修复轮 8 提交）

| 提交 | 发现 | 内容 | 测试锚点 |
|---|---|---|---|
| `6e5b1a2d` | #1 #16 | submit_result 工具体补会话闸（收进不绑 Runtime 的 `submit_result_checked`）+ check_subagent 补第二层递归闸 | 主会话拒/子 agent 放行/参数校验保留 3 测 + 四工具对称性守卫 |
| `6e70cb86` | **新发现（修复中发现）** + #3 + #5 | **resume 派发待办携原会话 onion 元列**——原先 payload 恒 null，而 runner 给行建新会话，配置档白名单交集/max_turn/提示词段/采样温度在续跑时全部静默丢失（bind_session 读新会话，注释承诺的重挂不成立）；reopen 清 started_at | `resume_locked_carries_previous_session_onion_into_dispatch`（携 validator/5 + 无配置档 null 两形态）+ reopen 测试补 started_at 断言 |
| `250d3a2b` | #4 | 机器验收豁免收进单一谓词：任一熔断标记（短标记覆盖轮数）或停滞即跳过 | `machine_verdicts_skip_fused_and_stalled_runs` |
| `8a7fd528` | #2 #8 | 提示词续跑引导改 status 判据；RESUME_ROUNDS==10 锚点闸；SPEC:152 账实同步 | `main_agent_addendum_promotes_resume_by_status_not_error_string` + `resume_rounds_constant_matches_the_numbers_in_static_texts` |
| `423b8471` | #6 #7 | 收尾闭包失败时事件回读行上真实终态（cancel 抢先窗口不再误报 failed）；恢复链注释改如实 | 无独立测试（runner 级竞态，实机验收兜底） |
| `67b89bb1` | #9 #17 | 任务包装字段契约收敛为指路一句（三份同文 → schema + SUBAGENT_BASE 双源）；RESULT_SCHEMA_HINT 补 checks 同构示例 | wrapper 断言反转 + hint 有效 JSON 测试补 checks 断言 |
| `4da4e812` | #10 #11 #12 #13 | 死代码三件：gate Notify 字段/门面双 re-export/CancelAck.ok；resume 前置校验收敛 let-else | CancelAck 测试断言改 already_terminal；全量编译 + clippy 绿 |
| `7c468604` | #18 #19 #20 | misc 组清子 agent 专属项 + NOT_GROUPED 扩四 + 「SUBAGENT_ONLY 不得进组」断言；三处过期注释改如实 | `groups_cover_every_builtin_tool` 强化 |

- **登记不动**：#14（「(n) 序号」去重循环 4 份，跨四子系统收敛收益有限）、#15（live 状态 SQL 字面量两处）。
- **实机待验**：续跑链（onion 重挂后 validator/probe/writereditor 子 agent 的续跑工具面与首跑一致；started_at 清空后时长口径）、submit_result 主会话拒绝（有单测，实机路径再扫一眼）、#6 的 cancel-vs-finalize 竞态事件文案。
- 门禁：每笔提交过 pre-commit（fmt/check/clippy/machete）；全量 cargo test --lib 与 tests-audit 对拍见提交后记录。报告计数勘误：发现 #8 原写「4 处出现、2 处硬编码」，实为 5 处出现、3 处硬编码（addendum 亦有一处，随 `8a7fd528` 一并收口）。
