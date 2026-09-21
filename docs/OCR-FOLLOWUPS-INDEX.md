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
| C2a-Q1 | C2a | critical(功能) | main+widget 共用 capability；拆分尝试已回滚 | widget 打开文件 | **won't fix**（已决，见 plan 附录） |
| C2a-1 | C2a | medium | opener:default 含 reveal-item-in-dir 且无 path scope | 若启用该命令 | Phase 6-T |
| C2a-2 | C2a | low | SkillsPanel 前端 openPath 未迁 Rust，仍是前端 opener path 依赖 | 便携模式边缘/要摘 $APPDATA 时 | Phase 6-T |
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
