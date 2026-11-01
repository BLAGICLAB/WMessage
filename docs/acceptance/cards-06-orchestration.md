# ACC-O · 编排与执行（L2 实机卡）

> 覆盖 `SPEC.md`「4. 编排与执行」12 条功能（4-91 ~ 4-102），对应矩阵 `docs/acceptance/matrix.csv` 的 D4 域。
> 判据写法与证据规范见 `docs/ACCEPTANCE-PLAN-2026-10-30.md`；审计事件名取自 `docs/acceptance/event-names.md` 白名单。
> 数据目录（macOS）：`~/Library/Application Support/com.renshi.wmessage/`（便携模式随 exe 走，见 `docs/acceptance/cards-08-platform.md` ACC-P-03）。下文的 `<data_dir>` 指该目录。
> 日志检索统一写法：`grep -F "<事件名>" "<data_dir>/bot.log" | tail -20`。
> 子 agent 产物目录：`<data_dir>/AI_Gen_Files` 下的子会话目录；机器可判定验收标准里的相对路径以此为基准。

### ACC-O-01 · 子 Agent 编排：无独立预算 + 幂等键 + 两轴收尾窗

- **SPEC**：4-91
- **层级**：L1+L2（自动档 `src-tauri/src/bot_orchestrator/orchestrator_tests.rs`；实机档见下）
- **历史卡**：M21,M22,M52,M58,M60,S11,M92（部分）
- **前置**：机器人总开关已开；已配可用模型；已用一次 `spawn_subagent`（聊天里说「派个子 agent 去查 X」即可）；本机装一个能连通的模型
- **步骤**：
  1. 聊天里发一句「派一个子 agent 去查一下当前目录有哪些文件」，记下机器人回复里出现的子 agent id（形如 `sa_` 开头）。
  2. 同一轮里让机器人「再派一次同样的子 agent」（目标、档位、验收标准、模型四项与上一步相同）。
  3. 右侧栏（或聊天内嵌区）看子 agent 行：数一数同一个目标出现了几行；读每行右侧的状态徽标文字。
  4. 让子 agent 跑一轮带读文件的活；跑完后在机器人回复里看它报出的子 agent id，与步骤 1 记的是否同一个。
  5. 等子 agent 走到终态，看这一行的终态徽标文字（六态之一）。
  6. 查该子 agent 的实时余量：让机器人对它调一次 `read_own_card`，看返回体里 `budgetUsage` 那块的字段。
  7. 查用量记账：让机器人查这条子 agent 行的三列 token（`prompt_tokens` / `completion_tokens` / `cache_read_tokens`），确认三列各自有值。
  8. 另起一个会话派一个**目标不同**的子 agent，确认它不会被前一个的幂等键吞掉（拿到新 id）。
- **判据**：
  1. 【可观测】步骤 2 的第二次派发**不新增活行**：同一个目标在子 agent 列表里仍只有一行，机器人回复里两次提到的 `subagentId` 是同一个字符串。
  2. 【可观测】步骤 2 的回执文案与步骤 1 不同：第一次说「已派发子 agent」，第二次说「未重复派发」（文案含这六个字，不是「已派发子 agent」）。
  3. 【可观测】步骤 3：子 agent 行的状态徽标只有六种取值——`排队中` / `执行中` / `完成` / `失败` / `已取消` / `预算耗尽`；`已取消` 与 `预算耗尽` 是两个不同的徽标，不合并成「失败」。
  4. 【可观测】步骤 6：`read_own_card` 返回体里有 `budgetUsage` 块，且该块同时含工具数与轮数的「已用 / 上限」两对字段（例如 `toolCallsUsed` / `toolCallsMax` / `roundsUsed` / `roundsMax`）；返回体里**不再有**独立的静态 `budget` 键。
  5. 【可观测】步骤 6 的上限值：工具轴上限 = 400、轮数轴上限 = 200（子 agent 与主会话同一套安全网，无独立预算——子 agent 的上限不小于主会话）。**注意**：配置档声明了 `max_turn` 时（内置 probe=6 / writereditor=8 / validator=4），实际可跑轮数被**收紧**到 `max_turn`，但 `budgetUsage.roundsMax` 仍报常量 200 —— 模型看到的余量比实际能跑的轮数多（见判据 12）。
  6. 【可审计】`<data_dir>/bot.log` 中出现 `tool.call`（检索命令：`grep -F "tool.call" "<data_dir>/bot.log" | tail -20`），行里带子 agent 的 id 与被调工具名——工具轴计数靠它。
  7. 【可审计】步骤 7：`<data_dir>/bot.log` 中出现 `llm.usage`（检索命令：`grep -F "llm.usage" "<data_dir>/bot.log" | tail -20`），行里带 `prompt_tokens` / `completion_tokens` / `cache_read_tokens` 三个键；同一子 agent 的三列在终态后仍有值（不是 NULL）。
  8. 【可核对】`grep -n "MAX_ROUNDS\|MAX_FUNCTION_CALLS" src-tauri/src/bot/constants.rs` 命中 `200` 与 `400` 两个常量；`grep -n "SUBAGENT_MAX_GLOBAL_RUNNING\|SUBAGENT_MAX_PER_SESSION_RUNNING" src-tauri/src/bot/constants.rs` 命中并发闸（3 / 2）——安全网与并发闸都是常量，子 agent 无独立预算。
  9. 【可核对】`grep -n "WRAP_UP_TOOLS" -A 6 src-tauri/src/bot/constants.rs` 命中收尾窗白名单，内容恰为 `write_artifact_file` / `write_file` / `edit_file` / `read_own_card` 四项；`grep -n "FUSE_SOFT_WARN_PCT\|FUSE_WRAP_UP_PCT\|WRAP_UP_ROUNDS" src-tauri/src/bot/constants.rs` 命中 90 / 95 / 3 三个数——两轴各在 90% 软告警、工具轴 95% 进窗、窗内再跑 3 轮。
  10. 【可核对】`grep -n "find_live_subagent_by_request" src-tauri/src/db/subagents.rs` 命中幂等键查询；`grep -n "已派发子 agent\|未重复派发" src-tauri/src/bot/subagent_tools.rs` 命中回执两种文案——幂等键命中时走的是第二句。
  11. 【可核对】`grep -n "SUBAGENT_CHECK_WAIT_MS_CAP" src-tauri/src/bot/constants.rs` 命中 `600_000`（等待上限 10 分钟）；`grep -n "finish_notify" src-tauri/src/bot_orchestrator.rs` 命中进程内 `Notify`——等待靠终态事件唤醒，不是固定心跳。
  12. 【可核对】**已知口径疑点（验收时重点复核）**：`grep -n "let rounds_max" src-tauri/src/bot/subagent_tools.rs` 显示 `read_own_card` 的 `roundsMax` 恒取 `constants::MAX_ROUNDS`（200），而 `grep -n "let max_rounds" -A 6 src-tauri/src/bot/subagent_runner.rs` 显示实际循环上限是 `min(200, 配置档 max_turn)`。点名 `validator`（`max_turn: 4`）跑一个多轮子任务，看它是否在**第 4 轮**就停——若停得住而 `budgetUsage.roundsMax` 仍报 200，记为**「实时余量」报的是共享上限而非生效上限**（模型看到的余量比实际能跑的轮数多 50 倍），开缺陷单。
