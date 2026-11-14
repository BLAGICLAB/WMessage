# 页面刷新问题检测报告

- 日期：2026-11-11
- 范围：Wmessage 前端（`src/`）与后端刷新相关链路（`src-tauri/src/`）
- 方法：静态代码审查（读码 + 调用链回溯 + 后端落库顺序核对）
- 基线：`5add9495`（工作区干净，无未提交改动）

## 0. 结论摘要

「刷新」在本项目有三层含义，本次全查：

| 层 | 含义 | 结论 |
|---|---|---|
| A. 页面重载 | 窗口 reload / webview 重启后状态恢复 | **有 3 处真实缺口**（P1×2、P2×1），其余链路健壮 |
| B. 刷新按钮 | 各面板「刷新」按钮的语义正确性 | **有 2 处真实缺陷**（P1×1、P2×1） |
| C. 数据自愈 | 后端变更后前端是否自动收敛 | 基本健全，未见 P0 |

**未发现 P0（数据丢失 / 永久不可用级）缺陷。**

整体防护水平高于一般项目：竞态令牌（`useRaceSeq`）、判别式读失败契约（读失败 ≠ 空库）、
写并发闸（`mutating.chain` / `writeInFlight`）、序号去重闸（`noteSeq`）四套机制已成体系。
下列缺陷是这套体系里的**漏网点**，不是设计缺失。

---

## 1. P1 级缺陷

### P1-1 刷新后回到「最近更新的会话」，不是用户离开时所在的会话

**位置**：`src/components/ChatPanel/useChatHistory.ts:234`

```js
const cur = list[0]; // 按 updated_at 倒序，第一个即最近会话
setSessionId(cur.id);
```

**问题**：会话列表查询按 `updated_at DESC` 排序（`src-tauri/src/db/bot_sessions.rs:66`），
前端无条件取 `list[0]`。当前会话 id **没有任何持久化**——
`grep -rn "lastSession\|currentSession\|activeSession" src/` 只命中
`ChatPanel.tsx:109` 的 `lastSessionRef`（组件内 ref，重载即失）与两处无关变量名。

**触发条件**：用户在会话 A（较早创建）里工作，会话 B（较晚创建）因另一条链路
（子 agent 汇报轮 / 定时作业 / API 写入）被更新过 → 用户刷新 → 落点跳到 B，
A 里的上下文（输入框草稿以外的在途态、计划浮层、提问卡）全部不对。

**为何算 P1**：刷新是用户最常见的「重新看一眼」动作，落点跳错等于把用户丢进
一个陌生上下文；草稿虽按会话存了（`chatDrafts.ts`），但 `draftKey = sessionId ?? ""`
也随落点一起跳走。

**修法建议**：把当前会话 id 落 `localStorage`（与会话存在同 key 前缀下），
挂载时优先读它并校验其仍在 `bot_sessions_load` 返回列表内，不在则回退 `list[0]`。

---

### P1-2 刷新后回灌轮次的「正文以库为准」分支可能永久缺终稿

**位置**：`src/components/ChatPanel/useChatHistory.ts:100-110`

```js
const msgs = rowsToMsgs(rows ?? [], sid);
const last = msgs[msgs.length - 1];
if (last?.role === "user" && attempt < 6) {
  window.setTimeout(() => void loadFinalText(sid, attempt + 1), 300);
  return;
}
if (sessionIdRef.current === sid) setMessages(msgs);
```

**问题**：`loadFinalText` 的重试上限是 6 次 × 300ms = **1.8 秒**，
超时后直接 `setMessages(msgs)` 把「只有用户消息、没有 assistant 终稿」的残缺历史
当最终结果落屏，且**不再重试**。

**关键时序**：后端终稿落库**恒在** `bot-turn-status` 终态事件**之后**
（`src-tauri/src/bot_chat.rs:274-277` 注释明确：「恒在该回合 bot-turn-status 终态事件之后」）。
前端在终态事件里调 `loadFinalText`（`useChatStream.ts:337`），
此时后端可能还在写库。1.8 秒内写不完（DB 写锁竞争、大正文、慢盘）即命中。

**触发条件**：后端 DB 写锁被其他写者占用 > 1.8s（`db::lock::DB_WRITE_LOCK` 全局串行），
期间刷新页面 → 终稿永久缺失，直到用户再手动触发一次加载。

