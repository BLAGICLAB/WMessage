# Batch Spec: U20D-A

## 目的

U20D-SUBA-STRUCT 的收尾裁剪（老板 2026-10-04 拍板：项目尚在测试期、数据即将
全部清空发布，不存在老数据/迁移兼容问题）：删除批 5 特意保留的「标题 🧩 前缀
路由兜底」——子 agent 会话路由从此**纯走 `bot_sessions.is_subagent` 结构化
字段**，生产代码中的 🧩 全部清零。

## 安全性论证（为何现在删是安全的）

- `chat-open-session` 事件只由 bot_chat 执行会话（manual/scheduled/batch，
  全部 is_subagent=0）发出，**子 agent 会话从不走补行路径**——补行行不需要
  标记，路由 false 恒正确；
- 子 agent 会话只经 `bot_sessions_load` 整表进入前端，行行携带 isSubagent；
- 兜底服务的对象（迁移前创建的 🧩 前缀旧会话）随数据清空不复存在。

## 改动

1. ChatPanel 路由删兜底：`isSubagent === true || startsWith("🧩")` →
   `isSubagent === true`（注释同步）；
2. subagent-finished 提示文案去 🧩 前缀（标题已无前缀，这是生产代码最后
   一处 🧩 装饰）；
3. types.ts Session 注释同步（isSubagent 是路由唯一依据；补行的执行会话
   本就不是子 agent，不带）。

## 保留确认（不在本批）

db/mod.rs 的 is_subagent ALTER 迁移**保留**——防个别未清空数据的测试机
（15 行无害防御）；句内单色 ✓/⌘K/errorHandler 原生兜底等其余保留项口径不变
（U20C-ICON1B 终扫清单）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U20D-A",
  "family": "ui-icon-migration",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/U20D-A.spec.md",
    "src/components/ChatPanel/ChatPanel.tsx",
    "src/components/ChatPanel/types.ts"
  ],
  "max_lines_added": 40,
  "max_lines_removed": 30,
  "max_new_files_lines": 80,
  "findings": [
    { "file": "src/components/ChatPanel/ChatPanel.tsx", "note": "路由纯走 isSubagent 字段（兜底删除的安全论证：chat-open-session 补行全是非子 agent 会话；旧前缀会话随数据清空消失）；subagent-finished 提示去 🧩" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
npx vitest --run                # 406/406
bash scripts/test-fast.sh       # exit 0
```
