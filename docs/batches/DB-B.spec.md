# Batch Spec: DB-B

## 目的

落地用户拍板的 B 类决策 3 项（db 域，2026-09-25 拍板）：#3 copy_legacy_db 坏源 fail-closed（A+C 组合）、#4 workspace_import_merge NULL 语义显式 Err（C）、#5 bot_history 覆写行序复合键固化（C）。对应 PHASE2-TRIAGE §4 B 类权威清单 #3/#4/#5。

## 人类可读摘要

- family: db-failclosed-policy
- 覆盖: B 类 3 项（3 实修，其中 1 项以注释+回归测试固化既有正确不变量）
- 预估 diff: 4 文件 / +160/-6（无新文件）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 4

## 逐条处置（原文均已核 + 用户拍板 2026-09-25）

**实修 ① src-tauri/src/db/paths.rs:58（C5-DB-05.1，用户拍 #3=A+C）**：`copy_legacy_db` 的 `Connection::open(legacy_db)` 失败臂现仅 push warn（"老库打开失败（跳过 checkpoint 直接拷贝）"）后继续字节级裸拷——损坏/占用/加密的源库被原样复制为主库（数据破坏级）。
**修法**：该臂改早退 `Err(CommandError::IoError)`，消息可操作：指明已拒绝拷贝、原始老库未改动（本函数对 legacy 全程只读，天然满足"保留坏库供人工恢复"）、排障步骤（关闭占用程序/修老库后重启重试）。db/mod.rs open_db 首装段 Err 臂（:82-84）补 `audit::write_event(AuditLevel::Error, "legacy_db_copy")`（ERROR 日志）后再返回 Err（UI 可操作提示 = Err 字符串上浮到前端 CommandError）。
**ripple**：调用点仅 mod.rs:72 一处，签名不变；既有测试 `copy_legacy_db_returns_err_when_legacy_db_missing` 仍 is_err()（失败点提前，断言不翻）。

**实修 ② src-tauri/src/db/workspace.rs:240（C5-DB-05.2，用户拍 #4=C）**：`workspace_import_merge_unchecked` 的 `cur.flatten().unwrap_or(0)` 把「目标行不存在」与「目标行 updated_at 为 NULL」并为 0——NULL 语义重载（导入行 0>0 不覆盖、带时间戳行可覆盖 NULL 行，均无定义）。
**修法**：match cur 三臂——None→0（行不存在，现状保留）；Some(None)→Err 可操作消息（fail-closed，指向 tx 已回滚、要求应用内编辑该项回填时间戳后重试；COALESCE+NOT NULL 为长期方向，涉 schema 迁移不入本批）；Some(Some(ua))→ua。导入行缺 updated_at 维持 unwrap_or(0)=不覆盖非 NULL 行（注释文档化，拍板未改此半边）。
**ripple**：返回 Result<usize, String> → CommandError::from 既有映射（Internal），调用点 workspace_import 不变。

**实修 ③ src-tauri/src/db/bot_history.rs:52（C5-DB-05.3，用户拍 #5=C）**：`bot_history_save_inner` 整批 created_at 用循环前单次 now，全量覆写后时间戳坍缩（OCR finding）。核后事实：`bot_history_load` 已 `ORDER BY id`（INTEGER PRIMARY KEY AUTOINCREMENT 自增=行序）——拍板 C 的「(now, row_sequence) 复合排序键」中 row_sequence 分量已在位，行序恢复本就不依赖 created_at。
**修法**：注释固化设计不变量（created_at=本次覆写的原子时刻、同批相同系有意；行序唯一由自增 id 恢复，按 created_at 排 bot_messages 的消费方均属缺陷）于 save 的 `let now` 与 load 的 ORDER BY 处 + 新增回归测试锁「覆写后 load 行序==写入序 && 批内 created_at 相等 && id 单调递增」。

## 测试（db/mod.rs 既有测试模块内新增，直调生产函数，仿 workspace_import_merges_by_updated_at_and_skips_empty_id 风格）

1. `mod tests`：copy_legacy_db_open_failure_fails_closed——legacy_db 传目录（Connection::open 必败 EISDIR）→ Err（消息含"拒绝拷贝"）+ dst 主库/-wal/-shm 三者均未产生
2. `mod tests`：bot_history_overwrite_restores_order_and_batch_created_at——save [a,b,c] 后 save [d,e]，直查 SQL：load 序==[d,e]、第二批 created_at 全等、id 单调递增
3. `mod ws_tests`：workspace_import_aborts_on_null_updated_at_target——seed 一行 updated_at=NULL → 导入任一时间戳项 → Err（消息含 updated_at）+ 该行未被覆盖 + 同批其他行未写入（tx 回滚）

