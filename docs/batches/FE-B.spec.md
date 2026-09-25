# Batch Spec: FE-B

## 目的

落地用户拍板的 B 类决策 5 项（frontend 域，2026-09-25 拍板）：#19 迁移失败跳种子（A）、#18 delete 抛错仍合并广播（B）、#21 新 confirm 覆盖前自动拒旧（B）、#22 围观执行期 execWatchRef 只拦 Send（B）、#20 skip 语义注释文档化（A）。对应 PHASE2-TRIAGE §4 B 类权威清单 #18/#19/#20/#21/#22。

## 人类可读摘要

- family: frontend-bclass-ux-semantics（批内 5 处独立改动，修复设施不共享——拍板落地批，如实声明）
- 覆盖: B 类 5 项（4 实修 + 1 注释文档化）
- 预估 diff: 6 文件 / +195/-15（无新文件）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 5

## 逐条处置（原文均已核 + 用户拍板 2026-09-25）

**实修 ① src/App.tsx:193（C5-FE-01 残余，用户拍 #19=A）**：legacy localStorage 迁移失败（JSON.parse 或 upsertTasks 抛错）被空 catch 吞后 `!migrated` → `upsertTasks(SEED)` 照常落种子——下次启动库非空不再进迁移分支，legacy 数据永久 orphan。
**修法**：catch 改标记 `migrateFailed=true` + console.error 留痕；种子落库条件改 `!migrated && !migrateFailed`——迁移失败时保留 legacy 待下次启动重试、跳过 SEED。removeItem 维持「成功迁移后才删」（空数组 legacy 仍删，行为不变）。
**ripple**：无（初始化块内闭环）。

**实修 ② src/App.tsx:308（C5-FE-01 残余，用户拍 #18=B）**：tasks-updated 非持久分支 `await upsertTasks(upserts); await deleteTaskRows(deletes);`——delete 抛错 → handler 异常 → queue catch → UI 不合并、不广播（upserts 已落盘 = 半成功）。
**修法**：deleteTaskRows 包 try/catch + console.error——delete 失败不阻断合并广播（失败行暂留 UI，后续 tasks-updated/tasks-changed 事件自愈；storage 层已 alert）；upsert 失败维持抛错走 queue catch（现状，upsert 失败时合并无意义）。
**ripple**：无。

**实修 ③ src/components/ConfirmMap/ConfirmMap.tsx:52（C5-FE-02 残余，用户拍 #21=B）**：新 bot-confirm 直接覆盖未响应 pending，旧 id 靠后端 60s 超时兜底拒（体验差语义不清）。
**修法**：listen 回调里 `setPending(p)` 前检查 `pendingRef.current`：存在且 `id !== p.id` → fire-and-forget `invoke("bot_confirm_response", { requestId: 旧 id, approved: false, always: null })`（.catch console.error 留痕——后端 oneshot+60s 兜底使重复拒收幂等安全），然后才覆盖。用户未看到的旧请求立即被拒，与「切换前自动拒旧」拍板一致。
**ripple**：无（组件内闭环）。

**实修 ④ src/components/ChatPanel/ChatPanel.tsx:416（C5-FE-06 残余，用户拍 #22=B；后端已核 bot_chat.rs:683 ChatGuard 会话级防重入在位——同会话并发软拒「请稍候再发」）**：围观执行会话期间用户可 Send 与 bot_execute_task 并发同会话（响应交错 + 执行收尾 bot_history_load 冲掉围观期输入显示）。
**修法**：新增 `execWatchRef`（当前围观中的执行会话 sid）：openExecSession(sid) 时置值；execute-task 收尾 .then 中该 sid 的 bot_history_load 刷新完成后清除；send 的守卫链加 `execWatchRef.current === sessionId` 拦截 + addHint 提示「⏳ 执行进行中，围观模式暂不能发送」。切换会话自由（busy 锁不涉）；斜杠命令（/stop）在守卫前不拦。
**ripple**：无（组件内；ChatPanel 无组件级测试 harness，行为由 tsc + 实现审查担保——spec 声明，与既有 constants/Fold/RichText 独立测试不冲突）。

**实修 ⑤ src/components/ArtifactBatchDialog.tsx:79（C5-FE-02 残余，用户拍 #20=A）**：skip 纯前端 setReady(null)，无永久 dismiss 路径（后端无 ack 协议，skip 后登记表保留、下次同任务执行重弹）。
**修法**：skip 处注释文档化语义（skip=这次不绑、下次再问；无永久 dismiss 为已知限制，新增 dismiss command 属扩 scope 未取）。零行为变更。
**ripple**：无。

## 测试

1. App.test.tsx +2：
   - 「legacy 迁移失败 → 跳过种子落库、legacy 保留」：db_load 返 [] + localStorage 置 legacy + 首次 db_upsert reject → 断言 db_upsert 仅 1 次调用且非 SEED 内容 + console.error
   - 「tasks-updated delete 失败仍合并广播」：db_upsert resolve + db_delete reject → 事件后新行上屏 + console.error（handler 不进 queue catch 的未处理 rejection）
2. ConfirmMap.test.tsx +1：「新 confirm 到达自动拒旧」：先到 confirm A（不响应）→ 到 confirm B → 断言 invoke("bot_confirm_response", { requestId: A.id, approved: false }) 被调 + pending 显示 B
3. ArtifactBatchDialog / ChatPanel：#20 纯注释、#22 无 harness——spec 声明

## 红线

