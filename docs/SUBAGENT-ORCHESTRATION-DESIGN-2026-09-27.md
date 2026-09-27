# 子 Agent 编排系统设计（方案 B 定稿）— 2026-09-27

> 状态：**设计定稿，待 A 期拆批实施**（本文档 = 新任务的实施基准）
> 依据：多轮讨论收敛——初版方案 B（异步派发+轮询回收）→ 卡即契约 → AI-native 卡片
> 六反转 → 提示词三层模板 → 已拍板默认值（见 §6）。
> 关联：HANDOFF-2026-09-26-v3 / TP-1~3（任务卡免快照写路径）/ PAR-1（并行会话）/
> bot_execute_task（任务执行聊天化——本设计的基建来源与边界对照）。

---

## 0. 一页结论

- **方案 B**：异步派发 + 轮询回收 + 卡即契约。主 agent 调度审计汇总，子 agent 执行长任务/分步任务（调研/编码）。
- **四个已拍板默认值**：递归硬禁一层（双保险）/ 预算默认值（§6）/ 并发上限（per-session 2、global 3，超限排队）/ 结果 schema（§7）。
- **分层**：运行层（spawn/check 事件流，快车道）与契约层（任务卡，慢车道）不混用、不冲突；spawn 的参数载体 = 子任务卡。
- **分期**：A 最小可用（预算/取消/审计/卡绑定四项不可省）→ B 能力分级（profile/模型分层/重试）→ C 进阶编排（链式/汇总）。

## 1. 目标与非目标

**目标**
1. 主 agent 可编程派发受管子 agent：拆解任务 → 写卡 → 派发 → 审计 → 汇总。
2. 子 agent 独立会话执行单一目标（网上调研 / 写代码 / 长资料整理），独立上下文与白名单工具。
3. 用户全程可见可干预：任务卡 = 唯一事实中心（进度/预算/结果/产物/取消）。
4. 成本与失控风险受硬顶约束；全程审计可按 subagent_id 串链。

**非目标（A 期明确不做）**
- 自动重试（主 agent 看失败摘要自行决定）；子 agent 递归派发；DAG 依赖声明；
  模型分层的默认映射表（A 期仅参数透传+回退，见 §6）；Python 闸门拆分（维持全局串行）。

## 2. 架构：两层不混用

```
用户 ──对话──> 主 agent（拆解/写卡/派发/审计/汇总）
                │ ①写主卡（编排计划：subtasks=派发清单）
                │ ②spawn_subagent(...) → orchestrator 建子卡 + 子会话，立即返回
                ▼
   ┌─ orchestrator（新增层）───────────────────────────┐
   │ 生命周期状态机 / 预算强制 / 并发排队 / 事件审计        │
   │  subagent_id / trace_id / parent_session_id / task_id │
   └───────────────────────────────────────────────────┘
                ▼
        子 agent 会话 × N（复用 exec 基建：独立会话 + 工具循环 + ChatGuard）
                │ 每轮读卡 diff 自适应 / 勾 subtask / 收尾 result 写卡
                ▼
        完成事件 → 主会话 hint + check_subagent 可查 → 主 agent 汇总
```

**与 bot_execute_task 的边界（不混用入口）**：`bot_execute_task` 是「单发子 agent」
（用户点执行，无编排语义）；`spawn_subagent` 是「受管子 agent」（主 agent 派发，
orchestrator 管生命周期/预算/审计/子卡绑定）。两者复用 exec 运行基建（独立会话 +
工具循环 + ChatGuard + exec_steps），但入口、存储、事件各自独立。

## 3. 子 agent 生命周期

状态机：`queued → running → succeeded | failed | cancelled | budget_exceeded`

- `queued`：并发上限满时排队（不失败）；有槽位即转 running。
- 标识四元组：`subagent_id`（orchestrator 生成）/ `trace_id`（贯穿日志）/ 
  `parent_session_id`（派发者主会话）/ `task_id`（子任务卡 id，一子 agent 一子卡）。
