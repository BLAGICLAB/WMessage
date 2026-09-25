# Batch Spec: MI-B3

## 目的

落地 C5-MI-05b（跨域阻塞族残余，用户拍板 A=async 化）：migration/commands.rs 两条 sync command 主线程文件读 → async + spawn_blocking（migration_log_read 既有模式）。簇内第三条 recovery.rs:197 经独立 reviewer（agent-22）核验为 **stale**（std::thread 上 block_on = B3 设计内唯一合法桥接，调用链 lib.rs:305→run.rs:462 std::thread::spawn→:484→recovery.rs:110 双证，与 run.rs:62 B3 先例逐字同款）——零代码登记。同根 commands.rs:170 medium（migration_status 轮询 fs 读）并入本批。

## 人类可读摘要

- family: migration-async-commands
- 覆盖: C5-MI-05b 2 条（2 处 async 化实修；recovery.rs 条 stale 零代码）+ 同根 medium 并入 + 同文件陈旧 docstring 计数修正
- 预估 diff: 1 文件 / +22/-8（无新文件）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 3

## 逐条处置（原文均已核 + 用户拍板 2026-09-25 MI-05b=A）

**实修 ① src-tauri/src/migration/commands.rs:20（OCR high：sync command 主线程 fs 读冻结窗口）**：migration_rules_load `pub fn` 直调 load_rules（fs::read_to_string）。
**修法**：改 `pub async fn` + `tauri::async_runtime::spawn_blocking(move || load_rules(&app))`（migration_log_read 同款模式），join 失败 map_err CommandError::Internal。前端 invoke 返回 Promise 形态不变，零前端改动。
**ripple**：无（Tauri command 边界内闭环）。

**实修 ② src-tauri/src/migration/commands.rs:243（OCR medium 同根：migration_status 轮询每次 fs 读）**：同款 async + spawn_blocking 化。
**ripple**：无。

**同文件陈旧注释修正**：commands.rs:3 模块 docstring「共 5 个 #[tauri::command]」实际现声明 7 个（含 MI-B2 新增 migration_cancel）——计数与清单对齐。

**零代码 ③ recovery.rs:197（stale，reviewer agent-22 pass）**：block_on 在 spawn_polling std::thread 上为 B3 设计内合法桥接；async 化需重构 spawn_polling 顶层（扩 scope 零收益）。登记于 triage §0/§2。

## 测试

- 无新测试：纯命令包装层重排（行为与返回类型不变），既有 commands.rs tests（migration_log_read tail 等）+ test-all 全量担保

## 红线

- family 一致性：只含 MI-05b async 化
- 不动 load_rules 内核（MI-B1 已定）与 recovery.rs 任何行
- 前端零改动
- 新注释不引用审计批次号

## spec 起草后自查三条

1. expected_files：src-tauri/src/migration/commands.rs = 1（全路径已列）
2. budget：A 类（两函数包装 + docstring ≈ +22/-8），上限 +45/-15；无新文件
3. fix 字段 ripple：两命令均为 Tauri 边界内闭环；recovery.rs 零代码不入 findings（prose + triage 登记）

## 自主执行规则

spec 经用户拍板（MI-05b=A，2026-09-25）后：
- agent 全权执行至 commit，不中途报 status / exit / diff
- 例外只有 stop 条件触发（含：async 化触发意外 ripple → 停手报）
- 完成时一次报：hash + 批定性 + OCR 轮次 + comments 处置 + push 两值
- 过程证据落 ~/.openclaw/cache/MI-B3/，不喊人

## Stop 条件（触发即停，报用户）

- compile_failure（无 X 形态退路）
- architecture_blocker（签名/调用点超预算）
- family_heterogeneity
- new_high_different_root（OCR r1 出的 high 非本批根因）
- gate_fail（batch-verify FAIL 且非 spec 声明调整可解）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "MI-B3",
  "family": "migration-async-commands",
  "expected_files": [
    "src-tauri/src/migration/commands.rs"
  ],
  "max_lines_added": 45,
  "max_lines_removed": 15,
  "findings": [
    {"id": "C5-MI-05b.1", "file": "src-tauri/src/migration/commands.rs", "line": 20, "fix": "migration_rules_load sync → async + spawn_blocking（log_read 模式）；同根 :170 medium（migration_status）并入同款改造；同文件 docstring 计数对齐。ripple：无"},
    {"id": "C5-MI-05b.2", "file": "src-tauri/src/migration/commands.rs", "line": 243, "fix": "migration_status sync → async + spawn_blocking（同上模式）。ripple：无"}
  ],
  "assertions_min": {
    "src-tauri/src/migration/commands.rs": 3
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

## 提交信息骨架

```
fix(migration): MI-B3 — migration-async-commands（MI-05b=A 拍板落地）

【family】sync command 主线程 fs 读 → async + spawn_blocking
【实修 2 处】
- commands.rs:20 migration_rules_load async 化（log_read 模式）
- commands.rs:243 migration_status async 化（轮询 fs 读同根）
【行为变更】无（返回类型/前端契约不变；主线程不再做文件 IO）
【stale 登记】recovery.rs:197 block_on = std::thread 上 B3 设计内合法桥接（reviewer agent-22 pass，零代码）
【自测】cargo fmt/check + tsc + test-all 全绿
【OCR】r1：<N> comments <处置>
【基线】D2: files=1(+A/-R) asserts=… tests=…
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/MI-B3.spec.md python3 scripts/batch-verify.py docs/batches/MI-B3.spec.md
```
