# agent 架构全面审计（2026-10-17）

## 处置状态（同日）

| 结论 | 状态 |
|---|---|
| §一-1 点名扫描前缀误判（`create_word` ⊂ `create_word_revisions`） | ✅ 已修并提交 `2d65dbd`：点名扫描改标识符整词匹配（只判 ASCII，中文紧贴标识符是正常写法）+ 性质锁。**未动词表与判决结构** |
| §一-2 提示词入参拼法 | ✅ 已修并提交 `b9e33a3`。**修的范围比初报大**：不止 `waitMs`，同类漂移共三处——`acceptanceCriteria`（spawn_subagent 的 **required** 参数，提示词里 11 处）、`contextSummary`、`waitMs`；另补结构锁（任何提示词不得出现 schema 属性名的 snake_case 形态） |
| §一-3 `tools.max_output_chars` 显示 ≠ 生效 | ✅ 已修并提交 `913898c`：上限单源提到 `bot::params`，写侧钳制 / 读侧展示 / 模型循环生效点三处共用；ParamDef 的 `default: "不截断"` 改 `30000` 并与 `DEFAULT_TOOL_OUTPUT_CAP` 绑测试 |
| §二 P2-4 `errorClass` 前端零消费 | ✅ 已修并提交 `8b1e7bc`：九类失败分类在 span 行尾渲染中文短词，未登记值走「失败（原值）」 |
| §二 P2-5 缓存 token 被丢 | ✅ 已修并提交 `8b1e7bc`：水位条按 trace 弹窗同口径单列缓存读/写；提前 return 改成四项全 0 才丢；百分比用「最近一轮 input + cache read」；后端那条承诺「命中率」的注释改成描述实际行为 |
| §二 P2-6 `clamp(1,10)` 第三处手写 | ✅ 已修并提交 `a3de6cb`：工具入参口改调 params 单源；顺带修 `u64 as u32` 先截断再钳会放行荒诞条数的真 bug |
| §二 P2-7 两处重复边界 | ✅ 已修并提交 `a3de6cb`：`verifier_retry_max` 补 `resolve_verifier_retry_max`；`max_function_calls` 的双钳收成派生链末端一个 `fuse_cap_for`，加源码扫描守卫 |
| §二 P2-8 架构文档失真 | ✅ 已修并提交 `9e6246c`：十处行数按实际刷新、删重复条目、「子卡（🧩）创建」与「acceptance 双写」两句改掉；`audit_module_map.py` 补行数对拍与去重断言（去重 key 含缩进前缀，否则子级 `paths.rs` 会误报） |
| §二 P2-9 对拍脚本已红却不在门禁 | ✅ 已修并提交 `9e6246c`：陈旧断言（`evolution_keep_shadow`，命令在删影子轨那笔已删）删除，桥对拍回到 16 passed；`tests-audit/` 接回 `test-all.sh` 第 3 步（2 秒），并记下一个更隐蔽的洞——这些文件名不匹配 pytest 默认 glob，整目录跑会 `no tests ran`（rc=5）静默放过，必须逐个点名 |
| §二 P2-10 trace 状态死分支 + 缺 model | ✅ 已修并提交 `8b1e7bc` |
| §二 P2-11 schema 过期默认值 | ✅ 已修并提交 `a3de6cb`：`maxToolCalls` 描述改为不撒谎的措辞（真源是 `loop.max_function_calls` 配置） |
| §二 P2-12 参数表只读区字面量 | ✅ 已修并提交 `a3de6cb`：六行改引真常量 + 补 `subagent.per_session_running` 条目（原 label 说全局、hint 写「每会话 2」而表里没那条）；测试双向钉死 |
| §二 P2-13 前端两张状态表 | ✅ 已修并提交 `8b1e7bc`：合到 `constants.ts` 一份，`succeeded` 统一「完成」 |
| §三-1 钩子层（L4/L5/L6） | ⏸ 用户拍板保留——它是 `docs/ONION-HOOKS-DESIGN-2026-10-16.md` 的多层洋葱键三层，接线为该设计期 1/2 的交付物，后续期的用户 onion yaml 钩子要用 |
| §五 幻觉守卫的触发层 | ⏸ 用户拍板维持现状（关键词触发层不动）。已修的只是判决层点名扫描的词边界 |
| §六-3 前缀关系锁 | ✅ 随 §一-1 落地（性质锁：前缀名对互不点名） |
| §六-1 module map 行数/去重断言 | ✅ 随 §二 P2-8 落地 |

