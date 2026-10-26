# 实机冒烟测试报告：2026-10-24 → 10-25 变更窗口

对 `docs/realmachine-smoke-test-2026-10-25.md`（M75–M92 + W10–W12）的执行报告。执行者：ZCode 代理（自动化驱动 UI + 只读取证）。执行时间：2026-10-25 16:52 → 21:10。

| 项 | 值 |
|---|---|
| 被测版本 | 源码 `415e2b23`（HEAD）；`src-tauri/target/debug/wmessage` 于 16:52 重新链接（前端 `dist/` 重打以纳入 10-25 前端拆分波，Rust 代码零改动，仅 `touch` 触发 context 重嵌） |
| 平台 | macOS（darwin 25.6.0 arm64）；**Windows 专章 W10–W12 全部未跑** |
| 运行形态 | dev 二进制 + `npm run dev`（vite :1420），与仓库现行开发形态一致（无 custom-protocol feature） |
| 模型 | 主力 MiniMax-M3（anthropic 端点；**昨日 402 额度耗尽今日已恢复**，全会话 60+ 轮请求零 4xx/5xx）；M78 用 DeepSeek V4 Flash（openai 端点）跑了两档对照 |
| 约束 | 用户要求「只留报告，不改代码」→ **变异注入 V76–V96 全部未执行**（需临时改源码注入），§四 登记 |
| 驱动 | `scripts/realmachine/smoke.sh`（FIFO 喂人工段）+ `/tmp/mutest/wm-ui.sh`（置前→坐标点击→pbcopy 粘贴→回车）+ 轮询截屏的「确认」按钮像素扫描守望（弹窗 danger 确认链） |
| 环境 | 全部临时改动（配置键、MCP 服务器、api flag、测试卡 5 张、测试技能 1 个）已还原/删除，收尾干净启动验证通过；`git status` clean |

**总评：18 张主卡全部有结论——16 张 PASS，M89 行为面 FAIL（1 项发现），M84 路由半 PASS / 副作用半 FAIL（1 项发现）。另出 2 项代码面发现与 4 项观察项（§三）。无 P0，无 panic，无回归。**

---

## 一、基线冒烟（smoke.sh 14 项）

| # | 检查项 | 结论 | 证据 |
|---|---|---|---|
| 1 | 启动无 panic | PASS | 12s 进程存活，stderr 无 panic（多次重启均同） |
| 2 | error.log 零增量 | PASS | 阶段 D 前后 274 行持平；全会话 +52 行逐条归因为测试自造（§六） |
| 3 | subagents usage 三列 | PASS | `prompt_tokens/completion_tokens/cache_read_tokens` 在位 |
| 4/4b | 库完整性 + 表数 | PASS | `integrity_check=ok`，26 张表 |
| 5/5b | 探针 mark/report | PASS | 水位落盘，report 可读（多次 mark/report 循环） |
| 6a | 启动截图 | PASS | 首页渲染完整（每日一图/KPI/图谱速览/日程） |
| 7–9 | 四页渲染 | PASS | 首页/任务（周网格+任务池）/聊天/设置逐页截图正常 |
| 10/11 | 两轮真实往返 | PASS | trace 180（「冒烟通过」，1.7s）/ 181（990ms），`trace.complete status=done` ×2，cache_hit_pct=100 |
| 12 | Enter 发送口径 | PASS | M92-6 合并验证：Shift+Enter 换行不发送、Enter 发送，`user.message` 内容含 `\n` |
| 13/13b | 优雅退出清理链 | PASS | Cmd+Q 1s 内退出；`app_exit_cleanup \| api_stopped=true \| exec_drained=true` |
| 14 | 文件面静态 | PASS | `builtin_skills/` 无 `.tmp` 残渣；`runtime/flags/` 四旗标齐 |

**脚本健壮性注记**：smoke.sh 阶段 C→D 在 nohup（无 TTY）+ 系统 bash 3.2 下死于 `COMPLETE\ufffd: unbound variable`（139 行，多字节字符紧邻变量名），阶段 D 四项由人工补齐。交互式终端下未复现验证——登记为脚本环境健壮性小项，非产品问题。

