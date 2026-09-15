# 自进化系统 Phase 1 — 永久存档

> Commit: `41a68e6 feat(evolution): 自进化系统 Phase 1 — 经验捕获 + 提案产出（仅写 audit）`
> 完成日期: 2026-09-15
> 状态: ✅ 完成（workdir 已清理，永久存档落地）

---

## 1. HANDOFF（30 行执行摘要）

**Patch**: `41a68e6`（8 files / 1646 insertions）

**本次交付**

1. `src-tauri/src/evolution/` 新模块（5 文件 / ~1600 行 / 50 个新单测）
2. `src-tauri/src/memory/consolidate.rs::run_consolidation` 末尾插入 1 行 hook（+1 行 ops.clone 准备）
3. `src-tauri/src/bot_chat.rs::bot_chat` 最终 return 处 hook trace 采集
4. `src-tauri/src/lib.rs` 加 `pub mod evolution;` + `.setup` 调 `register_app_handle`

**测试**: 713 passed / 0 failed / 2 ignored（baseline 663 → +50）

**用户下一步**

```bash
# 跑一周生产数据后：
grep "evolution.proposal" bot.log    # 看提案频率 / 质量
grep "trace.completed" bot.log       # 看失败/超时/中止触发

# 不想要了就回滚：
git apply -R <41a68e6 反向 patch>    # 或 rm -rf src-tauri/src/evolution/
```

**未做项清单**

- 不落表 / 不建 schema / 不改 AppState
- 不调第二次 LLM
- 不改 prompt / TOOLS schema / 命令名 / 事件名 / JSON 字段 / 错误码
- 不改 memory 对外接口
- 不触发任何应用路径（提案一律仅写 audit）
- 不修 baseline 144 clippy errors（预存在，越界）

**关键标注（必读）**

1. **执行时间字段用 `i64` ms 而非 `DateTime<Utc>`**——因 chrono `serde` feature 未启用（spec #6 禁改 Cargo.toml）。
2. **`ids.len` 是单次反思占位语义，不是跨次累计**——`Merge{ids:[a,b,c]}` 表示"LLM 这次觉得该合并"，**不等于**"同类问题发生 3 次"。Phase 1 dedup 工作正常（同 consolidate 重跑同 id）；Phase 2 真要累计需在 evolution 侧加进程内短窗口计数器。
3. **三类 ops 全映射到 MemoryHint 是有意收紧**——Q2=1 决策，好处是 MemoryHint 后续不可能触发 prompt/tool 应用，风险最低。Phase 2 一周后看数据决定是否拆 ToolSchemaHint。
4. **`tool_calls_count` 未追踪**——spec 1.5 采样规则 4 之一，函数 + 测试齐全但实际数据为 0，规则不触发。Phase 2 接入 bot_model_loop middleware 拦截即可。
5. **唯一 hook 点在 `bot_chat` 最终 return**——早期 return（ChatGuard 拦截 / chat_execute_tasks / skill auto-mode 终态）不走 run_model_loop，不构成完整执行轨迹。spec 主流程指此点。
6. **clippy 144 errors 全预存在**——`cargo clippy --all-targets -- -D warnings` 因 baseline 而非 Phase 1 失败。evolution/ 自身 0 errors。

---

## 2. Implementation Report（详细设计文档）

### 2.1 改动文件清单

**新建（src-tauri/src/evolution/）**

| 文件 | 行数 | 用途 |
|---|---|---|
| `mod.rs` | 35 | 模块门面 + `post_consolidation` 桥接入口 |
| `trace.rs` | 425 | ExecutionTrace / ToolCallSummary / TraceOutcome + sampling + maybe_record_trace |
| `proposal.rs` | 394 | EvolutionProposal + 枚举 + proposal_id 归一化 + 短 hash |
| `derive.rs` | 467 | 启发式 derive_proposals（Q2=1 保守阈值） |
| `emit.rs` | 291 | audit 写入 + 24h 进程内 dedup |

**修改**

| 文件 | 改动 |
|---|---|
| `src-tauri/src/lib.rs` | + `pub mod evolution;`（line 28 附近）+ `.setup` 调 `register_app_handle` |
| `src-tauri/src/memory/consolidate.rs` | `run_consolidation` 末尾插入 `evolution::post_consolidation(&ops_for_audit, &report)`（一行 + 一行 clone 准备） |
| `src-tauri/src/bot_chat.rs` | `bot_chat` 函数顶部加 `started_at_ms`；最终 return 前 hook `evolution::trace::maybe_record_trace`（10 行内） |

**未触碰**: Cargo.toml / memory/mod.rs / bot_model_loop.rs / app_state.rs / audit.rs / 任何 tauri command 签名 / 前端协议