> 全量门禁（`scripts/test-all.sh`）：Rust nextest + 前端 746 例 + 对拍 26 例全过，总耗时约 79s。

> 范围：agent 内核（`bot_orchestrator.rs` / `bot_model_loop.rs` / `bot_chat.rs` /
> `bot/*` / prompts / db.subagents）与前端的跨端契约。起点 commit `2a80f96`。
> 方法：四路只读 verifier 并行（内核一致性 / 冗余过度开发 / 验收校验负担 / 前后端契约），
> 每条 P1/P2 由主会话逐行复核；只读，未改任何代码。
> 前序基线：`REDUNDANCY-AUDIT-2026-10-14.md`、`AUDIT-2026-10-16.md`（其列条目本轮
> 抽查确已修，不再重复报告）。

## 总评

| 轴 | 判定 |
|---|---|
| 一致性 | 中上。状态机与事件层干净（六态唯一枚举、23 个事件全有听、命令 148/148 闭合）；裂缝集中在**参数单源**与**提示词↔schema 拼写**。3 条 P1 |
| 冗余 / 过度开发 | PASS。跨文件引用计数扫描下 **0 个死函数**、0 个未使用依赖、0 项「删掉会丢能力」。净可删约 30 行，全是同一表达式被重写 |
| 验收 / 校验是否繁琐 | **以纵深防御为主，约 25–30% 属重复仪式**。普通读路径 21 道闸、子 agent 生命周期 20+、参数落库 5 层；核心闸不可删，可压的是只计数不改判的钩子层与重复判定 |

## 一、P1（真断链，建议本轮修）

### 1. 幻觉守卫把 `create_word_revisions` 误判成 `create_word`（实测可达）

`mentioned_tool_names` 按 `text.contains(tool_name)` 扫全部 45 个工具名
（`bot/registry.rs:801-830`）。`create_word` 是 `create_word_revisions` 的前缀子串，
于是模型如实汇报修订模式工具时，点名表里同时出现两个名字，而凭证池只有后者：

```
text      = "create_word_revisions 执行完毕，返回 3 个修订版本。"
mentioned = ["create_word", "create_word_revisions"]
receipts  = ["create_word_revisions"]        # 真调过
unsupported = ["create_word"]                # → 判决为伪造
```

判定链：`bot_model_loop.rs:2094-2107`（点名/凭证）、`:2131-2178`（打回并注入返工指令）。
后果：命中 `prompts/system.rs:20` 与 `prompts/execute.rs:13` **推荐**的 Word 润色路径时，
模型被告知「你刚才说调了 create_word 并给出了结果，但没有任何 create_word 的成功调用
记录——这个结果是编造的」，返工指令又让它「立即发起 tool_call 实际执行」——可能真的
生成一份**非修订模式**的 docx，即为一个假的 accusation 付一次错误产物。

全 45 个工具名两两比对，只有这一对前缀关系（`subagent` ⊂ `spawn_subagent` 由别名表
去重兜住）。修法：点名扫描按长度降序命中后跳过已被更长命中覆盖的名字，或要求命中两侧
不是 `[a-z0-9_]`。回归锁：加一条静态断言「TOOLS_TABLE 中任一工具名不得是另一工具名的
前缀」——这条同时挡住将来新增工具时的复发。

### 2. 提示词教模型传 `wait_ms`，schema 与解析只认 `waitMs`

`prompts/subagent.rs:40`：「用 check_subagent 轮询（wait_ms 0 或短等待，不要高频空转）」
（镜像资产 `docs/prompts/subagent/main_agent.md:28` 同错）；schema 是 `waitMs`
（`bot/registry.rs:376`）；解析是裸 `v["waitMs"].as_u64().unwrap_or(0)`
（`bot_orchestrator.rs:2391`），`parse_tool_args` 无 snake→camel 归一
（`bot_orchestrator.rs:2275`）。模型照提示词写 → 等待值恒为 0 → 正好触发该提示词要防的
「高频空转」。同类漂移 AUDIT-2026-10-16 P1-5 修过一次（check_subagent 返回体），提示词
侧的字面拼写被漏掉。修法：`wait_ms` → `waitMs`（两处，含镜像资产）。