- **证据**：截图（子 agent 列表同一目标只有一行 + 六态徽标 + `read_own_card` 返回体里的 `budgetUsage`）+ `<data_dir>/bot.log` 片段（`tool.call` / `llm.usage`）
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "<事件名>" src-tauri/src` 确认事件名还在源码里（`tool.call` / `llm.usage` 两个都 grep 得到才算名还在）；子 agent 列表不出现行 = 前端监听问题（查 `subagent-spawned` 事件是否还在 `src/components/ChatPanel/ChatPanel.tsx` 注册）
- **缺口补验**：10-29 全常量化后的 200 轮 / 400 工具安全网与两轴收尾窗新口径未验

### ACC-O-02 · 子任务回报：成功才吸收（两段式）

- **SPEC**：4-92
- **层级**：L2 实机
- **历史卡**：M22（部分）
- **前置**：机器人总开关已开；已配可用模型；能稳定派出一个会失败的子 agent（如给它一个不可能完成的验收标准）
- **步骤**：
  1. 派一个子 agent，验收标准写成 `file_exists:report-does-not-exist.md`（文件必然不存在），让它去跑。
  2. 子 agent 转终态后，看父会话的下一轮里机器人有没有**重复**讲同一份回报。
  3. 停一次模型连通性（或临时把模型条目改错），让子 agent 在模型循环阶段失败（不是任务失败，是「模型压根没跑起来」）；看这段回报之后会不会再被提起。
  4. 恢复模型连通，重启应用，再让机器人汇报同一个子 agent 的结果；看这次汇报是否出现（重启后进程内去重集合清空）。
- **判据**：
  1. 【可观测】步骤 2：同一份子 agent 回报在**同一进程内**只被讲一次——父会话后续轮次里不再重复这段「子任务回报」内容。
  2. 【可观测】步骤 3：模型循环失败时，那份回报**仍留在 digest 里**——恢复连通后的下一轮里机器人重新讲出了这份结果（不是「以为已经说过」而永久沉默）。
  3. 【可观测】步骤 4：重启应用后，同一个子 agent 的结果**会被再讲一次**（进程内去重集合随进程重启清空，这是已知口径，不是缺陷）。
  4. 【可核对】`grep -n "mark_digested" src-tauri/src/bot_chat.rs` 命中，且该调用位于 `bot_chat_inner(...)` 返回**之后**（`for id in &out.absorb`）；`grep -n "mark_digested" src-tauri/src/bot/subagent_digest.rs` 命中定义处——两处合看即「模型循环成功后才记已吸收」。
  5. 【可核对】`grep -n "不在这里记已吸收" -A 4 src-tauri/src/bot/subagent_digest.rs` 命中——digest 构造函数只交出「要注入什么」与「该记哪些 id」，记动作由调用方在循环成功后做。
- **证据**：截图（父会话里回报只出现一次 + 失败后重讲一次的两轮对话）+ grep 终端输出
- **失败处置**：先分清「应用坏了」还是「卡片过期」——本卡无审计事件可查，改用源码检索分家：`grep -n "mark_digested" src-tauri/src/bot_chat.rs` 的调用位置若已挪到 digest 构造之前 = 卡片过期（实现退回两段式之前）
- **缺口补验**：两段式：模型循环成功后才记已吸收

### ACC-O-03 · 子 agent 的收尾契约：三段修复 + digest 补渲染 + 机器判定派发即验

- **SPEC**：4-93
- **层级**：L2 实机
- **历史卡**：M22,M52（部分）
- **前置**：机器人总开关已开；已配可用模型；能在产物目录里手工放一个文件（用于机器可判定验收标准）
- **步骤**：
  1. 派一个子 agent，验收标准写 `file_exists: 我的报告.md`（路径含空格），看派发是否当场被拒。
  2. 改成 `file_exists:../escape.md`（逃逸路径），再看派发是否当场被拒。
  3. 改成 `file_exists:report.md`，正常派发；等它跑完后看父会话 digest 里那行的渲染内容。
  4. 让子 agent 交一份**收尾 JSON 尾部带逗号**的收尾（可通过提示词诱导），看行状态是成功还是 `result_json_unparseable` 失败。
  5. 另派一个子 agent，验收标准含 `file_nonempty:empty.md`（文件存在但为空），看 digest 里成功行是否带「机检未过」。
  6. 看子 agent 的收尾 schema：模型被要求交回的 JSON 里哪些键是必填的。
- **判据**：
  1. 【可观测】步骤 1：派发**当场被拒**（不建行、不起 runner），错误文案说明「冒号后面必须给相对路径」并给出正例（如 `file_exists:report.md`）。
  2. 【可观测】步骤 2：逃逸路径同样当场被拒，文案点明是逃逸路径，不落任何活行。
  3. 【可观测】步骤 3：正常派发后，父会话 digest 里该子 agent 行渲染出摘要；成功行若有机检未过项，另带一行「⚠️ 机检未过 N 项：<条目名>」。
  4. 【可观测】步骤 4：收尾 JSON 带尾逗号时行状态**判成功**（本地修复救回来了），不是 `result_json_unparseable`；只有本地修复也救不回来（散文正文）才落 `result_json_unparseable` 失败。
  5. 【可观测】步骤 5：`file_nonempty:` 指向空文件时，该条判未过；digest 里出现「机检未过 1 项」且条目名是那条标准的文字。
  6. 【可观测】步骤 6：收尾 schema 的必填键只有 `status` 与 `summary` 两项；提示词里的 JSON 骨架不要求 `next_actions` / `confidence` 等键。
  7. 【可审计】`<data_dir>/bot.log` 中出现 `subagent.machine_check_passed` 或 `subagent.machine_check_failed`（检索命令：`grep -F "subagent.machine_check" "<data_dir>/bot.log" | tail -20`），行里带子 agent id 与被核验的相对路径。
  8. 【可审计】散文正文导致收尾解析失败、但机器标准逐条通过而改判成功时，`<data_dir>/bot.log` 中出现 `subagent.parse_failure_recovered`（检索命令：`grep -F "subagent.parse_failure_recovered" "<data_dir>/bot.log" | tail -20`）。
  9. 【可核对】`grep -n "MACHINE_PREFIXES" -A 2 src-tauri/src/bot/subagent_digest.rs` 命中 `file_exists:` / `file_nonempty:` 两个前缀；`grep -n "safe_rel_path" src-tauri/src/bot/subagent_spawn.rs` 命中路径闸（带空格/说明/逃逸一律拒）。
  10. 【可核对】`grep -n "fn repair_json_candidate" -A 30 src-tauri/src/bot/subagent_repair.rs` 命中两处修复（尾逗号 / 截断闭合），且在补答之前调用；`grep -n "必填键只有" -A 6 src-tauri/src/prompts/subagent.rs` 命中 schema 减重后的必填键。
  11. 【可核对】`grep -n "machine_unmet_criteria" -A 12 src-tauri/src/bot/subagent_digest.rs` 命中判定条件——只认 `machine=true` 且 `met=false`，模型自报的 `met=false` 不摆。
- **证据**：截图（派发被拒的提示 + digest 行渲染 + 收尾 schema）+ `<data_dir>/bot.log` 片段（`subagent.machine_check_*`）
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "<事件名>" src-tauri/src` 确认事件名还在源码里（`subagent.machine_check_failed` / `subagent.parse_failure_recovered` 两个都 grep 得到才算名还在）
- **缺口补验**：10-29 撤墙钟后口径已变：三段修复、digest 补渲染、机器判定派发即验未验

