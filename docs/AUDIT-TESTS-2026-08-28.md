# Rust Bot 全面审计 · 批次 8：测试体系与防回归（2026-09-02）

范围：`src-tauri/tests/`（mock_llm / llm_integration / skill_e2e + fixtures）、
各模块 `#[cfg(test)]`（lib 472 个）、`tests-audit/audit_pre_step_pre_execute.py`、
前端 vitest 14 个测试文件。三个并行 explore 子代理（集成测试与 mock drift /
单元测试质量 / tests-audit 脚本与前端覆盖），全部 P1 发现主代理逐条源码复核。

## 结论

**无 P0。** 存量测试质量普遍偏高（行为断言 + 反断言 + 事故驱动回归，抽查
ChatPanel/storage 均为高强度断言），问题集中在三处：**门禁已红、并行测试
真实交错、调度器 e2e 零覆盖**。

## P1（均已修 + 验证）

### P1-1 tests-audit 门禁当前是红的（pre-push 被卡）
- 证据：`audit_pre_step_pre_execute.py:187` 写死旧签名字面量
  `"run_model_loop(app, msgs, 8, &stop)"`，但 run_model_loop 早已加
  max_rounds/plan_state 参数（现调用 bot_chat.rs:647）。实跑 pytest：
  22 passed / **1 FAILED**。该脚本挂在 `.githooks/pre-push → scripts/test-all.sh`
  （`set -euo pipefail`）——装了钩子的机器 pre-push 必被卡死。
- 修法：字面量改正则 `run_model_loop\(\s*app\s*,\s*msgs\s*,`，只锁「无条件进
  LLM」这一行为不变量，参数演进不再误报。顺带：cargo 基线阈值 80（2026-08-18
  基线 83 的遗物）收紧到 400（当前 472，留 ~15% 缓冲）；bot.log 检查项
  `if exists` 静默空转改显式 `pytest.skip`（假绿 → 报告可见）。
- 验证：pytest 23 passed / 2 skipped（设计性 skip），FAIL 归零。

### P1-2 SKILL_RUNS 全局态并行测试实锤交错
- 证据（三条交错路径，均在同一 lib 测试进程内并行）：
  - `state.rs:264`（clear_terminal_removes_only_terminal_states）调无差别删除的
    `clear_terminal_skill_runs()`，会删掉 `state.rs:295`（reopen_failed_run…）
    刚插入的 Failed run → 其 `assert!(reopen…)` 拿 false 挂；
  - `lib.rs:671`（cleanup_on_exit…）经 `skill_terminate_all(None)` 把 :264 的
    Paused run 置 Terminated，随后 :264 的 clear 把它删掉 → `is_some()` 断言挂；
  - 反向：:264 的 clear 删掉 lib.rs:671 已 Terminated 的 "p2-24-skill" →
    其 `== Some(Terminated)` 断言挂。
- 修法：新增 `SKILL_RUNS_TEST_LOCK`（state.rs，cfg(test)），三个测试全程持有。
  全库无 serial_test 依赖，不引新 crate。

### P1-3 STOP_REGISTRY 的 stop_all 全局广播打断并行测试
- 证据：`bot_py.rs:2153`（stop_reader_returns_partial_when_stopped）在自身
  force_stop **之前**断言「未停时完整读取」（:2163 `assert_eq!(buf, data)`）；
  并行的 `lib.rs:671`（cleanup → `stop_all_executions()`，lib.rs:217）或
  `bot_slash.rs:367`（stop_all 用例）会提前置位该 guard → 首读即 EOF →
  buf 空 → 随机挂。
- 修法：新增 `STOP_TEST_LOCK`（bot_slash.rs，cfg(test)）；stop_all 两个调用方
  测试 + StopReader 用例全程持有。lib.rs:671 同时持两把锁（固定顺序
  SKILL_RUNS → STOP）。bot_py.rs:2590 的 stop 中断用例经评估**天然容忍**
  （提前置位与自身 force_stop 产生同一「已停止」结果），不加锁。

## P2（处理情况）

- **P2-1 EXITING 退出标志测试后永不复位**（已修）：lib.rs:671 经
  cleanup_on_exit_with 置位 EXITING（lib.rs:226），进程内无复位——当前无测试
  走生产 `run_python()` 入口所以没炸，今后谁加谁挂。补
  `reset_exiting_for_test()`，cleanup 测试收尾调用。