---

## 二、逐卡结论

### T0 档

**M75 轮内滑动窗 — PASS**。8 个 ~10.7KB 日志文件逐个读入后触发：`loop.tool_window.applied | chars=61624 | trimmed=2 | keep_recent=6`，全会话仅此一条（一次性 ✓）。库里 `exec_spans.result` 仍是原文（8×7703 字符）；用户消息未被裁；模型引用最近结果内容正确。彩蛋：被裁的两条模型自己用 `lookup_receipt` 调回了原文（M75↔M76 联动按设计工作）。

**M76 召回闭环 — PASS**。正向：新轮次问「B.md 第一行」→ `lookup_receipt` 无参列出 6 条凭证 → 按 `span_id=863` 调回完整原文 → 模型复述与文件逐字一致，未重读文件、未编造。反向：`tool_call_id="call_does_not_exist"` → 明确返回「查不到……不要凭记忆复述」，模型如实转告。

**M77 截断滞回 — PASS（判据口径登记）**。budget 临时 20000：第 10 轮起触发 `chat.history_truncated | dropped=13`；改 90% 档后 `dropped=7`——**参数档位生效实证**。设计真目标（缓存前缀稳定）达成：截断后的连续轮次 `cache_read_tokens` 31349→31369→31389 逐轮 +20（纯追加），hit 100%，模型对被裁内容如实说「记不清」并准确复述保留段主题。
**口径偏离**：`chat.history_truncated` 事件**每轮**重现（dropped 恒 13）而非卡片预期的「远少于轮数」——原因是历史每轮从 DB 全量重载、split 点稳定，裁剪结果不变，缓存前缀不受影响（行为对、记账口径与卡面预期不同）。该卡判据「出现次数远少于轮数」建议改锚 `cache_read` 稳定性；由此牵出发现 F3（每轮重复摘要调用）。

**M78 思考段回灌 — PASS（含「无反证也算 PASS」档）**。DeepSeek V4 Flash + 工具轮：开（=1）两轮、关（=0）两轮全部 200，无一例 400（该模型思考模式下不强制回传——把「上游不要求」记入报告，正是默认关的依据）。Anthropic 路径（MiniMax-M3）全会话 60+ 轮正常，不受开关影响。变异面（盖章点/Anthropic 塞键）未注入。

**M79 侧调自检 — PASS**。配侧调模型（临时 `selfcheckModelId` → MiniMax-M3 条目）：同 trace 内 6 文件读 + `create_task`，消息栈 67852 字符 > 40000 阈值 → `tool_selfcheck.sidecall | chars=67852 | verdict=pass`（非 unknown），主循环少占一轮、无「【自检】」注入（全文 0 命中），随后轮次照常。短历史对照：同会话早期小栈 create_task（trace 206）无 sidecall（留在 in-loop，且 in-loop 通过时本来就静默）。失败回落半未自然触发（侧调未失败过），由单测兜。

### T1 档

**M80 注入并入本轮消息 — PASS**。`remember_fact`（靛蓝色）→ 两轮闲聊 → 追问：答「靛蓝色」（记忆块生效）；长会话 `cache_hit_pct` 恒 100、`cache_read_tokens` 单调增长（MiniMax 口径 `input_tokens=0`、命中走 `cache_read_tokens`，≥60% 验收线按此口径以「hit=100% + 逐轮 +20 追加」达成）。Anthropic 端点（本改动的原始动机路径）全程实证。

**M81 注入失败留痕 — 未完整复现（诚实清单）**。两次外部 `BEGIN EXCLUSIVE` 持锁注入（发送后 +0.3s/延迟版）都没打进装配窗口：WAL 下 `ensure_table` 对已存在表是读路径、`touch_accessed` 仅命中时写，失败窗口工程上太窄。对话本身两次都正常完成（降级面 ✓）。`memory.injection_failed` 留痕分支存在（memory/mod.rs:364/369）且上层 `?` 链不上抛，由代码审读 + 单测兜。

