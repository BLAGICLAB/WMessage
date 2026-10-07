# Batch Spec: W11-REVIEW-HARDEN

## 目的

W10 复审后两项后续：①**轻量评审模型**（设计 §3.6 后置项）——拆解前澄清与节点级验收核查
共用一个可配置的模型库条目（省 token/提速），缺省跟随全局 active；②**导出路径统一加固**
（OCR r1 high 遗留共性面）——`check_export_path` 单点强化，全部导出/导入调用方受益。

## 人类可读摘要

- family: workflow-canvas
- 预估 diff: ~9 files / +330/-30 lines
- OCR 计划: r1, timeout 1800s, comments ≤ 4（重点：模型条目缺失降级跟随全局不报错、
  加固不破坏既有 save dialog UX、import 路径同样过闸）

## 设计

1. **summarize 模型覆盖**（bot_chat.rs）：`summarize_messages_with_model(app, prompt, msgs,
   model_id: Option<&str>)`——model_id 按条目 id 在 models_by_provider 双列表查找（ModelEntry
   自足：base_url/model 随条目）；key 走 `read_llm_key(provider, 合成 ActiveModelId, mbp)`
   （厂商级 key 优先回落全局，既有口径）；推理参数 `effective_inference` 同口径解析。
   **条目不存在/已停用/base_url 空 → 静默降级跟随全局**（评审是增强，模型配置错误不挡主流程）。
2. **设置键**（workflow_settings.rs）：`review_model`（条目 id，空串=跟随全局）；
   `WorkflowSettingsView`/`settings_set` +`review_model`（None=不改/Some("")=清除/Some(id)=设置，
   不校验条目存在——条目可后删，运行期降级兜底）；`review_model_id(conn) -> Option<String>`。
3. **接入点**：`workflow_clarify` 与 runner `check_acceptance` 各自 spawn_blocking 读一次设置，
   传覆盖调用（不穿参数层——两层各自读，避免 run_controller 参数继续膨胀）。
4. **导出路径加固**（tasks.rs `check_export_path` 单点强化，workspace/workflow/tasks/audit
   五个调用方自动受益）：trim 非空 → **词法拒 `..` 组件** → 扩展名 .json（大小写不敏感，既有）→
   文件名合法（非 . / .. / 空）→ **父目录必须存在且为目录**（canonicalize 解析软链）→
   **目标已存在时拒符号链**（symlink_metadata fail-closed，同 bot_fs resolve_writable 口径；
   import 侧同样过闸——符号链文件罕见，报错信息可解释）。
5. **前端**：设置页「验收与审计」段加「轻量评审模型」下拉（bot_get_config 模型库条目，
   停用条目不进列表，与 WorkflowPage 下拉同过滤；空选项 = 跟随全局模型）；types/lib 同步。

## 红线

- 评审模型条目缺失/停用/解析失败 → 静默降级跟随全局，绝不报错挡 clarify/验收
- check_export_path 是唯一闸门：五调用方（workspace/tasks/workflow 导出导入/审计导出）不各自为政
- 加固不改变 save dialog 交互（用户仍任选目录）；拦截的是 invoke 直达路径的 `..`/软链/悬空父目录
- 设置键空串语义 = 跟随全局（用户显式清除）；键缺失 = 默认跟随全局

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W11-REVIEW-HARDEN",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/batches/W11-REVIEW-HARDEN.spec.md",
    "DEVLOG.md",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/workflow_clarify.rs",
    "src-tauri/src/workflow_runner.rs",
    "src-tauri/src/db/workflow_settings.rs",
    "src-tauri/src/db/tasks.rs",
    "src-tauri/src/db/mod.rs",
    "src/lib/workflowAudit.ts",
    "src/components/SettingsPage/SettingsPage.tsx"
  ],
  "max_lines_added": 420,
  "max_lines_removed": 40,
  "findings": [
    {"id": "W11-1", "file": "src-tauri/src/bot_chat.rs", "line": 1203, "fix": "summarize_messages 重构为 inner(model_id) + summarize_messages_with_model（条目查找/降级/合成 ActiveModelId）"},
    {"id": "W11-2", "file": "src-tauri/src/db/workflow_settings.rs", "line": 1, "fix": "KEY_REVIEW_MODEL + review_model_id + view/set +review_model"},
    {"id": "W11-3", "file": "src-tauri/src/workflow_clarify.rs", "line": 1, "fix": "读设置传模型覆盖（spawn_blocking 读，降级 None）"},
    {"id": "W11-4", "file": "src-tauri/src/workflow_runner.rs", "line": 1, "fix": "check_acceptance 读设置传模型覆盖"},
    {"id": "W11-5", "file": "src-tauri/src/db/tasks.rs", "line": 1187, "fix": "check_export_path 加固：..词法拒/文件名合法/父目录存在canonicalize/目标符号链拒（spec 对齐：单测落 db/mod.rs 既有测试区，见 W11-9）"},
    {"id": "W11-9", "file": "src-tauri/src/db/mod.rs", "line": 2258, "fix": "check_export_path 加固单测（..词法/悬空父目录/符号链目标拒/软链父目录放行/新文件放行；spec 对齐：check_export_path 测试区在 mod.rs）"},
    {"id": "W11-7", "file": "src/lib/workflowAudit.ts", "line": 1, "fix": "WorkflowSettings +reviewModel + set 透传 reviewModel（类型与 invoke 同文件）"},
    {"id": "W11-8", "file": "src/components/SettingsPage/SettingsPage.tsx", "line": 1, "fix": "轻量评审模型下拉（bot_get_config 条目，停用过滤，空=跟随全局）"}
  ],
  "assertions_min": {
    "src-tauri/src/db/tasks.rs": 3,
    "src-tauri/src/bot_chat.rs": 2
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
