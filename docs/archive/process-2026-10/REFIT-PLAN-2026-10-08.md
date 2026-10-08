# 改造方案：过度开发收敛 + 注释整洁（2026-10-08）

> 依据：`SPEC.md`「开发基线」（单人口径）+ 同日全仓审计结论。
> 执行纪律：小步提交，一个逻辑变更一个 commit；每删一批引用就
> `cargo check`，每批收口跑 `bash scripts/test-all.sh` 全绿；
> 状态：批 1/1.5/2/3/4/4.5/5 已执行完毕；批 6 的六个文件删除被安全钩子（Mimosa）拦截，留待手动 git rm；批 7 收尾完成。

## 目标（可验收）

- 全仓净删 **≥ 4,500 行**（Rust ~4,000 + TS ~300 + 注释噪音 ~1,100 + 脚本 ~500）
- 注释噪音归零（grep 可验证）：横幅分隔线 0、工单号引用 0、「拍板/老板」记录 0
- 全套门禁绿：cargo nextest + vitest + clippy + machete + oxlint + knip 零 error
- 受影响文档同步：`docs/rust-bot-architecture.md` 模块树、`docs/EVOLUTION-LAYERING-BATCH-A.md`

## 明确不做（出界）

- GraphPage 特性与 sigma/graphology 3 个依赖——那是特性取舍，不是过度开发
- tests-audit/ 六个对拍脚本本体（已降级按需手跑；批 6 只删其中零引用的 4 个孤立脚本）
- `memory/` `bot/` `db/` `migration/` 模块——审计认定健康
- 一切行为变更——本方案全部是行为等价重构与死代码删除

## 批 1：删孤儿模块（Rust，预计 -3,300）

顺序 = 先断引用、再删文件，防 re-export 链断裂：

1. **eval harness**（~1,210）：删 `eval/{runner,case,sampler,feedback,config}.rs` +
   `bin/eval_run.rs`，`eval/` 只留 `metrics.rs`。删前 grep `tests/`（5,434 行）对
   `eval::` 的引用，被删模块对应的集成测试一并删（死代码的测试不算丢覆盖）。
2. **observe CLI 三件套**（~1,080）：`bin/observe_run.rs` + `observe/stop.rs` +
   `observe/synthetic.rs`。
3. **sandbox 平行实现**（~800）：`sandbox/shadow.rs` 整文件；`sandbox/io.rs` 整文件；
   `sandbox/routing.rs` 收敛到只剩 `fnv1a`（proposal.rs:193 在用）。
4. `candidate/conflict.rs` 整文件 + `candidate/mod.rs` 的 re-export（模块头自认
   「待真数据后接线」至今未接线）。
5. `activation.rs` 死半部（shadow_route/should_auto_trigger/can_transition/
   is_calibrating/evaluate_s2 恒 Allow 占位及其测试），保留 `load_state_from_file`
   与枚举定义。
6. 零调用散件：`ttl.rs` 的 is_expired/mark_expired/evict_expired、
   `change/status.rs::can_transition`、`kill_switch.rs::all_on`。
7. 全仓 `#[allow(dead_code)]` 残留约 15 处（bot_artifacts::peek、mcp secrets::has_secret、
   py/io 的 `_unused` 系列等），逐个确认零调用后删除。

验证：`cargo check` → nextest 全量 → `grep` 确认删除物零引用。

## 批 1.5：决策板证据增强（唯一新增批，约 +100~150 行）

删除的是管线和开关，保留三个纯函数内核移植进现行「人工决策板 + 直接落库」路径，
让每次拍板有证据。批 1 执行时先把这三个内核摘出暂存（或从 git 历史捞）：

1. **影子判定**（借 `sandbox/shadow.rs` 核心）：importance 启发式 + 与现有
   lesson top-3 对比的纯函数。`panel/commands.rs` 提案详情返回
   「采纳后：会进 lesson top-3 / 会被现有记忆覆盖（劝退）/ 暂无基准」。
2. **冲突标注**（借 `candidate/conflict.rs::is_conflict`，3 行谓词）：
   提案列表标注「⚠ 与已生效 evo:xxx 同目标冲突（impact 更低/更旧）」。
3. **指标卡 + 回滚预警**（`eval/metrics.rs` 已保留；借 `observe/stop.rs` 阈值思想）：
   决策板顶部加「近 30 天采纳 N / 回滚 M / 任务成功率 Δ」；回滚过阈值时提示
   「建议切手动档」。

前端配合点在批 4 之后做（EvolutionPanel 详情行 / 列表徽标 / 顶部卡）；
不新增表、不调 LLM、不改动落库行为。批 7 一并验收。

## 批 2：抽象降级（Rust，预计 -550）

1. **Middleware 框架拆除**：`middleware.rs`（619 行）的 trait + Registry +
   catch_unwind 链删掉，`lib.rs` 两处注册改为直调
   `intent_router::is_chat_execute_trigger` / `route_user_input`；
   `#[allow(dead_code)]` 的 pre_step_list/pre_execute_list 随之消失。
2. **EvolutionPolicy 单实现 trait → 自由函数**：gate/importance 降为
   `strategy.rs` 内自由函数；删 resolve/order_entries/SelectionResult 与
   「等价对照」测试；`policy.rs` 转发层一并消失。
3. **ShadowSink 泛型 → 具体类型**：`observe/shadow.rs` 生产 impl 只剩
   AppShadowSink，删 trait 与 Mock 泛型参数。

