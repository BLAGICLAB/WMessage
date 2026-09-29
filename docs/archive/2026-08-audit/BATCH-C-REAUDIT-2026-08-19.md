# Batch C 重审 + 修复报告（2026-08-19）

重审范围：`src-tauri/src/bot_py.rs`（1038 行全读）、`bot.rs`（1416 行，sandbox 调用方）、`lib.rs`（408 行，spawn/清理钩子）、`audit.rs`（172 行，审计写路径）、`bot_model_loop.rs`（Python 回退路径）。
原审计对照：`docs/kimi-audit/kimi-chunk-3-result.md`（8 条：C1-C4 + P2-9..12）。

## 阶段 1：审计结果

### 已知 C1-C4 + P2 状态复核

| 条目 | 状态 | 证据（当前代码） |
|---|---|---|
| C1 超时只护主进程，孙进程继承管道 → `rx.iter()` 永久阻塞 | **仍在** | bot_py.rs:276 `rx.iter()` 无宽限/无二次 kill_tree |
| C2 无内存/CPU 限额 + `timeout_secs` 无上限钳制 | **仍在** | bot_py.rs:209 `Duration::from_secs(timeout_secs.unwrap_or(60))`，无 rlimit/Job Object |
| C3 超时/失败路径不记审计 | **仍在** | bot_py.rs:831 `run_python(...)?` 在 "py_exec done" 审计之前；doc_* 同 |
| C4 `py_exec_sync` 同步阻塞跑在 async 上下文 | **仍在** | bot_py.rs:817 + bot.rs:1327 `tool_run_python` 同步调用，未见 spawn_blocking |
| P2-9 spawn 失败泄漏临时目录 | **仍在** | bot_py.rs:234 `cmd.spawn().map_err(...)?` 前已 create_dir_all + 写 run.py |
| P2-10 `detect_python()` 每次重新探测、探测无超时 | **仍在** | bot_py.rs:205 每次调用；:77/:105/:120 `.output()` 无超时 |
| P2-11 `truncate_for_log` 不剥换行 → 日志注入 | **仍在** | bot_py.rs:1053-1062 原样保留 `\n` |
| P2-12 `run_python` 并发无闸门 | **仍在** | 无信号量/队列 |

8 条全部仍在，均未修复——按 plan 留给 Batch C 单独批次，本次不动。

### 新发现列表（按严重度排序）

| ID | 位置 | 角度 | 严重度 | 一句话问题 |
|---|---|---|---|---|
| NEW-C-1 | bot_py.rs:851/883/911/940/959/997 | async 上下文 | **P1（已修）** | 6 个 `doc_*` async tauri 命令直接调阻塞 `run_python`，最长 120s 压占 async runtime worker（C4 同族但调用点不同，原审计只列了 py_exec_sync 工具链路径） |
| NEW-C-2 | bot_py.rs:295-306 | 审计完整性 | **P1（已修）** | `py_audit` 不拿 `BOT_LOG_LOCK`——`audit.rs:62-65` 明确 rotate+open+write 必须在同一把锁内（2026-08-18 并发错行事故后统一上锁），py_audit 漏网 |
| NEW-C-3 | bot_py.rs:261 | 子进程生命周期 | P2 | `child.try_wait()` 错误分支 `?` 直接返回：子进程成孤儿继续跑、临时目录泄漏、无审计 |
| NEW-C-4 | bot_model_loop.rs / bot_slash.rs StopGuard | 超时/取消 | P2 | `/stop`（StopGuard）只在工具轮次间检查，不覆盖在途 Python 子进程——停不掉，只能等 60-120s 超时 |
| NEW-C-5 | bot_py.rs:245/253 | stdout 读取 | P2 | 输出超 64KB 后 reader 线程 `take()` 到顶即关管道，Unix 下子进程被 SIGPIPE 杀死，`exit_code=None` 且无任何说明 |
| NEW-C-6 | audit.rs:82-84 + bot_py.rs:849 | 审计注入 | P2 | `write_event` 的 kv 值不剥换行（结构化 sink，与 P2-11 同族不同点）；`doc_extract` 审计行的 `path` 也未剥换行 |
| NEW-C-7 | bot_py.rs:1043 | 跨平台/路径 | P2(trivial) | `gen_out_path` 的 `trim_end_matches(".{ext}")` 大小写敏感：用户传 "周报.DOCX" → 产出 "周报.DOCX.docx" |

### 新发现详情