- 存储：新表 `subagents`（对齐现有 migrations 模式）：
  `id, status, profile, model, parent_session_id, task_id, objective, budget_json,
  result_json, error, created_at, started_at, finished_at`。
  持久化的理由：check_subagent 幂等可轮询、事件丢失可查、重启后状态不丢。
- 事件（全部进 bot.log，结构化键值）：`subagent_spawned / progress / done / failed /
  cancelled / budget_exceeded`；字段带 `subagent_id / task_id / parent_session_id /
  profile / model / turn / tool / elapsed_ms / tokens / reason`。

## 4. 卡即契约（任务卡 = 唯一事实中心）

**一子 agent 一子卡**；主 agent 先写主卡（编排计划），spawn 时 orchestrator 建子卡。

### 4.1 任务卡新字段（Phase A，走 TP-2 task_patch 既有通道）
- `assignee`：子 agent 会话 id（串链执行对话/审计/围观过滤）
- `budget`：{maxTurns, maxToolCalls, maxWallSeconds}——**上卡可见**，不是隐藏参数
- `result`：收尾结构化摘要（结论/产物路径/未完成项/逐条验收结果），卡片折叠展示

### 4.2 双写约定
spawn 的 `acceptance_criteria` **必填**，orchestrator 双写：子卡 note（人可见）+
子 agent 任务包装（机器消费）。子卡 subtasks 由主 agent 的分步计划写入。

### 4.3 用户干预路径（全部现有交互，零新 UI 概念）
- 改 subtasks / 补 note 验收 = 修改需求（子 agent **每轮开始重读子卡 diff**，一轮内生效）
- 软删子卡 = 取消派发（子 agent 读到 deletedAt → 自行终止，status=cancelled）
- 卡片停止按钮 = `cancel_subagent`（与对话内工具同 API）
- 交付 = done + result + 产物 bind_files 回卡

### 4.4 主卡与子卡
- 主卡：主 agent 的拆解计划（subtasks = 派发清单，一项对应一个子 agent）；主 agent
  通过 check 汇总后勾主卡计划项。
- 子卡：单个子 agent 的执行契约；子 agent **只勾自己的子卡 subtask**，不写任务卡
  主状态（服务端强制：子 agent 身份的 task_patch 仅放行 subtasks 键，见 §11）。

## 5. 工具接口（定死）

```
spawn_subagent(objective, profile, acceptance_criteria, budget?, model_profile?, parent_task_id?)
  -> { subagent_id, task_id, status }          # status: queued|running；非阻塞立即返回

check_subagent(subagent_id? | task_id?, wait_ms=0)   # wait_ms 上限 5000，幂等可轮询
  -> { status, progress, result?, error? }     # status: queued|running|succeeded|failed|cancelled|budget_exceeded

cancel_subagent(subagent_id)
  -> { ok }                                     # 与任务卡停止按钮同一 API
```

- profile：`research | coder | general`（A 期三档；工具白名单见 §5.1）。
- model_profile：A 期接受参数、未指定/无效值回退当前 active model；research/coder
  默认便宜快模型的映射表 B 期启用。
- acceptance_criteria：**spawn 必填**，数组，每条可检验（§8 关键写法）。
- 递归双保险：子 agent 工具白名单不暴露 spawn（前端工具清单断言）+ 服务端校验调用者
  身份（子 agent 会话调 spawn → InvalidArgument）。

### 5.1 工具白名单 profile
| profile | 白名单 |
|---|---|
| research | web_search、fetch_url、写产物文件（独立目录内）、list_files、read_file |
| coder | read_file、write_file（allowedDirs 内）、list_files、run_python、写产物文件 |
| general | research ∪ coder 去重 |
| 禁止（所有 profile） | spawn_subagent、任务卡主状态写（complete_task/update_task 等）、任务执行、权限变更 |

