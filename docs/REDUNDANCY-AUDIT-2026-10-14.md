# 冗余与功能重复全量审计（2026-10-14）

> 范围：全仓（src-tauri/src 127k 行 Rust + src/ 33k 行 TS/TSX）。方法：三路只读
> 扫描（Rust / 前端 / 交叉），逐条给 file:line 证据与可删行数估算；头部结论经
> 人工抽查复核（TodoCard 挂载点、PRAGMA 块数、addendum 逐字节比对均属实）。
> 口径：只算冗余 / 功能重复 / 死重——行为 bug 不在本报告（见同日修复批
> 7b9247e..3a697c6）。行数为估算，动手前以实际 diff 为准。

## 处置进度（2026-10-14 当日完成）

| 项 | 提交 | 备注 |
|---|---|---|
| Rust#1 db 迁移样板 16 份 → `ensure_columns` | `8fb5807` | 行为修正：PRAGMA 失败不再吞、duplicate-column 竞态容错覆盖全部补列点 |
| Rust#4 addendum 三份角色定义去重 | `04799c4` | 删 38 行内嵌、每次主对话省 1.16KB token，加防回归断言 |
| Rust#5/Addendum 同上 | `04799c4` | — |
| Rust#6 死常量 6 行 | `548764a` | brief.rs / trace.rs 全仓零引用删 |
| Rust#7 now_ms 六合一 → `db::now_ms` | `81e538f` | 15 文件；pre-epoch 负值替换 0 哨兵（修正非漂移） |
| Rust#8 配置钳制单源 | `5bc3fc6` | 4 个 clamp 函数为唯一来源；max_tool_calls 单点调用按 YAGNI 留内联 |
| Rust#9 audit 分类器单一化 | `f3c6dc9` | ToolFailureClass 枚举 + classify_tool_failure；is_tool_failure_text 转发包装随删 |
| 前端#5-#9 五项 | `2e08ac3` | bot_get_config 9 处 / 导入导出四合一 / 日期工具 / runWithBot / 停止分派 |
| 前端#1 TodoCard 只读化 | ⏸ 待拍板 | 报告写明需你定「归档/回收站卡还要不要恢复编辑」 |
| 前端#2/3/4 文件 chip×3 / 子任务×3 / 乐观更新×4 | ⏸ 排期 | — |
| Rust#2 DB 样板全仓 183 处 mutex_poisoned → `lock_ok` + `with_write_conn` | ⏸ Wave2 | 报告 P1，工作量 ~300 行 |
| Rust#3 转义/截断家族 8 个实现 + 同名 `escape_for_log` 行为分叉 | ⏸ Wave2 | 报告 P1，含真实分叉隐患 |
| 前端#10-14 微项 | ⏸ Wave2 | 10-50 行小批 |
| 双自检状态机合并 | ⏸ P3 | 两套刚各自跑稳，合并属可选 shrink |

**已净删 ~700 行（Rust 5 项 + 前端 1 项 + 死常量 + 角色去重）。**

## 总览

| 端 | 可净删 | 最大单项 |
|---|---|---|
| Rust（src-tauri/src） | ~677 行 | db 迁移 15 份拷贝（~180 行） |
| 前端（src/） | ~650 行 | TodoCard 不可达编辑机制（~340 行） |
| **合计** | **~1,330 行** | — |

## 门禁盲区（冗余得以存活的原因，先于一切清单）

**pre-commit 快门禁不跑任何 Rust 测试**（只有 fmt + cargo check + machete），
Rust 测试全在 pre-push 的 test-all。本审计时 main 领先 origin 36 个提交且未
推送——意味着这批提交**从未跑过全量测试**。实际后果（同日已修）：

- `task_chat_exec.rs` / `exec_trace.rs` 两个测试目标自 10-13 晚起编译不过
  （ModelLoopDeps 新字段未同步），`cargo test --no-run` 直接 101；
- `llm_integration.rs` 终稿自检两个用例样例文本 45/32 字 < 80 阈值，触发
  链路从未被端到端验证过即提交。

建议：test-fast 对 src-tauri 改动加 `cargo check --all-targets`（编译测试
目标即能挡 E0063 这类破损，成本远低于跑测试）；并保持推送节奏让 pre-push
门禁真实生效。

