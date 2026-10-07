# Evolution 域分层重构 · 批次 A：接口文档（只出文档，不写实现）

> 状态：**待签收**。签收清单全勾后才进批次 B。本文档所有类型/签名映射的是
> `src/evolution/` 现有真实代码（2026-10-07 主干，commit `77c9e62`），不是抽象设想。
>
> **一个必须先说清的事实修正**：任务书 1.1 描述「拓扑（DAG/快照/回放）与策略交织」。
> 实读代码后确认：**evolution 域没有 DAG，也没有通用事件回放**。它是单向流水线：
> consolidate ops → `derive_proposals`（纯函数）→ 提案/变更/落库三本 append-only
> jsonl 账本 → gate 判定 → `apply_one` 写记忆 → change 状态机。DAG 在 workflow 域
> （`workflow_runner.rs`），与本文档无关。因此本文档把任务书的「拓扑层」落到该域
> 真实对应物——**数据层**（账本/派生/状态机，纯函数），分层线不变、名词对齐现实。

---

## 0. 域现状底册（批次 A 的事实基础）

| 现有文件 | 实际职责 | 归层 |
|---|---|---|
| `derive.rs` | consolidate ops → `Vec<EvolutionProposal>`（含 impact 定档、id 派生） | 数据层（派生）+ 内嵌策略点（impact 定档） |
| `proposal.rs` | 提案类型 / `proposal_id` 确定性短 hash / `is_reversible` | 数据层 |
| `candidate/` | 候选池 jsonl 读写 + 冲突检测/消解/跨层排序 | 数据层 + 策略点（conflict 消解规则） |
| `change/` | ChangeRecord 状态机（Pending→…→Rollback）、`passes_auto_apply_gate`、layer 派生 | 数据层（状态机）+ 策略点（gate 变体） |
| `apply.rs` | `auto_apply_gate`、`apply_one`（幂等写 lesson、impact→importance）、批量入口 | 消费端 + 策略点 |
| `policy.rs` | `applyPolicy`（auto/confirm）配置读写（bot-config.json IO） | **上下文层**（不是策略纯函数——见 1.8 顺序修正） |
| `sandbox/kill_switch.rs` | kill 开关配置 + `should_shadow_only()` 等纯谓词 | 配置 IO + 纯谓词 |
| `mod.rs` | `EVOLUTION_STORE_LOCK`、post_consolidation 编排 | 上下文层（锁/编排） |
| `panel/`、`observe/`、`activation.rs`、`sandbox/shadow+io` | 决策板命令、观察面、实验态 | **本文档范围外**（独立批次，见 §9） |

并发事实：`EVOLUTION_STORE_LOCK` 单写者锁；`apply_from_consolidation` 走
`spawn_blocking`；panel 命令线程与 emit 主链并发触达同一批自由函数。
随机源事实：**全域 0 处随机**（grep 证实），一切判定确定性可复现。

---

## 1. 类型定义草案（字段 / 所有权 / 生命周期）

### 1.1 数据层输出：`TopologyView<'a>`（对齐现实的「只读账本视图」）

```rust
/// 数据层 → 策略层的只读视图。借引用，不拥有数据；构造廉价（纯切片包装）。
/// 生命周期 'a 绑定调用方持有的账本缓冲，策略层不得存储引用离开本调用。
pub struct TopologyView<'a> {
    /// 本轮派生的提案（derive.rs 产出，尚未入池）
    pub proposals: &'a [EvolutionProposal],
    /// 候选池现存条目（candidate jsonl 反序列化结果）
    pub entries: &'a [ProposalEntry],
    /// 变更账本（change jsonl），供状态机查询
    pub changes: &'a [ChangeRecord],
}
```

- 所有权：**只借不有**（性能约定见 §3）。
- 生命周期：调用栈内；跨线程时由调用方 `move` 底层数据（`Vec`），view 随用随建。

### 1.2 上下文注入：`EvalContext`（每周期一次）

```rust
/// 运行时上下文只读快照：每个 evolution 周期（一次 post_consolidation /
/// 一次 panel 批处理）构造一次，构造后不可变。
/// **不含随机源**——全域当前零随机（§7 不变式 7）；未来引入随机必须
/// 加进本结构由调用方注入，禁止策略内部 thread_rng()（CI 检查项，见 §8）。
pub struct EvalContext {
    /// applyPolicy 档位快照（policy.rs 读出；缺省/读失败 = Auto，与现状一致）
    pub apply_policy: ApplyPolicy,
    /// kill_switch 只读快照（kill_switch.rs 结构体克隆；读取失败 = 全关默认，
    /// 与 apply.rs 现状一致）
    pub kill_switch: KillSwitch,
    /// shadow 观察是否开启（observe::shadow::is_enabled 的当轮快照）
    pub shadow_enabled: bool,
    /// 本周期基准时间（替代散落的 chrono::Utc::now()，可测性）
    pub now_ms: i64,
}
```