### 3. `tools.max_output_chars` 设置页显示值 ≠ 运行时生效值

`bot/params.rs:403-409` 的 resolver 对 `Some(v)` 原样 `v.to_string()`，而
`bot_model_loop.rs:1076` 运行时 `.min(200_000)`；写侧 `bot/config/commands.rs:282-295`
的钳制清单里没有这个字段（其余七个字段都在）。用户把值设到 200K 以上，设置页显示
他设的值，运行时被静默改写——正是 `params.rs:30-31` 那段文档声称要杜绝的情形。
附带：`params.rs:267` 的 `default: "不截断"` 与同文件 `:407` 的 `None => "30000"`、
`bot_model_loop.rs:401` 的 `DEFAULT_TOOL_OUTPUT_CAP = 30_000` 自相矛盾，hint 文案
「默认不截断」也错。

## 二、P2

| # | 问题 | 证据 |
|---|---|---|
| 4 | `errorClass` 九类分类后端产出、落库、下发，前端零消费（类型声明在，渲染只读 `s.ok`） | `audit.rs:198-217` ↔ `src/lib/trace.ts:38`、`TracePanel.tsx:41-76` |
| 5 | `bot-usage-delta` 的 `cacheReadTokens`/`cacheCreationTokens` 前端不接，且 `i<=0&&o<=0` 提前 return 会丢弃只带缓存 token 的增量；后端注释声称「前端按命中率展示」，该 UI 不存在 | `bot_model_loop.rs:1033,1047-1056` ↔ `ChatPanel.tsx:944-961`。注：trace 弹窗有缓存两档（`TracePanel.tsx:352-364`），实时水位条没有 → 同一个指标两处 UI 两个数 |
| 6 | 钳制边界第三处手写：`clamp(1,10)` 绕过 `params::clamp_search_max_results` 单源（REDUNDANCY-AUDIT 第 8 项声称已修，只覆盖写侧与读侧两条路，工具入参口漏了） | `bot/tools.rs:1836` ↔ `bot/config/commands.rs:294`、`bot/params.rs:84` |
| 7 | `verifier_retry_max` 的 `0..=3` 写侧钳、读侧再钳（`bot_chat.rs:844` 违反 `params.rs:15-16`「读取点不得各写一份」）；`max_function_calls` 的 `1..=500` 在 `bot_model_loop.rs:955` 与 `:1230` 各钳一次 | 同左 |
| 8 | 架构文档漂移且对拍脚本看不见：`bot_model_loop.rs (1711)` 实为 3997、`bot/registry.rs (716)` 实为 2422、`bot_model_loop.rs` 条目重复出现两次（`:92` / `:153`）、`:147-148` 仍写「子卡（🧩）创建」而代码已无卡 | `docs/rust-bot-architecture.md:92,98,147-153`；`tests-audit/audit_module_map.py:26` 的正则只捕获文件名、`tree_entries()` 不去重 |
| 9 | `audit_tauri_bridge.py` 自检已红：`evolution_keep_shadow` 全仓不存在，实跑 1 failed / 15 passed；`tests-audit/` 已于 2026-10-08 降级为手跑，腐烂不可见 | `tests-audit/audit_tauri_bridge.py:477`、`scripts/test-all.sh:3` |
| 10 | TS `TraceRow.status` 含后端永不产生的 `"timeout"`（它只作 error_class 存在）+ 死徽标分支；`TraceRow` 缺后端已下发的 `model` | `src/lib/trace.ts:14`、`TracePanel.tsx:32` ↔ `db/trace.rs:31-34,117` |
| 11 | `spawn_subagent` schema 说 `maxToolCalls`「默认 100」，真源是 `maxFunctionCalls` 配置 | `bot/registry.rs:370` ↔ `bot/params.rs:110-115` |
| 12 | 硬编码只读区 5 行的字面量与代码常量各存一份、无测试绑定（`"3"`↔`MAX_GLOBAL_RUNNING`、`"50"`↔`db/subagents.rs:21`、`"2次/1.5s"`↔`bot_model_loop.rs:431-432`）；`subagent.max_running` 的 label/hint 自相矛盾，且「每会话 2」在表里没有条目 | `bot/params.rs:283-288,286-333` |
| 13 | 前端两张子 agent 状态标签表各写一份，`succeeded` 文案不同（完成 / 已完成） | `ContextRail.tsx:74` ↔ `ChatPanel.tsx:423` |

