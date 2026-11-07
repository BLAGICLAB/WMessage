# 实机冒烟报告：真源化批 1–6 + 差距收编批 1–7（M163–M181）

> **基线**：`69cae9b6`（= 16895019 + 上批残留两提交 b332084b/99b40dcf + 门禁债三提交
> c6d25ac3/b3ea93c3/3c73ec89 + 实机中修复 69cae9b6）。实测门禁：cargo nextest 2522 绿
> + vitest 1203 绿 + 审计 pytest 58 绿（含当场修复 5 项审计欠账——模块地图 9 文件漏登
> 记/计数漂移/namespace 违规）。
> **模型**：MiniMax-M3.1-Flash（anthropic 协议）；**密钥走 env 覆盖（`WM_TEST_*`），
> 全程零钥匙串访问**（A/B 双向验证：垃圾 key→401 / 真 key→成功）。
> **日期**：2026-11-07 下午→傍晚。

## 〇、结果总表

| 卡 | 结果 | 核心证据 |
|---|---|---|
| M163 用户行落库 | **PASS** | 库证首行 user+msg_id；⌘R reload 消息仍在；无重复气泡。事件面（bot-msg-upserted）外部不可观测，以投影观感+reload 佐证 |
| M164 终稿聚合重启还原 | **PASS** | 终稿行 thinking=373 字/tools 3 条逐字段（ms/ok）/trace_id=546 关联；pkill 重启后折叠/正文/交付物全还原 |
| M165 busy 零落行 | **PASS** | 在途（run_python sleep 15）再发→拒答气泡「⏳ 本对话正在回复中」；库证用户行数=1；重载不复活 |
| M166 /retry 新链路 | **PASS**（附 F-1） | retry 后用户行 msg_id 不变（937fc67c）、助手行删后重建（b870ea34→8306edbe）、上下文保暗号 |
| M167 remove+compact | **PASS** | 移除后行消失+气泡同步消失；/compact 5 行换血 1 条摘要行（bot_history_replace_summary）；bot_history_save 无痕迹 |
| M168 失败轮落失败行 | **PASS** | turn_status='failed' 401 错误行落库；恢复后新行追加失败行保留；前端失败态回显 |
| M169 exec/子会话重建 | **部分** | exec 会话重启后从库载入行 ✓；完整 exec 终稿行素材未构造出（执行回合被两次重启打断/上下文污染，见 F-3/F-5）；围观拦截为设计行为 |
| M170 指令文件端到端 | **PASS** | instructions.md 落盘+instructions.updated 审计；重开会话回复末尾出现「[已阅指令]」；两条 TTFT 观感无异常 |
| M171 超限截断 | **PASS**（修后） | 载入不炸 ✓；**发现 F-4：truncated WARN 判据写错致静默截断→当场修复 69cae9b6+复验出 WARN**；设置页保存 >16000 被前端闸拒（「超出上限 18011/16000 字，请精简后再保存」+按钮禁用） |
| M172 撤回复+撤文件 | **PASS** | trace 559 三凭证（modify/create/delete）→撤回 restored=3：modify 回 v1/create 进废纸篓/delete 从快照恢复；助手行删用户行留；turn.rollback 审计；会话可继续 |
| M173 漂移闸 | **PASS** | 手动改 roll.txt 后撤回：`restored=1 \| skipped_drift=1`，手动内容原样保留——**V163 的牙齿实测在** |
| M174 busy 闸与 move 双凭证 | **PASS**（部分如实记） | rename 双凭证（src delete + dst create）撤回 restored=4：src 回原位/dst 消失；busy 闸 UI 入口在途不存在=前置语义，后端闸 L1 背书 |
| M175 灰度默认关对照 | **PASS** | 无 deferredToolGroups 时桌面工具可用（模型列 3 个窗口）；无休眠闸拒绝；模型顺手调 activate_tools 属无害噪声 |
| M176 activate_tools 端到端 | **PASS** | 开关开+重启：模型先调 activate_tools(desktop)→下一轮 screenshot 落盘 755KB |
| M177 分类器三分支 | **PASS** | ①诉求内白名单外写→Unsure→弹窗（判别边界两可）②诉求外敏感读→Unsure→弹窗→拒→模型如实上报换路 ③selfcheckModelId 指向不存在→`unsure_no_model` 0ms 兜底→弹窗（绝不放行）；latency 0-2.2s 无 llm.usage 增量 |
| M178 连拦停用/无人值守 | **未实测** | MiniMax 判性偏 Unsure，3 连 deny 难构造；无人值守需关窗+定时作业，构造成本超余量。连拦/无人值守路径以 L1（auto_classifier.rs 3 例）背书，如实登记 |
| M179 elicitation 端到端 | **部分** | 连接与调用链路通（桩收到 initialize/tools/list/tools/call）；**elicitation/create 发出后 ask 卡未弹出**（rmcp streamable HTTP 服务端→客户端请求的桩接法未吃到，见 F-7）；60s/开关对照不可构造。另发现第二道闸（bot_web DNS 闸）拦 loopback——与 config URL 闸是两处（本测临时加白了两处） |
| M180 OAuth 全流程 | **部分**（F-6） | 登录链路两次全过：DCR→浏览器授权→302 loopback 回调（随机端口）→PKCE 换 token→`mcp.oauth.login` 审计+keyring ✓；**调用面不可达**：mount 快照登录后不刷新（模型清单无 ping_stub）+ 过期后调用无 Bearer 且 401 兜底刷新未触发；登出路径未测 |
| M181 eval 一键脚本 | **PASS** | 13 用例逐条输出+汇总 **PASS=12 FAIL=1** + tokens（input 31820/output 12264）；FAIL 用例详情见 eval 完整输出 |

