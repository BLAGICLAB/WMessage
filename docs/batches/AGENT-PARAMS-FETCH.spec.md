# Batch Spec: AGENT-PARAMS-FETCH

## 目的

两笔已验证的改动合批（worktree_clean 约束 + 两批改动同为工作区剩余未提交内容）：

1. **Agent 运行参数统一**（老板拍板）：「调用工具上限（全域熔断）」与「子 agent 工具调用预算」两条重复设置合并——删 `subagent_max_tool_calls`，子 agent 工具调用预算同源 `max_function_calls`（LLM 显式给的预算仍可覆盖）。
2. **fetch_url 全挂修复**：reqwest 0.13 `resolve_to_addrs` 在 https 场景按 addr 自带端口连接（URL 端口不覆盖），`check_public_url` 的 80 占位端口导致所有 https 抓取 TLS 发去 80 端口被回 InvalidContentType。修复 + 错误链展开诊断改进。

## 人类可读摘要

- families: agent-params-unify + web-fetch-port（两笔独立验证过的改动）
- 预估 diff: 6 files / +86/-50 lines
- 验证：`cargo test --lib bot::` 168 全绿（含新增 with_url_port 单测、真实网络 fetch_text 端到端 FETCH OK）；tsc 干净；SettingsPage 47 vitest 全绿
- 定位证据：bot.log 真实失败日志 + /tmp 独立复现（80 占位 → InvalidContentType；443 正确端口 → 200 OK）

## findings 原文核实

**① bot_web.rs check_public_url（high，真）**：`lookup_host((host, 80))` 端口占位 + `with_url_port` 缺失 → https 全挂。修法：出口重写 addr 端口为 URL 真实端口。
**② bot_web.rs 错误诊断（改进）**：reqwest 0.12+ Display 吞 source 链。修法：`err_chain` 逐层展开，send/read 两处 map_err 接入。
**③ bot/config/types.rs + commands.rs（统一）**：删 `subagent_max_tool_calls` 字段（BotConfig + BotConfigView + 落盘钳制 + 视图映射）。
**④ bot/params.rs（统一）**：`resolve_subagent_budget` 的 `max_tool_calls` 改读 `cfg.max_function_calls`；参数表删 `subagent.max_tool_calls` 条目；测试改写 + 新增缺省单测。
**⑤ SettingsPage.tsx（统一）**：删「子 agent 工具调用预算」输入行（6 处），「调用工具上限」hint 注明子 agent 同源。

## 修法约束

- SSRF 校验语义不变：仍逐跳校验 + 钉 IP 堵 DNS TOCTOU，仅端口重写
- `SubagentBudget` 结构与落库 budget_json 不动（LLM 显式预算/旧数据兼容）
- 老配置文件里的 `subagentMaxToolCalls` key 由 serde 忽略，零迁移

## Stop 条件

gate FAIL 且非 spec 声明调整可解即停。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "AGENT-PARAMS-FETCH",
  "family": "agent-params-unify+web-fetch-port",
  "expected_files": [
    "src-tauri/src/bot/config/types.rs",
    "src-tauri/src/bot/config/commands.rs",
    "src-tauri/src/bot/params.rs",
    "src-tauri/src/bot_model_loop.rs",
    "src-tauri/src/bot_web.rs",
    "src/components/SettingsPage/SettingsPage.tsx"
  ],
  "max_lines_added": 100,
  "max_lines_removed": 60,
  "findings": [
    {"id": "APF-a", "file": "src-tauri/src/bot_web.rs", "line": 935, "fix": "with_url_port 重写 addr 端口为 URL 真实端口，修 https 抓取 TLS 发 80 端口全挂"},
    {"id": "APF-b", "file": "src-tauri/src/bot_web.rs", "line": 20, "fix": "err_chain 展开 reqwest source 链，send/read 错误接入诊断"},
    {"id": "APF-c", "file": "src-tauri/src/bot/config/types.rs", "line": 220, "fix": "删 subagent_max_tool_calls 字段（两结构体 + 默认值）"},
    {"id": "APF-d", "file": "src-tauri/src/bot/params.rs", "line": 48, "fix": "resolve_subagent_budget 工具调用同源 max_function_calls；参数表删条目"},
    {"id": "APF-e", "file": "src/components/SettingsPage/SettingsPage.tsx", "line": 2137, "fix": "删子 agent 工具调用预算行，调用工具上限 hint 注明同源"}
  ],
  "assertions_min": {
    "src-tauri/src/bot_web.rs": 0
  },
  "ocr_plan": {
    "rounds": 0,
    "timeout_seconds": 0,
    "expected_max_comments": 0
  },
  "stop_conditions": [
    "gate_fail"
  ]
}
```

验证命令

```bash
cd src-tauri && cargo test --lib bot::
npx vitest run src/components/SettingsPage.test.tsx
```
