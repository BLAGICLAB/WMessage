# 实机冒烟报告：2026-10-29 → 10-31 变更回归（M93–M115）

执行时间 2026-11-01 07:23 → 10:35（macOS 本机，`e6546f8a..42fe53e8` 窗口，HEAD `42fe53e8`，debug 产物 + vite dev 形态，驱动 = smoke.sh + cliclick 坐标点击 + 剪贴板粘贴 + 轮询截屏）。方案与判据原文见 `docs/realmachine-smoke-test-2026-10-29.md`。

> **修复批次（同日 11:40 → 12:35）**：五条发现全部修复——D-M99-1（report 轮 reasoning 强制 off，消 400）、D-M110-1（SSE 攒批无缝拼接 + 键名中间切断锚点测试）、D-M95-1（resolve_secret 支持 `Bearer <引用>` + 未解析引用子串拒发）、D-M95-2（keyring NoEntry / env 空值报错拒发）、D-M101-1（口径写进 tool_in_flight 注释，行为不变）。全量 2214 单测绿 + clippy 干净；D-M95 两修复已实机复验（httpbin 回显 `Bearer wm-smoke-placeholder-...` 解析生效 / keyring 未配置厂商在解析阶段被拦「请求没有发到网络」）。D-M99-1 的端到端复验依赖子 agent 后台完成的汇报轮自动触发（主回合转述会消化 digest），留给日常使用观察；机理论证：Report 轮请求无 thinking 参数，「thinking 必须回传」规则定义域之外。

## 一、总览

| 档 | 结论 |
|---|---|
| 基线冒烟（smoke.sh 14 项） | **PASS=10 / FAIL=0 / SKIP=0**（启动无 panic、error.log 零新增、integrity_check ok、两轮往返 trace.complete ×2、Cmd+Q 优雅退出 + app_exit_cleanup） |
| 主卡 23 张（M93–M115） | **PASS 14 · FAIL 2（M95、M110，各带 P1 发现）· 部分 PASS 2（M101、M102）· 未覆盖 5（M105 带图半、M113、M115、M94 两半、M98 工具轴半）** |
| Windows 专章（W13–W15） | 未执行（本机为 macOS；W13–W15 需 Windows 实机，待用户安排） |
| 变异注入 | **V125 ✓、V97 ✓**（注入后判据均 FAIL 成立 = 原修复有效）；其余因时间与构造条件未跑 |
| 新发现 | **5 条**：D-M95-1（P1）、D-M95-2（P1）、D-M99-1（P1）、D-M110-1（P1）、D-M101-1（口径） |

## 二、逐卡结论（对应方案 §5.1）

