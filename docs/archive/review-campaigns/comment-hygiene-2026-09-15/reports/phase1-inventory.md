# Phase 1 Inventory — Comment Hygiene 2026-09-15

## 扫描命令与命中数

### src-tauri/src/
| 命令 | 命中 |
|------|------|
| `rg -n "//.*20[0-9]{2}[-/年][0-9]{1,2}" src/ --glob '!*/tests/*'` | 28 |
| `rg -n "//.*阶段\s*[0-9]" src/` | 66 |
| `rg -n "//\s*(let\|fn\|if\|for\|while\|match\|return\|pub\|use\|impl)\s" src/` | 0 |
| `rg -n "//\s*(TODO\|FIXME\|XXX\|HACK)\s*$" src/` | 0 |
| `rg -n "//\s*(返回\|设置\|增加\|减少\|判断\|循环\|遍历\|创建\|删除)" src/` | 53 |
| `rg -n "SAFETY" src/` | 0 |
| `rg -n "FIXME\|HACK" src/` | 0 |
| `rg -n "unsafe" src/` | 9 |

### src/ (TS/TSX)
| 命令 | 命中 |
|------|------|
| `rg -n "//.*20[0-9]{2}[-/年][0-9]{1,2}" src/` | 1 |
| `rg -n "//\s*(TODO\|FIXME\|XXX\|HACK)\s*$" src/` | 0 |

`cargo doc --no-deps` 无 `missing documentation` 警告 — pub 项均有 doc。

---

## A 类（机械删除，可自动执行）

### A1 — 日期戳 + 动作描述（删整条 or 删日期+阶段保留事实）

| 文件:行 | 原注释 | 动作 | 替换为 |
|---------|--------|------|--------|
| src-tauri/src/bot/registry.rs:1 | `//! Tool registry（阶段 2，2026-09-13）。` | 删整条 | （删除） |
| src-tauri/src/app_state.rs:1 | `//! 全局可变状态单一入口（阶段 3.1，2026-09-13）。` | 删整条 | （删除） |
| src-tauri/src/db/mod.rs:1 | `//! WMessage 任务数据存储 facade（2026-09-14 Sprint D）` | 删整条 | （删除） |
| src-tauri/src/py/mod.rs:1 | `//! \`bot_py\` 切片层（2026-09-14 Sprint C）` | 删整条 | （删除） |
| src-tauri/src/bot.rs:3 | `//! 阶段 1 拆分（2026-09-13）：原 bot.rs 3444 行 → \`bot/{mod.rs, config.rs, dispatch.rs, tools.rs}\`。` | 删日期+阶段 | `//! 原 bot.rs 3444 行 → \`bot/{mod.rs, config.rs, dispatch.rs, tools.rs}\`。` |
| src-tauri/src/bot_py.rs:3 | `//! 2026-09-14 Sprint C：原 3694 行 bot_py.rs 按 SRP 切片到 \`py/\` 子模块（env / runtime / ...）` | 删日期+阶段 | `//! 原 bot_py.rs 按 SRP 切片到 \`py/\` 子模块（env / runtime / ...）。` |
| src-tauri/src/bot_slash.rs:154 | `// 阶段 3.3 撤锁（2026-09-13）：原 \`STOP_TEST_LOCK\` 已删除。` | 删日期+阶段 | `// 原 \`STOP_TEST_LOCK\` 已删除。` |
| src-tauri/src/bot_skills/state.rs:288 | `// 阶段 3.3 撤锁（2026-09-13）：原 \`SKILL_RUNS_TEST_LOCK\` 已删除。` | 删日期+阶段 | `// 原 \`SKILL_RUNS_TEST_LOCK\` 已删除。` |
| src-tauri/src/lib.rs:99 | `// 平台相关 copy_file_* helper 已迁到 \`platform/copy_file.rs\`（Sprint B2，2026-09-14）。` | 删日期+阶段 | `// 平台相关 copy_file_* helper 已迁到 \`platform/copy_file.rs\`。` |
| src-tauri/src/platform/copy_file.rs:17 | `/// 2026-09-14 Sprint B2：从 \`lib.rs:98-122\` 搬过来，逻辑一字不动。` | 删日期+阶段 | `/// 从 \`lib.rs:98-122\` 搬过来，逻辑一字不动。` |
| src-tauri/src/platform/copy_file.rs:50 | `/// 2026-09-14 Sprint B2：从 \`lib.rs:126-185\` 搬过来，逻辑一字不动。` | 删日期+阶段 | `/// 从 \`lib.rs:126-185\` 搬过来，逻辑一字不动。` |
| src-tauri/src/bot_skills/parse.rs:375 | `assert_eq!(m.max_steps, 20); // 默认 = clamp 上限（2026-09-13 由 8 抬到 20）` | 删日期 | `assert_eq!(m.max_steps, 20); // 默认 = clamp 上限（由 8 抬到 20）` |
| src-tauri/src/app_state.rs:69 | `//! - **2026-09-13 追加**：\`cargo test --lib\`（进程内并行）下 \`exit_cleanup_tests\` 的` | 删日期标记 | `//! - 追加：\`cargo test --lib\`（进程内并行）下 \`exit_cleanup_tests\` 的` |
| src-tauri/src/paths.rs:76 | `/// 2026-09-13 追加实证（**进程内**并行，与 nextest 不同层）：` | 删日期 | `/// 追加实证（**进程内**并行，与 nextest 不同层）：` |

