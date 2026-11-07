# Agent 代码审计 SOP · 投喂 ZCode 专用

> **运行位置**：WMessage 仓库根目录（`src-tauri/Cargo.toml` / `package.json` 同级）
> **审计对象**：**只审 Agent 核心链路**——Rust 侧 `src-tauri/src/bot*` / `src-tauri/src/py/` /
> `src-tauri/src/prompts/` / `src-tauri/src/bot_fs.rs` / `src-tauri/src/bot_web*`。
> **不审**：前端样式、Tauri 壳、UI 布局、业务 CRUD（DB 层只审「Agent 写进去的那条路」）。
> **硬约束**：每条发现必须带 `file:line` + 原文代码片段（Receipt），**不允许编**。

---

## 0. 这份 SOP 与旧版的差异（先读这段，否则会照着旧版去审不存在的东西）

旧版 SOP 有三处**结构性失效**，照着跑必然产出幻觉：

| 旧版写法 | 仓库现状 | 后果 |
|---|---|---|
| 「三档 turn 预算 60/100/250、硬顶 500」「工具调用 150/250/700、硬顶 1200」「墙钟 300s/900s/2400s、硬顶 3600s」 | 2026-10-28 整套撤销：**无档位、无墙钟**，只剩单一安全网 `MAX_ROUNDS=200` / `MAX_FUNCTION_CALLS=400` | 审计者会把这 12 个「不存在的值」逐条报成「缺失」——12 条假发现 |
| 「核心目录在 `src/`（Rust）和对应 Python 子 Agent 目录」 | Rust 在 `src-tauri/src/`；`src/` 是**前端 TS**。本仓**没有 Python 子 Agent 目录**——Python 只有 `src-tauri/src/py/`（沙箱执行面） | 审计者会去 `src/` 里找 Rust，找不到就编 |
| 「RepoMap 代码索引构建」「AST Inspect / 代码解析」「Verifier / Probe / Validator 子 Agent 入口」 | 本仓**没有 RepoMap**。`ast_inspect` 只解析 **Python**（`src-tauri/src/py/ast.rs`）。Probe/Validator/WriterEditor 是 **per-onion 配置档**，不是代码入口 | 三个「模块不存在」→ 三个假发现 |

**本版的核心修正**：真值不再是「人抄进文档的定稿值」，而是**代码里那一处**。
下面的常量表由 `tests-audit/audit_sop_baseline.py` 逐条与 `src-tauri/src/bot/constants.rs` 对拍，
改代码不改这张表 → 门禁当场红。**SOP 自己不能过期，这是它唯一的长期有效性来源。**

---

## 1. 启动姿势

```bash
# 1. 进仓库根（路径按实际克隆位置替换）
cd /path/to/wmessage

# 2. 确认工作区干净——审计要从一个确定的基线读起
git status --short          # 期望：空
git log --oneline -1        # 记下这个 commit，写进报告头

# 3. 确认机器门禁此刻是绿的（不绿就先修门禁，别审）
bash scripts/test-fast.sh                      # 快检（提交前会跑）
# 一致性对拍：audit_*.py 不匹配 pytest 默认的 test_*.py 收集 glob，
# 整目录形态收 0 条（no tests ran 假绿），必须逐文件点名（与 test-all.sh 同形态）
python3 -m pytest \
  tests-audit/audit_error_codes.py tests-audit/audit_module_map.py \
  tests-audit/audit_tauri_bridge.py tests-audit/audit_doc_claims.py \
  tests-audit/audit_bot_namespace.py tests-audit/audit_file_size.py \
  tests-audit/audit_constants_single_source.py tests-audit/audit_acceptance_matrix.py \
  tests-audit/audit_sop_baseline.py -q
python3 tests-audit/audit_no_eval.py           # 独立守卫脚本（pytest 用例形态之外）
```

**审计阶段只读**——这一轮产出报告，修复是下一轮的事。
不要开 `--full-auto` / `--yolo` 之类的自动写权限；也不要给审计会话挂任何会改仓库的 MCP 工具。
审计者**唯一允许的写**是报告文件本身（建议写到仓库外，如 `/tmp/agent-audit-<日期>.md`）。

---

## 2. 真值基线（机器对拍维护，勿手改）

> 本节所有数字**不是**人抄的定稿值，而是与源码对拍的产物。
> 门禁：`python3 -m pytest tests-audit/audit_sop_baseline.py -q`
> 改 `bot/constants.rs` 而不改本节 → 门禁红（这正是要的行为）。

<!-- sop-baseline:start -->

### 2.1 Agent 风控常量（真值 = `src-tauri/src/bot/constants.rs`）

