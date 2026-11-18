# Agent 全量多角度审计报告（2026-11-17）

> 基线 commit `0b3d2265`；方法按 `docs/agent-audit-sop-zcode.md` 四段式（体检 → 深度审计 → 复核 → 修复）。
> 报告正文与逐条 Receipt 见 `/tmp/agent-audit-2026-11-17.md`（仓库外，临时）；本文件是**归档 + 处置状态**。
> 审计范围：Agent 核心链路（`src-tauri/src/bot*` / `py/` / `prompts/` / `bot_fs.rs` / `bot_web*`）
> + Agent 功能全景（MCP / 技能 / 子 agent / 定时作业 / 提醒 / 持久化真源化 / 流程状态机）+ 架构一致性。
> 8 个只读子代理并行（SOP A~J 十维 + 扩展 K/L/M 三片 + 体检），每片自做「回读定位 / 反向找反例 / 找调用方」
> 三动作复核，主审另抽查 4 条关键 Receipt 全成立。

## 一、结论

| 等级 | 数量 |
|---|---|
| 阻断 | 0 |
| 高 | 2（均在 MCP OAuth 面） |
| 中 | 22 |
| 低 | 28 |

**没有阻断级**——安全闸可绕过 / 预算可绕过 / 崩溃级问题均无。

## 二、已修（全部落地，四批 24 笔，未推送）

### 批一 · 高危两条（`ddc00174` `f87d708a`）

| 编号 | 问题 | 修法 |
|---|---|---|
| K-1 | OAuth 派生端点（发现第二跳 root / DCR / token endpoint）全复用钉原 host 的 client 直发，不过公网闸——恶意服务器可把宿主变成内网 SSRF 跳板 | 每个请求点各自过闸（字面量级 + DNS 全 IP 公网判定）并钉当跳 IP；`authorization_endpoint` 交浏览器前补字面量级闸。锚点：请求函数签名收口断言（出现外部 client 即红） |
| K-2 | OAuth 降级凭据文件走无 mode 的 `atomic_write`（0644）——Linux 无 keyring 时 access/refresh token 明文全局可读 | `raw_read/write/delete` 抽后端注入内核（同 McpSecrets 的 `_at` 先例），PlaintextFile 落盘统一走 `write_tmp_0600` + rename。锚点：0600 断言 |

### 批二 · 中危 13 条（`c18c295d` `fb27dac8` `9f2fa77e` `f1d75dbc` `a7a771a7` `09eb01ee`）

| 编号 | 问题 | 修法 |
|---|---|---|
| K-6 | 降级凭据 RMW 把 I/O 错误与 JSON 损坏都当「空 map」→ 一次写入抹掉其余服务器 token | NotFound 才算空、其余上抛、损坏 fail-closed |
| K-3 | 工具发现完成无条件插回 Idle 槽位 → 用户禁用期间产生幽灵条目（每轮清单都出现） | 插前核对启用快照，不生效记一条审计 |
| K-5 | stdio 行长闸对「待应答消息被截」与「噪声行被截」一视同仁跳过 → 请求永远等不到应答、只能等超时 | 按行首前缀判定 JSON-RPC 消息则读侧响亮失败；噪声行维持跳过（会话不断） |
| L-2 | 回合落库事实清场点在模型循环内部 → 配置/密钥早退时失败行取走上一轮残留，正文串台落库 | 清场提到命令层，早于主链 |
| L-3 | 开票与收尾都 fire-and-forget、次序不定 → 收尾 UPDATE 先于开票 INSERT 即静默 no-op，残行停 working 到超时 | 开票写完才进模型循环；收尾闸从 trace 开档成败上摘掉 |
| L-4 | 汇报轮照样开票与收尾用户轮的工作项行 → 用户轮的失败信号被唤醒又抹掉 | 开票与收尾两处都过用户轮闸（纯函数 + 接线守卫） |
| L-5 | 工作项成功终态不清残留 error | 成功时 `error = NULL` |
| D-1 | 出处池把「【系统提示】」注入算出处 → 守卫返工提示带着被判的数，模型复述即自洗白 | 出处池排除该前缀 |
| D-2 | 证据池只留头部而漏斗给模型看的是「头+指针+尾」→ 真话被误判无出处、返工还诱导重跑工具 | 证据池改头尾各留一半 |
| C-1 | 父 agent 提示教验收段「用 research profile」，与运行时相反（validator 的 profile 是 coder）→ 收窄掉 run_python/ast_inspect | 改「只传 stage=verifier、不要传 profile」+ 锚点 |
| M-1 | 模块树行数标注漂移 22 处 → `audit_module_map` 对拍门禁自 11-16 起一直红（pre-push 拦） | 回填行数 + 散文总行数同步 |
| K-4 | MCP stdio 树杀把「收尸」放在「树杀」前 → 组长被 reap 后组杀护栏失效、孙进程已 reparent 收不到，npx→node 孤儿恒漏 | 新增 `stop_tree_with_snapshot`（第一轮用事先拍好的快照）；close 在关 stdin 前拍快照、drop 顺序反转；`killable_now` 抽纯函数 + 锚点 |
| A-2 / I-1 | SOP 三处过期（常量 55→64、工具 67→69、上下文装配接线指向）+ 闸序表缺 0.57 级、早退口径写准（8 构造处 vs 9 路径）；SKILL-DSL 三处失真（max_steps 默认 50→100、上限 100→200、timeout_secs 标注废弃） | 文档同步 |