## 红线

- family 一致性：只含 db 域 3 项 B 类拍板落地
- 不动建表 schema（NULL→NOT NULL 长期方向不入本批）、不动 tasks.rs
- legacy 老库只读，任何分支不得写 legacy 路径
- 新注释不引用审计批次号

## spec 起草后自查三条

1. expected_files：db/paths.rs + db/mod.rs + db/workspace.rs + db/bot_history.rs = 4（全路径已列）
2. budget：A 类（臂替换 + match 展开行内小改）≈ +25/-3；B 类（3 个测试函数整块 ≈ 135 行）合计 ≈ +160/-6，上限 +220/-25；无新文件（max_new_files_lines 不设）
3. fix 字段 ripple：① mod.rs Err 臂 +audit 一处（已入 expected_files）；② 函数内闭环走既有 From<String> 映射；③ 纯注释+测试

## 自主执行规则

spec 经用户拍板（B 类 3 项方向 2026-09-25）后：
- agent 全权执行至 commit，不中途报 status / exit / diff
- 例外只有 stop 条件触发
- 完成时一次报：hash + 批定性 + OCR 轮次 + comments 处置 + push 两值
- 过程证据落 ~/.openclaw/cache/DB-B/，不喊人

## Stop 条件（触发即停，报用户）

- compile_failure（无 X 形态退路）
- architecture_blocker（签名/调用点超预算）
- family_heterogeneity
- new_high_different_root（OCR r1 出的 high 非本批根因）
- gate_fail（batch-verify FAIL 且非 spec 声明调整可解）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "DB-B",
  "family": "db-failclosed-policy",
  "expected_files": [
    "src-tauri/src/db/paths.rs",
    "src-tauri/src/db/mod.rs",
    "src-tauri/src/db/workspace.rs",
    "src-tauri/src/db/bot_history.rs"
  ],
  "max_lines_added": 220,
  "max_lines_removed": 25,
  "findings": [
    {"id": "C5-DB-05.1", "file": "src-tauri/src/db/paths.rs", "line": 58, "fix": "open 失败臂 warn 继续 → 早退 Err(IoError) 可操作消息（fail-closed，原库保留）；ripple：db/mod.rs Err 臂 +audit write_event Error"},
    {"id": "C5-DB-05.2", "file": "src-tauri/src/db/workspace.rs", "line": 240, "fix": "cur.flatten().unwrap_or(0) → match 三臂：Some(None)=NULL 显式 Err 中止导入（tx 回滚）；None→0 / Some(ua)→ua 现状；导入行缺时间戳 unwrap_or(0) 注释文档化。ripple：无（既有 From<String> 映射）"},
    {"id": "C5-DB-05.3", "file": "src-tauri/src/db/bot_history.rs", "line": 52, "fix": "单 now 保留 + save/load 两处注释固化 (created_at,id) 复合键不变量（行序由自增 id 恢复）+ 回归测试锁行序/批内等时/id 单调。ripple：无"}
  ],
  "assertions_min": {
    "src-tauri/src/db/mod.rs": 11
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
fix(db): DB-B — db-failclosed-policy（B 类拍板落地 3 项）

【family】坏源 fail-closed / NULL 语义显式化 / 覆写行序复合键固化
【实修 3 处】
- db/paths.rs:58 open 失败 warn 继续 → 早退 Err（fail-closed，原库保留，消息含排障步骤）
- db/workspace.rs:240 NULL updated_at → 显式 Err 中止导入回滚（None/Some 现状保留）
- db/bot_history.rs:52 注释固化 (created_at,id) 复合键不变量 + 回归测试
【行为变更】坏源老库不再被裸拷为主库；NULL 目标行导入由静默跳过/覆盖改为整批 Err
【自测】cargo fmt/check + tsc + test-all 全绿
【OCR】r1：<N> comments <处置>
【基线】D2: files=4(+A/-R) asserts=… tests=…
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/DB-B.spec.md python3 scripts/batch-verify.py docs/batches/DB-B.spec.md
```
