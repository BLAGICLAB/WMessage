# Agent 四盲区审计与补强（2026-11-21）

基线 `d748e7c1`，四段式（体检 → 三线深审 → 复核 → 分级修复）。这轮补的是 A~J
十维结构性照不到的盲区：**输入面/Prompt Injection、崩溃一致性、留存与增长、
近期改动回审**。深审由三路并行只读代理产出，P1 级发现逐条主会话亲验后才进
修复；修复 10 笔全部带锚点测试，全程全量绿。

## 一、发现总表与处置

### 线 A：输入面（11 条：高 4 / 中 4 / 低 3）

| # | 等级 | 一句话 | 位置 | 处置 |
|---|---|---|---|---|
| 1 | 高 | 子任务回报 digest 以 role=system 注入，Anthropic 转换提到顶层 system 字段（与系统指令同级） | `bot_chat/flow.rs:692-695` + `bot_anthropic.rs:70-74/:373-374` | ✅ 0bbc6de9 降级 role=user + 【系统提示】头（注入前缀族逐出证据池） |
| 2 | 高 | 子 agent 九段提示词零防线 + 任务包装 contextSummary 零截断（recon→executor 投毒链每环无「数据非指令」） | `prompts/subagent.rs` + `bot/subagent_runner/reporting.rs:22-31` | ✅ ddd87c61 SUBAGENT_BASE 禁止段补防线（一处覆盖七 profile） |
| 3 | 高 | EXECUTE 提示词零防线 + 任务卡备注可被早前轮次写入外部内容后由定时执行原文喂给模型（无人值守面） | `prompts/execute.rs` + `bot_chat/exec.rs:481-484` | ✅ ddd87c61 EXECUTE 加红线规则 + 补记忆条款 |
| 4 | 高 | interactive 技能正文整篇（≤51200 字符）零钳制零声明进 system，第三方技能安装弹窗只披露元信息 | `bot_chat/flow.rs:565-569` | ✅ 3a563f60 clip_document_body 两路径同口径钳制 + 数据声明头 |
| 5 | 中 | DSL 失败兜底把每步工具结果前 200 字原文拼进 system（无消毒无围栏无标注；bot_plan 同场景早有三件套） | `bot_chat/input.rs:8-17` | ✅ 5e1ccfe5 sanitize_untrusted_text 两支路共用 |
| 6 | 中 | MCP 返回零包裹 | `bot/mcp/mount.rs:393-430` | ✅ 62b2bb79（A4 漏斗统一包裹覆盖） |
| 7 | 中 | 网页正文 30K 字可驱动桌面六件套（确认弹窗闸在，ui_type/ui_click 无覆盖） | `bot/tools/doc.rs:677-745` | ✅ ddd87c61（红线段点名通道）+ 62b2bb79（包裹） |
| 8 | 中 | 技能 DSL `${stepN.result}` 全文替换可把 30K 正文整段写进下一步 args | `bot_skills/vars.rs:157-176` | 📋 登记不修（拍板：恶意/劣质技能面，信任边界=安装弹窗；引号外原样为 vars.rs 测试自认的登记缺口） |
| 9 | 低 | SYSTEM_PROMPT 整句重复两遍 | `prompts/system.rs:70-71` | ✅ ddd87c61 删一行 |
| 10 | 低 | 自检 snippet（80 字工具结果原文）进 user 消息 | `bot_model_loop/core.rs:1033-1034` | 📋 登记不修（头锚已被 evidence 逐出出处池 + 严格 Y/N，量级有限） |
| 11 | 低 | 执行链记忆块无「背景参考」条款 | `bot_chat/exec.rs:481-484` | ✅ ddd87c61（EXECUTE 红线规则并入） |

### 线 B：崩溃一致性 + 留存（8 条：中 2 / 低 6）