| 卡 | 结论 | 关键证据 |
|---|---|---|
| M93 run_python 断网硬闸 | **PASS** | 探针行 `run_python \| seatbelt_probe \| 出网闸生效`（09:59:41）；联网脚本失败形态 = DNS gaierror（非 200、非 ECONNREFUSED，deny network\* 连 DNS socket 一并拦）；纯计算脚本 `sum(i*i)`=332833500 正常跑完（无误杀）；子进程 `os.system(curl)` 输出 000（fork/exec 继承断网）。apps 同享半未覆盖 |
| M94 目的地闸与 http_request | **PASS（核心半）** | 白名单内 `dest=whitelist` 不弹窗；白名单外弹窗批准后 `dest=asked` 200；60s 无应答 `dest=confirm_denied`（Warn，fail-closed）；同 host 复请求 `dest=session`；换 host 独立弹窗（www/en.wikipedia、api.wikimedia 各自拒）；saveAs 落盘 `AI_Gen_Files/wmtest.png` 8090 字节 + 响应头 `set-cookie: ⟨值不回显⟩` 脱敏。**未覆盖**：每跳 302 判定（无可控公网 302 源——httpbin redirect-to 403 拒外域、v.gd 是 meta-refresh、is.gd/tinyurl 连接失败、本地 302 服务被公网闸正确拦截）；配置写入失效半（invalidate_grants 需设置页驱动） |
| M95 凭据引用与四处 sink 面 | **FAIL（反向半）**，正面判据全过 | 正面：key 值在 bot.log 全文与 wmessage.db 二进制 grep **均 0 次出现**；`tool.call args_preview=GET httpbin.org/headers`（sink_args_preview）；`bot.http_request` 恰 5 键 host/method/status/bytes/dest；trace span args 同款裁剪；整值 `env:WM_SMOKE_SECRET` 解析生效（httpbin 回显解析后的占位值）。**FAIL 见发现 D-M95-1 / D-M95-2** |
| M96 read_text_file 双闸 | **PASS** | 单行 5000 字符文件返回带 `[本行超 2000 字符已截断]` 标注，模型明确知道自己拿到的是残段；en-3000（365KB/848 行）`end=456` 被 56KB 字节预算收页 + 模型用 offset=457 续读；cn-300（43KB）300 行一页全读（预算内不误收） |
| M97 预算简化整套 | **PASS** | 设置页「机器人」卡无「Agent 运行参数」残留、任务模式选择器不在；参数表只剩 `tools.python_timeout_secs` 可编辑（600，钳制文案在位）；注入 5 个旧键（maxRounds/taskMode/maxTokens/reasoningEchoEnabled/subagentMaxWallSecs）启动**不报错**且被忽略；正常对话零预算报错；旧技能 `timeout_secs` 启动告警「已废弃（技能无墙钟），已忽略」。200/400 跑飞场景未覆盖 |
| M98 轮数轴收尾窗 | **PASS** | MAX_ROUNDS=10 下：`fuse_wrap_up_enter \| axis=rounds \| round=8 \| max_rounds=10` + `fuse_wrap_up_deny \| axis=rounds \| tool=read_text_file` + 最终 **带正文** done（模型自述「第 7 次就被收尾窗截止——这条路在一轮对话里走不完」并给出后续建议）；`fuse_wrap_up_rounds` 未出现属预期（模型进窗即转正文输出，未耗尽 3 轮自然完成）。工具轴对照半未覆盖 |
| M99 思考段无条件回灌 | **PASS + 发现** | openai 兼容 `deepseek-reasoner` 多轮工具调用 msgs 6→8→11→13 全程 200 零 400（该端点对回灌缺失必 400，已由同日 09:41 的 subagent 400 实证——回灌确实生效）。**发现 D-M99-1**：汇报路径漏回灌 |
| M100 LLM 重试 3 次 | **PASS** | baseUrl 指 503 假服务：`llm.retry` 恰 2 条（attempt=1/2，间隔 1.5s 固定无指数）+ 假服务恰好收到 3 次请求 + `llm.response recoverable=true err=上游服务异常` + 前端错误卡文案正确；/stop 在第 2 重试窗内打断（假服务停在第 5 次，第 6 次未发——重试循环确实被停，窗口精度受人手节奏限）。timeoutSecs=900 钳制半未单独驱动 |
| M101 子 agent 停滞三修 | **PASS（长工具半）+ 发现 D-M101-1** | sleep(40) > 20s 阈值**未被误杀**、正常 succeeded（tool_in_flight 兜住）；真死循环不触发 20s 判停滞（见发现）；pack/unpack 心跳半、DB 读失败半未覆盖 |
| M102 子 agent 三道直拒 | **PASS（B 路）** | 子 agent 请求白名单外域名：`bot.http_request \| dest=subagent`（Warn）**直接拒、无弹窗**。A 路（规则闸 ask_denied_subagent）、C 路（subagent_whitelist_deny）、计数半未覆盖 |
| M103 stage 自动挂载 | **PASS（核心半）** | spawn stage=recon 不传 onion：`onion.stage_attached \| stage=recon \| onion_id=probe` + subagents 表 profile=research。executor/verifier 档、显式 onion 优先、失败降级半未覆盖 |
| M104 提示词/schema 第二真相源 | **PASS（静态半）** | 「最多 200 轮工具循环」「输出约 8KB 封顶：超限部分转存文件并附指针」、web_search 默认 6——三处与常量一致；schemas.rs 唯一「默认 8」残留属 search_skills（SkillHub 搜索的 limit 参数），与 web_search 无关。动态半未单独驱动 |
| M105 压缩量尺全量序列化 | **PASS（链路半）** | `loop.msgs.age_compacted`（soft/hard 计数）在长会话多轮触发；带图量尺对比未覆盖（UI 发图受限），量尺实现人工核读（msgs_chars 整条序列化）+ 单测兜底 |
| M106 ask_user 聊天放开 | **PASS** | 聊天内提问卡三行（question/问这个/未回答时按）+ 选项 chips + 自由输入 + 双按钮全在；通知中心「聊天提问」+①角标；chip 点选→「回答并继续」→ 模型「已按『口径A：详版』记录」闭环；「按假设继续」路径闭环（M108 场景） |
| M107 等待环竞态 | **PASS（expired 路径）** | ASK_TIMEOUT_SECS=3 下提问挂起 → 3s 过期 → exec_span「提问在等待期内未获回答，已按你声明的假设继续。假设：默认按 A 执行」+ trace done（模型如实转告）。临界同时应答半（V121 场景）3s 窗口人手不可行，未覆盖；oneshot 关闭自旋半未覆盖 |
| M108 前端回灌状态机 | **PASS（豁免半）** + V125 | 提问挂起 40s+（15s 判死阈值的 2 倍+）**未判死**——豁免生效；判死-新轮不被清场半未覆盖（世代守卫有单测兜底）；切走切回正文半未覆盖。**V125 注入复验通过**（删豁免 → 提问挂起 28s 被判死 trace stopped） |
| M109 bot-confirm 统一出口 | **部分 PASS** | danger 确认弹窗 60s 超时默认拒绝 fail-closed 在案（`confirm_timeout \| write_file… \| /stop，默认拒绝`）；/stop 打断确认窗、重载还原倒计时、FIFO 还原、信封检查未覆盖（UI 驱动受限） |
| M110 SSE 拆行/攒批/注释行 | **FAIL（①）+ ③ PASS** | ③ flood：`llm.sse_data_batch_truncated \| bytes=2097663` **恰好 1 次** + 流不失败 + 会话继续 ✓。注释行忽略逻辑正确（unparsable sample 无 keep-alive 残留）。①② 见发现 **D-M110-1** |
| M111 流式内核 panic 兜底 | **PASS** | 注入 `panic!("M111…")` 于 run_model_loop：UI **立即**失败终态「⚠️ 内部错误：执行器异常退出（panic）：M111 SMOKE-TEST panic injection」+ `panic`（Error，thread/loc）+ `model_loop_panic`（Error，where=主聊天）+ trace failed ms=3 + 会话可继续（无永久在途）。文案较方案的「内部错误：模型循环异常终止」更详细（带 panic detail），语义等价 |
| M112 子 agent 等待薄片 | **PASS（行为面）** | check_subagent 等待期 /stop → trace stopped（09:52 `status=stopped` 佐证，等待环被打断）；500ms 薄片精度、落库重试半未覆盖 |
| M113 许可排队 /stop + MCP 并行 | **未覆盖** | parallel tool_calls 构造失败（deepseek-flash 两次均发串行调用，模型不配合）；许可上限半、MCP 坏服务器并行发现半均未构造 |
| M114 参数卡退役 | **PASS** | 与 M97 同截图：整卡不在、无空容器/错位；`python_timeout_secs` 值持久且钳制文案在位。clamp 钳制半（999→100）未覆盖 |
| M115 anthropic 探活改 POST | **未覆盖** | 模型行插头按钮的合成点击无响应（探活请求零发出）；判定放宽逻辑（4xx 非 401/403/404 判通）代码已核读；503 假服务场景旁证 5xx 判失败 |
| W13–W15 | 不适用 | 需 Windows 实机 |

