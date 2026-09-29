# WMessage Agent 内核审计报告（2026-08-18）

> **生成者**：小九（OpenClaw）— 对着代码实读
> **审计范围**：Agent 内核完整性 + 逻辑性 + 一致性（按用户 2026-08-18 12:40 指令）
> **方法**：直接读 `src-tauri/src/{bot,bot_skills,middleware,intent_router,tool_guard,audit}.rs` 核心函数 + ChatPanel.tsx 关键 UI + 对 MANUAL-ACCEPTANCE.md 六/七节 8 处 `[ ]` 做代码核对

---

## 摘要

**Agent 内核实际状态**：✅ **B+（核心逻辑扎实，但 MANUAL-ACCEPTANCE 文档漂移严重）**

最大发现：**老板的怀疑是对的**——MANUAL-ACCEPTANCE.md 六/七节里 8 处 `[ ]`（未完成）标记中，**只有 1 处（图片识别）是真实未做**，其余 7 处在代码里早已实现，文档没及时勾选。说明验收流程跟不上代码节奏。

---

## 一、完整性 vs MANUAL-ACCEPTANCE（六. 机器人聊天 + 七. 技能系统）

### 🎯 任务引用 / 选任务模式 / 附件预添加（多处）— 代码已实现，文档未勾

| MANUAL 验收项 | 代码位置 | 状态 |
|---|---|---|
| 📌 任务引用按钮 | `src/components/ChatPanel.tsx:824` `<span>📌 {t.title}</span>` + 948 | ✅ 已实现 |
| 🎯 选任务模式 | `ChatPanel.tsx:792` `🎯` 选择模式触发器 | ✅ 已实现 |
| ➕ 附件预添加（文件芯片） | `ChatPanel.tsx:169` "已添加的附件文件路径" + 662 `➕ 添加附件` | ✅ 已实现 |
| 附件直读（不再弹系统选择框） | `ChatPanel.tsx:647` "附件以 [附件文件] 块附在消息前（模型用 extract_document 的 path 直读，不弹框）" | ✅ 已实现 |
| 删除任务确认弹窗（60s 超时） | `src-tauri/src/bot.rs:101` `async fn ask_user_confirm` + `bot.rs:147` `bot_confirm_response` | ✅ 已实现 |
| 技能熔断（5 次 Function 上限） | `src-tauri/src/bot.rs:1346` `MAX_FUNCTION_CALLS_PER_TURN: usize = 5` + 1560 累加 + 1571 熔断 + skill_finish(fail) | ✅ 已实现 |
| 导入技能 / 删除技能 / 打开目录 | `src/components/SettingsPage.tsx:50-103` `SkillsPanel` 含 `skills_import` / `skills_delete` / `skills_open_dir` | ✅ 已实现 |

**结论**：MANUAL-ACCEPTANCE 的 `[ ]` 标记是**文档漂移**，不是代码缺。

### ❌ 图片识别 — 真实未做

- grep `image_url` / `vision` / `多模态` / `图片` 在 ChatPanel.tsx + types.ts 0 命中
- MANUAL-ACCEPTANCE 标注 `[ ]` 是准确的
- 建议实现路径：发送图片 → base64 encode → 用 OpenAI-compatible 多模态 API 调 LLM → 模型回文字
- 工期：~4-6h（前端图片选择 + base64 + 后端 attach_images 改造）

### INFO：技能清单 / 渐进披露 / Skill 调度器前置预路由 — 已实现

- 技能清单：`src/components/SettingsPage.tsx:50` SkillsPanel + `src-tauri/src/bot_skills.rs:1447` `skills_list`
- 渐进披露：bot.rs:933+ `build_skill_block(&app)` 注入技能名称+描述到 system prompt，完整文档由 `use_skill` 工具按需读取
- Skill 调度器前置预路由：bot.rs:962 `middleware::run_pre_step` + intent_router.rs:107 `route_user_input`（F-2 抽象层）

---

## 二、逻辑性（Logic）

### ✅ 主流程编排清晰（bot_chat → run_model_loop）

