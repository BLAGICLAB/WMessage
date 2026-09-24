# Batch Spec: EV-3b-F

## 目的

修 C5-EV-3b-F 3 条（哈希 / 校验缺），3 处独立改动：

1. **proposal.rs:186**（high）：`short_hash` 用 `DefaultHasher`——算法不在
   Rust 稳定性契约内（SipHash 变体历史变更过），toolchain 升级或跨机
   replay 审计日志会让同一 proposal 产出不同 proposal_id，静默打破
   dedup 契约。修：换成仓内已有的 **FNV-1a-64**（sandbox/routing.rs
   `fnv1a`，pub 可复用）——跨版本/跨机确定性。加已知答案测试
   （FNV-1a-64 标准向量："" → cbf29ce484222325、"a" → af63dc4c8601ec8c）。
   **影响面已知悉**：short_hash 的调用方 = proposal_id + derive.rs 矛盾
   判别位（EV-3b-D 加的）——两者都是前向生效的 dedup key；既有 jsonl
   条目保留旧 id，升级边界上同一 proposal 重派生会得新 id（最多重入
   一次，R 阶段 dev 特性，可接受）。id 形态不变（16 hex），
   DERIVABILITY.md 契约不破。
2. **observe/synthetic.rs:29**（high）：SyntheticConfig 的比例字段无校验
   ——负数/NaN/总和>1 静默歪斜分布，余量概率静默落 Pooled，打败
   「验证 R6 指标逻辑」的既定目的。**同根 medium :94 一并**：
   window_days=0 会在 next_int modulo-by-zero panic、负数 as u32 回绕、
   天文数字 i64 溢出。修：加 `SyntheticConfig::validate()`（各 ratio
   有限且 ∈[0,1]、promoted+rejected+expired ≤1、active+rolled_back ≤1、
   window_days ≥1），generate() 入口 `validate()` 不合法即 panic
   （带明确消息；唯一生产调用方 observe_run.rs 是 dev bin，expect 风格
   一致）。signature 不变（generate 仍返回 SyntheticData，无 ripple）。
3. **observe/shadow.rs:345**（high）：`shadow_apply_for_batch_with_app`
   （文档称「唯一对外入口」）直接调 append_change 却**不碰**
   TOTAL_WRITES/FAILED_WRITES 原子计数器——「失败率 >5% 告警」不变量
   对生产流量永不触发，total_writes/total_failed 少报。修：with_app 的
   Ok/Err 分支补与 trait 变体相同的 fetch_add（最小修；走 ShadowSink
   重构 = 结构改动不做）。**不碰 :269 medium**（with_reversibility 缺
   循环后告警——不属本簇行号）。

## 人类可读摘要

- family: integrity-missing（哈希不稳定 / 配置校验缺 / 计数器绕开）
- 覆盖 findings: 3（+1 同根 medium 一并）
- 预估 diff: 3 files / +63/-9 lines → 实测 +109/-11（A 类，budget 校正 **+115/-15**）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 3

## 核实的编译层事实（spec 前已核）

1. `short_hash` 是 pub(crate)，调用方 = proposal.rs proposal_id +
   derive.rs:119 矛盾判别位，无其它（grep 全仓）。
2. `fnv1a` 在 sandbox/routing.rs:12 为 `pub fn`，sandbox/mod.rs:13
   `pub mod routing`——proposal.rs 可 `use crate::evolution::sandbox
   ::routing::fnv1a`。
3. FNV-1a-64 标准向量："a" → 0xaf63dc4c8601ec8c（已知答案可锁）。
4. generate() 生产调用方唯一 = observe_run.rs:178（dev bin，expect
   风格）；SyntheticConfig 构造点 = 该 bin + 本文件 Default+测试。
5. with_app Ok/Err 分支（shadow.rs:347-383）与 trait 变体 :270-277 的
   计数器增量点一一对应——最小补法无歧义。
6. 既有 asserts：proposal 27 / synthetic 12 / observe/shadow 79。

## 修法

- **proposal.rs**：short_hash 改 FNV-1a-64（复用 routing::fnv1a，输出
  仍 16 hex）；doc 更新（算法=仓内固定 FNV-1a，跨版本稳定）；
  新增 known-answer 测试（2 asserts）。
- **synthetic.rs**：新增 `validate() -> Result<(), String>` + generate
  入口断言 + 测试（负 ratio / NaN / 总和>1 / window_days=0 各 panic）。
- **observe/shadow.rs**：with_app 两分支补 TOTAL_WRITES/FAILED_WRITES
  fetch_add（与 trait 变体逐点对齐）。

## 红线

- id 形态不变（16 hex）；不改 normalize_for_hash（数字归一化是有意设计）
- 不动 trace.rs compute_trace_id 的 DefaultHasher（trace.rs:74 medium，
  不属本簇；簇 H 是 trace 双轨 :104/:149/:193）
- generate signature 不变；不重构 with_app 走 ShadowSink（结构改动）
- 不碰 observe/shadow.rs :218/:269/:297/:303 mediums（不属本簇）

## spec 起草后自查三条

1. `expected_files` = 3：proposal.rs + observe/synthetic.rs +
   observe/shadow.rs。无 ripple（fnv1a 复用不改 routing.rs）。
2. budget = **A 类**：proposal +15/-8、synthetic +46/-1、shadow +2/-0：
   初估 +63/-9 → 实测 +109/-11（validate 结构体更新语法测试块 fmt 展开超估），校正 +115/-15 → OCR r1 采纳 2 medium（window_days 上限校验 + validator 本体契约测试）实测 +134/-11，再校正 → **max +140/-15**。
3. 三条 findings 的 fix 字段均已写明 ripple（均无）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "EV-3b-F",
  "family": "integrity-missing",
  "expected_files": [
    "src-tauri/src/evolution/proposal.rs",
    "src-tauri/src/evolution/observe/synthetic.rs",
    "src-tauri/src/evolution/observe/shadow.rs"
  ],
  "max_lines_added": 140,
  "max_lines_removed": 15,
  "findings": [
    {"id": "C5-EV-3b-F-1", "file": "src-tauri/src/evolution/proposal.rs", "line": 186, "fix": "short_hash 换仓内 FNV-1a-64（routing::fnv1a 复用），跨版本/跨机确定性 + known-answer 测试；id 形态 16 hex 不变；前向生效（既有 jsonl 旧 id 保留）；无 ripple（调用方仅 proposal_id + derive 判别位）"},
    {"id": "C5-EV-3b-F-2", "file": "src-tauri/src/evolution/observe/synthetic.rs", "line": 29, "fix": "SyntheticConfig::validate()（ratio 有限∈[0,1]、三桶和≤1、active+rolled_back≤1、window_days≥1——同根 medium :94 一并）+ generate 入口断言；signature 不变无 ripple"},
    {"id": "C5-EV-3b-F-3", "file": "src-tauri/src/evolution/observe/shadow.rs", "line": 345, "fix": "with_app Ok/Err 分支补 TOTAL_WRITES/FAILED_WRITES fetch_add（与 trait 变体逐点对齐），失败率告警不变量对生产入口恢复可触发；无 ripple"}
  ],
  "assertions_min": {
    "src-tauri/src/evolution/proposal.rs": 27,
    "src-tauri/src/evolution/observe/synthetic.rs": 12,
    "src-tauri/src/evolution/observe/shadow.rs": 79
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 3
  },
  "stop_conditions": [
    "compile_failure",
    "architecture_blocker",
    "family_heterogeneity",
    "new_high_different_root",
    "gate_fail"
  ]
}
```
