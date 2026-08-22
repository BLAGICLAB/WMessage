# WMessage Agent Python 部署运维手册

> 适用版本：wmessage-agent-new v0.3.0+（Phase 1-3 完整）  
> 配套文档：`SKILL_BODY.md` / `docs/adr/0003-*.md` / `docs/wmessage-agent-spec.md`

---

## 目录

1. [环境要求](#1-环境要求)
2. [安装步骤](#2-安装步骤)
3. [配置](#3-配置)
4. [启动](#4-启动)
5. [监控](#5-监控)
6. [升级路径](#6-升级路径)
7. [回滚策略](#7-回滚策略)
8. [故障排查](#8-故障排查)
9. [安全注意](#9-安全注意)
10. [性能基线](#10-性能基线)

---

## 1. 环境要求

### 1.1 系统要求

| 维度 | 最低 | 推荐 |
|---|---|---|
| 操作系统 | macOS 12+ / Ubuntu 20.04+ / Windows 10+ | macOS 13+ / Ubuntu 22.04+ |
| Python | 3.9 | 3.11+ |
| 内存 | 256 MB | 512 MB |
| 磁盘 | 100 MB | 500 MB（含 audit 历史）|
| 网络 | LLM API 可达 | 同上 |

### 1.2 Python 依赖

```toml
# pyproject.toml 关键依赖
[project]
requires-python = ">=3.9"
dependencies = [
    # Phase 1: 仅 stdlib
]

[project.optional-dependencies]
dev = [
    "pytest>=8.0.0",
    "pytest-timeout>=2.1.0",
]
llm = [
    "httpx>=0.27.0",    # LLM Provider 必需
]

doc = [
    "python-docx>=1.1.0",  # doc_make_word
    "openpyxl>=3.1.0",    # doc_make_excel (Phase 4)
    "python-pptx>=0.6.21", # doc_make_ppt (Phase 4)
    "reportlab>=4.0.0",    # doc_make_pdf (Phase 4)
]
```

### 1.3 必需环境变量

| 变量 | 必填 | 说明 |
|---|---|---|
| `MINIMAX_API_KEY` | 主 LLM 必填 | MiniMax-M3 API key |
| `KIMI_API_KEY` | fallback 1 | Kimi API key（fallback 用） |
| `DEEPSEEK_API_KEY` | fallback 2 | DeepSeek API key（fallback 用） |

主 LLM 必填；fallback 至少 1 个推荐（无 fallback 时单点失败）。

---

## 2. 安装步骤

### 2.1 标准安装

```bash
# 1. 克隆代码
cd ~/Projects/wmessage
git clone <repo> wmessage-agent-new  # 或已 clone 直接 cd

# 2. 创建 venv
cd wmessage-agent-new
python3 -m venv .venv
source .venv/bin/activate

# 3. 安装依赖（含 LLM + doc 生成）
pip install -e ".[dev,llm,doc]"

# 4. 验证安装
python -c "from main import HANDLERS; print('OK')"
```

### 2.2 最小安装（仅 Phase 1-2，不需要 LLM / doc 生成）

```bash
pip install -e ".[dev]"
```

适用于：CI / 单元测试 / mock-only 集成测试。

### 2.3 Docker 镜像（推荐生产部署）

```dockerfile
FROM python:3.11-slim

WORKDIR /app
COPY pyproject.toml .
COPY agent_core/ ./agent_core/
COPY llm_provider/ ./llm_provider/
COPY ipc/ ./ipc/
COPY db/ ./db/
COPY docs/ ./docs/
COPY main.py .

RUN pip install --no-cache-dir ".[llm,doc]"

ENV PYTHONUNBUFFERED=1
ENV MINIMAX_API_KEY=""

CMD ["python", "main.py"]
```

构建 + 运行：
```bash
docker build -t wmessage-agent:0.3.0 .
docker run -d \
  -e MINIMAX_API_KEY=$MINIMAX_API_KEY \
  -e KIMI_API_KEY=$KIMI_API_KEY \
  -v /var/wmessage/audit:/app/data \
  wmessage-agent:0.3.0
```

---

## 3. 配置

### 3.1 feature_flags.toml（必需）

**位置**：`config/feature_flags.toml`（或显式传入 `FlagChecker(path=...)`）

```toml
# ─────────── agent 运行时 ───────────
[agent_runtime]
runtime = "python"               # python | nodejs（兜底）
python_agent_users = []          # 白名单用户 ID（精确匹配）
python_agent_sessions = []       # 白名单 session ID

# ─────────── 功能开关 ───────────
[features]
reflexion_enabled = false        # 任务结束自我反思
llm_l2_router = true             # L1 未命中调 L2 LLM 兜底
orchestrator_worker = false      # 多子 Agent 并行（未实现）
audit_sqlite_enabled = true      # 审计双写 SQLite
tool_guard_whitelist_only = false  # 是否切白名单模式

# ─────────── LLM 配置 ───────────
[llm]
primary = "minimax-m3"
fallback = ["kimi", "deepseek"]

# ─────────── 灰度 ───────────
[gray]
python_agent_users = []          # 用户白名单
python_agent_sessions = []       # 会话白名单
python_agent_percent = 100       # 灰度百分比（0-100）
```

**热加载**：修改文件后，下一次 `flags()` 调用自动重载（mtime 检测）。

### 3.2 harness_policy.toml（可选）

```toml
[planner]
max_tasks = 20
default_risk = "low"

[executor]
react_max_steps_per_task = 30
tool_timeout_s = 60

[replan]
trigger_step_threshold = 3
max_replans_per_graph = 5

[verify]
max_retries = 2
require_machine_check = true   # 禁止 LLM 自评
```

### 3.3 LLM API Key 管理

**推荐**：用 secret manager（AWS Secrets Manager / Vault / 1Password CLI），不要 hardcode。

**临时（开发）**：
```bash
export MINIMAX_API_KEY="sk-..."
export KIMI_API_KEY="sk-..."
export DEEPSEEK_API_KEY="sk-..."
```

**生产（Docker）**：
```bash
docker run -e MINIMAX_API_KEY=$(cat ~/.secrets/minimax) ...
```

或 Docker secrets：
```bash
echo "$MINIMAX_API_KEY" | docker secret create minimax_key -
docker run --secret minimax_key -e MINIMAX_API_KEY_FILE=/run/secrets/minimax_key ...
```

### 3.4 SQLite 数据库

**默认位置**：`data/agent.db`（相对于工作目录）

**配置**：通过 `SqliteAuditLogger(db_path=...)` 显式传入，或环境变量 `WMSG_AGENT_DB`。

**备份策略**：每日 cron `sqlite3 data/agent.db ".backup /backup/agent-$(date +%F).db"`

---

## 4. 启动

### 4.1 开发模式

```bash
# 直接跑（看实时输出）
cd wmessage-agent-new
python main.py

# 测试 IPC（stdin 发一行 JSON-RPC）
echo '{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}' | python main.py
```

预期输出（2 行 ndjson）：
```jsonl
{"event":"ready","version":"0.3.0","phase":3,"pid":12345}
{"jsonrpc":"2.0","id":1,"result":{"pong":true,"ts":1734...}}
```

### 4.2 生产模式（Tauri 集成）

WMessage 主进程通过 `bot_py.rs` 自动 spawn Python 子进程：

```rust
// bot_py.rs 已实现
spawn_python_subprocess_with_heartbeat(
    "wmessage-agent-new",
    &["main.py"],
    env_vars: { MINIMAX_API_KEY, KIMI_API_KEY, ... }
)
```

子进程崩溃 → Rust 自动重启（5s 内最多 3 次，第 4 次返回 503）。

### 4.3 独立部署（Docker / systemd）

```bash
# Docker
docker run -d --name wmessage-agent \
  --restart unless-stopped \
  -e MINIMAX_API_KEY=$MINIMAX_API_KEY \
  -e KIMI_API_KEY=$KIMI_API_KEY \
  -v /var/wmessage/data:/app/data \
  -v /var/wmessage/audit:/app/audit \
  wmessage-agent:0.3.0

# systemd (Linux)
cat > /etc/systemd/system/wmessage-agent.service <<EOF
[Unit]
Description=WMessage Agent Python Kernel
After=network.target

[Service]
Type=simple
EnvironmentFile=/etc/wmessage/secrets.env
WorkingDirectory=/opt/wmessage-agent
ExecStart=/opt/wmessage-agent/.venv/bin/python main.py
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF
systemctl enable --now wmessage-agent
```

---

## 5. 监控

### 5.1 关键指标（写入 `monitoring/metrics.jsonl`）

| 指标 | 来源 | 阈值 |
|---|---|---|
| `error_rate` | audit_event | < 5% |
| `verify_pass_rate` | verify.pass / verify.fail | > 70% |
| `avg_task_steps` | task.done / task.steps | < 15 |
| `ipc_roundtrip_ms` | chat 响应时间 | P99 < 50ms |
| `python_crash_count` | subprocess 崩溃日志 | 0 / 24h |

### 5.2 SQLite 查询示例

```python
from agent_core.db import connect, AuditEventDAO

conn = connect("data/agent.db")
dao = AuditEventDAO(conn)

# 最近 24h 所有错误
errors = dao.query(level="Error", limit=100)
for e in errors:
    print(f"{e['ts_ms']} {e['event']}: {e['message']}")

# 某 session 全流程
events = dao.query(session_id="s-123", limit=200)
# → user.message → skill.start → tool.call → tool.return → skill.finish

# 失败的 task
fails = dao.query(event="task.fail", limit=50)
```

### 5.3 告警规则

```yaml
# Prometheus alertmanager 规则（示例）
groups:
- name: wmessage-agent
  rules:
  - alert: HighErrorRate
    expr: rate(audit_error_total[5m]) > 0.05
    for: 5m
    annotations:
      summary: "WMessage Agent error rate > 5%"

  - alert: PythonSubprocessCrashed
    expr: increase(python_crash_count[1h]) > 0
    annotations:
      summary: "Python subprocess 崩溃"
```

### 5.4 健康检查

```bash
# IPC 心跳（每 5s 一次）
echo '{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}' | python main.py | grep pong

# ready event 验证
echo '{"jsonrpc":"2.0","id":1,"method":"chat","params":{"message":"ping"}}' | python main.py | head -1
# 预期：{"event":"ready","version":"0.3.0",...}
```

---

## 6. 升级路径

### 6.1 标准升级流程

```
1. 备份当前代码 + 数据
   tar czf backup-$(date +%F).tar.gz wmessage-agent-new/ data/

2. 拉新代码
   git pull origin main  # 或 git fetch + merge

3. 跑测试套件
   cd wmessage-agent-new
   pip install -e ".[llm,doc]"
   pytest tests/  # 必须 433 全过

4. 灰度发布（feature_flags）
   # 步骤 1: 内部白名单 (5% users)
   feature_flags.toml: python_agent_users = ["dev-1", "dev-2"]
   
   # 步骤 2: 1% 全量（hash 决定）
   feature_flags.toml: python_agent_users = []
                     python_agent_percent = 1
   
   # 步骤 3: 10%
   python_agent_percent = 10
   
   # 步骤 4: 50% → 100%
   python_agent_percent = 100
   
   # 步骤 5: 完全切换
   python_agent_percent = 100

5. 监控关键指标
   - error_rate < 5% → 继续
   - verify_pass_rate > 70% → 继续
   - python_crash_count == 0 → 继续

6. 任意一步失败 → 回滚（见 §7）
```

### 6.2 灰度检查清单（每步前检查）

| 检查项 | 命令 |
|---|---|
| 单元测试全过 | `pytest tests/ -m "not slow"` |
| 集成测试全过 | `pytest tests/ -m slow` |
| LLM 调用延迟 < 5s | `time echo "..." \| python main.py` |
| error rate 0% | `sqlite3 data/agent.db "SELECT COUNT(*) FROM audit_event WHERE level='Error'"` |
| crash count 0 | `grep "subprocess.*crash" bot.log` |

### 6.3 回滚（任一指标超阈值）

```bash
# 1. 立即回滚：feature_flags.runtime 改回 nodejs
echo '[agent_runtime]
runtime = "nodejs"' > config/feature_flags.toml

# 2. 重启进程（pickup 新配置）
systemctl restart wmessage-agent  # 或 docker restart

# 3. 监控老 Rust 逻辑接管
tail -f bot.log  # 看 audit_event 恢复
```

回滚后保留 Python 代码（不删），下次升级继续用。

### 6.4 数据库迁移升级

```bash
# 新版本含新表 / 新列时
python -c "from agent_core.db import connect, migrate; \
  c = connect('data/agent.db'); \
  migrate(c, 'db/migrations'); \
  c.close()"

# 或 main.py 启动时自动跑（已实现）
python main.py
# → 检测到 _migrations 表里没有的新 migration 自动执行
```

---

## 7. 回滚策略

### 7.1 三层回滚

| 层级 | 触发条件 | 动作 | 生效时间 |
|---|---|---|---|
| L1 业务 | 单 Skill 报错率高 | feature_flags.disabled_skills += [...] | < 1min |
| L2 用户 | 单用户报错 | feature_flags.python_agent_users -= [user_id] | < 1min |
| L3 全量 | 全量 error > 10% | feature_flags.runtime = "nodejs" | < 1min |

### 7.2 代码回滚（Git）

```bash
# 回滚到上一个 tag
git checkout v0.2.0
pip install -e .
pytest tests/

# 完全回滚到 Rust agent（保留 Python 代码作下一轮升级用）
git checkout main -- bot_py.rs  # 仅回滚 Rust 侧
```

### 7.3 数据回滚（SQLite）

```bash
# 导出数据
sqlite3 data/agent.db ".dump" > backup-$(date +%F).sql

# 回滚到昨天
cp data/agent.db.bak data/agent.db  # 假设有每日 backup
```

---

## 8. 故障排查

### 8.1 常见错误

#### 启动失败：`ModuleNotFoundError: No module named 'httpx'`

```bash
pip install httpx  # 或
pip install -e ".[llm]"
```

#### 启动失败：`LLMAuthError: MINIMAX_API_KEY 未设置`

```bash
export MINIMAX_API_KEY="sk-..."
```

或检查 `feature_flags.toml` 的 `[llm] primary` 配置。

#### chat 无响应：subprocess 崩溃

```bash
# 看 stderr 日志
tail -f data/bot-py.stderr.log

# 常见原因：
# - LLM API 限流（429）→ 切到 fallback
# - audit SQLite 锁 → 重启进程
# - 内存泄漏 → 重启 + 升级
```

#### Skill 不命中：intent_router 返回 None

```bash
# 1. 检查 builtin skills 是否注册
python -c "from agent_core.skills.builtin import BUILTIN_SKILLS; print(list(BUILTIN_SKILLS.keys()))"

# 2. 检查自定义 SKILL.md
ls -la ~/.openclaw/skills/*/SKILL.md

# 3. 启用 L2 LLM 路由（在 feature_flags.toml）
echo "[features]
llm_l2_router = true" >> config/feature_flags.toml
```

#### file_hash 失败：`FileNotFoundError`

agent_loop 的 verify 步骤需要真实文件。检查 task 的 verify_hint.path 是否正确。

#### Verify 总是失败：replan 耗尽

```bash
# 1. 看 audit_event.task.fail 原因
sqlite3 data/agent.db "SELECT message FROM audit_event WHERE event='task.fail' ORDER BY ts_ms DESC LIMIT 5"

# 2. 临时绕过 max_replans
# harness_policy.toml
[replan]
max_replans_per_graph = 20
```

### 8.2 性能问题

#### IPC 延迟高（>100ms）

- 检查 LLM 调用是否频繁（fallback chain 是不是回退到慢模型）
- 检查 SQLite 写入是否阻塞（audit 关闭 `audit_sqlite_enabled = false` 试试）

#### 内存增长

- audit SQLite 太大：清理 30 天前的 event
- LLM provider httpx.Client 没关：每条 chat 创建新 client 会泄漏（待 Phase 4 修）

### 8.3 调试日志

```bash
# 实时日志
tail -f data/bot.log | grep -E "ERROR|WARN|skill.start|tool.call"

# 按 session 过滤
grep "session_id=s-123" data/bot.log | tail -50

# 按 skill 过滤
grep "skill=minimax-docx" data/bot.log
```

---

## 9. 安全注意

### 9.1 API Key 管理

- **永不 commit 到 git**：用 `.gitignore` 排除 `*.env`
- **永不写到日志**：audit_logger 自动屏蔽 `api_key` 字段
- **用 secret manager**：生产环境强烈推荐
- **定期轮换**：至少每 90 天一次

### 9.2 文件权限

```bash
# 数据目录权限（仅 owner 可读写）
chmod 700 data/
chmod 600 data/agent.db

# 日志目录权限
chmod 700 logs/
chmod 600 logs/*.log
```

### 9.3 Sandbox / 文件权限

- **run_python**（Phase 4+ 启用）：用 RestrictedPython / subprocess + setrlimit
- **文件访问**：仅允许白名单目录（`data/output/` / `AI_Gen_Files/`）
- **网络访问**：仅允许 LLM API endpoint + 内部服务

### 9.4 Tool 黑名单

已硬编码（`agent_core/tool_guard.py`）：

```python
DEFAULT_BLACKLIST = {
    "create_word_revisions",  # 仅 Skill 内
    "link_file_to_task",       # 仅 Skill 末尾
}
```

新增高风险 tool 时务必加入黑名单。

### 9.5 Audit 不变性

`audit_event` 表应设为 append-only（生产部署）：

```sql
-- 防止误删 audit
REVOKE DELETE, UPDATE ON audit_event FROM PUBLIC;
GRANT INSERT, SELECT ON audit_event TO wmessage_agent;
```

---

## 10. 性能基线

### 10.1 单元测试基线

```
Phase 1: 165 tests / 0.30s
Phase 2: 149 tests / 0.32s  
Phase 3: 119 tests / 0.56s
─────────────────────────
Total:  433 tests / 1.11s (无 slow)
        433 tests / 61s  (含 16 个 slow 集成)
```

### 10.2 LLM 调用延迟基线

| 模型 | avg | P95 | P99 |
|---|---|---|---|
| MiniMax-M3 (主) | 800ms | 1.5s | 3s |
| Kimi (fallback 1) | 1.2s | 2s | 4s |
| DeepSeek (fallback 2) | 600ms | 1.2s | 2.5s |

FallbackChain 平均延迟 = primary（P95，因为 90% 命中主）。

### 10.3 工具执行延迟基线

| Tool | avg |
|---|---|
| `doc_make_word`（python-docx） | 200ms |
| `doc_make_excel`（openpyxl） | 150ms |
| `run_command`（subprocess） | 50ms + 命令执行时间 |
| `file_hash`（hashlib） | < 5ms for <1MB |
| `eval_assert`（裸 eval） | < 1ms |

### 10.4 IPC 延迟基线

| 操作 | 延迟 |
|---|---|
| 启动（cold start） | 200-500ms |
| 启动（warm / keep-alive） | < 50ms |
| 单 ping / 单 chat（无 LLM） | < 5ms |
| chat（含 LLM 调用） | LLM 延迟 + 50ms IPC |

---

## 附录 A：目录结构

```
wmessage-agent-new/
├─ main.py                            # 进程入口
├─ pyproject.toml                     # Python 项目配置
├─ README.md
├─ docs/
│  ├─ SKILL_BODY.md                  # Skill 编写指南
│  ├─ OPERATIONS.md                  # 本文档
│  ├─ wmessage-agent-spec.md         # 完整 spec
│  └─ adr/
│     ├─ 0001-rust-to-python-agent.md
│     ├─ 0002-prev-agent-loop.md
│     └─ 0003-wmessage-agent-python-architecture.md
├─ agent_core/  (16 modules)
├─ llm_provider/  (4 modules)
├─ ipc/  (server.py)
├─ db/
│  ├─ schema.sql
│  └─ migrations/0001_initial.sql
├─ tests/  (15 files, 433 tests)
├─ config/
│  └─ feature_flags.toml              # 用户配置
└─ data/                              # 运行时生成（gitignored）
   ├─ agent.db                        # SQLite
   ├─ bot.log                         # 审计文件
   └─ output/                         # doc_make_* 输出
```

## 附录 B：常用命令速查

```bash
# 启动
python main.py

# 测试
pytest tests/                          # 全部
pytest tests/ -m "not slow"            # 仅单元
pytest tests/ --tb=short               # 短 traceback
pytest tests/test_planner.py -v        # 单文件详细

# 数据库
sqlite3 data/agent.db ".tables"        # 列表
sqlite3 data/agent.db "SELECT * FROM audit_event ORDER BY ts_ms DESC LIMIT 10"

# 日志
tail -f data/bot.log                   # 实时
grep ERROR data/bot.log | tail -20     # 错误

# 配置
cat config/feature_flags.toml
```

---

> 最后更新：2026-08-22  
> 维护者：小九  
> 反馈：飞书直接 @ 老板