#### NEW-C-1（bot_py.rs:851/883/911/940/959/997）— 已修 `1c9b954`
- 症状：`doc_extract` / `doc_make_word` / `doc_make_word_revisions` / `doc_make_excel` / `doc_make_pdf` / `doc_make_ppt` 均为 `#[tauri::command] async fn`，函数体内直接同步调用阻塞 `run_python`（固定 120s 超时）。Tauri async 命令跑在 async runtime 上，单次调用独占一个 worker 最长 120s，并发几个文档操作即可拖垮 runtime；工具链（bot.rs:1110 起）也 await 这些命令，同样被堵。
- 根因：30bee8b 起 doc_* 就是 async，但 `run_python` 调用一直没配 `spawn_blocking`（`py_env_check` 有、doc_* 没有）；原审计 C4 只核了 `py_exec_sync` 工具链路径，漏了这 6 个调用点。
- 修复策略：新增 `spawn_blocking_map` 助手（spawn_blocking + JoinError→错误字符串映射），6 个调用点统一改走它。不改 `run_python` 本身、不碰 `py_exec_sync`（C4 范围）、不动任何 pub fn 签名。
- 修复 commit：`1c9b954`
- 新增测试：`spawn_blocking_map_ok_passthrough` / `spawn_blocking_map_err_passthrough` / `spawn_blocking_map_panic_mapped_to_err`（happy + 业务错误 + panic 三路径，不真跑 Python）

#### NEW-C-2（bot_py.rs:295-306）— 已修 `885cd01`
- 症状：`py_audit` 写 bot.log 不拿锁；`audit::write_event`（audit.rs:70）与 `bot::audit_log`（bot.rs:213）都拿 `BOT_LOG_LOCK`，三者写同一文件——并发时 rotate 与写入存在竞态、append 交错错行。
- 根因：BOT_LOG_LOCK 是 2026-08-18 错行事故后补的，py_audit 这条写路径当时漏改。
- 修复策略：`py_audit` 保持签名不变，拆出路径参数版 `py_audit_to`（锁内 rotate+append），便于无 AppHandle 单测。
- 修复 commit：`885cd01`
- 新增测试：`py_audit_to_writes_wellformed_line`（格式）+ `py_audit_to_concurrent_no_interleaved_lines`（8 线程 × 100 行并发，断言 800 行整、无撕裂）

## 阶段 2：实际修复汇总

| ID | 严重度 | commit hash | 改了哪几行 | verify 结果 |
|---|---|---|---|---|
| NEW-C-1 | P1 | `1c9b954` | bot_py.rs +44/-12：新增 `spawn_blocking_map`（~15 行）+ 6 个 doc_* 调用点各改为闭包形式 + 3 个测试 | cargo build 干净；3 个新测试 pass |
| NEW-C-2 | P1 | `885cd01` | bot_py.rs +56/-2：`py_audit` 拆出 `py_audit_to` 并上 `BOT_LOG_LOCK` + 2 个测试 | cargo build 干净；2 个新测试 pass |

## 阶段 3：未修的（留到下个 batch）

| ID | 严重度 | 一句话问题 | 建议修复策略 | 估计工期 |
|---|---|---|---|---|
| C1 | P1 | 孙进程继承管道 → `rx.iter()` 永久挂死 | 主进程退出后收输出加宽限（如 2s），超时 kill_tree 整组 | 0.5d（Batch C 范围） |
| C2 | P1 | 无内存/CPU 限额 + 超时无上限钳制 | 超时钳 300s 上限 + Unix `pre_exec` RLIMIT_AS/RLIMIT_CPU、Windows Job Object | 1d（Batch C 范围） |
| C3 | P1 | 超时/失败路径不记审计 | `run_python` 内部统一记审计（含 Err 分支） | 0.5d（Batch C 范围） |
| C4 | P1 | `py_exec_sync` 同步阻塞在 async 上下文 | bot.rs:1327 调用点 spawn_blocking（与 NEW-C-1 同模式） | 0.25d（Batch C 范围） |
| NEW-C-3 | P2 | try_wait 错误分支泄漏子进程 + 临时目录 | 该分支补 kill_tree + remove_dir_all | 0.1d |
| NEW-C-4 | P2 | /stop 不覆盖在途 Python 子进程 | run_python 轮询循环里检查 StopGuard（需传入），触发即 kill_tree | 0.5d |
| NEW-C-5 | P2 | 输出超限 → SIGPIPE 杀子进程，exit_code=None 无说明 | reader 到顶后继续 drain 丢弃（不关管道），或结果里标注 "输出超限被截断" | 0.25d |
| NEW-C-6 | P2 | write_event kv 值 / doc_extract path 不剥换行 | 与 P2-11 合并修：审计写路径统一 sanitize（\n→\\n） | 0.25d |
| NEW-C-7 | P2(trivial) | `trim_end_matches(".{ext}")` 大小写敏感 | 比较前转小写 | 0.05d |
| P2-9..12 | P2 | （原审计 4 条，仍在） | 按 Batch C plan | —（Batch C 范围） |

## 总体 verify

- `cargo build`：干净，仅 2 个 pre-existing warning（dead_code，非本次引入）
- `cargo test --lib`：**237 passed / 2 failed** —— 2 个 fail 为已知 pre-existing（`scan_all_skills_in_debug_dir_parse_correctly` + `smoke_all_real_skills_run_dsl_loop_with_mock_executor`，C 路径 DSL 迁移遗留，不在范围）
- 净新增测试：+5（`spawn_blocking_map_*` ×3、`py_audit_to_*` ×2，全部 pass）
- 未跑真 Python 子进程；未跑 `tauri build`；未 push
