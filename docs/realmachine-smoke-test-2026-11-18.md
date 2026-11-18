# 实机冒烟测试方案（2026-11-18）

> 覆盖 2026-11-16 ~ 11-18 的四批修复共 25 笔（`origin/main..9d9f273f`）：审计报告
> `docs/agent-audit-2026-11-17.md` 的处置批次。
> 这批**全是行为变更**（OAuth 端点重写、公网闸收紧、重定向切方法、守卫判定口径、
> 会话落库时序、子 agent 提示词动态渲染），门禁与单测覆盖不到真实链路——本方案
> 是它们唯一的验证面。
> 前轮方案：`docs/realmachine-smoke-test-2026-11-15.md`（M182–M203 + W25–W27 + 变异
> V172–V192）。本轮卡号接续：**M204–M223 + W28–W29**，变异接续 **V193–V215**。

## 〇、跑法与口径

- **基线**：`9d9f273f`（或其后的修复提交）。跑前 `git log --oneline -1` 记录实际基线。
- **数据口径**：测试阶段老数据可删（用户拍板）；直写 sqlite 时间戳列一律毫秒整数
  （`reminders` 表的 `remind_at` / `fired_at` 都是 INTEGER）。
- **杀进程**：macOS `pkill -x wmessage`（禁 `pkill -f`）；Windows `taskkill /IM wmessage.exe /F`。
- **定位数据目录**：启动后 bot.log 首部 `app_data_dir` 审计行。本轮判据落在 data_dir 下：
  `bot.log`（全部事件证）、`wmessage.db`（`work_items` / `subagents` / `bot_messages` /
  `reminders` 表）、子 agent 产物目录（`gen_dir/<id>/`）。
- **审计证**：本轮全部事件证在 **bot.log 文件行**（`[ts] LEVEL | event | k=v`），不在
  sqlite——grep 模板见 §十。事件名断言用 `grep -c` 计数。
- **本轮特别提醒**：OAuth 派生端点、公网闸、目的地闸都是**真闸**，测它们时不要把
  闸关掉——本轮没有灰度开关，任何「先关闸再测」都会让判据失去意义。
- **凭据面判据**：本轮多处修的是「凭据/查询串不该进审计面」。判据用 **bot.log 全文
  grep**（不是只看事件行）——自由格式串（如 `reminder.set | ...`）也在其中。
- **结果回填**：每卡 PASS / FAIL / 部分 + 证据，填 §九回填位；FAIL 带变异锚点编号。
- **时间预算**：20 张主卡约 3.5 小时（OAuth 章最慢，要起真 MCP 服务器）；变异复跑 23 条
  （各需一次重建 + 复跑）另计 4 小时；Windows 两张本机跑不了，登记延后。

---

## 一、MCP OAuth 全链（审计 K-1 / K-2 / K-9 —— 本轮风险最高）

> 本轮把 OAuth 的派生端点（发现第二跳 root / DCR / token endpoint）全部改成「逐 URL 过
> 闸 + 钉当跳 IP」，并把降级凭据文件改 0600。改错的后果是「能登录但登不掉」或
> 「登录时把内网打一遍」——两个方向都要真链验证。

### M204 OAuth 登录全链（主卡）
- **步骤**：配置一台 HTTP 传输的 MCP 服务器（走 OAuth 那套，需支持
  `/.well-known/oauth-protected-resource` + `/.well-known/oauth-authorization-server`
  + DCR），点设置页「登录」，走完浏览器授权 → 回调 → 换 token。
- **判据**：
  1. `mcp.oauth.login` 事件 1 条（server id 对）；
  2. 登录成功后该服务器的工具出现在模型清单里（`mcp.tools_discovered` 1 条），
     且能实际调用成功；
  3. bot.log 全文 grep **无内网地址**（`127.0.0.1`、`169.254.`、`10.`、`192.168.`）——
     派生端点过闸后不该有任何指向内网的请求；
  4. 回调服务器只绑 127.0.0.1:0 随机端口（授权完成后该端口已关闭）。
- **变异锚点**：V193、V194、V195。
- **环境依赖**：需要一台真 OAuth MCP 服务器（官方 SDK 1.32.2 那套可复用前轮的手记）。
  没有则登记「环境受限」，判据 1/4 用纯函数面背书。

