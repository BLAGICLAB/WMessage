# BT-04a spec — TOCTOU 两条机械修（C5-BT-04 拆批 1/2）

## 拆批理由

C5-BT-04 四条两种修法形态：
- **本批（check-then-act 窗口机械收）**：bot_chat.rs:399 + bot_artifacts.rs:67。
- **下批（spawn_blocking 异步运行时重构，architecture 风险大）**：bot_fs.rs:167
  （critical：5+ async fn 直接调 std::fs 同步 syscall + 共享 walk helper）+
  bot/tools.rs:102（sanitize_task_files_in 内 canonicalize）。这两条修法 =
  async 调用链整体 spawn_blocking 化，签名/调用面大，单独成批细评。

## 目标 findings（2 条）

1. **bot_chat.rs:399**（high）：白名单校验用 `canonicalize(p)` 得 `c` 做比较，
   但随后 `std::fs::read(p)` 读的是**原路径**——校验与读取之间 symlink 掉包
   即绕过白名单，任意文件（≤MAX_IMAGE_BYTES）base64 进 LLM prompt。修：
   canonical 落点存下，读 `&canon` 不读 `p`（校验对象 = 读取对象同一化）。
2. **bot_artifacts.rs:67**（high）：`should_emit→peek`（弹窗快照）与
   `confirm_artifact_batch→take_all`（清空）之间新 `register()` 的产物被
   take_all 静默清掉（不在用户勾选内也不在登记表 = 丢）。修：take_all 返回值里
   「不在本次勾选 paths 内」的重新登记，留下一轮弹窗；确认路径行为不变。

## 行为变更

- bot_chat：图片读取目标从原始路径变 canonical 落点（同一文件；symlink 掉包
  窗口关闭）。正常路径无变化。
- bot_artifacts：并发窗内晚登记的产物不再丢失（重新登记，registered_at 刷新为
  重登时间）。确认绑定路径行为不变。

## spec 起草后自查三条

1. `expected_files` → `src-tauri/src/bot_chat.rs` + `src-tauri/src/bot_artifacts.rs`。
   无签名 ripple（register/take_all/confirm_artifact_batch 签名不变）。✓
2. budget → bot_chat ≈+6/-3；bot_artifacts ≈+8/-1。合计 ≈+14/-4 → budget +20/-8。✓
3. findings fix 字段列 ripple → 均无（两文件内部）。✓

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
  "batch_id": "BT-04a",
  "family": "toctou-check-then-act",
  "expected_files": [
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/bot_artifacts.rs"
  ],
  "max_lines_added": 20,
  "max_lines_removed": 8,
  "findings": [
    {"id": "C5-BT-04.3", "file": "src-tauri/src/bot_chat.rs", "line": 399, "fix": "canonicalize 落点存入 canon；白名单比较与 std::fs::read 都用 canon（原 read(p) 读原路径留掉包窗）。ripple：无"},
    {"id": "C5-BT-04.4", "file": "src-tauri/src/bot_artifacts.rs", "line": 67, "fix": "confirm_artifact_batch 尾部 take_all 返回值中不在 paths 勾选内的产物重新 register（并发窗晚登记不静默丢弃）。ripple：无"}
  ],
  "assertions_min": {
    "src-tauri/src/bot_chat.rs": 70,
    "src-tauri/src/bot_artifacts.rs": 14
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 4
  }
}
```