## 6. 预算与默认值（已拍板）

| 项 | 默认 | 说明 |
|---|---|---|
| 递归 | 硬禁一层 | 工具白名单 + 服务端双保险 |
| max_turns | 30（硬顶 50） | 工具循环轮数 |
| max_tool_calls | 100 | 累计工具调用次数 |
| max_wall_seconds | 600（10min） | 墙钟；**排队时间不计入**（从 running 起算） |
| 并发 | per-session 2 / global 3 | 超限排队不失败（status=queued） |
| 模型 | per-spawn 透传 + 回退 active | 分层映射 B 期 |
| 权限 | 只能继承或收紧 | allowedDirs 取交集；服务端强制 |
| 产物目录 | `gen_dir/subagents/{subagent_id}/` | 隔离，避免交错 |
| DB 写 | 子 agent 任务卡只读 | 仅 orchestrator 代勾 subtask（task_patch） |
| 取消 | A 期即有 cancel_subagent | 卡片停止按钮同 API |

预算触达 → 状态 `budget_exceeded`，停止新探索，已产出的部分结果照常落库（部分结果
不丢弃）。前端/提示词要求「剩余 3 轮或工具调用 80% 时主动收尾」（软收敛优先于硬顶）。

## 7. 结果回传 schema（子 agent 收尾必须输出，任务包装末尾重申）

```json
{
  "status": "succeeded|failed|cancelled|budget_exceeded",
  "summary": "…（摘要，不倒原文）",
  "acceptance_check": [{"criterion": "…", "met": true, "evidence": "…"}],
  "artifacts": [{"path": "…", "type": "…", "description": "…"}],
  "subtasks": [{"id": "…", "status": "done|failed|pending"}],
  "blockers": [],
  "next_actions": [],
  "confidence": 0.0
}
```

- 解析策略：取最终消息中最后一个 JSON 对象；解析失败 → `failed` +
  `error=result_json_unparseable`，原始文本存 summary（防丢）。B 期加一次廉价收尾重试。
- 主 agent **只消费** summary + artifacts + blockers + acceptance_check；长上下文留在
  子会话。orchestrator 解析后写子卡 result + 勾 subtask（代勾走 task_patch）。

## 8. 提示词资产（A2 期落 `docs/prompts/subagent/`，实现固化为后端常量）

### 8.1 main_agent.md（主 agent 系统提示追加段）
```text
你是主 agent，职责是理解用户意图、拆解任务、派发子 agent、审计进度、汇总结果。
你不亲自执行长任务；长任务一律交给子 agent。

## 何时派发（满足任一必须 spawn_subagent）
- 预计超过 5 轮工具调用
- 需要长上下文、多来源调研、多文件操作
- 需要写代码、跑脚本、长时间执行
- 用户明确要求后台执行或并行执行

不派发：简单问答、单步查询、用户要求直接回答、一句话能说清的事实。

## 派发流程
1. 建主卡：subtasks = 派发清单（一项一个子 agent）；note 写总体验收。
2. 每个派发项：objective 单一目标；acceptance_criteria 必填、每条可检验
   （不要写"调研清楚"，要写"覆盖至少 5 个产品，每个含官网 URL，输出 report.md"）。
3. 调用 spawn_subagent(...) —— 非阻塞，立即返回；马上告诉用户"已派发"，不等待。
4. 继续响应用户；用 check_subagent 轮询（wait_ms 0 或短等待，不要高频空转）。
5. 完成后取结果，只向用户汇报：结论、产物路径、未完成项、风险。

## 失败与预算
- failed / budget_exceeded / cancelled：先读 error 与 summary，
  决定重试、换 profile、降级直接回答，或如实向用户报告。
- 不得放宽子 agent 权限；不得让子 agent 再派发。
- 不向子 agent 倾倒主对话全文：只给目标、验收标准、必要上下文摘要。
```

