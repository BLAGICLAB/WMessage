# OpenCodeReview 全仓扫描核查与修复记录（2026-10-07）

- 扫描工具：OpenCodeReview CLI（`ocr scan` 全文件模式，与 diff 无关），provider
  minimax-cn / MiniMax-M3，session `c5dbf007-5068-4927-b64c-1f8e8363ee5b`。
- 规模：发现 1238 文件、按默认规则审 328 个生产代码文件（Rust / TypeScript / C# /
  Shell / Python；`*.test.*`、node_modules、dist 等按工具默认规则排除），产出
  1501 条评论，约 2084 万 token，耗时 2h10m。原始报告全文：
  `docs/ocr-scan-report-2026-10-07.md`（26000 行，仅存档不做逐条核查）。
- 核查范围：全部 **337 条 critical / high**（bug 236、security 58、other 27、
  performance 12、test 4——critical 24 + high 313）逐条对照真实代码核查；8 个并行
  评审代理分块核查并按纪律修复，主控逐文件复核 diff、统一跑全量门禁。medium 及以下
  共约 1160 条**未逐条核查**，留在原始报告中供后续按需排查。

## 判定统计（337 条 critical/high + 主控自处 9 条 = 346 条）

| 判定 | 数量（约） | 说明 |
|------|-----------|------|
| 确认为真、已修复 | ≈130 | 含 10 个新增回归测试；另约 30 处为契约/注释补强（doc-only） |
| 误报（FALSE_POSITIVE） | ≈55 | 误读代码/前提不成立，代表性案例见下 |
| 别处已有防线（ALREADY_HANDLED） | ≈15 | 均附防线位置引用 |
| 属实但不修（WONTFIX） | ≈110 | 均给理由：既有拍板、实验态模块、风险>收益、需产品决策 |
| 需人工拍板（VERIFIED-REAL-NEEDS-HUMAN） | 13 | 见下清单，未动代码 |

## 两条 security critical 的修复（主控亲自）

1. **bot_fs.rs `resolve_writable` 符号链接闸**：写工具（write_file / edit_file）此前
   只 canonical 校验父目录、不校验目标本身。edit_file 的读取会穿过目标软链**绕过读
   白名单**（模型可借此读白名单外文件）；atomic_write 的 rename 会把软链整体替换成
   普通文件。现目标末段是软链即 fail-closed 拒绝（可操作中文报错），父目录软链不受
   影响；检查与写入之间的竞态残余窗口按仓库惯例注释留档。附：OCR 原文「rename 跟随
   软链写 /etc/passwd」的说法不准确——rename 替换链接本身，真实危害在 edit 的读穿透。
2. **workflow_decompose.rs 附件边界校验**：`attachments` 直达自 Tauri invoke 参数，
   此前逐字透传 Python 抽取脚本（无存在性/软链/扩展名校验）——被攻破的 WebView 可借
   doc_extract 读任意可读文件。「对话框亲手选 = 明确授权」只对 dialog 路径成立。
   新增 `validate_attach_path`：canonicalize（解析软链到真实目标）→ 必须常规文件 →
   扩展名限 docx/xlsx/pptx/pdf（抽取脚本支持集，其余脚本本就 exit(1)）；不合规条目
   走占位说明不炸整包。附 2 个单测（含 .docx 软链指向非文档文件须拒的用例）。

同文件另修：`fail!` 宏 `$err` 双求值（format! 拼两遍）改单次绑定；校验失败重试时
保留附件段（原实现第二次尝试上下文反而更少，抽取成本白付）。`summarize_messages`
无超时一条判 ALREADY_HANDLED（summarize_http 请求级 `.timeout(60s)` 已存在）。

## 其余重点修复（按域，全部最小外科手术式 diff）

- **本地 API**：SSE writer 注册表「先 spawn 后注册」空窗致 stop 永不置位/线程泄漏
  （持锁内联注册 + 顺手收割已退出 writer）；MemStore 批量 upsert 改两段式原子成败；
  bearer scheme 大小写不敏感（RFC 7235）。