## Rust 端（按可删行数降序）

1. **dup · db/mod.rs 迁移样板 ×15**（236–464，~229 行区域）——「PRAGMA
   table_info 查列 → ALTER ADD COLUMN」同一 12 行块出现 15 次（10 次循环形
   + 5 次单列内联，9 次还在迭代本可合并的列常量表）；`db/migrations.rs:21`
   还有第 16 份。收敛为一个 `ensure_columns(conn, table, cols)` 后 ~180 行。
2. **dup · DB 访问样板全仓粘贴**——`mutex_poisoned` eprintln 183 处、
   `DB_WRITE_LOCK.lock()` 32 处、`open_db`+`ensure_table` 成对 30 处、
   `spawn_blocking(move ||` 124 处。重灾区：profile.rs（26）、bot_skills/
   state.rs（15）、evolution/emit.rs（10）、corrections.rs（2）。收敛为
   `lock_ok()` + `with_write_conn()` 两个 helper ~300 行。
3. **dup · 截断/转义 helper 一族 8 个具名实现 + 93 处内联**——两个同名
   `escape_for_log` 行为已分叉（audit.rs:19 弱版 vs bot/config/audit.rs:61
   强版，调用方按文件随机吃到哪个）+ paths.rs 第三份 + ratelimit 第四份；
   `truncate_chars` 两个模块同名不同签名（有无省略号）。收敛为一份强转义 +
   一个 `truncate_chars(s, n, ellipsis)`，~60 行并消除行为分叉。
4. **dup · bot_model_loop 双自检状态机**——工具自检与终稿自检各一套
   pending/expecting/节流三件套（~110 行），流式抑制条件逐字重复在两个
   emit 位点，N 提示文案逐字重复两处。可参数化为 `SelfcheckKind` 单机器
   省 ~50 行。**注**：同日修复后两套均已接线正确，合并属可选 shrink 非纠错；
   两套语义确有差异（Y/N 处置方向相反），合并时需保留分支。
5. **dup+delete · prompts/subagent.rs 角色定义三份**——PROFILE_RECON/
   EXECUTOR/VERIFIER 在 MAIN_AGENT_ADDENDUM 里有逐字节相同的拷贝（77–102
   vs 159–182，~1.16KB 双维护；已实测逐字相等）。stage 字段落地后运行时
   拼装走 const，addendum 里那份拷贝应改为「传 stage 即可」的一句话指引，
   省 26 行 + 每次主 agent 系统提示 1.16KB token。附带：edit_file mode=regex
   长句全仓 5 份拷贝。**stale allow 已在 0767776 清掉。**
6. **delete · 死常量 6 行**——db/brief.rs KIND_MANUAL/KIND_CLARIFY/
   SOURCE_AGENT/SOURCE_QA/SOURCE_SYSTEM、db/trace.rs TRACE_STATUS_TIMEOUT
   零引用。
7. **dup · now_ms() 六份实现**（bot_orchestrator/notifications/bot_skills/
   memory/api_handlers/migration 各一份）+ evolution 线 37 处内联时间戳。
   收敛一份 ~15 行。
8. **dup · 配置钳制双写**——bot/config/commands.rs 写侧 clamp 与 params.rs
   读侧 resolver 同一组边界字面量各抄一遍（`clamp(30,3600)`、`clamp(1,10)`
   等），db/subagents.rs 还有第三份。收敛到 params.rs 单源 ~12 行并消除
   漂移风险。
9. **shrink · audit.rs 关键词链两遍走**——classify_error_class 先调
   tool_call_failed 再重测同一组前缀做子分类；改单分类器返枚举 ~10 行。
   附 bot_skills/scheduler.rs `is_tool_failure_text` 纯转发包装 ~8 行（yagni）。

**查过无问题（留底）**：bot_anthropic vs bot_model_loop 非重复（适配层 vs
循环体，ghost-tool-call 双保险是有意的）；gen_dir_rule 两版受众不同不计；
keyring 构造正确委托；exec_steps/api 等处 allow 均有文档属有意。

## 前端（按可删行数降序）

