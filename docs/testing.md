# 测试与门禁手册

> 面向日常维护者（含 AI 代理）。回答三个问题：提交时什么在挡我、失败了怎么修、什么时候跑全量。
> 阶段 5（2026-09-11 审计收官）定型；门禁实现都在 `scripts/test-fast.sh` / `scripts/test-all.sh`。

## 日常提交流程

```bash
scripts/install-hooks.sh   # 首次 clone 后跑一次（幂等）：git 指向版本化 .githooks/
```

之后每次 `git commit` 自动跑 pre-commit → `scripts/test-fast.sh`：

- 按 staged/unstaged 改动文件**智能跳过**：只改了 docs/ 就什么都不跑（"nothing to test"）；
  碰 `src-tauri/` 才跑 Rust 链路，碰 `src/` 才跑 TS 链路，碰 `tests-audit/` 才跑 pytest collect。
- 预期耗时：无相关改动 0s；单链路几秒；全链路（含 vitest）< 30s。
- 手动跑一份同样的检查：`bash scripts/test-fast.sh`（不用 stage，看 unstaged+staged）。
- 万不得已跳过门禁：`git commit --no-verify`（别养成习惯——门禁抓到的都是真问题，
  若确定误伤见下文「豁免方式」）。

## 步骤清单（test-fast.sh）

| 步骤 | 防什么 | 失败了怎么修 |
|---|---|---|
| `[0/N]` 审计批次号防线 | 注释里混入审计过程产物（`批次N审计` / `P0-12` / `T1-3` / `NEW-C-6`），烂成考古化石 | 把批次号从新增注释里删掉，只留"为什么"；确需引用历史口径 → 行内加 `audit-ok` |
| `[1/N]` cargo fmt --check | 格式漂移 | `cargo fmt --manifest-path src-tauri/Cargo.toml` |
| `[2/N]` cargo check | 编译错误 | 按报错修 |
| `[2.5/N]` cargo machete | 未使用的 Rust 依赖（Cargo.toml 只进不出） | 删依赖；误报 → Cargo.toml `[package.metadata.cargo-machete] ignored = [...]`。未安装时该步 skip 并提示：`cargo install cargo-machete --locked` |
| `[3/N]` pytest collect | tests-audit 脚本语法坏掉 | 修对应 py 脚本 |
| `[3.5/N]` Tauri 桥一致性 | 前端 `invoke()` 调了未注册命令、`listen()` 听无人 emit 的事件（运行时才炸的坑提前到提交时炸） | 看 `tests-audit/audit_tauri_bridge.py` 报错指认的命令/事件名：补注册或修前端。反向（注册未调用/发而无听）只 warn 不 fail |
| `[3.6/N]` 错误码一致性 | Rust `CommandErrorCode` 枚举与前端 `CommandErrorCode` 联合类型漂移（前端 hint 分流静默失配） | 看 `tests-audit/audit_error_codes.py` 报错指认的 code：补 `error.rs` 变体 / `errorHandler.ts` 联合类型与 hint |
| `[3.7/N]` 模块地图对拍 | 架构文档模块树与实际模块脱节（模块删改/新增后文档没跟，新人第一入口就骗人） | 按 `tests-audit/audit_module_map.py` 报错改 `docs/rust-bot-architecture.md` 模块树（条目指向真实文件、每个源码文件都有条目） |
| `[4/N]` tsc --noEmit | 前端类型错误 | 按报错修 |
| `[4.5/N]` knip | 前端死文件/死 export/未使用 npm 依赖 | 删或改为内部使用；tauri 插件类包若只在 Rust 侧用（字符串 invoke），属低置信误报，按实际情况取舍 |
| `[5/N]` vitest --changed | 前端单测回归（只跑改动相关） | 修测试或修代码 |

注意点：

- **machete 的正确用法是位置参数** `cargo machete src-tauri`（不支持 `--manifest-path`，
  这个坑是 fail 路径注入验证时抓到的）。
