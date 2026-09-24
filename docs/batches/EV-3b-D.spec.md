# Batch Spec: EV-3b-D

## 目的

修 C5-EV-3b-D 3 条（溢出 / 不可逆无留痕 / 去重坍缩），三条同 family（
candidate 池完整性）但**批内 3 处独立改动**，互无 ripple：

1. **candidate/ttl.rs:41**（high）：`compute_expires_at` 用 unchecked
   `created_at_ms + TTL_MS`——jsonl 腐败/对抗值近 i64::MAX 会 wrap 成大负数
   → is_expired 恒 true → 条目被静默立刻过期/淘汰 = TTL 绕过 + 丢历史。
   **且核出真实生产路径不走该 fn**：`candidate/derive.rs:49` 与
   `panel/commands.rs:357` 两处内联 `now_ms + TTL_MS` 有同样缺陷——
   一并收敛到 `compute_expires_at`（saturating_add）单点实现。
   溢出语义：饱和到 i64::MAX = **永不过期**（保审计轨迹，符合本文件
   「软淘汰保历史」意图），而非 wrap 成负数（静默立刻淘汰）。
2. **candidate/ttl.rs:34**（high）：evict_expired 不可逆、无留痕。
   (a) status 门已被 EV-3b-C2（92d1b9a）修掉，本批**只做 (b)**：
   doc 写明调用约定（典型顺序先 mark_expired 再 evict_expired；
   evict 前调用方应持久化被删条目快照；当前零生产调用方）。
   **同文件 medium（:1）一并**：doc 写明 now_ms 时间源契约
   （必须与 created_at_ms/expires_at_ms 同源的 wall-clock Unix ms；
   跨重启持久化场景不可用单调时钟；NTP 回拨脆弱性知悉并接受——
   14 天 TTL 粒度下影响有限，调用方勿混时间源）。
3. **evolution/derive.rs:104**（high）：Contradiction 分支 summary 是固定串
   → 一轮 N 条矛盾产出 N 个**相同** proposal_id → 下游 dedup 坍缩成 1 条，
   静默丢 N-1 个 keep/drop 信号。修：summary 并入 keep/drop refs
   （`contradiction ruled between two memories (keep=..., drop=...)`）
   + id 追加 `short_hash(keep|drop_id)` 原始判别位——**normalize_for_hash
   把数字位归一成 'N'**，UUID hex 记忆 id 仅数字不同时 summary 归一化后
   仍坍缩（测试实测暴露），判别位须绕开归一化。确定性保持（同输入同 id）。
   ids 是记忆条目 id（哈希串），不属
   「summary 不泄原始路径/PII」契约的禁止项（该契约针对 content，
   既有测试只用 merge 路径，不受影响）。

## 人类可读摘要

- family: candidate-pool-integrity（溢出 / 不可逆 / 去重坍缩）
- 覆盖 findings: 3（+1 同文件 medium 一并 doc）
- 预估 diff: 4 files / +31/-6 lines → 实测 +52/-6（A 类；finding 2 的 doc
  段落本身是交付物，注释不可压，budget 执行中校正 +45/-12 → **+55/-12**）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 3

## 核实的编译层事实（spec 前已核）

1. `compute_expires_at` 生产调用方 = 0（仅 candidate/mod.rs re-export +
   自身测试）；真实生产路径是两处内联加法（candidate/derive.rs:49、
   panel/commands.rs:357）——只修 fn 不修调用点 = 假修，故收敛。
2. `evict_expired` 零生产调用方（C2 已核）；doc-only 改动无 ripple。
3. 固定串 "contradiction ruled" 全仓无下游依赖（grep 仅 derive.rs 自身）。
4. `proposal_id(category, target, summary)` 以 summary 为唯一可变输入
   （category/target 在 Contradiction 分支固定）——改 summary 即改 id。
5. panel/commands.rs:357 上下文是 toggle 续期（重置 TTL 再续 14 天），
   改为 `compute_expires_at(now_ms)` 语义等价（正常值域内）。
6. 既有 asserts：ttl 21 / evolution/derive 37 / candidate/derive 23 /
   panel/commands 19。
7. candidate/derive.rs:6 现 `use super::ttl::TTL_MS;`——加 import
   compute_expires_at（TTL_MS 仍被测试 :127/:142 用，保留）。

## 修法