| 常量名 | 常量值 | 一句话含义 |
|---|---|---|
| `MAX_ROUNDS` | `200` | 轮数安全网；主会话与子 agent **同一个数** |
| `MAX_FUNCTION_CALLS` | `400` | 工具调用安全网；主会话与子 agent **同一个数** |
| `HISTORY_BUDGET_CHARS` | `128_000` | 会话历史字符预算；超预算丢最旧 + 摘要，裁至 70% |
| `HISTORY_TRUNCATE_TARGET_PCT` | `70` | 历史截断滞回目标比例（不贴边裁，保住缓存前缀） |
| `MAX_TOKENS_DEFAULT` | `16_384` | 单轮输出 max_tokens 默认（仅 Anthropic 协议发送） |
| `MAX_TOKENS_CAP` | `32_768` | 单轮输出 max_tokens 硬顶（钳模型条目级输入） |
| `TOOL_OUTPUT_BYTES` | `128 * 1024` | 单条工具回填 payload 全局上限；超限转存文件 + 节选 + 指针 |
| `CONTEXT_CHARS_HARD_CAP` | `200_000` | 单次请求 msgs 总字符硬顶（压缩阶梯的基数） |
| `TOOL_WINDOW_TRIGGER_CHARS` | `24_000` | 轮内工具结果滑动窗触发阈值（累计超它开始裁） |
| `TOOL_WINDOW_KEEP_RECENT` | `6` | 滑动窗保留的最近工具结果条数 |
| `EVIDENCE_OUTPUT_CHARS` | `64 * 1024` | 证据（Receipt）单条输出上限（字符） |
| `LLM_HEADER_TIMEOUT_SECS` | `60` | LLM 首包超时（秒） |
| `LLM_MAX_ATTEMPTS` | `3` | LLM 请求总尝试数（首次 + 重试），429/5xx/网络错误重试至此 |
| `LLM_RETRY_DELAY_MS` | `1500` | 流式重试间隔（毫秒） |
| `HTTP_REQUEST_TIMEOUT_SECS` | `120` | http_request 单次请求超时上限（秒）；saveAs 下载放宽到下一条 |
| `HTTP_SAVE_TIMEOUT_SECS` | `300` | http_request saveAs（下载落产物目录）超时上限（秒） |
| `PYTHON_TIMEOUT_SECS` | `120` | Python 沙箱单次执行超时默认（秒） |
| `PY_MAX_TIMEOUT_SECS` | `600` | run_python 单次执行天花板；与停滞看门狗阈值零余量耦合，两个数必须一起动 |
| `APPS_MAX_TIMEOUT_SECS` | `1800` | apps（程序）单次运行天花板（用户直跑，不走 run_python 钳制路径） |
| `MCP_TIMEOUT_MAX_SECS` | `600` | MCP stdio 单调用超时天花板；与停滞看门狗阈值零余量耦合 |
| `READ_MAX_BYTES` | `100 * 1024` | 文件读取单次上限（字节） |
| `READ_MAX_LINE_CHARS` | `2000` | 单行渲染上限（字符）；超长行原样穿闸会撑爆上下文 |
| `READ_PAGE_TARGET_BYTES` | `56 * 1024` | read_text_file 默认页的**字节**预算（中文页自动窄、英文页自动宽） |
| `READ_MAX_LINES` | `2000` | 文件读取最大行数 |
| `GREP_MAX_HITS` | `50` | grep 最大命中条数 |
| `GREP_MAX_FILE_BYTES` | `2 * 1024 * 1024` | grep 参与扫描的单文件上限（字节） |
| `LIST_MAX_ENTRIES` | `200` | 目录列举最大条目数 |
| `WALK_MAX_DEPTH` | `5` | 目录递归深度上限 |
| `EDIT_MAX_FILE_BYTES` | `1024 * 1024` | 可编辑文件体积上限 |
| `WRITE_MAX_BYTES` | `2 * 1024 * 1024` | 写入文件内容上限 |
| `UNPACK_MAX_ENTRY_BYTES` | `64 * 1024 * 1024` | 归档解压：单个文件解压后上限 |
| `UNPACK_MAX_TOTAL_BYTES` | `256 * 1024 * 1024` | 归档解压：整包解压总量上限 |
| `UNPACK_MAX_ENTRIES` | `5000` | 归档解压：条目数上限 |
| `MAX_EXPANSION_RATIO` | `200` | 归档解压：放大比上限（防 zip bomb） |
| `RATIO_FLOOR_BYTES` | `64 * 1024 * 1024` | 放大比判定地板：解压总量超过它才查比值 |
| `PACK_MAX_ENTRY_BYTES` | `512 * 1024 * 1024` | 归档打包：单个文件上限 |
| `PACK_MAX_TOTAL_BYTES` | `2 * 1024 * 1024 * 1024` | 归档打包：整包总量上限 |
| `PACK_MAX_ENTRIES` | `20_000` | 归档打包：条目数上限 |
| `FUSE_SOFT_WARN_PCT` | `90` | 熔断阶梯第 1 段：软告警阈值（预算占比 %） |
| `FUSE_WRAP_UP_PCT` | `95` | 熔断阶梯第 2 段：收尾窗阈值（预算占比 %） |
| `WRAP_UP_ROUNDS` | `3` | 收尾窗轮数上限（窗口唯一的出口） |
| `WRAP_UP_TOOLS` | `["write_artifact_file", "write_file", "edit_file", "read_own_card"]` | 收尾窗放行的工具白名单：写产物 + 读自己的任务卡 |
| `MAX_REPLANS` | `2` | RePlan 全盘重规划次数上限 |
| `MAX_PLAN_STEPS` | `8` | 单份计划的步数上限 |
| `FINAL_SELFCHECK_ROUND_GAP_DEFAULT` | `8` | 终稿自检的轮次间隔（轮） |
| `FINAL_SELFCHECK_ROUND_GAP_CAP` | `100` | 终稿自检轮次间隔上限 |
| `VERIFIER_RETRY_MAX_CAP` | `2` | Verifier 审计失败后的修复重试预算上限（默认 1） |
| `SEARCH_MAX_RESULTS_CAP` | `10` | 联网搜索单次返回条数上限（硬顶） |
| `SEARCH_MAX_RESULTS_MIN` | `1` | 联网搜索单次返回条数下限 |
| `SEARCH_MAX_RESULTS_DEFAULT` | `6` | 联网搜索未声明条数时的默认值 |
| `ASK_BUDGET_DEFAULT` | `3` | ask_user 单次执行可问次数默认值 |
| `SKILL_DEFAULT_MAX_STEPS` | `100` | 技能步数闸默认值（**仅 auto/DSL 模式**，interactive 不设） |
| `SKILL_MAX_STEPS_LIMIT` | `200` | 技能 frontmatter `max_steps` 能声明到的最大值 |
| `SKILL_MAX_LIFETIME_SECS` | `3600` | 技能绝对寿命上限（秒）：**含 Paused 等确认的时间** |
| `SKILL_BUDGET_WARN_STEPS_DIVISOR` | `10` | 技能预算软警告：剩余步数 ≤ 总步数的 1/10 即提醒 |
| `SKILL_BUDGET_WARN_STEPS_CLAMP` | `(1, 5)` | 技能预算软警告：步数余量阈值钳制区间 |
| `SUBAGENT_MAX_GLOBAL_RUNNING` | `3` | 子 agent 全局并发上限 |
| `SUBAGENT_MAX_PER_SESSION_RUNNING` | `2` | 同一主会话同时运行的子 agent 上限 |
| `SUBAGENT_CHECK_WAIT_MS_CAP` | `600_000` | check_subagent 单次等待上限（毫秒） |
| `SUBAGENT_STALL_TIMEOUT_SECS` | `600` | 子 agent 停滞超时（秒）：子 agent 唯一的时间维度 |

