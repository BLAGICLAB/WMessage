# R2/R4 字段可派生性分析（DERIVABILITY.md）

> 日期：2026-09-18
> 承接：R0 VERIFICATION.md 冲突 #2（EvolutionProposal 加字段破坏约束 3）
> 任务：逐字段判断「能否从现有数据派生」，输出「可派生 / 不可派生」清单
> 决策权：可派生 → 复用现有结构（不动 schema）；不可派生 → 走独立 jsonl（`evolution-proposals.jsonl`）

---

## 现有 EvolutionProposal 字段（`evolution/proposal.rs:30-41`）

```rust
pub struct EvolutionProposal {
    pub proposal_id: String,           // 16 hex chars
    pub created_at_ms: i64,
    pub origin: ProposalOrigin,
    pub category: ProposalCategory,    // enum: MemoryHint/PromptHint/ToolSchemaHint/SkillHint
    pub target: ProposalTarget,        // enum: PromptSection/ToolSchema/SkillDsl/MemoryPolicy
    pub impact: ImpactLevel,           // enum: Low/Medium/High
    pub evidence: Evidence,            // struct: summary/occurrence_count/window_hours/related_refs
    pub suggestion: Suggestion,        // struct: text/structured_patch
}
```

## 现有 audit log 字段（`evolution/emit.rs:107-117`，锁死）

```rust
"evolution.proposal" event 写入：
proposal_id / category / impact / origin / target / occurrence_count / window_hours / summary / suggestion
```

## 现有 applied.jsonl 字段（`evolution/apply.rs:84-94`，5 字段）

```rust
proposal_id / mem_key / applied_at_ms / impact / summary
```

---

## R2/R4 计划新增字段逐一分析

### 字段 1：`layer`（R4 候选层扩展）

**spec 目标**：六层（参数 / 策略 / PromptHint / ToolSchema / Skill / Code）

**派生候选**：从 `category` 1:1 映射？

| category | 派生 layer |
|----------|-----------|
| MemoryHint | `Memory`（即"策略"层的一种） |
| PromptHint | `PromptHint` ✅ |
| ToolSchemaHint | `ToolSchema` ✅ |
| SkillHint | `Skill` ✅ |

**问题**：六层里 `参数`（model temperature 等）和 `Code`（R8）**不在 category 枚举里**。

**伪代码（仅可覆盖 4/6 层）**：

```rust
fn derive_layer(category: ProposalCategory) -> Option<EvolutionLayer> {
    match category {
        ProposalCategory::PromptHint => Some(EvolutionLayer::PromptHint),
        ProposalCategory::ToolSchemaHint => Some(EvolutionLayer::ToolSchema),
        ProposalCategory::SkillHint => Some(EvolutionLayer::Skill),
        ProposalCategory::MemoryHint => Some(EvolutionLayer::Memory), // 即"策略"
        // 参数 / Code → 当前 category 没有对应枚举，必须新增
    }
}
```

**结论**：⚠️ **部分可派生（4/6 层）。** 参数 / Code 两层不可派生。

**处置方案**：
- A. 扩 category 枚举加 `Code` / `Parameter` 变体 → 破坏 emit.rs:107 字段锁死
- B. layer 作为独立字段落 `evolution-proposals.jsonl`，不在 EvolutionProposal 加
- C. 暂时只覆盖 4 层，R8 再补 Code 层字段

**默认建议**：B（独立 jsonl，6 层枚举集中定义）。

---

### 字段 2：`change_id`（R2 ChangeRecord）

**spec 目标**：ChangeRecord.change_id: String (UUID)

**派生候选**：`change_id = proposal_id`？或 `change_id = format!("chg-{}", proposal_id)`？

**伪代码**：

```rust
fn derive_change_id(proposal_id: &str) -> String {
    // proposal_id 已是 16 hex chars（proposal.rs:188 short_hash）
    // change_id 加 "chg-" 前缀避免和 proposal_id 命名空间混淆
    format!("chg-{}", proposal_id)
}
```

**结论**：✅ **完全可派生。** 1:1 映射，幂等。

**处置方案**：直接在 ChangeRecord 构造时计算，不入 EvolutionProposal，不入 jsonl。