## 三、新发现（5 条，全部待拍板后修复）

### D-M95-1（P1）`Bearer env:XXX` 组合形态静默透传——引用机制被 HTTP 惯例绕过

- **现象**：模型按 HTTP 惯例传 `{"Authorization": "Bearer env:WM_SMOKE_SECRET"}`，httpbin 回显收到的就是**字面量** `Bearer env:WM_SMOKE_SECRET`——引用没有被解析、也没有报错。
- **根因**：`resolve_secret`（bot/tools/http.rs:48）只认**整值**以 `env:`/`keyring:` 开头；`Bearer ` 前缀导致 `strip_prefix` 不匹配 → `Ok(raw)` 原样出门。锚点测试只覆盖裸 `env:XXX` / `keyring:XXX`。
- **影响**：schema 写的是「鉴权头的值支持引用形式：keyring:厂商名 / env:变量名」，但 Authorization 头的形态几乎必然是 `Bearer <值>`——**按惯例用法时「以为带了鉴权」的请求裸奔**（恰是 resolve_secret 注释里说要防的事故），服务端 401 才发现。
- **修复方向（待拍板）**：a) 支持 `Bearer ` 前缀下的引用解析；b) 至少在值中检测到未解析的 `env:`/`keyring:` 子串时**报错拒发**（fail-closed）。

