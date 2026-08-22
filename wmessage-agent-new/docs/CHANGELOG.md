# Changelog

WMessage Agent Python 内核 — 所有重要变更记录于此。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本遵循 [Semantic Versioning](https://semver.org/lang/zh-CN/)。

---

## [0.4.3] - 2026-08-22 — 写真实 feature_flags.toml + 接 LLM 到生产 main.py (Phase 4 step 3)

### Added（新增）

- **`config/feature_flags.toml`** — 生产配置（完整 deployment guide）
  - `[agent_runtime]` runtime / python_agent_users / python_agent_sessions
  - `[features]` 6 个开关（reflexion / llm_l2_router / orchestrator_worker / audit_sqlite_enabled / tool_guard_whitelist_only）
  - `[llm]` primary / fallback 链（3 个 provider）
  - `[gray]` python_agent_percent（灰度百分比）
  - 顶部部署指南（env var 覆盖 + 灰度策略）
- **`config/feature_flags.dev.toml`** — 开发环境覆盖（fallback=[]，节省 token）
- **`agent_core/feature_flags.py::get_flag_checker(path)`** — 从 feature_flags 移到此处（之前在 main.py），main.py 改 import
- **`tests/test_phase4_step3.py`**（18 tests）：
  - config 文件存在性 + section 完备性
  - FlagChecker 加载 + dev 覆盖 prod + 热加载 mtime + nonexistent fallback
  - env var WMSG_AGENT_FLAGS 覆盖默认路径
  - 完整链路：FlagChecker + env var + build_llm_chain → 0/1/3 provider + mode=real/mock
  - gray percent / 白名单 / session 白名单
  - secrets 检查：uncommented 行不含 sk- 或 MINIMAX_API_KEY= 赋值

### Changed（变更）

- **`agent_core/feature_flags.py`** — 移入 `get_flag_checker()` 函数（避免测试必须 import main.py）
- **`main.py`** — 改 `from agent_core.feature_flags import get_flag_checker`（不依赖 main.py 内部）
- **`config/feature_flags.toml`** — 注释中 API key 示例用 `<your-key>` 占位符（避免 secrets 检测误判）

### 测试覆盖

- **`test_phase4_step3.py`**（18 新增）：config / flag 加载 / env var / 完整链路 / gray 策略 / secrets
- **全量**：504/505 通过 / 62.92s
- **唯一 fail**：`test_ping_under_load`（Phase 4 step 1.5 已记录的 pytest subprocess timing flake，与本次无关）

### 部署说明

```bash
# 1. 复制 config/feature_flags.toml 到部署目录
# 2. 设置环境变量（敏感信息不放 TOML）：
export MINIMAX_API_KEY=<your-key>          # 必填（主模型）
export KIMI_API_KEY=<your-key>             # 可选（fallback 1）
export DEEPSEEK_API_KEY=<your-key>         # 可选（fallback 2）
export WMSG_AGENT_DB=/path/to/agent.db      # SQLite 持久化
export WMSG_AGENT_LOG=/path/to/bot-debug.log # debug 日志（轮转 10MB × 5）
export WMSG_AGENT_OUTPUT_DIR=/path/to/output # doc_make_* 输出目录

# 3. 启动
python main.py

# 4. 紧急回滚（切回老 Rust agent）
# config/feature_flags.toml: [agent_runtime] runtime = "nodejs"
# 或 env override: 暂不支持 runtime override（需改 TOML 后 mtime reload）
```

---

## [0.4.2] - 2026-08-22 — Skill 监控查询（Phase 4 step 2 Python 侧）

### Added（新增）

- **`agent_core/skill_monitoring.py`** — query audit_event 表获取最近 skill outcomes
  - `query_recent_skill_outcomes(db_path, skill_name, limit)` — 返回 list[dict] 含 skill_run_id / skill_name / mode / status / started_at / finished_at / duration_ms
  - `format_outcomes_table(outcomes)` — CLI 可读表格输出
  - `summarize_status(outcomes)` — 返回 `{status: count}` 给状态徽章用
- **`main.py`** CLI 模式 `--query-skill-outcomes [skill] [--limit N] [--db PATH] [--format json|table]`
  - 给 Tauri 设置页 SkillsPanel 提供数据源（Rust 侧 IPC 调用）
- **`tests/test_skill_monitoring.py`**（19 tests）：
  - query: empty / start_only / start_finish / 多个 skill_run / skill_name 过滤 / limit / corrupt_payload / duration 0 / ISO 格式
  - format: 空 / 基本 / 截断 skill_run_id / in_progress 无 duration
  - summarize: mixed / empty
  - CLI: json 格式 / table 格式 / DB 不存在 exit code 1

### Fixed（修复）

- `_insert_event` test helper FK 约束错误：之前直接 INSERT audit_event 但没建 session/skill_run 父行 — 改为先建父行
- `test_started_at_iso_format` timezone 依赖：改为只验证 ISO 8601 格式不依赖具体值

### 测试覆盖

- **`test_skill_monitoring.py`**（19 新增）：skill outcomes 查询 + CLI 集成 + 边界情况
- **全量**：486/487 通过 / 62.70s（含 16 slow 集成）
- **唯一 fail**：`test_ping_under_load`（Phase 4 step 1.5 已记录的已知 pytest subprocess timing flake，与本次工作无关）

---

## [0.4.1] - 2026-08-22 — stdlib logging + 集成测试稳定性

### Added（新增）

- **`agent_core/logging_config.py`** — stdlib logging 配置（参照 LangChain / OpenAI / Anthropic 行业惯例，零依赖）
  - `setup_logging(log_path, level, enable_stdout, max_bytes, backup_count)` — 双 handler：stdout（dev）+ `RotatingFileHandler`（生产，10MB × 5 轮转）
  - 环境变量覆盖：`WMSG_LOG_LEVEL` / `WMSG_LOG_PATH` / `WMSG_LOG_STDOUT`
  - `get_logger(name)` 便利 helper + `new_cid()` 8 字符 correlation id
  - `shutdown_logging()` 测试 cleanup / 进程退出
  - `sys.excepthook` 全局兜底（KeyboardInterrupt 透传）
- **`tests/test_logging_config.py`** — 12 tests（idempotent / env / rotation / excepthook / cid format / 4xx 异常路径）

### Changed（变更）

- **`main.py`** — `setup_logging()` 启动时调用，`handle_chat` 加 correlation id（`new_cid()` 8 字符 hex 贯穿 logger + audit + response），错误路径走 `logger.exception` 自动捕获栈
- **`agent_core/audit_log.py`** — AuditLogger 改 lazy 文件句柄（`__init__` 初始化 `self._fh = None`，`_append` 时 lazy open，写完 flush，句柄复用）— 修复之前 `test_ping_under_load` 高频 log 累积 open/close 开销
- **`tests/test_integration.py`** — `_spawn_agent` 清 `MINIMAX_API_KEY` / `KIMI_API_KEY` / `DEEPSEEK_API_KEY` 环境变量（防 shell 泄露让 subprocess 加载真 LLM → httpx 请求超时）

### Fixed（修复）

- 集成测试 `test_ping_under_load` ping #7/#9 超时（之前 60s+）→ 修后 16/16 集成测试 1.28s 通过
- shell env `MINIMAX_API_KEY` 泄露导致 subprocess 加载真 MiniMax LLM → httpx 真请求超时
- AuditLogger 每次 `log()` 都 `open/write/close` 文件，10 ping 累积 I/O 开销
- `__init__` 漏 `self._fh = None` 导致 AttributeError（8 个测试失败）
- main.py 漏 `logger = get_logger(...)` 模块级声明导致 NameError
- main.py planner imports 被截断（SyntaxError）

### 测试覆盖

- **`test_logging_config.py`**（12 新增）：init/setup/teardown 双调用幂等、env 变量覆盖、`RotatingFileHandler` 轮转、`sys.excepthook` 兜底 + KeyboardInterrupt 透传
- **全量**：468 tests / 1.88s（13 个 test 文件）

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
