# ACC-T · 工具与技能（L2 实机卡）

> 覆盖 `SPEC.md`「3. 机器人聊天与模型」里工具/技能向的 12 条功能（3-79 ~ 3-90），对应矩阵 `docs/acceptance/matrix.csv` 的 D3c 域。
> 判据写法与证据规范见 `docs/ACCEPTANCE-PLAN-2026-10-30.md`；审计事件名取自 `docs/acceptance/event-names.md` 白名单。
> 数据目录 `<data_dir>`：macOS 默认为 `~/Library/Application Support/com.renshi.wmessage/`（便携模式随 exe 走）。产物目录是 `<data_dir>/AI_Gen_Files`。
> 日志检索统一写法：`grep -F "<事件名>" "<data_dir>/bot.log" | tail -20`（<data_dir>/bot.log 与 <data_dir>/error.log 同目录，WARN/ERROR 同步镜像到后者）。

### ACC-T-01 · 执行计划浮层

- **SPEC**：3-79
- **层级**：L1+L2（自动档 `src-tauri/src/bot/tools/plan.rs`、`src-tauri/src/bot/plan_state.rs`、`src/components/ChatPanel/PlanPanel.test.tsx`）
- **历史卡**：—（无：10-26 新增，全库无命中）
- **前置**：机器人总开关已开；已配可用模型；聊天页已至少发过一轮消息
- **步骤**：
  1. 聊天窗右上角点「执行计划」开关 → 浮层展开；记下系统窗口列表里本应用的窗口数。
  2. 浮层里从上到下读三段标题。
  3. 发一条多步请求（如「先读 A.txt，再据此写 B.txt，最后自查一遍」），等机器人调完 `update_plan`。
  4. 看「计划」段的步数、每步图标与文字样式；确认已完成那步有删除线。
  5. 再发一轮**不涉及改计划**的请求，等它跑完 → 看「计划」段底部有没有过期提示。
  6. 让机器人改一个文件 → 看「环境信息」段给的是概览还是清单。
  7. 点「在右侧栏看改动清单 →」→ 确认清单落在右侧栏文件页的「改动」子页。
  8. 关掉浮层开关再打开 → 看计划清单是否还在。
- **判据**：
  1. 【可观测】浮层是**主窗口内的浮层**：出现在聊天区右上角下方，右侧栏宽度不变；系统窗口列表里本应用窗口数与步骤 1 相同（不新增独立 OS 窗口）。
  2. 【可观测】浮层自上而下三段，标题依次为「计划」「后台任务」「环境信息」，三段各自成块、不合并成一段。
  3. 【可观测】步骤 3 前「计划」段显示「本会话暂无计划（模型调用 update_plan 后显示）」；建计划后该段显示「N 步」计数并逐条列出步骤文本。
  4. 【可观测】步骤 4 三态各有不同图标：`completed` 是勾号且文字带删除线；`in_progress` 是旋转指示器且文字为高亮色；`pending` 是空心圈。
  5. 【可审计】<data_dir>/bot.log 中出现 `bot-plan`（检索命令：`grep -F "bot-plan" "<data_dir>/bot.log" | tail -20`），且该行载荷顶层带会话号——信封缺会话号时前端按会话过滤会把它整条丢掉，浮层只在重挂载拉快照时才更新。
  6. 【可核对】`grep -n "只允许一条正在做" src-tauri/src/bot/plan_state.rs` 命中——「任一时刻至多一条 in_progress」是后端硬校验，不是模型自觉；`grep -n "rejects_bad_args_without_leaving_state" src-tauri/src/bot/tools/plan.rs` 命中——入参不合法不留半份状态。
  7. 【可观测】步骤 5：本轮开始后模型没再更新计划时，「计划」段出现「本轮尚未更新，状态可能已过期」；模型更新后该提示消失。
  8. 【可核对】`grep -n "turnStartedAt" src/components/ChatPanel/usePlanState.ts` 命中过期判定（`updatedAt < turnStartedAt`）。
  9. 【可观测】步骤 6：「环境信息」段只给**概览**——文件数 + 「累计 +N / -M 行」一行，**不列文件清单**；清单入口是「在右侧栏看改动清单 →」一行跳转，点了落到右侧栏文件页的「改动」子页。
  10. 【可核对】`grep -c "rusqlite" src-tauri/src/bot/plan_state.rs` 返回 0 —— 计划状态存进程内会话表、**不入库**（`static PLANS: OnceLock<Mutex<HashMap<..>>>`）。
  11. 【可观测】步骤 8：关掉再打开浮层，计划清单仍在——面板挂载时用 `bot_plan_state` 拉快照，不靠事件回放。
  12. 【可核对】`grep -n "bot_plan_state" src-tauri/src/lib.rs` 命中命令注册；`grep -n "applyEvent\|applySnapshot" src/components/ChatPanel/planStore.ts` 命中「事件=变化流、命令=状态源」两条写入口，且两条都按 `updatedAt` 比新旧（旧的不许盖新的）。
- **证据**：截图（浮层三段 + 三态图标 + 过期提示 + 环境信息概览）+ <data_dir>/bot.log 片段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "bot-plan" src-tauri/src` 确认事件名还在源码里（`bot-plan` grep 不到 = 卡片过期；grep 得到但机器上无事件 = 应用侧回归）
- **缺口补验**：10-26 新增，全库无命中：update_plan 整表替换、bot-plan 事件、面板三段与过期标记未验

### ACC-T-02 · Word 模板双层口径

- **SPEC**：3-80
- **层级**：L1+L2（自动档 `src-tauri/src/py/document/word_template_tests.rs`、`src-tauri/src/py/document/word_template_params_tests.rs`）
- **历史卡**：—（无：全库无实机卡）
- **前置**：设置页可进文档分区；准备一个排好版式的 .docx（含正文段、主标题、一/二/三级标题各若干）
- **步骤**：
  1. 设置页 → Word 模板面板 → 上传上面那个 .docx（模板名取文件名）。
  2. 打开该模板的参数编辑面板，看排版参数分了哪几组。
  3. 改一个数值（例如正文字号），保存 → 重新打开参数面板看是否回填改动后的值。
  4. 把模板名改掉并保存 → 回到模板列表看名字已变。
  5. 在磁盘上看模板目录：`ls "<data_dir>/word_templates/"`。
  6. 让机器人用该模板生成一份 Word，拿到产物路径。
  7. 只删旁车 json（保留 .docx），再让机器人用同一模板生成一次。
  8. 把该模板设为默认；再把默认指向另一份模板（或删掉所有模板），生成一次。
- **判据**：
  1. 【可观测】上传后面板里新增一行模板，**上传即完成参数提取**（面板能直接打开排版参数，不需要手动触发提取）。
  2. 【可观测】参数面板按「主标题 / 正文 / 一级标题 / 二级标题 / 三级标题」五组分块，每组含字体、字号、加粗、对齐、行距、首行缩进、段前段后。
  3. 【可核对】步骤 5：模板目录里成对出现 `<模板名>.docx` 与 `<模板名>.json`——**.docx 管纸张/页边距/页眉页脚（排版容器），旁车 json 管文字排版参数**。
  4. 【可观测】步骤 3：改完保存再打开，参数面板回填的是改动后的值（不是回落到提取值）。
  5. 【可核对】步骤 4：改名后磁盘上 `<旧名>.docx` / `<旧名>.json` 都不再存在，`<新名>.docx` / `<新名>.json` 存在（`grep -n "word_template_params_update_in" src-tauri/src/py/document.rs` 命中改名连带 docx / 旁车 json / 默认标记三处）。
  6. 【可观测】步骤 7：旁车缺失时生成**不报错**，参数自动重新提取并落回旁车（面板上模板参数区又能打开）。
  7. 【可核对】`grep -n "params: harvested" src-tauri/src/py/document.rs` 命中「旁车缺失→现场重提取」分支；`grep -n "word_template_reextract" src-tauri/src/py/document.rs` 命中面板上的「重新提取」入口。
  8. 【可观测】步骤 8：默认模板缺失时生成走内置版式（产物仍生成成功，不因缺模板而失败）。
  9. 【可核对】`grep -n "fn word_template_resolve" src-tauri/src/py/document.rs` 命中——显式名字优先、否则默认模板、都没有则 None（走内置版式）。
  10. 【可核对】`grep -n "模板必须是 .docx 文件" src-tauri/src/py/document.rs` 命中——非 .docx 文件在导入口就被拒，不进目录。
  11. 【可核对】生成那次 <data_dir>/bot.log 里有一条 `doc_make_word` 行（检索命令：`grep -F "doc_make_word" "<data_dir>/bot.log" | tail -20`），其 `tpl:` 字段是本次用的模板名、`params:` 字段是 `saved`（取存量旁车）或 `harvested`（现场重提取）——双层口径哪层生效看这行日志。
- **证据**：截图（模板列表 + 五组排版参数面板 + 改名后的列表）+ 模板目录 `ls` 输出 + 生成产物路径
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "word_template_params_update" src-tauri/src` 确认命令名还在源码里（四个命令名 `word_template_list` / `word_template_import` / `word_template_params_update` / `word_template_reextract` 都 grep 得到才算名还在）
- **缺口补验**：无实机卡：模板管纸张页边、旁车 json 管排版、缺旁车自动重提取