### M205 派生端点逐 URL 过闸（负向，主卡）
- **步骤**：用一台**恶意** OAuth MCP 服务器（自建最小 HTTP 服务）：保护资源元数据里回
  `authorization_servers: ["http://169.254.169.254/"]`，点「登录」。
- **判据**：
  1. 登录**失败**，错误文案指向「URL 安全闸」而不是「连不上」；
  2. bot.log 无任何指向 `169.254.169.254` 的请求记录（用审计里的 host 字段核对）；
  3. 同样把 `authorization_servers` 指到 `http://10.0.0.1/` 与 `file:///etc/passwd`
     各测一次，均被同一道闸拦下。
- **变异锚点**：V193、V194、V195。
- **注**：V193/V194/V195 三条变异必须**逐条**跑（分别对应第二跳 / token / authorize
  三个点的闸），不是跑一条代表全部。

### M206 凭据文件权限与登出不再假成功
- **步骤**：(a) macOS 观察 OAuth 令牌落点（System keyring 后端，不落文件）；(b) 若环境
  有 Linux 无 keyring 档，查 `bot-mcp-oauth.json` 权限；(c) 在**钥匙串不可用**的条件下
  点「登出」（如在受管服务里临时禁用钥匙串访问，或用 `keychain` 锁定态）。
- **判据**：
  1. macOS：令牌在钥匙串里（`security find-generic-password -s wmessage` 能查到
     `mcp-oauth:<id>` 条目），**不在** data_dir 的任何 json 里；
  2. Linux 降级档：`bot-mcp-oauth.json` 权限 **0600**（`stat -c %a`），同目录其他
     文件不是 0600（对照组，证明闸真的生效）；
  3. 钥匙串不可用时点登出 → **命令报错**（前端有提示），**没有** `mcp.oauth.logout`
     成功事件；钥匙串恢复后再登出 → 成功事件 1 条且令牌条目消失。
- **变异锚点**：V196、V197。
- **环境依赖**：判据 2 需 Linux 降级档（本机 macOS 受限则登记）；判据 3 需构造钥匙串
  故障（macOS 钥匙串锁定态可近似）。

### M207 elicitation 应答回声面（凭据脱敏）
- **步骤**：起一台声明 `elicitation.form` 能力的 MCP 服务器（per-server
  `allowElicitation=true`，**默认 false**，须显式开），触发一次反问，在应答里输入一段
  含引号/反斜杠的文本（如 `a"b-secret`），观察服务器回显。
- **判据**：
  1. bot.log 全文 grep **无** `b-secret`（应答值与转义形态都不该出现）；
  2. `bot-ask-resolved` 的 answer 恒为占位符 `[MCP 应答，值不落日志]`；
  3. 在**数字字段**里输入非数字（如误粘一段凭据）→ 错误文案**不含**你输入的内容，
     只说「输入不是数字，请重新输入」。
- **变异锚点**：V211、V212。

---

## 二、公网闸与 http_request 行为（审计 H-1 / H-2 / H-4 / H-6）

> 本轮把公网闸的 IPv6 判据与 MCP 闸**并成一份**（方向是收紧），并让 http_request
> 的重定向按 RFC 9110 切方法（303/301/302 的 POST 改 GET 丢体）。两条都会改变真实
> 联网行为，必须实测。

### M208 重定向按 RFC 切方法（主卡）
- **步骤**：本地起一个最小 HTTP 服务，提供 4 个端点：`/301`（301 → `/target`）、
  `/302`、`/303`、`/307`，`/target` 回显收到的 method 与 body。分别用 `http_request`
  发 POST（带 body）打这四个端点。
- **判据**：
  1. 301/302/303 三条 → `/target` 收到的 method 是 **GET** 且**无 body**；
  2. 307 → 收到的 method 仍是 **POST** 且 body 原样；
  3. 每跳都过了目的地闸（本地地址走的是「用户配置目的地」路径，若被拒则本卡改用
     公网可达的测试服务，或登记受限）。
- **变异锚点**：V198。

### M209 userinfo 不进审计面
- **步骤**：`http_request` 打一个带 userinfo 的 URL（如 `https://user:pass@<host>/x`）
  ——用一个会 404 的地址即可（错也要看审计）；另测一个 302 到带 userinfo 的 Location。
- **判据**：
  1. bot.log 全文 grep **无** `pass`（userinfo 不该出现在 tool.return preview、子 agent
     动作流、执行痕迹 span 任何一处）；
  2. 「来源：」行保留 scheme://host/path，userinfo 被替换成 `***@`；
  3. 正文节选里的第三方链接**未被裁**（对照：同一条日志里页面正文里的链接原样在）。
