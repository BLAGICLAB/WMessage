# AUDIT-FIX-PLAN-2026-09-29 — 修复方案（分批）

> 依据：`AUDIT-FULL-2026-09-29.md`（两轮审计合并定稿）。
> 前提：自进化功能**保留**（老板拍板）；未提交的 MCP 改动**合入**（先过 B0）。
> 原则：每批独立可验收、不跨批夹带；全部走仓库既有门禁（pre-commit batch-verify + test-fast，pre-push test-all strict）；每批实现后用本地 **ocr（OpenCodeReview）** 做差重复审后才提交。

## 0. 修复工作流（每批固定动作）

```
实现 → 定向测试 → ocr review 复审 → 修 ocr 意见 → batch spec → commit(pre-commit) → 批末汇总 DEVLOG
```

- **ocr 差重复审**：`ocr review`（审工作区 staged+unstaged+untracked；分支级用 `ocr review --from main --to <branch>`）；按输出 P0/P1 意见修复后重跑直至无 HIGH。
- **整文件复审**（重写幅度大的文件）：`ocr scan --path src-tauri/src/evolution/apply.rs` 等。
- **无 LLM 时的规则复审**：`ocr delegate preview` / `ocr delegate rule <files>`，由执行 agent 按 rules 自查。
- **留档**：`ocr session list` / `ocr session export` → 产物落 `docs/OCR-CODE-REVIEW-*`（已 gitignore，不入库）。
- 批内每个 commit 仍需按 pre-commit 门禁配 `docs/batches/` 的 BATCH spec；批完成写 DEVLOG 条目（沿用现有格式）。

## 1. 批次总览

| 批 | 主题 | 对应审计项 | 规模 | 依赖 |
|---|---|---|---|---|
| B0 | MCP 改动合入前必修 | §2 | S | 无（当前工作区） |
| B1 | 自进化数据安全 | P0-EV1、P0-EV2 | M | 无 |
| B2 | 自进化闭环语义 | P1-EV3/4/5/6 | M | B1 |
| B3 | 后端并发与流式稳健 | §3 P1×2 + P2×3 | M-L | 无（可与 B1/B2 并行） |
| B4 | 自进化工具补全与死代码决策 | P2-EV7~11 + §7 | M | B2；含 3 个拍板项 |
| B5 | 前端与工程健康 | §4、§5 | M | 无 |
| B6 | 仓库卫生与文档 | §8 | S | 无 |

## 2. B0 — MCP 改动合入前必修（S）

| # | 改动 | 位置 | 要点 |
|---|---|---|---|
| B0-1 | tools_json 拼装去 panic | `bot/registry.rs:964-974` | 尾部非 `\n]` 时回退返回 base + 记审计，删除对下游 unwrap 的依赖路径（配合 `bot_model_loop.rs:543` 改为 `unwrap_or_else` 落 audit 不 panic） |
| B0-2 | MCP 数据面定界 | `bot/mcp/manager.rs:199`、`mount.rs:50-56,212-226` | 挂载工具总数上限（建议 128，超出截断+audit）；单 inputSchema 序列化体积上限（建议 8KB）；structuredContent **序列化前**按字节 cap |
| B0-3 | e2e 去单例 | `bot/mcp/manager.rs:598-680` | `shared()` 用例改局部 `McpManager::default()`（同文件其余用例已是），消除并行 flaky 与槽位残留 |
| B0-4 | 小项打包 | `McpPanel.tsx:275,309,505`、`manager.rs:451` | timeoutSecs 前端钳 5..=600 整数；修复 :451 格式破损；占位符笔误；两个 800ms 刷新定时器挂卸载清理；SaveConfirmDialog 补 http headers 打码展示；~~删除确认改走 ConfirmMap~~ **实施修正**：App.tsx:715「confirm 走主窗口」指后端 bot-confirm 的渲染窗口（ConfirmMap 是后端工具确认的渲染器），非禁用 window.confirm；删除确认维持 window.confirm（与全仓 11 处有文档惯例一致） |

**新增回归测试**：registry 非法尾部回退；工具数>上限截断+audit；schema 超限截断；timeoutSecs 边界（前端）。
**验收**：`cargo test mcp` + `cargo test registry` + `cargo test model_loop` 全绿；`npx vitest run src/components/SettingsPage`；`ocr review` 无 HIGH。

## 3. B1 — 自进化数据安全（M）