### ACC-T-03 · 语义记忆体 v2

- **SPEC**：3-81
- **层级**：L1+L2（自动档 `src-tauri/src/memory/tests.rs`、`src-tauri/tests/memory_eval.rs`、`cargo test --lib memory::embed -- --ignored`）
- **历史卡**：M80、M81、M91（`docs/realmachine-mutation-test-2026-10-24.md`、`docs/realmachine-smoke-test-2026-10-25.md`）
- **前置**：应用已启动；机器人总开关已开；已配可用模型；设置页记忆区可进；记忆库里已有 ≥20 条记忆（含 ≥2 条 lesson）；仓库根有 `bge-small-zh-v1.5/` 模型目录（嵌入引擎冒烟前提）
- **步骤**：
  1. 设置页 → 记忆分区 → 看「记忆库」卡顶部的统计行。
  2. 展开「检索参数（高级）」卡，数输入框个数并记下每个占位符里的默认值。
  3. 把「相关记忆条数」改成 2，保存 → 重新打开面板看是否回填 2。
  4. 把全部输入框清空 → 保存 → 重新打开看是否回落成默认值。
  5. 记一条事实：「验收时先开日志再看界面」；随后用近义句再记一条，看记忆库行数与内容。
  6. 在记忆库列表里改写一条已有记忆的内容，看行数与类型列。
  7. 点「导出记忆」存一份 JSON；再点「导入记忆」导回同一份，看行数变化。
  8. 跑黄金集评估：`cd src-tauri && cargo test --test memory_eval eval_recall_report -- --ignored --nocapture`，抄下总榜 recall@5。
- **判据**：
  1. 【可观测】统计行形如「N/500 条 · 向量覆盖 M/N」——容量上限 500 是默认值，向量覆盖数 ≤ 总数（覆盖低 = 部分条目走关键词检索）。
  2. 【可观测】检索参数卡共 **8 个**输入框：注入字符预算（默认 4000）、相关记忆条数（5）、近期摘要条数（3）、经验教训条数（3）、库容量上限（500）、新近衰减天数（30）、去重合并阈值（0.92）、冲突提示阈值（0.75）。
  3. 【可观测】步骤 3：改值保存后再打开面板回填新值；下一条消息起生效。
  4. 【可观测】步骤 4：全空保存 = 回落默认（不把参数抹成 0）。
  5. 【可观测】步骤 5：近义的两条**不新增行**（行数不增），那条的内容是后写的一句——cos≥0.92 合并即替换。
  6. 【可核对】`grep -n "DEDUP_MERGE_COSINE\|DEDUP_HINT_COSINE" src-tauri/src/memory/store.rs` 命中 0.92 / 0.75；`grep -n "UPDATE mem_items SET content = ?1, tags = ?2, importance = ?3, source = ?4" src-tauri/src/memory/store.rs` 与 `grep -n "kind = ?5" src-tauri/src/memory/store.rs` 命中——合并是**整行覆盖** content/tags/importance/source/kind。
  7. 【可观测】步骤 6：改写一条记忆后行数不增，且该行的「类型」列与新内容自洽（不会出现内容是 lesson、类型仍是 fact 的行）。
  8. 【可观测】步骤 7：导出的 JSON 含语义向量；导回同一份后总行数不增（导入走语义去重，只增不删）。
  9. 【可观测】整理（定时或手动）产出的条目**不是 lesson**——「经验教训」只由四条渠道沉淀：模型主动记（`tool_record_lesson`）、任务失败、技能失败、用户纠正聚簇；整理只产合并 / 提炼 / 解决矛盾三种 ops。
  10. 【可核对】`grep -n "enum ConsolidateOp" -A 20 src-tauri/src/memory/consolidate.rs` 命中三种 op：合并 / 解决矛盾 / 提炼。
  11. 【可核对】`grep -n "bge-small-zh-v1.5" src-tauri/src/memory/embed.rs` 命中本地嵌入模型；`grep -n "fn hybrid_score_with" src-tauri/src/memory/rank.rs` 命中混合打分（关键词 + 向量 + 新近度衰减）。
  12. 【可核对】步骤 8：`cargo test --test memory_eval eval_recall_report -- --ignored --nocapture` 输出「[总榜] recall@1 = … recall@5 = …」，且 recall@5 > 0（评估器自检：0 说明嵌入→播种→检索链路整体坏了）；`grep -n "debug_assert_eq!(params.top_n, 5)" src-tauri/tests/memory_eval.rs` 命中——recall@5 的「5」与默认 topN 锁死一致。
  13. 【可审计】自动抽取落库时 <data_dir>/bot.log 中出现 `memory.extract_adjudicated`（检索命令：`grep -F "memory.extract_adjudicated" "<data_dir>/bot.log" | tail -20`）；抽取失败出现 `memory.extract_failed`——抽取失败不静默，该轮放弃也要留痕。
  14. 【可核对】`grep -n "fn maybe_extract_from_session" -A 12 src-tauri/src/memory/extract.rs` 命中——自动抽取按会话收尾触发，且受抽取限频约束。
