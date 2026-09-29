# 数据层与一致性审计报告（2026-08-28，批次 2）

> 范围：db.rs / migration.rs / mutation.rs / task_out.rs / 变更广播与前后端一致性。
> 方法：3 路并行子代理 + P0/P1 人工复核。基线：批次 1 修复后全绿。

## ✅ 本轮已修

### B2-P0 data.json 迁移：计数误判 + 残留复活（db.rs）
- 触发判定从「json 条数 > 库条数」改为「集合差」（json 含库缺失 id 才算有迁移内容）——
  原计数比较在「json ≤ 库但含缺失 id」时漏迁；
- 评估成功后无论是否补内容，data.json 一律改名退役为 `data.json.migrated`（可人工找回）——
  原先「不触发就保留」，而删除任务是硬删（`delete_tasks` 真 DELETE），库计数跌穿 json 计数时
  残留 json 会把已删除任务全部复活。回归测试含「退役后硬删不得复活」断言。

### B2-P1 copy_legacy_db 半拷贝无恢复（db.rs）
主库直拷目标路径，中断留半截文件且下轮永久跳过。改为先拷 `*.db.copying` 临时文件再 rename，失败清临时文件。

### B2-P1 bot_scheduler 三处写库零广播（bot_scheduler.rs）
清理过期 schedule / 记 sched_last / 执行结果写备注——全部裸 db_upsert 无广播，
主窗口（无轮询）长期显示旧 ⏰ 徽标/旧备注。三处补 broadcast_after_mutation（成功才广播）。

### B2-P1 锁外写者纳入 DB_WRITE_LOCK
`tool_remember_fact`（bot.rs）与 `persist_outcome_quiet`（scheduler.rs）原先锁外直写，
长事务（如 bot_history_save）期间撞 SQLITE_BUSY 静默丢失。已纳入全局写锁。

### B2-P1 WidgetApp 空列表守卫（WidgetApp.tsx）
`if (!list.length) return` 无条件跳过——主窗口删光任务后挂件永久显示旧数据，
点勾选还会把已删任务复活回库。改为只在「当前本就为空」时跳过（首载防空闪）。

### B2-P2 挂件 emit 失败静默吞（WidgetApp.tsx）
`applyAndSync` 的 `emit("tasks-updated").catch(() => {})`——挂件不落盘，上报失败 =
改动永不持久化（≤5s 被轮询静默回滚，用户视角「编辑神秘消失」）。失败改走 handleCommandError 弹错。

### B2-P2 主窗口 tasks-updated 监听串行化（App.tsx）
async 监听器多 await 让出点，连续事件从同一旧 tasksRef 出发互相覆盖合并结果。
改 Promise 链排队逐个处理；单事件失败 catch 不阻断队列（原先未处理 rejection 终止监听）。

### B2-P2 bot_history 写放大上限（db.rs）
单会话历史上限 2000 条（全量覆盖写保留最近 2000）——长会话每轮 O(n) 重写全表的
写放大/WAL 膨胀有界化。

### B2-P2 行内 JSON 损坏留痕（db.rs）
load_all 的 subtasks/files、load_workspace 的 links 解析失败原先静默读成空
（下次整行 upsert 把 NULL 写回 = 静默丢数据）。现在 eprintln 留痕可见。
彻底防护（字段级合并写入）属架构改造，见下方排期项。

## 已核对正确

- 事务完整性：db_upsert / 导入合并 / 迁移 / 历史保存全部事务包裹，中途失败回滚，有回归测试
- WAL + busy_timeout 2s + spawn_blocking 读写的连接管理；legacy 库 WAL checkpoint BUSY 判定
- 文件迁移引擎 journal 三段式（pending→move→commit）+ replay 对账 + file_path_untouched 防覆盖
- schema 纯增量 + 老列保留 → SQLite 时代内回退安全；MigrationGuard RAII 防重入
- 回收站/归档过滤各读路径一致（load_all 全量返回、过滤在调用方的统一约定）
- MutationOrigin 守卫、主窗口「先落盘后更新 UI」、TaskOut serde 与前端类型逐字段一致
- 数据目录全路径统一走 probe_dir 定版缓存

## 排期项（未修，记录在案）

- **整行覆盖的 lost-update**（最重要）：upsert 是 19 列全量替换 + `updated_at >=` 守卫，
  但所有写者都先刷新时间戳 → 守卫对「读旧快照→整行写回」失效。bot 工具 / API update /
  挂件快照上报都可能回滚另一端刚写的字段。修复方向：字段级合并写入或写前重读比对，
  属架构改造，单独立项。
- open_db 补丁序列每次调用全跑 + 并发 ALTER 首启瞬态（duplicate column，重试自愈）——
  可做进程内按路径 Once 缓存，收益是性能。
- migration replay 在启动 60s 后才跑（注释与行为不符，窗口内用户看到旧绑定）。
- 回退到 JSON 时代的旧版本 = 静默空数据（data.json 已迁走）——发版说明需明示不支持回退。
- API `get_task` 按 id 不过滤回收站（TaskOut 含 deletedAt 字段，客户端可自判，低风险）。
- 文件迁移阶段二逐条 upsert（每文件一个事务）可攒批优化（千级文件分钟级，可接受）。