| # | 改动 | 位置 | 要点 |
|---|---|---|---|
| B1-1 | **BotConfig 保真 evolution 块（P0-EV1）** | `bot/config/types.rs:78`、`bot/config/io.rs:269-278` | 方案 A（推荐）：BotConfig 增加 `#[serde(default)] evolution: Option<serde_json::Value>` 原样透传（读写不丢未知块）；方案 B：`#[serde(flatten)] extra: Map` 保底全部未知键。二选一后，**手工恢复**运行时 `bot-config.json` 的 `evolution` 块（shadow.enabled / activation，按 OBSERVATION_STATUS §1 原值） |
| B1-2 | **delete 级联 lesson（P0-EV2 半）** | `evolution/panel/commands.rs:189-192` | `cascade_source=true` 时同时按 `tags[0]=evo:<proposal_id>` 删 mem_items lesson |
| B1-3 | **rollback 够得着自动应用（P0-EV2 另半）** | `evolution/apply.rs`、`panel/commands.rs:430-437` | apply 成功时补写 ChangeRecord（status=Active，source=auto）；**或** rollback 命令兼查 applied.jsonl 按 proposal_id 调 `rollback_applied`。二选一（推荐前者，UI 语义统一） |
| B1-4 | 存量孤儿清理 | 数据修复（一次性） | 对现存 2 条孤儿 lesson（`evo:d1b23b6e4eb80156`、`evo:48f454e7491a62bb`）执行级联删除；操作与结果记 DEVLOG。走正式路径（B1-2 完成后用 delete 或 rollback），不手改 DB |

**新增回归测试**（审阅建议 #1/#6）：配置回写保真（`bot_set_config`/`update_config_file`/`persist_last_run` 后 evolution 键仍在——三种写路径全覆盖）；delete 级联断言 lesson 消失；rollback 对 auto 写入的 CR 生效。
**验收**：上述测试 + `cargo test evolution::` 全绿；手工冒烟：改一次设置页配置 → bot-config.json 仍含 evolution 块；`ocr review` 无 HIGH。

## 4. B2 — 自进化闭环语义（M，依赖 B1）

| # | 改动 | 位置 | 要点 |
|---|---|---|---|
| B2-1 | lesson 幂等修复（P1-EV3） | `evolution/apply.rs:62` 附近 + memory 合并链路 | 双保险：① consolidation 的 apply_ops merge **跳过带 `evo:` tag 的行**（lesson 不被吸收）；② apply 查重除 `find_by_key_tag` 外，增加「同 id lesson 内容仍存在」的相似度复查；③ merge-on-write 目标行已存在且非本链路写入时**拒写** + `evolution.apply_conflict` audit（防记忆劫持） |
| B2-2 | changes.jsonl 单写者锁（P1-EV4） | `evolution/observe/shadow.rs:356-366`、`evolution/mod.rs:77` | shadow 的 append_change 纳入 `EVOLUTION_STORE_LOCK`（spawn_blocking 内持锁或改走统一写入口） |
| B2-3 | shadow 去重（P1-EV4） | 同上 | 同 proposal_id 已存在未终态（Pending/Shadowing）CR 时跳过 append |
| B2-4 | change_id 唯一性与二次回滚（P1-EV5） | `panel/commands.rs:101-107,485-490` | **按拍板项 ①** 执行：禁止已回滚提案再次 toggle ON，或改为按「最后一条匹配行」回滚 + 行级唯一 id |
| B2-5 | 生产 shadow 失败率告警（P1-EV6） | `observe/shadow.rs:371-373` 收尾 | 生产入口循环结束后补 `audit_warning`（失败率>5%），对齐 trait 版 :222-226 |

**新增回归测试**（审阅建议 #2/#3/#4/#5/#7）：delete/reject 重生语义锁定；二次回滚场景；shadow×panel 并发交错无丢行无重复；lesson 吸收/劫持场景；告警触发。
**验收**：`cargo test evolution:: shadow::` 全绿 + 连跑 5 轮稳定（防并行 flaky）；`ocr scan --path src-tauri/src/evolution/apply.rs,src-tauri/src/evolution/observe/shadow.rs` 无 HIGH。

## 5. B3 — 后端并发与流式稳健（M-L，可与 B1/B2 并行）

