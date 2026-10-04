# OCR 修复批次 · follow-up 索引（一行一条，可 grep）

**本索引为「修复批衍生债」，非基线 finding**（基线 1145 条见 `docs/OCR-FIX-PLAN-2026-09-21.md`，两套账本分离）。**只分类，不修**。字段：`ID | 批次 | 严重度 | 根因一句话 | 触发条件 | 处置`。
grep 例：`grep 'C2b1' docs/OCR-FOLLOWUPS-INDEX.md`

**批次来源（防「无主」）**：

- `C1b` / `C2a` / `C2b-1` / `C2b-2` = OCR 修复批次，对应 commit `3266382` / `39675aa` / `8b0dfdb` / `95a25e9`
- `C2c-verify` = C2c Step 2 验证跑（`ocr review --commit 8b0dfdb`，产物 `~/.openclaw/cache/ocr-C2b-1-evidence/ocr-C2c-verify.json`）发现的项
- `流程` = `docs/OCR-FIX-PLAN-2026-09-21.md` 附录的流程事故登记
- **`Phase 6-T` = plan §Phase 6 的**前置子集**（TOCTOU/权限统一策略）—— 2026-09-21 裁决，先于 Phase 6「门禁恢复」（`docs/OCR-FIX-PLAN-2026-09-21.md` §Phase 6）
- `顺手修` / `独立小批` / `won't fix` = 本索引文末的分层处置