### A2 — 阶段号 + 事实（删阶段号，保留事实）

**注意**：含决策/口径/why 的不删阶段号（归 D 类）。只删纯阶段号标记的事实句。

| 文件:行 | 原注释 | 动作 | 替换为 |
|---------|--------|------|--------|
| src-tauri/src/app_state.rs:13 | `//! 全局状态总表（阶段 3.1 盘点；行号为盘点时的真实位置，改动后会漂）：` | 删阶段号 | `//! 全局状态总表（盘点时的真实位置，改动后会漂）：` |
| src-tauri/src/app_state.rs:85 | `// 阶段 3.3：SKILL_RUNS 已迁入 \`AppState.skill_runs\`（访问器见文件尾）。` | 删阶段号 | `// SKILL_RUNS 已迁入 \`AppState.skill_runs\`（访问器见文件尾）。` |
| src-tauri/src/app_state.rs:93 | `// 阶段 3.3：STOP_REGISTRY 已迁入 \`AppState.stop_registry\`（访问器见文件尾）。` | 删阶段号 | `// STOP_REGISTRY 已迁入 \`AppState.stop_registry\`（访问器见文件尾）。` |
| src-tauri/src/app_state.rs:112 | `// 阶段 3.3：CONFIRMS 已迁入 \`AppState.confirm_requests\`（访问器 \`confirms(app)\` 见文件尾）。` | 删阶段号 | `// CONFIRMS 已迁入 \`AppState.confirm_requests\`（访问器 \`confirms(app)\` 见文件尾）。` |
| src-tauri/src/app_state.rs:116 | `// 阶段 3.3：CHAT_RUNNING / EXEC_RUNNING / SCHED_RUNNING 三张防重入表均已迁入 \`AppState\`` | 删阶段号 | `// CHAT_RUNNING / EXEC_RUNNING / SCHED_RUNNING 三张防重入表均已迁入 \`AppState\`` |
| src-tauri/src/app_state.rs:121 | `// 阶段 3.3：PENDING 已迁入 \`AppState.pending\`（访问器见文件尾）。` | 删阶段号 | `// PENDING 已迁入 \`AppState.pending\`（访问器见文件尾）。` |
| src-tauri/src/app_state.rs:127 | `/// 阶段 3.2 起逐张收口；到 3.3 已迁入 **8 张**：\`SKILL_RUNS\`、\`STOP_REGISTRY\`、产物登记、` | 改阶段号 | `/// 逐步收口；已迁入 **8 张**：\`SKILL_RUNS\`、\`STOP_REGISTRY\`、产物登记、` |
| src-tauri/src/app_state.rs:226 | `/// 阶段 3.3：表进 \`AppState\` 后 \`StopGuard::new\` / \`new_task_exec\` 必须带 \`app\`` | 删阶段号 | `/// 表进 \`AppState\` 后 \`StopGuard::new\` / \`new_task_exec\` 必须带 \`app\`` |
| src-tauri/src/bot_chat.rs:451 | `// 阶段 3.3：CHAT_RUNNING 已迁入 \`AppState\`，访问器返回 Arc 克隆——` | 删阶段号 | `// CHAT_RUNNING 已迁入 \`AppState\`，访问器返回 Arc 克隆——` |
| src-tauri/src/bot_chat.rs:1028 | `// 阶段 3.3：EXEC_RUNNING 已迁入 \`AppState\`，访问器返回 Arc 克隆——` | 删阶段号 | `// EXEC_RUNNING 已迁入 \`AppState\`，访问器返回 Arc 克隆——` |
| src-tauri/src/bot_artifacts.rs:29 | `// 阶段 3.2：产物登记表已收进 \`AppState\`（\`app.manage\` 注入），访问器走 \`try_state\`；` | 删阶段号 | `// 产物登记表已收进 \`AppState\`（\`app.manage\` 注入），访问器走 \`try_state\`；` |
| src-tauri/src/lib.rs:234 | `// 阶段 3.2：全局可变状态容器（单一入口）；产物登记表已迁入，其余表逐张迁移` | 删阶段号 | `// 全局可变状态容器（单一入口）；产物登记表已迁入，其余表逐张迁移` |
| src-tauri/src/bot_skills/state.rs:4 | `// 阶段 3.3：SKILL_RUNS 已迁入 \`AppState\`；这里 \`pub(crate) use\` 让本模块与` | 删阶段号 | `// SKILL_RUNS 已迁入 \`AppState\`；这里 \`pub(crate) use\` 让本模块与` |
| src-tauri/src/bot_skills/state.rs:74 | `// 阶段 3.3：SKILL_RUNS 已迁入 \`AppState.skill_runs\`（再导出见文件头 use）。` | 删阶段号 | `// SKILL_RUNS 已迁入 \`AppState.skill_runs\`（再导出见文件头 use）。` |
| src-tauri/src/bot_slash.rs:20 | `// 阶段 3.3：停止注册表已进 \`AppState\`（\`NEXT_STOP_ID\` 留在 app_state 作全局发号器）；` | 删阶段号 | `// 停止注册表已进 \`AppState\`（\`NEXT_STOP_ID\` 留在 app_state 作全局发号器）；` |
| src-tauri/src/bot_slash.rs:491 | `// 注入式 mock handle 统一在文件作用域定义（阶段 3.3）` | 删阶段号 | `// 注入式 mock handle 统一在文件作用域定义` |
| src-tauri/src/bot_slash.rs:476 | `// 阶段 3.3：表随 AppState，本用例用独立实例 → 不共享全局态，无需串行锁` | 删阶段号 | `// 表随 AppState，本用例用独立实例 → 不共享全局态，无需串行锁` |
| src-tauri/src/bot_slash.rs:498 | `// 阶段 3.3：注入独立实例，不再与别的持 StopGuard 用例互斥` | 删阶段号 | `// 注入独立实例，不再与别的持 StopGuard 用例互斥` |
| src-tauri/src/bot_model_loop.rs:21 | `// 阶段 2：从 crate::bot::registry 单源派生的 TOOLS / MUTATING_TOOLS。` | 删阶段号 | `// 从 crate::bot::registry 单源派生的 TOOLS / MUTATING_TOOLS。` |
| src-tauri/src/bot_model_loop.rs:520 | `// 僵尸终态清理已上移到薄壳 \`run_model_loop\`（阶段 3.3：核心只读技能状态，` | 删阶段号 | `// 僵尸终态清理已上移到薄壳 \`run_model_loop\`（核心只读技能状态，` |
| src-tauri/src/bot_skills/state.rs:317 | `/// 阶段 3.3：注入独立 \`AppState\` 的 mock handle（SKILL_RUNS 随 AppState 隔离）` | 删阶段号 | `/// 注入独立 \`AppState\` 的 mock handle（SKILL_RUNS 随 AppState 隔离）` |
| src-tauri/src/bot_scheduler.rs:53 | `// 阶段 3.1：SCHED_RUNNING 的定义与 sched_running() 访问器已集中到 \`crate::app_state\`` | 删阶段号 | `// SCHED_RUNNING 的定义与 sched_running() 访问器已集中到 \`crate::app_state\`` |
| src-tauri/src/exec_steps.rs:37 | `// 阶段 3.3：PENDING 已迁入 \`AppState.pending\`，\`pending_map(app)\` 取注入实例` | 删阶段号 | `// PENDING 已迁入 \`AppState.pending\`，\`pending_map(app)\` 取注入实例` |
| src-tauri/src/exec_steps.rs:48 | `/// 阶段 3.3：挂起表已迁入 \`AppState\`，故取注入实例（缺失时兜底实例）。` | 删阶段号 | `/// 挂起表已迁入 \`AppState\`，故取注入实例（缺失时兜底实例）。` |
| src-tauri/src/exec_steps.rs:412 | `/// 测试用 mock handle：注入独立 \`AppState\`（阶段 3.3：挂起表/执行守卫随 AppState 隔离）` | 删阶段号 | `/// 测试用 mock handle：注入独立 \`AppState\`（挂起表/执行守卫随 AppState 隔离）` |
| src-tauri/src/exec_steps.rs:522 | `/// 阶段 3.3：表随 \`AppState\` 走，守卫 Drop 清的是 acquire 时手里那份实例。` | 删阶段号 | `/// 表随 \`AppState\` 走，守卫 Drop 清的是 acquire 时手里那份实例。` |
| src-tauri/src/tool_guard.rs:164 | `// 阶段 3.3：SKILL_RUNS 随 AppState；本用例注入独立实例（空表 → 未运行任何 Skill）` | 删阶段号 | `// SKILL_RUNS 随 AppState；本用例注入独立实例（空表 → 未运行任何 Skill）` |
| src-tauri/src/bot_py.rs:111 | `// 阶段 3.3：本用例自带注入实例的 StopGuard，别的用例的全局广播不再置位它` | 删阶段号 | `// 本用例自带注入实例的 StopGuard，别的用例的全局广播不再置位它` |
| src-tauri/src/bot_artifacts.rs:143 | `/// 阶段 3.2 样板：给 mock app 注入**独立** \`AppState\`（每测试一个实例）。` | 删阶段号 | `/// 样板：给 mock app 注入**独立** \`AppState\`（每测试一个实例）。` |
| src-tauri/src/bot_artifacts.rs:240 | `/// 阶段 3.2 验收：\`manage\` 注入的实例彼此隔离；未注入的路径走兜底实例，` | 删阶段号 | `/// 验收：\`manage\` 注入的实例彼此隔离；未注入的路径走兜底实例，` |
| src-tauri/src/bot_slash.rs:462 | `/// 测试用 mock handle：注入独立 \`AppState\`（阶段 3.3：停止注册表/确认表随 AppState 隔离）` | 删阶段号 | `/// 测试用 mock handle：注入独立 \`AppState\`（停止注册表/确认表随 AppState 隔离）` |
| src-tauri/src/bot_slash.rs:517 | `/// 阶段 3.3：待确认表迁入 \`AppState\` 后，注入实例之间必须隔离` | 删阶段号 | `/// 待确认表迁入 \`AppState\` 后，注入实例之间必须隔离` |
| src-tauri/src/bot_chat.rs:502 | `/// 生产代码不调用。阶段 3.3：表随 \`AppState\` 走，故取注入实例（缺失时兜底实例，` | 删阶段号 | `/// 生产代码不调用。表随 \`AppState\` 走，故取注入实例（缺失时兜底实例，` |
| src-tauri/src/bot_chat.rs:1825 | `/// 阶段 3.3：\`ChatGuard\` / \`ExecGuard\` 迁入 \`AppState\` 后的守卫语义与实例隔离。` | 删阶段号 | `/// \`ChatGuard\` / \`ExecGuard\` 迁入 \`AppState\` 后的守卫语义与实例隔离。` |
| src-tauri/src/bot_scheduler.rs:60 | `/// 阶段 3.3：表已迁入 \`AppState\`。Drop 里拿不到 \`app\`，所以在 acquire 时把表句柄` | 删阶段号 | `/// 表已迁入 \`AppState\`。Drop 里拿不到 \`app\`，所以在 acquire 时把表句柄` |
| src-tauri/src/bot_scheduler.rs:743 | `/// 阶段 3.3：\`SchedGuard\`（迁入 \`AppState\` 后）的守卫语义与实例隔离。` | 删阶段号 | `/// \`SchedGuard\`（迁入 \`AppState\` 后）的守卫语义与实例隔离。` |
| src-tauri/src/bot/config/mod.rs:1 | `//! Bot 配置 + 审计底座（阶段 2 拆分后位置）。` | 删阶段号 | `//! Bot 配置 + 审计底座（拆分后位置）。` |
| src-tauri/src/bot/config/mod.rs:3-4 | `//! 阶段 1：原 bot.rs 行 55–1193 全部配置/keyring/审计/constants/check_len 代码` + `//! 搬入 \`bot/config.rs\`；阶段 2（本次 run5）：\`bot/config.rs\` 1923 行 → 7 子模块。` | 删阶段号 | `//! 原 bot.rs 行 55–1193 全部配置/keyring/审计/constants/check_len 代码 搬入 \`bot/config.rs\`；\`bot/config.rs\` 1923 行 → 7 子模块。` |
| src-tauri/src/bot/dispatch.rs:7 | `//! TOOLS_TABLE / tools_json() / mutating_tools() 都在 bot::registry（阶段 2 单源真相）。` | 删阶段号 | `//! TOOLS_TABLE / tools_json() / mutating_tools() 都在 bot::registry（单源真相）。` |
| src-tauri/src/bot/registry.rs:189 | `// list_tasks 在原 bot.rs:155 收 (app) 不收 args——阶段 1 拆出来后已修正。` | 删阶段号 | `// list_tasks 在原 bot.rs:155 收 (app) 不收 args——拆出来后已修正。` |
| src-tauri/src/bot/registry.rs:190 | `// link_file_to_task 是 async fn，必须 .await——阶段 1 拆出来后已修正。` | 删阶段号 | `// link_file_to_task 是 async fn，必须 .await——拆出来后已修正。` |
| src-tauri/src/lib.rs:657 | `// 阶段 3.3 撤锁：本测试做的两类「全局广播」——skill_terminate_all(None) 与` | 删阶段号 | `// 撤锁：本测试做的两类「全局广播」——skill_terminate_all(None) 与` |
| src-tauri/src/lib.rs:663 | `// 阶段 3.3：注入独立状态容器（停止注册表随 AppState 隔离）` | 删阶段号 | `// 注入独立状态容器（停止注册表随 AppState 隔离）` |
| src-tauri/src/lib.rs:700 | `// 阶段 3.3：停止表随 AppState，本用例注入独立实例（下面 cleanup_on_exit_with` | 删阶段号 | `// 停止表随 AppState，本用例注入独立实例（下面 cleanup_on_exit_with` |

