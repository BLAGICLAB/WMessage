# Batch Spec: MI-B2

## 目的

落地用户拍板的 B 类决策 2 项（migration 域，2026-09-25 拍板）：#2 迁移取消机制（A：CancellationToken 语义贯穿——static AtomicBool + 关键步骤间检查点 + 取消 command 供未来 UI 接线）、#8 孤儿 pending 启动一次性清理（A：同 key 多 pending 留最新删旧 + 留痕）。对应 PHASE2-TRIAGE §4 B 类权威清单 #2/#8。

## 人类可读摘要

- family: migration-cancel-and-journal
- 覆盖: B 类 2 项（2 实修）
- 预估 diff: 7 文件 / +115/-5（无新文件）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 4

## 逐条处置（原文均已核 + 用户拍板 2026-09-25）

**实修 ① src-tauri/src/migration/run.rs（C5-MI-05.1，用户拍 #2=A）**：migration_run 的 spawn_blocking JoinHandle 无 abort/cancel 接线，取消语义无法表达（OCR high）。
**修法**（tokio CancellationToken 不入仓——static AtomicBool 等价且零依赖）：
- run.rs 新增 `static MIGRATION_CANCEL: AtomicBool`；`pub fn migration_request_cancel()`（置位）；`pub(crate) fn migration_cancel_requested()`（读）。
- `run_migration` 入口清零（MigrationGuard 保证串行，清零安全：无 run 时的置位请求随新 run 启动丢弃）。
- 检查点：阶段一 due 处理循环开头、阶段二主循环开头——命中则 log_line「迁移被取消，安全停止（已完成操作不回滚，未开始的不再开始）」+ `report.cancelled = true` + 提前 return Ok(report)（部分计数如实）。
- types.rs MigrationReport 加 `#[serde(default)] pub cancelled: bool`（前端 types.ts 同步可选字段，向后兼容——旧 payload 无此字段时 serde default 兜底是反向场景：Rust 侧序列化恒有该字段，前端可选标记）。
- commands.rs 新增 `#[tauri::command] pub fn migration_cancel() -> bool`（置位并返回 true）注册 lib.rs——取消 UI 由未来迭代接线（拍板 A 明示「表达不再需要结果」的机制本体，触发源命令面已备）。
**ripple**：lib.rs invoke_handler +1 行；前端 types.ts MigrationReport +cancelled?: boolean（纯类型，无 UI 改动）。

**实修 ② src-tauri/src/migration/recovery.rs:68（MI-04a 残余，用户拍 #8=A）**：既有库中 MI-04a 修复前产生的同 key 多条 pending（id DESC 下不可见但仍被 replay 扫描），重放会对同 key 重复处置。
**修法**：journal_replay_pending 开头加一次性清理——同 (task_id, src) 多条 pending 仅留 id 最大（最新尝试），其余 DELETE；清理数 >0 时 log_line 留痕。抽纯函数 `purge_orphan_pending(conn) -> Result<usize, String>`（不需 AppHandle，直测）。含 committed 的 pending 不删（可能是新一轮中断尝试，replay 本身能处置）。
**ripple**：无（replay 内部前置步骤）。

## 测试

1. recovery.rs：purge_orphan_pending——同 key 3 条 pending（id 1/2/3）→ 剩 id=3、返 2；他 key 单条 pending 不动；committed 混合场景不受影响
2. run.rs：migration_cancel_requested 开关（默认 false → request 后 true）
3. types 变更由既有 serde 序列化测试/编译担保（cancelled 字段 serde(default)）

## 红线

- family 一致性：只含 migration 域 2 项 B 类拍板落地
- 取消粒度 = 阶段循环边界（安全停止在事务/staging 边界，不中断单文件操作中间态）
- 不动 MigrationGuard 状态机 / spawn_polling 结构
- 前端零行为改动（types.ts 纯类型 + MigrationPanel 不接线取消按钮）
- 新注释不引用审计批次号

## spec 起草后自查三条

1. expected_files：migration/run.rs + migration/types.rs + migration/commands.rs + migration/recovery.rs + src-tauri/src/lib.rs + src/types.ts = 6（全路径已列）
2. budget：A 类（static+fn+检查点 ×2 + command + 注册 + types 字段 ≈ +48/-3）+ B 类（recovery 清理 ≈ +22；测试 ≈ +45）合计 ≈ +115/-5，上限 +180/-20；无新文件
3. fix 字段 ripple：① lib.rs 注册 + types.rs + src/types.ts 已入 expected_files；② replay 内部闭环

## 自主执行规则

spec 经用户拍板（B 类 2 项方向 2026-09-25）后：
- agent 全权执行至 commit，不中途报 status / exit / diff
- 例外只有 stop 条件触发
- 完成时一次报：hash + 批定性 + OCR 轮次 + comments 处置 + push 两值
- 过程证据落 ~/.openclaw/cache/MI-B2/，不喊人

## Stop 条件（触发即停，报用户）

- compile_failure（无 X 形态退路）
- architecture_blocker（签名/调用点超预算）
- family_heterogeneity
- new_high_different_root（OCR r1 出的 high 非本批根因）
- gate_fail（batch-verify FAIL 且非 spec 声明调整可解）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "MI-B2",
  "family": "migration-cancel-and-journal",
  "expected_files": [
    "src-tauri/src/migration/run.rs",
    "src-tauri/src/migration/types.rs",
    "src-tauri/src/migration/commands.rs",
    "src-tauri/src/migration/recovery.rs",
    "src-tauri/src/lib.rs",
    "src/types.ts"
  ],
  "max_lines_added": 180,
  "max_lines_removed": 20,
  "findings": [
    {"id": "C5-MI-05.1", "file": "src-tauri/src/migration/run.rs", "line": 96, "fix": "static MIGRATION_CANCEL + request/cancelled fn + run 入口清零 + 阶段循环检查点（命中 log_line + report.cancelled=true + 提前 return）+ MigrationReport #[serde(default)] cancelled + migration_cancel command 注册 lib.rs；前端 types.ts 加可选字段。ripple：lib.rs + types.rs + src/types.ts"},
    {"id": "C5-MI-04a.2", "file": "src-tauri/src/migration/recovery.rs", "line": 68, "fix": "journal_replay_pending 前置 purge_orphan_pending 纯函数：同 (task_id,src) 多 pending 留 id 最大删其余 + log 留痕；单测覆盖。ripple：无"}
  ],
  "assertions_min": {
    "src-tauri/src/migration/recovery.rs": 4,
    "src-tauri/src/migration/run.rs": 2
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 4
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

## 提交信息骨架

```
fix(migration): MI-B2 — migration-cancel-and-journal（B 类拍板落地 2 项）

【family】迁移取消机制 / journal 孤儿清理（用户拍板 2026-09-25：#2=A、#8=A）
【实修 2 处】
- run.rs 取消机制：static MIGRATION_CANCEL + 两阶段检查点安全停止（已完成不回滚）+ report.cancelled + migration_cancel command（取消 UI 由未来迭代接线）
- recovery.rs replay 前置孤儿清理：同 key 多 pending 留最新删其余 + 留痕（MI-04a 修复前历史遗留）
【行为变更】migration_cancel command 就绪；report 增 cancelled 字段（serde default 兼容）；重启重放不再对同 key 重复处置
【测试】+3（purge 三场景 / cancel 开关）
【OCR】r1：<N> comments <处置>
【基线】D2: files=6(+A/-R) asserts=… tests=…
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/MI-B2.spec.md python3 scripts/batch-verify.py docs/batches/MI-B2.spec.md
```