## 批 3：样板收敛（Rust，预计 -300）

1. export/import 五套平行命令样板（tasks/workflow/workspace/mem/workflow_audit）
   → 一个 `export_json` / `import_merge` helper。
2. 六个 `ensure_*` 列补丁函数 → 表驱动一条循环。
3. `candidate/derive.rs` 与 `change/derive.rs` 逐字重复的 derive 三胞胎 → 单份，
   删 `candidate/mod.rs` 的改名 re-export。

## 批 4：前端收敛（TS，预计 -300）

1. SettingsPage：15 个 `{mountedSections.has(x) && …}` 包裹块 → `{label, render}`
   配置表驱动。
2. App.tsx / WidgetApp.tsx 孪生接线（theme 订阅、tasks 收敛、通知权限、
   cancelled 自注销）→ 提取 `lib/useThemeSync` / `useTasksSync`。
3. `format.ts` relativeTime/untilTime 逐行镜像 → 合并为带方向参数的一个函数；
   两个 CSS 锁测试合并为一个文件。
4. ChatPanel 一个 effect 里 8 listen + 8 unlistenSafe → `useTauriListenAll`。

验证：vitest + knip + oxlint 全绿。

## 批 4.5：memoryTuning 设置页 UI（新增，约 +120~180 行）

记忆检索的 8 个参数目前只能手改 bot-config.json（`MemoryTuning`，U17），
设置页记忆区（MemoryPanel）补一张参数卡：

- 8 字段：injectionBudgetChars / topN / recentN / lessonN / capacity /
  decayDays / dedupMergeCosine / dedupHintCosine；钳制区间以后端
  `MemoryTuning::clamped` 为单源（前端只做输入提示，不重复实现钳制）
- 后端补一个 `memory_tuning_get/set` 命令：get 走现成 `read_memory_tuning`；
  set 复用 bot-config.json 原子写回，写入前过 clamped
- 不动注入路径代码；测试：命令往返 + 钳制边界

## 批 5：注释整洁（约 -1,100 行噪音）

规则 = **删过程、留语义**：

| 动作 | 对象 | 量 |
|---|---|---|
| 删 | 横幅分隔线（`// ───`），标题文字保留 | ~400 |
| 删 | 工单号括号引用（W8-ATTACH / OCR C5 / B0-1 / AUDIT-* / SUBA / NEW-D-4），其后的语义保留 | ~640 |
| 删 | 「老板 N:NN 拍板」「OCR rN 采纳」决策/评审过程记录，防连点/防注入等理由保留 | ~170 |
| 迁 | `paths.rs:59` 51 行 flake 调查日志、`app_state.rs:1` 76 行全局状态总表 → 压成 ≤5 行 + 指针（详情进 DEVLOG 或 docs/） | ~120 |
| 删 | 测试文件复述型注释（「触发折叠」「输入新 key → 保存」）、展望性空头注释、已删代码的历史变迁注释 | 散点 |

保留不动：serde wire 格式约束、命令副作用契约、PRAGMA 不变量与回归锁指针、
一切「为什么」型行内注释。

执行口径：按 grep 清单机械改写 + 逐行过目 diff；改写后的新增行不得含工单号
（删干净即天然合规，不会触发 test-fast 第 0 步）。

## 批 6：辅助设施（约 -500）

1. 删零引用脚本：`tests-audit/audit_bot_tools_alignment.py`、
   `tests-audit/regen_tools_baseline.py`、`scripts/triage-baseline-audit.py`、
   `scripts/ci-guard-tiny-http-vendor.sh`。
2. `git rm scripts/batch-verify.py scripts/install-git-hook.sh`（下线占位退役，
   Mimosa hook 拦 Bash，需手动执行）。
3. package.json knip `ignoreDependencies` 清理；16 个 test-only export 逐个评估：
   去 export 改行为级断言，确有取值价值者留并注释一句原因。

## 批 7：收尾验收

1. `bash scripts/test-all.sh` 全绿；clippy + machete + knip 零报告。
2. grep 验收归零：`─{3,}`/`═{3,}` 横幅、工单号模式、「拍板」——src/ 与 src-tauri/src/ 计数 0。
3. 文档同步：`docs/rust-bot-architecture.md` 模块树删 eval 子模块等条目；
   `docs/EVOLUTION-LAYERING-BATCH-A.md` 与策略层现状对齐。
4. 本文件移入 `docs/archive/`。

## 风险与回滚

- **批 1 最大风险**：`src-tauri/tests/` 集成测试引用被删模块 → 每个删除物先
  `grep -rn` 全仓（含 tests/、bin/），对应测试同 commit 删除。
- **re-export 链**：`policy.rs` / `candidate/mod.rs` / `sandbox/mod.rs` 的转发
  最后删，先删内部实现。
- **注释批唯一风险 = 误删语义** → 逐行 diff 人眼过（基线纪律：门禁只懂规则）。
- 每批独立 commit，回滚粒度 = 单 commit；批间无交叉依赖，可中止在任意批。

## 预期净收益

**-4,500 ~ -5,600 行**；依赖 0 增 0 减；门禁保持批 0 改造后的精简形态不变。
批 1.5（决策板证据增强 +100~150 行）与批 4.5（memoryTuning 设置 UI
+120~180 行）为改造性新增，单列不计入净删。
