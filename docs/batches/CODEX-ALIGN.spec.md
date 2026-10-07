# Batch Spec: CODEX-ALIGN 主聊天提示词吸收 Codex 分寸纪律

```json
{
  "batch_id": "CODEX-ALIGN",
  "family": "bot-prompt",
  "expected_files": [
    "docs/batches/CODEX-ALIGN.spec.md",
    "DEVLOG.md",
    "src-tauri/src/bot_plan.rs",
    "src-tauri/src/prompts/system.rs"
  ],
  "max_lines_added": 40,
  "max_lines_removed": 10,
  "max_new_files_lines": 80,
  "findings": [
    {"id": "CODEX-F-1", "file": "src-tauri/src/prompts/system.rs", "line": 6, "fix": "新增「遇到意外时」小节（三档分诊：预期不符停下问 / 无关异常不动它 / 删除覆盖先确认）——仅聊天路径，EXECUTE 无人值守循环不受影响；输出格式契约追加反倾倒三条 + 「路径:行号」引用；沟通风格追加体量适配；规则 19 句内追加批量替换例外（编号未动）"},
    {"id": "CODEX-F-2", "file": "src-tauri/src/bot_plan.rs", "line": 107, "fix": "format_plan_block 追加计划对账（每完成一步汇报进度、计划与实际不符以实际为准）——单测只锁编号渲染不锁尾部，已核对"},
    {"id": "CODEX-F-3", "file": "DEVLOG.md", "line": 5, "fix": "补 CODEX-ALIGN 条目：比差来源、四项落地、聊天/执行路径边界与验证结果"}
  ],
  "ocr_plan": {
    "rounds": 0,
    "timeout_seconds": 600,
    "expected_max_comments": 1
  },
  "stop_conditions": [
    "compile_failure",
    "gate_fail"
  ]
}
```

## 目的

对照 Codex CLI（openai/codex `codex-rs/core/gpt_5_codex_prompt.md`，GPT-5 版）比差，
吸收其「分寸感」纪律：遇到意外停下问人（带三档分诊）、不倾倒文件/命令输出、
「用户就在本机」场景自觉、回答体量适配、计划进度对账、多文件批量改动走脚本
的工具选择例外。

边界约束：「遇到意外时」只进 SYSTEM_PROMPT（聊天路径）——EXECUTE 是无人值守
50 轮循环，照抄「停下问人」会挂死批量执行；执行路径的阻塞语义已有子 agent
blocker 机制。规则编号 1-16 未动，无 schema 变更（tools_baseline 无需重生成），
claims 防幻觉句式不受影响。

验证：cargo test 1451 单测 + 全部集成套件 0 失败；tests-audit 30 passed
1 skipped（既有跳过项）。