- **P2-2 task_out.rs 契约零锁**（已修）：`TaskOut` 的 flatten + camelCase +
  status==column 三条前端契约此前 472 个测试无一能抓到（serde 属性被破坏只有
  前端运行时炸）。补 `task_out_wire_shape_locked` 序列化断言。
- **P2-3 format.ts 零覆盖**（已修）：scheduleToDatetime / isValidDateTimeLocal
  是注释里两次 NaN 历史事故的高发纯逻辑。补 `src/format.test.ts` 16 个用例
  （2026-02-31 进位拒绝 / weekly 三段解构分钟不丢 / monthly 月末顺延 /
  fallback 不扩散 NaN 等）。
- **P2-4 run_dsl_loop_sync 镜像 drift**（记录，补测试优先级 #1）：生产
  `run_skill_scheduler`（bot_skills/scheduler.rs:187+）零 e2e 覆盖；测试模块的
  同步镜像（:378-430）只有 parse→advance_dsl→失败判定骨架，**确认弹窗、停止
  令牌、审计、持久化、会话隔离、回滚窗口、Done 收尾全不进镜像**——而 8-27/28
  审计修复（P0-1 僵尸 Running、P0-5 回滚窗口）恰好密集在镜像照不到的地方。
  建议：以 fixtures/minimax-ppt 的 2-step DSL 为素材，给生产调度器补 mock_app +
  mock_llm 的真 e2e（工作量约半天，单独立项）。
- **P2-5 llm_integration 三层 drift 面**（记录）：mock 产出端手写字符串模板
  无共享 schema（`reasoning_content` 字段集成层零覆盖）；12 用例中 11 个走
  全量收字节而非生产的 drain_sse_lines 增量切行；run_model_loop 本体（重试/
  stop 检查点/think 拆分）零集成覆盖。排期补。
- **P2-6 弱断言清单**（记录，按性价比排序）：migration.rs:1525（只断言自建
  fixture 状态、不调 replay）；migration.rs:1711/1888/1928（测内联 str::replace
  而非生产 resolve_archive_dir）；migration.rs:2111 / db.rs:2131/2036/2239/2376
  （内联复刻生产逻辑，漂移不报警）；llm_integration.rs:182（401 用例实为 mock
  自测）；skill_e2e.rs:205（断言 fixture 自身内容，tautology）；
  scheduler.rs:753（skills 目录缺失时 eprintln 静默跳过早绿）。
- **P2-7 环境耦合**（记录）：lib.rs:671 真绑生产固定端口 4763（开发机跑着
  app 时 cargo test 挂）；api_handlers.rs 固定端口 48821-48825（范式正确的是
  bot_web.rs:1034 的 :0 动态端口）；lib.rs:671 / bot.rs:2431 读共享 data_dir 的
  bot.log 做 contains 断言（陈旧内容可假绿）。
- **P2-8 前端剩余盲区**（记录，按优先级）：KanbanBoard.spliceMove（拖拽排序
  纯函数，未导出不可测——需先导出）；MigrationPanel / ArchivePage 过滤逻辑 /
  theme.ts / MarkdownText 正则 / mutationOrigin 跨端同值锁。format.ts 已随
  P2-3 补齐。

## 测试数基线

| 套件 | 批次7 基线 | 批次8 | 说明 |
|---|---|---|---|
| cargo test --lib | 472 | **473** | +1（task_out 契约锁） |
| cargo 集成 | 20+8+8 | 20+8+8 | 不变 |
| vitest | 116 | **132** | +16（format.ts） |
| tests-audit pytest | 22过/1FAIL/1skip | **23过/0FAIL/2skip** | 门禁转绿 |

## 已确认无问题（防爆雷清单）

- 全项目零 `std::env::set_var`，无环境变量污染。
- mock_llm 消费端真同源（parse_sse_chunk / drain_sse_lines /
  accumulate_tool_call_delta 就是生产函数，bot_model_loop.rs:328/410/428）。
- db.rs / bot_model_loop.rs / migration.rs 核心系列为标准行为断言（抽查过）。
- 时间型测试除 migration.rs:1495（sleep 50ms 保证锁序，极端拥挤可误红——
  记录）与 lib.rs:642（80ms 零余量——记录）外均有 5-10 倍预算兜底。
- 源码锁测试 9 处中 2 处为解析结构化文件的稳态锁（capabilities JSON /
  版本单源），7 处文本窗口锁虽脆但锁的都是难单测的安全不变量，保留。
- skill_e2e.rs 8 个测试全部直调生产函数（非镜像），只是覆盖深度止于调度器之前。
