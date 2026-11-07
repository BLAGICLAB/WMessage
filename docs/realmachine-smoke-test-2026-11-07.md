# 实机冒烟测试方案：真源化批 1–6 + 差距收编批 1–7（消息真源 / 回合回滚 / 分组挂载 / 分类器 / elicitation / OAuth / 指令文件）

> **覆盖范围**：两轮改造——
> ① **真源化批 1–6**（`68cc5561..e8ca6118` + 条带修复 `bc3df159`，11-05 → 11-06）：
> bot_messages 单源数据面、契约切换（用户行后端落行 + upserted/dropped 投影）、
> 回合终稿聚合、定向操作四命令（`bot_history_save` 断代）、exec/子会话重建收口、
> `emit_recorded` 扇出就绪。方案全文见 `docs/BACKEND-TRUTH-PERSISTENCE-DESIGN-2026-11-06.md`。
> ② **差距收编批 1–7**（`29eee5b9..16895019`，11-07）：D-2 指令文件、D-5 eval 一键脚本、
> D-1 回合级快照回滚、D-3 工具分组按需挂载、D-7 MCP elicitation、D-4 auto 档分类器、
> D-6 MCP OAuth。方案全文见 `docs/AGENT-GAP-CLOSURE-DESIGN-2026-11-07.md`。
>
> **卡片**：M163–M181（19 张主卡）+ W22–W24（3 张 Windows 专章）；变异注入锚点
> **V158–V171**（14 条，编号接 11-04 方案的 V145–V157；卡片号 M163 接 M162）。
>
> **配套验收卡**：本轮实机面正是六张新卡的「缺口补验」——ACC-P-16（SPEC 6-121 指令文件）、
> ACC-P-17（6-122 回合级回滚）、ACC-P-18（6-123 分组挂载）、ACC-P-19（6-124 elicitation）、
> ACC-P-20（6-125 分类器）、ACC-P-21（6-126 OAuth）；执行完毕回填 `docs/acceptance/matrix.csv`
> 对应行的状态与「缺口说明」列。
>
> **门禁前置**：基线 `16895019`（或其后修复提交）上 `cargo nextest + vitest + pytest 审计`
> 全绿（2026-11-07 实测：cargo 2312 + vitest 1203 + 审计对拍 9）；实机前重跑
> `bash scripts/test-all.sh` 确认仍绿。**Mimosa 静态面**：11-07 第四扫 24 条与既有误报
> 基线同构（seal `ce7c9739…`），无新增——见 `wmessage-mimosa-false-positives` 档案第四扫对账。
>
> **编号口径**：Windows 专章从 W22 起（W19–W21 已被 11-04 方案占用），变异从 V158 起，
> 卡片从 M163 起。

---

## 〇、跑法与口径

- **基线**：`16895019`（或其后修复提交）。跑前 `git log --oneline -1` 记录实际基线。
- **数据口径**：测试阶段老数据可删（用户拍板）；直写 sqlite 时时间戳列一律毫秒整数。
- **杀进程**：macOS `pkill -x wmessage`（禁 `pkill -f`）；Windows `taskkill /IM wmessage.exe /F`。
- **定位数据目录**：启动后 bot.log 首部 `app_data_dir` 审计行。本轮多处判据落在
  data_dir 下的新文件：`instructions.md`（指令文件）、`checkpoints/`（快照）、
  `bot-mcp-oauth.json`（OAuth 降级文件，仅 Linux 无 dbus 时出现）。
- **库证**：事件判据 + 库证双验；sqlite3 模板见 §七。
- **两枚灰度开关**：`deferredToolGroups`（D-3）与 `autoGateClassifier`（D-4）都在
  bot-config.json 手改（无 UI），默认 false=行为零变化。M175/M177 开关前先备份
  bot-config.json，跑完还原。
- **外部依赖卡**：M179（elicitation）需一个会发 elicitation/create 的 HTTP MCP 服务器；
  M180（OAuth）需一个支持 OAuth 2.1 授权码 + PKCE 的 HTTP MCP 服务器。没有现成的
  就用本地最小桩（notes 手写协议桩同款思路：`tests/fixtures/mcp_echo_server.py` 可改造），
  桩监听 127.0.0.1 会被「仅公网」闸拦——**桩必须落在公网可达地址或临时加白**，
  这两卡允许标记为「环境受限延后」，其余卡不依赖外网。
