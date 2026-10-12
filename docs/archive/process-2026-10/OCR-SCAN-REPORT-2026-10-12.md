# OCR 全量扫描报告（2026-10-12）

> 扫描：`ocr scan`（full_scan，会话 d855cb56，MiniMax-M3，耗时 1h46m，24.9M tokens）
> 发现 1142 文件 / 审 383 文件 / 1587 条评论。原始件：`OCR-SCAN-FULL-2026-10-12.json`（同目录，勿手改）。
> 基线：`docs/OCR-SCAN-TRIAGE-2026-10-09.md`（425 条，标记 ✅/🔧/🛡/📋/❌）。
> 本报告只做「新面/复报面」切分与初判，替代逐条人工分诊；复报点位凭基线标记沿用判定，勿重查。

## 总量

| severity | 条数 |
|---|---|
| critical | 18 |
| high | 351 |
| medium | 757 |
| low | 451 |
| （空） | 10 |

## 与 10-09 基线的切分

以 `9d9b82c`（10-09 23:28，上次扫描时点）为基线 commit，`git diff --name-only` 得 255 个变更文件。

- **复报面（文件未变，170 条 crit/high）**：多为基线已分诊点位的重报，判定沿用账本。OCR 每轮措辞不同，逐条文本匹配不可靠（已知同点位复报的包含率仅 ~0.5），故按文件级切分 + critical 逐条人工对位。
- **新面（文件已变，199 条 crit/high）**：真实新增审查面，需分诊。上轮经验：分诊派发按 2-5 个代理一波（并行过高撞 LLM 限流）。

## critical 18 条初判