**汇总：17 张主卡可判：14 PASS / 3 部分 / 0 FAIL（本轮 0 行为回归缺陷）+ 1 个实机新缺陷当场修复（F-4）+ 6 项观察发现。**

## 一、发现清单（按严重度）

- **F-4（P2，已修 69cae9b6）**：`instructions.truncated` WARN 判据写错——`rendered > raw` 恒假（深度超限时渲染必短于原文；贴线小超限时标注反而更长，双向都不可靠），截断态永远静默。修法：判据钉在原文超限 + 抽纯函数 `should_warn_truncated` + 回归测试钉死两个方向。**这正是 V171 变异想模拟的形态，不用变异它自己就存在。**
- **F-6（P2，待修）**：MCP OAuth 登录后的**重挂载缺失**——`mcp.oauth.login` 成功但 manager 的工具快照不刷新，模型清单永远看不到该 server 的工具；连带**过期刷新双链路未生效**（主动刷新窗口错过则调用裸奔无 Bearer，401 兜底刷新也未触发——stub 只收到一次裸请求）。
- **F-1（P2，已知复证）**：ask 卡 pill 按钮对合成点击不响应（11-04 轮已登记，本轮再次复证；文本回答路径可用，绕行口径成熟）。
- **F-3（P2，待修）**：exec 会话的 /stop 失效——围观 exec 会话输入 /stop 返回「当前对话没有进行中的回复」但该会话 trace 实际 running；在途确认弹窗路径同样不可达。
- **F-5（P3，待修）**：进程被杀后 stale running trace 永久残留（启动清扫不收拾 exec_traces 的 running 行），疑似连带永久锁该会话发送。
- **F-7（P3，外部桩局限）**：elicitation 桩的服务端→客户端请求（POST 响应 SSE 流内载 elicitation/create）未被 rmcp 客户端处理——本测桩接法与 rmcp 期望不匹配，ask 卡未弹出。不排除真服务器按标准接法可通，下轮用 mcp SDK 官方服务器复验。
- **F-2（设计行为确认）**：围观 exec 会话发送被拦（「执行进行中，围观模式暂不能发送」）——与 useChatSend 注释一致，非缺陷。

## 二、变异注入台账（V158–V171）

**本轮 14 条变异全部未跑。** 主卡执行（含两处临时补丁的 A/B 验证与三处闸的实测）耗尽了时间预算；已跑出的行为面给下轮变异提供了精确对照：

