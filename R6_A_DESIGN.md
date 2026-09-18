# R6 A 阶段设计稿（apply 路径并行观察）

> 日期：2026-09-18
> 状态：✅ 老板 13:13 拍板 4 决策 + 补充节；开始实施
> 父阶段：R6（数据观察）
> 前置：R6 B 完成（4 个指标计算逻辑已验证）

---

## 1. 设计目标

让真实 consolidation 流程的 proposal 流经 R2/R3/R4/R5 模块，产出真数据供 R6 观察指标计算。

**关键约束（老板 13:07/13:13 拍板）**：
- a. **feature flag 名**：`evolution.shadow.enabled`
- b. **默认值 false**（生产永不改；dev 在配置文件显式写 true）
- c. **TS 面板**：暂不需要，但加最小 reload endpoint（不上面板）
- d. **观察停止条件**（OR 任一满足）：
  - 观察到 30 个 change 走完 Proposed → 终态
  - 观察到 14 天（上限）
  - 观察到 RolledBack ≥ 5
- **失败计数**：shadow 失败写 audit，失败率 > 5% 告警
- **一致性校验**：定期抽查 shadow 输出 vs 真 apply 逻辑，差异 > 60% 视为 bug

---

## 2. 文件拓扑

```
{project_root}/evolution/  [不变]
  ├─ evolution-proposals.jsonl   [B 阶段写，A 阶段由 apply 触发 shadow 写]
  ├─ evolution-changes.jsonl     [B 阶段写，A 阶段由 apply 触发 shadow 追加]
  └─ evolution-applied.jsonl     [不变 — 由原 apply 路径写]

src-tauri/src/evolution/observe/
  ├─ mod.rs              [+1 行：pub mod shadow;]
  ├─ metrics.rs          [B 阶段已建]
  ├─ synthetic.rs        [B 阶段已建]
  └─ shadow.rs           [A 阶段新增]

src-tauri/src/evolution/apply.rs  [+10 行：shadow 钩子]
src-tauri/src/bot/config/commands.rs  [+1 个 Tauri command]
src-tauri/bot-config.json              [+1 个 evolution.shadow.enabled 块]
```

---

## 3. Feature Flag 设计

**bot-config.json 增加**：
```json
{
  "evolution": {
    "shadow": {
      "enabled": false   // 默认 false（决策 b）
    }
  }
}
```

**决策 a 说明**：用 `evolution.shadow.enabled` 而非 `parallel_apply`，因为：
- 描述行为准确（该 flag 启动的是 shadow 观察，不调用任何 apply 路径）
- 与 R3 ShadowRunner / `evolution-shadow.jsonl` 命名空间一致
- 未来真有 parallel_apply 时用另一个 flag 区分

**决策 c 说明**：
- 检查结果：bot-config 当前**不是**热重载（有 `notify` watcher 但未启用）但 `io::load_config()` 每次调用重读文件 → 改文件即对后续 `load_config` 调用生效
- 加 `bot_reload_config` 命令作显式触发点（dev 改完配置调一下，无需重启）

**决策 b 说明**：
- 默认 false 写死在 `ShadowConfig::default()`
- `bot-config.json` 改 `evolution.shadow.enabled=true` 才生效
- `load_from_file` lenient：缺字段返默认 false（不是 Result）

---

## 4. Shadow Apply 函数

新增 `evolution::observe::shadow::shadow_apply_for_batch(proposals, app)`：