### 2.2 结构计数（真值 = 代码，机器对拍）

<!-- sop-counts:start -->

```
工具总数 = 67
调度早退点数 = 8
子 agent profile 档数 = 3
内置 onion 配置档数 = 3
```

<!-- sop-counts:end -->

- **工具总数 67** = `src-tauri/src/bot/registry/schemas.rs` 的 `TOOLS_TABLE` 条目数
  （= `SCHEMA_*` 常量数；含组挂载元工具 `activate_tools`）。`read_own_card` / `write_artifact_file` 仅子 agent 可见，不进主 agent 清单。
- **调度早退点数 8** = `src-tauri/src/bot/dispatch.rs` 生产段 `ToolResult::denied(` 的出现次数。
  由 `src-tauri/src/bot/dispatch/pre_tool_hook_tests.rs` 的计数断言锁住（断言值 8）。
- **子 agent profile 档数 3** = `research` / `coder` / `general`（`bot/registry/schemas.rs`）。
- **内置 onion 配置档数 3** = `probe` / `validator` / `writereditor`
  （`src-tauri/resources/onions/*.yaml`）。

<!-- /sop-baseline -->

---

## 3. 真实模块地图（**已核实**；审计者照此走，不要自己猜路径）

> 路径全部来自 2026-10-31 的实际仓库状态。行数会漂，**路径不会**。
> 若某条路径在审计时已不存在 → 那是仓库变了（好事），报「文档漂移」而不是硬编。

### 3.1 Agent 核心（审这里）

| 关注面 | 真实路径 | 行数 | 看什么 |
|---|---|---|---|
| 风控常量 | `src-tauri/src/bot/constants.rs` | 264 | 单一真相源；所有限额 |
| 参数注册表 | `src-tauri/src/bot/params.rs` | 181 | 已缩表到三行；**不再**是风控来源 |
| 工具调度 + 闸序 | `src-tauri/src/bot/dispatch.rs` | 984 | 8 级闸序、8 处早退、审计事件 |
| 闸序测试 | `src-tauri/src/bot/dispatch/pre_tool_hook_tests.rs` | — | 早退点计数断言、hook fail-open 断言 |
| 工具表 + schema | `src-tauri/src/bot/registry/schemas.rs` | 1226+ | 67 个工具（含 activate_tools）、profile 白名单、claims_patterns |
| 声称判定 | `src-tauri/src/bot/registry/claims.rs` | — | `claims_mutation` / `claimed_executed_tools` / `mentioned_tool_names` / `claimed_doc_paths` |
| Hook 分层 | `src-tauri/src/bot/hooks.rs` | 1058 | L4/L5/L6；层序不可配；L4 只读 |
| per-onion 配置 | `src-tauri/src/bot_onions.rs` | 1021 | `effective_tools` 交集收窄；配置档落盘 |
| 内置 onion 档 | `src-tauri/resources/onions/{probe,validator,writereditor}.yaml` | — | `allowed` 是**收窄**清单 |
| 模型主循环 | `src-tauri/src/bot_model_loop/core.rs` | 1159 | 轮次推进、工具循环 |
| 幻觉守卫 | `src-tauri/src/bot_model_loop/guard.rs` | 707 | 自检、证据收集、判决 |
| 证据层 | `src-tauri/src/bot_model_loop/evidence.rs` | 95 | `number_sourced` 数值溯源 |
| 熔断 | `src-tauri/src/bot_model_loop/fuse.rs` | 163 | 收尾窗判定、轮数熔断 |
| 回灌漏斗 | `src-tauri/src/bot_model_loop/output.rs` | 170 | per-tool 预算 × 全局 cap 取小；转存指针 |
| 上下文装配 | `src-tauri/src/bot_model_loop/context.rs` | 255 | 压缩阶梯接线、滑动窗 |
| 历史压缩 | `src-tauri/src/bot/compaction.rs` | 486 | 软压 70% / 硬压 85% 两档 |
| 溢出转存 | `src-tauri/src/overflow.rs` | — | 转存、指针文案、保留期清扫 |
| 子 agent 派发 | `src-tauri/src/bot/subagent_spawn.rs` | 425 | profile 解析顺序、并发闸、去重 |
| 子 agent 执行 | `src-tauri/src/bot/subagent_runner.rs` | 1026 | 停滞看门狗、工具 in-flight 标记 |
| 子 agent 摘要 | `src-tauri/src/bot/subagent_digest.rs` | — | 交接摘要渲染 |
| 子 agent 修复 | `src-tauri/src/bot/subagent_repair.rs` | 449 | 失败重试路径 |
| 子 agent 工具面 | `src-tauri/src/bot/subagent_tools.rs` | 424 | 递归禁用、工具白名单、stage 自动挂载配置档 |
| 工具级闸 | `src-tauri/src/tool_guard.rs` | — | 任务卡执行流、子 agent 会话上下文 |
| 文件工具 | `src-tauri/src/bot_fs.rs` + `bot_fs/{read,edit,receipt}.rs` | 528 | 路径闸、读写上限、变更凭证 |
| 联网工具 | `src-tauri/src/bot_web/{destination,http,fetch,search}.rs` | — | 公网闸、目的地闸、SSRF 防护 |
| Python 沙箱 | `src-tauri/src/py/runtime.rs` | 1485 | rlimit、探测、进程组连坐杀 |
| Python 出网闸 | `src-tauri/src/py/seatbelt.rs` | 175 | macOS `deny network*`、自证探测、fail-closed |
| Python 并发闸 | `src-tauri/src/py/gate.rs` | 121 | 有界并发信号量（许可 2）+ EXITING 复查 |
| AST 探查 | `src-tauri/src/py/ast.rs` | 954 | **仅 Python**；只解析不执行 |
| 提示词 | `src-tauri/src/prompts/{system,subagent,execute,planner,selfcheck}.rs` | — | 三处覆盖硬要求（见 §5.1 F 组） |
| MCP | `src-tauri/src/bot/mcp/` | — | 按需启停、机密、stdio 传输 |
| 技能 | `src-tauri/src/bot_skills/` | — | 准入核、步数闸、`allowed-tools` |

