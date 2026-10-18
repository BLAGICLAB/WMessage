# check_subagent 等待策略优化（固定 5s → 事件驱动 + 递进兜底）— 2026-10-18

> 状态：**已实施**（2026-10-18，P1–P4 全部落地）。本文保留原方案推导，
> 实施中偏离之处就地标注（见 §4 P2 与 §8）。
> 关联：`SUBAGENT-ORCHESTRATION-DESIGN-2026-09-27.md` §5（工具接口「定死」一节
> 将被本文 §4 取代）、`SPEC.md`「编排与执行」段、`prompts/subagent.rs::MAIN_AGENT_ADDENDUM`。

---

## 0. 一页结论

现在主 agent 每 5s 醒一次问一次「好了没」，既醒得频繁（每次一整个 LLM 轮次），
又醒得不巧（子任务 800ms 就完事，也要空等满 5s）。

改成：**服务端事件驱动唤醒（终态即返回）+ 递进步长兜底轮询**，并把
`waitMs` 的可用预算从 5s 放宽到 30s。一次工具调用就能覆盖大部分子任务全程，
主 agent 的轮次从「每 5s 一轮」降到「子任务结束时一轮」。

---

## 1. 现状（行号为本仓 commit 时点）

| 事实 | 位置 |
|---|---|
| `wait_ms` 上限 5s | `src-tauri/src/bot_orchestrator.rs:27`（`CHECK_WAIT_MS_CAP = 5_000`） |
| 等待期固定 150ms 一步 | `src-tauri/src/bot_orchestrator.rs:29`（`CHECK_POLL_INTERVAL_MS = 150`） |
| 等待循环每轮重开一次库 | `src-tauri/src/bot_orchestrator.rs:463-480` |
| `open_db` = 开文件 + 10 次 `execute_batch` + 7 个 migrations 辅助调用（实测约 60 条语句） | `src-tauri/src/db/mod.rs:106-357` |
| 工具入口二次 clamp | `src-tauri/src/bot_orchestrator.rs:2381-2391` |
| `call_check_subagent` 只取 `ctx.app`，丢掉 `ctx.stop` / `ctx.session_id` | `src-tauri/src/bot/registry.rs:606-610` |
| schema 文本写死「上限 5000，不要高频空转」 | `src-tauri/src/bot/registry.rs:374-377` |
| 提示词写死「waitMs 0 或短等待」 | `src-tauri/src/prompts/subagent.rs:40` |
| `/stop` 只在轮次边界检查，等待期间不生效 | `src-tauri/src/bot_model_loop.rs:1535`（同型检查见 1727/1758） |
| 终态唯一出口（事件唤醒点现成） | `src-tauri/src/bot_orchestrator.rs:2231-2248`（`emit_subagent_finished`） |
| 进程内通知原语已有先例 | `src-tauri/src/bot_orchestrator.rs:1239`（`SubagentGate.notify`） |
| owned 可跨线程的停止令牌 | `src-tauri/src/bot_slash.rs:76-78, 88-97`（`StopGuard::token` / `StopToken`） |
| 测试夹具：内存库 + 生产同源 DDL | `src-tauri/src/bot_orchestrator.rs:2662-2669`（`test_conn()`） |

**一次 5s 等待的实际开销**：约 33 轮 × （开一个 SQLite 连接 + 跑约 60 条建表/迁移
语句）。也就是 5 秒里把全量 DDL 跑了 33 遍，只为了看一眼一行 `subagents` 的状态。

---

## 2. 问题拆解：贵的不是查库，是「醒一次」

查库开销是次要的。真正的成本在**每返回一次 check_subagent，就等于多走一个完整的
LLM 轮次**：重发请求、重新流式、重算越来越长的上下文、把几百字符的轮询结果再塞进
历史。于是：

- 子任务 800ms 完成 → 父 agent 仍空等满 5s，还可能再醒一轮才拿到结果。
- 子任务 3 分钟 → 约 36 轮纯轮询，每轮往上下文里灌一段「仍在进行中」。
- 每轮再叠加 33 次全量 DDL 开库，对同库其他写者也是持续干扰。