- **变异锚点**：V199。

### M210 fetch_url 查询串不进审计面
- **步骤**：`fetch_url` 调一个带查询串的 URL（`https://<host>/p?q=secret`），成功与失败
  各一次（失败用会 404 的路径）。
- **判据**：
  1. bot.log 全文 grep **无** `secret`（入参预览、失败文案、`fetch_url | url:` 那条
     自家审计三处都不得带查询串）；
  2. 日志里该 URL 仍可见 host + path（信息没丢，只是凭据段被裁）；
  3. 模型可见面（对话气泡里的结果）**仍带**完整 URL（模型需要知道最终落点）——这条
     是口径的分界，裁错方向算 FAIL。
- **变异锚点**：V200、V201。

### M211 IPv6 过渡段收紧（负向）
- **步骤**：`http_request` / `fetch_url` 各打一次这些字面量地址：
  `http://[64:ff9b::c0a8:0101]/`（NAT64 → 192.168.1.1）、`http://[2002:0a00:0001::1]/`
  （6to4 → 10.0.0.1）、`http://[::c0a8:0101]/`（IPv4-compatible）。
- **判据**：
  1. 三条全被公网闸拒（错误文案指向公网闸）；
  2. **回归**：普通公网 IPv6（如 `https://[2606:4700:4700::1111]/`）仍**未被字面量级
     拦下**（放行与否取决于 DNS 解析，但不该被「IPv6 保留段」这条判据拒）；
  3. MCP HTTP 服务器的同一类地址同样被拒（两闸已单源）。
- **变异锚点**：V202。
- **注**：本卡是**收紧方向**的回归卡——若用户环境是 IPv6-only 且走 NAT64，联网工具会
  受影响，跑完在回填位注明（这是有意取舍，不是缺陷）。

---

## 三、守卫与上下文面（审计 D-1 / D-2 / E-1a）

### M212 系统提示不算出处（幻觉守卫二次防线）
- **步骤**：让模型报一个**没有出处**的数（如「这次处理了 424242 条记录」，而本次工具
  返回里没有这个数），观察守卫拦下并注入返工提示；然后让模型**照抄返工提示里的数**
  再报一次。
- **判据**：
  1. 第一次：`hallucination_guard` 事件 1 条（kind=unsourced_value 之类），本轮不
     直接放行；
  2. 第二次（模型复述同一个数）：**仍被拦**（`hallucination_guard.repeat` 或再次
     Continue）——修复前这里会因「返工提示里的数被当出处」而放行；
  3. bot.log 里能看到返工提示文本（`【系统提示】` 前缀），但该提示**不在**下一轮的
     出处判定里（这是本卡的实质）。
- **变异锚点**：V203。

### M213 证据池头尾对齐 + 滑动窗桩子披露
- **步骤**：(a) 让模型读一个 >64KB 的文件并引用**尾部**的一个数（如让子 agent 统计一个
  大 CSV 的末行数值）；(b) 让一轮内工具结果累计超 24K 字符触发滑动窗，看模型看到
  什么。
- **判据**：
  1. 引用尾部数字**不被判「无出处」**（修复前会误拦真话）；
  2. 滑动窗桩子文案**含钳制披露**（「存档原文按 N 字节上限截断，超出部分不在存档里」），
     **不含**「完整结果已存档」这种超出事实的说法；
  3. 模型在桩子情况下调 `lookup_receipt` 取回的内容与披露的上限一致（拿回的是被
     截过的那份，模型不误以为拿到全文）。
- **变异锚点**：V204、V214。

---

## 四、会话落库与工作项一致性（审计 L-2 / L-3 / L-4 / L-5 / L-8）

### M214 工作项只跟用户轮（失败信号不被抹）
- **步骤**：(a) 制造一次用户轮失败（如让模型调一个必然被拒的工具后失败）；(b) 在
  该会话派一个子 agent，等它收尾触发汇报轮；(c) 看「需要人处理」聚合面。
- **判据**：
  1. 用户轮失败后，`work_items` 表该行 stage=failed、error 非空；
  2. 汇报轮跑完后，该行**仍是 failed**（stage 不被唤醒回 working 再收成 idle）——
     这是本卡的实质（修复前失败信号当场消失）；
  3. 极短回合（模型一轮就返回）不留 stage=working 的残行（开票与收尾次序已钉）。
