# Batch Spec: EV-3b-G

## 目的

修 C5-EV-3b-G 3 条（持锁 / async 阻塞 IO），3 处独立改动：

1. **apply.rs:156**（high）：`DB_WRITE_LOCK` 一锁覆盖整批——open_db
   （文件打开 syscall）、JSONL append、audit emit 全在临界区内，
   无关 DB 写者被整条流水线串行化。**核实现状**：整批已在
   `spawn_blocking` 内（:150），不占 tokio worker——剩下的真问题是
   **锁范围**。修：open_db / ledger 路径 / now 预计算移出锁；
   每条 proposal 的临界区收窄到仅 `apply_one`（SQL 写）；
   `append_applied_record` + `audit_event!` 在锁外（逐条：锁内
   apply_one → 放锁 → append → audit，错误传播语义 `?` 逐条中止
   **保持不变**）。
2. **observe/shadow.rs:202**（high）：三个 `shadow_apply_*` async 入口
   在循环内同步调阻塞 `append_change`（open+writeln syscall），
   钉住 tokio worker。**核实**：trait 两变体（`shadow_apply_for_batch` /
   `_with_reversibility`）**零生产调用方**（仅测试 + MockShadowSink
   内存 sink，无真实 IO；AppShadowSink 也零调用方）——生产真问题只在
   `with_app`（apply.rs:227 唯一生产入口）。修：with_app 的
   append_change 调用包 `tauri::async_runtime::spawn_blocking`
   （path/cr clone 进闭包；JoinError 映射为写失败 Err 走既有
   failed/audit 路径）；trait 两变体加 doc 注明「测试/内存 sink 专用，
   接生产 sink 前须 spawn_blocking 化」（finding 给的 document 选项）。
3. **panel/commands.rs:0**（high）：8 个 `pub async fn` command 全在
   async runtime 上做同步 fs/SQLite IO（read_to_string / rewrite /
   open_db / delete_by_*），contended 时 stall worker 连
   ask_user_confirm 往返都受拖累。修：阻塞段包
   `crate::py::document::spawn_blocking_map`（仓内既有 helper，
   JoinError→String 映射一致）——
   - 全同步 5 个（toggle / delete / list_proposals / keep_shadow /
     list_changes）：整体 body 进一个闭包
   - 含 confirm await 的 3 个（promote / reject / rollback）：按
     「无锁快照段 → await 确认 → 持锁 RMW 段」既有分界拆两段阻塞闭包，
     await 不跨闭包（rollback 的 sync 段已是独立 fn，直接包）

**签名零变更**（所有 command/fn 签名不动，spawn_blocking_map 是
行为层包裹）；锁/并发语义不变（evolution store 锁、DB_WRITE_LOCK
纪律原样随闭包走）。

## 人类可读摘要

- family: async-blocking-io（锁范围过宽 / worker 钉住）
- 覆盖 findings: 3
- 预估 diff: 3 files / +106/-39 lines → 实测 +157/-77（A 类，budget 校正 **+165/-85**）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 3

## 核实的编译层事实（spec 前已核）

1. apply.rs:148-150 整批已在 `tauri::async_runtime::spawn_blocking`
   内——finding 的「async 上下文阻塞」前提对 apply.rs 部分过时，
   剩余真问题 = DB_WRITE_LOCK 范围。
2. `open_db`（db/mod.rs:55）内部**不**拿 DB_WRITE_LOCK（调用方纪律），
   移出锁安全。
3. shadow trait 两变体 + AppShadowSink 零生产调用方（grep 全仓）；
   with_app 唯一生产入口 = apply.rs:227。
4. shadow.rs 测试全 `#[tokio::test]`——spawn_blocking 有 runtime 上下文。
5. `spawn_blocking_map<F,T>(F: FnOnce()->Result<T,String>+Send+'static)`
   在 py/document.rs:1059 为 `pub async fn`，仓内 3 处既有使用先例
   （bot_skills/files.rs、py/commands.rs 等直接用
   `tauri::async_runtime::spawn_blocking`）。
6. ChangeRecord/ProposalEntry 均 owned Clone+Send；AppHandle Clone+Send
   +'static——闭包化合法。
7. promote(:282)/reject(:322)/rollback(:407) 含 ask_user_confirm await；
   其余 5 个 command 纯同步。rollback 的锁内段已是独立 sync fn
   `rollback_change_locked`（:432）。
8. 既有 asserts：apply 25 / observe/shadow 79 / panel/commands 19。

## 修法

- **apply.rs**：spawn_blocking 闭包内——open_db/ledger/now 提前到锁外；
  循环改「锁 scope 仅包 apply_one → append_applied_record + audit 锁外」。
- **observe/shadow.rs**：with_app 循环内 append_change 包
  spawn_blocking（JoinError→Err 走既有 failed 分支）；trait 两变体
  doc 注明测试/内存 sink 专用。
- **panel/commands.rs**：8 command 按上表包裹 spawn_blocking_map。

## 红线

- 签名零变更；不 drop async（调用链不动）
- 不改 `?` 逐条中止语义（partial-failure 形态保持现状——:111 medium
  的 audit-before-DB 方向不属本簇）
- 不动 panel/commands.rs :43/:80/:125/:282/:323（非本簇行号）
- audit_event! 内部阻塞 IO 不追（全仓普遍模式，超 scope）

## spec 起草后自查三条

1. `expected_files` = 3：apply.rs + observe/shadow.rs +
   panel/commands.rs。无 ripple（签名全不变）。
2. budget = **A 类**：apply +20/-12、shadow +16/-2、panel +70/-25：
   初估 +106/-39 → 实测 +157/-77（闭包化重缩进双向计数超估），校正 → **max +165/-85**。
3. 三条 findings 的 fix 字段均已写明 ripple（均无）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "EV-3b-G",
  "family": "async-blocking-io",
  "expected_files": [
    "src-tauri/src/evolution/apply.rs",
    "src-tauri/src/evolution/observe/shadow.rs",
    "src-tauri/src/evolution/panel/commands.rs"
  ],
  "max_lines_added": 165,
  "max_lines_removed": 85,
  "findings": [
    {"id": "C5-EV-3b-G-1", "file": "src-tauri/src/evolution/apply.rs", "line": 156, "fix": "DB_WRITE_LOCK 临界区收窄到仅 apply_one（SQL 写）；open_db/ledger/now 锁外预计算，append_applied_record+audit 锁外；逐条 ? 中止语义不变；无 ripple（内部重排，签名不变）"},
    {"id": "C5-EV-3b-G-2", "file": "src-tauri/src/evolution/observe/shadow.rs", "line": 202, "fix": "with_app（唯一生产入口）append_change 包 spawn_blocking，JoinError 映射为写失败走既有 failed/audit 路径；trait 两变体零生产调用方加 doc 注明测试/内存 sink 专用；无 ripple"},
    {"id": "C5-EV-3b-G-3", "file": "src-tauri/src/evolution/panel/commands.rs", "line": 0, "fix": "8 个 async command 阻塞段包 spawn_blocking_map（5 个整体闭包；promote/reject/rollback 按无锁快照段/持锁 RMW 段拆两段，await 不跨闭包）；签名零变更无 ripple"}
  ],
  "assertions_min": {
    "src-tauri/src/evolution/apply.rs": 25,
    "src-tauri/src/evolution/observe/shadow.rs": 79,
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