## 三、验收与校验：繁琐度账本

### 关卡账（同一事实判几处）

| 主路径 | 关卡数 | 必要 | 可压 |
|---|---|---|---|
| 一次普通文件读（白名单内） | 21（`bot_chat.rs:757` → `bot_fs.rs:1458`） | 18 | 3：pre_tool/post_tool 内置观察钩子只计数不改判（`dispatch.rs:395,579`）；终稿自检与幻觉守卫三支柱判同一件事（`bot_model_loop.rs:2225` vs `:2082-2132`） |
| 一次被判谎报后的返工 | 词表三支柱 → 一次打回预算 → 按原因分岔文案 → 重进终稿自检 | 真凭据只有 `receipts`（`:2483`），其余全是词表 | 终稿自检的重复判定（但它与守卫机制不同，**属决策项非缺陷**） |
| 子 agent spawn → done | 20+（`bot_orchestrator.rs:192` → `subagents.rs:148`） | 「产物在不在」四判里只有服务端机器验收 `file_exists`（`:1915`）不可自述 | verifier 自评（`:700`）/ check 计数（`:1167`）/ 前端展示（`SubagentDetailPanel.tsx:53`）三处可收敛为服务端一个真值 |
| 一次参数修改落库 | 5 层（前端 parseInt → 落盘钳 → sanitize → 读侧 resolver → 工具层 check_len） | 2 | 3（写侧 + 读侧重复，见 #6/#7） |

### 纯仪式（答不上「防什么真实场景」）

1. **钩子层三处内置观察者**（`hooks.rs:362/383/400`）：只 `fetch_add` + `log::info!`。
   更强的证据——`register_pre_tool` / `register_post_tool` / `register_post_turn` 在
   `src-tauri/src` 内**零调用方**（只在 hooks.rs 的 `#[cfg(test)]` 段与 6 处测试里用），
   三个计数器 getter 也只被自己文件内的测试读。生产可达的实现就是这三个计数器，
   即：每次工具调用多 3 次注册表遍历 + 3 行日志。这是本仓唯一一处真正的投机抽象
   （421 行生产段：注册表 / 层序 / 会话级禁用 / panic 捕获 / 只读视图）。
   **但它有设计背书**（`docs/ONION-HOOKS-DESIGN-2026-10-16.md` 期 1/2 已落地，
   用户配置的 onion yaml 钩子属后续期），所以这是**决策项**：留着等接线，或先摘生产接线
   只留测试。若摘，需同步删 `bot_model_loop.rs:3958` 的源码守卫测试。
2. `mentioned_tool_names` 里的别名 target 存在性复核（`registry.rs:813-821`）：编译期
   常量拼错已被 `registry.rs:3083` 的测试覆盖。
3. `check_subagent_locked` 的 `.min(subtasks_total)`（`bot_orchestrator.rs:323`）：
   `count_met_against_declared` 已按 declared 去重，数学上不可能超。
4. 提示词侧同一句强制说三遍：`prompts/subagent.rs:42/57/75` 三处「verifier 段是验收
   闭环强制环节」、`:42/75` 两处「不接受我自己看看就行」。纯 token 浪费。

### 不可动（各有唯一防护或独立输入面）

`bot_fs.rs:299-336` 的 canonicalize + 分量比较（唯一 FS 边界）；MCP URL 双验
（保存口 vs 发请求前，输入面不同）；`bot_web.rs` 的逐跳 SSRF（重定向后必须重校验）；
`bot_orchestrator.rs:1905` 的 `fused_on_rounds` 排除（防给熔断行凭空加机器验收结论）；
`dispatch.rs:431` 子 agent 白名单闸（schema 过滤防「看不见」，闸防「硬猜名字」）。

## 四、冗余与过度开发：判定 PASS

跨文件符号引用计数（src + tests 全量）扫描 agent 层 34 个文件 25,688 行后：