**A2 保留清单**（含决策/口径/why，**不删**）：
- `bot/config/mod.rs` 阶段 1/2 上下文（仍指向具体行号，事实依赖）
- `bot_model_loop.rs:393`「核心**只读**技能状态（判断是否短路）」→ 决策
- `bot_model_loop.rs:450`「阶段 3.3（口径：只读走注入）」→ 决策口径
- `bot/registry.rs:680`「阶段 2 spec 1: TOOLS JSON 与基线一致」→ spec 引用
- `bot_slash.rs:39`「注册表句柄（阶段 3.3）」→ 设计语义
- `bot_slash.rs:245`「阶段 3.1：待确认请求表...已集中到」→ 现状描述
- `app_state.rs:17-25` 总表行内的迁移标记（带迁移后的位置）
- `app_state.rs:74`「阶段 3.2 已开工」+ `lib.rs` 总览注释

### A5 — 纯复述（A 类：删整条）

| 文件:行 | 原注释 | 动作 |
|---------|--------|------|
| src-tauri/src/migration/types.rs:61 | `/// 删除成功数` | 删整条 |
| src-tauri/src/bot/tools.rs:808 | `/// 删除单条子任务` | 删整条 |
| src-tauri/src/api_handlers/mod.rs:148 | `// 创建` | 删整条 |
| src-tauri/src/bot_skills/manage.rs:250 | `/// 返回技能名。` | 删整条 |
| src-tauri/src/profile.rs:608 | `// 删除头像` | 删整条 |
| src-tauri/src/memory/mod.rs:422 | `/// 返回工具结果文本。` | 删整条 |

