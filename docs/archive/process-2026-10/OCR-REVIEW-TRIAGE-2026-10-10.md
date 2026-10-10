# OCR 范围评审分诊账本（2026-10-10，对象：2026-10-09 提交）

> 评审：`ocr review`（range `8bde82c..9d9b82c`，会话 f63d8062，MiniMax-M3，46min）——
> 10-09 的 37 个提交、269 文件变更（+30941/−3358）、219 文件入评、536 条原始发现。
> 原始件：`OCR-REVIEW-2026-10-09-RANGE.txt`（同目录，勿手改）。
> 分诊：8 个并行核实代理逐条对照 HEAD（efdab6b）代码，critical 之首由主会话亲验复核。
> 判定口径：REAL 已核实成立 / PARTIAL 现象在但前提或后果不成立（夸大或触发不可达）/
> FALSE_POSITIVE 误报（附反证）/ ALREADY_HANDLED 防线已在、昨日账本已登记或后续提交已修 /
> WONTFIX 属实但既定拍板取舍。low 343 条未逐条分诊（抽样为注释清理机械损伤，见主题四）。

## 总览

| 级别 | 原始条数 | 核实结论 |
|---|---|---|
| critical+high | 44 | 合并同根因后 37 项：REAL 13 / PARTIAL 15 / FP 4 / ALREADY_HANDLED 4 / WONTFIX 1 |
| medium | 149 | REAL 100（其中约 45 条为注释清理机械损伤）/ PARTIAL 24 / FP 7 / AH 2 / WONTFIX 16 |
| low | 343 | 未逐条分诊；抽样以「删工单号残留孤儿标点/畸形 JSDoc」为主 |

**最重要的一句**：昨天两批工作各留下一个用户可感的确定性回归——
S1-8 加锁（c8d19d6）引入**切模型死锁**；注释整洁批（eee9887）以 regex 清理在全仓留下
**约百处注释机械损伤**。另有两处与提交意图直接相悖的功能性丢失（create_skill 僵尸技能、
反思 lesson 空 ops 静默丢弃）。

## 主题一：上轮修复的次生问题（S1/S2 批修 A 带出 B）

| # | 位置 | 判定 | 说明 |
|---|---|---|---|
| R1 | `bot/config/commands.rs:349-357` | **REAL·critical** | `bot_set_active_model` 持 `CONFIG_WRITE_LOCK`（:350）后调 `bot_get_config`（:357），后者无条件进 `migrate_legacy_key`/`migrate_search_keys`（commands.rs:42/:52），两者再 `lock_config_write()`（io.rs:543/:578）；`lock_config_write`（io.rs:39-45）盲取 std Mutex、线程本地标记只喂断言无重入豁免 → **每次 🧠 切模型同线程死锁**。S1-8（c8d19d6）修丢失更新时引入。修法：`bot_get_config` 前 `drop(_g)`（磁盘即刚写入状态）。 |
| R5 | `bot_model_loop.rs:531-538` | **REAL·high** | model_override 命中时 inference 仍走 `effective_inference`（active 条目），`ResolvedModel.inference`（types.rs:540）全仓零消费——覆盖条目的 temperature/top_p/max_tokens/system_prompt 被静默顶替。昨日账本 L116 ✅ 只修了 reasoning 的 model/provider，inference 是同面漏网。修法：`resolved.map(|r| r.inference.clone()).unwrap_or_else(|| effective_inference(..))`。 |
| P8 | `api_handlers/body.rs:96-98` + `handlers.rs:221/:401` | PARTIAL | S2 把 lossy→strict 收严正确，但非法 UTF-8 归入 `Malformed`（文档语义=header 异常），两调用点一律回 "malformed Content-Length header"，Content-Length 合法仅 body 非法时误导排障。400 本身正确，实为 medium。修法：加 `InvalidUtf8` 变体分列文案。 |
| P6 | `bot_desktop.rs:167-178/:231` | PARTIAL | S2 毫秒后缀消了同秒窗口，残余同毫秒并发同名覆盖，极窄。可纳米戳/AtomicU64 收口。 |
| P7 | `bot_web.rs:351/:505/:540` | PARTIAL | S1-4 封顶改造顺带把三引擎 `.text()`（charset 感知 lossy）换成 strict `String::from_utf8`，与同文件 fetch_jina_reader（:1150 lossy）策略自相矛盾；GBK 前提夸大（现网多为 UTF-8）但任一坏字节=整引擎假失败「无结果」。修法：统一 `from_utf8_lossy`。 |