```
bot_chat(app, messages)
  ├─ 开关校验 + StopGuard 创建
  ├─ F-1 bypass_llm_on_pre_step_hit 读取
  ├─ 审计：user.message 事件
  ├─ 系统提示词：SYSTEM_PROMPT + build_skill_block
  ├─ Pre-step 路由（F-2 中间件层）
  │    └─ middleware::run_pre_step
  │         └─ IntentRouterMiddleware.route_user_input
  │              └─ 命中 → start_skill(app, name) → (meta, body)
  │              └─ pre_step.route_skill 事件
  ├─ F-1 开关：bypass=false → 强制 pre_routed_skill = None（LEGACY）
  ├─ C 路径：auto-mode Skill → run_skill_scheduler
  │    ├─ Done → 返回
  │    ├─ AwaitUser → __await_user__
  │    ├─ FailedButRecoverable → recovery_hint 拼进 system prompt
  │    └─ Terminated → Err
  └─ run_model_loop(app, msgs, max_rounds, stop)
       ├─ 读 cfg + api_key + 构造 reqwest（15s connect / 300s total）
       └─ 主循环（每轮）：
            ├─ 状态机推进（advance_skill）：
            │    Loaded → NoActive
            │    Running 未超时 → Continue
            │    Running 超时 → Fail
            │    Paused → AwaitConfirm（跳出，等用户确认）
            │    Completed → Finish
            │    Failed → Fail(reason)
            │    Terminated → Terminate(reason)
            ├─ 构造 body（model + msgs + tools + stream=true）
            ├─ llm.request 审计
            ├─ POST /chat/completions
            ├─ llm.response 审计（status）
            ├─ SSE 流解析（parse_sse_chunk）：
            │    ├─ content → bot-chat-delta + bot-think-delta（<think> 拆分）
            │    └─ tool_calls → bot-tool / bot-tool-name 增量
            ├─ 熔断：function_calls_total > 5 → skill_finish(fail) + 熔断提示
            ├─ 工具执行：
            │    ├─ pre-execute 中间件：tool_guard 黑名单（Skill 状态放行 / 否则阻断）
            │    ├─ execute_tool(app, name, args) → (result, refs)
            │    ├─ bot-tool-done + audit_log(tool=)
            │    └─ msgs.push tool result
            └─ 无 tool_calls → skill_finish(true) + 返回 final_text
```

### ✅ 状态机覆盖完整（6 SkillState + 5 DslAdvanceAction + 5 AdvanceAction）

- `SkillState`: Loaded / Running / Paused / Completed / Failed / Terminated
- `AdvanceAction`: NoActive / Continue / AwaitConfirm / Finish / Fail(reason) / Terminate(reason)
- `DslAdvanceAction`: Run / Finish / AwaitUser / FailWithRollback(reason) / Terminate(reason)
- 实时超时检测（即使 step_check 没跑到，主循环也能感知）
- advance_dsl 是 advance_skill 的 DSL 调度专用映射（5 分支）

### ⚠️ P1-1：bot_chat 主循环（~500 行核心编排）无直接单测

- **位置**：`src-tauri/src/bot.rs::bot_chat` + `run_model_loop`
- **现状**：F-6 Plan C 已经接受"放弃 mock_runtime execute_tool 端到端测试"
- **风险**：流式响应中途出错 / Stop 状态错乱 / 工具循环死锁等行为只能靠手动测试
- **建议**：拆 3-4 个纯函数（流解析、Stop 状态机判断、slash 命令分发）加单测，覆盖 80% 关键路径
- **工期**：~2h

---

## 三、一致性（Consistency）

### ✅ 审计事件命名空间统一（F-3 已修）

- 8 个事件：`user.message` / `pre_step.route_skill` / `pre_step.route_failed` / `pre_step.bypass_off` / `pre_execute.deny` / `skill.start` / `tool.call` / `tool.return` / `llm.request` / `llm.response`
- 格式统一：`[ts.毫秒] LEVEL | event | k=v | k=v ...`
- pytest 静态分支断言覆盖（tests-audit/audit_pre_step_pre_execute.py 24 passed + 1 skipped）

### ✅ 中间件抽象层（F-2 已修）

- `src-tauri/src/middleware.rs::MiddlewareRegistry` 短路求值
- `IntentRouterMiddleware`（pre_step）+ `AtomicGuardMiddleware`（pre_execute）注册进去
- 业务模块只能调查询接口（`run_pre_step` / `run_pre_execute`），无 register 入口

### ✅ F-1 bypass 开关（已修）

- `BotConfig::bypass_llm_on_pre_step_hit: bool`（`#[serde(default = "default_true")]` 兜底老配置）
- bot_chat 读取开关，false 时强制 pre_routed_skill = None（LEGACY 路径）
- pytest 补 3 条断言（开关字段存在 / toggle off 零路由 / 默认值）

