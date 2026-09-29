# Batch Spec: B6-HYGIENE

## 目的

按 `docs/AUDIT-FIX-PLAN-2026-09-29.md` B6 批（仓库卫生与文档）：

1. 删本地两代 portable（`wmessage-portable-2026-09-18` 目录+zip、`-09-25`，
   共 ~380MB；`-09-28` 为待验收现行包保留）——均未入库，无 git 变更。
2. 根目录自进化五稿 `git mv` → `docs/evolution/`（R2_DESIGN / R6_A_DESIGN /
   DERIVABILITY / OBSERVATION_STATUS / VERIFICATION）+ `evolution/observe-report-r6b-smoke.json`
   同迁；根目录 `evolution/` 清空删除（phase-1/2.md 本就在 docs/evolution/）。
3. `docs/archive/` 归档：8 月审计 22 份（含 kimi-audit/）→ `2026-08-audit/`；
   HANDOFF-2026-09-23(-v2)/09-25 → `handoffs/`（保留现行 26-v3）；bug-hunt 与
   comment-hygiene 两轮评审材料 → `review-campaigns/`；archive/README.md 索引。
   本地 OCR-CODE-REVIEW 三格式留一（json，html/txt 删）——均 gitignored 不入库。
4. `health-check.sh`（gitignored 本地脚本）→ `scripts/health-check.sh` 并入库，
   .gitignore 移除对应行；`README.txt`（未跟踪便携包说明）→ `packaging/README.txt`。
5. `docs/SKILL_DSL.md` → `docs/SKILL-DSL.md`（6 处活引用同步）；`SPEC.md` 顶部
   加「历史存档声明」（原始需求快照，现行架构以 rust-bot-architecture.md 为准）；
   README 目录树 4 处过时点更正（db.rs→db/、bot/config.rs→config/、bot_py.rs
   薄门面补 py/ 指向、补 bot/mcp/、bot_skills/、bot_orchestrator、evolution/）
   + 文档清单补架构文档/docs/evolution/ + docs/archive/。
6. OBSERVATION_STATUS「969/969」与 phase-2「812 通过」加「截至交付日」标注
   （B6-6：历史数字防被误读为现行基线）。

## 红线

- 纯文档/文件搬移批：零源码改动（scripts/health-check.sh 内容与原文件一致，
  仅用法行改为 scripts/ 路径）。
- 归档=移动非删除（git mv 全程；审计历史可追溯）。

## 测试

- 文档批无代码路径：test-fast 智能跳过（无 src 改动）；模块地图对拍不受影响
  （rust-bot-architecture.md 条目未变，SKILL-DSL 改名已同步）。

## spec 起草后自查三条

1. expected_files 即本次 staged 全集（74 项：rename 对以新路径计，git 状态里
   rename 对的旧路径不计入 file_set 判定——batch-verify 以 staged 新路径清单为准）。
2. budget：修改 9 文件 +60/-30；rename 64 对 + archive README + health-check.sh
   171 行（moved，内容不变）。