**EvalContext 归属四问（任务书 1.2 第三块，逐条定死）**：

1. **gate 判定依赖什么？** → 只依赖 proposal 自身字段
   （`category == MemoryHint && impact ∈ {High, Medium}`，实读
   `auto_apply_gate` / `passes_auto_apply_gate` 证实）→ **纯策略**。
   `applyPolicy` 档位与 kill_switch 不进 gate 函数——它们决定「gate 通过后走
   自动轨还是候选池」，属上下文分流，由调用方用 `EvalContext` 判定。
2. **重要性评分依赖什么？** → 只依赖 `proposal.impact`（apply.rs 现状
   `High→4，其余→3`；derive.rs 定档 `merge→Medium` 等）→ **纯策略**，
   无运行时权重，不走注入。
3. **抽样选择的随机源** → 现状不存在抽样（conflict 消解完全确定性：
   impact 降序 → created_at 升序）。EvalContext **不含随机源字段**；
   若未来需要，按本节头部约束注入。
4. **回放** → 该域唯一「重放」是 change 状态机转移与 applied 账本统计重建，
   均为**数据层纯函数**消费账本序列；策略层不写重放状态，只能经数据层
   产生新记录（写路径仍受 `EVOLUTION_STORE_LOCK` 纪律约束）。

### 1.3 策略层输出类型

```rust
/// gate 判定结果。**Reject 是正常返回值，不是 Err**（任务书 1.10，写死）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// 过 gate，允许进入自动轨（是否真自动落库仍由 EvalContext.apply_policy 分流）
    Approved,
    /// 不过 gate（如 Low impact / 非 MemoryHint）——进候选池或丢弃，属正常业务结果
    Rejected { reason: &'static str },
}

/// 重要性评分（写 lesson 时落 mem_items.importance；DB 层已有 1..=5 clamp）
pub type ImportanceScore = u8;

/// 冲突消解结果（现 resolve_conflict 的 (winner, loser) 语义收拢）
pub struct SelectionResult<'a> {
    pub winner: &'a ProposalEntry,
    pub loser: &'a ProposalEntry,
}
```

### 1.4 错误分层（任务书 1.10）

```rust
/// 数据层错误：账本损坏 / 状态机非法转移 / 快照断裂
pub enum DataError {
    LedgerCorrupt { file: &'static str, line: usize },
    IllegalTransition { from: ChangeStatus, to: ChangeStatus },
}
/// 策略层错误：策略无法执行（上下文缺失等）。**gate Reject 永远不是这个**。
pub enum PolicyError {
    ContextMissing(&'static str),
}
```

现状对齐：`read_all` 坏行跳过 + `.corrupt` 备份已是数据层容错；状态机
`status::transition` 已返 `Err`。批次 B 只补类型不换行为。

---

## 2. 纯函数与 trait 签名草案

### 2.1 数据层（自由函数，保持现状签名，只补类型）

```rust
// 均已存在、已纯，批次 B 不动逻辑：
derive_proposals(ops: &[ConsolidateOp], report: &ConsolidateReport) -> Vec<EvolutionProposal>
read_all / write_proposals / append_applied_record（账本 IO，锁纪律同现状）
status::transition(...) -> Result<ChangeStatus, String>
```

### 2.2 策略 trait（默认实现 = 现行为逐函数搬运）

```rust
/// 策略层 trait。Send + Sync（拍板项 §9-P1 建议=要，见该节论证）。
/// 实现**必须纯**：不得读文件、不得读时钟、不得持锁——一切环境输入经
/// TopologyView / EvalContext 注入（可测性不变式 §7-6）。
pub trait EvolutionPolicy: Send + Sync {
    /// gate：proposal 是否够格自动轨。Reject 是正常值。
    fn gate(&self, p: &EvolutionProposal) -> GateDecision;

    /// 重要性映射（写 lesson 时用）
    fn importance(&self, impact: ImpactLevel) -> ImportanceScore;

    /// 冲突消解：胜负判定（现 resolve_conflict 规则原样）
    fn resolve<'a>(&self, a: &'a ProposalEntry, b: &'a ProposalEntry)
        -> SelectionResult<'a>;

    /// 跨层排序（现 sort_entries_cross_layer 规则原样；原地排序属消费
        /// 便利方法，语义纯——同输入同序）
    fn order_entries(&self, entries: &mut [ProposalEntry]);
}

/// 默认实现：逐函数搬运现行为，**一行逻辑不改**（批次 B 等价对照的基准）。
pub struct DefaultEvolutionPolicy;
```

