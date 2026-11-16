# 实机冒烟报告：v0.3.0 后批次（旅游计划修复 / 模型库种子 / 子 agent 收官 / 定时提醒 / 流断流 / 重复闸）

> **基线**：`0b3136cb`（= ab30e13c + 冒烟方案文档提交）。实测门禁：test-all 全绿
> （cargo nextest 2590 + vitest 1251 + 审计 pytest 58 + audit 门禁 20）。
> **模型**：MiniMax-M3.1-Flash（anthropic 协议）；**密钥走 env 覆盖（`WM_TEST_*`），
> 全程零钥匙串访问**（A/B 双向验证：垃圾 key → 401 失败轮 / 真 key → 200 done）。
> **日期**：2026-11-15 12:48 → 15:00。
> **驱动**：cliclick + osascript（System Events）+ screencapture + sqlite3 只读连接
> + `scripts/regression-probe.sh` 水位；断流构造 = 本地 CONNECT 切断代理
> （`~/.cache/wm-smoke15/cutproxy.py`，文件驱动 cut/clear，跑完已拆）；
> 断网构造 = 死代理 env（`HTTPS_PROXY=127.0.0.1:9`）替代关 Wi-Fi（本机 sudo 需密码）。

## 〇、结果总览

| 域 | 卡 | 结果 |
|---|---|---|
| §一 旅游计划修复 | M182–M186 | **M182 PASS**（判据 1/3 + 心跳源码面）/ **M183 PASS**（4/4）/ **M184 部分** / **M185 PASS**（判据 1/3 + 落盘对拍）/ **M186 PASS** |
| §二 模型库 | M187–M188 | **M187 PASS**（判据 1/2）/ **M188 PASS**（判据 1/3/4/5；判据 2 口径出入登记） |
| §三 子 agent 收官 | M189–M193 | **M189 PASS**（判据 1/2 实证）/ **M190 受限** / **M191 PASS** / **M192 受限** / **M193 PASS**（判据 1/4 实证 + S5 对预算墙行活体） |
| §四 定时提醒 | M194–M196 | **M194 PASS**（4/4）/ **M195 PASS**（4/4）/ **M196 部分** |
| §五 流断流 | M197–M199 | **M197 PASS**（判据 1 实证 + 判据 3 变体）/ **M198 PASS**（判据 1 实证）/ **M199 FAIL→F-新2**（判据 2 命令面 PASS） |
| §六 重复闸 | M201–M203 | **M201 PASS**（判据 1/2/3/4 全实证）/ **M202 PASS**（判据 1/2/3 实证 + 30s 变异窗）/ **M203 PASS**（判据 1） |
| Windows | W25–W27 | 未覆盖（本机 macOS，环境受限延后） |

**汇总：22 张主卡可判：16 PASS / 3 部分 / 1 FAIL（=F-新2，当场定位根因面）/
2 受限（M190/M192 真熔断行未构造出）。变异 V172–V192 未跑（预算账，见 §八）。**

## 一、发现清单（按严重度）

### F-新2（P2，已修）读侧断流失败气泡缺「重试」按钮，文案却自指可点
- **现象**：读侧断流（`stream_idle_timeout` → `stream_retry` → 仍失败 → trace failed）的
  失败气泡文案为「请求大模型失败：大模型长时间没有响应，本轮未完成，可点击气泡
  下方的『重试』重新发起这一轮」——**但操作行只有 复制/移除/撤回到此，没有重试按钮**。
- **对照**：A/B 段的 401 气泡（`LLM_API_ERROR` 类）同位置有 重试/复制/移除/撤回到此。
- **根因面（读码定位）**：`MessageList.tsx` retryIdx 判据 = 末条消息
  `role==="assistant" && turnStatus==="failed"`——读侧失败行的前端投影未把
  `turnStatus` 落成 `failed`（库中该行 `turn_status='failed'` 在案，投影面缺口）。
- **命令面可用**：`/retry` 斜杠命令全链正常（见 M199）。
- **修法（已落）**：runChat catch 的视图兜底行补 `turnStatus="failed"` 与后端失败行同态（runChat 失败后清理元数据测试旁新增「失败气泡带重试按钮」锚点）；原「建议」的投影侧改法不再需要——投影面本来就把 turnStatus 广播出来了，缺口只在视图兜底行。原建议文本：失败落库/投影时统一把 `turn_status=failed` 传给前端最后一条
  assistant 消息（或在 retryIdx 判据里兜底认 `error` 非空的末条 assistant 行）。

