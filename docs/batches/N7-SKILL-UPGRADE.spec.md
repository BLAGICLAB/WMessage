# Batch Spec: N7-SKILL-UPGRADE 技能系统四点升级 + Agent Skills 开放标准对齐

```json
{
  "batch_id": "N7-SKILL-UPGRADE",
  "family": "bot-skills",
  "expected_files": [
    "docs/batches/N7-SKILL-UPGRADE.spec.md",
    "DEVLOG.md",
    "docs/rust-bot-architecture.md",
    "docs/MCP-COMPUTER-USE-SETUP.md",
    "src-tauri/src/bot_skills/parse.rs",
    "src-tauri/src/bot_skills/state.rs",
    "src-tauri/src/bot_skills/manage.rs",
    "src-tauri/src/bot_skills/runtime.rs",
    "src-tauri/src/bot_skills/scheduler.rs",
    "src-tauri/src/bot_skills/vars.rs",
    "src-tauri/src/bot_skills/recommend.rs",
    "src-tauri/src/bot_fs.rs",
    "src-tauri/src/bot/registry.rs",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/tests/fixtures/minimax-ppt/SKILL.md",
    "src-tauri/tests/skill_e2e.rs",
    "src/components/SettingsPage/SkillsPanel.tsx",
    "src/components/SettingsPage/types.ts"
  ],
  "max_lines_added": 700,
  "max_lines_removed": 80,
  "findings": [
    {"id": "N7-1", "file": "src-tauri/src/bot_skills/parse.rs", "line": 316, "fix": "①兼容审计：unknown_tool_names 纯函数（steps+rollback vs 注册表）；SkillInfo.unknown_tools（scan 时算）+ 运行期 skill.compat_warn 审计 + 全未知步骤提前 Terminated；use_skill 返回头部兼容警告（MCP 场景注记）"},
    {"id": "N7-2", "file": "src-tauri/src/bot_skills/recommend.rs", "line": 1, "fix": "②语义化推荐：rank_skills（embed_text + tag_similar::cosine 排序截断 top5，embed 注入式可测，降级原样）；build_skill_block_for(app, query) async 化（spawn_blocking 纪律），bot_chat 两调用点接 query"},
    {"id": "N7-3", "file": "src-tauri/src/bot_skills/parse.rs", "line": 130, "fix": "③参数契约：params frontmatter 多行列表（- key: 说明（必填）/（默认 X））→ SkillParam{key,desc,required,default}；use_skill schema +params 对象；start_skill 必填缺失拒绝（教学化列表）+ 默认回填；vars ${params.key} 替换（substitute_vars_with_params）"},
    {"id": "N7-4", "file": "src-tauri/src/bot_skills/runtime.rs", "line": 470, "fix": "④失败回流：sink_skill_failure_lesson（record_lesson_core 直沉淀 kind=lesson scenario=skill:{name}）——runtime skill_finish Failed 与 scheduler failed_recoverable 双接入；不建新表不改 post_consolidation 签名"},
    {"id": "N7-5", "file": "src-tauri/src/bot_skills/parse.rs", "line": 360, "fix": "⑤allowed-tools（agentskills.io 字段）：frontmatter 解析 + SkillRun 镜像 + 调度器步骤双闸（声明了才限制；违规走回滚+FailedButRecoverable+skill.allowed_tools_violation 审计）"},
    {"id": "N7-6", "file": "src-tauri/src/bot_skills/runtime.rs", "line": 150, "fix": "⑥第三层披露：use_skill 返回尾部列 references/*.md 绝对路径；allowed_dirs 会话级放行活动技能目录（SkillRun.dir 新字段，load_skill_meta 返回三元组）"},
    {"id": "N7-7", "file": "src-tauri/src/bot_skills/manage.rs", "line": 8, "fix": "⑦version 字段：frontmatter 解析 → SkillInfo.version → 设置页展示 + use_skill 头部"},
    {"id": "N7-8", "file": "src/components/SettingsPage/SkillsPanel.tsx", "line": 142, "fix": "前端：未知工具技能名标红（title 提示清单）+ 版本展示；types.ts SkillInfo 同形"}
  ],
  "assertions_min": {
    "src-tauri/src/bot_skills/parse.rs": 4,
    "src-tauri/src/bot_skills/vars.rs": 3,
    "src-tauri/src/bot_skills/recommend.rs": 3
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

技能系统四点升级（兼容审计/语义推荐/参数契约/数据回流）+ Agent Skills 开放标准
（agentskills.io）对齐（allowed-tools/version/第三层披露/description 规范）。
调研结论：技能自生成（Voyager）不做——EVOMAL(2026) 自投毒风险，「人工确认安装」
即抗性设计。

## 附带修复

- minimax-ppt fixture 的 `list_tasks` 是 T1 合并时的真实遗留失效——兼容审计上线
  当场抓获（4 个 e2e 全部 Terminated），fixture 更新为 query_tasks、e2e 断言同步

## 出界（留档）

Levenshtein 模糊匹配；MCP 工具名静态解析（运行期注记）；语义推荐接管 IntentRule；
参数自动抽取；skill_step_events 新表；post_consolidation 签名；前端大改
（仅 SkillsPanel 标红+版本两处）；技能自生成（EVOMAL 投毒风险）
