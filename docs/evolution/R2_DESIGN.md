# R2 ChangeRecord 设计稿（按 B 方案：新增 evolution-changes.jsonl）

> 日期：2026-09-18
> 状态：**已拍板 B**（新增独立 evolution-changes.jsonl）
> 决策时间线：
> - 10:52 老板一页纸决策请求，给出 A/B/C/D 四选项，初选 D（纯内存）
> - 12:40 老板追问「D 不完美，有没有更完美的方案」→ 我提 G（落 mem_items）方案
> - 12:44 老板验 G 三个致命假设（#2 注入污染 / #3 淘汰盲区 / #1 命名空间依赖纪律），切 B
> 决策理由：
> - 老板约束 #4 重读：「所有持久化用 jsonl 文件」= jsonl 是 database table 的替代品，不破约束 #4
> - G 8 未验证假设里 3 个致命（#2 确认 / #3 部分确认 / #1 依赖前缀纪律），验证成本 1-2 天
> - R2 时间窗口 Day 4-7，验证来不及
> - B 设计稿已画好（jsonl IO 零假设）
> - 「零新文件」不是设计目标，「形式合规 ≠ 精神合规」
> 后续路径：未来要做 G 时，先做完 8 个验证，写 ADR 记录。

---

## 1. 设计目标

建立版本链 + 状态机，让每个 evolution 提案有完整生命周期记录：
- `Proposed → Shadowing → ShadowPassed → Approved → Canary → Active → Rejected / RolledBack → Expired`

---

## 2. 文件拓扑（B 方案：新增独立 evolution-changes.jsonl）

```
{project_root}/evolution/
  ├── eval_set.jsonl              [R1 已有，保留]
  ├── eval-results-*.jsonl        [R1 已有，保留]
  ├── evolution-applied.jsonl     [apply.rs 已有，5 字段不变]
  ├── evolution-feedback.jsonl    [R1 已有，保留]
  └── evolution-changes.jsonl     [R2 新增，本设计主目标]
```

不写新数据库表。`evolution-changes.jsonl` 是 jsonl 文件，不破约束 #4（老板 12:44 重读约束 #4：「jsonl 是 database table 的替代品」）。

---

