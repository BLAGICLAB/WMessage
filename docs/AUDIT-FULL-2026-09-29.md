# AUDIT-FULL-2026-09-29 — 全仓多角度审计 + 自进化专项复审（合并版）

> 日期：2026-09-29 · 方式：Mimosa 密封深度扫描 + 6 路并行审计 + 自进化专项深查（代码评审 + 真实运行测试）
> 关联：`AUDIT-FIX-PLAN-2026-09-29.md`（修复方案）；本文取代口头两轮汇报，是两轮审计的唯一定稿口径。

## 0. 总体结论

- **未发现 P0 级安全漏洞**；测试全绿（前端 333/333，Rust 1248 passed / 0 failed），tsc 零错误，knip 零报告。
- 四件核心事项：① 未提交的 MCP 改动合入前须修 3 个点；② **自进化功能经复查确认为真实功能（保留拍板成立），但存在 2 个 P0 级真 bug**；③ Rust 供应链无自动化 CVE 视野 + clippy 262 条告警；④ 过程性文档缺归档出口。
- 上轮 ponytail「自进化栈约 5900 行零生产调用」的结论**部分不成立**，本文 §6 已修正（observe/shadow.rs 实为真接线；其余按「真接线 / 有意停摆 / 纯开发工具」三类重新归档）。

---

## 1. 安全审计

### 1.1 Mimosa 密封扫描（deep）

- Scan ID：`scan-2026-09-28T22-29-12.285Z-6801a8ed0294` · seal：`sha256:2456698676795c01d8ff5150ece22cd9d9feba4ac30930bbd9b4ed156549ef0b`
- 产物：`~/.mimosa/security-scans/project-35c8c4f5947b2bbdf573b390/scan-2026-09-28T22-29-12.285Z-6801a8ed0294`
- 依赖离线比对：1047 包，0 命中。运行状态 **inconclusive**（动态派发覆盖缺口），verdictEffect: none。
- 13 条 finding 人工裁定：
  - 12 条（6 HIGH + 6 MEDIUM）同链 `py/document.rs → eval/config.rs:64` 路径穿越 → **误报**。实证：`py/document.rs` 通篇无 `eval` 引用；sink `eval::config::load` 唯一调用方是开发期 CLI，路径模块内写死，无「Web 请求输入」来源。
  - 1 条 MEDIUM「MongoDB 排序注入」位于 `src-tauri/target/doc/static.files/search-*.js`（rustdoc 构建产物）→ 误报；**后续扫描应排除 `target/`**。
- 依纪律声明：以上为静态 advisory 的人工研判，不构成「项目完全安全」结论。

### 1.2 攻击面人工核查（7 项）

| 攻击面 | 结论 | 等级 |
|---|---|---|
| tauri.conf / capabilities | CSP `default-src 'self'` 无 unsafe-inline script；opener 限 `$APPDATA/**` 且有防回退测试锁（lib.rs:569-609） | P3 |
| invoke 命令面 / bot_fs | 约 60 命令；canonicalize + 分量级前缀比较 + unix inode TOCTOU 复查（bot_fs.rs:194-314,475-501），三组真实 FS 单测；残留：`copy_file_with_title`（lib.rs:53）无路径校验、Windows 无 inode 复查 | P3 |
| 本地 HTTP API (127.0.0.1:4763) | Bearer UUID token + 恒定时间比较 + 0600 原子写；vendor tiny_http slowloris patch 实证生效；SSE 鉴权在后 + 32 客户端上限；无 CORS 头；残留：不校验 Host 头（DNS rebinding 面，token 兜底） | P3 |
| Python 子进程 | 资源隔离完备（unix rlimit / Win Job Object / 组杀 / 看门狗）；**文件系统零隔离**，schema 文案「本机沙箱」有误导；「远程内容→模型→run_python」为最大本地外泄链路 | **P2** |
| MCP 宿主 | stdio 白名单双验 + 字面量 spawn，命令注入面封死；URL 双闸 + 禁重定向；残留：工具条数/inputSchema/structuredContent **无上限**、无提示注入来源标注、DNS 解析级校验缺（文档化接受） | **P2** |
| keyring / 密钥 | 无密钥落日志路径；明文 fallback 仅 Linux 无 dbus（0600+自动回迁）；MCP env/headers 明文存 bot-config.json（模块头已声明，不同级于主 key） | P3 |
| tool_guard 确认 | fail-closed：60s 超时 Deny、uuid oneshot 防重放、后台/不可见直接拒绝；残留：挂件隐藏时确认静默拒绝（可用性陷阱） | P3 |