### D-M95-2（P1）`keyring:不存在的厂商` 不报错、发空 header——违反方案明确判据

- **现象**：`{"X-Test": "keyring:这个厂商不存在XYZ"}` → HTTP 200 发出，httpbin 回显该头为**空串**。方案 M95 判据原文「应当明确报错说读不到，不能静默发一个空 header 出去」。
- **根因**：`read_vendor_key` 对 keyring `NoEntry` 走 `classify_get_password` 的良性 fallback（`Ok("")`）——该 fallback 是为「未配置」语义设计的，但 resolve_secret 拿到空串照发。read_llm_key 主链路有 has 前置 + `VendorKeyMissing` 报错，resolve_secret 这条路没有等价守卫。
- **修复方向**：resolve_secret 内对 keyring 路径区分 NoEntry（报「厂商未配置」）与真故障；空解析结果一律拒发。

### D-M99-1（P1）子 agent 汇报路径 thinking 未回传 → 上游 400 → 汇报重试 41 次全败

- **现象**：DeepSeek anthropic 端点（thinking.budget_tokens=4096）下，子 agent 本体 succeeded 后，**汇报（subagent report）LLM 请求连环 400**：`subagent_report_failed | error=400: The content[].thinking in the thinking mode must be passed back to the API`——2 秒间隔重试 **41 次**全败（09:41:10–09:42:05），主 agent 收不到汇报。
- **根因**：M99「发了就回」的盖章点覆盖了主聊天多轮、自检返工、幻觉守卫返工、长度续跑——**汇报路径是第五条**，漏了。
- **影响**：thinking 模型下子 agent 的产出永远汇报不回主 agent（重试烧 token + 拖满 SUBAGENT_CHECK_WAIT_MS_CAP）。
- **修复方向**：汇报请求的消息装配与主循环共用同一段回灌盖章逻辑（抽公共函数或汇报路径接入同一 stamp 点）。

### D-M110-1（P1）SSE 多行 data 重拼用 `\n` 分隔——「代理切断长 JSON」场景必坏

- **现象**：假 SSE 服务把一条 `content_block_delta` 的 data JSON 切成两条 `data:` 行（模拟代理按 token 边界断开重发超长行）→ 重拼结果 `"delt\na": {...}`（JSON key 中间被塞进真实换行）→ 解析失败 → `llm.sse_chunk_unparsable`（Warn）→ **正文整块丢失**（UI 空泡）。split 与 comment 两模式均复现。
- **根因**：`sse.rs:507` 按规范字面实现「多行 data 用 `\n` 拼接」。该规范针对「发送方主动多行」，而本卡靶点「代理切断超长行」重拼必须**无缝**。
- **影响**：正文本该由这次修复（bf228f2c/931e89fa）救回来的场景，恰恰是现在丢得最干净的场景——有留痕（unparsable Warn）但正文全丢。
- **修复方向（待拍板）**：拼接顺序改为「先无缝 join 试解析，失败再降级 `\n` join」；或对 data 片段检测 JSON 断点位置决定是否补分隔。

### D-M101-1（口径观察）tool_in_flight 把工具执行期的停滞判定整体挂起——「停滞看门狗是子 agent 唯一时间维度」的口径在工具执行期不成立

- **现象**：SUBAGENT_STALL_TIMEOUT_SECS=20 下，子 agent 跑 `while True: sleep(1)` 死循环脚本：20s **不判停滞**（tool_in_flight 挂起判定），兜底只剩 run_python 自身 120s 超时；工具超时返回后模型再发工具，判定又被挂起——停滞看门狗在「工具执行期」永不触发。
- **定性**：长工具不被误杀（M101 修①的目标）与真卡死 20s 强停（方案判据第 3 步）**在实现上互斥**，修①选择了前者。行为安全（无永久卡死：工具超时 + MAX_ROUNDS 双兜底），但与方案「子 agent 没有墙钟后 600s 停滞是唯一时间维度」的表述冲突。
- **修复方向（待拍板）**：tool_in_flight 挂起的同时给工具执行期一个独立上限（如 max(工具超时, 停滞阈值)）；或接受现状并把方案/SPEC 的口径改成「工具内卡死由工具超时兜底」。

## 四、变异注入结论（对应方案 §5.2）