## 主题二：功能性丢失（与提交意图相悖）

| # | 位置 | 判定 | 说明 |
|---|---|---|---|
| R2 | `bot_skills/manage.rs:451-456` | **REAL·critical** | create_skill：目录用参数 `name`（:532），`parse_meta` 遇 frontmatter `name:` 即覆盖（parse.rs:169），无一致性校验；路由/清单用 meta.name（:205/:237）、加载/删除按目录名（state.rs:199、manage.rs:664）→ 失配即产出**列表可见、use_skill 报不存在、删除静默 no-op** 的僵尸技能。schema 明示 frontmatter 必含 name，LLM 填名不一致完全现实。修法：`meta.name != name` 即拒绝。 |
| R3 | `bot_skills/manage.rs:774-788` | REAL·high | R2 的契约（meta.name==name）无测试锁定，全仓无 mismatch 用例。随 R2 同提交补 `create_skill_rejects_frontmatter_name_mismatch`。 |
| R6 | `memory/consolidate.rs:460-463` | **REAL·high** | ops 为空时早退（:461-463）在 `post_consolidation`（:504）之前，lesson 静默丢弃；而 prompt（prompts/consolidate.rs:17-21）明确鼓励 `{"ops":[]}` + 独立 lesson 段——**架空 5c576b4 反思双产物设计**。修法：早退分支补 `post_consolidation(&[], &default(), lesson.as_deref())` 或守卫改 `ops.is_empty() && lesson.is_none()`。 |
| R4 | `bot_skills/scheduler.rs:314-343` | REAL·high | 兼容门 `known` 仅取 `tools_index()`，主步全未知的纯 MCP 技能被确定性终止；而 dispatch 确有 MCP 兜底（dispatch.rs:410 → mount.rs:154），parse.rs:480 注释自认「MCP 未知只警告」。修法：`known` 并入 `mcp_mounted_tools()`（mount.rs:134）。 |

## 主题三：其余 crit/high REAL