**M82 易错工具 schema 示例 — PASS**。静态半：5 条示例行在 `tools_baseline.json`（create_task 的 due 示例、edit_task 列值示例、complete_task 前置确认示例、query_tasks view 示例、fetch_url 403/429 示例）；ui_type 的示例（先 activate_window/target 形如 12358:0.7）在运行时 schema（`SCHEMA_UI_TYPE`）中在位——夹具按设计只锁核心 28 工具前缀，ui_type 属后追加族不进前缀，对拍测试口径如此，非漂移。动态半：M79 轮 `create_task` 首参 `due=2026-12-31T15:00`（T 分隔、分钟补零）、`column=todo`，一次成型零重试。

**M83 并行引导 — PASS**。「依次读 A/B/C」→ `exec_spans` 同 `turn=0` 三条 `read_text_file`；run_python 在下一 turn。多次同类请求（biglogs 8 连读）也是逐 turn 批量读。未见不该并行的合并（check_subagent 恒单发）。

**M84 会话档技能 — 路由半 PASS / 副作用半 FAIL（发现 F2）**。通过半：`create_skill` 弹 danger 确认（60s 窗口实测两次，超时即拒绝——fail-closed ✓）；确认后立即生效（同会话说「蓝鲸跃海」→ `skill.start | name=bluewhale-leap`）；frontmatter 被强制 `scope: session`（模型没写 scope，落盘是 session）；**新会话 B 无 `skill.start`**；**重启后同触发词无路由**；UI 确认弹窗文案本身写明「仅本会话…重启后不再自动触发」。失败半：B 会话模型**点名**调了 `use_skill(bluewhale-leap)`（被「另一会话占用」闸拦住后才改手动读文件）——会话档技能的名字泄露进了全局广告面，见 F2。附：技能体是散文步骤 + `mode=auto` → 执行报「无可执行步骤（DSL 解析为空）」，失败诚实（观察项 O-1）。

**M85 采集金丝雀 — 正常半 PASS / 反向半未跑**。开启本地 HTTP API（写 `api-enabled.flag` → 重启自动拉起，4763 LISTEN）：巡检周期内无 `service.listening_canary_failed` / `listening_sample_failed`（转折记账语义下，「持续正常」即无事件 ✓）；`list_services` 返回 `listeningSampleOk:true`（面板数据源活着，顺带覆盖面板「已采集」状态）。反向半（摘 lsof）：`run_lsof` 带 `/usr/sbin/lsof` 绝对路径兜底，PATH 摘除无效，破坏兜底需 sudo 改系统文件权限——超出「不改代码也不动系统」边界，未做；`canary_blind` 判定由 `bot/services/tests/canary.rs` 单测兜。

**M86 MCP 超限留痕 — PASS**。测试夹具服务器（`/tmp/mutest/mini-mcp-big.mjs`，echo 固定返 600KB，非仓库代码）经设置页「工具」按钮发现（`mcp.tools_discovered | id=mini-big | tools=1`）→ 调用触发两条留痕：`tool.output.over_budget | tool=mcp_minibig_echo | server=MiniBig | output_bytes=600004 | max_bytes=30000`（原始字节数 + server 字段齐）→ 二级 30049/30000。模型看到的是整形后内容 + 溢出文件指针，最终回复如实转述「600,004 字节被截断、只回显 X 段、尾部不完整」，还主动 `run_python` 读溢出文件核验了 30049 字节与首尾内容。按需生命周期也顺带实证：开关开着不拉子进程、调用期拉起、面板文案「0 个工具 · 待发现工具」。

**M87 审计行格式 — PASS**。水位后 41 行切片全量分类：单一结构化形态 `[ts] (LEVEL \| )?事件 \| kv`（`py_exec` 三行为「无级别结构化行」合法形态，出自已桥接 write_event 的白名单存量调用点）；freeform_freeze 守卫 + 白名单在位；工具失败 WARN 镜像 `error.log` 实证（read_text_file ENOENT 同步出现在两处）；探针 report 对其覆盖域解析正常（注：report 的域列表本就不含 `chat./llm.` 等域，属探针口径非格式问题）。

### T2 档