### ACC-O-04 · 输入规范化（L0）：不可见字符清洗

- **SPEC**：4-94
- **层级**：L2 实机
- **历史卡**：—（无：全库无实机卡）
- **前置**：机器人总开关已开；已配可用模型；能往聊天输入框里粘带不可见字符的文本（准备方法见步骤）
- **步骤**：
  1. 在一个纯文本编辑器里造四段脏文本：①`\u{FEFF}`（BOM）开头 + 正文；②正文中间插 `\u{200B}`（零宽空格）；③正文用 CRLF 换行；④正文前后带首尾空格。用带引号的写法准备，避免被编辑器本身清掉。
  2. 把第 ① 段粘进聊天输入框并发送；看机器人回复是否**原样**认出正文（不被 BOM 干扰），并查日志。
  3. 把第 ② 段粘进输入框发送；看机器人是否把它当成两个词（零宽空格被换成真空格）。
  4. 把第 ③ 段粘进输入框发送；看库里这条消息的行尾是 LF（不是 CRLF）。
  5. 把第 ④ 段粘进输入框发送；看库里这条消息首尾没有空白。
  6. 发一条**干净**消息（无任何脏字符），查日志里**没有** `user.input_normalized` 这一条。
  7. 另测两条**不许**被清洗的：`\u{200D}`（ZWJ，拼合 emoji 序列，如 👨‍👩‍👧）与 `\u{200C}`（ZWNJ）。
- **判据**：
  1. 【可审计】步骤 2/3/4/5：`<data_dir>/bot.log` 中出现 `user.input_normalized`（检索命令：`grep -F "user.input_normalized" "<data_dir>/bot.log" | tail -20`），行里带 `stripped=`（被删的不可见字符个数，整数）。
  2. 【可审计】步骤 6：这条干净消息**不产生** `user.input_normalized`——有改动才记，没改动不记。
  3. 【可观测】步骤 2：BOM 不影响路由，机器人回复里认得出正文（不是把 BOM 当成正文第一个字符）。
  4. 【可观测】步骤 3：零宽空格被换成**真空格**（不是被删掉导致两个词粘连）——机器人回复里两个词是分开的。
  5. 【可观测】步骤 4：库里这条消息的行尾统一为 LF（用编辑器或 `sqlite3` 打开库看原始字节，无 `\r`）。
  6. 【可观测】步骤 5：库里这条消息首尾无空白。
  7. 【可观测】步骤 7：ZWJ / ZWNJ **保留**——emoji 序列渲染成一个家庭 emoji 而不是三个独立人；波斯文样例里的 ZWNJ 不消失（不变成连写的普通字）。
  8. 【可核对】`grep -n "INVISIBLE_JUNK" -B 12 src-tauri/src/bot_chat.rs` 命中不可见垃圾字符集合（BOM / word-joiner / soft-hyphen），且**不含** U+200D / U+200C；`grep -n "u{200B}" -A 4 src-tauri/src/bot_chat/input.rs` 命中「零宽空格 → 真空格」这一支。
  9. 【可核对】`grep -n "normalize_user_input" src-tauri/src/bot_chat/flow.rs` 命中调用点，且该调用位于 `TurnOrigin::User` 分支内（汇报轮不改写库里的历史）。
