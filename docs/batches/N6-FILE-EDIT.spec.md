# Batch Spec: N6-FILE-EDIT agent 文件编辑工具（edit_file + write_file）

```json
{
  "batch_id": "N6-FILE-EDIT",
  "family": "bot-tools",
  "expected_files": [
    "docs/batches/N6-FILE-EDIT.spec.md",
    "DEVLOG.md",
    "docs/rust-bot-architecture.md",
    "docs/MCP-COMPUTER-USE-SETUP.md",
    "docs/skills/file-edit-best-practice/SKILL.md",
    "src-tauri/src/bot_fs.rs",
    "src-tauri/src/bot/registry.rs",
    "src-tauri/src/tool_guard.rs",
    "src-tauri/src/prompts/system.rs",
    "src-tauri/src/prompts/subagent.rs",
    "tests-audit/audit_bot_tools_alignment.py"
  ],
  "max_lines_added": 560,
  "max_lines_removed": 40,
  "findings": [
    {"id": "N6-1", "file": "src-tauri/src/bot_fs.rs", "line": 316, "fix": "可写根 ≠ 读白名单：build_writable_raw/writable_dirs = AI_Gen_Files + 任务卡绑定文件夹 + cfg.allowedDirs（不含桌面/下载/文档默认项——读可以写必须显式授权）"},
    {"id": "N6-2", "file": "src-tauri/src/bot_fs.rs", "line": 340, "fix": "resolve_writable：resolve_with_perm 写语义变体——父目录 canonical 校验（目标可不存在）、白名单外 perm_mode 三分支（ask 用 ask_user_confirm danger 每次确认不持久化；yolo 审计放行；strict 拒）"},
    {"id": "N6-3", "file": "src-tauri/src/bot_fs.rs", "line": 430, "fix": "try_apply_edit 三级匹配（Aider 模式）：精确唯一→应用 / 多处→MultiHit / 0处→空白容错（逐行 trim_end，CRLF 免疫，保留主导换行符）唯一→应用注明级别 / 全失败→NotFound+reflection 提示（先 read_text_file、注意缩进、附文件前 3 行）；不做 Levenshtein（误替换风险>收益，留档）"},
    {"id": "N6-4", "file": "src-tauri/src/bot_fs.rs", "line": 560, "fix": "edit_file：edit_file_sync 独立同步内核（≤1MB/UTF-8）+ 写闸门 + db::atomic_write 原子写回 + 变更行数摘要 + error_kind 审计（自进化经 tool.call_failed 采集）"},
    {"id": "N6-5", "file": "src-tauri/src/bot_fs.rs", "line": 640, "fix": "write_file：新建直接写；覆盖已存在 ask_user_confirm(danger)（interactive=false 自动拒 → 子 agent 只能新建不能覆盖，edit_file 无此限留档）；validate_write_content（NUL/2MB）；父目录必须已存在；atomic_write"},
    {"id": "N6-6", "file": "src-tauri/src/bot/registry.rs", "line": 316, "fix": "两 schema + 两 ToolDef（mutating=true，claims edit「已修改/已编辑」write「已写入/已创建文件」）插在 screenshot 后；计数 37→39（主可见 37）；MCP 拼装 37+1；baseline 前 28 前缀不变"},
    {"id": "N6-7", "file": "src-tauri/src/prompts/system.rs", "line": 19, "fix": "规则 19 扩展：edit_file/write_file 用法（先读后改/唯一性/容错/覆盖确认）+ use_skill 指向编辑技能模板 + 可写目录边界（编号不动）"},
    {"id": "N6-8", "file": "docs/skills/file-edit-best-practice/SKILL.md", "line": 1, "fix": "可安装技能模板：先读后改/唯一性锚点技巧/大改拆小步/改完验证闭环/失败处理（use_skill 知识型，系统零改动）"}
  ],
  "assertions_min": {
    "src-tauri/src/bot_fs.rs": 5,
    "src-tauri/src/bot/registry.rs": 2
  },
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

agent 文件工具全只读（read_text_file/grep_files/list_files），不能改代码与配置。
本批加 edit_file/write_file——业界编辑格式调研（Claude Code str_replace / Aider
多级回退 / Codex V4A / Cursor fast apply）后采用 **str_replace + Aider 三级匹配
回退**：DeepSeek/GLM 级模型对空白/缩进敏感度高，多级回退 + reflection 错误提示
把头号失败模式（空白不一致）就地消化，失败详情进审计喂自进化。

## 三大系统结合

- **自进化**：error_kind（not_found/multi_hit）+ reflection 文案 → tool.call_failed
  审计 → PREVR/record_lesson → evolution 反思统计「哪类文件编辑常失败」
- **Skills**：file-edit-best-practice SKILL.md 模板（use_skill 知识型，系统零改动）
- **MCP 边界**：文件编辑必须原生（写闸门在宿主侧不可被外部 MCP 绕过），指南补节

## 出界（留档）

Levenshtein 模糊匹配（误替换>收益）；V4A 多 hunk（格式特训依赖）；fast apply
（需微调模型）；独立写白名单持久化（Always 走既有 allowed_dirs）；run_python
写旁路硬拦截（维持软闸+审计）；RESEARCH 档不加写工具；replaceAll 参数（多处
命中报错引导加上下文）；前端零改动