- knip 在 devDependencies（`^6.35.1`），门禁走本地安装：`npx --no-install knip --no-progress`。
- 批次号防线只看 staged 的 `.rs/.ts/.tsx` **新增行**——存量历史注释不会误炸。

## 全量验证（push 前 / 大改动后）

```bash
bash scripts/test-all.sh   # pre-push 自动跑：nextest 全量 + tests-audit + vitest
```

分项：

```bash
cd src-tauri && cargo test --lib          # Rust lib 单测（当前 637 例）
npm test                                  # 前端 vitest（当前 209 例，21 文件）
cargo test --test llm_integration         # 集成：mock LLM 全链路
cargo test --test task_chat_exec          # 集成：任务卡执行聊天化
cargo test --test skill_e2e               # 集成：Skill DSL 端到端
cargo test --test memory_v2_degraded      # 集成：记忆体 v2 降级模式
cargo test --lib memory::embed -- --ignored   # bge 模型真实推理冒烟（需仓库根 bge-small-zh-v1.5/）
```

## 一致性检查（tests-audit/）

源码文本断言式检查（不跑业务代码，读源码断言结构/协议），`test-all.sh` 全量跑：

- `audit_pre_step_pre_execute.py`：主调度循环与 pre-step/pre-execute 中间件联动规格锁
- `audit_tauri_bridge.py`：Tauri 桥双向一致性（命令注册 ↔ 前端 invoke、emit ↔ listen）
- `audit_error_codes.py`：错误码跨语言一致性（`error.rs::CommandErrorCode` ↔
  `errorHandler.ts::CommandErrorCode` 集合与声明顺序）
- `audit_module_map.py`：模块地图对拍（`docs/rust-bot-architecture.md` 模块树 ↔ 实际
  `src-tauri/src/**/*.rs`：树条目必须指向真实文件，每个源码文件必须有条目）

Rust 侧另有编译期协议锁先例可参考：`bot/registry.rs` 的 `registry_tests`
（schema ↔ `TOOLS_TABLE` ↔ baseline 三源一致 + `ToolDef.name` 与 schema 内名一致）、
`bot_model_loop.rs` 的 `tools_schema_parses`、`lib.rs` 的 `dead_commands_not_registered`、
`mutation.rs`/`consts.rs` 的协议漂移锁。

### 安全相关锁（改动判定逻辑时先看这里）

| 锁 | 位置 | 拦什么 |
|---|---|---|
| 白名单逃逸 | `bot_fs.rs::allowlist_rejects_traversal_and_prefix_similar_dir` / `allowlist_rejects_symlink_escape` | `..` 穿越、软链逃逸、前缀相似目录（`/x/ab` vs `/x/abc`）误吞——判定核 `is_within_allowlist` 是 canonicalize 之后的分量比较 |
| 打开/删除放行口 | `bot_skills/files.rs::openable_*`（3 例） | 绑定集合必须**精确命中**；产物目录内才放行，`..`/软链/前缀相似目录/不存在路径全拒（`open_file_path`、`delete_bound_file` 是前端直达命令，属 XSS→RCE 一跳） |
| intent 规则松紧 | `intent_router.rs::every_docx_pattern_has_positive_and_negative_case` | **每条 pattern 一正一负**（正例命中该条、负例整条路由 PassThrough），防规则越写越松把无关输入捞进 Skill 链路 |
| bypass 开关语义 | `bot/config.rs::bypass_llm_switch_defaults_true_and_honors_explicit_false` | 只有显式 `false` 才关 bypass；文件缺失/损坏/缺字段一律默认开 |

## 门禁误伤时的豁免方式

| 门禁 | 豁免 |
|---|---|
| 批次号防线 | 行内加 `audit-ok`（留一句为什么引历史口径更好） |
| cargo machete | Cargo.toml `[package.metadata.cargo-machete] ignored = ["包名"]`（误报时） |
| knip | 优先按报告修；确属误报再考虑 knip 配置豁免（并在注释写清原因） |
| 全部 | `git commit --no-verify`（最后手段） |
