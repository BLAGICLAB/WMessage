# Batch Spec: <BATCH_ID>

## 目的

<一句话：这批修什么>

## 人类可读摘要

- family: <family-name>
- 覆盖 findings: <N>
- 预估 diff: <F> files / +<A>/-<R> lines
- OCR 计划: r<rounds>, timeout <seconds>s, 期望 comments ≤ <max>

## 红线

- family 一致性：本批只含 <family>，不混入其他 family
- 0 FP / 0 family 异质
- 编译层事实必须核实，不许凭印象

## spec 起草后自查三条（APW-02a 2026-09-23 立）

1. `expected_files` 是否覆盖全部写入路径（含签名 ripple 的 caller + 新测试文件）
2. budget 是 A 类还是 B 类？两类公式分开算？（见 PHASE2-TRIAGE.md §5 “budget 估算法 SOP”）
3. findings 逐条 fix 字段是否显式列出 ripple 的文件 + 行号？

自查未过不许发审。补完再审。

## 自主执行规则

spec 被 reviewer 批准后:
- agent 全权执行至 commit,不中途报 status / exit / diff
- 例外只有 stop 条件触发
- 完成时一次报: hash + 批定性 + OCR 轮次 + comments 处置 + follow-up
- 过程证据落 ~/.openclaw/cache/<batch-id>/,不喊人

## Stop 条件(触发即停,报 reviewer)

- compile_failure (无 X 形态退路)
- architecture_blocker (签名/调用点超预算)
- family_heterogeneity
- new_high_different_root (OCR r1 出的 high 非本批根因)
- gate_fail (batch-verify FAIL 且非 spec 声明调整可解)

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "<BATCH_ID>",
  "family": "<family-name>",
  "expected_files": [
    "<path/to/file1>"
  ],
  "max_lines_added": 10,
  "max_lines_removed": 5,
  "findings": [
    {"id": "<finding-id>", "file": "<path>", "line": 128, "fix": "<一句话>"}
  ],
  "assertions_min": {
    "<path/to/test/file>": 3
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 5
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

提交信息骨架

```
<type>(<scope>): <BATCH_ID> — <family>

【family】<一句话定义>
【实修 <N> 处】
- <file:line>：<改前 → 改后>
【自测】cargo fmt / cargo check --tests / scripts/test-all.sh 全绿
【OCR】r<N>：exit <code> / <comments> comments / <elapsed>
【基线】<baseline note>
```

per-command timeout 说明

batch-verify.py 的 `check_command` 支持 per-command timeout（默认：cargo fmt 60s / cargo check 180s / test-all.sh 600s）。
实现：subprocess.run(..., timeout=X)——Python 标准库自动 kill 子进程组，不会留孤儿。
新批若调用 check_command 加新工具，按"短工具 60s / 长工具 600s+ 估"配置。

验证命令

```bash
python3 scripts/batch-verify.py docs/batches/<BATCH_ID>.spec.md --tests
```