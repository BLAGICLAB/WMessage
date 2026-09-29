# Batch Spec: EV-B2

## 目的

按 `docs/AUDIT-FIX-PLAN-2026-09-29.md` B2 批（自进化闭环语义，P1×4 + 拍板④）：

- **B2-1（P1-EV3）lesson 幂等修复 + 防记忆劫持**（三重闸）：
  ① consolidation 的 merge/contradiction **跳过带 `evo:` key tag 的行**（lesson
  被吸收 = key 消失 = 幂等失效，实证同提案 apply 3 次）；
  ② apply 查重补残留复查：key tag 在**任意** tag 位 + 同 kind=lesson 内容逐字相同；
  ③ `insert_item` merge-on-write（cosine≥0.92）对**异 key** 的 evo lesson 拒写
  （`InsertOutcome::RefusedForeignMerge`）→ apply 层 `evolution.apply_conflict`
  audit + `ApplyReport.conflicts_refused` 计数（实证行 cb60ad9b 被 lesson 劫持）。
- **B2-2（P1-EV4）changes.jsonl 单写者锁**：shadow 生产循环与 apply 补写的
  append 全部纳入 `EVOLUTION_STORE_LOCK`（阻塞闭包内取锁不跨 await；锁序：
  apply 先释放 DB_WRITE_LOCK 再取本锁，与 panel 本锁内开 DB 连接无环）。
  ocr r2 采纳：shadow 的**去重判定与唯一 id 派生一并挪进锁内窗口**（读/判/写
  同窗，无 TOCTOU、读不受半行写干扰）。
- **B2-3（P1-EV4）shadow 去重**：同 proposal_id 已有
  未终态（Pending/Shadowing/ShadowPassed，含 panel toggle ON 写的）→ 跳过
  append（info audit + `ShadowReport.deduped` 真实计数），不再无条件重复 Pending 行。
- **B2-4（P1-EV5）二次回滚卡死**：`change::derive::unique_change_id_for` 行级唯一
  change_id（`chg-<pid>` 占用则 `-2`/`-3` 递增）——**三家写者统一**
  （panel toggle / shadow / apply CR 落行前锁内派生，ocr r2 HIGH② 采纳）；
  复用 toggle 挂 parent_id 血缘指向前一条 CR；前端 EvolutionPanel onToggle 对有
  RolledBack 行的提案再点 ON 弹二次确认「上次已回滚，确认再次启用？」（拍板①）。
- **B2-5（P1-EV6）生产 shadow 失败率告警**：生产入口循环结束后按全局原子计数器
  补 `evolution.shadow_warning`（>5%），对齐 trait 版 sink.audit_warning。
- **拍板④ delete 口径**：注释与行为对齐（proposals 行无论 status 一律删、
  changes 仅级联删 pending），后端 command doc + 前端注释两处。

## 红线

- 用户记忆路径行为不变：防劫持闸只拦 `evo:` key 的 lesson；同 key 重放仍走 merge。
- toggle ON 首次启用行为不变（基础 change_id 空闲即用）；dedup 未终态命中仍复用。
- EVOLUTION_STORE_LOCK 临界区纪律不变：panel RMW 全窗持锁；新入锁的两处只在
  append 单点持锁（append 内部不再加锁，mod.rs:77 声明成立）。

## 测试

- 新增 10 个回归：拒写防劫持 ×2（store 层拒写+原行保真 / 同 key 重放放行）、
  merge 排除 lesson ×2（混合引用存活 / 全 lesson 跳过）、contradiction drop 保护 ×1、
  apply 残留查重 ×2（key 任意位 / 同内容）、apply 冲突拒写 ×1（target 保真）、
  next_unique_change_id ×1、changes.jsonl 4 线程×25 并发 append 无丢行无重 id ×1。
- 实测：evolution:: 283 / memory 56 连跑 5 轮全绿（防并行 flaky 验收项）；
  tsc --noEmit / EvolutionPanel vitest 13 过。

## spec 起草后自查三条

1. expected_files 11：九个源文件（含两个测试文件、memory/mod.rs 防御臂）+
   DEVLOG + 本 spec。
2. budget：修改 +490/-15（tests 大头 +71/+60）；新文件仅本 spec。
3. assertions_min 按 gate 正则填实测-1（staged 全文计数）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "EV-B2",
  "family": "evolution-loop-semantics",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/EV-B2.spec.md",
    "src-tauri/src/evolution/apply.rs",
    "src-tauri/src/evolution/change/derive.rs",
    "src-tauri/src/evolution/change/mod.rs",
    "src-tauri/src/evolution/observe/shadow.rs",
    "src-tauri/src/evolution/panel/commands.rs",
    "src-tauri/src/memory/consolidate.rs",
    "src-tauri/src/memory/consolidate/tests.rs",
    "src-tauri/src/memory/mod.rs",
    "src-tauri/src/memory/store.rs",
    "src-tauri/src/memory/tests.rs",
    "src/components/EvolutionPanel/EvolutionPanel.test.tsx",
    "src/components/EvolutionPanel/EvolutionPanel.tsx"
  ],
  "max_lines_added": 720,
  "max_lines_removed": 30,
  "findings": [
    {"id": "B2-1", "file": "src-tauri/src/memory/store.rs", "line": 213, "fix": "InsertOutcome::RefusedForeignMerge：异 key evo lesson merge-on-write 拒写（防记忆劫持）"},
    {"id": "B2-1", "file": "src-tauri/src/memory/consolidate.rs", "line": 266, "fix": "merge/contradiction 跳过 evo: 行（keep/drop 双侧对称保护）"},
    {"id": "B2-1", "file": "src-tauri/src/evolution/apply.rs", "line": 55, "fix": "残留查重（key 任意位+同内容）+ ConflictRefused → evolution.apply_conflict audit"},
    {"id": "B2-2", "file": "src-tauri/src/evolution/observe/shadow.rs", "line": 380, "fix": "读/判/写同窗走 EVOLUTION_STORE_LOCK；apply 补写同锁"},
    {"id": "B2-3", "file": "src-tauri/src/evolution/observe/shadow.rs", "line": 415, "fix": "锁内去重判定（同 proposal_id 未终态跳过）+ deduped 真实计数"},
    {"id": "B2-4", "file": "src-tauri/src/evolution/change/derive.rs", "line": 89, "fix": "unique_change_id_for：三家写者统一行级唯一 id（评审 HIGH② 采纳）+ panel parent_id 血缘"},
    {"id": "B2-5", "file": "src-tauri/src/evolution/observe/shadow.rs", "line": 470, "fix": "生产循环收尾 evolution.shadow_warning（>5% 对齐 trait 版）"}
  ],
  "assertions_min": {
    "src-tauri/src/memory/consolidate/tests.rs": 60,
    "src-tauri/src/memory/tests.rs": 80,
    "src-tauri/src/evolution/apply.rs": 38,
    "src-tauri/src/evolution/observe/shadow.rs": 80,
    "src-tauri/src/evolution/panel/commands.rs": 25,
    "src-tauri/src/evolution/change/derive.rs": 25
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 900,
    "expected_max_comments": 45
  },
  "stop_conditions": ["compile_failure", "architecture_blocker", "family_heterogeneity", "new_high_different_root", "gate_fail"]
}
```

## 验证命令

```
cargo test evolution:: && cargo test memory && npx tsc --noEmit && npx vitest run src/components/EvolutionPanel
# 验收项：cargo test evolution:: 连跑 5 轮全绿
```