- **变异锚点**：V205。
- **库证**：`SELECT stage, error, updated_at FROM work_items WHERE session_id=?;`

### M215 回合事实命令层清场（失败行不串台）
- **步骤**：(a) 让子 agent 收尾触发一次**汇报轮**（它会把正文流进该会话）；(b) 紧接着
  让用户轮在**密钥失效**条件下失败（设置页清空 API key 后立刻发消息）。
- **判据**：
  1. 失败行的 content **只含本轮的错误**，不含上一轮汇报轮的正文（修复前会拼在一起
     落库）；
  2. 库里 `bot_messages` 里该 failed 行的 content 与界面显示一致；
  3. 正常路径（密钥有效）不受影响：失败行只含错误。
- **变异锚点**：V206。

### M216 /clean 广播删除投影
- **步骤**：单窗口跑 `/clean`；再开第二个窗口看同一会话。
- **判据**：
  1. `/clean` 后 `bot-msg-dropped` 事件 1 条（载荷含被删的 msgIds 列表）；
  2. **第二窗口**的会话视图同步清空（不刷新页面的情况下）——这是本卡的实质（修复前
     第二窗口仍显示旧消息）；
  3. 前端改为「落库成功再清视图」：invoke 失败时**视图不清空**且有提示（可用改库锁
     或断连构造失败）。
- **变异锚点**：V207。

---

## 五、MCP stdio 生命周期与工具发现（审计 K-3 / K-5 / K-4 / K-10 / K-11）

### M217 工具发现与槽位自愈
- **步骤**：(a) 起一台 stdio MCP 服务器，等工具发现完成；(b) 在**发现进行中**（最长约
  20s）于设置页禁用该服务器；(c) 再看模型清单；(d) 另测：握手超时后槽位自愈。
- **判据**：
  1. 情形 (b) 之后模型清单里**不再出现**该服务器的工具（修复前是幽灵条目，每轮都
     出现，直到下一次 MCP 配置写才消失）；
  2. 出现 `mcp.tools_discovered_but_disabled` 事件（不生效时有留痕）；
  3. 情形 (d)：停滞的 Connecting 槽位按 40s 判为 down，发现路径能重新走起来
     （`mcp.tools_discovered` 再来一条，面板不再永远「待发现」）。
- **变异锚点**：V208。

### M218 超长响应行快速失败
- **步骤**：让 MCP 服务器在一次工具调用里返回**单行 >1MB** 的 JSON-RPC 响应（如
  返回一个 1.5MB 的 base64 字段）。
- **判据**：
  1. 调用**快速失败**（秒级，不是等满超时）——修复前要干等到 600s；
  2. 错误可读（提到响应超单行上限）；
  3. **回归**：服务器刷一条超长**噪声行**（如进度条）后，仍能正常应答下一条请求，
     会话不断。
- **变异锚点**：V209。

### M219 树杀兜底先拍树再收尸（孤儿进程）
- **步骤**：起一个 `npx` 包装的 stdio MCP 服务器（npx → node 两层），在中途强杀
  wrapper（或触发握手超时），随后查进程表。
- **判据**：
  1. 真实服务器进程（node）**不留活**（`pgrep -f "<server-name>"` 为空）——修复前
     npx 死了 node 活着占端口；
  2. 端口不被占（`lsof -i :<port>` 空）；
  3. 不误杀同组无关进程（对照组：另起一个同 prefix 的无关进程，确认它还在）。
- **变异锚点**：V210。
- **注**：本卡的进程名/端口取你实测时的实际值，填回填位。

---

## 六、技能、提示词与提醒/作业（审计 C-1 / C-3 / E-1a / K-9 / L-6 / L-7 / L-9 / D-4 / D-5）

### M220 验收段档位口径
- **步骤**：让主 agent 派一个 verifier 段子 agent（按提示词只传 `stage=verifier`、不传
  profile），让它用 `run_python` 做一次只读复核。
- **判据**：
  1. verifier 段**实际可用** `run_python` 与 `ast_inspect`（不被 dispatch 白名单闸拒）；
  2. 主 agent 的提示词里**不含**「验收段用 research profile」这类与运行时相反的
     指示（grep 源码与实际下发的 system prompt 两处）；
  3. 若父 agent 自行传了 `profile=research` → 该次调用会被白名单闸拒（属既有口径：
     显式档位覆盖 yaml 档），不算 FAIL，但要在回填位记下实际行为。
