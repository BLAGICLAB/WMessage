# Bug Hunt 2026-09-15 — FINAL REPORT

> 自主模式运行 07:50 → ~08:30,HEAD `00d494f`(branch main,working tree 修复后已 revert 干净)。3 个 bug 修复 + 1 个误报澄清 + 3 个留档不动。

## 扫描命中

| 阶段 | 命令/工具 | 命中 |
|---|---|---|
| 静态 grep | `unwrap/expect/panic!/unreachable!/todo!/unimplemented!` | 890 unwrap / ~10 expect / 0 panic(全在 `#[cfg(test)] mod tests` 内) |
| 静态 grep | `as usize` / `len()-N` / 字节切 `[..]` | ~30 / 3(已 verified safe)/ ~25(全有边界守卫) |
| 静态 grep | `Mutex.lock().unwrap()`(生产) | 5(全 sync 路径,无跨 await) |
| 静态 grep | `std::fs` in async fn | 0 |
| 静态 grep | `SQL format!` | 1(`db/tasks.rs:200` DELETE IN,placeholder only,SAFE) |
| 静态 grep | `Regex::new` | 1 runtime + 6 LazyLock 编译期 |
| 静态 grep | `unsafe {}` | 9(全 FFI,py/runtime + platform/copy_file) |
| 静态 grep | `impl Drop` | 11(全 RAII 守卫) |
| 静态 grep | `#\[tauri::command\]` | 62 个 |
| 静态 grep | 前端 `dangerouslySetInnerHTML/innerHTML/eval/new Function` | 0 |
| clippy baseline | `--all-targets -- -D warnings` | 144 errors(预期 — 项目明文保留 lint 留档) |

## Bug 统计

| 等级 | 命中 | 修了 |
|---|---|---|
| **S0 Critical** | 0 | 0 |
| **S1 High** | 0 | 0 |
| **S2 Medium** | 1 | 1 |
| **S3 Low** | 2 | 2 |
| NEEDS-DECISION | 0 | — |
| NOT-A-BUG | 1 | — |

**合计**:6 个候选 → 3 个真 bug 全修 → 1 个误报识别 → 2 个 DEVLOG 留档不动。

## 每 bug 状态

| ID | 严重度 | 一行摘要 | 状态 | Patch |
|---|---|---|---|---|
| BUG-001 | S2 | `std::sync::MutexGuard` 跨 `.await`(7 处) → `tokio::sync::Mutex + LazyLock` | **已修** | `patches/BUG-001.patch` (114 行) |
| BUG-002 | S3 | `Default::default()` 后逐字段赋值(3 个测试)→ struct literal `..Default::default()` | **已修** | `patches/BUG-002.patch` (45 行) |
| BUG-003 | S3 | `items after test module`(2 文件)→ 函数搬到 `mod tests` 前 | **已修** | `patches/BUG-003.patch` (64 行) |
| BUG-004 | S3 | `too_many_arguments` × 2 (8/7) | **不动**(DEVLOG 2026-09-14 口径:禁止为过 lint 拆函数) | — |
| BUG-005 | S3 | `too_many_lines` × 2 (run_migration_inner 314 行 / run 254 行) | **不动**(DEVLOG 留档) | — |
| BUG-006 | S3 | `very complex type` × 8 | **不动**(单独立项,抽 type alias) | — |

## NEEDS-DECISION 清单(用户必须拍板的)

**无**。

BUG-004/005/006 不属于 NEEDS-DECISION — 老板 8/24 在 DEVLOG 已明文「禁止为过 lint 拆函数」且 2026-09-14 clippy 摸底已留档,本轮尊重口径不动。

如需打破口径走 single response,单独立项即可。

## NOT-A-BUG 清单(避免重复检查)

| 项 | clippy 误报原因 | 证据 |
|---|---|---|
| `tests/mock_llm.rs` 3 个 variant "never constructed" (`RawSse`/`AnthropicStreamError`/`AnthropicTextThenToolCall`) | clippy 跨 test binary 分析盲区 | `tests/llm_integration.rs:849/888/1053/1092` 已 `push_behavior(MockBehavior::Variant(...))` 构造 4 次 |

## Phase 4 全量回归(apply all 3 patches 后跑)