| ID | 批次 | 严重度 | 根因一句话 | 触发条件 | 处置 |
|---|---|---|---|---|---|
| C1b-1 | C1b | high | grep_files/list_files 走 walk+ReadDir，无 inode re-check，TOCTOU 暴露面不同 | 迭代期间文件被换 | Phase 6-T |
| C1b-2 | C1b | high | resolve_with_perm 只保护 open 窗口；后续 read_capped_file 又开一次 fd，不在保护内 | 读取与校验间被换 | Phase 6-T |
| C1b-3 | C1b | medium | inode re-check 不覆盖 symlink target 被替换为另一文件 | canonical 不变但 target 换 | Phase 6-T |
| C1b-4 | C1b | medium | Windows 端 capture_pre_ino 直接 None，整体 inode re-check 不执行 | Windows 上读文件 | Phase 6-T(需产品决策) |
| C1b-5 | C1b | medium | capture_pre_ino None 时 fail-open vs fail-closed 未定 | metadata 失败 | Phase 6-T(需产品决策) |
| C1b-6 | C1b | medium | spawn_blocking_map 契约把 io::Error 转 String，丢失 ErrorKind | 需要按 ErrorKind 分流时 | Phase 6-T |
| C1b-7 | C1b | low | grep/list 的 walk 包 spawn_blocking 后 unwrap_or_default() 吞错误 | walk/read 未来返 Err | Phase 6-T |
| C1b-8 | C1b | low | OCR CLI 无进度 flush / 产物心跳，中途被杀则产物全丢 | review 耗时 > 调用侧上限 | 独立小批(与 DIAG 决策配套) |
| PROC-1 | 流程 | medium | OCR review「后台产物不可见」流程事故 | review 后台跑 | ✅ 已落地（C2c Step 2，commit `4a3e64a`）|
| PROC-2 | 流程 | medium | 跨 IPC 边界调用链调研不足（漏 getCurrentWindow 直调类） | 做跨边界搜证时 | 独立小批(流程) |
| C2a-Q1 | C2a | critical(功能) | main+widget 共用 capability；拆分尝试已回滚 | widget 打开文件 | **永久 wontfix**（2026-09-26 拍板 A：widget 保留拖拽/缩放为产品特性；翻案唯一路径=widget 只读化重构，届时 capability 拆分同批） |
| C2a-1 | C2a | medium | opener:default 含 reveal-item-in-dir 且无 path scope | 若启用该命令 | Phase 6-T |
| C2a-2 | C2a | low | SkillsPanel 前端 openPath 未迁 Rust，仍是前端 opener path 依赖 | 便携模式边缘/要摘 $APPDATA 时 | Phase 6-T defer 维持（2026-09-26 拍板 B：便携模式真实报障触发） |
| C2a-3 | C2a | low | 便携模式 app_data_dir() 失败分支无自动化覆盖 | app_data_dir 失败 | Phase 6-T |
| C2a-4 | C2a | low | GUI 点击级验证未做（屏幕锁定） | 解锁后 | 独立小批(补验) |
| C2b1-L1 | C2b-1 | low | canonical_string 用 to_string_lossy 丢非 UTF-8 字节 | 路径含非 UTF-8 | 顺手修（seen@C2c-verify #4）|
| C2b1-L2 | C2b-1 | low | 部分测试仍传 raw Some(&gen) vs gen_canon | 改该测试时 | 顺手修 |
| C2b1-r3a | C2b-1 | low | Path::exists() 在 check 之前，删除竞态时错误信息误导 | 路径检查后被删 | 顺手修 |
| C2b1-r3b | C2b-1 | low | canonical_if_openable 每次重 canonicalize gen_dir（open 一次 IPC 两次） | 每次 open/delete | 顺手修（seen@C2c-verify #3）|
| C2b2-1 | C2b-2 | medium | 消费侧 link_kind_contributes_path 字面集与 db::workspace::ALLOWED_LINK_KINDS 重复，注释称「同集」但无强制 | 白名单集合变更时 | 顺手修 |
| C2b2-2 | C2b-2 | low | errorHandler hint「删除后重新添加」与 recoverable=true 语义不符 | 前端展示该错误 | 顺手修 |
| C2b2-3 | C2b-2 | medium | db/mod.rs 测试 mk 闭包硬编码 link id "l1" / target_uri "/tmp/x.app"，数据语义不一致 | 改该测试时 | 顺手修 |
| C2c-v1 | C2c-verify | medium | recheck→副作用之间仍有内核层 TOCTOU 窗口（canonical 分量可被 swap） | 攻击者 swap 目录分量/重建父为 symlink | Phase 6-T（opener API 无 fd/O_NOFOLLOW）|
| C2c-v2 | C2c-verify | medium | spawn_blocking_map 失败 → 绑定集塌成空 → 之后所有 open/delete 被拒直到重启 | spawn_blocking 闭包 panic | 顺手修 |
| C2c-v3 | C2c-verify | low | recheck_canonical 用 InvalidArgument 表示安全事件，语义混淆 | 监控/日志需区分 bad-input vs race | 顺手修 |
| C2c-v4 | C2c-verify | medium | M1 不完整：`open_db`/`load_workspace` 仍同步跑在 async runtime | 多 IPC 并发 | 顺手修 |
| C2c-v5 | C2c-verify | low | `db_load`/`open_db` 失败分支丢 `{e}`，审计无法区分 DB 锁/迁移/IO | DB 错误排查时 | 顺手修 |
| PROC-3 | 流程 | low | docs-only 提交不被 OCR 审（path/extension 过滤）= 预期行为，非事故 | docs-only commit | 已固化 SOP（D1 诊断文档 + 本索引）|
| HOOK-1 | 流程 | medium | pre-commit 的 `cargo fmt --check` 是**全仓**检查 × 既有 57 文件漂移 → 拦任何提交（本批用 (a) 清漂移解除紧急面；机制改动归本项）| 提交时 | 独立评估 |
| GATE-1 | 流程 | low | `test-fast.sh` 含 fmt 检查、`test-all.sh` 不含 → **push 侧无 fmt 门禁**（`--no-verify` 之外的第二条绕过路径）| push 时 | 独立评估（与 HOOK-1 一并）|
| C2d-v1 | C2d-r1 | medium | `gen_canon` 塌 None 时无 audit（C2c-v5 只盖了 DB/workspace 两分支，第三条失败路径仍隐形）| gen_dir/canonical 失败 | 顺手修 |
| C2d-v2 | C2d-r1 | medium | `link_kind_contributes_path` 的 `kind != "url"` 隐含依赖 "url" 在 ALLOWED 内，脆耦合 | 写入侧白名单收缩时 | 顺手修 |
| C2d-v3 | C2d-r1 | medium | `gen_dir_canon`「必预 canonical」契约仅文档未强制（raw `/var` 传入静默 fall through）| 新调用方传 raw gen | 顺手修 |
| C2d-v4 | C2d-r1 | low | delete_bound_file 为 gen_dir 付了 create_dir_all+canonicalize 却 `_gen` 丢弃 | 每次 delete IPC | **won't fix**（微优化，无错误路径）|
| C2d-v5 | C2d-r1 | low | `let mut raw = raw;` 重绑定遮蔽多余（FnOnce 直接消耗即可）| — | **won't fix**（纯风格）|
| C2d-v6 | C2d-r1 | medium | per-path `canonical_string` 失败静默 drop（无 audit）| 绑定文件被删/不可达 | 顺手修 |
| C2d-v7 | C2d-r1 | medium | 注释宣称「合并进一个 blocking 任务」但 `db_load` 自带 spawn_blocking（over-claim）| — | 顺手修（注释准确性）|
| C4-v1 | C4 | high | `STORAGE_KEY = "***"`（src/storage.ts:2）—— 与 C4-3 同根因（占位未替换），但**影响真实用户数据迁移**（老版本升级时 `App.tsx:192` 读 "***" 取旧数据，多应用并存必撞）。严重度不低于 C4-3：本批**不修**，因不知老版本用什么 key 即改 = 仍读错位置（信息不足，非授权问题）。| 用户升级路径 | 独立评估（需先考据老 key）|
| C4-v2 | C4 | — | `PANEL_H_MIN=800 > PANEL_H default=560` 逻辑矛盾真实存在，但具体 MIN 值是产品决策（widget 内容最小可用高度需设计定），三问无代码依据：本批**wontfix-pending-product-decision**，不动代码、不写 TODO（D2: 注释不算修复）。| 用户手动 resize | ✅ 已修（DEC-1 9ff9d1a，2026-09-26 拍板 A 底部放开：MIN 800→400，区间 400–900，默认 560 不动）|
| PROC-4 | 流程 | — | OCR 假阳性累积统计：C3 一批 1 条（H1），C4 一批 2 条（C4-2 / C4-5）—— 假阳性率需要被跟踪。C4-2 证据：nextest `--help` 列 `[possible values: integer or "num-cpus"]`；C4-5 证据：format.ts:93/124 实际内容均无 finding 描述的 bug。累积到 3-4 条再评估是否调 OCR prompt / config / 过滤规则。| OCR 轮次 | 独立评估（阈值触发）|
| C4-r1-H1 | C4-r1 | critical→已修 | mutating 原声明在 App() 函数体内 → 每 render 重建 → 串行化跨 render 失效（OCR C4-r1 实证）。r2 验证：抬到模块作用域后链跨 render 衔接（同时改：mutate 闭包每 render 新建但读到同一模块对象），fix = 1 行 const 移位。| — | ✅ 已修（r2 验证）|
| C4-r1-M1 | C4-r1 | medium | observe-run --synthetic --proposals /path/to/x.jsonl 时 CLI 给的路径被 silent 覆盖（synthetic 隔离的设计），用户无任何提示 | 用户传路径被无视 | 顺手修（可选：打印 stderr 警告）|
| C4-r1-L1 | C4-r1 | low | r1 修复后 mutating 模块级 singleton 跨测试残留 → 测试间的 mutate 状态可能泄漏（机制与顺序测试的潜在缺陷边界）| 测试隔离 | 顺手修（测试 setup/teardown 重置 chain）|
| C4-r1-L2 | C4-r1 | low | SIZE_KEY 新值无老 key 读回退（loadSize 无 fallback）—— 但全仓 grep 确认 *** 无任何残留引用，老数据实际不存在 | 老用户 | 顺手修（双向兼容：先读新 key、失败再读老 key）|
| C3-r1-H1 | C3-r1 | high→**假阳性** | DbWriteGuard「隐式 Send」断言为假（std MutexGuard 本就 !Send，已静态断言实证）| — | **不改**（OCR false positive）|
| C3-r1-H2 | C3-r1 | high | `evolution_keep_shadow` guard 持满 async 体（今天无 await、future 仍 Send；加 await 即编译错）| 未来加 await | Phase 6-T（注：无当前缺陷，失败模式=编译期拦截）|
| C3-r1-M1 | C3-r1 | medium | DbWriteGuard 缺 `#[must_use]`（`lock_db_write();` 无绑定会立即析构）| 漏绑定 | 顺手修 |
| C3-r1-M2 | C3-r1 | medium | `rollback_count` 无直接单测（它是 C3-2 的整数真源）| — | 顺手修 |
| C3-r1-M3 | C3-r1 | medium | EVOLUTION_STORE_LOCK 临界区含 DB I/O（delete / delete_evolution_mem_item）| 高并发 | 顺手修 |
| C3-r1-M4 | C3-r1 | medium | rollback 两阶段间 detail 文案与实际数据可能不一致（① 快照 vs ② 重载）| 并发修改 | 顺手修 |
| C3-r1-M5 | C3-r1 | medium | 测试持全局 DB_WRITE_LOCK → 并行度下降 | cargo test | 顺手修 |
| C3-r1-L1 | C3-r1 | low | DbWriteGuard Drop 顺序：先清标记后释放锁（微窗口内 holding 报 false）| — | 顺手修 |
| C3-r1-L2 | C3-r1 | low | `_g` 字段名过简（公有 struct 唯一字段）| — | 顺手修 |
| C3-r1-L3 | C3-r1 | low | `rollback_count` 被调两次（各建一次 HashMap）| — | 顺手修 |
| C3-r1-L4 | C3-r1 | low | live_keys 用 `HashMap<&str,()>` 而非 `HashSet<&str>`（仅成员测试）| — | 顺手修 |
| C3-r1-L5 | C3-r1 | low | `rollback_count` 为 pub 但仅内部使用 | — | 顺手修 |
| C3-r1-L6 | C3-r1 | low | migrations.rs 注释小不一致（OCR 未指明细节，需复核）| — | 顺手修 |
| C3-r1-L7 | C3-r1 | low | metrics.rs:94 同类建议（HashSet）| — | 顺手修（与 L4 合并）|
| C3-r1-L8 | C3-r1 | low | **`keep_shadow` 的 `summary:` 标签实际传 `record.suggestion_text`**（文案与实现不符）| — | **D2 射程，单独看** |
| DESIGN-1 | C3 | — | `rolled_back_at` 的窗口语义定义（观察窗下 created_at_ms vs rolled_back_at 如何选）| stop/metrics 口径 | 独立评估（不本批改）|