**安全 Top 5 待办**：① MCP 数据面定界（工具总数 / schema 体积 / structuredContent 预序列化上限）；② `mcp_server_save` 后端二次确认（封 webview JS 直接 invoke 的纵深缺口）；③ `run_python` 去「本机沙箱」措辞 + MCP 结果进上下文加来源标注；④ MCP HTTP 传输 DNS 解析级校验；⑤ bot-config.json 中 headers/env 预留 KeySlot 迁移路径。

## 2. 未提交的 MCP 集成改动（17 处变更）

整体可合入。**合入前必修 3 项**：

1. `bot/registry.rs:964-974`：`&base[..base.len()-2]` 摘尾仅 `debug_assert` 保护，release 下契约破坏 → 非法 JSON → `bot_model_loop.rs:543` `from_str().unwrap()` panic。改 checked 回退。
2. MCP 数据面定界：`manager.rs:199`（发现结果无条数上限）+ `mount.rs:56`（inputSchema 原样透传无体积上限）+ `mount.rs:212`（structuredContent 先完整序列化再截 30k，内存尖峰）。
3. e2e 测试去 `shared()` 进程级单例（`registry.rs:608` + `manager.rs:598-680`），消除并行 flaky。

次要：`manager.rs:451` 格式破损；`manager.rs:217-233` stderr 环形单行无长度上限；`manager.rs:199-202` 子进程继承父进程全量环境（建议 env_clear + 白名单）；`McpPanel.tsx:275` timeoutSecs 未钳 5..600；确认弹窗不含 http headers（常含 token）；TS `MCP_STDIO_LAUNCHERS` 与 Rust 白名单手工双维护（建议源码锁单测）。

## 3. Rust 后端架构与质量

总体：分层清晰；非测试路径 unwrap/panic 全仓约 12 处均有理由；错误码 24 机器码 + 前端 parity 脚本；锁临界区短且有锁序注释。

- **P1|750 行巨型函数**：`bot_model_loop.rs:513-1262` `run_model_loop_core`；同类 `bot/tools.rs:553`（445 行）、`migration/run.rs:65`（400 行）。
- **P1|Python 全局串行闸**：`py/runtime.rs:300` `PY_RUN_GATE` 持锁跨子进程全生命周期（≤300s），无队列上限/公平性/取消传播；文档转换共用此闸。建议 `Semaphore(2~3)`。
- **P2|SUBA-3 并发闸边角**（`bot_orchestrator.rs`）：排队 50ms 轮询不查取消令牌（:820-852）且排队期占住 ExecGuard；Running 在真正运行前写库（:926-938）；墙钟超时只 drop future 不停在飞工具（:1193-1202）。
- **P2|LLM 流式 300s 全程超时**（`bot_model_loop.rs:412-415`）：长流 >5min 整轮失败且流开始后无恢复；每运行新建 reqwest::Client。
- **P2|工具 JSON panic 面**：`bot_model_loop.rs:543-545`（含外部 MCP description 的拼装结果 unwrap）；`bot/registry.rs:951` 指针比较判分支。
- P3：legacy data.json 迁移静默失败（db/migrations.rs:108-148）；嵌入引擎失败永久缓存（memory/embed.rs:35）；每操作开新 SQLite 连接（db/mod.rs:57）；EventHub 锁内文件 IO（api_server.rs:150-165）；并发闸 FIFO 名存实亡（bot_orchestrator.rs:838-843）。

## 4. 前端 React/TS 质量

亮点：tsc 零错误、生产 0 处 `any`、0 快照测试、错误码 Rust↔TS 防漂移、react-markdown 默认转义（无 rehype-raw / dangerouslySetInnerHTML，`javascript:` 降级不可点）、密钥仅 keyring。

- **P1|ChatPanel 1851 行**（22 useState + 15 useEffect）；`ChatPanel.tsx:473` 流式每 rAF 帧 setMessages 拷贝整数组 + `:1382` `key={i}` 无 memo → **流式期间每帧重渲染全部历史消息**。
- **P1|无 linter**：6 处 `eslint-disable` 为死注释（App.tsx:411,421、useTauriListen.ts:28 等）。
- P2：拖放监听注册竞态泄漏（ChatPanel.tsx:1160-1168）；McpPanel `window.confirm` 违反「confirm 走主窗口」拍板（McpPanel.tsx:309 vs App.tsx:715）；`transport: string` 应收 union（McpPanel.tsx:23、SettingsPage/types.ts:63）；tsconfig 缺 `noUncheckedIndexedAccess`；McpPanel 两处 800ms setTimeout 未清理（:296,:331）。
- P3：`McpPanel.tsx:505` 占位符笔误；McpPanel 测试缺编辑回填 / http 分支 / 非法 timeout 用例。

