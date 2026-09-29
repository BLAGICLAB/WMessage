# Rust Bot 全面审计 · 批次 7：前后端契约（2026-08-28）

范围：`src/`（ChatPanel / WidgetApp / SettingsPage / MigrationPanel / TodoCard / App 及
lib 封装层）↔ `src-tauri/src` 全部 58 个 `#[tauri::command]` 与 12 个 emit 事件、
22 个 CommandError code。三个并行 explore 子代理分别提取「Rust 命令面 / Rust 事件与
错误面 / 前端调用与监听面」，全部 P1/P2 发现由主代理逐条源码复核（本报告行号均已核对）。

## 结论

**无 P0。** 主干契约健康：

- **命令面 58/58 一致**：定义即注册（lib.rs:429-488 机器比对零差异），前端全部
  invoke 的命令名、参数名（camelCase 默认转换：`sessionId`/`taskId`/`requestId`/
  `isDir`/`apiKey`）、返回类型消费方式均与 Rust 签名吻合。无孤儿命令、无悬空注册、
  无参数名写错。
- **事件面全闭环**：Rust 发射的 12 个事件（bot-chat-delta / bot-think-delta /
  bot-tool / bot-tool-name / bot-tool-done / bot-confirm / bot-skill-failed /
  tasks-changed / tasks-updated / profile-changed / quick-add / toggle-theme）全部
  有前端监听方且 payload 字段逐一对上；前端互发的 8 个事件（workspace-updated /
  workspace-changed / edit-task / execute-task / bot-changed / bot-config-changed /
  tasks-changed / tasks-updated）走 Tauri 全局广播，Rust 侧零 listen（grep 为零），
  属设计如此的窗口间总线。`tasks-updated` 的 api/migration 源只 emit 给 main，
  主窗合并后统一 `emit("tasks-changed")`（App.tsx:233）转发挂件重读——链路口径一致。
- **错误面**：22 个 code 中 21 个在 `hintForCode`（errorHandler.ts:42-84）有显式
  提示，default 分支只缺提示不吞错（仍 alert message，P2-35 兜底链完整）。
- 事件 payload 不含 API key/token；`bot-chat-delta` 等六个流式事件均经
  `emit_stream` 收口注入 sessionId 且仅交互实例发射（bot_model_loop.rs:540-546），
  与前端按会话过滤的消费方式（ChatPanel.tsx:423-553）互洽。

## P1（处理情况）

### P1-1 ChatPanel 模型切换静默抹掉授权模式 permMode（已修）
- 证据：挂件头部 🧠 菜单切模型走 `applyModelConfig`（ChatPanel.tsx:373-403），
  先 `bot_get_config` 再整体回写 `bot_set_config`——但读入的类型声明和回写对象
  **都不含 `permMode`**。Rust 侧 `bot_set_config`（bot.rs:554-573）是全量覆写
  bot-config.json，`BotConfig` 容器级 `#[serde(default)]`（bot.rs:49）把缺失的
  `permMode` 填成 `None` 落盘。
- 后果：用户在设置页把授权模式改成 `strict`/`yolo` 后，只要在挂件上切一次模型，
  授权模式就被静默重置回 `ask`（None 的默认语义），无任何提示。
- 修法：applyModelConfig 的读入类型补 `permMode`，回写对象透传
  `permMode: c.permMode ?? null`（与 tavilyKey 等字段同模式）。

## P2（处理情况）

- **P2-1 `py_set_enabled` 错误载荷是裸字符串**（已修）：58 个命令中唯一返回
  `Result<bool, String>` 的（bot_py.rs:61），与其余 57 个的 CommandError 四字段
  结构不同构，前端拿不到结构化 code。已改为 `CommandResult<bool>`，IO 失败映射
  `CommandError::IoError`，与 `bot_set_enabled`（bot_slash.rs:352）对齐。
- **P2-2 `TASK_INVALID_STATE` 无前端专属处理**（已修）：errorHandler.ts 的
  hintForCode 缺该 code（落默认分支，只提示 message 无 hint——不吞错）；且
  ChatPanel.tsx:762 的防重入识别靠 `formatCommandError(e).includes("正在执行中")`
  **message 子串匹配**，而后端该拒绝是结构化 `CommandError::TaskInvalidState`
  （bot_chat.rs:844 / exec_steps.rs:250）——message 文案一改，防重入 UX 静默退化
  回 2026-08-19 的幻影错误气泡。已修：hintForCode 补 case；ChatPanel 改用
  `isCommandError(e) && e.code === "TASK_INVALID_STATE"`。
- **P2-3 死命令：`bind_file`（db.rs:108）与 `db_merge`（db.rs:1346）**（记录，
  排期删）：已注册进 generate_handler（lib.rs:440 等）但前端零调用（前端只用
  `bind_files` 复数形；`db_merge` 只有测试注释引用）。属多余命令面，删除涉及
  generate_handler 与 db.rs 两个文件，随下次迭代清理。
- **P2-4 `bot_stop` / `bot_confirm_response` 返回 `()` 非 Result**（记录）：
  调用失败无法向前端报错，`bot_confirm_response` 对不存在/已过期的 requestId
  静默 no-op（bot_slash.rs:320-336）——前端弹窗已关、后端实际未消费，用户无感知。
  当前无实际受害路径（60s 超时兜底），记录备查。