## 优先级分层（0 条本轮开修）

- **A. 下次碰同文件顺手修（16）**：
  `C2b1-L1`⁵ `C2b1-L2` `C2b1-r3a` `C2b1-r3b`⁵、`C2b2-1` `C2b2-2` `C2b2-3`、
  `C2c-v2` `C2c-v3` `C2c-v4` `C2c-v5`、**`C2d-v1` `C2d-v2` `C2d-v3` `C2d-v6` `C2d-v7`**（均在 `bot_skills/files.rs`）。
- **B. 独立小批（3）**：`C1b-8`（OCR 进度 flush / 产物心跳）、`PROC-2`（跨 IPC 流程）、`C2a-4`（GUI 补验）。
  （`PROC-1` 已落地 → 移出本层。）
- **C. Phase 6-T（11）**：`C1b-1..C1b-7`、`C2a-1`、`C2a-2`、`C2a-3`、**`C2c-v1`**（架构/TOCTOU 统一策略）。
- **D. won't fix + 理由（3）**：`C2a-Q1` —— widget capability 拆分已尝试并**回滚**（未采纳，不产生 commit）；
  `C2d-v4`（delete 侧 gen 微优化，无错误路径）；`C2d-v5`（`let mut raw = raw` 纯风格）。
- **已闭环**：`PROC-1`（C2c Step 2 落地）、`PROC-3`（SOP 固化）。