- **变异锚点**：无（源码面已锁）。
- **注**：判据 2 可直接 grep 源码：`prompts/subagent.rs` 里不应再出现「用 research
  profile」。

### M221 executor 工具行随生效集渲染
- **步骤**：派一个 executor 段子 agent（`stage=executor`，默认挂 writereditor 档），
  让它改一个文件；再派一个**显式给别的配置档**的 executor 段。
- **判据**：
  1. 默认路径：提示词里的工具行与实际可用工具一致（edit_file / write_file /
     run_python 可用且能真调通）；
  2. 显式换档时：提示词里的工具行**只列该档实际生效的工具**（不列被收掉的）——
     模型不该照着一个被收掉的工具名去调（那会撞白名单闸白烧一轮）；
  3. 模型全程无「工具不可用」类失败（对照判据 2 的后果面）。
- **变异锚点**：V213。

### M222 提醒广播与作业不双跑
- **步骤**：(a) 聊天里让模型建一条提醒（+2 小时），切到「定时」页；(b) 同一作业，
  先让它到点排队（占满并发闸），再手动点「立即执行」。
- **判据**：
  1. 情形 (a)：**不用刷新页面**，定时页就出现新提醒（`reminders-changed` 事件带
     1 条；修复前模型建的提醒不广播，页面停在旧快照）；
  2. 情形 (b)：手动触发**不与到点派发重复跑**（同一 occurrence 只有一次执行——
     对照 `scheduled_job_runs` 的行数与 token 消耗）。
- **变异锚点**：无。

### M223 僵尸提醒退休与变更话术
- **步骤**：(a) 建一条 schedule 串损坏或极难的周期提醒（可手改库里 `reminders.schedule`），
  让它触发一次；(b) 让模型声称「已把 a.txt 重命名为 b.txt」但**没有**真调工具。
- **判据**：
  1. 情形 (a)：触发后该行**被删**（不是留一条 `remind_at` 为 NULL 的僵尸行——它既不
     触发也无人清扫）；`SELECT COUNT(*) FROM reminders` 核对；
  2. 情形 (b)：该声称被幻觉守卫拦下（`hallucination_guard` 1 条，kind=mutation 类）——
     修复前「已重命名」不在任何工具的 claims_patterns 里，守卫完全看不见。
- **变异锚点**：无。
- **注**：情形 (a) 的 schedule 串要能被 `occurrence_after` 算出「无下次」——若当前实现
  永远算得出（生产不可达），本卡登记为「不可激活」并记明理由。

---

## 七、Windows 专章（本机 macOS 跑不了，登记延后）

### W28 Windows Job Object 限额与拆分后行为
- **步骤**：Windows 上跑 run_python 与 apps（长脚本 + 内存 hog 脚本各一）。
- **判据**：
  1. 内存超限的脚本**被限额拦住**（Job Object PROCESS_MEMORY 生效）；
  2. 宿主退出时子进程**跟着死**（KILL_ON_JOB_CLOSE）；
  3. **拆分后回归**：`py/win_job.rs` 拆出后行为与拆分前一致（限额、kill-on-close、
     `create_kill_on_close` 三条都在）；
  4. Job Object 创建失败时**留有审计**（本轮新加的留痕行）。
- **变异锚点**：V215。

### W29 F-2 真补评估（是否补 ACTIVE_PROCESS）
- **步骤**：Windows 上跑一个 fork 风暴脚本（`multiprocessing` 大量起进程），观察系统
  是否被拖垮。
- **判据**：
  1. 当前行为记录在案（无进程数上限，只有内存/CPU/超时约束）；
  2. 给出「补 `JOB_OBJECT_LIMIT_ACTIVE_PROCESS`」的实测结论——半生效的限额比没有更
     危险，本轮只把注释写准，能力补齐与否留给实机结论。
- **变异锚点**：无。
- **注**：本卡是**评估卡**不是验收卡，结论写回填位即可。

---

## 八、变异注入锚点总表（V193–V215）