- **证据**：截图（统计行 + 8 项参数卡 + 合并前后对比）+ 导出 JSON 文件 + 评估器总榜输出
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "memory.consolidate" src-tauri/src` 确认事件名还在源码里（`memory.consolidate` / `memory.consolidate_now` / `memory.consolidate_failed` 三个都 grep 得到才算名还在）
- **缺口补验**：自动抽取、去重阈值、记忆整理、检索参数 8 项、黄金集 recall@5 未验

### ACC-T-04 · MCP 外部服务器

- **SPEC**：3-82
- **层级**：L1+L2（自动档 `src-tauri/src/bot/mcp/config.rs`、`src-tauri/src/bot/mcp/manager/e2e_stdio.rs`）
- **历史卡**：S03、S04、S10、S13、S17、S18、M34、M50、M51、M86、W04（`docs/realmachine-regression-test.md`、`docs/realmachine-mutation-test-2026-10-24.md`、`docs/realmachine-smoke-test-2026-10-25.md`）
- **前置**：设置页可进 MCP 服务分区；准备一台 stdio 服务器（如 `npx -y @modelcontextprotocol/server-everything`）与一台 HTTP 服务器地址；`ps` / `lsof` 可用
- **步骤**：
  1. 新增一台 stdio 服务器，填命令与参数，**不填任何机密** → 保存。
  2. 打开该服务器的「启用」拨杆 → 立刻查进程：`ps aux | grep -c "<该服务器命令>"`。
  3. 展开该服务器的进程资源区（pid / 内存 / CPU / 已运行时长 / 监听端口），记下第一列数字。
  4. 停 6 秒再看这一列数字（应已变化），并确认「监听端口」那一行有内容还是空。
  5. 在这台服务器上填一个机密（env 或 header），保存 → 重新打开编辑框看它显示成什么。
  6. 把编辑框里那个机密值**原样不动**保存 → 回到钥匙串/配置文件核对值没被清掉。
  7. 把编辑框里那个机密值改写 → 保存 → 核对磁盘上/钥匙串里是新值。
  8. 把编辑框里那个机密**整行删掉** → 保存 → 核对该项已消失。
  9. 改这台服务器的 env 值（改 API token）并保存 → 看状态点是否变化、是否重连。
  10. 新增一台 HTTP 服务器，URL 分别试 `http://127.0.0.1:8080/mcp`、`http://192.168.1.10/mcp`、`http://example.com/mcp`，逐个保存。
  11. 让机器人在聊天里真正调用一次这台 stdio 服务器的工具 → 再查 `ps`。
  12. 让机器人调用一台故意配坏的服务器，再让它再调一次 → 看面板状态点与冷却行为。
  13. 看列表里那条内置 Playwright 浏览器条目：有没有删除按钮、启用拨杆初始是什么态。
- **判据**：
  1. 【可观测】步骤 2：**打开开关不启动任何子进程**——`ps` 里查不到该服务器命令的进程；面板状态点是蓝色「静默待命」（tooltip：已启用但没有子进程在跑，大模型真正调用时才拉起，用完即关）。
  2. 【可观测】步骤 11：调用发生后 `ps` 里出现该进程（调用期拉起）；调用结束后进程消失（调用完立刻回收，不跨调用存活）。
  3. 【可观测】步骤 12：配置不可用的服务器状态点转红「已断开/失败」并带错误 tooltip；冷却期内面板照常显示该错误，不反复重连（发现失败进 10 分钟冷却）。
  4. 【可核对】`grep -n "DISCOVERY_RETRY_COOLDOWN_MS" src-tauri/src/bot/mcp/manager.rs` 命中 10 分钟冷却；`grep -n "clear_discovery_cooldown" src-tauri/src/bot/mcp/manager.rs` 命中——用户手动开关 / 改配置 / 点「工具」即解除冷却。
  5. 【可观测】步骤 5：编辑框里机密显示为**掩码**（键名照常回填，值是掩码），拿不到明文。
  6. 【可核对】`grep -n "fn mcp_server_secrets_masked" src-tauri/src/bot/mcp/commands.rs` 命中脱敏视图命令；步骤 6/7/8 三种编辑语义分别对应「原样保存=保留 / 改写=替换 / 删行=删除」，任一都不该把钥匙串里已有的机密静默清空（旧口径「空 = 清空」是已知坑）。
  7. 【可审计】<data_dir>/bot.log 中出现 `mcp.server_saved`（检索命令：`grep -F "mcp.server_saved" "<data_dir>/bot.log" | tail -20`）；删服务器出现 `mcp.server_deleted`，拨开关出现 `mcp.server_toggled`。
  8. 【可审计】工具清单发现成功出现 `mcp.tools_discovered`，发现失败出现 `mcp.discover_failed`，连接断开出现 `mcp.disconnected`。
  9. 【可观测】步骤 9：改了 env 里的 API token 后该服务器**重连**（指纹认 env/headers）——状态点重新走一遍「待发现 → 静默待命」。
  10. 【可核对】`grep -n "pub fn fingerprint" -A 15 src-tauri/src/bot/mcp/manager.rs` 命中 env / headers 参与哈希——**不用序列化值算指纹**（机密字段被剥掉，改了不换指纹 = 不重连 = 旧凭据继续用）。
  11. 【可观测】步骤 10：`127.0.0.1` 与 `192.168.1.10` 两种内网地址**在保存口就被拒**（给出「仅允许公网」类原因）；公网地址能保存。
  12. 【可核对】`grep -n "check_public_url" src-tauri/src/bot/mcp/manager.rs` 命中 DNS 级闸（域名解析出的每个 IP 都必须公网，判据单源在 `bot_web::check_public_url`）；`grep -n "resolve_to_addrs" src-tauri/src/bot/mcp/manager.rs` 命中——解析结果钉给 reqwest，校验与请求之间不再二次解析（DNS TOCTOU 窗口关闭）。
  13. 【可核对】`grep -n "redirect::Policy::none" src-tauri/src/bot/mcp/manager.rs` 命中——禁跟随重定向（公网端点 302 到内网即穿透「仅公网」闸）。
  14. 【可核对】`grep -n "MAX_STDOUT_LINE" src-tauri/src/bot/mcp/stdio_transport.rs` 命中 1MB——stdio 读侧每行最多取 1MB，超出部分丢弃且**会话不断**。
  15. 【可观测】步骤 3/4：进程资源区的 pid / 内存 / CPU / 已运行时长随时间变化（5 秒轮询，采样层同一条 5s TTL 缓存），并有一行「此进程由 MCP 客户端按需拉起，不在本地服务表中」。
  16. 【可核对】`grep -n "listeningSupported\|resourceSupported" src/components/SettingsPage/McpPanel.tsx` 命中——平台不支持时面板明说「本平台暂不支持进程资源采集 / 监听端口采集」，**不把采空显示成「无监听端口 / 无资源占用」**。
  17. 【可核对】`grep -n "mcp-tool-cache.json" src-tauri/src/bot/mcp/tool_cache.rs` 命中——工具清单发现一次即落盘，之后**没有连接也能把工具挂进主 agent 清单**（否则模型看不见它、永远不会去调）。
  18. 【可观测】步骤 13：内置 Playwright 条目在列表里，**没有删除按钮**（只有启用拨杆），初始是**停用**态——内置条目只能开关、不可删。
  19. 【可核对】`grep -n "fn is_builtin_id" -A 4 src-tauri/src/bot/mcp/builtin.rs` 命中内置 id 判据；`grep -n "enabled: false" src-tauri/src/bot/mcp/builtin.rs` 命中内置条目的默认停用态；`grep -n "{!s.builtin" src/components/SettingsPage/McpPanel.tsx` 命中「内置条目隐藏删除按钮」的前端守卫。
