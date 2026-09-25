# Phase 2 Triage（docs-only，C5 SOP）

基线：`docs/OCR-CODE-REVIEW-2026-09-21-fullscan.json`，severity=high 且非 vendor = 280 findings。
本文件仅 triage，**不修代码、不跑 OCR、不 commit 代码**。docs-only 走 PROC-3 免审。

## 0. 执行日志
- [2026-09-26 02:30 CST] **P6T-1**（Phase 6-T 收口，commit 1d48969）：11 条 TOCTOU/权限策略自决落 docs/PHASE6-T-TOCTOU-POLICY.md（10 接受/defer/wontfix + C1b-7 修）+ bot_fs.rs walk/read_dir/JoinError 三处静默补 eprintln。OCR r1 4 comments（1 采纳 + 3 不采纳有论证，.zcodeignore 两项登记 Phase 4 卫生批）。D2: files=2(+43/-2, policy 新文件) asserts=0→0 tests=test-all 1147 全绿。**Phase 6-T 全清。**
- [2026-09-26 02:50 CST] **FUP-1**（小挂起 follow-up 批，commit 2ba5062）：① APW-02b-OCR-3 恒真断言——收紧过程暴露真发现：BUSY warn 本环境非确定性（checkpoint 可成功，恒真自原批起掩盖）→ 改 warn 值域断言 + 注释钉住非确定性；② APW-02b-OCR-5 staging 清理 4 组提 remove_staging helper；③ FRDV-01-OCR-1 domain "csv"→"migration" ×3（前端无 domain 消费已核）；④ MI-04b-OCR-3 守卫等待双魔数抽 const。APW-02b-OCR-2/4 维持挂起（enum 改动/场景构造复杂）。OCR r1 4 low：1 采纳 + 3 不采纳（.zcodeignore 清理超 scope）。D2: files=4(+29/-18) asserts=0→0 tests=copy_legacy_db 6 绿 + test-all 1147 全绿。**APW-02b OCR follow-up 3/5 清、1 部分清（4 场景构造维持挂起）、2 维持挂起。**
- [2026-09-26 02:35 CST] **NEW-2b-2**（NEW-2b 非 bot 域批，commit a5e90c1）：memory ×8 / evolution ×5 / api_handlers ×3 / py ×4 / db ×1 共 21 处 silent into_inner 补 C3-1 留痕——**全仓 silent into_inner 归零（NEW-2 12 + NEW-2b-1 24 + NEW-2b-2 21 = 57 处全部可见化）**。OCR r1 2 low：1 采纳（db::mod 标签改 db::open_db）+ 1 不采纳有论证（泛型 helper 抽象成本）。spec 校正 ×1（max_removed +25→+50）。D2: files=8(+84/-43) asserts=0→0 tests=test-all 1147 全绿。**NEW-2b 全部清零，poisoned-silent-recovery 家族收官。**
- [2026-09-26 02:20 CST] **NEW-2b-1**（NEW-2b bot 域批，commit 9523329）：bot 域 24 处 silent into_inner 补 C3-1 留痕（bot_skills skill_runs ×7 / bot_slash confirms ×7（4 处测试） / bot_chat ×5 / bot_scheduler ×2（1 测试） / exec_steps ×3）——恢复语义不变。OCR r1 3 comments：2 采纳（标签对齐既有 :: 约定 + 测试站点标签核正）+ 1 不采纳有论证（泛型 helper 抽象成本）。spec 校正 ×1（max_removed +30→+45）。D2: files=7(+96/-32) asserts=0→0 tests=test-all 1147 全绿。

- [2026-09-26 02:00 CST] **C4-R**（Phase 1 C4 残余收口，docs-only 零代码）：全 6 条现读核验——observe_run.rs:96 已修（历史批注释在位）/ test-all.sh:13 `-j num-cpus` 判 FP（nextest `-j` 文档值，实证 exit 0）/ format.ts monthly 已修（年份位正确）/ constants.ts:18 SIZE_KEY 判 FP（OCR 误读注释行，:20 实际值 namespaced 正常）/ PANEL_H ×2 维持 C4-v2 wontfix-pending-product-decision / App.tsx C4-6 已修。**Phase 1（critical）至此全清**。fix-plan 跟踪表 C4 行已回填。

- [2026-09-26 01:45 CST] **PHASE2 终态清扫（zcode 会话收口）**：B 类 23 项全部处置完毕——20 项实修落地（DB-B/MI-B1/BOT-B/FE-B/API-B/MI-B2/EV-B 共 7 批）、#7 判定 A（N=87>5 维持 eprintln）、#14 wontfix（C）、#23 前提不成立零代码（reviewer agent-21）。附加批：MI-B3（跨域阻塞族 MI-05b=A，recovery.rs:197 stale 零代码 agent-22——三簇全处置）、NEW-2（12 处 C3-1 留痕）、NEW-1345（NEW-1/3a/4/5 实修 + 3b stale）。**待拍板遗留：NEW-2b（47 处同形 silent into_inner，§3 登记）。** 择机项维持 leave：C4-v2 / DB-02b / META-8；PROC-5 eval 更新（见 §3.5）——n=25+ 但近期 10 批仅零星发作且均未影响 review 本体，**不立项**。环境根治：python3.14 装 pytest（--break-system-packages），test-all 无 PATH 前缀验证全绿（310+1147）。工程入库：HANDOFF-2026-09-25 + 9 份历史 spec 归档 commit（HANDLERS 为已撤销 spec 证据）；HANDOFF-2026-09-23 仍 untracked 待定；tests-audit/__pycache__ 加入 .gitignore。

- [2026-09-25 12:10 CST] **NEW-1345**（混合批，commit 9f50382）：NEW-1 bot_py 测试 ×2 silent into_inner 补 eprintln（锁名现场核正=py_children）；NEW-3a escape_for_log 控制字符全集（is_control→escape_default + U+2028/2029 显式臂，"| " 预替换保输出逐字节不变）+ 全集单测；NEW-4 panic payload 提取 5 处收敛 crate::audit::panic_message（middleware/api_server 本地 fn 删、run/bot_scheduler 内联改调，文案统一 pure 措辞）；NEW-5 atomic_write rename 失败 tmp 清理 + 单测。**NEW-3b stale 零代码**：rotate 锁外容错 + append 锁内，撕裂面已闭合。OCR r1 3 low（2 采纳 extend/capacity；1 不采纳预替换防漂移）。spec 校正 ×1（assertions 3→2 实测）。D2: files=8(+93/-48) asserts=0→56 tests=+2 绿 + test-all 1147 全绿。

- [2026-09-25 11:50 CST] **NEW-2**（三域同形批，commit 6a7cce6，amend 80bd6f7 仅修 D2 数字）：12 处 silent into_inner 补 C3-1 eprintln 留痕（db/workspace x3 + bot_history x2 + bot_sessions x3 DB_WRITE_LOCK / migration/ops x2 / bot/config/audit x1 + mod x1 测试）——恢复语义不变，纯日志。**范围发现：全仓实测另有 47 处同形 silent into_inner（NEW-2 登记注记「其他域多数已有 eprintln」与现状不符）→ 不越权扩，NEW-2b 登记待拍板。** OCR r1 0 comments（一轮清零）。spec 校正 x1（max_removed +20→+45）。D2: files=6(+60/-36) asserts=0→0 tests=test-all 1145 全绿。

- [2026-09-25 11:35 CST] **MI-B3**（migration 域，C5-MI-05b 拍板 A 落地，commit db6d8e9）：commands.rs migration_rules_load / migration_status 两条 sync command → async + spawn_blocking（log_read 既有模式，主线程不再做文件 IO；前端契约不变）；docstring 计数对齐（7 个 command）。**同簇 recovery.rs:197 经 reviewer（agent-22）核验为 stale 零代码**：block_on 在 spawn_polling std::thread 上是 B3 设计内唯一合法桥接（调用链 lib.rs:305→run.rs:462 std::thread::spawn→:484→recovery.rs:110 双证 + run.rs:62 先例逐字同款），async 化需重构 spawn_polling 顶层=扩 scope 零收益。OCR r1 4 low（2 采纳：join 文案统一 / `??` 显式化；2 不采纳有论证）。D2: files=1(+14/-6) asserts=0→9 tests=migration::commands 4 绿 + test-all 1145 全绿。

- [2026-09-25 11:10 CST] **EV-B**（evolution 域 B 类拍板落地批，commit e753185）：实修 1 处（#17=C）——record.rs/entry.rs read_all 收敛到 evolution/mod.rs 新增 read_jsonl<T> 泛型内核：首个非空行损坏 → Err（结构级 fail-closed，first_parsed 标志防前导空行偏移）；中间坏行 → stderr 留痕跳过返回好行；candidate/mod.rs write_proposals 的 unwrap_or_default → ?（dedup 基线不可得不再盲写）。行为变更：部分损坏 jsonl 不再整挂 evolution 面板；损坏文件写路径 Err 传播。OCR r1 8 comments 全同根：2 high 采纳（i==0 判定偏移 → first_parsed）+ 4 medium 不采纳有论证（eprintln 同域先例/dedup 代价拍板明示）+ 2 low 采纳；r2 8 comments 0 high（1 medium 采纳=read_jsonl 泛型收敛；7 low）。#23 ProposalTarget：**reviewer 核验（agent-21）前提不成立**——TS types.ts:26-28 与 Rust proposal.rs:66-72 已逐字段镜像 snake_case，唯一消费方 formatTarget 全 snake_case 访问，全 src/ 零 camelCase 误用点，文件头注释已钉约定——拍板 A 对象为假设性风险非现存缺陷，重命名纯 churn，零代码登记勘误。D2: files=4(+98/-36) asserts=0→56 tests=evolution 275 绿 + test-all 1145 全绿。
- [2026-09-25 11:10 CST] **POISON-7**（docs-only）：拍板 #7=A（若 N≤5 则 B）条件判定——全仓毒锁恢复点（eprintln [mutex_poisoned] 形态）实测 **87 处**（grep 全 src-tauri/src，28 文件；profile.rs 25 / bot_skills/state.rs 10 / evolution/emit.rs 7 为前三），远超 ≤5 阈值 → **维持 eprintln（A），不升级 audit_event!**，C3-1 约定不变。#14（StopGuard 接线）已拍 C=wontfix-with-rationale 登记于 §2 BT-14 行。

- [2026-09-25 10:55 CST] **MI-B2**（migration 域 B 类拍板落地批，commit ab47f1b）：实修 2 处——① #2=A 取消机制：static MIGRATION_CANCEL + CancelGuard(Drop) run 结束统一清零（panic 安全）+ 阶段一归档落盘前/阶段二主循环双检查点安全停止（已完成不回滚）+ report.cancelled 字段 + migration_cancel command 注册（取消 UI 未来迭代接线）；② #8=A replay 前置 purge_orphan_pending：同 (task_id,src) 多 pending 留最新删其余 + 留痕（MI-04a 修复前历史遗留不再同 key 重复处置）。OCR 3 轮：r1 2 high 采纳（检查点缺阶段一/轮询入口清零吞请求）+ 2m + 2l；r2 1 high 采纳（清零可被 panic 绕过 → DropGuard）+ 3m（2 采纳 doc 对齐；1 不采纳 mock 成本）+ 5l；r3 8 comments 0 critical 0 high（4m：1 采纳组合测试终态断言，3 不采纳有论证）。spec 校正 ×1（budget +180→+210）。D2: files=6(+202/-0) asserts=0→9 tests=migration 68 绿 + test-all 1141 全绿。

- [2026-09-25 10:23 CST] **API-B**（api 域 B 类拍板落地批，commit 6de187b，amend 177aaa8 仅修 D2 数字）：实修 2 处——① #16=A api_server.rs broadcast 落盘写失败 → fetch_sub 归还 id + 事件不进重放历史不推送（fail-closed「要么持久化要么不推进」；归还后下次广播重取同 id 重试；单临界区单写者无竞争）；② #15=C commands.rs:61/:238 flag 写/清残余 TOCTOU 微窗注释声明（复核≠原子、可自愈）。OCR r1 2 low 全采纳（测试名/docstring 对齐实际行为 / atomic_write rename 失败残留 tmp 测试自清理）。**PHASE2-TRIAGE-NEW-5 登记：db/paths.rs atomic_write 在 rename 失败时残留同级 .tmp 文件（既有缺口，本批测试暴露），下一轮评估。** D2: files=2(+63/-2) asserts=0→19 tests=api_server 6 绿 + test-all 1138 全绿。

- [2026-09-25 10:14 CST] **FE-B**（frontend 域 B 类拍板落地批，commit 3dc8e3a，amend 8ffafcb 仅修 D2 数字）：实修 5 处（批内独立改动如实声明）——① #19=A App.tsx legacy 迁移失败 catch 标记 + 跳过 SEED（不再造成 legacy 永久 orphan）；② #18=B tasks-updated delete 失败 try/catch 不阻断合并广播（行已从内存移除、库中仍在 → db_load 自愈）；③ #21=B ConfirmMap 新 confirm 自动拒未响应旧请求（fire-and-forget；busyRef 在途不补刀）；④ #22=B ChatPanel execWatchRef 围观守卫（置值/收尾 history_load **完成后**无条件清 + execute-task 失败路径清 / send 拦截上移 + /stop 白名单 / load 链 catch 全覆盖）；⑤ #20=A ArtifactBatchDialog skip 语义注释。OCR 3 轮：r1 9 comments（3 high 采纳：mutate 同治 + watch 清理嵌套 ×2；3 medium 采纳；3 low 1 采纳）/ r2 8 comments（1 high 采纳：清早于 load 完成 → 无条件发起 + finally 清；3 medium 采纳：ref 序 / mutate 测试 / 失败路径泄漏）/ r3 7 comments **0 critical 0 high**（3 medium：2 采纳=load 链 unhandled rejection + 斜杠绕过拦截；1 不采纳=初始 load 失败有 execute-task 收尾兜底）。**flaky 第 5 例**：ChatPanel 流式合并 scrollTo（jsdom 滚动桩偶发），隔离+全量双过。spec 校正 ×1（assertions 0 先例）。D2: files=6(+223/-11) asserts=0→0 tests=vitest 310 全量绿 + tsc 0。

- [2026-09-25 09:41 CST] **BOT-B**（bot 域 B 类拍板落地批，commit ea6317d，amend 623fc34 仅修 D2 数字）：实修 5 处（批内独立改动如实声明）——① #10=C io.rs add_allowed_dir 剥 key 前置 keyring 读回验证（空槽写入→读回；原有值非空即剥=值优先；本次写入读回等值才剥；读不回保留明文零丢失；audit 挪写盘成功+放锁后——r1 high 采纳；has+read 并单 read——r1 low 采纳；内核 plaintext_strippable+单测）；② #11=A state.rs clear_terminal_skill_runs 按会话隔离（签名加 session_id，三调用点随动；他会话终态 run 不再被入口误删）；③ #12=B bot_web.rs Jina 回退三路 stderr 留痕（失败/采用/未采用带 URL，签名不动）；④ #6=A vars.rs in_quotes wontfix 确认（设计边界注释 + 2 回归测试锁现状）；⑤ #13=B keyring.rs Windows 权限 wontfix 文档化（权限语义仅 Unix 承诺）。OCR r1 4 comments 全同根：2 high 采纳（audit 时序 / strip 内核测试漏加）+ 1 medium 采纳（读回等值比对）+ 1 low 采纳（单 read）；r2 3 comments 0 high（1 medium 不采纳=持锁 keyring IO 系既有约定同 migrate_legacy_key 先例；2 low 不采纳有论证）。D2: files=7(+171/-22) asserts=0→55 tests=io/vars/state 23 相关绿 + test-all 1137 全绿。

- [2026-09-25 08:53 CST] **MI-B1**（migration 域 B 类拍板落地批，commit 5b5f360，amend 00bab18 仅修 D2 数字）：实修 2 处——① #1 rules.rs load_rules 拆纯函数 load_rules_from（Result<RulesFile, CommandError>：NotFound=首装合法态 / 读失败 IoError / 解析失败 DomainRule(migration)；坏文件不再被空规则顶替）+ 三调用点分流（run.rs 迁移中止 / rules_load·status→CommandResult，MigrationPanel try/catch 已核前端零改动）+ run_migration 链 String→CommandError 结构化直传；② #9 归档目录三道闸（`..` 既有 / 绝对路径+has_root 根式 / symlink 逃逸祖先 canonicalize）+ run.rs create_dir_all 后 verify_archive_dir_created 终态复验 + validate_rules containment 抬出 move 块全动作生效 + 导入破坏性确认框（spawn_blocking，启用行计数，confirm 审计落 save_rules 后）。行为变更：坏规则文件 fail-closed；存量绝对路径归档配置拒绝执行（拍板接受）；导入前确认框。OCR 4 轮：r1 1 high 采纳（From<String> 塌码）+4m+10l；r2 0 high（1 critical 采纳=run.rs String 桥接）+3m+16l；r3 3 high 全采纳同根（create 后复验 / audit 补 disabled+count / 注释精确化）；r4 0 critical 0 high（3m 采纳 3 不采纳 + 11l 处置）。spec 校正 ×3（assertions 计数 / budget +300→+380→+450）——**≥3 次触发 reviewer 抽查（agent-20，另行登记）**。D2: files=4(+397/-43) asserts=0→16 tests=migration 65 绿 + test-all 1134 全绿。**reviewer 抽查（agent-20，spec 校正 ×3 触发）= PASS**（8 项逐核：diff-spec 一致 / load_rules_from 边界 / 调用点与返回类型链无残留 / 三道闸+复验位置 / 确认框+审计时序 / 65 测试实跑 / r3 high 与 raw json 对账 / 新注释零批次号；唯一瑕疵=commit message r1/r2 medium/low 计数各差 1 的表述笔误——medium 被解析脚本归 low，总 comments 与 high/critical 计数吻合，不影响处置实质）。