### 8.2 subagent_base.md（子 agent 通用系统提示）
```text
你是子 agent，只执行任务包装中的单一目标。
你没有 spawn_subagent 工具，也不得请求派发子 agent。

## 工作流
1. 读 objective 与 acceptance_criteria。
2. 规划 3~7 步，对应自己的 subtasks。
3. 每轮开始重读任务卡：subtasks/note 有变更 → 调整计划；卡片被软删 → 立即终止。
4. 每完成一步勾选对应 subtask。
5. 只在白名单工具内操作。
6. 预算意识：剩余 3 轮或工具调用达 80% → 停止新探索，整理当前结果。
7. 收尾必须输出结构化 JSON（schema 见任务包装末尾），不要输出无关散文。

## 禁止
- 写任务卡主状态（只能经系统勾自己的 subtask）。
- 放宽权限；无限重试；遇阻塞记录 blocker。
- 把长原文倒给主 agent：只回摘要和产物路径。
```

### 8.3 profiles/research.md
```text
你是 research 子 agent。
工具：web_search、fetch_url、写产物文件。
- 每个关键结论给出来源 URL。
- 区分事实与推断。
- 收尾输出报告文件路径和来源列表。
```

### 8.4 profiles/coder.md
```text
你是 coder 子 agent。
工具：read_file、write_file（allowedDirs 内）、list_files、run_python。
- 先读后改，小步验证；能跑测试就跑测试。
- 产物路径写入 artifacts。
- 不要改任务卡主状态。
```

### 8.5 任务包装（spawn 注入子会话的 user message，由 orchestrator 从子卡渲染）
```text
[子任务派发]
subagent_id: {id}
objective: {objective}

acceptance_criteria:
- {criterion_1}
- {criterion_2}

上下文摘要:
{只给必要背景，不给主对话全量}

预算:
max_turns={n}, max_tool_calls={n}, max_wall_seconds={n}

产物目录:
gen_dir/subagents/{subagent_id}/

完成后必须输出以下 JSON：
{ …§7 schema 原文… }
```

### 8.6 关键写法（实现与评审的检查点）
1. 验收标准必须可检验；2. acceptance_criteria spawn 必填且双写；3. 子 agent 工具
清单不含 spawn（system prompt 双保险）；4. 预算软收敛优先硬顶；5. 主 agent 只消费
摘要；6. 收尾 JSON 在包装末尾重复提醒。

## 9. 并发与资源

- 并发上限 per-session 2 / global 3；超限 `queued` 排队（FIFO），有槽位转 running。
- Python：子 agent 的 run_python 仍走全局 PY_RUN_GATE 串行（既有取舍，触发体感再拆）。
- 任务执行（bot_execute_task）与子 agent 并存：ChatGuard 按会话隔离，天然并行。
- 前端：PAR-1 并行会话已支持多会话流式/围观；A3 增加按 subagent_id 过滤围观。

## 10. 安全边界

1. 递归双保险（§5）。
2. 权限：perm_mode/allowedDirs 只能继承或收紧；allowedDirs 取交集；服务端强制。
3. **网络请求 SSRF 防护**：子 agent 的 fetch_url 类请求仅允许 http/https；发请求前
   校验 host，拒绝 localhost、环回、私有（10/8、172.16/12、192.168/16）与保留地址
   （169.254/16、::1、fc00::/7、fe80::/10、0.0.0.0 等）。A2 复核现有实现并补测试。
4. 产物目录隔离：gen_dir/subagents/{subagent_id}/，子 agent 文件写不得越目录
   （coder 的 write_file 仍受 allowedDirs 交集约束）。
5. 子 agent 对任务卡主状态只读：服务端按调用者身份过滤 task_patch 可写键（仅 subtasks）。
6. 提示注入面：任务包装由 orchestrator 渲染（不接受 LLM 自由拼接的 system 段）；
   子卡内容（用户可改）进入子 agent 上下文属预期行为（用户即数据属主，威胁模型
   锚定本地单用户桌面，与 PHASE6-T 一致）。

