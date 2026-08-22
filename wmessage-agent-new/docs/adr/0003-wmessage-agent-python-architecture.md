# ADR-0003：WMessage Agent Python 内核架构

> **状态**：Accepted  
> **日期**：2026-08-22  
> **决策人**：老板  
> **依赖**：ADR-0001（Rust → Python 迁移）、ADR-0002（PREV 主循环）

---

## 1. 背景 / 问题

ADR-0001 决定把 Agent 业务从 Rust 迁移到 Python 子进程。ADR-0002 决定采用 Plan-Replan-Execute-Verify 主循环。

迁移 + 主循环敲定后，还需要决定 **Python 端的具体架构**：
- LLM 怎么接入？
- 工具怎么执行（直接 Python exec / 透传 Rust IPC / mock）？
- Skill 怎么管理（builtin 硬编码 / SKILL.md 文件 / 注册表）？
- Intent 怎么路由（关键词 / LLM 语义 / 双层）？
- 安全怎么把关（黑名单 / 白名单）？
- 审计怎么落地（文件 / SQLite / 双写）？
- 灰度怎么做（feature flags / 用户白名单 / 百分比）？

每个决策都需要 trade-off，本文一锤定音。

---

## 2. 决策概览

```
用户 chat
  ↓
intent_router (L1 关键词 → L2 LLM 兜底)
  ↓ 命中 builtin skill
TwoLayerIntentRouter
  ↓
agent_loop (PREV 主循环)
  ↓
Plan → Execute → Replan → Verify
  ↓
executor.execute(name, args) → 真工具调用（python-docx 等）
  ↓
audit_logger → 文件 + SQLite 双写
  ↓
响应 → Rust 薄壳 → Tauri 前端
```

---

## 3. 决策清单

### 3.1 LLM Provider — 抽象 + FallbackChain

**决策**：抽象 `LLMProvider` 基类 + 3 个实现（MiniMax-M3 / Kimi / DeepSeek）+ `FallbackChain`。

**理由**：
- MiniMax-M3 是主模型，但单一供应商有 rate limit / outage 风险
- FallbackChain 在主失败时自动切换下一家（LLMUnavailableError / LLMRateLimitError / LLMTimeoutError）
- LLMAuthError **不触发 fallback**（配置错误需立即暴露）

**配置**：`MINIMAX_API_KEY` / `KIMI_API_KEY` / `DEEPSEEK_API_KEY` 环境变量；`feature_flags.toml` 的 `[llm]` section 决定 primary / fallback。

**拒绝的方案**：
- 只接 MiniMax：风险高，无降级
- 用 LangChain 统一抽象：引入重依赖，bug 难排查
- 每处直接调各厂商 API：重复代码，切换供应商成本高

### 3.2 Real Tool Executor — 直接 exec，不绕 IPC

**决策**：`RealToolExecutor` 直接调 Python 库（python-docx / openpyxl / python-pptx）生成文件，不通过 IPC 透传 Rust。

**理由**：
- 进程内 exec 比 IPC 快（省 5-10ms / 调用）
- Phase 2 已验证 Mock 路径端到端跑通；Phase 3 切到 Real 时架构稳定
- Rust `bot_py.rs` 仍是 WMessage 主进程的真执行路径；Python 这边是 redundant 路径（用于 test / standalone / 跨平台）

**拒绝的方案**：
- 全走 Rust IPC：每次工具调用绕一圈 Rust → Python script，延迟高
- 双进程（Python agent 主 + Python executor 子）：架构复杂，Phase 3 不必要

**未来扩展**：Phase 4+ 可加 `IPCExecutor`（透传 Rust `execute_tool`），按 tool_name 选择 executor。

### 3.3 Skill Registry — builtin + 自定义 SKILL.md

**决策**：`SkillRegistry` 合并 builtin skills + 自定义 SKILL.md；builtin 优先同名 custom。

**理由**：
- builtin（minimax-docx/xlsx/pptx）由 Python 团队维护，确保稳定可用
- 自定义 SKILL.md 允许老板 / 用户扩展（放 `~/.openclaw/skills/<name>/SKILL.md`）
- builtin 优先 → 核心 Skill 不会被同名 custom 误覆盖

**注册表流程**：
```
1. SkillRegistry() 实例化
2. register_builtin() → 加载 minimax-docx/xlsx/pptx
3. load_custom_dir("~/.openclaw/skills") → 扫描 SKILL.md
4. find_by_intent(query) → 返回 skill 或 None
```

### 3.4 Intent Router — 双层（L1 关键词 + L2 LLM）

**决策**：`TwoLayerIntentRouter` — L1 关键词命中直接返回（高置信度 0.95）；L1 未命中调 LLM 兜底（低置信度 < 0.6 → 不路由）。

**理由**：
- L1 子串匹配：~0ms / 查询，覆盖 90% 场景
- L2 LLM 分类：~200-500ms / 查询，仅在 L1 miss 时触发
- 低置信度不强制路由 → 走通用 LLM 决策（避免误分类）