## 5. 测试 / 构建 / 依赖健康度

| 检查项 | 状态 | 摘要 |
|---|---|---|
| tsc | ✅ | 0 错误 |
| vitest | ✅ | 333/333（41 文件） |
| knip | ✅ | 零发现 |
| clippy | ✅/⚠️ | 0 error，**262 warnings**（48 种 lint；含 tests/task_chat_exec.rs 3 处 `await_holding_lock`） |
| cargo test | ✅ | 1248 passed / 0 failed / 4 ignored |
| npm audit | ✅/⚠️ | 生产 0；dev 3 moderate（vitest 链 GHSA-82fw-gwwq-j7x9，fix 可解） |
| cargo audit | ⏭️ | 未安装 |

供应链：tiny_http vendor patch 实证落地且有 CI 守卫；rmcp 3.5 feature 取舍合理（CVE-2026-63128 仅注释自述，未独立核验）；**ort 2.0.0-rc.13 + download-binaries 构建期联网下载 = 可复现构建风险**，建议 onnxruntime 二进制随包分发。

## 6. 自进化专项复审（保留拍板后的深查）

### 6.1 接线真相（修正 ponytail 结论）

| 类别 | 模块 | 证据 |
|---|---|---|
| 真接线，有运行实证 | derive / emit / apply / proposal / change.record / panel 8 命令 / trace（占位） | consolidate.rs:413 → post_consolidation；lib.rs:312/506-513；bot.log 有 `evolution.proposal` 行；applied.jsonl 5 条真实写入；DB 2 条存活 lesson；9-19 用户真实操作过面板 |
| 真接线但实际停摆 | observe/shadow.rs | apply.rs:140-247 在 is_enabled 时真实调用（「零生产调用」结论不成立）；但 flag 被 P0-1 抹掉，实际恒关 |
| 有意停摆（设计） | activation 状态机大半、9 态 change 状态机、TTL、conflict、B 校准全链 | 与 OBSERVATION_STATUS.md「工程完成，等使用」一致；S2 占位不可达保证逐条核对成立 |
| 纯开发工具 | eval/ 全部、observe/{metrics,stop,synthetic}、bin/{eval_run,observe_run} | 唯一入口是两个 CLI |
| 假接线（文档不实） | sandbox/kill_switch | kill_switch.rs:23-24 声称「apply.rs 入口检查」，apply.rs 无任何 KillSwitch 引用 |

### 6.2 实测结果（2026-09-29，全程 /tmp 沙箱，仓库零污染）

- cargo build --bins ✅；11 过滤器 555 项断言全绿（evolution:: 275、shadow:: 52、candidate:: 48、eval:: 34 …）。
- observe_run 实跑：metrics / `--synthetic` / `--check-stop` 三模式通过；**`--synthetic` 会 truncate 覆盖 `../evolution.synthetic/`，严禁在仓库根直接跑**。
- eval_run 实跑：**不需要 API key**（feedback 为启发式聚合）；855 case 跑通，指标计算正确；现状因 bot-config.json 缺 `evolution.eval` 块需自备 `--config`。
- evolution.synthetic/ 5 个 jsonl（855+100+55+40 行）全部合法且与当前 schema 一致。
- 前端 EvolutionPanel 13/13 通过。短板：面板 8 命令仅单元层覆盖，无 AppHandle 级集成测试。

### 6.3 发现（自进化）

