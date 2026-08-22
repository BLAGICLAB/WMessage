# wmessage-agent-new

WMessage Agent 的 Python 内核。

## 定位

- **后端轻壳**（Tauri Rust）只做 IPC 转发 + 系统集成
- **Agent 业务**（本仓库）全部在 Python 子进程内跑
- **前端**（React）零改动

## 通信

stdin/stdout JSON-RPC 2.0（主通道）+ 本地 HTTP 4789（备选/调试）。

参见 `docs/wmessage-agent-spec.md §Y IPC 协议细节`。

## 主循环架构

Plan-Replan-Execute-Verify（PREV，内嵌 ReAct）。

参见 `docs/wmessage-agent-spec.md §X 主循环架构`。

## 模块

| 文件 | 角色 | Phase |
|---|---|---|
| `agent_core/planner.py` | 粗粒度 TaskGraph 生成（DAG）| 1 |
| `agent_core/verify.py` | 强校验 + 弱校验 | 1 |
| `agent_core/react_step.py` | ReAct 单步（思考 / 工具 / 观察）| 1 |
| `agent_core/types.py` | 共享 dataclass | 1 |
| `ipc/server.py` | JSON-RPC over stdin/stdout | 1 |
| `main.py` | 进程入口 | 1 |
| `agent_core/agent_loop.py` | PREV 主循环编排 | 2 |
| `agent_core/replanner.py` | 触发条件 + 重规划 | 2 |
| `agent_core/intent_router.py` | 关键词 L1 路由 | 2 |
| `agent_core/skill_runtime/` | Skill body 加载 + 调用 | 2-3 |
| `agent_core/tool_guard.py` | 黑名单拦截 | 2 |
| `agent_core/audit_log.py` | 8 类结构化事件 | 2 |
| `llm_provider/` | MiniMax-M3 / Kimi / DeepSeek | 3 |

## 快速跑通

```bash
# 安装
pip install -e ".[dev]"

# 单测
pytest -v

# 跑起来（手动喂 JSON-RPC 测试）
python main.py < test_request.json
```

## 当前阶段

**Phase 1（地基）**：IPC 骨架 + planner/verify/react_step 三个模块 + 单测。无业务 Skill 迁移，无 LLM 接入。