### 3.2 不审（但**引用**得到）

前端 `src/`（TS/TSX）、`src-tauri/src/db/` 的业务 CRUD、`src-tauri/src/ui/`、
`src-tauri/src/api_handlers/`、打包脚本。
例外：DB 层被 Agent 直接写入的表（`bot_sessions` / `subagents` / `trace*` / `bot_history`）
如果 Agent 的写入路径有缺口，那属于 Agent 缺陷，要审。

---

## 4. 第一阶段 Prompt · 仓库体检（先做这一步）

**目的不是「让 ZCode 探索」，是让它用 §3 的地图去**验证**。**
自己探索 = 幻觉的来源；照地图核对 = 事实。

```
你是 WMessage 项目的代码审计员。仓库在当前工作目录，Rust 侧在 src-tauri/src/，
前端在 src/（本次不审）。

【任务：对拍下面这张模块地图，逐条确认它在当前代码里成立】

我会给你一份「关注面 → 真实路径 → 该看什么」的表。请对每一行：
1. 确认该文件存在（ls 一下）；
2. 打开它，grep 出 2~5 个关键符号（函数名 / 常量名 / 结构体名）证明你确实读了；
3. 如果文件不存在或该关注面在这个文件里找不到 → 输出【地图漂移】并说明现状，
   不要用「应该是在 xxx」的方式猜一个替代路径。

【硬性规则】
- 所有结论必须来自你 grep / read 出来的真实内容
- 读文件单次最多 500 行，大文件分页
- 找不到就标【未找到】，禁止凭 WMessage 这个名字脑补
- 不要读完整大文件；先 grep 定位符号，再按行号读片段

【地图】
（把 §3.1 那张表原样贴进来）
```

**输出要求**：一张三列表格（关注面 / 路径是否成立 / 关键符号及行号）+ 一段【地图漂移】清单。

---

## 5. 第二阶段 Prompt · 深度审计（贴在体检结果之后）

### 5.0 通用规则（每次审计都要先念）

```
【发现格式】每条发现按此输出，缺一栏视为未完成：

### 发现 #N
- 风险等级：[阻断 / 高 / 中 / 低]
- 位置：src-tauri/src/<path>:<行号>（多个位置就列多个）
- 代码原文：贴 3~15 行**逐字**原文（Receipt，不要改写、不要重排）
- 问题：一句话说清楚这是什么缺陷
- 触发条件：什么输入 / 什么路径会踩到（要具体到「哪个工具 + 什么参数」）
- 修复建议：一句话
- **反证**：如果这条是误报，你打算怎么被反驳？（对照 §6 假阳性清单逐条排除）

【等级判据】
- 阻断：安全闸可被绕过 / 预算可被绕过 / 数据会被写坏 / Agent 会崩
- 高：闸在但有缺口（fail-open 被当成 fail-closed）、权限可被放宽
- 中：错误处理会吞掉真实失败、状态不一致、竞态
- 低：可读性、冗余、注释与实现不符

【禁止】
- 找不到问题的维度也要写一句「本维度未发现阻断级问题，抽查了 X/Y/Z」——
  不要默认全绿，也不要为凑数报无关紧要的事
- 遇到「设计如此」的（见 §6 假阳性清单），不要报成缺陷；
  若你认为它是缺陷，必须在「反证」栏写出**为什么现有设计意图不成立**
- 不要改任何文件
```

### 5.1 审计维度（A~J，共 10 组）

逐组过。**每组的具体检查点已写在下面，不要自己扩题**。

---

#### A. 风控常量与单一真相源

真值：`src-tauri/src/bot/constants.rs`（§2.1 那张表就是它）。

- [ ] 上面 55 个常量在 `constants.rs` 里**逐字**存在、值一致？（这是基线自检，不是发现项）
- [ ] **有没有第二份字面量**：`MAX_ROUNDS` / `MAX_FUNCTION_CALLS` / `TOOL_OUTPUT_BYTES` /
      `READ_MAX_BYTES` / `GREP_MAX_HITS` 这些数，在**其他生产文件**里有没有被重新写一遍？
      （`tests-audit/audit_constants_single_source.py` 已守这条——**审计它有没有漏网**，
      而不是重跑一遍。找该门禁覆盖不到的盲区：比如 `#[cfg(test)]` 内联的模块、
      `include_str!` 进来的 YAML、字符串字面量里的数字。）
- [ ] `params.rs` 现在只剩三行。**确认没有别处还在把 `params.rs` 当风控来源**——
      旧版参数卡退役后，有没有残留的读 `PARAMS_TABLE` 拿预算的代码？
- [ ] 熔断阶梯的三段（90% / 95% / 100%）在 `bot_model_loop/fuse.rs` 里真的按**占比**判定吗？
      还是某处按绝对次数？安全网调整时阶梯会不会错位？
- [ ] `WRAP_UP_TOOLS` 白名单里的 4 个工具，在收尾窗内真的**不计配额**吗？
      白名单判定有没有被绕过（大小写 / 别名 / 参数拼写）？
- [ ] `WRAP_UP_ROUNDS=3` 的窗口出口：轮数用尽后是否**真的停**，
      还是仍能通过别的路径再发工具调用？

---

#### B. 调度闸序与早退点

真值：`src-tauri/src/bot/dispatch.rs` 的 `execute_tool_impl`。闸序（**顺序本身是设计**）：

