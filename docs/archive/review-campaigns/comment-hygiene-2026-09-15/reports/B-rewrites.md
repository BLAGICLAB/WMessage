# B 类语义改写建议 — Comment Hygiene 2026-09-15

> **状态**：仅建议，**未执行**。用户回来拍板。
> **原则**：决策内容**直接写在注释里**，不引用 SPEC.md / DEVLOG.md 等外部文档。
> 一文件一个 patch，方便回滚。

---

## B-1 — `src-tauri/src/app_state.rs:25, 30`

**原文**（两处加粗 + 嵌套标点）：
```rust
//! | `SESSION_ORIGINS`（**有意留档**·情况 1，定义留 `tool_guard.rs:45`） | `Mutex<HashMap<String, TaskExecOrigin>>` | 单次执行 | `unregister_exec_session`（`bot_chat.rs:1415`） | 不需要（详情见下「不收口」条目） |
//! ...
//! - `SESSION_ORIGINS`（`tool_guard.rs:45`，`HashMap<String, TaskExecOrigin>`）：
//!   **情况 1「session 元数据」——有意留档，不进依赖容器**（2026-09-13 判据定档）。
```

**问题**：表行 + 详情段的"留档"声明加了粗；`（2026-09-13 判据定档）` 这个日期括注其实只是元数据，判据内容（写一次 / 读一处 / 随 session 消亡）已经在下方「三条测量结论」里写全了。

**改写**（去加粗 + 删日期括注，因判据已在下方）：
```rust
//! | `SESSION_ORIGINS`（有意留档·情况 1，定义留 `tool_guard.rs:45`） | `Mutex<HashMap<String, TaskExecOrigin>>` | 单次执行 | `unregister_exec_session`（`bot_chat.rs:1415`） | 不需要（详情见下「不收口」条目） |
//! ...
//! - `SESSION_ORIGINS`（`tool_guard.rs:45`，`HashMap<String, TaskExecOrigin>`）：
//!   情况 1「session 元数据」——不进依赖容器。
//!   写一次（`bot_chat.rs:1373`/`1415`，夹在 `run_task_in_chat_with` 一头一尾），
//!   读一处（`bot/tools.rs:876 is_task_execution_flow`），随 session 消亡（key = session_id）；
//!   详见下方三条测量 + 两条附带事实。
```

---

## B-2 — `src-tauri/src/app_state.rs:62-75`

**原文**：
```rust
//! 测试期的跨进程共享（非全局状态，但同属「进程级假设」，一并留档·2026-09-13）：
//! - 数据目录解析（`paths.rs::probe_log_dir`）在测试构建下 = `target/debug/deps/`，
//!   而 nextest 是每测试一进程 → `bot.log` / `wmessage.db` / `*.flag` / 降级 key 文件跨进程共享；
//! - `profile.json` 已按 pid 隔离（`profile.rs::test_isolated_dir`）—— 唯一被实证打中的共享文件
//!   （曾 6 次复现 profile 用例随机挂，修后 5×nextest 全绿）；
//! - 其余**5 次 nextest 全绿、暂无实证**，按「等实锤再动」留档；下沉到 `probe_log_dir` 的
//!   三档方案（B1/B2/B3）与各自代价写在 `paths.rs::probe_log_dir` 的 doc 里。
//! - 追加：`cargo test --lib`（进程内并行）下 `exit_cleanup_tests` 的
//!   `api-enabled.flag` 断言失败过 1 次；**pid 隔离覆盖不到进程内并行**
//!   （同 pid 的测试共享目录，nextest 因每测试一进程才避开它）。实证细节与决策（C1：只留档、
//!   等定位到具体调用点再动）见 `paths.rs::probe_log_dir` 的 doc「追加实证 / 决策」两段。
```

**问题**：两处加粗（`**5 次 nextest 全绿、暂无实证**` / `**pid 隔离覆盖不到进程内并行**`）在 `cargo doc` 渲染里突兀；`一并留档·2026-09-13` 这个日期是元数据可去掉。注释本身的**实质内容**（pid 隔离、5 次 nextest 实证、exit_cleanup 那次 1 失败）都已经写在注释里，没引外部文档 —— 这部分保留。