| # | 位置 | 判定 | 说明 |
|---|---|---|---|
| R7 | `paths.rs:278-284` | REAL（两条重复合并） | fallback 写 bot.log 的 chmod 失败 `let _` 静默；注释自称「与 audit::open_log_append 一致」但 audit.rs:354-359 对同文件同操作是 eprintln 留痕 + 「失败不能静默」。实际 medium（首次目录翻转才可达）。修法：照 audit 补一行 eprintln。 |
| R8 | `bot/config/io.rs:190-195` | REAL（实际 low） | `if let Some(evo) = obj.get_mut("memoryTuning") { let _ = evo; }` 零副作用死代码（c6dac56 引入），违反「不留死代码」。整块删除。 |
| R9 | `WmDocxRevisions/Program.cs:625-632` | REAL（降 medium） | LCS 守门注释/告警说「退化为整体替换」，实际单条 replace opcode 被下游 ReviseInPlace/fresh-doc 按逐段对齐拆成数千条行级修订——告警文案与消费口径相反。改两行文案（真发整删整增反而降质量）。 |
| R10 | `py/document.rs:1486-1501 / 1873-1887` | REAL（降 medium，两条合并） | `word_template_resolve_in`/`pptx_template_resolve_in` 对模型可控 `template`（SCHEMA_CREATE_WORD/PPT 暴露）无组件校验即 join，可指到模板目录外；影响有界（读侧+存在探针）。修法：照 `word_template_params_update_in`（:1790-1792）既有校验拒绝 `/`、`\`、`..`。 |
| R11 | `py/document.rs:1394-1397/:1427` | REAL（降 low） | doc_make_ppt 显式模板缺失静默回退内置母版，审计行不记模板（doc_make_word 的 tpl_note 记的也是已解析名，同病）。补 requested→fallback 审计。 |
| R12 | `EvolutionPanel.tsx:436-448` | REAL（实际 medium） | onChange 只挡 `Number.isFinite`，1.5/-1 直通 state → serde usize 拒绝 → 原始报错直出、值卡非法态。修法：`step={1}` + `Number.isInteger` + 前端钳制。 |
| R13 | `ui/main.css:708-713` | REAL（视觉） | `.save-btn.is-pressed`(0,2,0) 被 `.save-btn:hover:not(:disabled)`(0,3,0) 压过 background-color/color——鼠标按压（主路径）只余阴影+1px 下沉，深色下反馈反向。修法：提特异性或补 `:hover` 复合（勿用 `:not(:hover)`，会放过鼠标主路径）。 |

## PARTIAL 摘要（现象在、后果不成立或已窄化）

- `bot/registry.rs:979` create_skill `claims_patterns:&[]`：`claims_mutation` 聚合全部 mutating 工具 pattern，「已创建技能」被 create_task 的「已创建」子串兜住，残余仅「已注册」类话术（低频）。真缺口是测试硬编码样本——改遍历 `TOOLS_TABLE.iter().filter(mutating)`。
- `bot_skills/builtin.rs:56` None==None 不自愈：当前资产带 version 恒 Some，前置不可达；TOCTOU 仅审计标签互换。可选加固：disk=None 强制 Updated + 测试断言 version 存在。
- `bot_skills/vars.rs:6` 32-hex 遮蔽连字符 UUID：属实但下游按不存在 id **响败**非静默错乱。建议收紧 v4 形态（13 位=4、17 位 [89ab]）。
- `api_server.rs:159` history 中毒不同步：触发需锁中毒（临界区仅 push/pop，近不可达），注释自认既定语义；建议与落盘 fail-closed 对齐。
- `bot/config/types.rs:582` keyring 兜底吞 Err → 跨厂商静默：爆炸半径窄（需条目级故障+全局读取成功），注释已拍板「统一走全局兜底」。WONTFIX 边缘。
- `memory/extract.rs:438` 批内 Update 降级 New 仍可经 merge（cosine≥0.92）覆盖首条 content/溯源：条件性，kind 不被冲。修法：merge 拒绝本批 claimed id。
- `evolution/mod.rs:167` 同步 IO 阻塞 worker：真问题但只包 thresholds 读是装饰性修复——post_consolidation 整体（jsonl RMW/sqlite/audit 直写）都该进 spawn_blocking。
- `trace_sink.rs:213` registry 先注销后 trace_finish 计数低估：昨日账本 L321 已登记 🔧，不重复开票。
- 前端：`MemoryPanel.tsx:659`（原判 critical）链路真但 `memory_tuning_get` 后端恒 Ok，仅 IPC 层失败可达——加 loaded 态禁用 Save 属廉价加固；`:662` 毫秒级窗口；`ChatPanel.tsx:301/:960` 镜像/守卫缺口各一帧/近不可达（昨日已登记 🔧）；`ModelRow.tsx:106` catch 后 onTested 徒劳 reload+抖动；`WordTemplatePanel.tsx:36` 对话框模态不可达，防御性一行。

## FALSE_POSITIVE（附反证）

- `Program.cs:55` self-copy 穿越可比对绕过：out 由 `gen_out_path_in`（document.rs:1445-1463 while exists 递增）保证不与现存文件冲突，originalPath 必已存在，二者不可能同一文件。
- `document.rs:1420` import file_stem 穿越：file_stem 是末段组件，不含分隔符；`..` 场景 join 后是目录内怪名，无穿越。
- `bot_fs.rs:657` regex ReDoS：regex 1.13.1 有限自动机引擎、线性时间保证，不存在灾难性回溯；建议的 size_limit(64MB) 比默认 10MiB 更宽松，方向反了。
- `main.css:708` --inset-bg 浅色缺失：`:root` 23 行有定义（OCR 把 :root 范围读错）。

## ALREADY_HANDLED

- `document.rs` 回归锁恒真断言：评审版本（9d9b82c）属实，今日 9174e8b 已把该测试替换为有效的 `docx_template_params_single_source`。
- `bot/config/io.rs:164` TUNING_WRITE_LOCK 跨写者：昨日账本 L59 已标 📋（已登记 follow-up）。
- `WordTemplatePanel.tsx:55` 行内变更无 busy 守卫：`disabled={busy}` + 模态对话框已兜住，无可达并发。

## medium 层 REAL 中的行为类要点（100 条 REAL 的非注释部分，~55 条）

- **谎报/审计面**：`bot/tools.rs:1562` PDF 丢图误记 `doc_word_images_sanitized`；`manage.rs:602-630` skill_create 失败无审计；`db/mod.rs:139-141` legacy copy 一分支只 eprintln 不进审计。
- **吞错**：`manage.rs:550-554` `let _ = remove_dir_all` 吞回滚失败（Windows 文件锁下永久误报同名）；`evolution/policy.rs:130-132` let-else 吞全部 IO 错误（与 read_apply_policy_at 口径分裂）。
- **资源/放大**：`bot_fs.rs:678-687` edit regex 输出无上限（1MB 可放大数十倍直写）；`:1006-1017` 替换计数不进审计。
- **路径tokenizer**：`bot/registry.rs:660-672` 边界表无 UNC 分支，UNC 假路径绕过存在性校验（注意：OCR 建议的加 `'\\'` 进边界会拆碎盘符路径，勿照抄）。
- **模板静默**：`bot/tools.rs:1499-1501` 显式模板不存在静默空白默认（与 R11 同根）。
- **PPT**：`py/document.rs:942-943` 无条件覆盖 slide 尺寸 16:9，4:3 模板拉伸。
- **前端**：`HoldToConfirmDelete.tsx:233` 键盘 setTimeout 无 ref 清理；`:100-118` done 后 phase 卡 pressing 重按矛盾态；`:97-99` 倒退中重按进度环跳 0；`ExecuteBar.tsx:54-59` 空任务卡运行中 label 与 onStop 行为矛盾；`MemoryPanel.tsx:674-687` Save 无 ref 闸可双 invoke；`EvolutionPanel.tsx:171-183` 保存后不回拉（输入 0 后端落 2，UI 分歧）；`WidgetApp.tsx:300-301` top 边 clamp 上限用 sw-44 可溢出 176px；`main.css:827` `has-liquid` 类无对应选择器（动画无条件播放）；`WordTemplatePanel.tsx:155/207` 加载失败 error 与 EmptyState 同渲染、confirm 与 DeleteConfirmDialog 模式不一。
- **测试缺口（含金量高）**：`error.rs:339` 控制字符剥离零覆盖（且 0x7F 不在剥离集，注释笼统）；`evolution/proposal.rs:233` 非 ASCII 大写折叠零覆盖；`derive_lesson_proposal` 全仓唯一调用点无任何用例；`bot_model_loop.rs:1723` 断言依赖 `/tmp/wm-guard-test/...` 外部不存在（外部残留即翻转）。

## 主题四：注释整洁批（eee9887）机械损伤——本轮最大簇

约 **45 条 medium + 343 条 low 的主体**同源：regex 删工单号把多行 JSDoc 折成单行、
残留 ` * `/孤儿 `（）：`/双空格/悬空词（「起」「：」开头），遍布
SettingsPage.tsx（22+ 处）、App.tsx、useChatUi、GraphCanvas、McpPanel、MessageList、
GoalNode、graph.ts、ErrorBoundary、migration/journal（6 处）、bot_model_loop（5 处）、
db/brief、bot_artifacts、prompts/subagent 等；**`paths.rs:302` 残留工单号 NEW-1345，
踩硬禁令**。建议单开一个机械化修复批 + 一条 CI 约束（注释清理后 grep 孤儿标点）。

## 建议修复批（待拍板，本账本不动手）

1. **批 A·正确性回归（建议立即）**：R1 死锁（一行）、R6 lesson、R5 inference、R4 MCP 门、R2+R3 僵尸技能+测试、R12 前端整数、R13 css。
2. **批 B·加固**：R10 模板名校验、R11+tools.rs 模板回退审计、R7 chmod 留痕、R8 死代码、P7 统一 lossy、P8 InvalidUtf8、medium 吞错/审计面/regex 上限/UNC/16:9。
3. **批 C·注释损伤大扫除**：主题四全量（含 low 同类）。
4. **批 D·测试补网**：error.rs/proposal.rs/lesson 派生/阈值回拉等。
5. **不修**：FP 4 条、已拍板 WONTFIX（types.rs:582、io.rs:164、app_state 盘点、scheduler CAS、profile poisoned 形态等 16 条 medium）、PARTIAL 观察项。
