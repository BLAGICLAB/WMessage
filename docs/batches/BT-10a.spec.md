# BT-10a spec — 缩短临界区：锁内 IO/回调全移出（C5-BT-10 拆批 1/2）

## 拆批说明

C5-BT-10 四条中前三条同 family（缩短临界区：锁内不做 IO / 不调外部钩子）；
bot_scheduler.rs:468（无 panic recovery + 无关闭信号 + 无并发上限）是**调度器
韧性**主题，family 不同 → 拆 BT-10b 单独评（涉 catch_unwind / semaphore 设计选型）。

## 目标 findings（3 条实修）

- **audit.rs:22**：`audit_log` 持 BOT_LOG_LOCK 跨 data_dir（首次调用文件系统
  syscall）+ rotate_log_if_large（stat+rename 数 MB）+ append。修：data_dir +
  rotation 移到锁外，锁内只剩 open+append+drop。已核 `rotate_log_if_large`
  （db/paths.rs:29-36）容错竞争——metadata 失败跳过、rename 失败 `let _=`，
  双写者并发 rotation 最坏=当次不轮转下次补上，无数据破坏。
- **runtime.rs:83**（start_skill）：switched 技能的 audit_log_hook 在
  skill_runs mutex 内调用（钩子写盘=持锁跨 IO，重入即死锁）。修：锁内收集
  (name, step) 对 + 完成 insert，drop guard 后再发 switched 审计 + skill.start
  audit_event。审计相对顺序不变（switched 先于 skill.start）。
- **runtime.rs:393**（skill_finish）：失败分支 `load_skill_meta`（逐技能读
  SKILL.md 磁盘 IO）+ audit_log_hook 在 mutex iter_mut 循环内。修：锁内做状态
  迁移 + 收集 (name, step, actions.clone(), rollback, ok/fail) 快照，drop guard
  后在锁外构造 log 文本 / rollback_hint / 发钩子。rollback_hint「最后一个命中
  run 覆盖」语义保持不变。

## 行为变更

- 零（状态迁移/审计内容/审计顺序/返回值全部不变；仅临界区收窄 + IO 移出锁）。
- 隐性收益：rotation 慢 IO 不再饿死其它 audit 写者；start_skill/skill_finish
  不再持锁等磁盘。

## spec 起草后自查三条

1. `expected_files` → `src-tauri/src/bot/config/audit.rs` +
   `src-tauri/src/bot_skills/runtime.rs`。无签名 ripple（三处都是函数体重构）。✓
2. budget → audit.rs ≈+6/-4；start_skill ≈+14/-10；skill_finish ≈+35/-25。
   合计 ≈+55/-39 → budget +70/-50。✓
3. findings fix 字段列 ripple → 均无。✓

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
  "batch_id": "BT-10a",
  "family": "critical-section-shrink",
  "expected_files": [
    "src-tauri/src/bot/config/audit.rs",
    "src-tauri/src/bot_skills/runtime.rs"
  ],
  "max_lines_added": 70,
  "max_lines_removed": 50,
  "findings": [
    {"id": "C5-BT-10.1", "file": "src-tauri/src/bot/config/audit.rs", "line": 22, "fix": "data_dir + rotate_log_if_large 移到 BOT_LOG_LOCK 外，锁内只剩 open+append。已核 rotate 容错竞争（paths.rs:29-36）。ripple：无"},
    {"id": "C5-BT-10.2", "file": "src-tauri/src/bot_skills/runtime.rs", "line": 83, "fix": "start_skill：锁内收集 (name,step) + insert，drop guard 后发 switched 审计 + skill.start audit_event，审计顺序不变。ripple：无"},
    {"id": "C5-BT-10.3", "file": "src-tauri/src/bot_skills/runtime.rs", "line": 393, "fix": "skill_finish：锁内状态迁移 + 快照 (name,step,actions,rollback,ok)，drop guard 后锁外 load_skill_meta + 构造 log/rollback_hint + 发钩子；末 run 覆盖语义不变。ripple：无"}
  ],
  "assertions_min": {
    "src-tauri/src/bot/config/audit.rs": 0,
    "src-tauri/src/bot_skills/runtime.rs": 60
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 4
  }
}
```
