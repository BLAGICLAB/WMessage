# Batch Spec: KIMI-ALIGN 主聊天提示词吸收 Kimi 行为协议

```json
{
  "batch_id": "KIMI-ALIGN",
  "family": "bot-prompt",
  "expected_files": [
    "docs/batches/KIMI-ALIGN.spec.md",
    "DEVLOG.md",
    "src-tauri/src/bot/registry.rs",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/prompts/system.rs",
    "src-tauri/tests/fixtures/tools_baseline.json"
  ],
  "max_lines_added": 60,
  "max_lines_removed": 10,
  "max_new_files_lines": 80,
  "findings": [
    {"id": "KIMI-F-1", "file": "src-tauri/src/prompts/system.rs", "line": 6, "fix": "SYSTEM_PROMPT 新增三个非编号小节（沟通风格/注入内容处理/输出格式契约，插在规则 23 与安全红线之间）；规则 14 改写为时效性判断程序。规则编号 1-16 冻结（规则 17 交叉引用），mod.rs 锚点全保留"},
    {"id": "KIMI-F-2", "file": "src-tauri/src/bot_chat.rs", "line": 58, "fix": "gen_dir_rule 追加交付文件命名规范（可读中文名，不用 report_v2/拼音/代号）——聊天与任务卡执行两路径共用"},
    {"id": "KIMI-F-3", "file": "src-tauri/src/bot/registry.rs", "line": 148, "fix": "query_single_task / web_search / complete_task 三处 schema description 补「何时不用」；JSON 结构零改动"},
    {"id": "KIMI-F-4", "file": "src-tauri/tests/fixtures/tools_baseline.json", "line": 3, "fix": "schema 描述修订后按显式流程用 tests-audit/regen_tools_baseline.py 重生成（registry_tests::tools_json_matches_baseline 锁）"},
    {"id": "KIMI-F-5", "file": "DEVLOG.md", "line": 5, "fix": "补 KIMI-ALIGN 条目：比差来源、三小节内容、约束校验（claims 句式/编号冻结/锚点）与验证结果"}
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

对照 Kimi K3 agent 系统提示词（asgeirtj/system_prompts_leaks 收录版）逐条比差，
补 wmessage 缺失的「行为协议层」：怎么说话（沟通风格）、怎么对待每轮注入的
记忆/引用块/技能清单（注入内容处理）、前端机器解析的输出格式（输出契约），
并把规则 14 从「触发词枚举」升级为时效性判断程序。工具侧三处 schema 补
「何时不用」。

硬约束三条全部校验通过：规则编号 1-16 未动（规则 17 有「按规则 1-16 处理」
交叉引用）；mod.rs 锚点（规则：、web_search、AI_Gen_Files、绝不）原样保留；
沟通风格措辞不回避「已…」句式——claims_mutation 防幻觉守卫（bot_model_loop.rs:1346）
靠它检测虚假汇报，检测面不变。

验证：cargo test 1451 单测 + 全部集成套件 0 失败（tools_baseline 漂移一次，
按 regen_tools_baseline.py 显式流程重生成）；tests-audit 对拍脚本全绿。