## 11. 分期与拆批（A 期实施计划，每批走既有批循环 SOP）

### A1 orchestrator 核心（服务端，不暴露 LLM 工具）
- `subagents` 表 + 生命周期状态机 + spawn/check/cancel 内部实现
- 子卡创建（一子 agent 一子卡）+ parent_task_id 关联 + acceptance 双写
- 审计事件 subagent_spawned；预算/并发字段落表
- 验收：单测覆盖状态机全部转移 / spawn 建子卡 / cancel 置 cancelled / 缺卡 TaskNotFound

### A2 工具暴露 + 提示词
- 三工具进 registry + 递归双保险 + 任务包装渲染 + 四提示词资产嵌入
- 主 agent 系统提示接入；子 agent 收尾 JSON 解析落库
- 验收：spawn 非阻塞；check 幂等；子 agent 工具清单断言无 spawn；SSRF 校验测试补齐；
  收尾 JSON 解析失败 → failed+result_json_unparseable

### A3 预算/并发/前端
- 预算强制（turns/tool_calls/wall）→ budget_exceeded；并发 per-session 2 / global 3 排队
- 前端：围观按 subagent_id 过滤、卡片停止按钮 = cancel_subagent、三字段（assignee/
  budget/result）投影
- 验收：超限停机写原因且部分结果落库、并发排队 FIFO、卡片停止可用、
  任务卡三字段可见

## 12. 风险与缓解

| 风险 | 缓解 |
|---|---|
| 成本翻倍 | 预算硬顶 + 模型分层（B）+ 摘要长度限制 |
| 目标传递损耗 | acceptance_criteria 必填 + 双写 + 子 agent 收尾对照自检 |
| 失控循环 | 轮数/次数/墙钟三硬顶 + 软收敛提示 + 超限即停 |
| 失败静默 | check 幂等可轮询 + 事件丢失去查 subagents 表 + 完成事件进主会话 hint |
| 并发调试难 | trace_id 贯穿 + subagent_id 串链 bot.log |
| 围观混乱 | 前端按 subagent_id 过滤（A3） |
| 结果 JSON 解析失败 | 解析失败→failed+原文落 summary，不静默丢 |

## 13. 验收清单（整个 A 期的完成判定）

- [ ] spawn 非阻塞，返回 subagent_id/task_id/status
- [ ] 子卡自动创建并与 parent_task_id 关联，acceptance 双写
- [ ] check 幂等可轮询；wait_ms 短等待生效
- [ ] cancel 可停（对话工具与卡片按钮同效）
- [ ] 预算三顶生效，超限 budget_exceeded 且部分结果落库
- [ ] 并发上限排队 FIFO
- [ ] 子 agent 无 spawn 工具；服务端拒绝子 agent 调 spawn
- [ ] 子 agent 任务卡主状态只读；subtask 代勾走 task_patch
- [ ] 产物落 gen_dir/subagents/{subagent_id}/ 并 bind 回子卡
- [ ] 结果 JSON 解析落库；解析失败不丢原文
- [ ] 审计事件全链（spawned/progress/done/failed/cancelled/budget_exceeded）可按 subagent_id 串链
- [ ] SSRF 校验测试在位
- [ ] 前端：并行围观、按 subagent_id 过滤、卡片三字段投影、停止按钮
- [ ] 主 agent 提示词接入后：真实任务「派发→围观→汇总」端到端跑通

## 14. 待办登记（实施时建批）

- A1/A2/A3 三批按上节拆分，批名建议 `SUBA-1/2/3`（family: subagent-orchestration）。
- 实施顺序依赖：A1 → A2 → A3 串行；A1 可独立验收（不暴露工具即可合入）。
- 本设计文档为唯一基准；实施中的偏离走 docs 勘误，不改本文历史结论。