⁵ = seen@C2c-verify（验证跑复现，仍未修）。

**C3-r1（C3 批 OCR 轮，16 条）分层**：`C3-r1-H1` 假阳性（不改）｜`C3-r1-H2` → Phase 6-T｜`C3-r1-M1..M5` / `C3-r1-L1..L7` → 顺手修｜
`C3-r1-L8` → **D2 射程，单独看**（文案与实现不符）｜`DESIGN-1`（rolled_back_at 窗口语义）→ 独立评估。

## W1-CANVAS（工作流画布骨架，2026-10-04 OCR r1 修复批衍生债）

> 修复批 commit 见 git log `W1-CANVAS`；r1 全量报告 `docs/OCR-CODE-REVIEW-2026-10-04-w1.json`。
> 3 critical + 2 high + 8 medium/low 已随批修复，以下为显式缓期项。

| ID | 批次 | 严重度 | 根因一句话 | 触发条件 | 处置 |
|---|---|---|---|---|---|
| W1F-1 | W1-CANVAS-r1 | low | `un.then((f)=>f())` 清理模式吞 unsubscribe 拒绝 | listen promise reject | wontfix（与全仓既有 listen 用法一致；统一收口属独立批） |
| W1F-2 | W1-CANVAS-r1 | medium | open_db 迁移循环 PRAGMA+ALTER 样板已三份 | 加新列时 | 独立小批（迁移辅助函数化，db family，不混入 workflow-canvas） |
| W1F-3 | W1-CANVAS-r1 | medium | workflow_rename 改名不广播 | 其他窗口正在看工作流列表 | wontfix-by-design（改名经「保存」路径持久化；画布打开时 workflow_load 取现值；跨窗列表刷新随 W3 执行态一并处理） |
| W1F-4 | W1-CANVAS-r1 | medium | reloadTasks 并发调用无串行化 | 快速连续保存 | Phase2（两次读均为已提交快照，后完成者胜，无丢数据面；与 mutating 链合并属编排批次） |
| W1F-5 | W1-CANVAS-r1 | low | createBlank/openWorkflow 丢弃未保存编辑无确认 | dirty 时切换工作流 | W2（需确认弹窗 promise 化，随拆解 UX 批处理） |
| W1F-6 | W1-CANVAS-r1 | low | TaskNode memo 因 data 每次重建而失效 | 每渲染 | wontfix（画布 ≤30 节点， reconciliation 开销可忽略） |
| W1F-7 | W1-CANVAS-r1 | low | types.ts CanvasPos/WorkflowSaveBinding 未导出 | 外部引用类型时 | wontfix（knip 禁止无消费导出；需要时随消费方同批导出） |
| W1F-8 | W1-CANVAS-r1 | low | edge id 以 "->" 拼接的格式耦合 | localId 含 "->" 时 | wontfix（localId 恒为 uuid/真实任务 id，不变量已注释在生成/解析两处） |
| W1F-9 | W1-CANVAS-r1 | low | addNode 以窗口中心而非画布中心落点 | 画布被遮挡/滚动时 | W2（画布 DOM 中心换算，随 UX 小批） |
| W1F-10 | W1-CANVAS-r1 | low | App reloadTasks 每渲染新引用 | 每渲染 | wontfix（仅在保存/删除后调用，无热路径） |

