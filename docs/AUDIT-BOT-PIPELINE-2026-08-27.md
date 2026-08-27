# Rust Bot 工具 / Skill 链路审计报告（2026-08-27）

> 审计范围：`bot.rs` / `bot_chat.rs` / `bot_model_loop.rs` / `bot_plan.rs` / `bot_fs.rs` / `bot_slash.rs` / `middleware.rs` / `tool_guard.rs` / `exec_steps.rs` / `bot_skills/*`（约 1.2 万行），对照 `docs/SKILL_DSL.md` / `docs/SKILL-RUNTIME.md`。
> 审计方式：4 路并行子代理分域审计（工具注册表面 / DSL 调度器 / 系统提示词 / 中间件管线），P0 级发现均已人工在源码复核属实。

## P0 —— 确定性 bug / 结构性冲突（本次修复对象）

### P0-1 Skill DSL 跑完不收尾，状态机泄漏为「僵尸 Running」
`bot_skills/scheduler.rs:296-309` 的 Done 分支只写审计和 DB，整条成功路径无 `skill_finish` 调用；`bot_chat.rs:396-401` 拿到 Done 直接 return。SkillRun 停留在 `Running` 直到超时被清理。后果：
- `is_skill_active_for` 只认 Running → 技能结束后窗口期内（默认 180s，最长 600s）本会话原子工具持续放行，AtomicGuard 形同虚设；
- 窗口期内同会话每条普通消息的工具调用都被 `skill_on_step` 计入该僵尸 run，累计超 `max_steps` 后后续所有工具调用被步数熔断拒绝——一次成功的技能执行把会话打成半残；
- 持久层已写 `kind=done`，内存态 Running，两处真相不一致。

### P0-2 Prompt 指使模型裸调原子黑名单工具，任务卡路径必然被拦
- `create_word_revisions`、`link_file_to_task` 在 `ATOMIC_TOOLS`（`tool_guard.rs:14-19`），无活动 Skill 时硬拦；
- 但 SYSTEM_PROMPT 规则 10（`bot_chat.rs:54`）与 EXECUTE_SYSTEM_PROMPT 规则 2/3（`bot_chat.rs:511-512`）让模型直接调它们，而 `execute_task_core` 从不 `start_skill`——任务卡执行路径下调 `link_file_to_task` 绑产物、调 `create_word_revisions` 做修订，每次都被自家网关否决；
- 拦截文案（`tool_guard.rs:29-30`）是写给用户的话术，接收者却是模型，模型收到后无可执行下一步，且「通过执行任务卡（🤖 按钮）」的指引在任务卡执行中构成死循环；
- 两个工具的 schema description（`bot_model_loop.rs:135,157`）完全没提示「Skill 内专用」，schema 层契约与守卫直接打架。

### P0-3 `complete_task` handler 不读 `taskId`
`bot.rs:1264-1269` 只读 `title` 做模糊匹配，没走 `resolve_task`；schema（`bot_model_loop.rs:82-85`）声明「taskId 可选，优先于 title」，EXECUTE 规则 4 明确要求「taskId 用任务卡 id」。模型照 prompt 只传 taskId → 确定性报「complete_task 缺少 title」。

### P0-4 幻觉守卫被「调用过但未成功」的变更架空
`bot_model_loop.rs:652-656` 在工具执行前就按名字置 `mutation_done=true`。原子工具被门禁拦截、`delete_task` 被用户拒绝、写工具执行失败——都算「动过手」，之后模型幻觉汇报「已删除/已生成」守卫不再触发。应按执行结果置位。

### P0-5 回滚段调原子工具必被自家门禁拦截，且结果静默吞掉
`run_rollback_segment`（`scheduler.rs:108-115`）执行时 run 已被标 `Failed`，而 `is_skill_active_for` 只认 Running → 回滚段里的 `create_word_revisions`/`link_file_to_task` 被 AtomicGuard 硬拒；返回值又被 `let _ =` 丢弃，`rollback_attempted` 恒等于「段非空」。与 `SKILL_DSL.md` §4.3.2「前端据此提示人工核对」契约直接冲突——回滚全挂时前端显示「已回滚」，安全护栏被架空。

## P1 —— 逻辑分叉 / 一致性缺陷（✅ 2026-08-27 已全部修复，见 DEVLOG 同日复盘）

### P1-6 失败判定三套口径各说各话
- `is_tool_failure_text`（`scheduler.rs:82-88`）：纯 `starts_with` 前缀；
- `classify_text`（`audit.rs:69-82`）：`contains` 全串扫描；
- 熔断/暂停文案「技能…已强制终止」「技能已暂停…」「用户拒绝了删除」均不以「失败/错误」开头 → 判为成功。
后果：熔断恰好发生在 DSL 最后一个 step 时（parse 层不校验步数 vs max_steps），循环结束返回 `Done("✅ 自动执行完成")`，熔断错误文本进汇总与 `kind=done` 持久记录；用户拒绝删除若发生在末步同理。反向地，`classify_text` 的 contains 语义驱动 PREVR——成功结果里提到「失败」二字就累计 `consec_failures`，误触发 Replan。建议统一：execute_tool 早退路径返回统一「失败：」前缀，或调度器直接读状态机而非猜文本。

