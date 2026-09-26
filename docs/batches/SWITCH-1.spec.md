# Batch Spec: SWITCH-1

## 目的

产品变更（用户拍板 2026-09-26）：agent 流式回复期间，主窗/挂件聊天区**允许实时切换对话、新建对话**（现状 busy 锁全禁）；约束 = 回复必须仍落在发起它的会话，不串台。

## 现状机制（溯源）

- `bot_chat` invoke 已带 `sessionId`；六个流式事件 payload 均带 sessionId；最终回复由前端 `persistHistory(sid)` **按发起会话落库**；UI 更新已有 `sessionIdRef === sid` 守卫（runChat 收尾）。→ 回复归属在后端契约层已是正确的，busy 锁只是前端单流式缓冲（streamingMeta 全局单份 + 增量按「当前视图会话」过滤）的配套简化。
- 三处 busy 锁：switchSession:645 / newSession:669 / deleteSession:683。

## 修法（最小而准，单交互回复不变）

1. **回复绑定会话显式化**：新增 `busySidRef`（enterBusy(sid) 记录回复所属会话，exitBusy 清除）；/stop 目标从「当前视图会话」改为 `busySidRef ?? 当前视图`（切换后 /stop 仍停真正在回复的那个）。
2. **解锁**：switchSession / newSession 移除 busy 拦截；deleteSession 仅拦「删除正在回复的会话」（其余会话随便删）。send 在 busy 时由静默 return 改为 hint 指引（可切换，发送等完成或 /stop）；/retry、/compact 维持 busy 拦截（同为交互式 LLM 操作，保持单飞行）。
3. **流式路由改造**（保证不乱）：
   - thinking/tool/skillFailed 元数据：`sid === busySidRef` 即累加进 streamingMeta（**权威副本不随视图丢失**，收尾合并完整）；视觉 setMessages 仅 `sid === sessionIdRef.current`（原有气泡守卫仍在，双保险）。
   - text 增量：维持纯视觉（`sid === sessionIdRef.current` 才进 pending 缓冲）；切走期间增量丢弃是**安全的**——最终正文 = 后端 `full.text` 权威值，非增量拼接。
   - **切回正在回复的会话**：history_load 后补一个空 streaming 占位气泡，后续增量继续可见（收尾 full.text 校正全文）。
4. 执行会话围观（openExecSession / chat-open-session 排队）语义不动；/stop 围观期例外不动。

## 测试

- busy 中切换会话 → 允许；回复完成后 `bot_history_save` 落到**原会话** sid（不串台核心钉）。
- busy 中切走时向原会话发流式 delta → 新会话视图不出现该内容（无污染）。
- busy 中新建对话 → 允许（bot_session_create 被调）。
- busy 中删除正在回复的会话 → 拦截（bot_session_delete 不被调）；删其他会话允许。

## 红线

- bot_chat / 流式事件后端契约零改动（纯前端路由）
- 单交互回复不变（send 在 busy 时仍拦）；persistHistory 按 sid 落库语义不变
- 执行会话围观 / chat-open-session 排队 / 拍板 #22=B 语义不动

## spec 起草后自查三条

1. expected_files 2（ChatPanel.tsx + ChatPanel.test.tsx）
2. budget +180/-25（代码 ≈+60/-15，测试 ≈+120/-10）
3. fix 字段：组件内闭环，无后端/签名变更

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "SWITCH-1",
  "family": "chat-live-switch",
  "expected_files": [
    "src/components/ChatPanel/ChatPanel.tsx",
    "src/components/ChatPanel.test.tsx"
  ],
  "max_lines_added": 180,
  "max_lines_removed": 25,
  "findings": [
    {"id": "SWITCH-1", "file": "src/components/ChatPanel/ChatPanel.tsx", "line": 645, "fix": "busy 期解锁切换/新建（删除仅拦回复中会话）；busySidRef 绑定回复归属会话；流式元数据按 busySid 累加（视图无关）、text 增量纯视觉、切回补流式占位气泡；/stop 目标改 busySid；send busy 改 hint 指引"}
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
BATCH_SPEC=docs/batches/SWITCH-1.spec.md python3 scripts/batch-verify.py docs/batches/SWITCH-1.spec.md
```