同时注意：主 agent **并不依赖轮询**。`subagent_digest_block`
（`bot_orchestrator.rs:976-995`）在父会话每次组装消息时就会注入终态回报。
轮询只是「早点知道」的快车道——那它就该按「早知道」的成本来设计，而不是按
「固定 5s 心跳」来设计。

---

## 3. 方案总览

四块，可独立提交，**顺序有硬依赖**：

| 编号 | 改动 | 收益 | 依赖 |
|---|---|---|---|
| P1 | 等待期步长递进（150→300→600→1200→2400→3000 封顶） | 5s 内开库 33 次 → ~5 次；30s 内 → ~11 次 | 无 |
| P2 | 终态事件唤醒（`Notify`，读状态前 `enable()` 预注册） | 终态返回延迟 ≈0；稳态读库 ≈0 次 | 无 |
| P3 | 等待期响应 `/stop`（接 `ToolCtx.stop`） | 长等待不锁死停止键 | 无 |
| P4 | `waitMs` 上限 5s → 30s + schema/提示词改口径 | 父 agent 轮次从「每 5s」降到「结束时」 | **P3 必须先落地** |

P1 单独可上（纯收益、零风险）；P1+P2+P3 合起来才让 P4 的 30s 上限安全。

---

## 4. 具体改法

### P1 步长递进

```rust
/// 等待期步长阶梯（毫秒）。首查走 150ms 保「秒退子任务立刻可见」，
/// 之后逐步拉长，避免长等待里反复开库跑 DDL。
fn next_poll_step(step: usize) -> u64 {
    const LADDER: [u64; 6] = [150, 300, 600, 1200, 2400, 3000];
    LADDER[step.min(LADDER.len() - 1)]
}
```

只改 `check_subagent` 循环里的 sleep 时长，其余不动。**不动连接复用**：
`rusqlite::Connection` 不跨 `await` 持有，要复用就得占住 blocking 线程或把连接提
到 app 级共享池——而 P2 上线后读库次数本就趋近 0，这笔投入当场蒸发（YAGNI）。

### P2 终态事件唤醒

新增进程内唤醒设施，与 `SubagentGate` 的 `Notify` 同风格但独立：

- `finish_notify()`：`OnceLock<tokio::sync::Notify>` 懒初始化
- `emit_subagent_finished`（终态**唯一**出口，已含排队期取消 / 启动中止 /
  ghost 路径）末尾补一行 `finish_notify().notify_waiters();`
- 等待循环改为「挂好唤醒监听 → 读状态 → 非终态则
  `timeout(next_poll_step, notified())` → 醒了就重读」。

必须处理的坑：**`notify_waiters()` 不留 permit**，通知早于 `notified()` 注册就会丢。
解法是用 `Notified::enable()` **在读状态之前**把等待者挂上，堵死那个窗口——
于是最初设想的 `AtomicU64` generation 计数器变得多余，按 YAGNI 删掉：
没有丢通知的窗口，generation 也就不再兜得住任何额外情形。漏通知只剩
「将来新增的收尾路径忘了 notify」一种，兜底阶梯覆盖它，代价是最多一个
封顶步长的延迟，不影响正确性。

复杂度上，`Notify` 是现有 tokio 依赖（features 已含 `sync`），不需要新 crate。

### P3 等待期响应 `/stop`

`call_check_subagent`（`registry.rs:606-610`）改为透传 `ctx.stop`：
`ctx.stop.map(|g| g.token())` 得到 owned `StopToken`（`bot_slash.rs:76`），
下传到 `tool_check_subagent` / `check_subagent`，等待循环每轮查一次
`token.stopped()`，命中即返回当前状态（附 `error` 注明「用户停止」）。

现状是 `/stop` 只在轮次边界检查（`bot_model_loop.rs:1535`），所以等待期间按不响——
`waitMs` 从 5s 抬到 30s 之前，这一条必须先补。

### P4 上限与口径

- `CHECK_WAIT_MS_CAP`：5_000 → 30_000。**保守取值**，不直接上 60s：再往上对交互
  体验的边际收益迅速变小，而 P3 未生效时的停止阻塞是线性增长的。落地后按
  `subagent_check_wait` 审计事件的真实分布再评估。