## W2-DECOMPOSE（AI 拆解，2026-10-04 OCR r1 修复批衍生债）

> 修复批 commit 见 git log `W2-DECOMPOSE-r1`；r1 全量报告（本地）
> `docs/OCR-CODE-REVIEW-2026-10-04-w2.json`（26 条，2C+9M+13L，去重后约 20 独立项）。
> 2 critical（拆解竞态）+ 8 medium + 4 low 已随批修复，以下为显式缓期项。

| ID | 批次 | 严重度 | 根因一句话 | 触发条件 | 处置 |
|---|---|---|---|---|---|
| W2F-1 | W2-DECOMPOSE-r1 | medium | 拆解重试循环无整体 deadline（2×60s 上限） | 模型慢时 | wontfix-by-design（前端可取消=序号守卫，UI 不阻塞；服务端硬中断需 abort channel，随 W3 执行取消基建一并做） |
| W2F-2 | W2-DECOMPOSE-r1 | low | workflow_decompose 命令级（重试/审计路径）无测试 | 改命令体时 | W3（需 summarize_messages mock 挂具，随执行引擎测试基建同批） |
| W2F-3 | W2-DECOMPOSE-r1 | low | EmptyHero JSX 双层条件分支密度高 | 改空态 UI 时 | wontfix（纯结构重构，无行为面；随下次空态改版顺手拆） |
| W2F-4 | W2-DECOMPOSE-r1 | low | 指引段仅 maxLength 无字数/token 反馈 | 粘贴长文本时 | wontfix（2000 上限已拦极端；token 估算属设置页全局议题） |
| W2F-5 | W2-DECOMPOSE-r1 | low | guidance state 无跨组件订阅 | 未来出现第二个编辑入口时 | wontfix（当前全应用唯一编辑点在设置页卡片，读发生在 invoke 时） |

## W3-RUNNER（执行引擎，2026-10-04 OCR r1 修复批衍生债）

> 修复批 commit 见 git log `W3-RUNNER-r1`；r1 全量报告（本地）
> `docs/OCR-CODE-REVIEW-2026-10-04-w3.json`（11 条，2H+5M+4L，全部随批修复，无缓期项；
> activeId 仅喂 progress 三元的 deps 冗余随重构成自然消除）。