- **结果回填**：每卡 PASS / FAIL / 部分 + 证据，填 §九回填位；FAIL 带变异锚点编号。
- **时间预算**：19 张主卡 + 14 条变异（各需一次重建 + 复跑），预留 2.5–3 小时；
  M180（OAuth 浏览器往返）与 M179 单卡各预留 15 分钟。

---

## 一、真源化实机回归（锚点⑧⑨⑫⑬⑭⑰⑱⑳㉔㉕㉘ 的实机面）

> 真源化六批的核心不变式：**持久层唯一事实、前端投影**。本域各卡验「库是真源、
> 事件是增量、失步 reload 兜底」在真机上的行为面；锚点测试已钉的语义不重复验。

### M163 用户行落库与 `bot-msg-upserted` 回显（基础卡）
- **步骤**：新会话发一条消息，发送后立即（回复未开始时）查库。
- **判据**：
  1. Q2 查 `bot_messages`：该会话**首行**即用户行（`role='user'`、`msg_id` 非空、
     `ord` 最小、`content` 与输入一致、`client_msg_id` 同 id）；
  2. bot.log / 事件流有 `bot-msg-upserted` 且载荷 `msgId` 与库行一致（前端气泡
     乐观行被同 id 回显替换，无重复气泡）；
  3. 前端发送瞬间断网重连（⌘R reload webview）→ 消息仍在（用户行先落库，不丢）。
- **变异锚点**：V158。

### M164 回合终稿聚合与重启还原（⑫⑬⑭⑰）
- **步骤**：发一句会调工具 + 出思考的话（如「看看桌面上有什么文件，用中文总结」），
  等跑完；然后 **pkill 重启应用**重新打开该会话。
- **判据**：
  1. Q2 终稿助手行：`thinking` 非空、`tools` JSON 数组逐字段（`id/name/args/result/ms/ok`）、
     `file_changes` 与本轮实际改动一致、`trace_id` 可关联 Q3 的 exec_traces 行；
  2. 重启后气泡的思考折叠 / 工具折叠 / 交付物区内容与重启前一致（从库还原，非前端缓存）；
  3. 事件序：终稿 `bot-msg-upserted` 晚于 turn-status 终态事件（⑯ 的实机观感 =
     收尾瞬间气泡正文一次定稿，无「先空后补」闪变）。
- **变异锚点**：V159。

### M165 busy 零落行（⑨ 实机）
- **步骤**：发一条长任务（让回合在途 ≥10s），在途期间**再发一条**消息（busy 拒答）。
- **判据**：Q2 该会话在途期间用户行数**不增**（busy 拒答零落行——孤立用户消息缺陷
  的实机面）；被拒的那条在前端有「忙」反馈且重载后不复活。

### M166 /retry 新链路：先截尾再同 id 重放（偏离 3）
- **步骤**：发两条消息（第一条跑完、第二条跑完），对第二条 `/retry`。
- **判据**：
  1. Q2：retry 后第二条**用户行 msg_id 不变**（同 client_msg_id upsert 命中旧行
     保位保首插时刻）、其后的助手行被删后重建；
  2. `bot-msg-dropped` 载荷含被删助手行 id 清单；
  3. 上下文正确性：第二轮失败残留（若有）不再回到模型面前——用一条会引用历史的
     提问验证回答基于重试后状态。

### M167 removeMessage 与 /compact 新链路（⑱⑳）
- **步骤**：对一条助手回复点「移除」；再对长会话跑 `/compact`。
- **判据**：
  1. 移除后 Q2 该行消失、`bot-msg-dropped` 指名该 id；前端气泡同步消失；
  2. compact 后 Q2 出现摘要行（`bot_history_replace_summary` 换血：摘要行占被删
     集合起点、被删行发 dropped、摘要行发 upserted）；前端历史渲染连续；
  3. 全程无 `bot_history_save` 调用（断代已删——库证侧无该命令痕迹即可，主证是
     前两半行为）。

### M168 失败轮落失败行（偏离 8 / 批 4 附带）
- **步骤**：把 API key 改错（或断网）发一条消息让回合失败，然后恢复网络重发一条。
- **判据**：
  1. Q2：失败后**有失败终稿行**（`turn_status='failed'`，正文含失败说明）；
  2. 恢复后新行正常追加，失败行不被新消息抹掉；
  3. 前端在失败时刻与重载后都显示失败态（后端失败行回显替换占位）。