**A5 其余 47 条**：含 WHY / 契约 / 边界条件 / 审计要求，**保留**（详见 D 类）。

---

## B 类（需拍板）

| 文件:行 | 原注释 | 问题 | 建议 |
|---------|--------|------|------|
| src-tauri/src/app_state.rs:31 | `//!   **情况 1「session 元数据」——有意留档，不进依赖容器**（2026-09-13 判据定档）。` | 用了 `**...**` 加粗 + 多个引号嵌套，可读性差 | 简化为单层标点，去加粗 |
| src-tauri/src/app_state.rs:62-69 | 进程级假设的「留档·2026-09-13」连续几行带「**2026-09-13 追加**」标签 | 时间戳已嵌入事实，混在 markdown 里 | 把"决策依据"独立到 commit message / SPEC.md，正文只留事实 |
| src-tauri/src/paths.rs:65 | `/// # 测试期的跨进程共享（留档，2026-09-13 决策：暂不下沉）` | 「留档」+「决策」混杂 | 拆成两段：第一段说现象，第二段说决策与口径 |
| src-tauri/src/paths.rs:104 | `/// 决策（2026-09-13，口径 C1）：**只留档、不盲改**` | 加粗+括号+冒号嵌套 | 简化为单句 |

---

## D 类（保留，列出以证明扫描到过）