**M88 气泡限宽 — PASS**。长回复（截断转述那段）与用户气泡右边界对齐（截图量得 ≈446px 同线），不通栏、不伸到头像下。

**M89 参数被拒不降级 — FAIL（发现 F1）**。诱导塞非法 due（「下个月初」原样提交）：turn 0 `create_task` 被闸拒绝（原文「截止时间格式必须是 YYYY-MM-DD 或 YYYY-MM-DDTHH:mm」）→ **turn 1 立即丢参重试**（`{"title":"闸拒绝测试卡"}` 无 due）建卡成功。缓解面同样在案：模型随后如实转告闸拒绝原文、明说「没有替你换算成任何具体日期」、自检自评「未达预期」、给用户三选项（补 due/留空/删卡）——但「禁止丢参降级重试」的提示词纪律没拦住第一跳。另登记口径修正：闸实际接受 `YYYY-MM-DD` **或** `YYYY-MM-DDTHH:mm` 双形态（`db/tasks.rs:765`），卡片「只认 T 形」的表述过时；此前模型自行把「下个月初」换算成 `2026-11-01` 建卡属于合法值非降级（用户没拦，不算 FAIL）。

**M90 两条自检不串味 — PASS**。终稿族：`final_selfcheck.fail`（M75 那轮 121 行/文件的终稿复审，带 detail 全文，WARN 镜像 error.log）；工具族：`tool_selfcheck.sidecall`。两族事件名零交叉；终稿 fail 当轮 trace 照常 complete（不阻塞）；侧调模型两次请求语义不同（终稿判文 vs 工具达意，事件 kv 可辨）。

**M91 拆分波抽样 A — PASS**。聊天往返十余轮（含多模态轮、用量条、右侧任务/文件页签）；任务卡写口 UI 完成 ✓（任务池圆钮 → `completed_at` 落库）+ bot 建/删 5 张；技能页/日志页/词元统计页渲染正常；`remember_fact` 写入与跨轮召回 ✓；`error.log` 全会话零 panic。未逐项点验：子任务 UI 增删、标签 UI（10 上限/30 字）、拖拽改列、归档原位封存（诚实清单）。

**M92 拆分波抽样 B — PASS（定时半缺口归 F4）**。设置页「保存配置」一次 → 整份回写**不丢字段**（我在配置里临时加的 `selfcheckModelId` 幸存——前端整份回写没抹掉后端新增键，本批三个新参数同理安全）；分区切换多次（基础/机器人/MCP/本地服务/词元统计来回）无输入丢失、无列表重复；图谱页 58 节点力导向渲染 + 节点详情卡（着色器/材质工厂搬移无恙）；子 agent 派发→动作流→收尾摘要全链 ✓（8 文件 968 行统计正确，报告落盘，`subagent_done | succeeded | 22.6s`）；Enter/Shift+Enter ✓。**缺口**：让模型「建一个 2 分钟后的定时作业」，模型落成 **due 卡**（`due=2026-10-25T18:26`）而非带 plan 窗口的排期卡——到点只触发 `due_notify`（h1/t0 两档都来了）并把卡推到 doing，**没有派生 origin=scheduled 的执行 trace**（库里 scheduled 执行链停留在 10-24 那些 plan 窗口卡）。执行机制本身在（10-24 有 4 条成功记录），是模型工具选形/提示词引导缺口，见 F4。

---

## 三、发现与观察项

### 发现（建议进修复队列，按优先级）