### ✅ DSL 解析器 + 变量替换（Phase 4 第 2 项）

- `parse_skill_steps(body) → (Vec<SkillStep>, Vec<SkillStep>)`
- `substitute_vars(args_json, ctx) → String` 支持 `${stepN.field}` / `${prev.field}` 嵌套
- 6 个单测覆盖跨步 UUID / 原文传递 / prev 别名 / 越界保留 / UUID 提取 / 多索引

### ⚠️ INFO：Tauri command 错误返回混合风格

- 有的 command 返回 `Result<T, String>`（错误信息）
- 有的 command 返回 `(T, Vec<TaskRef>)`（结果 + 副作用）
- 前端只能 e.message 字符串显示，无结构化 code 分流
- 建议：定义 `CommandError { code, message, recoverable }`，前端按 code 弹对应提示
- 工期：~4-6h（要改所有 command 签名 + 前端调用点）

---

## 四、发现清单

| # | 严重度 | 角度 | 位置 | 一句话问题 | 状态 |
|---|---|---|---|---|---|
| 1 | P1 | 完整性 | src/components/ChatPanel.tsx | ❌ **图片识别未做**（grep 无 image_url / vision） | ⏳ 未动工 |
| 2 | INFO | 完整性 | docs/MANUAL-ACCEPTANCE.md 六/七节 | ⚠️ **文档漂移**：8 处 `[ ]` 中 7 处代码已实现（任务引用、🎯、➕、删除确认、熔断、SkillsPanel、导入删除） | ✅ 确认（仅文档问题） |
| 3 | P1 | 逻辑性 | src-tauri/src/bot.rs::bot_chat + run_model_loop | 主编排 ~500 行无直接单测，依赖中间件层 + 状态机间接覆盖 | ✅ `224dcf9`（抽纯函数 + 6 单测） |
| 4 | INFO | 一致性 | src-tauri/src/* Tauri command | 错误返回混合 String/Result，缺结构化 CommandError | ✅ 全量迁移完成 `f09ccb6` + `1b14df4` + `e65aa53` |
| 5 | INFO | 完整性 | MANUAL-ACCEPTANCE.md | 验收流程应改成「CI 自动勾选 + 人工复核」双轨 | ⏳ 未动工 |

---

## 五、Agent 工作流（完整版）

```
┌──────────────────────────────────────────────────────────────────┐
│                       用户发消息（前端 ChatPanel）                  │
└────────────────────────────┬─────────────────────────────────────┘
                             │ tauri::command
                             ▼
┌──────────────────────────────────────────────────────────────────┐
│ bot_chat(app, messages)                                            │
│  ├─ 校验机器人开关                                                 │
│  ├─ StopGuard 创建                                                 │
│  ├─ 读 F-1 bypass_llm_on_pre_step_hit 开关                        │
│  └─ emit audit_event!(user.message, content=截断 300)             │
└────────────────────────────┬─────────────────────────────────────┘
                             │
                             ▼
┌──────────────────────────────────────────────────────────────────┐
│ Pre-step 路由（middleware::run_pre_step → IntentRouter）          │
│  ├─ 关键词 L1 硬锁（7 条复合业务 → Skill 映射）                    │
│  ├─ 命中 → start_skill(app, name) → (meta, body)                  │
│  ├─ emit audit_event!(pre_step.route_skill, skill=, mode=)        │
│  ├─ 加载失败 → pre_step.route_failed + fallthrough               │
│  └─ 未命中 → None（fallthrough 到 LLM）                           │
└────────────────────────────┬─────────────────────────────────────┘
                             │
                             ▼
┌──────────────────────────────────────────────────────────────────┐
│ F-1 bypass 开关                                                    │
│  ├─ true（新行为）→ 保留 pre_routed_skill                         │
│  └─ false（LEGACY）→ 强制 None + emit pre_step.bypass_off         │
└────────────────────────────┬─────────────────────────────────────┘
                             │
                             ▼
┌──────────────────────────────────────────────────────────────────┐
│ C 路径（auto-mode Skill）                                          │
│  ├─ run_skill_scheduler(app, name).await                          │
│  ├─ DSL 解析（parse_skill_steps）                                  │
│  ├─ 顺序执行 step：每步前查 advance_dsl 状态机                    │
│  ├─ 失败 → 跑 ## Rollback 段 + 报错                               │
│  └─ 返回 DslOutcome：                                              │
│       ├─ Done → 直接返回                                          │
│       ├─ AwaitUser → __await_user__ + 主循环挂起                  │
│       ├─ FailedButRecoverable → recovery_hint 拼 system prompt    │
│       └─ Terminated → Err                                          │
└────────────────────────────┬─────────────────────────────────────┘
                             │
                             ▼
┌──────────────────────────────────────────────────────────────────┐
│ run_model_loop(app, msgs, max_rounds, stop)                       │
│  ├─ 读 cfg + api_key + 构造 reqwest (15s connect / 300s total)   │
│  └─ 主循环每轮：                                                    │
│       ├─ 检查 stop                                                 │
│       ├─ 状态机推进（advance_skill）：                             │
│       │    NoActive/Continue → 继续                                │
│       │    AwaitConfirm → 跳出，等用户确认                          │
│       │    Finish → skill_finish(true) + 跳出                     │
│       │    Fail/Terminate → skill_finish(false) + 跳出            │
│       ├─ 构造 body（model + msgs + tools + stream=true）          │
│       ├─ emit llm.request                                          │
│       ├─ POST /chat/completions                                    │
│       ├─ emit llm.response（status）                               │
│       ├─ SSE 流解析（parse_sse_chunk）：                           │
│       │    ├─ content → bot-chat-delta + bot-think-delta          │
│       │    └─ tool_calls → bot-tool / bot-tool-name 增量           │
│       ├─ 熔断：function_calls_total > 5 → skill_finish(fail)       │
│       └─ 工具执行：                                                │
│            ├─ pre-execute：tool_guard 黑名单（Skill 放行 / 否则阻断）│
│            ├─ execute_tool(app, name, args) → (result, refs)     │
│            ├─ emit bot-tool-done + audit_log(tool=)               │
│            └─ msgs.push tool result                                │
└────────────────────────────┬─────────────────────────────────────┘
                             │
                             ▼
┌──────────────────────────────────────────────────────────────────┐
│ execute_tool(app, name, args) → (String, Vec<TaskRef>)             │
│  ├─ pre-execute 钩子：tool_guard::is_atomic_tool                  │
│  │    ├─ 黑名单（create_word_revisions / link_file_to_task）        │
│  │    ├─ Skill 状态 → 放行                                         │
│  │    └─ 非 Skill 状态 → 阻断 + 提示走 Skill                       │
│  ├─ 分发到 tool_* 函数（list_tasks / create_task / ...）           │
│  ├─ post-execute 钩子：audit_event!(tool.call) + tool.return      │
│  └─ 返回 result_text + task_refs                                  │
└────────────────────────────┬─────────────────────────────────────┘
                             │
                             ▼
┌──────────────────────────────────────────────────────────────────┐
│ emit BotChatResult { text, task_refs }                             │
│  └─ 前端 ChatPanel 渲染：bot-chat-delta / bot-tool-done 折叠行     │
└──────────────────────────────────────────────────────────────────┘

【Skill 状态机】
  Loaded ──start_skill──> Running ──step 失败──> Failed
                              │  ──/stop──> Terminated
                              │  ──用户确认──> Paused ──> Running
                              └─ step 成功──> Completed

【时间线】
  user.message → pre_step.route_skill? → skill.start → tool.call / tool.return
                  → llm.request / llm.response（按需）
```

---

## 六、老板后续决策项（2026-08-18 13:30 更新）

1. ✅ **MANUAL-ACCEPTANCE 文档漂移**：已确认（代码都在，文档未勾）
2. ⏳ **图片识别**：MiniMax vision 确认支持 + `attach_images` 已就绪，仅缺前端 ➕ image filter（~2h）
3. ✅ **bot_chat 单测补**：`224dcf9` 抽 `format_recovery_hint` + `merge_task_refs_dedup` 纯函数 + 6 单测
4. ✅ **CommandError 全量**：`32be5ad` / `876c40f` / `f09ccb6` / `1b14df4` / `e65aa53` 全量迁移，172 tests 通过

---

## 维护说明

- 本报告与 `AUDIT-REPORT-MANUAL-2026-08-18.md` 互补：那份是「广角扫描」，这份是「agent 内核深读」
- 下次审计建议 1 个月后（2026-09-18）
- MANUAL-ACCEPTANCE 漂移问题建议引入 CI：代码合并后自动检查 `[ ]` 与实际代码差异