| # | 等级 | 一句话 | 位置 | 处置 |
|---|---|---|---|---|
| 1 | 中 | work_items 全仓无 DELETE、无解除命令：failed 行在「需要人处理」聚合面永久报警且删会话不解（subagents 同操作即随删=ack，work_items 是三源唯一无 ack） | `db/bot_sessions.rs:205-243` | ✅ 删会话事务补级联 DELETE（随本批提交） |
| 2 | 中 | 主聊回合崩溃丢终稿：用户行已落库、终稿行没写，重启后聊天面永久悬空用户行（进程内 Err 有失败行，崩溃路径没有；work_items/trace 两面信号在故不到高） | `bot_chat/flow.rs:715-860` 组合落库链 | ✅ 启动清扫翻态与合成 turn_status=failed 的 assistant 行同事务（随本批提交；全量同事务按拍板不追） |
| 3 | 低 | resume sidecar 孤儿文件 | `bot/subagent_runner.rs:839-851` | 📋 登记不修（判定链双闸兜住正确性——只会响亮拒续不会续错，纯磁盘问题） |
| 4 | 低 | effect_queue give-up 行永久留库 | `db/effects.rs:132-137` | 📋 登记不修（重投豁免已修，剩余纯盘面增长） |
| 5 | 低 | notifications pending 无回收 | `notifications.rs:452` | 📋 登记不修（与 D-2 通知静默同域，随 D-2 拍板一起定） |
| 6 | 低 | turn_record / groups 两个内存只增表 | `bot/turn_record.rs:37` | 📋 登记不修（堆积速率低，回收点在案） |
| 7 | 低 | app_runs / apps output / AI_Gen_Files 无清扫 | `apps/manage.rs:107` 等 | 📋 登记不修（AI_Gen_Files 是用户资产口径；apps output 随程序删除走） |
| 8 | 低 | bot_sessions 子 agent 行每 spawn 一行永留 | `db/bot_sessions.rs:240` | 📋 登记不修（删除会话即随删，无独立报警面） |

### 线 C：22 笔回审（真缺陷：中 1 / 低 1；其余全干净）

逐笔结论（关键笔）：9c486a86 run_subagent 拆九 helper **干净**（8 条早退路径逐
段等价、兜底写口 5→3 有测试锁且语义等价、排队 10-tick 轻连接早于本批非回归）；
badf267a 出口带回累积历史**干净**（15 出口全回工作栈 msgs，7 调用方全解构四元
组）；5c83f382 数值闸重写**干净**（预筛健全性有证明；2 疑点见下）；a 类 6 笔抽
查（含 09c39806 的 21 个测试函数逐体对拍）全干净。

| # | 等级 | 一句话 | 位置 | 处置 |
|---|---|---|---|---|
| M-1 | 中 | a9335ac5 把 bot-tool-done 的 args 从全量换成 80 字 preview：前端「展开可看入参」（500 字渲染契约）与 turn_record 落库面持久退化；与 audit_args_for_log（「审计要看全参数，不能跟着压到 80 字」）口径自相矛盾 | `bot_model_loop/core.rs:1082-1090` + `bot/dispatch.rs:21-29` | ✅ 55cafd18 sink_args_full（URL 型照旧裁剪，其余 500 字）；running 预览维持 80 字 |
| L-1 | 低 | CAP_TOOL_NOTE_LEN_UPPER=96 < 实际说明行 103~112 字节，兜底输出仍超 cap 7~16 字节 | `bot_model_loop/output.rs:151-178` | ✅ 901624aa 提到 128，边界测试按新常量重算 |

疑点登记（不改码，待实机观察）：S-1 句段粒度让开的诚实终稿误伤率（「披露在上
一句、裸数值独立成段」频率只能实机判）；S-2 用户手打【系统背景】开头时整条不进
出处池（fail-closed 方向，概率极低）。

## 二、高危四条的机制要点（修复依据，亲验记录）

1. **digest 提权链**：flow.rs push role=system → `bot_anthropic.rs:70-74` 把数组
   里**所有** role=system 消息抽进 system_blocks → `:373-374` 落请求顶层 system
   字段。子 agent summary 是模型自产文本，而子 agent 读过网页/MCP/文件——投毒
   二阶可达父 agent 系统级。同仓 inject.rs/摘要块早因同一机制改并进 user，digest
   是漏网的同类。