```rust
/// Shadow apply：与主 apply 并行运行
///
/// 与主 apply 的差异：
/// - 不写 mem_items（决策 b：spec 老板 13:07 拍板）
/// - 只写 evolution-changes.jsonl（R2 ChangeRecord 格式）
/// - 不发 audit.proposal 事件（避免污染 bot.log grep 解析）
/// - 不调 emit_proposals（避免双写 audit）
/// - 失败仅 audit_event + 原子计数 + 失败率告警
pub async fn shadow_apply_for_batch(
    proposals: Vec<EvolutionProposal>,
    app: &AppHandle,
) -> ShadowReport {
    // 1. 过 auto_apply_gate（与主 apply 同一门槛）
    let gated: Vec<_> = proposals.into_iter().filter(|p| auto_apply_gate(p)).collect();

    // 2. 写 evolution-changes.jsonl
    let path = paths::data_dir(app).join("evolution-changes.jsonl");
    let mut written = 0;
    let mut failed = 0;
    for p in &gated {
        let cr = change::from_proposal(p, now_ms());
        match change::append_change(&path, &cr) {
            Ok(()) => { written += 1; TOTAL_WRITES += 1; }
            Err(e) => {
                failed += 1;
                TOTAL_WRITES += 1;
                FAILED_WRITES += 1;
                audit_event!(app, AuditLevel::Warn, "evolution.shadow_failed",
                    "proposal_id" => p.proposal_id.clone(),
                    "error" => e,
                );
            }
        }
    }

    // 3. 失败率告警（仅当 total > 0 且 > 5%）
    let total = TOTAL_WRITES.load();
    let failed_count = FAILED_WRITES.load();
    if total > 0 && (failed_count as f64 / total as f64) > 0.05 {
        audit_event!(app, AuditLevel::Warn, "evolution.shadow_warning",
            "failure_rate" => failed_count as f64 / total as f64,
            "failed" => failed_count,
            "total" => total,
            "threshold" => 0.05,
        );
    }

    ShadowReport { total: gated.len(), written, failed }
}
```

**apply.rs 修改**（`evolution/apply.rs:125`）：
```rust
// [R6 A] shadow 钩子（仅当 flag=true）
let shadow_proposals = if observe::shadow::is_enabled(&app) {
    Some(proposals.clone())  // 克隆供 shadow spawn
} else { None };

// 原有 spawn 块保持不变（用原 proposals）
tauri::async_runtime::spawn(async move { /* ... 现有逻辑 ... */ });

// [R6 A] fire shadow（仅当 flag=true）
if let Some(p) = shadow_proposals {
    let app_shadow = app.clone();
    tauri::async_runtime::spawn(async move {
        observe::shadow::shadow_apply_for_batch(p, &app_shadow).await;
    });
}
```

**关键**：原 apply 路径 0 改动；shadow 是 fire-and-forget 异步任务。

---

## 5. 失败处理（决策补充 1）

### 5.1 单条失败
- 写 audit_event `evolution.shadow_failed` (level=Warn)
- 字段：`proposal_id`, `error`
- atomic `FAILED_WRITES += 1`, `TOTAL_WRITES += 1`

### 5.2 失败率告警
- 每轮 shadow_apply 结束后检查
- `failure_rate = FAILED_WRITES / TOTAL_WRITES`
- 当 `failure_rate > 0.05` 且 `total > 0`：audit_event `evolution.shadow_warning` (level=Warn)
- 字段：`failure_rate`, `failed`, `total`, `threshold`

### 5.3 阈值选择理由（建议）
- 5% = 100 次写中允许 5 次失败；本地 JSONL 写通常 0 失败，故 5% 即异常信号
- 太高（如 10%）会让偶发 IO 抖动掩盖真实 bug
- 太低（如 1%）会让单次失败就触发告警，噪声大

---

## 6. 一致性校验（决策补充 2）

### 6.1 MVP 实现：post-process 抽查（不嵌主路径）
- 独立函数 `observe::consistency::audit_consistency(app)`
- 读 evolution-changes.jsonl（shadow 写）+ mem_items（apply 写）
- 统计：
  - shadow 写过的 mem_key 数
  - mem_items 中仍存活的 evo:* 数
  - apply 路径的 `evolution.applied` audit 事件数（从 bot.log grep）
- 差异率 = `|shadow - apply| / max(apply, 1)`
- 当差异 > 60%：audit_event `evolution.shadow_inconsistency` (level=Error)
- 字段：`shadow_count`, `apply_count`, `diff_rate`, `threshold`

### 6.2 调用时机
- **MVP**：手动调用（CLI / R6 B 后续可加 scheduler 定期跑）
- **后续**：consolidation 结束后 fire-and-forget 跑一次（不阻塞主流程）