- `waitMs` **语义不变**（仍是总预算，`0` 仍是单次读，缺省仍是 0），只是上限放宽——
  老调用方与幂等可轮询的契约不受影响。
- schema 描述改成给模型明确指令，例如「`waitMs` 是可放心给大的等待预算；服务端
  内部递进等待，子任务一结束立刻返回，不必自己反复短轮询」。
- `MAIN_AGENT_ADDENDUM` 第 4 步（`prompts/subagent.rs:40`）把「waitMs 0 或短等待，
  不要高频空转」换成同口径指引。**两处必须一起改**：只改后端不改 schema，模型仍然
  按旧描述传 5000，优化等于没做。

### 审计

加一条 Info 级 `subagent_check_wait`：`wait_ms` / 实际等待 ms / 退出原因
（`terminal` | `timeout` | `stop`）。这是本次优化唯一的验证手段——否则「轮次少了」
只能靠感觉。成本是一次结构化事件。

---

## 5. 测试

全部落在 `bot_orchestrator.rs` 的 `orchestrator_tests`（复用 `test_conn()` 内存库 +
`SUBAGENTS_DDL`）。

先做一步可测性重构：把等待核心抽成对「读函数」泛型，
`wait_until_terminal(read: impl Fn() -> CheckState, deadline, notify, gen, stop)`，
这样无需 `AppHandle` 即可直接测。

| 用例 | 断言 |
|---|---|
| `next_poll_step` 阶梯 | 单调递增、封顶 3000 |
| 已在终态 | 立即返回，不睡（elapsed < 100ms） |
| 200ms 后置 succeeded | 返回 `succeeded`，elapsed < 1s（容差放宽防 CI 抖） |
| 无终态、wait=300ms | 返回 `running`，elapsed ≥ 300ms |
| 停止令牌预置位 | 立即返回，不等到 deadline |
| 别的子 agent 收尾引发伪唤醒 | 醒来重读、非终态就继续等；断言上界因此留 1 次余量 |

另外 `prompts/subagent.rs` 现有测试加新口径锚点断言（该文件已有 `contains` 式断言
的先例）。

门禁：`bash scripts/test-fast.sh`、`cargo clippy --manifest-path src-tauri/Cargo.toml`。

---

## 6. 风险与边界

| 风险 | 处置 |
|---|---|
| 长等待期间 UI 无反馈 | 工具调用行本身是 running 态，右侧栏 3s 轮询仍在推进轮数，不空窗 |
| 通知丢失导致收不到终态 | `enable()` 预注册消掉丢通知窗口；再兜一层递进步长，最多多等一个封顶步长，不影响正确性 |
| `/stop` 被长等待锁死 | P3 先于 P4 落地；等待循环每轮查令牌 |
| 模型仍按旧行为传 5000 | schema 与提示词同步改口径；审计事件可验证实际分布 |
| 放宽上限后行为不可预期 | `waitMs` 语义与幂等性不变，仅放宽；单测覆盖终态早退 |

## 7. 显式不做

- **不改前端 `SUBAGENT_POLL_MS`（3s）**：`src/components/ChatPanel/constants.ts:43`
  那是 UI 刷新轮数读数的节奏，与 agent 侧等待无关，两条链路不要混。
- **不加随机抖动**：各 `wait` 相互独立，无对齐问题，抖动只增复杂度。
- **不改 `waitMs` 缺省值**：仍为 0（单次读），不静默改语义。
- **不做 SQLite 连接池**：被 P2 的读次数下降消解。
- **不引入新的等待原语 crate**：tokio `Notify` 够用（`sync` feature 已在依赖里）。

---

## 8. 提交切分与文档同步

原计划按 P1 → P2 → P3 → P4 拆四个 commit，实施后合并为两个：四块改动都落在同一个
`wait_until_terminal` 循环里（步长、唤醒、停止、上限互相咬合），拆开会产生
「能编译但没意义」的中间态，反而更难 review。实际切分：

1. `perf(bot)`：等待策略 + `/stop` 接线 + schema/提示词口径 + 测试（行为变更与其测试同commit）
2. `docs`：本文 + `SPEC.md`「编排与执行」段 + 2026-09-27 设计文档 §5 标注取代关系