| # | 严重度 | 面 | 内容 | 证据 |
|---|---|---|---|---|
| F1 | P2 | 提示词/行为 | **参数被闸拒绝后模型丢参重试**。`bef13ba3` 只补的「禁止降级重试」纪律拦不住第一跳：拒绝后下一 turn 即无 due 重建卡。缓解面（转告原文/自检未达预期/给选项）在，但用户要求的截止时间已经丢了。建议：把该纪律并进工具拒绝回执的固定尾巴（闸返回值里带「不要丢参重试，转告用户要合法值」），比底座提示词更贴近案发现场 | trace 213 turn0/turn1（§二 M89） |
| F2 | P2 | 技能可见性 | **会话档技能进全局广告面**。路由表正确过滤 `session_scoped`（manage.rs:256），但 `build_skill_block` 只滤 `enabled` 不滤 `session_scoped`（manage.rs:295 起）——B 会话系统提示里能看到名字，模型点名 `use_skill` 才被占用闸拦住。跨会话不劫持的最后一道闸是好的，但名字泄露违背「文件在场但全局不可见」的设计意图（manage.rs 注释原话） | trace 216 turn0；manage.rs 两处口径差 |
| F3 | P2 | 成本/记账 | **截断后每轮重复付摘要 LLM 调用**。历史每轮从 DB 全量重载（dropped 恒 13），`truncate_with_summary_core` 对 `keep_from>0` 无「已摘要」短路，每轮重跑 `summarize_messages`；且摘要走独立 HTTP 路径**不进 `llm.usage` 记账**——长会话截断后每轮多一次隐藏请求。建议：`persist_summary` 已落库，重截断时复用水位（ dropped 集合未变化就跳过重摘要），或至少给摘要加 `llm.usage` 记账 | summary.rs:74-94 无短路；dropped=13×5 轮；usage 每轮仅主请求一条 |
| F4 | P2 | 工具/提示词语义 | **「定时执行」被落成 due 卡，到点不执行**。due 是截止通知语义，执行排期靠 plan 窗口（`plan_start/plan_end`，窗口外不设排期）；模型面对「2 分钟后运行」选了 `create_task`+due，结果只有通知没有执行。建议：create_task 的 schema 示例或提示词点名「要定时执行须建 plan 窗口（task_patch）」，或 create_task 直接收 plan 参数 | exec_traces 无 57e12f… 的执行记录；10-24 scheduled 链在 |

### 观察项（不拦版本，登记备查）

| # | 内容 |
|---|---|
| O-1 | `mode=auto` + 散文体步骤 → 「技能无可执行步骤（DSL 解析为空）」。失败诚实不假成功，但 create_skill 的 schema/弹窗可提示「auto 需要 DSL 步骤块」，否则模型建的技能首跑必败 |
| O-2 | 截止提醒补发文案失真：卡创建时已过 h1 窗口 → h1 立即补发，横幅文案仍写「还有 1 小时截止」（实际剩 2 分钟）。相对时间文案应按实际剩余时间生成 |
| O-3 | `manage_file` 删除走 Finder trash，在密集自动化场景连续两单 AppleEvent 超时（-1712，各 120s）才成功。错误如实上抛 ✓，但超时阈值与重试策略值得看一眼；人工使用不受影响 |
| O-4 | `frontend.js_error: ResizeObserver loop completed with undelivered notifications` 会话期间 31 条（多为自动化连点触发），无伴随功能异常。经典 React 测量噪音，可考虑防抖上报以免淹没 error.log 镜像 |

### 方法论附注（给下一轮执行者）

- 弹窗确认行（`confirm | id:`）是**落判后**才写的——守望弹窗要盯屏幕（像素扫描「确认」实心按钮），盯日志会吃满 60s 超时被默认拒绝。
- 危险操作队列是逐个弹的（逐文件删除 = N 个弹窗），守望要循环消费。
- 页面切换后**必须截图校验**再发消息：设置页子页签与主侧栏同坐标，两次发送点进空处白等。
- `head -c` 按字节切中文会把 3400「字节」当 3400「字」用（实际 ~1133 字），灌历史用 python 写整段。
- 读 `bot.log` 认时间戳：文件从 9 月累积未轮转，tail 会混着昨天的行。

---

## 四、变异注入（V76–V96）

**全部未执行**——用户约束「不改代码」，变异需临时改源码注入。各变异对应的单测侧兜底不变（tool_window.rs / summary 段 / guard.rs / canary.rs 等）；本报告的行为面证据全部来自无注入的真实链路。

| 编号 | 状态 |
|---|---|
| V76–V96 | 未跑（约束放弃），卡片判据的「牙齿」待下一轮带注入授权的复验 |

---

## 五、未覆盖诚实清单

