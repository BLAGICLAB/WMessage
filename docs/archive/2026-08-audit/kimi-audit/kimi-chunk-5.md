# Chunk 5: 前端状态管理 + IPC 错误（v2 窄范围）

## 上下文
- **仓库**：`/Users/renshi/Projects/wmessage/`
- **已有审计**（请勿重复）：4 份原始审计 + chunk-1~4 结果（API 8 + DB 10 + Python 8 + Profile/Middleware/Audit 11，共 37 条新发现）
- **任务主线**：老板现在遇到不少 bug，找出架构层面的根因
- **严格只读**，不改代码

## 重要（避免再爆 token）
- **不要使用 sub-agent / Explore / Task 工具**——直接 Read 下面的文件
- **不要并行**——一个一个顺序读
- 文件太大就按 2000 行切片读（offset + limit）
- 不要做任何 grep / 全文搜索

## 任务
审查 wmessage 前端**状态层 + IPC 错误处理**——找**架构层 / 状态竞态 / 错误可见性**的潜在 bug 根因。

## 范围（必读，按顺序）
- `src/storage.ts` (126 行, 状态层)
- `src/App.tsx` (445 行, 顶层)
- `src/lib/errorHandler.ts` (4.8 KB, IPC 错误处理)
- `src/components/TaskCardContent.tsx` (346 行, 任务卡渲染)
- `src/components/ArchivePage.tsx` (109 行, 归档页)
- `src/components/TrashPage.tsx` (58 行, 回收站)

## 重点关注
1. **状态竞态**：storage.ts 与 IPC 推送的事件谁权威？并发 fetch vs 事件流会否覆盖？多窗口（主窗 + 挂件）状态同步？
2. **IPC 错误用户可见性**：所有 `invoke` 调用是否 catch？哪些静默吞错？`handleCommandError` 的覆盖面？错误分级（致命 vs 可恢复）？
3. **事件监听器生命周期**：useEffect 清理 / 窗口关闭 / 切页面监听是否解绑？事件 ID 重复？
4. **归档/删除回滚**：ArchivePage / TrashPage 操作后任务状态机是否一致？恢复后是否回到原字段？archivedAt / deletedAt 字段处理？
5. **任务卡渲染一致性**：TaskCardContent 这套渲染逻辑是否其他页面（Archive / Trash / Main）复用？字段显示差异？

## 跳过（已审过，**不要列**）
- chunk-1~4 全部内容
- 已有 4 份原始审计覆盖的 ChatPanel 关键 UI / CommandError / 拖拽 / 折叠规则
- 后端 bot_chat / bot_model_loop / Skill 调度
- ChatPanel.tsx / WidgetApp.tsx / TodoCard.tsx / SettingsPage.tsx（这些大文件留到下一块单独跑）

## 输出格式（≤ 50 行）
1. **风险清单**（按 P0 > P1 > P2 排序）：
   - [P0] [file:line] [一句话描述] [建议]
2. **整体评估**（一句话）
3. **与已有审计差异**：重复 0 条，新发现 N 条

## 约束
- 严格只读，不改代码
- 输出 ≤ 50 行
- 不要用 Explore / Task / Agent 工具
- 不重复已有 P1/P2 项
- 没发现就明说"无新发现"
