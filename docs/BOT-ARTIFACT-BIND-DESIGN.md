# Bot 产物绑定设计方案（D4d 重设计）

> 2026-09-11 · 状态：**已实施**（commit `52f7b20` + 后续 `9894beb` OCR 修复同批）
> 目标：bot 流程结束后把"产物 → 任务卡"的绑定动作从「LLM 立即 db_upsert」改为
> 「登记 → 流程结束 → 用户勾选确认」，按执行入口分流触发，让用户拿回最终决定权。

**老板拍板的 4 个核心决策**（按对话顺序）：
- **D1a**：登记表用内存 HashMap（进程重启清空，「没帮上」可接受）
- **D2a**：弹窗触发 hook 在 `run_task_in_chat_with` 收尾阶段
- **D3b**：新前端组件 `ArtifactBatchDialog.tsx`（默认全选多选）
- **D4d**：按 `TaskExecOrigin` 三态分流触发（Manual 看 status=done；Scheduled/Batch 不论状态都弹）

D4d 是替代原 D4a 的关键决定——老板发现 D4a（统一看 status=done）会让定时任务点完成
时停掉下次触发，致命。

## 1. 现状与问题

| 项 | 旧实现（重构前） | 问题 |
|---|---|---|
| `tool_bind_file` | 弹系统选择框让用户挑路径 → `db_upsert` 立即绑 | LLM 永远拿不到这种交互机会；用户挑的也是给 LLM 看的，奇怪 |
| `tool_link_file_to_task` | 路径直读 → 校验在 AI_Gen_Files 内 → 立即 `db_upsert` | bot 流程内任何时候调一次立即绑；LLM 自评"中间产物"也直接绑，无审查 |
| 黑名单拦截 | `ATOMIC_TOOLS = ["link_file_to_task"]` + `is_skill_active(session_id)` 判定 | 任务卡执行流程不依赖 SkillRun（用 EXECUTE_SYSTEM_PROMPT 上下文），黑名单判断错位 |
| bind_file / link_file 关系 | "白名单 + 黑名单"互补，但都是立即 db_upsert | 没有「登记」概念，重复调用 / 误判 kind 都不能补救 |
| 权限模式适配 | `bind_file` 不走授权（用户亲手挑文件） | bot 工具走授权模式后，bot 借 link_file 绑 ~/.ssh/id_rsa 仍会被 extract_document 白名单放行读走（**安全漏洞**） |

关键事实：bot 流程「🤖 按钮 / ⏰ 定时 / 📦 批量」三条路径最终都进同一个 `run_model_loop`（`bot_model_loop.rs`），
执行上下文用 `TaskExecOrigin` 三态区分；现行黑名单只看 SkillRun 状态，对这条路径天然失明。

## 2. 核心设计：登记 ≠ 绑

**重构后语义**：

```
bot 流程内调 link_file_to_task
   │
   ▼
仅「登记」到内存 bot_artifacts::REGISTRY（task_id → Vec<RegisteredArtifact>）
   │  - 路径必须在 AI_Gen_Files 内（保留原安全白名单）
   │  - 验证文件存在
   │  - 显式 kind: final（最终产物）/ intermediate（中间产物）
   ▼
run_model_loop 收尾 → run_task_in_chat_with 末尾
   │
   ▼
should_emit(task_id, origin, task.column) 分流判定
   │  - Manual + status=done + 有 final 产物 → emit
   │  - Scheduled / Batch + 有 final 产物（不论 status）→ emit
   │  - intermediate 不参与弹窗
   ▼
emit "artifact-batch-ready" Tauri event → 前端 ArtifactBatchDialog 弹窗
   │
   ▼
用户勾选（默认全选）→ invoke("confirm_artifact_batch", taskId, paths)
   │  - bot_artifacts::apply_files_to_task + db_upsert + broadcast_after_mutation
   │  - 清理登记表该 task_id 的所有登记
   ▼
任务卡上出现新绑定的文件
```

**新约束**：
- 「登记」和「绑定」明确分离——LLM 在流程内只能登记，最终决定权在用户
- 三种执行入口分别对待（**核心创新点**）：定时/批量不能像 Manual 那样用 status=done 判定，否则下次触发会被停
- 流程外（普通 chat 场景）调 link_file_to_task 直接拒——防登记表被反复污染

## 3. D4d 按 TaskExecOrigin 分流

老板原本提的 D4a 是「统一看 status=done 才弹」，技术看似简单，但在 ⏰ 定时场景下致命：
老板追问发现 `find_due_tasks` 在 `bot_scheduler.rs:264-269` 按 `column == "done"` 过滤
已完成的定时任务，**点完成会让下次到点不触发**。

**D4d 三态分流**：

| Origin | 来源 | 触发判定 | 为什么 |
|---|---|---|---|
| `Manual` | 🤖 按钮 | `task.column == "done"` 才弹 | 用户主动点完成表示认可，不重复弹会让用户错失绑定时机 |
| `Scheduled` | ⏰ 定时器 | **不论 status 都弹** | 定时执行要保持下次能触发，绝不能让 LLM 调 `complete_task` 把卡片切到 done |
| `Batch` | 📦 批量 | **不论 status 都弹** | 同 Scheduled——批量触发的卡不应被自动完成 |