3. 无 findings（纯搬移批，findings 留空走 skip）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "B6-HYGIENE",
  "family": "repo-hygiene",
  "expected_files": [
    ".gitignore",
    "DEVLOG.md",
    "README.md",
    "SPEC.md",
    "docs/AUDIT-FIX-PLAN-2026-09-29.md",
    "docs/MANUAL-ACCEPTANCE.md",
    "docs/PHASE2-TRIAGE.md",
    "docs/SKILL-DSL.md",
    "docs/archive/README.md",
    "docs/archive/evolution-phase-1.md",
    "docs/archive/2026-08-audit/AUDIT-AGENT-CORE-2026-08-18.md",
    "docs/archive/2026-08-audit/AUDIT-AGENT-FLOW-2026-08-18.md",
    "docs/archive/2026-08-audit/AUDIT-API-2026-08-28.md",
    "docs/archive/2026-08-audit/AUDIT-BOT-PIPELINE-2026-08-27.md",
    "docs/archive/2026-08-audit/AUDIT-CONTRACT-2026-08-28.md",
    "docs/archive/2026-08-audit/AUDIT-DATA-2026-08-28.md",
    "docs/archive/2026-08-audit/AUDIT-FIX-PLAN-2026-08-18.md",
    "docs/archive/2026-08-audit/AUDIT-LLM-2026-08-28.md",
    "docs/archive/2026-08-audit/AUDIT-PLAN-BOT-2026-08-27.md",
    "docs/archive/2026-08-audit/AUDIT-PLATFORM-2026-08-28.md",
    "docs/archive/2026-08-audit/AUDIT-REPORT-KIMI-FAILED-2026-08-18.trace.md",
    "docs/archive/2026-08-audit/AUDIT-REPORT-MANUAL-2026-08-18.md",
    "docs/archive/2026-08-audit/AUDIT-SCHED-2026-08-28.md",
    "docs/archive/2026-08-audit/AUDIT-SECURITY-2026-08-27.md",
    "docs/archive/2026-08-audit/AUDIT-SUMMARY-2026-08-28.md",
    "docs/archive/2026-08-audit/AUDIT-TESTS-2026-08-28.md",
    "docs/archive/2026-08-audit/BATCH-C-REAUDIT-2026-08-19.md",
    "docs/archive/2026-08-audit/BATCH-D-REAUDIT-2026-08-19.md",
    "docs/archive/2026-08-audit/kimi-audit/AUDIT-FIX-PLAN-KIMI-2026-08-18.md",
    "docs/archive/2026-08-audit/kimi-audit/AUDIT-PLAN-KIMI-2026-08-18.md",
    "docs/archive/2026-08-audit/kimi-audit/AUDIT-REPORT-KIMI-2026-08-18.md",
    "docs/archive/2026-08-audit/kimi-audit/BATCH-B-REAUDIT-2026-08-19.md",
    "docs/archive/2026-08-audit/kimi-audit/kimi-chunk-1-result.md",
    "docs/archive/2026-08-audit/kimi-audit/kimi-chunk-1.md",
    "docs/archive/2026-08-audit/kimi-audit/kimi-chunk-2-result.md",
    "docs/archive/2026-08-audit/kimi-audit/kimi-chunk-2.md",
    "docs/archive/2026-08-audit/kimi-audit/kimi-chunk-3-result.md",
    "docs/archive/2026-08-audit/kimi-audit/kimi-chunk-3.md",
    "docs/archive/2026-08-audit/kimi-audit/kimi-chunk-4-result.md",
    "docs/archive/2026-08-audit/kimi-audit/kimi-chunk-4.md",
    "docs/archive/2026-08-audit/kimi-audit/kimi-chunk-5-result.md",
    "docs/archive/2026-08-audit/kimi-audit/kimi-chunk-5.md",
    "docs/archive/2026-08-audit/kimi-audit/kimi-chunk-6-result.md",
    "docs/archive/2026-08-audit/kimi-audit/kimi-chunk-6.md",
    "docs/archive/2026-08-audit/kimi-audit/kimi-chunk-7-result.md",
    "docs/archive/2026-08-audit/kimi-audit/kimi-chunk-7.md",
    "docs/archive/2026-08-audit/kimi-audit/kimi-chunk-8-result.md",
    "docs/archive/2026-08-audit/kimi-audit/kimi-chunk-8.md",
    "docs/archive/handoffs/HANDOFF-2026-09-23-v2.md",
    "docs/archive/handoffs/HANDOFF-2026-09-23.md",
    "docs/archive/handoffs/HANDOFF-2026-09-25.md",
    "docs/archive/review-campaigns/bug-hunt-2026-09-15/FINAL-REPORT.md",
    "docs/archive/review-campaigns/bug-hunt-2026-09-15/HANDOFF.md",
    "docs/archive/review-campaigns/bug-hunt-2026-09-15/patches/BUG-001.patch",
    "docs/archive/review-campaigns/bug-hunt-2026-09-15/patches/BUG-002.patch",
    "docs/archive/review-campaigns/bug-hunt-2026-09-15/patches/BUG-003.patch",
    "docs/archive/review-campaigns/bug-hunt-2026-09-15/reports/BUG-001.md",
    "docs/archive/review-campaigns/bug-hunt-2026-09-15/reports/BUG-002.md",
    "docs/archive/review-campaigns/bug-hunt-2026-09-15/reports/BUG-003.md",
    "docs/archive/review-campaigns/bug-hunt-2026-09-15/reports/phase1-static.md",
    "docs/archive/review-campaigns/comment-hygiene-2026-09-15/HANDOFF.md",
    "docs/archive/review-campaigns/comment-hygiene-2026-09-15/reports/B-rewrites.md",
    "docs/archive/review-campaigns/comment-hygiene-2026-09-15/reports/C-additions.md",
    "docs/archive/review-campaigns/comment-hygiene-2026-09-15/reports/phase1-inventory.md",
    "docs/batches/B6-HYGIENE.spec.md",
    "docs/batches/BT-08a.spec.md",
    "docs/evolution/DERIVABILITY.md",
    "docs/evolution/OBSERVATION_STATUS.md",
    "docs/evolution/R2_DESIGN.md",
    "docs/evolution/R6_A_DESIGN.md",
    "docs/evolution/VERIFICATION.md",
    "docs/evolution/observe-report-r6b-smoke.json",
    "docs/evolution/phase-2.md",
    "docs/rust-bot-architecture.md",
    "scripts/health-check.sh"
  ],
  "max_lines_added": 400,
  "max_lines_removed": 60,
  "findings": [],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
bash scripts/test-fast.sh   # 无 src 改动 → nothing to test / 智能跳过
ls docs/archive/ docs/evolution/
```