- **bot 工具链**：`tool_read_text_file` canonicalize 失败不再回退原始路径（防
  lexical starts_with 绕过）；bot_artifacts 回写前重走 AI_Gen_Files containment 闸；
  tool_call arguments 单条 1MiB 累积上限（index 上限只管条数管不住单条 OOM）。
- **凭据**：keyring 降级文件写改临时文件 + sync_all + 原子 rename（防断电截断 key）；
  `read_llm_key` 的 read Err 透传（原 `if let Ok` 吞真实故障 → 静默回落全局 key）。
- **MCP / Skills**：新建 MCP 服务器 id 落库失败补偿清 keyring 孤儿；skill 步骤守卫
  Drop 判定+删除收进单临界区（防竞态误删新注册）；skill 名解析按括号实际字节数推进
  （多字节字符切片错位）。
- **调度 / 工作流**：定时建卡失败回滚 sched_last（下周期重试）；workflow_runner 上游
  表方向修正（原反表导致上游简报装错）+ 任务体 panic 未发终态时 Drop 守卫补发失败
  终态（防收单环永久挂起）；画布保存不再冲掉定时（定点 UPDATE 而非扩 ON CONFLICT）。
- **SQLite / 迁移**：`reset_bot_assigned_with` 改 CAS 抢占（原 load/exec/store 三步
  分离可双双执行）；`ensure_files_column` 容忍并发 duplicate column；keyring 写入
  原子化；migration journal 永久性失败清行、瞬时失败保留（终结对账死环）。
- **memory / evolution**：consolidate 三分支改逐 id 点查（替换 load_all 全量）；插入
  拒写含逗号标签（防 round-trip 劈裂）；merge/distill proposal_id 拼排序 refs 二次
  hash（防删改混淆）；applied ledger 追加纳入单写者锁。
- **前端**：ActivityPage loadError 空态渲染错位；挂件 6 条乐观更新路径失败回滚
  （带「未被新库态覆盖」守卫）；Export/Import 对话框互斥门；TrashPage 按 deletedAt
  倒序；ArchivePage 📌 跳转目标若折叠自动翻开；graphPrefs 坏键自愈 + isStringArray
  元素级校验；profile 写路径代际守卫（在飞旧快照不得覆盖新写入值）；确认弹窗新请求
  顶替时旧调用方不再悬挂；SchedulePage/TracePanel/NotificationsPage 竞态令牌一批。
- **C# docx 修订工具**：`ReadDocxLines`（File.Copy 失败的回退路径）遍历序与行文本
  口径改回「段落在前、表格行在后 + RowText」，与在地球路径和 Python 抽取脚本严格
  一致（此前文档序交叉遍历 + InnerText 全后代文本，回退时 DiffList 错位）；表格锚点
  插段落的 pPr/rPr 改取该表末行末格末段（原 Ancestors 对 Table 恒空，表后插入丢样式）。
  注意：OCR 对该文件的首要 finding（F0）**方向写反**——「段落在前表格在后」恰是
  Python/在地的统一口径，真正的离群者正是回退路径，已按镜像修复。
- **脚本 / 治具**：batch-verify.py 加 timeout fail-closed；fetch_ocr_models.sh 已存在
  文件补 SHA256 pinning；install-git-hook.sh 临时文件+原子 mv；graph-demo 治具
  unhandledrejection 对称捕获 + 未 mock 命令显式失败（防「看着正常其实炸了」）。

## 代表性误报（供后续扫描交叉比对）

- C# Program.cs F0（上文，方向反转，实修镜像处）。
- bot_fs 慢路径/锁序列化类：LOG_WRITE 锁的存在目的就是串行化 rotate+append 防行撕裂。
- 「tokio Mutex 中毒应 fail-closed」类：全仓 45+ 文件既定惯例是 eprintln + into_inner。
- `DefaultHasher::new()` 「非确定」：SipHasher13 固定密钥，跨重启确定性成立
  （随机种子的是 `RandomState`）。
