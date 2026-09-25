# Batch Spec: META-1

## 目的

工具链 batch：把 batch-verify gate 升级（worktree_dirty → fail）+ 修 OCR r1 报的 5 条 hook 真 finding + 落地 hook + verify 脚本版本化（5 文件同 commit）。本批是 C5-AP-06 收口的 meta-tooling 后续。

## 人类可读摘要

- family: tooling（独立类目，不属四个代码 family 中任何一个）
- 覆盖 findings: 6（5 hook OCR + 1 batch-verify 升级）
- 预估 diff: 5 files / +50/-25 lines
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 10

## 红线

- family 一致性：本批只含 tooling 类目，不混入其他代码 family
- 0 FP / 0 family 异质
- 本批 走 旧 batch-verify gate（gate 自身在 META-1 里改，新 gate 下一批生效）
- 本批 commit 期间 worktree 不许 git clean（untracked 文件含 batch-verify.py，丢了 gate 断）

## Stop 条件（遇到即停，报 reviewer）

- worktree 未清（除 staged 外有未 stage 修改）— OCR 前必须清
- pre-commit hook gate fail 且非本批 spec 声明的 4 files
- OCR 新 high 非同根因
- 三层自测 fail（batch-verify.py / install-git-hook.sh 改动不能破坏 hook 流程）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "META-1",
  "family": "tooling",
  "expected_files": [
    "scripts/batch-verify.py",
    "scripts/install-git-hook.sh",
    "docs/BATCH-SPEC-TEMPLATE.md",
    ".githooks/pre-commit"
  ],
  "max_lines_added": 60,
  "max_lines_removed": 25,
  "max_new_files_lines": 500,
  "findings": [
    {"id": "META-1.1", "file": "scripts/batch-verify.py", "fix": "worktree_dirty warning → fail + 新增 check_worktree_clean（强制一批一清）"},
    {"id": "META-1.2", "file": "scripts/batch-verify.py", "fix": "check_command 支持 per-command timeout；subprocess.run(..., timeout=X) Python 标准库自动 kill 子进程组（cargo fmt 60s / cargo check 180s / test-all.sh 600s）"},
    {"id": "META-1.3", "file": "scripts/install-git-hook.sh", "line": "HOOK_TEMPLATE", "fix": "git diff --cached exit code 检查（1.6 之外的独立防线：防御 repo 损坏、index lock、submodule 状态等 1.6 兜不住的场景）"},
    {"id": "META-1.4", "file": "scripts/install-git-hook.sh", "line": "HOOK_TEMPLATE", "fix": "command -v python3 前置核"},
    {"id": "META-1.5", "file": "scripts/install-git-hook.sh", "line": "HOOK_TEMPLATE", "fix": "! -d 单独报错（区分 file missing vs directory）"},
    {"id": "META-1.6", "file": "scripts/install-git-hook.sh", "line": "HOOK_TEMPLATE", "fix": "hook 开头 cd toplevel 兜底"},
    {"id": "META-1.7", "file": "scripts/batch-verify.py", "fix": "line_budget 拆为 modified-only (M files) + 新 check_new_files_budget (A files)；spec 加 max_new_files_lines 字段（新文件首次纳入场景）"},
    {"id": "META-1.8", "file": "scripts/batch-verify.py", "fix": "加 --quiet flag; install-git-hook.sh 调用改 --quiet"}
  ],
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 8
  },
  "stop_conditions": [
    "compile_failure",
    "architecture_blocker",
    "family_heterogeneity",
    "new_high_different_root",
    "worktree_dirty_before_ocr"
  ]
}
```

提交信息骨架

```
chore(tooling): META-1 — 工具链 batch-verify 升级 + 5 条 hook 真 finding 修

【scope】6 项
- batch-verify: worktree_dirty warn → fail + check_worktree_clean
- hook: timeout 60 / git exit code / python3 前置 / ! -d 单独报错 / cd toplevel 兜底

【鸡生蛋】META-1 走旧 gate（改后的版本还未 commit）
META-1 commit 后新 gate 生效，下一批用

【自测】bash scripts/install-git-hook.sh（双跑 md5 一致）
OCR r1：扫描 worktree（含 staged + META-1 5 files）
batch-verify：META-1.spec.md 验证（file_set + line_budget + findings_files_in_diff + check_worktree_clean）

【family】tooling（独立类目）
【红线】5 文件同 commit 落地 / worktree 期间不许 git clean
```

验证命令

```bash
BATCH_SPEC=docs/batches/META-1.spec.md python3 scripts/batch-verify.py docs/batches/META-1.spec.md
```