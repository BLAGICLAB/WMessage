# Rust Bot 全面审计 · 批次 5：调度与后台任务（2026-08-28）

范围：`bot_scheduler.rs`（定时调度器）、`lib.rs`（setup / 退出清理）、`exec_steps.rs`（逐步执行），
及关联的 `bot.rs` 工具分发、`bot_slash.rs` 停止/确认基础设施、`bot_chat.rs` 执行核心、
`bot_py.rs` Python 子进程生命周期。

方法：explore 子代理全量扫描 → 主线程逐条源码复核（本报告只收录复核属实的发现）。

## 调用链

```
[启动] lib.rs setup → bot_scheduler::start_scheduler（tokio interval 30s）
        │ find_due_tasks（db_load 全量 → occurrence_after(sched_last) <= now，顺带清过期 at:）
        ▼
run_scheduled：SchedGuard 防重入 → bot_get_enabled 检查 → 跑前记 sched_last
        → bot_chat::execute_task_core(interactive=false)
            → ExecGuard → StopGuard::new_task_exec(false) → set_bot_assigned(true)
            → bot_model_loop::run_model_loop（stream_to_widget=false 全哑火）
                → execute_tool_with_stop → execute_tool_impl（interactive=false 贯穿）

[退出] RunEvent::ExitRequested → cleanup_on_exit
        = api_stop_for_exit（join accept+SSE writer）→ skill_terminate_all → kill_all_py_children → 销毁主窗口
```

## 发现汇总

| # | 级别 | 位置 | 结论 | 处置 |
|---|------|------|------|------|
| F1 | P1 | bot_scheduler.rs:329-334 | 调度循环对每张到点卡 `spawn` 后立即 `await`：单卡最坏 50 轮 ×（LLM 300s + Python 300s）可跑数小时，期间全部后续到点任务排队 | **已修**：spawn 不 await + 单任务 30min 整体超时 |
| F2 | P2 | bot_scheduler.rs:73/92/113/131/148 | 本地时刻解析一律 `.single()`：DST 切换日目标时刻不存在/歧义时返回 None，`occurrence_after` 整体 None，而基准 sched_last 不变 → daily/weekly 任务**永久静默失效**（国内时区无感） | **已修**：`resolve_local` 歧义取较早者、不存在顺延 ≤3h |
| F3 | P2 | bot_scheduler.rs:202, 240-246 | recurring 补跑无时效上限（关机一周后启动会补跑一周前的到点）；机器人开关关闭期间的到点任务不消费，重开瞬间全部补跑 | **已修**（2026-09-02 老板拍板 2h 窗口）：`classify_due` 超窗 occurrence 判 Missed 不补跑，sched_last 记为现在顺延到下一周期；新任务首跑与 at: 语义不变 |
| F4 | P3 | bot_scheduler.rs:258-281 | sched_last 跑前记（防 30s 重触发，方向正确），执行中进程被杀则本次 occurrence 无声消失 | 记录，取舍可接受 |
| F5 | P1 | bot.rs:805→1738-1756；bot.rs:1916→bot_py.rs:1727 | 后台定时执行仍会弹**原生文件对话框**：`bind_file` 分发处丢弃 interactive 无条件弹框；`extract_document` 无 path 时 `doc_extract` 无条件 `blocking_pick_file`。无人在场时弹模态框 + spawn_blocking 永久阻塞 → 该后台任务卡死（叠加 F1 拖死全调度） | **已修**：两处 interactive=false 直接返回「后台执行不能弹窗选文件」 |
| F6 | P1 | lib.rs:185-210, 468-475 | 退出清理对在途模型循环**零取消**：不置位任何 StopGuard、不等待。后果：生成文件写一半残留、sched_last 已消费执行无声消失、后台任务事实上无任何手段可叫停 | **已修**：`stop_all_executions()` 置位全部实例 + 2s drain 宽限 |
| F7 | P2 | bot_py.rs:556, 689-716 | `kill_all_py_children` 按注册表快照杀；退出时正阻塞在 `PY_RUN_GATE` 上的排队者拿到锁后仍 spawn 新 Python → 孤儿进程 | **已修**：`mark_exiting()` 全局标志，过闸门后复查拒绝 |
| F8 | P2 | exec_steps.rs:237 vs 275-337 | 逐步执行 `start()` 的 ExecGuard 随函数返回即释放，确认挂起期与续跑全程裸奔：同一张卡若是定时任务，调度器可并发执行同一卡（两套守卫互不知晓） | **已修**：ExecGuard 随 PendingExec 存活到 clear_for |
| F9 | ~~P2~~ | bot_chat.rs:879 | 子代理报「bot_assigned 崩溃残留无启动期清扫」——**复核为误报**：db.rs:419-426 `open_db` 启动期已 `UPDATE tasks SET bot_assigned = 0`（每进程一次，有测试 `reset_bot_assigned_real_db_clears_flag`） | 无需修 |
| F10 | P3 | bot_scheduler.rs:31 vs bot_chat.rs:793 | SchedGuard/ExecGuard 双套防重入注册表语义分裂（F8 的根因） | 记录，合并未做（架构项） |
| F11 | P2 | exec_steps.rs:258-271 | **复核中新发现**（子代理未报）：逐步执行 `start()` 的错误路径不复位 bot_assigned——`ok_or_else` 早退和 run_step 起步失败（从未 park，clear_for 是 no-op）都让卡片永远顶机器人头像 | **已修**：两条错误路径显式 `set_bot_assigned(false)` |