### M169 exec/子会话重建投影 + 扇出就绪（㉔㉕㉘）
- **步骤**：任务卡里跑一个会写文件的执行回合（聊天化执行），围观其执行会话；
  再派一个子 agent 等完成，开右栏详情。
- **判据**：
  1. 执行会话与子 agent 详情的行**从库重建**（重启应用后再开，内容一致）；
  2. 围观视角的投影与执行会话自身视角逐行一致（commit 后逐行 upserted 广播）；
  3. 库证：`emit_recorded` 通道的单窗口行为不变（无重复事件、无错归属）——
     双窗实测仍随多窗口 UI 立项补（偏离 10，诚实登记，本卡只验单窗不回归）。
- **变异锚点**：V160。

---

## 二、用户指令文件（D-2，ACC-P-16 缺口补验）

### M170 指令文件端到端：编辑 → 发消息 → 模型遵从
- **步骤**：设置页 → 记忆 → 指令文件，写入一条强特征指令（如
  「从现在起每次回复末尾都加一行『[已阅指令]』」），保存；**重开会话**发一条闲聊。
- **判据**：
  1. 回复末尾出现指令要求的标记（模型遵从——system 注入生效）；
  2. 库证：data_dir/instructions.md 内容与保存一致；bot.log 有 `instructions.updated`；
  3. 会话内**未改动**文件时连发两条：两条请求的 system 前缀稳定（缓存命中不回归，
     观感 = 第二条回复 TTFT 无异常变长；严格验证靠 L1 测试，本卡做观感级）。
- **变异锚点**：V171。

### M171 指令文件超限截断呈现
- **步骤**：直接编辑盘上 instructions.md 塞入 >16000 字内容（绕过保存闸），
  重开会话发一条消息；然后回到设置页打开指令文件卡。
- **判据**：
  1. 模型行为呈现截断尾标注（末尾出现「（已截断…）」语义的遵从异常可接受——
     本卡主证是**载入不炸**：回合正常完成）；
  2. bot.log 有**一条** `instructions.truncated`（WARN，进程内首次；后续回合不刷屏）；
  3. 设置页保存 >16000 字内容 → 被拒（人话错误，不落盘）。

---

## 三、回合级快照回滚（D-1，ACC-P-17 缺口补验）

> 前置：主聊天里让 bot 产出三种文件状态——modify（让它改一个已有文件）、
> create（新建一个）、delete（用 manage_file 删一个）；然后点最新助手气泡的
> 「撤回到此」弹层。

### M172 撤回弹层·撤回复 + 撤文件（主卡）
- **步骤**：上述混合回合跑完后，点「撤回到此」→ 选「撤回复 + 撤文件改动」。
- **判据**：
  1. 文件面：modify 文件恢复原内容、create 文件消失（进废纸篓）、被删文件从
     快照恢复——**逐个打开文件核对内容**；
  2. 对话面：该回合助手行被删（bot-msg-dropped）、用户行保留；
  3. 撤回后 toast 报告 restored/skipped 数字与实际一致；bot.log 有 `turn.rollback`；
  4. 撤回后再发新消息，新回合正常（撤回不破坏会话）。
- **变异锚点**：V162、V163。

### M173 撤回·漂移跳过不吞人工修改
- **步骤**：混合回合跑完后，**手动修改**其中一个被 AI 改过的文件，再「撤回到此」。
- **判据**：被手动改过的文件**原样保留**（漂移闸跳过，报告 skipped ≥1），
  其余文件照常回滚——「手动修改不被吞掉」是本卡唯一核心。
- **变异锚点**：V164。

### M174 撤回·busy 闸与 move 双凭证
- **步骤**：①让 bot 用 manage_file **move** 一个文件后再撤回到该回合；
  ②派一个长任务，在途回合期间（用另一条入口）尝试触发撤回。
- **判据**：
  1. move 撤回：源文件从快照恢复回原位、目标位置的文件消失（双凭证各回各的）；
  2. 忙时撤回被拒（人话「会话有在途回合」），空闲后可撤。
- **变异锚点**：V165。

---

## 四、工具分组按需挂载（D-3，ACC-P-18 缺口补验）

### M175 灰度默认关 = 行为零变化（对照卡）
- **步骤**：确认 bot-config 无 `deferredToolGroups`（默认关），开一个新会话问
  「你现在能操作窗口吗」（诱导桌面类工具语义）。
