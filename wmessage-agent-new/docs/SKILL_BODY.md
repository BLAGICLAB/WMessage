# WMessage Agent Skill 编写指南

> 适用版本：wmessage-agent-new v0.2.0（Phase 2）
> 配套文档：`docs/wmessage-agent-spec.md` §W（frontmatter schema）/ §X-Y-Z（架构 / IPC / 数据）

## 目录

1. [快速开始](#1-快速开始)
2. [Builtin Skills（内置 3 个文档生成）](#2-builtin-skills)
3. [Intent 路由](#3-intent-路由)
4. [PREV 主循环](#4-prev-主循环)
5. [添加新 Builtin Skill（开发者指南）](#5-添加新-builtin-skill)
6. [从 Chat 调用 Skill](#6-从-chat-调用-skill)
7. [调试与测试](#7-调试与测试)
8. [Phase 3 路线图](#8-phase-3-路线图)

---

## 1. 快速开始

### 1.1 5 行代码从 chat 触发 Word 生成

```python
# 用户在 chat 输入："帮我生成一个 Word 文档，标题是季度总结"
# → intent_router 命中 minimax-docx
# → LLM 在白名单内调 doc_make_word
# → executor 透传 Rust 薄壳 → 真实生成 .docx
# → 返回文件路径
```

### 1.2 端到端命令（CLI / IPC）

```bash
# 启动 agent 进程
python main.py

# 从 stdin 发 JSON-RPC
echo '{"jsonrpc":"2.0","id":1,"method":"chat","params":{"message":"做个 word"}}' | python main.py

# 输出（ndjson）
{"event":"ready","version":"0.2.0","phase":2,"pid":12345}
{"jsonrpc":"2.0","id":1,"result":{
  "plan": {...},
  "routed_skill": "minimax-docx",
  "loop": {"status": "ok", "completed_tasks": ["t1"], ...},
  "audit_lines": [...]
}}
```

---

## 2. Builtin Skills

### 2.1 当前内置的 3 个文档生成 Skill

| Skill | 触发词 | 底层 Rust 命令 | 输出格式 |
|---|---|---|---|
| **minimax-docx** | "生成 word"、"做个文档"、"docx" | `doc_make_word` | .docx |
| **minimax-xlsx** | "做个表格"、"生成 excel"、"xlsx" | `doc_make_excel` | .xlsx |
| **minimax-pptx** | "做个 PPT"、"做幻灯片"、"pptx" | `doc_make_ppt` | .pptx |

### 2.2 设计原则

- **Python 端只定义元数据**（name / intents / tools_whitelist）
- **真执行在 Rust 薄壳**（`bot_py.rs` 已有完整 doc_make_* 实现）
- **Phase 2 IPC**：`executor.execute("doc_make_word", args)` → IPC → Rust `doc_make_word` → python-docx 生成文件

> 为什么不重复造轮子？Rust `bot_py.rs` 已有成熟实现（含 theme、customColors、tables、超时保护）。Python 端做调度，避免两份实现漂移。

### 2.3 Skill 元数据 schema

```python
@dataclass
class BuiltinSkill:
    name: str                          # 唯一标识（kebab-case）
    description: str                   # 一句话描述（用于 intent 匹配 / 日志）
    intents: List[str]                 # 关键词列表（L1 路由匹配）
    mode: str                         # "auto" | "interactive"
    risk: str                         # "low" | "medium" | "high"
    tools_whitelist: List[str]         # 允许的 tool_name（限制 LLM 调用范围）
    react_max_steps: int = 15          # 单 task ReAct 上限
    timeout_s: int = 60                # 全局超时（秒）
    tags: List[str] = []               # 调试 / 分类用
```

---

## 3. Intent 路由

### 3.1 Phase 2 L1 路由（关键词启发式）

```python
from agent_core.intent_router import route_intent

# 命中 docx
route_intent("帮我生成一个 Word 文档")  # → BuiltinSkill(name="minimax-docx", ...)

# 命中 xlsx
route_intent("做个表格")  # → BuiltinSkill(name="minimax-xlsx", ...)

# 命中 pptx
route_intent("做个 PPT")  # → BuiltinSkill(name="minimax-pptx", ...)

# 不命中
route_intent("讲个笑话")  # → None（走通用 LLM 决策）
```

### 3.2 路由规则

- **子串匹配**（小写）：用户查询 ⊇ skill.intents 任一项即命中
- **大小写不敏感**：生成 WORD / Create PPTX 都能命中
- **优先级**：按 `BUILTIN_SKILLS` dict 插入顺序（docx → xlsx → pptx），先到先得

### 3.3 Phase 3 计划

- LLM L2 路由（语义判断，初期关）：关键词 L1 命中 → 直接返回；L1 未命中 → LLM 判意图
- 自定义 SKILL.md 加载：`~/.openclaw/skills/*/SKILL.md` 扫描，与 builtin 合并注册表

---

## 4. PREV 主循环

来自 `docs/wmessage-agent-spec.md` §X.2。

### 4.1 主循环架构

```
[用户 chat]
   ↓ intent_router
[命中 builtin skill / 未命中]
   ↓
[Plan] TaskGraph 生成（heuristic 或 LLM）
   ↓
[Execute] 内层 ReAct 循环
   ↓
[Replan] 触发条件：失败 / 步数阈值 / 环境漂移
   ↓
[Verify] 强校验（机器）+ 弱校验（LLM judge）
   ↓
[Finish] audit + 返回响应
```

### 4.2 4 个核心模块

| 模块 | 文件 | 职责 |
|---|---|---|
| AgentLoop | `agent_core/agent_loop.py` | 主循环编排（Plan → Execute → Replan → Verify）|
| Planner | `agent_core/planner.py` | TaskGraph 生成（DAG，启发式 + LLM 模式）|
| ReAct | `agent_core/react_step.py` | 单步逻辑（思考 → 工具 → 观察）|
| Verify | `agent_core/verify.py` | 4 种 kind 校验（command / file_hash / assert / llm_judge）|

### 4.3 触发条件

| 钩子 | 触发时机 | 行为 |
|---|---|---|
| `pre_plan` | 用户消息进入 | 加载上下文、调 planner |
| `pre_execute` | 工具调用前 | tool_guard 黑名单校验（Phase 2 暂未实现）|
| `post_execute` | 工具调用后 | audit + 分级（Info/Warn/Error）|
| `pre_verify` | task 完成 | 准备 verify 命令 |
| `post_verify` | verify 完成 | 通过 / 不通过 → 重试 / 继续 |
| `pre_replan` | 触发条件命中 | 调 replanner |
| `pre_response` | 回流前 | 拼装响应 + 写审计 |

### 4.4 状态机

5 个 AdvanceAction：

```python
class AdvanceAction(Enum):
    NoActive     = "no_active"
    AwaitConfirm = "await_confirm"  # ←→ 用户确认（如 AWAIT_USER 状态）
    Continue     = "continue"
    Finish       = "finish"
    Fail         = "fail"            # + reason
    Terminate    = "terminate"       # + reason（外部终止）
```

---

## 5. 添加新 Builtin Skill

### 5.1 标准流程（10 分钟）

**Step 1**：在 `agent_core/skills/builtin.py` 加 Skill 定义

```python
PDF_SKILL = BuiltinSkill(
    name="minimax-pdf",
    description="生成 PDF 文档（标题 + 段落，reportlab 实现）",
    intents=["生成 pdf", "做个 pdf", "pdf 文件", "PDF 报告"],
    mode="auto",
    risk="low",
    tools_whitelist=["doc_make_pdf"],   # ← 透传到 Rust doc_make_pdf
    react_max_steps=10,
    timeout_s=60,
    tags=["document", "pdf"],
)

BUILTIN_SKILLS["minimax-pdf"] = PDF_SKILL
```

**Step 2**：跑单测验证

```bash
pytest tests/test_intent_router.py -v
```

**Step 3**：加端到端集成测试

```python
# tests/test_integration.py
def test_chat_intent_routes_to_pdf_skill():
    # 类似 docx/xlsx/pptx 测试
    ...
    assert result["routed_skill"] == "minimax-pdf"
```

### 5.2 添加 Risk > low 的 Skill

如果 Skill 调用工具有副作用（如删除任务、改数据库），需要 risk = "medium" 或 "high"：

```python
ARCHIVE_SKILL = BuiltinSkill(
    name="minimax-archive",
    description="归档任务（从 Doing → Done 列）",
    intents=["归档任务", "move to archive"],
    mode="auto",
    risk="medium",                       # ← 风险等级
    tools_whitelist=["archive_task"],
)
```

Phase 3 会基于 risk 弹确认框（参考 WMessage 现有风险分级）。

### 5.3 多个工具的 Skill

```python
SUMMARY_SKILL = BuiltinSkill(
    name="minimax-task-summary",
    description="任务汇总：list → summary → output",
    intents=["任务汇总", "看看进展"],
    mode="auto",
    risk="low",
    tools_whitelist=["list_tasks", "query_single_task", "create_task"],  # ← 多个
    react_max_steps=20,                # ← 多个工具需要更多步数
)
```

### 5.4 自定义 Skill Body（Phase 3 路线）

Phase 3 引入 SKILL.md loader 后，可以从 `~/.openclaw/skills/<name>/SKILL.md` 加载自定义 Skill：

```markdown
---
name = "my-custom-skill"
description = "我的自定义 Skill"
mode = "auto"
risk = "low"
intents = ["做 XX", "XX 一下"]
tools_whitelist = ["custom_tool"]
---

# 我的自定义 Skill

详细说明 LLM 该怎么分步执行...

## Step 1: 第一步
custom_tool({"arg": "value"})
```

---

## 6. 从 Chat 调用 Skill

### 6.1 IPC 协议（stdin/stdout JSON-RPC 2.0）

来自 `docs/wmessage-agent-spec.md` §Y。

#### 请求格式

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "chat",
  "params": {
    "message": "帮我生成一个 Word 文档，标题是季度总结",
    "context": {                       // 可选，环境状态
      "files_hint": ["src/main.py"],
      "git_status": "M src/main.py"
    },
    "skillBody": "...",                // 可选，显式注入（覆盖 intent router）
    "skillName": "user_query",         // 可选，audit 用
    "sessionId": "s-123",              // 可选，会话 ID
    "maxTasks": 20                     // 可选，默认 20
  }
}
```

#### 响应格式

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "plan": {
      "graph": {
        "tasks": [...],
        "mode": "auto"
      },
      "reasoning": "命中 minimax-docx，整段作为 1 个 task"
    },
    "routed_skill": "minimax-docx",
    "loop": {
      "status": "ok",
      "completed_tasks": ["t1"],
      "failed_tasks": [],
      "replan_count": 0,
      "duration_ms": 1234,
      "ctx": [{"index": "t1", "title": "...", "id": "...", "result": "..."}],
      "error": null
    },
    "audit_lines": [
      "[1734567890123] Info | skill.start | skill_run_id=sr-1 | skill=minimax-docx",
      "[1734567890456] Info | tool.call | task_id=t1 | tool=doc_make_word",
      "[1734567890789] Info | tool.return | ...",
      "[1734567891012] Info | task.done | task_id=t1",
      "[1734567891234] Info | skill.finish | ..."
    ]
  }
}
```

### 6.2 ping / shutdown

```json
// ping
{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}
// → {"jsonrpc":"2.0","id":1,"result":{"pong":true,"ts":1734...}}

// shutdown（发完关闭 stdin 让进程退出）
{"jsonrpc":"2.0","id":1,"method":"shutdown","params":{}}
```

---

## 7. 调试与测试

### 7.1 跑测试

```bash
# 全部
pytest -v

# 单独模块
pytest tests/test_intent_router.py -v

# 集成测试（spawn 子进程，慢）
pytest tests/test_integration.py -v -m slow

# 跳过慢测试
pytest -v -m "not slow"
```

### 7.2 调试某个 Skill

```python
# 在 REPL 里直接调
from agent_core.intent_router import route_intent

skill = route_intent("做个 word 文档")
print(skill.name, skill.tools_whitelist)
# → minimax-docx ['doc_make_word']
```

### 7.3 查看 audit 事件

每次 chat 响应里 `audit_lines` 包含所有事件：

```
[1734567890123] Info | skill.start | skill_run_id=sr-1 | skill=minimax-docx
[1734567890456] Info | tool.call | task_id=t1 | tool=doc_make_word
[1734567890789] Info | tool.return | task_id=t1 | tool=doc_make_word | result=已生成
[1734567891012] Info | task.done | task_id=t1
[1734567891234] Info | skill.finish | skill_run_id=sr-1 | status=ok | ...
```

事件类型见 `agent_core/audit_log.py:AuditEvent` 常量。

### 7.4 常见问题

#### Q1: intent_router 不命中？

- 检查 `intent` 是否包含在查询中（子串匹配）
- 大小写不敏感，但**语言敏感**（中文意图匹配中文查询）
- 调试：`route_intent(query)` 看返回什么

#### Q2: chat 后任务卡在 AWAIT_USER？

- LLM 输出了 `ask_user` 动作（缺少信息 / 需要确认）
- 检查响应里的 `loop.error` 和 `loop.ctx` 看哪一步 ask_user

#### Q3: task 一直失败（replan_exhausted）？

- 检查 `audit_lines` 看每次 replan 的 reason
- 通常是 verify_hint 不达标（如 command exit_code != 0）
- 调大 `max_replans_per_graph` 临时绕过

---

## 8. Phase 3 路线图

| 模块 | 状态 | 备注 |
|---|---|---|
| 真实 LLM（MiniMax-M3 / Kimi） | ⏳ Phase 3 | 替换 ScriptedLLM mock |
| 真实 IPC executor | ⏳ Phase 3 | 替换 MockExecutor，透传 Rust |
| 自定义 SKILL.md 加载 | ⏳ Phase 3 | 扫描 `~/.openclaw/skills/*/SKILL.md` |
| LLM L2 路由 | ⏳ Phase 3 | 关键词 L1 兜底 + LLM 语义补充 |
| tool_guard 黑名单 | ⏳ Phase 3 | 拦截高风险 tool_call |
| audit_event 落 SQLite | ⏳ Phase 3 | audit_event 表已建，Phase 3 接入 |
| Skill 监控 UI | ⏳ Phase 4 | 设置页 + 状态徽章 |

---

## 参考

- `docs/wmessage-agent-spec.md` — 完整 spec（含 ADR / IPC / 数据模型）
- `agent_core/skills/builtin.py` — 3 个 builtin skill 定义
- `agent_core/intent_router.py` — 关键词路由
- `agent_core/agent_loop.py` — PREV 主循环
- `tests/test_intent_router.py` — 30+ 个路由单测
- `tests/test_integration.py` — 端到端集成测试