- **P0-EV1|BotConfig 写路径静默抹掉 `evolution` 块**：`bot/config/types.rs:78` BotConfig 无 evolution 字段/flatten 保底，`io.rs:269-278` 全量序列化——任何配置写（含每轮 consolidation 的 persist_last_run）都删块。运行时实证：`target/debug/bot-config.json`（9-28）evolution key 已消失。「观察态等真数据」第 0 环已断。
- **P0-EV2|自动应用的 lesson 撤不掉**：apply 不写 ChangeRecord（rollback UI 查不到，commands.rs:430-437）；`evolution_delete_proposal` 级联不删 `evo:` lesson（commands.rs:189-192）；`rollback_applied` 零生产调用。实证：9-19 删除全部 3 提案后，DB 仍存活 2 条 evo lesson（importance 4/3）持续经注入影响行为。
- **P1-EV3|幂等被推翻 + 记忆劫持**：applied.jsonl 实证同提案 apply 3 次（lesson 被 consolidation merge 吸收丢 tag → 查重失效）；`insert_item` merge-on-write（store.rs:230+，cosine≥0.92）把 lesson 内容+tags UPDATE 覆盖到既有记忆行（行 cb60ad9b 实证被 16:51 lesson 劫持），此时回滚=删原记忆。
- **P1-EV4|changes.jsonl 多写者绕锁**：shadow.rs:356-366 不持 `EVOLUTION_STORE_LOCK`（mod.rs:77「一把锁」声称不实），与面板 RMW rewrite 并发有丢更新窗口；shadow 对同 id 无条件重复 append Pending 行。
- **P1-EV5|二次回滚永久卡死**：toggle ON 的 dedup 不含 RolledBack（commands.rs:101-107）→ 同 change_id 二次入行；rollback 用 `position()` 首匹配（:485-490）永远命中旧行。
- **P1-EV6|生产 shadow 丢失失败率告警**：trait 版有 `audit_warning`（shadow.rs:222-226），生产入口 `shadow_apply_for_batch_with_app` 收尾无告警调用。
- P2-EV7|delete ≠ 废案：merge 类提案 id 是稳定泛化 id（derive.rs:75/143 + proposal.rs:140 数字归一化），删后同 id 重生（实证同 id 两条不同 merge）；且所有 merge 提案共享一个 id。
- P2-EV8|eval_run applied 路径推导错误（runner.rs:174-182 得到 `eval_set.applied.jsonl` 而非 data_dir 的 `evolution-applied.jsonl`，无 `--applied` 覆盖）→ 回滚率/污染存活期恒 0；`case_passed = cases.len()` 恒 100% 占位。
- P2-EV9|9 态状态机 / TTL / conflict 生产死代码 → 候选池只增不减、Pending 永不推进 → observe 指标 approval/rollback/pollution 结构性失真。
- P2-EV10|trace 采集占位：bot_chat.rs:947 只传 Success/Aborted、tool_calls 恒空 → Failure 与 >10 工具调用规则不可达。
- P2-EV11|read_jsonl 首行损坏持续 fail-closed 无自愈（mod.rs:49-57），该文件无备份。
- P3：panel 裸连接写 SQLite 不持 DB_WRITE_LOCK（commands.rs:190,512）；`evolution_toggle_proposal` 无状态校验可复活 Rejected/Expired；promote 段间 TOCTOU。

### 6.4 文档漂移裁定

- OBSERVATION_STATUS.md §4/§5（S0/S1 不调 evaluate、S2 占位 Allow、Block 不可达）与代码**逐条相符**。
- 其「shadow.enabled: true」声称**已被运行时证伪**（P0-EV1）。
- 「969/969」「812 通过」为历史值；此后加入的 write_proposals/shadow 接线正是本轮 P1-EV4/EV5 病灶，文档未同步。

## 7. 过度设计（保留自进化后的残余项）

- middleware.rs 注册表机制 311 行 ≈ 3 个顺序函数调用（可缩至约 110）。
- api.rs TaskStore trait 单实现（MemStore 仅测试）；`now_ms()` 5 份重复；`StreamEnd` 枚举 4 变体 `allow(dead_code)`；`bot_artifacts::peek()` 无调用方。
- evolution 栈内**纯开发工具**（eval/ + 2 bins + observe 三件）用户已拍板保留；真正待决策的残余死代码：change::status 9 态机、candidate::ttl、candidate::conflict、sandbox/（除 fnv1a）、activation 的 save_state/shadow_route 等零调用函数——去留与「接线激活」二选一（见修复方案 B4 拍板项）。

## 8. 仓库卫生与文档

- pack 82.26 MiB，模型 blob 56.19MB（68%）。单次提交、几乎不变 → 直列入库可接受；模型若迭代再议 LFS/并行目录。
- docs/ 230 文件中约 100+ 一次性产物（AUDIT-* 20、HANDOFF 4、OCR-CODE-REVIEW 三格式 8、三个已完结专项目录）；过程:参考 ≈ 175:55。建议 `docs/archive/`。
- README.md 目录树 4 处过期（db.rs→db/、config.rs→config/、migration.rs→migration/、ChatPanel.tsx→ChatPanel/）；SPEC.md 应加「历史存档」声明。
- 根目录 5 个 evolution 设计稿该进 docs/evolution/；health-check.sh 被 ignore 仅存本地（换机即丢）应收编 scripts/；README.txt 该进 packaging/。
- 健康：ignore 零泄漏、脚本/钩子链路自洽、tests-audit/ 活门禁、DEVLOG 高质量（有 10 天增量未提交）。

---

## 9. 结论

自进化功能**今天实测能跑通**（编译/555 单测/两 CLI 全分支/数据 schema/前端 13 测试全绿），主闭环有真实运行数据；但「能用 ≠ 能撤」（P0-EV2），且观察态第 0 环已被 P0-EV1 掐断。修复顺序与批次划分、每项验收标准见 `AUDIT-FIX-PLAN-2026-09-29.md`。