| 序 | 闸 | 位置 | 失败方向 |
|---|---|---|---|
| 0 | `tool.call` 记账 | `dispatch.rs:538` | — |
| 0.5 | L4 `pre_tool` 钩子层 | `dispatch.rs:542` `run_pre_tool_layer` | 钩子返 Err/panic → **fail-open 放行**；只有 `Interrupt` 才拒 |
| 0.55 | 活动技能 `allowed-tools` | `dispatch.rs:570` | 硬拒 |
| 0.6 | per-tool 规则表 | `dispatch.rs:580` `tool_rule_gate` | `deny` 硬拒；`ask` 弹确认，被拒则拒 |
| 2 | Skill 步骤钩子 | `dispatch.rs:587` `skill_on_step` | 熔断 = **Denied**（不是 Warn） |
| 1.6 | 子 agent 会话白名单 | `dispatch.rs:636` | 硬拒 |
| 3 | 入参 JSON 闸 | `dispatch.rs:681` `invalid_args_message` | 硬拒 |

- [ ] 上表 8 级闸在**当前代码**里顺序是否一致？有没有某级被挪位或被跳过？
- [ ] **早退点恰好 8 处**（`ToolResult::denied(` 在生产段出现 8 次）——
      逐处确认每处对应上表哪一级；多一处少一处都是发现。
- [ ] 8 处早退**全部是 `Denied` 不是 `Warn`**。`Warn` 会被 trace 记成「成功」——
      这条正是 `pre_tool_hook_tests.rs` 里 `no_gate_early_return_uses_warn_status` 守的。
      确认那条断言还活着、还在跑（clippy 缺失会让「测试从未被收集」悄无声息）。
- [ ] 每处早退都**配平了 `tool.return`** 吗？漏配 → 统计面板出现悬挂调用。
- [ ] L4 钩子 fail-open 是**有意设计**（`hooks.rs` 模块头纪律 3：钩子不得中断主链路）。
      审计重点不是「它 fail-open」，而是：**钩子 panic 时会不会 panic 到 Agent 主循环**？
      `run_pre_tool_layer` 里有没有 `catch_unwind`？panic payload 有没有进审计？
- [ ] `use_skill` 在 0.55 / 0.6 / 2 三处都豁免——三处豁免是否**一致**？
      漏一处 = 技能切换能绕过技能自己的 allowed-tools。

---

#### C. 权限收窄链（谁能把工具面放宽）

真值：`bot/registry/schemas.rs::profile_whitelist` + `bot_onions.rs::effective_tools` +
`dispatch.rs` 1.6。

子 agent 实际可用工具 = **profile 白名单 ∩ per-onion `allowed`**（交集，只收不放）。

- [ ] `effective_tools` 的交集语义：yaml `allowed` **能不能放宽** profile？
      （代码里 `allowed.is_empty() || …` 表示空 allowed = 不收窄——确认这是刻意的，
      还是「配置看着生效、实际落空」的入口。）
- [ ] profile 解析顺序（`subagent_spawn.rs::resolve_spawn_profile`）：
      显式 `profile` > onion 的 `profile:` > `stage` 派生。
      **运行时 profile 优先于 yaml 的 profile**——让 yaml 覆盖运行时 profile 就是放宽。
      确认这条没被改反。
- [ ] 三个 profile 的工具面里，`research` 有没有混进写文件/执行类工具？
      `coder` 有没有混进联网工具？（`research` 应当无 `edit_file`/`write_file`/`run_python`。）
- [ ] **递归禁用**：子 agent 调 `spawn_subagent` / `check_subagent` / `cancel_subagent`
      是否一律拒？在哪一层拒的（dispatch 1.6 还是别的）？
- [ ] 子 agent 拿到的工具里，有没有**比父会话多**的？（权限放大）
- [ ] 技能 `allowed-tools` 与 onion `allowed` 与 profile 白名单，三者叠加后
      会不会出现「任一层认为允许就放行」而不是「全部允许才放行」？

---

#### D. 证据链与幻觉守卫

真值：`bot_model_loop/guard.rs` + `bot_model_loop/evidence.rs` + `bot/registry/claims.rs`。

**注意术语**：本仓的 "Receipt" 有两个不同的东西，别混——
`bot_fs/receipt.rs::FileChangeReceipt` 是**文件变更凭证**（diff / sha / before 快照）；
「模型说的数有没有出处」是 `evidence.rs` + `guard.rs` 这条链。

- [ ] `claims_mutation`（声称改了但本轮没动过手）是从 `TOOLS_TABLE` 的
      `claims_patterns` **派生**的，还是又有了一份手写字面量？
      新增 mutating 工具时忘记填 `claims_patterns` 会漏判——查有没有这种缺口。
- [ ] `number_sourced` 的三条「算找到」路径：字面命中 / 四舍五入等价 / 出处里本来就有。
      「四舍五入等价」是给如实转述留的例外——它会不会被用来自证编造？
- [ ] `context_sources` 明确**排除 assistant 消息**（模型自己说的不算证据）。
      确认这条没被放宽——放宽了模型就能给自己作证。
- [ ] `guard_verdict` 的判定分支：有没有「判为可疑但只记日志不阻断」的路径？
- [ ] `claimed_executed_tools` 与 `mentioned_tool_names` 是否都走同一份
      `TOOLS_TABLE` 词表？两处词表分家会怎样？
- [ ] `lookup_receipt`（`bot/tools/trace.rs`）：旧轮工具结果换成桩子后，
      模型能不能真的**取回原文**？取回路径的权限是否收窄到本会话？
- [ ] `EVIDENCE_OUTPUT_CHARS` 截断后，凭证还**可核验**吗？
      （截断在中间 vs 截尾部，可核验性不同。）

---

#### E. 上下文预算与回灌漏斗

真值：`bot/compaction.rs` + `bot_model_loop/output.rs` + `bot_model_loop/context.rs` +
`src-tauri/src/overflow.rs`。

- [ ] **两层预算不要混**：`HISTORY_BUDGET_CHARS`（持久化对话历史裁到多大）vs
      `CONTEXT_CHARS_HARD_CAP`（一次请求连同轮内工具结果最多多大）。
      查有没有把两者当同一个数用的地方。