- **证据**：截图（状态点三态 + 脱敏视图 + 进程资源区 + 仅公网拒绝提示）+ 两次 `ps` 输出 + <data_dir>/bot.log 片段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "mcp.server_saved" src-tauri/src` 确认事件名还在源码里（`mcp.server_saved` / `mcp.server_deleted` / `mcp.server_toggled` / `mcp.tools_discovered` / `mcp.disconnected` / `mcp.discover_failed` 六个都 grep 得到才算名还在）
- **缺口补验**：覆盖面最广的一块

### ACC-T-05 · MCP 工具函数名跨断连稳定

- **SPEC**：3-83
- **层级**：L1+L2（自动档：全库无专项单测，靠 `src-tauri/src/bot/mcp/mount.rs` 的登记簿单测 + 实机）
- **历史卡**：S10（`docs/realmachine-regression-test.md`）
- **前置**：设置页可进 MCP 服务分区；两台**净化后前缀相同**的服务器（如分别命名 `my-tool` 与 `my.tool`，两者净化后同为 `my_tool`），两台各有同名工具 `read`
- **步骤**：
  1. 两台服务器都打开启用开关，各点一次「查看工具」，记下工具清单里的函数名。
  2. 在聊天里让模型调一次第二台服务器的 `read`（记下模型实际写的函数名）。
  3. 手动关掉第一台服务器的开关（让它断连），等状态点转灰/转红。
  4. 回到聊天，**不刷新模型上下文**（同一轮或紧邻一轮），让模型再调一次同一个工具名。
  5. 看模型写的函数名有没有变，以及最终落到哪台服务器。
  6. 把第一台重新打开，回到聊天让模型再调一次同一个工具名。
- **判据**：
  1. 【可观测】步骤 1：两台服务器的工具在模型清单里分别长成 `mcp_my_tool_read` 与 `mcp_my_tool_read_2`（同前缀同工具名靠 `_2` 消歧）。
  2. 【可观测】步骤 3/4：第一台断连后，模型清单里**第二台的函数名不变**（仍是 `mcp_my_tool_read_2`）——序号不因「当前在场顺序」前移。
  3. 【可观测】步骤 4：模型按旧名调用时**仍能路由到第二台**（不是「未知工具」），服务器当前离线时按名字定位到它并懒重连。
  4. 【可核对】`grep -n "pub struct NameRegistry" -A 40 src-tauri/src/bot/mcp/mount.rs` 命中进程级登记簿；`grep -n "fn owner" src-tauri/src/bot/mcp/mount.rs` 命中——dispatch 反查同走登记簿。
  5. 【可核对】`grep -n "已分配过" src-tauri/src/bot/mcp/mount.rs` 命中——名字一经分配即**占用**、不再释放给别人（服务器改名后旧名仍归它）。
  6. 【可核对】`grep -n "disambiguation_is_deterministic_for_sorted_input" src-tauri/src/bot/mcp/mount.rs` 命中——A 离开后 B 的 `_2` 序号不漂移的单测在源码里。
  7. 【可核对】`grep -n "disambiguation_suffix_colliding_with_real_tool_name_stays_unique" src-tauri/src/bot/mcp/mount.rs` 命中——消歧后缀撞到真实工具名时继续递增（不产出重名）。
- **证据**：截图（两次工具清单的函数名对比）+ 聊天里模型两次发出的工具名 + <data_dir>/bot.log 片段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "NameRegistry" src-tauri/src` 确认登记簿还在源码里；grep 得到但序号漂了 = 实现回归（这是静默错调，只能实机发现）
- **缺口补验**：—

### ACC-T-06 · 技能系统

- **SPEC**：3-84
- **层级**：L1+L2（自动档 `src-tauri/src/bot_skills/runtime/tests.rs`、`src-tauri/src/intent_router.rs`）
- **历史卡**：S01、S02、S06、M23、M24、M25、M47、M84（`docs/realmachine-regression-test.md`、`docs/realmachine-mutation-test-2026-10-24.md`、`docs/realmachine-smoke-test-2026-10-25.md`）
- **前置**：设置页可进机器人技能分区；已装 ≥2 个用户技能，其中一个 frontmatter 声明了 `intents`，另一个名字带 `-`
- **步骤**：
  1. 设置页 → 机器人技能分区 → 看列表每行的徽章与右端控件。
  2. 聊天里发「用 `<带连字符的技能名>` 做个东西」，看模型先读了哪个技能。
  3. 聊天里发一句只命中另一个技能 `intents` 正则的话，看模型读了哪个技能。
  4. 聊天里同时点名 A 技能、又说一句命中 B 技能正则的话，看模型读哪个。
  5. 聊天里用不同大小写写同一个技能名（如正文写大写、清单里小写），看是否仍命中。
  6. 聊天里让机器人「装一个技能」，给一个 GitHub 仓库地址（`owner/repo#技能名`）、一个 SkillHub 技能页地址、一个 zip 直链，逐个试。
  7. 安装确认窗弹出时先点「拒绝」，看目录里是否留下半份技能。
  8. 装成功后回设置页看技能列表是否新增，并看该技能是否出现在下一轮的系统提示词清单里。
- **判据**：
  1. 【可观测】步骤 1：技能列表每行有状态徽章（运行结果），最右是拨杆开关（按钮后不挂文字解释，解释走悬停 tooltip）。
  2. 【可观测】步骤 2/3/4：点名优先于 intent 正则——步骤 4 里模型读的是**被点名的那个**技能。
  3. 【可观测】步骤 5：大小写不敏感——正文写大写仍命中清单里的小写技能名。
  4. 【可核对】`grep -n "fn explicit_name_pattern" -A 25 src-tauri/src/intent_router.rs` 命中——点名只对带 `-` / `_` 的名字生效（单词名撞普通词会劫持路由），且要求名字前后是非名字字符。
  5. 【可核对】`grep -n "点名优先于 intent 正则" src-tauri/src/intent_router.rs` 命中——点名规则排在 intent 规则之前。
  6. 【可核对】`grep -n "fn tool_install_skill" src-tauri/src/bot_skills/remote.rs` 命中——`install_skill` 支持 GitHub / SkillHub / zip 三种来源；`grep -n "SKILLHUB_API" src-tauri/src/bot_skills/remote.rs` 命中——`search_skills` 走 SkillHub 公开搜索 API 免鉴权。
  7. 【可观测】步骤 7：点「拒绝」后**不留半份目录**（安装失败回滚），技能列表不新增。
  8. 【可核对】`grep -n "install_skill_dir" src-tauri/src/bot_skills/remote.rs` 命中——装完即入「已安装技能」清单并重建意图路由。
  9. 【可审计】<data_dir>/bot.log 中出现 `skill.start`（检索命令：`grep -F "skill.start" "<data_dir>/bot.log" | tail -20`），载荷含技能名、风险等级、运行模式、步数上限、回滚策略。
  10. 【可审计】预路由命中技能时出现 `pre_step.route_skill`（载荷含技能名与模式）；路由失败出现 `pre_step.route_failed`——**路由失败不阻断聊天**，放行模型。
  11. 【可核对】`grep -n "fn resolve_skill_name" src-tauri/src/bot_skills/remote.rs` 命中——frontmatter 的 name 不合法（中文/符号）时改用来源里的 slug；`grep -n "fn validate_skill_name" src-tauri/src/bot_skills/manage.rs` 命中——技能名只留 ASCII 字母数字 + `-` / `_`。
  12. 【可核对】`grep -n "zip slip\|软链\|zip bomb" src-tauri/src/bot_skills/remote.rs` 命中——解包复用归档限额闸，落盘前拦下越界解包。
