# Phase 1 — 静态扫描报告(只找不改)

> 基线: HEAD `00d494f`,working tree clean。`cargo test --lib` 663 ✓ / `npm test` 209 ✓ / `cargo clippy -D warnings` 144 errors(预期 — 项目明文保留 lint 留档)。

## 扫描方法

- 静态 grep 全覆盖:`unwrap/expect/panic!/unreachable!/todo!/unimplemented!`、`as usize`、`len()-N`、`[..]` 字节切片、`PathBuf::/canonicalize/starts_with`、`fs::remove/write/rename`、`Mutex/RwLock/Arc`、`.lock().unwrap()`、`async + std::fs`、`Utc::now/Local::now`、`reqwest::Client`、`SQL format!`、`Regex::new`、`impl Drop`、`unsafe`、`#![deny]`、`items after test module`、`too_many_arguments/lines`、`MutexGuard across await`、`Default::default() field assignment`
- Tauri command 注册清单 62 个,逐个看签名参数(路径/字符串/数字/数组)
- 前端:`dangerouslySetInnerHTML/innerHTML`、`eval/new Function`、`localStorage/sessionStorage`、`JSON.parse`、`.then/.catch`
- 通读关键文件: `bot_web.rs` HTML 解析、`bot_chat.rs` 截断、`bot_fs.rs` 路径白名单、`api_auth.rs` token 处理、`intent_router.rs` 路由、`bot/dispatch.rs` 调度、`db/tasks.rs` SQL、`api_handlers/commands.rs` API 锁、`migration/run.rs` 后台轮询

## 扫描统计

| 项 | 命中数 | 处理 |
|---|---|---|
| `.unwrap()` (非 tests/) | 890 | 全在 `#[cfg(test)] mod tests` 内或 sync 代码 — 不计 bug |
| `.expect()` (非 tests/) | ~10 | 全部有语义化 msg,99% 在测试 fixture 或 migration 测试 |
| `panic!/unreachable!/todo!/unimplemented!` (非 tests/) | 0 业务路径 | 全部在 `#[cfg(test)]` 或带 impossible 分支 |
| `as usize` | ~30 | 多为 ONNX 维度 / 行号 → 已知安全 |
| 字节切 `[a..]` (Unicode 风险) | ~25 | 全有边界守卫(verified) |
| `Mutex.lock().unwrap()` (生产) | 5 | 全部 sync 路径,无跨 await(verified) |
| `std::fs` in async fn | 0 | grep 命中为非 async 函数,假阳性 |
| `SQL format!` | 1 | `db/tasks.rs:200` DELETE IN — placeholder only,SAFE |
| Regex runtime compile | 1 | `bot_fs.rs:432` 有 regex::escape 兜底;`intent_router.rs:132` 有 .ok() 跳过 |
| `unsafe {}` | 9 | 全在 FFI (py/runtime.rs + platform/copy_file.rs),无可改 |
| `impl Drop` | 11 | 全 RAII 守卫(ActiveGuard/ChatGuard/StopGuard 等),设计正确 |
| `dangerouslySetInnerHTML/innerHTML` | 0 | ✓ |
| `eval/new Function` | 0 | ✓ |

## Bug 候选清单

### BUG-001 (S2): MutexGuard 跨 .await(async 测试中)
**位置**: `tests/skill_e2e.rs:371, 470, 527, 621` + `tests/task_chat_exec.rs:95, 228, 276`(共 7 处)
**证据**:
```rust
// skill_e2e.rs:371
let _serial = SKILL_SCHED_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
// ... 测试体内后续 .await ...
```
`std::sync::MutexGuard` 在 `#[tokio::test]` async fn 内持续持有,直到函数末尾才 drop。clippy 报 `MutexGuard is held across an await point` × 7。
**风险**:
- tokio 多线程 runtime 下,持锁跨 await 可让 worker 调度错乱
- 若 task 被 cancel(drop 闭包未跑),guard 提前 drop,其他 waiter 可能惊群
- 当前测试 single-thread 跑不表现,但加大并发或换 `current_thread` → `multi_thread` 会 flaky
**类别**: latent — 测试稳定后才暴露
**触发条件**: 跑 `cargo test` 多线程并发时;或 refactor 切到 `flavor = "multi_thread"`
**修复方向** (Phase 2/3 验证后):
- 改 `tokio::sync::Mutex`(async-aware);或
- 把 guard 绑到 RAII 块里,await 前显式 drop