**改写**（去加粗 + 删日期元数据）：
```rust
//! 测试期的跨进程共享（非全局状态，但同属「进程级假设」，留档观察）：
//! - 数据目录解析（`paths.rs::probe_log_dir`）在测试构建下 = `target/debug/deps/`，
//!   而 nextest 是每测试一进程 → `bot.log` / `wmessage.db` / `*.flag` / 降级 key 文件跨进程共享；
//! - `profile.json` 已按 pid 隔离（`profile.rs::test_isolated_dir`）—— 唯一被实证打中的共享文件
//!   （曾 6 次复现 profile 用例随机挂，修后 5×nextest 全绿）；
//! - 其余 5 次 nextest 全绿、暂无实证，按「等实锤再动」留档；下沉到 `probe_log_dir` 的
//!   三档方案（B1/B2/B3）与各自代价写在 `paths.rs::probe_log_dir` 的 doc 里。
//! - 追加：`cargo test --lib`（进程内并行）下 `exit_cleanup_tests` 的
//!   `api-enabled.flag` 断言失败过 1 次；pid 隔离覆盖不到进程内并行
//!   （同 pid 的测试共享目录，nextest 因每测试一进程才避开它）。实证细节与决策（C1 口径：
//!   实证不足时不动代码）见 `paths.rs::probe_log_dir` 的 doc「追加实证 / 决策」两段。
```

---

## B-3 — `src-tauri/src/paths.rs:65`

**原文**：
```rust
/// # 测试期的跨进程共享（留档，2026-09-13 决策：暂不下沉）
```

**问题**：括注里塞了"日期 + 决策"，但**整段理由**（pid 隔离 / nextest 每进程一进程 / 5 次实证 / 三档方案代价）已经写在 `probe_log_dir` 的 doc 块里。括注只是元数据，删了不影响阅读。

**改写**（拆成两段：标题 + 状态简述）：
```rust
/// # 测试期的跨进程共享
///
/// 留档观察，未下沉到依赖容器。pid 隔离在 profile.json 上有效，
/// 其余文件暂无实证。详见下方 `probe_log_dir` 整段 doc（三档方案 + 实证 + 代价）。
```

---

## B-4 — `src-tauri/src/paths.rs:104`

**原文**：
```rust
/// 决策（2026-09-13，口径 C1）：**只留档、不盲改** —— 上面那条进程内实证尚未定位到具体
/// 调用点，无靶点的修改等于猜；下次复现时先定位「哪个调用点删/写了哪个文件」，再按
/// 进程间走 B1/B2/B3、进程内走「按测试隔离（治本）」或「窄串行锁（打补丁）」选档。
```

**问题**：括号里塞日期 + "口径 C1"（项目内部简称）；加粗 + 破折号嵌套。

**改写**（C1 口径的**内容**直接写在注释里，不留简写）：
```rust
/// 决策：只留档、不盲改 —— 上面那条进程内实证尚未定位到具体
/// 调用点，无靶点的修改等于猜；下次复现时先定位「哪个调用点删/写了哪个文件」，再按
/// 进程间走 B1/B2/B3、进程内走「按测试隔离（治本）」或「窄串行锁（打补丁）」选档。
/// 
/// 决策口径（C1）：实证不足时不动代码 —— 等能稳定复现再说。无靶点的"修"等于猜。
```

---

## 风格说明

所有改写后**决策内容**（判据、决策、实证、方案与代价）**直接落在注释里**，不写"见 SPEC.md / 参见 DEVLOG / 标记 X 在 docs/"这类引用。

跨文件引用（如 `paths.rs::probe_log_dir`、`tool_guard.rs:45`）属**代码内交叉引用**，不视为外部文档，保持不动。

## 应用方式（用户决定）

- 单条：手动编辑或让 AI 按本文件执行
- 全部：`git apply` 一个合成的 `B-rewrites.patch`（未生成，留给用户）
- 回滚：与 A 类独立，单独 revert 即可