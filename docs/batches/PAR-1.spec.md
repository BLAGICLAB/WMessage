# Batch Spec: PAR-1

## 目的

用户指令：agent 回复期间切换到新对话后，**能在新对话发消息 / 执行任务卡 / 选模型发消息**——即**并行会话回复**。现状：切换自由（SWITCH-1），任务卡执行后端本就不吃锁（ChatGuard 按会话隔离，`running: HashSet<sessionId>`），但前端**全局单飞行**（单一 busy + 单份 streamingMeta）把发送拦死、流式元数据只有一份。

## 修法（按会话隔离，ChatPanel.tsx）

1. **在途集合替代全局锁**：`inflightSids`（state，驱动 UI）+ `inflightRef`（同步镜像，监听器判断）。enterChat(sid)/exitChat(sid) 维护；`busy` 语义改为**按视图会话**（`viewedBusy = inflightSids.has(sessionId)`）。
2. **流式元数据按会话隔离**：`streamingMeta` 单对象 → `streamingMetaMapRef: Map<sid, meta>`；thinking/tool/skillFailed 事件按 `inflightRef.has(sid) || sid===viewed` 累加（并行回复各自累积互不串台；执行围观经 viewed 路径不变）；收尾并入后删除条目。
3. **发送按会话判定**：`send` 拦截条件从「任意 busy」改为「**本会话**在途」——不同会话可并行发送；同会话重复发送仍拦（hint 文案更新）。`runChat` 双保险守卫同步改。
4. **/stop、/retry、/compact、移除回复、➕、停止键、placeholder**：全部从「任意 busy」改为「视图会话在途」（viewedBusy）——S1 回复中在 S2 里这些功能全部可用。
5. **执行会话跳转**：chat-open-session 仅当「正在围观流式回复中的会话」时排队，否则立即跳转（并行世界下围观不再被锁）。
6. 删除会话守卫：拦「删除任一在途回复的会话」（inflightRef.has(sid)）。

## 红线

- 后端契约零改动（bot_chat/流式事件/ChatGuard 本就按会话隔离）
- persistHistory 按 sid 落库、UI 更新 sessionIdRef===sid 守卫不变
- SWITCH-1 行为保持：切换/新建自由、删除在途会话被拦、切回补占位气泡（条件改 inflightRef.has）

## 测试

- 新增：并行回复——S1 回复挂起 → 切 S2 发消息 → 两次 bot_chat（不同 sessionId）→
  各自落库不串台。
- 既有 SWITCH-1 测试全绿（行为保持）。

## spec 起草后自查三条

1. expected_files 2（ChatPanel.tsx / ChatPanel.test.tsx）
2. budget +200/-100（实测 +184/-82，校正 ×1）
3. fix 字段：组件内闭环，后端零改动

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "PAR-1",
  "family": "chat-parallel-replies",
  "expected_files": [
    "src/components/ChatPanel/ChatPanel.tsx",
    "src/components/ChatPanel.test.tsx"
  ],
  "max_lines_added": 200,
  "max_lines_removed": 100,
  "findings": [
    {"id": "PAR-1", "file": "src/components/ChatPanel/ChatPanel.tsx", "line": 70, "fix": "全局单飞行改按会话隔离：inflightSids/inflightRef 替代 busy/busySidRef；streamingMeta 按 sid Map 隔离；发送//stop//retry//compact/停止键/placeholder/移除/➕ 全部按视图会话判定；执行跳转仅围观流式时排队；并行 bot_chat 各自落库"}
  ],
  "assertions_min": {},
  "ocr_plan": {
    "rounds": 0,
    "timeout_seconds": 0,
    "expected_max_comments": 0
  },
  "stop_conditions": ["compile_failure", "architecture_blocker", "family_heterogeneity", "new_high_different_root", "gate_fail"]
}
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/PAR-1.spec.md python3 scripts/batch-verify.py docs/batches/PAR-1.spec.md
```