- **P2-5 `bot-confirm` 无会话归属时白等 60s**（记录，理论边界）：前端对
  `sessionId == null` 的确认一律不弹窗（ChatPanel.tsx:608，按设计过滤后台任务），
  后端靠 60s 超时拒绝兜底（bot_slash.rs:268）。当前主链路 interactive=true 的执行
  实例恒带 sessionId（runChat 无 sid 早退），仅 DSL 调度器无 StopGuard 遗留路径
  （execute_tool，bot.rs:728-730）interactive 缺省 true 且 session=None——
  若该路径触发确认将白等 60s。现无现实触发面，记录备查。
- **P2-6 序列化风格漂移**（记录）：`AppConsts`（consts.rs:9）、`MigrationStatus`
  （migration.rs:472）、`RulesFile`（migration.rs:437）无 `rename_all`，线上是
  snake_case；前端已按 snake_case 声明类型匹配（**契约一致，非 bug**），与项目
  主流 camelCase 风格不一。另：`SkillInfo.enabled`（manage.rs:8）后端返回但
  前端类型未声明未使用；ChatPanel 两处 `bot_get_config` 类型声明口径不一（已随
  P1-1 修复统一含 permMode）。

## 契约对照表（命令，摘要）

58 条全量逐条对账，结论一致 ✅，仅列有备注的条目：

| 命令 | 参数对账 | 返回对账 | 备注 |
|---|---|---|---|
| `bot_chat` / `bot_execute_task` | ✅ messages/sessionId、taskId/sessionId | ✅ `{text, taskRefs}` | 防重入拒绝为 TASK_INVALID_STATE（见 P2-2） |
| `bot_set_config` | ⚠️→✅ ChatPanel 漏传 permMode（P1-1 已修） | ✅ | Rust 全量覆写语义 |
| `bot_stop` | ✅ sessionId: string\|null ↔ Option<String> | ⚠️ 返回 `()`（P2-4） | |
| `bot_confirm_response` | ✅ requestId/approved/always | ⚠️ 返回 `()`（P2-4） | |
| `py_set_enabled` | ✅ | ⚠️→✅ 裸 String 错误（P2-1 已修） | |
| `bind_file` / `db_merge` | — | — | 前端零调用（P2-3） |
| `app_consts` / `migration_status` | ✅ | ✅ snake_case（P2-6 风格漂移） | 前端类型已按 snake_case 声明 |
| 其余 50 条 | ✅ | ✅ | 参数名 camelCase 转换全部吻合 |

## 契约对照表（事件）

| 事件 | 方向 | payload 对账 | 备注 |
|---|---|---|---|
| `bot-chat-delta` / `bot-think-delta` | Rust→widget | ✅ `{text, sessionId}` | emit_stream 统一注入 sessionId |
| `bot-tool` / `bot-tool-name` / `bot-tool-done` | Rust→widget | ✅ `{id, name[, args], sessionId}` | 仅这三个 bot-tool 前缀 |
| `bot-confirm` | Rust→widget | ✅ `{id, tool, detail, kind, sessionId}` | 回填走 invoke 非事件（P2-5 边界） |
| `bot-skill-failed` | Rust→widget | ✅ `{skillName, reason, completedSummary, rollbackAttempted, sessionId}` | |
| `tasks-updated` | Rust(bot/api/migration)+前端(widget) | ✅ `{source?, upserts, deletes}` | source 枚举五值两端一致；api/migration 只发 main，主窗合并后 `tasks-changed` 转发挂件 ✅ |
| `tasks-changed` / `workspace-changed` | 双向（纯通知） | ✅ unit payload | |
| `profile-changed` | Rust→全局 | ✅ `ProfileView{user,bot}` | |
| `quick-add` / `toggle-theme` | Rust→main | ✅ 无 payload | |
| `execute-task` / `edit-task` / `bot-changed` / `bot-config-changed` / `workspace-updated` | 前端↔前端 | ✅ | Tauri 全局广播，Rust 零监听（设计） |

## 错误码映射表

22 个 code：`BOT_DISABLED / API_KEY_MISSING / KEYRING_ERROR / HTTP_START_FAILED /
PORT_IN_USE / AUTH_MISSING / AUTH_INVALID / PAYLOAD_TOO_LARGE / TASK_NOT_FOUND /
TASK_INVALID_STATE / INVALID_ARGUMENT / DB_ERROR / IO_ERROR / UNKNOWN_TOOL /
ATOMIC_TOOL_BLOCKED / SKILL_LOAD_FAILED / SKILL_NOT_INSTALLED / LLM_REQUEST_FAILED /
LLM_API_ERROR / CONFIRM_TIMEOUT / CONFIRM_REJECTED / INTERNAL`。

- 前端显式 hint：21/22（P2-2 修复后 22/22）；default 分支不吞错（无 hint 仍弹 message）。
- CommandError 手写 Serialize 输出固定四字段 `{code, message, recoverable, platform}`；
  变体携带字段只拼进 message，前端可结构化消费的仅 code/recoverable/platform——
  前端 CommandErrorPayload 类型未声明 `platform`（未使用，无害）。