- family 如实声明：批内 5 处独立改动，修复设施不共享
- 不动后端任何文件（#22 前置已核 ChatGuard 在位，不需要后端改动）
- #19 只改初始化块语义，不碰 res.ok 读失败分支（既有 fail-closed 保持）
- #21 拒旧必须 fire-and-forget（不 await，不阻塞新 confirm 上屏）
- 新注释不引用审计批次号

## spec 起草后自查三条

1. expected_files：src/App.tsx + src/App.test.tsx + src/components/ConfirmMap/ConfirmMap.tsx + src/components/ConfirmMap/ConfirmMap.test.tsx + src/components/ChatPanel/ChatPanel.tsx + src/components/ArtifactBatchDialog.tsx = 6（全路径已列）
2. budget：A 类（App 初始化块重构 +14/-8；#18 try/catch +7/-1；ConfirmMap 拒旧 +12/-1；ChatPanel ref+守卫 +12/-2；skip 注释 +5/-1）+ B 类（App.test 2 用例 ≈ +90；ConfirmMap.test 1 用例 ≈ +45）合计 ≈ +195/-13，上限 +270/-30；无新文件
3. fix 字段 ripple：全部组件内闭环；#22 后端 ChatGuard 已核（bot_chat.rs:683）；#19/#18 无跨文件 ripple

## 自主执行规则

spec 经用户拍板（B 类 5 项方向 2026-09-25）后：
- agent 全权执行至 commit，不中途报 status / exit / diff
- 例外只有 stop 条件触发
- 完成时一次报：hash + 批定性 + OCR 轮次 + comments 处置 + push 两值
- 过程证据落 ~/.openclaw/cache/FE-B/，不喊人

## Stop 条件（触发即停，报用户）

- compile_failure（无 X 形态退路）
- architecture_blocker（签名/调用点超预算）
- family_heterogeneity
- new_high_different_root（OCR r1 出的 high 非本批根因）
- gate_fail（batch-verify FAIL 且非 spec 声明调整可解）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "FE-B",
  "family": "frontend-bclass-ux-semantics",
  "expected_files": [
    "src/App.tsx",
    "src/App.test.tsx",
    "src/components/ConfirmMap/ConfirmMap.tsx",
    "src/components/ConfirmMap/ConfirmMap.test.tsx",
    "src/components/ChatPanel/ChatPanel.tsx",
    "src/components/ArtifactBatchDialog.tsx"
  ],
  "max_lines_added": 270,
  "max_lines_removed": 30,
  "findings": [
    {"id": "C5-FE-01.19", "file": "src/App.tsx", "line": 193, "fix": "legacy 迁移失败 catch 标记 migrateFailed + console.error；种子条件 !migrated && !migrateFailed——失败保留 legacy 待下次重试，不再落 SEED 造成永久 orphan。ripple：无"},
    {"id": "C5-FE-01.18", "file": "src/App.tsx", "line": 308, "fix": "tasks-updated 非持久分支 deleteTaskRows 包 try/catch + console.error——delete 失败不阻断合并广播（失败行暂留自愈）；upsert 失败维持抛错。ripple：无"},
    {"id": "C5-FE-02.21", "file": "src/components/ConfirmMap/ConfirmMap.tsx", "line": 52, "fix": "listen 回调 setPending 前自动拒旧：pendingRef 存在且 id 不同 → fire-and-forget bot_confirm_response(旧, false) + catch 留痕。ripple：无"},
    {"id": "C5-FE-06.22", "file": "src/components/ChatPanel/ChatPanel.tsx", "line": 416, "fix": "execWatchRef 围观执行会话守卫：openExecSession 置值 / 收尾 history_load 后清除 / send 守卫链拦截 + addHint；切换会话自由；后端 ChatGuard 已核（bot_chat.rs:683）。ripple：无（无 harness，spec 声明）"},
    {"id": "C5-FE-02.20", "file": "src/components/ArtifactBatchDialog.tsx", "line": 79, "fix": "skip 语义注释文档化（这次不绑下次再问；无永久 dismiss 为已知限制）。ripple：无（零行为变更）"}
  ],
  "assertions_min": {
    "src/App.test.tsx": 4,
    "src/components/ConfirmMap/ConfirmMap.test.tsx": 3
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 5
  },
  "stop_conditions": [
    "compile_failure",
    "architecture_blocker",
    "family_heterogeneity",
    "new_high_different_root",
    "gate_fail"
  ]
}
```

## 提交信息骨架

```
fix(fe): FE-B — frontend-bclass-ux-semantics（B 类拍板落地 5 项）

【family】批内 5 处独立改动，修复设施不共享（拍板落地批，如实声明；用户拍板 2026-09-25：#19=A、#18=B、#21=B、#22=B、#20=A）
【实修 5 处】
- App.tsx:193 迁移失败跳过种子、保留 legacy 待重试（不再永久 orphan）
- App.tsx:308 tasks-updated delete 失败仍合并广播（失败行暂留自愈）
- ConfirmMap.tsx:52 新 confirm 到达自动拒旧（fire-and-forget + catch 留痕）
- ChatPanel.tsx:416 execWatchRef 围观期只拦 Send（切换会话自由；后端 ChatGuard 已核）
- ArtifactBatchDialog.tsx:79 skip 语义注释文档化（零行为变更）
【行为变更】迁移失败启动不再落种子；delete 失败行暂留 UI；旧 confirm 即时被拒；围观期 Send 被拦并提示
【测试】+3（App ×2 / ConfirmMap ×1）
【OCR】r1：<N> comments <处置>
【基线】D2: files=6(+A/-R) asserts=… tests=…
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/FE-B.spec.md python3 scripts/batch-verify.py docs/batches/FE-B.spec.md
```