---

### 字段 3：`evidence`（R4 候选层扩展）

**事实核查**：`evidence` 字段**已经存在**于 EvolutionProposal（`proposal.rs:60-71` Evidence struct）：

```rust
pub struct Evidence {
    pub summary: String,            // ✅ emit.rs:115 已发
    pub occurrence_count: u32,      // ✅ emit.rs:113 已发
    pub window_hours: u32,          // ✅ emit.rs:114 已发
    pub related_refs: Vec<String>,  // ❌ emit.rs 未发（字段锁死范围外）
}
```

**派生结论**：
- summary / occurrence_count / window_hours：✅ 已在 audit log
- related_refs：❌ 未在 audit log（已写但锁死，不能加）

**伪代码**：

```rust
fn derive_evidence(p: &EvolutionProposal) -> &Evidence {
    &p.evidence  // 直接返回，零成本
}

fn derive_evidence_for_audit(p: &EvolutionProposal) -> AuditEvidence {
    // 仅 audit log 已发的子集；related_refs 不进 audit
    AuditEvidence {
        summary: p.evidence.summary.clone(),
        occurrence_count: p.evidence.occurrence_count,
        window_hours: p.evidence.window_hours,
    }
}
```

**结论**：✅ **完全可派生。** evidence 字段已经在 EvolutionProposal struct 里，无需新增。

**处置方案**：零动作（R4 spec 误以为 evidence 是新字段——其实已在）。

---

### 字段 4：`status`（R2 ChangeStatus 枚举）

**spec 目标**：ChangeStatus { Proposed, Shadowing, ShadowPassed, Approved, Canary, Active, Rejected, RolledBack, Expired }

**派生候选**：能从 EvolutionProposal 现有字段派生吗？

| EvolutionProposal 字段 | 派生 status 候选 |
|----------------------|-----------------|
| 都不足以表达生命周期 | ❌ |

**结论**：❌ **不可派生。** status 是 lifecycle 字段，跟随时间变化。

**处置方案**：ChangeRecord.status 单独落 `evolution-changes.jsonl`（R2 主线）。

---

### 字段 5：`parent_id`（R2 ChangeRecord）

**spec 目标**：Option<String>，版本链回溯

**派生候选**：EvolutionProposal 没有 parent 概念。

**结论**：❌ **不可派生。**

**处置方案**：ChangeRecord.parent_id 单独落 `evolution-changes.jsonl`。

---

### 字段 6：`eval_before` / `eval_after`（R2 ChangeRecord）

**spec 目标**：EvalResult 快照

**派生候选**：可从 `evolution-eval-results.jsonl`（R1 产出）按时间窗口过滤得出，但**需要时间戳对齐**。

**伪代码（弱化派生）**：

```rust
fn derive_eval_before(applied_at_ms: i64, eval_results: &[MetricsReport]) -> Option<MetricsReport> {
    eval_results.iter()
        .filter(|r| r.evaluated_at_ms <= applied_at_ms)
        .max_by_key(|r| r.evaluated_at_ms)
        .cloned()
}
```

**结论**：⚠️ **弱可派生**（依赖外部评估时序）。建议仍存 ChangeRecord 快照避免查表代价。

**处置方案**：ChangeRecord.eval_before/_after 落 `evolution-changes.jsonl`（独立快照，避免回溯计算）。

---

### 字段 7：`applied_at` / `rolled_back_at`（R2 ChangeRecord）

**派生候选**：
- applied_at ✅ 可从 `evolution-applied.jsonl` 派生（已有 applied_at_ms 字段）
- rolled_back_at ❌ 无 jsonl 记录回滚事件（除非走 audit log 的 grep）

**结论**：applied_at ✅ 可派生；rolled_back_at ❌ 不可派生。

**处置方案**：applied_at 派生；rolled_back_at 落 `evolution-changes.jsonl`。

---

### 字段 8：`hard_constraint_compliance`（bool）

**派生候选**：检查 EvolutionProposal 是否符合 9 条硬约束？

**伪代码**：