谁构造、从哪来：调用点（`apply_from_consolidation` / panel 批处理）各自构造
`&DefaultEvolutionPolicy` 静态实例 + 当轮 `EvalContext`；**不进 Tauri State**
（拍板遗留 §9-P1）。

### 2.3 上下文分流谓词（保留自由函数形态，收进上下文层）

```rust
// policy.rs 现有 auto_apply_allowed 保留签名；批次 B 内部改为
// ctx.apply_policy 判定（语义不变：Confirm = 不自动）。kill_switch 的
// should_shadow_only()/should_disable_notification() 保持纯谓词不动。
```

---

## 3. 性能与所有权约定（写死，不留实现期）

1. `TopologyView<'a>` **只借不有**；策略方法签名一律 `&self` + 借用入参。
2. 数据层纯函数不整图拷贝：账本读取维持现有「整文件读 → 反序列化一次」
   （jsonl 体量 ≤ 数百行、百 KB 级，现状即此，批次 B 不改读法）。
3. 策略层输出全部小对象（枚举/引用对），零分配热点。`Rejected.reason`
   用 `&'static str`（文案常量），不做 String 拼接。
4. **性能敏感点标注**（允许拥有的唯一例外）：`EvalContext.kill_switch`
   为结构体克隆（小、每周期一次）；除此之外任何「拥有化」改动必须在
   批次 B 的 PR 描述里单独立项说明。

---

## 4. 数据流向图（只读/产生新快照标注）

```
consolidate ops ──► derive_proposals（数据层·纯）──► Vec<EvolutionProposal>
                                                            │
                    ┌───────────────────────────────────────┘
                    ▼
        TopologyView<'a>（只读视图：proposals + 候选池条目 + 变更账本）
                    │
                    ▼
   EvalContext（每周期一次：apply_policy / kill_switch / shadow / now_ms）
                    │
                    ▼
   EvolutionPolicy（策略层·纯）：gate → Reject{reason}? ──► 进候选池（数据层写）
                    │                                └─► Approved
                    │                                          │
                    │            EvalContext.apply_policy ─────┤
                    │                                          ▼
                    │                     Auto → apply_one（importance 来自策略层）
                    │                                           │
                    ▼                                           ▼
        conflict 消解（policy.resolve/order）        lesson 写入 + change 账本追加
                    │                                  （数据层写，EVOLUTION_STORE_LOCK）
                    ▼
        SelectionResult{winner, loser} → 败者丢弃（数据层）
```

只读：TopologyView / EvalContext 消费；产生新快照/新行：仅经数据层写函数
（jsonl append / DB insert），策略层永不直接落盘。

---

## 5. 迁移映射表（旧函数 → 新函数/trait 方法）

| 旧（现签名不动） | 新归属 | 迁移方式 |
|---|---|---|
| `apply.rs::auto_apply_gate(p)` | `DefaultEvolutionPolicy::gate` | **零逻辑委托**（旧体只剩一行调新） |
| `change/derive.rs::passes_auto_apply_gate(p)` | `DefaultEvolutionPolicy::gate` 的同义入口 | **零逻辑委托**（两函数现状语义相同：MemoryHint + 非 Low；批次 B 收敛为单一实现，差异在测试里钉死为等价） |
| `apply.rs::apply_one` 内 `impact→importance` 映射 | `DefaultEvolutionPolicy::importance` | **零逻辑委托**（`High→4，_→3`） |
| `candidate/conflict.rs::resolve_conflict(a,b)` | `DefaultEvolutionPolicy::resolve` | **零逻辑委托** |
| `candidate/conflict.rs::sort_entries_cross_layer(v)` | `DefaultEvolutionPolicy::order_entries` | **零逻辑委托** |
| `candidate/conflict.rs::impact_ord / layer_priority` | 策略层私有辅助（随 resolve/order 搬） | 搬迁，旧 pub 函数委托保留 |
| `change/derive.rs::derive_layer(category)` | 数据层保留（layer 是账本字段派生，非决策） | **不动** |
| `policy.rs::auto_apply_allowed / read_apply_policy*` | 上下文层保留（配置 IO），签名不变 | **不动逻辑**（批次 B 仅让调用方经 EvalContext 传值） |
| `apply.rs::apply_from_consolidation` 的 kill/notify 分流 | 调用方构造 EvalContext 后判 `kill_switch` 谓词 | **适配**（现读现判语义保留：cfg 在 spawn_blocking 内读） |
| `derive.rs` impact 定档（merge→Medium 等） | 数据层保留 | **不动**（证据强度定档是派生规则，不是运行时决策；改它属批次 C 后的算法演进） |

