# Batch Spec: B5-HEALTH

## 目的

B5 批「前端与工程健康」的**自动化与门禁部分**（B5-1 / B5-5 / B5-6 一阶段 /
B5-7 部分）。B5-2/B5-3（ChatPanel memo + 拆分 1851 行）与其余 clippy 机械微修
单独成批（本批先把地基铺好）。

- **B5-6 clippy 分级治理（273 → 169，-38%）**：
  - `cargo fix`（rustc 级）清 **24 个死 import**（两个 cargo fix 误删的测试
    facade/测试专用 import 已恢复并在 bot_py.rs 注释里立警告）；
  - `clippy --fix` 机械批吃掉 3；
  - **三类纯噪音 lint 在 Cargo.toml `[lints.clippy]` 显式放宽**（各带理由注释，
    放宽 = 有意工程决策非漏修）：doc_lazy_continuation（52 条文档列表续行，
    语义修复需逐条改写文档）、type_complexity（24 条测试 mock 泛型签名）、
    too_many_arguments（4 条装配入口）；
  - **await_holding_lock 15 条（审计点名的危险类）核实为测试串行锁故意持有**
    （#[tokio::test] 独立 current-thread runtime，guard 持至测试结束正是串行化
    语义）→ 三个测试文件 allow + 理由注释，非放任真问题。
  - 剩余 169 条为 30+ 类风格微修（useless vec 8 / redundant closure 7 /
    needless borrow 7 / Default field init 8 …），--fix 无 suggestion，逐条
    手修登记后批（清单可由 `cargo clippy --all-targets` 再生）。
- **B5-1 linter 接入**：oxlint 1.86（devDep）+ `.oxlintrc.json`
  （correctness/perf 类 + react-hooks：**6 处既有 eslint-disable 注释重新生效**）；
  test-fast.sh 新增 [4.6/N] 步骤（error 拦 / warning 放行）；`npm run lint`；
  knip ignoreDependencies 声明。存量 32 条 warn（exhaustive-deps /
  set-state-in-effect / no-map-spread）随 ChatPanel 批清理。
- **B5-5 tsconfig**：`noImplicitOverride` 启用（3 处 ErrorBoundary 成员补
  override）。`noUncheckedIndexedAccess` 实测 **174 处**（72% 在测试文件，
  审计预估「十余处」差一个量级）→ 登记后批，本批不硬启。
- **B5-7 部分**：`npm audit` 复核（3 moderate，无 high/critical；audit fix 无
  动作）；cargo-deny 引入评估为重型操作，登记随 B6。

## 红线

- bot_py.rs 的 audit/env/io glob re-export 是有意的 facade（测试经 super:: 解析）
  ——已加注释禁止自动 fix 触碰 use 区。
- oxlint 门禁不阻塞存量 warning（warn 不拦 commit），error 级（rules-of-hooks）
  当场拦。

## 测试

- 全量回归：evolution 283 / task_chat_exec 14 / llm_integration 37 / py 64 /
  mcp 42 / config 74 / vitest 339 / tsc 0 错 / test-fast 7s 全绿（含新 oxlint 步骤）。
- clippy 273 → 169（剩余分布清单在 DEVLOG）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "B5-HEALTH",
  "family": "engineering-health",
  "expected_files": [
    ".oxlintrc.json",
    "DEVLOG.md",
    "docs/batches/B5-HEALTH.spec.md",
    "package-lock.json",
    "package.json",
    "scripts/test-fast.sh",
    "src/App.tsx",
    "tsconfig.json",
    "src-tauri/Cargo.toml",
    "src-tauri/src/api_handlers/commands.rs",
    "src-tauri/src/api_handlers/handlers.rs",
    "src-tauri/src/api_handlers/util.rs",
    "src-tauri/src/bot/config/keyring.rs",
    "src-tauri/src/bot/config/mod.rs",
    "src-tauri/src/bot/mcp/commands.rs",
    "src-tauri/src/bot_fs.rs",
    "src-tauri/src/bot_model_loop.rs",
    "src-tauri/src/bot_orchestrator.rs",
    "src-tauri/src/bot_py.rs",
    "src-tauri/src/db/mod.rs",
    "src-tauri/src/db/paths.rs",
    "src-tauri/src/evolution/activation.rs",
    "src-tauri/src/evolution/observe/shadow.rs",
    "src-tauri/src/py/document.rs",
    "src-tauri/src/py/env.rs",
    "src-tauri/src/py/runtime.rs",
    "src-tauri/tests/skill_e2e.rs",
    "src-tauri/tests/task_chat_exec.rs"
  ],
  "max_lines_added": 560,
  "max_lines_removed": 60,
  "findings": [
    {"id": "B5-6", "file": "src-tauri/Cargo.toml", "line": 9, "fix": "clippy 分级治理：三类噪音 lint 显式放宽（带理由）+ cargo fix 死 import 清理（273→169；波及 17 文件的 use 区，测试 facade import 已恢复）"},
    {"id": "B5-6", "file": "src-tauri/src/evolution/observe/shadow.rs", "line": 509, "fix": "await_holding_lock 测试豁免（串行锁故意持有，3 文件）"},
    {"id": "B5-1", "file": ".oxlintrc.json", "line": 1, "fix": "oxlint 接入：react-hooks 生效 6 处 eslint-disable + test-fast [4.6/N] error 级门禁"},
    {"id": "B5-5", "file": "tsconfig.json", "line": 18, "fix": "noImplicitOverride 启用（3 处补 override）；noUncheckedIndexedAccess 实测 174 处登记后批"}
  ],
  "assertions_min": {
    "src-tauri/src/evolution/observe/shadow.rs": 80
  },
  "stop_conditions": ["compile_failure", "architecture_blocker", "family_heterogeneity", "new_high_different_root", "gate_fail"]
}
```

## 验证命令

```
cargo clippy --all-targets 2>&1 | grep -c '^warning'   # 169（剩余登记）
npx oxlint src && npx tsc --noEmit && npx vitest run
bash scripts/test-fast.sh
```
