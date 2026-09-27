# Batch Spec: SUBA-3

## 目的

用户指令：以 SUBAGENT-ORCHESTRATION-DESIGN-2026-09-27.md 为基线实施 A 期第三批。
SUBA-3 = 设计 §11 A3「预算/并发/前端」：并发上限（per-session 2 / global 3）排队
FIFO（超限 queued 不失败）；task_tool_calls 预算计数（dispatch 层累计，触顶拒新调用）
；前端：任务卡三字段（assignee/budget/result）投影、停止按钮 = cancel_subagent、
按 subagent_id 过滤围观事件。前置：SUBA-2 已接 runner（墙钟/轮数预算已生效）。

## 修法（按层）

1. **并发排队**（bot_orchestrator.rs）：AppState 增 subagent_running 计数 + 等待队列
   VecDeque（ FIFO）。runner 开跑前 acquire 槽位：global ≥3 或该 parent_session ≥2
   → 入队等待（tokio::sync::Notify 唤醒）；check 的 progress 带排队位置。取消的排队
   项出队即弃。reset：终态释放槽位 + notify。
2. **tool_calls 预算强制**（dispatch.rs）：SUBAGENT_SESSIONS ctx 已带 budget；
   ctx 增 used_tool_calls 计数器（Mutex<usize>）——dispatch 白名单闸后累计，超
   max_tool_calls → 拒绝（reason=budget_tool_calls_exceeded 的 ToolResult，模型收
   到后自然收尾）。熔断双保险不变（model loop fuse）。
3. **前端**（ChatPanel.tsx / TaskCardContent / types）：
   - types.ts：Task 增 assignee?/budget?/result?（camelCase 对齐 wire）。
   - 任务卡：result 非空时折叠展示（状态 + summary 截断）；budget 徽标（轮数/墙钟）；
     assignee = 子会话 id 不直接展示（串链键），卡片渲染子任务样式已有 🧩 前缀标题。
   - 停止按钮：子卡流式中停止 → invoke cancel_subagent（taskId）替代 bot_stop。
   - 围观过滤：subagent-finished 事件按 parentSessionId 提示（轻量 toast/系统行）；
     事件流 sessionId 过滤既有机制沿用（PAR-1）。
4. **排队恢复**（轻量）：open_db 后 queued 且 created_at 超过 10min 的行标记
   failed(stale_queue)——重启丢队列的行不永挂（A 期不做重启续跑，设计 §3 持久化
   语义 = 可查可取消即可，勘误登记）。

## 红线

- 主会话/任务执行零行为变更：并发闸只作用于 runner；dispatch 计数只对子 agent 会话。
- vitest 既有 323 用例不动；新 UI 状态全部可选字段（undefined 安全）。
- ChatPanel 仅增量：不做大重构（RE1-MP02 刚落地）。

## 测试

- orchestrator：并发闸单测（3 全局/2 per-session 排队 FIFO 顺序、取消出队、终态释放）
  ——排队器抽纯结构 Queued 行为可 in-memory 测；tool_calls 计数累计/触顶拒绝。
- vitest：TaskCard 三字段投影（result 折叠/budget 徽标/无字段不渲染）；类型完整。
- 全量回归三层。

## spec 起草后自查三条

1. expected_files 9（orchestrator/dispatch/app_state/排队器 + ChatPanel 组 + 类型）
2. budget：修改 +650/-80
3. fix 字段：并发闸集中 orchestrator 新增并发模块；前端三字段纯投影

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "SUBA-3",
  "family": "subagent-orchestration",
  "expected_files": [
    "src-tauri/src/bot_orchestrator.rs",
    "src-tauri/src/bot/dispatch.rs",
    "src-tauri/src/tool_guard.rs",
    "src-tauri/src/app_state.rs",
    "src/types.ts",
    "src/components/TaskCardContent.tsx",
    "src/components/TaskCardContent.test.tsx",
    "src/components/ChatPanel/ChatPanel.tsx",
    "docs/rust-bot-architecture.md"
  ],
  "max_lines_added": 700,
  "max_lines_removed": 90,
  "findings": [
    {"id": "SUBA-3", "file": "src-tauri/src/bot_orchestrator.rs", "line": 1, "fix": "并发排队：SubagentGate（global 3/per-session 2 FIFO，Notify 唤醒，取消出队，check progress 带排队位）+ runner 接闸 + queued 超时清理"},
    {"id": "SUBA-3", "file": "src-tauri/src/bot/dispatch.rs", "line": 1, "fix": "tool_calls 预算：SubagentSessionCtx 计数器累计，触顶拒绝（budget_tool_calls_exceeded）"},
    {"id": "SUBA-3", "file": "src/components/TaskCardContent.tsx", "line": 1, "fix": "三字段投影：result 折叠卡（status+summary）、budget 徽标、undefined 安全"},
    {"id": "SUBA-3", "file": "src/components/ChatPanel/ChatPanel.tsx", "line": 1, "fix": "子卡流式停止按钮走 cancel_subagent；subagent-finished 按 parentSessionId 轻提示"}
  ],
  "assertions_min": {
    "src-tauri/src/bot_orchestrator.rs": 30,
    "src/components/TaskCardContent.test.tsx": 8
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 600,
    "expected_max_comments": 8
  },
  "stop_conditions": ["compile_failure", "architecture_blocker", "family_heterogeneity", "new_high_different_root", "gate_fail"]
}
```

## 验证命令

```
BATCH_SPEC=docs/batches/SUBA-3.spec.md python3 scripts/batch-verify.py docs/batches/SUBA-3.spec.md
cargo fmt --check && cargo check && npx tsc --noEmit && bash scripts/test-all.sh
```