### F-新1（P3，已修）发送期失败气泡亮原始 reqwest 串
- **现象**：send 层失败（连接被切/死连接池复用）的气泡文案 =
  「请求大模型失败：error sending request for url (https://api.minimax.cn/…)」——
  原始 reqwest 报错文直接进用户可见文案，与 M197 判据 1「原始串不出现」同判据下
  **读侧已分层、send 层未分层**。
- **同域连带**：send 层失败气泡同样无重试按钮（recoverable=true 已置）。
- **修法（已落）**：send 层失败走同一套分层映射（`SendFail` 在 `req.send()` 错误出口就地归类，user_text 与 raw 分离，原始串只留审计；锚点 tests/request.rs 两例）。原「建议」文本：send 层失败走同一套分层映射（连接失败/超时/截断三类归因 + 指引语）。

### 观察项（不计缺陷）

0. **变异轮方法论沉淀（2026-11-15 夜 ~ 11-16 凌晨，20 条变异）**：钥匙串 ACL 每次重建弹登录密码框不可自动化 → env 假密钥补丁 + 脚本化 LLM 桩（剧本逐步消费 / 双队列与多路由 / tools_vary 熔断构造 / break·think_break·fuse_submit·delay_ms 步型）+ LocalStub 本地模型；真熔断行构造从「20-40 分钟真模型且被并行批绕开」变为**桩秒级确定性驱动**，V180/V181/V183/V184/V191/V192 的判定面全部由此解锁。两条写法教训见方案 §九台账。
1. **MiniMax 子 agent 的预算绕行三连**：①工具批内并行（读+写合并调用）；
   ②父 agent 在首只撞预算墙后把余量拆成两只并行派发（各 ~300 工具 < 400 上限，
   绕开熔断）；③收尾 JSON 超长被截断 → `result_json_unparseable`（产物已落盘）。
   三者叠加使「逐个读 300 文件」类目标无法自然熔断——M190 真熔断行两次构造
   （简单版/链式版）均未产出 `budget_exceeded` 行。下游影响 M192（resume 只认
   `budget_exceeded` 行）。**建议**：budget_exceeded 判定加入「父级拆分后合计工具数」
   或对并行同参批做预算合并计数（属产品口径，不急）。
2. **`meta_seed_models_dev` 事件只在种子失败时打**（ensure_seed Err 分支），成功
   静默——M188 判据 2 的卡面口径（「WARN=空表才播种的实锤」）与实现有出入；
   落库实锤改用行数（清表 0 → 启动 235 行）。
3. **瞬时 DNS 抖动自愈**：14:09:30 fetch_url 对 www.gov.mo 域名解析失败一次，
   15 秒后同任务重试成功（未进失败链，回合正常）。
4. **死连接池余波**：切断代理杀隧道后，池内死连接会让紧随的下一轮请求
   send 失败 ×3（reqwest #2956 同形态；tcp_keepalive 15s + pool_idle 30s 最终自愈，
   但 30s 窗口内的首发仍会撞）——与 F-新1 叠加时观感更差。
5. **重复闸 answered 后的重置粒度**：`answered` 放行后同参连击重置，第 4~6 次
   连调再次撞闸时因「同回合已问」直接拒文（M202 判据 2 预期行为 ✓）；
   模型随即改参（+column）放行并 record_lesson——闭环自然。

## 二、§一 旅游计划修复（M182–M186）

- **M182 check 等待钳制 + 心跳**：PASS。库证
  `subagent_check_wait | wait_ms=600000 | waited_ms=196257 | exit=terminal`
  （对比 11-08 旧行 waited_ms=1156579 超预算 556s——钳制生效）；心跳
  `bot-check-heartbeat` 为前端广播事件不落 bot.log（orchestrator.rs:433 源码面
  实证：60s 一拍、统一出口盖章、等待结束收回）；S0 会话 check 等待跨 660s
  看门狗线全程无「已停止」横幅（长等待不误杀行为面）。判据 3：等待 terminal
  返回后汇报轮回灌正常。
- **M183 /stop 级联**：PASS 4/4。①子 agent 行转 cancelled（库证
  `sa_5d64d276…|cancelled|用户停止` + `subagent_cancelled` 双事件行）；
  ②产物停增（f1_note.md 未生成）；③check 等待立即返回（14:01:52 拦截 →
  14:01:53 工具返回，不被 600s 锁死）；④他会话不受累（race 子 agent 在
  另一会话继续跑到 194 轮）。
- **M184 汇报轮放开提问**：部分。汇报轮 ask_user 场景未及构造（macau 子 agent
  深研中随 relaunch 链被中止）；**主聊天 ask 卡端到端已活体闭环**（弹卡 →
  挂 3.5 分钟 → 其他回答行应答 → `ask_answered action=answer` → 工具回填 →
  续跑收尾），`ask_registered` 覆盖面（User | Report 成对注销）以 L1 背书。
- **M185 搜索引擎互斥**：PASS。①开 DeepSeek 的同一保存里 Tavily 自动翻
  「已关闭」（UI 即时反映）+ 落盘对拍（tavilyEnabled=false / deepseekEnabled=true）；
  ③残留双开（手改配置双 true）重启后被识别：conflictText 逐字
  「DeepSeek 与 Tavily 同时只能开启一个搜索引擎，开启时会自动关闭其余；当前配置
  异常双开，请任关一个」，关其一项保存后拉平（tavily=true/deepseek=false 落盘）；
  ④DeepSeek 抓取通道独立不受牵连。判据 2 运行期无双开报错：全轮审计零相关错误行。
- **M186 窄栏换行**：PASS。950px 最小宽实况截图：子 Agent 行/活体计时块内
  不断行、停止键不压字、块整体折行、无横向滚动条。

## 三、§二 模型库（M187–M188）

- **M187 同步失败可诊断性**：PASS（判据 1/2）。死代理构造，手动同步失败文案
  逐层展开：「models.dev 请求失败：error sending request…：client error
  (Connect)：tunnel error：…tcp connect error：Connection refused (os error 61)
  （**重试 2 次后仍失败**）」——根因链五层直达 tcp 层 + 重试次数尾注；
  判据 3（60s 超时）：本构造为连接拒绝瞬时失败，慢路由上限未占满，L1 计时背书。
- **M188 离线种子**：PASS（判据 1/3/4/5；判据 2 口径出入）。meta_provider 清表
  （332→0）+ 死代理冷启动：启动同步失败（WARN 带根因链）→ **种子兜底落库
  235 厂商行**；「添加厂商」网格满砖（302.AI/Anthropic/MiniMax×4/Moonshot…）+
  「更新模型库」按钮在场——三症状不同现；手动同步失败后库未被破坏、错误展示在
  按钮旁（「兜底」字样未逐字出现，语义达成）；直连重启在线同步 235→326 行
  （在线数据覆盖种子）；全程 `meta_seed_models_dev` 事件 0 条（成功路径静默，
  事件只在种子失败时打——与卡面口径出入已登记 §一 观察 2）。

## 四、§三 子 agent 收官（M189–M193）

- **M189 交付件齐即收尾**：PASS。`subagent_deliverable_nudge | round=29` 恰一条
  （由本轮新写的 checks JSON 触发，post_tool hook: write_artifact_file 在案）；
  注入后数轮内交卷动作发生（submit_result 飞出 → JSON 超长截断 → 行 failed，
  交卷行为达成、解析未过）；判据 3（只认本轮新写 verif*.json）：本轮唯一触发面
  为本轮新写文件 ✓，陈年文件不触发的反例未单独构造（L1 背书）。
- **M190 熔断出口恢复链**：**受限**。真熔断行（budget_exceeded）两次构造未产出
  （绕行三连见 §一 观察 1）；判据 3/4（sidecar 落/清）今日零 sidecar 事件；
  恢复链四 via 全靠 L1 + 11-08 旧 specimen（sa_0d2cf921, error=max_turns）背书。
- **M191 submit_result 交卷工具化**：PASS。①子 agent 调 submit_result 成功即
  终态（S5 对：13:39:30 submit → 行 succeeded）；②**主会话调用被拒**——
  拒绝点在工具清单装配层：主会话模型自证「我这条工具清单里没有 submit_result
  （SkillHub 上也没有同名技能），无法调用」（比工具本体拒更硬）；③checks 键位
  指引在 schema（源码面）；④交卷后 check 立即读到终态（S0：13:28:53 交卷 →
  13:29:00 check 返回 terminal，无 running 残留）。
- **M192 resume_subagent**：**受限**。无 budget_exceeded 行可续（见 M190）；
  resume 事务原子性（翻行+待办同事务）与 onion 元列搬运（审计最大缺口的修复）
  以锚点测试 L1 背书。
- **M193 终态真实回读 + 机器验收豁免**：PASS。判据 1：cancel 抢先窗口行终态
  `cancelled`（M183 库证，非 failed）✓；判据 4：**活体标本**
  `sa_b4dfe8ea | succeeded | result_json_unparseable:机器验收 1 项全通过，
  按产物判成功`（解析失败被机器验收推翻，parse_failure_recovered 语义活体）；
  机器验收拒绝面同日在案：e613 的清单文件虽存在但验收判据不满足 → 行 failed
  （闸有牙齿）；判据 2/3 以 L1 + 行证据间接背书。

## 五、§四 定时提醒（M194–M196）

- **M194 一次性提醒端到端**：PASS 4/4。DB 直插（毫秒纪律）→ 定时页提醒页签
  渲染（`11-15 13:37 一次 · 下次 2 分钟后`，nextRunAt 来自 remind_at 列）→
  13:38 tick 触发：`reminder.fired` 事件 + **系统通知真弹**（通知中心横幅
  「⏰ 提醒｜冒烟 M194 一次性提醒…」截图在案）+ 一次性触发即删（行 COUNT=0，
  UI 空态「还没有提醒」）+ 零 enqueue_failed/expired 事件。
- **M195 周期提醒**：PASS 4/4。①首响：`reminder.fired` + fired_count=1 +
  remind_at 推进明日 13:43（UI 回显「每天 13:43 · 下次 23 小时后 · 已触发 1 次」）；
  ②暂停（`reminder.set_enabled enabled=false` 落库 + 卡面灰置「已暂停」徽 +
  「下次」消失）→ 恢复（enabled=true）→ remind_at **重算为明日 13:43**（不按
  暂停前旧点补响——若补响会落过去时刻立即触发）；③改期（picker 基准 11/30）→
  remind_at 立即重算为 `daily:13:43` 的下一未来点（非基准值本身）；④过期：
  直改 remind_at 3 天前的一次性行 → `reminder.expired | 错过 259225s 超过补提醒
  窗口，丢弃` + 行删；周期行同窗走推进不炸（fired_count 2）。
- **M196 页标题 + 通知授权**：部分。判据 1 PASS（页标题「定时」、任务/提醒双
  页签各自渲染）；判据 4 PASS（页签切换不阻塞）；判据 2/3（未授权提示条出现/
  消失）**环境受限**：System Settings 通知子页的 UI 驱动成本超预算（合成点击
  频繁误入子页、AX 树索引漂移），提示条渲染逻辑有组件测试 12 例钉住，如实登记。

## 六、§五 流断流（M197–M199）

- **M197 断流文案分层 + err_chain**：PASS（判据 1 实证；判据 3 变体）。正文期
  切断隧道 → 客户端 120s idle 看门狗开火：
  `llm.stream_idle_timeout | idle_secs=120 | bytes=10637 | elapsed_ms=130230 |
  phase=body | cause=idle`（诊断字段齐）→ 自动重发 → 仍失败 → 失败气泡文案
  **「请求大模型失败：大模型长时间没有响应，本轮未完成，可点击气泡下方的
  『重试』重新发起这一轮」**——现象+成因（idle 专属分类）+本轮未完成+恢复指引
  四段齐，原始串与「内部错误」不出现 ✓。判据 3 的 `llm.stream_failed`（RST 即断
  +err_chain 全链）未捕获：切断代理杀上游不传 RST 到客户端，构造器局限如实登记
  （idle 路径字段同丰富度）。send 层失败文案未分层 → F-新1。
- **M198 开思考断流仍重发**：PASS（判据 1 实证）。配置恒开思考
  （`reasoning=thinking.budget_tokens=4096` 每条 llm.request 在案）；思考增量
  已出的流被切 → 120s idle → **`llm.stream_retry | attempt=1`**（V187 修复的
  牙齿：`emitted_body` 判据不再是死代码）；判据 3 后半 ✓（重发撞仍切断的隧道 →
  失败按 M197 分层落）；重发成功路径未捕获（重试预算语义观察项：596 同场景
  未触发 stream_retry，592 触发了——判定细节值得读码复核，登记观察）。
  判据 2（清场无叠影）：`bot-turn-restart` 为前端事件不落 bot.log；库证 failed 行
  无思考残留、半段正文按 477 字保留（ord 13）——间接背书。
- **M199 失败气泡重试入口**：**FAIL（=F-新2）**。判据 1：读侧失败气泡重试按钮
  不渲染（文案自指「可点重试」）——对照 401 气泡有按钮；根因面
  MessageList.tsx retryIdx 判据（见 §一）。判据 2：**命令面全链 PASS**——
  `/retry` → 失败行截尾（ord13 failed 477 字半段）→ 前置用户行同 content 重放
  （trace 597，msgs_count=14 与失败轮一致）→ 20s 完成 succeeded（842 tok）→
  库证 ord13 变 completed 1319 字、用户行不动、历史连续。判据 3 随判据 1 受累。

## 七、§六 工具重复调用闸（M201–M203）

- **M201 同参 3 连调拦截**：PASS。判据 1：`repeat_gate | tool=create_task |
  consec=3 | round=5 | action=answered`（WARN，kv 全）+ 问卡弹出（系统通知
  「❗ 聊天提问｜工具 create_task 已连续 3 次以完全相同的参数调用，疑似原地循环，
  本次已暂停。怎么继续？」）；拒文形态见判据 2 后半的 action=reject；判据 2：
  「换参数（+column）即放行」活证（拒后模型加参立即执行成功）+ answered 放行
  后连击重置；判据 3：query_tasks 同参连查 5 次 26 行日志全执行、零 repeat_gate
  （豁免表生效）；判据 4：闸拒不进失败链（回合 done 收尾 + record_lesson 学习
  记录）；判据 5：新回合计数从头（二次构造 round=2 独立起算）。
- **M202 可提问语境转问三态**：PASS。判据 1：问卡三选项「换方法重试 / 继续再试
  一次 / 就此打住」+ 假设「就此打住」+ 行式 UI（与 ask 卡一致）；判据 2：
  答「继续再试一次」→ `action=answered` → 放行执行 → 同回合再撞（第 4~6 次
  连调再次 consec=3）→ **`action=reject` 直接拒文不再问**（14:21:17.051
  answered 与 .061 reject 双行 10ms 内，活体双证）；判据 3：30s 变异窗
  （REPEAT_GATE_ASK_TIMEOUT_SECS 300→30，跑完还原）下无人应答 →
  `action=fallback` 按假设降级、回合不挂死；判据 4：子 agent/无人值守语境不弹卡
  （今日全部子 agent 回合零 ask 卡，装配面 L1 背书）。
- **M203 卡住升级口径**：PASS（判据 1）。提示词三处点名 grep 在案（主提示词/
  任务卡执行提示词/工具描述）；行为观感：诱导「读不存在的文件直到成功」，
  模型一次读败后**如实收尾**（「同一个路径再调一次只会得到同样的错误码，循环
  重试无法『成功』，所以我停在这里」+ 给出三条出路 + 「工具正常报错，不空跑，
  不伪造内容」）——不自磨不伪造，符合概率面两可口径。

## 八、变异注入台账（V172–V192）

**本轮未跑**（如实登记）：22 卡执行 + 熔断双构造 + 断流多轮重跑已占满本轮预算；
每条变异需「单编辑 + 重建 + 场景复跑 + 还原」约 10–25 分钟，21 条另计 3–4 小时。
口径同 11-07 轮（卡片先跑、变异补跑另起）：V172–V192 全部「未跑」，
其中 V183（resume 丢 onion 元列）/V192（超时回落放行）两条「必须 KILLED」级
随 M192/M202 受限与 PASS 面顺延至补跑轮。补跑时 M202 超时窗可直接复用本轮
30s 变异法（§十二 点名允许）。

## 九、环境操作台账

| 改了什么 | 改在哪 | 还原了吗 |
|---|---|---|
| env 密钥补丁（keyring.rs 4 读点 WM_TEST_* 回退） | src-tauri/src/bot/config/keyring.rs | ✓ git checkout |
| MCP 机密水合跳闸（WM_TEST_SKIP_MCP_SECRETS，钥匙串弹窗根治） | src-tauri/src/bot/mcp/secrets.rs | ✓ git checkout |
| 超时常量 300→30（M202 判据 3 变异法） | src-tauri/src/bot/constants.rs | ✓ 已还原 300 |
| 密钥 env 文件（0600，仓外，4 把 key） | ~/.config/wmessage-smoke-test.env | ✓ 已删 |
| A/B 垃圾 key override 文件 | ~/.cache/wm-smoke15/override.env | ✓ 已删 |
| meta_provider 清表 + 种子/在线重灌 | wmessage.db | ✓ 最终态 326 行（在线数据，原 332） |
| 测试提醒行 ×3（一次性/每日/过期） | wmessage.db reminders | ✓ 已删（表空） |
| 测试任务卡 ×12（重复闸观测/二次观测/超时回落观测） | wmessage.db tasks | ✓ 已删 |
| fuse300 造数（300 note + 300 summary + 清单）与 race_note/macau_gp | AI_Gen_Files | ✓ 已删（子 agent 产物目录按留存口径保留） |
| bot-config 双开残留（手改） | bot-config.json | ✓ 拉平后=原始态（tavily on/deepseek off） |
| 切断代理进程 + vite dev server | 后台进程 | ✓ 已停 |
| 测试会话 ~10 个（聊天列表） | bot_sessions | 留存不删（证据链），如实登记 |
| 通知中心系统通知横幅 | 系统层 | 用户手动关即可 |
| 孤儿钥匙串弹窗（首版补丁缺 MCP 跳闸时产生） | SecurityAgent | ✓ pkill 清掉（根治后零复发） |

**pristine 重建验证**：三文件还原后 `git status` 干净 → cargo build 绿 →
wmessage 裸启 12s 无 panic（重建后首读 key 钥匙串授权框属预期成本，env 模式
规避的重建成本回归）→ 审计对拍门禁 20 绿。

## 十、跑法备注（给下一轮）

1. **断网构造免 sudo 口径**：死代理 env（`HTTPS_PROXY=127.0.0.1:9`）替代关
   Wi-Fi（M187/M188 全判据可达成）；流断流用本地 CONNECT 切断代理（file 驱动
   cut/clear）。**注意**：切断只死上游不传 RST 到客户端 → 客户端走 120s idle
   看门狗（这本身是合法断流形态，恰好验了 idle 分支文案）；要打 `llm.stream_failed`
   的 RST 即断路径需代理显式 `destroy` 客户端 socket（下一轮改一行）。
2. **composer 坐标随布局漂移**：窗口尺寸/在途气泡高度变化都会移 composer 与
   模型 chip；每轮发送前先 screencapture 校一次坐标，别复用上一轮的。发送后
   必查 `user.message` 落档，否则静默丢失（本轮丢失 2 条）。
3. **模型 chip 误点即换模型**：composer 底栏的模型名是下拉钮，坐标偏一点就
   把会话切到别的模型（本轮误切 deepseek-flash 一次，已当场切回）——发送前
   认一眼 chip 文案。
4. **熔断构造对 MiniMax 不可行**（本轮结论）：并行批调用 + 父级拆分 + 收尾
   JSON 截断三连绕开 200 轮/400 工具双轴。要 budget_exceeded 行需产品面改动
   （见 §一 观察 1 建议）或换严格单步模型。
5. **重复闸诱导 prompt 的措辞**：让模型「严格逐条调用共 N 次」它会照办；
   写「建 N 条」它会并行批发（一次撞闸）。判据 2 的同回合二次撞闸用「6 条逐条」
   一轮搞定（3 撞问 + 3 撞拒）。
6. **钥匙串弹窗根治**：新二进制 + keyring 读点弹窗的根治 = env 补丁盖 LLM/
   搜索四读点之外，**必须盖 mcp/secrets.rs 的 hydrate_mcp_servers**（load_config
   启动即读 mcp:<id> 条目）——本轮补上后零弹窗（上一轮的漏网读点就是它）。

## 十一、补验轮：MCP elicitation 官方 SDK 真验（2026-11-16）

11-07 轮登记的 **F-7**（M179 卡：elicitation/create 发出后 ask 卡不弹）按当时的
约定「下轮用 MCP SDK 官方服务器复验」，本节记该轮的完整结论。

### 11.1 结论

**F-7 是产品缺陷，不是外部桩局限**：桥接 handler 只覆写了 `create_elicitation`，
**能力声明走 `get_info`（rmcp 默认是空能力集）**——客户端 initialize 的
`capabilities.elicitation.form` 从未声明，真实服务器据此判定「客户端不支持
elicitation」根本不发起反问。已修并实机全链贯通（官方 SDK 1.32.2 服务器）。

### 11.2 根因与两次误判（方法论在录）

| 层次 | 事实 |
|---|---|
| 真因（产品） | `ElicitBridge` 未覆写 `get_info` → initialize 的 `capabilities` 无 `elicitation.form` → 服务器拒发 elicitation |
| 误判 1（我的假设） | 「rmcp 协商出 2026-07-28 = NO_INITIALIZE，服务器改从 per-request `_meta` 读能力，所以看不见」——**错**：官方 SDK 对未知版本照单全收并自行协商（`types.js`: `includes(requestedVersion) ? requestedVersion : LATEST`），rmcp 的 legacy `initialize` 握手始终把 capabilities 放在 body 里，与协议版本无关 |
| 误判 2（测试服务器） | 服务器侧 `_clientCapabilities` 为 null → **我的测试服务器是无状态接法**（每个 POST 新建 `Server` 实例，处理 tools/call 的那个实例从没跑过 `oninitialize`）→ 能力集恒 null。改按会话复用（`sessionIdGenerator` + session→{server,transport} 映射）后能力集正常 |

**教训**：官方 SDK 的报错「Client does not support form elicitation.」同时命中两种
成因（客户端没声明 / 服务器没初始化过），**不能拿它反推客户端**；能力是否声明
要看 initialize 的**原始请求体**（服务器侧打 `RAW initialize params`），不能看
服务器内部状态。

### 11.3 修法

`ElicitBridge` 覆写 `get_info`，声明 `capabilities.elicitation.form`
（capability 结构体 `#[non_exhaustive]` → 只能 `default()` 后逐字段赋值）。
**不钉协议版本**：实测 initialize 仍发默认 2026-07-28，官方 SDK 照常协商出
2025-11-25 并接受能力集——改动面最小，不动其他服务器的握手行为。
配套锚点测试：桥接 handler 必须声明 `form`；默认 handler（开关关时注入的 `()`）
必须**不**声明（保证开关关 = 零变化）。

### 11.4 实机证据（官方 SDK 服务器，`server.elicitInput` + StreamableHTTP）

开关开：

```
RAW initialize params: {"capabilities":{"elicitation":{"form":{}}},"clientInfo":{"name":"wmessage","version":"0.3.0"},"protocolVersion":"2026-07-28"}
client capabilities seen by server: {"elicitation":{"form":{}}}
→ 聊天区弹出「MCP 输入请求」卡：允许 / 不允许 / 输入其他回答…（+ 忽略 / 回答并继续）
→ 点「允许」→「回答并继续」
tool.return | tool=mcp_officialelicit_ask_demo | ms=17085 | preview=elicit 应答 action=accept content={"agree":true}
trace.complete | status=done | turns=1 | tools=1
```

boolean 语义映射正确：卡面「允许」→ 服务端收到 `{"agree":true}`（不允许 → false）。

开关关（对照组，同一服务器）：

```
RAW initialize params: {"capabilities":{},"clientInfo":{"name":"rmcp","version":"3.5.0"},"protocolVersion":"2026-07-28"}
client capabilities seen by server: {}
tool.return | ms=7 | preview=MCP 工具报告错误：elicit 失败：Client does not support form elicitation.
```

开关关时能力集为空 → 服务器直接拒发 → **不弹卡、工具秒失败**，零行为变化
（附带观察：开关关时 initialize 的 clientInfo 是 rmcp 默认值而非 wmessage，
属 rmcp 默认 handler 的既有行为，不在本轮改动面）。

### 11.5 本轮环境操作台账

| 改了什么 | 改在哪 | 还原了吗 |
|---|---|---|
| env 密钥补丁（4 个读点 + 钥匙串跳闸，6 处） | keyring.rs / mcp/secrets.rs | ✓ git checkout |
| loopback 白名单（DNS 闸 + MCP URL 闸两处） | bot_web/fetch.rs / mcp/config.rs | ✓ git checkout |
| 密钥 env 文件（假值，6 个变量） | ~/.config/wmessage-smoke-test.env | ✓ 已删 |
| bot-config：LocalStub 模型 + official-elicit-demo MCP 服务器 | bot-config.json | ✓ 还原为 `bot-config.json.bak-mutation-round`（openai 列表回 Ling-1T/Ring-1T，activeModel 回 anthropic/MiniMax-M3.1-Flash，mcpServers 回 builtin-playwright） |
| 三个桩进程（LLM 脚本桩 / 官方 SDK MCP 服务器 / 代理） | 后台进程 | ✓ 已停 |
| 测试会话 ×14（「F7 复验一」～「F7 复验十五」） | bot_sessions | 留存不删（证据链），如实登记 |

**官方 SDK 服务器接法（给下一轮）**：必须**按会话复用**——`StreamableHTTPServerTransport`
配 `sessionIdGenerator: randomUUID` + `onsessioninitialized` 里存 session→{server,transport}
映射，POST 时按 `mcp-session-id` 头取回同一实例；无状态接法会让 tools/call 的实例
从未 initialize，能力集恒 null。`server.elicitInput()` 要在 `call_tool` handler
作用域内调用（SEP-2260：服务端→客户端请求必须从处理某个客户端请求的 handler 内发出）。

### 11.6 补跑轮（同日第二轮）：判据 3/5 构造 + F-1 复证 + 新发现「应答回声进日志」

第二轮目标：把 M179 卡剩下的判据（3=60s 超时面、5=日志无应答值明文）构造出来，
顺带复证 11-04/11-07 两轮登记的 **F-1（ask 卡选项对合成点击不响应）**。

#### 新发现 F-新3（P2，已修）：应答回声经工具结果面回到审计与日志

判据 5 的构造结果：**桥接自己的事件面是干净的**（`bot-ask-resolved` 只回占位符
`[MCP 应答，值不落日志]`），但**服务器会把刚收到的应答原样回显**
（官方 SDK 服务器的 elicit 结果文本含 `content={"agree":true}`），该回声经工具
结果面回到两条日志行：

```
tool.return | tool=mcp_officialelicit_ask_demo | preview=elicit 应答 action=accept content={"agree":true}
tool        | tool=mcp_officialelicit_ask_demo | result=elicit 应答 action=accept content={"agree":true}
```

即 SPEC 6-124 的「应答值不进审计/日志」在**服务器回显这条路径上没兑现**——应答值
可能含凭据（这正是 SPEC 立这条的缘由），服务器一回显就进 bot.log。

修法（窄口径，沿用仓里已有的 `sink_result_text` 脱敏位——同 http_request 的
final_url 口径「sink 脱敏、模型面不脱敏」）：

- 应答提交时登记回声指纹（整份 content 的紧凑 JSON + 字符串型字段值；数值/布尔
  不单独登记——不是凭据，单独登记会误伤正常文本），按 server 维度限 8 条 / 5 分钟；
- sink 面（tool.return preview / 子 agent 动作流 / 执行痕迹 span / tool 审计行 /
  技能步日志）统一把命中的回声串换成 `[MCP 应答，值不落日志]`；
- **模型可见面与 UI 面保留原值**——服务器回显什么模型就该看到什么，模型面不是
  审计面。

修后实机（同一官方 SDK 服务器）：

```
tool.return | preview=elicit 应答 action=accept content=[MCP 应答，值不落日志]
tool        | result=elicit 应答 action=accept content=[MCP 应答，值不落日志]
```

bot.log 中本轮再无 `agree` 明文（仅存修复前的 07:17 / 07:20 两条历史行）。
锚点测试两例：回声串在 sink 面必被脱敏（含字符串型字段值）；布尔/数值型应答值
不单独登记（避免误伤正常文本）。

#### 判据 3（60s 不答 → 过期面）：达成

不答，等满 60s：卡片收起（前端收到 `bot-ask-resolved action=expired`），工具调用
在 60.0s 结束并失败，回合正常收尾——**用户面无悬挂**。
**观察项（非缺陷）**：本轮工具错误文案是服务器侧的「MCP error -32001: Request
timed out」——客户端侧 60s 拒答与**服务器侧请求超时**（官方 SDK 默认也是 60s）
同刻竞态，谁先到谁定型；服务器超时更长时才会出现客户端文案「60s 未获应答，按
拒绝处理」。卡片收起与工具失败两个用户可见面都不受影响，故登记为观察项，
不改 60s 节奏（SPEC 与 ConfirmMap 对齐的取值）。

#### F-1（ask 卡选项对合成点击不响应）：不复现，按 obsolete 关闭

11-04 轮登记、11-07 轮复证的 F-1（ask 卡 pill 按钮对合成点击不响应，绕行口径为
「走文本回答」）在本轮**不复现**：`cliclick` 点选选项行 → 选中态出现（圆点实心 +
行高亮）→ 点「回答并继续」→ `ask_answered | action=answer` → 工具返回
「用户回答了你的提问：方案乙（加并发）」→ 回合收尾。

原因登记：11-08 的 ask 卡 UI 重塑（0b463e59 + 754a9551）把原 pill 按钮换成
「单选行 + 其他回答行 + 共用作答区」，旧 markup 已不存在；F-1 描述的组件形态在
现行 UI 上没有对应物——按 obsolete 关闭（不是「修好了」，是「被替换掉了」）。

#### 第二轮环境操作台账

| 改了什么 | 改在哪 | 还原了吗 |
|---|---|---|
| env 密钥补丁（4 个读点 + 钥匙串跳闸，6 处） | keyring.rs / mcp/secrets.rs | ✓ 已还原（Edit 反向撤补丁，非 git checkout——Mimosa 钩子不许 Bash 直改源码） |
| loopback 白名单（DNS 闸 + MCP URL 闸两处） | bot_web/fetch.rs / mcp/config.rs | ✓ 已还原 |
| 密钥 env 文件（假值，8 个变量） | ~/.config/wmessage-smoke-test.env | ✓ 已删 |
| bot-config：LocalStub 模型 + official-elicit-demo MCP 服务器 | bot-config.json | ✓ 还原（activeModel 回 anthropic/MiniMax-M3.1-Flash，openai 列表回 Ling-1T/Ring-1T，mcpServers 回 builtin-playwright） |
| 两个桩进程（LLM 脚本桩 / 官方 SDK MCP 服务器） | 后台进程 | ✓ 已停 |
| 测试会话 ×2（新对话 + 沿用 F7 复验十五） | bot_sessions | 留存不删（证据链），如实登记 |