| # | 位置 | 要点 | 初判 |
|---|---|---|---|
| C1 | `src-tauri/src/bot/config/io.rs:326` | `from_value().unwrap_or_default()` 吞 JSON 类型错配→空 map→keyring「总是覆写」语义下可能清掉用户手改的 secret | **新点，待分诊**（io.rs 基线后已改；比已知 unwrap_or_default 主题多了 secret 覆写后果） |
| C2 | `src-tauri/src/bot/mcp/config.rs:485` | `find_trim_collisions` 有实现有测试但 `upsert_in_config` 从不调用，空白差异键静默合并丢 secret | 复报，沿用基线 `config.rs:483` 🛡 |
| C3 | `src-tauri/src/bot_skills/manage.rs:705` | `copy_dir_all` 递归无边界：FIFO 阻塞 IPC 线程 / 无字节与深度上限 / 硬链外带 / socket-device 静默拷 | **新点，待分诊**（manage.rs 基线后已改，create_skill 新路径） |
| C4 | `src-tauri/src/db/subagents.rs:334` | 状态迁移 read-check-UPDATE 无事务，并发 last-writer-wins 覆写 started_at/finished_at | 复报，沿用基线同点位 🔧 critical（并发前提不成立） |
| C5 | `src-tauri/src/db/paths.rs:43` | atomic_write 无 fsync，进程在 write 与 rename 之间被杀可留空/截断文件 | **待分诊**（与基线 `paths.rs:37` rename-on-Windows 同族但 fsync 角度新；paths.rs 基线后已改） |
| C6 | `src-tauri/src/db/workspace.rs:171` | Mutex 中毒 `e.into_inner()` 静默恢复，持锁 panic 后 SQLite 状态可能撕裂仍继续写 | 模式复报（基线 `evidence.rs:171` 同模式 📋；workspace.rs 基线条目在 ：250/:287） |
| C7 | `src-tauri/src/evolution/candidate/entry.rs:80` | O_APPEND 原子性只在 PIPE_BUF 内成立，write_all 大条目多 syscall 可交错 | 复报同族，沿用基线 `entry.rs:0` 🔧（read_all/append/rewrite_jsonl 无同步） |
| C8 | `src-tauri/src/evolution/candidate/mod.rs:47` | read-then-append 去重非原子，主路径与 shadow hook 并发可双写 | 复报，沿用基线同点位 🛡 |
| C9 | `src-tauri/src/evolution/panel/evidence.rs:171` | 读路径持全局 DB_WRITE_LOCK 跨三个 DB 操作，序列化所有消费方 | 复报，沿用基线同点位 📋 |
| C10 | `src-tauri/src/memory/rank.rs:142` | `now_ms - base` 无检查 i64 减法，未来时间戳 debug panic / release 回绕 | 复报，沿用基线同点位 📋 |
| C11 | `src-tauri/src/migration/commands.rs:64` | UTF-16 LE/BE 分支 lossy 静默吞不成对代理项，与「破坏性规则不建在损坏解码上」的注释矛盾 | 复报，沿用基线同点位 🔧（PARTIAL 批在册） |
| C12 | `src-tauri/src/migration/types.rs:9` | JournalEntry 无 Serialize/Deserialize，注释宣称的崩溃恢复落不了盘 | 复报，沿用基线同点位 ❌（误报有反证，勿翻案） |
| C13 | `src-tauri/src/migration/ops.rs:246` | `copy_dir_recursive` 不查也不清残留 `<dst>.copying`，崩溃后重试混入陈旧文件 / rename 失败留垃圾 | **新点，待分诊**（基线只有 `ops.rs:85` 📋 与 `:188` 🔧，:246 是新点位） |
| C14 | `src-tauri/src/profile.rs:387` | 跨扩展名换头像时旧文件先删、save_data 后行，save 失败则磁盘停在「无头像」态 | 复报同族，沿用基线 `profile.rs:381` 🔧（delete-before-commit） |
| C15 | `src/test/node-shims.d.ts:3` | readFileSync 缺无 encoding 参数的 Buffer 重载，最常见调用形态直接类型报错 | **新点，待分诊**（基线无此文件；小而真） |
| C16 | `src/components/WorkspacePage.tsx:125` | persist 里 setItems 先于 beginWrite，窗口期外部 workspace-changed 触发 reload 覆写乐观 UI | 同族复报（基线 `WorkspacePage.tsx:114` 📋 critical 乐观更新次序家族；文件基线后已改，点位需对一对） |
| C17 | `src/components/WorkspacePage.tsx:167` | 拖拽排序同款竞态，UI 弹回拖前顺序 | 同点位复报（基线 `:168` ❌ 误报有反证；与 ：114 家族重叠） |
| C18 | `src/components/WorkflowCanvas/HoldToConfirmDelete.tsx:234` | confirmFiredRef 在 1.2s 后复位，父级 async onConfirm 未决窗口内二次长按可重复触发破坏性操作 | **新点，待分诊**（基线无此文件 crit/high；本文件上轮修过键盘 timer ref，本次是 async 未决窗口新角度） |

小结：复报/沿用 12 条（C2 C4 C6 C7 C8 C9 C10 C11 C12 C14 C16 C17——C6/C16/C17 为同族/同点位），**新点待分诊 6 条**（C1 C3 C5 C13 C15 C18）。

## 覆盖缺口（本轮降级文件）

- `src-tauri/src/bot_chat.rs`、`src/components/SettingsPage/SettingsPage.tsx`：prompt 超 max_tokens 80% 阈值告警，审查质量可能打折。
- `src-tauri/src/eval/mod.rs`：子任务上下文压缩超阈被停，该文件可能只覆盖了一部分。
- 工具调用失败 7/764（详情在 JSON `tool_calls.failure_details`）。
- 预滤 7 个超限文件未审（含上次扫描原始件等，多为归档产物，非源码）。

## 建议下一步

1. 6 条新点 critical 优先人工分诊（C1 secret 覆写、C3 copy_dir_all 边界、C18 重复破坏性触发值得先看）。
2. 199 条 changed-file crit/high 按上轮模式分诊（2-5 代理一波）；unchanged-file 158 条凭基线标记沿用，不重查。
3. 分诊账本建议沿用 10-09 格式（逐条标记 ✅/🔧/🛡/📋/❌），作为 10-12 基线。