### P1-7 Replan 两处失效
- `bot_model_loop.rs:738` 把 `last_failed_tool`（只是工具名）当 `fail_reason` 传给 Planner，真正错误文本没进 REPLANNER_PROMPT；
- 失败的 replan 不计 `replans_used`（`:737-748` 只在成功时 +1），Planner 持续失败时每轮白烧一次调用，「≤2 次硬上限」名不副实（最终靠 50 轮兜底）。

### P1-8 /stop 与会话隔离体系不一致
`bot_stop`（`bot_slash.rs:98-113`）的 `skill_terminate_all` 不按会话过滤、`exec_steps::clear` 清任意会话挂起——2026-08-26 会话隔离后唯独 /stop 一停全停。另外：工具批循环体内无 `stop.stopped()` 检查（一轮 20 个 tool_calls 中途停不下，run_python 除外）；DSL 内 `execute_tool` 传 `stop=None`，在途 Python 脚本必须跑完；/stop 本身零审计。

### P1-9 `AwaitUser` 断头路
`bot_chat.rs:404` 把字面量 `"__await_user__"` 当回复文本返回，前端无任何处理；确认后也没有机制重启调度器续跑。

### P1-10 幻觉守卫误拦 / 漏拦
- `MUTATING_TOOLS`（`bot_model_loop.rs:28-43`）漏 `link_file_to_task`（与 bind_file 同写路径）→ 技能内绑产物成功汇报「已绑定」反被误拦；
- 动词表含「完成/生成」→「已完成搜索」「已生成分析结果」（只读 / run_python 任务）误拦；缺「保存/写入/记住」→「已保存到 AI_Gen_Files」漏拦；
- 守卫补轮话术（`:630`）让模型调 `toggle_subtask`/`bind_file`——与逐步执行模式禁令冲突。

### P1-11 同一 system prompt 内规则互斥
EXECUTE 规则 4「完成后用 complete_task 标记完成」与 STEPWISE_ADDENDUM「不要调用 complete_task」拼在同一 system 消息（`exec_steps.rs:163-168`），靠模型自行仲裁。

### P1-12 exec_steps 两处
- `PENDING` 全局单槽（`exec_steps.rs:34`）：会话 A 挂起中，会话 B 触发逐步执行直接覆盖 A，无审计；
- `resume` 错误路径（`exec_steps.rs:267-305`）：pending 已 take、LLM 失败后不复位 `bot_assigned`、无 clear 审计——与 `start()` 的错误清理不对称。

## P2 —— 健壮性 / 文案漂移（未修，待排期）

- **parse.rs**：step 编号不校验（重号时变量替换静默取首个）；同步步骤写两行工具调用第二行静默覆盖；回滚段标题三套说法互不识别（parse 只认 `## Rollback`，runtime 只认 `## 回滚`，文档写 `## 回滚（Rollback）`）；BOM 导致 frontmatter 整体静默丢失；max_steps 文档与实现三方数字打架（5/8/20）。
- **vars.rs**：`${step1.result}` 无 JSON 转义直插 args_json → 含换行/引号即产出非法 JSON，被 `parse_args` 静默降级为 Null 参数——`SKILL_DSL.md` §8.3 的 create_excel 示例按此实现不可能工作。
- **manage.rs**：`skills_import` 不校验最终技能名 → 非法名「装得上、用不了、删不掉」；`enabled: false` 的技能仍被 `build_skill_block` 广告给 LLM（路由表已正确排除，两个入口语义不一致）。
- **文案漂移**：create_ppt 描述「三套配色主题」（实际 10 套 + customColors）；web_search 描述「Bing+百度」（已有 Tavily 路由）；run_python 描述「需开启 Python」（yolo 模式绕过）；`bot_chat.rs:437` 与 `parse.rs:18` 注释仍写「默认 20 轮」（实际 50）。
- **审计空洞**：/stop、50 轮熔断、确认弹窗超时/拒绝、LLM 网络失败、Replan 预算耗尽等路径无审计事件。
- **middleware**：registry 存在但 pre_execute 侧为空时对原子工具 fail-open（`middleware.rs:103-110`），与「registry 缺失 fail-closed」语义不对称。
- **runtime.rs 其它**：/stop 后迟到的确认点击仍会执行危险动作（`bot_slash.rs:250-259`：`skill_confirm_result` 找不到 Paused 但 `tx.send(approved)` 照发）；暂停等待期间同会话新消息得到空白回复（`bot_model_loop.rs:471-473`）；`risk_level: low` 未按文档强制 auto（`parse.rs:148-153`）。
- **无重入保护**：聊天路径无 per-skill/per-session 执行锁，同会话两条消息并发命中同一技能路由 → 同一批工具重复执行。

## 已核对一致的部分

- TOOLS schema ↔ execute_tool_impl 分发：28 ↔ 28 一一对应，无孤儿工具；模型编造工具名有「未知工具」兜底。
- 工具参数契约全链路一致（create_word tables / extract_document offset/limit / run_python timeoutSecs / create_ppt theme+customColors / bot_fs 三工具的数字常量）。
- 门禁顺序正确：pre_execute 在 skill_on_step 计步之前，被拦不耗步数；Skill 内工具调用全部过 execute_tool 无旁路；registry 缺失对原子工具 fail-closed 有测试钉死。
- session_id 透传主链（bot_chat → run_model_loop → execute_tool_impl → 各工具）无断点。
- 熔断常数互相一致：轮数 50 / 单轮工具 50 / 软警告 35 / 技能步数默认 8 clamp 1-20 / 绑定文件 10。
