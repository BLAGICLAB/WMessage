# Batch Spec: TP-2

## 目的

用户指令：核对主窗任务卡所有状态变更路径的同类写冲突隐患并修复，使「待办、今日、完成、存档、删除」顺畅可变。TP-1 已修 ✅ 切换；本批把**其余全部单任务写路径**切到同一免快照架构。

## 路径审计结论（2026-09-26 全量盘点）

| 路径 | 现机制 | 冲突面 | TP-2 处置 |
|---|---|---|---|
| ✅ 完成/取消 | task_set_column（TP-1） | 无 | 已修 |
| 软删→回收站 deleteTask | 快照整行 | **有** | → task_patch |
| 回收站恢复 TrashPage | updateTask 快照整行 | **有** | 随 updateTask 修复 |
| 归档恢复 ArchivePage+卡片恢复钮 | updateTask 快照整行 | **有** | 随 updateTask 修复 |
| 标题/备注/标签/子任务/截止/折叠/日程/文件绑定 updateTask | 快照整行 | **有** | → task_patch |
| 彻底删除 hardDeleteTask | db_delete（按 id，无基线） | 无 | 保持 |
| 拖拽跨列+列内排序 commitBoardOrder | 快照整行（多行+order） | 有（低频） | **不改**：多行+order 语义一次性写；正常单实例下快照新鲜；冲突已有 rmw_conflict 审计留痕——登记观察 |
| 新建 addTask | 新行无基线 | 无 | 保持 |
| 导入/绑定文件 | 后端命令 | 无 | 保持 |

## 修法

**新命令 `task_patch(app, id, patch)`**（db/tasks.rs，与 task_set_column 同管道）：
`lock_db_write` 同锁内读现行（缺 → TaskNotFound）→ `apply_task_patch`（白名单键逐个应用，
未知键/受保护键/空标题 → InvalidArgument 响亮失败）→ 基线 = 锁内现读 → 写 → 返回新行 →
广播同 TP-1（tasks-changed + tasks-updated source=main）。
patch 键 = Task serde camelCase 字段白名单（column/completedAt/archived/deletedAt/
collapsed/title/note/tags/subtasks/due/files/filePath/fileIsDir/order/schedule/
schedLast/botAssigned）；id/updatedAt/expectedUpdatedAt 受保护。
**null = 清空**（前端 wrapper 把 undefined 归一为 null——JSON 序列化丢 undefined 键，
不归一则「清空」语义静默丢失）。

前端（App.tsx）：
- `patchTask(taskId, patch)` wrapper：undefined→null 归一 → invoke → applyRemoteRows。
- `updateTask` 改为计算 due-today 规则补丁后走 patchTask（ArchivePage/TrashPage/TodoCard
  各恢复/编辑入口经 onUpdate 自动受益，组件零改动）。
- `deleteTask` 改 patchTask({deletedAt, schedule: null, schedLast: null})（软删清调度语义不变）。
- 硬删/拖拽/新建不动。

## 测试

- Rust：apply_task_patch 4 断言组（列+清空字段 / 未知键拒 / 受保护键拒 / 空标题拒）+
  task_patch 锁内段写后重载一致。
- 前端：App.test「软删除」用例改断言 task_patch 载荷（schedule/schedLast null 语义钉死）。

## 红线

- upsert_tasks 守卫、Task serde、task_set_column（TP-1）零改动
- 拖拽/新建/导入路径不动
- patch 不允许触达 id/updatedAt/expectedUpdatedAt（updated_at 由服务端统一打戳）

## spec 起草后自查三条

1. expected_files 4（tasks.rs / lib.rs / App.tsx / App.test.tsx）
2. budget +300/-30（实测 +290/-25，校正 ×1）
3. fix 字段：命令内闭环；组件侧零接口变更（updateTask 签名不变）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "TP-2",
  "family": "task-targeted-transition",
  "expected_files": [
    "src-tauri/src/db/tasks.rs",
    "src-tauri/src/lib.rs",
    "src/App.tsx",
    "src/App.test.tsx"
  ],
  "max_lines_added": 300,
  "max_lines_removed": 30,
  "findings": [
    {"id": "TP-2", "file": "src-tauri/src/db/tasks.rs", "line": 505, "fix": "新增 task_patch 命令 + apply_task_patch 白名单应用器：同锁内读现值→逐键应用（null=清空；未知/受保护键/空标题 InvalidArgument）→基线=锁内现读→写→广播；前端 updateTask/deleteTask 改走 task_patch（undefined→null 归一），软删/恢复/归档/编辑全线免快照竞态"}
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
BATCH_SPEC=docs/batches/TP-2.spec.md python3 scripts/batch-verify.py docs/batches/TP-2.spec.md
```