- **证据**：截图（机器人回复 + 库里消息的原始字节）+ `<data_dir>/bot.log` 片段（`user.input_normalized`）
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "<事件名>" src-tauri/src` 确认事件名还在源码里（`user.input_normalized` grep 不到 = 卡片过期；grep 得到但日志里没有这一条 = 应用侧回归）
- **缺口补验**：`user.input_normalized` 全库无卡：零宽空格/BOM/CRLF 规范化未验

### ACC-O-05 · 子 Agent 配置档管理：列表 / 导入 / 导出 / 坏文件显形

- **SPEC**：4-95
- **层级**：L1+L2（自动档 `src-tauri/src/bot_onions.rs`）
- **历史卡**：—（无：全库无实机卡）
- **前置**：应用已启动；能进设置页「技能」分区（配置档面板与「机器人技能」同一区）；准备两个 `.yaml` 文件（见步骤）
- **步骤**：
  1. 设置页 → 「技能」分区 → 读「机器人技能」卡下方那一块（配置档列表），数一数有几行、各行写了什么。
  2. 逐行读：名字、角色、档位、工具数、轮数上限；确认前三份带「（内置）」标记。
  3. 在数据目录的 `onions/` 下手工放一个**坏文件**（YAML 语法错误），回设置页刷新，看这一行怎么显形。
  4. 准备一份好文件 `/tmp/acc-onion-good.yaml`，内容含 `onion_id: acc-onion-good`、role、profile、allowed、max_turn；点「导入配置档」选它。
  5. 准备一份坏配对文件 `/tmp/acc-onion-mismatch.yaml`，内容里的 `onion_id` 写成 `acc-onion-other`（与文件名不一致），导入它。
  6. 把步骤 4 那份**同名**再导入一次（改内容后导）。
  7. 点步骤 4 那行的导出按钮，存到 `/tmp/acc-onion-export.yaml`；用编辑器打开看行内注释是否还在。
- **判据**：
  1. 【可观测】步骤 1：配置档列表与「机器人技能」在同一分区（同一页滚动可达），列表每行给出名字 + 角色 + 档位 + 工具数（`工具 N 个`）+ 轮数上限（`轮数上限 N`，未声明则不显示这一段）。
  2. 【可观测】步骤 2：内置三份 `probe` / `writereditor` / `validator` 带「（内置）」标记；行内不含任何「编辑」控件（配置档只支持导入 / 导出）。
  3. 【可观测】步骤 3：坏文件**照样列出**，该行名字染成警示色，行尾显形「— 此文件无效：<坏在哪>」并给出具体原因（不是笼统的「加载失败」）。
  4. 【可观测】步骤 4：导入成功，提示「配置档「acc-onion-good」已导入」，列表里多出该行；新行**不带**「（内置）」。
  5. 【可观测】步骤 5：文件名与 `onion_id` 不一致时**导入被拒**，错误文案点明「文件名须与文件内的 onion_id 一致」；列表里**没有**新增行。
  6. 【可观测】步骤 6：同名导入**被拒**且错误文案说明「已存在：不会静默覆盖」；被导入的那份内容没被换掉（导出后与导入前逐字一致）。
  7. 【可观测】步骤 7：导出的文件是**落盘原文**——行内注释（含 `#` 开头的行）原样保留，不是重新序列化的结果。
  8. 【可观测】步骤 3：坏文件那行**仍可导出**（导出键不禁用）——坏在 YAML 或字段上，原文往往正是要手改的东西。
  9. 【可核对】`grep -n "onions_list\|onions_import\|onions_export" src-tauri/src/lib.rs` 命中三条命令注册；`grep -n "check_import_contract" -A 25 src-tauri/src/bot_onions.rs` 命中判定核（后缀 → 名字闸 → 解析校验 → 文件名与 `onion_id` 一致），命令与单测共用同一个核。
  10. 【可核对】`grep -n "不会静默覆盖" src-tauri/src/bot_onions.rs` 命中同名拒；`grep -n "落盘原文" src-tauri/src/bot_onions.rs` 命中导出按原文写盘。
- **证据**：截图（配置档列表 + 坏文件行 + 导入被拒提示）+ 导出文件的编辑器截图（注释还在）+ grep 终端输出
- **失败处置**：先分清「应用坏了」还是「卡片过期」——本卡无审计事件，改用源码检索分家：`grep -n "check_import_contract" src-tauri/src/bot_onions.rs` 命中 = 契约还在；不命中 = 卡片过期
- **缺口补验**：导入导出、文件名与 onion_id 一致、坏文件显形未实机验

### ACC-O-06 · per-onion 采样温度：配置档温度覆盖模型条目温度

- **SPEC**：4-96
- **层级**：L2 实机
- **历史卡**：—（无：全库无实机卡）
- **前置**：机器人总开关已开；已配可用模型；模型条目上自己设过一个非零温度（例如 0.9）；数据目录 `onions/` 下已有一份带 `temperature: 0` 的配置档（内置 `validator` 即是）
- **步骤**：
  1. 打开模型条目，记下它自带的温度值（设成 0.9 便于观察）。
  2. 派一个**不点名**配置档的子 agent，让它做一件输出应当确定的小事（如「把这句话原样重复三遍」），观察输出。
  3. 派一个点名 `validator` 配置档（`temperature: 0`）的子 agent，做同一件事，观察输出。
  4. 抓这次请求体：看 `temperature` 字段实际送出的值。
  5. 把模型条目的温度改成 0.1，再重做步骤 3，抓请求体。
- **判据**：
  1. 【可观测】步骤 4：点名 `validator` 的那次请求体里 `temperature` 是 `0`，**不是**模型条目的 0.9——配置档压过条目级。
  2. 【可观测】步骤 5：把模型条目改成 0.1 后，点名 `validator` 的请求体里 `temperature` **仍是 `0`**（配置档恒定压过条目）。
  3. 【可观测】步骤 2：不点名配置档时，请求体里的 `temperature` 取模型条目的值（0.9），不是配置档的值——覆盖只在点名配置档时生效。
  4. 【可核对】`grep -n "fn resolve_temperature" -A 18 src-tauri/src/bot_model_loop/shell.rs` 命中优先级（配置档 > 条目级），注释里写明「采样参数不是权限，不适用只收紧」。
  5. 【可核对】`grep -n "temperature_override" -B 4 -A 6 src-tauri/src/bot_model_loop/shell.rs` 命中注入点；`grep -n "temperature: 0" src-tauri/src/bot_onions.rs` 命中内置 validator 档的声明。
  6. 【可核对】`grep -n "fn apply_inference_params" -A 8 src-tauri/src/bot_model_loop/request.rs` 命中「有值才写请求体」——未点名配置档时不写这个键。
- **证据**：截图（模型条目温度设置 + 两次子 agent 输出）+ 抓到的请求体片段（含 `temperature` 字段）+ grep 终端输出
- **失败处置**：先分清「应用坏了」还是「卡片过期」——本卡无审计事件，改用源码检索分家：`grep -n "resolve_temperature" src-tauri/src/bot_model_loop/shell.rs` 命中 = 覆盖逻辑还在；不命中 = 卡片过期
- **缺口补验**：配置档温度覆盖模型条目温度未验

### ACC-O-07 · 任务图谱 = 三维星云：节点大小 / 着色 / 完成口径 / 降级