| 锚点 | 变异注入 | 必须被哪张卡逮住 |
|---|---|---|
| V193 | OAuth 发现第二跳 root 复用钉原 host 的 client（去掉逐 URL 过闸） | M204（登录行为变）/ M205（内网请求被发出，判据 2 红） |
| V194 | token endpoint 不再过闸（复用原 client 直发） | M205（请求打到内网，判据 2 红） |
| V195 | `authorization_endpoint` 的字面量级闸删（scheme/私网不拦） | M205（`file://` 或私网 authorize URL 被放行，判据 3 红） |
| V196 | 降级凭据文件回退 `atomic_write`（0644） | M206（权限不再是 0600，判据 2 红） |
| V197 | 登出回到 `let _ =` 吞错 + 无条件记成功 | M206（钥匙串故障时仍记成功、用户无提示，判据 3 红） |
| V198 | 重定向不分派方法（四种全保持） | M208（301/302/303 收到 POST + body，判据 1 红） |
| V199 | final_url 只裁查询串、不裁 userinfo | M209（bot.log 出现 `pass`，判据 1 红） |
| V200 | `sink_args_preview` 不给 fetch_url 特裁 | M210（入参预览带查询串，判据 1 红） |
| V201 | fetch_url 自己的 `audit_log` 不裁 URL | M210（日志带查询串，判据 1 红） |
| V202 | 公网闸 IPv6 过渡段判定移除 | M211（三条过渡段地址放行，判据 1 红） |
| V203 | 出处池不排除「【系统提示】」前缀 | M212（模型复述返工提示里的数即放行，判据 2 红） |
| V204 | 证据池只留返回文本头部（回到 `chars().take`） | M213（引用尾部数字被判无出处，判据 1 红） |
| V205 | 工作项闸去掉 origin（汇报轮也开票收尾） | M214（失败信号被唤醒又收成 idle，判据 2 红） |
| V206 | `turn_record::begin` 移回模型循环内部 | M215（失败行拼进上一轮汇报轮正文，判据 1 红） |
| V207 | `/clean` 不广播 `bot-msg-dropped` | M216（第二窗口不收敛，判据 2 红） |
| V208 | 槽位插入不核对启用快照 | M217（幽灵工具条目每轮出现，判据 1 红） |
| V209 | 行长闸恢复「一视同仁跳过」（不判 JSON-RPC） | M218（大响应被跳过 → 干等超时，判据 1 红） |
| V210 | 树杀顺序倒回（先 wait 收尸再树杀） | M219（node 孤儿占端口，判据 1/2 红） |
| V211 | elicit 解析失败文案回显用户输入 | M207（错误链把输入带进审计，判据 3 红） |
| V212 | 回声指纹不登记 JSON 转义形态 | M207（转义形态的应答值漏进审计，判据 1 红） |
| V213 | executor 工具行改回写死 writereditor 档 | M221（显式换档时清单与白名单脱节，判据 2 红） |
| V214 | 滑动窗桩子文案改回「完整结果已存档」 | M213（文案与实际钳制不符，判据 2 红） |
| V215 | Windows Job Object 创建失败不留痕 | W28（无失败审计，判据 4 红；需 Windows 实机） |

---

## 九、结果回填位

| 卡 | 结果 | 证据（事件行 / 库证输出） | 备注 |
|---|---|---|---|
| M204 | | | |
| M205 | | | |
| M206 | | | |
| M207 | | | |
| M208 | | | |
| M209 | | | |
| M210 | | | |
| M211 | | | |
| M212 | | | |
| M213 | | | |
| M214 | | | |
| M215 | | | |
| M216 | | | |
| M217 | | | |
| M218 | | | |
| M219 | | | |
| M220 | | | |
| M221 | | | |
| M222 | | | |
| M223 | | | |
| W28 | 未覆盖 | 本机为 macOS | 环境受限延后 |
| W29 | 未覆盖 | 本机为 macOS | 环境受限延后 |

**变异执行台账**（V193–V215 共 23 条待跑）：

| 锚点 | 跑否 | 结论（KILLED / SURVIVED / 受限） | 证据 |
|---|---|---|---|
| V193 | | | |
| V194 | | | |
| V195 | | | |
| V196 | | | |
| V197 | | | |
| V198 | | | |
| V199 | | | |
| V200 | | | |
| V201 | | | |
| V202 | | | |
| V203 | | | |
| V204 | | | |
| V205 | | | |
| V206 | | | |
| V207 | | | |
| V208 | | | |
| V209 | | | |
| V210 | | | |
| V211 | | | |
| V212 | | | |
| V213 | | | |
| V214 | | | |
| V215 | | | |

---

## 十、库证查询与日志证

