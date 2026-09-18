# 观察态交付 — 状态说明

> **工程完成，等使用。**
> 日期：2026-09-18 · 状态：S0_observe（默认）

## 1. 已实现

| 阶段 | 内容 | 测数 |
|------|------|------|
| A（R7→A） | 删 5 维 engine → 落 `is_reversible`；保留 shadow hook | 950 |
| B 骨架 | 3 态状态机 + 4 态 audit + `evaluate_s2` 占位 + synthetic 隔离 | +14 |
| B 校准前置 | §12.7 状态持久化（`save_state`）+ S2 真调 evaluate（A 方案） | +5 |

**全量回归**：969 / 969 通过

**关键路径**：
- `src-tauri/src/evolution/activation.rs` — 状态机 / 配置 / 评估 / 持久化
- `src-tauri/src/evolution/observe/shadow.rs` — wrapper / 4 态 audit / state 路由
- `src-tauri/src/evolution/proposal.rs` — `is_reversible`
- `src-tauri/bot-config.json` — `shadow.enabled: true` + `activation.mode: calibrating` + `activation_state: s0_observe`
- `evolution.synthetic/` — synthetic 数据隔离（默认不读）

## 2. 有意未做 + 原因

| 项 | 状态 | 原因 |
|---|------|------|
| B 校准（真数据 → 阈值 → 候选生成） | 🔜 等真数据 | 当前真数据 = 0，触发条件无意义 |
| Expire 字段（policy 到期） | 🔜 P1 | 无 policy 可过期 |
| 漂移检测 | 🔜 P1 | 无分布可检 |
| false_block 反馈 | 🔜 P1 | 无 S2 active 可误 block |
| 状态切态 Tauri 命令 | 🔜 等 §6 候选生成器 | 切态无东西可确认 |
| S2 写主记忆 | 🔜 等真 policy | 无规则时写 = 无规则全写（spec §17 禁止） |

**不是遗漏，是设计。**

## 3. 恢复条件

```
真数据 > 0
  → observe-run 看分布
    → 识别重复模式
      → 填阈值（min_occurrences / min_proposals）
        → 候选生成（§6）
          → 用户确认 → 进 S2
```

每步是硬约束：缺任何一环，下一步不启动。

## ⚠️ 隐含假设 + 出路

**隐含假设**：dev 真实使用 agent → 产生 proposal → 落 evolution-changes.jsonl → 数据累积。

**若使用不增长，B 校准永远启动不了** —— 这是无限期等待，不是技术故障。

**三条出路**（产品决策，非工程决策）：

1. 推 dev 真实使用 → 不可控（dev = 用户自己）
2. 接受长期 S0，交付观察态 → **当前选择**
3. 目标用户是内部团队 → 手填 policy，跳过校准 → 偏离 §2 已答结论

## 诊断（22:13 四环定位）

| 环 | 状态 | 证据 |
|---|------|------|
| 1. dev 是否在用 | ❌ 未用 | wmessage 目录最新 Sep 12；DB 无 `bot_sessions` |
| 2. proposal 生成 | ⚠️ 未触发 | 无上游流量 |
| 3. gate 通过 | ⚠️ 未测 | 无 proposal |
| 4. 落盘路径 | ✅ 正确 | synthetic 隔离未误伤；写入点 `evolution-changes.jsonl` ✓ |

**断点：环 1，非 pipeline 故障。**

## 4. S2 evaluate 显式分层（A 方案，老板 22:10 拍板）

```
gate → is_reversible（防御纵深） → state 路由：
  S0/S1 → 不调 evaluate，写 shadow + audit no_policy_applied
  S2   → evaluate_s2 (占位 Allow) → 写 shadow + audit allowed
        Block 不可达（等真 policy 才可能触发）
```

- `is_reversible`：各态都跑（防御纵深）
- `evaluate_s2`：仅 S2 调用；占位阶段永远返 `Allow`，`Block` 枚举值保留但不可达
- 防止「无 policy 隐式等于 Allow 全写」（spec §17）

## 5. 4 态 audit 语义

| 态 | 时机 | 实/占 |
|---|------|------|
| `no_policy_applied` | S0/S1 写 shadow | ✅ 实 |
| `Allow` | S2 + 可逆 + evaluate_s2 Allow | ✅ 实（`policy_version: calibrating`） |
| `Block` | S2 + 不可逆 / 高风险 | ⚠️ 不可达（占位阶段） |
| `Expire` | policy 到期回 S0 | ⚠️ 占位（P1） |

---

**这是「观察态交付」**，不是「还没做完」，不是「已经做完」。

有意停在 S0，等外部条件（dev 真使用）成熟。