| 项 | 原因 | 兜底 |
|---|---|---|
| W10–W12 Windows 专章 | 本机 macOS | Windows 侧照手册人工跑 |
| M81 注入失败留痕 | 失败窗口工程上打不进（WAL+读路径），两次注入未触发 | memory/mod.rs 留痕分支 + 上层降级链代码审读 |
| M85 反向半（摘 lsof） | `run_lsof` 有 `/usr/sbin` 兜底，破坏需 sudo 动系统 | `bot/services/tests/canary.rs` |
| M78 Anthropic × 开档组合 | MiniMax-M3 anthropic 路径只在默认关下跑了大量轮次，「开+anthropic」组合未专门跑（协议上 anthropic 走 thinking 块与开关无关，风险低） | 协议转换层代码路径 |
| M77 的 50 档位 | 70 与 90 两档已证参数生效，50 档省略 | `clamp 50..=90` 单测 |
| M82 其余四工具首参实证 | 仅 create_task 跑了动态半；ui_type/fetch_url/edit_task/complete_task 靠静态 + 既有失败率统计 | tools_baseline 对拍门禁 |
| M91 子任务/标签/拖拽/归档封存 UI | 抽样卡时间盒限制，UI 主链已过 | db/tasks.rs 写口已被 bot 链覆盖 |
| M92 定时页专屏 | 定时半改由任务卡+通知面覆盖；执行链缺口即 F4 | 10-24 scheduled 执行链记录 |
| 侧调失败回落半（M79） | 侧调全程未失败 | guard.rs 回落分支单测 |

---

## 六、环境操作台账（全部已还原）

| 操作 | 还原 |
|---|---|
| 配置加 `selfcheckModelId`（M79/M90） | 已删 |
| `historyBudgetChars=20000` → 删；`historyTruncateTargetPct=90` → 删；`reasoningEchoEnabled=1/0` → 删 | 收尾确认三键 absent（默认值生效） |
| baseUrl/model/apiProvider 切 DeepSeek（M78） | 还原 MiniMax anthropic 三元组 |
| mcpServers 加 `mini-big`（M86） | 已删，仅剩 builtin-playwright |
| 写 `runtime/flags/api-enabled.flag`（M85） | 已删，重启验证 4763 关闭 |
| 测试卡 5 张（闸拒绝测试卡/下月复盘/冒烟侧调自检卡/冒烟侧调自检卡二/只回复：定时冒烟 OK） | 全部删除（deleted_at 落库） |
| 技能 `bluewhale-leap` | 目录已删 |
| 前端 `dist/` 重打 + 二进制重链接（仅重嵌，无源码改动） | git clean，无需还原 |
| `/tmp` 夹具（biglogs/A-C.md/mini-mcp-big.mjs/守望脚本） | 留在 /tmp（系统自清），未进仓库 |
| 会话痕迹 | 测试会话留在应用内（测试期口径：不做数据清理），机器人写了一条记忆「喜欢的颜色=靛蓝色」如需可手动删 |

**error.log 全会话 +52 行归因**：16 条 tool.return WARN（不存在的文件、闸拒绝、60s 确认超时×5、占用中技能等，全是测试动作的直接产物）+ 2 条 manage_file ERROR（O-3 的 Finder trash 超时）+ 31 条 ResizeObserver 前端噪音（O-4）+ 1 条 llm.response WARN（我切供应商时三键不同步的 404，反证 D-12 说人话路径）+ 1 条 final_selfcheck.fail（M75 轮终稿复审留痕）+ 1 条 memory.extract_failed（收尾验证启动时空摘要，优雅 WARN）。**零 panic、零未归因错误。**

---

## 七、修复台账（报告发布当晚闭环）

四个代码面发现已按基线修复（先测试后修、一个逻辑变更一个 commit、快门禁逐笔绿），行为面已在实机复验：

