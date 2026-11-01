# ACC-E · 模型错误与守卫（L2 实机卡）

> 覆盖 `SPEC.md`「3 机器人聊天与模型」第 3 节的 22 条功能条目（3-57 ~ 3-78），对应矩阵 `docs/acceptance/matrix.csv` 的 D3b 域。
> 判据写法与证据规范见 `docs/ACCEPTANCE-PLAN-2026-10-30.md`；审计事件名取自 `docs/acceptance/event-names.md` 白名单。
> 数据目录（macOS）：`~/Library/Application Support/com.renshi.wmessage/`（便携模式随 exe 走，见 `docs/acceptance/cards-08-platform.md` ACC-P-03）。下文的 `<data_dir>` 指该目录。
> 日志检索统一写法：`grep -F "<事件名>" "<data_dir>/bot.log" | tail -20`。
> 错误注入统一走**可编排的 mock 网关**（返回指定状态码 / 上游错误对象的 `type` 字段 / 200 流内错误载荷），不真去撞上游。

### ACC-E-01 · 截断自愈

- **SPEC**：3-57
- **层级**：L2 实机
- **历史卡**：—（无：全库无实机卡）
- **前置**：已配 Anthropic 兼容协议的模型；该模型支持思维链；`max_tokens` 留空走默认（当前默认 16384，硬顶 32768）
- **步骤**：
  1. 设置页 → 模型中心 → 该 Anthropic 模型条目，把 `max_tokens` 调到 1024（造「思维链吃光预算」的条件），保存。
  2. 进聊天，发一条需要长输出的请求（如「写一份 3000 字的架构说明」）。
  3. 看助手气泡里的正文是否被截断、有没有续跑后的接续内容。
  4. 用 mock 网关把上游的 `stop_reason` 固定为 `max_tokens`，再发一次，观察是否续跑。
- **判据**：
  1. 【可审计】`<data_dir>/bot.log` 中出现 `llm.length_continued`（检索命令：`grep -F "llm.length_continued" "<data_dir>/bot.log" | tail -20`），其 `attempt` 字段 ≤ 2。
  2. 【可观测】助手气泡里的最终正文是**拼接后的完整版**：前半段与首轮输出一致，后半段是续跑补上的内容，中间无重复段落（续跑注入的提示是「上一条回复超出长度上限被截断。请从断处接着写，不要重复已写的内容」）。
  3. 【可观测】正文尾部**没有**「（回复因长度限制被截断，可以让我「继续」补完）」这句兜底提示——该提示只在续跑配额用尽后才出现。
  4. 【可核对】`grep -n "MAX_LENGTH_CONTINUATIONS" src-tauri/src/bot_model_loop/finalize.rs` 显示 `const MAX_LENGTH_CONTINUATIONS: usize = 2;`，与判据 1 的 `attempt` 上界一致。
  5. 【可核对】`grep -n "max_tokens" src-tauri/src/bot_anthropic.rs` 显示 Anthropic 的 `stop_reason=max_tokens` 被 `map_stop_reason` 映射为 `length`，是续跑的触发条件。