### BUG-002 (S3): `Default::default()` 后赋字段(测试 fixture 反模式)
**位置**: `bot_skills/runtime.rs:670, 678, 689`(clippy: `field assignment outside of initializer`)
**证据**:
```rust
let mut m = SkillMeta::default();
m.name = "x".into();
m.enabled = false;
```
**风险**: 极低,代码工作正确;只是反 builder 模式,影响代码风格
**类别**: design
**修复方向**: 改为 `SkillMeta { name: "x".into(), enabled: false, ..Default::default() }`

### BUG-003 (S3): `items after test module` × 2(代码组织)
**位置**: `api_auth.rs:127` / `bot/config/mod.rs:77`
**证据**: clippy `items after a test module`
**风险**: 无 — 不影响行为,IDE/clippy 视觉无碍
**类别**: design
**修复方向**: 把 `#[cfg(test)] mod tests` 挪到文件底部

### BUG-004 (S3): `too_many_arguments` × 2(API 设计)
**位置**: `memory/store.rs:351` (8 args) / `py/runtime.rs:404` (8 args)
**证据**: clippy 阈值 7,实际 8
**风险**: 无 — 签名稳定,后续可能加 9th
**类别**: design — DEVLOG 2026-09-14 口径「禁止为过 lint 拆函数」
**修复方向**: 单独立项,Introduce Parameter Object(本次不动)

### BUG-005 (S3): `too_many_lines` × 2(已在 DEVLOG 留档)
**位置**: `migration/run.rs:34` run_migration_inner **314 行** / `lib.rs:169` run **254 行**
**证据**: clippy 阈值 200
**风险**: 无 — DEVLOG 已明确「禁止为过 lint 拆函数,只作留档观察」
**类别**: design — 已留档
**修复方向**: 不动

### BUG-006 (S3): `very complex type` × 8
**位置**: `audit.rs:608` + tests 7 处
**证据**: clippy `very complex type used. Consider factoring parts into type definitions`
**风险**: 无 — 复杂度但不危险
**类别**: design
**修复方向**: 抽 type alias

## 假阳性 / 已排除(避免重复扫到)

| 项 | 位置 | 排除理由 |
|---|---|---|
| `bot_chat.rs:107` `messages.len() - 1` | bot_chat.rs:107 | 循环只在 `messages` 非空时执行,不可能 underflow |
| `bot_fs.rs:397` `offset + slice.len() - 1` | bot_fs.rs:397 | `offset = v["offset"].as_u64().unwrap_or(1).max(1)`,offset ≥ 1 保底;slice.len() ≥ 0 |
| `bot_web.rs:514` `haystack.as_bytes()[from..]` | bot_web.rs:514 | `from >= haystack.len()` 守卫返回 None |
| `bot_web.rs:498` `inner[..end]` / `tag[..end]` 等 | bot_web.rs | end 来自 `String::find`,恒 ≤ inner.len() |
| `bot_web.rs:478/482/485` `rest[pos + 2..]` 各种 | bot_web.rs | `consumed.min(tail.len())` / `end.min(...)` 边界保护 |
| `decode_entities` 实体解析 | bot_web.rs:477-498 | consumed 用 `.min()`,无 panic 风险 |
| `db/tasks.rs:200` `format!("DELETE FROM tasks WHERE id IN ({placeholders})")` | db/tasks.rs:200 | placeholders = `?, ?, ?` 串,不是用户输入,SQL 注入安全 |
| `bot_py.rs:355-1211` 大量 `.expect(...)` | bot_py.rs | 全部在 `#[cfg(test)] mod tests` 内 |
| `migration/*` 大量 `.ok()` 错误吞没 | migration/mod.rs:122+ | journal 幂等操作(已 committed/cleared 再清=无害),有 `// 已知` 注释或审计 |
| `api_auth.rs:117/123` `let _ = fs::write/remove_file` | api_auth.rs | 开关 flag 文件,best-effort 设计 |
| `bot_skills/manage.rs:366` `impl Drop for TempDir` | bot_skills/manage.rs | RAII 临时目录清理,正确 |
| `chrono::Local::now` 混用 `Utc::now` | bot/tools.rs:25+404 | 设计:Local 给人类读,Utc 给 DB 存 |

## 待 Phase 2 验证

- [ ] BUG-001: 在 multi-thread runtime 下跑现有测试,确认是否真 flaky
- [ ] BUG-002/3/4/5/6: design 问题,不需要动态验证,直接进 Phase 3 决定修不修

## 设计选择(NEEDS-DECISION)

- 9 unsafe(全 FFI):不修,FFI 必需
- 62 个 Tauri command:逐个签名过,无明显 input 验证缺失(路径走 bot_fs.rs 白名单核,字符串走 API_MAX_* 上限);不修
- 11 个 Drop:全部 RAII 守卫,设计正确;不修