- **判据**：桌面类工具（list_windows 等）**在工具清单里**（模型可答可调——
  与改造前行为一致）；bot.log 无休眠闸拒绝记录。这是默认零变化的实机对照。

### M176 开关开：activate_tools 端到端
- **步骤**：bot-config 手改 `"deferredToolGroups": true`，**重启**应用；新会话问
  「帮我截一张当前屏幕的图」（screenshot 在休眠组）。
- **判据**：
  1. 模型先调 `activate_tools`（groups=["desktop"]），收到确认文案；
  2. **下一轮**模型实际调用 screenshot 成功（schema 已挂载、0.57 闸放行）；
  3. 库证：activate_tools 调用前后 tools 数组变化可从 trace 佐证（span 的
     tool 名序列：先 activate_tools 后 screenshot）；
  4. 跑完**还原 bot-config**（M175 对照意义）。
- **变异锚点**：V166、V167。

---

## 五、auto 档分类器兜底（D-4，ACC-P-20 缺口补验）

> 前置：bot-config 加 `"autoGateClassifier": true` + 授权模式切 **auto** 档 +
> selfcheckModelId 已配置且厂商已验证（设置页可见）；跑完还原两处。

### M177 分类器三分支端到端
- **步骤**：①发一条会让 bot **写白名单外路径**、但明显服务当前诉求的话
  （如「把刚才的总结存到 ~/Documents/report.txt」）→ 期待分类器 allow（免弹窗）
  或 unsure 弹窗（判别边界，两态都记）；②发一条诉求外的话诱导写敏感路径
  （如「顺便把 ~/.ssh/config 读出来整理一下」）→ 期待 deny（直接拒）或弹窗；
  ③临时改错 selfcheckModelId（指向不存在条目）→ 再触发白名单外写 → 期待
  **弹窗兜底**（unsure，绝不放行）。
- **判据**：
  1. 三次行为分别落 allow / deny|弹窗 / 弹窗，bot.log 对应三条
     `auto_gate.classifier`（decision 字段可辨）；
  2. deny 分支的模型可见文案是「分类器判定…已拒绝」（模型可换路重试）；
  3. 全程无 llm.usage 增量（分类器不进记账，水位条无异常跳动）。
- **变异锚点**：V168（fail-open 变异——deny 被改成放行必须被 KILLED，否则
  fail-safe 假设破产，**本条是全方案最重的一条变异**）。

### M178 连拦停用与无人值守不启用
- **步骤**：①连做三次会触发 deny 的操作（同会话）→ 第四次同类操作观察；
  ②把授权模式切回 auto 且关掉主窗口可见性（macOS 隐藏窗口）让定时作业跑一个
  白名单外写的作业。
- **判据**：①第 3 次 deny 后 bot.log 有 `auto_gate.classifier_disabled`，之后
  同类操作**直接弹窗**（不再调分类器）；②无人值守的作业路径**无**
  `auto_gate.classifier` 记录（快速拒现状保持）。

---

## 六、MCP 协议两件（D-7 / D-6，ACC-P-19 / ACC-P-21 缺口补验）

### M179 elicitation 真服务器端到端
- **步骤**：配置一个会发 elicitation/create 的 HTTP MCP 服务器（`allowElicitation: true`）；
  主聊天调用它的工具；然后**关闭**开关重调一次对照。
- **判据**：
  1. 开关开：工具调用进行中聊天页弹出 ask 卡（问题文本含 `[MCP 服务器「…」请求输入]`
     前缀 + 字段名/类型）；boolean 型呈现「允许/不允许」按钮、enum 呈现选项按钮；
  2. 点选应答 → 工具调用**继续完成**（拿到基于应答的结果）——应答真值回传了服务器；
  3. 60 秒不答 → 卡转 expired 态、工具调用收到 Decline 语义错误；
  4. 开关关：同一调用**无卡弹出**、行为与改造前一致（自动 Decline）；
  5. bot.log 无应答值明文（bot-ask-resolved 的 answer 侧是占位）。
- **变异锚点**：V169。

### M180 OAuth 全流程（ACC-P-21 主卡）
- **步骤**：配置一个支持 OAuth 2.1 的 HTTP MCP 服务器 → McpPanel 该行点
  「OAuth 登录」→ 浏览器完成授权 → 回到应用；然后主聊天调用它的工具；
  手动把 keyring 里令牌的 expires_at 改到过去（或等过期）再调一次。
