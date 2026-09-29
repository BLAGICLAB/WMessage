# Chunk 1: API 层安全 + 边界

## 上下文
- **仓库**：`/Users/renshi/Projects/wmessage/`
- **已有审计**（请勿重复）：`docs/AUDIT-AGENT-CORE-2026-08-18.md` / `docs/AUDIT-AGENT-FLOW-2026-08-18.md` / `docs/AUDIT-FIX-PLAN-2026-08-18.md` / `docs/AUDIT-REPORT-MANUAL-2026-08-18.md`
- **任务主线**：老板现在遇到不少 bug，找出架构层面的根因（不要小修小补）
- **严格只读**，不改代码

## 任务
审查 wmessage 外部 API（设置页开启外部机器人 / 设置页填 token 那个口子）三个文件，找**架构层 / 安全 / 边界**的潜在 bug 根因。

## 范围（必读）
- `src-tauri/src/api_handlers.rs` (37 KB)
- `src-tauri/src/api_server.rs` (5 KB)
- `src-tauri/src/api_auth.rs` (3 KB)

## 重点关注
1. **鉴权**：token 校验路径是否有泄漏点（路径绕过 / 错误响应格式泄漏 token 状态 / 401 vs 403 区分 / token 持久化存储）
2. **限流**：所有写路径（POST/PUT/DELETE）是否都限流？`/api/health` 免鉴权是否会被滥用？SSE 长连接是否单独限流？
3. **SSE**：`/api/events` 的生命周期（断连检测 / 内存泄漏 / 客户端失联时 emitter 是否还能写出 / 内部 hub 处理）
4. **入参校验**：路径参数 / 查询参数 / JSON body / 任务 id（UUID?）/ due 时间戳的边界
5. **错误路径**：on_error closure / 错误回客户端格式 / 错误日志是否含敏感信息（token / 任务内容）/ panic 容忍

## 跳过（已审过，**不要列**）
- 端口 4763 强绑 127.0.0.1 的设计（已知）
- API 流程高层正确性（AGENT-FLOW 流程 5 已覆盖）
- TauriStore 数据层封装（架构层 OK）
- CommandError 序列化（已全量迁移）

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