- **SPEC**：4-97
- **层级**：L1+L2（自动档 `src/components/GraphPage/graph-*.test.ts`（9 个纯函数文件））
- **历史卡**：M13,M67,S26,S27,W07,M92（部分）
- **前置**：应用已启动；看板里已有 ≥10 张任务卡（其中 ≥3 张带标签、≥2 个不同成员、≥2 张「已完成」列的卡）；能改任务依赖
- **步骤**：
  1. 左导航 → 「任务图谱」；等构图跑完（画布上「构图计算中…」消失）。
  2. 数一数节点数量；点一个连边最多的节点，看它铭牌里的度数数字；再点一个孤立的节点（度数 0 或 1）看它的点径。
  3. 切换着色维度（顶部图例按钮「按标签」/「按成员」/「按状态」），每次看节点颜色与图例 HUD 的变化。
  4. 看状态着色档：进行中的节点什么色、逾 14 天没动的「钉子户」什么色、「已完成」的节点什么色（是否低亮石板灰）。
  5. 在画布内改一条依赖（拖一条新边或删一条），看构图是否重置（节点位置是否全部重新洗牌）。
  6. 把系统设置切到「减少动效」（macOS：系统设置 → 辅助功能 → 显示 → 减少动态效果），回图谱页，看相机是否自转、云噪声是否漂。
  7. 在图谱页用方向键在节点间移动游标，回车选中，Esc 清除选择；听是否有 aria-live 播报。
  8. 在浏览器控制台或设置里禁用 WebGL2 后重开图谱页（另见 `docs/acceptance/cards-90-windows.md` 的硬件加速口径）。
- **判据**：
  1. 【可观测】步骤 2：节点大小**只与连接数有关**——度数高的节点点径明显更大；度数 0/1 的孤立节点是全场最小的点。同一张图里没有两个度数相同而点径差很多的节点。
  2. 【可观测】步骤 2：孤立节点的点径**不塌成亚像素**（屏幕上仍看得见一个点，不是一层灰雾）。
  3. 【可观测】步骤 3：着色维度恰好三档，按钮文案是「按标签」/「按成员」/「按状态」，默认落在「按标签」；切档后节点颜色随之改变，图例 HUD 同步换成该档的说明。
  4. 【可观测】步骤 4：状态着色档里，进行中的节点是青绿（`#35d6b5` 一族）、待办是粉红（`#ff4d8d` 一族）、逾 14 天未动的钉子户是琥珀（`#e8a33d` 一族）、已完成是低亮石板灰。
  5. 【可观测】步骤 5：页内改依赖**不重置构图**——改完只有受影响的那几个节点移动，其余节点坐标保持不变（不是全部重新洗牌）。
  6. 【可观测】步骤 6：系统声明「减少动效」时，相机不自转、云噪声不漂；布局不播中间态（直接出终态位置）。
  7. 【可观测】步骤 7：方向键能在任务节点间移动游标、回车选中、Esc 清除选择；游标落点有 aria-live 文本播报（形如「任务：<标题>」）。
  8. 【可观测】步骤 8：拿不到 WebGL2 上下文时，画布位置给出说明文字（含「WebGL2」四字），**不是白屏**，也不抛未捕获异常。
  9. 【可核对】`grep -n "export function nodeRadius" -A 6 src/components/GraphPage/graph-scene.ts` 命中「连接数 √ 压缩」（公式含 `Math.sqrt`），函数里**没有**耗时字段入参。
  10. 【可核对】`grep -n "NODE_PX_MIN" src/components/GraphPage/graph-scene.ts` 命中星点屏幕下限（1.6 px，按布局外径折算，不写死世界值）。
  11. 【可核对】`grep -n "REST_RATIO" -A 10 src/components/GraphPage/graph-scene.ts` 命中弹簧静息长度（任务 / 标签 / 成员 / 状态四档按布局外径比例给）；`grep -n "BLOOM_THRESHOLD" src/components/GraphPage/GraphCanvas.tsx` 命中自发光阈值（0.42）——星体越过它起辉、云团恒在其下。
  12. 【可核对】`grep -n "isTaskCompleted" src/components/GraphPage/graph-build.ts` 命中——图谱的完成口径走 `lib/taskScope.isTaskCompleted` 这一个判定源。
  13. 【可核对】`grep -n "textContent" src/components/GraphPage/GraphCanvas.tsx` 命中铭牌装配用 `textContent`（**不是** `innerHTML`）——标题里的尖括号不会被当标记解析。
- **证据**：截图（星云全景 + 三档着色各一张 + 孤立节点近景 + 减少动效态 + WebGL2 缺失说明）+ grep 终端输出
- **失败处置**：先分清「应用坏了」还是「卡片过期」——本卡无审计事件，改用源码检索分家：`grep -n "nodeRadius" src/components/GraphPage/graph-scene.ts` 命中 = 节点大小口径还在；不命中 = 卡片过期
- **缺口补验**：自发光阈值、力场锚点、弹簧静息长度、星点缩放下限未验

### ACC-O-08 · 词元统计四档计价：缓存单列不折进输入

- **SPEC**：4-98
- **层级**：L1+L2（自动档 `src-tauri/src/db/trace/tests.rs`）
- **历史卡**：M61（全）
- **前置**：机器人总开关已开；已配可用模型；至少跑过一轮长对话（轮数越多缓存命中越多）
- **步骤**：
  1. 设置页 → 「词元统计」分区；看汇总行的两格：「累计缓存未命中」与「累计缓存命中」。
  2. 聊天里跑几轮（每轮把上一轮的历史带回去），看聊天页会话水位条上的数字分档。
  3. 开一次执行详情（点某个回合的「执行轨迹」），看 token 数后面的缓存读/写两档。
  4. 直接查库：读 `exec_traces` 表某几行的 `prompt_tokens` / `cache_read_tokens` / `cache_write_tokens` 三列。
  5. 查一个 OpenAI 形状的模型条目（若只有 Anthropic 形状则跳过本步，记录跳过理由）。
