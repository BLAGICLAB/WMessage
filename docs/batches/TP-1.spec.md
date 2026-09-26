# Batch Spec: TP-1

## 目的

用户实测两次「写冲突」弹窗（主窗完成列取消✅；间隔 16.7s / 33.9s——远超 SYNC-1 修的亚秒竞态窗口），且冲突任务行事后从库中消失。用户三问：快照机制是什么/规则有没有问题/为什么挂件没事。根因定性：**整行快照回写架构对「后台写者 + 实例重启/重叠」天然脆弱**；✅ 状态切换是高频意图操作，不应依赖前端快照。

## 取证结论（2026-09-26 21:47-21:48 事故）

- T2 写者（21:47:45）**无 bot/迁移/调度/审计痕迹**；事发窗口恰逢 dev 实例被 tauri dev 监视器重启（21:48:29 新进程 89586，与本次 cargo 测试触发重编译吻合）+ 孤儿实例群（22:02 清理时证实 tauri dev/vite/app 三件套残留）。重叠实例/重启窗口内，跨实例事件不通 → 快照永久陈旧。
- **dev 数据目录 = `src-tauri/target/debug/`**（exe 可写走便携模式），与打包版（Application Support，9 月 13 日后无写）是两套库——先前取证方向曾因此偏航，已纠正。
- 冲突任务行现已不在 dev 库（全库仅剩 2 任务），回收站为空——用户重试期间自行清理的可能性最高，无数据可恢复，如实告知。

## 快照机制（回答用户）

主窗/挂件各自维护全量任务内存快照（tasksRef）；每次 UI 写 = **整行写回 + 基线（快照里的 updatedAt）比对**（db/tasks.rs RMW 守卫，防丢更新）。基线过期的任何写都被拒并弹窗。规则定时器（每分钟 today/archive 规则）、迁移轮询（10 分钟）、bot/调度器写库都会推进 DB 行版本——事件总线正常时快照实时收敛；**事件不可达（实例重叠/重启窗口）时快照永久陈旧**。

## 修法（服务端定向状态迁移，免除快照参与）

**新命令 `task_set_column(app, id, col)`**（db/tasks.rs）：`lock_db_write` 同锁内 `load_all` 找行（缺 → TaskNotFound）→ 应用语义（→done：completed_at=now、archived=false、bot_assigned 清；→todo/doing：completed_at/archived 清、bot_assigned 留）→ **基线 = 锁内现读 updated_at** → upsert（同锁内不可能冲突）→ 返回新行。广播：`tasks-changed`（挂件重读收敛）+ `emit_to("main","tasks-updated", source="main")`（主窗合并、不回写）。

配套：
- `db_upsert_for` RMW 冲突分支加 audit_log（bot.log 留痕 task id/基线/现行——未来事故可直接归因）。
- `lib/mutationOrigin.ts`：BACKEND_PERSISTED_SOURCES 加 "main"。
- 前端接线：App 新 handler setTaskColumn（invoke → applyRemoteRows 本地合并）+ KanbanBoard/SizableTodoCard/TodoCard 传 onSetColumn；TodoCard.toggleDone 两个分支改走它；WidgetApp.toggleDone 同命令 + 本地乐观更新（不再 emit tasks-updated 整行回写，靠 tasks-changed 收敛）。
- Rust 测试 3 条：done→todo 清 completed_at/archived 保 bot_assigned / todo→done 记 completed_at 清 bot_assigned / id 缺失 TaskNotFound。

## 红线

- 不动 upsert_tasks 守卫语义（其余整行写路径照旧，弹窗面收敛但不废除）
- 拖拽列变更（commitBoardOrder）本批不改（fresh 度较高，登记观察）
- TaskStatus wire 格式 / Task serde 零改动

## spec 起草后自查三条

1. expected_files 8（校正 ×1：补 TodoCard/types.ts——TodoCardViewProps 增 onSetColumn）
2. budget +260/-30（校正 ×2：fmt 后实测 +243/-12）
3. fix 字段：命令内闭环；TodoCard 向后兼容（onSetColumn 缺省回落 onUpdate）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "TP-1",
  "family": "task-targeted-transition",
  "expected_files": [
    "src-tauri/src/db/tasks.rs",
    "src-tauri/src/lib.rs",
    "src/App.tsx",
    "src/components/KanbanBoard.tsx",
    "src/components/TodoCard/TodoCard.tsx",
    "src/components/TodoCard/types.ts",
    "src/components/WidgetApp/WidgetApp.tsx",
    "src/lib/mutationOrigin.ts"
  ],
  "max_lines_added": 260,
  "max_lines_removed": 30,
  "findings": [
    {"id": "TP-1", "file": "src-tauri/src/db/tasks.rs", "line": 441, "fix": "新增 task_set_column：同锁内读现值→应用列语义（done↔todo/doing + completed_at/archived/bot_assigned）→基线=锁内现读→写→tasks-changed+tasks-updated(main) 广播；db_upsert_for 冲突分支 audit_log 留痕"}
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
BATCH_SPEC=docs/batches/TP-1.spec.md python3 scripts/batch-verify.py docs/batches/TP-1.spec.md
```