- **证据**：截图（技能列表徽章 + 安装确认窗 + 拒绝后的列表）+ 三轮聊天里模型读技能的截图 + <data_dir>/bot.log 片段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "skill.start" src-tauri/src` 确认事件名还在源码里（`skill.start` / `pre_step.route_skill` / `pre_step.route_failed` 三个都 grep 得到才算名还在）
- **缺口补验**：—

### ACC-T-07 · 技能配额

- **SPEC**：3-85
- **层级**：L1+L2（自动档 `src-tauri/src/bot_skills/runtime/tests.rs`、`src-tauri/src/bot_skills/parse.rs`）
- **历史卡**：—（无：10-26/10-29 改口径，全库无命中）
- **前置**：能读改技能目录的 SKILL.md；准备两个技能，一个 `mode: auto`、一个 `mode: interactive`
- **步骤**：
  1. 在一个技能的 frontmatter 里写 `timeout_secs: 30`，重启应用，看启动输出（终端 stderr）与该技能是否仍按秒数被掐断。
  2. 在 `mode: auto` 的技能里写 `max_steps: 3`，让机器人跑这个技能（超过 3 步），看是否被熔断。
  3. 在 `mode: interactive` 的技能里写 `max_steps: 2`，让机器人跑这个技能（超过 2 步），看是否被熔断。
  4. 在一个技能里写 `max_steps: 9999`，看它被钳到多少。
  5. 在一个技能里写 `max_rounds: 10`，跑一轮，看这轮最多几轮结束。
  6. 在一个技能里写 `max_rounds: 9999`，跑一轮，看是否突破安全网。
  7. 看模型把技能预算跑满前是否收到过收尾提醒。
- **判据**：
  1. 【可观测】步骤 1：技能**无墙钟**——`timeout_secs` 不再生效，技能不会在 30 秒被掐断。
  2. 【可核对】`grep -n "timeout_secs 已废弃" src-tauri/src/bot_skills/parse.rs` 命中——解析时忽略并告警一次；`grep -n "SKILL_MAX_LIFETIME_SECS" src-tauri/src/bot/constants.rs` 命中 3600 秒绝对寿命上限（含 Paused 等待时间，作 Paused 槽位泄漏保护，不是性能预算）。
  3. 【可观测】步骤 2：`auto` 模式技能超过声明步数被熔断，结束时原因写明「超过最大步数上限（N 步）」。
  4. 【可观测】步骤 3：`interactive` 模式**不设步数闸**——超过声明的 2 步仍继续跑（不被「超过最大步数上限」终止）。
  5. 【可核对】`grep -n "SKILL_DEFAULT_MAX_STEPS\|SKILL_MAX_STEPS_LIMIT" src-tauri/src/bot/constants.rs` 命中 100 / 200——默认 100、声明上限 200。
  6. 【可核对】步骤 4：`max_steps: 9999` 被钳到 200（`grep -n "clamp(1, MAX_STEPS_LIMIT)" src-tauri/src/bot_skills/parse.rs`）。
  7. 【可观测】步骤 5：`max_rounds: 10` 时这一轮最多跑 10 轮就结束（轮数上限被往下钳）。
  8. 【可核对】`grep -n "fn resolve_max_rounds" -A 6 src-tauri/src/bot_model_loop/shell.rs` 命中 `safety_net.min(n)`——`max_rounds` 只许往下钳，不得突破安全网；`grep -n "MAX_ROUNDS" src-tauri/src/bot/constants.rs` 命中 200。
  9. 【可观测】步骤 7：步数余量不足时模型收到收尾提醒（软警告，不中断）。
  10. 【可核对】`grep -n "skill_budget_warn_limits" -A 6 src-tauri/src/bot_skills/runtime.rs` 命中——软警告阈值按总步数比例算并钳到上下界。
  11. 【可审计】技能启动时 <data_dir>/bot.log 中出现 `skill.start`（检索命令：`grep -F "skill.start" "<data_dir>/bot.log" | tail -20`），载荷里的 `max_steps` 字段是**生效值**——frontmatter 声明 9999 时这里应显示 200（被钳到上限），声明 3 时显示 3。
  12. 【可观测】模型一个字都没说就结束时，收到技能出口兜底：技能名 + 结束原因 + 产物文件路径。
- **证据**：截图（技能跑完后的结束原因行 + 收尾提醒）+ 启动 stderr 输出 + <data_dir>/bot.log 片段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "SKILL_MAX_STEPS_LIMIT" src-tauri/src` 确认配额常量还在源码里（三个常量 `SKILL_DEFAULT_MAX_STEPS` / `SKILL_MAX_STEPS_LIMIT` / `SKILL_MAX_LIFETIME_SECS` 都 grep 得到才算名还在）
- **缺口补验**：10-26/10-29 改口径：无墙钟无独立预算、max_steps 仅 auto 生效未验

### ACC-T-08 · 技能执行中断的两条根因与三处修法

- **SPEC**：3-86
- **层级**：L1+L2（自动档 `src-tauri/src/bot_skills/runtime/tests.rs`）
- **历史卡**：—（无：10-26 新增，全库无命中）
- **前置**：准备一个 `mode: interactive` 的技能，步骤里会连续调一个必然失败的工具（如 `read_text_file` 一个不存在的路径）
- **步骤**：
  1. 让机器人跑这个 interactive 技能，让它连续失败若干次 → 看技能是第几次失败后终止的。
  2. 在第 1 次失败后，让模型改用另一条能成功的路 → 看失败计数是否清零。
  3. 让机器人跑一个正文很长的技能（SKILL.md 大于 20KB），看模型收到的技能正文是不是完整的（有没有「…节选…」截断）。
  4. 让机器人用 `grep_files` 搜一个**具体文件**（传该文件的绝对路径作 `dir`），看是否被拒。
  5. 用 `grep_files` 传一个目录路径再搜一次，看目录形态是否照常工作。
  6. 看 `grep_files` 的工具描述里有没有写清「目录 / 单文件」两种形态。
- **判据**：
  1. 【可观测】步骤 1：interactive 模式**连续失败 3 次**才判死——前两次失败技能仍在跑，模型有机会换策略。
  2. 【可核对】`grep -n "SKILL_INTERACTIVE_FAIL_LIMIT" src-tauri/src/bot_skills/runtime.rs` 命中 3；`grep -n "fn should_fail_skill_on_step_error" -A 4 src-tauri/src/bot_skills/runtime.rs` 命中——`auto` 一步失败即判死（回滚依赖 `advance_dsl → advance_skill → Failed` 感知失败），`interactive` 连续 3 次才判死。
  3. 【可审计】<data_dir>/bot.log 中出现 `skill_step_fail`，详情区分「暂不判死」与「判死」两种措辞（审计事件名不新增，仍是 `skill_step_fail`）。
  4. 【可观测】步骤 2：中间成功一次后计数清零——之后重新从第 1 次失败算起，不会「攒够 3 次就死」。
  5. 【可观测】步骤 3：模型收到的技能正文**不被全局输出 cap 压制**——长技能的正文整段送达，不出现「…节选…」把中段流程章节截掉。
  6. 【可核对】`grep -n "DOCUMENT_OUTPUT_TOOLS" -A 3 src-tauri/src/bot_model_loop/output.rs` 命中 `use_skill`——文档类输出按自己的 per-tool 预算走，**不与全局 cap 取小**；`grep -n "max_output_bytes: 65536" src-tauri/src/bot/registry/schemas.rs` 命中——per-tool 预算从 8192 抬到 65536。
  7. 【可观测】步骤 4：`grep_files` 的 `dir` 传**单个文件的绝对路径**时不再 Deny，只搜那一个文件并返回命中行。
  8. 【可核对】`grep -n "enum GrepTarget" -A 8 src-tauri/src/bot_fs/read.rs` 命中——目录 / 单文件两种形态；`grep -n "fn grep_target" -A 8 src-tauri/src/bot_fs/read.rs` 命中按路径类型定搜法。
  9. 【可观测】步骤 5：`dir` 传目录时递归 walk 照常工作（与单文件形态互不干扰）。
  10. 【可核对】`grep -n "SCHEMA_GREP_FILES" src-tauri/src/bot/registry/schemas.rs` 命中——工具描述同步说清两种形态。