- **判据**：
  1. 【可观测】步骤 1：汇总行恰好两格并列——「累计缓存未命中」与「累计缓存命中」；缓存命中**没有**被并进「未命中」那一格。
  2. 【可观测】步骤 2：水位条上缓存读/写是**单列**，不折进输入/输出；当前 prompt 体量按「最近一轮 input + cache read」算（长会话里 input 只剩增量，cache read 是大头）。
  3. 【可观测】步骤 3：执行详情弹窗在 token 数之后附「缓存读 / 缓存写」两档，不是合并成一个总数。
  4. 【可核对】步骤 4：`exec_traces` 表存在 `cache_read_tokens` 与 `cache_write_tokens` 两列，且它们**不是** `prompt_tokens` 的一部分——某行 `prompt_tokens` 小于该次请求的总输入量时，差额正好落在缓存列上。
  5. 【可核对】`grep -n "cache_read_tokens\|cache_write_tokens" src-tauri/src/db/trace.rs` 命中两列的建表与读写；`grep -n "cached_tokens" -B 4 -A 8 src-tauri/src/bot_model_loop/stream.rs` 命中 OpenAI 形状解析时**减去缓存档**再落库（`prompt_tokens` 含 `cached_tokens`，落库前减掉）。
  6. 【可核对】`grep -n "stream_options" -B 3 -A 8 src-tauri/src/bot_model_loop/request.rs` 命中两处：请求侧带 `include_usage` 索取末块 usage；网关不认时收到 400 即降级（摘字段原样重发、该次不记用量）。
  7. 【可核对】`grep -n "llm.stream_options_unsupported" src-tauri/src/bot_model_loop/request.rs` 命中降级审计事件名（该事件在 `docs/acceptance/event-names.md` 白名单内）。
  8. 【可审计】用过 OpenAI 形状模型且网关不支持 `stream_options` 时，`<data_dir>/bot.log` 中出现 `llm.stream_options_unsupported`（检索命令：`grep -F "llm.stream_options_unsupported" "<data_dir>/bot.log" | tail -20`）。
- **证据**：截图（词元统计汇总两格 + 水位条分档 + 执行详情缓存两档）+ `exec_traces` 三列的查询输出 + `<data_dir>/bot.log` 片段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "<事件名>" src-tauri/src` 确认事件名还在源码里（`llm.stream_options_unsupported` grep 不到 = 卡片过期）
- **缺口补验**：—

### ACC-O-09 · Agent 透明化：执行详情 / 通知中心 / 痕迹保留期清扫

- **SPEC**：4-99
- **层级**：L1+L2（自动档 `src/components/TracePanel/TracePanel.test.tsx`）
- **历史卡**：M60,M61,M71,S14,S16（部分）
- **前置**：应用已启动；机器人跑过至少一轮带工具调用与文件改动的任务；受管服务（如有）曾经起过
- **步骤**：
  1. 聊天里找一个回合，点助手气泡上的「执行轨迹」，看执行详情面板里列了什么（工具、耗时、成败、文件改动）。
  2. 在文件改动条目上点回滚，看能否把文件恢复到本次改动前。
  3. 把执行详情导出成 JSONL，看导出结果落盘在哪。
  4. 左导航 → 「通知」，看收件箱里三类决策消息（记忆提案 / 产物绑定 / 任务提问）与服务告警分别长什么样；逐类点它的处理按钮。
  5. 设置页 → 「数据管理」，找执行痕迹的保留期设置（公示的天数）与「立即清理」按钮，点它。
  6. 重启应用，等启动后的自动清扫跑完，再看痕迹条数与日志。
- **判据**：
  1. 【可观测】步骤 1：执行详情面板同时给出**工具调用**（名称 + 耗时 + 成败）与**文件改动证据**（±行数 / diff），失败项带 `error_class` 分类。
  2. 【可观测】步骤 2：文件级回滚可用——点回滚后文件内容恢复到本次 AI 修改前，且改动清单里该条标为已回滚。
  3. 【可审计】步骤 2：`<data_dir>/bot.log` 中出现 `file.rollback`（检索命令：`grep -F "file.rollback" "<data_dir>/bot.log" | tail -20`）。
  4. 【可审计】步骤 3：`<data_dir>/bot.log` 中出现 `trace.export`（检索命令：`grep -F "trace.export" "<data_dir>/bot.log" | tail -20`），导出文件落在数据目录的 `exports/` 下。
  5. 【可观测】步骤 4：通知收件箱里三类**决策**消息各自独立成卡——「记忆提案」（可批准 / 拒绝）、「产物绑定」、「任务提问」；受管服务告警是第四类、只给「知道了」一个出口，不带决策按钮。
  6. 【可观测】步骤 5：数据管理卡公示执行痕迹保留期（默认 30 天），并有手动清理入口；点完给出清理条数或「无过期痕迹」的明确回执，不是静默无反应。
  7. 【可审计】步骤 5：有痕迹被删时 `<data_dir>/bot.log` 中出现 `trace.retired`，行里带 `removed=`（删掉条数）与 `days=`（按的天数）；没有过期痕迹时**不产生**这一条（`removed=0` 不写）。
  8. 【可审计】步骤 6：启动后自动清扫发生时 `<data_dir>/bot.log` 中同样出现 `trace.retired`（启动一次 + 每天一次，两条触发共用同一口径）。
  9. 【可核对】`grep -n "spawn_retention_sweep" src-tauri/src/lib.rs` 命中启动即跑；`grep -n "fn retire_traces_before_days" -A 20 src-tauri/src/db/trace.rs` 命中手动与自动共用的清理内核；`grep -n "trace.retention_days" src-tauri/src/bot/params.rs` 命中参数表里的保留期那一行。
  10. 【可核对】`grep -n "KIND_MEMORY\|KIND_TASK_QUESTION" src-tauri/src/notifications.rs` 命中通知 kind 常量（决策消息落库按 kind 分类，不是一锅粥）。
- **证据**：截图（执行详情面板 + 回滚前后 + 通知收件箱三类卡 + 数据管理保留期卡）+ `<data_dir>/bot.log` 片段（`file.rollback` / `trace.export` / `trace.retired`）
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "<事件名>" src-tauri/src` 确认事件名还在源码里（`file.rollback` / `trace.export` / `trace.retired` 三个都 grep 得到才算名还在）
- **缺口补验**：通知中心三类决策事件消息化、痕迹三表保留期清扫未验

### ACC-O-10 · 执行痕迹的入口与筛选：会话维度直查 + 后端按来源过滤