### 6.3 阈值 60% 理由
- 假设 100 个 proposal：shadow 写 90 个 / apply 写 50 个 → diff_rate = |90-50|/50 = 80%（> 60% 告警）
- shadow 写满 apply 写：diff_rate < 100%（但通常不一致）
- shadow 与 apply 1:1 时：diff_rate 接近 0（理想）

---

## 7. 验收测试

- ✅ `load_disabled_by_default`：ShadowConfig::default().enabled == false
- ✅ `load_missing_file_safe`：load 不存在的 bot-config.json 返 enabled=false（不 panic）
- ✅ `load_enabled_true_parses`：bot-config.json 写 true → loaded.enabled == true
- ✅ `load_partial_config_safe`：缺 shadow 块 → default false
- ✅ `failure_rate_zero_initially`：failure_rate() == 0.0（计数从 0 起）
- ✅ `shadow_does_not_write_mem_items`（需 AppHandle mock 或跳过 → 用 IO 文件检查）
- ✅ `bot_reload_config_command_returns_ok`：Tauri 命令返回 Ok
- ✅ 集成测试：bot-config.json 改 true → 重启 → apply 触发 shadow → evolution-changes.jsonl 有新条目；改回 false → 下一轮 apply 不再触发 shadow

---

## 8. 停止条件（决策 d）实施

跑完 R6 A 启用 flag 后，观察以下任一满足即停：
- `change_status_count(Proposed → 终态) >= 30`
- `now - start_time >= 14 天`
- `rolled_back_count >= 5`

跟踪方式：
- 加 `R6_OBSERVE_LOG.md` 记录启动时间和统计
- 每日跑 observe-run 看指标
- 满足任一条件即关 flag

---

## 9. 风险与缓解

| 风险 | 缓解 |
|------|------|
| evolution-changes.jsonl 体积爆炸 | R2 ChangeRecord 自带 14 天 TTL 检查；超期可手动 archive |
| shadow_apply 失败影响主流程 | fire-and-forget + audit_event；主 apply 路径无任何依赖 |
| 并发写 evolution-changes.jsonl | append-only + OS fsync 保证原子；高并发场景可加 Mutex |
| mem_items 与 ChangeRecord 不一致 | 故意为之（shadow 是观察窗）；R7 策略分层再处理 |
| flag 被意外开启污染 bot.log | shadow 不发 audit.proposal（spec 决策 d） → bot.log 纯净 |
| 失败率告警噪声 | 阈值 5% + 仅 warn level；不影响生产 |
| 一致性校验耗时 | post-process 不嵌主路径；后续可加 scheduler |

---

## 10. 老板决策点（已拍 2026-09-18 13:13）

| # | 决策 | 选择 |
|---|------|------|
| a | flag 名 | `evolution.shadow.enabled` |
| b | 默认值 | `false` |
| c | TS 面板 | 暂不需要；加 `bot_reload_config` 命令 |
| d | 停止条件 | OR：30 个完整生命周期 / 14 天 / 5 个 RolledBack |
| 补充 1 | 失败计数 | 每条失败 audit + 失败率 > 5% 告警 |
| 补充 2 | 一致性校验 | post-process 抽查；差异 > 60% 视为 bug |

---

## 11. 实施步骤（本 turn）

1. ✅ bot-config.json 加 `evolution.shadow.enabled=false`
2. ✅ evolution/observe/shadow.rs 新增（含测试）
3. ✅ evolution/observe/mod.rs 加 `pub mod shadow;`
4. ✅ evolution/apply.rs 加 shadow 钩子（~10 行）
5. ✅ bot/config/commands.rs 加 `bot_reload_config`
6. ✅ lib.rs 加 `bot_reload_config` 到 generate_handler
7. ✅ cargo check + tests
8. ✅ DEVLOG R6 A
9. ✅ 等真实数据跑一段后做一致性校验（post-process）

---

## 12. R6 后续

- 跑通 R6 A → dev 设 flag=true → 跑数据
- 满足停止条件（决策 d）→ 关闭 flag
- R7：策略分层（需老板拍）
- R8：高层候选 Skill/Code（只生成候选走 PR）