1. **delete · TodoCard 不可达编辑机制 ~340 行**——看板被任务时间网格替代后，
   TodoCard 只在 ArchivePage/TrashPage 只读挂载（已人工复核挂载点），
   `!archived && !trashed` 之外的整个编辑面（挑文件/勾完成/标题·备注·标签·
   子任务·截止编辑/trace/运行 bot）全部不可达；SubtaskRow 的编辑分支同理
   （readOnly 恒真）。二选一：坍缩成只读卡，或明示为休眠看板代码。
2. **dup · 任务文件绑定流 ×3 + 文件 chip 列表 ×3 ~190 行**——pickFile/
   pickFolder（TodoCard vs TaskDetailPanel）与 chip 列表（TodoCard/
   TaskCardContent/TaskDetailPanel）三份渲染；「最多绑定 N 个文件」alert
   粘贴 4 次。收敛 `lib/taskFiles.ts` + `TaskFileChips` 组件。
3. **dup · 子任务行三种渲染 ~60 行**——SubtaskRow / TaskDetailPanel /
   TaskCardContent 各一份；toggle 映射逻辑也两份。
4. **shrink · WidgetApp 乐观更新样板 ×4 ~40 行**——同一 `task_patch` 乐观
   回滚形状贴在 toggleCollapsed/commitTitle/toggleSubtask/removeFile 四处，
   收敛 `patchTaskOptimistic()`。
5. **dup · bot_get_config 内联调用 8 处 ~45 行**——各自声明内联 payload
   类型；App.tsx 与 WidgetApp.tsx 的「取配置→设字号→听变更」整段 effect
   逐字重复（~35 行 ×2）。
6. **shrink · App.tsx 导入导出四胞胎 ~30 行**——exportTasks/importTasks/
   exportWorkspace/importWorkspace 同形，参数化省一半。
7. **shrink · 日期时间工具重复 ~25 行**——toLocalDT≡buildPlanDT、
   localDateStr≡dayKey（注释自己承认同口径）、basename 本地重声明、
   "MM-DD HH:mm" 格式化 5 份、周一开周内联 3 处、DOW1 标签数组 4 份、
   due "T09:00" 回填 2 份。收敛 week.ts/format.ts。
8. **dup · runWithBot / openOneFile 薄包装 ~25 行**——runWithBot 字节等价
   两份 + 第三份 emit 拷贝；open_file_path 绕过了声明的统一入口 openTarget.ts。
9. **shrink · ChatPanel 停止分派两份 ~15 行**（注释自己写「同一套分派」）。
10. **shrink · copyOneFile ok/err 定时器块 ×2 ~12 行**。
11. **dup · trace_list 绕过 lib 包装 2 处 ~4 行**。
12. **yagni · 小死包装**——WidgetApp openFilePath 纯转发、SubtaskRow 内联
    重声明 types.ts 已有接口。
13. **dup · link 图标三元 ×2 ~8 行**。
14. **shrink · 每分钟时钟 effect ×2（+30s 变体 ×1）~6 行**——收敛 useNow()。

**查过无问题（留底）**：五套确认弹窗各有文档化理由不合并；lib 导出零死
（knip 门禁有效，但建议 package.json knip entry 补 src/main.tsx 加固）；无
注释掉的代码块、无无人设置的 feature flag；types.ts 契约单源；Sigma 图谱与
@xyflow 画布是不同功能非重复。

## 处置建议

- **P1（一次专项可清）**：Rust 1/2/3（迁移样板、DB 样板、转义截断家族）
  合计 ~540 行，且第 3 项含真实行为分叉（同名转义函数强弱不一）；
  前端 1（TodoCard 只读化）单项 ~340 行，需先拍板「归档卡还要不要编辑」。
- **P2（顺手清理）**：Rust 5/6/7/8/9、前端 5/6/7/8/9，多为 10-50 行小项；
  第 5 项（addendum 拷贝）顺带每次对话省 1.16KB 系统提示 token。
- **P3（可选/暂缓）**：双自检状态机合并（先让两套各自正确运行一段时间）、
  前端 10-14 微项攒一批清。
- **不建议动**：负面清单所列的有意双保险/差异化实现；「第二个调用方出现前
  不抽象」的基线仍然适用——上表收敛动作全部有 ≥2 个现存调用方，不属投机抽象。