**为何算 P1**：用户看到的是「我刚问的那句话，机器人答的内容不见了」，
且不会自愈（没有重试、没有事件兜底——终态事件已经消费过了）。

**修法建议**：重试上限按「后端写锁已释放」而非固定次数决定；
或在 `bot-msg-upserted` 事件侧对「当前会话尾行是 user 且无对应 assistant 行」
保持监听直到终稿行到达（后端 `emit_row_upserted` 已在终稿落库后广播，可直接对齐）。

---

### P1-3 ServicesPanel 刷新无并发闸，旧响应可覆盖新响应

**位置**：`src/components/SettingsPage/ServicesPanel.tsx:102-118, 150-156`

```js
const refresh = async () => {
  try {
    setSnapshot(await invoke<ServicesSnapshot>("services_list"));
  } catch (e) { ... }
};
useEffect(() => {
  refresh();
  const timer = setInterval(refresh, 5000);   // ← 5s 轮询
  return () => clearInterval(timer);
}, []);
...
<button onClick={refresh} aria-label="刷新服务列表">   // ← 无 disabled / 无 loading
```

**问题**：三处缺陷叠加：
1. **无竞态令牌**——项目已有 `useRaceSeq` 工具（`src/lib/useRaceSeq.ts`），
   同类面板（`SchedulePage.tsx:78`、`NotificationsPage.tsx:351`、`MemoryPanel.tsx:191`、
   `UsageStatsCard.tsx:758`）全部在用，唯独此处没有。
2. **5s 轮询与手动刷新可并发**——`setInterval` 的 `refresh` 尚在飞，
   用户点「刷新」再发一次，慢的旧响应后到会覆盖新的快照。
3. **按钮无 `disabled` / loading 态**——对照 `LogsPanel.tsx:127`（有 `disabled={busy}`）
   与 `UsageStatsCard.tsx:821`（有 `disabled={loading}` + 旋转图标），此处可无限连点。

**为何算 P1**：本面板是「看 + 停」的服务控制面，pid / 运行时长 / 资源占用都在变，
快照回退会让用户对着过期 pid 点「停止」，或误判服务已死。

**修法建议**：引入 `useRaceSeq`（仓库既有工具，零新增依赖）；
`refresh` 加 `busy` 闸并把按钮 `disabled={busy}`。

---

## 2. P2 级缺陷

### P2-1 刷新失败后错误提示不清除

**位置**：`src/components/SettingsPage/ServicesPanel.tsx:106`、`:194`

```js
} catch (e) {
  handleCommandError(e, "services_list", { silent: true });
  setError(formatCommandError(e));       // ← 设了，但成功分支不清
}
...
{error && <p className="mt-2 text-xs text-[var(--danger)]">{error}</p>}
```

**问题**：`refresh` 成功分支只 `setSnapshot`，**没有 `setError("")`**。
一次失败后错误行常驻，直到组件卸载。同一页的 `stop` 流程里有 `setError("")`（`:123`），
说明是遗漏而非有意。

**对照**：`MemoryPanel.tsx:205`（`setLoadError("")`）、`SchedulePage.tsx:85`（`setLoadError(null)`）、
`NotificationsPage.tsx:357`（`setLoadError("")`）均在成功分支清错误。

**触发条件**：任意一次 `services_list` 失败（后端未起 / DB 锁超时），之后即使刷新成功，
红色错误行仍在。

---

### P2-2 UsageStatsCard 的 `loadOverview` 未受竞态令牌保护

**位置**：`src/components/SettingsPage/UsageStatsCard.tsx:739-755, 780-783`

```js
const loadOverview = useCallback(() => {
  setLoading(true);
  setError(null);
  Promise.all([invoke("usage_stats_daily",...), invoke("usage_stats_by_model",...)])
    .then(([d, m]) => { setDays(...); setModels(...); })
    .catch((e) => setError(String(e)))
    .finally(() => setLoading(false));
}, []);                    // ← 无 race 令牌

const refresh = useCallback(() => {
  loadOverview();
  loadModelDays();
}, [loadOverview, loadModelDays]);
```