## 3. ChangeRecord 结构（独立 jsonl 持久化，B 方案）

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeRecord {
    pub change_id: String,                       // "chg-" + proposal_id（派生，DERIVABILITY.md 字段 2）
    pub parent_id: Option<String>,               // 父 change_id；根 = None
    pub schema_version: u32,                     // 默认 1（派生）
    pub layer: EvolutionLayer,                   // 6 层枚举（Memory/PromptHint/ToolSchema/Skill/Parameter/Code）

    pub origin: MutationOrigin,                  // 来自哪条 ProposalOrigin
    pub proposal_id: String,                     // 关联到 EvolutionProposal.proposal_id
    pub target: ProposalTarget,                  // 同 EvolutionProposal.target（透传）
    pub candidate: Candidate,                    // 候选快照（不可变）

    pub eval_before: Option<EvalResult>,         // 弱派生但存快照（DERIVABILITY 字段 6）
    pub eval_after: Option<EvalResult>,          // 同上

    pub status: ChangeStatus,                    // 9 态状态机
    pub hard_constraint_compliance: bool,        // 派生（构造时算，DERIVABILITY 字段 8）

    pub created_at_ms: i64,                      // ChangeRecord 创建
    pub applied_at: Option<i64>,                 // = evolution-applied.jsonl[mem_key].applied_at_ms
    pub rolled_back_at: Option<i64>,             // 不可派生，独立落
    pub rollback_reason: Option<String>,

    pub human_approver: Option<String>,          // 谁批准的（"boss" / "auto"）
    pub approval_source: ApprovalSource,         // AutoApplied / HumanApproved / Rejected / NotApproved
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ChangeStatus {
    Proposed,       // 候选刚产出，未沙箱
    Shadowing,      // R3 影子测试中
    ShadowPassed,   // 影子通过，待人工/自动批准
    Approved,       // 已批准（AutoApplied / HumanApproved 显式区分在 ApprovalSource）
    Canary,         // R3 金丝雀 5%
    Active,         // 已全量生效
    Rejected,       // 拒绝（人工/自动/合规失败）
    RolledBack,     // 已回滚
    Expired,        // 超期（TTL 14 天后未推进）
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ApprovalSource {
    AutoApplied,        // 满足 gate 后自动应用
    HumanApproved,      // 人工从 R5 面板批准
    SystemRejected,     // 系统拒绝（hard_constraint_compliance=false）
    HumanRejected,      // 人工拒绝
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvolutionLayer {
    Parameter,      // 参数层（R7+）
    Policy,         // 策略层 = MemoryPolicy
    PromptHint,
    ToolSchema,
    Skill,
    Code,           // R8+，只生成候选走 PR
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    /// 提案的 suggestion.text（不可变快照）
    pub suggestion_text: String,
    /// evolution-applied.jsonl 的 mem_key（"evo:<proposal_id>"）
    pub mem_key: String,
    /// evolution-applied.jsonl 的 impact（透传）
    pub impact: ImpactLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalResult {
    pub task_success_rate: f64,
    pub tool_call_efficiency: f64,
    pub behavior_deviation: f64,
    pub rollback_rate: f64,
    pub pollution_survival_days: f64,
    pub evaluated_at_ms: i64,
}
```

---

## 4. 关键决策点

### 4.1 状态机非法路径拦截

```
✅ 允许：
  Proposed → Shadowing → ShadowPassed → Approved → Canary → Active
  Proposed → Rejected
  Approved → Active（直跳 canary，R3 配置）
  Active → RolledBack
  任何状态 → Expired（TTL 14 天）

❌ 禁止：
  Proposed → Active（绕过沙箱）
  Rejected → Active（不可复活）
  RolledBack → Active（不可复活，需新 proposal）
```

### 4.2 AutoApplied vs HumanApproved 显式区分

硬约束 ② 要求 status 必须 Pending → Approved → Applied。
- `status = Approved` + `approval_source = AutoApplied` → 系统自动走完
- `status = Approved` + `approval_source = HumanApproved` → 人工批准后走完
- Audit log 里写两条不同事件：`evolution.auto_applied` / `evolution.human_approved`

### 4.3 hard_constraint_compliance 计算时机

构造 ChangeRecord 时计算（pure function）：

```rust
fn check_compliance(p: &EvolutionProposal) -> bool {
    // 硬约束 5/6：仅 MemoryHint + High/Medium 可自动应用
    // 硬约束 7：tags[0] = "evo:<proposal_id>"（apply 入口保证）
    // 硬约束 8：importance 4/3 + source="system"（apply 入口保证）
    // 硬约束 4：不写新表（实施期审计）
    matches!(p.category, ProposalCategory::MemoryHint)
        && !matches!(p.impact, ImpactLevel::Low)
}
```

不通过 → ChangeStatus=Rejected, ApprovalSource=SystemRejected。

### 4.4 parent_id 链

- 首条 ChangeRecord：`parent_id = None`
- 子代提案（同一问题的迭代版本）：`parent_id = "chg-<原 proposal_id>"`
- 链查询：`SELECT change_id FROM evolution-changes.jsonl WHERE parent_id = ?`

---

## 5. 模块结构（B 方案：jsonl 持久化）

```
src-tauri/src/evolution/
  ├── apply.rs          [已有，仅改 audit_event 名]
  ├── change/           [R2 新增]
  │   ├── mod.rs        — ChangeRecord + 公开 API
  │   ├── record.rs     — struct 定义 + jsonl 读写（Serialize/Deserialize）
  │   ├── status.rs     — 状态机 + 非法路径拦截
  │   ├── derive.rs     — 从 EvolutionProposal 派生 ChangeRecord
  │   └── tests.rs      — 单测（非法 status / parent 链 / schema_version / 全生命周期）
```

`pub mod evolution::change;` 加到 lib.rs（+1 行注册，不改其他）。

---

## 6. 单测清单（验收，B 方案）

- ✅ `status_rejects_illegal_transition`：Proposed → Active 抛 Err
- ✅ `status_allows_legal_transition`：Proposed → Shadowing → ... → Active 全过
- ✅ `parent_chain_query_returns_root_and_descendants`
- ✅ `schema_version_v1_defaulted_when_missing`
- ✅ `integration_full_lifecycle`：Proposal → ChangeRecord → Active → RolledBack 全链路
- ✅ `hard_constraint_compliance_false_for_low_impact`
- ✅ `approval_source_distinguishes_auto_vs_human`
- ✅ `jsonl_roundtrip`：ChangeRecord 写入 → 读回 → 字段一致
- ✅ `jsonl_appends_preserve_old_records`：多次写不丢历史

---

## 7. 集成测试清单

- ✅ 端到端：consolidate → apply → ChangeRecord 写 jsonl → status=Active → 人工回滚 → status=RolledBack → 下一轮 injection_block 不再包含
- ✅ 并发：两条 ChangeRecord 写 jsonl 的原子性（追加模式天然原子）
- ✅ Schema 兼容：读老格式 jsonl（5 字段无 layer）不 panic
- ✅ 回放：读 jsonl → 重建 ChangeRecord 列表 → parent_id 链完整

---

## 8. 验收对应 spec R2（B 方案）

| spec 要求 | 本设计实现（B 方案） |
|-----------|-------------------|
| ChangeRecord 结构 | ✅ 第 3 节 |
| evolution-changes.jsonl 写入/读取 | ✅ 第 2 节 + 第 5 节 record.rs |
| status 流转函数（禁止非法路径） | ✅ 第 4.1 节 + 第 5 节 status.rs |
| parent_id 链查询 | ✅ 第 4.4 节 |
| schema_version 兼容 | ✅ 第 3 节（默认 1） |

---

## 9. G 方案 ADR（Future Work）

如果未来要做 G（落 mem_items），先做完 8 个验证：

| # | 假设 | 12:44 现场验证状态 |
|---|------|-------------------|
| 1 | tags 命名空间不冲突 | ⚠️ 依赖前缀纪律（脆） |
| 2 | 不会被注入 prompt | ❌ **致命**：`rank.rs:160-165` `hits` 不过滤 kind，会污染 |
| 3 | 不会被淘汰 | ❌ **部分致命**：Active 重要性 4 大致安全，无 kind 特殊保护 |
| 4 | store::update_by_id 存在 | ✅ R0 已确认 |
| 5 | insert_item(None) 跳去重 | ✅ R0 已确认 |
| 6 | kind 是 String 非 enum | ✅ R0 已确认 |
| 7 | content 长度允许 | ⚠️ MAX=800（mod.rs:33），store 层不校验，自管 |
| 8 | 嵌入检索有意义 | N/A 主观 |

G 方案 ADR 待 8 验证完成后写。

---

## 10. 启动条件（已满足）

老板拍 B（2026-09-18 12:44）→ 本设计直接实施。

R2 实施预计 4 天（Day 4-7），今天 12:44 开干。