- **死函数 0 个**（全仓出现次数 = 1 的 `pub fn`：0；第一版脚本曾误报 34 个，只统计外部
  引用漏掉同文件内部调用，修正后全部证伪）
- **未使用依赖 0 个**（`Cargo.toml` 里 `ort` 出现两次是 Windows 目标特化覆盖，非重复键）
- **净可删约 30 行**，全部是同一表达式被重写：软警告阈值表达式抄三遍（`bot_model_loop.rs:1233,1239,3847`）、
  `fmtMs` 前端两份（`MessageList.tsx:348` / `TracePanel.tsx:35`）、`SystemTime` 手写毫秒而下一行就在调
  `now_ms()`（`bot/tags_consolidate.rs:238`）、`audit_log_hook` 是 `audit_log` 的纯别名
  20+ 调用点（`bot/config/audit.rs:18`）

**查过不是过度设计**（免得下轮重查）：hooks 三个 trait（第二 impl 在测试里，且接缝测试
要求它存在）；`apps` 的 `*_core` 拆分（两个调用方）；`truncate_with_summary_core` 泛型
（可测性注入，4 个专属测试）；`BUILTIN_TOOL_COUNT = 45`（被测试钉死且 `TOOLS_TABLE` 是
`static` 而非 `const`）；`escape_for_log` / `claims_patterns` / config 字段（真单源，
抽样 11 个敏感字段均有真实读取方）。

## 五、幻觉守卫专项点评（用户点名的「验收繁琐」主因）

最近 4 笔提交（`0d06b60`→`2a80f96`）为这一个守卫付出：词表 13 → 22 条、
`llm_integration.rs` +521 行、DEVLOG 三段换方向记录。作者自己的结论已经写在
DEVLOG 7185-7200：「加完『返回了』它会说『跑出来是 100』……这是无穷的追词游戏」。

判断：**分层方向是对的（关键词只做触发、凭证表做判决），但触发层必然漏**，
且每次补词都是一次误伤决策。第 1 条 P1 正是这个结构的必然产物——漏不在判决层，
而在点名扫描的字符串匹配没有词边界。

两条可选方向（属决策，不属缺陷）：

1. 收窄触发层：词表只保留「完成态 + 明确工具名」的组合，把「我跑了」这类裸完成态
   交给凭证表兜（点名时必须带工具名才有可查对象）。代价：零凭证轮 + 不点名的谎报
   漏网率上升。
2. 换成结构化判据：数字型结论必须绑定本次提问的 `tool_call_id`（trace 已在手里），
   「声明过的步骤」复用既有 `bot_plan::PlanState`（DEVLOG 7146 自己也指向这条）。
   代价：要接 bot_plan，多一条依赖边。

## 六、门禁建议（三条都是本轮发现的具体成因）

1. `tests-audit/audit_module_map.py`：正则加行数捕获组与 `wc -l` 对拍；
   `tree_entries()` 加 `len(entries) == len(set(entries))` 断言 → 直接挡住 #8。
2. `tests-audit/audit_tauri_bridge.py:477` 的 `evolution_keep_shadow` 断言是陈旧自检
   （实跑 1 failed / 15 passed）：先决定该命令是否该恢复（产品决策），再修断言；
   修完把两个脚本接回 `test-all.sh`（已降级手跑三个月，腐烂零可见）。
3. 新增静态锁：`TOOLS_TABLE` 内无工具名是另一工具名的前缀（挡 #1 复发）；
   `PARAMS_TABLE` 硬编码行的值 == 对应代码常量（挡 #12）。

## 七、验证记录

- 复核方式：P1 三条 + P2 的 #4/#5/#6 与 #8 由主会话逐行读代码/实跑脚本确认；
  #1 用与 `mentioned_tool_names` 同谓词的脚本复现（`text.contains(name)` × 45 个工具名）。
- 未跑全量测试（只读审计）；`audit_tauri_bridge.py` 实跑过一次，1 failed / 15 passed。
- 旧审计抽查：六态枚举单一、`receipts` 作用域「本次提问」、`now_ms` 单源、
  `SubagentBudget::clamped()` 无旁路 —— 均属实，声称已修的项本轮未发现复发。