## 已核对无问题（防重复报）

- 休眠/关机恢复：interval 单调钟 + `occurrence_after(sched_last) <= now` 墙钟比较天然 catch-up，醒后 Burst 空转无害；at: 有 `at_expired` 放弃逻辑（防重启补跑过期一次性任务）。
- 流式推流：`stream_to_widget = interactive` 统一收口（bot_model_loop.rs），后台任务不向挂件发增量。
- 确认弹窗：`ask_confirm_inner` 的 `!interactive → deny` 兜底正确（bot_slash.rs:206），仅 F5 两个系统对话框漏网。
- 单卡 panic 被 spawn 隔离，主循环存活；三个 RAII 守卫均抗毒化（into_inner）。
- 退出清理顺序 API→Skill→Python 合理，join 均有超时，无死锁面。

## 修复清单（对应 commit）

1. **F1** bot_scheduler.rs：`start_scheduler` 循环 spawn 后不再 await（每张卡独立并发，SchedGuard/ExecGuard 仍防同卡重入）；`run_scheduled` 外包 `SCHED_TASK_TIMEOUT=30min` 超时，超时记 `sched_timeout` 审计并复位 bot_assigned。
2. **F2** bot_scheduler.rs：新增 `resolve_local()`，daily/weekly/monthly/at/at_expired 五处 `.single()` 全部换掉。
3. **F5** bot.rs：`bind_file` 分发处透传 interactive；`tool_bind_file` / `tool_extract_document`（无 path）在 interactive=false 时直接返回引导文案（改用 `link_file_to_task` / 提供 path），不弹框。
4. **F6** bot_slash.rs + lib.rs：`stop_all_executions()`（置位全部实例，含后台）+ `active_execution_count()`；`cleanup_on_exit` 第一步置位、API/Skill 清理后 drain 等待 ≤2s。
5. **F7** bot_py.rs：`static EXITING: AtomicBool` + `mark_exiting()`；`run_python` 过 PY_RUN_GATE 后复查，退出中直接拒绝（排队者不再 spawn 孤儿）。
6. **F8** exec_steps.rs：PendingExec 持有 ExecGuard，`start → run_step → park` / `resume → 续跑 → 再 park` 全链路透传，挂起期间调度器无法再并发执行同一卡。
7. **F11** exec_steps.rs：`start()` 的「无未勾子任务」早退与起步失败两条路径显式复位 bot_assigned。

## 回归测试

- bot_scheduler：`resolve_local` 正常时刻等价 single；调度循环不再串行 await 的源码锁。
- bot.rs：bind_file/extract_document 后台拒弹窗的源码锁。
- bot_slash：`stop_all_executions` 置位交互+后台两类实例。
- bot_py：退出标志在闸门后复查的源码锁。
- exec_steps/bot_chat：PendingExec 持有 ExecGuard 的源码锁 + ExecGuard RAII 语义单测。