- [ ] 压缩阶梯：软压 70%（tool 消息留首 200 + 尾 100 字符 + sidecar 指针）、
      硬压 85%（留首 30 + 尾 30 + 指针）。阶梯的**基数是哪个常量**？
      基数写错 → 实际压缩点跟文档说的不一样。
- [ ] 已压缩消息带 `⟦c:…⟧` 标记，软压跳过、硬压再压但**指针必须保留**。
      二次压缩有没有把指针丢了（丢了 → 原文永远取不回）？
- [ ] 回灌漏斗：`per-tool 预算 × 全局 cap` 取小。文档类工具（`DOCUMENT_OUTPUT_TOOLS`）
      按 per-tool 走、不与全局 cap 取小——这个例外是刻意的，确认没被误扩到别的工具。
- [ ] 超限转存（`overflow::spill`）失败时：**转存失败兜底**是什么？
      会不会静默丢内容（模型看到半截结果以为完整）？
- [ ] 转存指针文案的**可达范围**是否说清了？
      （`read_text_file` 只读前 100KB——指针承诺「用 offset 分页读完整」时，
      超过 100KB 的文件后半段读不到，指针文案必须提前说破。）
- [ ] 滑动窗 `TOOL_WINDOW_TRIGGER_CHARS` / `TOOL_WINDOW_KEEP_RECENT`：
      裁掉的是「最旧」还是「最旧但保留一条」？留一条是防模型忘了上一轮做过什么——
      这条如果被优化掉，是真实回归。
- [ ] `overflow` 保留期 30 天 + 启动即扫 + 每日一扫：清扫单轮 panic 会不会让循环退出？
      （停了 = 溢出文件无限堆积。）

---

#### F. Python 沙箱与出网闸

真值：`src-tauri/src/py/runtime.rs` + `seatbelt.rs` + `gate.rs`。

**口径提醒**：本仓「沙箱」只有一个含义——**资源 + 生命周期**维度
（内存 / CPU / 进程数 / 时长 / 输出 / 临时目录 / 进程组连坐杀）。
**不隔离文件系统、不隔离网络（除 macOS seatbelt）、不禁子进程**。

- [ ] `RLIMIT_AS` = timeout×8MB，钳 256MB–2GB；`RLIMIT_CPU` = timeout+10s；
      `RLIMIT_NPROC` = 用户进程基数+64。逐条核。
- [ ] `NPROC` 是**按用户计数**——单个子进程照样起得来（`curl` 能跑）。
      确认「禁子进程」这句话在本仓**没有**被当成已有能力。
- [ ] 超时后进程组**连坐杀**：有没有留下 zombie？
- [ ] `SETRLIMIT_PROBE_SCRIPT` 逐资源探测：探测值取自生产上限（由 argv 传入，
      不写死在脚本文本里）。探测值被拒 → 从严判 Unavailable 留痕。
      查有没有「探测失败就当可用」的路径（静默失效 = 最危险的一种）。
- [ ] macOS seatbelt：(deny network*) 前缀通配（socket 全家含 inbound/bind）。
      **fail-closed 且无放行开关**——闸不证明生效就拒跑。
      查有没有任何一条路径绕过 fail-closed。
- [ ] Windows 侧网络面是**软约束**（提示词），不是强制。确认代码没有在
      Windows 上假装有 seatbelt。
- [ ] **出网红线的三处覆盖**（硬要求，见 §5.1 F 组）：主聊天提示词 + 子 agent profile +
      工具描述。只改一处 = 暗道留着。
- [ ] `py/gate.rs` 的有界并发（许可 2）：等待许可期间的 EXITING 复查有几次？
      排队最坏 2×600s 才拿闸——`/stop` 响应得上来吗？
- [ ] `run_python` 的出网禁令，与 `fetch_url` / `web_search` / `http_request`
      的**可用性**之间有没有被误连（子 agent 档位里 coder 无联网工具——
      联网本属 recon 阶段，这是分工硬依据）。

---

#### G. 子 agent 编排与停滞看门狗

真值：`bot/subagent_spawn.rs` + `bot/subagent_runner.rs`。

- [ ] 并发上限（全局 3 / 单会话 2）是在 **spawn 入口同步强制**，
      还是异步窗口里能被穿透？
- [ ] `resolve_spawn_profile` 的解析顺序（见 §5.1 C）。
- [ ] 去重路径（`subagent_spawn_deduped`）：同一请求重复 spawn 会不会真的合并，
      还是只是记了个日志还跑了两份？
- [ ] **停滞看门狗**（`SUBAGENT_STALL_TIMEOUT_SECS=600`）：子 agent 没有墙钟，
      这是它唯一的时间维度。工具执行期（`run_python` / MCP 单调用同为 600s）
      靠 `tool_guard::set_subagent_tool_in_flight` 挂起判定——
      **零余量误杀窗**是这个标记存在的唯一理由。查标记有没有漏设/漏清
      （漏清 = 工具一返回就误判停滞；漏设 = 工具执行期被误杀）。
- [ ] 看门狗查询失败时按「未取消」处理——查这个 fallback 是不是把
      「查询失败」当成「健康」（假阴）而不是「异常停掉」。
- [ ] 子 agent 失败时，父会话拿到的是**结构化错误**还是 panic 字符串？
- [ ] `subagent_digest` 的摘要是否**真的被父 agent 读到**（没读 = 白跑一轮）。

---

#### H. 联网目的地与内容闸

真值：`bot_web/destination.rs` + `bot_web/http.rs` + `bot_web/fetch.rs` + `bot_web/search.rs`。

- [ ] **公网闸** `check_public_url`：协议 + 主机 + **DNS 全部 IP** 都要公网
      （含环回 / 私网 / 链路本地 `169.254.x`）。查有没有只查主机名的路径。
- [ ] **钉 IP 防 DNS TOCTOU**：解析后连固定 IP，Host 头另设。
- [ ] 重定向**手动逐跳**校验 Location，不跟随自动重定向。逐跳都判？
      https→http 降级拒了没有？
