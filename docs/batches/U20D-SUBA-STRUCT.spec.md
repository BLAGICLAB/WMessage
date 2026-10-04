# Batch Spec: U20D-SUBA-STRUCT

## 目的

图标迁移批 5（方案 `docs/UI-ICON-PLAN-2026-10-04.md` 收官）：**🧩 子 agent 路由
契约结构化**——停止键/斜杠 /stop 的分派此前依赖会话标题 `🧩` 前缀
（`ChatPanel.startsWith("🧩")` → cancel_subagent / bot_stop），标题即契约导致
emoji 永远去不掉。本批把子 agent 身份落到 `bot_sessions.is_subagent` 结构化
字段，标题去 emoji 前缀（纯文字「子任务：X」），路由改读字段。

## 实现（两端同批）

- **后端 db**：`bot_sessions` 建表加 `is_subagent INTEGER NOT NULL DEFAULT 0`；
  存量库走 PRAGMA table_info + ALTER 迁移（同 tasks 列迁移先例）；`BotSession`
  加 `is_subagent: bool`（serde camelCase → `isSubagent`），load 带出；创建
  拆两变体——`bot_session_create_inner`（普通，=0，bot_chat 新对话与命令面
  不变）+ 新 `bot_session_create_subagent_inner`（=1，orchestrator runner
  setup 专用）。
- **后端 orchestrator**：runner 会话标题与 spawn 子卡标题去 🧩 前缀；runner
  建会话改走 subagent 变体（同一事务内，与 set_subagent_session/assignee
  回填同锁）。
- **前端**：`Session.isSubagent?: boolean`；路由改
  `currentSession?.isSubagent === true || currentTitle.startsWith("🧩")` ——
  **存量旧会话兜底**：迁移前创建的子 agent 会话行默认 0 但标题带前缀，标题
  判定保留为兼容回退，新旧会话路由都不丢。
- 聊天进度事件里的「📋 任务：…」标题为另一族展示文案（无路由依赖），不动。

## 测试

- 后端新增 `bot_sessions.rs` 单测：subagent 变体写 1 / 普通创建默认 0 /
  load 读回一致 / 标题无前缀；orchestrator 源锁测试改造——旧「emoji 前缀
  3 处计数锁」换成「subagent 变体接线锁 + 前缀清零锁」（concat! 防自匹配，
  dead_command_tests 同款手法）；SUBA-1 卡片标题断言去 emoji。
- db/mod.rs 测试夹具 `setup_bhs_db` 建表补列（5 处位置 INSERT 补第 5 列）。
- 前端：ChatPanel 路由断言所在用例（📋 任务 payload fixtures）不涉本契约，
  零改动。

## 红线核对

- 存量库迁移默认 0 + 前端标题回退 = 旧会话路由零丢失；
- 普通「新对话」创建路径签名不变（bot_chat / 命令面零改动）；
- SQL 全参数绑定（新 INSERT/SELECT 均参数化；ALTER 的列名为编译期常量）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U20D-SUBA-STRUCT",
  "family": "ui-icon-migration",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/U20D-SUBA-STRUCT.spec.md",
    "src-tauri/src/bot_orchestrator.rs",
    "src-tauri/src/db/bot_sessions.rs",
    "src-tauri/src/db/mod.rs",
    "src/components/ChatPanel/ChatPanel.tsx",
    "src/components/ChatPanel/types.ts"
  ],
  "max_lines_added": 260,
  "max_lines_removed": 60,
  "max_new_files_lines": 150,
  "findings": [
    { "file": "src-tauri/src/db/bot_sessions.rs", "note": "is_subagent 字段 + 创建双变体（inner=普通 / subagent_inner=标记）；load 带出；db 层单测锁写读" },
    { "file": "src-tauri/src/bot_orchestrator.rs", "note": "runner 建会话走 subagent 变体（同一事务）；标题去 emoji；源锁测试从「前缀计数」改「变体接线 + 前缀清零」" },
    { "file": "src/components/ChatPanel/ChatPanel.tsx", "note": "路由改 isSubagent 字段优先 + 旧标题前缀兜底（迁移前旧会话零丢失）" },
    { "file": "src-tauri/src/db/mod.rs", "note": "PRAGMA+ALTER 迁移（tasks 列迁移同款）；测试夹具补列" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
cargo nextest run -E 'test(bot_sessions) or test(subagent) or test(orchestrator)'  # 41 绿
bash scripts/test-all.sh && bash scripts/test-fast.sh                              # exit 0
python3 scripts/batch-verify.py docs/batches/U20D-SUBA-STRUCT.spec.md
```