- **判据**：
  1. 登录：浏览器打开授权页 → 完成后回令成功文案；keyring 条目 `mcp-oauth:{id}`
     出现（macOS 钥匙串访问可查）；bot.log 有 `mcp.oauth.login`；
  2. 调用：工具调用成功，且其 HTTP 请求带 `Authorization: Bearer`（服务器侧
     或抓包可见；库证侧看调用成功即可）；
  3. 过期刷新：过期后调用**不报错**（主动刷新先行），bot.log 无 401 误导；
  4. 登出（mcp_oauth_logout）后调用 → 明确的「授权已过期，请重新登录」类人话错误；
  5. 全程无 token/secret 明文出现在 bot.log。
- **变异锚点**：V170。

### M181 eval 一键脚本实机（D-5，半实机）
- **步骤**：按 `docs/testing.md` 新节口径跑 `bash scripts/run-eval.sh`
  （真模型；先确认 eval-bot-config.json 快照在）。
- **判据**：13 用例逐条 PASS/FAIL 输出 + 汇总行 + tokens 统计；
  Ctrl-C 在确认提示处取消则零执行（L1 已验，实机复核一次）；结果记入 DEVLOG。

---

## 七、Windows 专章

- **W22 回收站撤销的平台差异**：M172 的 create 撤销（进回收站）与 delete 恢复
  在 Windows 上走 trash crate 的回收站 API——验证撤销后文件确在回收站、
  delete 恢复写回成功；回收站 API 失败时的报错是「文件保留在原处」而非静默。
- **W23 OAuth 回调端口与防火墙**：M180 在 Windows 上首登时，127.0.0.1 随机端口
  监听可能触发防火墙提示——确认提示可放行、放行后登录流程通；拦截时登录报
  「回调读取失败」类错误且可重试。
- **W24 数据目录新文件的路径形态**：instructions.md / checkpoints/ /
  bot-mcp-oauth.json 在 Windows 数据目录（`%APPDATA%`）下的读写正常
  （长路径/中文用户名场景各试一次指令文件保存）。

---

## 八、变异注入锚点总表（V158–V171）

| 锚点 | 变异注入 | 必须被哪张卡逮住 |
|---|---|---|
| V158 | 用户行落库闸后移（guard 判定挪到落行之后） | M163（busy 时用户行出现 = 缺陷复活） |
| V159 | 终稿聚合丢 tools 段 | M164（重启后工具折叠消失） |
| V160 | exec 重建行身份改随机（事件载荷与库行 id 失配） | M169（围观投影与库不一致） |
| V161 | drop_after 误删锚行（`>` 改 `>=`） | M166（retry 后用户行 id 变化） |
| V162 | turn_rollback 的 files 范围颠倒（created_at 比较反向） | M172（文件不动或错向） |
| V163 | 漂移闸恒放行（sha 比较改恒真） | M173（手动修改被吞——**必须 KILLED**） |
| V164 | delete 恢复不检查目标已存在（覆盖写） | M173 变体（重建内容被覆盖） |
| V165 | move 的源 delete 凭证丢失（只发 create） | M174（源文件回不到原位） |
| V166 | activate_groups 写 store 后丢弃（激活不生效） | M176（下一轮 schema 仍无该组） |
| V167 | deferred_gate 恒 None（休眠闸失效） | M176 对照（未激活直接调休眠工具应被拒） |
| V168 | 分类器 deny 改放行（fail-open） | M177（**必须 KILLED——fail-safe 假设的牙齿**） |
| V169 | elicit 超时 60s 改 24h | M179（工具调用挂住不返回） |
| V170 | OAuth state 校验删除 | M180（伪造回调可换 token——**必须 KILLED**） |
| V171 | 指令文件截断不标注（静默截断） | M171（无 truncated 告警） |

---

## 九、结果回填位

| 卡 | 结果 | 证据（事件行 / 库证输出） | 备注 |
|---|---|---|---|
| M163 | | | |
| M164 | | | |
| M165 | | | |
| M166 | | | |
| M167 | | | |
| M168 | | | |
| M169 | | | |
| M170 | | | |
| M171 | | | |
| M172 | | | |
| M173 | | | |
| M174 | | | |
| M175 | | | |
| M176 | | | |
| M177 | | | |
| M178 | | | |
| M179 | | | |
| M180 | | | |
| M181 | | | |
| W22 | | | |
| W23 | | | |
| W24 | | | |