| 发现 | 修复 commit | 内容 | 实机复验 |
|---|---|---|---|
| F1 丢参重试 | `7ad0a6cd` | due 拒绝回执自带「勿丢参或改填别的值重试——原样转告用户」指引；共享判据 `due_format_error` 不动（task_patch / HTTP 报错语义不变） | PASS：重发同款非法 due，模型被拒后只调 `get_current_time` 给日期建议，明确「没有再发第二次调用」，未再建无 due 卡 |
| F2 广告面泄露 | `69479cd5` | `SkillInfo` 带上 `session_scoped`，两处清单装配共用 `llm_advertisable`（enabled 且非会话档）；设置页列表不受影响 | 单测钉住（扫描传播 + 四态判定）；随修干净启动 |
| F3 每轮重摘要 | `e11e0206` | 进程内按会话存 (dropped 指纹, 摘要) 水位：指纹相同复用不重发请求，历史增长挤进截断区则失配重摘要；命中留 `chat.summary_reused` 审计 | PASS：dropped 7→9→11 渐进期正确逐轮重摘要（指纹失配）；短消息轮 dropped=11 稳定后连续两轮 `chat.summary_reused` |
| F4 定时口头承诺 | `16fa2eac` | `SCHEMA_CREATE_TASK` 的 due 描述写明「due 只触发截止提醒、到点不执行；到点自动执行目前只能用户在定时页建（schedule_reminder 只说话）」，禁止声称 due 卡会自动运行。是否向模型开放定时执行工具属产品能力变更，另行拍板 | PASS：重问「3 分钟后自动运行」，模型改选 `schedule_reminder` 并主动如实说明「只发通知不执行，要自动执行去定时页」 |

复验环境与首轮相同（dev 产物 + vite；budget 临时 20000 测 F3 后已还原）；error.log 复验段新增 4 行均为测试动作产物（闸拒绝 WARN 等）。观察项 O-1～O-4 未在本批处理。

---

## 八、观察项 O-1～O-4 复测（2026-10-26 追加）

执行时间 2026-10-26 21:12 → 22:05，被测版本 `16fa2eac`（与首轮同一版，零代码改动，纯行为面复测）。运行形态 dev 二进制 + vite：**会话开始时原 dev 会话已把 app 与 vite 一并带走**（bot.log 无 app_exit_cleanup，进程双双消失），本批重启 vite + dev 二进制；首启弹「wmessage.bot.v1」钥匙串授权（ACL 拍板项仍在），按既定 `WM_DEV_DISABLE_KEYRING=1` 旁路重启后全程稳定。

### 8.1 逐项结论

**O-1 散文技能首跑必败 — 复现成立，另出两层新发现**

- 动态半：`create_skill` 产出的散文体技能 libai-poem（body 4804 字符，「## 步骤」是自然语言编号清单，frontmatter 无 mode）→ 说触发词「念旧诗」→ 40ms 内 `skill_dsl_empty | body_len: 4804` → 用户气泡「⚠️ 内部错误：技能「libai-poem」无可执行步骤（DSL 解析为空）」——**失败诚实不假成功 ✓**
- 静态半：`SCHEMA_CREATE_SKILL` 只写「步骤用 ## 标题分节」；确认弹窗只写「auto = 命中后不经 LLM 复核直接执行技能步骤」。两处都没说 **auto 需要 `## Step N:` + 工具调用行的 DSL 步骤块**——O-1 原登记成立
- **新增① mode 词表不闭合**：schema 未枚举 mode 合法值，模型自创 `mode: read`（SKILL.md 落盘可证）；`parse.rs:205` 只认 `auto|interactive`，非法值按风险推导**静默归一成 auto**（低风险默认）——确认窗展示 mode 与模型声明不一致，且归到的是最激进的档位
- **新增② 失败归类失真**：首跑失败文案是「**内部错误**：…」，实为技能内容形态与模式不匹配——吓用户且误导归因方向
- 阳性对照：同一会话模型自写的 DSL 技能（pineapple-leap v2，interactive + `query_tasks` 单步）跑通（`skill_step_ok` → `skill_completed | steps: 1`）——失败面特定于「散文体 × auto」
- 修法方向（登记待拍板）：schema 枚举 mode 值并写明 auto 需要 Step 块；create_skill 准入核对「mode=auto 且 DSL 解析为空」提前 denied（现在能建、首跑必败）；失败文案去「内部错误」改面向用户的技能语义错误
- 边缘观察：libai-poem 终止后，下一轮（删除轮）的回复气泡里又冒出一条「技能「libai-poem」已终止：暂停即终止（resumable=false）（共 1 步）」——跨轮的技能状态清扫消息落进了无关轮次，登记备查

