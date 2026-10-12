# OCR 全量扫描分诊账本（2026-10-12）

> 扫描：`ocr scan`（full_scan，会话 d855cb56，MiniMax-M3，审 383 文件 1587 条）。
> 本账本收录**新面** critical+high 205 条（基线 commit 9d9b82c 之后变更的 255 个文件上的全部 crit/high，
> 加 2 条未变文件上的新点位 idx 201/204）。分诊由 6 个只读代理完成，2026-10-12。
> 原始件：`OCR-SCAN-FULL-2026-10-12.json`（同目录，勿手改）。切分报告：`OCR-SCAN-REPORT-2026-10-12.md`。
> 未变文件上的其余 170 条 crit/high 复报 10-09 基线点位，判定沿用 `docs/OCR-SCAN-TRIAGE-2026-10-09.md`，不重复收录。
> 处置标记：✅ REAL 已修（本批）/ 🔧 PARTIAL 属实但前提或后果不成立，需人工复核 / 🛡 ALREADY_HANDLED 防线已在 / 📋 不修（建议性/取舍）/ ❌ FALSE_POSITIVE 误报。
> **后续扫描复报同点位凭本账本沿用判定，勿重查**。

## 判定分布：REAL ✅ 15 ｜ PARTIAL 🔧 30 ｜ 🛡 15 ｜ 📋 93 ｜ ❌ 52

REAL 15 条已全部修复（复现测试先行，见同日审计批提交）；🔧 PARTIAL 攒批待人工复核，不随本批修。

