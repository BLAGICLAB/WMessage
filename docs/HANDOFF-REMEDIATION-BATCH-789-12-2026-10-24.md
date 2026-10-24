# 交接提示词：Agent 整改剩余四批（批 7 / 8 / 9 / 12）

> 用途：本文是交给下一个编码代理的完整交接提示词（2026-10-24）。
> 前情：整改方案 `docs/AGENT-REMEDIATION-PLAN-2026-10-24.md` 的 13 项已完成 9 批
> + 批 5 主体（git log `0a7949d4..fb613e0a` 共 14 个 commit，pre-push 级门禁全绿），
> 剩余批 7 / 8 / 9 / 12 由你推进。

## 〇、你的任务与工作区

- 仓库：`/Users/renshi/Projects/wmessage`（Tauri 2 桌面应用 + 内置 AI agent，Rust ~12.5 万行）
- 任务：按 `docs/AGENT-REMEDIATION-PLAN-2026-10-24.md` §三 推进剩余四批——
  **批 7**（run_model_loop_core 拆分）、**批 8**（bot_orchestrator 拆分）、
  **批 9**（命名空间双路径收敛）、**批 12**（模块审计补批 4/批 6，只读）。
- 动手前先跑 `git log --oneline -16` 确认 14 个整改 commit 在场、`git status` 干净。

## 一、必读（按序，动手前读完）

1. `AGENTS.md` — 工作约定第一屏（硬性禁令 / 命令 / 工作方式）。
2. `docs/AGENT-REMEDIATION-PLAN-2026-10-24.md` — 总方案：§三 各批详案、
   **§七 偏离登记 6 行**（批 1/2/5 的偏离会改变你看到的代码形态与锚点）。
3. `docs/rust-bot-architecture.md` §1 模块树 — 你新建/移动的文件必须同步登记
   §1 与 §6.4（含行数标注），否则对拍门禁拦提交。

## 二、门禁与环境事实（上一会话实测，别再试错）

**仓库级门禁（人人都会撞）**：

- `git commit` 自动跑 `scripts/test-fast.sh`：fmt --check / cargo check /
  **clippy --all-targets -D warnings** / machete / tsc / knip / oxlint / vitest --changed。
  `cargo fmt` 忘跑会被拦；clippy 的 needless_borrow / too_many_lines /
  redundant_closure 是高频拦截项。
- pre-push 跑 `scripts/test-all.sh`（nextest 全量 + vitest + tests-audit 对拍 33 例）。
- **工单号防线**：`*.rs/*.ts/*.tsx` 新增行禁含工单号模式（`P0-12` / `批次N审计` /
  「拍板」等；`docs/*.md` 不受限）。注释只写"为什么"，确需引用历史口径行内加 `audit-ok`。
- `tests-audit/audit_module_map.py` 三道对拍：模块树条目 ↔ 真实文件、§6.4 清单双向、
  **散文数字（共 161 个文件 / 约 12.5 万行）与逐文件行数标注 (NNN) 必须与实测一致**。
  拆完文件先本地跑 `python3 -m pytest tests-audit/audit_module_map.py -q` 再提交。
- 当前测试基线（你的每批交付底线）：Rust lib **2003 例**全绿 +
  集成 llm_integration 103 / skill_e2e 12 / task_chat_exec 14 全绿 +
  vitest 1065 例；`bash scripts/mutation-trace-funnel.sh` 保持 **KILLED=12 SURVIVED=0**。
- `bot/mcp/manager.rs` 的 `connect_discover_call_roundtrip`（e2e_stdio 组）有 30s
  桩启动竞态，**偶发首跑失败，复跑即绿**——先复跑 2 次再当真失败处理。

**工具链预检钩子（仅当你的运行环境同样挂 Mimosa 时适用）**：

- Bash 里 sed/awk/python 直写 `src-tauri/src` 源码会被拦（连只读 sed -n 都可能误判）
  → 读文件用 Read 工具、改文件用 Edit/Write 工具，不要硬刚。
