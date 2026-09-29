# Comment Hygiene 2026-09-15 — Handoff

## 仓库
- 路径：`/Users/renshi/Projects/wmessage`
- HEAD：`00d494f404e37c129d0275c54c8a39e2f49d1e4b`
- 测试基线：663 passed / 0 failed / 2 ignored（`src-tauri` lib，A + BC1 + BC2 后仍 663 passed）

## 状态
- 当前 Phase：**收工**（A + B + C 全部已应用 + BC2 外部文档引用内联完成）
- 工作树：**27 文件已修改，未 commit**

## 统计

| 轮 | 类别 | 处理 | 文件 |
|----|------|------|------|
| Phase 2 | A 类（机械） | **62 处** | 21 文件 |
| Phase 3 Round 1 | B 类（语义改写） | **4 处** | 2 文件（`paths.rs`、`app_state.rs`） |
| Phase 3 Round 1 | C-1（SAFETY） | **9 处** | 2 文件（`copy_file.rs`、`py/runtime.rs`） |
| Phase 3 Round 1 | C-2（总表阶段号清理） | **8 行** | `app_state.rs:17-25` |
| Phase 3 Round 2 | 外部文档引用内联 | **6 处** | 6 文件（`bot_chat.rs`、`memory/{consolidate,mod,rank,store}.rs`、`audit.rs`） |
| D 类 | 保留 | 50+ | 全部不动 |

### A 类明细（62 处）
- A5 纯复述删除：6 条
  - `migration/types.rs:61`、`bot/tools.rs:808`、`api_handlers/mod.rs:148`、`bot_skills/manage.rs:250`、`profile.rs:608`、`memory/mod.rs:422`
- A2 阶段号 strip：43 条（14 文件）
- A1 日期+阶段 strip：13 条（其中 1 条回滚：见下）

### A 类回滚
- **`bot_skills/parse.rs:375`** — 内联尾注释（`assert_eq!` 代码行带注释），规则"只动注释行"未满足，已回滚。**NEEDS-DECISION**：是否接受内联注释 strip（语义仅去日期），或永久保留。

### B 类明细（4 处，B-1 ~ B-4）
- **B-1 app_state.rs**：`SESSION_ORIGINS` 表行去加粗 + 详情段去加粗/日期括注
- **B-2 app_state.rs**：进程内并行实证段去加粗 + 留档观察
- **B-3 paths.rs**：跨进程共享章节标题简化 + 状态简述
- **B-4 paths.rs**：决策段去加粗 + 去日期 + 简化口径引用

### C-1 SAFETY 明细（9 处）
- macOS NSPasteboard FFI × 3（`platform/copy_file.rs:40/44/65`）
- Win32 Job Object FFI × 4（`py/runtime.rs:67/92/98/105`）
- POSIX libc × 2（`py/runtime.rs:148/431`）

### C-2 总表阶段号清理（8 行，`app_state.rs:17-25`）
- SKILL_RUNS / STOP_REGISTRY / CONFIRMS / CHAT_RUNNING / EXEC_RUNNING / SCHED_RUNNING / PENDING / REGISTRY
- 全部从 `**已迁入 \`AppState.X\`**（阶段 Y.Z）` 简化为 `→ \`AppState.X\``

### BC2 外部文档引用内联（6 处）
- **`bot_chat.rs:1068`** — TASK-CHAT-EXECUTION 5 项评审已确认决策 inline
- **`memory/consolidate.rs:1`** — §10 consolidation 设计（候选/解析/调度/配置）inline
- **`memory/mod.rs:1`** — v1→v2 5 维度差异 inline（旧删 commit e1234a2）
- **`memory/rank.rs:1`** — 注入三段（pinned / top-5 / recent-3）+ 命中刷新 inline（原引用节号错：§3 应为 §4）
- **`memory/store.rs:1`** — 淘汰分公式 + 语义去重阈值 + 向量检索策略 inline
- **`audit.rs:463`** — 去除 SPEC.md / DEVLOG.md 名称提及，改为"长文档（README/规范等）"

## 验证

| 检查 | 结果 |
|------|------|
| 非注释行 diff | ✓ 空（仅改注释） |
| `cargo fmt --check` | ✓ 通过 |
| `cargo test --lib` | ✓ 663 passed |
| 硬红线 §1 只动注释行 | ✓ |
| 硬红线 §11 不 commit | ✓ |
| 硬红线 §2 不删 D 类 | ✓ |
| 硬红线 §3 不改 /// ↔ // | ✓ |
| 硬红线 §4 不改语言 | ✓ |

## 交付物

```
.comment-hygiene-2026-09-15/
├── HANDOFF.md                        ← 本文件
├── FULL.patch                        ← A+B+C 完整 patch（A 轮生成时仅含 A，B+C 后已重生成）
├── baseline/
│   ├── git-hash.txt
│   ├── git-status-before.txt
│   ├── test-lib.txt                  ← 原始基线 663 passed
│   ├── test-lib-after-A5.txt
│   ├── test-lib-after-A2.txt
├── reports/
│   ├── phase1-inventory.md           ← Phase 1 分类清单
│   ├── B-rewrites.md                 ← B 类改写建议（**已执行**）
│   ├── C-additions.md                ← C 类补全建议（**已执行**）
│   └── _raw/                         ← 原始 grep 命中
├── patches/
│   ├── A-comments.patch              ← A 类 patch（548 行，vs HEAD，A-only diff）
│   ├── A5-batch1.patch               ← A5 子批次
│   ├── A2-batch2.patch               ← A2 子批次
│   ├── BC-comments.patch             ← A+BC1 完整 diff（690 行，vs HEAD）
│   └── BC2-comments.patch            ← A+BC1+BC2 完整 diff（803 行，vs HEAD）
└── todos/

```

## 用户如何提交

### 方案 1 — 直接 commit 全部改动
```bash
cd /Users/renshi/Projects/wmessage
git add src-tauri/
git commit -m "chore(comment): A 62 + B 4 + C-1 9 + C-2 8 注释整理"
```

### 方案 2 — 只 commit A 类（BC 回滚）
```bash
cd /Users/renshi/Projects/wmessage
# 回滚 BC（在 A 之上）
git apply -R .comment-hygiene-2026-09-15/patches/A-comments.patch  # 不对，先 reset 到 HEAD
git checkout -- src-tauri/
git apply .comment-hygiene-2026-09-15/patches/A-comments.patch
git add src-tauri/ && git commit -m "chore(comment): A-class only (62 changes)"
```

### 方案 3 — 完全回滚
```bash
cd /Users/renshi/Projects/wmessage
git checkout -- src-tauri/
rm -rf .comment-hygiene-2026-09-15
```

## 建议 review 顺序
1. HANDOFF.md（5 分钟）✓ 你正在看
2. `patches/BC-comments.patch`（30 分钟，逐条确认 — 含 A+B+C 全部 87 处）
3. `reports/B-rewrites.md` / `reports/C-additions.md`（各 5 分钟，确认实际改动匹配建议）