- [ ] **目的地闸**（`http_request`）：产品自有出口 vs 用户目的地分层；
      用户目的地走 `allowed_domains` 白名单 + 白名单外按次弹窗（仅本会话生效）。
      子 agent 与无人值守场景**直接拒**，不弹窗。
- [ ] 目的地闸「每跳都判」——重定向到白名单外时拦得住吗？
- [ ] 凭据处理：`keyring:` / `env:` 前缀的引用真值**不过模型**；
      响应头里的凭据类（`set-cookie` 等）**只回名**。
      查有没有哪条路径把真值回显给了模型。
- [ ] `args_preview` 对 `http_request` 裁成 method+host+path——查询串不进审计面。
      查别的路径有没有把查询串写进审计/日志。
- [ ] `fetch_url` 内容类型闸只收 html/xml/text（JSON/PDF 拒）——这条是刻意的，
      确认没被放宽。
- [ ] **已知缺口（不是缺陷，但要知道）**：`fetch_url` 无目的地白名单
      （2026-10-31 拍板 D1 不做）——GET 查询串外传是**已登记**的缺口。
      审计者不要把它报成新发现；如果它的现状与登记口径不符，那才是发现。

---

#### I. 技能链

真值：`src-tauri/src/bot_skills/` + `docs/SKILL-DSL.md`（DSL 文档）。

- [ ] `create_skill` / `install_skill` 的**准入核** + 用户确认是否都在？
      （历史 P0：create_skill 无确认即改写进程级全局路由表。）
- [ ] 准入核查的是**意图模式**（intents 危险词 / 模式可编译且不过宽），
      不是代码行为——确认没有把「核查意图」写成「核查安全」的假象。
- [ ] 步数闸：`SKILL_DEFAULT_MAX_STEPS=100` **只管 auto/DSL 模式**；
      interactive 不设步数闸（连续失败判死兜着）。
      确认代码里没有把 interactive 也套上闸（那会让交互式技能莫名失败），
      也没把 auto 的闸漏掉（那才是死循环）。
- [ ] `SKILL_MAX_LIFETIME_SECS=3600` **含 Paused 等确认的时间**——
      等确认的技能不判过期就永久占着同名槽位。
- [ ] 技能的 `max_steps` 声明上限 200 是在哪钳的？frontmatter 写 9999 会怎样？
- [ ] 技能熔断在 dispatch 2 段：熔断必须记 `Denied`（不是 Warn）。

---

#### J. Hook 分层与配置漂移

真值：`bot/hooks.rs` + `bot_onions.rs` + `src-tauri/resources/onions/*.yaml`。

- [ ] **层序不可配**：模块头纪律 1——注册表只决定同层内执行序，
      层序由 Rust 调用栈保证。确认 `HookLayer` 里**没有**任何排序字段或入参。
- [ ] **L4 只读**：钩子入参是 `&ToolCallView`，字段全是共享借用，
      类型层面没有改写 `args` 的路径。确认没有 `&mut` 漏进来。
- [ ] 已接线层：`pre_tool` / `post_tool` / `post_turn` 三层都有真实调用点
      （`dispatch.rs` 的 `run_post_tool`、`bot_model_loop/core.rs` 的 `run_post_turn`）。
      确认返回**被消费**——`run_post_turn` 跑了却把结果丢掉，等于钩子改写白做。
- [ ] `HookLayer::from_name` 返回 None = **该层在设计里存在但尚未接线**，
      调用方必须当「没关成」记审计，不能静默当成功。
- [ ] per-onion 的 `hooks: <layer>: ""` = **该层显式关闭**；
      空的 `hooks` map（没写任何层）= 一个层都没关。
      这两种「看起来一样」的形态在代码里分得开吗？
- [ ] 内置 onion 档新增内置工具时不同步核对 `allowed`，
      工具会被**交集悄悄收掉**——这是「配置看着生效、实际落空」最常见的一种。
      查当前 66 个工具 vs 三份 yaml 的 `allowed` 是否有已知的静默收窄。

---

### 5.2 输出格式（第二阶段末尾）

```
- 按 A→J 顺序输出
- 每个维度末尾给一个小结：阻断数 / 高数 / 中数 / 低数
- 最后给一张「按风险排序的 Top 10 修复清单」
- 全程只读，不要改任何文件
- 遇到 §3 地图上的模块在代码里对不上 → 标【地图漂移】，不要自己发明文件名
```

---

## 6. 假阳性清单（**照着这些别报**）

> 审计者最常见的失败不是漏报，是把**有意设计**报成缺陷。
> 下列每一条在仓库里都有明确的设计理由。**要报它，必须在「反证」栏
> 写出「为什么这个设计理由不成立」。**

| 现象 | 为什么是设计如此 |
|---|---|
| L4 钩子返 Err / panic 时**放行** | 钩子不得中断主链路（`hooks.rs` 模块头纪律 3）。真实风险是 panic 逃逸到主循环，不是 fail-open 本身 |
| 受管服务**不继承 rlimit** | 刻意为之：否则 `RLIMIT_AS=480MB` 会让 node 启动即失败。规格明写「不继承 run_python 的 rlimit」 |
| `run_python` **不隔离文件系统** | 本仓沙箱只管资源与生命周期；「沙箱」二字容易被误读成「它被关住了」 |
| `run_python` **能起子进程** | `NPROC` 是限量非禁止；且网络面已由 macOS seatbelt 内核级关掉 |
| Windows 上 py 出网只有**提示词软约束** | seatbelt 是 macOS 机制；Windows 无等价强制面（已知平台差异） |
| `fetch_url` **无目的地白名单** | 2026-10-31 拍板 D1 不做，已登记为已知缺口（GET 查询串外传）。现状与登记口径不符才算发现 |
| `start_service` **无 rlimit** | 见上（同族设计） |
| 子 agent **没有墙钟** | 2026-10-28 预算简化整套撤掉墙钟；子 agent 的唯一时间维度是 600s 停滞看门狗 |
| `params.rs` 只有**三行** | 参数卡退役的既定结果；不是「漏删」 |
| `WRAP_UP_TOOLS` 白名单**不含** `read_text_file` | 收尾窗只放行写产物 + 读自己的任务卡；继续探索没有意义 |
| `SKILL_DEFAULT_MAX_STEPS` **只管 auto** | interactive 不设步数闸是刻意的 |
| `research` profile **不能**写文件 | 只读档，配 `write_artifact_file` 落素材（产物目录），不是「不能产出」 |
| 熔断阶梯按**占比**而非绝对次数 | 安全网调整时阶梯位置自动跟着走，改成绝对数会立刻错位 |
| 历史截断裁到 **70%** 而不是贴边 | 留余量稳住缓存前缀，贴边裁会把缓存前缀断成每轮一次 |