### 2.2 数据类型设计理由

#### ExecutionTrace

```rust
pub struct ExecutionTrace {
    pub trace_id: String,
    pub session_id: String,
    pub origin: MutationOrigin,
    pub started_at_ms: i64,        // 不是 DateTime<Utc>
    pub ended_at_ms: i64,
    pub turn_count: u32,
    pub tool_calls: Vec<ToolCallSummary>,  // Phase 1 留空 Vec
    pub outcome: TraceOutcome,
    pub skill_used: Option<String>,
    pub memory_injected_count: u32,
    pub task_refs: Vec<String>,
}
```

**关键决策**：

- **时间字段用 `i64` epoch ms 而非 `DateTime<Utc>`**：spec 硬约束 #6 禁止改 Cargo.toml；chrono 的 `serde` feature 未启用。改用 `i64` 与代码库其它 `_ms` 字段一致。
- **`origin` 复用现有 `MutationOrigin` 枚举**（mutation.rs 5 个值）。
- **`tool_calls: Vec<ToolCallSummary>` 留空 Vec**：Phase 1 不追踪逐次工具调用（避免侵入 bot_model_loop 主循环），只采样整体信息。这是**对 spec 1.5「tool_calls > 10」采样规则的占位**——规则函数 + 测试覆盖齐全，但实际数据为 0，规则不触发。HANDOFF 标注此点。
- **`outcome` 是结构化枚举**（Success / Failure { reason } / Aborted），不存原始堆栈或异常消息。reason 只存分类标签，不存敏感信息。

#### EvolutionProposal

```rust
pub struct EvolutionProposal {
    pub proposal_id: String,
    pub created_at_ms: i64,
    pub origin: ProposalOrigin,
    pub category: ProposalCategory,
    pub target: ProposalTarget,
    pub impact: ImpactLevel,
    pub evidence: Evidence,
    pub suggestion: Suggestion,
}
```

- **`ProposalCategory` 四种 + `ProposalTarget` 四种子类型**——Phase 1 实际只用到 `MemoryHint` + `MemoryPolicy`，其它字段为 Phase 2 拆分保留入口。
- **`Evidence` 不存原始堆栈 / 文件路径 / 对话内容**。
- **`Suggestion.structured_patch` 永远 `None`**（Phase 1）。

#### proposal_id 归一化（核心去重逻辑）

```rust
pub fn normalize_for_hash(s: &str) -> String {
    // 大写→小写；数字→'N'；非字母数字→' '；合并连续空白；trim
}
```

**设计动机**：LLM 措辞差异（"工具 A 失败" vs "tool A failed" vs "工具-A失败。"）不应导致不同 id。归一化后：

- `"Tool A failed 3 times"` → `"tool a failed N times"`
- `"tool A failed 5 times"` → `"tool a failed N times"`（数字归一化）
- `"tool-A failed 7 times."` → `"tool a failed N times"`（标点归一化）

三者得到同一 proposal_id。

**关键决策**：
- 数字占位符用 **大写 'N'**（与字母视觉区分更明显）
- CJK 字符是 `is_alphanumeric()`，**保留**——中文摘要天然稳定
- 最终 8 字节 hash（16 hex 字符）—— 短到 grep 友好、足够 24h 内去重

### 2.3 derive_proposals 检测规则 + 反例

**规则（Q2=1 锁定，保守阈值）**

| 输入 | 阈值 | 产出 | Impact |
|---|---|---|---|
| `Merge { ids }` | `ids.len() >= 3` | 1 条 MemoryHint | Medium |
| `Contradiction` | 任意（≥1） | 1 条 MemoryHint | High |
| `Distill { ids }` | `ids.len() >= 5` | 1 条 MemoryHint | Low |

**单次反思上限 5 条**（`MAX_PROPOSALS_PER_ROUND`）。

**关键反例（测试覆盖）**

| 输入 | 期望 |
|---|---|
| 空 ops | 空 Vec |
| `Merge{ids.len=2}` + `Distill{ids.len=4}` | 空 Vec（都不到阈值） |
| `Merge{ids.len=2}` | 空 Vec |
| `Distill{ids.len=4}` | 空 Vec |
| 10 个 Contradiction | 5 条（上限截断） |
| 9 个混合 ops | 5 条 |
| 空 content | 不 panic |
| 10000 字符 content | 不 panic |
| Unicode content | 不 panic |
| 100 ids 的 merge | 不 panic；related_refs 截到 3 |

**占位语义（必读）**：`evidence.occurrence_count = ids.len()` 是**单次反思内**的语义，**不是跨次累计**。Phase 1 用它作占位即可——同一 consolidate 重跑产出同一 id，dedup 工作正常。Phase 2 若要真正的跨次累计，需要在 evolution 侧加一个短窗口计数器。