- **candidate/ttl.rs**：`compute_expires_at` 改 `saturating_add(TTL_MS)` +
  doc 写明溢出语义（饱和=永不过期保轨迹）；模块头补 now_ms 时间源契约 +
  evict 调用约定（先 mark 后 evict / evict 前持久化快照）；新增测试
  `compute_expires_at_saturates_on_overflow`（3 asserts）。
- **evolution/derive.rs**：Contradiction summary 并入 keep/drop refs；
  新增测试 `contradiction_proposals_have_distinct_ids_per_pair`
  （2 条矛盾 → 2 proposals 且 id 相异）。
- **candidate/derive.rs**：import + :49 调用点收敛到 compute_expires_at。
- **panel/commands.rs**：:357 调用点收敛到 compute_expires_at。

## 红线

- 不改 evict_expired 谓词（C2 已修，勿重复）
- 不改 proposal_id 算法本身（DefaultHasher 稳定性是 3b-F 簇的事）
- 不改 Merge/Distill 分支 summary（各自的 id 已含 ids.len 区分度，
  不在本簇 finding 内）
- jsonl schema / 持久化字段不动；既有条目旧 id 保留（前向生效）

## spec 起草后自查三条

1. `expected_files` = 4：candidate/ttl.rs + evolution/derive.rs +
   candidate/derive.rs + panel/commands.rs。ripple = 后两文件各 1 行
   调用点收敛（已列）。
2. budget = **A 类**：ttl +16/-2、evolution/derive +12/-1、
   candidate/derive +2/-2、commands +1/-1：初估 +31/-6 → 实测 +52/-6
   （doc 交付物行数超初估），校正 +55/-12 → 测试暴露第二层缺陷
   （normalize_for_hash 数字归一化使 UUID 判别位坍缩，需追加原始 hash
   判别位 + 回归测试）实测 +59/-9，再校正 +65/-12 → OCR r1 采纳（medium：复合 id 破 16 hex 契约 → 改二次 hash 保持 16 hex；low：summary 不并入 refs 避免与 related_refs 重复 + 32 字符 UUID 回归断言 + ttl 测试固定 now_ms）实测 +76/-8，三校正 → **max +80/-12**。
3. 三条 findings 的 fix 字段均已写明 ripple（finding 1 的 ripple =
   两个内联调用点文件）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "EV-3b-D",
  "family": "candidate-pool-integrity",
  "expected_files": [
    "src-tauri/src/evolution/candidate/ttl.rs",
    "src-tauri/src/evolution/derive.rs",
    "src-tauri/src/evolution/candidate/derive.rs",
    "src-tauri/src/evolution/panel/commands.rs"
  ],
  "max_lines_added": 80,
  "max_lines_removed": 12,
  "findings": [
    {"id": "C5-EV-3b-D-1", "file": "src-tauri/src/evolution/candidate/ttl.rs", "line": 41, "fix": "compute_expires_at 改 saturating_add（溢出=永不过期保审计轨迹，非 wrap 成负数静默立刻淘汰）+ 回归测试；ripple: candidate/derive.rs:49 与 panel/commands.rs:357 两处内联 now_ms+TTL_MS 收敛到该 fn 单点实现"},
    {"id": "C5-EV-3b-D-2", "file": "src-tauri/src/evolution/candidate/ttl.rs", "line": 34, "fix": "doc-only：模块头写明 evict 调用约定（先 mark_expired 后 evict_expired；evict 前持久化快照；零生产调用方）+ now_ms 时间源契约（wall-clock Unix ms 同源；NTP 回拨风险知悉接受）；status 门已被 EV-3b-C2 修掉不重复"},
    {"id": "C5-EV-3b-D-3", "file": "src-tauri/src/evolution/derive.rs", "line": 104, "fix": "Contradiction 分支 summary 并入 keep/drop refs + id 追加 short_hash(keep|drop_id) 原始判别位（normalize_for_hash 数字归一化使 UUID 仅数字不同的对仍坍缩，判别位绕开归一化）使每条矛盾产相异 proposal_id；确定性保持；无 ripple（固定串无下游依赖）"}
  ],
  "assertions_min": {
    "src-tauri/src/evolution/candidate/ttl.rs": 21,
    "src-tauri/src/evolution/derive.rs": 37,
    "src-tauri/src/evolution/candidate/derive.rs": 23,
    "src-tauri/src/evolution/panel/commands.rs": 19
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