### 批三 · 安全向三条 + 低危五处（`f56c3280` `35723ad8` `67c42174`）

| 编号 | 问题 | 修法 |
|---|---|---|
| H-1 | http_request 的「来源：」行只裁查询串，userinfo（`user:pass@`）原样进三类 sink（入参侧早已用 `host_str()` 裁掉，同口径自相矛盾；重定向 Location 的 userinfo 更是服务端可控） | 新增 `strip_url_secrets`（裁 userinfo+query+fragment），sink 面按工具名分派 |
| H-2 | fetch_url 的查询串进审计面**三处**（入参预览裸 80 字截断、失败文案带完整 URL、自己那条 `audit_log` 写完整 URL 进 bot.log） | 入参与结果各自裁剪；`audit_log` 一并裁。口径：只裁模型自己来源的那个 URL，正文里的第三方链接不动；模型可见面不裁 |
| H-4 | 公网闸与 MCP HTTP 闸各一份 IPv6 保留段判定，MCP 侧更严（6to4/Teredo/NAT64/IPv4-compatible/组播/文档段）——bot_web 侧此前不拒；SPEC 宣称的「判据单源」也没单成 | 判据补进 bot_web 那一份，MCP 侧改为委托它。**方向是收紧**：IPv6-only 网络走 NAT64 well-known 前缀的公网地址也会被拒 |
| K-9 | OAuth 登出清令牌失败被吞且无条件记成功审计 | 上抛，登出没成就不记成功审计 |
| D-4 / D-5 | lookup_receipt 取不到凭证漏了第三种原因（超 500 条查询窗口）；变更话术词表缺「已重命名/已排好/已恢复执行」 | 文案补全 + 词表补齐 + 锚点 |
| L-6 | 手动触发不查派发去重集合 → 到点派发排队时手动插队，同一 occurrence 跑两遍（两张卡两份 token） | 手动入口同查该集合，命中跳过并留痕 |
| L-7 | 模型经聊天建/删提醒不广播 → 定时页停在旧快照 | 工具路径补广播 |
| L-9 | 周期提醒算不出下次触发点时写 `remind_at=NULL` → 永挂僵尸提醒 | 退休即删 |

### 批四 · 论证后选定 10 条（`0abe0aec` `e6ffe936` `090d1e36` `7c53a862` `3f05c2ed`）