| 标记 | idx | 位置 | 要点（判定理由） |
|---|---|---|---|
| ❌ high | 0 | `src-tauri/dotnet/WmDocxRevisions/Program.cs:82` | 「Create 对已存在路径抛异常、用户拿不到产物」实测不成立：用本仓同版本（dotnet 10 + DocumentFormat.OpenXml 3.5.1）验证 Create 对已存在路径直接覆盖不抛异常，回退新建必然产出；ReviseInPlace 异常时 autoSave 留的是合法部分修订 docx 且随后被整体覆盖 |
| ❌ high | 1 | `src-tauri/dotnet/WmDocxRevisions/Program.cs:510` | 前提不成立：原文行与 run 映射同口径——ReadDocxLines(Program.cs:602) 复用 ParaText/InnerRuns，Python 提取（py/document.rs:30）用 python-docx paragraph.text 同样只数直接子级 w:r，一致性检查是同口径比同口径不会因 w:ins/w:del 触发；口径外结构整段删+增是 Program.cs:398-399 注释明示的保底设计 |
| 📋 high | 2 | `src/ui/main.css:1082` | 对比度数值属实（#e5484d/#ff6b62 对 #fff 约 3.9:1/2.8:1 低于 4.5:1），但 10.5px 时间胶囊属视觉打磨，仓库无 WCAG 门禁，暗色提亮是主题色结构锁锚定的设计决策，要修属设计侧拍板 |
| 📋 high | 3 | `docs/design-mock/task-timeline-mock.html:820` | docs/design-mock 静态设计演示件，不进产品构建与任何门禁，拖拽示意行为不构成产品缺陷 |
| 📋 high | 4 | `docs/design-mock/task-timeline-mock.html:791` | 同上，设计演示件的示意实现，坐标精度不进产品 |
| 📋 high | 5 | `docs/design/home-dashboard-mockup.html:197` | docs/design 静态 mockup，cursor:grab 是视觉示意；无障碍语义属功能落地实现时的事，mock 不修 |
| 📋 high | 6 | `docs/design/home-dashboard-mockup.html:472` | 静态 mockup 的示意数据刻度，不进产品构建与门禁 |
| 📋 high | 7 | `src-tauri/src/api_handlers/handlers.rs:51` | expect 输入是编译期常量合法 ASCII（Content-Type / application/json; charset=utf-8），tiny_http from_bytes 仅对非法 header token 报 Err——panic 不可达；「不变式注释+expect」为全仓既有惯例（157 处），OnceLock 化是零行为收益卫生建议 |
| 📋 high | 8 | `src-tauri/src/api_handlers/handlers.rs:483` | completed_at 语义=「当前处于完成态的时间戳」，离开 Done 清空是该字段既定语义而非审计丢失（非 Done 任务挂 completed_at 反而误导）；同请求显式 archived 入参在状态块之后应用（:529）不受清空影响 |
| 📋 high | 9 | `src-tauri/src/api_handlers/handlers.rs:506` | 建议性重构（抽 helper/换 source of truth）：双写点仅 create/update 两处且带注释锚定，无行为缺陷；YAGNI——第二个调用方出现前不抽象 |
| 📋 high | 10 | `src-tauri/src/app_state.rs:97` | 锁纪律已在各访问器注释钉死（「无守卫，锁只在 park/take 期间持有」等），模块头留档「保持 std Mutex 不引新依赖」取舍；扫 exec_steps/bot_slash/workflow_questions 未发现跨 await 持锁点，改 tokio Mutex 属无实证违规点的防御性重构 |
| 📋 high | 11 | `src-tauri/src/app_state.rs:115` | 模块头 :80-81 已明示集成测试走兜底实例、公开面不扩是本阶段拍板（「要给它一个入口需另议」）；无实际 flake 报告前不动 |
| 📋 high | 12 | `src-tauri/src/api_server.rs:139` | 代码注释 :135-136 已明确反驳该修法：「落盘移出锁需另加同步才能保住落盘序=id 序，代价大于收益」，事件频率人级；单写者临界区是有意设计 |
| ❌ high | 13 | `src-tauri/src/audit.rs:332` | lib.rs:11 `mod audit;` 为私有模块，pub static 实际可见性=crate 内，不存在「暴露给整个 crate 图」的外泄面；三处写入点共用该锁是注释明示的设计 |
| 📋 high | 14 | `src-tauri/src/audit.rs:404` | 基线同点位（10-09 audit.rs:465 判 📋 WONTFIX）：frontend_event_report 是同步 command，持锁阻塞 IO 已 eprintln 留痕，审计写入允许 best-effort 是既有口径 |
| 📋 high | 15 | `src-tauri/src/audit.rs:351` | S1-1 定案口径即「补 chmod + 失败 eprintln 留痕」（与 S1-3/S1-6 best-effort 同级）；Unix 下 chmod 失败而后续 append 成功的组合近乎不可达（同属主失败≈写也会失败） |
| 📋 high | 16 | `src-tauri/src/bing.rs:50` | 冷缓存并发双拉取后果=同内容重复写+一次冗余 HTTP（每天最多一次），写内容恒等不会互踩，无正确性影响 |
| 📋 high | 17 | `src-tauri/src/bing.rs:48` | 毫秒级一次性磁盘 IO（~400KB 读+base64）；与基线 tools.rs:1255「async fn 内同步 fs 判 WONTFIX」同款口径 |
| ❌ high | 18 | `src-tauri/src/bot/config/commands.rs:294` | 前提不成立：`let _g = lock_config_write()`（:297）先于 load_config（:298）获取并持有到 write（:301），load→write 全程同锁，正是 S1-8 修复形态；扫描器把「load_config 内部不加锁」误读为「读不受锁保护」 |
| 📋 high | 19 | `src-tauri/src/bot/config/commands.rs:658` | 基线同点位（10-09 commands.rs:653 判 📋 WONTFIX）；且 docstring :640 明示「全局主 key 读不到（未配置/故障）按无 key 探活，状态如实返回」是有意口径，厂商 key 分支真实故障仍上抛 |
| 🛡 high | 20 | `src-tauri/src/bot/config/types.rs:56` | 防线已在：keyring.rs:164-208 write_key_file_to「创建即 mode(0o600) + tmp+rename + 已有文件补 set_permissions(0o600)，失败记 WARN 审计」，0600 不变式在打开点强制，finding 的「若 opener 不用 0600」假设与实现不符 |
| 🛡 high | 21 | `src-tauri/src/bot/config/mod.rs:531` | 防线已在：keyring.rs:455-500 migrate_legacy_keyring_entry 实现了 finding 要求的完整迁移路径（读取回落 v0 → 复制 v1 → 删 v0，幂等+故障下次再试+审计事件），不止测试钉字面量 |
| 📋 high | 22 | `src-tauri/src/bot/config/keyring.rs:663` | 「竞态」结果（回落全局 key）与「厂商 key 未配置」的既定回落路径完全一致，全为用户本人凭据无越权面；触发需本机用户在毫秒窗内经外部工具手删 keychain 条目，单人应用前提不成立，单次读取重构不改变可观察行为 |
| 📋 high | 23 | `src-tauri/src/bot/config/io.rs:39` | 基线同点位（10-09 io.rs:568 判 📋 WONTFIX）；io.rs:31-32 注释明示「keyring IO 不拆出锁——拆则读→keyring→写竞态重开，启动期一次性路径可接受」是有意取舍 |
| 🔧 critical | 24 | `src-tauri/src/bot/config/io.rs:326` | 属实半边：from_value().unwrap_or_default() 吞 JSON 类型错误违反仓库不吞错误口径，且 store_server_secrets 无 has-first、全空入参会删除既有条目；不成立半边：「clobber 用户新值」需 手改坏形配置+前次部分迁移/编辑失败残留 多重前提叠加，且有迁移前 0600 备份文件作回滚网——需人工复核是否把解析错误改上抛 |
| 📋 high | 25 | `src-tauri/src/bot/config/io.rs:479` | 注释 :469-470 明示「只警告不拒写——本地推理服务是合法场景，且不能破坏存量用户配置」是有意取舍；单人应用 base_url 是用户自己配的 |
| ✅ high | 26 | `src-tauri/src/bot/config/io.rs:196` | write_config_atomic docstring（io.rs:77-80）明示 bot-config.json 崩溃安全要求 fsync「故不复用 db::atomic_write」，而 write_memory_tuning_at 写同一文件却走无 fsync 的 db::paths::atomic_write（db/paths.rs:38 仅 write+rename）——同文件双标准；且该函数已持 lock_config_write 满足其 debug_assert 契约｜修法：:196 改调 crate::bot::config::io::write_config_atomic(path, &raw) |
| ❌ high | 27 | `src-tauri/src/bot/config/io.rs:350` | 前提不成立：全部调用方（io.rs:384/503/544/579、mcp/commands.rs:45 均 `?` 上抛；lib.rs:351 启动路径失败只 eprintln 且不触发任何配置写回）都不会在 Err 后继续写配置；外科手术式剥离仅在全部成功后执行 |
| ❌ high | 28 | `src-tauri/src/bot/registry.rs:521` | tool_use_skill 是同步 fn 直接返回 ToolResult（bot_skills/runtime.rs:165 `pub fn ... -> ToolResult`），async 块求值即 ToolResult，不存在 Future 套 Future；类型对齐由编译门禁证明 |
| 📋 high | 29 | `src-tauri/src/bot/registry.rs:501` | 三条「待确认不变式」均已在位：workflow_questions.rs:589-598 select 每 500ms 轮询 stopped()、:600-607 等待结束无条件摘除 waiter、&StopGuard 借用使悬空守卫在安全 Rust 不可表达；future 中途被 drop 漏摘 waiter 属基线 bot_chat.rs:1809 同款已判 WONTFIX |
| 📋 high | 30 | `src-tauri/src/bot/tools.rs:1302` | TOCTOU 触发需本机进程在 canonicalize 与 python 打开间的毫秒窗内换软链——单人应用无此攻击者（模型只能提议路径不能改文件系统），失败后果也只是模型读到本机另一文件 |
| 📋 high | 31 | `src-tauri/src/bot/tools.rs:1241` | 基线同点位（10-09 tools.rs:1255 同函数同主张判 📋 WONTFIX）：同步 canonicalize 毫秒级，单人桌面应用既有取舍 |
| 📋 high | 32 | `src-tauri/src/bot/tools.rs:1256` | 基线同点位：基线 59 描述即「walks every task and calls std::fs::canonicalize for each bound file on the async worker」，本条为其细化，同判 WONTFIX |
| 📋 high | 33 | `src-tauri/src/bot/tools.rs:1168` | 与基线 59 同款取舍：exists+canonicalize 微秒~毫秒级本地调用，包 spawn_blocking 只加样板无行为收益 |
| 📋 high | 34 | `src-tauri/src/bot/mcp/secrets.rs:142` | 边界校验已在：mcp/config.rs:135-145 validate_server 对 id 做长度上限+控制字符校验（正常写路径均过）；手改 json 的极端脏 id 只会 keyring Entry::new 报错→迁移响亮中止明文保留，无注入/越权面 |
| 🛡 high | 35 | `src-tauri/src/bot/mcp/secrets.rs:302` | 基线同点位（secrets.rs:301 🛡）：S1-2 修复已在位，write_tmp_0600 在 rename 前无条件收紧 0600（secrets.rs:315-322），残留暴露面只剩毫秒级写窗口 |
| ❌ high | 36 | `src-tauri/src/bot_anthropic.rs:119` | 基线同点位（bot_anthropic.rs:119 ❌）：audit::tool_call_failed 首行 `let _ = name;` 显式忽略工具名、只判文本（audit.rs:130-141），空串对分类零影响 |
| ❌ high | 37 | `src-tauri/src/bot_anthropic.rs:288` | 非对象兜底空 object 是行 278 注释明示的约定；工具 schema 全来自内部注册表/MCP（恒为对象），且 Anthropic input_schema 要求 object，透传数组/标量反被 API 拒——前提不成立且修法有副作用 |
| 🛡 high | 38 | `src-tauri/src/bot_anthropic.rs:474` | 幽灵 tool_call 有双防线：出站转换层跳过无 id/name 条目并连带丢弃孤儿 tool_result（bot_anthropic.rs:61-62、83-87、111），主循环收尾按 name 为空 retain 兜底（bot_model_loop.rs:1334-1347）；前提还需 Anthropic 违约发缺 id/name 的块 |
| 🛡 high | 39 | `src-tauri/src/bot_anthropic.rs:499` | 同 idx 38 根因：input_json_delta 幽灵条目同样被 bot_model_loop.rs:1334-1347 的幽灵兜底与出站跳过防线拦下 |
| ✅ high | 40 | `src-tauri/src/bot_skills/builtin.rs:63` | fs::write 中断留下截断 SKILL.md，若 frontmatter 完整则 versions_match 判 Unchanged（builtin.rs:60、76-81）自愈不触发、破损永久（比 finding 声称的更重）；写兄弟临时文件再 rename 覆盖即可 |
| ✅ high | 41 | `src-tauri/src/bot_py.rs:87` | 测试注释声称锁「排空到 EOF」但只断言 buf.len()==cap，删掉 py/io.rs:20 排空 copy 该测试照样通过，承诺的回归锁不住｜修法：测试改用计数 Read 适配器包 Cursor，断言底层被读满 cap+100 字节（排空到 EOF） |
| ❌ high | 42 | `src-tauri/src/bot_fs.rs:56` | 非法 action 在落盘清洗层即被剔除且同 tool 去重（sanitize_tool_rules，config/commands.rs:142-167），[bogus,deny] 形态正常写入路径不存在；手改配置属本机可信输入（单人本地口径），None 兜底走用户自己的 perm_mode 而非放行，行为有测试锁定（bot_fs.rs:100-109）属既有取舍 |
| 📋 high | 43 | `src-tauri/src/bot_fs.rs:1011` | 机制属实但攻击前提是本机另一进程抢竞态窗口，单人本地无权限边界可穿（能换 inode 即能直接读该文件）；缺口已在代码注释登记为 Phase 6 follow-up 设计债（bot_fs.rs:1286-1339） |
| 📋 high | 44 | `src-tauri/src/bot_orchestrator.rs:985` | 同步 DB 读是注释明示的评审拍板节流取舍（bot_orchestrator.rs:982-983：open_db 不该 50ms 一次压 async worker → 500ms 粒度）；本地 SQLite 单行读亚毫秒级，stall 后果不成立 |
| 🔧 high | 45 | `src-tauri/src/bot_orchestrator.rs:1208` | 机制属实（Cancelled→Failed 被 update_subagent_status 拒绝、`let _` 吞错），但后果只是极罕见双重竞态下残留一个空 bot_sessions 行（会话本按围观/审计设计持久保留），无数据完整性问题；两个建议修法均有副作用 |
| 📋 high | 46 | `src-tauri/src/bot_scheduler.rs:1239` | mutex poison 前提是进程内已有 panic 的异常态；unwrap_or(true) 静默跳过是防定时作业重复执行的 fail-safe 取舍（注释「与扫描分发同语义」在位），补审计行属建议性 |
| ❌ high | 47 | `src-tauri/src/bot_fs.rs:56` | 头条与 idx 42 同点位同判（清洗层+可信输入+测试锁定）；捆绑的 replace_all 重复计算/walk 吞错/ErrorKind/ToolResult::ok/expand_tilde 各子项均为建议性或 finding 自注「已登记设计债」 |
| 📋 high | 48 | `src-tauri/src/bot_fs.rs:1003` | 同 idx 43 根因（edit 路径无 inode re-check），Phase 6 已登记、单人本地威胁模型下后果不成立 |
| 🔧 high | 49 | `src-tauri/src/bot_model_loop.rs:850` | mutation_done 会话级不重置与 851 行注释「本轮」措辞确有出入，但「补一轮机会每次对话只用一次」是注释明示的设计：守卫只在无工具调用的收尾轮检查，按轮重置会把真实变更后的正常汇报轮误判为幻觉打回——建议修法有副作用 |
| ✅ critical | 50 | `src-tauri/src/bot_skills/manage.rs:705` | 拖入外来技能目录属不可信输入口径：FIFO 落到 fs::copy 读取端永久挂起 IPC 线程、深嵌套递归可栈溢出；同函数已有 symlink 防线恰证明该面在威胁模型内｜修法：copy_dir_all 用 entry.file_type() 只放行 is_file()（顺带覆盖 FIFO/socket/device/symlink），递归加深度上限 |
| ✅ high | 51 | `src-tauri/src/bot_skills/manage.rs:365` | 同上口径：无上限 read_to_string 遇超大 SKILL.md 整文件进内存（IPC 线程 OOM/长挂）｜修法：先 fs::metadata 校验 ≤256KB 再读（对齐 CREATE_MAX_SKILL_CHARS 既有策略） |
| ❌ high | 52 | `src-tauri/src/bot_fs.rs:47` | 与 idx 42 同点位（C1b 失败语义），同一反证 |
| 📋 high | 53 | `src-tauri/src/bot_fs.rs:934` | 同 idx 43/48 根因（edit_file_sync 裸读无 inode 校验），Phase 6 已登记 |
| 📋 high | 54 | `src-tauri/src/bot_skills/scheduler.rs:372` | 「每 step 前查状态机」是协作式取消的设计语义（scheduler.rs:368-371 注释），在途 /stop 由 execute_tool 停止令牌与下一步边界兜住；把活状态读进 advance_dsl 不消除在途窗口 |
| 📋 high | 55 | `src-tauri/src/bot_skills/scheduler.rs:157` | 回滚 continue 是「尽力而为、最大恢复面」的既有语义：逐步审计留痕+failed_steps 汇总上报；首败即 break 反而留更多未撤销状态 |
| ❌ high | 56 | `src-tauri/src/bot_skills/runtime.rs:63` | 基线同点位（runtime.rs:58 ❌ critical）：64-66 行先把一切非对象归一为 Null，serde_json IndexMut 对 Null 的语义是替换为空对象再插入（index_or_insert），只有 Array/标量会 panic 而它们已被归一，panic 路径不可达 |
| 📋 high | 57 | `src-tauri/src/bot_skills/runtime.rs:289` | 单人本地应用：args 预览进本地审计/回执/lesson，读者与数据主体同为本人、无越权读者（与基线 dispatch.rs:261「审计预览含敏感数据」📋 同族同判） |
| ❌ high | 58 | `src-tauri/src/bot_skills/runtime.rs:220` | 外来技能唯一安装入口 copy_dir_all 跳过 symlink（manage.rs:712-714），references 软链根本进不了已安装目录；手放软链=本机用户自己；且列表仅暴露 .md 文件名、实际读取仍走 read_text_file 白名单闸，越界路径读不动 |
| 📋 high | 59 | `src-tauri/src/bot_skills/vars.rs:83` | expect 锁的是「find_iter 产出的 match 必可再 captures」的 regex crate 结构不变量，属断言非吞错；改 continue 反而把不变量破坏静默化，「未来改 regex/换版本会 panic」属推测（YAGNI） |
| 📋 high | 60 | `src-tauri/src/bot_skills/vars.rs:65` | serde_json::to_string(&str) 不可能失败（str 序列化无错误路径），unwrap_or_default 是不可达分支防御；换 expect 只为不可能场景加 panic，属口味建议 |
| 📋 high | 61 | `src-tauri/src/db/mod.rs:490` | 现无任何递归取锁路径（有则上线即死锁暴露）；poison 走 [mutex_poisoned] eprintln+into_inner 是全仓库统一约定（scheduler/orchestrator/runtime 同款），取锁前加断言属防御性建议 |
| 📋 high | 62 | `src-tauri/src/bot_web.rs:1082` | Jina 回退的隐私边界是注释明示的既有拍板（bot_web.rs:1073-1077「目标 URL 会发给 r.jina.ai（公网地址本身，低风险）」+三路 stderr 留痕）；stderr 是本机日志、无越权读者 |
| 📋 critical | 63 | `src-tauri/src/db/paths.rs:43` | atomic_write 服务于配置/JSON 小文件，write+rename 不 fsync 是桌面应用常见取舍，掉电最坏后果是配置回默认可重建；SQLite 库文件自身有 WAL+synchronous 保护（conn_pragmas 测试锁死） |
| 📋 high | 64 | `src-tauri/src/db/paths.rs:42` | 同目标并发写需绕过各自域锁（CONFIG_WRITE_LOCK/DB_WRITE_LOCK），单用户单实例下不存在；PID 后缀属建议性加固 |
| 🔧 high | 65 | `src-tauri/src/db/paths.rs:141` | 2a→2c 间崩溃留孤儿 -wal 属实，但正常重导路径 2a/2b 的 rename 会覆盖孤儿边车，「孤儿与新建空主库配对」还需叠加「二跑时源库恰好无 wal」的巧合，后果链不成立；且入口契约守卫（paths.rs:70-78）已挡 db_path 存在场景 |
| ❌ high | 66 | `src-tauri/src/db/paths.rs:28` | Rust std::fs::rename 在 Windows 走 MoveFileExW(MOVEFILE_REPLACE_EXISTING)，目标存在时覆盖而非 ERROR_ALREADY_EXISTS；paths.rs:27 docstring「旧 .old 覆盖」与实现一致；基线 paths.rs:37 同一误解已判 ❌ |
| 📋 high | 67 | `src-tauri/src/db/tasks.rs:762` | finding 声称「无注释辩护」与代码不符：tasks.rs:771-772 明示「尽力而为：无 brief_entries 表的连接上不炸主删除」（与 delete_tasks 同口径）；事务化会让级联失败回滚整个主删除，与文档化语义相反 |
| 📋 high | 68 | `src-tauri/src/bot_skills/parse.rs:376` | DSL 契约即「单行、结尾 ）」（parse.rs:375 注释），合法输入切片恒正确（finding 自认）；畸形输入在下游 JSON 解析以错误暴露，括号深度扫描属报错质量打磨 |
| 📋 high | 69 | `src-tauri/src/bot_skills/parse.rs:466` | 多行 JSON 写法本不在 DSL 内；空 tool_name 步骤在运行期以「未知工具：」报错使技能可见失败（非静默 no-op），补 authoring 时校验与既有编号校验同族、属建议性加固 |
| 🔧 high | 70 | `src-tauri/src/db/workflow_audit.rs:282` | 基线同点位：导出走 save-dialog 用户自选本地路径、体量 KB-MB 级，high 高估；write 挪进同一 spawn_blocking 属可选优化（基线 146 同判） |
| 📋 high | 71 | `src-tauri/src/db/workflow_audit.rs:163` | keep_runs==0 全清是测试钉住的刻意语义（workflow_audit.rs:433），三个生产调用方均读经 settings_set 5..=100 钳制入库的值，0 只能来自手改 DB；为假想调用方加 guard 属投机加固 |
| ❌ high | 72 | `src-tauri/src/error.rs:0` | 反证 src/bot_chat.rs:1171 与 src/bot_model_loop.rs:1066：body_preview 两处构造点均已 chars().take(300) 截断，multi-MB 串到 message() 的前提不成立 |
| ❌ high | 73 | `src-tauri/src/db/workflow_settings.rs:152` | 反证 src/db/mod.rs:155-156：busy_timeout=2s 是连接级 PRAGMA，unchecked_transaction 只跳过嵌套事务检查不绕 busy handler，且进程内写者已由 DB_WRITE_LOCK 串行化 |
| 📋 high | 74 | `src-tauri/src/evolution/change/derive.rs:88` | 函数注释已点名「三家写者落行前统一经此派生」的锁约定，改签名收 MutexGuard/加 try_lock 断言属建议性加固 |
| 📋 high | 75 | `src-tauri/src/evolution/change/derive.rs:113` | 「入口须已过 gate、Rejected 流转即 Err」已写入函数文档且 apply.rs:556 有测试钉住该 Err 行为，显式 gate 调用属防御性重复，「未来放宽终态规则」是假想前提 |
| 🔧 high | 76 | `src-tauri/src/evolution/mod.rs:182` | join 失败 repanic 属实，但 expect 使失败可见（tokio 记 task panic）而非静默丢弃，建议的 eprintln 吞错与仓库「不吞错误」纪律冲突，需人工定夺 |
| 🔧 high | 77 | `src-tauri/src/db/workflow.rs:198` | 吞错属实但 attachments 列仅由本应用 serde_json 写入，损坏前提是手改/磁盘损坏（本机单人场景几乎不可达），后果只是附件显示为空；inspect_err 留痕一行可选做 |
| 📋 high | 78 | `src-tauri/src/evolution/proposal.rs:104` | 基线同点位（204 判 WONTFIX）：newtype/enum 化属建议性，隐私契约由注释+构造点自查承担，无现实泄漏路径 |
| ❌ high | 79 | `src-tauri/src/evolution/proposal.rs:74` | 反证：全仓 ProposalTarget 构造点 payload 均为代码字面量（src/evolution/derive.rs:132 等），无外部名进 tag()；且 normalize_for_hash 尾段无竖线，「类别→tag→norm」竖线拼接字符串级可逆无碰撞 |
| 📋 high | 80 | `src-tauri/src/lib.rs:151` | 基线同主题（221 判 WONTFIX）：退出清理是文档化尽力而为设计（模块注释明示「不强等」），轮询任务随进程退出自然终止，补全 stop 句柄属建议性加固 |
| 📋 high | 81 | `src-tauri/src/lib.rs:336` | 启动期迁移失败 eprintln+注释明示「后续任何配置写路径会重试迁移」的幂等尽力而为模式，与启动期其他清理失败同款处理，升级审计事件属建议性 |
| 🔧 high | 82 | `src-tauri/src/memory/extract.rs:604` | auto 分支裸 autocommit 逐条落库属实，但中途失败以 Err 上抛步骤可见失败、下轮反思重提取+去重可自愈；与 Confirm 分支对齐包 conn.transaction() 属低风险改进，需人工定夺 |
| 📋 high | 83 | `src-tauri/src/memory/extract.rs:85` | pending_insert 已是私有函数且唯一生产调用方 extract.rs:639 显式传 &tx，「未来调用方忘包事务」属投机前提 |
| 📋 high | 84 | `src-tauri/src/migration/journal.rs:31` | 锁约定已在函数文档明示且唯一生产入口 journal_pending 内部持 db_write_lock，inner 仅测试直调；加 UNIQUE/UPSERT 属防御性改造（YAGNI） |
| 🔧 high | 85 | `src-tauri/src/migration/journal.rs:0` | 基线同点位（252 PARTIAL/253 FP）：存查两侧同用 to_string_lossy 自洽，None 路径已拒绝并留痕；碰撞需同库两个仅差非 UTF-8 字节的路径，现代系统实际不可达 |
| 📋 high | 86 | `src-tauri/src/migration/recovery.rs:138` | 基线同点位（243 判 WONTFIX）：状态翻转失败行留 pending、下次启动 replay 重试是文档化收敛机制，log_line 已留痕，.ok() 属既有取舍 |
| 🔧 high | 87 | `src-tauri/src/migration/recovery.rs:214` | TOCTOU 半边不成立：db_upsert 已有 expected_updated_at 冲突拒写防线（src/db/tasks.rs:338）且 recover_move_db 已设快照基线；O(N*M) 半边属实但 M=pending 条目极小且仅启动期跑，属可忽略的建议性优化 |
| 🔧 high | 88 | `src-tauri/src/migration/run.rs:123` | due 二次 emit 属实但载荷与首次完全相同，前端按 id upsert 幂等无用户可见差异；删 run.rs:127 的 emit 属可选清理 |
| 🔧 high | 89 | `src-tauri/src/ocr.rs:38` | 基线同点位（244）：macOS 上 file:///UNC 被 resolve_with_perm canonicalize 二道闸拒绝，Windows UNC 可达但读局域网共享不算「上传外网」；按基线把 file: 等前缀补进拒绝清单是一行事 |
| 📋 high | 90 | `src-tauri/src/ocr.rs:195` | 推理 panic 是假想前提（推理错误走 Result 已处理），中毒后 lock().map_err 返回 Err 是可见降级而非静默损坏，重启恢复；catch_unwind+Mutex<Option> 重构属投机复杂度 |
| 📋 high | 91 | `src-tauri/src/paths.rs:85` | 后果只是重复一条 WARN 审计，首次探测实际发生在启动单线程早期，并发首探概率趋零；CAS 化属建议性加固 |
| 📋 high | 92 | `src-tauri/src/paths.rs:105` | 独立线程是为避 BOT_LOG_LOCK 死锁的注释明示取舍，丢的只是诊断日志一行；supervisor 通道属建议性改造 |
| ❌ high | 93 | `src-tauri/src/py/document.rs:791` | 反证 src/py/document.rs:790-798：类型翻转循环嵌在 for row 循环体内、每行 append 后立即执行，所有行均被清洗，「只洗最后一行」是嵌套误读 |
| ❌ high | 94 | `src-tauri/src/task_out.rs:81` | 反证：Task/TaskOut 无 skip_serializing_if（src/db/tasks.rs:169-246 仅 serde(default)），bot_assigned:None 序列化为 null 字段仍在，camelCase 失效时断言两分支同时为假会失败，测试真实生效 |
| 📋 high | 95 | `src-tauri/src/tag_similar.rs:37` | 基线同点位（279 判 WONTFIX）：全程持锁防缓存击穿是注释拍板的取舍，词表小调用频率低，串行代价可忽略 |
| ❌ high | 96 | `src-tauri/src/workflow_clarify.rs:180` | 反证 src/bot_chat.rs:1154-1156：per-request 60s timeout 已存在；且 provider Err 即 break 不重试（重试仅限校验失败并改写 prompt），「无限阻塞/瞬时失败连发两次」两半前提均不成立 |
| 🔧 high | 97 | `src-tauri/src/workflow_questions.rs:446` | 扣减先行、insert 失败不回冲属实，但「无信号」半边不成立（Err 分支已落 insert_failed 审计 Warn），后果仅预算少 1 次且降级假设路径是红线设计内行为；改为成功后扣减或失败回冲需人工定夺 |
| 🔧 high | 98 | `src-tauri/src/workflow_questions.rs:484` | 窗口客观存在但 notif_insert 完成与 waiter 注册之间无 await（亚毫秒），应答入口又是 emit/OS 通知之后的用户动作，现实不可达；要堵可让应答端短暂复查 waiter 表，需人工定夺 |
| 📋 high | 99 | `src-tauri/tests/exec_trace.rs:349` | 5s 轮询窗口已宽松，加 trace_sink flush 屏障属测试基建增强（建议性），断言失败信息增强一行可选 |
| 📋 high | 100 | `src-tauri/tests/exec_trace.rs:86` | 注释已实锤该分歧且按 session_id+task_id 双路删除兜底清场，仓内无按 session_id 聚合失败行的消费方；透传 caller session_id 属上游契约改进建议 |
| ✅ high | 101 | `src-tauri/src/workflow_runner.rs:578` | contains_key 与 insert 之间夹着 await 的 DB 加载，双击/调度+手点可双过闸，第二个 insert 静默顶掉首个 RunHandle 致同图双跑且首个失去取消入口；把占位 Entry 插入挪进第一段锁内、早期失败路径删除占位 |
| 📋 high | 102 | `src-tauri/tests/task_chat_exec.rs:5` | 文件头注释已明示「current-thread runtime 下故意持锁跨 await」的前提与依据，tokio::sync::Mutex 化属建议性加固 |
| 📋 high | 103 | `src-tauri/tests/task_chat_exec.rs:268` | runner 在 guard 获取后才同步被调用的顺序由现有实现保证，测试失败即响；为假想重构加 exec_guard introspection 属测试过度设计 |
| ❌ high | 104 | `src-tauri/Cargo.toml:89` | 反证 Cargo.toml:108-110：注释已明示「特性取并集 → ort-sys disable-linking、构建期不下载不链接」是有意设计，发现声称注释不了解特性统一与事实相反，且无构建失败实证 |
| ❌ high | 105 | `src/components/HomePage/homeData.ts:72` | 唯一生产调用方 DueTicker.tsx:83 传入的是 dueSoonTasks 输出（homeData.ts:104 已滤 due===null），且 JS 中 null-数字 不抛 TypeError 而是按 0 折算，「运行时 TypeError」前提双重不成立 |
| 📋 high | 106 | `src/components/HomePage/homeData.ts:100` | 含逾期是文档化口径（homeData.ts:95「含逾期」、98「截止在今天以前 ~ 未来 horizon 天内」）且有测试钉住（homeData.test.ts:83「含逾期与 7 天内截止」断言逾期任务列最前）；未完成的老逾期任务正是该曝光的，加下界反而藏任务 |
| ❌ high | 107 | `src/components/EvolutionPanel/types.ts:52` | 反证 src-tauri/src/evolution/change/record.rs:129-135：Rust ApprovalSource 枚举刻意含 Pending（注释「还没批准（Pending 状态时）」，derive.rs:57 同步产出），TS 类型是忠实镜像，删掉反而与线协议脱节 |
| 📋 high | 108 | `src/components/GraphPage/graph-adapter.ts:34` | 「缺失序号归 0」是被测试钉住的刻意兜底（graph-adapter.test.ts:127-142 断言 owner:0），且 ownerOrder 由 GraphPage 从同一任务集经 collectOwners 构建，未映射 owner 实际不可达 |
| 📋 high | 109 | `src/components/GraphPage/graph-adapter.ts:114` | ownerKey 唯一生产者 ownerColorKey 只产 "self"/"owner:<有限数字>"（?? 0 兜底），"owner:abc" 不可达；非法键落 t5 已有测试钉住（graph-adapter.test.ts:153），Number.isFinite 守卫属投机加固 |
| ❌ high | 110 | `src/components/ChatPanel/useChatUi.ts:57` | WebView 触摸点按会补发合成 mousedown（compatibility mouse events，本仓无 touchstart preventDefault 用例），点外部仍能关闭；基线同主题（useOutsideClose Escape，基线 318）已判 WONTFIX，输入模态扩展属建议性 |
| 📋 high | 111 | `src/components/GraphPage/graph-build.ts:141` | 行为被测试钉住（graph-build.test.ts:128「工作流白名单只保留对应 hub 与成员」断言非白名单任务 w3 被剪）——过滤到指定工作流时隐藏其它工作流任务是意图行为，「只约束工作流卡」指 hub 侧不建白名单外 hub，属注释措辞非行为 bug |
| ❌ high | 112 | `src/components/WidgetApp/constants.ts:23` | 反证 tauri.conf.json:3 productName "wmessage"、package.json:2 name "wmessage"——`wmessage-widget-` 前缀就是产品名，非拼写错误 |
| ❌ high | 113 | `src/components/TodoCard/types.ts:24` | CardDrag 唯一注入方是 TodoCard.tsx:956 的 useDraggable（注释明示「无排序上下文」），useSortable 消费方（WorkspacePage/SortableWorkspaceCard）自建卡片不进 CardDrag；且 TS 结构类型对非字面量赋值允许多余字段，「必 fail 结构兼容」不成立 |
| ❌ high | 114 | `src/components/TaskTimeline/week.ts:126` | 主循环（week.ts:111-125）覆盖与显示周任一日窗口有交叠的跨周计划（Sun→Tue 在 Mon 周视图里 Mon 段照常产出）；起点在别周且无窗口交叠 = 整个计划不在本周，week.ts:87 注释明示「跨周部分不产出分段（翻周后可见）」 |
| 🔧 high | 115 | `src/components/useInlineEdit.ts:53` | 机制属实但前提不成立：4 个消费方 onCommit 均同步退编辑态（TodoCard.tsx:102、SubtaskRow.tsx:38、WidgetApp.tsx:629），「onCommit 后 editing 仍 true」现在不存在；且建议修法（先清旗再 onCommit）会破坏「同步卸载 blur 防双提交」保护——onCommit 内 setEditing(false) 触发的 blur 在返回前到达，必须读到 true |
| 🛡 high | 116 | `src/components/WorkflowCanvas/graph.ts:115` | 基线同点位（基线 323 判 ALREADY_HANDLED）：Rust 侧 workflow_decompose.rs:292-313 校验 dependsOn 越界直接报错并拓扑重排，前端拿到的下标恒有效 |
| ❌ high | 117 | `src/lib/errorHandler.ts:168` | 仓库无 == 禁令（AGENTS.md/SPEC/.oxlintrc.json 均无 eqeqeq），`== null` nullish 判定是本仓惯用法（archiveRule.ts:29、MarkdownText.tsx:36、EvolutionPanel.tsx:463 多处在用） |
| ❌ high | 118 | `src/lib/errorHandler.ts:109` | 反证 errorHandler.ts:85-92 注释：code 刻意保持 string 以容忍 Rust 先行新增、default 分支即文档化的未知 code 兜底，改型 CommandErrorCode 反而破坏前向兼容；漂移已有三道防线（Rust 测试 + 前端 ALL_COMMAND_ERROR_CODES 测试 + audit_error_codes.py 提交门禁，26-29 行注释列明） |
| 🛡 high | 119 | `src/lib/archiveRule.ts:14` | 基线同点位（基线 324 判 ALREADY_HANDLED）：App.tsx:403-404 启动 effect 先 await loadArchiveDaysFromConfig() 再加载数据/首套规则（注释明示防竞态），重复加载是幂等赋值无半程态 |
| 🔧 high | 120 | `src/lib/archiveRule.ts:51` | 基线同点位（基线 325 判 PARTIAL）：缺 completedAt 老卡前后端两侧口径一致（都不会按真实完成时间归档），回填 now 只在调用方持久化返回克隆时才落盘；对象 churn 半边仅波及 done 未归档老卡、量级小——去留需人工定夺 |
| 📋 high | 121 | `src/lib/useTauriListen.ts:42` | 基线同点位（基线 331 判 WONTFIX）：handler 单次捕获是文档化契约（useTauriListen.ts:23「调用方只应传 setState 类稳定语义回调」），加 ref 转发属建议性加固 |
| 🛡 high | 122 | `src/lib/workflowAudit.ts:55` | 反证 src-tauri/src/db/workflow_settings.rs:143-167：settings_set 写入前 retention 钳 5..=100、ask_budget 钳 1..=5（u8 类型使 NaN/负数/小数在 serde 反序列化即报错），读侧 ask_budget_per_exec 再钳一次，前端预校验是防御性重复 |
| 🛡 high | 123 | `src/lib/workflowPrompt.ts:7` | 基线同点位（基线 333 判 ALREADY_HANDLED）：GUIDANCE_KEY 唯一写入口是 SettingsPage textarea，已 maxLength={MAX_GUIDANCE_CHARS}（SettingsPage.tsx:4152），卸载兜底写入的 guidanceRef 亦源自同一受限输入框 |
| ✅ high | 124 | `src/lib/openTarget.ts:37` | 惰性匹配 + 扩展名锚定使 archive.tar.gz/.tar.bz2 等复合扩展被截成 archive.tar，点击/📄按钮拿错路径必 ENOENT 报错｜修法：FILE_EXTS 的 tar 分支改成可选后缀组（tar 后跟 .gz/.bz2/.xz/.zst）并置于 gz 之前 |
| 📋 high | 125 | `src/lib/openTarget.ts:83` | 三支已共用同一 handleCommandError 中心出口（openTarget.ts:84/88/93）无重复路径，.catch 即发即忘是 void 入口的刻意形态；改 async/await 无行为差异，「未来加 retry/telemetry」属 YAGNI |
| ❌ high | 126 | `src/storage.ts:103` | 反证 src-tauri/src/db/tasks.rs:309-340：upsert 的 ON CONFLICT DO UPDATE SET 子句不含 created_at 列（152-154 注释「UPDATE 永不覆盖该列」），undefined 序列化为字段缺失后老行的创建时间由 DB 层结构性地保住 |
| 📋 high | 127 | `src/storage.ts:0` | 整数重排分支仅在浮点间隙耗尽（hi-lo<=1e-9）触发且注释明示未变序行保留原引用；重排行的 order 变更本就需落盘并经 diffTaskRows 纯排序豁免携带 expectedUpdatedAt 基线，「并发写者失效」是单人本地应用下的假想场景 |
| 📋 high | 128 | `src/theme.ts:34` | localStorage 配额/禁用写失败在 Tauri 桌面 WebView 不可达（6 字节值），先套用后持久化保证点击即时生效；persist-first 反而让失败时点击毫无反馈，顺序是正确取舍 |
| 📋 high | 129 | `src/test/scrollNoop.ts:12` | 基线同点位（基线 340 判 WONTFIX）：测试专用 helper，JSDoc 明示仓规针对运行时代码、jsdom scoped polyfill 是此前 finding 认可的形态；grep 证实仅 test 文件引用、无运行时导入 |
| 🛡 high | 130 | `src/profile.ts:81` | 基线同点位（基线 334 判 ALREADY_HANDLED）：唯一调用方 ProfileRow.tsx 对三个写操作全部 try/catch 且 handleCommandError + setError 内联呈现（63-65、77-79、93-100） |
| 📋 high | 131 | `src/components/ChatPanel/Fold.tsx:20` | 基线同点位（基线 347 判 WONTFIX）：三处调用点（MessageList.tsx:94/113/304）children 恒非空，aria-expanded 与 aria-controls 分叉的前提不可达 |
| 📋 high | 132 | `src/components/ChatPanel/InputArea.tsx:304` | 嵌套三元属风格口味，AGENTS.md 与 oxlint 配置均无禁令；三档循环语义直白且按钮带全称 title 提示，映射表重构无行为收益 |
| ❌ high | 133 | `src/components/ActivityPage/index.tsx:182` | 反证 src/lib/trace.ts:20-21 promptTokens/completionTokens 为非可选 number + src-tauri/src/db/trace.rs:46 INTEGER NOT NULL DEFAULT 0（i64 非 Option 恒序列化），undefined 相加产生 NaN 的前提不可达 |
| ❌ high | 134 | `src/components/ActivityPage/index.tsx:52` | React 19（package.json ^19.1.0）卸载后 setState 是安全 no-op（无泄漏、无警告、不波及重挂实例），reqIdRef 已保证新旧响应次序；mountedRef 属过时模式 |
| ❌ high | 135 | `src/components/ChatPanel/SessionList.tsx:133` | 同 117：仓库无严格相等禁令，`s.updatedAt != null` 是 nullish 判定惯用法，展开为 !== null && !== undefined 纯属风格噪音 |
| 🛡 high | 136 | `src/components/ChatPanel/SessionList.tsx:139` | onDelete 即 ChatPanel.deleteSession（ChatPanel.tsx:1522），其中 991-996 已弹 confirm「删除对话…消息记录一并删除」、986-988 拦截在飞会话，单击不会无确认销毁 |
| 🛡 high | 137 | `src/components/ChatPanel/RichText.tsx:31` | 防线在 openTarget 内部：URL/路径两支各自 .catch(handleCommandError)（openTarget.ts:84/88）、未识别目标直调 handleCommandError（93-100），失败必弹窗不静默，调用方无 rejection 可泄漏 |
| 📋 high | 138 | `src/components/ChatPanel/RichText.tsx:35` | RichText.tsx:22-23 注释明示取舍：路径 token 不设 href 防中键/复制链接把文件路径当 URL，role+tabIndex+Enter 已保键盘可达；补 Space 键属 a11y 增强（基线同类 a11y 增强均判 WONTFIX） |
| 🔧 high | 139 | `src/components/ChatPanel/ChatPanel.tsx:1012` | 不清理属实，但后果不成立：sessionUsage 是纯内存 state 从不落盘（「bot_history_save 携带陈旧条目」反证——全仓无该 Record 的持久化路径），单条仅 3 个数字、窗口重载即清；补一行 delete 可选做 |
| ✅ high | 140 | `src/components/ChatPanel/ChatPanel.tsx:1081` | 属实：runChat 在 1067 行 set(sid,{})，delete 只在 try 成功分支（1082），catch/finally（1113-1124）都不删，失败会话的 meta（含流式累积的 thinking 文本）滞留到该会话下次 runChat 或重启；修法零风险｜修法：把 streamingMetaMapRef.current.delete(sid) 挪进 finally（meta 已在 1081 行先行捕获，挪动无行为变化） |
| 🔧 high | 141 | `src/components/ChatPanel/ChatPanel.tsx:280` | JSON.parse 无保护属实，但「逃逸所有外层 catch」前提不成立：rowsToMsgs 五个调用点（329/755/807/944/1035）全在 try/catch 或 .catch 内，最坏后果仅是该会话历史静默不显示；且数据源是本机 bot_history_save 自写 JSON，畸形前提弱 |
| 🛡 high | 142 | `src/components/ChatPanel/MessageList.tsx:374` | 基线同根因（crit-high #377 MarkdownText.tsx:54 🛡）：openTarget 内部对 openUrl/open_file_path/未知目标三条路径全部 catch 并 handleCommandError 弹错（src/lib/openTarget.ts:84-100），调用处注释「失败弹错不静默」指的正是该内部防线，无需调用方再接 |
| 🔧 high | 143 | `src/App.tsx:554` | ruleChanged 写失败（582）时 tasksRef 确实不更新、下次合并从旧基线出发会复现 SYNC-1 写冲突弹窗，但「静默」前提不成立：storage 层 upsertTasks 失败已 alert（storage.ts:35-43）+ console 留痕，且注释明示「靠下次事件/db_load 自愈」的既有取舍；基线同族（#359/#360 🔧）加回查属加固非必修 |
| 📋 high | 144 | `src/App.tsx:475` | 删除失败时行本就仍在库中，「复活」即事实真相而非自愈假象：storage 层 deleteTaskRows 失败已 alert（storage.ts:46-54），调用点注释明示此为既定取舍；建议的失败回查不改变结局（删除失败的行必然回来），只是更早显示同一事实 |
| 🔧 high | 145 | `src/components/EvolutionPanel/EvolutionPanel.tsx:132` | refresh 无 in-flight 守卫属实，但后果弱：面板每次操作后都会重新 refresh 自愈，过期视图是瞬时的；最实际场景是换筛选时旧响应后到盖掉新筛选列表，加 seq 令牌属低风险加固，需人工复核是否值得 |
| ❌ high | 146 | `src/components/EvolutionPanel/EvolutionPanel.tsx:304` | 双击二发被 setDeleteTarget(null)（308）+ 开头 null 守卫（305）双保险阻断（弹窗随 state 卸载、二次调用走 early return），「busy 未及置位可滑过」不成立；失败场景 runCmd 已 setError 弹横幅，重开弹窗即可重试，属常规 UX |
| 📋 high | 147 | `src/components/EvolutionPanel/EvolutionPanel.tsx:133` | 三个 invoke 合用一个 try/catch 属实，但失败已有 error 横幅 + silent toast 提示，且下次任意操作即整板刷新；拆 per-invoke catch 属可读性打磨，无功能缺陷 |
| 📋 high | 148 | `src/components/EvolutionPanel/DeleteConfirmDialog.tsx:91` | 级联删除已有层层防线：默认不勾选 + 警示文案（勾选后永久删除/不勾选可重生对比说明）+ 焦点默认落取消 + busy 禁用，与文件头「设计权衡」注释一致；再加输入确认步骤属建议性加固（单用户本机自删自己的记忆库） |
| ✅ high | 149 | `src/components/GraphPage/GraphCanvas.tsx:494` | sigma v3 Camera 无公有 animated 属性（isAnimated()=!!私有 nextFrame，dist/sigma.esm.js:140-142；.d.ts 无该字段），495 行赋值只是挂了个无人读取的自有属性，纯死代码 + 「阻止画布平移」误导注释；基线 #366 同点位判 FP 恰因该赋值不生效，本次按死代码成立｜修法：删除 GraphCanvas.tsx:494-495 两行（行为不变） |
| ✅ high | 150 | `src/components/HomePage/GraphMini.tsx:65` | 时区混比属实：dayKeys 是本地口径日键（homeData.ts:9 dayKey 本地分量拼接），而 new Date(dayKey) 按 UTC 午夜解析再 toDateString 本地化，西半球时区整体偏一天；同文件 weeklyBuckets（homeData.ts:202）已确立 dayKey(new Date(ms)) 本地口径｜修法：nodeMatchesDay 第二分支改用 dayKey(new Date(t.completedAt)) === dayKey（复用 homeData 的 dayKey）；勿采纳 scanner 的 toISOString 方案——那是 UTC 口径会反伤 UTC+ 用户 |
| 📋 high | 151 | `src/components/HomePage/DayGridMini.tsx:42` | 7×N 重扫属实但量级无关痛痒：单人本地任务库数百条、segsForWeek 纯内存计算，渲染耗时微秒级；预分桶重构属投机性能优化（YAGNI） |
| ✅ high | 152 | `src/components/HomePage/HomePanels.tsx:188` | 属实：notifications===null 且无错时 body 渲染「加载中…」（198-199），footer 却因 pending=null→0 落入「都处理完了」分支（188-196），加载期间两区文案自相矛盾｜修法：footer 增加 notifications === null && !notifError 时的加载分支（如「加载中…」或空串），与 body 同口径 |
| ❌ high | 153 | `src/components/HomePage/WeeklyChart.tsx:84` | 越界前提不成立：activeDay 全部产生于 0-6 索引（WeeklyChart:150 data.map、DayGridMini:90-92 col.dayIdx、GraphMini hoverTask findIndex 越界转 null），data 恒为 weeklyBuckets 固定 7 桶，仓库内无越界生产者，「父组件传陈旧/负/越界值」纯属假想 |
| 📋 high | 154 | `src/components/MarkdownText.tsx:44` | 两处 <a> 渲染重复属实且注释互相锚定（79 行「与行内 code 渲染同款」），但仅 ~25 行、两处静态共存、各自注释说明 href 策略差异，抽取 Linkish 组件属可读性打磨；e.repeat 防护是附带的建议性项 |
| 🛡 high | 155 | `src/components/MigrationPanel.tsx:90` | 双开后果不成立：后端 migration_run 有 RAII 单飞守卫（run.rs:48 MigrationGuard::acquire → ops.rs:26-37 RUNNING compare_exchange，重入直接 Err「迁移正在进行中」并经 handleCommandError 弹给用户），手动/自动并发与前端竞态都被后端兜住 |
| 📋 high | 156 | `src/components/HomePage/HomePage.tsx:157` | 函数每渲染重建属实但无性能后果：30s 一次的重渲染对象是几个迷你面板，无卡顿证据；useCallback/memo 化属投机优化，第二个真实性能问题出现前不动（YAGNI） |
| ✅ high | 157 | `src/components/HomePage/HomePage.tsx:131` | 属实：文件头注释（第 6 行）承诺「执行痕迹/通知本页拉取（30s 轻轮询 + 变更广播）」，traces 有 setInterval（120-124）而 notifications 只有挂载拉取 + 广播（131-142），挂载拉取与监听器附着的窗口期内丢事件后无自愈｜修法：notifications 补 setInterval(loadNotifications, REFRESH_MS) + clearInterval 清理，对齐 traces 同款模式与文件头注释 |
| 📋 high | 158 | `src/components/NotificationsPage/NotificationsPage.tsx:110` | busy 态已挡连点（disabled={busy}），后端每次落库成功都广播 notifications-changed（notifications.rs:280-281、342、363 及各调用方），同进程事件投递可靠，重复操作后端幂等；七条动作各加 finally reload 属防御性冗余（YAGNI） |
| ❌ high | 159 | `src/components/SettingsPage/ApiProviderSelect.tsx:45` | 空数组前提不成立：options 只有两个静态非空来源——默认 API_PROVIDER_OPTIONS 常量与 SettingsPage:189 的 API_FORMAT_OPTIONS（2 项），无任何动态/空数组生产者，current.label 无可达崩溃路径 |
| ❌ high | 160 | `src/components/SettingsPage/ModelRow.tsx:85` | 后果不成立：React 19 卸载后 setState 是无害 no-op（18 起已移除警告，本仓 react ^19.1.0），无「泄漏状态」；onTested 迟到回调是设计内行为（110-112 注释明示 r.ok=false 也要通知父级刷新 verified 绿点），mounted-ref 守卫无收益 |
| 🔧 high | 161 | `src/components/SettingsPage/ModelRow.tsx:244` | 非受控+onBlur 是 241-242 注释明示的设计（防 "0." 中途态被受控回写打断），「父级替换 model 后 blur 写回旧值」窗口存在但极窄（需编辑态中恰逢外部 reload 且对未触碰字段失焦）；scanner 附带的 re-key 修法会强制重挂载丢焦点，副作用明显，需人工复核 |
| 📋 high | 162 | `src/components/SchedulePage/SchedulePage.tsx:251` | 「看起来已取消」前提不成立：job 分支 handleCommandError 非静默弹错（256），workflow 分支 setWorkflowSchedule 内部 catch 弹错（178-180），失败用户必见；失败后是否自动恢复 armed 两步态属交互口味 |
| 📋 high | 163 | `src/components/HomePage/PoolPager.tsx:42` | 「点击时 activeDay 非空」场景实际不可达：所有联动源在指针离开时都清 activeDay（WeeklyChart:152/DayGridMini:59-63 onMouseLeave、GraphMini leaveNode、行 hover 上抛后移开即清），指针到翻页钮时 activeDay 已回 null；「联动结束指针未离开保留手动页」是 39-41 行注释明示的设计 |
| 🛡 high | 164 | `src/components/SettingsPage/McpPanel.tsx:331` | 基线同点位（crit-high #382 McpPanel.tsx:308 🛡 critical）：前端校验缺失不影响安全——后端保存口强制 http_url_is_public（bot/mcp/config.rs:182-183，scheme 白名单+host 安全闸）且 manager 发请求前再验一次，localhost/私网/link-local 在 Rust 侧直接拒 |
| 📋 high | 165 | `src/components/SettingsPage/McpPanel.tsx:170` | env 值是本机用户自己在同一屏幕刚输入的机密，确认页明文回显有防呆价值（与「请确认是你预期的」文案配套）；header 打码 env 不打码是「header 常为长随机机密、env 常为用户自配项」的既有差异，按 key 名启发式打码属建议性加固 |
| 📋 high | 166 | `src/components/SettingsPage/MemoryPanel.tsx:714` | 「全空=回落默认」「留空=跟随默认」是 709 行注释明示的既有语义（与 ModelRow 推理参数同约定），loaded 门（726-727）已防未加载误存；非法值由后端 clamped 统一钳制（bot/config/io.rs:163、188 + memory/tests.rs:739 全字段钳制测试），前端加校验是重复防线 |
| 📋 high | 167 | `src/components/SettingsPage/UsageStatsCard.tsx:416` | 共享 error 行的显示语义瑕疵属实但属打磨：loadOverview 仅挂载/手动刷新触发且两次并发拉的是同参等价数据（days:365 无变量），「旧盖新」无实际差异；拆 overviewError/modelDaysError 属 UI 状态细分建议 |
| 📋 high | 168 | `src/components/TaskTimeline/HoverTitle.tsx:22` | 350ms 窗口内滚动/位移致 chip 短暂错位属实但瞬时：滚动即触发边界事件走 hide() 清除，hover 提示属原生 title 心智的轻量件，重构取值时机属打磨 |
| 📋 high | 169 | `src/components/TaskTimeline/HoverTitle.tsx:36` | 顶端无钳制属实但后果仅是贴近视口顶时 chip 轻微出界，且池/网格井元素少有贴视口顶场景；与 168 同属 hover 提示打磨，非缺陷 |
| ❌ high | 170 | `src/components/SettingsPage/WordTemplatePanel.tsx:161` | 竞态入口不存在：startEdit（265-266）、remove（283-284）、reextract（193-194）、取消/保存（202/209）全部 disabled={busy}，startEdit 进场即 setBusy 且 React 离散事件间刷新，「params_get 在途时点删除」点不到按钮 |
| ❌ high | 171 | `src/components/SettingsPage/WordTemplatePanel.tsx:126` | editing 中途变 null 的路径全被 disabled={busy} 封死：remove/saveEdit/取消在 reextract 在途时均禁用（202/209/284），confirm 模态也阻塞面板交互，{...null, params} 前提不可达 |
| 📋 high | 172 | `src/components/TaskTimeline/TaskDetailPanel.tsx:77` | Escape 关闭丢弃未 blur 提交的草稿属实，但与标题输入级「Esc 还原」机制（60-63、289-293 skipTitleCommit）共同构成「Esc=取消编辑」的连贯语义，备注/标签 blur 提交是防每键 task_patch 的既有设计；改为输入框内 Esc 不关闭属交互拍板非缺陷 |
| 📋 high | 173 | `src/components/TaskTimeline/TaskDetailPanel.tsx:64` | 「外部改标题恰逢用户正在编辑同一字段」在单用户本地应用是罕见窗口，任务切换/外部改名回填是标准草稿模式；加 activeElement 守卫会让外部改名在编辑期间不可见，两边都是取舍，无证据前不动 |
| ❌ high | 174 | `src/components/TaskTimeline/TaskDetailPanel.tsx:37` | 「Rust 按 UTC 解析致跨日漂移」不成立：后端对 planStart 只做 NaiveDateTime 定长格式校验（db/tasks.rs:893-910，无时区语义、无换算），scheduler 不消费 plan_start，字符串前后端均按本地挂钟口径原样存取（前端 parsePlanDT 逐字段读本地分量），全链路无 UTC 转换点 |
| 🔧 high | 175 | `src/components/TaskTimeline/TaskTimelinePage.tsx:315` | 前提可达（拖拽按住期间按 ⌘N 切视图可中途卸载），但泄漏的监听在下一次 pointerup（手势自然结束）即被 onUp 自移除，期间 setState 对已卸载组件是 React 18 no-op，无累积泄漏无崩溃 |
| 🔧 high | 176 | `src/components/TodoCard/TodoCard.tsx:139` | cur 确先于 await 捕获，但原生文件对话框是模态、挡住本端全部交互，「用户快速重点/其他卡操作」前提不可达，唯一并发写源是机器人对同卡的绑定，单用户场景概率极低且只丢一次增量 |
| 🔧 high | 177 | `src/components/TodoCard/TodoCard.tsx:166` | 与 pickFile 同族同判：模态对话框期间并发改 files 在本应用交互模型下几乎不可达，后果限于偶发双文件夹绑定一次 |
| 📋 high | 178 | `src/components/SettingsPage/WordTemplateStyleForm.tsx:144` | 切单位不换算属实，但输入框即时按新单位回显（auto 1.5=360 → 固定值显示 18 磅），所见即所存、UI 与后端无错配，换算策略属产品取舍 |
| ❌ high | 179 | `src/components/SettingsPage/WordTemplateStyleForm.tsx:253` | React 对未变化的 open prop 不重写 DOM（details 的 open 不在受控恢复机制管辖内），用户原生折叠后重渲染不会弹回展开，「只能常开/all-closed 不可达」的前提不成立 |
| 🔧 high | 180 | `src/components/TracePanel/TracePanel.tsx:321` | 基线同点位（TracePanel.tsx:279 判 🔧，清 rows 半边已修）：残留的 seq 守卫属同一竞态类，但面板是全屏模态、打开期间无法换 workflowId，同面板重复点击的乱序响应同源同数据，无可见后果 |
| 📋 high | 181 | `src/components/WidgetApp/TaskCard.tsx:50` | 基线同族：仓库对键盘可达性缺口一贯不修（如基线 SplitBar.tsx:52 分隔柄无键盘等价判 📋、SplitBar.tsx:58 aria-label 判 📋），本条按既有取舍沿用 |
| ✅ high | 182 | `src/components/WorkflowCanvas/GoalNode.tsx:93` | 拆解假设是 LLM 自由输出、可含重复文本，raw 字符串作 key 触发重复 key 告警，且重拆解整表替换时按 key 配对会错误复用/丢弃重复项，面板恰以「完整展示模型假设」为目的｜修法：key 改用索引：`data.assumptions.map((a, i) => <li key={i}>`（只读列表无重排，索引 key 安全） |
| 🔧 high | 183 | `src/components/WorkflowCanvas/ClarifyCard.tsx:59` | 基线同点位（ClarifyCard.tsx:58 判 🔧）：onToggleEdit 单向置 true 属实，但回到摘要行仍有「清除回答」路径，交互取舍类，攒批人工复核 |
| ❌ high | 184 | `src/components/WorkflowCanvas/TaskNode.tsx:139` | 受控输入（value={draftTitle}）必须 onChange 回写否则无法输入；父级 onDraftTitleCommit 仅 setNodes 本地草稿态，无 IPC/历史写副作用，「commit 命名却逐键触发」不构成缺陷 |
| 📋 high | 185 | `src/components/WorkflowCanvas/TaskNode.tsx:63` | 无位移的按压 click 照常触发删除（React Flow 拖拽需位移启动）；轻微手抖仅挪动草稿节点位置且保存才落库，补 nodrag 属建议性打磨 |
| 🔧 high | 186 | `src/components/WidgetApp/WidgetApp.tsx:360` | settle 确不钳 y，但扫描声称的两个后果均不成立：挂载恢复处已双向 clamp（WidgetApp.tsx:341-342，基线 storage.ts ✅ 修复面），持久化出屏不会发生，仅剩会话内用户主动把条拖出屏底的边缘态 |
| ❌ high | 187 | `src/components/WidgetApp/WidgetApp.tsx:483` | 反证 ops 式代数：474/476 行尺寸钳制（w≤startPos.x+startPos.w，h≤startPos.y+startPos.h）保证 nx≥0 且 nx+w=startPos.x+startPos.w≤sw（北向同理 ny+h≤sh），西/北向越界代数上不可达 |
| ❌ high | 188 | `src/ui/ErrorBoundary.tsx:19` | React 渲染期抛错即卸载边界下整棵子树（fiber 删除、state 丢失、cleanup 跑净），重试时 children 本就全新挂载，「children 保留内部 state 会在下一帧再抛」的前提不成立，key 重挂载与现状等效 |
| ❌ critical | 189 | `src/components/WorkspacePage.tsx:125` | setItems(next) 与 beginWrite() 是相邻同步语句，JS 运行完整个同步块才让出事件循环，workspace-changed 回调不可能插队执行，所述「writeInFlight 仍为 0 的窗口」长度为零 |
| ❌ critical | 190 | `src/components/WorkspacePage.tsx:167` | 同 189：setItems(next) 到 beginWrite() 之间全为同步代码（filter/map 无 await），外部事件无法在两行之间投递；基线 WorkspacePage.tsx:168 同族快照竞态判 ❌ 误报 |
| ❌ high | 191 | `src/components/WorkflowCanvas/HoldToConfirmDelete.tsx:120` | 防线在调用方：onConfirm→deleteWorkflow（WorkflowPage.tsx:886-900）自带 try/catch，失败经 handleCommandError 弹错并附重试，扫描声称「零反馈无重试路径」与事实相反 |
| ❌ high | 192 | `src/components/WorkflowCanvas/HoldToConfirmDelete.tsx:256` | 同 191：键盘路径的 onConfirm 同样落在 deleteWorkflow 的内部 catch + handleCommandError，错误有弹窗有重试，两条入口行为本就一致 |
| 🔧 critical | 193 | `src/components/WorkflowCanvas/HoldToConfirmDelete.tsx:234` | 1.2s 墙钟复位确实与 onConfirm 完成无关，但重触发仅对同一 activeId 重复 workflow_delete（done 窗口内 pointer/key 全被挡，成功后 createBlank 置 activeId=null 且按钮整个卸载），重复删除幂等、无破坏放大，critical 高估 |
| ❌ high | 194 | `src/components/WorkflowCanvas/HoldToConfirmDelete.tsx:214` | 唯一调用点 WorkflowPage.tsx:1119 未传 disabled（默认恒 false），「parent 中途翻转 disabled 吞掉确认」前提在本仓库不可达；删除成功后是整卡卸载而非禁用 |
| ❌ high | 195 | `src/components/WorkflowCanvas/SaveButton.tsx:153` | onPointerDown 已 setPointerCapture（SaveButton.tsx:141），指针捕获期间 pointerleave 被规范抑制、pointerup 一律派发回按钮，拖出按钮外松手仍会保存；仅旧 webview capture 抛错的降级路径才有此行为，而那正是 abort 的语义 |
| ✅ high | 196 | `src/components/WorkflowCanvas/WorkflowPage.tsx:195` | ❓提问开关（WorkflowPage.tsx:1041 仅 setAsksEnabled）与澄清回答随 save 的 clarifyMeta 落库但不进 dirty 快照，SaveButton 被 !dirty 禁用（SaveButton.tsx:83），单独切开关后无法保存、导航即静默丢失｜修法：把 asksEnabled 连同 lastAnswers 同时纳入 snapshot()（195 行）与 openWorkflow（220 行）、save（859 行）两处 savedSnapshot 的 JSON，三处同步保持 dirty 判定一致 |
| 🔧 high | 197 | `src/components/WorkflowCanvas/WorkflowPage.tsx:690` | catch 缺 seq 守卫属实（晚到失败会 setRunning(false) 压新画布、错误弹窗误归属），但 running 态有 5s 轮询与 openWorkflow 复位兜底、短暂自愈；finally 不复位 startBusyRef 的指控反向——切换后新画布理应可立刻开跑，不复位反而卡死 |
| ❌ high | 198 | `src/components/WorkflowCanvas/WorkflowPage.tsx:789` | 反证 workflow_runner.rs:1532-1534：workflow-report 全仓唯一发射点恒带 workflowId，`payload.workflowId &&` 短路的 undefined 分支实际不可达，「错落报告」无触发源 |
| 🔧 high | 199 | `src-tauri/src/migration/ops.rs:281` | 吞 rename 错误属实但非静默：非 EXDEV 失败仍经 copy/remove 以原始错误上抛、journal 正确清理；且按建议收紧到仅 EXDEV 会回归「dst 父目录缺失靠 copy 回退补建」的现有行为（mod.rs:762 测试锚定），修法有副作用 |
| 📋 high | 200 | `src-tauri/src/migration/ops.rs:282` | 基线同族（py/harvest.rs:24 TOCTOU 判 📋）：is_dir 快照到 copy 之间微秒窗内被并发改型的前提只有本机用户亲手同路径操作，失败路径全部上抛无静默损坏 |
| ✅ critical | 201 | `src-tauri/src/migration/ops.rs:246` | staging `.copying` 建前不查残留、步骤 5 rename 失败不清理：进程崩溃残留后下轮 claim_dst_name 仍选同名 dst，create_dir_all 幂等 + fs::copy 覆盖会把陈旧文件混进原子提交的归档副本，违背本函数自述的 APW-02a 原子性契约｜修法：步骤 3 建 staging 前查 `staging.symlink_metadata().is_ok()` 即返 DomainRule（或先 remove_dir_all），步骤 5 fs::rename 失败时补 `let _ = fs::remove_dir_all(&staging)` 再上抛 |
| 📋 high | 202 | `src-tauri/src/migration/ops.rs:303` | 计数键 src_str 来自 DB 绑定的 file_path、每轮轮询字符串恒定，门控对真实输入源生效；「大小写漂移重置计数」需用户中途以异写法重绑同一路径，属推测性加固（单人本地应用口径，Windows-only 假想） |
| 📋 high | 203 | `src/test/node-shims.d.ts:3` | 现有测试调用点（main.css.test.ts:11、taskFiles.test.ts:89）全部用 "utf-8"，shim 头注释明示最小化设计（项目不装 @types/node）；扩全量编码枚举属第二个用例出现前不做的 YAGNI |
| ❌ critical | 204 | `src/test/node-shims.d.ts:3` | 仓库不存在不带 encoding 的 readFileSync 调用（grep 全量仅两处且均带 "utf-8"），shim 注释明示用途就是「读源码文件为字符串」，「无 encoding 重载缺失使 shim 失去意义」的前提不成立，critical 高估 |