---

## 7. 第三阶段 Prompt · 复核（**新增：旧版没有这一步**）

> 旧版直接从「审计报告」跳到「修复」。中间少一步：
> **让审计者自己先证伪自己的发现**。这一步能砍掉 30%~50% 的假发现。

```
对你刚才输出的每一条发现，逐条做以下复核，并输出复核结论表：

| 发现 # | 复核动作 | 结论 |
|---|---|---|
| #1 | 我 grep 过 <符号>，确认它只在这一处定义 | 成立 / 降级 / 撤回 |
...

复核动作至少做这三件：
1. **回读定位**：用给出的 file:line 打开那一段，确认「代码原文」与文件内容**逐字**一致
   （改写过一个标点 = 伪造 Receipt，撤回整条）。
2. **反向找反例**：grep 同一个符号在**全仓**的其他出现点。
   存在第二处实现 → 你可能只看到了其中一半（降级为「需确认」）。
3. **找调用方**：grep 谁调用了它。看**全部**调用方有没有绕过你担心的那条路径。
   只要有一个调用方是安全的，风险就要降级。

判定规则：
- 三项都过 → 保留原等级
- 有「第二处实现」或「调用方有一条是安全的」→ **降一级**
- 回读对不上原文 → **撤回**
- 撤回与降级的条目，单列一张表说明原因

另外：把你发现的**「设计如此」条目**单列一张表（现象 / 证据 / 为什么是设计），
这表会并入假阳性清单，供下一轮审计者参考。

输出：复核后的最终发现表 + 撤回/降级说明表 + 设计如此清单。
```

---

## 8. 第四阶段 Prompt · 修复轮（审计报告**确认后**再贴）

```
基于已复核的审计报告，按 Top 清单逐项修复。

规则：
1. 一次只修一个发现，修完 self-review 再下一个
2. 每个修复必须保持证据链不破（改了文件操作逻辑要同步改守卫校验规则）
3. 改 bot/constants.rs 时，不要改定稿值本身，只改「代码是否真的在读这些值」
4. 每修一项输出：diff 摘要 + 为什么这样改 + 对应发现编号
5. 全部修完跑：
   - bash scripts/test-fast.sh
   - tests-audit 对拍（逐文件点名，形态同 §1 启动姿势第 3 步）
   把结果贴出来

【本仓硬性约定（违反即返工）】
- 注释不带工单号 / 日期决策 / 拍板记录（只进 DEVLOG）；不画横幅分隔线
- 不吞错误：禁空 catch、禁 unwrap()/expect() 绕错、禁 as any 逃逸类型
- 不留死代码：本次改动孤儿化的函数 / 导入 / 依赖随同一提交删除
- 不加投机抽象：第二个调用方出现前不抽象
- 新文件 ≤1500 行；行为变更必须换测试；修 bug 先写复现测试再修
- 密钥只走系统 keyring / 环境变量
- 提交前人眼看一遍 diff
```

---

## 9. 验收动作（人这边做）

ZCode 出报告后，**机器先过，人再抽查**。

### 9.1 机器验收

```bash
# 1. SOP 自己的基线没过期（本 SOP 的常量表 vs 源码）
python3 -m pytest tests-audit/audit_sop_baseline.py -q

# 2. 报告里引用的事件名在源码里真存在
python3 -m pytest tests-audit/audit_doc_claims.py -q

# 3. 仓库门禁仍绿
bash scripts/test-all.sh
```

### 9.2 人工抽查（3 件事）

| 抽查项 | 怎么验 |
|---|---|
| **Receipt 真实性** | 随机挑 3 条「发现」，`sed -n '行号p'` 打开源码逐字对——**改过一个标点就是伪造**，整条撤回 |
| **没编文件名** | 报告里出现的每个文件，`ls` 一下真的存在 |
| **地图漂移识别对了** | 报告里标【地图漂移】的条目，确认它说的是真漂移而不是自己没找到 |

### 9.3 打断重跑的条件

出现以下任一情况，直接打断：

- 引用了不存在的文件 / 行号对不上 → **它在幻觉**
- 把「没有 unwrap」当结论但没 grep 过 → **它在拍脑袋**
- 把 §6 假阳性清单里的条目报成缺陷，且反证栏没给出「为什么设计理由不成立」
  → **它在制造噪声**，先把 §6 贴给它重跑
- 引用了 §2.1 里不存在的常量（档位 / 墙钟 / 三档预算） → **它在拿旧版 SOP 的记忆审计**
- 绕过只读权限偷偷改文件 → **立刻停**

---

## 10. 本 SOP 的维护规则

这张表会过期。改动下列任一项时**必须同步本文并跑门禁**：

- `bot/constants.rs` 里任一常量的值 / 增删 → 同步 §2.1（门禁会拦）
- `bot/registry/schemas.rs` 的 `TOOLS_TABLE` 条目数 → 同步 §2.2
- `bot/dispatch.rs` 的闸序或早退点数 → 同步 §5.1 B
- `bot/registry/schemas.rs` 的 profile 白名单 → 同步 §5.1 C
- 新增 / 移除审计维度 → 同步 §5.1
- 设计意图变更（某条从「缺陷」变成「设计如此」）→ 同步 §6

执行：

```bash
python3 -m pytest tests-audit/audit_sop_baseline.py -q
```

**同步时只改事实，不改判断**；判断留在方案文档里。