## W4-TEMPLATE（模板导入导出，2026-10-04 OCR r1 修复批衍生债）

> 修复批 commit 见 git log `W4-TEMPLATE-r1`；r1 全量报告（本地）
> `docs/OCR-CODE-REVIEW-2026-10-04-w4.json`（17 条，1H+6M+10L）。
> 1H+4M+4L 随批修复，以下为显式缓期项。

| ID | 批次 | 严重度 | 根因一句话 | 触发条件 | 处置 |
|---|---|---|---|---|---|
| W4F-1 | W4-TEMPLATE-r1 | low | 成功提示用 alert()（与 App.tsx 导入导出成功提示同款先例） | 每次导入导出 | wontfix-by-now（统一 toast 属全应用反馈管线重构，独立批） |
| W4F-2 | W4-TEMPLATE-r1 | low | 导出默认文件名与 createBlank 命名不同源 | 用户清空名字后导出 | wontfix（对话框自身有同名冲突处理，无数据风险） |
| W4F-3 | W4-TEMPLATE-r1 | low | 导出按钮不反映 dirty | 未保存时导出 | wontfix-by-design（title 已声明"导出已保存版本"，属功能语义） |
| W4F-4 | W4-TEMPLATE-r1 | low | description 死字段（导出恒 None） | — | wontfix-by-design（格式 v1 完整性保留；将来加列时启用，serde default 无害） |
| W4F-5 | W4-TEMPLATE-r1 | medium | 空 goal/空 dependsOn 由下游校验链报错（非文件层） | 导入畸形文件 | wontfix（下游报错已带节点序号归属，文件层重复校验无增益） |
| W4F-6 | W4-TEMPLATE-r1 | low | rfNodes deps 中 activeId 仅喂 progress 三元 | 每渲染 | wontfix（≤30 节点冗余重算无观察开销） |


## W1–W4 全量对照（2026-10-04 收口，4f371cd..HEAD 合并 diff 复审）

> 报告（本地）`docs/OCR-CODE-REVIEW-2026-10-04-w1-w4-full.json`：73 条 = 1C+4H+37M+31L。
> **1C+4H 全部随收口批修复**（TOCTOU 防重入原子化 / 断点续跑死锁 / workflows 行入事务 /
> 停止按钮跨工作流残留 / createBlank 复位回归），含死锁回归测试。
> 68 条 medium/low 按主题三分落账如下（"已账"= 既有条目覆盖；"顺手修候选"= 独立小批；
> "观察项"= 无行为面，收益不抵 churn）。