**配套 EXECUTE_SYSTEM_PROMPT 改动**（`bot_chat.rs:855` 规则 4）：
原版统一「用 edit_task 写摘要 + complete_task 标记完成」拆两段：
- Manual：保留 `complete_task`
- Scheduled / Batch：警告「**不要调 complete_task**（否则下次到点不触发）」，改用 `edit_task` 写摘要到备注

## 4. 黑名单退役与会话注册表

**旧黑名单**（`tool_guard.rs`）：
```rust
pub const ATOMIC_TOOLS: &[&str] = &["link_file_to_task"];
// is_skill_active(session_id) 判定放行
```

问题：任务卡执行流程用 EXECUTE_SYSTEM_PROMPT 上下文，**不依赖 SkillRun**，
`is_skill_active` 永远 false → 黑名单误把 link_file_to_task 当 Skill 内部原子。

**新会话注册表**（`tool_guard.rs`）：
```rust
static SESSION_ORIGINS: OnceLock<Mutex<HashMap<String, TaskExecOrigin>>> = OnceLock::new();

pub fn register_exec_session(session_id: &str, origin: TaskExecOrigin)
pub fn unregister_exec_session(session_id: &str)
pub fn is_task_execution_flow(session_id: Option<&str>) -> bool
```

**使用流程**：
- `run_task_in_chat_with` 头部 `register_exec_session(sid, origin)`
- `tool_link_file_to_task` 第一步 `is_task_execution_flow(session_id)` 判定
- `run_task_in_chat_with` 末尾 `unregister_exec_session(sid)`（无论成败）

普通 chat 场景 LLM 调 link_file_to_task → `session_id=None` → `is_task_execution_flow`
返回 false → 工具直接拒「普通对话场景调用此工具无效果（不报错也不绑），不要反复尝试」。

## 5. 数据结构与命令

**登记表**（`bot_artifacts.rs`，内存 HashMap）：
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtifactKind { Final, Intermediate }

pub struct RegisteredArtifact {
    pub path: String,
    pub kind: ArtifactKind,
    pub registered_at: i64,
}