- **证据**：截图（连续失败后的技能状态行 + 长技能正文全文出现在对话里 + 单文件 grep 命中）+ <data_dir>/bot.log 片段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "skill_step_fail" src-tauri/src` 确认事件名还在源码里（grep 不到 = 卡片过期；grep 得到但机器上无 = 应用侧回归）
- **缺口补验**：10-26 新增：失败语义按 mode 分岔、use_skill 正文不受全局 cap 压制、grep_files 兼容单文件

### ACC-T-09 · PPT 制作中断的四个根因与四处修法

- **SPEC**：3-87
- **层级**：L1+L2（自动档 `src-tauri/src/bot_py/tests.rs`）
- **历史卡**：—（无：10-26 新增，全库无命中）
- **前置**：设置页已开 Python 编程；准备一个白名单外的文件路径（触发文件访问授权窗）；聊天窗保持在前台可见
- **步骤**：
  1. 让机器人读一个**白名单外**的文件 → 弹文件访问授权窗 → 点「允许」→ 看技能有没有被终止。
  2. 让机器人执行一个危险动作（触发 danger 确认窗）→ 确认后看技能状态。
  3. 让机器人跑一段会失败的 Python（`raise ValueError("x")`）→ 看工具结果是失败还是成功。
  4. 看这次失败后的报错文案里有没有「脚本已保留在 ……」与「只改出错的那几行」提示。
  5. 按提示里的路径用 `read_text_file` 读回那个脚本 → 看能不能读回。
  6. 让机器人用 `scriptPath` 指向那个落盘脚本重跑（不重写代码）→ 看是否执行了同一份文件。
  7. 让机器人改脚本里出错那一行后再用 `scriptPath` 重跑 → 看是否跑通。
  8. 查临时目录：`ls "$(getconf DARWIN_USER_TEMP_DIR)wmessage-py-runs/"`（macOS）看失败那次是否留了目录。
  9. 跑一个成功的 Python → 再查同一目录 → 看成功那次的目录是否已被清掉。
- **判据**：
  1. 【可观测】步骤 1：**只有 danger 暂停技能**——文件访问授权窗（file_access）弹出并确认后，技能**继续运行**，界面不出现「技能已终止：暂停即终止」。
  2. 【可核对】`grep -n "if kind == CONFIRM_KIND_DANGER" src-tauri/src/bot_slash.rs` 命中——暂停技能的调用点被条件包住，只有危险确认才暂停。
  3. 【可核对】`grep -n "pending_confirm_id" src-tauri/src/bot_skills/runtime.rs` 命中——确认结果与暂停按 confirm id 一一绑定（认错的暂停被误恢复/误终止比不恢复更糟）。
  4. 【可观测】步骤 3：脚本 `raise` 时工具结果为**失败**（不是成功）——退出码非 0 如实报工具失败。
  5. 【可核对】`grep -n "fn run_failed" src-tauri/src/bot/tools/doc.rs` 命中；`grep -n "脚本退出码" src-tauri/src/bot/tools/doc.rs` 命中失败文案形态。
  6. 【可观测】步骤 4：失败文案里带「脚本已保留在 <目录>/<脚本名>，可 read_text_file 读回后只改出错的那几行，再用 run_python 的 scriptPath 重跑」。
  7. 【可观测】步骤 5：该落盘脚本能原样读回（沙箱根目录恒在读白名单），且读回内容与写出时一致。
  8. 【可核对】`grep -n "scriptPath" src-tauri/src/bot/registry/schemas.rs` 命中——`run_python` 支持 `scriptPath`（已落盘脚本优先于 `code`），且工具描述写明「出错只改出错那一行重跑，不必整段重写」。
  9. 【可观测】步骤 8：失败那次在 `wmessage-py-runs/` 下留有运行目录（跑完即删的旧行为已改掉）；步骤 9：成功那次的目录已被清掉（留存只发生在失败路径）。
  10. 【可核对】`grep -n "fn finish_run_dir" -A 12 src-tauri/src/py/runtime.rs` 命中——只有退出码 0 才清目录；`grep -n "retained_note" src-tauri/src/py/runtime.rs` 命中失败提示带留存目录。
  11. 【可核对】`grep -n "SCRIPT_FILE" src-tauri/src/py/runtime.rs` 命中——脚本正文与执行入口分文件（看门狗不拼进正文，报错行号与模型看到的文件一致）。
  12. 【可核对】`grep -n "emitted_any" src-tauri/src/bot_model_loop/sse.rs` 命中——流读取失败可重发本轮的判据是「有没有向前端吐过内容」，上限 1 次；`grep -n "fn skill_finish" src-tauri/src/bot_skills/runtime.rs` 命中放弃重试时的收尾路径（收尾与重试只在一处做）。
  13. 【可审计】脚本失败那次 <data_dir>/bot.log 中出现 `tool.return`（检索命令：`grep -F "tool.return" "<data_dir>/bot.log" | tail -20`）且工具名是 `run_python`——工具失败配平落账，调用数与返回数对得上（配平缺失 = 统计面板出现「悬挂调用」）。
- **证据**：截图（授权窗确认后技能继续 + 失败报错文案 + scriptPath 重跑成功）+ `wmessage-py-runs/` 目录两次 `ls` 输出 + <data_dir>/bot.log 片段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "emitted_any" src-tauri/src` 确认重发判据还在源码里（`emitted_any` / `pending_confirm_id` / `scriptPath` 三处任一 grep 不到 = 卡片过期）
- **缺口补验**：10-26 新增：授权窗误杀、悬空 run、scriptPath 落盘、运行目录留存四条修法未验

### ACC-T-10 · 内置技能已退役

- **SPEC**：3-88
- **层级**：L2 实机（**负向卡**：判据是「观察不到」；退役项复活即缺陷）
- **历史卡**：M48（`docs/realmachine-smoke-test-2026-10-25.md`）
- **前置**：能看数据目录；能找到内置技能物化目录（`<data_dir>/builtin_skills/`）
- **步骤**：
  1. 设置页 → 机器人技能分区 → 通读技能列表，找有没有 `pptx-design`。
  2. 在数据目录里列出内置技能物化目录：`ls "<data_dir>/builtin_skills/"`。
  3. 手动造一个「退役残留」：在 `<data_dir>/builtin_skills/` 下建一个子目录 `zzz-old-skill/`，里面放一份 `SKILL.md`。
  4. 重启应用，等启动完成。
  5. 再列一次物化目录；再看设置页技能列表里有没有 `zzz-old-skill`。
  6. 查启动审计：`grep -F "builtin_skill_retired" "<data_dir>/bot.log" | tail -20`。
  7. 确认用户技能目录没被误删：列出 `<data_dir>/skills/`（或用户导入技能所在目录），确认用户技能还在。