**问题**：`loadModelDays` 有 `race` 令牌（`:758-770`），`loadOverview` **没有**。
连点「刷新」时两次 `loadOverview` 并发，慢的旧响应后到会覆盖新数据。
`loading` 只在 `loadOverview` 里管，按钮 `disabled={loading}` 恰好遮住了这条——
但 `refresh` 同时调 `loadModelDays`，而 `loadModelDays` **不设 `loading`**，
所以「刷新」按钮在 `loadModelDays` 在飞期间仍可点。

**影响小于 P1-3**：数据是当日累计统计，回退幅度有限、且下一轮轮询会自愈。

**修法建议**：`loadOverview` 也用 `race` 令牌，与 `loadModelDays` 同口径。

---

## 3. 已核实为「非缺陷」的项（避免误报）

这几处看着可疑，实读后确认是正确的，记下来免得下次重复怀疑：

| 疑点 | 结论 | 依据 |
|---|---|---|
| `bot_history_save` 断代删除会不会让刷新丢历史 | **不会**。该命令已退役，前端不再调用 | `src-tauri/src/lib.rs` 无注册；`src/` 仅测试 mock 残留 |
| 回灌时终态事件与落库顺序颠倒 | **正确**。终态事件先于落库，前端有重试与事件兜底 | `bot_chat.rs:274-277` + `useChatStream.ts:337` |
| `mergeInflight` 引用不稳定导致 effect 反复重跑 | **稳定**。`useCallback([])` 空依赖 | `ChatPanel.tsx:217-239` |
| 刷新后 `fileChanges` 丢失 | **已修**。后端 `bot_messages.file_changes` 列存在且读写齐全 | `db/bot_history.rs:77` + `bot_chat.rs:345` |
| `bot_sessions_load` 排序不稳定导致消息顺序乱 | **正确**。`ORDER BY ord, id` 稳定排序 | `db/bot_history.rs:64` |
| 后端回放缓冲收尾后立即被清，回灌补不齐 | **留 120s** | `stream_log.rs:47` `RETAIN_AFTER_FINISH_MS` |
| 刷新后提问卡（ask_user）丢失 | **可恢复**。回放缓冲内含 `bot-ask` 事件，实时与回放共用处理器 | `useChatStream.ts:454-483` |
| 工作区刷新覆盖乐观态 | **有写闸**。`writeInFlight` + `reloadQueued` | `WorkspacePage.tsx:80-112` |
| 任务页刷新与在途写撞车 | **已串行化**。并入全局 `mutating.chain` | `App.tsx:793-809` |
| 读失败被当成空库 | **未发生**。判别式返回 `{ok:true}\|{ok:false}` | `storage.ts:37-49` |

---

## 4. 未能确认 / 需实机复验的疑点

以下三点静态读码无法定论，建议实机验证后再定性：

1. **P1-2 的实际触发概率**：需要构造 DB 写锁竞争 > 1.8s 的场景
   （例如长事务写入时刷新）。静态看逻辑成立，但 1.8s 窗口在正常负载下可能很少命中。
2. **`bot-msg-upserted` 事件能否兜底 P1-2**：终态事件消费后，
   `useChatStream.ts:330-338` 的 `isResumed` 分支已 `clearResumed`，
   终稿行的 upsert 事件是否仍能被 `ChatPanel.tsx:325` 的监听器接住并补上——需实机确认。
3. **P1-1 的实际影响面**：`updated_at` 在每次写消息时推进
   （`db/bot_history.rs:185,234,306,358`），所以「当前会话不是最近更新的会话」
   在多会话并行使用时是否频繁发生，需实机观察。

---

## 5. 建议处置顺序

| 序 | 项 | 理由 |
|---|---|---|
| 1 | P1-3 ServicesPanel 刷新闸 | 改动最小（仓库已有 `useRaceSeq`），风险最低，收益直接 |
| 2 | P2-1 错误提示不清除 | 一行 `setError("")`，同类面板已有一致做法 |
| 3 | P1-2 终稿缺失 | 需先实机确认第 4 节疑点 2，再定修法 |
| 4 | P1-1 会话落点 | 涉及新持久化键，需拍板「记住上次会话」是否属预期行为 |
| 5 | P2-2 UsageStatsCard | 影响最小，可与其他刷新面板统一整改时一并做 |

按 AGENTS.md 约定：修复应「行为变更换测试」，每项补一条锚点测试；
且「不改 SPEC 功能条目则不必同步验收矩阵」——本次均属缺陷修复、不新增功能条目，
预计无需动 `docs/acceptance/` 矩阵与卡片。