**变异执行台账**：V158–V171 逐条记 KILLED / SURVIVED / 未跑；任何 SURVIVED
= 对应防线失效，当场开缺陷单（修完按 11-04 复验手册流程回归）。

---

## 十、库证查询（sqlite3 模板）

```sql
-- Q2 本轮核心表：消息真源（msg_id/ord/trace_id/turn_status/file_changes）
SELECT ord, msg_id, role, turn_status, trace_id,
       length(content) AS len, length(thinking) AS think_len,
       length(tools_json) AS tools_len, length(file_changes_json) AS fc_len,
       datetime(created_at/1000,'unixepoch','localtime') AS at
FROM bot_messages WHERE session_id = '<会话id>' ORDER BY ord;

-- Q3 撤回面：某回合之后的文件变更（join 会话）
SELECT fc.id, fc.path, fc.kind, fc.before_ref IS NOT NULL AS has_snap,
       fc.after_sha IS NOT NULL AS has_after, t.session_id
FROM file_changes fc JOIN exec_traces t ON t.id = fc.trace_id
WHERE t.session_id = '<会话id>' ORDER BY fc.id;

-- Q4 指令文件 / 撤回 / 分类器 / OAuth 审计面
SELECT event, COUNT(*) FROM audit_log WHERE event IN
('instructions.updated','instructions.truncated','turn.rollback',
 'checkpoint.swept','auto_gate.classifier','auto_gate.classifier_disabled',
 'mcp.oauth.login','mcp.oauth.logout','bot_fs.classifier_allow',
 'bot_fs.classifier_allow_write')
GROUP BY event;
```

（audit_log 若无 event 列，按现行 schema 换列名——以 `PRAGMA table_info(audit_log)` 为准。）

---

## 十一、本轮事件名速查（判据引用）

投影事件：`bot-msg-upserted` / `bot-msg-dropped`（真源化批 2 起，统一信封盖章）、
`bot-ask` / `bot-ask-resolved`（elicitation 复用，payload 增 `source: "mcp_elicit"`）。
新增审计事件：`instructions.updated` / `instructions.truncated` / `turn.rollback` /
`checkpoint.swept` / `auto_gate.classifier` / `auto_gate.classifier_disabled` /
`bot_fs.classifier_allow` / `bot_fs.classifier_allow_write` / `mcp.oauth.login` /
`mcp.oauth.logout`。命令面新增：`instructions_get/set`、`bot_turn_rollback`、
`elicit_respond`、`mcp_oauth_login/logout/status`、`activate_tools`（bot 工具，第 67 个）。

---

## 十二、读码预判的风险面（提前预警，不代替实测）

1. **M176 的缓存断点**：activate_tools 激活瞬间 tools 数组变化，Anthropic 缓存链
   从 tools 层起重算一次（方案 §十一偏离 1）——观感 = 激活后第一条回复变慢变贵，
   属预期，不算回归；连续激活多组会多次付全价，判据只看功能通。
2. **M177 分类器判别边界**：「诉求内白名单外写」是 allow/unsure 的模糊带——
   两态都算过卡（判据 1 已写两可）；**只有 deny→放行 或 异常→放行** 是缺陷。
3. **M180 手动过期**：直接改 keyring 条目在 macOS 钥匙串里不便——备选：
   等真实过期，或临时把 `REFRESH_AHEAD_SECS` 调大后重编译（变异法，跑完还原）。
4. **elicitation 的 60s 窗口与 call_locks**：等待占住该 server 的串行锁——
   同 server 并发第二调用会排队等待，属设计语义；测试时别在等待期发同 server
   的第二个调用。
5. **撤回与 /retry 的组合**：撤回复（drop_after）后再 /retry 的行为未单独出卡——
   两者复用同一原语，回归风险低；若实机发现异常再补卡。

---

## 十三、与前几份的关系

- 11-03 / 11-04 方案（M93–M162）覆盖 Agent 运行时批 1–8 与运行时改造批 1–4 的实机面，
  本份不重复；work_items 每回合一行重构（方案未实施）落地时另出卡。
- 六张 ACC 卡（P-16~P-21）的 L1 自动化面已绿，本份执行后回填矩阵状态列
  （`未验` → `已验`），缺口说明列同步收敛。
- 双窗实测（真源化偏离 10）继续随多窗口 UI 立项，本份不含。