- **证据**：截图（截断态与续跑后的最终气泡）+ `<data_dir>/bot.log` 中 `llm.length_continued` 的行
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -rnF "llm.length_continued" src-tauri/src` 确认事件名还在源码里；无该行 = 本轮没撞上限（换更长的请求或更小的 `max_tokens` 再试），有该行但正文没接上 = 应用侧缺陷
- **缺口补验**：全库无命中：Anthropic 撞 max_tokens 后续跑 ≤2 轮拼接

### ACC-E-02 · 大模型 API 错误说人话

- **SPEC**：3-58
- **层级**：L1+L2（L1 自动化 `src/components/ChatPanel.stream.test.tsx`；实机档见下）
- **历史卡**：rerun§2.2（`docs/realmachine-rerun-2026-10-22.md`）
- **前置**：已配 OpenAI 兼容协议的模型；可编排的 mock 网关可按需返回指定状态码与错误体
- **步骤**：
  1. 把 mock 网关配成返回 402（body 内含 `{"type":"error","error":{"type":"insufficient_balance_error","message":"账户余额不足"}}`），在聊天里发一条消息。
  2. 看气泡里显示的完整文案，逐字读。
  3. 依次把网关改成 401 / 403 / 404 / 429 / 500，各发一条消息，每次记下气泡文案。
  4. 再把网关改成 400（body 内含上游自己的 `message`），发一条消息。
  5. 在 `<data_dir>/bot.log` 里检索本轮错误事件，确认日志里没有上游原始 JSON。
- **判据**：
  1. 【可观测】402 那一轮的气泡文案形如「大模型 API 错误 402：账户余额或额度不足：请到厂商控制台充值，或在设置页切换其他大模型（上游：账户余额不足）」——含状态码指引，且**不含**上游原始 JSON（气泡里不得出现 `insufficient_balance_error` 或 `{"type":"error"` 这类字面）。
  2. 【可观测】其余状态码的指引逐条对得上：401 → 「API Key 无效或未配置：请到设置页「机器人设置」检查密钥」；403 → 「该模型无权访问」；404 → 「模型或接口不存在（可能已下线）」；429 → 「上游限流：请稍后重试」；5xx → 「上游服务异常：请稍后重试」。
  3. 【可观测】400 那一轮**没有**状态码指引句，只有上游自己说的那句——400 不进指引表。
  4. 【可观测】每条错误文案里的上游原句长度 ≤ 160 字（超出部分被截断，句尾不再有完整 JSON）。
  5. 【可审计】`<data_dir>/bot.log` 中本轮错误事件带 `code=LLM_API_ERROR`；日志行里**不出现**上游原始 JSON 片段（用 `grep -F "insufficient_balance_error" "<data_dir>/bot.log"` 应无命中）。
  6. 【可观测】前端 `LLM_API_ERROR` 的兜底提示**不是**「请稍后重试」，而是「请检查 API 配置与账户状态后再重试」。
- **证据**：截图（402/400 两轮气泡文案逐字）+ `<data_dir>/bot.log` 片段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -rnF "llm.response" src-tauri/src` 确认事件名还在源码里；气泡甩原始 JSON = 应用侧缺陷（人话化未生效），气泡文案对但日志里还有 JSON = 审计侧漏脱敏
- **缺口补验**：—

### ACC-E-03 · 200 流内错误也进重试网

- **SPEC**：3-59
- **层级**：L2 实机
- **历史卡**：—（无：全库无实机卡）
- **前置**：可编排的 mock 网关可返回 **HTTP 200** 且流内带错误载荷（Anthropic 形状 `{"type":"error","error":{"type":"overloaded_error",...}}` 或 OpenAI 形状 `{"error":{"type":"rate_limit_error",...}}`）
- **步骤**：
  1. mock 网关配成：HTTP 200，第一条 SSE 数据是 `{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}`，之后恢复正常流。
  2. 在聊天里发一条消息，观察这一轮是失败还是自己恢复了。
  3. 网关改成 `rate_limit_error` 再发一条；改成 `authentication_error` 再发一条；改成自造的 `my_custom_error` 再发一条。
  4. 网关改成「先吐一段正文、再吐流内错误」，发一条消息，观察是否重发（不应重发：已经向前端吐过内容）。
  5. 检索 `<data_dir>/bot.log` 里的流内错误与重试记录。
- **判据**：
  1. 【可审计】`<data_dir>/bot.log` 中出现 `llm.stream_error`（`grep -F "llm.stream_error" "<data_dir>/bot.log" | tail -20`），且该行带 `code=LLM_API_ERROR`、`recoverable=true`——**不是** `code=INTERNAL`。
  2. 【可审计】`overloaded_error` 与 `rate_limit_error` 两轮各出现一条 `llm.stream_retry`，其 `attempt=1`（重发上限 1 次）；`authentication_error` 与自造 type 两轮**没有** `llm.stream_retry`（不可重试，不重发）。
  3. 【可观测】`overloaded_error` 与 `rate_limit_error` 两轮最终**给出了正常答复**（重发后成功）；`authentication_error` 那轮给出的是错误文案，气泡里含状态码指引「API Key 无效或未配置」，不是「不可自动重试；请反馈日志」。
  4. 【可观测】第 4 步那一轮（先吐正文再报错）**没有**重复内容：气泡里用户看到的正文只有一份，没有同一段话出现两遍。
  5. 【可核对】`grep -n "overloaded_error\|api_error\|server_error\|rate_limit_error\|authentication_error\|insufficient_balance_error\|insufficient_quota" src-tauri/src/bot_model_loop/sse.rs` 列出全部归一映射；其中 `overloaded_error`→529、`api_error`/`server_error`→500（两者都落在 5xx 段、可重试）、`rate_limit_error`→429、`authentication_error`→401、`insufficient_balance_error`/`insufficient_quota`→402。
  6. 【可核对】`grep -n "MAX_STREAM_RETRIES" src-tauri/src/bot_model_loop/request.rs` 显示 `pub(crate) const MAX_STREAM_RETRIES: usize = 1;`——与判据 2 的重发上限一致。
- **证据**：截图（各轮气泡终态）+ `<data_dir>/bot.log` 中 `llm.stream_error` / `llm.stream_retry` 的行
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -rnF "llm.stream_retry" src-tauri/src` 确认事件名还在源码里；`llm.stream_error` 出现但 `recoverable=false` = 应用侧缺陷（流内错误被降级成内部错误）
- **缺口补验**：10-26 新增，全库无命中：HTTP 200 流内报错按 type 归一状态码并进重试网

### ACC-E-04 · 新增代码不得再走 blanket 降级

- **SPEC**：3-60
- **层级**：L0 门禁（本卡不进实机，是提交即跑的机器判定项）
- **历史卡**：—（无：全库无实机卡）
- **前置**：工作区有本次改动的 `.rs` 差异（已 add 或有未暂存改动）；已装 Rust 工具链
- **步骤**：
  1. 看 `scripts/test-fast.sh` 的步骤 0.5「CommandError 降级防线」：它取本次改动的新增行（`git diff --cached -U0` 或 `git diff -U0`，外加未跟踪的 `.rs` 文件），逐行扫 `CommandError::from(` 与 `: CommandError = ….into()`，命中即退出码 1。
  2. 跑 `bash scripts/test-fast.sh`，记退出码。
  3. 自查本次改动的新增行里有没有裸用 blanket 降级的写法。
  4. 反向确认防线不是空转：临时在一行上写 `CommandError::from("x")`（不加 `audit-ok`），再跑一次脚本，看它是否拦下；随后删掉这行。
- **判据**：
  1. 【可观测】`bash scripts/test-fast.sh` 退出码为 0，且输出里有「✓ [0.5/N] CommandError 降级防线通过」这一行。
  2. 【可核对】自查命令无命中：`git diff --cached -U0 -- '*.rs' | grep '^+' | grep -v '^+++' | grep -v 'audit-ok' | grep -E 'CommandError::from\(|: CommandError = .*\.into\(\)'` 输出为空（无本次改动的未跟踪 `.rs` 文件时同样为空）。
  3. 【可核对】`grep -n "CommandError 降级防线" -A 25 scripts/test-fast.sh` 命中该防线，且扫描口径含 `CommandError::from(` 与 `: CommandError = .*\.into\(\)` 两种形态。
  4. 【可核对】反向确认：第 4 步临时加的那行**被拦下**（脚本退出码非 0，并打印命中行与修法提示），证明这道防线有牙齿、不是空转。
  5. 【可核对】确实需要 blanket 降级的行**行内带 `audit-ok`** 且行尾写明为什么该是内部故障——`grep -rn "audit-ok" src-tauri/src/` 的每一处命中都能答出理由。
- **证据**：脚本退出码 + 两段终端输出（放行态与拦下态）+ 临时行的 diff
- **失败处置**：本卡判据本身即防线；命中即当场修（改成显式变体：`CommandError::Internal` / `DbError` / `LlmApiError{..}` / `InvalidArgument{..}` 等），或在行内加 `audit-ok` 并写明理由
- **缺口补验**：属 L0 门禁项，不进实机；新增行命中 CommandError::from( 即拦

### ACC-E-05 · 语义性失败不再降级成不可恢复的内部错误

- **SPEC**：3-61
- **层级**：L1+L2（L1 自动化 `src-tauri/src/bot_chat/command_error_mapping_tests.rs`；实机档见下）
- **历史卡**：—（无：全库无实机卡）
- **前置**：应用已启动；机器人总开关已打开；有一张可编辑的任务卡
- **步骤**：
  1. 触发一条任务写冲突：在两个窗口同时改同一张任务卡的标题，让后写的一方撞上基线比对失败（`db_upsert_for` 的 RMW 冲突）。
  2. 让机器人给一条已存在的通知解一条状态非法的请求（`status` 只允许 done / dismissed），观察错误提示与前端可恢复提示。
  3. 把模型条目的 base_url 指到一个不可达的地址（连接失败 / 超时），在聊天里发一条消息。
  4. 每一步都看气泡/提示里的错误文案，以及是否出现「重试」入口。
  5. 在 `<data_dir>/bot.log` 里查本轮是否有 `llm.request_failed` 留痕（第 3 步的网络类失败会走这条审计事件）。
- **判据**：
  1. 【可观测】第 1 步的报错文案是业务语义（形如「任务状态不允许该操作：…」），**不是**「内部错误：…」；同一提示旁有「重试」入口。
  2. 【可观测】第 2 步的报错文案是「参数校验失败：…」形，提示语指向参数而非「反馈日志」。
  3. 【可观测】第 3 步的报错文案是网络/请求类（形如「大模型请求失败：…」），前端提示是「请检查网络与 API 配置后重试」，**不是**「不可自动重试；请反馈日志」。
  4. 【可观测】第 3 步的错误弹窗 / toast 里带机器可读 code（形如 `code=LLM_REQUEST_FAILED`）与 `recoverable=true`；前端的 `LLM_REQUEST_FAILED` 提示是「请检查网络与 API 配置后重试」，**不是** `INTERNAL` 那句「不可自动重试；请反馈日志（含复现步骤）」。
  5. 【可核对】`grep -n "TASK_INVALID_STATE\|INVALID_ARGUMENT\|LLM_REQUEST_FAILED" src-tauri/src/error.rs` 命中三个稳定 code，且三者都在 `is_recoverable()` 里返回 true（`grep -n "fn is_recoverable" -A 35 src-tauri/src/error.rs` 可见 `Self::TaskInvalidState { .. } => true` / `Self::InvalidArgument { .. } => true` / `Self::LlmRequestFailed(_) => true`）；`Self::Internal(_) => false` 那条仍在，说明降级路径没被误当成可恢复。
  6. 【可核对】前端提示按 code 分流：`grep -n "TASK_INVALID_STATE\|INVALID_ARGUMENT\|LLM_REQUEST_FAILED" -A 2 src/lib/errorHandler.ts` 可见三个 case 各给一句可执行提示（任务状态不允许该操作… / 请检查输入参数 / 请检查网络与 API 配置后重试），没有一条回落到 `INTERNAL` 的「不可自动重试；请反馈日志」。
- **证据**：截图（三处错误提示与「重试」入口）+ `<data_dir>/bot.log` 中第 3 步的 `llm.request_failed` 行
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -rnF "llm.request_failed" src-tauri/src` 确认事件名还在源码里；提示仍说「反馈日志」= 变体映射没生效（应用侧缺陷）
- **缺口补验**：TaskInvalidState / InvalidArgument / LlmRequestFailed 精确化未验

### ACC-E-06 · 执行类谎报守卫

- **SPEC**：3-62
- **层级**：L1+L2（L1 自动化 `src-tauri/tests/llm_integration/guard.rs`；实机档见下）
- **历史卡**：S16、M89、M90、overflow H/J（`docs/realmachine-regression-test.md`、`docs/realmachine-smoke-test-2026-10-25.md`、`docs/realmachine-overflow-test.md`）
- **前置**：可编排的 mock 网关可让模型在**零工具调用**的情况下输出指定终稿原话；应用已启动；机器人总开关已打开
- **步骤**：
  1. mock 网关让模型首轮终稿输出：「执行完毕，sleep 120 秒后工具真实返回，打印了 done」，全程不给任何工具调用。
  2. 看这一轮是否被当场打回、模型是否接着去真调工具。
  3. 让模型输出「工具调用结果：」作标题行、下一行才写「- run_python：调用失败」（失败措辞 + 工具名不在同一句），观察是否被拦。
  4. 让模型输出「- create_word：✅ 调用成功」但全程没有 create_word 调用（点名判据）。
  5. 让模型用中文别名说「子 agent 跑完了」，但本次提问从未派过子 agent（中文别名表判据）。
  6. 让模型如实汇报一次失败（形如「- run_python：调用失败」），确认不被误伤。
  7. 让模型在正文里改写/省略工具入参但如实汇报成功，确认不被误伤（入参不参与判决）。
- **判据**：
  1. 【可观测】第 1 步那一轮**没有**把那句谎报交给用户：气泡里看不到「执行完毕…打印了 done」作为最终答复，模型接着调了工具并如实转述真实返回。
  2. 【可审计】`<data_dir>/bot.log` 中出现守卫拦下的 WARN 行（`grep -F "hallucination_guard" "<data_dir>/bot.log" | tail -20`），其 `kind` 为「声称已执行但本次提问零凭证」一类。
  3. 【可观测】第 3 步（失败措辞与工具名不在同一句）**不被拦**：该轮正常结束，不出现打回提示。
  4. 【可观测】第 4 步（点名了没跑过的工具）**被拦**：模型被要求真去调那个工具。
  5. 【可观测】第 5 步（中文别名点名、且没派过子 agent）**被拦**：中文别名表把「子 agent」映射到 `spawn_subagent` 后查不到成功记录，判伪造。
  6. 【可观测】第 6 步（如实汇报失败）**不被拦**：这轮正常结束，不打回。
  7. 【可观测】第 7 步（入参被改写但如实汇报）**不被拦**：这轮正常结束，不打回。
  8. 【可核对】`grep -n "TOOL_ALIASES" -A 6 src-tauri/src/bot/registry/claims.rs` 显示别名表含「子 agent」「子代理」「subagent」→ `spawn_subagent`。
  9. 【可核对】`grep -n "claimed_executed_tools" src-tauri/src/bot/registry/claims.rs` 命中「执行措辞与工具名落在同一句」的点名判据实现；`grep -n "subagent_spawned" src-tauri/src/bot_model_loop/guard.rs` 命中「派过子 agent → 点名判据整档让开」。
- **证据**：截图（每一步的气泡终态与打回提示）+ `<data_dir>/bot.log` 中守卫拦下行
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -rnF "hallucination_guard" src-tauri/src` 确认事件名还在源码里；谎报被原样交给用户 = 守卫没生效（应用侧缺陷），第 6/7 步被误伤 = 触发层过宽（同样是缺陷）
- **缺口补验**：点名判据、中文别名表、子 agent 保守让开、入参不参与判决四项未验

### ACC-E-07 · 易错工具的 schema 里带真实示例

- **SPEC**：3-63
- **层级**：L1+L2（L1 自动化 `src-tauri/src/bot/registry/schemas.rs`；实机档见下）
- **历史卡**：M82、V89（`docs/realmachine-smoke-test-2026-10-25.md`）
- **前置**：应用已启动；机器人总开关已打开；已配模型
- **步骤**：
  1. 在设置页与源码里各查一次这五个工具的 schema 描述：`ui_type`、`create_task`、`fetch_url`、`edit_task`、`complete_task`。
  2. 在聊天里发一条要建任务的请求，看模型首轮给的 `create_task` 参数（尤其 `due` 与 `column`）。
  3. 再发一条要改列的请求，看 `edit_task` 的 `column` 取值。
  4. 再发一条要完成任务的请求，看 `complete_task` 的参数。
  5. 记下首轮参数是否一次就对（没因参数错被拒一次再改）。
- **判据**：
  1. 【可核对】这五个工具的 schema `description` 里各带一段「示例：」：`create_task` 的 `due` 示例写明 `2026-10-24` / `2026-10-24T09:00` 两种合法形态且写明「写 2026-10-24 09:00 或只写 2026-10 都会被拒」；`column` 示例写明只有 todo / doing；`edit_task` 的 `column` 示例写明只有 todo / doing / done；`complete_task` 的 `title` 示例写明「只能完成未完成的卡」；`fetch_url` 的 `url` 示例写明 403/429/404 与非网页文本 Content-Type 会被拒；`ui_type` 的描述写明先 `activate_window` 置前、控件不支持按压时先用 `ui_click` 聚焦。
  2. 【可核对】`grep -n "SCHEMA_UI_TYPE\|SCHEMA_CREATE_TASK\|SCHEMA_FETCH_URL\|SCHEMA_EDIT_TASK\|SCHEMA_COMPLETE_TASK" src-tauri/src/bot/registry/schemas.rs` 命中这五个 schema 常量，且它们都出现在 `TOOLS_TABLE` 里。
  3. 【可观测】第 2 步模型首轮给出的 `create_task` 参数里 `due` 是 `2026-10-24` 或 `2026-10-24T09:00` 形态、`column` 是 todo / doing 之一，没有「2026-10-24 09:00」这类非法写法。
  4. 【可观测】第 3 步模型首轮给出的 `edit_task` 的 `column` 是 todo / doing / done 之一，没有 archived。
  5. 【可观测】第 2 ~ 4 步各只发生一次工具调用即成功，没有「参数被拒 → 模型改一次再调」的重试。
- **证据**：截图（模型首轮参数与工具调用折叠行）+ 五个 schema 常量的源码片段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -rnF "SCHEMA_CREATE_TASK" src-tauri/src` 确认常量还在源码里；示例缺失 = 卡片过期（schema 被改），示例在但首轮参数仍错 = 强模型「无牙齿」，按下方缺口口径记账而非判失败
- **缺口补验**：V89 实测「无牙齿（强模型）」——护栏价值声明需改口径

### ACC-E-08 · 互不依赖的调用合并到同一轮

- **SPEC**：3-64
- **层级**：L2 实机
- **历史卡**：M83、V90（`docs/realmachine-smoke-test-2026-10-25.md`）
- **前置**：应用已启动；机器人总开关已打开；已配模型；工作区里有若干可读文件
- **步骤**：
  1. 发一条要求批量读的请求（如「读一下工作区里这三个文件，各自第一行是什么」）。
  2. 看工具调用折叠行里这几条调用是不是同一轮发出的（`🔧 调用工具 N/M` 的进度没有一进一出地逐个爬）。
  3. 再发一条有依赖的请求（先查 id 再按 id 改），确认它**没有**被合并。
  4. 记下这几轮各发了几次请求。
- **判据**：
  1. 【可观测】第 1 步的多个读调用在**同一轮**发出：工具调用折叠行的 N/M 进度里，这几个工具名同批出现，不是「一进一出」逐个爬。
  2. 【可审计】`<data_dir>/bot.log` 中这一轮只出现一条 `llm.request`（合并成同一轮 = 少一次请求），而不是每个读操作各一条。
  3. 【可观测】第 3 步（有依赖的链）**没有被合并**：先查后改分成两轮发生。
  4. 【可核对】`grep -n "并行发起互不依赖的调用" src-tauri/src/prompts/system.rs` 命中系统提示词里的并行引导条目，其点名范围含批量读文件 / 批量 grep / 批量列目录 / 批量查任务卡 / 批量建卡 / 批量改名。
  5. 【可核对】`grep -n "check_subagent\|run_python" src-tauri/src/prompts/system.rs` 显示这两个工具**不在**并行点名的合并范围内（等子 agent / 看脚本输出是天然串行）。
  6. 【可核对】安全网未松：合并只是「同一轮发」，每个调用仍逐个过工具闸——`grep -n "execute_tool" src-tauri/src/bot/dispatch.rs` 显示执行入口在进程内逐个调用（写类工具的确认弹窗仍是一次一弹）。
- **证据**：截图（工具调用折叠行的 N/M 进度）+ `<data_dir>/bot.log` 中该轮的 `llm.request` 条数
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -rnF "llm.request" src-tauri/src` 确认事件名还在源码里；同一轮多调用照样逐个爬 = 强模型「无牙齿」（引擎本就支持多 tool_calls），按下方缺口口径记账
- **缺口补验**：V90 同为「无牙齿（强模型）」

### ACC-E-09 · 旧轮工具结果可召回

- **SPEC**：3-65
- **层级**：L1+L2（L1 自动化 `src-tauri/src/bot/tools/trace_tests.rs`（8 例）；实机档见下）
- **历史卡**：M76、V78、V79（`docs/realmachine-smoke-test-2026-10-25.md`）
- **前置**：应用已启动；机器人总开关已打开；已配模型；工作区里有一个内容已知的文件
- **步骤**：
  1. 第一轮：让模型读某个文件并把第 42 行原样贴出来（文件内容自己先备好一份备查）。
  2. 第二轮（同一会话，新的一轮）：问「刚才那个文件第 42 行是什么？不要重新读文件」。
  3. 看模型是调 `lookup_receipt` 取回，还是重新读了一遍，还是凭印象编。
  4. 再问一个本会话里从未发生过的 id（`tool_call_id="call_does_not_exist"`），看回执。
  5. 查 `<data_dir>/bot.log` 与痕迹表里的 `lookup_receipt` 调用。
- **判据**：
  1. 【可审计】`<data_dir>/bot.log` 中第二轮出现 `tool.call` 且 `tool=lookup_receipt`（`grep -F "lookup_receipt" "<data_dir>/bot.log" | tail -20`）。
  2. 【可核对】`sqlite3 "<data_dir>/wmessage.db" "select name, args, result from exec_spans where name='lookup_receipt' order by id desc limit 5;"` 里最近一行的 `args` 带 `tool_call_id`（或 `span_id` / `trace_id`），其 `result` **逐字含**第 42 行原文。
  3. 【可观测】模型贴出的第 42 行与备查文件那一行**逐字一致**（不是复述、不是重读、不是编造）。
  4. 【可观测】第 4 步（查不存在的 id）得到明确的「查不到」提示，模型**没有**据此编一段原文出来。
  5. 【可观测】第 2 步模型**没有**重新调 `read_text_file`（若调了，就是没走召回闭环，白花一次工具调用）。
  6. 【可核对】`grep -n "SCHEMA_LOOKUP_RECEIPT" src-tauri/src/bot/registry/schemas.rs` 命中该 schema；其描述写明「不传参数=列出本会话最近的工具调用凭证；传 tool_call_id / span_id / trace_id=调取那一条的完整返回」。
  7. 【可核对】`grep -n "lookup_receipt" src-tauri/src/bot/registry/schemas.rs` 显示它在 `TOOLS_TABLE` 里；`grep -n "fn tool_lookup_receipt" src-tauri/src/bot/tools/trace.rs` 命中实现。
- **证据**：截图（两轮对话与工具调用折叠行）+ `exec_spans` 里 `lookup_receipt` 那一行的 `args`/`result` + `<data_dir>/bot.log` 片段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -rnF "lookup_receipt" src-tauri/src` 确认工具名还在源码里；`exec_spans` 里没有这一行 = 模型没走召回（强模型自愈成多跳回），按下方缺口口径记账
- **缺口补验**：V79 判据被变异报告判为需改（强模型自愈成多跳回）

### ACC-E-10 · 思考段可回灌

- **SPEC**：3-66
- **层级**：L1+L2（L1 自动化 `src-tauri/src/bot_model_loop/tests/sse.rs`；实机档见下）
- **历史卡**：M78、V82、V83（`docs/realmachine-smoke-test-2026-10-25.md`）
- **前置**：已配 DeepSeek / Moonshot / MiniMax 三家中任一家**带思考模式**的模型（OpenAI 兼容协议）；应用已启动
- **步骤**：
  1. 进聊天，发一条需要工具调用的请求（让模型先调一次工具，再据其返回作答）。
  2. 抓这一轮发出的请求体（mock 网关侧记录，或上游请求日志），看 assistant 消息上有没有带 `reasoning_content`。
  3. 再看第二轮（工具调用之后的下一轮）请求体里的最后一条 assistant 消息。
  4. 确认这段思考段是不是**原样**回传（不是摘要、不是丢弃）。
  5. 换一家没吐思考段的模型，跑同样的流程，确认请求体零变化。
- **判据**：
  1. 【可观测】抓到的请求体里，模型吐过思考段的那条 assistant 消息带 `reasoning_content` 字段，内容与上一轮流里那段**逐字相同**。
  2. 【可核对】`grep -n "stamp_last_assistant_reasoning" -A 20 src-tauri/src/bot_model_loop/stream.rs` 命中盖章实现：盖在**消息栈最后一条 assistant 消息**上（`rposition` 找最后一条），更早的 assistant 消息一个字节不动。
  3. 【可核对】`grep -n "reasoning_content" src-tauri/src/bot_model_loop/stream.rs` 命中「已有则不动」的守卫（同一条消息不会被盖两次）。
  4. 【可核对】盖章覆盖三条再发请求的路径：`grep -n "stamp_last_assistant_reasoning" src-tauri/src/bot_model_loop/*.rs` 的调用点覆盖自检返工、幻觉守卫返工、长度续跑这三条路径。
  5. 【可观测】第 5 步（没吐思考段的模型）请求体与上一轮**逐字相同**，没有多出空的 `reasoning_content` 键。
  6. 【可审计】`<data_dir>/bot.log` 中每轮 `llm.request` 的 `reasoning` 字段（`grep -F "llm.request" "<data_dir>/bot.log" | tail -20`）如实描述该模型的思考口径（如 `thinking=budget_tokens=…` / `reasoning_effort=…` / `none`）。
  7. 【可观测】带思考的模型在第二轮**没有**报 400（DeepSeek 口径：带 tools 的请求缺 `reasoning_content` 直接 400）。
- **证据**：抓到的两轮请求体 JSON 片段（`reasoning_content` 字段）+ `<data_dir>/bot.log` 中 `llm.request` 的 `reasoning` 字段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -rnF "llm.request" src-tauri/src` 确认事件名还在源码里；上游根本不吐 `reasoning_content` = 本机不可激活（按下方缺口口径记账），吐了但回传时丢了 = 应用侧缺陷
- **缺口补验**：10-29 改「发了就回」后卡面口径过时；V82/V83 本机不可激活

### ACC-E-11 · 轮内工具结果滑动窗

- **SPEC**：3-67
- **层级**：L1+L2（L1 自动化 `scripts/mutation-trace-funnel.sh`（M1–M12）；实机档见下）
- **历史卡**：M75、V76、V77（`docs/realmachine-smoke-test-2026-10-25.md`）
- **前置**：应用已启动；机器人总开关已打开；已配模型；工作区里有体量大的目录（一次 grep 能出几百行）
- **步骤**：
  1. 发一条重读请求（如「grep 工作区所有 .log 文件并列出每一行」），产出要够大。
  2. 继续追问两三轮，让那些工具结果留在上下文里。
  3. 看 `<data_dir>/bot.log` 里有没有滑动窗生效的记录。
  4. 核对痕迹表里被裁掉那几条的 `result` 列是不是原文。
  5. 让模型复述你前面说过的一句话（用户消息），以及最近一次工具结果的内容。
- **判据**：
  1. 【可审计】`<data_dir>/bot.log` 中出现 `loop.tool_window.applied`（`grep -F "loop.tool_window.applied" "<data_dir>/bot.log" | tail -20`），其 `keep_recent=6`、`chars` > 24000、`trimmed` ≥ 1。
  2. 【可观测】被裁掉的那条工具结果在气泡/上下文里变成了桩子，形状是「⚠️ 已省略以控制上下文。工具 {name} 的完整结果已存档（调用 id={id}），需要时调用 lookup_receipt(tool_call_id="{id}") 调取原文。」
  3. 【可核对】`sqlite3 "<data_dir>/wmessage.db" "select name, length(result) from exec_spans where trace_id=<本轮 trace> order by id;"` 里被裁掉那几条的 `result` **仍是原文**（长度不被改成桩子长度）——只改 prompt，DB 零改动。
  4. 【可观测】模型能准确复述你前面说过的话（用户消息一个字节没被裁）。
  5. 【可观测】模型引用最近 6 条里的工具结果时内容正确，没有把被裁的旧结果当成最近一次的结果用。
  6. 【可核对】`grep -n "TOOL_WINDOW_TRIGGER_CHARS\|TOOL_WINDOW_KEEP_RECENT" src-tauri/src/bot/constants.rs` 显示阈值 24_000 与保留 6；`grep -n "apply_tool_window" -A 20 src-tauri/src/bot_model_loop/context.rs` 显示候选集只含 `role=tool` 且 content 是字符串的消息（带图片的多模态消息不在集合内）。
  7. 【可核对】一次性：`grep -n "tool_window_applied" src-tauri/src/bot_model_loop/request.rs` 显示跨阈值但候选数 ≤ keep_recent 时**不烧掉**唯一一次裁剪机会（`trimmed > 0` 才落位）。
- **证据**：截图（被裁后的桩子形态）+ `<data_dir>/bot.log` 中 `loop.tool_window.applied` 的行 + `exec_spans` 的 `result` 长度对照
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -rnF "loop.tool_window.applied" src-tauri/src` 确认事件名还在源码里；没出现该事件 = 这一轮没超 24,000 字符（换个更重的读再试），出现但库里 `result` 也被改成桩子 = 应用侧缺陷（DB 被动过）
- **缺口补验**：F5「一次性门空消费」已修 fd43e4da，需复验

### ACC-E-12 · 长历史的工具自检走侧调通道

- **SPEC**：3-68
- **层级**：L2 实机
- **历史卡**：M79、V84、V85、V86（`docs/realmachine-smoke-test-2026-10-25.md`）
- **前置**：已配一个可用的侧调模型条目（`selfcheckModelId` 指向模型库里的一个条目）；会话历史足够长（消息栈 ≥ 40,000 字符）
- **步骤**：
  1. 在设置页把侧调模型配好，确认条目厂商已通过连接测试。
  2. 开一个新会话，用若干轮把消息栈堆到 40,000 字符以上（可让模型读大文件并追问）。
  3. 触发一次变更类工具成功（让模型建一张任务卡），工具成功后模型会被问一次「请仅用一个字回答：Y/N」的自检。
  4. 看这条自检是走侧调（小请求）还是占用主循环一整轮。
  5. 把 mock 侧调服务改成可控地失败，再触发一次，观察是否回落 in-loop（自检闸不丢）。
  6. 摘掉侧调模型配置，再触发一次，观察是否留在主循环内判。
- **判据**：
  1. 【可审计】`<data_dir>/bot.log` 中出现 `tool_selfcheck.sidecall`（`grep -F "tool_selfcheck.sidecall" "<data_dir>/bot.log" | tail -20`），其 `chars` ≥ 40000、`verdict` 为 pass / fail。
  2. 【可审计】该 `tool_selfcheck.sidecall` 之后**没有**紧跟一条占主循环整轮的自检请求——主循环的请求数没有因为这次自检 +1（侧调不占主循环轮次）。
  3. 【可观测】第 5 步（侧调失败）出现 `tool_selfcheck.sidecall_failed`，随后主循环照常注入「【自检】」问句并消费判定——**闸不丢**。
  4. 【可观测】第 6 步（没配侧调模型）不出现 `tool_selfcheck.sidecall`，自检留在主循环内问。
  5. 【可核对】`grep -n "SIDE_SELFCHECK_MIN_CHARS" -A 4 src-tauri/src/bot_model_loop/guard.rs` 显示阈值 = 消息预算的 1/5（`CONTEXT_CHARS_HARD_CAP / 5` = 200,000 / 5 = 40,000 字符）。
  6. 【可核对】`grep -n "side_selfcheck_routes_to_sidecall" -A 12 src-tauri/src/bot_model_loop/guard.rs` 显示三条硬条件同时成立才路由：配了侧调模型、上下文够长、拿得到用户意图（`has_intent`）。
  7. 【可核对】`grep -n "SIDE_SELFCHECK_PROMPT" -A 4 src-tauri/src/prompts/selfcheck.rs` 显示侧调 prompt 带「用户要求：{intent}」一段（侧调看不到主对话，不带意图只会答 Y）。
- **证据**：截图（侧调命中时的自检视觉提示）+ `<data_dir>/bot.log` 中 `tool_selfcheck.sidecall` / `tool_selfcheck.sidecall_failed` 的行
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -rnF "tool_selfcheck.sidecall" src-tauri/src` 确认事件名还在源码里；出现该事件但主循环轮数照样 +1 = 侧调没省下那一轮（应用侧缺陷）；数请求分不出谁是谁 = 变异报告已指出的锚点缺陷，改看事件序列而不是数请求
- **缺口补验**：V86 注入面成立但行为面不敏感

### ACC-E-13 · 守卫拦下后的两种处置

- **SPEC**：3-69
- **层级**：L2 实机
- **历史卡**：—（无：全库无实机卡）
- **前置**：可编排的 mock 网关可让模型**连续两轮**输出同一句谎报（每次都在零工具调用的情况下编造一次执行）
- **步骤**：
  1. 让模型首轮终稿谎报一次执行（形如「执行完毕，sleep 120 秒后工具真实返回，打印了 done」）。
  2. 看这一轮是不是被**打回**（模型接着跑了一轮），以及用户最终看到的是不是返工后那一版。
  3. 让模型返工后**仍然**编同样的谎报。
  4. 看第二次拦下后系统怎么处置：有没有再让模型跑一轮、终稿末尾挂了什么。
  5. 在 `<data_dir>/bot.log` 里查守卫拦下的审计记录。
- **判据**：
  1. 【可观测】第 2 步：第一次谎报**没有**原样交给用户——用户最终看到的是返工后那一版；终稿末尾**没有**「⚠️ …没有实际发生」的更正句（第一次处置是打回，不是挂更正）。
  2. 【可观测】第 4 步：第二次谎报**没有**再触发一轮返工（模型没有第三次重跑），终稿末尾**挂了**一条如实更正（形如「上面关于 {工具} 的执行结果是编造的——本次提问和本会话历史里它都没有成功返回过。」）。
  3. 【可观测】终稿**原文保留不删**：那句谎话仍在终稿里，更正是**追加**在末尾，不是替换。
  4. 【可审计】`<data_dir>/bot.log` 中出现 `hallucination_guard.repeat`（`grep -F "hallucination_guard.repeat" "<data_dir>/bot.log" | tail -20`），带 `kind` 与 `detail` 两个字段。
  5. 【可审计】同一轮里 `hallucination_guard.repeat` **只出现一次**（第二次处置不再返工，也不再重复记一条）。
  6. 【可核对】`grep -n "claim_retry_used" src-tauri/src/bot_model_loop/guard.rs` 显示返工机会每次提问只有一次（第一次置位后第二次走 `hallucination_guard.repeat` 分支挂更正）。
  7. 【可核对】反向确认：`grep -n "fabrication_notice" src-tauri/src/bot_model_loop/guard.rs` 显示更正句走的是独立的 `fabrication_notice` 通道（原文不被改写）。
- **证据**：截图（第一次终态 + 第二次终态末尾的更正句）+ `<data_dir>/bot.log` 中 `hallucination_guard.repeat` 的行
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -rnF "hallucination_guard.repeat" src-tauri/src` 确认事件名还在源码里；第二次仍返工 = 返工次数闸没生效（应用侧缺陷），第二次挂更正但把原文删了 = 处置口径错（谎话已流出，删掉只会让用户更信那句假的）
- **缺口补验**：全库无命中：第二次谎报不再返工、改挂如实更正 + hallucination_guard.repeat 审计

### ACC-E-14 · 自检判定字样不是答复

- **SPEC**：3-70
- **层级**：L1+L2（L1 自动化：无；实机档见下）
- **历史卡**：M26（`docs/realmachine-mutation-test-2026-10-23.md`）
- **前置**：可编排的 mock 网关可让模型在**自检问句悬着**时补回一个单字 Y；应用已启动；机器人总开关已打开
- **步骤**：
  1. 先造一个**没问过自检**的回合：发「只回答 Y 或 N」，让模型回一个 `Y`。看用户收到的是不是这个 `Y`。
  2. 再造一个**问过自检**的回合：让模型调一次变更类工具，工具成功后模型被问「请仅用一个字回答：Y/N」；随后让模型用行动代替回答（继续调工具），隔一轮再补回一个 `Y`。
  3. 看这次那个 `Y` 有没有被当成终稿交给用户。
  4. 让模型连续三次都只回判定字样，看追问几次后放弃。
- **判据**：
  1. 【可观测】第 1 步的 `Y` **原样交付给用户**——这是用户明说的合法答复，闸不得劫走（负向判据：没问过自检的回合里，一个 `Y` 就是合法答复）。
  2. 【可观测】第 3 步那个补回的 `Y` **没有**成为终稿：回合继续跑，模型随后交出真实结果。
  3. 【可观测】第 2 步这一轮里能看到自检视觉提示「✅ 自检通过」或「⚠️ 自检未达预期」的流式提示，而不是光秃秃一个 `Y`。
  4. 【可观测】第 4 步：追问额度用尽后，用户收到的是一句可读说明（形如「（模型只回了自检判定「Y」，没有给出可读的结果）」），**不是**一个裸 `Y`。
  5. 【可核对】`grep -n "MAX_LATE_VERDICT_RETRIES" src-tauri/src/bot_model_loop/finalize.rs` 显示追问额度硬顶 = 2。
  6. 【可核对】`grep -n "user_demands_yn_answer" -A 12 src-tauri/src/bot_model_loop/finalize.rs` 显示豁免判据存在：用户最近一条**真实**消息里明确要求 Y/N 时，裸 Y/N 是合法答复，闸不拦。
  7. 【可核对】`grep -n "selfcheck_question_pending" src-tauri/src/bot_model_loop/finalize.rs` 显示出口闸的判据是「自检问句此刻仍悬着」而不是「本提问问过自检」。
- **证据**：截图（第 1 步的裸 `Y` 交付态 + 第 3 步的续跑态 + 第 4 步的兜底说明态）
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -rn "MAX_LATE_VERDICT_RETRIES" src-tauri/src` 确认额度常量还在源码里；第 1 步的裸 `Y` 被劫走 = 作用域判据写错（应用侧缺陷），第 3 步的 `Y` 成了终稿 = 出口闸没生效（应用侧缺陷）
- **缺口补验**：—

### ACC-E-15 · 内容轴的两道判据

- **SPEC**：3-71
- **层级**：L2 实机
- **历史卡**：—（无：全库无实机卡）
- **前置**：可编排的 mock 网关可让模型在工具调用后输出指定终稿；应用已启动
- **步骤**：
  1. 造判据一（结果无出处）：让模型调一次工具（返回某个真实数），随后终稿报一个**别的**数（如工具给 1234.56、终稿报 5050），且带成功态执行措辞与结果措辞。
  2. 造判据一的对照：终稿报工具**真实给过**的数（1234.56），看是否放行。
  3. 造判据二（回声调用）：让模型把某句话原样写进工具参数，工具返回的就是那句话，随后终稿拿它当结论依据。
  4. 造判据二的对照：同一提问里再补一次真读到现场的调用（如 `list_files`），看结论是否放行。
  5. 造让开情形：终稿如实说明「执行没做成（被拦 / 跑不了）」，看是否让开。
- **判据**：
  1. 【可观测】第 1 步**被拦**：模型被要求真去执行或明确写成推算值，不能把 5050 当执行结果报出来。
  2. 【可观测】第 2 步**放行**：报工具真实给过的数不触发拦下。
  3. 【可观测】第 3 步**被拦**：结论无工具支撑（返回内容就是模型自己写进参数的那句），模型被要求换能读到现场的工具重查。
  4. 【可观测】第 4 步**放行**：同一提问里只要有一次读到现场的调用，结论就有依据，不判回声。
  5. 【可观测】第 5 步**放行**：如实说明执行没做成时不触发（那给的是自己的推算，不是编出来的执行结果）。
  6. 【可审计】`<data_dir>/bot.log` 中守卫拦下行（`grep -F "hallucination_guard" "<data_dir>/bot.log" | tail -20`）的 `kind` 分别是「声称的结果无出处: {值}」与「结论无工具支撑」两类。
  7. 【可核对】`grep -n "claimed_result_numbers\|number_sourced" src-tauri/src/bot/registry/claims.rs src-tauri/src/bot_model_loop/guard.rs` 命中判据一的抽取与出处比对；`grep -n "is_echo_call" src-tauri/src/bot/registry/claims.rs` 命中判据二的回声判定。
  8. 【可核对】`grep -n "RESULT_CLAIM_MARKERS" -A 12 src-tauri/src/bot/registry/claims.rs` 显示结果措辞单列一份、不进执行词表（这样「1+1 的结果是 2」这类算术句不触发）。
- **证据**：截图（每一步的气泡终态与打回提示）+ `<data_dir>/bot.log` 中守卫拦下行的 `kind` 字段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -rnF "hallucination_guard" src-tauri/src` 确认事件名还在源码里；第 1/3 步没被拦 = 两道判据没生效（应用侧缺陷），第 2/4/5 步被拦 = 判据过宽（同样是缺陷）
- **缺口补验**：「回声」全库无命中：结果无出处与回声调用两道判据

### ACC-E-16 · 文件声称的落盘凭证闸

- **SPEC**：3-72
- **层级**：L1+L2（L1 自动化 `src-tauri/src/audit/tests.rs`；实机档见下）
- **历史卡**：S16、overflow J（`docs/realmachine-regression-test.md`、`docs/realmachine-overflow-test.md`）
- **前置**：可编排的 mock 网关可让模型输出指定终稿；应用已启动
- **步骤**：
  1. 让模型声称生成了一个**磁盘上不存在**的绝对路径（形如「已生成 /tmp/never_existed_001.docx」）。
  2. 让模型声称生成一个**磁盘上存在、但本次提问没有任何工具写过**的路径（自己在磁盘上先放一个同名文件，再让模型声称它）。
  3. 让模型把「已生成」与路径**分开写**（「已生成」在上一段、路径在下一段），看是否误伤。
  4. 让模型如实说明「这个文件是上一轮生成的」（跨提问重述），看是否误伤。
  5. 让模型如实说明「我用 run_python 写好了文件」（不可证伪的写入通道），看是否让开。
- **判据**：
  1. 【可观测】第 1 步**被拦**：模型被要求真去调用文档生成工具，不能拿一个不存在的路径当产物报出去。
  2. 【可观测】第 2 步**被拦**：路径在磁盘上存在，但没有任何工具写过它——报「这个路径没有落盘凭证」。
  3. 【可观测】第 3 步**不误伤**：触发词与路径不在同一句时不算「声称产出」（模型照着改口提示复述「已生成」而路径在后面另一段，是如实更正）。
  4. 【可观测】第 4 步**不误伤**：跨提问的重述是真话，不判谎报。
  5. 【可观测】第 5 步**让开**：本次提问跑过 `run_python` 这类不可证伪的写入通道时，这道判据不生效（没有凭证推不出「不是工具产物」，宁可漏判不可误伤）。
  6. 【可审计】`<data_dir>/bot.log` 中守卫拦下行的 `kind` 分别是「声称已生成但路径不存在」与「声称已生成但无落盘凭证」。
  7. 【可核对】`grep -n "unreceipted_path\|fake_gen_path" src-tauri/src/bot_model_loop/guard.rs` 命中两道校验（存在性 + 落盘凭证）的两处赋值；`grep -n "unverifiable_ran" src-tauri/src/bot_model_loop/guard.rs` 命中不可证伪通道的让开条件（`run_python` / `mcp_` 前缀 / `spawn_subagent`）。
  8. 【可核对】`grep -n "file_receipts" src-tauri/src/bot_model_loop/shell.rs` 命中凭证查询入口（进程内集合 + 痕迹表 `file_changes` 历史两处来源）。
- **证据**：截图（每一步的气泡终态与打回提示）+ `<data_dir>/bot.log` 中守卫拦下行的 `kind` 字段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -rnF "bot.file_receipts_query_fail" src-tauri/src` 确认凭证查询通道还在源码里；第 2 步没被拦 = 凭证闸没生效（应用侧缺陷），第 3/4/5 步被拦 = 判据过宽（同样是缺陷）
- **缺口补验**：句级触发、路径存在性 + 工具写过两问、不可证伪通道让开未验

### ACC-E-17 · 生成类文档的落盘凭证

- **SPEC**：3-73
- **层级**：L1+L2（L1 自动化 `src-tauri/src/bot/dispatch/commit_and_report_tests.rs`；实机档见下）
- **历史卡**：—（无：全库无实机卡）
- **前置**：应用已启动；机器人总开关已打开；本机依赖齐全（`openpyxl` / `python-docx` / `python-pptx` / `pypdf` / `reportlab` 五个文档硬依赖全绿，见设置页「机器人 → 本机运行环境」那张卡）
- **步骤**：
  1. 分别让模型生成一份 Word（`create_word`）、一份 Excel（`create_excel`）、一份 PPT（`create_ppt`）、一份 PDF（`create_pdf`）；`create_word_revisions` 用带修订要求的 Word 任务触发一次。
  2. 每次生成后打开右侧栏「文件」工作面 → 「改动」子页，看该文件是否在清单里。
  3. 核对痕迹表里该文件的那条变更记录。
  4. 对照一个纯文本产物（`write_file`），看两档凭证的差别。
- **判据**：
  1. 【可观测】五个文档工具生成的产物都出现在右侧栏「改动」子页的清单里（不是只出现在「交付物」子页）。
  2. 【可核对】`sqlite3 "<data_dir>/wmessage.db" "select path, kind, added, deleted, after_sha, diff from file_changes order by id desc limit 10;"` 里这五个产物各有一行，`after_sha` 非空（按字节算的内容哈希），`added` 与 `deleted` **恒为 0**，`diff` 为空（行数两档恒 0，不产行 diff）。
  3. 【可核对】`grep -n "build_binary_file_receipt" src-tauri/src/bot/tools/doc.rs` 命中五个文档工具各自的凭证产出点（Word / 修订 Word / Excel / PPT / PDF 各一处）。
  4. 【可核对】`grep -n "fn build_binary_file_receipt" -A 25 src-tauri/src/bot_fs/receipt.rs` 显示二进制档凭证只带路径 + 内容哈希、不带行 diff（docx/xlsx/pptx 是 zip 容器，按行 diff 只会产乱码）。
  5. 【可核对】对照项：`grep -n "fn build_file_change_receipt" -A 24 src-tauri/src/bot_fs/receipt.rs` 显示写文本工具走的是另一档（有行 diff、行级 added/deleted），与二进制档不同源。
  6. 【可观测】右侧栏里这些产物的详情给的是「二进制文件」形态的说明，不是一屏乱码行 diff。
- **证据**：截图（右侧栏「改动」子页里这五个产物）+ `file_changes` 表的查询输出（path / after_sha / added / deleted / diff 五列）
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -rnF "build_binary_file_receipt" src-tauri/src` 确认函数还在源码里；`file_changes` 里查无这个产物 = 凭证没产（应用侧缺陷）；产物在库里但右侧栏看不到 = 呈现侧缺陷
- **缺口补验**：五个文档生成工具的二进制档落盘凭证未验

### ACC-E-18 · 截图按变更类登记、也给落盘凭证

- **SPEC**：3-74
- **层级**：L1+L2（L1 自动化 `src-tauri/src/bot_desktop.rs`；实机档见下）
- **历史卡**：S16（`docs/realmachine-regression-test.md`）
- **前置**：应用已启动；机器人总开关已打开；macOS 已授予屏幕录制权限（未授权时截出来是壁纸/黑图）
- **步骤**：
  1. 让模型截一次屏（如「截一下当前屏幕」）。
  2. 看右侧栏「文件」工作面 → 「改动」子页里有没有这张 PNG。
  3. 让模型如实说「已截屏」而本轮**没有**真的调 `screenshot`（用 mock 网关造），看是否被拦。
  4. 让模型说「已查看截图内容」（只读动作，不是变更声称），看是否误伤。
  5. 核对痕迹表里 PNG 那条变更记录。
- **判据**：
  1. 【可观测】第 2 步：截图 PNG 出现在右侧栏「改动」子页里（按变更类登记的可见后果）。
  2. 【可核对】`sqlite3 "<data_dir>/wmessage.db" "select path, kind, added, deleted, after_sha, diff from file_changes where path like '%.png' order by id desc limit 5;"` 里该 PNG 有一行，`after_sha` 非空，`added`/`deleted` 恒 0、`diff` 为空（与生成类文档同档：二进制档凭证）。
  3. 【可观测】第 3 步**被拦**：模型不调工具就报「已截屏」会被守卫拦下（变更类声称可见）。
  4. 【可观测】第 4 步**不误伤**：「已查看截图内容」不触发变更声称（词表只收「已截屏 / 已保存截图 / 截图已保存」，不收「已截图」）。
  5. 【可核对】`grep -n "claims_patterns" -A 3 src-tauri/src/bot/registry/schemas.rs` 中 `screenshot` 那条是 `mutating: true` 且 `claims_patterns: &["已截屏", "已保存截图", "截图已保存"]`。
  6. 【可核对】`grep -n "build_binary_file_receipt" src-tauri/src/bot_desktop.rs` 命中截图的二进制档凭证产出点。
- **证据**：截图（右侧栏「改动」子页里的 PNG）+ `file_changes` 表的查询输出 + 第 3/4 步的气泡终态
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -rnF "build_binary_file_receipt" src-tauri/src` 确认函数还在源码里；PNG 进了「改动」页但库里查无凭证行 = 凭证没落库（应用侧缺陷）；库里查得到但右侧栏看不到 = 呈现侧缺陷
- **缺口补验**：—

### ACC-E-19 · 工具成败只由 status 承担

- **SPEC**：3-75
- **层级**：L1+L2（L1 自动化 `src-tauri/src/bot/dispatch/commit_and_report_tests.rs`；实机档见下）
- **历史卡**：S05（`docs/realmachine-regression-test.md`）
- **前置**：应用已启动；机器人总开关已打开；已配模型
- **步骤**：
  1. 造**参数闸**拒绝：让模型给 `read_text_file` 传一个不存在的路径，或给 `create_task` 传一个非法 `column`。
  2. 造**门禁 / 用户拒绝**：让模型调一个白名单外的写盘路径，或在确认弹窗上点「否」。
  3. 造**执行失败**：让模型去读一个能打开但读不出内容的路径，或让 `fetch_url` 抓一个必然失败的地址。
  4. 造一个**设计内的降级**（`Warn` 档）：让模型调 `ask_user` 而不回答（按声明的假设继续）。
  5. 每一步都看工具调用折叠行的成败标记、`<data_dir>/bot.log` 里 `tool.return` 的级别、以及痕迹表 `exec_spans` 的 `ok` 列。
- **判据**：
  1. 【可观测】第 1 步（参数闸拒绝）的工具调用**没动手**（文件夹里没出现任何改动），在折叠行里显为被拒。
  2. 【可观测】第 2 步（门禁 / 用户拒绝）同样显为被拒，且**没有**写入发生。
  3. 【可观测】第 3 步（执行失败）显为失败，与前两步**不同类**（前两步是「没动手」，这一步是「动了没成」）。
  4. 【可观测】第 4 步（`ask_user` 无回答后按假设继续）显为**成功**——`Warn` 算成功，不按失败处理。
  5. 【可审计】`<data_dir>/bot.log` 中 `tool.return` 的级别与 status 对齐：`Ok` → INFO、`Warn` → WARN、`Error` → ERROR、`Denied` → WARN（`grep -F "tool.return" "<data_dir>/bot.log" | tail -20`）。
  6. 【可核对】`sqlite3 "<data_dir>/wmessage.db" "select name, ok, error_class from exec_spans order by id desc limit 10;"` 里 `ok` 列与 status 一致：`Ok` / `Warn` 记 1，`Error` / `Denied` 记 0；`error_class` 只是子类细分（`gate_denied` / `user_denied` / `timeout` / `permission` / `not_found` / `exec_error` 一类），**不承担成败判定**。
  7. 【可核对】`grep -n "pub enum ToolStatus" -A 12 src-tauri/src/bot/registry.rs` 命中四态定义（`Ok` / `Warn` / `Error` / `Denied`）；`grep -n "fn is_success" -A 3 src-tauri/src/bot/registry.rs` 显示 `Warn` 算成功。
  8. 【可核对】`grep -n "fn from_tool_status" -A 10 src-tauri/src/audit.rs` 显示审计级别由 status 派生（`Ok`→INFO / `Warn`→WARN / `Error`→ERROR / `Denied`→WARN），不再对文本做失败判定。
  9. 【可核对】`grep -n "fn classify_tool_failure" -B 6 src-tauri/src/audit.rs` 显示它的注释明写「内置工具的成败判定走 `ToolStatus`，**不再走这里**」——两套口径没有并存的余地。
- **证据**：截图（四步的工具调用折叠行标记）+ `<data_dir>/bot.log` 中 `tool.return` 的级别 + `exec_spans` 的 `ok` 列查询输出
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -rnF "tool.return" src-tauri/src` 确认事件名还在源码里；`exec_spans` 表的 `ok` 列与 `tool.return` 级别对不上 = 两套口径漂移（应用侧缺陷），第 4 步被记成失败 = `Warn` 档归属错（应用侧缺陷）
- **缺口补验**：Denied 与 Error 三类归属未验：参数闸/门禁拒绝/执行失败

### ACC-E-20 · 工具输出回灌上限

- **SPEC**：3-76
- **层级**：L1+L2（L1 自动化 `docs/realmachine-overflow-test.md`（10 场景）+ `scripts/overflow-probe.sh`；实机档见下）
- **历史卡**：S14、S15、overflow A–J、V93（`docs/realmachine-regression-test.md`、`docs/realmachine-overflow-test.md`、`docs/realmachine-smoke-test-2026-10-25.md`）
- **前置**：应用已启动；机器人总开关已打开；已配模型；`scripts/overflow-probe.sh locate` 能定位到 data_dir
- **步骤**：
  1. 先跑 `bash scripts/overflow-probe.sh locate`，确认探针找到的是本轮验收用的那份 data_dir。
  2. 跑 `bash scripts/overflow-probe.sh mark` 打水位线。
  3. 让模型产出一个**超大**工具结果（如让 `run_python` 打印几十万行，或让 `list_files` 列一个超大目录）。
  4. 看模型收到的那一截是什么形状（头尾节选 + 指针），以及 `<data_dir>/overflow/` 下有没有转存文件。
  5. 跑 `bash scripts/overflow-probe.sh report` 汇总水位线之后的事件与溢出文件清单。
  6. 让模型按指针去 `read_text_file` 分页读回转存文件。
- **判据**：
  1. 【可审计】`<data_dir>/bot.log` 中出现 `tool.output.over_budget`（`grep -F "tool.output.over_budget" "<data_dir>/bot.log" | tail -20`），其 `output_bytes` 为实际字节数、`max_bytes` 为生效上限。
  2. 【可审计】紧接着出现 `tool.output.spilled`，带 `tool` / `original` / `cap` / `path` 四个字段，`path` 指向 `<data_dir>/overflow/<YYYYMMDD>/<uuid>.txt`。
  3. 【可观测】模型收到的是「头尾节选 + 指针」：指针文案含「完整内容：{路径}」并说明该文件是完整内容、下方是节选；模型按指针 `read_text_file` 能读回原文。
  4. 【可核对】生效上限 = per-tool 预算与全局 cap **取小**：`grep -n "fn effective_output_cap" -A 12 src-tauri/src/bot_model_loop/output.rs` 显示 `per_tool.min(global_cap_bytes)`；`grep -n "max_output_bytes" src-tauri/src/bot/registry/schemas.rs` 显示默认 8192、`read_text_file` / `fetch_url` / `list_files` 给到 65536。
  5. 【可核对】`grep -n "TOOL_OUTPUT_BYTES" src-tauri/src/bot/constants.rs` 显示全局 cap 是常量 128KB（当前口径：设置页的 `maxToolOutputBytes` 键已随参数常量化退役，全局 cap 取常量）。
  6. 【可核对】切点落在 code point 边界：`grep -n "fn byte_head_end\|fn byte_tail_start" src-tauri/src/bot/format.rs` 命中按字节切且不切残 UTF-8 字符的实现。
  7. 【可核对】转存失败有兜底：`grep -n "tool.output.spill_failed" src-tauri/src/bot_model_loop/output.rs` 命中落盘失败后退回纯头尾截断的分支（不落空、不放行原文）。
- **证据**：截图（模型收到的那一截与指针）+ `<data_dir>/bot.log` 中 `tool.output.over_budget` / `tool.output.spilled` 的行 + `overflow-probe.sh report` 的输出 + 转存文件的头尾各一截
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -rnF "tool.output.spilled" src-tauri/src` 确认事件名还在源码里；`over_budget` 报了但 `spilled` 没报 = 截断发生处与审计口径分家（应用侧缺陷），指针给的路径不存在 = 转存兜底没生效（应用侧缺陷）
- **缺口补验**：覆盖最厚的一块，仍需按新构建复跑

### ACC-E-21 · read_text_file 读取面补两道闸

- **SPEC**：3-77
- **层级**：L1+L2（L1 自动化 `src-tauri/src/bot_fs/tests.rs`（`read_slice_*`）；实机档见下）
- **历史卡**：—（无：全库无实机卡）
- **前置**：应用已启动；机器人总开关已打开；已配模型；工作区里备好两个测试文件（一个含超长单行，一个中文多行）
- **步骤**：
  1. 造**超长单行**文件（单行 > 2000 字符，如 minified JSON），让模型读它并把内容贴出来。
  2. 造**中文多行**文件（每行约 60 汉字、总量大于 56KB），让模型读它。
  3. 看工具结果里那一行是否带截断标注、以及页尾是否带字节预算提示。
  4. 让模型数一下刚才那页有多少行。
  5. 再造一个**全空行**文件（行数 > 2000），让模型读它，看行数硬顶。
- **判据**：
  1. 【可观测】第 1 步：超长单行被截断到 2000 字符，并带标注「…[本行超 2000 字符已截断]」——**标注必在**，不是静默截断。
  2. 【可观测】第 2 步：工具结果页尾带「；本页达 56KB 字节预算」，且页尾另带「…（截断，继续读请用 offset={n}」提示。
  3. 【可观测】第 4 步：中文页的行数明显少于 500（默认页按渲染字节累计到 56KB 收页，中文页自动收窄），**不是**固定 500 行。
  4. 【可观测】第 5 步：全空行文件的页只到 2000 行（行数硬顶），不是把整个文件读完。
  5. 【可审计】`<data_dir>/bot.log` 中本轮出现 `bot_fs.read`（`grep -F "bot_fs.read" "<data_dir>/bot.log" | tail -20`），带 `path` / `offset` / `end` / `total` 四个字段（`end` 是本页末行号、`total` 是文件总行数）。
  6. 【可核对】`grep -n "READ_MAX_LINE_CHARS\|READ_PAGE_TARGET_BYTES\|READ_MAX_LINES" src-tauri/src/bot/constants.rs` 显示三个常量分别是 2000、56KB（`56 * 1024`）、2000。
  7. 【可核对】`grep -n "fn render_read_slice" -A 30 src-tauri/src/bot_fs/read.rs` 命中两道闸的实现：单行按**字符**数截断（整字保留不切残字），页按**渲染字节**累计到 56KB 收页，且首行必放行。
- **证据**：截图（工具结果里的截断标注与页尾提示）+ `<data_dir>/bot.log` 中 `bot_fs.read` 的 `end` / `total` 字段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -rnF "bot_fs.read" src-tauri/src` 确认事件名还在源码里；超长行原样穿过（无标注）= 单行闸没生效（应用侧缺陷），中文页仍是 500 行 = 字节预算闸没生效（应用侧缺陷）
- **缺口补验**：10-29 新增：单行 2000 字符截断闸、默认页 56KB 收页未验

### ACC-E-22 · 工具清单 65 个

- **SPEC**：3-78
- **层级**：L1+L2（L1 自动化 `src-tauri/src/bot/registry/registry_tests.rs`；实机档见下）
- **历史卡**：M01–M08、M11–M13、M82、M86、S16、S25（`docs/realmachine-mutation-test-2026-10-23.md`、`docs/realmachine-smoke-test-2026-10-25.md`、`docs/realmachine-regression-test.md`）
- **前置**：应用已启动；机器人总开关已打开；已配模型
- **步骤**：
  1. 数一遍注册表里的工具条目数（源码侧与模型实际收到的清单各数一遍）。
  2. 把这份清单与架构事实表逐条对拍，看有没有哪条工具在事实表里没被点名。
  3. 在聊天里让模型列出它这次拿到的工具清单里与「任务 CRUD / 文档生成 / 文件编辑 / 沙箱 / 联网 / 桌面自动化 / 技能 / 计划」无关的冷门工具各叫什么。
  4. 抽查三个工具的实际可用性（`get_current_time`、`list_reminders`、`update_plan`）。
- **判据**：
  1. 【可核对】`grep -c 'name: "' src-tauri/src/bot/registry/schemas.rs` 输出 **65**——`TOOLS_TABLE` 实列 65 条，本卡以 `TOOLS_TABLE` 为准。
  2. 【可核对】`grep -n "pub static TOOLS_TABLE" -A 3 src-tauri/src/bot/registry/schemas.rs` 命中该表；`grep -n "65" docs/TOOL-EXECUTION-FACT-SHEET-2026-10-30.md` 命中事实表自述的 65 条口径。
  3. 【可核对】事实表逐条对拍：`docs/TOOL-EXECUTION-FACT-SHEET-2026-10-30.md` 里**每个**工具名都被点名——实测 `complete_task` 漏了（事实表实点 64 个），本卡把它列为已知缺口（见下方缺口补验）。
  4. 【可观测】模型这一轮拿到的工具清单条数 = 65（模型自己列出的数量与注册表一致）。
  5. 【可观测】第 4 步抽查的三个工具都可用：`get_current_time` 返回当前时间、`list_reminders` 返回提醒清单（空列表也是正常返回）、`update_plan` 在聊天窗右上角浮层里显示步骤。
  6. 【可核对】分类对照：清单按 SPEC 附录 A 的口径覆盖任务 CRUD/子任务/绑定、文档生成（Word/修订/Excel/PPT/PDF）、文件编辑（`edit_file`/`write_file`）、文件读写+grep、`run_python` 沙箱、`ast_inspect`、联网（`web_search`/`fetch_url`）、`ocr_image`、截图、电脑辅助五件、桌面 UI 自动化六件、时间、记忆工具、`use_skill`、远程技能安装与检索、`ask_user`、`update_plan`。
- **证据**：截图（模型列出的工具清单）+ 注册表条目数与事实表对拍的终端输出
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -rnF "TOOLS_TABLE" src-tauri/src` 确认工具表还在源码里；条数 ≠ 65 = 事实表与注册表分家（先核注册表，再改文档），注册表本身条数变了 = 应用侧改动需同步 SPEC 附录 A
- **缺口补验**：事实表漏 complete_task（实列 64 vs 实际 65）；逐工具行为覆盖不均，9/10 任务卡工具本体无直接单测
