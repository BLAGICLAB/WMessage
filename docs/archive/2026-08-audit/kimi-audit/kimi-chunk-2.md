# Chunk 2: DB 层 + 迁移

## 上下文
- **仓库**：`/Users/renshi/Projects/wmessage/`
- **已有审计**（请勿重复）：`docs/AUDIT-AGENT-CORE-2026-08-18.md` / `docs/AUDIT-AGENT-FLOW-2026-08-18.md` / `docs/AUDIT-FIX-PLAN-2026-08-18.md` / `docs/AUDIT-REPORT-MANUAL-2026-08-18.md` / `docs/kimi-audit/kimi-chunk-1-result.md`（API 8 条已审）
- **任务主线**：老板现在遇到不少 bug，找出架构层面的根因（不要小修小补）
- **严格只读**，不改代码

## 任务
审查 wmessage 持久层（SQLite + 迁移系统）两个文件，找**架构层 / 事务 / 迁移 / 并发 / 错误恢复**的潜在 bug 根因。

## 范围（必读）
- `src-tauri/src/db.rs` (39 KB)
- `src-tauri/src/migration.rs` (43 KB)

## 重点关注
1. **事务边界**：有没有"读 + 改 + 写"没包事务的路径？长事务？事务粒度（包太大 / 太小）？
2. **迁移幂等**：迁移失败后能否重跑？部分应用过的迁移怎么办？`migrations` 表是否能识别已应用？版本回滚？
3. **并发**：SQLite + 多线程（Tauri 用什么 runtime？），锁竞争 / busy timeout / WAL 模式开关 / 写写冲突
4. **错误恢复**：迁移中断 / 磁盘满 / 锁文件残留 / `wmessage.db` 损坏 / `migration.rs` 抛错后下次启动能否自愈
5. **连接管理**：Connection 池？句柄泄漏？长连接持有锁？`db_load` / `db_upsert` 在哪个线程执行？
6. **SQL 注入面**：字符串拼接的 raw SQL？`format!` 进 sqlx / rusqlite？
7. **模式漂移**：schema 升级后老数据是否兼容？字段 nullable / default 值策略？

## 跳过（已审过，**不要列**）
- API 层 P0-2 文件拆分（已落地）
- CommandError 序列化（已全量迁移）
- SQLite 整体行级增量存储改造（commit c7d4a3d 已落地）
- 已有 4 份审计 + chunk-1-result 里覆盖的项

## 输出格式（≤ 50 行）
1. **风险清单**（按 P0 > P1 > P2 排序）：
   - [P0] [file:line] [一句话描述] [建议]
2. **整体评估**（一句话）
3. **与已有审计差异**：重复 0 条，新发现 N 条

## 约束
- 严格只读，不改代码
- 输出 ≤ 50 行
- 不重复已有 P1/P2 项
- 不要给"小修小补"建议（拼写 / 注释缺失 / 命名），只给**架构层面**或**潜在 bug 根因**的发现
- 没发现就明说"无新发现"
