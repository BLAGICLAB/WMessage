# OCR 修复批次 · follow-up 索引（一行一条，可 grep）

**只分类，不修**（D1 约束）。字段：`ID | 批次 | 严重度 | 根因一句话 | 触发条件 | 处置`。
grep 例：`grep 'C2b1' docs/OCR-FOLLOWUPS-INDEX.md`

**批次来源（防「无主」）**：

- `C1b` / `C2a` / `C2b-1` / `C2b-2` = OCR 修复批次，对应 commit `3266382` / `39675aa` / `8b0dfdb` / `95a25e9`
- `流程` = `docs/OCR-FIX-PLAN-2026-09-21.md` 附录的流程事故登记
- **`Phase6` = 本 plan 自己的收尾阶段** —— `docs/OCR-FIX-PLAN-2026-09-21.md` §「Phase 6 — 架构一致性门禁恢复」（主线 Phase 1–5 完成后执行）→ **非孤儿批次**
- `顺手修` / `独立小批` / `won't fix` = 本索引文末的分层处置

| ID | 批次 | 严重度 | 根因一句话 | 触发条件 | 处置 |
|---|---|---|---|---|---|
| C1b-1 | C1b | high | grep_files/list_files 走 walk+ReadDir，无 inode re-check，TOCTOU 暴露面不同 | 迭代期间文件被换 | Phase6 |
| C1b-2 | C1b | high | resolve_with_perm 只保护 open 窗口；后续 read_capped_file 又开一次 fd，不在保护内 | 读取与校验间被换 | Phase6 |
| C1b-3 | C1b | medium | inode re-check 不覆盖 symlink target 被替换为另一文件 | canonical 不变但 target 换 | Phase6 |
| C1b-4 | C1b | medium | Windows 端 capture_pre_ino 直接 None，整体 inode re-check 不执行 | Windows 上读文件 | Phase6(需产品决策) |
| C1b-5 | C1b | medium | capture_pre_ino None 时 fail-open vs fail-closed 未定 | metadata 失败 | Phase6(需产品决策) |
| C1b-6 | C1b | medium | spawn_blocking_map 契约把 io::Error 转 String，丢失 ErrorKind | 需要按 ErrorKind 分流时 | Phase6 |
| C1b-7 | C1b | low | grep/list 的 walk 包 spawn_blocking 后 unwrap_or_default() 吞错误 | walk/read 未来返 Err | Phase6 |
| C1b-8 | C1b | low | OCR CLI 无进度 flush / 产物心跳，中途被杀则产物全丢 | review 耗时 > 调用侧上限 | 独立小批(与 DIAG 决策配套) |
| PROC-1 | 流程 | medium | OCR review「后台产物不可见」流程事故已登记但未修 | review 后台跑 | 独立小批(= D1 Item A 决策落地) |
| PROC-2 | 流程 | medium | 跨 IPC 边界调用链调研不足（漏 getCurrentWindow 直调类） | 做跨边界搜证时 | 独立小批(流程) |
| C2a-Q1 | C2a | critical(功能) | main+widget 共用 capability；拆分尝试已回滚 | widget 打开文件 | **won't fix**（已决，见 plan 附录） |
| C2a-1 | C2a | medium | opener:default 含 reveal-item-in-dir 且无 path scope | 若启用该命令 | Phase6 |
| C2a-2 | C2a | low | SkillsPanel 前端 openPath 未迁 Rust，仍是前端 opener path 依赖 | 便携模式边缘/要摘 $APPDATA 时 | Phase6 |
| C2a-3 | C2a | low | 便携模式 app_data_dir() 失败分支无自动化覆盖 | app_data_dir 失败 | Phase6 |
| C2a-4 | C2a | low | GUI 点击级验证未做（屏幕锁定） | 解锁后 | 独立小批(补验) |
| C2b1-L1 | C2b-1 | low | canonical_string 用 to_string_lossy 丢非 UTF-8 字节 | 路径含非 UTF-8 | 顺手修 |
| C2b1-L2 | C2b-1 | low | 部分测试仍传 raw Some(&gen) vs gen_canon | 改该测试时 | 顺手修 |
| C2b1-r3a | C2b-1 | low | Path::exists() 在 check 之前，删除竞态时错误信息误导 | 路径检查后被删 | 顺手修 |
| C2b1-r3b | C2b-1 | low | canonical_if_openable 每次重 canonicalize gen_dir（open 一次 IPC 两次） | 每次 open/delete | 顺手修 |
| C2b2-1 | C2b-2 | medium | 消费侧 link_kind_contributes_path 字面集与 db::workspace::ALLOWED_LINK_KINDS 重复，注释称「同集」但无强制 | 白名单集合变更时 | 顺手修 |
| C2b2-2 | C2b-2 | low | errorHandler hint「删除后重新添加」与 recoverable=true 语义不符 | 前端展示该错误 | 顺手修 |
| C2b2-3 | C2b-2 | medium | db/mod.rs 测试 mk 闭包硬编码 link id "l1" / target_uri "/tmp/x.app"，数据语义不一致 | 改该测试时 | 顺手修 |

## 优先级分层（0 条本轮开修）

- **A. 下次碰同文件顺手修（7）**：`C2b1-L1` `C2b1-L2` `C2b1-r3a` `C2b1-r3b`（均在 `bot_skills/files.rs`）、
  `C2b2-1` `C2b2-2` `C2b2-3`（`files.rs` / `errorHandler.ts` / `db/mod.rs`）。
- **B. 独立小批（4）**：`PROC-1`（= D1 Item A 决策落地：调用侧 timeout + 判据）、`C1b-8`（OCR 进度 flush）、
  `PROC-2`（跨 IPC 流程）、`C2a-4`（GUI 补验）。
- **C. Phase 6 主线（10）**：`C1b-1..C1b-7`、`C2a-1`、`C2a-2`、`C2a-3`（架构/TOCTOU 统一策略，既定阶段）。
- **D. won't fix + 理由（1）**：`C2a-Q1` —— widget capability 拆分已尝试并**回滚**（未采纳，不产生 commit），
  真正的降面路径另列于 plan（Phase 6 / 产品决策）。
