# Batch Spec: MI-B1

## 目的

落地用户拍板的 B 类决策 2 项（migration 域，2026-09-25 拍板）：#1 load_rules 结构化 Err（B 主案；现场核得 caller 恰 3 处 ≤3，不触发 A 应急）、#9 archive_dir 三道闸（A=绝对路径拒绝 + symlink 逃逸校验 + 破坏性 move/delete 导入确认）。对应 PHASE2-TRIAGE §4 B 类权威清单 #1/#9。

## 人类可读摘要

- family: migration-rules-failclosed
- 覆盖: B 类 2 项（2 实修）
- 预估 diff: 4 文件 / +230/-28（无新文件）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 4

## 逐条处置（原文均已核 + 用户拍板 2026-09-25）

**实修 ① src-tauri/src/migration/rules.rs:26（C5-MI-03.1，用户拍 #1=B；caller=3 走 B 主案）**：load_rules 两臂静默降级——解析失败仅 log_line 后 `RulesFile::default()`，读取失败 `Err(_) => default()` 全静默；坏规则文件被空规则顶替后，任何隐式/显式 save 均以空规则覆写用户配置（数据丢失级）。
**修法**：拆 `load_rules_from(path) -> Result<RulesFile, String>` 纯函数（NotFound → Ok(default)（首装合法态，注释钉住边界）；读失败非 NotFound / 解析失败 → Err 可操作消息）+ `load_rules(app)` 包装（Err 时 log_line 迁移日志留痕后透传）。三个调用点显式分流（签名改已获拍板）：
- run.rs:41 `let rules = load_rules(app)?;`——规则不可读 → 迁移中止（不再以空规则执行任何 move/delete）；
- commands.rs:21 `migration_rules_load -> CommandResult<RulesFile>`——UI 拿 Err；
- commands.rs:172 `migration_status -> CommandResult<MigrationStatus>`——面板轮询拿 Err（前端 MigrationPanel 两调用点均在既有 try/catch 内，零前端改动，已核）。
**ripple**：save_rules 触发面仅 migration_rules_import（显式用户动作，grep 已核），无隐式 save 链；既有测试无 load_rules 直调（grep 已核）。

**实修 ② src-tauri/src/migration/ops.rs:74 + rules.rs:58 + commands.rs:74（C5-MI-08.3 残余，用户拍 #9=A 三项全做）**：resolve_archive_dir 现「绝对路径放行（文档化特性）」+ 无 symlink 逃逸校验（源码注释自记「同属 B 类待拍项」）；导入侧无破坏性提示。
**修法**：
- 拆 `resolve_archive_dir_checked(base, template)` 纯函数内核，三道闸：`..` 组件拒绝（既有）；**绝对路径拒绝**（收窄原「文档化特性」，拍板 A；消息给相对写法示例）；**symlink 逃逸校验**（沿最深已存在祖先 canonicalize，须仍位于 canonical 基准目录内——叶子尚不存在时只能校祖先，注释声明）。
- validate_rules（导入期早错）补绝对路径拒绝（仅 move 规则有 archive_dir，与既有 `..` 拒绝同位）。
- migration_rules_import：validate 通过后、save 前，若规则含 move/delete → 系统确认框（tauri-plugin-dialog blocking message，spawn_blocking 内防主线程死锁——同文件 pick_file 先例）列「N 条移动归档 / M 条删除，执行后文件将离开原位置或被删除（不可自动撤销）」，取消 → ConfirmRejected（既有错误码）。
**ripple**：绝对路径为既有存量配置的行为变更（运行期 resolve 返 Err 明示拒执行，进迁移报告/日志）——拍板 A 明示接受；前端零改动。

## 测试（rules.rs / ops.rs 新增 #[cfg(test)]，纯函数直测不需 AppHandle）

1. rules.rs：load_rules_from 缺文件 → Ok(default)（首装合法态边界钉住）；坏 JSON → Err（消息含「规则文件解析失败」）；合法 JSON（camelCase 字段）→ Ok 且字段回读正确
2. rules.rs：validate_rules 绝对路径 archive_dir → Err
3. ops.rs：resolve_archive_dir_checked——相对正常 → Ok(base.join)；`..` → Err；绝对路径 → Err（消息含「相对路径」）；symlink 逃逸 → Err（temp base 内建 symlink 指向 base 外）
4. 既有 BUSY/checkpoint 等迁移测试不回归（test-all 全量担保）

## 红线

