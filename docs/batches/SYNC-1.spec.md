# Batch Spec: SYNC-1

## 目的

用户实测 bug（2026-09-26 21:13，附截图）：主窗口完成列取消✅弹「写冲突：…整行写回被拒（基线 updated_at=T1，现行 T2）」；挂件同操作正常。根因 = 主窗口**两条互不感知的写链竞态**，RMW 基线跨链失效。

## 根因机制（溯源）

- 主窗口所有 UI 写（toggleDone/拖拽/编辑/**每分钟规则定时器**）走模块级 `mutating.chain`（App.tsx:25，mutate:246 读 tasksRef → 落盘 → 更新 tasksRef:272）。
- `tasks-updated` 事件合并处理器（App.tsx:311-359 handle）却挂在 useEffect **局部 `queue`**（:362）——与 mutating.chain 互不感知。
- handle 内多个 await 让出点（:329 挂件行回写 / :341 规则回写 / :357 广播），`tasksRef.current = next`（:354）在 await 之后才执行。
- **竞态**：handle 在 await 窗口期内，用户点 UI（mutating.chain 出队）→ 读到**旧 tasksRef** → handle 落盘（行 updatedAt=T2）→ UI 写带着旧基线 T1 撞上 T2 → db/tasks.rs:220 RMW 守卫拒写 → storage.upsertTasks alert 弹窗（storage.ts:33 契约）。
- 用户时序吻合：交叉测试两窗口（验收清单 §1 第 6 条）时，挂件 toggle 触发的事件合并还在 await 窗口内，主窗口取消✅已出队读旧快照。
- 挂件为何正常：挂件单链（applyAndSync 同步更新自身 state），且写路径只有主窗一个持久化方。

## 修法（最小而准）

**两条链合一**：tasks-updated 回调并入 `mutating.chain`，删除局部 queue。全窗口「读快照 → 落盘 → 更新 tasksRef」对 tasks 数据严格串行，RMW 基线不再跨链失效。单事件失败 catch 不阻断后续（既有语义不变，落盘冲突靠下次事件/db_load 自愈）。

回归钉：App.test 新增用例——merge 的落盘 await 挂起期间，UI mutate 的 db_upsert **不得**出队（旧代码两条链会立即写，用例红）。

## 不在本批

- workspace-updated 的 listener 也是独立队列模式（工作区数据无 RMW 守卫，无弹窗面）——登记观察，不随批改。
- api.rs TauriStore 直写路径：现役 API server 走 api_handlers/commands.rs:107 构造的 TauriStore（带 emit 回调 :109-120，`source: "api"` 通知主窗）✓ 已覆盖，不动。
- merge handle 单事件失败后 tasksRef 停留旧值（靠后续事件自愈）——既有设计，不改。

## spec 起草后自查三条

1. expected_files 2（src/App.tsx + src/App.test.tsx）
2. budget +90/-15（代码 ≈+12/-4，测试 ≈+60）
3. fix 字段：单文件闭环，无签名变更

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "SYNC-1",
  "family": "main-window-task-sync",
  "expected_files": [
    "src/App.tsx",
    "src/App.test.tsx"
  ],
  "max_lines_added": 90,
  "max_lines_removed": 15,
  "findings": [
    {"id": "SYNC-1", "file": "src/App.tsx", "line": 362, "fix": "tasks-updated 合并队列从 useEffect 局部 queue 并入模块级 mutating.chain：全窗口任务写（UI mutate/规则定时器/事件合并）单链串行，消除 handle await 窗口内 UI 写读旧 tasksRef 的 RMW 基线竞态"}
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
BATCH_SPEC=docs/batches/SYNC-1.spec.md python3 scripts/batch-verify.py docs/batches/SYNC-1.spec.md
```