```rust
fn derive_compliance(p: &EvolutionProposal) -> bool {
    // 硬约束 5/6：category=MemoryHint && impact ∈ {High, Medium} 才能自动应用
    // 其他约束由 apply.rs 入口校验
    matches!(p.category, ProposalCategory::MemoryHint) 
        && !matches!(p.impact, ImpactLevel::Low)
}
```

**结论**：✅ **可派生**（pure function）。

**处置方案**：ChangeRecord 构造时计算，不入 jsonl。

---

### 字段 9：`schema_version`（u32）

**派生候选**：当前所有提案都是 v1（默认）。

**结论**：✅ **可派生**（默认 1）。

**处置方案**：ChangeRecord 构造时 default=1，不入 jsonl；R2+ 改 schema 时显式 bump。

---

### 字段 10：`eval_set_version`（R1 → R2 衔接）

**事实**：R1 已经把 eval_set 写到 `evolution/eval_set.jsonl`（855 行）。

**派生结论**：不需要新字段。R2 通过文件修改时间或 R1 jsonl 里的 `case_id` 范围感知版本。

---

## 总览清单

| 字段 | spec 阶段 | 可派生？ | 派生方式 / 落点 |
|------|----------|---------|----------------|
| `layer` | R4 | ⚠️ 部分（4/6 层） | 不可派生部分走 `evolution-proposals.jsonl` |
| `change_id` | R2 | ✅ 完全 | `format!("chg-{}", proposal_id)`，不入 jsonl |
| `evidence` | R4 | ✅ 完全（已在 struct） | 零动作 |
| `status` | R2 | ❌ 不可 | `evolution-changes.jsonl` |
| `parent_id` | R2 | ❌ 不可 | `evolution-changes.jsonl` |
| `eval_before/_after` | R2 | ⚠️ 弱可 | 落 `evolution-changes.jsonl`（快照优先） |
| `applied_at` | R2 | ✅ 可 | 从 applied.jsonl 派生 |
| `rolled_back_at` | R2 | ❌ 不可 | `evolution-changes.jsonl` |
| `hard_constraint_compliance` | R2 | ✅ 可 | 构造时算，不入 jsonl |
| `schema_version` | R2 | ✅ 可 | 默认 1，构造时定 |

---

## 派生方案建议（R2 实施参考）

**ChangeRecord 字段来源（按派生性分组）**：

```
来源 1：从 EvolutionProposal 派生（构造时算，不持久化）
  - change_id        = "chg-" + proposal_id
  - hard_constraint_compliance = category+impact 判定
  - schema_version   = 1

来源 2：从 evolution-applied.jsonl 派生（查表，不复制）
  - applied_at       = applied.jsonl[mem_key].applied_at_ms

来源 3：必须持久化（落 evolution-changes.jsonl）
  - status
  - parent_id
  - eval_before / eval_after（弱派生但存快照）
  - rolled_back_at
```

**不可派生但需要持久化的总字段数**：4（status / parent_id / eval_before / eval_after / rolled_back_at —— 实际 5）

**jsonl 文件拓扑**（R2 实施后）：
- `evolution-applied.jsonl`：保留原 5 字段不变（仅 R1 + apply.rs 写）
- `evolution-changes.jsonl`：**新增**，5 字段 × N 条 ChangeRecord
- `evolution-proposals.jsonl`（R4 可能引入）：**仅当 layer 字段确实需要独立时**

---

## 风险 / 边界

1. **layer 字段**：当前可派生 4 层（MemoryHint/PromptHint/ToolSchemaHint/SkillHint → Memory/PromptHint/ToolSchema/Skill）。R8 引入 Code 层时需要扩 category 枚举（破 emit.rs 锁死）或新增独立 layer 字段。**R4 实施前需明确 R8 路径**。

2. **applied_at 派生**：跨 jsonl 查表需要把 applied.jsonl 全量加载到内存，O(N) 内存；500 行规模 OK。

3. **eval_before/_after 弱派生**：依赖 eval_runner 跑的时刻和 ChangeRecord applied_at 的时序对齐；如果用户从未跑过 eval，eval_before 为 None。

---

## 待老板拍板

无（本冲突是纯技术派生性分析，结论自洽）。

R2 实施前需要老板拍的是 **冲突 #3**（applied.jsonl 是否允许扩字段），见下条消息。