**拒绝的方案**：
- 纯 L1：自定义 Skill 表达力差
- 纯 L2：每次都慢，token 消耗大
- LLM LLM LLM：太重

### 3.5 Tool Guard — 黑名单 + 可选白名单模式

**决策**：`ToolGuard` 默认黑名单模式（仅 blacklist 中的工具被阻断）；可选白名单模式（更严）。

**黑名单**（来自 SPEC §X.5）：
- `create_word_revisions`（Word 修订模式专用，仅 Skill 内可用）
- `link_file_to_task`（Skill 末尾绑产物专用）

**双保险**：Rust 薄壳 `bot.rs` 也有同名黑名单，两处都拦截。

### 3.6 Audit — 文件 + SQLite 双写 + 自动建 parent

**决策**：`SqliteAuditLogger`（继承 `AuditLogger`）双写 — bot.log 文件 + audit_event 表。FK 链（session / skill_run / graph / task_node）自动派生 + 自动建。

**理由**：
- 文件：与 Rust bot.log 兼容（wmessage-agent-spec.md §X.6 约定）
- SQLite：结构化查询（"上周所有失败 task" / "审计某 session 全流程"）
- 自动建 parent：避免 FK 约束静默吞 insert；测试和真实场景都受益
- best-effort 写入：SQLite 失败不阻塞文件日志

**事件命名空间**（11 类）：
```
user.message / pre_step.route_skill / pre_step.route_failed /
pre_execute.deny / skill.start / tool.call / tool.return /
verify.start / verify.pass / verify.fail / skill.finish /
replan.triggered / task.done / task.fail
```

### 3.7 Feature Flags — TOML + 热加载 + 灰度

**决策**：`FlagChecker` 加载 `config/feature_flags.toml`，支持 mtime 热加载；灰度按用户白名单 + 百分比（hash 决定确定性桶）。

**Schema**：
```toml
[agent_runtime]
runtime = "python"               # python | nodejs

[features]
reflexion_enabled = false        # Phase 3 默认关
llm_l2_router = true             # Phase 3 默认开
audit_sqlite_enabled = true

[llm]
primary = "minimax-m3"
fallback = ["kimi", "deepseek"]

[gray]
python_agent_users = []          # 白名单
python_agent_sessions = []
python_agent_percent = 100       # 0-100，灰度百分比
```

**热加载**：每次 `flags()` 调用检查 mtime，变化自动重载，无需重启。

**拒绝的方案**：
- 远程配置中心：Phase 3 不必要，运维复杂
- 环境变量：不可热加载
- 数据库：双写一致性复杂

---

## 4. 整体架构图

```
┌─────────────────────────────────────────────────────┐
│ Rust 薄壳（bot_py.rs + tauri::command）               │
│ - Tauri 运行时（窗口 / 托盘 / 打包）                  │
│ - IPC 转发（前端 ↔ Python）                          │
│ - Python 子进程托管（spawn / heartbeat / restart）   │
└──────────────────┬──────────────────────────────────┘
                   │ stdin/stdout JSON-RPC
                   ↓
┌─────────────────────────────────────────────────────┐
│ Python Agent 内核（wmessage-agent-new）              │
├─────────────────────────────────────────────────────┤
│ main.py                                              │
│   ├─ ipc/server.py   JSON-RPC 2.0 协议              │
│   └─ handlers: chat / tool.execute / ping / shutdown│
├─────────────────────────────────────────────────────┤
│ agent_core/                                          │
│   ├─ intent_router.py      L1 关键词                │
│   ├─ llm_intent_classifier  L2 LLM 双层              │
│   ├─ skill_registry.py      builtin + SKILL.md       │
│   ├─ planner.py            TaskGraph 生成            │
│   ├─ agent_loop.py         PREV 主循环              │
│   ├─ react_step.py         ReAct 单步               │
│   ├─ verify.py             4 种 verify kind         │
│   ├─ replanner.py          触发条件 + 重规划          │
│   ├─ real_executor.py      doc_make_* 真生成         │
│   ├─ tool_guard.py         黑名单拦截                │
│   ├─ audit_log.py          结构化事件 + 文件         │
│   ├─ audit_sqlite.py       SQLite 双写 + 自动建 parent│
│   ├─ feature_flags.py      TOML 灰度 + 热加载        │
│   └─ db.py                 SQLite + migration        │
├─────────────────────────────────────────────────────┤
│ llm_provider/                                        │
│   ├─ base.py    LLMProvider / FallbackChain          │
│   ├─ minimax.py MiniMax-M3 (主)                     │
│   ├─ kimi.py    Kimi (fallback 1)                    │
│   └─ deepseek.py DeepSeek (fallback 2)               │
└─────────────────────────────────────────────────────┘
                   ↓ HTTP
┌─────────────────────────────────────────────────────┐
│ 外部：MiniMax-M3 / Kimi / DeepSeek API                │
└─────────────────────────────────────────────────────┘
```

