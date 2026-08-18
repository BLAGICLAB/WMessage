# Chunk 5 审计结果：前端状态管理 + IPC 错误（v2 窄范围）

- **日期**：2026-08-18
- **范围**：`src/storage.ts`、`src/App.tsx`、`src/lib/errorHandler.ts`、`src/components/TaskCardContent.tsx`、`src/components/ArchivePage.tsx`、`src/components/TrashPage.tsx`
- **方式**：严格只读，逐文件顺序精读
- **结论**：重复 0 条，新发现 11 条（P0×2 / P1×5 / P2×4）

## 风险清单（按 P0 > P1 > P2 排序）

### P0

1. **[storage.ts:13-21 + App.tsx:88-107] 读失败伪装成空库，触发种子写入**
   `db_load` 静默失败返回 `[]`，启动逻辑把"读失败"当"新库" → 写入 SEED（甚至跑 localStorage 迁移分支），用户看到空板 + 种子卡，且种子真实落库。
   **建议**：区分 error 与 empty，失败时告警且不进种子/迁移分支。

2. **[App.tsx:147-150] mutate 落盘未 await 即广播，存在回写竞态**
   `upsertTasks` fire-and-forget，未 await 就 `emit("tasks-changed")`；挂件收广播立刻 `db_load` 可能读到提交前快照并回写旧数据——与 178-184 行注释已修的竞态同族、方向相反。
   **建议**：await 落盘后再 emit。

### P1

3. **[App.tsx:188-197] 事件合并路径的规则改动不落盘，三端长期不一致**
   `tasks-updated` 合并路径里 `applyTodayRule/applyArchiveRule` 的改动只进 UI/tasksRef，不落盘 → 主窗、挂件、DB 三方长期不一致；且 194 行用 `JSON.stringify` 全量比较判 same，order 浮点漂移会触发无意义 setState。
   **建议**：规则改动 diff 后回写。

4. **[App.tsx:282-286] 导入后重读失败 → 清空 UI 并广播**
   `importTasks` 后 `loadTasksFromDb` 静默失败 → `setTasks([])` + 广播挂件，制造"数据全丢"假象。
   **建议**：重读失败保留原列表并提示。

5. **[TaskCardContent.tsx:92-95] Escape 取消后 blur 仍提交草稿**
   Escape 触发 `onCancelTitle` 后 input blur 仍会触发 `onCommitTitle(draft)`，取消的草稿可能被提交。
   **建议**：cancel 置标志位，blur 时检查。

6. **[storage.ts:24-41, 81-98] 全部写路径静默吞错，UI/DB 永久分叉**
   所有写操作 `silent:true` 且 `mutate` 不 await 不重试 → 落盘失败用户零感知。
   **建议**：失败计数 + 重试，连续失败弹非静默告警。

7. **[errorHandler.ts:123-138] 错误分级缺失 + 空消息静默**
   `recoverable` 只进 console 不参与 UI 分级；非结构化错误 `msg` 为空时连 alert 都没有（纯静默）。
   **建议**：`recoverable=false` 强制可见，空 msg 给兜底文案。

### P2

8. **[storage.ts:116-117 + App.tsx:144-146] 全量重排刷新所有 updatedAt，破坏导入合并语义**
   `assignInsertOrder` 间隙耗尽时全量整数重排 → 所有行 diff 变脏、`updatedAt` 全刷新 → 破坏导入合并"按 updatedAt 取新"语义。
   **建议**：重排不刷 updatedAt（或用独立排序字段）。

9. **[App.tsx:144-146] mutate 直接改写 state 对象字段**
   `t.updatedAt = now` 原地修改，同一引用在 tasksRef/state 共享，违反不可变约定，可能污染后续 `taskEq` 比较。

10. **[TrashPage.tsx:18 + ArchivePage.tsx:25] 回收站未排序 + 软删除不清调度字段**
    回收站列表未 `sortByOrder`（与归档页不一致）；软删除不清 `schedule/botAssigned`，回收站任务字段原样保留，定时器是否仍会触发需后端确认（前端侧无防护）。

11. **[TaskCardContent.tsx:12-14, 67-69] 双实现漂移风险 + useEffect 依赖缺失**
    与 TodoCard 双实现靠人工同步（注释自认漂移风险）；`useEffect` 依赖缺 `task.title`，编辑态期间外部改标题草稿不刷新。

## 无问题项

- **事件监听器生命周期**：App.tsx 6 处 `listen` 全部 `unlisten.then(f=>f())` 清理，无泄漏、无重复监听。

## 整体评估

状态层核心隐患是"**单向乐观 + 全链路静默**"：写不 await、错不上报、读失败伪装成空库，三条叠加使 UI/DB/挂件分叉成为必然而非偶发。

## 与已有审计差异

- 与 4 份原始审计 + chunk-1~4 结果比对：**重复 0 条，新发现 11 条**（P0×2 / P1×5 / P2×4）。