- **SPEC**：4-100
- **层级**：L1+L2（自动档 `src/components/ActivityPage/ActivityPage.test.tsx`）
- **历史卡**：—（无：全库无实机卡）
- **前置**：应用已启动；同一个会话里跑过 ≥2 轮带工具调用的对话；另有手动 / 定时 / 批量来源的执行痕迹各至少一条
- **步骤**：
  1. 左导航 → 「活动」，读来源筛选排：有哪些筛选项。
  2. 选「聊天」，看列表里出现的执行是不是都属聊天来源；记下条数。
  3. 选「定时」，看列表内容是否全部换成定时来源的执行。
  4. 先在「全部」里翻到第 50 条之后仍存在的那类来源（若不足 50 条则用批量来源凑），再切到该来源筛选——看它**能不能被筛出来**。
  5. 回到聊天页，点某个回合助手气泡上的「执行轨迹」，看打开的是不是**那一回合**的执行。
  6. 在同一个会话里换另一回合再点「执行轨迹」，看是否列出该会话的多次执行供切换。
- **判据**：
  1. 【可观测】步骤 1：来源筛选排有一排按钮，「全部」之外还有「手动 / 定时 / 批量 / 聊天」四档；当前档按钮呈选中态。
  2. 【可观测】步骤 2/3：切到某一来源后，列表里每一行的来源标签都是该来源（不是「先取 50 条再在前端筛」——那会出现混排或空列表）。
  3. 【可观测】步骤 4：排在 50 条之外的执行**能被筛出来**（列表非空且全是该来源）；若筛出来是空列表 = 过滤在前端做的，是缺陷。
  4. 【可观测】步骤 5：点某个回合的「执行轨迹」，打开的执行详情就是**那一回合**的那一次执行（不跳到同会话里别的执行）。
  5. 【可观测】步骤 6：同一会话的多次执行列成一条条可切换的条目，不是只显示最后一次。
  6. 【可核对】`grep -n "traceListByOrigin" -A 8 src/lib/trace.ts` 命中前端把 origin 传给后端命令 `trace_list`；`grep -n "pub fn trace_list" -A 14 src-tauri/src/db/trace.rs` 命中后端按 `origin` 过滤（过滤发生在 SQL 侧，不在返回后）。
  7. 【可核对】`grep -n "origin" src/components/ActivityPage/index.tsx` 命中「切筛选即重拉」的逻辑（切档时重新 invoke，而不是拿已有 50 条在前端过滤）。
- **证据**：截图（活动页来源筛选排 + 各来源筛选后的列表 + 「执行轨迹」打开的执行详情）+ grep 终端输出
- **失败处置**：先分清「应用坏了」还是「卡片过期」——本卡无审计事件，改用源码检索分家：`grep -n "traceListByOrigin" src/lib/trace.ts` 命中 = 后端过滤接线还在；不命中 = 卡片过期
- **缺口补验**：「活动页」全库无卡：后端按 origin 过滤未验

### ACC-O-11 · Agent 运行参数全常量化：无参数卡、只读三行

- **SPEC**：4-101
- **层级**：L1+L2（自动档 `src-tauri/src/bot/constants.rs`）
- **历史卡**：—（无：全库无实机卡）
- **前置**：应用已启动；能进设置页；能读数据目录下的配置文件
- **步骤**：
  1. 设置页左侧分类逐个读一遍，找有没有「Agent 运行参数」卡。
  2. 打开数据目录的 `bot-config.json`，搜风控相关键名（`max_rounds` / `max_function_calls` / `history_budget` / `max_tokens` / `tool_output`）。
  3. 设置页 → 「数据管理」→ 找参数只读表，数一数有几行、各是什么。
  4. 在「机器人」分区找 Python 超时、痕迹保留期、记忆抽取限频这三处取值入口，确认它们的生效值与范围。
  5. 确认输入栏 ⚡（推理强度）按会话覆盖是否还在。
- **判据**：
  1. 【可观测】步骤 1：设置页**没有**「Agent 运行参数」卡；左侧 13 个分类里没有这一项（分类名逐个可数：基础设置 / 数据管理 / 机器人 / 模型设置 / 记忆 / 技能 / 任务图谱 / MCP 服务 / 本地服务 / 文件整理 / 词元统计 / 操作历史 / 日志）。
  2. 【可核对】步骤 2：`bot-config.json` 里搜不到风控键——200 轮 / 400 工具 / 128K 历史预算 / 16384 max_tokens / 128KB 回填上限**全是常量**，没有配置键。
  3. 【可核对】`grep -n "MAX_ROUNDS\|MAX_FUNCTION_CALLS\|HISTORY_BUDGET_CHARS\|MAX_TOKENS_DEFAULT\|TOOL_OUTPUT_BYTES" src-tauri/src/bot/constants.rs` 命中五个常量，值依次是 200 / 400 / 128_000 / 16_384 / 128 * 1024。
  4. 【可观测】步骤 3：只读参数表恰好**三行**——Python 超时（`tools.python_timeout_secs`）、痕迹保留期（`trace.retention_days`）、记忆抽取限频（`memory.extract_min_interval_secs`）；没有轮数 / 工具数 / 历史预算那几行。
  5. 【可核对】`grep -n "key: \"" src-tauri/src/bot/params.rs` 命中恰好三个 `key:`（上述三个），与步骤 3 数出的行数一致。
  6. 【可核对】`grep -n "MAX_TOKENS_CAP\|MAX_TOKENS_DEFAULT" src-tauri/src/bot/constants.rs` 命中单轮 max_tokens 的默认 16384 与硬顶 32768；`grep -n "TOOL_WINDOW_TRIGGER_CHARS\|TOOL_WINDOW_KEEP_RECENT" src-tauri/src/bot/constants.rs` 命中滑动窗常量——回灌「发了就回」、滑动窗恒开，无开关。
  7. 【可观测】步骤 5：聊天输入栏的 ⚡ 按会话覆盖仍在（推理强度后台默认档常量化为 medium，会话覆盖保留）。
- **证据**：截图（设置页分类列表 + 只读参数三行 + 输入栏 ⚡）+ `bot-config.json` 的 grep 输出 + 常量 grep 输出
- **失败处置**：先分清「应用坏了」还是「卡片过期」——本卡无审计事件，改用源码检索分家：`grep -n "MAX_ROUNDS" src-tauri/src/bot/constants.rs` 命中 = 常量还在；不命中 = 卡片过期
- **缺口补验**：撤参数卡后无卡：常量与生效值口径未验