| # | 改动 | 位置 | 要点 |
|---|---|---|---|
| B3-1 | 排队可取消 + 状态口径 | `bot_orchestrator.rs:820-852,926-938` | `wait_slot` 轮询同时查停止令牌/DB 终态，取消即退队并释放 ExecGuard；Running 写库挪到拿到 slot 之后 |
| B3-2 | 墙钟超时先 stop 后收尾 | `bot_orchestrator.rs:1193-1202` | timeout 分支先 `stop.stop()` + 短 grace 再 drop，让在飞 Python/MCP 自行收尾 |
| B3-3 | 流式超时改型 + Client 复用 | `bot_model_loop.rs:412-415,703-760`、`bot_chat.rs:1091`、`bot_plan.rs:121` | 去掉 300s 总超时，改逐 chunk idle 超时；全局 `OnceLock<reqwest::Client>` 复用连接池 |
| B3-4 | Python 闸有界并发 | `py/runtime.rs:300,322-341` | `PY_RUN_GATE` Mutex → `Semaphore(2~3)`（退出清理的 EXITING 复查机制平移）；考虑文档类/工具类分闸 |
| B3-5 | run_model_loop_core 拆分（大件，可单列后批） | `bot_model_loop.rs:513-1262` | 按「回合」抽流消费/工具执行/收尾三单元，各 ≤200 行；不违背「不为过 lint 拆函数」惯例（现状已伤可测性） |

**验收**：`cargo test` 全量绿 + `cargo test task_chat_exec llm_integration`（现有并发回归）；并发闸取消场景新增测试；`ocr review` 无 HIGH。

## 6. B4 — 自进化工具补全与死代码决策（M，依赖 B2）

| # | 改动 | 位置 | 要点 |
|---|---|---|---|
| B4-1 | eval_run 路径修正（P2-EV8） | `eval/runner.rs:174-182`、`bin/eval_run.rs` | applied 路径默认指向 data_dir/evolution-applied.jsonl + 新增 `--applied` 覆盖；`case_passed` 占位在输出中标注 |
| B4-2 | trace 接真实数据（P2-EV10） | `bot_chat.rs:947`、`trace.rs:108` | 扩展 model_loop 返回：tool_calls 明细、Failure 分支可达、turn/memory 计数（trace.rs:108 注释的既定计划） |
| B4-3 | jsonl 损坏自愈（P2-EV11） | `evolution/mod.rs:49-57` | 读前自动备份损坏文件（.corrupt-<ts>），跳过坏行继续 + WARN audit；或至少「首行损坏」不再永久 fail-closed |
| B4-4 | panel 纪律（P3 组） | `panel/commands.rs:190,512` + toggle/promote | 裸连接写改持 `DB_WRITE_LOCK`；toggle 增加终态校验（不可复活 Rejected/Expired）；promote 段间复检 |
| B4-5 | **死代码去留（拍板项 ②）** | change::status、candidate::ttl、candidate::conflict、sandbox/（除 fnv1a）、activation 零调用函数、kill_switch | 三选一：接线激活（实现 Shadowing→Approved→Active 推进引擎 + apply 入口接 KillSwitch）/ 归档删除 / 维持现状但在模块头标注「实验态，无生产调用」。拍板前**先修 kill_switch.rs:23-24 的不实注释** |
| B4-6 | MCP env/headers 迁 KeySlot（拍板 ③=迁） | `bot/mcp/config.rs` + 新 `bot/mcp/secrets.rs` + `bot/config/io.rs` + `commands.rs` | **方案已出待拍板**：`MCP-KEYSLOT-MIGRATION-DESIGN-2026-09-29.md`（迁移路径 / 失败回滚 / 涉及文件 / 日志脱敏 / 测试计划）。拍板后实施，建议排在 B1 之后单开一批 |

**验收**：`cargo test eval:: trace::` + observe_run 对 /tmp 数据全流程重跑（指标不再恒 0 的项记录新基线）；`ocr review` 无 HIGH。

## 7. B5 — 前端与工程健康（M，随时可做）

| # | 改动 | 要点 |
|---|---|---|
| B5-1 | 接入 linter | oxlint（或 eslint+typescript-eslint+react-hooks）；现有 6 处 eslint-disable 重新生效；挂 `.githooks/pre-commit` |
| B5-2 | ChatPanel 流式性能 | 抽 `MsgBubble` + `React.memo`（比较 content/streaming/tools 引用），流式帧只渲染最后一条；`key={i}` 改稳定 id |
| B5-3 | ChatPanel 拆分（可与 B5-2 分批） | 1851 行 → 会话列表/消息列表/输入区三文件，目标 ≤600 行/文件 |
| B5-4 | McpPanel 补齐 | 拖放监听改 cancelled-flag 模式（复用 useTauriListen）；两个 800ms setTimeout 挂卸载清理；`transport` 收 union；补编辑回填/http 分支/非法 timeout 测试；修 ：505 占位符笔误 |
| B5-5 | tsconfig 收紧 | `noUncheckedIndexedAccess`（预计十余处索引访问点逐一处理） |
| B5-6 | clippy 262 清零 | 先修 tests/task_chat_exec.rs 3 处 `await_holding_lock`（tokio Mutex 或缩小持锁范围），其余批量 `cargo clippy --fix` + 人工复核；目标 `--all-targets` 0 warning 后考虑 CI 卡 -D warnings |
| B5-7 | 供应链自动化 | 引入 cargo-deny（或装 cargo-audit 进 CI）；`npm audit fix`（vitest → 4.1.11）；评估 onnxruntime 二进制随包分发替代 download-binaries |