```bash
# 通用：定位数据目录（bot.log 首部的 app_data_dir 审计行）
head -50 "$APPDATA/wmessage/bot.log" | grep -m1 app_data_dir

# ── OAuth 面（M204/M205/M206）──────────────────────────
grep -c "mcp.oauth.login"   "$LOG"      # 登录成功计数（应为 1）
grep -c "mcp.oauth.logout"  "$LOG"      # 登出计数
grep -Ei "127\.0\.0\.1|169\.254\.|10\.[0-9]|\[2002:|\[64:ff9b" "$LOG" | head   # 内网请求痕迹（应为空）

# ── 公网闸与 http 面（M208/M209/M210/M211）──────────────
grep -c "bot.http_request" "$LOG"
grep -E "来源：https?://" "$LOG" | head        # 「来源：」行的 userinfo 应是 ***@
grep -c "fetch_url | url:" "$LOG"              # 该行应只见 host+path

# ── 守卫与上下文面（M212/M213）─────────────────────────
grep -c "hallucination_guard" "$LOG"
grep -c "hallucination_guard.repeat" "$LOG"
grep -c "【系统提示】" "$LOG" | head -1        # 返工提示存在性

# ── 会话落库面（M214/M215/M216）───────────────────────
sqlite3 "$DB" "SELECT session_id, stage, error FROM work_items ORDER BY updated_at DESC LIMIT 5;"
sqlite3 "$DB" "SELECT msg_id, substr(content,1,80) FROM bot_messages WHERE content LIKE '%错误%' ORDER BY ord DESC LIMIT 5;"

# ── MCP 生命周期面（M217/M218/M219）───────────────────
grep -c "mcp.tools_discovered" "$LOG"
grep -c "mcp.tools_discovered_but_disabled" "$LOG"
pgrep -f "<你的 MCP server 名>" || echo "无残留进程（期望）"

# ── 提醒与作业面（M222/M223）──────────────────────────
sqlite3 "$DB" "SELECT id, remind_at, fired_count FROM reminders ORDER BY updated_at DESC LIMIT 5;"
sqlite3 "$DB" "SELECT COUNT(*) FROM reminders WHERE remind_at IS NULL;"   # 期望 0（M223 判据 1）
sqlite3 "$DB" "SELECT job_id, COUNT(*) FROM scheduled_job_runs GROUP BY job_id;"  # M222 判据 2
```

**事件名速查**（本轮判据引用，源码里均为真实事件名）：

| 事件名 | 出处 | 判据卡 |
|---|---|---|
| `mcp.oauth.login` / `mcp.oauth.logout` | `bot/mcp/oauth.rs` | M204 / M206 |
| `mcp.tools_discovered` / `mcp.tools_discovered_but_disabled` | `bot/mcp/manager.rs` | M204 / M217 |
| `onion.stage_attached` | `bot/mcp/manager.rs` | M220 / M221 |
| `tool.output.over_budget` / `tool.output.spilled` | `bot_model_loop/output.rs` | M213 |
| `hallucination_guard` / `hallucination_guard.repeat` | `bot_model_loop/guard.rs` | M212 / M223 |
| `bot-msg-upserted` / `bot-msg-dropped` | `bot_chat.rs`（统一出口盖章） | M216 |
| `bot-ask-resolved` | `bot/mcp/elicit.rs` | M207 |
| 自由格式串：`reminder.set \| ...` / `reminder.cancel \| ...` | `bot_reminders.rs` | M222 |
| 自由格式串：`bot.http_request` | `bot/tools/http.rs` | M208 |

---

## 十一、本轮与前轮的关系

- 前轮 `docs/realmachine-smoke-test-2026-11-15.md` 覆盖 v0.3.0 后七批 46 提交
  （M182–M203 + W25–W27 + 变异 V172–V192）。**本轮覆盖的是那之后的四批 25 笔**
  （审计报告 `docs/agent-audit-2026-11-17.md` 的处置批次），两轮卡号与变异编号接续不重叠。
- 本轮修的东西大半是**闸与判定口径**，卡位偏「负向」：M205 / M211 是明确的负向卡
  （不该通过的必须被拦），其余以「行为面 + 审计面」双判据为主。
- 本轮**无灰度开关**：OAuth 逐 URL 过闸、公网闸 IPv6 收紧、重定向切方法、守卫口径
  全部恒生效——对照面只能靠「字面量级」与「单测锚点」两条背书。