2. **A4 包裹的三条承重约束**（亲验于 evidence.rs:84-86 与 claims.rs:587-597）：
   包裹头撞【系统提示】等前缀 → `usable_source_text` 整条剥除 → 工具真数被幻觉
   守卫误伤；包裹头不剥 → 回声判定 `is_echo_call` 全量失效。修法：前缀常量
   `bot::format::TOOL_OUTPUT_ENVELOPE_PREFIX` 单源，claims `result_section` 先剥
   整行再判；包裹只对 Ok/Warn/Error（Denied 拒文是系统指引）。
3. **三处覆盖硬要求**沿 run_python 联网口径的既定模式：主提示词点名通道、
   SUBAGENT_BASE 一处覆盖七 profile（装配点 BASE+profile 拼接）、工具描述补
   边界句（fixture 已重生成）。

## 三、中危拍板记录

复核后中危 7 条，三条被 3a 覆盖（A#6/A#7/A#11），余四条用户拍板：
**修** A#5、B#1、C M-1；**不修** A#8（登记）；B#2 单独拍板选「启动补合成失败
行」。低危 10 条里顺手修 2（A#9、L-1），余 8 条登记。

## 四、留存面两张清单（已收编进 SOP M 维度）

**有留存**：trace 三表 30d（启动+每日）、overflow 30d、py_runs 1h 龄、
scheduled_job_runs 每作业 20 条、bot_messages 每会话 2000 行、mem_items 500、
app_versions 滚动 100、migration_journal replay 后删、skill_outcomes 键覆盖。

**无留存（登记的增长边界）**：bot_sessions（子 agent 行）、subagents、app_runs、
notifications、effect_queue give-up 行、tag_merge_history（低频）、AI_Gen_Files
（用户资产）、apps output（随删程序）、session_history.json（孤儿，判定链兜住）。

## 五、挂账台账（本轮处置口径：只进台账）

| 项 | 状态 | 一句话 |
|---|---|---|
| D-3 整词边界 | 待拍板 | 宁可放过取向，登记不修 |
| F-2 Windows 侧 | 待实机 | 上轮审计登记项，实机专项时处理 |
| A-1③ SOP 维护 | ✅ 本轮关闭 | K/L/M 三维已收编进 SOP（audit_sop_baseline 绿） |
| D-2 通知静默丢失 | 待拍板 | 三方案在案；notifications pending 回收随它一起定 |
| updater 镜像兜底 | 待拍板 | v0.3.0 首更失败实锤；只救未来版本（端点固化在 exe） |
| 冒烟 8 卡 | 待实机 | 全量版剩余 M204-207 / M217-219 / M228 |
| 双窗实测 | 待立项 | 真源化「随多窗立项补」 |
| F-新5 | 待修 | 11-19 冒烟收官登记项 |
| fetch_url 无目的地白名单 | 拍板不做 | D1 已拍，GET 查询串外传靠提示词+在场可见性兜底 |
| start_service / MCP stdio 无 rlimit | 已知 | 事实表强制缺口清单在案，排 run_python 前面 |
| install_skill 无签名校验 | 已知 | 准入核查意图模式不查代码行为 |
| 技能 ${stepN.result} 长度钳 | 本轮拍板不修 | 信任边界=安装弹窗；引号外原样为登记缺口 |
| 终稿崩溃丢失的全量补偿 | 拍板不做 | 真源化级设计；本轮以合成失败行兜叙事面 |
| S-1/S-2 数值闸疑点 | 待实机观察 | 误伤率实机判后再定是否扩窗口 |
| 崩溃一致性低危五项 | 登记 | sidecar 孤儿 / give-up 行 / notifications / 内存两表 / apps output |

## 六、验收记录

- Rust：cargo test --lib 2446 绿（基线 2435 + 新锚点 11）；clippy -D warnings 绿
- 前端：oxlint 0 errors（50 warnings 均为存量）
- tests-audit 九项对拍 58 过（含 audit_sop_baseline / audit_module_map 行数同步 /
  audit_doc_claims）
- 新增/更新锚点测试 11 条：包裹三锚（声明头在场 / 留证据池 / 固定文案无数字）、
  回声剥头两锚、digest 头前缀锚 + 接线守卫改判、SkillBody 钳制锚、recovery hint
  三件套两锚、sink_args_full 两锚、sweep 合成行锚、级联删除锚