- **判据**：
  1. 【可观测】步骤 1：技能列表里**没有** `pptx-design`——随包分发的内置技能清单当前为空。
  2. 【可核对】`grep -n "pub const BUILTIN_SKILLS" -A 3 src-tauri/src/bot_skills/builtin.rs` 命中清单为空数组。
  3. 【可观测】步骤 3/4/5：启动后 `zzz-old-skill` 这个不在清单里的子目录**被删掉**（物化目录里不再有它），技能列表里也不出现它——不是僵尸技能。
  4. 【可审计】步骤 6：<data_dir>/bot.log 中出现 `builtin_skill_retired`（检索命令：`grep -F "builtin_skill_retired" "<data_dir>/bot.log" | tail -20`），载荷带被清的技能名；清理逐项审计，不是静默批量删。
  5. 【可核对】`grep -n "builtin_skill_purge_failed" src-tauri/src/bot_skills/builtin.rs` 命中——某项删不掉时逐项记 `builtin_skill_purge_failed` 且**不阻断启动**（残留顶多在列表里多一项，用户仍可手动删）。
  6. 【可观测】步骤 7：用户技能目录里的技能**一个没少**——清理只动 `builtin_skills/`，不碰同父的用户 `skills/` 目录。
  7. 【可核对】`grep -n "fn asset_dir_matches_registration_list" src-tauri/src/bot_skills/builtin.rs` 命中——资产目录与登记清单的一致性由单测守住（只放资产不登记 = 哑资产，永远不会被物化）。
- **证据**：截图（技能列表里无 `pptx-design`）+ 物化目录两次 `ls` 输出 + <data_dir>/bot.log 片段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "builtin_skill_retired" src-tauri/src` 确认事件名还在源码里；**本卡是负向卡**：看到 `pptx-design` 出现在列表里就是缺陷，不是卡片过期
- **缺口补验**：负向卡：随包分发技能清单须为空、旧物化副本启动时清掉

### ACC-T-11 · 技能启用开关

- **SPEC**：3-89
- **层级**：L1+L2（自动档 `src-tauri/src/bot_skills/manage/tests.rs`）
- **历史卡**：M47（`docs/realmachine-smoke-test-2026-10-25.md`）
- **前置**：设置页可进机器人技能分区；已装 ≥2 个技能
- **步骤**：
  1. 关掉技能 A 的拨杆 → 看该行是否整行置灰。
  2. 打开 A 的 `SKILL.md`，看 frontmatter 里是不是多了 `enabled: false`。
  3. 聊天里发一句本该命中 A 的 `intents` 正则的话 → 看模型有没有被路由进 A。
  4. 让模型直接 `use_skill` A → 看报什么。
  5. 看下一轮的系统提示词里还有没有 A（清单里被广告出去的名字）。
  6. 把 A 的开关打开 → 看 `SKILL.md` 的 `enabled` 是否变回 true、整行是否恢复。
  7. 若同名技能在多个目录都有副本（用户目录 + 内置物化目录），关掉后看**哪一份**被改了。
  8. 手改 A 的 `SKILL.md`（把 `enabled` 改成 false）→ 重启应用 → 看列表里的拨杆状态。
- **判据**：
  1. 【可观测】步骤 1：拨杆关掉后该技能整行**置灰**；拨杆是按钮形态，按钮后不挂文字解释（解释走鼠标悬停 tooltip）。
  2. 【可核对】步骤 2：开关状态写进技能**自身** `SKILL.md` 的 frontmatter `enabled` 键——单一事实源，不另开状态表。
  3. 【可核对】`grep -n "fn skills_set_enabled" -A 20 src-tauri/src/bot_skills/manage.rs` 命中命令；`grep -n "fn write_enabled_to" -A 15 src-tauri/src/bot_skills/manage.rs` 命中——**原子写**（同目录 tmp + rename），且写完必须能被解析器读回目标值，读不回就不写。
  4. 【可观测】步骤 3：关掉后用户消息**不被劫持**进 A（意图路由不产出该技能的路由条目）。
  5. 【可观测】步骤 4：模型显式 `use_skill` A 被拒，报「技能「A」已被禁用」。
  6. 【可核对】`grep -n "fn preflight" -A 6 src-tauri/src/bot_skills/runtime.rs` 命中——启动预审以 frontmatter 的 `enabled` 为准。
  7. 【可观测】步骤 5：下一轮系统提示词的技能清单里**没有** A（清单过滤，技能不再被广告给模型）。
  8. 【可核对】`grep -n "fn build_skill_block" -A 12 src-tauri/src/bot_skills/manage.rs` 命中——三道闸（清单过滤 / 意图路由 / 启动预审）都以同一个 `enabled` 键为准。
  9. 【可观测】步骤 6：开关打开后 `SKILL.md` 里 `enabled` 变回 true，整行恢复；`grep -n "fn rebuild_intent_routes" src-tauri/src/bot_skills/manage.rs` 命中——改开关后立刻重建路由表（否则关掉的技能仍留在进程级路由缓存里）。
  10. 【可核对】步骤 7：`grep -n "fn effective_skill_md" -A 8 src-tauri/src/bot_skills/manage.rs` 命中——落盘目标是**生效副本**（按 `skill_search_paths` 优先级取第一份存在的 SKILL.md），否则「关的是 A、生效的是 B」。
  11. 【可观测】步骤 8：手改 `enabled: false` 后重启，列表里该技能的拨杆显示为关——frontmatter 是唯一事实源，外部手改同样生效。
  12. 【可审计】<data_dir>/bot.log 中出现 `skill.enabled_changed`（检索命令：`grep -F "skill.enabled_changed" "<data_dir>/bot.log" | tail -20`），载荷含技能名与目标开关值。
- **证据**：截图（置灰行 + tooltip + SKILL.md 内容）+ <data_dir>/bot.log 片段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "skill.enabled_changed" src-tauri/src` 确认事件名还在源码里（`skill.enabled_changed` grep 不到 = 卡片过期）
- **缺口补验**：—

### ACC-T-12 · 产物统一落 AI_Gen_Files

- **SPEC**：3-90
- **层级**：L1+L2（自动档 `src-tauri/src/bot_artifacts.rs`）
- **历史卡**：M86、M92（`docs/realmachine-smoke-test-2026-10-25.md`）
- **前置**：应用已启动；机器人总开关已开；已配可用模型；聊天窗右侧栏可展开
- **步骤**：
  1. 先列产物目录：`ls "<data_dir>/AI_Gen_Files/"`（首次启动应已存在）。
  2. 让机器人生成一份 Word、一份 Excel、一份 PDF、一份 PPT，各指定同名（如 `周报`）。
  3. 再列产物目录，看同名产物是否变成 `周报 (1).docx` 这类形态。
  4. 再生成一次同名产物，再列目录，看序号继续递增而不是覆盖。
  5. 对比生成前后的文件**内容**：旧文件的修改时间应不变（没被覆盖）。
  6. 点右侧栏文件页的「打开产物文件夹（AI_Gen_Files）」→ 看系统文件管理器打开的是不是同一个目录。
  7. 让机器人跑一个会产出文件的 Python 脚本 → 看产物是不是也落在同一目录。