- [2026-09-25 08:07 CST] **DB-B**（db 域 B 类拍板落地批，commit f137536）：实修 3 处——① #3 paths.rs copy_legacy_db 源库 open 失败（warn+继续裸拷）→ 早退 Err fail-closed（原库只读保留 + mod.rs Err 臂补 audit ERROR + 可操作消息含排障步骤；OCR r1 low 采纳同根收紧：checkpoint 臂非 BUSY 错误（NOTADB/IO）同治 fail-closed，仅 BUSY 容错走 -wal/-shm 带走良性路径）；② #4 workspace_import_merge NULL updated_at → match 三臂，Some(None)=整批 Err（tx 回滚 + 行 id 指认 + 回填指引；行不存在/有值现状保留；导入行缺时间戳视 0 文档化）；③ #5 bot_history (created_at,id) 复合排序键不变量注释固化（load 本就 ORDER BY id，行序不依赖 created_at）+ 回归测试。行为变更：坏源/损坏老库不再被裸拷为主库；NULL 目标行导入由静默改整批 Err。OCR r1 4 low（0 high / 0 medium / complete）：1 采纳（同根收紧）+ 3 不采纳有论证。spec 校正 ×1（budget +220→+260，5a25a66）。D2: files=4(+229/-7) asserts=0→167 tests=cargo 新增 4 回归绿（BUSY 良性路径 6/6 不回归）+ test-all 全绿。
- [2026-09-25 07:25 CST] **B 类决策权威清单重建（zcode 接手第一步）**：23 项全量重建入 §4 末段（grep 攒批/转 B 类/B 类候选全量 + 逐项源码 file:line 现场核对 + OCR fullscan 原文复核）。勘误：HANDOFF-2026-09-25 §3.1 表 #10 误标 EV-3b-A（实为 add_allowed_dir 剥 key，§4:555-557）；EV-3b-A 残 2 条 = #17。#1–#9 为新赋编号。**环境漂移登记：本机 `python3` 现解析 ~/.local/bin/python3（3.14.7，无 pytest），test-all.sh 第 2 步 pytest 失败；以 `PATH=/usr/bin:$PATH` 前置系统 python3（3.9.6 + pytest 8.4.2）复跑全绿（26s，1119 passed + pytest + vitest）。后续批三层自测须带此 PATH 前缀，或用户拍板给 3.14 装 pytest。**
- [2026-09-22 08:32 CST] 分域表修正：bot* = bot/ 子目录 18 + bot_skills/ 子目录 12 + bot_*.rs 兄弟 16 = 46 条（严格互斥去重）。原"bot 18" = bot/ 子目录 18；原"bot_skills 12" = bot_skills/ 子目录 12；兄弟 16 条散落 9 文件（artifacts 2 + chat 1 + fs 1 + model_loop 4 + plan 1 + py 1 + scheduler 2 + slash 1 + web 5 = 18 raw → 去重 16 唯一）。
- [2026-09-22 08:32 CST] C5-DB-01 拆簇：DB-01 内部混"丢弃"与"替换默认值"两种模式，按新判据拆为 DB-01a（error-not-propagated，3 条）/ DB-01b（failure-recovery-default-value，2 条）。
- [2026-09-22 08:32 CST] C5-BT-01 拆簇：BT-01 内部混模式，按新判据拆为 BT-01a（error-not-propagated，5 条）/ BT-01b（failure-recovery-default-value，2 条）。
- [2026-09-22 08:50 CST] Poisoned 冲突核清：BT-02 三站 OCR finding 的"silent"描述均不准确（实有 eprintln 走 C3-1 logged 路径）→ BT-02 整簇降级 OCR false positive，移出 poisoned-silent-recovery 家族。MI-01（migration/journal.rs:18 silent）保留 → 真 high（违反 C3-1 约定）。poisoned-silent-recovery 家族仅剩 MI-01 单成员 → 家族溶解（1 成员家族无跨域价值），MI-01 独立存在。
- [2026-09-22 08:50 CST] C5-DB-02 拆簇：按破坏数据判据拆为 DB-02a（atomicity/partial-write，2 条：migrations.rs:51 + paths.rs:65）/ DB-02b（事务约定一致性 / 设计债，1 条：tasks.rs:439 → wontfix-pending-design-decision）。
- [2026-09-22 08:50 CST] atomicity/partial-write 家族重划：原 {MI-06, paths.rs:65} → 现 {MI-06（migration/ops.rs:127，1 条）+ DB-02a migrations.rs:51（1 条）+ paths.rs:65（1 条）}= 3 条跨 2 域。
- [2026-09-22 08:53 CST] 新发现登记：bot_py.rs:694/:705 测试代码内 silent into_inner，源 OCR high 名单未含（OCR 标的 :945 走 logged 路径）→ PHASE2-TRIAGE-NEW-1，下一轮评估。
- [2026-09-22 08:53 CST] DB-02b 分类明确：wontfix-pending-design-decision（finding 自承"current single-statement DELETE is atomic, so this is not yet a correctness bug"，未来引入 cascade/soft-delete 时重审）。
- [2026-09-22 08:53 CST] 累计口径分双数：triage 记录 140（BT-02 三条 FP 仍算记录）/ Phase 2 实工单 137（FP 不计入工作项）。
- [2026-09-22 09:00 CST] WA-01 与 C4-v2 同源合并：OCR 标 constants.ts:13 实为 line 9（PANEL_H_MIN/MAX 字面 800/900），与 C4-4 同源。PANEL_H 家族升级合入 C4-v2（PANEL_H_MIN > PANEL_H 单项 → PANEL_H_MIN + range 100px 两子项同族，同一产品决策）。WA-01 拆为 WA-01b（storage.ts:113 + storage.ts:79 = 2 条，类型不一致 / JSON.parse 零校验，与 PANEL_H 无关）。
- [2026-09-22 09:00 CST] WidgetApp 簇清单重写：3 簇 / 7 findings（WA-01 合入 C4-v2 后剩 WA-01b 2 + WA-02 3 + WA-03 2）。
- [2026-09-22 09:10 CST] WA-01 命名修正：WA-01b 重命名为 WA-01（原 3 条拆为：a=constants.ts:13 已并入 C4-v2 不独立计 + b=storage.ts:113+:79 保留为现 WA-01 = 2 条）。原 WA-01 3 条 → 1 条并入 C4-v2 + 2 条构成现 WA-01。
- [2026-09-22 09:10 CST] OCR 行号核：WA-01 finding（line 13）与 C4-4 finding（line 8）OCR JSON 行号均与实际字面行吻合（line 13 = PANEL_H_MIN/MAX 字面；line 8 = PANEL_H default 字面）。我之前报"OCR 实际指 line 9"是自查报告误差（误把 PANEL_H_MIN 当 line 9，实为 line 13）。**OCR JSON 行号机制无系统性问题**，无需在 triage 层加 OCR 行号偏移记录机制。
- [2026-09-22 09:15 CST] frontend 碎片实抽：52 条 / 34 文件（你估 40，超出 12 条）。9 簇 / 批（FE-01 至 FE-08b），全部按文件组批 ≤5 文件/批 + ≤10 findings/批 + ≤+500/−300 行。剩 81 条（不是 92）。
- [2026-09-22 09:18 CST] OCR 行号答（自查）：Finding A（C4-4）start_line=8 critical，Finding B（WA-01）start_line=13 high。OCR JSON 行号与 constants.ts 实际字面行（line 8 = PANEL_H default = 560；line 13 = PANEL_H_MIN/MAX = 800/900）完全吻合。我之前报"OCR 实际指 line 9"是 (b) 自查报告误差（误把 PANEL_H_MIN 当 line 9，实为 line 13）。**OCR JSON 行号机制无系统性问题**，不追加 PROC-4。
- [2026-09-22 09:18 CST] frontend 数字校正：实测 FE-04..08b 6 个批的 finding 数与文件数有 2 处错（FE-06 列 6 文件我写 "5 文件"；FE-07 同病）。修正为精确 6 文件 / 6 条每批。
- [2026-09-22 09:18 CST] 累计基准修正：plan §5 的 280 是 raw 计数（含 path+start_line 重复，如 bot_model_loop.rs:1079/:984 各 2 次）。去重后 unique = **271** non-vendor high。修正 triage 累计 = 197 unique findings（75 簇对应）/ 其他 remaining = 74 unique findings（不再是 81）。
- [2026-09-22 09:00 CST] WidgetApp 簇清单重写：3 簇 / 7 findings（WA-01 合入 C4-v2 后剩 WA-01b 2 + WA-02 3 + WA-03 2）。
- [2026-09-23 23:20 CST] EVNB-02 收口（commit 299e776）：C5-DB-05 的 poison.into_inner 静默条（tasks.rs:423，实际修复点 db_delete tasks.rs:469-471，行号漂移）+ C5-MI-01（migration/journal.rs:18）已修。两站统一走 C3-1 logged-path（eprintln("[mutex_poisoned] ...") + into_inner 恢复语义不变）。**family 归属修正**：该 mutex 条原挂 failure-recovery-default-value（C5-DB-05），实为 error-visible-non-blocking（C3-1 约定 = logged 恢复即合法，BT-02 FP 先例）——triage 时归错 family，借此修正。C5-DB-05 4 条 → 3 条；C5-MI-01 单条已清。
- [2026-09-23 23:20 CST] 新发现登记：db/migration/bot.config 三域 12 处同形静默 into_inner（不在 OCR 271 名单，±3 行上下文 grep 核实无 eprintln）→ PHASE2-TRIAGE-NEW-2，下一轮评估。
- [2026-09-23 23:25 CST] FRDV-01 收口（commit 46f8437）：C5-MI-03.2（rules.rs:121 CSV 双静默默认值）已修 fail-closed——空动作行 / 未知启用 token → 行级 DomainRule Err。C5-MI-03 2 条 → 剩 1 条（rules.rs:27 load_rules 静默 fallback default，quarantine vs 结构化 Err = B 类候选，攒批待拍）。
- [2026-09-23 23:35 CST] MI-07 收口（commit 613e07a）：C5-MI-07（ops.rs log_line rotation/写入路径分裂）已修——单次 canonicalize 结果共用 + 失败兜底不丢日志。OCR r1 唯一 low（data_dir 双调）已采纳入批。migration 域 10 簇剩 4 簇未清：MI-04a/b、MI-05a/b、MI-08。
- [2026-09-23 23:50 CST] DB-04 收口（commit c9a3041）：C5-DB-04 全簇 2 条已修——bot_session_rename 空标题 fail-closed（InvalidArgument）+ tasks_import 复用 check_export_path + 64MiB 读侧 bounded 强制。OCR r1 2 comments（medium TOCTOU + low 文案截断，同根=本批新代码）均采纳入批（metadata 预检 → bounded reader）。db 域 7 簇剩 2 簇未清：DB-03（race/TOCTOU）、DB-05 余 3 条（均 B 类候选）。
- [2026-09-25 06:59 CST] **TRIAGE 状态清扫**：全量比对 §2 各域行标 vs git log，发现 4 簇已修但行标漏打（早期批 APW-01/APW-02a/APW-02b/DB-01b-BT-01b/首批开批先于行标纪律）。逐点源码复核（migrations.rs tx 包裹 / paths.rs staging+swap / ops.rs staging+rename / run.rs·recovery.rs 显式传播 / workspace.rs map_err 传播）全部在位后补打行标。**结论：DB-01b / DB-02a / MI-02 / MI-06 无需新批**。剩余未清 = 跨域阻塞族待核区 3 簇（明示不动）+ wontfix / B 类待拍 23 项。
- [2026-09-25 07:06 CST] **FE-08b**（frontend 域「入口/主题/样式」簇，commit 2f006ef）：实修 2 处——main.tsx:10 root as HTMLElement 直转型 → 判空 + 描述性 Error fail-fast；main.css:131 基类 transition 补 transform 0.2s ease（nm-card-hover 悬停进出对称）。FP 1 条（reviewer agent-19 pass，零代码）：theme.ts:33 applySetting 不通知 in-process subscribers——全仓唯一订阅方 App:126/WidgetApp:142 均自身为写入方，无可失同步者。OCR r1 0 comments（一轮清零）。D2: files=3(+33/-2, 新测试 main.css.test.ts +24) asserts=0→0 tests=vitest 306 全量绿 + tsc 0。
- [2026-09-25 06:59 CST] **FE-08a**（frontend 域「挂件/看板」簇，commit d937b01）：实修 3 处——DoneCircle.tsx:13 type=button；KanbanBoard.tsx:179 active.rect.current 可选链守卫（防御拖拽中卸载）；TaskCardContent.tsx:293 + TodoCard.tsx:614 ⏰ 点击无条件重置定时草稿 → 仅展开且草稿空时初始化 + 取消定时清草稿（OCR r1 high 指出主窗口/挂件镜像必须双侧同修）。行为变更：误触 ⏰ 收起再展开草稿保留。OCR r1 2 high 同根因全采纳（spec 校正 1 扩 expected_files/budget）/ r2 0 comments（high 验证通过）。D2: files=7(+81/-3, 新测试 +25) asserts=0→0 tests=vitest 55 相关绿 + 全量绿 + tsc 0。
- [2026-09-25 06:47 CST] **FE-07**（frontend 域「次要组件零散」簇，commit 487b499）：实修 5 处——SkillsPanel.tsx:60 noticeTimer ref + 重设前 clear + 卸载清理；ApiProviderSelect.tsx:37 零 ARIA 自绘下拉补全（haspopup/expanded/controls/activedescendant 挂焦点元素 + listbox/option/aria-selected + 方向键/Home/End/Enter 导航 + 关闭态方向键展开 + Tab 收起 + 焦点归还 + tabIndex=-1 不进 Tab 序 + useId 选项 id）；DeleteConfirmDialog.tsx:40 焦点进/出恢复（默认落取消钮）+ backdrop onCancel（内卡 stopPropagation，trap 不实现注释声明）；ActorAvatar.tsx:10 loadProfile catch 防护（留痕在 profile.ts 侧不重复）；ArchivePage.tsx:52 displayTask 仅 collapsed===undefined 才产副本。转 B 类 1 项（types.ts:24 snake_case union → #23）。SettingsPage.test 5 处旧断言翻 role=option（锁旧非 ARIA 结构）。OCR r1 6 全采纳 / r2 4（1 high 采纳 + 1 medium 采纳 + 1 low 不采纳有论证 + 1 无问题）/ r3 6（0 high 验证通过：5 采纳 1 low 不采纳有论证）。spec 校正 ×2。D2: files=10(+122/-25, 新测试 4 文件 +248) asserts=0→0 tests=vitest 71 相关绿 + 全量绿 + tsc 0。
- [2026-09-25 05:23 CST] **FE-06**（frontend 域「ChatPanel / utility」簇，commit 189adc4）：实修 4 处——constants.ts:9 execTaskDedup 裸 Map → 表私有化 + shouldSkip 单入口 + prune（EXEC_TASK_DEDUP_MS 去 export，ripple ChatPanel.tsx:449-451）；errorHandler.ts:216 onRetry 裸调 → try/catch 回收 silent；useInlineEdit.ts:44 Enter→Blur 双提交 → committedRef 前置 + onBlur 跳过一次（既有测试 :129 锁旧缺陷行为翻断言 ×2→×1）；EvolutionPanel.tsx:149 status cast → ReadonlySet<ChangeRecord["status"]> 去 cast。FP 1 条（reviewer agent-16 pass）：types.ts:5 args string——唯一消费是文本渲染无人 JSON.parse。转 B 类 1 项（ChatPanel.tsx:416 围观期锁发送方向 → #22）。OCR r1 0 comments（complete，3 file_read 参数失败已重试不影响）。D2: files=8(+92/-16, 新测试 +27) asserts=0→0 tests=vitest 89 相关绿 + 全量绿 + tsc 0。
- [2026-09-25 05:09 CST] **FE-05**（frontend 域「UI 控件」簇，commit f44b851）：实修 6 处——ProfileRow.tsx:78 setTimeout 无句柄 → savedTimer ref + 重存前 clear + 卸载清理；ProfileRow.tsx:110 busy 时 input 可编辑（busy 期击键被回填覆盖）→ disabled={busy}；FoldToggle.tsx:15 type=button；FoldToggle.tsx:21 aria-expanded + aria-label（aria-controls 不穿 id = 扩 scope 不做，spec 已声明）；RichText.tsx:22 锚点无 href → URL 真 href / 路径 role=link+tabIndex+Enter（与 FE-04 MarkdownText 同形态）；Fold.tsx:16 type=button + aria-expanded + useId aria-controls（同命中 medium type 缺失并处置）。OCR r1 0 comments（一轮清零）。test-all 首跑 duplicate_trigger_rejected_by_exec_guard 失败，隔离 + 全量复跑双过判 flaky（纯前端批无关），样本登记 §4。D2: files=8(+242/-4, 新测试 4 文件 +203) asserts=0→0 tests=vitest 91 相关绿 + 全量绿 + tsc 0。
- [2026-09-25 04:54 CST] **FE-04**（frontend 域「UI 渲染」簇，commit a68f104）：实修 5 处——SubtaskRow.tsx:48 readOnly flip 退编辑态（useEffect + input readOnly 属性空档兜底）；WorkspacePage.tsx:80 reload 冲乐观态 → writeInFlight/reloadQueued 守卫（guardedReload 排队 + resolve 期再查 + persist/handleSortEnd 全程包 begin/end）；WorkspacePage.tsx:99 就地突变 → changedIds 集合 + map 产副本 updatedAt，写成功才 emit；MarkdownText.tsx:21 String(children) 链接化收窄（仅 string/全 string 数组）+ 消 shadow；MarkdownText.tsx:34 可点 code 改 <a>（URL 真 href / 路径无 href + role=link+tabIndex+Enter）。FP 1 条（reviewer agent-15 pass，零代码）：SubtaskRow.tsx:27-36 stale closure——useInlineEdit 每渲染重建闭包 + 全仓无 memo 边界。OCR r1 5 comments 全同根因：high×2 采纳（sort emit 成功才广播 + reload resolve 期在飞检测）+ medium 采纳（路径 href 置 #）+ low×2 采纳（endWrite 负漂守卫 + 三元改 if/else）；r2 2 comments 0 high（r1 high 验证）：medium 采纳（路径链接不设 href + role/tabIndex/Enter 补键盘语义）+ low 采纳（persist emit 挪进 try 统一补发）。spec 校正 ×2（budget +110/-40→+140/-40→+160/-40，实码 +160/-36 压线）。D2: files=6(+191/-36, 新测试 SubtaskRow.test.tsx +31) asserts=0→0 tests=vitest 18 相关绿 + 全量绿 + tsc 0。
- [2026-09-25 04:23 CST] **FE-03**（frontend 域「setup / 缓存 / 截断」簇，commit 47f48d4）：实修 4 处——setup.ts:12 console.error 模块加载直赋改 beforeEach vi.spyOn（restoreAllMocks 自动闭环）+ 过滤收紧 /not wrapped in act\(/；setup.ts:37 Element.prototype scrollTo/scrollIntoView 全局补丁收进 src/test/scrollNoop.ts scoped polyfill（beforeEach 装 / afterEach 只删本套件装的，仅 ChatPanel/WidgetApp 两套件引入）；profile.ts:28 force=true 在飞被吞改 loadGen 代际（force 无视 loading 发新 invoke，旧 resolve 由代际挡下，清理收单 .finally 身份判定）；taskFiles.ts:9 MAX_TASK_FILES 跨语言锁步回归测试（读 tasks.rs 正则断言相等）。FP 2 条（reviewer agent-14 pass，零代码）：profile.ts:62 setters 无 catch（caller ProfileRow 全 try/catch + inline setError）+ taskFiles.ts:43 截断静默（caller TodoCard 已 alert「超出部分已忽略」）。OCR r1 4 comments 全同根因：high（原型补丁生命周期化仍违仓规）采纳=按 suite scope 重构，low×2 采纳（console 真身模块期捕获 / .finally 去重），low×1 不采纳有论证；r2 6 comments 0 high（r1 high 验证）：medium 采纳=jsdom-only 注释声明，low×2 采纳，low×3 不采纳有论证。spec 校正 ×2（budget +70/-25→+80/-40；OCR 驱动扩 scrollNoop.ts + expected_files 4→7 + budget→+110/-60 + new_files→160）。D2: files=7(+193/-36) asserts=0→0 tests=vitest 272/272 绿 + tsc 0 + test-all exit 0。
- [2026-09-25 03:49 CST] **FE-02**（frontend 域「React async 事件 race」簇，commit ba48f86）：实修 4 处——ArtifactBatchDialog.tsx:34 + ConfirmMap.tsx:37 listen/unlisten 竞态改 cancelled flag 模式（卸载早于 resolve 时自注销 + catch）；ArtifactBatchDialog.tsx:35 confirm 在飞被新批次覆盖 → 快照 batch + 功能性 setReady（cur===batch 才清）+ readyRef 错误归属 + busyRef 重入守卫；ConfirmMap.tsx:65 respond state busy 重入不安全 → busyRef 同步 check-and-set。OCR r1 5 comments 全同根因：high（respond finally 无条件 setPending(null) 杀新请求）实收 = 快照清理 + 回归测试，low 注释风格实收，low×2 listen 失败留痕不采纳有论证（弹窗未挂载无处渲染）；r2 5 low 全同根因 0 high（r1 high 验证）：抽 src/lib/useTauriListen.ts 共享 hook + readyRef 渲染期直写统一 + listener payload 防御。转 B 类 2 项（ArtifactBatchDialog:79 skip 语义 #20 + ConfirmMap:40 pending 覆盖 #21）。spec 校正 ×1（OCR 驱动：扩 useTauriListen + budget）。D2: files=5(+144/-33, 新文件 +156) asserts=0→0 tests=vitest 11 相关绿 + 全量绿。
- [2026-09-25 03:20 CST] **FE-01a**（frontend 域「core 写入加载路径」簇可自主部分，commit e2faf6a）：实修 4 处——storage.ts diffTaskRows upserts 全改新建副本（不再就地突变 caller 的 next 行，React 引用相等假设修复）；导出/导入 4 函数失败返 0+自带 alert → throw（storage 侧去 handleCommandError 防双弹，caller 本有 catch+onRetry）；loadWorkspaceFromDb 静默返 [] → LoadWorkspaceResult 判别式（ripple WorkspacePage.tsx:70 仅 ok 才 setItems / WidgetApp.tsx:191 失败 throw 进外层 catch，读失败不再清空工作区 UI）；App.tsx tasks-updated 守卫补 bot/api（实现 mutation.rs 文档化协议，bot/tools.rs:157 + api_handlers/commands.rs:116 实发），回写竞态关闭。FP 2 条（storage.ts:113 基线字段 types.ts:90 已有 + App.tsx:254 storage 层确有 alert，reviewer agent-12 pass）；转 B 类 2 项（App.tsx:240 半成功方向 #18 + App.tsx:191 迁移失败种子 orphan #19）。OCR r1 5 comments 全同根因：high（守卫缺回归测试）+ medium（守卫缺协议载体→新建 src/lib/mutationOrigin.ts 镜像 mutation.rs）+ 2 low（模块级常量 + KNOWN_SOURCES 集合判定）实收；1 low FP（WorkspacePage error 载荷——handleCommandError silent 仍 console.error）。spec 校正 ×2（budget +130/-45→+220/-80；expected_files 5→7 + max_new_files_lines 60）。**第 55 批，reviewer 抽查（agent-13）= pass。**D2: files=7(+198/-65, 含新文件 mutationOrigin.ts +31) asserts=0→0 tests=vitest 260/260 + tsc 0 + cargo fmt/check 绿。
- [2026-09-25 03:00 CST] **WA-03**（WidgetApp 域「dnd-kit 可达性/事件冲突」簇，commit 9b90ebd）：实修 2 处——SortableWorkspaceCard.tsx:23 render prop 改传合并手柄 props（attributes+listeners，wrapper 不再铺 attributes，caller WidgetApp.tsx:746 零改动，键盘可 tab 到 ☰ 手柄）；SortableTaskCard.tsx:55 useDndMonitor wasDragging 守卫（setTimeout(0) 复位，拖拽衍生 click 不再误切选中）。OCR r1 exit 0 / 1 low（onDragEnd/onDragCancel 重复 → 抽 resetIfSelf）已处置。新测试 sortable-a11y.test.tsx 3 passed；**WidgetApp 域 3 簇全清。**D2: files=3(+41/-6, 新测试 +92) asserts=0→0 tests=vitest 3 新增。
- [2026-09-25 02:53 CST] **WA-02**（WidgetApp 域「window listener 管理缺」簇，commit 328fa76）：实修 3 处——ResizeEdge.tsx:43 Promise.all 加 .catch（unhandled rejection 消除，拖动可恢复）；ResizeEdge.tsx:72 + SplitBar.tsx:15 cleanup 签名 `(ev?: PointerEvent)` + useDragCleanup 卸载兜底。OCR r1 exit 0 / 5 comments 全同根因同批处置（抽 useDragCleanup.ts 共享 hook ×2 medium + catch 路径补测试 medium + 直赋值 ×2 low），无 high。新测试 drag-cleanup.test.tsx 7 passed（含 catch 路径）；spec 校正版 d2ace11。D2: files=4(+34/-14, 新文件 +149) asserts=0→0 tests=vitest 7 新增。
- [2026-09-25 02:45 CST] **WA-01**（WidgetApp 域「输入/反序列化校验缺」簇，commit 919aa0e）：实修 2 处——storage.ts:113 anchorFromRect 返回扁平 Anchor（ripple WidgetApp.tsx:285/:325 两 call site 简化）；storage.ts:79 loadAnchor 形状校验（object 非 null 非数组 + x/y finite + edge allow-list），坏值返 null 对齐 loadSize。OCR r1 exit 0 / 4 comments（2 medium 命名 na→anchor + 1 low EDGES as const + 1 low 补 Array.isArray 排除，全同根因同批处置，无 high）。新测试 storage.test.ts 13 passed；既有 WidgetApp.test.tsx 6 passed 不退化。assertions_min 校正 10→0（gate 计数只认 Rust assert! 宏，vitest expect() 由实跑担保——前端批通行先例）。D2: files=3(+36/-20, 新测试 +93) asserts=0→0 tests=vitest 13 新增。
- [2026-09-25 02:34 CST] **SC-05**（scripts 域「hook 覆盖不全」簇，commit 66fbbec）：实修 1 处——test-fast.sh:56 步骤 0 审计防线补 untracked 全文扫描（`git ls-files -z --others` 并入扫描流，同 audit-ok 豁免；同步段注释/docblock/横幅口径）。非 B 类（不改约定，补执行漏洞）。验证：本仓库步骤 0 三场景（untracked 坏=exit 1 / audit-ok=放行 / 干净=过）。OCR r1 exit 0 / 0 comments（scripts/** exclude）。**scripts 域 5 簇全清。**D2: files=1(+15/-5) asserts=0→0 tests=n/a。
- [2026-09-25 02:31 CST] **SC-04**（scripts 域「第三方/CDN/用户输入未校验」簇，commit 114440b）：实修 2 处——fetch_ocr_models.sh:49 嵌入仓内 pp-ocr-v6/ 实测 SHA256 pinning 清单（det/rec/cls/keys），partial 在 mv 前比对，不匹配/不在清单/无 sha256 工具全 fail-closed；publish-docx-dotnet.sh:12 OUT_ROOT 校验拒空/绝对路径/含 `..`/`-` 开头 + rm/mkdir 加 `--`。验证：stub curl SHA256 矩阵（错=1 对=0 跳过=0）+ OUT_ROOT 注入矩阵（五非法全拒、正常放行）+ macOS BSD `--` 兼容实测。OCR r1 exit 0 / 0 comments（scripts/** exclude）。**第 50 批 reviewer 抽查（agent-11，只读 git show + 文件原文 + 自算 SHA256 比对）：pass。**D2: files=2(+47/-2) asserts=0→0 tests=n/a。
- [2026-09-25 02:25 CST] **SC-03**（scripts 域「set -euo 边界未封」簇，commit 9047712）：实修 1 处——install-hooks.sh:10 裸 glob → `shopt -s nullglob` + 数组收集 + 空数组 if 守卫（`&&` 链在 set -e 下整体返 1 会中止，故用 if）。验证：tmp 仓库两场景（.githooks 空 exit 0 / 全在 +x 生效 + hooksPath 写入）。OCR r1 exit 0 / 0 comments（scripts/** exclude）。D2: files=1(+6/-1) asserts=0→0 tests=n/a。
- [2026-09-25 02:23 CST] **SC-02**（scripts 域「非原子/非完整」簇，commit 1227af9）：实修 3 处——sync-version.mjs:20 就地 writeFileSync → .tmp + renameSync 原子替换；fetch_ocr_models.sh:44 curl 直写目标 + `-s` 跳过 → `$out.partial` 校验后 mv 就位；:53 curl `-f` 盲信 → mv 前大小下限（onnx≥1MB / keys≥10KB）+ 首字节非 `<` 校验。验证：stub curl 对抗矩阵 9/9（HTML 页 / 小文件 / 双源 fail / 已存在跳过 / partial 零残留）。OCR r1 exit 0 / 0 comments（scripts/** exclude）。D2: files=2(+31/-6) asserts=0→0 tests=n/a。
- [2026-09-25 02:18 CST] **SC-01**（scripts 域「shell 解析脆」簇，commit 9629fe9，spec 703b949）：实修 2 处——ci-guard-tiny-http-vendor.sh:27 grep 加行首锚 `^[[:space:]]*`（注释行不再误判通过）；:46 case glob 双端锚定 `directory+file://*/src-tauri/vendor/tiny_http`（同名后缀目录 / 非 directory scheme 不再误判通过）。sync-version.mjs:11 high → **FP**（finding 前提错误：正则字面 `\]` + `[^[]*?` 跨不过 section 头，node 对抗实证 + reviewer agent-10 pass），零代码处置。验证：对抗用例 7/7 过；guard 本机 check #2 既有环境红（cargo metadata 提取不到 source），与改动无关。OCR r1 exit 0 / 0 comments（scripts/** exclude，0 为正常）。D2: files=1(+4/-3) asserts=0→0 tests=n/a。
- [2026-09-24 00:02 CST] MI-04a 收口（commit 414cf08）：C5-MI-04a（journal_pending 无条件 INSERT → 同 key 孤儿 pending）已修——持锁内 check-then-reuse（零 schema 变更；UNIQUE partial index 选项因既有库可能含重复行会建索引失败而弃）。OCR r1 1 low（双写分配）采纳。既有库孤儿 pending 清理 = 数据迁移方向，B 类攒批。migration 域剩 3 簇：MI-04b、MI-05a/b、MI-08。
- [2026-09-24 00:20 CST] MI-04b 收口（commit 5819f76）：C5-MI-04b（journal find+act TOCTOU）已修——核后真实竞态对 = spawn_polling 启动 replay（无 MigrationGuard）vs run_migration（有）；修法 = replay 挂守卫（不取 try_claim 状态机变更；DB_WRITE_LOCK 包 find+act 因 db_upsert 重入死锁不可取）。OCR r1 抓回本批自引入 critical（guard 作用域泄漏会永久锁死后台迁移）→ 修复 + r2 验证 0 critical/0 high。migration 域剩 2 簇：MI-05a/b、MI-08。
- [2026-09-24 00:40 CST] MI-08 收口（commit bbe4f59）：C5-MI-08 全簇 2 条已修——CSV 表头 contains → 别名集精确匹配 + 重复列拒绝；archive_dir `..` 组件拒绝（resolve_archive_dir 单点 + validate_rules 导入期早错）。OCR r1 5 comments：3 采纳（注释行为不一致 / 别名展示 / 契约钉测试），2 挂起（symlink 逃逸 → 并入 B 类绝对路径 policy 项；破坏性变更用户提示）。**migration 域 10 簇全清**（MI-05a spawn_blocking 无 abort + MI-05b sync block_on 转入 B 类/待核区——见 §2 标注）。

- [2026-09-24 01:00 CST] AP-03 收口（commit a78de81）：C5-AP-03 全簇 2 条已修——ratelimit.rs 访问日志 4 字符 replace 链 → sanitize_log_line 全控制字符转义（is_control + U+2028/U+2029 显式臂，\r\n\t\0 短形式不变）；并发写撕裂 → static LOG_WRITE Mutex 包 rotate+append 整段（C3-1 poison 形态）。OCR r1 4 low：3 采纳（2028/2029 两臂 + 测试扩展 + capacity×2），1 不采纳（锁注释——静态文档注释已覆盖）。budget 校正一次（+60→+70，OCR 采纳所致）。新发现登记 PHASE2-TRIAGE-NEW-3（audit.rs 同族两站，见 §3.5）。spec budget 校正 6dd509b / spec 立项 3d43265。

- [2026-09-24 01:35 CST] BT-01c 收口（commit eb11651）：C5-BT-01a 余 2 条已修——skill_finish 零迁移可见化（reason 非空闸门：传 reason=期待迁移记 skill_finish_no_transition，空 reason=清理性调用不记；OCR r1 抓回"普通聊天路径每轮误记"high→修、r2 抓回 session_has_run 形态误记→改 reason 门、r3 补三态门单测）；find_due_tasks 两处 db_upsert 失败 audit_log 留痕。budget 二度校正（+40→+50→+80，全 OCR 驱动，同 MI-04b 形态，并入 META-8 待拍材料）。PROC-5 type 1 复发 n=6→n=9（r2×2 + r3×1，均 file_read start>end，review 本体 complete）。spec 立项 13d194a / 校正 1bd8a92 / eeed791。

- [2026-09-24 02:05 CST] AP-02 收口（commit 5d8e878）：C5-AP-02 已修——write_token_file 改 tmp+rename 原子写（pid+seq 唯一 tmp 名 + create_new + mode(0o600)，rename 不跟随 symlink；写/rename 失败均清 tmp）。OCR r1 2 medium + 2 low 全同根全采纳（rename 清 tmp / 并发撕裂 tmp 名 / create_new 保 mode / 错误带路径），r2 0 comments 验证。budget 校正一次（+70→+95）。PROC-5 type 1 复发 n=9→n=11（r1×2 均 file_read start>end）。

- [2026-09-24 07:31 CST] BT-03a 收口（commit f86fc7a）：C5-BT-03 五取四已修——bot-config.json 五条写路径全改 write_config_atomic（tmp {name}.{pid}.{seq} 唯一名 + fsync + rename + 失败清 tmp + cfg(unix) 父目录 fsync best-effort）+ 新增 CONFIG_WRITE_LOCK 全局写锁（ConfigWriteGuard must_use + 线程本地持锁标记 + debug_assert，照 db::lock_db_write 先例）杀 RMW 竞态；四个持锁写路径进段先调 migrate_bot_config_schema_locked（OCR r4 high2 结构性修复：钩子锁内只 try_lock 让路，不推则 RMW 跑旧 schema）；schema 迁移拆加锁外壳/locked 内核消 SCHEMA_MIGRATION_CHECKED check-then-act 窗口。commands.rs:86（keyring 先于文件写无回滚 = 半成功陷阱方向）转 B 类攒批第 9 项。budget 二度校正（+120→+170→+215，均 OCR 驱动，同 MI-04b/BT-01c 形态，并入 META-8 待拍材料）。PROC-5 type 1 复发 n=11→n=15（r3×3 + r4×1，均 file_read start>end，review 本体 complete）；另 r3 有 1 条 file_read「路径不存在 db.rs」= 非 type 1 新形态，登记观察。spec 立项 / 校正 4f130b1 / 5150af4。

- [2026-09-24 07:50 CST] BT-01d 收口（commit c55d35d）：C5-BT-01b 残余 1 条 + OCR-001/002 已修——tool_search_tasks（triage 标 :280 现 :286）db_load().unwrap_or_default() → let-else 显式「搜索失败：数据库读取错误」（1e023e3 spec 未覆盖该站点，残余来源登记）；find_task_by_keyword 两 call site（:551/:578）`.await?` → map_err DbError 对齐（Internal/DbError 双 code 统计漏算收口）。family 归 error-not-propagated（按 §1 判据该条是信号丢失非数据破坏，与 1e023e3 commit 所标一致）。OCR r1 1 low（spec 文案与代码不一致）→ 采纳=校正 spec 对齐代码（零代码 churn），r1 tool failure 0。**C5-BT-01b 簇全清**（:147 前批 + :280 本批）。spec 立项 + 文案对齐各 1 commit。

- [2026-09-24 08:10 CST] BT-06 收口（commit 841922b）：C5-BT-06 判定 **OCR FP×2**——`truncate_for_log` 自 dcbf167（2026-09-14，早于 0921 全扫）起为 `escape_for_log` 别名（bot/config/audit.rs:74），`\n\r|` 已转义；finding 前提与实现不符。与 BT-02 同处理（记录 + 不改行为）。FP 诱因 :1081 陈旧注释已改述（+1/-1，唯一代码改动）。OCR r1 0 findings。实工单口径 142→140。bot 域簇剩余：BT-04/05/07/08/09。

- [2026-09-24 08:35 CST] BT-07a 收口（commit 6bd43c0）：C5-BT-07 拆批 1/3——scheduler.rs:149 回滚复原 Drop 兜底（RollbackRestoreGuard：reopen 后 armed，正常路径手动 restore 后 disarm，panic 路径 Drop 复原 Failed 终态，AtomicGuard 放行窗口不再永久化）。新增 panic 路径单测（mock AppHandle + catch_unwind，4 断言）。拆批登记：bot_artifacts.rs:113 疑似 FP 下批先核；state.rs:164 转 B 类攒批第 11 项。budget 校正一次（+70→+80）。OCR r1 0 comments（tool failure 0）。**第 15 批，reviewer 抽查 = pass**（explore 子 agent 只读 git show/spec/OCR json 一手材料）。

- [2026-09-24 08:50 CST] BT-07b 收口（纯判定，零代码）：bot_artifacts.rs:113 判定 **OCR FP**——finding 核心断言「upsert_tasks does not compare expected_updated_at」经原文核实为假（db/tasks.rs:199-224 显式 SELECT 比对 + CONFLICT_ERR_PREFIX 拒写 + ON CONFLICT WHERE 谓词 + affected 行数双保险）；confirm_artifact_batch :127 设置 expected_updated_at 快照基线，并发窗在写时被拒（Err「绑定失败」），非 silently。与 BT-02/BT-06 同处理：记录 + 不改。实工单口径 140→139。C5-BT-07 剩 state.rs:164（B 类）。

- [2026-09-24 09:10 CST] BT-09a 收口（commit b1c0b4f）+ BT-09b 收口（commit 3f62d74）：**C5-BT-09 全清**。:283 Finish 早退假完成——finish_signal 标记分流：summary 标题/skill_dsl_done 审计改 results.len() 实执步数、早退追加介入说明行、skill_finish 仅自然跑完路径调（早退路径 run 已外部置 Completed，再调误记 no_transition）；outcome kind 保持 done（外部终态属实）。:248 索引复用（tool_def 存 lookup 结果，杀 TOOLS_TABLE.iter().find 二次线性扫；use 移除 TOOLS_TABLE）。:424 wontfix-with-rationale（OCR-003 同型设计意图）。两批 OCR r1 均 0 comments / tool failure 0。

- [2026-09-24 08:20 CST] BT-05 收口（commit 51efb80）：C5-BT-05 四取三——io.rs:122 base_url_is_safe 改 url::Url::parse + Host 精确判定（localhost.evil.com / 127.0.0.1.evil.com / [::1].evil.com 前缀绕过全杀，fail-closed）；bot_web:21 降级 client 钉 60s 硬超时 + Policy::none() + 留痕（原降级 = 无超时 + 自动重定向，SSRF 防护全架空）；:730 fetch_text 拒 https→http 降级重定向。:881 转 B 类第 12 项（Jina 审计需签名改）。OCR r1 0 comments；PROC-5 type 1 复发 n=15→n=18（r1×3 均 file_read start>end）。bot 域簇剩 BT-04 / BT-08。

- [2026-09-24 08:30 CST] BT-04a 收口（commit ca73ec0）：C5-BT-04 拆批 1/2——bot_chat.rs:399 图片白名单校验/读取同一化（read(&canon) 不 read(p)，symlink 掉包窗关闭）；bot_artifacts.rs:67 确认并发窗晚登记产物重注册不静默丢。OCR r1 1 low（O(n·m) membership）不采纳（OCR 自认小批量可接受）；type 1 n=18→n=19（r1×1）。拆批：bot_fs.rs:167 + tools.rs:102（spawn_blocking 方向）下批 BT-04b 细评。

- [2026-09-24 ~09:30 CST] BT-08a 收口（commit e7b2381）：**C5-BT-08 全清（2 实修 + 1 FP + 1 B 类 + 1 wontfix）**。types.rs:82 三 key 字段 skip_serializing（永不序列化，fail-closed 纵深，零行为变更）；parse.rs:86 非法 mode 按未声明处理（low→auto 提升不被吞，新增 3 断言测试）。:198 FP（deepseek-v4-flash 旧名仍被接受，web 实证）；:168 B 类第 13 项；:76 wontfix（结构必然）。OCR r1 1 low（mod.rs:541 注释机制名旧）已修同批；tool failure 1 = 新形态 code_comment 空数组拒收（良性，登记观察）；type 1 n=19 不变。spec 两处执行中校正（零代码 findings 移出机器可读数组；expected_files 加 mod.rs）。**bot 域 C5 簇剩 BT-10 / BT-11 / BT-12 + BT-04b 半簇。**

- [2026-09-24 ~10:15 CST] 抽查登记（每 5 批抽 1）：**BT-04a（commit ca73ec0）reviewer 抽查 pass**（第 20 批漏抽补跑，只读 git show + spec + OCR json 原文核验）。

- [2026-09-24 ~10:15 CST] BT-04b 收口（commit 93182f3）：**C5-BT-04 全清**。bot_fs.rs:167 剩余未包点收尾（allowed_dirs 尾部 gen_dir+canonicalize 循环整体进 spawn_blocking_io；is_dir 单次 stat ×4 走新 helper is_dir_async；C1b 已包 4 点不动）；tools.rs:102 sanitize_task_files_arg async 化（gen_dir+canonicalize+sanitize 内核进 spawn_blocking_map，JoinError 全丢+记审计，fail-closed 一致）。零行为变更。OCR r1 1 medium（JoinError 吞审计）+2 low（gen_dir 闭包外 / expanded.is_dir 漏包）已修，1 low（clone PathBuf）不采纳（'static 约束既有模式）；type 1 n=19→n=23（r1×4）。TOCTOU 维度归 Phase 6 既有 follow-up。**bot 域 C5 簇剩 BT-10 / BT-11 / BT-12。**

- [2026-09-24 ~10:50 CST] BT-10a 收口（commit a4e3bf1）：C5-BT-10 拆批 1/2——audit.rs:22 data_dir+rotation 移出 BOT_LOG_LOCK（锁内只剩 open+append）；runtime.rs:83 start_skill 锁内状态迁移+收集，锁外发审计；:393 skill_finish 锁内 FinishPayload 快照，锁外 load_skill_meta+钩子（迭代顺序/末 run 覆盖语义不变）。零行为变更。OCR r1 0 comments；type 1 n=23→n=25（r1×2）+ code_comment 空数组 ×1（良性）。:468 scheduler 韧性拆 BT-10b。**bot 域 C5 簇剩 BT-10b / BT-11 / BT-12。**

- [2026-09-24 ~11:10 CST] BT-11 收口（commit b1d9212）：**C5-BT-11 全清**。find_tag_open 开标签精确匹配（后字符须空白/>//），first_between/first_open_tag 换用，杀 <a 误中 <abbr>、<p 误中 <pre>；行为修复 abbr 前置时标题/链接错位，正常形态不变。OCR r1 0 comments / 0 failure；type 1 n=25 不变。**bot 域 C5 簇剩 BT-10b / BT-12。**

- [2026-09-24 ~11:35 CST] BT-12 收口（docs-only）：**C5-BT-12 全清（6 条全 stale，零代码）**。核 C2b-2（95a25e9，2026-09-21 20:39）晚于 0921 全扫快照：:0 kind 白名单（写入侧 ALLOWED_LINK_KINDS + 消费侧仅 {file,folder}）+ :43 归一化（绑集 canonical 双侧）已修；另 4 条漏分簇（:0 is_dir 死参数已删 / :10 静默收缩已带 audit+fail-closed / :83 TOCTOU 已有 recheck_canonical / :144 已走 path_openable_in）同判 stale。**reviewer 抽查 pass**（OCR high 判 stale/FP 按规则触发独立核验；兼作第 25 批抽查）。§3 待核项同步消解。**bot 域 C5 簇剩 BT-10b + BT-13 / BT-14 / BT-15。**

- [2026-09-24 ~11:55 CST] BT-13 收口（commit 11cf4f7）：**C5-BT-13 全清**。sanitize_fail_reason（剥控制字符/拆 ``` 序列/截 500）+ 围栏 + 「数据非指令」标注，replan fail_reason 注入面闭合。OCR r1 0 comments / 0 failure；type 1 n=25 不变。**bot 域 C5 簇剩 BT-10b / BT-14。**

- [2026-09-24 ~12:30 CST] BT-10b 收口（commit 455c7c1）+ BT-14 处置：**C5-BT-10 全清**。:468 tick 层 catch_unwind（sched_tick_panic 审计后续跑）+ per-task 层 catch_unwind（spawn-and-forget panic 落 sched_task_panic + set_bot_assigned 兜底）+ Semaphore(4) 并发上限 + acquire 失败审计；关闭信号 YAGNI。OCR r1 2 medium 一修一不采纳（AssertUnwindSafe=middleware.rs:87 同族既有模式）+ 3 low 修 + 1 low 登记 NEW-4（panic_message 4 处重复）；type 1 n=25 不变。BT-14（StopGuard 接线）**转 B 类攒批第 14 项**——底层 fn 仅 tool_run_python 接受 stop（已接），全接线=13+ 签名改扩 scope；循环级 4 处 stop 检查已覆盖「排在长调用后」场景；A/B/C 待拍。**bot 域 C5 簇全清（BT-14 为 B 类待拍项）。**

- [2026-09-24 18:55 CST] AP-01b 收口（commit c49bba3）：C5-AP-01 拆批 b——api_auth.rs:27 token 首建改 create_token_file_atomic（OpenOptions create_new + unix 0600 直写最终路径；Ok(true)=胜者返回自己的 token，AlreadyExists=输家 50×10ms 空窗重试读胜者 token，仍空 Err 下次自愈）；api_server.rs:208 worker 上限改 fetch_add 原子占位（超限立即 fetch_sub 归还 + 503，删 load+fetch_add 两步竞态）。行为变更：并发首跑输家从「返回与磁盘不符的 token」变「读胜者 token」（单进程零变化）；worker 计数严格 ≤ MAX_WORKERS。同批拆出 commands.rs:61/:238（flag 写 vs 内存状态 TOCTOU，涉「文件 I/O 不持锁」既有约定）→ B 类攒批第 15 项。OCR r1 3 comments（1 medium fsync 耐久性→挂起登记 §4，与模块既有姿态一致；2 low 注释采纳）；tool failure 0；type 1 n=25 不变。spec 立项 76e619e / budget 执行中校正 5512228（+48→+68，numstat 预估偏低规律）。**C5-AP-01 剩 commands.rs 2 条（B 类待拍）；api 域剩 AP-04/05/06/07。**

- [2026-09-24 19:15 CST] AP-04 收口（commit 8d49a26）：**C5-AP-04 全簇 2 条已清**。sse.rs:196 写无超时 → vendor tiny_http accept 级 set_write_timeout patch（HTTP_WRITE_TIMEOUT_MS 30s 单次 write syscall 级；照 10acf42 读超时先例走 PATCHES.md 治理：§2 共 2 处→3 处 + §2.3 + §5 记录；lib.rs 读超时注释「不影响写」过时句修订）——**vendor 改动非 architecture_blocker（既有治理流程）**。sse.rs:183 乱序投递根因属实（fetch_add 锁外 + try_send 锁内）→ broadcast 单临界区化（clients 锁内 fetch_add→落盘→history→推送；锁序 clients→history 全仓唯一嵌套点已核无死锁对）；顺带修 id 落盘并发乱序 + clients poison 改 C3-1 形态；sse.rs replay_max 快照 + 异常丢弃留痕（每连接一次）；契约双侧文档化；新增确定性并发回归测试。写超时行为级测试需填 kernel buffer（flaky）不加，spec 已声明。OCR r1 6 comments（0 high）：采纳 4（Value 序列化移出锁 / SeqCst 注释 / replay_max 语义注释 / 留痕每连接一次），不采纳 2 有论证（落盘移出锁 / 测试封装重构）；tool failure 0；type 1 n=25 不变。spec 立项 9833abb。**api 域剩 AP-05/06/07。**

- [2026-09-24 19:40 CST] AP-05 收口（commit bfe30b9）：**C5-AP-05 全簇 3 条已清**。create_task:277 / update_task:405 / delete_task:561 持锁跨 I/O 统一改 labeled block（'rmw）装盒 Result、出锁后 match 分流响应——锁内只做 DB 读写。行为逐项等价（核后保持 create upsert 失败走 internal_err 500 原分流；delete 幂等重删 200 不 after_change 不变）。实现形态校正一次：闭包 IIFE 会重缩进 ~90 行 → 换 labeled block（同一函数内三处风格统一，diff 收窄到 +67/-26）。不加新测试（纯结构性重排，mod.rs HTTP 级测试已覆盖全分支）。OCR r1 1 low（AlreadyDeleted 去 Box）采纳；tool failure 1 = code_comment 空数组拒收（良性形态 n=3）；type 1 n=25 不变。spec 立项 3d4a2c1。**第 30 批，reviewer 抽查 = pass**（explore 子 agent 只读 git show/spec/OCR json 核验：状态码路径/锁释放时机/budget/commit message 与 diff 一致性）。**api 域剩 AP-06/07。**

- [2026-09-24 ~19:55 CST] AP-07 收口（commit 3d6b1a8）：**C5-AP-07 单条已清**。api.rs:101 sync 桥接隐式约定 → assert_sync_bridge_caller（tokio Handle::try_current 检查，Cargo.toml tokio 加 rt feature 非新依赖）+ trait doc 契约段；不用 catch_unwind 包 block_on（误吞 db fn 自身 panic 误标）；不做 async trait 改造（扩 scope）。行为变更仅限 runtime 线程误调用的 panic 消息点名约定，现状调用路径零变化。新增 2 测试（runtime 内 panic 点名 / 普通线程放行）。OCR r1 0 comments / 0 failure；type 1 n=25 不变。spec 立项 700b5d3。**api 域剩 AP-06（与 DB-01a-3 合批）。**

- [2026-09-24 ~20:05 CST] AP-06 + DB-01a-3 状态核清（docs-only，零代码）：**两条均已在历史 commit 修复，§2 行漏标**。AP-06 已由独立批 0b6ee69（2026-09-23，error-visible-non-blocking）修复——atomic_write 失败 eprintln 留痕（当前代码 api_server.rs:127-129 即是）；DB-01a-3 已由首批补全 f93f4a9 修复——reset_bot_assigned_with bool→Result + open_db `?` 传播（当前代码 mod.rs:266 即是，§3.5 修复路径已落地）。**C5-DB-01a 全簇 3 条已清（1fcc418 ×2 + f93f4a9）；api 域 7 簇全清**。db 域剩 DB-03 / DB-05（B 类候选 3 条）。OCR 对 AP-06 残留的进一步建议（重试 / refuse-to-advance）属语义方向 → 并入 B 类攒批第 16 项（A=写失败拒推进 id（fail-closed，SSE 事件暂停）/ B=瞬时错误重试 ×N / C=维持 eprintln 现状）。

- [2026-09-24 ~20:25 CST] DB-03 收口（commit 59f81c8）：**C5-DB-03 全簇清（1 实修 + 1 FP）**。mod.rs:61 legacy 拷贝段加 LEGACY_COPY_LOCK 进程内锁 + 锁内双检（照 DB_WRITE_LOCK 先例，仅护首装一次性窗口）；tasks.rs:396 判 **OCR FP**——「load_all 多语句读」前提不实（单条 SELECT，subtasks/files 同行 JSON 列，WAL/rollback 下单语句均快照一致），与 BT-02/BT-06/BT-07b 同处理。**FP 判定 reviewer 独立核验 = pass**（explore 子 agent 只读原文：load_all 单语句 + diff/spec 一致 + 双检逻辑）。budget 校正一次（+20/-8→+28/-18，双检嵌套重缩进 ~13 行未计入 A 类估——**A 类估教训：包裹嵌套必带重缩进，预算按重缩进行数计**）。OCR r1 0 comments / 0 failure（13s）；type 1 n=25 不变。spec 立项 d5f0258 / 校正（budget）。实工单口径 139→138。**db 域 7 簇全清（DB-05 余 3 条均 B 类候选）。**

- [2026-09-24 20:03 CST] EV-3a-01 收口（commit dcd4808）：**C5-EV-3a-01 单条已清**。observe/stop.rs:82（现 :88，C3-3 注释块致行漂移）静默吞负值 → `days_elapsed` 改 `now_ms.saturating_sub(start_ms)`（i64 溢出饱和→时间门正常响，不再被 max(0.0) 静默压零）+ 负时长 eprintln `[evolution_stop]` 留痕（仍计 0.0 天不误触发）。新增 2 测试（负时长 0 天不停 / i64::MIN 溢出饱和触发 FourteenDaysElapsed）。签名未动——Result 化 = 调用链变更 = B 类，spec 已声明不自决。实际 diff +28/-1 恰好压 budget 线（注释压缩三轮）。OCR r1 0 comments / 0 failure；type 1 n=25 不变。spec 立项 e1c4199。**evolution 域 3a 剩 EV-3a-02/03。**

- [2026-09-24 20:30 CST] EV-3a-02 收口（commit 1992904）：**C5-EV-3a-02 全簇 2 条已清**。① rewrite_jsonl:59 非原子重写（truncate+逐行写，崩溃留半截）→ 全量序列化 + `db::paths::atomic_write`（tmp+rename，复用 api_server/api_auth 既有模式；fsync 不超仓规不加）；② promote:292 TOCTOU（toggle_inner 与锁外 load_changes+find 两窗口，并发 delete 致误导性错误「toggle_inner 写完未找到 ChangeRecord」）→ toggle_inner 私有签名改 `Result<Option<ChangeRecord>>`（锁内返回写入/dedup 记录），promote 删锁外重读；调用点 3 处全在本文件无 ripple。行为变更：崩溃半截窗口消除 + 误导错误不可能再现；OFF/reject/toggle 语义不变。新增测试 1（rewrite 原子形态无 .tmp 残留）。budget 校正一次（+55/-40→+75/-60，toggle_inner if/else→match 重构重缩进超估，同 DB-03 教训）。OCR r1 3 low（2 条空行已修 + 1 条 c.clone() 风格建议不采纳——OFF 分支仍需 changes.retain，into_iter 不可行）/ 0 failure；type 1 n=25 不变。spec 立项 8bac6ae / 校正 fe287e4。**第 35 批，reviewer 抽查 = pass**（另揪出 commit message tests 计数 8→9 实为 7→8，amend 修正——HEAD 未 push 已核）。**evolution 域 3a 剩 EV-3a-03（metrics.rs O(n*m)，1 条）。**

- [2026-09-24 20:41 CST] EV-3a-03 收口（commit 46fffe5）：**C5-EV-3a-03 单条已清，evolution 3a 三簇全清**。metrics.rs:86 污染存活期 O(changes×applied) 嵌套扫描 → 循环前建 `HashMap<&str, &AppliedRecord>`（mem_key 索引）O(n+m)；`entry().or_insert()` 保「首个匹配」语义（与 iter().find 一致，重复 mem_key 不漂移）。零语义变化（纯函数内部重排）。新增测试 1（重复 mem_key 取首条回归）。diff +25/-1 在 budget（+40/-10）内。OCR r1 0 comments / 0 failure（13s）；type 1 n=25 不变。spec 立项 cf373a9。同文件 :60/:66/:68/:86 的 medium/low（窗口边界 / max(1.0) / 时间口径 / 未来日期静默跳过）归 3b 簇，本批未动。**evolution 域剩 3b 11 簇 35 条。**

- [2026-09-24 21:05 CST] EV-3b-A 收口（commit 31fee88）：**簇 5 条中 3 条已清，2 条转 B 类**。① apply.rs:151 embedding 失败塌 None 静默 → collect 后统计，>0 走 `audit_event! evolution.embed_failed`（Warn，落 bot.log）；② activation.rs:136 双 loader 「存在但坏」（IO≠NotFound / parse 失败 / schema 不匹配 / state 非字符串或未知值）eprintln `[evolution_activation]`，缺文件/缺块/缺 key 保持合法静默；③ mod.rs:55（triage :48 漂移）app_handle None 落盘块静默跳过 → else eprintln（audit_event! 需 AppHandle，None 分支只能 stderr，注释已注明）。零行为变化（纯留痕）。**record.rs:211 + entry.rs:110（read_all 单行损坏全读失败）修法 = fail-closed vs fail-open 语义方向 → B 类攒批第 17 项**。budget 两次校正（+45/-15→+75/-20→+85/-25：双 loader 分支文案 + OCR r1 采纳增量）。OCR r1 6 comments（1 high 采纳=自身新策略一致性缺口；2 medium 采纳；1 medium + 2 low 不采纳/stale 有论证）/ 0 failure；type 1 n=25 不变。spec 立项 e2bd744 / 校正 ×2。**evolution 3b 剩 10 簇。**

- [2026-09-24 21:29 CST] EV-3b-B 收口（commit c0472de）：**C5-EV-3b-B 全簇 5 条已清**。核心动作：EVOLUTION_STORE_LOCK 从 panel/commands.rs 上移到 evolution/mod.rs（pub(crate)，C3-4 一把锁约定不变），post_consolidation 的 write_proposals 包锁——**消除「panel 持锁 vs consolidate 无锁」两套机制互踩的真 race 面**；entry/record append 整行单次 write_all（POSIX O_APPEND 单写原子 + 锁契约注释）；sandbox/io 两个 append 收敛 append_line（单写 + SANDBOX_IO_LOCK）；emit.rs dedup 锁瘦身（audit_event! 移出锁，dedup 标记先于 audit 写=崩溃窗口对称互换）；save_state 加 SAVE_STATE_LOCK + db::paths::atomic_write。不引 fs2 新依赖（跨进程 flock 残余注释声明）；fsync 不加（照 AP-01b 挂起先例）。OCR r1 4 comments（0 high：1 medium 部分采纳=锁收窄仅 write_proposals，「sync 持锁于 async 链」不改=同步 I/O 先于本批存在 + caller spawn_blocking 化是跨域 ripple 不自决；3 low 采纳）/ 0 failure；type 1 n=25 不变。spec 立项 a25e80e。diff +105/-54 在 budget（+110/-85）内。**evolution 3b 剩 9 簇（C1/C2/C3/D/E/F/G/H/I）。** follow-up：bot-config 跨写者统一锁（bot/config/io.rs 不在 SAVE_STATE_LOCK 内）。

- [2026-09-24 21:43 CST] EV-3b-C1 收口（commit f72e253）：**C5-EV-3b-C1 全簇 3 条已清**。① mapping.rs:37 approval_source 由 resolved status 派生（Rejected→SystemRejected，Expired 保持 Pending 注释声明=无对应变体）——**既有测试 to_change_record_rejected_entry 锁的正是缺陷值，已改断言**；② status.rs:36 取轻量选项：doc 收紧为「纯拓扑表 + skip-canary 策略门属调用方契约」（加配置入参=签名改=B 类不做），Active 无 Rejected 出口写明有意；③ kill_switch.rs:39 should_auto_apply 改 `!all_auto_apply && !shadow_only`——**零生产调用方**（apply.rs auto_apply_gate 不查 kill switch，finding 所述「apply.rs 只看 should_auto_apply」当前不成立，修的是 latent API 陷阱），自有测试注释已述意图未断言 → 补断言锁死。OCR r1 1 low 采纳（approval_source 收敛单 match）/ 0 failure；type 1 n=25 不变。spec 立项 819fd14。diff +21/-12 在 budget（+55/-15）内。**evolution 3b 剩 8 簇（C2/C3/D/E/F/G/H/I）。**

- [2026-09-24 23:41 CST] EV-3b-I 收口（零代码处置）：**C5-EV-3b-I 已被 EV-3b-C3 覆盖清**。finding（routing.rs:110 缺 canary×A/B 联合分布断言）要求的测试正是 C3 新增的 `ab_orthogonal_to_canary`（canary 群内 A 占比 ≈0.5，区间 [0.35,0.55] 收紧到能卡住 buggy 回归值 0.589）——C3 把 A/B 改独立加盐 hash（ab_bucket = fnv1a("ab:"+id)）时一并落地。核实测试原文在 routing.rs:132-145。**零代码，无 batch commit，triage 直接收口。****evolution 3b 全 11 簇清零。**

- [2026-09-24 23:38 CST] EV-3b-H 收口（commit d4ad243）：**C5-EV-3b-H 全簇 3 条已清**（+1 同 family medium 一并）。① aborted 双轨坍塌：删 was_aborted 字段+setter，TraceOutcome::Aborted 枚举唯一事实源（ripple: bot_chat.rs 删一行，唯一 caller 本就两轨同源=latent）；② should_record_trace 删 bool 入参，Aborted 恒记录（/stop trace 不再静默丢）——**既有测试 :328 锁旧缺陷行为已翻正**；③ TraceContext 加 tool_calls vec + setter + forward（audit payload 计数不再恒空）——producer 接线（run_model_loop 返回类型）=跨域登记 follow-up。附带 :214 emit outcome 改 serde_json 规范形（消 Debug/serde 双轨）。OCR r1 7 comments（采纳 4：删 tool_calls_count 占位消新双轨 + 测试改名 + 过时注释 + 显式化；不采纳 2 假设性/信息性）/ 0 failure；type 1 n=25 不变。spec 立项 aca979c / 校正 1 次（budget +45/-35→+80/-70）。diff +76/-69。**evolution 3b 剩 1 簇（I）。**（补：**第 45 批 reviewer 抽查 = pass**——EV-3b-G，explore 子 agent 只读 git show/spec/OCR json/代码原文核验。）

- [2026-09-24 23:25 CST] EV-3b-G 收口（commit 59a46e3）：**C5-EV-3b-G 全簇 3 条已清**。① apply.rs DB_WRITE_LOCK 收窄到仅 apply_one（open_db/ledger/now 锁外预计算；append+audit 锁外；逐条 ? 中止语义不变）——核出 finding 前提部分过时：整批本就在 spawn_blocking 内，真问题=锁范围；② observe/shadow.rs with_app append_change 包 spawn_blocking（唯一生产入口）；trait 两变体零生产调用方加 doc 注明测试/内存 sink 专用；③ panel/commands.rs 8 个 async command 阻塞段包 spawn_blocking_map（5 整体闭包 + 3 个含 confirm await 的拆两段，await 不跨闭包）——签名零变更。OCR r1 3 low（采纳 cr move；不采纳 Arc<PathBuf> YAGNI / is_panic 分支 tauri::Error 无此 API 编译证伪回退）/ 0 failure；type 1 n=25 不变。spec 立项 a3946cc / 校正 1 次（budget +140/-55→+165/-85）。diff +159/-77。**evolution 3b 剩 2 簇（H/I）。**

- [2026-09-24 23:09 CST] EV-3b-F 收口（commit 2c70cb6）：**C5-EV-3b-F 全簇 3 条已清**（+1 同根 medium 一并）。① proposal.rs short_hash 弃 DefaultHasher 换仓内固定 FNV-1a-64（routing::fnv1a 复用）+ known-answer 标准向量锁算法——跨版本/跨机确定性，dedup 契约不再受 toolchain 升级威胁；id 形态 16 hex 不变，前向生效（升级边界重派生最多重入一次，R 阶段 dev 特性知悉接受）；② synthetic.rs SyntheticConfig::validate()（ratio 有限∈[0,1]、三桶和≤1、active+rolled_back≤1）+ 同根 medium window_days≥1 一并，generate 入口 fail-fast panic（signature 不变无 ripple）；③ observe/shadow.rs with_app 补 TOTAL/FAILED_WRITES fetch_add 与 trait 变体逐点对齐——失败率 >5% 告警对生产入口恢复可触发。OCR r1 3 comments（2 medium 采纳：window_days 上限 + validator 本体契约测试；1 low 不采纳：双面 API——改 signature ripple 不值得，doc 已写明 panic 契约）/ 0 failure；type 1 n=25 不变。spec 立项 4a1f7a6 / 校正 2 次（budget +85→+115→+140/-15）。diff +134/-11。**evolution 3b 剩 3 簇（G/H/I）。**

- [2026-09-24 22:02 CST] EV-3b-C2 收口（commit 92d1b9a）：**C5-EV-3b-C2 全簇 2 条已清**（+1 同根 medium 一并）。① ttl.rs evict_expired 加 status 门（仅 Pooled/Expired 可硬删，过期 Promoted/Rejected 保审计轨迹；零生产调用方）+ 回归测试；② derive.rs `hard_constraint_compliance` → `passes_auto_apply_gate`（只检约束 5/6 却声称 9 条全合规的名实不符；重命名=finding 轻量选项，扩检查=语义方向不做）+ 同根 medium 开放式否定改显式白名单 High|Medium（今日等价，未来变体编译期可见）。ripple：change/mod.rs re-export 一行 + 测试改名。OCR r1 4 comments（2 对重复：测试名未随 fn 改=spec ripple 遗漏，采纳；措辞修订，采纳）/ 0 failure；type 1 n=25 不变。spec 立项 9c121ac / 校正 1 次（budget +42/-18→+55/-28）。diff +47/-22。**evolution 3b 剩 7 簇（C3/D/E/F/G/H/I）。**（补：**第 40 批，reviewer 抽查 = pass**——explore 子 agent 只读 git show/spec/OCR json/代码原文核验。）

- [2026-09-24 22:19 CST] EV-3b-C3 收口（commit 7f84fcf）：**C5-EV-3b-C3 全簇 2 条已清**（+1 同位置 low 一并）。① routing.rs:21 A/B 改独立加盐 hash `ab_bucket`（fnv1a("ab:"+id)），与 canary 桶正交——canary 不动（成员稳定契约）；is_ab_a 零生产调用方=契约修正无消费者；bucket 降 pub(crate) 移出 re-export（同位置 low，零外部调用方）。新增 ab_orthogonal_to_canary 测试（区间 [0.35,0.55]，OCR 指出 [0.30,0.70] 含 buggy 值 0.589 卡不住回归 → 收紧，实测正交后 0.411）。② kill_switch.rs:15 边界归一化：load_from_file 对 all_auto_apply=true+shadow_only=false 矛盾组合强制 shadow_only=true + eprintln 留痕；**两处既有测试锁旧矛盾态已改断言**；类型级禁绝（私有字段重构）YAGNI。finding 所述 should_auto_apply 部分已被 C1 修复。OCR r1 1 medium 采纳 / 0 failure；type 1 n=25 不变。spec 立项（随抽查补记同 push）。diff +50/-10 恰压 budget（+50/-15）。**evolution 3b 剩 6 簇（D/E/F/G/H/I）。**

- [2026-09-24 22:55 CST] EV-3b-E 收口（commit 2636428）：**C5-EV-3b-E 全簇 3 条已清**（同根）。**triage 行号标注 observe/shadow.rs 系笔误，实为 sandbox/shadow.rs**（按内容定位核实：WithDummy no-op trait）。删 WithDummy trait+impl+链式调用+死变量 candidate_importance（importance 仍经 hypothetical 排序参与决策，被删的只是冗余局部拷贝；「接入决策」=语义方向 B 类不取）；附带同 family 删 :16 死 import fnv1a。零外部使用无 ripple。OCR r1 0 comments / 0 failure；type 1 n=25 不变。spec 立项 19c4511。diff +0/-14。**evolution 3b 剩 4 簇（F/G/H/I）。**

- [2026-09-24 22:42 CST] EV-3b-D 收口（commit e244309）：**C5-EV-3b-D 全簇 3 条已清**（+1 同文件 medium 一并）。① ttl.rs compute_expires_at 改 saturating_add（腐败/对抗 created_at_ms 近 i64::MAX 不再 wrap 成负数=静默立刻淘汰；溢出=永不过期保审计轨迹）——**核出真实生产路径不走该 fn**：candidate/derive.rs:49 + panel/commands.rs:357 两处内联 now_ms+TTL_MS 同缺陷，一并收敛单点实现；② ttl.rs doc-only：evict 调用约定（先 mark 后 evict / evict 前持久化快照；零生产调用方）+ now_ms 时间源契约（wall-clock 同源，NTP 回拨知悉接受）——(a) status 门 C2 已修不重复；③ derive.rs Contradiction id 追加 short_hash(keep|drop_id) 二次 hash 判别位（一轮 N 条矛盾不再坍缩成 1 个 proposal_id 丢 N-1 信号）——**测试暴露第二层缺陷**：normalize_for_hash 数字归一化成 'N'，UUID hex 仅数字不同的对在 base_id 仍坍缩，判别位绕开归一化。OCR r1 5 comments（1 medium + 4 low 全采纳：复合 33 字符 id 破 16 hex 契约 → 改二次 hash 保形态；summary 不并入 refs 避免与 related_refs 双写；32 字符 UUID 回归断言；ttl 测试固定 now_ms）/ 0 failure；type 1 n=25 不变。spec 立项 649e9ad / 校正 3 次（budget +45→+55→+65→+80/-12：doc 交付物超初估 + 第二层缺陷 + OCR 采纳增量）。diff +76/-8。**evolution 3b 剩 5 簇（E/F/G/H/I）。**

## 1. 跨域同模式家族
按"错误去哪了" + "是否破坏数据"两轴判，**4 家族**（poisoned-silent-recovery 已溶解 — 见执行日志；error-visible-non-blocking 已重新引入 for C5-AP-06 only — 见 §3.5 异常 2 更新）：

### error-not-propagated（错误信号丢失，调用方收到空/无，**不破坏已有数据**）
- C5-DB-01a（db，3 条：workspace.rs:54 返回空 Vec / tasks.rs:185 ON CONFLICT 哑火 / mod.rs:251 bool 被丢）→ **已清（1fcc418 ×2 + f93f4a9 ×1）**
- C5-EV-3b-A（evolution，5 条）
- C5-BT-01a（bot*，5 条）
- C5-MI-02（migration，3 条）

### failure-recovery-default-value（错误被替换成默认值，**且可能覆写/破坏已有数据**——优先于 error-not-propagated）
- C5-DB-05（db，3 条：默认 updated_at=0 / corrupt 源吞掉 / 批量 inserted_at 全用同一 now；poison.into_inner 静默条已修 → EVNB-02，family 修正为 error-visible-non-blocking）
- C5-DB-01b（db，2 条：workspace.rs:92/:209 unwrap_or_else("[]") 写入数据库 → 用户 links 被静默覆写为）
- C5-BT-01b（bot*，2 条：tools.rs:147/:280 静默吞 DB load 失败返回空 task list）
- C5-MI-03（migration，2 条：rules load fallback default + CSV 输入 coerce → 用户配置被静默覆写）→ **CSV coerce 条已修（FRDV-01）；剩 1 条（rules.rs:27，B 类候选）**

### error-visible-non-blocking（错误日志可见但调用方继续，**非错误传播**，仅记录）
- C5-AP-06（api，1 条）：src-tauri/src/api_server.rs:127 broadcast atomic_write 失败仅 eprintln!（log-and-continue 形态）

**与 error-not-propagated 的区别**：error-not-propagated 是"错误不传播、调用方无感"；error-visible-non-blocking 是"错误不传播但日志可见（std/stderr）"。两者都不阻断调用方，但后者有可见痕迹（需运维查看 stderr）。

修复路径：error-not-propagated 修法 = 错误显式传播（? / Err 返）；error-visible-non-blocking 修法 = 仅 log 化（不传播）+ 可选结构化日志（log crate / log_line）。

### atomicity/partial-write（写到一半失败，不清理不回滚，跨 db / migration 两域）
- C5-MI-06（migration/ops.rs:127 copy_dir_recursive mid-fail → dst 部分填充，1 条）
- C5-DB-02a（db，2 条：migrations.rs:51 multi-row UPDATE 无事务 + paths.rs:65 partial-copy after rename）

跨 db / migration 两域同模式，3 条 findings。修复设施不同：db 走事务 rollback，migration 走 cleanup；不同批但同 family，跨域一致性检查同时出现。

**两家族区分判据**：错误去哪了 = 丢弃 / 替换默认值；是否破坏数据 = 是/否。DB-01b vs DB-01a 的真正分界是"序列化失败时静默用空数组覆写用户数据"（破坏），不是"返回空"（不破坏）。跨域一致性检查时，failure-recovery-default-value 严格按破坏数据评估，error-not-propagated 仅按信号丢失评估。

## 2. 簇清单（已确认域）

### 域 db（7 簇 / 16 findings）
- C5-DB-01a：error-not-propagated（workspace.rs:54 + tasks.rs:185 + mod.rs:251），3 条 → **已清**：workspace.rs:54 + tasks.rs:185 首批 1fcc418；mod.rs:251 首批补全 f93f4a9（reset_bot_assigned_with bool→Result + `?` 传播）
- C5-DB-01b：failure-recovery-default-value（workspace.rs:92 + :209），2 条 → **已清（DB-01b-BT-01b，commit 1e023e3）：序列化失败返 Err 拒写，不再 silent "[]" 覆写——2026-09-25 06:59 源码复核 map_err 传播在位，行标补打**
- C5-DB-02a：atomicity/partial-write（migrations.rs:51 + paths.rs:65），2 条 → **已清（APW-01 8e7c9b4 迁移包事务 + APW-02b 497e4c1 3 文件 swap+回滚）——2026-09-25 06:59 源码复核 tx/staging 在位，行标补打**
- C5-DB-02b：事务约定一致性 / 设计债（tasks.rs:439）→ **wontfix-pending-design-decision**，触发条件 = 未来引入 cascade/soft-delete 多语句 delete 时重审
- C5-DB-03：race / TOCTOU（mod.rs:61 + tasks.rs:396），2 条 → **已清（DB-03，commit 59f81c8：mod.rs:61 锁+双检实修；tasks.rs:396 判 FP——load_all 单语句快照一致，reviewer 核验 pass）**
- C5-DB-04：input-validation（bot_sessions.rs:115 + tasks.rs:485，含 1 security），2 条 → **已修（DB-04，commit c9a3041）**
- C5-DB-05：failure-recovery-default-value（paths.rs:50 + workspace.rs:199 + bot_history.rs:52），3 条（tasks.rs:423 poison.into_inner 条已修 → EVNB-02，归 error-visible-non-blocking）→ **B 类 3 条已清（DB-B，commit f137536，§0 2026-09-25 08:07；用户拍板 #3=A+C / #4=C / #5=C）**

### 域 evolution（19 簇 / 39 findings）
3a（C3 周边，3 簇 / 4 findings）：
- C5-EV-3a-01：observe/stop.rs:82 静默吞负值，1 条 —— **已清**（dcd4808，§0 2026-09-24 20:03）
- C5-EV-3a-02：panel/commands.rs:43/:282 RMW 缺陷，2 条 —— **已清**（1992904，§0 2026-09-24 20:30）
- C5-EV-3a-03：observe/metrics.rs:86 O(n*m)，1 条 —— **已清**（46fffe5，§0 2026-09-24 20:41）

3b（其余，11 簇 / 35 findings）：
- C5-EV-3b-A：silent-error-swallow 跨文件（apply.rs:151 + activation.rs:136 + mod.rs:48 + record.rs:211 + entry.rs:110），5 条 —— **3 条已清**（31fee88，§0 2026-09-24 21:05）；record.rs:211 + entry.rs:110（read_all 单行损坏 fail-closed vs fail-open 方向）→ **B 类攒批第 17 项**
- C5-EV-3b-B：jsonl 写 race / 非原子（record.rs:187 + entry.rs:81 + sandbox/io.rs:46 + emit.rs:105 + activation.rs:257），5 条 —— **已清**（c0472de，§0 2026-09-24 21:29）
- C5-EV-3b-C1：状态转移/条件判定错（mapping.rs:37 + change/status.rs:36 + kill_switch.rs:39），3 条 —— **已清**（f72e253，§0 2026-09-24 21:43）
- C5-EV-3b-C2：双侧逻辑不一致（ttl.rs:23 + change/derive.rs:34），2 条 —— **已清**（92d1b9a，§0 2026-09-24 22:02）
- C5-EV-3b-C3：设计约定未强制（routing.rs:21 + kill_switch.rs:15），2 条 —— **已清**（7f84fcf，§0 2026-09-24 22:19）
- C5-EV-3b-D：批内 3 处独立改动（溢出/不可逆/去重，ttl.rs:41 + ttl.rs:34 + derive.rs:104），3 条 —— **已清**（e244309，§0 2026-09-24 22:42）
- C5-EV-3b-E：死代码 / no-op（**sandbox/shadow.rs**:0 + :87 + :163，triage 原标注 observe 系笔误），3 条 —— **已清**（2636428，§0 2026-09-24 22:55 CST）
- C5-EV-3b-F：哈希/校验缺（proposal.rs:186 + observe/synthetic.rs:29 + observe/shadow.rs:345），3 条 —— **已清**（2c70cb6，§0 2026-09-24 23:09 CST）
- C5-EV-3b-G：持锁/async 阻塞 IO（apply.rs:156 + observe/shadow.rs:202 + panel/commands.rs:0），3 条 —— **已清**（59a46e3，§0 2026-09-24 23:25 CST）
- C5-EV-3b-H：trace 双轨/静默丢（trace.rs:104 + trace.rs:149 + trace.rs:193），3 条 —— **已清**（d4ad243，§0 2026-09-24 23:38 CST）
- C5-EV-3b-I：关键不变量测试缺（routing.rs:110），1 条 —— **已清**（C3 7f84fcf 覆盖，§0 2026-09-24 23:41 CST 零代码处置）

### 域 migration（10 簇 / 15 findings）
- C5-MI-01：poisoned mutex silent recovery（DB_WRITE_LOCK silent 路径，**违反 C3-1 约定**），1 条，**家族已溶解，独立存在** → **已修（EVNB-02，commit 299e776：journal.rs db_write_lock 闭包加 mutex_poisoned eprintln，恢复语义不变）**
- C5-MI-02：error-not-propagated（migration/run.rs:132 + :270 + recovery.rs:87，unwrap_or(None) / filter_map(r.ok) 吞 DB err，调用方无感），3 条 → **已清（首批开批，commit eca3622）：unwrap_or(None)/filter_map(r.ok) → ? 显式传播——2026-09-25 06:59 源码复核在位，行标补打** 条
- C5-MI-03：failure-recovery-default-value（migration/rules.rs:27 + :121，破坏数据：rules 默认覆盖 + CSV 输入 coerce），2 条 → **:121 已修（FRDV-01，commit 46f8437）；:27 已清（MI-B1，commit 5b5f360，§0 2026-09-25 08:53；用户拍板 #1=B 主案 caller=3）**
- C5-MI-04a：migration/journal.rs:34 INSERT 无去重，1 条 → **已修（MI-04a，commit 414cf08）**
- C5-MI-04b：migration/journal.rs:126 unlocked find+act TOCTOU，1 条 → **已修（MI-04b，commit 5819f76；replay 纳入 MigrationGuard）**
- C5-MI-05a：migration/commands.rs:116 spawn_blocking 无 abort，1 条 → **B 类候选（#2）已拍 A → 已清（MI-B2，commit ab47f1b，§0 2026-09-25 10:55：取消机制贯穿 + command 就绪，取消 UI 未来接线）**
- C5-MI-05b：migration/commands.rs:19 + recovery.rs:197 sync/block_on 阻塞，2 条 → **已拍 A（2026-09-25）→ 已清（MI-B3，commit db6d8e9，§0 2026-09-25 11:35）：commands.rs 两条 async 化；recovery.rs:197 stale 零代码（reviewer agent-22：std::thread 上 B3 设计内合法桥接）——跨域阻塞族三簇至此全部处置完毕**
- C5-MI-06：migration/ops.rs:127 copy_dir_recursive mid-fail → dst 部分填充（**atomicity/partial-write 家族**），1 条 → **已清（APW-02a，commit d443a07）：顶层 staging sibling + 原子 rename + 失败清 staging——2026-09-25 06:59 源码复核在位，行标补打**
- C5-MI-07：migration/ops.rs:202 路径不一致（rotation target ≠ write target），1 条 → **已修（MI-07，commit 613e07a）**
- C5-MI-08：migration/rules.rs:99 + :67 解析脆 / 校验缺（批内 2 处独立改动：contains 太宽松 + archive_dir 无路径校验 = 1 security），2 条 → **已修（MI-08，commit bbe4f59）；残余（archive_dir 绝对路径/symlink/破坏性提示，B 类 #9）已清（MI-B1，commit 5b5f360，§0 2026-09-25 08:53）**

### 域 scripts（5 簇 / 10 findings）
- C5-SC-01：shell 解析脆（sync-version.mjs:11 + ci-guard-tiny-http-vendor.sh:46 + :27），3 条 → **已清（SC-01，commit 9629fe9；sync-version.mjs:11 判 FP 零代码处置，§0 02:18）**
- C5-SC-02：非原子/非完整（sync-version.mjs:20 + fetch_ocr_models.sh:44 + :53），3 条 → **已清（SC-02，commit 1227af9，§0 02:23）**
- C5-SC-03：set -euo 边界未封（install-hooks.sh:10），1 条 → **已清（SC-03，commit 9047712，§0 02:25）**
- C5-SC-04：第三方/CDN/用户输入未校验（fetch_ocr_models.sh:49 + publish-docx-dotnet.sh:12），2 条 → **已清（SC-04，commit 114440b，§0 02:31）**
- C5-SC-05：hook 覆盖不全（test-fast.sh:56），1 条，**与 HOOK-1/HOOK-2 同根因家族** → **已清（SC-05，commit 66fbbec，§0 02:34）**

### 域 api（7 簇 / 14 findings）
- C5-AP-01：TOCTOU race（api_auth.rs:27 + api_handlers/commands.rs:61 + :238 + api_server.rs:208），4 条 → **api_auth.rs:27 + api_server.rs:208 已修（AP-01b，commit c49bba3）；commands.rs:61/:238 转 B 类攒批第 15 项（改「文件 I/O 不持锁」约定方向）**
- C5-AP-02：symlink + 权限（api_auth.rs:73），1 条 → **已修（AP-02，commit 5d8e878）**
- C5-AP-03：日志输出缺陷（api_handlers/ratelimit.rs:45 + :36，批内 2 处独立改动），2 条 → **已修（AP-03，commit a78de81）**
- C5-AP-04：SSE writer 缺陷（api_handlers/sse.rs:196 + :183），2 条 → **已修（AP-04，commit 8d49a26：vendor 写超时 patch + broadcast 单临界区化）**
- C5-AP-05：持锁跨 I/O（api_handlers/handlers.rs:277 + :405 + :561），3 条 → **已修（AP-05，commit bfe30b9：labeled block 装盒出锁后响应）**
- C5-AP-06：静默吞错（api_server.rs:126），1 条，error-visible-non-blocking 家族 → **已修（独立批 0b6ee69，2026-09-23：atomic_write 失败 eprintln 留痕）；残留语义方向（B 类 #16）已清（API-B，commit 6de187b：写失败拒推进 id fail-closed——§0 2026-09-25 10:23）**
- C5-AP-07：sync block_on 隐式约定（api.rs:101），1 条 → **已修（AP-07，commit 3d6b1a8：运行时检查 + trait 契约文档化）**

### 域 bot*（15 簇 / 46 findings；去重后）
子目录 = 18 / bot_skills 子目录 = 12 / 兄弟 bot_*.rs = 16
- C5-BT-01a：error-not-propagated，bot/config + bot_skills/scheduler + bot_scheduler（5 条丢弃：commands.rs:30 + keyring.rs:263 + slash.rs:456 + skills/scheduler.rs:428 + scheduler.rs:0）→ **已清**：-1/-2 退批 error-visible-non-blocking（见退批登记）；slash.rs:456 前批已修；skills/scheduler.rs:428 + scheduler.rs:0 **已修（BT-01c，commit eb11651）**
- C5-BT-01b：failure-recovery-default-value，bot/tools（tools.rs:147 + tools.rs:280，替换成空 list）→ **已清**：:147（active_tasks + 三 call site）前批 1e023e3；:280（tool_search_tasks 残余）**已修（BT-01d，commit 见 §0）**
- C5-BT-02：**OCR false positive（3 条均不准确）** —— OCR 忽略代码中的 eprintln!("[mutex_poisoned] ...") 缓解措施，据此判定 "silent"——前提与实现不符。三站实际都是 logged 路径：C3-1 约定的合法实现。三条 finding 的站实有 eprintln：bot_py.rs:945 + bot_skills/state.rs:109 + bot_skills/runtime.rs:58。**记录 + 不改**，与 C3-r1-H1 / C4-2 / C4-5 同处理。**triage 记录 140 仍含此 3 条 FP，Phase 2 实工单 137 不计**。
- C5-BT-03：config 写非原子 / RMW 无锁（commands.rs:86 + schema.rs:170/:178 + io.rs:100/:76），5 条 → **4/5 已修（BT-03a，commit f86fc7a）；commands.rs:86（keyring 先于文件写无回滚 = 半成功陷阱方向）转 B 类攒批待拍；同簇残余（add_allowed_dir 剥 key，B 类 #10）已清（BOT-B，commit ea6317d，§0 2026-09-25 09:41）**
- C5-BT-04：TOCTOU on canonicalize/whitelist（bot_fs.rs:167 + bot/tools.rs:102 + bot_chat.rs:399 + bot_artifacts.rs:67），4 条 → **全清（拆批）**：:399 + :67 **已修（BT-04a，commit ca73ec0）**；:167 + :102 **已修（BT-04b，commit 93182f3，blocking syscall 全量移出 async runtime）**——:167 的 TOCTOU 维度归既有 follow-up「Phase 6: resolve_with_perm 其它调用点 TOCTOU 统一策略」
- C5-BT-05：URL/host bypass（config/io.rs:122 + bot_web.rs:21/:730/:881），4 条 → **3/4 已修（BT-05，commit 51efb80）**；:881（Jina 回退审计缺口，B 类 #12）**已清（BOT-B，commit ea6317d：回退三路 stderr 留痕，签名不动——§0 2026-09-25 09:41）**
- C5-BT-06：log injection via model strings（bot_model_loop.rs:1079 + :984），2 条 → **OCR false positive（2 条均不准确）**——`truncate_for_log` 自 dcbf167（2026-09-14，早于 0921 全扫）起为 `escape_for_log` 别名（bot/config/audit.rs:74），`\n` `\r` `|` 已转义，注入面不存在；finding 前提「只限长度不剥换行」与实现不符。FP 诱因 = :1081 陈旧注释（描述修复前行为），**已改述（BT-06，commit 841922b）**。与 C5-BT-02 同处理：记录 + 不改行为。**triage 记录仍含此 2 条 FP，Phase 2 实工单 142 再扣 2 = 140**。
- C5-BT-07：state machine / 乐观并发不一致（bot_artifacts.rs:113 + bot_skills/state.rs:164 + bot_skills/scheduler.rs:149），3 条 → **拆批**：:149 **已修（BT-07a，commit 6bd43c0，Drop 兜底）**；:113 **OCR false positive（BT-07b 判定）**——finding 称「upsert_tasks does not compare expected_updated_at」与实现不符：db/tasks.rs:199-224 显式 SELECT + 冲突返 CONFLICT_ERR_PREFIX（另有 ON CONFLICT WHERE 谓词 + affected 行数双保险），confirm_artifacts_batch :127 正设置了 expected_updated_at 快照基线，并发窗写时被拒非「silently」；:164 全局清理 vs 按会话（B 类 #11）**已清（BOT-B，commit ea6317d：clear_terminal_skill_runs 按会话隔离，§0 2026-09-25 09:41）**
- C5-BT-08：input validation / serialization 缺（config/types.rs:198/:82 + config/keyring.rs:168 + bot_skills/parse.rs:86 + bot_skills/vars.rs:76），5 条 → **拆批**：:82 **已修（BT-08a，commit e7b2381，三 key 字段 skip_serializing 永不序列化，零行为变更）**；:86 **已修（同 commit，非法 mode 按未声明处理，low→auto 提升不被吞，语义归一 SKILL_DSL.md）**；:198 **FP（web 实证）**——deepseek-v4-flash 旧名仍被接受（V4.1-Flash GA 后服务名 deepseek-flash，旧名请求由新模型服务，deepseek.ai/pricing 2026-09-18）；:168 **转 B 类攒批第 13 项**（Windows ACL，macOS 不可测）；:76 **wontfix-with-rationale（待醒后确认）**——占位符使模板构造上非合法 JSON，启发式是结构必然，替换后 serde_json 兜底
- C5-BT-09：dispatch/scheduler 语义错（bot/dispatch.rs:248/:424 + bot_skills/scheduler.rs:283），3 条 → **已清（拆批）**：:283 **已修（BT-09a，commit b1c0b4f，Finish 早退假完成收口）**；:248 **已修（BT-09b，commit 3f62d74，索引复用）**；:424 **wontfix-with-rationale**（与 OCR-003 同型：ToolResult::ok 失败文案是 dispatch.rs:402-404 注释钉死的设计意图）
- C5-BT-10：持锁跨 IO / sync IO in loop（config/audit.rs:22 + bot_skills/runtime.rs:83/:393 + bot_scheduler.rs:468），4 条 → **全清（拆批）**：:22 + :83 + :393 **已修（BT-10a，commit a4e3bf1，锁内 IO/钩子全移出临界区，零行为变更）**；:468 **已修（BT-10b，commit 455c7c1，tick/task 双层 catch_unwind + Semaphore(4)）**，关闭信号按 App 生命周期 YAGNI 登记
- C5-BT-11：web 解析脆（bot_web.rs:412 + :422），2 条 → **已清（BT-11，commit b1d9212）**：find_tag_open 开标签精确匹配（后字符须空白/>//），杀 <a 误中 <abbr>、<p 误中 <pre>；行为修复 abbr 前置时标题/链接错位
- C5-BT-12：workspace-link 残留（bot_skills/files.rs:0 + :43），2 条 → **全清（stale，C2b-2 已修）**：:0 kind 白名单 + :43 归一化不一致均已被 95a25e9（2026-09-21 20:39，**晚于 0921 全扫快照**）修复——link_kind_contributes_path 白名单（ALLOWED_LINK_KINDS 单一来源）+ 绑集 canonical 双侧比对。同文件另 4 条（:0 is_dir 死参数 / :10 静默收缩 / :83 TOCTOU / :144 raw contains，**triage 漏分簇补登记**）同判 stale——is_dir 已从 IPC 删、Err 分支全带 audit + C2c-v2 fail-closed、recheck_canonical 紧邻副作用、delete 走 path_openable_in。**reviewer 抽查 pass**（6 条独立核验）
- C5-BT-13：prompt injection via fail_reason（bot_plan.rs:234），1 条 → **已修（BT-13，commit 11cf4f7）**：sanitize_fail_reason 剥控制字符+拆 ``` 序列+截 500，围栏+「数据非指令」标注
- C5-BT-14：StopGuard broken（bot/registry.rs:267），1 条 → **B 类 #14 已拍 C（2026-09-25）→ wontfix-with-rationale**：全接线=13+ 签名改扩 scope、循环级 4 处 stop 检查已覆盖长调用场景、mutating 工具全为快 DB/文件操作——StopGuard 设计意图=长操作+循环级中断，已知风险登记

### 域 WidgetApp（3 簇 / 7 findings）
- C5-WA-01（原 WA-01 拆后残余，原 3 条拆为：a=constants.ts:13 已并入 C4-v2；b 仅剩 2 条 storage.ts:113 + storage.ts:79）：输入/反序列化校验缺（storage.ts:113 Anchor 结构不一致 + storage.ts:79 loadAnchor JSON.parse 零校验），批内 2 处独立改动（类型对齐 vs JSON.parse 校验，修复设施不同），2 条 → **已清（WA-01，commit 919aa0e，§0 02:45）**
- C5-WA-02：window listener 管理缺（ResizeEdge.tsx:43 Promise.all 无 catch + ResizeEdge.tsx:72 同步附加无 useEffect + SplitBar.tsx:15 组件卸载不清理），批内 3 处独立改动（加 .catch vs 重构 useEffect vs 加 cleanup，修复设施不同），3 条 → **已清（WA-02，commit 328fa76，§0 02:53）**
- C5-WA-03：dnd-kit 可达性/事件冲突（SortableWorkspaceCard.tsx:23 键盘 drag 不可达 + SortableTaskCard.tsx:55 drag→click 选中误触发），批内 2 处独立改动（aria 属性 vs click/drag 阈值，修复设施不同），2 条 → **已清（WA-03，commit 9b90ebd，§0 03:00）**

**PANEL_H 同源合并**：OCR 标 constants.ts:13 = 实指 line 13（PANEL_H_MIN/MAX 字面 800/900），与 C4-4 同源。C4-v2 升级合入（PANEL_H_MIN > PANEL_H + range 100px 两子项同族）。WA-01 原 3 条拆为：a=constants.ts:13 已并入 C4-v2（不独立计），b=storage.ts:113+:79 保留为 WA-01（重命名后剩 2 条）。

### 域 frontend 碎片（9 簇 / 52 findings；按文件组批 ≤5 文件/批 + ≤10 findings/批 + ≤+500/−300 行）

- C5-FE-01：core 写入加载路径（src/storage.ts:62/109/113/122 + src/App.tsx:191/240/254/291 = 8 条），批内 8 处独立改动（错误返回 0 / mutate 副作用 / 错误传播断 / legacy data 丢 / source whitelist 注释错等），修复设施各异 → **部分清（FE-01a，commit e2faf6a，§0 2026-09-25 03:20）：4 实修 + 2 FP（reviewer pass）；:240 → B 类 #18、:191 → B 类 #19**
- C5-FE-02：React async 事件 race（src/components/ArtifactBatchDialog.tsx:34/35/79 + src/components/ConfirmMap/ConfirmMap.tsx:37/40/65 = 6 条），批内 6 处独立改动（listen/unlisten race / 异步 state race / reentrancy），修复设施各异 → **部分清（FE-02，commit ba48f86，§0 2026-09-25 03:49）：4 实修；:79 → B 类 #20、:40 → B 类 #21**
- C5-FE-03：setup / 缓存 / 截断（src/test/setup.ts:12/37 + src/profile.ts:28/62 + src/lib/taskFiles.ts:9/43 = 6 条），批内 6 处独立改动（console.error 拦截泄漏 / profile force 失效 / taskFiles 截断静默 / 跨语言常量锁步），修复设施各异 → **已清（FE-03，commit 47f48d4，§0 2026-09-25 04:23）：4 实修 + 2 FP（reviewer agent-14 pass）**
- C5-FE-04：UI 渲染（src/components/TodoCard/SubtaskRow.tsx:27/:48 + src/components/WorkspacePage.tsx:80/:99 + src/components/MarkdownText.tsx:21/:34 = 6 条），批内 6 处独立改动（SubtaskRow stale closure / readOnly 未重置 + WorkspacePage 乐观状态被覆盖 / in-place mutation + MarkdownText String(children) 不安全 / code 可点击不可聚焦），修复设施各异 → **已清（FE-04，commit a68f104，§0 2026-09-25 04:54）：5 实修 + 1 FP（reviewer agent-15 pass）**
- C5-FE-05：UI 控件（src/components/SettingsPage/ProfileRow.tsx:78/:110 + src/components/FoldToggle.tsx:15/:21 + src/components/ChatPanel/RichText.tsx:22 + src/components/ChatPanel/Fold.tsx:16 = 6 条 / 4 文件），批内 6 处独立改动（ProfileRow setTimeout 未存句柄 / busy 未禁用 input + FoldToggle type 属性 / aria 状态 + RichText anchor 无 href + Fold 折叠状态不可访问），修复设施各异 → **已清（FE-05，commit f44b851，§0 2026-09-25 05:09）：6 实修，OCR r1 0 comments**
- C5-FE-06：ChatPanel / utility（src/components/ChatPanel/ChatPanel.tsx:416 + src/components/ChatPanel/constants.ts:9 + src/components/ChatPanel/types.ts:5 + src/lib/errorHandler.ts:216 + src/components/useInlineEdit.ts:44 + src/components/EvolutionPanel/EvolutionPanel.tsx:149 = 6 条 / 6 文件），批内 6 处独立改动（ChatPanel 并发 busyRef / Map 导出 / 类型泄漏 / onRetry 无 catch / 双提交 / 类型断言 bypass），修复设施各异 → **部分清（FE-06，commit 189adc4，§0 2026-09-25 05:23）：4 实修 + 1 FP（reviewer agent-16 pass）；:416 → B 类 #22**
- C5-FE-07：次要组件零散（src/components/SettingsPage/SkillsPanel.tsx:60 + src/components/SettingsPage/ApiProviderSelect.tsx:37 + src/components/EvolutionPanel/DeleteConfirmDialog.tsx:40 + src/components/EvolutionPanel/types.ts:24 + src/components/ActorAvatar.tsx:10 + src/components/ArchivePage.tsx:52 = 6 条 / 6 文件），批内 6 处独立改动（setTimeout unmounted / 无 WAI-ARIA / 无 focus 管理 / snake_case 泄 / promise 无 catch / displayTask 重复创建），修复设施各异 → **部分清（FE-07，commit 487b499，§0 2026-09-25 06:47）：5 实修；:24 → B 类 #23**
- C5-FE-08a：其他 UI 组件（src/components/DoneCircle.tsx:13 + src/components/KanbanBoard.tsx:179 + src/components/TaskCardContent.tsx:293 = 3 条 / 3 文件），批内 3 处独立改动（button type 缺 / rect.current deref null / schedule 静默覆盖） → **已清（FE-08a，commit d937b01，§0 2026-09-25 06:59）：3 实修（OCR r1 同根因镜像扩 TodoCard）**
- C5-FE-08b：顶层配置（src/main.tsx:10 + src/theme.ts:33 + src/ui/main.css:131 = 3 条 / 3 文件），批内 3 处独立改动（root null check 缺 / applySetting 不通知 / nm-card-hover transition 重复声明） → **部分清（FE-08b，commit 2f006ef，§0 2026-09-25 07:06）：2 实修 + 1 FP（reviewer agent-19 pass）；theme.ts:33 判 FP；theme.ts:84-95/:13-21 属其他行非本簇**

**FE-08 拆分理由**：FE-08a = 通用 UI 组件（按钮 + 拖拽 + 卡片内文），FE-08b = 应用入口与全局配置（React root + 主题 + 全局样式）。主题域不同：UI 组件修复针对单组件行为，全局配置修复影响整个应用启动/主题传播。不混批。

## 3. 待核区（首批启动前必核）

- **C5-BT-12 ≈ C2b-2 修复未覆盖** → **已核消解（2026-09-24）**：95a25e9 的 kind 集合 = 写入侧 ALLOWED_LINK_KINDS ["url","file","folder"] + 消费侧仅 {file,folder} 贡献路径，app/command/未来 kind 两侧均拒。C5-BT-12 全簇 6 条（含漏分簇 4 条）均为 0921 全扫快照早于 95a25e9 的 stale finding，零代码处置，见 §2 C5-BT-12 行。
- **C5-SC-05 ≈ HOOK-1/HOOK-2 家族**：test-fast.sh 审计批次号防线仅扫 tracked，不含 untracked。HOOK-1（fmt 全仓检查 × 既有漂移）/ HOOK-2（knip 静态盲区）已登记，SC-05 属同家族。
- **AP-07 / BT-14 severity 来源**：源 JSON 验证 severity=high（OCR 真实定级，非 medium/low 混进），保留。
- **C4-v2 升级（PANEL_H 同源合并，状态：wontfix-pending-product-decision）**：原 C4-v2 只含 PANEL_H_MIN > PANEL_H 单项。现 OCR WA-01 finding（constants.ts:13 start_line，OCR 行号与实指 line 13 PANEL_H_MIN/MAX 字面 800/900 同源）合入 → C4-v2 升为 **两个子项**：(a) PANEL_H_MIN > PANEL_H 逻辑矛盾（C4-4 原 observation）+ (b) PANEL_H_MIN/MAX range 仅 100px 窄区间（WA-01 新 observation）。**同一产品决策（widget 内容最小可用高度 + 上下限），产品拍板时一次改完两处，别只修一个**。**C4-v2 最终状态 = wontfix-pending-product-decision（继承 C4-4 状态，不进实工单 144 的"待修"集合，只占位 / 待产品拍板）**。WA-01 拆出的 constants.ts:13 条不独立计为 finding。首批决策时一并处理 C4-v2 升级。

### 新发现登记（PHASE2-TRIAGE-NEW-*）

不在源 OCR high 名单、triage 跑出过程中顺手发现，下一轮评估：

- **PHASE2-TRIAGE-NEW-1**：bot_py.rs:694 + bot_py.rs:705 测试代码内 silent into_inner（**已清——NEW-1345，commit 9f50382，§0 2026-09-25 12:10**）源 OCR 标的是 :945（有 eprintln），这两处不在 OCR 范围。是 silent 路径，与 C3-1 约定（带 eprintln）不一致。是否需引入 Phase 2 high 待评估。**不并入 BT-02 / MI-01**（"顺手扩"是 triage 层禁忌）。
- **PHASE2-TRIAGE-NEW-2b**（2026-09-25 NEW-2 批登记；**bot 域 24 处已清——NEW-2b-1，commit 9523329，§0 2026-09-26 02:20**；实测修正 47→45，其余 21 处见 NEW-2b-2）：NEW-2 实施时全仓 grep 实测另有 **47 处**同形 silent into_inner 超出 NEW-2 登记口径（bot_slash.rs x7、memory x8、bot_skills/state x5、py x4、bot_chat x5、exec_steps x3、api_handlers/handlers x3（API_RMW_LOCK）、evolution/emit x3、evolution/apply x2、bot_scheduler x2、bot_skills/scheduler x1、db/mod.rs:67（LEGACY_COPY_LOCK）——NEW-2 登记注记「其他域多数已有 eprintln」与现状不符。修法同 NEW-2 机械形态。**已拍板执行（2026-09-26）：bot 域 24 处 NEW-2b-1 已清 + 其余 21 处 NEW-2b-2 已清——全仓 silent into_inner 归零**。
- **PHASE2-TRIAGE-NEW-2**（2026-09-23 EVNB-02 批登记）：db / migration / bot.config 三域 **12 处同形静默 into_inner**（**12 处已清——NEW-2，commit 6a7cce6，§0 2026-09-25 11:50**），均不在源 OCR 271 high 名单。站点（±3 行上下文 grep 核实无 eprintln，2026-09-23 23:2x 工作树）：workspace.rs:173 / :189 / :278、bot_history.rs:89 / :108、bot_sessions.rs:78 / :100 / :112、migration/ops.rs:236 / :246、bot/config/audit.rs:26、bot/config/mod.rs:581。与 C3-1 约定（logged 恢复）不一致。EVNB-02 只修 OCR 名单内 2 站，**不顺手扩**（triage SOP）。修复形态预期与 EVNB-02 相同（统一走 `lock_db_write()` 或补 eprintln），下一轮评估是否引入 Phase 2 工单。注：bot_skills/* 与 py/* 域另有大量 into_inner 站点，多数已有 eprintln（BT-02 先例），本登记仅含已核实静默的 12 处。
- **PHASE2-TRIAGE-NEW-3**（2026-09-24 AP-03 批登记）：bot/config/audit.rs 两处同族缺口（**3a 已清 / 3b stale——NEW-1345，commit 9f50382，§0 2026-09-25 12:10**），均不在 OCR 271 high 名单——(a) `escape_for_log` 只转义 `|` `\n` `\r`，缺 `\t` / `\0` / ESC 等其余控制字符（AP-03 sanitize_log_line 已覆盖的全集）；(b) audit.rs:302 `audit_log` 与 ratelimit.rs 同形态 rotate+append 无锁，并发写可撕裂。AP-03 只修 ratelimit.rs（OCR 名单内），**不顺手扩**（triage SOP）。修复形态预期：复用/对齐 sanitize_log_line + LOG_WRITE 同款锁，下一轮评估是否引入 Phase 2 工单。
- **PHASE2-TRIAGE-NEW-5**（2026-09-25 API-B 批登记）：db/paths.rs `atomic_write` 在 rename 失败时残留同级 `.tmp` 文件（**已清——NEW-1345，commit 9f50382：rename 失败臂 best-effort 清理 + 单测**）（`fs::write(&tmp)` 成功、`rename(&tmp, path)` 失败的路径无清理）。API-B fail-closed 测试（id_path 指向目录）暴露。影响面：所有 atomic_write 调用点（api_server SSE id / storage 导出等），失败残留为匿名 tmp 垃圾文件，无数据危害（下次同名写覆盖）。修法候选：rename 失败臂 best-effort `remove_file(&tmp)`（照 AP-02 write_token_file 先例）。下一轮评估。
- **PHASE2-TRIAGE-NEW-4**（2026-09-24 BT-10b 批登记）：panic payload 字符串化逻辑（**已清——NEW-1345，commit 9f50382：crate::audit::panic_message 单点收敛**）（downcast_ref::<&str> → String → "非字符串 panic"）已 4 处重复：api_server.rs panic_message helper / middleware.rs panic_message helper / migration 域 1 处 / bot_scheduler.rs 新增 2 处（tick + task 层）。抽统一 helper 到 audit 或 util 模块 = 跨文件重构，超 BT-10b scope。下一轮评估是否引入。

### 跨域一致性检查第一步（triage 全跑完后那一步）

triage 全跑完后、首批决策前，按大类把簇列出复查修复设施是否真的统一，不统一的按 DB-01/BT-01 同法拆。**不做这一步，Phase 2 首批可能挑到一个看起来 ≤10 条实际跨 3 种设施的簇，重演 C2b 拆簇**。

大类清单（triage 全跑完后填）：

· 所有 TOCTOU/race 类 — 例：C5-DB-03（2 条，原子创建 vs 读锁不同设施）、C5-AP-01（4 条 token 并发 / state 原子化 / fetch_add 三设施）、C5-MI-04（2 条 UNIQUE 约束 vs 锁包整段）
· 所有 error-swallow / silent-fallback 类
· 所有 state machine 类
· 所有 持锁跨 IO 类

逐簇复查"修复设施是否真的统一"。当前已发现潜在不一致候选（待全 triage 完统一复查）：

· C5-DB-03（mod.rs:61 check-then-act vs tasks.rs:396 读侧同步）— 原子创建 vs 读锁不同设施，可能拆
· C5-AP-01（4 条 token 写并发 / state check-then-act / fetch_add）— 三种设施，可能拆
· C5-EV-3b-C1（3 条 状态转移/条件判定错）— 已拆同根因，保留
· C5-EV-3b-G（3 条 持锁/async 阻塞 IO）— DB_WRITE_LOCK 持锁跨 IO / shadow_apply 阻塞 / panel async 阻塞，三种设施，可能拆
· 其他见 triage 全跑完后的复查结果

### 跨域阻塞族待核区（triage 全跑完后复查）

三个簇都含"阻塞"元素，但设施各异（spawn_blocking vs drop-then-respond vs 抬锁外）。跨域一致性检查时一并复核是否拆/合，**不现在动**：

- C5-MI-05b（migration，sync/block_on 阻塞，2 条）
- C5-AP-05（api，持锁跨 I/O，3 条）
- C5-EV-3b-G（evolution，持锁/async 阻塞 IO，3 条）

## 3.5 已知异常（commit 收口后登记，防“历史误读为当时正确”）

### 首批 commit 1fcc418 已知异常

首批 Phase 2 commit (1fcc418) 收口后自查发现的两个动作失误，记录于此防后人不读原 commit 不明所以：

- **异常 1：批定性数字错**
  - 拍板原文（13:40:34）：「首批 = AP-06 (1) + DB-01a-2 (2) + MI-02 (3) = **4 条**」
  - 事实：1 + 2 + 3 = **6 条**。拍板时拍板文本「4 条」为算错（错在拍板者，未延展到被拍板者本轮出错）。
  - 处置：amend 重写 commit message 标题为「2 簇 / 5 条」（AP-06 移出后）、body 标注“AP-06 移出本批独立处理”。**现存 commit (1fcc418) 数字以 5 条为准**。
  - SOP（本轮起）：批定性数字不许沿用拍板者提供，必须自行加和核对一遍再报。

- **异常 2：AP-06 自决降级未报**（17:14、首批草表未列入独立批登记）
  - 拍板原文（13:40:34）：「C5-AP-06 (1 条) api_server.rs:126 → atomic_write(...)? 显式传播」
  - 事实链：? 在 broadcast() -> () 编译不过（E0277）→ 改签名要动 5 调用点（超首批范围）→ 未拍板自行降级为 `if let Err(e) = ... { eprintln!(...) }`。
  - 本质：C5-DB-01a-3（mod.rs:251）以“log-and-continue 形态、family 异质”为由移出首批。AP-06 同一形态（eprintln! 同样非错误传播），应同样移出；但本次未拍板自决降级，留在首批**破坏了 family 语义同质**。
  - 处置：amend 还原 api_server.rs 为「let _ = atomic_write(...)」原版。AP-06 单独一批处理（归 error-visible-non-blocking family — 该 family 已重新引入 for AP-06 only）；C5-DB-01a-3 归 error-not-propagated family（首批补全）。现存 commit (1fcc418) 不含 api_server.rs 改动。
  - SOP（本轮起入 §5）：**拍板动作被执行时遇到编译/架构阻碍（签名不允许、类型不匹配、需动 N 调用点）→ 停手报，不降级。降级 = 改拍板 = 越权。可选项由用户拍。**

- **C5-DB-01a-3 定性修正（19:55，migrations.rs:84 源头吞）**
  - 首批时把 C5-DB-01a-3 定性为“mod.rs:251 加 log”——只看最外层。实现核验发现错误在**源头**就披吞：migrations.rs:84 `match exec() { Err(_) => false }` 把闭包 Err 转成 bool，调用方拿不到失败信号。
  - 修复路径：改 `reset_bot_assigned_with` 签名 `-> bool` 为 `-> Result<(), String>` + `Err(e) => Err(e)`（迁移函数内部源头错误传递）+ mod.rs:253 加 `?`（生产者 caller）+ db/mod.rs 测试 4 处 `let ok =` → `let r =` + 4 处 `assert!(!ok)` → `assert!(matches!(r, Err(_)))` / `assert!(ok)` → `assert!(r.is_ok())`（表示法变更，同首批 tasks.rs:185 处置）。
  - 总面积：migrations.rs 4 行 + mod.rs 1 行 + db/mod.rs 测试 8 行 = **13 行**（首批估 3 行严重漏算）。
  - 归类：error-not-propagated family（首批家族成员），不是 log-and-continue。
  - 连带：error-visible-non-blocking family 重新引入 for C5-AP-06 only（前提变化——DB-01a-3 走真传播归 error-not-propagated，AP-06 仍 log-and-continue 形态；C5-AP-06 单独一批处理，不与 DB-01a-3 同批）。

### DB-01b-BT-01b batch follow-up（2026-09-23, commit 1e023e3）

DB-01b-BT-01b 收口后自查发现 4 项 follow-up：

- **报告数字纠正（无 ID）**：报告原文“spec 列 5 项”数字错；正确“spec 列 4 条，实修 6 处”（spec findings 数组只有 DB-01b.1/.2 + BT-01b.1/.2 = 4 条；BT-01b.2 单 finding 覆盖 3 call site，拆为 BT-01b.2a/2b/2c 三个修改点）。commit 1e023e3 message 已写“实修 6 处”，未含错数；**commit 不动**，本节留痕为下次报告 review 拍板 SOP 补充。SOP（本轮起生效）：spec findings 数自行 `jq '.findings | length'` 核，不沿用拍板者提供数字。
- **PHASE2-TRIAGE-SPEC-001（合并 SPEC-001 + SPEC-002）**：spec 起草前置核 = **函数签名 + 调用链 + 返回类型枚举值** —— 三条任一没核，spec 前提可能错。
  - SPEC-001 例：BT-01b.2 fix 字段写 `ToolResult::ok(text, refs, status=ToolStatus::Fail)` —— 实际 `ToolResult::ok` 是 2 参数固定 status=Ok（registry.rs:60-62），`ToolStatus` 只有 `Ok/Warn/Error` 无 `Fail`（registry.rs:42-46）。
  - SPEC-002 例：BT-01b.2b 标 tools.rs:516 = tool_complete_task 直接调用，实际经中间函数 `find_task_by_keyword`（tool_tools.rs:516 定义 + :547/571 调用）触发 —— 只核了函数签名，未核调用链。
  - 修法（下批起生效）：spec 起草工具加三检脚本（grep 签名 + grep caller + grep 枚举值），与 NEW-META-6 同型（hook 拦不到的事前补漏）。
- **PHASE2-TRIAGE-OCR-001**：tools.rs:547 [medium/maintainability] —— **已修（BT-01d，文案定稿「按关键词查找失败」）**。原登记：`find_task_by_keyword` `.await?` → `CommandError::Internal`，与 resolve_task 内 active_tasks 显式 `CommandError::DbError` 不一致；同一 DB 故障两种 code，统计漏算一半。
- **PHASE2-TRIAGE-OCR-002**：tools.rs:571 [medium/maintainability] —— **已修（BT-01d，同 OCR-001 一并）**。
- **PHASE2-TRIAGE-OCR-003**：tools.rs:160 [medium/bug] wontfix-with-rationale —— `ToolResult::ok` + 错误文本是 dispatch.rs:402-404 明确设计意图（原文：“失败也走 `ToolResult::ok` 而非 `err`，让 LLM pipeline severity classifier 不把‘完成任务失败：DB error’误判为 fatal”），OCR 建议改 `ToolResult::error` 与既有约定冲突，**不修**。若产品决定后续加 `ToolStatus::Fail` 变体，可同时改 handler 层 + commit message 解释（当前为应用层应用既有约定的动作，不构成 family 异质——family 边界见 1e023e3 commit message）。

【OCR 原文核验记录（DB-01b-BT-01b）】
- PHASE2-TRIAGE-OCR-001/002/003：均为 OCR r1 /tmp/ocr-DB-01b-BT-01b-r1.json 原文，handlers.rs 评论数 = 0（与 OCR-001/002/003 路径无关，本批仅改 workspace.rs + tools.rs）；comments 数组长度 3 条，全 medium 0 high，无 stop condition 触发（compile_failure / architecture_blocker / family_heterogeneity / new_high_different_root / gate_fail 均未触发）。

### 待核 / 后续 (Follow-up)

- **PHASE2-TRIAGE-NEW-HANDLERS-1**: api_handlers/handlers.rs:342 (`create_task`) `let _ = after_change(...)` 吞 Err——同模式 silent-discard。Scope control：本批仅 api_server.rs + api_handlers/mod.rs + util.rs，未动 handlers.rs；handlers 层 silent-discard 是新发现，待独立批处理（OCR r1 21:49 报 high）。修复路径（OCR 建议）：用 `log_line` 替代 `let _ =` 或 propagate。
- **PHASE2-TRIAGE-NEW-HANDLERS-2**: api_handlers/handlers.rs:496 (`update_task`) 同模式 silent-discard，同上处理。
- **PHASE2-TRIAGE-NEW-HANDLERS-3**: api_handlers/handlers.rs:584 (`delete_task`) 同模式 silent-discard，同上处理。

【OCR 原文核验记录（今日补）】
- PHASE2-TRIAGE-NEW-HANDLERS-FP-1：HANDLERS spec 撤销 / FP 登记
  - OCR/报告描述：handlers.rs:342/496/584 "silently discarding after_change errors"
  - 核验结果：after_change（util.rs:90）-> ()；notify_change（api.rs:34）-> ()；两函数均无 Result 返回，无 Err 可丢
  - 判定：finding 前提不成立 → FP（OCR r1 /tmp/ocr-C5-AP-06-r1.json 无 handlers.rs 评论，handlers.rs 评论数 = 0）
  - 影响：HANDLERS.spec.md 未执行（gate 前 stop 触发），working tree 已回 HEAD
  - 状态：撤销，不开批

- PHASE2-TRIAGE-NEW-META-6：hook 用 --fast，不带 --tests → 编译错漏过 hook
  - 事实：gate-pre-commit.txt 不含 cargo_check_tests
  - 影响：E0308 这类编译错到 commit 时才发现
  - 修法（下批起生效）：spec 加 manual_gate_full_tests: true 字段，hook 校验该字段存在
  - 不本批动作

- PHASE2-TRIAGE-NEW-META-7：报告层脑补 findings（不核 OCR 原文直接报为事实）
  - 事实：handlers.rs 3 条 silent-discard 是 21:46 报告里被写进 §3.5 NEW-HANDLERS-1..3，但 OCR 原文 (/tmp/ocr-C5-AP-06-r1.json) 无 handlers.rs 评论
  - 同源问题已发生多次（"已贴/✓ 无内容"模式），与今天前面几次截断同型
  - 修法（下批起生效）：审计链重审 + triage spot check（明日 09:00 后做：随机抽 5–10 条 finding 核对 OCR 原文，不合格就修 triage）
  - 不本批动作

【明日 triage spot check（今日登记，明日做）】
- 抽 5–10 条 finding 核对 OCR 原文：handlers.rs 3 条是脑补 → 其他 high 也可能有同类问题
- 时间：明天 09:00 后开
- 触发条件：开下一批 family 前必须完成
- 工具：jq + grep OCR 原文 paths + cross-check finding 注册来源

### OCR 已知异常

- **OCR: unavailable**：首批 commit 无 OCR 复审。工具内部 file_read 参数错（start_line 80 > end_line 60）。证据目录 `~/.openclaw/cache/ocr-C5-B1-failure/` 已建但文件未完整落盘。三层自测（fmt + check + test-all 全绿）代替 OCR 复审。
- **PROC-5: OCR 工具与 infra 稳定性**
  - 类型 1: file_read tool-bug（start_line > end_line），证据: 1fcc418 批次 + EVNB-01（n=2）+ EVNB-02 单 run 4 次（n=3）+ FRDV-01（n=4）+ MI-07 单 run 2 次（n=5）+ MI-08 单 run 2 次（**n=6**，超触发线仍复发，评估批待 reviewer 排期）
  - 类型 2: timeout-class hang（SIGKILL / stdout 0 bytes），证据: APW-02a 批次
  - 类型 3: rate-limit（HTTP 429 重试耗尽，status=failed，comments=null），证据: APW-02b 批次 + BT-01a 批次（**n=2**）
  - 累积规则: 同类 ≥2 次触发单批评估 OCR 调用方式调整 —— **type 3 已 n=2，触发条件达成**（待评估项：退避策略 / 调用频率 / 供应商限流配额；本轮不动作，开独立评估）
  - **[2026-09-26 eval 结论（用户拍板：不出新批）]** type 1 累计 n=25+，但 2026-09-25/26 的 10 个批（B 类 7 + NEW 3）实测仅零星发作（file_read 参数越界均被重试机制兜底、review 本体全部 complete、0 批因 OCR 失败受阻）——**维持现状不立项**；后续若出现 review 本体失败或 comments 丢失再重估。type 3 同批维持观察。

### APW-02b OCR r1 disposition（2026-09-23 补，audit 链修正）

【审计链修正】commit 497e4c1 message OCR 段写「【OCR r1: D 路径 unavailable】原因: HTTP 429 rate_limited」
——与实际不符。实际 OCR r1 有**两次运行**：
- **run A 成功**：status=complete，5 findings，MiniMax-M3，7m32s，run_id 33bbb88f-c275-4db4-ad7f-815af341e80f（17:49）
- run B 失败：status=failed，429 rate_limited，0 findings，run_id c8da10cc-6132-4f5a-9d82-90e490e2e175（18:01）

497e4c1 的「unavailable」只对 run B 成立，**漏了 run A 的成功结果**。497e4c1 已 push（amend 窗口关闭），
按既有规矩用本修正 commit 补 audit 链，不改 497e4c1。

【两次运行「都叫 r1」的根因】路径撞车：两次都写 /tmp/ocr-APW-02b-r1.json —— run B 覆盖 run A 的 raw；
run A 仅靠 /tmp/ocr-APW-02b-r1.clean.json 找回。cache 命名亦误导：
- ~/.openclaw/cache/APW-02b/ocr-r1.json = run A（complete）
- ~/.openclaw/cache/APW-02b/ocr-r1-raw.json = run B（failed）
改进（下次）：OCR 输出按 run_id 命名，不硬编码 -r1。

【5 条 finding 逐条处置（对当前 71a35bf 工作树核）】
| # | file:line | sev | 处置 |
|---|---|---|---|
| 1 | db/paths.rs:97 | **critical** | **已在 497e4c1 修复** ✓ —— Phase 2a/2b 已加 `if tmp_wal.exists()` / `if tmp_shm.exists()` 守卫（OCR 与测试 ENOENT 同指一 bug） |
| 2 | db/paths.rs:79 | medium | 挂起 → follow-up（`Ok(0)` sentinel 混淆「零字节成功」与「不适用」） |
| 3 | db/mod.rs:456 | low | 挂起 → follow-up（`writes_three_files_returns_warns` 断言恒真） |
| 4 | db/mod.rs:419 | low | 挂起 → follow-up（`returns_err_when_legacy_db_missing` 未触达 tmp-* 清理） |
| 5 | db/paths.rs:90 | low | 挂起 → follow-up（4 组 cleanup 近似重复，建议提 helper） |

【5 条原文】见 ~/.openclaw/cache/APW-02b/ocr-r1.json（run A）。

### APW-02b OCR follow-up 索引（2-5 挂起项）

- **APW-02b-OCR-2（medium）**：db/paths.rs:79 `Ok(0)` sentinel —— 下游 `copy_main.err().or(...)` 链无法区分
  「边车不存在（不适用）」与「零字节成功拷贝」。修法候选：用 `Option<u64>` 或显式 enum。
- **APW-02b-OCR-3（low）**：db/mod.rs:456 断言 `warns.is_empty() || warns.iter().any(...)` 恒真，不锁定契约。修法：改成精确断言（如 `assert_eq!(warns.len(), N)` 或指定 warn 内容）。 **→ 已清（FUP-1，commit 2ba5062，§0 2026-09-26 02:50）——收紧暴露 BUSY 非确定性，落 warn 值域断言**
- **APW-02b-OCR-4（low）**：db/mod.rs:419 测试未真正触达 tmp-* 清理路径（legacy_db 不存在 → copy 在写 tmp-* 前已失败 → remove_file 是 no-op）。修法：构造「copy 成功但后续阶段失败」的场景。
- **APW-02b-OCR-5（low）**：db/paths.rs:90 四组 cleanup 近似重复，建议提 `fn cleanup_staging(...)` helper。 **→ 已清（FUP-1，commit 2ba5062：remove_staging helper）**

（均不本批修；下次 APW-02b follow-up 或相关批并入。）

### C5-BT-01a 退批登记（2026-09-23）

- **C5-BT-01a-1**（bot/config/commands.rs:30 + :31）：`bot_get_config` 内 `let _ = io::migrate_legacy_key(&app);` / `let _ = io::migrate_search_keys(&app);`。**修法 = (b) log warn + 继续**（migrate 是幂等 + 可重试 + 非前置副作用，callee doc 明示「App 启动时调用一次，设置页读配置时也会兜底触发」）→ **family = error-visible-non-blocking**（与 C5-AP-06 同 family），非 error-not-propagated。**退 BT-01a 批**；未来 error-visible-non-blocking family 成批时并入。

- **C5-BT-01a-2**（bot/config/keyring.rs:272-276）：`delete_api_key_at` 内 v0 遗留条目清理 `let _ = old.delete_credential();`。
  **修法 (d)**：保留原顺序（主删除先算 `let r = ...`）+ 双 Error 分流——
  主删除失败 → 返 Err（遗留未动，无副作用）；主删除成功 + 遗留清理失败 → 记审计 + 不阻断（返 Ok，主目标达成）。
  **family = error-visible-non-blocking**（同 C5-AP-06 / C5-BT-01a-1）。
  **退 (c) 理由**：(c)「遗留清理挪到主删除前」= 反方向半成功陷阱（遗留成功 + 主删除失败 → 遗留已清、主 key 未删；用户以为没删掉任何东西 = 半清）。
  退 BT-01a 批；未来 error-visible-non-blocking family 成批时并入。

## 4. 累计

### EVNB-01 follow-up 登记（2026-09-23, commit 9b0d767）

- **EVNB-01-OCR-1（medium/maintainability，挂起）**：keyring.rs:276 遗留清理失败走
  eprintln（stderr 易逝）而非 bot.log。OCR 建议 thread `&AppHandle` 进 `delete_api_key_at`
  ——**涉签名变更 + 调用点 ripple，超本批红线**；且 family 语义（error-visible-non-blocking）
  已由 eprintln 满足（与 C5-AP-06 同先例）。若未来统一"bot 域错误落 bot.log"标准，随该批一并做。
- **PROC-5 type 1 复发（n=2）**：EVNB-01 OCR r1（/tmp/ocr-EVNB-01-20260923-224348.json）run 内
  `file_read failed: start_line 200 > end_line 100`——与首批 1fcc418 同类。本次 review 本体
  complete（1 finding 产出），非整 run 失败。**type 1 累积 n=2，达单批评估触发线**（同 type 3
  的 n=2 规则）；是否开评估批由 reviewer 排期。
- 【OCR 原文核验记录（EVNB-01）】r1 = /tmp/ocr-EVNB-01-20260923-224348.json（原始首行为
  [ocr] 错误行，.clean.json 为剥首行后解析版）：status=complete / comments=1（medium，
  即 EVNB-01-OCR-1）/ 0 high / 4m33s / tool failure 1（file_read）。

### EVNB-02 follow-up 登记（2026-09-23, commit 299e776）

- **PROC-5 type 1 复发（n=3）**：EVNB-02 OCR r1（/tmp/ocr-EVNB-02-20260923-230527.raw.json）
  单 run 内 **4 次** `file_read failed: start_line > end_line`（range 反转：160>30 / 80>30 /
  70>25 / 165>30；目标 tasks.rs 1 次 + 上下文文件 bot_history/bot_sessions/workspace 3 次）。
  4 次失败均未中目标区段——files_reviewed=2（tasks.rs + journal.rs 均实际审到）、
  comments=0、terminal_state=complete，非整 run 失败。**type 1 累积 n=3**（首批 1fcc418 /
  EVNB-01 / EVNB-02），超 n=2 触发线后仍在复发；评估批仍待 reviewer 排期（同 EVNB-01 登记）。
- 【OCR 原文核验记录（EVNB-02）】r1 raw = /tmp/ocr-EVNB-02-20260923-230527.raw.json
  （raw 文件头部混有多行 `[ocr] ✘ file_read failed ...` 前缀行——比 EVNB-01 的单行更重，
  解析需 `grep -v '^\[ocr\]'` 全量剥除，.clean2.json 为剥后解析版）：status=complete /
  comments=0 / 0 high / 16s / tool failure 4（均 file_read range 反转，见上）/
  model=MiniMax-M3 / run_id=8b52e6ce-65b3-4a2e-878b-bb16d9bcb255。
- **comments=0 的可信度注记**：reviewer 曾尝试 file_read 上下文文件 bot_history.rs /
  bot_sessions.rs / workspace.rs（正是 NEW-2 登记的同形站点域），均因 type 1 bug 失败；
  但 diff 本体经 source_artifact 提供，目标 2 文件审到且 0 findings。0 comments 成立，
  不因工具失败打折。

### FRDV-01 follow-up 登记（2026-09-23, commit 46f8437）

- **FRDV-01-OCR-1（low ×3 同根）**：rules.rs `domain: "csv"` vs `domain: "migration"`（**已清——FUP-1，commit 2ba5062：×3 统一 "migration"，前端无 domain 消费已核；源 medium Mixed error-type conventions 维持挂起**
  不一致（OCR r1 指 :125/:131/:145 三处）。**根因是既有代码**：行级错误的 `domain: "csv"`
  约定先于本批存在（未知动作 arm 原本就是 "csv"），本批两个新 arm 沿袭同函数行级错误约定。
  源 fullscan 已有同根 medium（rules.rs:94-111 "Mixed error-type conventions"），非 271 high
  名单项。统一方向（全改 "migration"）会动既有 arm + domain 是前端消费的契约字段 → 超
  C5-MI-03.2 scope，挂起待与源 medium 同批评估。**只改本批新 arm 不改旧 arm = 函数内更碎，
  明确不做。**
- **PROC-5 type 1 复发（n=4）**：FRDV-01 OCR r1 又一次 `file_read failed: start_line 120 >
  end_line 110`（rules.rs，未中目标区段）。累积：1fcc418 / EVNB-01 / EVNB-02（单 run 4 次）/
  FRDV-01 = **n=4 runs**。评估批仍待 reviewer 排期。
- 【OCR 原文核验记录（FRDV-01）】r1 = /tmp/ocr-FRDV-01-20260923-232054.raw.json
  （.clean.json 为 `grep -v '^\[ocr\]'` 剥前缀后解析版）：status=complete / comments=3
  （全 low 同根，即 FRDV-01-OCR-1）/ 0 high / 1m17s / tool failure 1（file_read range 反转）/
  model=MiniMax-M3 / run_id 见 manifest。

### MI-04b follow-up 登记（2026-09-24, commit 5819f76）

- **MI-04b-OCR-1（medium，挂起）**：replay 单次性——journal_replay_pending 只在
  spawn_polling 启动路径调用一次；本批新增的 5min 守卫等待超时后跳过本轮 = 该启动期内
  pending 留待下次重启。OCR 建议 replay 纳入 polling loop 或超时后调度延迟重试——
  属 polling loop 语义设计变更，超 MI-04b scope。注意 run.rs 内联 src-missing 修复路径
  （:139/:276）对 pending 有兜底，非"永久卡死"。
- **MI-04b-OCR-2（low，FP 登记）**：OCR 建议 `Some(_g)` → `Some(_)`——**不采纳**：
  `Some(_)` 不绑定值会立即 drop guard，replay 在无守卫状态下执行 = 恰好破坏本批修复。
  `_g` 绑定是语义必需（持活到 replay 结束）。同类"模式匹配建议丢绑定"属 OCR 系统性
  误判倾向，后续批遇同类直接按 FP 处理并登记。
- **MI-04b-OCR-3（low）**：守卫等待 60×5s 双魔数未绑定为单一不变量（**已清——FUP-1，commit 2ba5062：GUARD_WAIT_ATTEMPTS/STEP_SECS const**（建议抽
  const WAIT_MAX/RETRY_STEP）。采纳需 +3 行超本批已二次校正的 budget，挂起；改超时时
  连同注释/日志文案一起改。
- **PHASE2-TRIAGE-NEW-META-8**：budget 校正同批两次（14→26 OCR critical 采纳；26→33/-10
  rustfmt 嵌套重排）超 SOP「估算错允许一次」额度。根因：OCR 驱动的批内形态变更发生在
  budget 设定之后。SOP 补充候选：OCR r1 采纳 critical/medium 导致的形态变更允许跟随
  校正（不计入一次额度），或 spec budget 统一加 OCR 采纳余量。待 reviewer 拍。
- 【OCR 原文核验记录（MI-04b）】r1 = /tmp/ocr-MI-04b-20260924-001336.clean.json
  （1 critical guard 作用域泄漏 + 2 medium 等待无上限 + 2 重复条目；同根=本批新代码，
  全部采纳修复）；r2 = /tmp/ocr-MI-04b-r2-20260924-001839.clean.json（0 critical / 0 high /
  3 comments = 上述三条；tool failure 0；critical 修复验证通过）。

### AP-03 follow-up 登记（2026-09-24, commit a78de81）

- **AP-03-OCR-1（low，不采纳登记）**：OCR 建议在 LOG_WRITE 锁处加注释命名锁覆盖的三步
  ——不采纳：`static LOG_WRITE` 的文档注释已写明"串行化 rotate+append"，再加是重复。
- 【OCR 原文核验记录（AP-03）】r1 = ~/.openclaw/cache/AP-03/ocr-r1-20260924-005403.json
  （status complete；4 comments 全 low：锁注释[不采纳] / capacity×2[采纳] /
  U+2028-U+2029 未覆盖[采纳，同根] / 测试扩展[采纳]；tool failure 0；elapsed 1m25s）。

### BT-01c follow-up 登记（2026-09-24, commit eb11651）

- **BT-01c-OCR-1（low，挂起）**：find_due_tasks "每 tick 最多 3 次 db_load" 的 race 窗口
  （源 finding 附带提及）——窗口收窄属设计变更（合并 RMW 为单次 load+upsert 事务），
  本批只加可见性，挂起待与 DB-03 race 簇同批评估。
- **PROC-5 type 1 复发（n=9）**：BT-01c OCR r2 ×2 + r3 ×1，均
  `file_read start_line > end_line`（bot_model_loop.rs:1010>830、runtime.rs:370>120、
  runtime.rs:360>110），review 本体 complete。type 1 累计 n=6→n=9。
- 【OCR 原文核验记录（BT-01c）】r1 = ~/.openclaw/cache/BT-01c/ocr-r1-20260924-010950.json
  （1 high 同根=普通聊天路径误记闸门缺失 + 2 low ids 未截断，全采纳；tool failure 0）；
  r2 = ocr-r2-20260924-011525.json（0 high；2 low=session_has_run 形态在 :594+:1006
  连发下仍误记 → 闸门改 reason 非空；tool failure 2=type 1）；r3 =
  ocr-r3-20260924-012658.json（0 high；1 low=三态门补单测，采纳；tool failure 1=type 1）。
  r3 后仅新增测试，生产码未变，无 r4。

### AP-02 follow-up 登记（2026-09-24, commit 5d8e878）

- **PROC-5 type 1 复发（n=11）**：AP-02 OCR r1 ×2（file_read start>end：300>50、240>50），
  review 本体 complete。type 1 累计 n=9→n=11。
- 【OCR 原文核验记录（AP-02）】r1 = ~/.openclaw/cache/AP-02/ocr-r1-20260924-063129.json
  （2 medium + 2 low 全同根：rename 失败遗留明文 tmp / 固定 tmp 名并发撕裂 /
  mode 只对新建生效 / 错误缺路径——全采纳；tool failure 2=type 1）；
  r2 = ocr-r2-20260924-063631.json（0 comments，修复验证通过；tool failure 0；elapsed 1m36s）。

- 【OCR 原文核验记录（BT-03a）】r1 = ~/.openclaw/cache/BT-03a/ocr-r1-20260924-064640.json
  （3 medium + 1 low：dir fsync / 审计移锁后 / rename 失败测试——采纳；keyring 在锁内
  不拆有论证；tool failure 0）；r2 = ocr-r2-20260924-065235.json（1 medium + 3 low：
  cfg(unix) 门采纳，锁窗/测试锁/死锁测试不采纳；tool failure 0）；
  r3 = ocr-r3-20260924-065843.json（11 comments：采纳 ConfigWriteGuard + 持锁标记 +
  debug_assert + guard 命名统一，其余 7 条不采纳有论证；tool failure 4 = type 1×3 +
  路径不存在 db.rs×1）；r4 = ocr-r4-20260924-070813.json（10 comments 含 2 high 同根：
  high1 注释精确化 + high2 schema 迁移进锁——结构性修复；tmp 唯一名/参数更名/去 cfg 门
  采纳；tool failure 1 = type 1）；r5 = ocr-r5-20260924-072227.json（0 high，6 medium +
  10 low：采纳 2 条=闭包锁内禁重入文档化 + migrate_bot_config_schema_locked 收窄
  pub(crate)；tool failure 0；elapsed 3m9s）。
- 【BT-03a 不采纳登记】keyring 锁窗（r1/r5：启动期一次性路径可接受，commands.rs:86 顺序
  问题已单独立 B 类）；tmp 崩溃残渣（rename 后崩溃留 tmp，下次写覆盖，低风险）；
  try_lock/lock 不对称（让路语义有意）；测试用生产锁（debug_assert 要求持锁，契约如此）；
  remove_file 静默（best-effort 清理）；release 模式持锁强校验（debug_assert + 命名约定
  + guard 类型 = 既定设计，照 db 先例）；毒锁恢复后 audit_event!（改 C3-1 约定属 B 类，
  现状照 db::lock_db_write 先例）；读移出锁（RMW 原子性正是本批要修的行为，读出锁
  = 重引入竞态）；add_allowed_dir 剥 key 统一走 locked 内核（r5 medium：防御纵深方向
  正确，但迁移失败（keyring 不可用）时明文 key 仍在文件，剥 key 写盘 = 密钥丢失——
  数据破坏相关修法方向，转 B 类攒批第 10 项待拍）。
- 【OCR 原文核验记录（BT-01d）】r1 = ~/.openclaw/cache/BT-01d/ocr-r1-20260924-073830.json
  （1 low：spec 文案「按关键词查找任务失败」与代码「按关键词查找失败」不一致——
  采纳=校正 spec 对齐代码，零代码 churn；tool failure 0；status complete）。
- **PROC-5 type 1 复发（n=18）**：BT-05 OCR r1 ×3（file_read start>end：770>100、
  805>50、855>15），review 本体 complete（0 comments）。type 1 累计 n=15→n=18。
- 【OCR 原文核验记录（BT-05）】r1 = ~/.openclaw/cache/BT-05/ocr-r1-20260924-081624.json
  （0 comments；tool failure 3 = type 1×3；elapsed 1m23s；status complete）。

### AP-01b follow-up 登记（2026-09-24, commit c49bba3）

- **AP-01b-OCR-1（medium/durability，挂起）**：create_token_file_atomic 写后无 fsync/
  sync_data——胜者进程在 write_all 返回后被杀（OOM/断电）时，输家重试窗内可读空文件。
  OCR 自认该遗漏与模块既有耐久性姿态一致（write_token_file 轮换路径亦无 fsync）。
  加 fsync = 耐久性语义变更 + 首建路径性能特征变化，超本批 scope；若未来统一
  「token/配置落盘 fsync」标准，随该批一并做（与 BT-03a r1 已采纳的父目录 fsync
  best-effort 对齐）。
- 【OCR 原文核验记录（AP-01b）】r1 = ~/.openclaw/cache/AP-01b/ocr-r1-20260924-184844.raw.json
  （status=complete / comments=3：1 medium=fsync（挂起，见上）+ 2 low=sync 阻塞说明 /
  SeqCst 一致性说明（均以注释采纳）/ 0 high / tool failure 0 / elapsed 2m33s /
  files_reviewed=2）。

### AP-04 follow-up 登记（2026-09-24, commit 8d49a26）

- **AP-04-OCR-1（medium，不采纳有论证）**：broadcast 锁内 atomic_write 的盘 I/O stall
  会阻塞并发 sse_connect 注册（同 clients 锁）。不采纳：落盘移出锁需另加同步才能保住
  「落盘序 = id 序」（CAS 只守 last_persisted 值，管不住并发 atomic_write 的 rename
  先后），代价大于收益；事件频率人级，注释已写明取舍。若未来事件频率量级变化，
  随「锁内 I/O」统一重构批再评。
- **AP-04-OCR-2（low，不采纳）**：测试直用 `pub clients` 字段（建议 cfg(test) 访问器）。
  与 api_handlers/mod.rs:504 既有测试模式一致；EventHub 封装重构超本批 scope。
- 【OCR 原文核验记录（AP-04）】r1 = ~/.openclaw/cache/AP-04/ocr-r1-20260924-190845.raw.json
  （status=complete / comments=6：2 medium + 4 low 全同根，0 high；采纳 4 不采纳 2，见上 /
  tool failure 0 / elapsed ~2m16s / files_reviewed=2）。
- 【OCR 原文核验记录（AP-05）】r1 = ~/.openclaw/cache/AP-05/ocr-r1-20260924-192325.raw.json
  （status=complete / comments=1：low=AlreadyDeleted 去 Box，采纳 / 0 high /
  tool failure 1 = code_comment 空数组拒收（良性形态 n=3）/ elapsed ~4m2s /
  files_reviewed=1）。

### FE-01a follow-up 登记（2026-09-25, commit e2faf6a）

- **B 类攒批第 18 项（App.tsx:240，半成功陷阱方向）**：tasks-updated 合并路径 upsert 成功 + delete 抛错时 UI 不更新、不广播。修法方向选项：A=回滚已成功的 upserts（补偿写，复杂且自身可失败）；B=仍合并广播（delete 失败行暂留 UI，下事件自愈）；C=维持现状仅靠 storage 层 alert 提示。待拍。
- **B 类攒批第 19 项（App.tsx:191，数据破坏相关修法方向）**：finding 字面前提错误（removeItem 本就在 upsert 之后，reject 不执行）；真实残留 = 迁移失败 → 种子落库 → legacy localStorage 数据永久 orphan。修法方向：A=迁移失败时跳过种子落库、保留 legacy 待下次启动重试；B=维持现状（orphan 无害但不雅）。待拍。
- 【OCR 原文核验记录（FE-01a）】r1 = ~/.openclaw/cache/FE-01a/ocr-r1.raw.json（status=complete / comments=5：1 high + 1 medium + 3 low 全部同根因，处置见 §0 / 0 failure / elapsed ~2m28s / files_reviewed=3）。

### FE-02 follow-up 登记（2026-09-25, commit ba48f86）

- **B 类攒批第 20 项（ArtifactBatchDialog.tsx:79，skip 语义方向）**：skip 纯前端 setReady(null)。核后事实：后端无 ack 协议——should_emit 用 peek 不取（bot_artifacts.rs:82）、confirm_artifact_batch 空 paths 早返不清登记（:110）、emit fire-and-forget（bot_chat.rs:1474）；skip 后登记表保留，下次执行同任务重弹。「阻塞后续处理」无证据。真实缺口 = 无永久 dismiss 路径。选项：A=维持现状+注释文档化（skip=这次不绑下次再问）；B=新增 dismiss Tauri command 清登记（扩 scope）；C=skip 走 confirm 空 paths + 改后端早返为清登记（后端行为变更）。待拍。
- **B 类攒批第 21 项（ConfirmMap.tsx:40，pending 覆盖方向）**：新 bot-confirm 到达直接覆盖未响应的 pending，旧 id 靠后端 60s 超时兜底拒绝。核后事实：后端每 confirm 独立 uuid+oneshot+60s（bot_slash.rs:297-341），并发未决合法，前端单窗 UI 是瓶颈。选项：A=前端队列逐个展示；B=切换前自动拒绝旧 id（用户未看到的请求被立即拒）；C=维持现状靠 60s 兜底+文档化。待拍。
- **B 类攒批第 22 项（ChatPanel.tsx:416，围观执行会话期锁发送方向）**：openExecSession 不加 busyRef——围观执行会话期间用户可 Send 与 bot_execute_task 并发同会话（响应交错 + 执行收尾 bot_history_load 冲掉围观期输入）。核后事实：既有注释约定「执行本身不吃 busy 锁（会话隔离天然并发）」；busy 锁会连带堵 switchSession/newSession/deleteSession。选项：A=finding 原方案全 busy 锁（围观期被困直到执行完）；B=专用 execWatchRef 只拦 Send（切换会话自由，收尾 .then 里清）；C=维持现状 + 先核后端 bot_chat 同会话并发是否自有防重入。待拍。
- **B 类攒批第 23 项（EvolutionPanel/types.ts:24，ProposalTarget snake_case union）**：tool_name/skill_name/policy 字段 verbatim 来自 serde response，camelCase 习惯的 consumer 写 toolName 会静默 undefined 但编译过。选项：A=字段 camelCase 重命名（serde rename + 全部消费方面改 = 跨域签名改）；B=字段级 JSDoc 文档化 verbatim 契约（不改类型）；C=wontfix。待拍。
- 【OCR 原文核验记录（FE-02）】r1 = ~/.openclaw/cache/FE-02/ocr-r1.raw.json（status=complete / comments=5：1 high 实收 + 1 low 实收 + 1 low 实收注释 + 2 low 不采纳 / 0 failure）；r2 = ~/.openclaw/cache/FE-02/ocr-r2.raw.json（status=complete / comments=5 全 low 实收 / 0 high）。

- **FE-03（47f48d4，2026-09-25 04:23）OCR 核验记录**：r1 4 comments（1 high 采纳→scrollNoop 按 suite scope 重构 + spec 校正 2；2 low 采纳；1 low 不采纳有论证）；r2 6 comments 0 high（1 medium 采纳=jsdom-only 注释声明；2 low 采纳；3 low 不采纳有论证）。无 B 类新增。

- **FE-04（a68f104，2026-09-25 04:54）OCR 核验记录**：r1 = ~/.openclaw/cache/FE-04/ocr-r1.raw.json（status=complete / comments=5：2 high + 1 medium + 2 low 全采纳）；r2 = ~/.openclaw/cache/FE-04/ocr-r2.raw.json（status=complete / comments=2：1 medium + 1 low 全采纳 / 0 high——r1 high 验证通过）。无 B 类新增。

- **FE-05（f44b851，2026-09-25 05:09）OCR 核验记录**：r1 = ~/.openclaw/cache/FE-05/ocr-r1.raw.json（status=complete / comments=0）。**flaky 样本 +1**：wmessage::task_chat_exec duplicate_trigger_rejected_by_exec_guard（test-all 首跑挂 / 隔离跑过 / 全量复跑过，纯前端批无关）——继端口 4763 ×2 + BotDisabled ×1 后第 4 例观察样本。无 B 类新增。

- **FE-06（189adc4，2026-09-25 05:23）OCR 核验记录**：r1 = ~/.openclaw/cache/FE-06/ocr-r1.raw.json（status=complete / comments=0 / tool failure 3 均 file_read 参数越界已重试）。

- **FE-06 reviewer 抽查（agent-17，2026-09-25 05:40，第 60 批）= pass**：四项修法语义逐核（shouldSkip 窗口语径 / committedRef 双提交 / onRetry 兜底 / ACTIVE_STATUSES 类型收窄）+ 翻断言测试复核，无回归。
- **FE-07（487b499，2026-09-25 06:47）OCR 核验记录**：r1 = ~/.openclaw/cache/FE-07/ocr-r1.raw.json（comments=6 全采纳）；r2 = ~/.openclaw/cache/FE-07/ocr-r2.raw.json（comments=4：1 high 采纳 + 1 medium 采纳 + 1 low 不采纳有论证 + 1 无问题）；r3 = ~/.openclaw/cache/FE-07/ocr-r3.raw.json（comments=6 / 0 high：5 采纳 1 low 不采纳有论证）。无 B 类新增（#23 为 finding 转登）。

- **FE-08a（d937b01，2026-09-25 06:59）OCR 核验记录**：r1 = ~/.openclaw/cache/FE-08a/ocr-r1.raw.json（comments=2 全 high 同根因采纳：TodoCard 镜像 + 取消清草稿）；r2 = ~/.openclaw/cache/FE-08a/ocr-r2.raw.json（comments=0，high 验证通过）。无 B 类新增。

- **FE-08b（2f006ef，2026-09-25 07:06）OCR 核验记录**：r1 = ~/.openclaw/cache/FE-08b/ocr-r1.raw.json（status=complete / comments=0）。备注：本批一次 test-all 后台跑 exit=1 无失败详情，立即复跑 exit=0 全绿，判环境偶发。无 B 类新增。

- **DB-B（f137536，2026-09-25 08:07）OCR 核验记录**：r1 = ~/.openclaw/cache/DB-B/ocr-r1.raw.json（status=complete / comments=4 全 low：① audit 调用形态重复[不采纳：Ok-warns 循环与 Err 臂 level/分支语义不同，统一徒增间接层] ② open-fail 测试仅 EISDIR 单模式[采纳=checkpoint 非 BUSY fail-closed 收紧 + NOTADB 损坏源测试，同 #3 根因] ③ 错误文案断言脆[不采纳：文案即拍板要求的可操作提示，锁定防退化] ④ 批内等时断言依赖单次 now[不采纳：该断言正是本批锁死的不变量本身，非 flaky]）/ 0 failure / 0 high。

- **MI-B1（5b5f360，2026-09-25 08:53）OCR 核验记录**：r1 = ocr-r1.raw.json（15 comments：1 high From<String> 塌码[采纳→load_rules_from 结构化 CommandError] / 4 medium 2 采纳 2 不采纳 / 10 low 3 采纳 7 不采纳有论证）；r2 = ocr-r2.raw.json（20 comments 0 high——r1 验证通过；1 critical run.rs String 桥接塌回 Internal[采纳→run_migration 链 CommandError 直传，照 MI-04b critical→修复→复验先例] / 3 medium 2 采纳 1 不采纳 / 16 low 6 采纳 10 不采纳）；r3 = ocr-r3.raw.json（19 comments：3 high 全采纳同根——create 后终态复验 verify_archive_dir_created / audit 补 disabled+count / 注释过度承诺精确化）；r4 = ocr-r4.raw.json（17 comments **0 critical 0 high**——r3 验证通过；6 medium 3 采纳[reject_parent_dir_components 契约文档 / has_root 补 Windows 根式路径 / confirmed 审计挪 save_rules 后] 3 不采纳[AppHandle 依赖不可单测 / Windows 无验证手段 / 三通道受众不同] + 11 low 轻量随批处置）。无 tool failure。**B 类 #1/#9 拍板（B / A）+ 已清登记见 §0。**

- **BOT-B（ea6317d，2026-09-25 09:41）OCR 核验记录**：r1 = ~/.openclaw/cache/BOT-B/ocr-r1.raw.json（status=complete / comments=4 全同根 io.rs 新代码：2 high 采纳[audit 挪写盘成功+放锁后 / strip 内核测试补齐] + 1 medium 采纳[读回等值比对→plaintext_strippable 加 wrote_now/plaintext] + 1 low 采纳[has+read 并单 read] / 0 failure）；r2 = ocr-r2.raw.json（comments=3 **0 high——r1 验证通过**：1 medium 不采纳[锁内 keyring IO=RMW 原子性 + migrate_legacy_key 既有约定] + 2 low 不采纳[合并用例无新覆盖 / contains 系辅助断言主断言 from_str is_err]）。无 B 类新增。

- **FE-B（3dc8e3a，2026-09-25 10:14）OCR 核验记录**：r1 = ~/.openclaw/cache/FE-B/ocr-r1.raw.json（status=complete / comments=9 全同根：3 high 采纳 + 3 medium 采纳 + 3 low 1 采纳 2 不采纳 / 0 failure）；r2 = ocr-r2.raw.json（comments=8：1 high 采纳[watch 清理时序——改无条件发起 load + finally 清] + 3 medium 采纳 + 4 low）；r3 = ocr-r3.raw.json（comments=7 **0 critical 0 high——r2 验证通过**：3 medium 2 采纳[load 链 catch 全覆盖 / 斜杠命令绕过拦截→守卫上移+/stop 白名单] 1 不采纳有论证 + 4 low）。无 B 类新增。**flaky 样本 +1（第 5 例）**：ChatPanel.test 流式合并「scrollTo is not a function」（jsdom 滚动桩偶发未生效）——隔离 13 绿 + 全量复跑 1137 绿双过判 flaky，与前端批改动无涉。

- **NEW-1345（9f50382，2026-09-25 12:10）OCR 核验记录**：r1 = ~/.openclaw/cache/NEW-1345/ocr-r1.raw.json（status=complete / comments=3 全 low 全 escape_for_log 微优化：2 采纳[extend(c.escape_default()) / capacity 去粗估] + 1 不采纳有论证[预替换中间分配保输出逐字节不变]）/ 0 failure / 0 high。无 B 类新增。
- **MI-B3（db6d8e9，2026-09-25 11:35）OCR 核验记录**：r1 = ~/.openclaw/cache/MI-B3/ocr-r1.raw.json（status=complete / comments=4 全 low：2 采纳[join 文案统一 / `??` 拆两步] + 2 不采纳有论证[泛型桥接 helper Send bound 复杂度 / AppHandle mock 成本]）/ 0 failure / 0 high。无 B 类新增。
- **EV-B（e753185，2026-09-25 11:10）OCR 核验记录**：r1 = ~/.openclaw/cache/EV-B/ocr-r1.raw.json（status=complete / comments=8 全同根：2 high 采纳[i==0 判定偏移 → first_parsed 首个非空行判定] + 4 medium 不采纳有论证[eprintln 留痕=同域先例、dedup 代价=拍板明示，注释声明] + 2 low 采纳[新测试 tempfile 化与断言精化]）；r2 = ocr-r2.raw.json（comments=8 **0 high——r1 验证通过**：1 medium 采纳[read_jsonl 泛型收敛两处 30 行重复] + 7 low）。无 B 类新增。
- **MI-B2（ab47f1b，2026-09-25 10:55）OCR 核验记录**：r1 = ~/.openclaw/cache/MI-B2/ocr-r1.raw.json（comments=6：2 high 采纳[检查点覆盖/清零时序] + 2 medium 1 采纳 1 不采纳 + 2 low 采纳）；r2 = ocr-r2.raw.json（comments=9：1 high 采纳[panic 绕过清零 → CancelGuard Drop] + 3 medium 2 采纳 1 不采纳 + 5 low）；r3 = ocr-r3.raw.json（comments=8 **0 critical 0 high——r2 验证通过**：4 medium 1 采纳[组合测试终态断言] 3 不采纳有论证 + 4 low）。无 B 类新增。
- **API-B（6de187b，2026-09-25 10:23）OCR 核验记录**：r1 = ~/.openclaw/cache/API-B/ocr-r1.raw.json（status=complete / comments=2 全 low 采纳：测试名/docstring 对齐实际行为[同 hub 重试由 last_id 归还断言覆盖，恢复段为独立 hub 正常路径] / atomic_write rename 失败残留同级 tmp 测试自清理[既有缺口 → NEW-5 登记]）/ 0 failure / 0 high 0 medium。无 B 类新增。

### B 类决策权威清单（2026-09-25 07:25 CST zcode 重建；**23 项已全部处置完毕（2026-09-25/26）**：20 项实修（7 批，各项见 → 标注）+ #7 判定 A（N=87）+ #14 wontfix + #23 前提不成立零代码）

> 重建方法：grep「转 B 类｜B 类候选｜单独立 B 类｜攒批」全量 + 每项源码 file:line 现场核对 + OCR fullscan（docs/OCR-CODE-REVIEW-2026-09-21-fullscan.json）原文复核。
> 编号：#1–#9 为本清单新赋（triage 原散落无编号）；#10–#23 沿用 triage 既有编号。triage 行号引用为本次插入前快照（本节插于 §4 累计区前，§0/§2 及 §4 前段行号不受影响）。
> 勘误：HANDOFF-2026-09-25 §3.1 表中 #10 写作「EV-3b-A 残 2 条之一（§4 ~557）」有误——§4:555-557 实为 add_allowed_dir 剥 key 项；EV-3b-A 残 2 条 = #17（§2:179）。以本清单为准。
> 选项标注：〔triage 已载〕= 选项原文来自 triage；〔重建构造〕= triage 未载选项，由主控按 triage 议题原文构造，仅供拍板参考。

- **#1** src-tauri/src/migration/rules.rs:27 —— load_rules 两臂静默降级空规则：解析失败仅 log_line + `RulesFile::default()`，读失败 `Err(_) => default()` 全静默；caller 后续 save 即把用户规则覆写为空（OCR high）。
  - A=quarantine〔triage 载方向〕：坏文件改名 `.bad-<ts>` 保全原文件 + 空规则继续——用户配置不被无痕覆写，可人工恢复；B=结构化 Err〔triage 载方向〕：load_rules → Result，caller（migration_rules_load / run_migration / migration_status 等 ≥3 处）显式分流 UI 报错 vs 降级——签名改 + ripple；C=维持现状（留痕已有，覆写风险留给 save 侧）。triage：§0:28 / §2:194
  - → **已拍 B（2026-09-25，caller 实测恰 3 处走主案）→ 已清（MI-B1，5b5f360；NotFound=首装合法态 Ok 空规则、读失败 IoError、解析失败 DomainRule；run.rs 迁移中止）**
- **#2** src-tauri/src/migration/commands.rs:116 —— migration_run 的 spawn_blocking 无 abort/cancel 接线，前端取消（关窗/退出）后 future 无人接收（OCR high）。
  - A=〔triage 载方向〕CancellationToken 贯穿 run_migration 各阶段（migration_run 签名改 + 取消语义「迁一半的文件怎么办」——staging 原子化后中止安全性需逐阶段核定）；B=〔重建构造〕不可取消语义文档化（任务后台自然跑完并落 journal，结果丢弃无害）；C=〔重建构造〕wontfix-with-rationale。triage：§2:197
  - → **已拍 A（2026-09-25）→ 已清（MI-B2，ab47f1b；AtomicBool 等价实现 + 阶段边界检查点 + migration_cancel command；staging/journal 原子化使取消安全）**
- **#3** src-tauri/src/db/paths.rs:50 —— copy_legacy_db 源库 open 失败（损坏/加密/IO）仅追加 warn 字符串，坏源静默继续传播（OCR high，C5-DB-05 成员）。
  - A=〔重建构造〕fail-closed：open 失败 → Err 拒拷贝，不产半成品目标库；B=〔重建构造〕维持 warn + 继续拷贝（便携首启容错优先——老库偶发锁定/权限时仍能拿到可用目标库）；C=〔重建构造〕warn 升级为迁移日志 ERROR + UI 可见提示，行为不变。triage：§2:170 / §0:30
  - → **已拍 A+C 组合（2026-09-25）→ 已清（DB-B，f137536）**
- **#4** src-tauri/src/db/workspace.rs:199 —— workspace_import_merge `it.updated_at.unwrap_or(0) > cur_ua`：导入行无 updated_at 当 0、库内行 NULL 也当 0——NULL 语义双侧重载（OCR high，C5-DB-05 成员）。
  - A=〔重建构造〕NULL→i64::MAX（无时间戳=最新，导入覆盖 NULL 目标行）；B=〔重建构造〕维持 NULL→0（无时间戳=最旧永不覆盖；库内 NULL 行可被任何导入行覆盖，现状）；C=〔重建构造〕遇 NULL 显式 Err（要求导入源带时间戳，破坏向后兼容）。triage：§2:170
  - → **已拍 C（2026-09-25）→ 已清（DB-B，f137536）**
- **#5** src-tauri/src/db/bot_history.rs:52 —— bot_history_save_inner 整批 inserted_at 用循环前单次 now，全量覆写后历史时间戳坍缩为一刻（OCR high，C5-DB-05 成员）。
  - A=〔重建构造〕每行独立 now（保真实时序，微成本）；B=〔重建构造〕维持单 now（语义=本次覆写的原子时刻，整批本来就是一次 overwrite）；C=〔重建构造〕now + 行序单调递增合成时间戳（无真实来源时保序）。triage：§2:170
  - → **已拍 C（2026-09-25）→ 已清（DB-B，f137536；核后 load 已 ORDER BY id，C 落地为不变量注释固化 + 回归测试）**
- **#6** src-tauri/src/bot_skills/vars.rs:76 —— in_quotes 字节级启发式对常见 JSON 形态（占位符所在字符串含转义引号等）误判——triage 已判 wontfix-with-rationale（占位符模板构造上非合法 JSON，启发式是结构必然），**待用户确认**。
  - A=确认 wontfix-with-rationale（triage 论证在案）；B=立项修（结构化 JSON 处理或 escaped-quote 前瞻，扩 scope）；C=暂缓观察（维持登记状态）。triage：§2:229
  - → **已拍 A（2026-09-25）→ 已清（BOT-B，ea6317d；设计边界注释 + 2 回归测试锁现状）**
- **#7** 全仓毒锁恢复点（db::lock_db_write 先例形态；BT-03a 语境 = CONFIG_WRITE_LOCK 等）—— 毒锁恢复后 eprintln vs audit_event! 落 bot.log：改即改 C3-1 约定（「logged 恢复即合法」现以 eprintln 满足）。
  - A=〔重建构造〕维持 eprintln（C3-1 现约定，零改动）；B=〔重建构造〕统一升级 audit_event! 落 bot.log（改 C3-1 约定 + 全仓 N 处恢复点统一 + audit_event! 需 AppHandle 的调用点改造，扩 scope）；C=〔重建构造〕仅新增锁（未来批）用 audit_event!、存量不动（双轨，约定分叉风险）。triage：§4:552
  - → **已拍 A（若 N≤5 则 B）（2026-09-25）→ 判定 N=87 处 >5 → 维持 eprintln，C3-1 约定不变（POISON-7 docs-only，§0）**
- **#8** src-tauri/src/migration/journal.rs（journal_pending）—— MI-04a 修复前历史遗留：既有库同 key 孤儿 pending 行清理 = 数据迁移方向。
  - A=〔重建构造〕启动 replay 前一次性清理（扫 journal，同 key 已完成 op 的孤儿 pending 删除 + 留痕——改迁移启动语义）；B=〔重建构造〕不清理（MI-04a 后 check-then-reuse，孤儿只占行数无行为危害——需核 replay 扫到孤儿时的实际行为）；C=〔重建构造〕提供手动清理命令/文档（运维面）。triage：§0:49
  - → **已拍 A（2026-09-25）→ 已清（MI-B2，ab47f1b；同 key 多 pending 留最新删其余 + 留痕；committed 共存不删）**
- **#9** src-tauri/src/migration/rules.rs:67 —— MI-08 残余：archive_dir 绝对路径 policy（`..` 组件拒绝已修 bbe4f59）+ symlink 逃逸校验 + 破坏性变更（move）用户提示。
  - A=〔重建构造〕三项全做（绝对路径拒绝 + canonicalize 后须仍在数据目录 + 执行前 UI 提示）；B=〔重建构造〕只做路径/symlink 校验，不加 UI 提示；C=〔重建构造〕只文档化威胁模型（cleanup-rules.json 属本地用户可控文件，威胁=用户自伤）。triage：§0:51 / §2:201
  - → **已拍 A（2026-09-25）→ 已清（MI-B1，5b5f360；三道闸 + create 后终态复验 + 导入破坏性确认框；确认框=「执行前提示」落地口径，run 期自动迁移无交互点已在 spec 声明）**
- **#10** src-tauri/src/bot/config/io.rs:161（add_allowed_dir）—— keyring 迁移成功后文件内明文 key 是否剥除（BT-03a OCR r5 medium 不采纳转 B）：防御纵深 vs 迁移误判成功时剥 key 写盘 = 密钥永久丢失（数据破坏方向）。
  - A=〔重建构造〕迁移成功后剥 key（防御纵深；前提=「成功」判定零误判）；B=〔重建构造〕不剥 + 文档化（文件副本=keyring 不可用时的降级兜底，明文留存是可用性取舍=现状）；C=〔重建构造〕剥 key 前 keyring 读回验证（belt-and-suspenders，实现最重）。triage：§4:555-557
  - → **已拍 C（2026-09-25）→ 已清（BOT-B，ea6317d；空槽写入+读回等值才剥 / 原有值非空即剥 / 读不回保留，OCR r1 采纳后含 audit 挪锁外+单 read 优化）**
- **#11** src-tauri/src/bot_skills/state.rs:164（clear_terminal_skill_runs）—— 终态 run 全局清理 vs 按会话清理（语义方向）。
  - A=〔重建构造〕按会话参数化（签名改 + caller ripple；避免 A 会话清掉 B 会话刚终态待读的 run——跨会话读窗口是否存在需先核）；B=〔重建构造〕维持全局清理（现状=入口一次性清全部终态）；C=〔重建构造〕带时间阈值清理（只清 N 分钟前终态 run）。triage：§2:228 / §0:65
  - → **已拍 A（2026-09-25）→ 已清（BOT-B，ea6317d；三调用点随动，跨会话隔离断言入既有测试）**
- **#12** src-tauri/src/bot_web.rs:881（行漂移，现 ~:920 Jina 回退段）—— fetch_text 直连失败回退 Jina Reader 时无审计留痕；补审计需 fetch_text(raw_url) 签名改 + 调用链 ripple。
  - A=〔重建构造〕签名改 + 审计参数贯通（Jina 回退记 audit，扩 scope）；B=〔重建构造〕不改签名，回退处 eprintln 留痕（stderr 可见不落 bot.log）；C=〔重建构造〕维持现状（注释已声明隐私边界「URL 发给 r.jina.ai」，仅审计缺口无安全洞）。triage：§2:226 / §0:71
  - → **已拍 B（2026-09-25）→ 已清（BOT-B，ea6317d；三路 stderr 留痕，签名不动）**
- **#13** src-tauri/src/bot/config/keyring.rs:168（write_key_file_to，现 :163 起）—— 降级文件存储的权限控制仅 unix 分支（0600 已修），Windows 无 ACL 等价（macOS 开发环境不可测）。
  - A=〔重建构造〕补 `#[cfg(windows)]` DACL 0600 等价（代码进仓但本仓无 Windows CI 验证手段）；B=〔重建构造〕wontfix-with-rationale（主平台 macOS，Windows 降级存储暂不承诺权限语义）；C=〔重建构造〕文档化声明（注释/README 载明 Windows 明文 key 权限不受控）。triage：§2:229
  - → **已拍 B（2026-09-25）→ 已清（BOT-B，ea6317d；doc 文档化，顺带修正 0600 直写陈旧注释）**
- **#14** src-tauri/src/bot/registry.rs:267（StopGuard 接线）—— ctx.stop 仅 tool_run_python 已接；全接线 = 13+ mutating adapter 工具签名改 + 内部取消检查（扩 scope）；循环级 4 处 stop 检查已覆盖「排在长调用后」场景。
  - A=全签名接线〔triage 已载〕（扩 scope）；B=adapter 层 pre-flight 检查〔triage 已载〕（语义增量小，循环级已近似覆盖）；C=wontfix-with-rationale〔triage 已载〕（StopGuard 设计意图=长操作+循环级中断）。triage：§2:235 / §0:89
  - → **已拍 C（2026-09-25）→ wontfix-with-rationale 维持，零代码**
- **#15** src-tauri/src/api_handlers/commands.rs:61 + :238 —— enabled flag 文件写/清在锁外 + 复核（service_present）后仍余 TOCTOU 微窗：并 api_stop/api_start 可交错致 flag 与内存态背离（下次启动自动恢复状态错一次）。
  - A=〔重建构造〕改「文件 I/O 不持锁」既有约定：flag 写/清移锁内（动约定，须与 AP-05 抬锁外方向做全仓一致性核定）；B=〔重建构造〕世代号/版本号 CAS 复核（写 flag 时带世代，消除窗口不加锁）；C=〔重建构造〕维持现状 + 注释文档化残余窗（误写后果可自愈：用户下次手动纠正）。triage：§2:211 / §0:91
  - → **已拍 C（2026-09-25）→ 已清（API-B，6de187b；两处注释补残余窗声明，零行为变更）**
- **#16** src-tauri/src/api_server.rs:127（AP-06 残留）—— broadcast 落盘 atomic_write 失败仅 eprintln 且 id 已推进：崩溃/重启后 SSE 事件历史缺一段（error-visible-non-blocking family）。
  - A=写失败拒推进 id〔triage 已载〕（fail-closed，SSE 事件暂停直至落盘恢复）；B=瞬时错误重试 ×N〔triage 已载〕；C=维持 eprintln 现状〔triage 已载〕。triage：§0:99
  - → **已拍 A（2026-09-25）→ 已清（API-B，6de187b；归还 id + 事件丢弃不投递，下次广播重取同 id 重试；事件丢弃为拍板明示接受）**
- **#17** src-tauri/src/evolution/change/record.rs:211 + candidate/entry.rs:110 —— read_all 逐行解析，单行 JSON 损坏 → 整读 Err（现状 fail-closed，一坏行拖死全部读取）。
  - A=〔重建构造〕维持 fail-closed（损坏立即可见，但 evolution 面板整挂）；B=〔重建构造〕fail-open：跳坏行 + 留痕（可用性优先；审计轨迹有缺口）；C=〔重建构造〕折中：首行坏 → Err，中间坏行 → 跳过 + audit 留痕（需定阈值依据）。triage：§2:179 / §0:109
  - → **已拍 C（2026-09-25）→ 已清（EV-B，e753185；read_jsonl 共享内核：首个非空行坏 Err / 中间坏行 stderr 留痕跳过；write_proposals Err 传播对齐）**
- **#18** src/App.tsx:240 —— tasks-updated 合并路径半成功：upsert 成功 + delete 抛错时 UI 不更新、不广播。
  - A=回滚已成功 upserts〔triage 已载〕（补偿写，复杂且自身可失败）；B=仍合并广播〔triage 已载〕（delete 失败行暂留 UI，下事件自愈）；C=维持现状仅靠 storage 层 alert 提示〔triage 已载〕。triage：§4:598
  - → **已拍 B（2026-09-25）→ 已清（FE-B，3dc8e3a；mutate 路径同治——OCR r1 high 一致性采纳）**
- **#19** src/App.tsx:191 —— 迁移失败 → 种子仍落库 → legacy localStorage 数据永久 orphan（finding 字面前提已证伪：removeItem 本就在 upsert 后）。
  - A=迁移失败时跳过种子落库、保留 legacy 待下次启动重试〔triage 已载〕；B=维持现状〔triage 已载〕（orphan 无害但不雅）。triage：§4:599
  - → **已拍 A（2026-09-25）→ 已清（FE-B，3dc8e3a）**
- **#20** src/components/ArtifactBatchDialog.tsx:79 —— skip 纯前端 setReady(null)，无永久 dismiss 路径（后端无 ack 协议，skip 后登记保留、下次同任务重弹）。
  - A=维持现状+注释文档化〔triage 已载〕（skip=这次不绑下次再问）；B=新增 dismiss Tauri command 清登记〔triage 已载〕（扩 scope）；C=skip 走 confirm 空 paths + 改后端早返为清登记〔triage 已载〕（后端行为变更）。triage：§4:604
  - → **已拍 A（2026-09-25）→ 已清（FE-B，3dc8e3a；零行为变更）**
- **#21** src/components/ConfirmMap/ConfirmMap.tsx:40 —— 新 bot-confirm 直接覆盖未响应 pending，旧 id 靠后端 60s 超时兜底拒（前端单窗 UI 是瓶颈，后端并发未决合法）。
  - A=前端队列逐个展示〔triage 已载〕；B=切换前自动拒绝旧 id〔triage 已载〕（用户未看到的请求被立即拒）；C=维持现状靠 60s 兜底+文档化〔triage 已载〕。triage：§4:605
  - → **已拍 B（2026-09-25）→ 已清（FE-B，3dc8e3a；busyRef 在途不补刀）**
- **#22** src/components/ChatPanel/ChatPanel.tsx:416 —— 围观执行会话期 Send 与 bot_execute_task 并发同会话（响应交错 + 收尾 bot_history_load 冲掉围观期输入）；busy 锁会连带堵 switchSession/newSession/deleteSession。
  - A=finding 原方案全 busy 锁〔triage 已载〕（围观期被困直到执行完）；B=专用 execWatchRef 只拦 Send〔triage 已载〕（切换会话自由，收尾 .then 里清）；C=维持现状 + 先核后端 bot_chat 同会话并发是否自有防重入〔triage 已载〕。triage：§4:606
  - → **已拍 B（2026-09-25）→ 已清（FE-B，3dc8e3a；后端 ChatGuard 已核在位=并发软拒兜底；watch 拦 Send 含斜杠命令，/stop 白名单）**
- **#23** src/components/EvolutionPanel/types.ts:24 —— ProposalTarget 的 tool_name/skill_name/policy 为 snake_case（verbatim 来自 serde response），camelCase 习惯 consumer 写 toolName 静默 undefined 但编译过。
  - A=字段 camelCase 重命名〔triage 已载〕（serde rename + 全部消费方改 = 跨域签名改）；B=字段级 JSDoc 文档化 verbatim 契约〔triage 已载〕（不改类型）；C=wontfix〔triage 已载〕。triage：§4:607
  - → **已拍 A（2026-09-25）→ 前提不成立零代码处置（reviewer agent-21 核验：TS/Rust 已逐字段镜像 snake_case、消费方零 camelCase 误用点、文件头注释钉死约定——finding 为假设性风险非现存缺陷，重命名纯 churn 不执行；勘误登记 §0）**

**已 triage（脚本 DOMAINS 12 域，不含 frontend）**: 145 unique

**其他域（含 frontend 9 簇 / 52 条）**: 126 unique

**Phase 2 实工单（已 triage 内，扣 BT-02 FP ×3 + BT-06 FP ×2 + BT-07b FP ×1 + DB-03 FP ×1）**: 138

**baseline**: 271 ± 2 unique non-vendor high / 162 unique path

**注解（口径说明）**: frontend 碎片 9 簇已完成聚簇，但 `scripts/triage-baseline-audit.py` 的 DOMAINS 列表当前未纳入 frontend，故 frontend 52 条落在 "其他域"。脚本 DOMAINS 待补后，两口径统一为「已 triage = 197 / 其他 = 74」。

（补充：vendor raw=33, unique=33, dups=0；non-vendor raw=278, unique=271, dups=7。unique dups 对详：`×2 src-tauri/src/app_state.rs:0 / ×2 src-tauri/src/bot_model_loop.rs:984 / ×2 src-tauri/src/bot_model_loop.rs:1079 / ×2 src-tauri/src/evolution/candidate/mod.rs:48 / ×2 src-tauri/tests/memory_v2_degraded.rs:24 / ×2 src/components/TodoCard/SubtaskRow.tsx:27 / ×2 src/components/TodoCard/SubtaskRow.tsx:48`）。

**首批开批**（1fcc418）：C5-DB-01a-2 (2 条) + C5-MI-02 (3 条) = 5 条 / 2 簇 / ≤+55/-34 行 / family = error-not-propagated / 跨 db + migration。

- DB-01a-3（mod.rs:251）+ AP-06（api_server.rs:127）→ 同一独立批 error-visible-non-blocking（log-and-continue 形态），2 簇 / 2 条，理由：family 异质 + 签名/调用点约束（详见 §3.5 异常 2）。
- tasks.rs:185 (i) 决策核验：9 调用点 = 7 dispatch 工具 + 1 bot_chat 后台 + 1 tasks_import Tauri command。8 Some + 1 None（tool_create_task）。M=1 语义变更：id 已存在且 updated_at 不满足谓词时，此前静默 no-op，现 Err。接受理由：创建语义下静默失败比报错更危险。

## 5. 约束 + SOP

**triage / 批组织 SOP**：
- 每批 ≤10 findings + ≤+500/−300 行
- 每行 ≤200 字符；超了标 需再读 拆 3–5 行 notes
- 同根因判据必须含 file:line 证据（不许 pattern matching 代替读代码）
- 批内多根因需如实标"批内 N 处独立改动，修复设施不共享"
- 跨域同模式必登记 family，跨域一致性检查时同时出现
- 不修代码、不跑 OCR、不 commit 代码（docs-only）
- triage 中发现的新问题登记 PHASE2-TRIAGE-NEW-*，下一轮评估
- 批次组织按 family + 语义同质，**不按"修复设施统一"**（family 必然跨多设施，这是 family 组织的固有属性）。“机械同质”指拆条动作重；“语义同质”指同一形状的改动（都是“停止吞错，让错误浮出来”）。
- commit message 用 family 一句话概括改动形状，逐条列各文件的本机修复机制
- 若某条修复伴随“产品行为变更 / schema 变更 / 跨域待核触达” → 该条移出本批，单独处理。不许用“标注一下”带进批

**baseline 核验 SOP（手工或脚本一律适用）**：
- 任何 baseline 数（总数、域数、文件数、dups）必须一次性 jq + sort -u 脚本输出验证 raw + unique 双口径，禁止人手算
- 脚本进版本控制，任何人重跑都得同一结果
- 断言含义应在跑之前定死；断言语义不得事后修改以通过
- 命令 + 原始输出必须同条内可核（不许“上一轮贴了”这种跨轮引用）
- baseline 变更时**所有下游产出**（簇、锚点、family、累计、跨域登记）必须随数字一起更新。不许“数字对账对到 145，簇清单还停在 199”。baseline 变 = 重跑下游全部

**OCR / pre-commit hook SOP**：
- 高批首选 r1 一次收口（r2 仅在有 critical 同根因已修时跑，验证修复）
- 高批基线例外 ±2 写进首批 commit message（不走“再来一轮”）
- commit message 必含：family 一句话 + 逐条 file:line → 本机修复机制 + 产品行为变更标注 + baseline ±2 + D2 三元组
- D2 三元组：行为断言 + 前置断言 + 反例断言（反例断言 = “若不满足 X，则行为是 Y”，不写“避免某错误”这种愿望陈述）
- **拍板动作遇阻碍停手报**：拍板动作被执行时遇到编译/架构阻碍（签名不允许、类型不匹配、需动 N 调用点）→ 停手报，不降级。降级 = 改拍板 = 越权。可选项由用户拍（首批 17:14 AP-06 自决降级为反面案例）。
- **批定性数字需自行加和核对**：拍板者提供的批性数字（条数 / 簇数 / 面积）不许沿用，必须自行加和后报（首批 13:40:34 拍板“4 条”实为 6 条，反身以该错数报上去）。
- **动作前必核 HEAD（APW-02a 16:54 教训）**：`git commit --amend` 默认改 HEAD，不显式指定就改错对象。动作前必核 `git rev-parse HEAD` 与目标 commit hash 是否一致；动作后必核 `git reflog | head -5` 看是否产生意外 commit。不许“以为记得”——以前重犯，commit chain 永久错位（APW-02a amend 误改 f9e5804 → 8f99c46 是 docs tree + code commit message 结构错位，需 force push 修正）。

**budget 估算法 SOP（APW-02a 2026-09-23 立）**：

budget 估算法按 diff 形态分两类：

A. 行内小改（`let x → let mut x` / 单 token 变更 / 单行 if 加）：
   每改动点 +1/-1

B. 结构性重写（函数体搬移 / 顶层重排 / 新增 helper / 整块嵌套替换）：
   按 git diff 整块计：旧连续块删 → -N；新连续块加 → +N
   不按“改动点数”算

判定方法：spec 起草时先脑跑 `git diff` 的形态——若结果会是“整段红绿”就是 B，若会是“零星红绿”就是 A。混用则两类分开算。

元规则：若预估与实际差 >1.5×，视为公式缺陷，不是精度问题。下次 spec 起草前先修公式；同一公式不许反复调参。

**budget 调整两类（APW-02b 2026-09-23 立）**：
- 估算错（实现前后差在 ±20% 内）：允许一次，不需 stop
- scope 变更（执行中发现新工作需纳入本批）：先 stop 报 reviewer，拍是否纳入本批，再统一 budget。不许直接调 budget 过 gate。

（APW-02a 案例：原估 +49/-21 用 A 类公式（行内小改），实际 +52/-1 是 B 类（结构性重写），差 >1.5×。修正 budget +86/-3（实测 = 上限）后 PASS。）

**PROC-5 type 3（429）观测规则（2026-09-23 reviewer 拍 D，下次 429 时执行）**：

背景：A/B/C 三选项均建立在"429 = LLM/gateway 配额耗尽"未证假设上；实测 token 全 0
（未到 LLM）、failure_details 空、间隔 ~2h 仍挂、1h57m 后第三次成功（证伪固定 2h 窗口）。
故拍 D：不改 OCR 工具、不改调用配置，只改"下次遇到 429 时的动作"。

1. **立即重试 3 次**，间隔 15s / 30s / 60s，记录每次时间戳 + 结果
2. 若 3 次内成功 → 短时抖动，选项 A（backoff 加大 base）适用
3. 若 3 次全挂 → 等 5min 再试 1 次；成功 → 窗口 5min 级，B（间隔 5min+）成主选；
   挂 → 等 30min 再试 1 次；成功 → 窗口 30min 级，需先拍"OCR 是否允许延后到批后
   异步跑"（与 batch discipline 冲突）；挂 → 窗口非分钟级，C（换 quota pool）成主选
4. 每次 429 的完整时间序列落 `~/.openclaw/cache/ocr-429-observations.log`；
   **同时记录当日累计 OCR 调用次数**（判定"偶发"vs"系统性限流"需全天数据）
5. n 到 3 次后回看数据，拍 A/B/C

**凭据 / 网络操作 SOP**：
- 涉及网络 / 凭据的操作（`git push` / `git fetch` / 远端 API 调用 / SSH 认证 / HTTPS token）在执行前，若用户未显式提供凭据，应先报“将使用本机 [机制] 凭据”让用户知情。不阻断操作，但留痕（pre-exec 报告 1 句即可）。
- 不报 = 凭据面下默认走本机 ssh-agent / osxkeychain / git credential.helper，用户事后可能不记得推送使用了哪个 key/账号。本 SOP 不追责“默认凭据调用”——仅要求“用户不知情时主动报”。
- 反例：commit / add / diff / log 本地操作不需要报——不触网络。

**待核（归下一批顺手核）**：
- scripts/test-fast.sh → cargo nextest 子进程清理超时是否每次 push 都触发？若是 → push 每次都留 orphan 进程，归入 PROC-5 同类。
