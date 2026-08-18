# Chunk 4: Profile + 中间件 + 审计

## 上下文
- **仓库**：`/Users/renshi/Projects/wmessage/`
- **已有审计**（请勿重复）：4 份原始审计 + chunk-1~3 结果（API 8 条 + DB 10 条 + Python 8 条，共 26 条新发现）
- **任务主线**：老板现在遇到不少 bug，找出架构层面的根因（不要小修小补）
- **严格只读**，不改代码

## 任务
审查 wmessage 的 workspace profile（迁移/导入/导出）、中间件管道、审计日志三个模块，找**架构边界 / 清理路径 / 跨模块一致性**的潜在 bug 根因。

## 范围（必读）
- `src-tauri/src/profile.rs` (22 KB)
- `src-tauri/src/middleware.rs` (10 KB)
- `src-tauri/src/audit.rs` (6 KB)

## 重点关注
1. **Profile**：workspace 导入/导出/迁移/清理路径——文件操作原子性？workspace 删除后关联任务/文件/历史是否清理？profile 切换时工作目录状态？并发 profile 写入？
2. **Profile 数据完整性**：profile JSON 字段 schema 校验？向前兼容（旧版本字段缺失/新增字段）？损坏文件的恢复策略？
3. **中间件管线**：pre_step / pre_execute 顺序是否确定？中间件 panic 影响？MiddlewareRegistry 短路语义？注册路径是否容易漏注册？
4. **中间件与业务耦合**：IntentRouter / AtomicGuard 是否绕过某些路径？暗门（绕过中间件直接调 execute_tool 的可能）？
5. **审计**：事件格式统一性（F-3 已修，但 [ts.毫秒] / LEVEL / key=v 格式在其他模块是否一致）？审计在 panic 路径是否写？buffer/flush 策略？日志轮转？debug 下审计是否被压低？
6. **跨模块一致性**：profile.rs 与 lib.rs 启动顺序关系？audit 路径是否覆盖 profile 期间？中间件与 Skill 状态机的耦合？
7. **错误路径**：profile 加载失败是否回退到默认？中间件失败是否 fallthrough？审计失败是否吞掉？

## 跳过（已审过，**不要列**）
- chunk-1 (API ErrorHub / 限流) / chunk-2 (DB 事务 / 迁移 atomic) / chunk-3 (Python 沙箱)
- 已有 4 份原始审计覆盖的项
- bot_skills / bot_chat / bot_model_loop 主流程
- Skill DSL 解析 / 状态机 / 熔断

## 输出格式（≤ 50 行）
1. **风险清单**（按 P0 > P1 > P2 排序）：
   - [P0] [file:line] [一句话描述] [建议]
2. **整体评估**（一句话）
3. **与已有审计差异**：重复 0 条，新发现 N 条

## 约束
- 严格只读，不改代码
- 输出 ≤ 50 行
- 不重复已有 P1/P2 项
- 不要给"小修小补"建议，只给**架构层面**或**潜在 bug 根因**的发现
- 没发现就明说"无新发现"