### ACC-O-12 · 程序：归档（可逆）与彻底删除（不可逆）两个不同图标

- **SPEC**：4-102
- **层级**：L1+L2（自动档 `src-tauri/src/apps/manage/tests.rs`）
- **历史卡**：M30,M42,M56,M69,M70（部分）
- **前置**：应用已启动；程序页已有 ≥1 个程序（草稿或已上架）；能进「程序」分区；能查数据目录的 `apps/` 子目录
- **步骤**：
  1. 左导航 → 「程序」，看每个程序卡右上角的图标（**钥匙扣/归档图标**），并确认这一排图标里**没有**垃圾桶。
  2. 点某个程序的归档图标，看确认框写了什么；确认后看程序页列表与头部「已归档 N」。
  3. 点头部「已归档 N」进归档视图，看这一行上的图标（**垃圾桶**），并确认归档视图里**没有**归档键。
  4. 在归档视图里点垃圾桶，看确认框逐项写明会失去什么。
  5. 确认删除；然后查数据库三张表与磁盘目录：行是否都没了、目录是否整个没了。
  6. 在归档视图里对另一个程序点恢复，看它是否回到程序页的草稿态。
  7. 看程序卡的「单次运行时长（秒）」输入位与卡片上的声明值显示。
  8. 跑一个必然超时的程序（或把它的 `timeoutSecs` 声明调到很小），看运行记录里的状态与耗时。
- **判据**：
  1. 【可观测】步骤 1：程序卡上的那个键是**归档**（Archive 钥匙扣图标），不是垃圾桶；程序页这一排**没有**任何垃圾桶图标。
  2. 【可观测】步骤 2：确认框写明「脚本、数据与运行历史都保留，可在「已归档」里恢复」，确认后该程序从程序页消失、头部「已归档 N」的 N 加一。
  3. 【可观测】步骤 3：归档视图里那一行是**垃圾桶**（Trash2）图标，与程序页的归档图标**形状不同**；归档视图里**没有**归档键（它已经是归档态）。
  4. 【可观测】步骤 4：彻底删除的确认框逐项写明将失去的东西：脚本与说明书、`data/` 里的预置数据、`input/` 里用户自己放的文件、`output/` 里的全部产物、运行历史与版本快照；按钮文案是「彻底删除」。
  5. 【可核对】步骤 5：`apps` / `app_versions` / `app_runs` 三张表里该程序的行都没了（查库各得 0 行）；数据目录 `apps/<id>/` 整个目录不存在（`data/` / `input/` / `output/` 一并没了）。
  6. 【可核对】`grep -n "先库后文件" -B 4 -A 12 src-tauri/src/apps/manage.rs` 命中删除顺序——**先删库、后删目录**（反过来会留下「库里说有、盘上没了」的孤儿条目）。
  7. 【可观测】步骤 6：归档视图里的「恢复」把程序放回程序页，状态回到草稿态；恢复最后一条归档后「已归档」开关**仍然在**（还能切回程序列表）。
  8. 【可观测】步骤 7：程序卡上显示声明的单次运行时长（形如「最长 N 分/秒」），编辑区有「单次运行时长（秒）」输入位，留空 = 用内置默认。
  9. 【可核对】`grep -n "fn normalize_declared_timeout" -A 8 src-tauri/src/apps/manage.rs` 命中声明值钳制（钳到 apps 域天花板 1800 秒 = 30 分钟）；`grep -n "APPS_MAX_TIMEOUT_SECS" src-tauri/src/py/runtime.rs` 命中 `1800`；`grep -n "DEFAULT_TIMEOUT_SECS" src-tauri/src/py/runtime.rs` 命中内置默认 120 秒。
  10. 【可核对】`grep -n "fn resolve_run_timeout" -A 8 src-tauri/src/apps/commands.rs` 命中取值优先级——调用方显式值 > 程序声明 > 内置默认 120 秒（该函数已把结果钳到 apps 天花板 1800）。
  11. 【可观测】步骤 8：超时的那次运行，运行记录的状态是「超时」一类（**不是**「运行启动失败」），且**记了真实耗时**（不是 0）。
  12. 【可核对】`grep -n "\"timeout\"\|\"stopped\"\|\"failed\"" src-tauri/src/apps/mod.rs` 命中 `app_runs` 表 status 列的四档取值 `ok` / `failed` / `timeout` / `stopped`；`grep -n "fn failed_run_report" -A 8 src-tauri/src/apps/mod.rs` 命中失败分类——超时那次程序明明跑起来了，不记成「运行启动失败」。
  13. 【可核对】`grep -n "Archive\|Trash2" src/components/AppsPage/AppsPage.tsx` 命中两个**不同**的图标名——不可逆与可逆不共用同一个垃圾桶图标。
  14. 【可核对】**已知口径疑点（验收时重点复核）**：`grep -n "fn resolve_timeout" -A 8 src-tauri/src/py/runtime.rs` 的上限是 `MAX_TIMEOUT_SECS = 600`（run_python 工具路径的上限），而 apps 域的天花板是 `APPS_MAX_TIMEOUT_SECS = 1800`；两者不在同一层——`apps/commands.rs::resolve_run_timeout` 先钳到 1800，随后 `apps/mod.rs::run` 把该值原样交给 `runtime::run_python` → `run_python_at` → `resolve_timeout`，**在那里又被 600 钳一次**。按此链路推演：声明 900 / 1800 / 7200 秒的程序，实际生效值都是 600 秒（`resolve_timeout` 报 `clamped=true`）。验收时用一个声明 >600 秒的程序实测真实运行时长：若不到 600 秒就被砍，记为**实现与 SPEC 不符**（SPEC 4-102 写「一律钳到全局天花板 30 分钟」），开缺陷单。
- **证据**：截图（程序页归档图标 + 归档视图垃圾桶图标 + 两个确认框全文 + 运行记录状态）+ 三张表的查询输出 + `apps/` 目录列表（删除前后）+ grep 终端输出
- **失败处置**：先分清「应用坏了」还是「卡片过期」——本卡无审计事件，改用源码检索分家：`grep -n "fn delete_core" src-tauri/src/apps/manage.rs` 命中 = 彻底删除逻辑还在；不命中 = 卡片过期
- **缺口补验**：归档视图、彻底删除三表与目录、整文件集上架、版本回滚、timeoutSecs 未验