---

## 5. 后果

### ✅ 正面

1. **可测**：433 tests，1.11s 全过；慢集成测试也通过（spawn 子进程真实通信）
2. **可扩展**：新 builtin Skill 加 `agent_core/skills/builtin.py` 即可；自定义 Skill 写 SKILL.md 即可
3. **可降级**：MiniMax 5xx → Kimi 429 → DeepSeek 200 → 返回
4. **可观测**：8 类结构化审计事件，写文件 + SQLite，UI 可查
5. **可灰度**：user / session / percent 三粒度，TOML 热加载
6. **可回滚**：feature_flags.runtime = "nodejs" 秒回老 Rust Agent
7. **可独立运行**：Python agent 不依赖 Tauri，CLI / test 都能跑

### ❌ 负面

1. **IPC 延迟**：每次 chat 多 5-10ms（spawn + 心跳 + 解析）
2. **跨语言调试**：stack trace 跨 Rust / Python
3. **Python 生态锁**：3.9+ 必需；不能跑 3.8 以下
4. **子进程崩溃需要 auto-restart**：已实现，但增加复杂度
5. **双语言维护**：招聘 / 工具链 / 文档都得分开

### ⚠️ 风险 + 缓解

| 风险 | 缓解 |
|---|---|
| LLM Provider 全挂 | FallbackChain 3 tier；全失败返回 error 给前端 |
| Python 子进程崩 | auto-restart（5s 内最多 3 次）+ stderr 落日志 |
| audit SQLite 满 | 30 天滚动删除（Error 永久）；监控 size |
| 灰度错误（命中率 0% 或 100%）| 监控指标 + 自动告警 |
| IPC 长连接断 | heartbeat 5s + reconnect 逻辑 |
| Skill body 格式不一致 | 校验 frontmatter；非法跳过（best-effort） |

---

## 6. 拒绝的方案回顾

| 方案 | 拒绝原因 |
|---|---|
| LangChain 统一抽象 | 引入重依赖，bug 难排查；3 个 provider 直接调够用 |
| 透传 Rust IPC 执行 doc_* | 多绕一圈 Rust → Python script，延迟 +5-10ms |
| 双进程（agent + executor） | Phase 3 不必要，复杂度高 |
| 远程配置中心 | Phase 3 不必要；TOML 文件 + 热加载够用 |
| YAML Skill DSL | 老 WMessage Skill 还在用；新格式 TOML frontmatter + 旧格式 YAML fallback |
| 全 L1 / 全 L2 路由 | 单层覆盖差；L1 命中 90% 场景，L2 兜底长尾 |
| 单一 LLM 供应商 | outage 风险；FallbackChain 是工业界标配 |

---

## 7. 实施参考

| 模块 | 文件 | 行数 |
|---|---|---|
| IPC | `ipc/server.py` + `main.py` | 250 |
| PREV 主循环 | `agent_core/agent_loop.py` + `replanner.py` | 600 |
| LLM Provider | `llm_provider/*.py` | 600 |
| Real Executor | `agent_core/real_executor.py` | 200 |
| Skill Registry | `agent_core/skill_registry.py` | 200 |
| Intent Router | `agent_core/intent_router.py` + `llm_intent_classifier.py` | 350 |
| Tool Guard | `agent_core/tool_guard.py` | 130 |
| Audit | `agent_core/audit_log.py` + `audit_sqlite.py` | 400 |
| DB | `agent_core/db.py` + `db/*.sql` | 500 |
| Feature Flags | `agent_core/feature_flags.py` | 230 |
| Tests | `tests/*.py` | 4000+ |

---

## 8. 关联

- **ADR-0001** — Rust → Python 迁移（决策 Rust 薄壳 + Python 全业务）
- **ADR-0002** — PREV 主循环（决策 Plan → Execute → Replan → Verify 范式）
- **wmessage-agent-spec.md §X-Y-Z-W-V** — 完整 spec 文档
- **SKILL_BODY.md** — Skill body 编写指南
- **OPERATIONS.md** — 部署运维手册

---

## 9. 未来扩展（Phase 4+ 路线图）

| 模块 | 优先级 | 说明 |
|---|---|---|
| Reflexion 反思循环 | 中 | 默认关；feature_flags.reflexion_enabled |
| Orchestrator-Worker | 低 | 预留 `spawn_worker` 接口 |
| 远程 LLM Provider | 中 | Anthropic / OpenAI / Ollama |
| 真实 IPC executor | 中 | 透传 Rust `execute_tool`（替代 RealToolExecutor） |
| Skill 监控 UI | 低 | 设置页展示 skill 状态 + 最近 outcome |
| Web UI 监控 | 低 | audit_event 查询面板 |
| 自动性能 profiling | 低 | 每个 tool_call 耗时统计 |

---

> 决策日期：2026-08-22  
> 实施完成：Phase 1-3 全过（433 tests / 1.11s）  
> 决策者：老板  
> 起草：小九