| 编号 | 问题 | 修法 |
|---|---|---|
| K-7 | elicitation 数字字段解析失败的错误文案回显用户输入前 40 字——该路径在应答登记**之前** return（永不脱敏），经错误链进审计与 span，违「应答值不进审计/日志」契约 | 只报类型不报原文 |
| K-6(b) | 应答值含引号/反斜杠时服务器回显 JSON 转义形态，指纹只登记原值 → 凭据漏进审计 | 转义形态一并登记 |
| K-10 | 类型白名单里的死条件（第二支恒被第一支覆盖） | 删（随之删只服务该文案的 `truncate`） |
| H-6 | http_request 重定向四种状态全按「保持方法与体」→ 303 该改 GET 丢体、301/302 的 POST 也该改（创建类操作做两遍、请求体搬到另一个源） | 决策抽纯函数 + 锚点 |
| F-1 | Windows Job Object 创建失败时内存/CPU 限额与 kill-on-close 全丢且零日志线索 | 补审计留痕（Unix 侧「未生效必须留痕」同款口径） |
| F-2 | 进程数上限注释声称两机制齐备（Windows 无 ACTIVE_PROCESS） | 注释写准（真补需 Windows 实机验证，本轮不做） |
| L-8 | `/clean` 删全表不发 `bot-msg-dropped`（真源化「每个定向操作都广播」唯一缺口）；前端先清视图再 invoke 且失败 silent | 后端删前取 msg_id 删后广播；前端落库成功再清视图 + 正常提示 |
| K-11 | Connecting 停滞判定（40s 自愈窗）零测试覆盖 | 补纯函数锚点 |
| E-1(a) | 滑动窗桩子宣称「完整结果已存档」而痕迹钳 16KB | 如实披露钳制 |
| C-3 | executor 角色工具行写死 writereditor 档 → 显式给别的配置档时与实际生效集脱节（模型照调必撞闸） | 工具行按实际生效集渲染（常量改函数） |

## 三、论证后明确不修（登记，附理由）

| 编号 | 判断 | 理由 |
|---|---|---|
| G-4 | effect 重投 → 双 runner | **审计描述的主链不成立**：`finalize_subagent` 只写终态、不派 runner；同一 `subagent_id` 的 dispatch effect 只 enqueue 一次。真实触发链只剩「dispatch attempt 收尾 CAS 写失败 × 闸满排队」，概率极低且修复需代次标记新机制 |
| A-4+H-5 | 闸路径与 http_request 的同步 IO | **实际开销远小于描述**：`hydrate_mcp_servers` 有进程内缓存，未读过才敲 keyring → 每次工具调用只是亚毫秒级配置读（总开销 <40ms）；缓存化有安全副作用（权限规则读旧值 = 安全闸放宽） |
| D-3 | 出处字面命中的整词边界 | 守卫整体是「宁可放过」取向，改了可能误伤如实转述 |
| J-1 | 三份内置 yaml 未列 submit_result | 行为无害（`effective_tools` 兜住，交卷出口永远可用）；且内置档启动时只播种不覆盖，存量 yaml 不会被更新 → 改了也不生效 |
| C-2 | spawn 去重键不含 onion/stage | 需加列 + 迁移；偶发（要求目标/验收/档位/模型全同） |
| E-1(b) | 真能拿回超限工具结果的完整版 | 需新机制（溢出文件路径入 span + lookup_receipt 能读）+ 实机验证 |
| F-2（能力部分） | 真补 Windows `ACTIVE_PROCESS` 限额 | 需设 `ActiveProcessLimit` 并在 Windows 实机验证——半生效的限额比没有更危险 |
| M-2~M-11 | 架构面（双实现去重、四个贴线文件拆分、272 个 pub 收窄等） | 属重构，按「不过度开发」登记不实施（M-2 消除的是口径漂移种子，可另议） |

## 四、门禁终态

`cargo test --lib` 2414 绿 · clippy `-D warnings` 净 · fmt 净 · `tests-audit` 58/58 绿 · 前端 1252 绿。

**本批改动全部未过实机**——涉及 OAuth 登录全链、公网闸收紧（可能影响真实联网）、http_request 重定向行为、
守卫判定口径、会话落库、子 agent 提示词，属行为变更面。实机验收卡见 `docs/realmachine-smoke-test-2026-11-15.md`
（覆盖 v0.3.0 后七批，不含本批），本批需另立方案。