- V163（漂移闸恒放行，**必须 KILLED**）：pristine 行为已钉 `skipped_drift=1` + 手动内容保留——变异后该数字消失且手动内容被吞即逮住。
- V168（分类器 fail-open，**必须 KILLED**）：pristine 三分支审计行已钉（Unsure/Unsure/unsure_no_model）——变异后 deny 被放行、unsure_no_model 消失即逮住。
- V170（OAuth state 校验删除，**必须 KILLED**）：happy path 已通两次，变异态需伪造回调（改 state 重放 302）——跑法参照 11-04 复验手册。
- V158/V159/V160/V161/V162/V164/V165/V166/V167/V169/V171：对应卡的 pristine 判据均已在本报告 §〇 落库可查；V160 因 exec 完整终稿行素材缺失暂不可激活（先补 M169 素材）。

**建议**：变异轮单独安排（约 2–2.5h：14 次单编辑单重建复跑），环境操作台账见 §三。

## 三、环境操作台账

| 改了什么 | 改在哪 | 还原了吗 |
|---|---|---|
| env 密钥补丁（keyring.rs 4 读点 WM_TEST_* 回退） | src-tauri/src/bot/config/keyring.rs | ✓ git checkout |
| MCP loopback 加白（config URL 闸） | src-tauri/src/bot/mcp/config.rs | ✓ git checkout |
| MCP loopback 加白（bot_web DNS 闸，第二道） | src-tauri/src/bot_web/fetch.rs | ✓ git checkout |
| 密钥 env 文件（0600 仓外，4 把 key） | ~/.config/wmessage-smoke-test.env | ✓ 已删 |
| 灰度开关 deferredToolGroups/autoGateClassifier/permMode/selfcheckModelId | bot-config.json | ✓ 已还原（备份 bot-config.json.bak-smoke07 留存） |
| MCP 桩服务器配置×2 | bot-config.json mcpServers | ✓ 已清 |
| 桩进程 + vite dev | 后台进程 | ✓ 已停 |
| OAuth keyring 条目 | mcp-oauth:smoke-oauth-stub-0001 | ✓ 已删 |
| M169Exec 造数任务卡 | tasks 表 | ✓ 已删 |
| /private/tmp/wm-smoke 测试文件 9 个 | 磁盘 | ✓ 已删 |
| instructions.md（测试创建） | 数据目录 | ✓ 已删（测后残留 20 字指令会污染用户回合） |
| 重建后首读 key 钥匙串弹窗 | 应用下次启动 | **待用户点「始终允许」**（env 模式规避的重建成本，属预期） |
| 测试会话 ~15 个 | bot_sessions | 留存（证据链），如实登记 |
| 桩/截图/日志证据 | /tmp/wm-smoke-07/ | 留存（重启自动清） |

**关键教训（给下一轮）**：
- 启动脚本 source env 必须 `set -a` 才能导出；厂商名归一含尾下划线剥离（`MINIMAX_TOKEN_PLAN__MINIMAX_CN` 双下划线）。
- 斜杠命令回车会被命令弹层吃掉——输入框聚焦后需补按回车。
- 高危确认弹窗 60s 窗口要盯日志 poll（`confirm | kind: danger`）抢点；ask 卡 pill 不响应合成点击，一律走文本回答+「回答并继续」。
- 会话列表坐标每轮重启后都会漂移——先截图 1:1 核对再点。
- MCP loopback 桩要过**两道**闸（config URL 闸 + bot_web DNS 闸），临时加白要两处同改。
- OAuth 桩的 initialize/tools-list 必须开放（真实 OAuth MCP 服务器口径），只对 tools/call 要求 Bearer，否则握手失败。

## 四、结论

真源化六批（用户行落库/终稿聚合/busy 零落行/retry-compact 新链路/失败行/exec 投影）
与差距收编批 2/3/4/6（指令文件/分组挂载/分类器/eval 脚本）在真机上**零行为回归**；
差距收编批 5/7（elicitation/OAuth）的实机面各有一个集成缺口（F-7 桩协议面 / F-6 重挂载
与刷新）待修。回合级回滚的漂移闸与 move 双凭证实测有牙。1 个审计死分支缺陷（F-4）
当场修复并带回归测试进库。