### D — 不变量 / 决策 / 兼容性 / 性能 / SAFETY 类

| 文件:行 | 原注释（节选） | 保留理由 |
|---------|----------------|----------|
| src-tauri/src/paths.rs:499 | `// (a) ts 形如 \`[2026-09-13 12:34:56.789]\`：分钟级别即视为合规` | 时间戳格式契约 |
| src-tauri/src/bot_scheduler.rs:537 | `// 2026-08-16 是周日（weekday=7）` | 测试 fixture 用具体日期 |
| src-tauri/src/bot_scheduler.rs:576 | `// 2 月无 31 日 → 顺延到 3 月 31 日（after=2026-01-20）` | 测试 fixture 用具体日期 |
| src-tauri/src/audit.rs:5-6 | 老/新 log 格式示例带日期 | doc comment 中展示日志契约 |
| src-tauri/src/db/mod.rs:470,891,961 | 老 schema 描述带 2026-08-14/19 日期 | fixture 描述 |
| src-tauri/src/paths.rs:42 | `// （防 2026-02-31 被 Date 静默进位、防 25:99、防不完整输入产生坏数据——` | SAFETY：日期解析不变量 |
| src-tauri/src/bot_artifacts.rs:9 | `//! 设计权衡见 workspace 内部讨论 2026-09-11 D1a 拍板。` | 决策来源引用 |
| src-tauri/src/bot_skills/parse.rs:32 | `// 2026-09-13 从 8 抬到 20（= clamp 上限）：实测 gorden-ppt-skill 这类` | 调参原因 + 实证 |
| src-tauri/src/bot_fs.rs:27 | `/// 遍历时跳过的大而杂目录（另跳过所有 . 开头隐藏目录）` | 跳过策略契约 |
| src-tauri/src/bot/config/keyring.rs:173 | `opts.mode(0o600); // 创建时即 0600，无「先 0644 后 chmod」窗口` | SAFETY：权限 race window |
| src-tauri/src/bot/config/keyring.rs:443 | `let _ = old_entry.delete_credential(); // 删除失败不致命：v1 已有值，下次早退` | 失败处理契约 |
| src-tauri/src/paths.rs:306 | `// 创建即 0600（与 audit::open_log_append 一致）；写失败 eprintln 不阻塞` | SAFETY + 一致性 |
| src-tauri/src/bot/tools.rs:474 | `/// 删除任务到回收站：**弹窗确认后才执行**（危险操作护栏；60s 无响应默认拒绝）` | 危险操作护栏契约 |
| src-tauri/src/bot_web.rs:548 | `/// 删除指定标签的完整块（含嵌套同名标签）：去 script/style/nav/footer 等噪声。` | 操作语义 |
| src-tauri/src/memory/mod.rs:605 | `/// 删除放在同一事务（半完成不留中间态）。` | 原子性不变量 |
| src-tauri/src/error.rs:223 | `/// 返回枚举而非 \`&str\`：调用点编译期可查，拼写漂移不再可能。` | 设计理由 |
| src-tauri/src/api_handlers/body.rs:16 | `/// 返回，worker 永久占住并发名额（MAX_WORKERS=64 占满即全员 503）。` | 资源不变量 |
| src-tauri/src/api_server.rs:127 | `/// 返回 (id, msg)，writer 端据此与在线推送去重` | 契约 + 去重策略 |
| src-tauri/src/migration/recovery.rs:67 | `/// 返回（恢复条数, 错误条数）供调用者记日志。` | 契约 |
| src-tauri/src/migration/recovery.rs:189 | `/// 返回值：Ok(true)=已修复；Ok(false)=任务 file_path 已不指向 expected_src（用户重绑/已解绑）、` | 边界条件 |
| src-tauri/src/bot/config/schema.rs:86 | `/// 设置页保存前回填派生字段：bot_model_loop 只看 base_url/model/` | 派生字段约束 |
| src-tauri/src/bot/config/schema.rs:134 | `/// 返回 (from, to)；已是当前或更高版本 → None（不降级「装过更新版后回退」的配置）。` | 兼容性约束 |
| src-tauri/src/bot/config/types.rs:273 | `/// 返回给前端的配置视图：不含任何 key 本体，只有 has 标志` | 安全约束（不泄漏 key） |
| src-tauri/src/bot/tools.rs:43 | `/// 返回 Some((files, truncated))；无 files 字段返回 None（不改绑定）。` | 边界条件 |
| src-tauri/src/bot/tools.rs:92 | `/// 返回 (保留列表, 丢弃数)。` | 契约 |
| src-tauri/src/bot/tools.rs:525 | `/// 返回 (task, 定位说明)；找不到返回错误文案。` | 契约 |
| src-tauri/src/middleware.rs:31 | `/// 返回 RouteAction::PassThrough 也算「命中」并触发短路` | 行为契约 |
| src-tauri/src/bot_chat.rs:117 | `/// 返回 (保留的消息, 丢弃条数)。` | 契约 |
| src-tauri/src/bot_chat.rs:1108 | `/// 创建执行会话（标题 = 来源前缀 + 任务标题）并把任务块作为 user 消息落库，` | 标题策略 |
| src-tauri/src/paths.rs:193 | `/// 返回实际移动的文件名（调用方记审计）。幂等：` | 审计 + 幂等性 |
| src-tauri/src/bot_scheduler.rs:130 | `/// 返回在 after 之后最近的触发时间；一次性已过或格式无效返回 None。` | 边界条件 |
| src-tauri/src/bot_artifacts.rs:103 | `/// 返回成功绑定的文件数。` | 契约（短但有调用方依赖） |
| src-tauri/src/audit.rs:302 | `/// 返回 bool：成功 true / IO 失败 false。调用方一般不关心（IO 失败已 eprintln` | 契约 |
| src-tauri/src/bot_model_loop.rs:177 | `/// 返回 None = 非 data 行 / JSON 解析失败 / choices 为空。` | 边界条件 |
| src-tauri/src/bot_model_loop.rs:278 | `/// 返回 false = index 超上限，该 delta 被丢弃（调用方记审计）。` | 边界条件 + 审计 |
| src-tauri/src/bot_model_loop.rs:725 | `// 返回 Some = 流内错误载荷，调用方收尾报错` | 边界条件 |
| src-tauri/src/profile.rs:1059 | `// 返回默认让程序能启动` | 启动兜底 |
| src-tauri/src/memory/mod.rs:570 | `/// 返回落库后最旧的 REFLECTION_BATCH 条 summary（id, content），供 Reflection 触发判定。` | 用途说明 |
| src-tauri/src/memory/rank.rs:119 | `/// 返回 (四段原料, 命中条目 id 列表（hits+lessons，访问强化口径）)。` | 语义 |
| src-tauri/src/bot_slash.rs:450 | `/// 设置机器人聊天开关（写/删 flag，返回生效后的状态）` | 副作用 |
| src-tauri/src/py/env.rs:165 | `/// 返回 None = dotnet 或工具不可用（调用方回退 Python 脚本路径）。` | 回退策略 |
| src-tauri/src/memory/store.rs:218 | `/// 返回 (结果, 冲突提示列表[content 摘要])` | 契约 |
| src-tauri/src/memory/store.rs:380 | `/// 删除（remember_fact 空 value = 遗忘语义）：返回是否真删到` | 语义边界 |
| src-tauri/src/bot_anthropic.rs:200 | `/// 返回 (块数组, 跳过的非 data URL 图片数)。` | 契约 |
| src-tauri/src/bot_anthropic.rs:410 | `/// 返回 None = 非 data 行（含 \`event:\` 行）/ 空行 / ping / 无需消费的事件 /` | 边界条件 |
| src-tauri/src/bot_skills/manage.rs:183 | `/// 遍历搜索路径读每个 SKILL.md 的 frontmatter \`intents\`；` | 行为契约 |
| src-tauri/src/bot_skills/manage.rs:297 | `/// 删除范围：用户装的 skill 落在 \`skills_dir()\` (data dir)；debug build 下` | 位置契约 |
| src-tauri/src/bot_skills/files.rs:124 | `/// 删除任务卡绑定的本地文件/文件夹（回收站彻底删除时调用）。` | 调用时机 |
| src-tauri/src/bot_skills/runtime.rs:8 | `/// 返回 Ok 表示放行启动。` | 契约（短但语义关键） |
| src-tauri/src/bot_skills/runtime.rs:143 | `/// 返回 Err 表示该 Skill 必须立即终止（模型收到错误后停止后续步骤）。` | 终止语义 |
| src-tauri/src/bot_skills/runtime.rs:348 | `/// 返回回滚建议文本（失败且 rollback=auto 且有动作记录时非空），调用方拼进回复让模型执行逆操作。` | 调用方行为 |
| src-tauri/src/bot_skills/runtime.rs:97 | `/// 返回值契约（SKILL_DSL.md §4.3.2）：true = 段存在且全部回滚步骤无失败——` | spec 引用 |
| src-tauri/src/bot_skills/runtime.rs:176 | `/// 遍历 \`skill_search_paths(app)\`：数据目录找不到 → dev 模式 fallback target/debug/skills。` | 回退路径 |