- 源码里「env 变量拼路径再读文件」会判路径穿越 → 常量路径或显式报错（`bot/eval.rs` 是过审先例）。

## 三、批 7 · run_model_loop_core 拆分（2025 行 → core 本体 ≤400 行）

- 对象：`src-tauri/src/bot_model_loop.rs`（现 6018 行）。**拆的是
  `run_model_loop_core`（按函数名定位，约 :1850 起）**，不是薄壳
  `run_model_loop`（约 :1368 起）——薄壳头部注释解释了它为什么长
  （钩子相对答案定稿的位置不变量，`post_turn_layer_tests` 在守），那部分不在本批。
- 五个提取单元（锚点以函数/关键字定位，别信行号；方案 §三 批 7 有表）：
  1. `prepare_round`：工具 schema 选取 / 熔断上限派生（fuse_cap_for）/ 轮前 compaction
  2. `request_round`：SSE 请求装配 / 429 指数重试 / Anthropic thinking 重建 /
     `shed_oldest_history` 砍史重发
  3. `finalize_without_tools`：无工具收尾——防幻觉守卫 + 终稿自检判定
  4. `dispatch_round`：工具轮——`execute_tool` 分发 / 结果回灌 / 凭证登记（written_paths）
  5. `check_fuses`：轮数与墙钟软警告 + 熔断出口带正文（FUSE marker）
- 共享局部收敛到私有 struct `RoundCtx`：只聚合既有局部变量，**不加方法不加 trait**。
- 纪律：**每个提取单元一个 commit**；纯机械搬移——签名语义不动、行为不动，
  diff 里出现任何非搬移的行即回退重做；每 commit 前
  `cargo test --lib -- bot_model_loop::` + `cargo test --test llm_integration` 全绿。
- 批 5 偏离后的现状（别按旧锚点找）：core 里已无 `audit_log` 局部变量（审计统一走
  结构化 `audit` 闭包）；溢出漏斗已抽成泛型 `apply_output_cap`（在薄壳 execute_tool
  闭包内调用，不在 core 里）。
- core 收进 400 行内时移除其上的 `#[allow(clippy::too_many_lines)]`。
- 第二步（私有函数挪 `bot/loop/` 子模块）**本批不做**，留待拍板。

## 四、批 8 · bot_orchestrator 拆分（6097 行 → 每文件 ≤1200）

按职责拆五个文件（bot_orchestrator.rs 保留门面 re-export，`crate::bot_orchestrator::`
旧路径 1:1 兼容——批 9 才统一收口）：

```
bot/subagent_spawn.rs    spawn / clamp_budget / 并发闸 SubagentGate
bot/subagent_runner.rs   run_subagent + 取消 watcher + tokio timeout 硬停
bot/subagent_digest.rs   digest 构建 / 注入 / mark_digested / 父会话回报
bot/subagent_repair.rs   repair_final_json_once（收尾 JSON 三段修复）
bot/subagent_tools.rs    tool_spawn_subagent / 产物写工具 / gen_dir_rule
```

- `run_subagent`（约 947 行）内部再按段提取私有函数（排队 → ChatGuard → 循环 →
  收尾落库），手法同批 7。
- **runner 的 `catch_unwind` 包裹不能搬丢**——有源码守卫测试钉着
  「runner future 必须包 catch_unwind」（搬移后跑 `cargo test --lib -- bot_orchestrator` 验证）。
- 每文件一 commit；§1 模块树 + §6.4 随批登记（行数标注同步刷新）。

## 五、批 9 · 命名空间双路径收敛（方向已拍板：A 门面转正，别翻案）

- 现状摸底（10-24 数字，动手前重新 grep 核实）：`crate::bot::` 门面路径 913 处 vs
  绕过门面的直接深层引用（`crate::bot_chat::` / `crate::bot_model_loop::` /
  `crate::bot_orchestrator::` 等）约 372 处。