| 编号 | 卡 | 注入后判据 FAIL？ | 观测 |
|---|---|---|---|
| V97 | M93 | **成立** | `(allow network*)` 注入后探测打 `SB_ERR 61`（ECONNREFUSED）→ `seatbelt_unavailable`（「沙箱内 socket 到了内核——deny network\* 未拦截」）→ **fail-closed 拒跑**；run_python 不再执行联网脚本 |
| V125 | M108 | **成立** | 豁免删除后提问挂起 28s 被看门狗判死（trace 442 stopped）——豁免确实在守 |
| 其余 V98–V135 | — | 未跑 | 时间与构造条件限制（详 §五） |

> V97 首次注入曾出现「假阴性」教训：python 脚本替换命中的是**注释行**（文件里第一个 `(deny network*)` 在 doc-comment 里），常量未变、二进制仍旧——复验变异类注入必须 `strings <二进制> | grep` 确认注入进产物，再跑场景。

## 五、未覆盖（诚实清单）

| 项 | 原因 |
|---|---|
| M94 每跳 302 判定 | 无可控公网 302 源：httpbin redirect-to 403 拒外部域、v.gd 返回 meta-refresh 页（reqwest 不跟）、is.gd/tinyurl 本地网络不可达（Connection reset/deadline）、本地 302 服务被公网闸正确拦截（环回/私网拒——本身是公网闸生效的旁证）。代码层已核读（http.rs:117 循环头每跳 check_destination）+ destination.rs 单测兜底 |
| M94 配置写入失效半（invalidate_grants） | 需设置页 UI 保存动作触发运行时写入；手动改文件+重启会连会话授权一起清，测不出独立效果 |
| M93 apps 同享半 | 程序页需上架 app 才能跑，驱动成本超预算 |
| M97 200/400 跑飞场景 | 构造 200 轮对话成本过高；常量与熔断阶梯由 audit_sop_baseline 门禁对拍 |
| M98 工具轴对照半 | 400 次工具调用构造不现实；轮数轴已验（axis=rounds 可区分） |
| M99 动态半（文案自节流） | 模型服从度不可控；静态对齐 + 常量对拍门禁兜底 |
| M105 带图量尺对比 | UI 发图（+按钮菜单/文件对话框）合成点击不可达；量尺实现人工核读 + 单测兜底 |
| M102 A 路/C 路/计数半 | A 需设置页配 per-tool 规则；C 需子 agent 主动试档外工具（recon 子 agent 未试）；计数半依赖 A/C 构造 |
| M103 executor/verifier 档、显式优先、失败降级 | 同一机制（attach_stage_onion）已被 recon 档验证；逐档重复驱动成本>边际价值 |
| M107 临界同时应答半 / oneshot 关闭自旋半 | 3s 超时窗口人手不可行；删除会话时机难精确卡在等待环内 |
| M108 判死-新轮不被清场半、切走切回半 | 判死-新轮需 120s 静默工具 + 精确时序；世代守卫有单测兜底（4ce6a1ca/30852dbf） |
| M109 /stop 打断确认窗、重载还原倒计时、FIFO、信封 | 需 webview reload 与多弹窗并发时序；确认弹窗点击（合成）在 danger 卡上不可达 |
| M112 薄片 500ms 计量、落库重试半 | 停止语义已验（stopped）；落库需 BEGIN EXCLUSIVE 持锁构造 |
| M113 全卡 | 模型不配合 parallel tool_calls（两次引导均串行）；MCP 坏服务器未配置 |
| M115 全卡 | 探活插头按钮合成点击无响应（请求零发出）；判定逻辑代码核读 + 503 场景旁证 5xx 判失败 |
| M114 clamp 钳制半 | final_selfcheck_round_gap 无 UI 入口；改文件+重启成本>价值 |
| V98–V96 其余变异 | 时间预算；已跑的 V97/V125 覆盖了「fail-closed 无放行开关」与「看门狗豁免」两个最关键防线 |
| W13–W15 | Windows 实机不在本机 |

## 六、环境操作台账（对应方案 §5.4）