- family 一致性：只含 migration 域 2 项 B 类拍板落地
- 不动 run_migration 阶段语义（除规则 Err 中止）；不碰 #2（CancellationToken 另批 MI-B2）
- 前端零改动（已核 MigrationPanel try/catch 在位）
- 新注释不引用审计批次号

## spec 起草后自查三条

（执行中校正 ×3：×1 assertions_min 8/5→7/4；×2 budget +300→+380；×3 budget +380→+450，OCR r1 high + r2 critical + r3 high×3 采纳增量）

1. expected_files：migration/rules.rs + migration/ops.rs + migration/run.rs + migration/commands.rs = 4（全路径已列）
2. budget：A 类（签名/臂行内改 ≈ +35/-16）+ B 类（确认框块 ≈ +24；两处测试整块 ≈ +165）合计 ≈ +350/-42，上限 +450/-50（执行中校正 ×3：OCR r1 high + r2 critical + r3 high×3 采纳——结构化 CommandError 链 / 确认框 audit+启用行计数 / create 后终态复验 verify_archive_dir_created / 补测；OCR 采纳增量随批校正）
3. fix 字段 ripple：① 三调用点全列（run.rs/commands.rs×2）+ 前端零改动已核；② validate_rules/ops/commands 三点已入 expected_files；ConfirmRejected 为既有错误码零新增

## 自主执行规则

spec 经用户拍板（B 类 2 项方向 2026-09-25）后：
- agent 全权执行至 commit，不中途报 status / exit / diff
- 例外只有 stop 条件触发
- 完成时一次报：hash + 批定性 + OCR 轮次 + comments 处置 + push 两值
- 过程证据落 ~/.openclaw/cache/MI-B1/，不喊人

## Stop 条件（触发即停，报用户）

- compile_failure（无 X 形态退路）
- architecture_blocker（签名/调用点超预算）
- family_heterogeneity
- new_high_different_root（OCR r1 出的 high 非本批根因）
- gate_fail（batch-verify FAIL 且非 spec 声明调整可解）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "MI-B1",
  "family": "migration-rules-failclosed",
  "expected_files": [
    "src-tauri/src/migration/rules.rs",
    "src-tauri/src/migration/ops.rs",
    "src-tauri/src/migration/run.rs",
    "src-tauri/src/migration/commands.rs"
  ],
  "max_lines_added": 450,
  "max_lines_removed": 50,
  "findings": [
    {"id": "C5-MI-03.1", "file": "src-tauri/src/migration/rules.rs", "line": 26, "fix": "load_rules 拆纯函数 load_rules_from（NotFound→Ok default 边界钉住；读/解析失败→Err 可操作消息）+ wrapper log_line 留痕；三调用点显式分流：run.rs ? 中止迁移 / migration_rules_load→CommandResult / migration_status→CommandResult（前端 try/catch 已核零改动）。ripple：commands.rs 两签名 + run.rs 一行"},
    {"id": "C5-MI-08.3", "file": "src-tauri/src/migration/ops.rs", "line": 74, "fix": "resolve_archive_dir 拆 checked 纯内核三道闸：..拒绝（既有）+ 绝对路径拒绝（拍板收窄）+ symlink 逃逸 canonicalize 祖先校验；validate_rules 补绝对路径早错；migration_rules_import 破坏性 move/delete 系统确认框（spawn_blocking + ConfirmRejected）。ripple：rules.rs validate_rules + commands.rs import 确认块"}
  ],
  "assertions_min": {
    "src-tauri/src/migration/rules.rs": 7,
    "src-tauri/src/migration/ops.rs": 4
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
fix(migration): MI-B1 — migration-rules-failclosed（B 类拍板落地 2 项）

【family】规则文件 fail-closed / 归档目录三道闸（用户拍板 2026-09-25：#1=B、#9=A）
【实修 2 处】
- rules.rs:26 load_rules → Result（NotFound=首装合法态；坏文件 Err 不再空规则顶替）；run.rs 迁移中止 / rules_load·status 返 CommandResult
- ops.rs:74 archive_dir 三道闸（..既有 + 绝对路径拒绝 + symlink 逃逸校验）；validate_rules 早错；import 破坏性确认框
【行为变更】坏规则文件不再被空规则静默顶替；存量绝对路径归档配置运行期拒绝执行（拍板接受）；规则导入前弹破坏性确认
【测试】+8：load_rules_from 三态 / validate 绝对路径 / resolve 四态含 symlink 逃逸
【OCR】r1：<N> comments <处置>
【基线】D2: files=4(+A/-R) asserts=… tests=…
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/MI-B1.spec.md python3 scripts/batch-verify.py docs/batches/MI-B1.spec.md
```