### D-unsafe — 安全关键路径（**无 SAFETY 注释，见 C 类**）

| 文件:行 | unsafe 块 | 当前是否有 SAFETY | 备注 |
|---------|-----------|------------------|------|
| src-tauri/src/platform/copy_file.rs:40 | `unsafe { pb.setPropertyList_forType(...) }` | 无 | macOS NSPasteboard FFI |
| src-tauri/src/platform/copy_file.rs:44 | `unsafe { NSPasteboardTypeString }` | 无 | 同上 |
| src-tauri/src/platform/copy_file.rs:65 | unsafe 块 | 无 | 块级 unsafe |
| src-tauri/src/py/runtime.rs:67 | unsafe 块 | 无 | Python C API |
| src-tauri/src/py/runtime.rs:92 | unsafe 块 | 无 | 同上 |
| src-tauri/src/py/runtime.rs:98 | unsafe 块 | 无 | 同上 |
| src-tauri/src/py/runtime.rs:105 | unsafe 块 | 无 | 同上 |
| src-tauri/src/py/runtime.rs:148 | `unsafe { libc::getpgid(...) }` | 无 | POSIX 直接调用 |
| src-tauri/src/py/runtime.rs:431 | unsafe 块 | 无 | Python C API |

**9 处 unsafe 全无 SAFETY 说明** — 见 C 类建议。

