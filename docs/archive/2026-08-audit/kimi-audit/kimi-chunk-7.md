# Chunk 7: 资源生命周期 + 后台任务（v2 窄范围）

## 上下文
- **仓库**：`/Users/renshi/Projects/wmessage/`
- **已有审计**（请勿重复）：
  - 4 份原始审计 + chunk-1~5 结果（48 条新发现）
  - F-1~F-7 修复 plan
- **任务主线**：老板现在遇到不少 bug，找出架构层面的根因
- **严格只读**，不改代码

## 重要（避免再爆 token）
- **不要使用 sub-agent / Explore / Task 工具**——直接 Read 下面的文件
- **不要并行**——一个一个顺序读
- 文件太大就按 2000 行切片读（offset + limit）
- 不要做任何 grep / 全文搜索（除非本 prompt 内明确允许的定向 grep）

## 任务
审查 wmessage **资源生命周期 + 后台任务**——找**连接清理 / 文件句柄 / 未 await 的 tokio 任务 / SSE 客户端清理 / 关闭钩子遗漏**的潜在 bug 根因。

## 范围（必读，按顺序）
- `src-tauri/src/lib.rs` (403 行, setup / run / cleanup / 全局快捷键 / 托盘 / 插件挂载)
- 所有 `#[tauri::command]` 函数（chunk 6 已扫过错误签名；本 chunk **只看资源生命周期相关**：tokio::spawn / channel 创建 / 文件打开 / HTTP 连接 / SSE handler / Mutex 锁）
- `src-tauri/src/api_server.rs` (chunk 1 已审过 API 安全 / 限流 / SSE；本 chunk **只看连接清理钩子**)
- `src-tauri/src/api_handlers.rs` (同上，**只看 handler panic 后资源回收**)
- `src-tauri/src/bot_scheduler.rs` (定时任务调度器，**看后台任务生命周期**)
- `src-tauri/src/bot_py.rs` (Python 子进程，chunk 3 已审过沙箱；**只看 spawn 失败 / 子进程泄漏**)
- `src-tauri/src/migration.rs` (chunk 2 已审过事务；**只看启动期迁移任务泄漏**)

允许定向 grep（不全文搜）：
- `grep -n "tokio::spawn\|spawn_blocking\|mpsc::\|oneshot::\|Mutex::\|File::open\|fs::File"` 看每个 spawn 是否有对应的 abort / drop
- `grep -n "tauri::Builder\|on_window_event\|on_page_load\|setup(" 看生命周期钩子

## 重点关注
1. **tokio::spawn 后无 abort**：所有 `tokio::spawn(...)` 的句柄是否被持有？窗口关闭 / 应用退出时是否泄漏？长时间运行的工具调用（run_python / 大模型流式）退出时是否清理？
2. **mpsc / oneshot channel 生命周期**：oneshot 通道等待用户确认的路径（老板 2026-08-17 21:17 危险操作确认），超时后 sender 是否 drop？receiver 是否会永久挂死？
3. **文件句柄 / 临时目录**：Python 沙箱的临时目录（chunk 3 P2-9 已知未清扫），启动时是否有清扫机制？db wal_checkpoint 文件句柄是否正确 close？
4. **SSE 客户端清理**：api_server 启动的 mpsc channel 满时踢客户端的清理路径（chunk 1 P1-A2 已知）；踢客户端时是否清理了它对应的订阅 / 锁？
5. **关闭钩子遗漏**：lib.rs 的 `tauri::Builder` 是否有 `on_window_event` / `RunEvent::Exit`？退出时 SSE 服务 / Python 子进程 / HTTP client 是否被优雅关闭？
6. **Mutex 死锁 / 持锁 await**：profile.rs 已用 Mutex（chunk 4 D4 已知无锁竞态）；其它文件是否有持锁 await / 持锁调 sync fn？
7. **全局快捷键 / 托盘生命周期**：lib.rs 的 `GlobalShortcutExt` 注册 / 注销 / 重复注册保护？

## 跳过（已审过，**不要列**）
- chunk-1~5 全部内容
- 已有 4 份原始审计覆盖的 ChatPanel UI / 拖拽 / 折叠规则 / CommandError / bot_chat 主循环
- middleware / intent_router / tool_guard / audit（chunk 4 审过）
- chunk 1 P1-A1~A5（API 资源安全 5 项）已列
- chunk 3 P2-9（Python 临时目录未清扫）已列

## 输出格式（≤ 50 行）
1. **风险清单**（按 P0 > P1 > P2 排序）：
   - [P0] [file:line] [一句话描述] [建议]
2. **整体评估**（一句话）
3. **与已有审计差异**：重复 0 条，新发现 N 条
4. **统计**：`tokio::spawn` 总数 N / 未持句柄 N / Mutex 总数 N / 持锁 await N / 文件 open 未 close N

## 约束
- 严格只读，不改代码
- 输出 ≤ 50 行
- 不要用 Explore / Task / Agent 工具
- 不重复已有 P1/P2 项
- 没发现就明说"无新发现"