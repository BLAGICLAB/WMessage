# BT-04b spec — blocking-syscall-on-async-runtime 收尾（C5-BT-04 拆批 2/2）

## 背景核（spec 前调用面核查结论）

bot_fs.rs:167 finding 列的 6 个阻塞点，**4 个已被 C1b 批包过**（resolve_with_perm
canonicalize :194 / capture_pre_ino :418 / read_capped_file :375 / grep+list walk
:656/:755），spawn_blocking_map 设施现成（py/document.rs）。**剩余未包点**：

1. `allowed_dirs`（bot_fs.rs:118-149）：canonicalize 循环 + gen_dir 在 async fn 内
   同步跑，调用点 :195（resolve_with_perm）+ :639（grep 默认目录）。
2. `is_dir()` 单次 stat ×3：:512（read_text_file）/ :645（grep）/ :744（list_files）。
3. `sanitize_task_files_in`（tools.rs:96-116）：逐文件 canonicalize（≤10 条），
   调用链 sanitize_task_files_arg(:74) ← tool_create_task(:362 async) /
   tool_edit_task(:598 async)。:1542/:1548 是测试调用，不动。

调用面可控（sanitize_task_files_arg 是私有 fn，ripple 仅 2 个调用点加 .await），
**非 architecture_blocker**。

## 目标 findings（2 条实修）

- **bot_fs.rs:167**：
  - `allowed_dirs` 尾部（gen_dir + merge_raw_dirs + canonicalize 循环）整体移入
    单个 spawn_blocking_io 闭包（app/cfg_dirs/task_dirs move 进闭包，JoinError
    unwrap_or_default 保「无目录→空」语义）。
  - 3 处 `is_dir()` 单次 stat 各包 spawn_blocking_io（闭包恒 Ok(bool)）。
- **tools.rs:102**：`sanitize_task_files_arg` 改 async——gen_dir+canonicalize+
  sanitize_task_files_in 整体进 spawn_blocking_map 闭包（JoinError
  unwrap_or_default = 全丢，与 sanitize fail-closed 语义一致）；2 调用点加 .await。

## 行为变更

- 零（纯执行线程迁移：同步 syscall 从 runtime 线程移到 blocking 线程，返回值/
  拒绝语义/审计全部不变）。

## TOCTOU 部分（两条 finding 的另一半）——本批不做

finding 的 TOCTOU 维度（check 与 use 之间 symlink swap）已由既有 follow-up 覆盖：
「Phase 6: resolve_with_perm 其它调用点 TOCTOU 统一策略」+「Phase 6: symlink
target 替换防护收紧」（bot_fs.rs:527-529 注释钉死）。本批只关 performance 维度。

## spec 起草后自查三条

1. `expected_files` → `src-tauri/src/bot_fs.rs` + `src-tauri/src/bot/tools.rs`。
   ripple：sanitize_task_files_arg 2 调用点（同文件内）。✓
2. budget → allowed_dirs 重构 ≈+14/-8；is_dir×3 ≈+18/-3；sanitize async 化
   ≈+12/-6。合计 ≈+44/-17 → budget +60/-25（留 ~10% rustfmt/闭包组装余量）。✓
3. findings fix 字段列 ripple → tools.rs 那条列了 2 调用点。✓

## 自主执行规则

spec 被 reviewer 批准后:
- agent 全权执行至 commit,不中途报 status / exit / diff
- 例外只有 stop 条件触发
- 完成时一次报: hash + 批定性 + OCR 轮次 + comments 处置 + follow-up
- 过程证据落 ~/.openclaw/cache/<batch-id>/,不主动读回

## Stop 条件(触发即停,报 reviewer)

- compile_failure (无 X 形态退路)
- architecture_blocker (签名/调用点超预算)
- family_heterogeneity
- new_high_different_root (OCR r1 出的 high 非本批根因)
- gate_fail (batch-verify FAIL 且非 spec 声明调整可解)

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "BT-04b",
  "family": "blocking-syscall-on-async-runtime",
  "expected_files": [
    "src-tauri/src/bot_fs.rs",
    "src-tauri/src/bot/tools.rs"
  ],
  "max_lines_added": 60,
  "max_lines_removed": 25,
  "findings": [
    {"id": "C5-BT-04.1", "file": "src-tauri/src/bot_fs.rs", "line": 167, "fix": "剩余未包点收尾：allowed_dirs 尾部（gen_dir+canonicalize 循环）整体进 spawn_blocking_io；is_dir() 单次 stat ×3（:512/:645/:744）各包 spawn_blocking_io。C1b 已包 4 点不动。ripple：无（allowed_dirs 调用点签名不变）"},
    {"id": "C5-BT-04.2", "file": "src-tauri/src/bot/tools.rs", "line": 102, "fix": "sanitize_task_files_arg 改 async，gen_dir+canonicalize+sanitize 内核整体进 spawn_blocking_map（JoinError 全丢 = fail-closed 一致）。ripple：tool_create_task:416 / tool_edit_task:668 两调用点加 .await"}
  ],
  "assertions_min": {
    "src-tauri/src/bot_fs.rs": 44,
    "src-tauri/src/bot/tools.rs": 37
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 4
  }
}
```