「需要适配」仅 1 处（kill/notify 分流），其余全部零逻辑委托。

## 5.1 迁移顺序（对任务书 1.8 的现实修正）

任务书定的 policy→apply→derive→panel 是按文件名平推。实读后：`policy.rs`
是配置 IO（上下文层），根本不是策略纯函数，先行它没有可抽的 trait 内容。
**从实际依赖出发的批次 B 顺序**（每步全量测试后再下一步）：

1. **B-1 `candidate/conflict.rs` + `change/derive.rs`**：最纯（无 IO 无锁），
   trait 首次落地 + 快照对照测试。
2. **B-2 `apply.rs`**：`auto_apply_gate` / importance 收进 trait；`apply_one`
   改调 trait（行为零变化）。
3. **B-3 `derive.rs`**：只确认数据层纯度、不动逻辑（映射表判定不迁）。
4. **B-4 上下文收拢**：`EvalContext` 构造进 `apply_from_consolidation` 与
   panel 批处理；kill/notify 分流改读 ctx（唯一适配点）。
5. **panel 消费端**：仅改传参方式（构造 ctx），无策略逻辑。

---

## 6. 测试重写清单（批次 B 当场改，批次 C 不再改契约）

| 测试 | 处置 |
|---|---|
| `apply.rs::tests` 全部（gate_rejects_low 等） | 旧入口断言**原样保留为回归**；新增同断言走 trait 入口的镜像用例 |
| `change/derive.rs::tests` gate 相关 | 同上（旧→新双覆盖） |
| `candidate/conflict.rs::tests` 全部 | 同上 |
| `apply.rs::tests::auto_applied_cr_rejects_non_gate_proposal` | 保留；补 trait 入口等价用例 |
| 新增**快照对照**（必须有，任务书 1.7-1） | 同一输入集：旧自由函数输出 vs trait 路径输出，序列化逐字节相等（gate 结果 / importance / 消解胜负 / 排序序） |
| 新增**新旧等价对照** | 至少一组：`auto_apply_gate(p) == matches!(policy.gate(p), Approved)` 全 impact×category 笛卡尔覆盖（3×4=12 例，穷举） |
| 属性测试 / 回放对照（1.7-2/3） | **不做**：全域无随机、无事件回放，无从随机化也无回放序列可比；不造假需求 |
| 影子模式（1.7 条件项） | **不做**：`evolution.shadow.enabled` 默认关、无常态运行时流量（kill_switch 默认全关），不满足「真实运行时流量」前提 |

---

## 7. 不变式（批次 B 每步 PR 描述必须复述）

1. 账本 append-only：proposals/changes/applied jsonl 只追加/整文件 RMW（现状），策略层零直写。
2. change 状态机转移合法（status.rs 既有校验），非法转移 = DataError。
3. 提案 id 确定性：同输入同 id（normalize_for_hash + 排序 refs 二次 hash，现状）。
4. 同一 TopologyView + 同一 EvalContext → 策略层输出恒等（纯度）。
5. 旧入口与新入口输出等价（等价对照测试钉死）。
6. 策略层零环境读取：无文件 IO、无时钟、无锁、无 thread_rng（CI 检查 §8）。
7. 全域无随机源；引入随机必须经 EvalContext 注入。
8. 写路径锁纪律不变：EVOLUTION_STORE_LOCK 单写者、apply 不进该锁（panel 三段式纪律，现状）。
9. kill_switch「现读现判」语义不变（每周期构造 ctx 时读一次，周期内不重读——与现状每轮一次读一致）。

---

## 8. 依赖方向 CI 检查（批次 B 落地 `tests-audit/audit_evolution_layering.py`）

grep 级断言（同 audit_module_map.py 风格）：

- 数据层文件（`derive.rs`、`proposal.rs`、`change/`、`candidate/`）**禁止** `use crate::evolution::policy`、`use crate::bot::`（配置读）、`tauri::AppHandle`。
- 策略文件/实现（`DefaultEvolutionPolicy` 所在文件）**禁止** `std::fs`、`chrono::Utc::now`、`rand`、`Mutex`、`use crate::evolution::panel`。
- 全域禁止 `thread_rng`（不变式 7 的机器化）。
- 违例 exit 1，进 pre-commit/test-all 的 tests-audit 环节。