| 改了什么 | 改在哪 | 还原了吗 |
|---|---|---|
| MAX_ROUNDS 200→10（M98） | bot/constants.rs:23 | ✓（git diff 零残留） |
| SUBAGENT_STALL_TIMEOUT_SECS 600→20（M101） | bot/constants.rs:264 | ✓ |
| ASK_TIMEOUT_SECS 24h→3s（M107） | ask_engine.rs:152 | ✓ |
| panic! 注入（M111） | bot_model_loop/shell.rs:166 | ✓ |
| V97：`(deny network*)`→`(allow network*)` | py/seatbelt.rs:20（含首次误改注释行） | ✓ |
| key_backend 强制 PlaintextFile（绕钥匙串弹窗） | bot/config/keyring.rs:73 | ✓ |
| V125：看门狗豁免删除 + STREAM_DEAD_IDLE_MS 660s→15s | useChatStream.ts:533 / ChatPanel/constants.ts:45 | ✓（两处均还原） |
| bot-config.json：permMode ask↔yolo、allowedDomains 加删、旧键注入、reasoner 测试条目、vendor 字段删/还原、apiProvider/baseUrl/activeModelId 多次切换 | data_dir/bot-config.json | ✓（恢复 anthropic + DeepSeek V4.1 Flash + yolo，reasoner 条目已删，vendor 字段已还原） |
| 明文 key 文件 2 个（bot-api-key.txt / bot-api-key-vendor-DeepSeek-36cafc1b.txt，security CLI 读出、0600） | data_dir/ | ✓（已删除，测试结束即删——对应「密钥移入、测完删掉」口径；WM_SMOKE_SECRET env 最后一次启动起不再注入） |
| 假服务 fake_sse.py（anthropic SSE 三模式）/fake_503.py | /tmp/wm-smoke-servers/ | 进程已杀；脚本留 /tmp（不进仓库） |
| M96 测试文件（long-line/cn-300/en-3000）与 M94 产物 wmtest.png | /tmp/wm-smoke/ 与 data_dir/AI_Gen_Files/ | 留置（临时目录/产物目录，不碍事） |
| 收尾校验 | `git diff --stat -- src-tauri/ src/` 为**空**（源码零残留）；唯一工作区 diff 为并行会话的未提交文档（未触碰） |

**遗留给用户**：应用正在运行（干净 HEAD 产物 + 还原后配置）；keyring 走回了 System 后端，**启动/首次请求会再现钥匙串 ACL 弹窗**（既有拍板项，条目 ACL -A 待拍板）——需要手动点一次「允许」。

## 七、驱动坑（方法论）

1. **SecurityAgent 钥匙串弹窗完全免疫合成输入**（CGEvent 点击/键盘/double-click 全部无效）——之前观察到的「点掉了」实为 securityd 超时自动撤窗。本次最终绕法：`security find-generic-password -w`（CLI 授权路径可读值，不弹 GUI）+ 临时改 key_backend 走明文降级文件，测完全部删除。重编译会改 CDHash → ACL 失效 → 必弹窗，测试期尽量减少编译次数。
2. **应用退出时写回配置**会覆盖测试者手改的 bot-config.json——改配置必须在进程完全退出（pgrep 确认）之后。
3. **会话级模型选择压过全局 activeModelId，且请求模型名取自顶层 `model` 字段**——UI 下拉的「默认」标签不等于跟随配置里的 active；改配置切模型必须同时改顶层 model + activeModelId（且条目必须 enabled=true，disabled 条目会被 resolve 静默回退到第一个 enabled 条目）。
4. **danger 确认弹窗/提问卡的按钮位置随内容高度变化**——固定坐标点击不可靠，每轮弹窗都要现场截图定位；「回答并继续」按钮的成功命中案例全在双击或 400ms 间隔双击。
5. **变异注入必须验证产物**：`strings <binary> | grep` 确认注入串进了编译产物（本次 V97 首次替换命中的是注释行，产出一个假阴性）。
6. **`osascript keystroke` 是发给前台应用的全局事件**——`tell process X to keystroke "q" using command down` 限定不了目标，X 不在前台时 **Cmd+Q 会退出前台应用**（本次多次把宿主 IDE 退掉的就是它）。停 wmessage 用 `kill -TERM`；粘贴/回车类合成输入前必须先确认目标 frontmost，否则内容会落进前台窗口。
7. **`pkill -f <模式>` 会自匹配杀掉承载命令的 shell**（bash -c 的命令行含模式串，pkill 只排除自己不排除父 shell）——会话反复异常中断的元凶之一。用 `pkill -f '[t]模式'` 字符类技巧或 pgrep+kill 指定 pid。