---

## C 类（需拍板）

### C1 — `pub` 项缺 SAFETY 说明

9 处 `unsafe` 块全部位于 FFI/POSIX/Python C API 边界，目前无任何 SAFETY 说明。建议在每个 unsafe 块前添加 `// SAFETY: <理由>`：

| 文件:行 | unsafe 上下文 | 建议 SAFETY 内容 |
|---------|--------------|------------------|
| src-tauri/src/platform/copy_file.rs:40 | `NSPasteboard::setPropertyList_forType` | 「`&NSString` 由 `from_str` 持有，存于本栈帧，调用期间不释放；`&paths` 为 `CFArray` 借用的 typed array，调用方保证生命周期」 |
| src-tauri/src/platform/copy_file.rs:44 | `NSPasteboardTypeString` 常量 | 「常量定义，无别名问题」 |
| src-tauri/src/platform/copy_file.rs:65 | 块级 | 「块内调用见各行 SAFETY」 |
| src-tauri/src/py/runtime.rs:67 | Python C API | 见 pyO3 文档对应函数的不变量 |
| src-tauri/src/py/runtime.rs:92 | 同上 | 同上 |
| src-tauri/src/py/runtime.rs:98 | 同上 | 同上 |
| src-tauri/src/py/runtime.rs:105 | 同上 | 同上 |
| src-tauri/src/py/runtime.rs:148 | `libc::getpgid` | 「pid 由调用方传入，已通过 Pid::as_raw 保证非 0；返回值用于 == 比较，无 wrapper 安全调用」 |
| src-tauri/src/py/runtime.rs:431 | Python C API | 见 pyO3 文档 |