- skill 名路径穿越：`SKILL_NAME_CHARS_OK` 谓词层已禁 `.` `/` `\` NUL，且有回归测试。
- eval 采样 `&s[..8]` 多字节 panic：属实已修；同类「前 300 字 body_preview 泄密」
  不成立（仅本机 UI/审计展示同一用户自己的入参）。

## 主控复核拦下的问题（代理 patch 不是全收）

- 3 处编译错误：`migrations.rs` 的 `Ok(())` 应为 `Ok(_)`（execute 返回 usize）；
  bot_fs 软链探针闭包签名不符；均为笔误级，当场修正。
- 1 处测试回归：`py/harvest.rs` sweep「未来 mtime 按过期处理」与既有测试钉死的语义
  （刚重建目录 mtime 略晚于采集的 now 必须保留——NTP 毫秒抖动即触发误删）冲突，
  回退为「未来 mtime 按 age=0 保留」，真实时间越过歪掉 mtime 后自然恢复清扫。
- 1 处死 import：types.rs 单次 `read_vendor_key` 决策修复后 `bot::has_vendor_key`
  转发失去全部调用方，最小清理。
- 批次号红线：全部新增行过 pre-commit 同款正则扫描，干净。

## 需人工拍板清单（属实、未修，附修复思路）

1. `bot/mcp/manager.rs` F62：normalize 先于 validate 合并了原始键，重名检测需
   normalize 返 Result 或 save 口前置查重（动签名，波及 commands.rs）。
2. `memory` F179：`record_lesson_core` 加 tuning 参数需同步改 memory/tests.rs 7 处调用。
3. `evolution` F139：拓扑/策略分层签名重构，破既有钉死测试，需先拍分层口径。
4. `api` F163：退出需 `api_stop_for_exit` 阻塞在途 handler 计数，属退出协议重设计。
5. `memory/panel.rs` F172：`store::update_by_id` 返回 `conn.execute` 影响行数，
   panel 侧 0 行时报「记忆不存在或已被删除」（一行改动 + 调用点）。
6. `src/types.ts` F266：`filePath/fileIsDir` 放宽 `string | null`（WidgetApp 直连
   invoke 时 undefined 键被 JSON 丢弃 → 旧 filePath 复活；TodoCard 路径已被归一化覆盖）。
7. `src/types.ts` F275：`\| string` 使 verdict 字面量 union 退化；收紧前先核对 Rust
   runner 的 verdict 取值空间。
8. `App` F291：execute-task 收尾时把 taskId 记入 finished 集合，chat-open-session 到达
   时以 watch=false 重开（涉及拍板 #22 的守卫生命周期重构）。
9. `tests-audit/evolution_gov.rs` F213：中途 assert 留脏——scopeguard/Drop 包 cleanup。
10. `tests-audit/exec_trace.rs` F225：失败路径按 session_id='-' 清不到 trace 行——
    cleanup 增加按 task_id 删 exec_traces 再级联。
11. `tests-audit/skill_e2e.rs` F234：还原只在 happy path——模块级 Drop guard 调
    `rebuild_routes(vec![])`。
12. `scripts/ci-guard-tiny-http-vendor.sh` F242：`--no-deps` 使守卫必失败且
    Cargo.lock 无 source 行——改查 `.manifest_path` 后缀 vendor/tiny-http 并实跑验证。
13. `tauri.conf.json` F10：CSP unsafe-eval 是否必需需运行时验证（第三方库可能用
    new Function），验证后再收紧。

## 验证证据

- `cargo nextest run`：1573 全绿（含新增回归测试）；`cargo check` 0 error；
  `cargo fmt` 已过；`cargo clippy` 无 error（137 条 warning 为存量基线，本次仅净减）。
- `pytest tests-audit/`（桥一致性 / 错误码 / 模块地图 / 前置事件）：全绿，0 xfail。
- `npm test`（vitest 全量）：全绿。`bash scripts/test-all.sh` 总耗时 74s。
- 变更面：124 个文件，+约 2000/−约 570 行（含本报告与原始扫描报告存档）。

## 遗留

- medium 及以下约 1160 条未逐条核查（原始报告在 `docs/ocr-scan-report-2026-10-07.md`，
  可按 `grep '\[bug · medium\]'` 之类按类别捞）。
- 测试代码（约 90 个 `*.test.*`）不在本次扫描范围；需要时可 `ocr scan --path` 单独扫。
- clippy 137 条存量 warning 可作独立清理批。