### 2.4 trace 采样规则（spec 1.5）

| 规则 | 触发 | 边界 |
|---|---|---|
| 1. outcome 是 Failure | Failure enum | - |
| 2. tool_calls_count > 10 | > 10 | 边界 10 不触发 |
| 3. duration_ms > 60_000 | > 60s | 边界 60_000 不触发 |
| 4. was_aborted | bool | - |

**Phase 1 实际激活**：规则 1 + 规则 3 + 规则 4
**Phase 1 未激活**：规则 2（tool_calls_count 未追踪）

**集成点**：`bot_chat.rs` 唯一 hook 点 = `bot_chat` 函数最终 return 处（line ~825）。早期 return 不构成完整执行轨迹。

### 2.5 单元测试（50 个新增）

| 模块 | 测试数 |
|---|---|
| `evolution::trace` | 9 |
| `evolution::proposal` | 13 |
| `evolution::derive` | 15 |
| `evolution::emit` | 6 |

实际 `cargo test --lib` 显示 **713 passed / 0 failed / 2 ignored**（baseline 663 → +50）。

### 2.6 边界检查输出

| 检查 | 结果 |
|---|---|
| `cargo fmt --check` | ✓ 干净（rustfmt 已应用于 evolution/*） |
| `cargo check --lib` | ✓ 0 errors, 16 warnings（全部预存在） |
| `cargo test --lib` | ✓ 713 passed |
| `cargo clippy evolution/` | ✓ 0 errors |
| `git diff | grep '\.execute\(\|CREATE TABLE\|ALTER TABLE'` | ✓ 空 |
| `git diff | grep 'run_model_loop\|chat_completions'` | 1 行（注释字符串，非实际调用） |
| `git diff | grep 'SYSTEM_PROMPT'` | ✓ 空 |
| `git diff src-tauri/src/bot_model_loop.rs` | ✓ 空 |
| `git diff src-tauri/src/app_state.rs` | ✓ 空 |

`cargo clippy --all-targets -- -D warnings` 失败（144 pre-existing errors），不在 Phase 1 范围。

### 2.7 接线策略（Q1=B 路径 B）

**依赖方向**：
```
memory::consolidate::run_consolidation
    ↓ (一行调用)
evolution::post_consolidation(&[ConsolidateOp], &ConsolidateReport)
    ↓
evolution::derive::derive_proposals → Vec<EvolutionProposal>
    ↓
evolution::emit::emit_proposals(Vec<EvolutionProposal>) → EmitReport
    ↓
audit_event!("evolution.proposal", ...)  (通过全局 OnceLock<AppHandle<Wry>>)
```

memory 模块完全不感知 evolution 存在——由 `evolution::post_consolidation` 的签名（只接 `&[ConsolidateOp]` + `&ConsolidateReport`）保证。

**AppHandle 传递**：lib.rs::run() 的 .setup 调一次 `register_app_handle`；emit 内部维护 `OnceLock<AppHandle<tauri::Wry>>`；未注册时 eprintln 跳过 audit 写入（不 panic）。

**ops.clone() 的必要性**：`run_consolidation` 的现有结构里闭包吃掉了 `ops`，外层拿不到。在 spawn_blocking 之前 clone 一份留作审计信号源（`ConsolidateOp` 已 derive `Clone`）。

### 2.8 本次未做的事

| 未做 | 原因 |
|---|---|
| 数据库表 / schema 迁移 | spec #1 |
| AppState 字段改动 | spec #1 |
| 调第二次 LLM | spec #2 |
| 改 prompt / TOOLS schema / 命令名 / 事件名 / JSON 字段 / 错误码 | spec #3 |
| 改 memory 现有对外接口 | spec #4 |
| 改 consolidate 反思逻辑 | spec #5 |
| 新增依赖 / 升级 / cargo update | spec #6 |
| 改安全校验 | spec #7 |
| 删除 / 注释测试 / 放宽断言 | spec #8 |
| 触发任何应用路径 | spec #10 |
| 验证管道 / apply 机制 / 前端 UI | Phase 1 不做 |
| 跨次累计 | spec 占位语义 |
| 早期 return 处 hook | spec 不改主流程 |
| `tool_calls` 实际追踪 | 侵入 bot_model_loop |
| `MemoryHint` 拆分 | spec 主动放弃分类粒度 |
| 修复 baseline 144 clippy errors | 预存在，越界 |

### 2.9 回滚方式

```bash
cd /Users/renshi/Projects/wmessage
rm -rf src-tauri/src/evolution/ docs/evolution/phase-1.md
git revert 41a68e6            # 或 git reset --hard HEAD~1（如果未 push）
# 然后从 .gitignore 删 .evolution-*/
```