**验收**：`npx tsc --noEmit` + `npx vitest run` + `npx knip` + `cargo clippy --all-targets`（0 warning）全绿；`ocr review` 无 HIGH。

## 8. B6 — 仓库卫生与文档（S，随时可做）

1. 删本地两代 portable（`wmessage-portable-2026-09-18`、`-09-25` 及旧 zip，释放 ~305MB，均未入库）。
2. `git mv` 根目录五稿（R2_DESIGN/R6_A_DESIGN/DERIVABILITY/OBSERVATION_STATUS/VERIFICATION）→ `docs/evolution/`；`evolution/observe-report-r6b-smoke.json` 同迁。
3. 建 `docs/archive/`：AUDIT-*（8 月批 20 份）、HANDOFF-2026-09-23(-v2)/09-25、OCR-CODE-REVIEW 三格式留一、kimi-audit/、bug-hunt-/、comment-hygiene-/。
4. `.gitignore` 移除 `health-check.sh` 并 `git mv` 进 `scripts/`；`README.txt` → `packaging/`（同步改 PACKAGING 文档引用）。
5. README.md 目录树 4 处更正 + 文档清单补 `docs/rust-bot-architecture.md`；SPEC.md 顶部加「历史存档」声明；`SKILL-DSL.md` → `SKILL-DSL.md`。
6. DEVLOG 补 10 天未提交增量 + 本轮审计条目；OBSERVATION_STATUS/phase-2 的历史测试数字加「截至日期」标注（B1/B2 完成后更新恢复条件状态）。

## 9. 语义决策（2026-09-29 已全部拍板）

| # | 问题 | 拍板结果 | 落点 |
|---|---|---|---|
| ① | 已回滚提案能否再次 toggle ON | **允许，但需二次确认**：有回滚历史的提案再点 ON 时弹「上次已回滚，确认再次启用？」——禁止会锁死重试路径，简单放行易误点循环，二次确认兼顾 | B2-4 按此实现 |
| ② | evolution 死代码（9 态机/ttl/conflict/sandbox/kill_switch） | **按自进化专项审计口径修**：kill_switch 真接线（其文档声称 apply 入口检查，属「假接线」缺陷，接线成本小且与既有设计一致）；其余（9 态机/ttl/conflict/sandbox 观察件）维持有意停摆，模块头标注「实验态：S0 观察态设计，等真数据（OBSERVATION_STATUS §2/§3），无生产调用」 | B4-5 |
| ③ | MCP env/headers 明文存 bot-config.json | **迁 KeySlot**。安全决策，先出迁移方案（现有明文 key 迁移路径 / 失败回滚 / 涉及文件：配置加载、UI 保存、日志脱敏），老板拍板后再动代码 → `docs/MCP-KEYSLOT-MIGRATION-DESIGN-2026-09-29.md` | B4-6，方案先行 |
| ④ | delete 的语义 | **维持现行为 + 改注释**：无「同 id 重生」实际痛点证据前不改产品行为；commands.rs:175 注释与行为对齐 | B2 回归测试锁定现行为 |
| ⑤ | run_python schema 文案 | **改**：「本机沙箱」→「资源受限（CPU/内存/时长），无文件系统隔离」 | B3 或随 B4 文案批 |

①②④⑤ 边跑边落；③ 方案先行。

## 10. 全量验收闸（每批合并前 + 最终）

```
bash scripts/test-fast.sh          # pre-commit 同款
bash scripts/test-all.sh           # pre-push strict（含 tests-audit 四件套）
cargo test && cargo clippy --all-targets
npx tsc --noEmit && npx vitest run && npx knip
```

最终完成后：Mimosa 复扫一次（排除 `target/`），对照本文 §1.2 复核攻击面无回归；`ocr scan` 对 evolution/ 全目录出一份终审报告留档。
