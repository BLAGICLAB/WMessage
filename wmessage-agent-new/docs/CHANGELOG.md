# Changelog

WMessage Agent Python 内核 — 所有重要变更记录于此。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本遵循 [Semantic Versioning](https://semver.org/lang/zh-CN/)。

---

## [0.3.0] - 2026-08-22 — Phase 1-3 完整收尾

### Added（新增）

#### Phase 1 — IPC + 3 大核心模块 + mock Skill 端到端
- **IPC 协议**：`ipc/server.py` JSON-RPC 2.0 over stdin/stdout，包含 5 个标准错误码
- **planner**：粗粒度 TaskGraph 生成（启发式 + LLM 模式）+ DAG 校验（环/缺失/重复 ID）
- **verify**：4 种 kind（command / file_hash / assert / llm_judge）+ LocalVerifyExecutor
- **react_step**：ReAct 单步（构造 prompt → 调 LLM → 解析 action）+ 工具白名单校验
- **mock Skill 端到端**：`skill_loader.py` / `var_substitute.py` / `mock_executor.py` / `skill_runner.py`
  - 标准 mock Skill fixture `tests/fixtures/mock_skill_archive_demo.md`
  - 变量替换（`${step1.id}` / `${prev.result}`）+ UUID 提取 + ctx 累积

#### Phase 2 — PREV 主循环 + DB + 3 builtin Skills
- **agent_loop.py**：PREV 主循环编排（Plan → Execute → Replan → Verify）
  - 5 状态机（Continue / AwaitUser / Finish / Fail / Terminate）
  - 外层 `outer_iter` + `rerun_needed` 重跑机制
  - ReAct 单步内嵌 + verify 真实执行
- **replanner.py**：触发条件（task_failed / step_threshold / env_drift / global_limit）+ 错误分类
- **audit_log.py**：8+ 类结构化事件 + 与 bot.log 兼容格式
- **db.py**：SQLite + migration runner + SessionDAO / SkillRunDAO / AuditEventDAO
- **db/schema.sql + migrations/0001_initial.sql**：6 张表（session / session_message / task_graph / task_node / skill_run / audit_event）
- **skills/builtin.py**：3 个内置 Skill
  - `minimax-docx`：生成 Word 文档（透传 Rust `doc_make_word`）
  - `minimax-xlsx`：生成 Excel（透传 Rust `doc_make_excel`）
  - `minimax-pptx`：生成 PPT（透传 Rust `doc_make_ppt`）
- **intent_router.py**：关键词 L1 路由（builtin skills）
- **main.py 升级**：chat handler 接 AgentLoop 跑完整 PREV 主循环

#### Phase 3 — LLM Provider + Real Executor + 灰度
- **llm_provider/**：
  - `base.py`：LLMProvider 抽象 + LLMRequest/LLMResponse/LLMChunk + 5 类异常
  - `minimax.py`：MiniMax-M3 主模型（OpenAI 兼容 API）
  - `kimi.py`：Kimi fallback 1（Moonshot API）
  - `deepseek.py`：DeepSeek fallback 2
  - `FallbackChain`：主失败 → 自动 fallback（Unavail/RateLimit/Timeout）
- **real_executor.py**：`RealToolExecutor` 真实生成 .docx（python-docx 实现）+ 继承 LocalVerifyExecutor
- **skill_registry.py**：合并 builtin + 自定义 SKILL.md 加载（builtin 优先）
- **tool_guard.py**：黑名单（`create_word_revisions` / `link_file_to_task`）+ 可选白名单模式
- **audit_sqlite.py**：SqliteAuditLogger 双写（文件 + SQLite），FK 链自动派生
- **llm_intent_classifier.py**：LLM L2 双层路由（L1 miss → LLM 兜底，confidence 阈值 0.6）
- **feature_flags.py**：TOML 配置 + mtime 热加载 + 用户/会话/百分比灰度

#### 文档
- `docs/SKILL_BODY.md`（433 行）：Skill body 编写指南
- `docs/OPERATIONS.md`（716 行）：环境 / 安装 / 配置 / 启动 / 监控 / 升级 / 回滚 / 故障排查 / 安全 / 性能
- `docs/adr/0003-wmessage-agent-python-architecture.md`（314 行）：Python 架构 7 决策（ADR-0003）
- `docs/PR_DESCRIPTION.md`：PR 描述模板
- `docs/CHANGELOG.md`：本文件

### Changed（变更）

- `bot_py.rs`：**未修改**（保持 Rust 薄壳稳定；Python 端独立验证后再切流量）
- `src/`：**未修改**（前端零改动）
- `package.json` / `Cargo.toml`：**未修改**（新增 Python 子项目自包含依赖）

### Fixed（修复）

- ✅ `_read_until_response` timeout 递减 bug（ping #9 在 60s 超时）→ 改用 deadline + 每读 1s 独立 budget
- ✅ planner 英文句号误伤 `config.yaml` → 改用 `\. `（period + space）
- ✅ verify_hint prompt 只打 `kind=` → 按 kind 输出对应字段
- ✅ agent_loop Replan 后不重跑 → 重构主循环用 outer_iter + rerun_needed
- ✅ extract_task_id 只看 last_tool_result → 也看 history 的 tool_call
- ✅ classify_error 中文"超时"未匹配 → 加 `超时` 模式
- ✅ audit_event FK 约束静默吞 insert → logger 自动派生 + 建 parent 链
- ✅ MockExecutor 没实现 VerifyExecutor 接口 → 继承 LocalVerifyExecutor

### Deprecated（弃用）

无

### Removed（移除）

无

### Security（安全）

- **API Key 管理**：通过环境变量（`MINIMAX_API_KEY` / `KIMI_API_KEY` / `DEEPSEEK_API_KEY`），不写入 git
- **Tool 黑名单**：硬编码 `create_word_revisions` / `link_file_to_task`（仅 Skill 内可用）
- **Audit 不变性**：建议生产部署 `REVOKE DELETE, UPDATE ON audit_event FROM PUBLIC`
- **双保险**：Rust 薄壳 + Python 双层黑名单拦截

---

## 路线图（Future）

### [0.4.0] - Phase 4（待规划）

- [ ] `doc_make_excel` 完整实现（openpyxl + 公式支持）
- [ ] `doc_make_pdf` 完整实现（reportlab + 中文字体）
- [ ] `doc_make_ppt` 完整实现（python-pptx + 10 套 theme）
- [ ] `run_python` sandbox（RestrictedPython / subprocess + setrlimit）
- [ ] Reflexion 反思循环（`feature_flags.reflexion_enabled = true`）
- [ ] Skill 监控 UI（设置页状态徽章 + 最近 outcome）
- [ ] 真实 LLM Provider（接 MiniMax-M3 到 main.py 生产配置）

### [0.5.0] - Phase 5（待规划）

- [ ] Orchestrator-Worker 多子 Agent 并行
- [ ] 远程 LLM Provider（Anthropic / OpenAI / Ollama）
- [ ] 真实 IPC executor（透传 Rust `execute_tool`）
- [ ] Web UI 监控面板（audit_event 实时查询）
- [ ] 自动性能 profiling（每个 tool_call 耗时统计）
- [ ] 远程配置中心（替换 TOML 文件）

---

## 版本对比

| 版本 | 测试 | 累计 LOC | 关键能力 |
|---|---|---|---|
| 0.1.0 (Phase 1) | 165 | ~2000 | IPC + planner/verify/react_step + mock Skill e2e |
| 0.2.0 (Phase 2) | 314 | ~3500 | PREV 主循环 + DB + 3 builtin Skills + main.py |
| 0.3.0 (Phase 3) | 433 | ~4500 | LLM Provider + Real Executor + Registry + Guard + Flags |

---

[0.3.0]: https://github.com/BLAGICLAB/wmessage/releases/tag/v0.3.0