### C2 — 复杂函数缺 why 注释（暂列示，未细查）

无强制需要，**建议用户拍板**是否补充。

### C3 — 导出 TS 函数缺文档

`src/` 仅 1 处日期戳命中（`format.ts:42`，已含 WHY，保留）。TS 侧基本干净。

---

## NEEDS-DECISION

| 项 | 文件:行 | 不确定点 |
|----|---------|----------|
| ND-1 | src-tauri/src/app_state.rs:13 | `//! 全局状态总表（阶段 3.1 盘点；行号为盘点时的真实位置，改动后会漂）` — 删"阶段 3.1 盘点"还是保留？删了"盘点"上下文不明。建议保留作为「本表为快照」标记。 |
| ND-2 | src-tauri/src/app_state.rs:17-25 | 总表内每行的「**已迁入 ...（阶段 X.Y）**」标记 — 是 strip 还是整段重写为 `→ 位置：AppState.xxx`？语义变化较大，建议 B 类处理。 |
| ND-3 | src-tauri/src/lib.rs:657 | `// 阶段 3.3 撤锁：本测试做的两类「全局广播」——...` —「撤锁」是该测试存在的**理由**，删阶段号后语义还在但失去"为何要做这两类广播"上下文。**建议保留**。改为 D 类。 |
| ND-4 | src-tauri/src/bot/registry.rs:680 | `/// 阶段 2 spec 1: TOOLS JSON 与基线一致。` — 「阶段 2 spec 1」是测试 spec 引用，建议保留为 D 类。 |
| ND-5 | src-tauri/src/bot_model_loop.rs:520 | `// 僵尸终态清理已上移到薄壳 run_model_loop（阶段 3.3：核心只读技能状态，` — 阶段号后是**口径描述**，建议保留为 D 类。 |
| ND-6 | src-tauri/src/bot_skills/parse.rs:32 | `// 2026-09-13 从 8 抬到 20（= clamp 上限）：实测 gorden-ppt-skill 这类` — 删日期后失去"何时调的"信息，但调参事实保留。**建议删日期**。已归 A1。 |
| ND-7 | src-tauri/src/bot_skills/parse.rs:375 | `assert_eq!(m.max_steps, 20); // 默认 = clamp 上限（2026-09-13 由 8 抬到 20）` — 同上，已归 A1。 |

---

## 总结

- A 类（机械执行）：约 **60 处**（14 处 A1 + 41 处 A2 + 6 处 A5 纯复述）
- B 类（语义改写）：**4 处**（`app_state.rs:31, 62-69` + `paths.rs:65, 104`）
- C 类（补 SAFETY）：**9 处** unsafe 块无 SAFETY 注释
- D 类（保留）：**50+ 处**已扫描到的实质性注释
- NEEDS-DECISION：7 处