- 做：`crate::bot::` 定为唯一规范路径（bot.rs 从「兼容层」改口为「bot 族唯一对外
  门面」，同步改注释）；把 372 处直接深层引用迁到门面，**门面缺的 re-export 补齐**；
  完成后加源码扫描守卫——bot 族文件（bot.rs / `bot/` 目录 / `bot_*.rs` 扁平文件）
  之外禁止直接 `use crate::bot_*::` 深层路径（bot 族内部互引不受限）。
  守卫可进 audit.rs 守卫族或 tests-audit 脚本。
- 手法：编译器驱动逐文件迁，按文件族分 commit（memory/ → meta/ → 顶层非 bot 文件 →
  bot 族互引）；每 commit 编译零错 + 全测试绿。
- B 方案（删门面、913 处全迁深层路径）已否决存档于方案 §三 批 9，不重议。

## 六、批 12 · 模块审计批 4 / 批 6（只读，不改码）

- 对象（账本 `docs/module-audit-2026-10-22.md` 剩余两批）：批 4 = M2 聊天决策核心
  （bot_chat / bot_model_loop / intent_router）+ M6 记忆（memory/）+ M7 技能
  （bot_skills/）；批 6 = M8 MCP（bot/mcp/）+ M9 服务进程（bot/services.rs、
  bot/procs.rs）。编号以账本既有分组为准，开审前先核对一遍。
- 方法沿用账本既定：只读子代理分模块审 + **P1 及以上由你逐条读码复核（行号 +
  触发条件）后才记为事实**，P3 附代码行；发现全部记账进账本新增的批 4 / 批 6 两节，
  **修复不在本轮**（另开批次）。
- 增量口径（不重审已核绿的）：10-21 审计的 MCP 子审结论、账本各批「已查证无问题」
  清单、M2/M6-M9 盲区抽查的 12 条 P3——后者只复核「是否仍成立」。
- **你审的是批 5 改造后的代码**：audit_log 已桥接（bot.log 行 = INFO | event |
  detail=...）、create_skill 会话档路由、端口闸金丝雀、MCP over_budget 都是
  10-24 新落地——按新形态审；若发现解析端有假设老行格式的地方，**记为发现，别顺手修**。

## 七、顺序、收尾与禁做清单

- 顺序：批 7 → 批 8 → 批 9（三批都大改 bot_* 文件，**必须串行**）；批 12 只读，
  可穿插并行。
- 每批动手前 checkpoint：工作区干净 commit + `cargo test --lib` 全绿。
- 全部完成后总验收：`bash scripts/test-all.sh` 全绿 +
  `bash scripts/mutation-trace-funnel.sh` KILLED=12 +
  `python3 -m pytest tests-audit/audit_error_codes.py tests-audit/audit_module_map.py tests-audit/audit_tauri_bridge.py tests-audit/audit_doc_claims.py -q`
  （必须逐个点名，glob 收不到会假绿）+ `python3 tests-audit/audit_no_eval.py`。
- 在方案 §七 偏离登记回填你的偏离（锚点漂移 / 新决策），逐批 commit message 写清
  落点与行数前后对比。
- **禁做**：不主动 `git push`（用户自己推）；不主动出 Windows 包；不跑评估器
  `bot::eval`（花真钱，MiniMax 账户当前 402 余额不足）；不动已完成九批的行为；
  顺手重构未要求的遗留 = 违反 AGENTS.md。
- 口径拿不准：先查方案 §七 偏离登记 → 再查 DEVLOG / git log → 仍拿不准就停下问用户，
  别自作主张改口径。

## 八、完成汇报格式

每批一段：落点 commit 号、行数前后对比（core ≤400 / 最大文件 ≤1200 / 深层引用 0）、
测试数（lib + 集成 + vitest）、批 12 的发现清单（P0/P1/P2/P3 分级 + 行号）。