| ID | 级 | 文件 | 要点 | 处置 |
|---|---|---|---|---|
| FULL-1 | low | bot_chat.rs | The `as_str()` mapping (lines 1298-1305) is duplicated verbatim at lines 1570-1575 to produce the same `"manua | | 顺手修候选（小批） |
| FULL-2 | low | WidgetApp.tsx | The shared filter condition `notWorkflow(t) && !t | | 观察（行为正确） |
| FULL-3 | medium | App.tsx | reloadTasks and the inline onTasksReload arrow are recreated on every render | | W1F-10 已账 |
| FULL-4 | medium | App.tsx | The inline arrow for onTasksReload is recreated on every render | | W1F-10 已账 |
| FULL-5 | medium | GoalNode.tsx | The invisible `Handle type="target"` contradicts the docstring's "不参与连线" claim | | 观察（隐藏手柄为实现需要） |
| FULL-6 | low | GoalNode.tsx | `React | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-7 | medium | tasks.rs | The `dependsOn` branch only rejects empty-string elements | | W4F-5 已账（下游报错带归属） |
| FULL-8 | medium | tasks.rs | The new validation branches (`origin`, `workflowId`, `dependsOn`, `canvasPos`) and the new `load_tasks_by_work | | 观察（enum 化属契约小批） |
| FULL-9 | low | tasks.rs | `Task::origin` is exposed as `Option<String>` despite this diff also introducing `TASK_ORIGIN_USER` / `TASK_OR | | 观察（enum 化属契约小批） |
| FULL-10 | medium | tasks.rs | `canvas_x` and `canvas_y` are read and written as `Option<f64>` with no `is_finite()` guard | | 顺手修候选（写口 is_finite 拦截） |
| FULL-11 | low | tasks.rs | `workflow_id` lacks referential integrity | | 观察（级联删除已保证一致性） |
| FULL-12 | medium | SettingsPage.tsx | Silent failure when persistence fails: `setDecomposeGuidance` returns `false` when `localStorage` is unavailab | | W4F/设置卡 已账（返回 bool 已修 UI 侧） |
| FULL-13 | medium | workflow.rs | Production panic risk: ` | | 顺手修候选（expect→可读错误） |
| FULL-14 | medium | workflow.rs | Inconsistent time source inside the same file: `workflow_save` reads `let now = chrono::Utc::now() | | 观察（时钟源差异无语义影响） |
| FULL-15 | medium | workflow.rs | NaN ordering collapses to `Equal` and silently corrupts the topological export order | | 顺手修候选（写口 is_finite 拦截） |
| FULL-16 | low | workflow.rs | `depends_on | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-17 | low | workflow.rs | Unused tuple destructure: both slots (`p | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-18 | low | workflow.rs | `workflow_rename` is the only write command in this file that does not broadcast on the existing `tasks-change | | W3 批已账（W1F-3） |
| FULL-19 | medium | TaskNode.tsx | `memo` on TaskNode is effectively decorative given how the parent assembles `data` | | W1F-6 已账（memo 失效 ≤30 节点） |
| FULL-20 | low | TaskNode.tsx | `nodeBorder` conflates two distinct cases under "green ring" | | 观察（两绿态语义合并已注释） |
| FULL-21 | medium | workflow_runner.rs | **Audit and notification understate `done` and inflate `skipped` on breakpoint-resume runs | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-22 | medium | workflow_runner.rs | **O(N²) DB load per node completion — single-row API already exists | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-23 | medium | workflow_runner.rs | **`mark_skipped` does the same O(N²) full-table load — use the single-row lookup here too | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-24 | medium | workflow_runner.rs | **Controller hangs indefinitely if any spawned node task panics before `tx | | 顺手修候选（expect→可读错误） |
| FULL-25 | low | workflow_runner.rs | **`mark_skipped` silently swallows all errors — no audit/log on failure | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-26 | low | workflow_runner.rs | **Notification send failure logged via `eprintln!` instead of the structured audit channel | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-27 | medium | workflow_decompose.rs | Macro hygiene: `$err` is interpolated twice (in `$err | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-28 | medium | workflow_decompose.rs | Asymmetric validation: `title` is trimmed and rejected when empty, but `note` is only checked for the upper ch | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-29 | medium | workflow_decompose.rs | Duplicate indices inside a single `depends_on` (e | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-30 | low | workflow_decompose.rs | User-facing `reason` strings embed Rust identifiers and the raw field name `dependsOn` (`第 {} 个任务的 dependsOn 含 | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-31 | low | workflow_decompose.rs | `goal_trimmed` is interpolated raw into `user_content` via `format!("总目标：{goal_trimmed}")` | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-32 | medium | SettingsPage.tsx | Stale-ref race on unmount: `guidanceRef | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-33 | low | SettingsPage.tsx | listen() cleanup has two latent issues: (1) if the promise rejects (Tauri IPC failure) the rejection is silent | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-34 | medium | GoalNode.tsx | Accessibility: `outline-none` removes the default focus indicator without supplying a `focus-visible` alternat | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-35 | medium | GoalNode.tsx | Accessibility: same focus-visible issue as the input — `outline-none` strips the default ring with no replacem | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-36 | medium | GoalNode.tsx | Permanently-disabled button doesn't read as disabled on a neumorphic surface: it still has the raised `nm-outs | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-37 | medium | TaskNode.tsx | The node container has no `relative` positioning class, but the delete button uses `absolute -right-2 -top-2` | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-38 | medium | workflow_runner.rs | `build_dag` is O(N×D) due to a nested linear scan: for each dep `d`, it calls `tasks | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-39 | medium | workflow_runner.rs | Error context is dropped when a node task fails | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-40 | low | workflow_runner.rs | `running | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-41 | low | workflow.rs | `by_id` is built with `iter() | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-42 | low | workflow.rs | When a node is kept, `j` was already known at the moment `real_id[i] = existing[j] | | 观察（时钟源差异无语义影响） |
| FULL-43 | low | workflow.rs | When a node is kept, `j` was already known at the moment `real_id[i] = existing[j] | | 观察（时钟源差异无语义影响） |
| FULL-44 | low | workflow.rs | `by_id` is built with `iter() | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-45 | low | types.ts | Workflow | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-46 | low | workflow_decompose.rs | `strip_fences` doesn't handle the case where the language tag is adjacent to the opening fence with no newline | | 观察（隐藏手柄为实现需要） |
| FULL-47 | medium | workflow_decompose.rs | `audit::write_event` is a synchronous function that performs filesystem I/O (resolves `data_dir`, rotates the  | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-48 | medium | workflow_decompose.rs | The retry loop has no backoff or timeout | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-49 | medium | workflow_decompose.rs | Contract and validation disagree on the upper bound | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-50 | low | workflow_decompose.rs | The success audit event omits an `outcome` field while the failure path emits `("outcome", "failed")` | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-51 | medium | graph.ts | `draftFromDecompose` blindly indexes `nodes[d]` from LLM-returned indices with no defensive check | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-52 | medium | graph.ts | `draftFromTasks` keeps `dependsOn` entries whose ids aren't in the reconstructed `nodes` array | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-53 | low | graph.ts | `if (from === to) return true;` is redundant: BFS initializes the stack with `to` and the first iteration chec | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-54 | low | graph.ts | The comment says "缺坐标的行补 dagre 布局" (rows missing coords get dagre layout), but the implementation re-lays out  | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-55 | low | tasks.rs | Validation gap: the dependsOn branch only rejects empty-string elements | | W4F-5 已账（下游报错带归属） |
| FULL-56 | medium | tasks.rs | `workflow_id` is a plain `TEXT` column without an index, yet `load_tasks_by_workflow` filters by it and `workf | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-57 | medium | tasks.rs | The `TASK_ORIGIN_USER` constant exists for validation but is silently normalized to `None` on storage — so the | | 观察（enum 化属契约小批） |
| FULL-58 | low | graph.ts | `draftFromTasks` uses an inline `import(" | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-59 | low | graph.ts | `wouldCreateCycle` rebuilds the entire downstream adjacency map from scratch on every invocation | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-60 | medium | WorkflowPage.tsx | The single `armedTimerRef` is shared between two independent two-step confirmations | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-61 | medium | WorkflowPage.tsx | TaskNode and GoalNode are both wrapped in `React | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-62 | medium | WorkflowPage.tsx | `cancelDecompose` only bumps `decomposeSeqRef` so the late response is discarded (the comment explicitly says  | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-63 | low | WorkflowPage.tsx | The `useEffect(() => { tasksRef | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-64 | medium | WorkflowPage.tsx | Several fire-and-forget `invoke( | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-65 | low | WorkflowPage.tsx | In `onEdgesChange`, `const [, target] = c | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-66 | low | WorkflowPage.tsx | Three different `alert( | | W4F-1 已账（alert 先例） |
| FULL-67 | low | WorkflowPage.tsx | `useCallback` here returns a function that is immediately invoked on every render (`snapshot()` at line 134) | | 观察项（无行为面/已有注释/收益<30 行） |
| FULL-68 | medium | WorkflowPage.tsx | `stopRun` keeps `running` as `true` after the backend acknowledges stop and only updates it via the 300 ms rec | | 观察（时钟源差异无语义影响） |

## Mimosa 完整安全扫描（2026-10-04，deep，seal sha256:cc5f36…）

> 扫描 ID `scan-2026-10-04T09-58-00.914Z-6286ace6087f`，产物在
> `~/.mimosa/security-scans/project-35c8c4f5947b2bbdf573b390/<scanId>/`。
> 13 条 = 6H+7M，全部与工作流功能无关（工作流 9 个 focus 文件**零发现**）。

| ID | 批次 | 严重度 | 根因一句话 | 触发条件 | 处置 |
|---|---|---|---|---|---|
| SEC-1 | Mimosa-1004 | high×6+medium×6（同一发现重复计数） | eval/config.rs:64 `load(path)` 接收本地 API 请求侧路径读文件，污点链未做白名单归一 | 本地 HTTP API 的 eval 配置加载被恶意路径调用 | 独立小批（eval family：路径白名单/固定到数据目录；既有代码，非本功能引入） |
| SEC-2 | Mimosa-1004 | medium | "MongoDB 动态排序字段注入"——全项目无 MongoDB 依赖（Cargo.toml/package.json/源码零命中） | — | 误报（heuristic 误配），不改代码 |
