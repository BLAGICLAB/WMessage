# Bug Hunt 2026-09-15 — 自主运行 Handoff(不 commit)

## 状态
- 开始时间: 2026-09-15T07:50+08:00
- 当前 Phase: **4 已收工**
- 交付: 3 个独立 patch + 合并 FULL.patch + 完整验证证据 + 0 commit
- HEAD: 00d494f(branch main,working tree 修复后已 revert 干净)
- 交付形式: working tree 改动 + 每 bug 一个 patch
- HEAD: 00d494f(branch main,working tree clean)
- 纪律: 绝不 commit/push/分支/stash/amend/rebase/touch .git;Phase 1-2 只找不改

## Baseline(2026-09-15T07:50)
- `cargo test --lib`:**663 passed; 0 failed; 2 ignored**(6.21s)
- `npm test`:**20 test files / 209 passed**(3.12s)
- `cargo clippy --all-targets -- -D warnings`:**144 errors**(预期 — 项目明文保留 lint 留档;`clippy.toml` 阈值 200、warn-only;DEVLOG 2026-09-14 口径「禁止为过 lint 拆函数」)
  - Phase 4 回归不能直接 `cargo clippy -- -D warnings`,需用更宽阈值或显式 allow;详见 Phase 4

## Bug 清单总览(Phase 1+2 完成,Phase 3 进行中)

| ID | 严重度 | 位置 | 一句话描述 | 状态 |
|---|---|---|---|---|
| BUG-001 | S2 | `tests/skill_e2e.rs:270,371,470,527,621` + `tests/task_chat_exec.rs:27,95,228,276` | `std::sync::MutexGuard` 在 `#[tokio::test] async fn` 内跨 `.await` 持有 | **已修**(patches/BUG-001.patch,reports/BUG-001.md) |
| BUG-002 | S3 | `bot_skills/runtime.rs:670,678,689` | `Default::default()` 后逐字段赋值,应改 struct literal | **已修**(patches/BUG-002.patch,reports/BUG-002.md) |
| BUG-003 | S3 | `api_auth.rs:127` + `bot/config/mod.rs:77` | `items after test module` — test module 应放文件底 | **已修**(patches/BUG-003.patch,reports/BUG-003.md) |
| BUG-004 | S3 | `memory/store.rs:351` (8/7) + `py/runtime.rs:404` (8/7) | `too_many_arguments` | **不动**(DEVLOG 留档 8/24 口径) |
| BUG-005 | S3 | `migration/run.rs:34` 314 行 + `lib.rs:169` 254 行 | `too_many_lines` | **不动**(DEVLOG 留档) |
| BUG-006 | S3 | `audit.rs:608` + tests 7 处 | `very complex type` | 待决策(抽 type alias 单独立项) |

## 已修复 / 待决策 / 无法复现

### 已修复 (3 项)
- **BUG-001** [S2 latent]: `std::sync::MutexGuard` 跨 `.await` 持锁 → `tokio::sync::Mutex + LazyLock`;7 处全消,skill_e2e 13/13 + task_chat_exec 14/14 + lib 663/663
- **BUG-002** [S3 design]: `Default::default()` 后逐字段赋值 → struct literal `..Default::default()`;3 个测试函数改写
- **BUG-003** [S3 design]: `items after test module` → 2 个函数搬至 `mod tests` 之前;纯位置调整,函数体不动

### 待决策 / 不动 (3 项)
- **BUG-004** `too_many_arguments` (8/7) × 2 — DEVLOG 2026-09-14 口径:禁止为过 lint 拆函数,留档观察
- **BUG-005** `too_many_lines` × 2 (run_migration_inner 314 行 / run 254 行) — 同口径
- **BUG-006** `very complex type` × 8 — 单独立项(抽 type alias),本次范围外

### 无法复现 / 误报
- clippy `MockBehavior` 3 个 variant "never constructed" — 实际在 `tests/llm_integration.rs:849/888/1053/1092` 已构造,clippy 跨 test binary 分析盲区,不动

## 建议用户 review 顺序
1. **本文件(HANDOFF.md)** — bug 表 + 状态
2. **`FINAL-REPORT.md`** — 完整统计 + Phase 4 回归证据
3. **`reports/BUG-001.md`** — S2 latent,改动最大(引入 LazyLock),先看
4. **`reports/BUG-002.md`** + **`reports/BUG-003.md`** — S3 mechanical,改动小
5. **`patches/*.patch`** — diff 自带逐行解释
6. **`reports/phase1-static.md`** — 扫描方法学 + 已排除项(避免重复检查)

## 用户如何提交 / 回滚
- 提交全部: `git add -A && git commit -m "..."`
- 回滚全部: `git checkout -- . && git clean -fd`
- 回滚单个 bug: `git apply -R .bug-hunt-2026-09-15/patches/BUG-XXX.patch`

## 已知边界
- clippy -D warnings 跑不过(留档 144 条),Phase 4 用 `-- -D clippy::correctness -D clippy::suspicious -D clippy::security` 收紧到合理范围
- busy_timeout 保持 2s;不改 prompt/schema/命令名/事件名/JSON 字段/错误码;不改安全校验;不新增依赖
- 测试文件改动只在「新增复现测试」,不动既有断言
