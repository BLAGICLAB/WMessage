# Batch Spec: TP-3

## 目的

用户实测：主窗拖拽任务卡（今日→完成/待办）仍弹写冲突——TP-2 登记观察的 commitBoardOrder
（多行快照整行回写）是最后一个冲突面；同时挂件侧还有 6 个残余快照操作（折叠/标题/
日程/子任务/文件/排序），其 emit 载荷带挂件基线（挂件快照最旧可差一个轮询周期 5s+），
主窗代persist时同样可撞守卫。本批全部免快照化。

## 修法

### 后端：task_reorder（排序专用，ord-only）
- `ReorderItem { id, order }` 批量：同锁内读现值 → **只改 order**（内容/updated_at 零
  改动）→ 基线 = 锁内现读（等值通过）→ 缺失行跳过 → 返回更新行 → 广播（同 TP-1/2）。
- 设计要点：RMW 守卫防的是「旧内容压新内容」，排序写不携带内容——带整行快照反而有
  丢失风险；ord-only 写对任何并发写者天然安全，无需 bump updated_at。

### 前端
- App `commitBoardOrder`：本地乐观（列语义 + assignInsertOrder 照旧计算）**不再
  upsertTasks**；列变更行走 task_set_column（TP-1 命令，语义 1:1：进 done 记完成
  时间/清 bot 标记、出 done 清完成时间），order 变更行走 task_reorder（通常 1 行，
  邻界压缩时全列）。
- WidgetApp：toggleCollapsed / commitTitle / setSchedule / toggleSubtask / removeFile
  → task_patch（TP-2 命令）+ 本地乐观（新增 applyLocal，不再 emit tasks-updated 整行
  回写）；handleDragEnd → task_reorder。addTask（新行无基线）保持。

## 测试

- Rust：task_reorder 2 断言组（ord-only：内容与 updated_at 不动 / 缺失行跳过多条批量）。
- 前端：现有 318 全绿（无拖拽驱动用例）；tsc。

## 红线

- upsert_tasks 守卫 / task_set_column / task_patch（TP-1/2）零改动
- assignInsertOrder 算法不动（前端纯计算）；DragOverlay/handle 签名不变
- reorder 不 bump updated_at（排序非内容变更）

## spec 起草后自查三条

1. expected_files 4（tasks.rs / lib.rs / App.tsx / WidgetApp.tsx）
2. budget +250/-35（实测 +236/-30，校正 ×1）
3. fix 字段：命令内闭环；commitBoardOrder/handleDragEnd 签名不变

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "TP-3",
  "family": "task-targeted-transition",
  "expected_files": [
    "src-tauri/src/db/tasks.rs",
    "src-tauri/src/lib.rs",
    "src/App.tsx",
    "src/components/WidgetApp/WidgetApp.tsx"
  ],
  "max_lines_added": 250,
  "max_lines_removed": 35,
  "findings": [
    {"id": "TP-3", "file": "src-tauri/src/db/tasks.rs", "line": 640, "fix": "新增 task_reorder 命令：批量 ord-only 写（同锁内读现值、基线等值通过、内容与 updated_at 零改动、缺失行跳过、广播）；App commitBoardOrder 改 task_set_column+task_reorder 双命令；WidgetApp 折叠/标题/日程/子任务/文件/排序 6 路径改 task_patch/task_reorder + applyLocal"}
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
BATCH_SPEC=docs/batches/TP-3.spec.md python3 scripts/batch-verify.py docs/batches/TP-3.spec.md
```