| 项 | 结果 |
|---|---|
| `cargo fmt --check` | ✓ 干净 |
| `cargo check --tests` | ✓ 1.56s |
| `cargo test`(全量,7 个 test binary + lib + doc) | ✓ **735 passed; 0 failed; 3 ignored**(详见下) |
| `cargo clippy --tests -- -W clippy::await_holding_lock` | ✓ skill_e2e 11→7(-4)、task_chat_exec 4→1(-3),**7 处 await_holding_lock 全消** |
| `cargo clippy --all-targets -- -D warnings` | ✗ 仍 144 errors,**pre-existing baseline**,未引入新 warning(已 documented) |
| `cargo build --release` | 跳过(耗时长,本次范围外) |
| `npm test` | ✓ **20 files / 209 passed** |

### cargo test 详细(apply all patches 后)

| Binary | Tests |
|---|---|
| lib | 663 passed, 2 ignored |
| 单元 test (其他) | 0 passed |
| api_handlers / exit_cleanup_tests 等 | 34 passed |
| middleware tray_tests 等 | 1 passed |
| llm_integration | 10 passed |
| **skill_e2e** | **13 passed** ← BUG-001 验证 |
| **task_chat_exec** | **14 passed** ← BUG-001 验证 |
| doc-tests | 0 passed, 1 ignored |

合计 735 passed / 0 failed / 3 ignored。

### Patch 应用顺序

```bash
cd ~/Projects/wmessage
git apply .bug-hunt-2026-09-15/patches/BUG-001.patch  # std::sync::Mutex → tokio
git apply .bug-hunt-2026-09-15/patches/BUG-002.patch  # struct literal
git apply .bug-hunt-2026-09-15/patches/BUG-003.patch  # test module 位置
```
或用合并版:`git apply .bug-hunt-2026-09-15/FULL.patch`(223 行,5 文件)

## 用户 review 关键检查点

1. **BUG-001** 改动有「加分项」(引入 `LazyLock<tokio::sync::Mutex<()>>`),其他 2 个改动是纯位置调整。建议先看 BUG-001 报告,确认 `LazyLock` 风格可接受。
2. **BUG-002** 把 `let mut m = X::default(); m.a = ...; m.b = ...;` 改成 `let m = X { a: ..., b: ..., ..Default::default() };`,语义等价(rustfmt 会自动把单行 wrap 成多行)。一处保留 `let mut`(`preflight_rejects_blacklist_intent` 后续还有 mutation)。
3. **BUG-003** 纯位置搬移,函数体一字不动 — `verify_bearer` 仍在 `pub fn verify_bearer` 公开、`_unused_marker` 仍 `#[allow(dead_code)]`,无 API/可见性变化。
4. **不动项** BUG-004/005/006 如要打破 DEVLOG 2026-09-14 留档口径(「禁止为过 lint 拆函数」),需要单独 ack 才能开工。

## 下一步建议

1. **立刻可合**:3 个 patch 独立、无相互依赖、无 production 行为变化、无依赖变更。`git apply .bug-hunt-2026-09-15/FULL.patch && cargo test && npm test` 一条命令验收。
2. **单独立项(可选)**:
   - BUG-004 `too_many_arguments` × 2:Introduce Parameter Object 拆 config struct
   - BUG-006 `very complex type` × 8:抽 type alias
   - 任意一项估 0.5-1 天,需要先 ack DEVLOG 留档口径
3. **clippy warning backlog(单独立项,估半天)**:55 条 auto-fixable 一次性 `cargo clippy --fix --lib -p wmessage --` 处理;余下 89 条手工评估(多为 `items_after_test_module` / `doc list item without indentation` / `useless_conversion` 等)。
4. **本次范围外**:本轮扫描未发现 S0/S1,生产路径主功能稳定。

## 交付清单

```
.bug-hunt-2026-09-15/
├── HANDOFF.md                    # 主入口 — 状态表 + 用户提交/回滚指引
├── FINAL-REPORT.md               # 本文件
├── FULL.patch                    # 3 patches 合并版(223 行,5 文件)
├── baseline/
│   ├── git-hash.txt              # 00d494f404e37c129d0275c54c8a39e2f49d1e4b
│   ├── git-status-before.txt     # 干净(空)
│   ├── test-lib.txt              # 663 passed baseline
│   ├── clippy.txt                # 144 errors baseline
│   ├── npm-test.txt              # 209 passed baseline
│   └── BUG-001-before.txt        # git status(修复前)
├── patches/
│   ├── BUG-001.patch             # 114 行 — MutexGuard 跨 await
│   ├── BUG-002.patch             # 45 行  — Default::default 反模式
│   └── BUG-003.patch             # 64 行  — items after test module
└── reports/
    ├── phase1-static.md          # Phase 1 全扫描结果(只读)
    ├── BUG-001.md                # S2 latent bug 报告 + 验证证据
    ├── BUG-002.md                # S3 design bug 报告
    └── BUG-003.md                # S3 design bug 报告
```
