# Chunk 6: 错误处理全栈（v2 窄范围）

## 上下文
- **仓库**：`/Users/renshi/Projects/wmessage/`
- **已有审计**（请勿重复）：
  - 4 份原始审计：AUDIT-AGENT-CORE / AUDIT-AGENT-FLOW / AUDIT-FIX-PLAN / AUDIT-REPORT-MANUAL（2026-08-18）
  - chunk-1~5 结果（API 8 + DB 10 + Python 8 + Profile/MW/Audit 11 + Frontend 11 = 48 条新发现）
  - F-1~F-7 修复 plan（已知事件命名空间 / A/C 路径 / mock_runtime 等已收敛）
- **任务主线**：老板现在遇到不少 bug，找出架构层面的根因
- **严格只读**，不改代码

## 重要（避免再爆 token）
- **不要使用 sub-agent / Explore / Task 工具**——直接 Read 下面的文件
- **不要并行**——一个一个顺序读
- 文件太大就按 2000 行切片读（offset + limit）
- 不要做任何 grep / 全文搜索

## 任务
审查 wmessage **错误处理全栈**——找**错误分级 / 用户可见性 / 监控埋点 / 跨层一致性**的潜在 bug 根因。

## 范围（必读，按顺序）
- `src-tauri/src/error.rs` (347 行, CommandError 全栈定义)
- `src/lib/errorHandler.ts` (chunk 5 已审过 IPC 分级，**跳过内部，** 只对照后端枚举对齐度)
- 前端**除** `errorHandler.ts` 之外的错误处理点（**grep** `\.catch\|throw \|Promise\.reject\|isError\|toast(.*err` —— 这里允许定向 grep，不许全文搜索）
- 所有 `#[tauri::command]` 函数（60 个）的错误返回签名（grep `Result<.*Error\|Result<.*String>` 抓所有签名，每个读 5-10 行验证是否走 CommandError）

## 重点关注
1. **错误分级一致性**：error.rs 的 `recoverable` 字段在前端/后端是否真覆盖到每条路径？哪些 code 后端标注 recoverable=false 但前端按可恢复处理？反过来？
2. **未迁移死代码**：`Result<T, String>` 是否还有残留？CommandError 全栈迁移计划 vs 实际完成度？
3. **监控埋点**：哪些错误进 audit / 不进 audit？致命错误有无 alerting？前端 console.error vs 用户可见 toast 的边界？
4. **错误传播链**：后端 Tauri command → 前端 invoke → try/catch → 用户提示，整条链上是否有信息丢失（被吞 / 被 stringify / 被 generic 包装）？
5. **跨平台错误差异**：Windows / macOS / Linux 错误码差异？CommandError 是否覆盖了 macOS keyring / Windows ACL / Linux 文件权限特殊错？

## 跳过（已审过，**不要列**）
- chunk-1~5 全部内容（含 chunk-5 已审的 errorHandler.ts:123-138）
- 已有 4 份原始审计覆盖的 ChatPanel UI / CommandError 全栈迁移 / 拖拽 / 折叠规则
- 后端 bot_chat / bot_model_loop / bot_scheduler / bot_slash / bot_skills
- middleware / intent_router / tool_guard / audit（chunk 4 审过）
- API 层 / DB 层 / Python 沙箱（chunk 1/2/3 审过）

## 输出格式（≤ 50 行）
1. **风险清单**（按 P0 > P1 > P2 排序）：
   - [P0] [file:line] [一句话描述] [建议]
2. **整体评估**（一句话）
3. **与已有审计差异**：重复 0 条，新发现 N 条
4. **覆盖度统计**：`Result<T, String>` 残留 N 处 / `Result<T, CommandError>` 已用 N 处 / `recoverable=false` 路径 N 条

## 约束
- 严格只读，不改代码
- 输出 ≤ 50 行
- 不要用 Explore / Task / Agent 工具
- 不重复已有 P1/P2 项
- 没发现就明说"无新发现"