static REGISTRY: OnceLock<Mutex<HashMap<String, Vec<RegisteredArtifact>>>> = OnceLock::new();
```

**Tauri command**（`bot_artifacts.rs::confirm_artifact_batch`）：
```rust
#[tauri::command]
pub async fn confirm_artifact_batch(
    app: AppHandle,
    task_id: String,
    paths: Vec<String>,
) -> Result<usize, String>
```

- 用户在 `ArtifactBatchDialog` 勾选确认后调用
- 走 `apply_files_to_task` + `db_upsert` + `broadcast_after_mutation`（与旧 bind_files 同链路）
- 同步 `take_all(task_id)` 清登记表
- 返回成功绑定的文件数

**前端组件**（`src/components/ArtifactBatchDialog.tsx`，141 行）：
- 监听 `artifact-batch-ready` Tauri event（payload: `{taskId, taskTitle, sessionId, origin, paths}`）
- 弹模态框，默认全选多选
- 「全选 / 跳过 / 绑定选中」三按钮
- 「绑定选中」调 `invoke("confirm_artifact_batch", {taskId, paths})`
- 「跳过」关弹窗（产物不绑，下次可手动）

**收尾 emit payload**（`bot_chat.rs`）：
```json
{
  "taskId": "t-123",
  "taskTitle": "OCR 识别合同",
  "sessionId": "session-xxx",
  "origin": "manual" | "scheduled" | "batch",
  "paths": ["/Users/.../AI_Gen_Files/result.txt", ...]
}
```

## 6. 关键文件改动清单

| 文件 | 改动 | 行数 |
|---|---|---|
| `bot_artifacts.rs`（新增） | 登记表 + should_emit 分流 + confirm_artifact_batch Tauri command + 5 单测 | +211 |
| `bot.rs` | 删 `tool_bind_file`（~80 行）+ dispatcher 分发臂 + 重写 `tool_link_file_to_task` 登记语义 + session_id 检查 + 改 `apply_files_to_task` 为 pub | -132 / +56 |
| `bot_chat.rs` | `run_task_in_chat_with` 头部 register_exec_session、尾部 unregister + should_emit + emit `artifact-batch-ready`；EXECUTE_SYSTEM_PROMPT 规则 4 拆 Manual/Scheduled-Batch | +39 |
| `bot_model_loop.rs` | 删 `bind_file` schema + MUTATING_TOOLS 引用 + 测试 list；MUTATING_TOOLS 数组大小 16→15 | -8 / +11 |
| `tool_guard.rs` | ATOMIC_TOOLS 清空；新增 `register/unregister_exec_session/is_task_execution_flow`；3 测试更新（黑名单断言 + 死命令锁 + 三态注册） | +120 / -29 |
| `middleware.rs` | 2 atomic_guard 测试 fail-closed→fail-open 适配黑名单清空后的新语义 | +20 / -30 |
| `bot_skills/scheduler.rs` | mock executor 删 `bind_file` 分支 | -1 |
| `lib.rs` | `pub mod bot_artifacts` + invoke_handler 注册 `confirm_artifact_batch` | +2 |
| `ArtifactBatchDialog.tsx`（新增） | 监听事件 + 多选弹窗 + 调 `confirm_artifact_batch` | +141 |
| `App.tsx` | 挂载 `<ArtifactBatchDialog />` | +2 |

**净增 +383 行 / -208 行**（含新模块 + 前端组件）

## 7. 安全性考量

**保留的安全约束**（从旧 `tool_link_file_to_task` 迁移过来）：
1. **路径白名单**：仅接受 `AI_Gen_Files` 目录内的文件
   - 防 LLM 借 link_file 间接读 ~/.ssh/id_rsa（这是旧版就修过的安全洞）
2. **路径必须真实存在**：`std::fs::Path::new(&path).exists()` 校验
3. **canonicalize 校验**：`std::fs::canonicalize` 防 ../ 绕过
4. **会话上下文校验**：`is_task_execution_flow(session_id)` 防普通 chat 反复登记

**新增的语义安全**：
1. **登记 ≠ 绑**：LLM 没法把中间产物/临时文件偷偷绑到任务卡
2. **D4d 分流**：定时/批量执行不会被 LLM 误调 `complete_task` 杀下次触发
3. **用户最终决定权**：默认全选 + 可取消，跳过也行，不强制

## 8. 失败模式与边界

| 场景 | 行为 | 备注 |
|---|---|---|
| 流程中断（timeout / panic） | unregister + 不 emit | D1a 「没帮上」可接受，登记表该 task_id 全部丢失 |
| 进程重启 | 登记表清空 | 同上，D1a 设计选择 |
| 用户跳过弹窗 | `take_all(task_id)` 清登记 + 不绑 | 用户主动放弃 |
| 用户在弹窗里全取消 | 仍调 confirm，paths=[] | 走 db_upsert 0 条路径，等价跳过 |
| LLM 在流程外（普通 chat）调 link_file_to_task | 直接拒 | 「普通对话场景调用此工具无效果（不报错也不绑），不要反复尝试」 |
| LLM 标 `kind=intermediate` 给最终产物 | 不参与弹窗 | D1a 「没帮上」接受；schema 已说明 |
| 任务卡执行流程被 EXECUTE_SYSTEM_PROMPT 强约束 | 流程结束不论成败都 emit（按分流判定） | 只有 should_emit 三态都不满足才不弹 |

## 9. 实施偏差（与早期讨论相比）

设计讨论过程中有 5 个 Q&A，老板拍板的关键决定汇总：

| 问题 | 候选 | 拍板 | 原因 |
|---|---|---|---|
| Q1 流程上下文识别 | 新 BotContext enum / 复用 TaskExecOrigin | **复用 TaskExecOrigin** + `bot_assigned` 字段 | Q1 由老板手动确认两条入口都走 run_task_in_chat 新建 session |
| Q2 弹窗时机 | 单产物即时弹 / 批量汇总弹 / bot 流程结束批量弹 | **流程结束批量弹 + 默认全选** | 不打扰 LLM 中间步骤；最终决定权在用户 |
| Q3 弹窗选项 | 绑/不绑 / 三选一加改名 / 多选批量 | **多选批量（默认全选）** | 同上「最终决定权」思路延伸 |
| Q4 新工具命名 | link_file_to_task / request_bind_artifact_to_task | **保留 link_file_to_task 名 + 重写内部** | 最小破坏，避免外部 prompt / 测试引用断裂 |
| Q5 旧工具删除兼容 | 直接删 / 保留 bind_file / 保留 link_file_to_task 作 legacy | **直接删 bind_file + 重写 link_file_to_task** | bind_file 前端零直调（TodoCard 用 bind_files 复数形），删它零成本 |

**D4d 是替代原 D4a 的关键决定**：老板发现 D4a（统一看 status=done）会让定时任务点完成
时停掉下次触发——技术上 D4a 看似统一，但业务上定时任务绝不应被自动切到 done。

## 10. 后续可优化方向

仅记录，本次实施**未做**，留给未来：
1. **登记条目持久化**（D1b）：落 DB 新表 `task_artifacts`，进程重启不丢。代价：每次流程结束的登记表清理逻辑要带 db_upsert 同步。
2. **弹窗 UI 加预览**：每个产物路径显示大小/修改时间/缩略图（前 200 字）。代价：要后端给每个 RegisteredArtifact 追加 metadata。
3. **路径记忆**：用户给某类任务经常选/不选某个产物，下次默认勾选行为按习惯调整。代价：要持久化用户偏好。

---

**配套 commit**：
- `52f7b20` refactor(bot): link_file_to_task 改为登记语义 + 删 bind_file（主提交）
- `9894beb` fix(ocr): cls 模型输入为固定 80x160（同批修复，D4d 出包前验证完整链路）