**O-2 截止提醒补发文案失真 — 复现成立**

- 建 due=21:28 的卡（21:25:57 创建）→ 21:26:01 H1 补发（实际剩 **1 分 59 秒**），系统横幅逐字：「任务即将截止 / 「O2文案测试」**还有 1 小时截止** (10-26 21:28)」——相对文案与括号内截止时间自相矛盾（截图 `/tmp/mutest/o2/frames/f_212603.png`）
- 对照档正常：21:28:01 T0 横幅「任务已到截止时间 / …截止时间已到 (10-26 21:28)」逐字准确
- 锚点：`due_notify.rs:174`（补发判定复用 H1 档）+ `:198`（「还有 1 小时截止」硬编码）。修法：render() 在补发场景按实际剩余时间生成相对文案
- 顺带再证 F4 口径：due 卡到点只通知不执行；模型建卡后如实回报 due（16fa2eac 的 schema 生效）

**O-3 manage_file 删除走 Finder trash — 温和版复现，完整 -1712 未打出**

- 正常路径：批准后 trash 删除 0.6–1.2s（beta 581ms / gamma 1211ms），`manage_file.delete` 留痕正常
- **温和版实锤：本实例首单删除（alpha）在批准后 `trash::delete` 静默阻塞 ~90 秒**才完成（无报错无进度，用户侧只见工具转圈）——与首轮两单 -1712（各 120s）同源：Finder AppleEvent 延迟不可控、trash 无超时/重试兜底
- Finder AppleEvent 压力环（0.5s 间隔密集列举 home、持续 4 分钟）下删除照常 1.2s——完整 -1712 未在受限压力下复现，维持「密集自动化负载相关」登记
- 弹窗面：60s 超时拒绝 fail-closed 如实留痕（`confirm_timeout` + tool.return WARN）；模型被拒后**未擅自重试**、如实转告（F1 纪律再次成立）
- 驱动坑（方法论补一条）：多个删除做成**并行 tool_calls** 后，确认弹窗是预排队的——点掉前一单后下一单 3ms 内顶上，逐个消费的守望端基线重置会吸掉后续弹窗（本轮守望脚本吃了一亏）

**O-4 ResizeObserver 前端噪音 — 低产复现**

- 结构化压力（约 70 次页面切换 + 30 次窗口尺寸振荡 + 3 轮确认弹窗开闭 + 1 轮流式期间振荡）产出 **4 条** `frontend.js_error: ResizeObserver loop…`（弹窗开闭 2 条 @21:46、流式振荡 2 条 @21:57），**无伴随功能异常**（各页渲染正常、对话正常、零 panic）
- 首轮 31 条来自 4.5 小时连续多驱动自动化；结构化短压力下速率 ≈ 4 条/轮——「防抖上报」建议维持，优先级低

### 8.2 环境台账

| 项 | 处置 |
|---|---|
| vite + dev 二进制 | 会话开始时已被原 dev 会话带走 → 本批重启，**保持运行**（可用状态交还） |
| 钥匙串弹窗 | 拒绝并改走 `WM_DEV_DISABLE_KEYRING=1`（ACL 拍板项仍挂，未动钥匙串） |
| O2文案测试卡 | 已软删（`deleted_at` 直写**整型毫秒**——牢记 Text 型毒行教训） |
| libai-poem / pineapple-leap 技能目录 + AI_Gen_Files 备份 | 已删 |
| o3-alpha/beta/gamma.txt | 已进废纸篓（可恢复） |
| error.log 本轮 +5 行 | 全部归因：4 条 ResizeObserver + 1 条删除超时 WARN；零 panic 零未归因 |
| /tmp 夹具（o2 frames、守望脚本、压力环） | 留系统自清，未进仓库 |