---

## 9. 拍板遗留项（选项 + 默认建议 + 影响面；不含「暂定」）

| # | 问题 | 选项 | 默认建议 | 影响面 |
|---|---|---|---|---|
| P1 | trait 对象是否进 Tauri State？ | A. 不进，调用点静态构造<br>B. 进 State（可热换实现） | **A**（无第二实现需求；B 引入 State 泛化+管理 UI，无收益） | A≈0；B 动 lib.rs/panel |
| P2 | `GateDecision::Rejected` 是否带 reason？ | A. 带 `&'static str`<br>B. 无载荷 | **A**（审计/测试断言要指认拒绝原因；零分配） | A≈0 |
| P3 | `passes_auto_apply_gate` 与 `auto_apply_gate` 语义重复 | A. 批次 B 收敛为单一 trait 实现，两旧函数都委托<br>B. 保留两套各自实现 | **A**（重复实现正是本次要消的；等价对照测试覆盖收敛风险） | A 动 change/derive.rs 委托体 |
| P4 | importance 返回类型 | A. `u8` 裸类型<br>B. newtype | **A**（DB 层 clamp 已有；newtype 纯仪式） | — |
| P5 | 影子模式 | A. 不做<br>B. 做 | **A**（无常态流量，任务书 1.7 自己的前提不满足） | — |
| P6 | 批次 C 观察期 | A. 批次 B 合入后 ≥1 个全量测试日 + 1 次 push 门禁<br>B. 固定天数 | **A**（本仓无生产部署流量，测试面即全部流量） | — |
| P7 | `Send + Sync`（任务书 1.11） | A. trait 要求 Send+Sync<br>B. 不要求 | **A**（现状自由函数已跨 spawn_blocking/命令线程；不写死则批次 C 后想换并发模型要重开签名） | A≈0（纯函数天然满足） |
| P8 | 回放/账本格式版本化（任务书 1.12） | A. 不版本化，新字段走 `#[serde(default)]` 向后兼容（仓内既有先例）<br>B. 加 version 字段 | **A**（evolution 账本无跨版本 replay 消费方；批次 C 不删不改任何 jsonl 格式，无迁移脚本需求） | — |

---

## 10. 各批次回滚方式（动手前写清）

- **批次 A（本文档）**：文档本身，`git revert` 单提交即可，无代码影响。
- **批次 B**：trait + 默认实现是**纯增量**；旧函数体为委托。回滚 = `git revert`
  对应提交，旧函数恢复原体，行为逐字节回到 B 前（可回滚性硬要求 1.6 的
  落实方式：**不删任何旧符号、不改任何旧签名**）。
- **批次 C**：单独分支；删除旧委托函数 + 测试镜像用例。回滚 = 丢弃分支。
  因 B 已保证旧=新（等价对照），C 的删除对行为零影响——这是 C 敢做成
  「纯删除」的依据。
- **CI 检查脚本**：独立文件，`git revert` 或从 tests-audit 移除即回退。

## 10.1 批次 C 明确不做清单

不混新逻辑、不混性能优化、不混其他域；不改 jsonl 格式；不改测试契约
（B 已改完）；不动 `observe/`、`activation.rs`、`sandbox/`（范围外）。

---

## 11. 签收清单

- [x] `EvalContext` 归属明确（§1.2 四问逐条：gate 纯策略 / importance 纯策略 / 无随机源写死 / 回放属数据层）
- [x] 拓扑（数据）层无策略依赖（§8 CI 规则 + §5 映射表标注）
- [x] 策略 trait 可注入随机源（结构上经 EvalContext；当前无随机字段，§1.2-3 + 不变式 7）
- [x] 迁移映射完整（§5 十行映射，仅 1 处适配）
- [x] 不变式写清（§9 条）
- [x] 回滚方式写清（§10，批次 B 纯增量可 revert）
- [x] 测试重写清单写清（§6，含等价对照穷举 12 例；属性/回放/影子三项不做并给据）
- [x] 性能所有权约定写清（§3，借不有 + 唯一例外标注）
- [x] 错误分层写清（§1.4，DataError/PolicyError 分型）
- [x] gate 拒绝语义写清（§1.3：Reject 是正常值；Err 仅限无法执行）
- [x] 拍板遗留项列出选项（§9 八项，全部带默认建议与影响面）

**范围外（独立批次，本文档不覆盖）**：`observe/`（shadow/synthetic/metrics/stop
观察面）、`activation.rs`（实验态）、`sandbox/shadow+io`（沙箱管线）。