- **判据**：
  1. 【可核对】`grep -n "fn gen_dir" -A 8 src-tauri/src/db/paths.rs` 命中——产物目录唯一入口是 `<data_dir>/AI_Gen_Files`，**解析即建**（`create_dir_all`），启动时预建（`grep -n "gen_dir(app.handle())" src-tauri/src/lib.rs` 命中启动预建调用）。
  2. 【可观测】步骤 1：应用刚启动完，产物目录就已存在（不用先生成任何东西）。
  3. 【可观测】步骤 2/3：同名产物落成 `周报.docx`、`周报 (1).docx`、`周报 (2).docx` —— **同名加 `(n)`，永不覆盖**。
  4. 【可核对】`grep -n "fn gen_out_path_in" -A 18 src-tauri/src/py/document.rs` 命中——命名循环从 `(1)` 起递增，`while candidate.exists()` 保证不覆盖既有文件。
  5. 【可核对】步骤 5：`ls -l --time-style=full-iso`（或 macOS `ls -lT`）看第一个 `周报.docx` 的修改时间在后续两次生成后**没有变**——覆盖即缺陷。
  6. 【可观测】步骤 6：点「打开产物文件夹（AI_Gen_Files）」打开的系统目录与 `<data_dir>/AI_Gen_Files` 是同一个（便携模式下前端拼不出来路径，由后端 `bot_open_gen_dir` 解析后回传）。
  7. 【可核对】`grep -n "fn bot_open_gen_dir" -A 10 src-tauri/src/bot_skills/files.rs` 命中——路径由后端解析，前端不自己拼。
  8. 【可观测】步骤 7：Python 运行目录里的产物被搬进同一个产物目录（`grep -n "fn harvest_run_outputs" -A 10 src-tauri/src/py/harvest.rs` 命中搬运动作），搬不进时如实报错不静默丢。
  9. 【可核对】`grep -n "fn register" -A 12 src-tauri/src/bot_artifacts.rs` 命中——产物登记表按 task_id + path 去重登记；`grep -n "fn should_emit" -A 15 src-tauri/src/bot_artifacts.rs` 命中——流程结束按 `TaskExecOrigin` 分流决定是否出汇总（手动执行要任务卡已 done 才出，定时/批量不论 column 都出，intermediate 不参与）。
  10. 【可观测】跑一个带最终产物的任务卡流程 → 通知中心出现一张「产物绑定」卡（内嵌复选列表，默认全选），勾选后点「绑定选中（N）」→ 产物绑到该任务卡上；点「跳过」则不绑。
  11. 【可核对】生成产物那次 <data_dir>/bot.log 里有一条 `doc_make_word` 行，其 `out:` 字段指向 `<data_dir>/AI_Gen_Files/...`——产物路径由后端解析并回写日志，前端不自己拼（`grep -n "doc_make_word{tpl_note} | out" src-tauri/src/py/document.rs` 命中该行写法）。
  12. 【可审计】生成产物走的是工具通道：<data_dir>/bot.log 中本轮出现 `tool.call` 与配平的 `tool.return`（检索命令：`grep -F "tool.call" "<data_dir>/bot.log" | tail -20`），工具名是 `create_word` / `create_excel` / `create_pdf` / `create_ppt` 之一。
- **证据**：截图（右侧栏「打开产物文件夹」入口 + 产物汇总弹窗）+ 产物目录三次 `ls` 输出 + 文件修改时间对比
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "bot_open_gen_dir" src-tauri/src` 确认命令名还在源码里（`bot_open_gen_dir` 与 `gen_out_path_in` 都 grep 得到才算名还在）
- **缺口补验**：同名加 (n) 永不覆盖、启动预建、产物登记表弹窗未验
### ACC-P-11 · run_python 断网与目的地闸

- **SPEC**：6-116
- **层级**：L1+L2（自动档 `src-tauri/src/bot_web/destination.rs`、`src-tauri/src/py/seatbelt.rs`）
- **前置**：应用已启动；机器人总开关已开；已配可用模型；macOS（seatbelt 平台）；本机可出网
- **步骤**：
  1. 【自动】`cd src-tauri && cargo test --lib bot_web::destination py::seatbelt` 全绿——白名单三形态匹配 / IDN 归一 / 产品出口免闸 / 子 agent 与无人值守直接拒 / 会话授权生命周期 / sandbox 内 socket 必 EPERM。
  2. 设置页「机器人」分区找到「联网工具目的地白名单」，填入 `api.github.com` 保存。
  3. 让机器人调 `http_request` GET `https://api.github.com/zen`（白名单内）→ 应直接放行无弹窗。
  4. 让机器人调 `http_request` GET 一个白名单外地址（如 `https://example.com`）→ 应弹确认窗；点批准；**同会话再请求一次同域 → 不再弹**。
  5. 新建会话重复步骤 4 的同域请求 → 应重新弹窗（会话授权不跨会话）。
  6. 让机器人调 `http_request` 带 `"Authorization": "keyring:<已配厂商名>"` 调一次带鉴权的 API → 返回值里搜不到 key 明文；`grep -F "<key明文>" <data_dir>/bot.log` 无命中。
  7. 让机器人用 `http_request` 的 `saveAs` 下载一个 CSV → 文件落 `<data_dir>/AI_Gen_Files/`，返回的是路径与字节数；再让 `run_python` 读该路径做统计 → 断网后能力不回退的关键一步。
  8. 让机器人 `run_python` 跑 `import urllib.request; urllib.request.urlopen("https://example.com")` → macOS 上应报 PermissionError/network 被拒，错误如实回灌（不是静默成功）。
  9. 建一个定时任务，让它在无人值守时调 `http_request` 访问白名单外地址 → 任务应快速失败（不等 60s），bot.log 记 `bot.http_request` 且 `dest=unattended`。
  10. 让机器人调 `open_url` 于定时任务内 → 应被拒（无人值守不打开浏览器）。
- **判据**：
  1. 【可核对】步骤 3/4/5 的行为与 `bot_web/destination.rs` 的 `classify` 分支一致；批准后同会话同 host 不再弹（锚点测试 `yolo_and_session_grant_lifecycle`）。
  2. 【可核对】步骤 6：凭据只走 `keyring:` 前缀引用——真值在宿主解开（`bot/tools/http.rs::resolve_secret`），审计与 preview 只见条目名（`dispatch.rs` 的 http_request 特例 preview）。
  3. 【可核对】步骤 7：`saveAs` 文件名清洗（`save_name_rejects_path_semantics` 锚点）拒绝路径分隔符与 `..`；重名加时间戳不覆盖。
  4. 【可审计】步骤 4/6/9 的每次请求 bot.log 出现 `bot.http_request`，字段只有 host/method/status/bytes/dest——**不含完整 URL、不含 header 值、不含 body**。
  5. 【可核对】步骤 8：`py/seatbelt.rs` 的 `(deny network*)` profile + 自证探测（EPERM 才算生效）；闸失效时拒跑并审计（fail-closed，无放行开关）。
- **证据**：弹窗截图（含剩余时长）+ bot.log 的 `bot.http_request` 行 + saveAs 文件 ls 输出 + run_python 网络报错截图
- **失败处置**：先用 `cargo test --lib bot_web::destination py::seatbelt` 分层——自动档红 = 代码问题；自动档绿而实机不符 = 配置/环境问题（白名单拼写、python 未装）
- **缺口补验**：白名单弹窗在场的视觉形态、会话授权跨重启行为、saveAs 慢网 300s 上限未验
