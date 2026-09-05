# WMessage 开发日志

> 面向开发者的里程碑记录。产品规格见 `SPEC.md`，项目说明见 `README.md`。

## 2026-09-05（周五）链接打开彻底修复：聊天下方文档/网址链接「有时打不开、有时显示 Program」

老板报 bug：聊天后窗口下方的文档链接、网址链接，点击有时打不开，有时显示 program。

**根因（三个叠加）**：
①路径正则按空白截断——ChatPanel `RichText`/`extractFilePaths` 的路径字符集排除 `\s`，「C:\Program Files\...」「周报 修订版.docx」这类带空格路径被切成空格前一段：链接显示成「Program」（basename("C:\Program")）、点击打开一个不存在的路径。
②所有点击失败都是 silent catch——白名单拒、路径不存在、scope 拒全部静默，用户分不清没点上还是打不开。
③`looksLikeUrl` 把「C:」误判为 URL scheme（单字符即匹配）——工作区手动录入 Windows 路径被存成 url kind，点击走 openUrl 被 opener scope（仅 https?/mailto/tel）拒绝。另发现 URL 字符集把 ASCII `?` 当边界，带查询串的网址被截断打开错页面。

**修复**：
- 新增 `src/lib/openTarget.ts` 统一入口：`LINK_OR_PATH_RE` 路径分两支——带空格路径惰性锚定已知扩展名（docx/pdf/png 等），无空格路径保持旧行为；URL 分支放行 ASCII `?!`（查询串不截断），全角标点仍作边界；`openTarget` 按内容判定 URL/路径（不信任存储的 kind，兼容历史错配数据），裸域名自动补 https://；失败一律弹错可见（handleCommandError 非静默）
- ChatPanel（RichText + 📄 文件按钮 + extractFilePaths）、MarkdownText（a/code）、WidgetApp（工作区链接 + 绑定文件）、WorkspacePage（openLink）全部改走统一入口；`looksLikeUrl` scheme 要求至少两字符（排除「C:」）
- Rust `open_file_path` 加存在性前置检查：路径不存在回「路径不存在（可能已被移动或删除）：…」中文可读错误并记审计（原先透传 OS 英文报错且被前端静默吞掉）

**测试**：新增 `openTarget.test.ts` 9 例（Program Files 带空格完整匹配、文件名含空格、URL+文件夹旧行为不变、无扩展名不误判、extractFilePaths 去重/跳 URL/反引号穿透、normalizeUrl、openTarget 分发、失败弹 alert 回归锁）。vitest 21 文件 195 全过（186→195）、tsc 零错、cargo bot_skills 90 全绿。

## 2026-09-05（周五）修订模式修复：段落改几个字不再整段标删重写（.NET + Python 双引擎）

用户反馈：修订时段落里只有几个字要改，产物却把整段标删、整段重写，看不出究竟改了哪几个字。

**根因一（.NET 引擎主因）**：Program.cs 自实现 LCS 的 DiffList 合并缺陷——raw opcodes 先按同 tag 合并、del+ins 相邻并 replace 的逻辑只做一次，「del 块后跟多个 ins 块」（如连续两段都改写）时只有首个 ins 并入 replace，余下旧单元走整段标删、新文本整段插新段落。

**根因二（双引擎口径不一致）**：行内字符级 diff 前的自洽校验要求「run 文本拼接 == 段落文本」，两套口径却对不上——.NET 单元文本用 `para.InnerText`（含域代码 instrText/文本框等全部后代文本）而 run 映射只数直接子级 w:r 的 w:t；Python 单元文本用 python-docx `para.text`（含 \t/\n/超链接文本）而 `run_text()` 只读 w:t。含超链接/域/tab/手动换行的段落校验必败，落「整段删+整段增」保底。

**修复（两引擎同语义）**：①DiffList 合并改为「change 块（del/ins/replace）相邻即并入 replace 段」，对齐 difflib.get_opcodes 形态；②段落文本口径统一为 extract_document 同款（python-docx 1.2.0 para.text）：只数直接子级 w:r + w:hyperlink 内的 run，w:tab/w:ptab→\t、w:br/w:cr→\n、w:noBreakHyphen→'-'，域代码/已有修订不计——单元文本、run 映射、模型所见提取文本三方一致，正常段落恒走字符级 diff，保底只留域代码/内容控件等口径外结构；③文本入 run 与提取口径互逆（\t→w:tab、\n→w:br），equal 片段里的 tab/换行重建后不变形；④整段标删/行内重建前 hyperlink 解包（run 保留 rPr 外观），链接文本不再漏标删；⑤表格行/单元格文本、ReadDocxLines 同步换 ParaText 口径。

**测试**：`dotnet_revisions_in_place_preserves_formatting` 夹具加 tab+超链接混合段回归锁（断言只删「三」增「四」、tab 保留、链接文本作 equal 片段保留、不得整段标删）；Python 兜底脚本手工实测同夹具同断言通过。cargo test 全目标全绿。

## 2026-09-02（周三）修订模式改为就地修订：保留原文档格式/字体（.NET + Python 双引擎）

老板拍板：修订模式要保留原文的格式和字体进行修订。原先两引擎都是从零新建宋体 12pt 文档做纯文本 diff——标题样式、加粗、字体、表格结构全丢。

实现（两引擎同语义）：original_path 可读时复制原文档 → 段落级对齐（与 extract_document 同一口径：正文非空段落文档序在前、表格行在后）→ equal 段落原样不动（格式自然保留）；改动段落行内字符级 diff——equal 片段克隆原 run 的 rPr 拆段、del/ins 克隆锚点 run 的 rPr；整段删把含文本 run 转 w:del（w:t→w:delText）保 rPr；新增段落 pPr/rPr 克隆自锚点段落；表格行按 " | " 拆回单元格逐格 diff（格数对不上整行标删+表后插新段）。就地失败（文件损坏等）回退原新建模式保底有产物。

**踩坑**：重写 Program.cs 时把 DiffList 回溯 equal 分支的 `y++` 抄丢了——对齐整体错位（段落配错行），单测全绿没抓到（原 e2e 只断言 ins/del 存在），手工带格式实测才暴露。教训：diff 对齐类逻辑必须测「配对正确性」不能只测「标记存在」。

测试：bot_py.rs 新增 `dotnet_revisions_in_place_preserves_formatting`——最小 docx 夹具（zip+手写 document.xml：pStyle 标题段 + 加粗 run + 普通段），断言 equal 段落的 pStyle/`<w:b/>` 原样保留、改动段落行内 w:ins/w:del、无 w:date、stdout 走「保留原文格式」路径；另有手工实测（python-docx 造含标题/加粗/楷体/表格的原文，dotnet + Python 两条路径产物逐段核对一致）。TOOLS 描述 / SYSTEM_PROMPT 规则 10 / doc_make_word_revisions 注释同步「就地修订保留原文格式」。cargo test 全目标 479 → **480**（+20+8+9）全绿。

## 2026-09-02（周三）Word 修订模式去 Skill 化 + 去掉修订日期 + 系统提示词对齐工具

老板三条拍板：①修订日期不要了；②文档润色修订模式执行不对——不需要相关 Skill，要走 dotnet 修订模式；③系统提示词逐行对齐工具实际功能。

**修订日期下线**：.NET 工具（WmDocxRevisions/Program.cs）与 Python 兜底脚本（MAKE_DOCX_REVISIONS_SCRIPT）的 w:ins/w:del 不再写 w:date（保留递增 w:id + author=WMessage AI）；dotnet e2e 单测加 `!xml.contains("w:date=")` 回归锁，已重建 Release dll 并实测产物无 w:date。

**修订模式去 Skill 化**：create_word_revisions 移出 ATOMIC_TOOLS 原子黑名单（聊天直调放行，不再被「不允许裸调」拦回 create_word）；引擎本来就强制 .NET OpenXML 优先（run_doc_revisions：dotnet 试跑→失败回退 Python），去 Skill 后修订模式在聊天里开箱即用。link_file_to_task 仍是原子（任务卡执行流程经 StopGuard.allow_atomic 放行，不变）。波及面同步更新：tool_guard（黑名单 + 拦截消息 + 单测）、middleware 三个单测、audit.rs / bot_model_loop.rs 各一个用例、llm_integration.rs 三个用例（原「黑名单拦截 create_word_revisions」反转为「非 Skill 状态直调放行」回归锁）、skill_e2e.rs 三个用例、tests-audit 门禁断言改为「ATOMIC_TOOLS 数组内不得含 create_word_revisions」；TOOLS schema 的 create_word_revisions description 去掉「内部原子…裸调会被拦截」话术；bot.rs / bot_slash.rs 两处注释同步。

**系统提示词逐行核对**（SYSTEM_PROMPT 21 条 + 安全红线 + EXECUTE_SYSTEM_PROMPT 7 条，对照 TOOLS schema 与生成脚本实现）：规则 10 重写——删掉「内部原子工具/技能未加载改用 create_word」整段与「商务提案标题可加粗加大」（生成器无此参数化能力），保留 track changes / originalPath / 截断传 original / 不覆盖原文件；规则 11 删 PDF「文档类型决定风格」（MAKE_PDF_SCRIPT 是固定排版，无风格参数）；其余逐条核验一致（Word 排版黑体标题+宋体正文+首行缩进 ✓ MAKE_DOCX_SCRIPT；PDF STSong ✓；PPT 微软雅黑+10 主题键 ✓ MAKE_PPTX_SCRIPT；图片扩展名/附件路径直读 ✓ IMAGE_EXTS+extract_document；[文档路径] 返回格式 ✓ bot.rs:1948）。

测试：cargo test 全目标 479+20+8+9 全绿（含新建 dotnet 无日期断言、三处黑名单语义反转用例）；tests-audit pytest 23过/1FAIL/1skip——唯一 FAIL 是 test_bot_log_path_uses_portable_dir，环境问题（本机 target/debug/bot.log 只剩一条 app_exit_cleanup，与本次改动无关）。

## 2026-09-02（周三）挂件聊天区支持拖文件添加附件

老板需求：把文件直接拖到挂件聊天窗口 = 在聊天窗口添加附件（等同 ➕ 选文件），随消息一起发送。

实现（ChatPanel.tsx）：Tauri 窗口 `dragDropEnabled` 默认开启，OS 级拖放不触发 HTML5 drop，改走窗口级 `onDragDropEvent`（enter/over/leave/drop）。落点过滤：position 为物理像素，除 `scaleFactor` 转 CSS 像素后与聊天区根节点（rootRef）矩形比对——只有落在聊天区内的 drop 才加入附件，拖到挂件任务列表区的文件不归聊天管；enter/over 落在聊天区时显示「松开以添加文件」虚线提示层。去重逻辑抽成 `addFiles`（➕ 选文件 / 拖入共用）。

测试：ChatPanel.test.tsx 新增拖放用例（enter 提示层 / 区内 drop 加附件 / 同路径去重 / 区外 drop 忽略），`@tauri-apps/api/window` mock 捕获 onDragDropEvent 回调手动触发；WidgetApp.test.tsx 的 winMock 补 `onDragDropEvent`（ChatPanel 挂件内挂载所需）；ChatPanel 订阅加 try/catch 兜底非 Tauri 环境。vitest 134 → **135 全绿**；tsc 零错。

## 2026-09-02（周三）定时补跑 2h 时效窗口（批次5审计 F3 定版）

老板拍板：recurring 补跑时效窗口 2h，超窗跳过。原先无窗口——关机一周启动会补跑一周前的到点、机器人开关关闭期间的到点在重开瞬间全补跑。

实现（bot_scheduler.rs）：新增纯函数 `classify_due`（Run / Missed / NotDue 三态）——recurring（daily/weekly/monthly）的下一 occurrence 距现在超 `CATCHUP_WINDOW=2h` 判 Missed：不补跑，`sched_last` 记为现在把该 occurrence 消费掉（顺延到下一周期），记 `sched_missed` 审计 + broadcast 同步三端。两条边界语义明确保留：新任务（sched_last=None）「下一次触发立即生效」的首跑不变；at: 一次性任务仍由 at_expired 放弃逻辑处理、不走补跑窗口。

测试：`classify_due` 6 个单测（窗口内 Run / 三种周期超窗 Missed / 2h 整边界 Run、2h+1min Missed / 新任务首跑 / at: 不判 Missed / 未到点 NotDue）。cargo test 全目标 473 → **479 全绿**。

## 2026-09-02（周三）全面审计批次 8：测试体系与防回归（无 P0，3 个 P1 已修）

按 `docs/AUDIT-PLAN-BOT-2026-08-27.md` 收尾批，三个并行 explore 子代理 + 全部 P1 主代理逐条源码复核，报告落盘 `docs/AUDIT-TESTS-2026-08-28.md`（含盲区清单 + 补测试优先级建议）。

**P1 三个（均已修 + 验证）**：
- **tests-audit 门禁是红的**：`audit_pre_step_pre_execute.py:187` 写死旧签名 `run_model_loop(app, msgs, 8, &stop)`，函数加参后 pytest 1 条 FAIL——pre-push 钩子（test-all.sh, set -e）必被卡。改正则锁行为不变量；顺带 cargo 基线阈值 80（当时基线 83 的遗物）收紧到 400，bot.log 空转检查改显式 skip。pytest 22过/1FAIL → **23过/0FAIL/2skip**
- **SKILL_RUNS 并行测试实锤交错**：state.rs:264 的无差别 clear × state.rs:295 的 Failed 重开 × lib.rs:671 的 terminate_all(None) 三条路径互相删/改对方的 run，随机挂。新增 `SKILL_RUNS_TEST_LOCK`（不引 serial_test 依赖）三处全程持有
- **stop_all 全局广播打断并行测试**：lib.rs:671 / bot_slash stop_all 用例的 stop_all_executions 会提前置位 bot_py.rs:2153 的 guard，使其「未停时完整读取」断言随机挂。新增 `STOP_TEST_LOCK` 三处持有（lib.rs 双锁固定顺序 SKILL→STOP）。cargo test ×3 连跑验证稳定

**P2 三个（已修）**：EXITING 退出标志测试后永不复位（补 `reset_exiting_for_test`，防未来 run_python 集成测试误挂）；`task_out.rs` 补 flatten/camelCase/status==column 线缆契约锁（此前 serde 属性被破坏 472 个测试无一能抓到）；`format.ts` 补 16 个用例（scheduleToDatetime 四分支 + isValidDateTimeLocal 进位拒绝——注释里两次 NaN 历史事故的高发纯逻辑长期零覆盖）。

**补测试优先级建议（记录，已落报告）**：#1 生产 run_skill_scheduler 真 e2e（run_dsl_loop_sync 镜像照不到确认/审计/持久化/回滚窗口/Done 收尾，8-27/28 的 P0 修复密集区恰在镜像外；fixtures/minimax-ppt 现成素材）；#2 run_model_loop 本体集成覆盖（重试/stop/think 拆分只有纯函数碎片）；#3 KanbanBoard.spliceMove（需先导出）；#4 弱断言清理（6 处内联复刻/恒等断言）。

**测试**：cargo test 全目标 472 → **473**（+task_out 契约锁）+20+8+8 三轮连跑全绿；vitest 116 → **132 全绿**；tsc 零错；tests-audit pytest 门禁转绿。

## 2026-09-02（周三）全面审计批次 7：前后端契约（无 P0，1 个 P1 已修）

按 `docs/AUDIT-PLAN-BOT-2026-08-27.md` 推进，三个并行 explore 子代理（Rust 命令面 / Rust 事件与错误面 / 前端调用与监听面）+ 全部发现主代理逐条源码复核，报告落盘 `docs/AUDIT-CONTRACT-2026-08-28.md`（含命令/事件/错误码三张契约对照表）。

**结论**：主干契约健康——58 个命令定义=注册=前端调用三方一致（参数 camelCase 转换全部吻合，无孤儿/悬空）；Rust 12 个 emit 事件全闭环有监听、payload 字段逐一对上，前端 8 个互发事件走全局广播属设计；22 个 CommandError code 修复后 100% 有前端 hint。事件 payload 无 key/token 泄漏。

**P1 一个（已修 + 回归测试）**：**挂件模型切换静默抹掉授权模式**——ChatPanel `applyModelConfig` 整体回写 `bot_set_config` 时漏传 `permMode`，后端全量覆写 + BotConfig 容器级 `serde(default)` 把缺失字段填 None，用户在设置页配的 strict/yolo 被静默重置回 ask。修复：读入类型与回写对象补 permMode 透传。

**P2 两个（已修）**：`py_set_enabled` 错误载荷从裸 String 对齐 CommandError 四字段结构（58 个命令中唯一例外）；`TASK_INVALID_STATE` 补 hintForCode case，ChatPanel 防重入识别从 message 子串「正在执行中」改用结构化 code（文案漂移即静默退化的脆弱点消除，拒绝文案改为透传后端 message，已完成/已归档拒绝同路径受益）。

**记录不修（排期/备查）**：死命令 `bind_file`/`db_merge`（已注册零调用，排期删）；`bot_stop`/`bot_confirm_response` 返回 `()` 失败不可见（60s 超时兜底，无现实受害路径）；`bot-confirm` 无会话归属时白等 60s（仅 DSL 遗留无守卫路径的理论边界）；AppConsts/MigrationStatus snake_case 风格漂移（契约一致非 bug）。

**测试**：vitest 113 → **116 全绿**（permMode 透传回归 / TASK_INVALID_STATE code 识别 / 22 code hint 全覆盖 3 个新用例）；cargo test 全目标 **472+20+8+8 全绿**；tsc 零错。

## 2026-08-28（周五）滚动条深浅色适配（Windows 主窗口 + 挂件）

原先全局没有任何滚动条样式：深色模式下 WebView2 原生滚动条仍是浅色，突兀。修复（src/ui/main.css，主窗口/挂件共用）双机制：

- `color-scheme: light/dark` 随 `.dark` class 切换——原生滚动条与表单控件自动随主题
- 自定义 webkit 细滚动条（8px、透明轨道、拇指 `--t6`、hover `--t5`）——走主题变量，深浅两套自动生效，贴合新拟态低饱和风格

测试：新增 `src/ui/main-css.test.ts` 两个源码锁用例（vitest `css:false` 会吞掉 `.css`/`?raw` 导入，直接读文件断言）；vitest 111 → **113 全绿**；tsc 零错。
踩坑：项目未装 @types/node，测试读文件补了 `src/test/node-shims.d.ts` 最小声明。

## 2026-08-28（周五）全面审计批次 6：跨平台与资源（无 P0，4 个 P1 已修）

按 `docs/AUDIT-PLAN-BOT-2026-08-27.md` 推进，单代理探索（其 Read 工具故障只覆盖 bot_py.rs 前 1000 行，未覆盖区由主线程补读）+ P1/P2 逐条人工复核，报告落盘 `docs/AUDIT-PLATFORM-2026-08-28.md`。

**P1 四个（均已修 + 回归测试）**：
- **probe_dir 把 .app 包内目录当便携数据目录**：dmg 拖到 ~/Applications 后 `wmessage.app/Contents/MacOS` 可写，数据库/日志/AI_Gen_Files 会全写进 app 包内（破坏签名、删 app 即删全部数据）。修复：`is_macos_app_bundle_dir` 判定，该形态跳过便携分支直落 app_data
- **构建机绝对路径烧进发布二进制**：`dotnet_revisions_dll` 的 dev 候选用 `env!("CARGO_MANIFEST_DIR")`（信息泄露 + 发布版纯死路径）。修复：cfg(debug_assertions) 门控
- **绿色包「免装 .NET」未达成**：framework-dependent 构建不含运行时，且运行侧只认 PATH 里的 dotnet CLI。代码侧修复：`dotnet_revisions_entry()` 优先直跑随包 apphost exe（self-contained 即免装运行时，失败自动回退 Python——回退链复核完整）；**打包侧需老板改** `dotnet publish -r win-x64 --self-contained`
- **macOS 崩溃路径 Python 孤儿永久驻留**：mac 无 Job Object/KILL_ON_JOB_CLOSE 等价物，RLIMIT_CPU 限 CPU 时间管不住睡眠型失控脚本。修复：Unix 脚本注入父进程看门狗前导（ppid 变 1 = 父死即自退，2s 轮询）

**P2 两个（均已修）**：pid 复用误杀防护（`is_live_group_leader` 组首校验——只进退出清理路径；kill_tree 的 drain_timeout 路径必须保持无条件组杀，否则孙进程占管道 reader 永不 EOF，回归测试实锤）；macOS GUI 极简 PATH 补固定路径探测（/opt/homebrew/bin、/usr/local/share/dotnet 等）。

**踩坑**：源码锁测试字节下标切片遇中文注释会 panic（`&text[a..b]` 切断多字节字符）——批次5/6 共 5 处统一改字符安全截取。

**记录不修**：审计 append 失败只 eprintln（GUI 无人可见，需前端可见性改造）；安装包无 bundle.resources 不含 dotnet 工具（Python 兜底行为正确但静默，打包决策）；hardenedRuntime:false + ad-hoc 签名（发版决策）；孙进程 setsid 逃逸（非沙箱，已知 trade-off）；AI_Gen_Files 无上限（用户产物）。

**测试**：cargo test --lib 466 → **472 全绿**（.app 探测跳过 / 组首校验 / debug 门控 / exe 优先 / 看门狗注入等 6 个新用例），集成 20+8+8 全绿；vitest 111 全绿；tsc 零错。
**注意**：看门狗改动影响全部 Python 执行路径，建议手动冒烟一次 run_python 类工具（如让机器人跑一段打印 + 一个 create_word）确认正常。

## 2026-08-28（周五）全面审计批次 5：调度与后台任务（无 P0，3 个 P1 已修）

按 `docs/AUDIT-PLAN-BOT-2026-08-27.md` 推进，单代理探索 + P1/P2 逐条人工复核（复核纠出子代理 1 条误报 + 1 条漏报），报告落盘 `docs/AUDIT-SCHED-2026-08-28.md`。

**P1 三个（均已修 + 回归测试）**：
- **调度循环串行 await 堵死全线**：原先对每张到点卡 spawn 后立即 await，单卡最坏 50 轮 ×（LLM 300s + Python 300s）可跑数小时，期间全部后续定时任务排队。修复：spawn 不 await（同卡重入仍由 SchedGuard/ExecGuard 防）+ 单任务 30min 整体超时（超时记 sched_timeout 审计并兜底复位 bot_assigned）
- **后台定时任务仍弹原生文件框**：bind_file 分发处丢弃 interactive 无条件弹框；extract_document 无 path 时 doc_extract 无条件 blocking_pick_file——无人在场时模态框让 spawn_blocking 线程永久阻塞，任务卡死。修复：两处 interactive=false 直接返回引导文案（改用 link_file_to_task / 传 path），不弹框
- **退出对在途模型循环零取消**：cleanup_on_exit 原先只清 API/Skill/Python，在途 run_model_loop 收不到任何停止信号，后台任务事实上无任何手段可叫停。修复：`stop_all_executions()` 置位全部实例（含后台，与 /stop 只停本会话交互实例互补）+ ≤2s drain 宽限，审计带 exec_stopped/exec_drained

**P2 四个（均已修）**：DST 切换日本地时刻 `.single()=None` 导致 daily/weekly 任务永久静默失效（`resolve_local`：歧义取较早、不存在顺延 ≤3h，五处统一）；退出时 PY_RUN_GATE 排队者拿锁后仍 spawn 孤儿 Python（`mark_exiting` 闸门后复查）；逐步执行 ExecGuard 起步即释放、确认挂起期调度器可并发执行同一卡（守卫改随 PendingExec 存活到 clear/take）；逐步执行 start() 两条错误路径不复位 bot_assigned 卡片永顶头像（复核新发现，子代理漏报）。

**误报纠正**：子代理报「bot_assigned 崩溃残留无启动期清扫」——db.rs open_db 启动期已有 `UPDATE tasks SET bot_assigned = 0`（每进程一次，有测试锁定），无需修。

**待拍板**：recurring 补跑无时效窗口（关机一周启动会补跑一周前的到点；开关关闭期间的到点任务重开瞬间全补跑）——窗口长度是产品决策（建议 2h），未动。**记录项**：sched_last 跑前记导致执行中崩溃本次 occurrence 无声消失（取舍正确）；SchedGuard/ExecGuard 双套守卫语义分裂（合并属架构项）。

**测试**：cargo test --lib 459 → **466 全绿**（resolve_local 等价 / 调度循环不串行 await 源码锁 / 后台拒弹窗源码锁 / stop_all 覆盖两类实例 / 退出标志闸门后复查源码锁 / ExecGuard RAII + PendingExec 持守卫源码锁 7 个新用例），集成 20+8+8 全绿；vitest 111 全绿；tsc 零错。
**注意**：调度并发化与退出 drain 属运行行为改动，建议手动冒烟一次（挂一个 1 分钟后的 daily 任务观察到点执行 + 执行中 Cmd+Q 退出无残留进程）。

## 2026-08-28（周五）全面审计批次 4：本地 HTTP API Server（无 P0）

按 `docs/AUDIT-PLAN-BOT-2026-08-27.md` 推进，单代理探索 + P1/P2 人工复核，报告落盘 `docs/AUDIT-API-2026-08-28.md`（含端点×认证×校验矩阵）。

**结论**：主干安全设计到位——只绑 127.0.0.1、除 health 外全端点 Bearer+ct_eq、token 0600 不进日志、body 1MB 硬上限、worker 64+panic 拦截、无 CORS（DNS rebinding 无利用面）。**无 P0**。

**修复**：
- **P1-2 API 写路径 RMW 无锁**：update/delete/create 的 load→改→upsert 两段式原先在 64 个并发 worker 下互相用旧快照整行覆盖；加进程级 `API_RMW_LOCK` 全程持锁（P2-3 create 的 max_order 并发撞号一并消除）。跨路径（API vs UI/bot）lost-update 仍属已立项的字段级合并写入架构项
- **P1-3 死 SSE 连接占位**：clients 条目带 writer 存活令牌（Weak），注册前收割尸体——原先尸体只在下次广播失败时移除，安静期内占满 32 名额新连接全 503
- **P2-2**：api_start 显式映射 `HttpStartFailed{port,reason}`（原先落成无结构 Internal，前端 code 分支永远等不到）；P2-8 双发竞态：检查+写入收进同一把锁
- **P2-4**：create 的 note/filePath、update 的 filePath 统一 trim 后存储（原先存原文，与注释矛盾）
- **P2-5**：8 处 500 响应不再回吐 DB 错误原文（含 SQL 片段/路径），对外统一 "internal error"，原文转义后进 api.log
- **P2-6**：rotate_token 的 api_stop 失败时回滚旧 token 文件（消「文件新 token、服务认旧 token」三态窗口）
- **P2-9**：update 显式传空白 title 按 400 拒绝（与 create 对齐，原先静默忽略）

**记录残留（不修）**：P1-1 tiny_http accept 级 slowloris（header 滴注可饿死全部连接，彻底修需换 HTTP 栈，注释过度声称已修正）；P2-1 body 阶段慢读只占 worker 名额；P2-7 API 写操作只进 api.log 文本、不进结构化 bot.log（handler 无 AppHandle，留待穿层）。

**测试**：cargo test --lib 456 → **459 全绿**（新增空 title 400 / trim 存储 / SSE 尸体收割 3 用例），集成 36 全绿；vitest 111 全绿；tsc 零错。

## 2026-08-28（周五）全面审计批次 3：LLM 协议与流式（2 个 P0 已修）

按 `docs/AUDIT-PLAN-BOT-2026-08-27.md` 推进，3 路并行审计（SSE/tool_calls、think/上下文、mock 保真度）+ P0/P1 人工复核，报告落盘 `docs/AUDIT-LLM-2026-08-28.md`。

**P0 两个（均已修 + 回归测试）**：
- **同名 Skill 跨会话顶号**：SKILL_RUNS 以技能名为键，会话 B 启动同名技能会把会话 A 的 run 整个顶掉——A 的步数/超时熔断、回滚动作记录、收尾全部静默失效。修复：start_skill 插入前检查，别会话的 Running/Paused 同名技能拒绝启动
- **并行会话流式串台**：bot-chat-delta / bot-think-delta / bot-tool* / bot-skill-failed 六个流式事件 payload 原先不带 sessionId，emit_to("widget") 单窗广播 + 前端无过滤 = 两个会话并行跑时输出互相串进气泡。修复：后端 emit_stream 统一注入 sessionId，前端六监听器按 sessionIdRef 过滤（对齐 bot-confirm 既有模式）

**P1 六个（均已修）**：干净 EOF（无 [DONE]/finish_reason）时残缺 tool_calls 不再被当完整回复执行（流截断显式报错/追加提示）；UTF-8 多字节字符跨 TCP chunk 不再产生 U+FFFD（改字节缓冲按行切——arguments 里的 `` 是合法 JSON 会被真实执行，比正文乱码更危险）；200 流内 error 载荷（OneAPI 类网关）不再静默吞成空白回复；429/5xx/发送失败重试一次（1.5s 退避，仅流式产出前，无重放风险）；聊天主路径加 10 万字符历史预算（原先全量透传，长会话直接 400）；历史图片改「最后 3 条消息内」窗口（原先「最近两条 user」永不失效，一张图每轮对话重复 base64 重发，单请求最多 ~32MB）

**P2 八个（均已修）**：流尾残余行冲刷；tool_calls index 上限 64（恶意 index 撑内存）；空 tool_call id 合成占位（严格 API 400）；finish_reason=length/content_filter 用户可见提示；reasoning_content 字段解析（DeepSeek-reasoner 推理同走 bot-think-delta）；非流式路径（bot_compact/Planner）剥 `<think>` 段；审计转义漏网 6 处补 truncate_for_log（模型给的工具名、工具结果 preview、DSL tool_name、前端 session_id）；ChatMsg.role 白名单（非 assistant 一律按 user，防污染历史注入 system/tool 角色）

**测试体系**：生产 tool_calls 累积抽纯函数 `accumulate_tool_call_delta`，llm_integration 复用同一实现（消除测试自写平铺式累积的漂移面）；mock_llm 新增 StreamError / FragmentedTextReply（N 字节切片可在多字节字符中间切断）两个 behavior + 对应用例；mock 注释里过时的生产行号引用修正。**立项项**：run_model_loop 主体（~440 行 HTTP 错误包装/流中断/工具编排）因依赖 Tauri AppHandle 零测试触达，后续抽可注入 base_url/client 的纯 async 函数后补端到端。

**测试**：cargo test --lib 433 → **456 全绿**（流内 error/reasoning_content/分片无 U+FFFD/index 上限/重试白名单/历史预算/图片窗口/think 剥除/同名技能冲突等 23 个新用例），集成 29 → **36 全绿**；vitest 110 → **111 全绿**（新增会话过滤用例）；tsc 零错。

## 2026-08-28（周五）全面审计批次 2：数据层与一致性

按 `docs/AUDIT-PLAN-BOT-2026-08-27.md` 推进，3 路并行审计（db 并发事务 / 迁移可靠性 / 广播一致性）+ 人工复核，报告落盘 `docs/AUDIT-DATA-2026-08-28.md`。

**修复**：
- **B2-P0 data.json 迁移**：触发判定从计数比较改集合差（漏迁场景堵上）；评估成功后一律改名退役为 `data.json.migrated`——原先「不触发就保留」+ 硬删任务 = 陈年 json 复活已删任务（测试固化了旧语义，已改写为「退役后硬删不得复活」回归）
- **B2-P1 copy_legacy_db 半拷贝防护**：先拷 `*.db.copying` 再 rename——原先中断留半截文件且 `!db_path.exists()` 守卫让下轮永久跳过，打开截断库报 malformed
- **B2-P1 bot_scheduler 三处写库补广播**：清理过期 schedule / 记 sched_last / 执行结果写备注原先零广播，主窗口（无轮询）长期显示旧 ⏰ 徽标/旧备注
- **B2-P1 锁外写者纳入 DB_WRITE_LOCK**：remember_fact / persist_outcome_quiet 原先锁外直写，长事务期间 SQLITE_BUSY 静默丢失
- **前端三处**：WidgetApp 空列表守卫改「当前本就为空才跳过」（原先删光任务后挂件永久显示旧数据、还能复活已删任务）；挂件 emit 失败不再静默吞（上报失败 = 改动永不落盘，改弹错）；App.tsx tasks-updated 监听 Promise 链串行化（连续事件从同一旧 tasksRef 出发互相覆盖）
- **B2-P2**：bot_history 单会话上限 2000 条（写放大有界化）；行内 JSON 字段损坏 eprintln 留痕（彻底防护需字段级合并写入，列入排期）

**排期项**：整行覆盖 lost-update（upsert 19 列全量替换 + 时间戳守卫被刷新绕过，修复需字段级合并，架构改造单独立项）；open_db 补丁序列按路径 Once 缓存（性能）；migration replay 60s 延迟；回退 JSON 时代版本 = 空数据需发版说明。

**测试**：cargo test --lib 433 全绿（语义回归用例改写），集成 29 全绿；vitest 110 全绿；tsc 零错。

## 2026-08-27（周四·午 5）全面审计批次 0+1：基线 + 安全纵深（2 个 P0 实锤已修）

按 `docs/AUDIT-PLAN-BOT-2026-08-27.md` 开跑。批次 0 基线：cargo 428+29 / vitest 110 / tsc 全绿。
批次 1 安全纵深（4 路并行审计 + P0/P1 人工复核），报告落盘 `docs/AUDIT-SECURITY-2026-08-27.md`。

**P0 两个（均已修 + 回归测试）**：
- **SEC-P0-1 SSRF**：`http_client()` 未禁 reqwest 自动重定向，`fetch_text` 的「3xx 逐跳校验」是死代码——公网 URL 302 到 `127.0.0.1`/云 metadata 完整可打。修复一行 `.redirect(Policy::none())` 激活手工逐跳校验；回归测试用本地 302 端点钉死「不得自动跟随」
- **SEC-P0-2 白名单单点破口**：`create_task/edit_task` 的 `files` 参数零校验——模型把任意目录标 `isDir:true` 绑进任务卡，`allowed_dirs` 并入白名单且先于 permMode 分流，strict 模式也被架空。修复：模型来源 files 仅放行 AI_Gen_Files 内已存在文件（强制 isDir=false，对齐 link_file_to_task），被拒记审计；`allowed_dirs` 过滤回收站任务（删卡即解权）

**P1 七个（均已修）**：grep_files walk 内软链跟随（跳过符号链接，fail-closed）；IPv4-mapped IPv6（`::ffff:127.0.0.1`）与 CGNAT 100.64/10 补判；`open_file_path`/`delete_bound_file` 限定任务卡绑定集合 + 工作区链接 + AI_Gen_Files（堵前端 XSS→RCE 一跳），导出命令限 .json；Linux 降级明文 key 在 keychain 恢复时自动迁回并删除；fetch/Jina 响应体改流式有界读取（Content-Length 撒谎不再吃内存）；日志注入漏网点全部补 escape（session_id/任务标题/确认详情/路径/技能名）；图片附件过白名单（桌面/下载/文档/图片 + AI_Gen_Files，[附件文件] 块污染不再外发任意图片）

**P2 小项**：key 文件 OpenOptions mode(0o600) 原子创建（消「先 0644 后 chmod」窗口 + chmod 失败告警）；bot.log 三处写入点统一 `open_log_append`（创建即 0600 + 已有文件补 chmod）；dotnet 修订工具纳入并发闸门（run_python 拆 gate/ungated，run_doc_revisions 入口统一持锁——std Mutex 不可重入，闸门只能提在入口层）；api_rotate_token 复用 write_token_file 保 0600；create_task/edit_task 的 files schema 描述同步安全约束

**记录项（不修，已知残留）**：DNS TOCTOU（校验与连接两次解析，缓解需 resolve 钉 IP）；LLM base_url 用户自配零校验（key 随配置外发，建议后续加 https 警告）；.NET dll 无哈希校验（威胁不高于 exe 替换，发版流水线补）。

**测试**：cargo test --lib 428 → **433 全绿**（重定向策略/mapped-v6+CGNAT/软链跳过/files 校验/附件白名单 5 个新用例），集成 29 全绿；vitest 110 全绿；tsc 零错。
**踩坑**：本地 302 测试端点必须先读请求再回响应（hyper 对未消费请求即收响应报 UnexpectedMessage）。

## 2026-08-27（周四·午 4）审计 P2 全修：解析加固 + 变量转义 + 防重入 + 审计补洞

承接「午 3」P1 修复，把审计报告剩余 P2 全部清掉（至此 P0/P1/P2 三轮清零）：

- **parse.rs**：BOM 剥离（原先带 BOM 的 SKILL.md frontmatter 整体静默丢失）；`risk_level: low` 未显式声明 mode 时按文档约定推导 auto（补 mode_explicit 标记区分「没写」与「显式默认值」）；回滚段标题统一识别 `## Rollback` / `## 回滚` / `## 回滚（Rollback）`（`is_rollback_heading` 共享给 runtime::rollback_section，原先两个解析器各认一半）；回滚段每行工具调用是一个独立回滚步骤（原先后续行静默覆盖只留最后一行）；Step 内多行工具调用 / 步骤序号重复 → 解析期 fail-fast（原先静默覆盖/错位）
- **vars.rs**：`${...}` 替换值落在 JSON 字符串内时自动转义（`replace_ctx` 上下文感知）——原先裸插含换行/引号的结果产出非法 JSON，被 parse_args 静默降级成 Null 参数，SKILL_DSL.md §8.3 示例不可能工作；`${stepN.id}` 无 UUID 时保留占位符（原先替换为空串无法诊断）
- **manage.rs**：`skills_import` 校验最终技能名合法性（原先非法名「装得上、用不了、删不掉」）；SkillInfo 补 enabled 字段，`build_skill_block` 过滤禁用技能（原先禁用技能仍被广告给 LLM，与路由表口径不一致）
- **middleware.rs**：registry 存在但 pre_execute 为空时，原子工具从「有声放行」改 fail-closed（与 registry 缺失口径一致）
- **审计补洞**：LLM 网络失败（llm.request_failed）、流式中断（llm.stream_failed）、轮数熔断（fuse_rounds）、Replan 预算耗尽（plan.replan_budget_exhausted，只记一次）、确认弹窗超时（confirm_timeout）、用户点拒绝（confirm_denied）全部留痕
- **bot_slash.rs**：/stop 时本会话在途确认弹窗立即按拒绝收尾（sender drop → 等待侧走超时拒绝分支）——原先 /stop 后迟到的确认点击仍会放行危险动作
- **bot_chat.rs**：会话级防重入 ChatGuard（原先聊天路径无锁，两条并发消息命中同一技能路由会 start_skill 互踩 + 副作用工具重复执行）；主循环 AwaitConfirm 跳出时 last_streamed 为空给可读提示（原先空白回复）
- **schema 文案漂移**：create_ppt「三套主题」→「10 套 + customColors」；web_search 补 Tavily 路由；run_python 补 yolo 免开关说明
- **SKILL_DSL.md**：max_steps 数字对齐实现（默认 8，clamp 1-20）；Rollback 段标题别名与「按声明顺序执行」语义写清

**测试**：cargo test --lib 425 → **428 全绿**（vars 转义 ×3），集成 29 全绿。前端本轮无改动。

## 2026-08-27（周四·午 3）审计 P1 七项全修（失败判定统一 + /stop 会话化 + 逐步执行隔离）

承接「午 2」的 P0 五连修，把审计报告（`docs/AUDIT-BOT-PIPELINE-2026-08-27.md`）的 7 个 P1 全部修掉：

- **P1-6 失败判定统一**：`audit.rs` 新增 `tool_call_failed`（全链路唯一真相源）——熔断「已强制终止」/暂停「技能已暂停」/门禁拦截「⚠️」/用户拒绝四类文案原先两套口径都不认（末步熔断误报「✅ 完成」、门禁拦截对 PREVR 隐身），现在统一判失败；`classify_text` / `is_tool_failure_text` / `skill_on_step_post` / PREVR / 幻觉守卫全部重接线到同一判定
- **P1-7 Replan 修复**：fail_reason 从「只传工具名」改为带真实错误文本（`last_fail_reason` 跟踪）；replans_used 不管成败都消耗预算（原先失败 replan 不计数，Planner 故障时每轮白烧）
- **P1-8 /stop 会话化**：StopMap 注册表带 session_id，`bot_stop(sessionId)` 只停当前会话（技能终止/挂起清理/停止标志全部按会话过滤；lib.rs 退出清理传 None = 全部）；工具批循环体内补 `stop.stopped()` 检查——/stop 后剩余调用回填占位 tool 响应（保 tool_calls→tool 协议完整）不再执行；DSL 调度器透传 StopGuard（在途 run_python 可中断，原先必须跑完）；/stop 本身补审计
- **P1-9 AwaitUser 断头路**：`__await_user__` 哨兵不再直出前端，换成用户可读的暂停提示 + 审计
- **P1-10 幻觉守卫修正**：动词表去「完成」（「已完成搜索」类只读汇报误拦）补「保存/记住」漏拦，「已完成任务」走 PLAIN 整段匹配；守卫补轮话术区分逐步执行模式（不误导模型代调 toggle_subtask）；`link_file_to_task` 已随 P0-4 补进 MUTATING_TOOLS
- **P1-11 逐步执行规则互斥**：STEPWISE_ADDENDUM 补「与上方任务卡执行规则冲突时以本段为准」
- **P1-12 exec_steps 隔离**：PENDING 全局单槽 → 按会话分槽（HashMap），跨会话不再静默覆盖；resume 的 Continue/Redo 错误路径补审计 + 复位 bot_assigned 头像（原先 LLM 失败后任务卡永远顶机器人头像）
- 顺手修：软警告话术「连续调用」→「累计调用」；两处「默认 20 轮」过时注释 → 50；前端 ChatPanel 两处 bot_stop 调用传 sessionId + 测试断言同步

**测试**：cargo test --lib 421 → **425 全绿**（tool_call_failed ×2、claims_mutation +1、exec_steps 会话分槽 +1），集成 29 全绿；vitest 110 全绿；tsc 零错。

## 2026-08-27（周四·午 2）Bot 工具/Skill 链路全面审计 + P0 五连修

**审计**（4 路并行子代理 + 人工复核）：报告落盘 `docs/AUDIT-BOT-PIPELINE-2026-08-27.md`，覆盖 DSL 调度器 / 系统提示词 / 工具注册表面 / 中间件管线的逻辑性、完整性、一致性。结论：schema↔dispatch 28↔28 对齐、门禁顺序正确、session 透传主链无断点；但发现 5 个 P0 + 7 个 P1 + 一批 P2（失败判定三口径分叉、/stop 一停全停、`__await_user__` 直出前端等，待排期）。

**P0 修复**：
- **P0-1 DSL Done 不收尾**：`run_skill_scheduler` 成功路径补 `skill_finish`（Running→Completed）——原先僵尸 Running 让原子闸门洞开 + 后续工具调用被计入僵尸 run 直至步数熔断卡死会话
- **P0-2 任务卡执行路径的原子工具合法化**：`StopGuard` 加 `allow_atomic`（`new_task_exec` 构造），`execute_task_core` / `exec_steps` 三处切换；`execute_tool_impl` 门禁放行 `is_skill_active || allow_atomic`——此前 EXECUTE prompt 要求的 `link_file_to_task`/`create_word_revisions` 在无 Skill 的任务卡路径必被自家网关硬拦；SYSTEM_PROMPT 规则 10 补「被拦改用 create_word」回退措辞；两个原子工具的 schema description 标注「内部原子」属性
- **P0-3 `complete_task` 走 `resolve_task`**：taskId 精确匹配优先 + 交叉校验，与 schema「taskId 优先于 title」和 EXECUTE 规则 4 对齐——原先只读 title，只传 taskId 时确定性失败
- **P0-4 `mutation_done` 按执行结果置位**：新增 `mutation_succeeded`（门禁拦截 ⚠️ / 用户拒绝 / Warn/Error 分级失败都不算「动过手」），幻觉守卫不再被「调过但失败」的调用架空；顺手把 `link_file_to_task` 补进 `MUTATING_TOOLS`（审计 P1-10 之首，与本次改动直接相关）
- **P0-5 回滚段不再自咬**：`state.rs` 新增 `reopen_failed_run_for_rollback` / `restore_failed_run_after_rollback`（回滚窗口内 Failed→Running 临时重开让原子工具过门禁，结束后复原终态）；`run_rollback_segment` 逐步判定成败 + 逐条审计（不再 `let _ =` 吞掉），返回值改为契约语义「段存在且全部回滚步骤无失败」（对齐 SKILL_DSL.md §4.3.2）

**测试**：新增 mutation_succeeded 4 例 + reopen/restore 1 例；cargo test --lib 416 → **421 全绿**，skill_e2e 8 全绿。踩坑：`⚠️` 是双码点字符（U+26A0+FE0F），char 字面量编译报错，用字符串字面量。

## 2026-08-27（周四·午）修订文档收口统一入口：强制 .NET，Python 兜底

**动因**（老板指令）：修订文档生成走 Rust 内部统一入口，强制 .NET 优先、Python 仅兜底；顺带修掉一个真问题——dotnet 分支此前在 async fn 里**同步直跑** `run_dotnet_revisions`，最长 120s 阻塞压在 async runtime worker 上（NEW-C-1 修过 doc_* 同款问题，dotnet 分支是漏网之鱼）。

**实现**：
- `bot_py.rs` 新增 `run_doc_revisions` 统一入口：整体 spawn_blocking 隔离，强制 .NET 优先——不可用（无运行时/无 dll）、执行失败（非零退出）、**运行错误（spawn/超时等，此前 `?` 直抛不兜底）** 三种情况一律记审计后回退 Python 脚本；返回 `(PyRunResult, 引擎标记)`
- `doc_make_word_revisions` 变薄：只拼参数 + 调统一入口 + 按退出码判成败；签名改为返回 `(路径, 引擎)`，成功/失败审计统一带 `engine: dotnet|python`
- `bot.rs` `tool_create_word_revisions`：适配新签名，成功消息标注实际引擎（.NET OpenXML / Python 兜底）
- 行为不变：参数契约（title/originalPath/original/revised/filename）、输出落 AI_Gen_Files 不覆盖、回退后产物与旧 Python 路径完全一致

## 2026-08-27（周四）修订模式切 .NET OpenXML 官方修订（Python 回退保留）

**动因**（老板指令）：修订模式原用 python-docx 手拼 OOXML（`OxmlElement` 逐个拼 w:ins/w:del），改用 .NET OpenXML SDK 的官方修订 API（`InsertedRun`/`DeletedRun`），类型系统层面保证「w:del 内必须 w:delText」这类铁律不可能写错；minimax-docx 技能的 `references/track_changes_guide.md` + `Samples/TrackChangesSamples.cs` 为依据。

**实现**：
- 新工具 `src-tauri/dotnet/WmDocxRevisions/`（net8.0 + DocumentFormat.OpenXml 3.5.1，与技能同版本可命中本地 NuGet 缓存；`RollForward=LatestMajor` 兼容只装 .NET 10 运行时的机器）
- 段落级 + 行内字符级 diff 用 LCS opcodes（替代 difflib.SequenceMatcher，输出形态对齐：equal/delete/insert/replace 合并相邻段）；超长段落（n×m > 4M 单元格）退化整段替换防内存爆
- 修订标记：唯一递增 w:id（1001 起）+ author「WMessage AI」+ ISO8601 UTC date；删除/新增的删除线与颜色交给 Word 审阅视图渲染（不写死字符级格式，更贴近官方行为）
- Rust 侧：`run_python_at` 入口参数化（`run.py` → `entry: &str`）→ dotnet 走同一执行内核（超时/限额/进程组强杀/审计全继承）；`cached_dotnet` 探测 + `dotnet_revisions_dll` 定位（exe 同目录 dotnet/ → 开发模式 CARGO_MANIFEST_DIR/dotnet/）；**dotnet 或 dll 不可用、或 dotnet 执行失败 → 自动回退原 Python 脚本**（行为不变，审计记 `engine: dotnet` / fallback 留痕）
- 发布注意：Windows 绿色包需把 `wm-docx-revisions.dll`（及其 deps）放进 exe 同目录 `dotnet/`；未放则静默走 Python 路径
- 测试：新增 `dotnet_revisions_tool_generates_valid_track_changes` 端到端（本机有 dotnet 才跑，解开 docx 验证 w:ins/w:del/delText/author）；dev-dependency 加 zip（deflate）；cargo 416 全绿
- 踩坑：顶级语句里 record 声明必须在最后（CS8803）；`zip` crate default-features=false 会关掉 deflate 解不开 docx

## 2026-08-26（周三·深夜 3）修复 Windows 绿色版数据目录漂移

**现象**（老板反馈）：绿色版运行时生成文件大多在 WMessage 文件夹内，但有几次 AI_Gen_Files 建到了文件夹外。

**根因**：便携探针 `probe_dir`（exe 目录可写用它，不可写退系统应用数据目录）**每次调用都现写探针文件**，无缓存——杀软临时锁定 / UAC 状态变化 / 压缩包内直接双击运行等瞬时失败，会把当次数据目录翻转到 app_data，AI_Gen_Files、数据库、bot.log 分裂两地（翻转那次机器人看到的还是另一份任务库）。

**修复**（audit.rs）：
- 探测结果进程内 `OnceLock` 定版（`probe_dir_cached`）：首次 `probe_log_dir` 调用定版，整个运行期不再翻转
- 兜底翻转记 WARN 审计 `data_dir_fallback`（写清 exe_dir / resolved / 原因），写进翻转后的目录的 bot.log，可诊断；**写日志走独立线程**——调用方可能正持有 BOT_LOG_LOCK（audit_log → data_dir → 这里），write_warn_audit_to 再拿同一把锁会死锁（std Mutex 不可重入，实锤挂死 cargo test 一轮）
- `bot.rs` 降级 key 路径（plaintext_key_path）优先复用定版缓存，防 key 文件与数据库分裂两地
- **测试构建不缓存**（cfg(test) 每次现探）：同进程多测试各自探测不同临时目录，全局缓存会互相劫持（实锤 3 个测试失败）；定版语义由可注入内核 `probe_dir_cached_in` 的单测覆盖
- 测试：新增「定版后探测条件变化不改变结果」用例；cargo 415 全绿

## 2026-08-26（周三·深夜 2）PREVR 第 1+2 层：失败换策略提示 + 复杂任务动态计划

**动因**：架构对比后老板拍板落地 PREVR（Plan-Execute-Verify-Replan）的前两层——rust bot 此前工具失败只会把错误抛给用户，不会自己换办法。

**第 1 层：执行器内验证（run_model_loop）**
- 工具结果复用审计分级（classify_text Warn/Error = 失败）做失败检测
- 单次失败 → 注入「换策略」提示（换参数/换工具/拆小步骤）；同工具连续 ≥2 次失败 → 禁止相同调用、要求如实告知用户
- 与 soft_warn 同一协议安全位：提示在本轮 tool 响应全部回填后才注入（不破坏 tool_calls→tool 序列）

**第 2 层：动态计划（新模块 bot_plan.rs）**
- `needs_plan` 保守启发式（多步关键词/「先…再…」/多附件）命中才触发 Planner，简单问答零额外成本
- Planner = 单次非流式 LLM 调用，输出 JSON 步骤列表；`parse_plan` 容忍包裹文字、上限 8 步；失败/解析不出 → 降级原自由循环不阻断聊天
- 计划注入 system prompt（【执行计划】块，含「走不通就调整并说明」指令）；仅聊天主路径启用，execute_task_core / exec_steps 目标单一不规划
- **Replan**：run_model_loop 内同工具连续失败 ≥2 且有计划 → 带失败原因重规划剩余步骤（≤2 次硬上限防死循环），新计划注入对话
- 计划纯提示词文本，执行仍走 execute_tool 全量安全网关（白名单/授权/确认/熔断），无绕过通道
- 审计：plan.generate / plan.skip / plan.replan / plan.replan_failed
- 测试：bot_plan 7 个新用例（needs_plan 命中/放过、parse 容错/截断/拒绝、plan 块渲染）；cargo 414 全绿；npm 110 全绿；tsc 通过

## 2026-08-26（周三·深夜）会话隔离修复：流式/Skill/确认/逐步执行全链路按会话归属

**动因**（老板要求审计「聊天会不会串」）：审计发现存储层（bot_messages 按 session_id）和前端切换（sessionIdRef 竞态守卫）是隔离的，但运行期有四个串线通道。

**修复**：
- **流式事件（P0）**：run_model_loop 的 bot-chat-delta/bot-think-delta/bot-tool* 全部收口到 `emit_stream` 闭包，只在交互实例（StopGuard.is_interactive）广播；后台定时任务（execute_task_core interactive=false）不再向挂件推流——此前定时任务到点触发时，其 LLM 输出会追加进用户当前会话的 streaming 气泡并被 persistHistory 当成本轮对话落库
- **Skill 按会话归属（P1）**：SkillRun 新增 session_id；start_skill/active_skill_run_for/skill_finish/skill_mark_paused/skill_confirm_result/skill_on_step(_post)/is_skill_active_for 全部按会话过滤——会话 A 暂停中的 Skill 不再被会话 B 的主循环推进、确认或收尾；start_skill 的「单活动技能切换」也只结束同会话技能
- **确认弹窗按会话归属（P1）**：ConfirmMap 条目记录归属会话，bot-confirm 事件带 sessionId，前端只弹当前会话的确认；后台执行（interactive=false）不弹窗直接拒绝（无人在场必超时，且会串进用户当前会话）；bot_confirm_response 从条目取回会话再恢复对应 Skill
- **逐步执行挂起按会话归属**：PendingExec 带 session_id，has_pending_for 按会话匹配——会话 A 挂起等确认时，会话 B 的消息不再被 resume 截胡
- 透传链：StopGuard::new(interactive, session_id) → bot_chat/bot_execute_task 命令收 sessionId 参数 → run_model_loop/execute_tool_impl/各工具；exec_steps 三个 StopGuard 同理
- 测试：后端 407 全绿（StopGuard/SkillRun 构造同步补字段）；前端新增「确认弹窗按会话过滤」用例（捕获 bot-confirm 监听器，验证别会话/无归属不弹、本会话弹）；110 全绿 + tsc 通过

## 2026-08-26（周三·夜）熔断上限 50 + 发送/停止一体键

- **熔断放宽**（老板拍板）：默认对话轮数 20 → **50**；单轮 Function 调用上限 10 → **50**，软警告 7 → 35（保持 ~30% buffer）。失控防护仍靠幻觉守卫 + 软警告 + 停止键
- **发送/停止一体键**（老板拍板）：聊天发出后发送键变为红框正方形 ■ 停止键（`text-[var(--danger)]`），点击即 `bot_stop` 立马中断本次运行；中断或回复结束自动变回发送键。输入框 placeholder 同步提示；/stop 斜杠命令保留可用
- 测试：熔断边界断言改 50/51 与 50 轮默认值；ChatPanel 新增「busy 时停止键点击调 bot_stop」用例
- 验证：`cargo test --lib` 407 全绿；`npm test` 109 全绿；tsc 通过

## 2026-08-26（周三·晚）任务卡绑定文件交互改版：点名直开 + chip 内「复制」字样

**动因**（老板指令）：📂 打开 / 📋 复制两个 emoji 按钮与具体文件不对应（多文件时 📂 还要弹选择列表），交互绕。

**改造**（主窗口 TodoCard + 挂件 TaskCardContent 同步）：
- 打开：删 📂 按钮与多文件选择列表 → **点绑定文件名/文件夹名直接打开**该文件（chip 名变 button）
- 复制：删 📋 按钮 → 每个 chip 在解绑 × 前加「**复制**」字样（逐文件 `copy_file_with_title`，复制文件+标题）
- 样式：chip 小一号字号（名称 11px / 复制 10px）+ `nm-inset` 凹陷底色与卡片底色区分
- 绑定操作行只留绑定类按钮（＋绑定文件 / 📁绑定文件夹 / ×解绑全部）
- WidgetApp 回调改 per-path：onOpenFilePath / onCopyFilePath（删除 onOpenFile/onCopyFile 整卡回调）
- `copy_files_with_title`（多文件复制）随改版整体下线：命令、macOS/Windows 平台 helper、invoke 注册全删
- 测试：TodoCard/TaskCardContent 的 📂 多选列表与 📋 用例改写为新交互；`npm test` 108 全绿 + tsc 通过

## 2026-08-26（周三）文件访问改造：白名单硬拦 → 执行前授权（Kimi CLI 风格）+ yolo 模式

**动因**（老板原话：「该读的不让读，还要绑定文件，流程繁琐」）：read_text_file/grep_files/list_files/extract_document 白名单外硬拒绝，要读其它文件得先绑定任务卡或改设置页，流程打断。

**改造**：
- `bot-config.json` 新增 `permMode`：`strict`（白名单外硬拒，旧行为）/ `ask`（**新默认**，白名单外弹授权窗）/ `yolo`（全放行不弹窗，文件工具 + run_python 免开关）；`PermMode::from_cfg` 非法值回退 ask
- `bot_fs::resolve_with_perm` 三分流：白名单命中静默放行；ask → 复用 ConfirmMap 弹三选一窗（允许一次 / 始终允许该目录 / 拒绝，60s 超时与挂件不可见默认拒绝）；yolo → 直接放行；全程记审计（`bot_fs.yolo_allow / ask_allow_once / ask_allow_always / ask_denied`）
- 「始终允许该目录」：文件取父目录、目录取自身，自动追加进 `allowedDirs` 落盘（`bot::add_allowed_dir`）
- **allowedDirs 语义修正**：旧「非空整体替换内置默认」→ 新「在内置默认（桌面/下载/文档 + 任务卡绑定文件夹）之上**追加**」——否则始终允许写入一个目录后默认目录反而失效
- extract_document / create_word_revisions 的 path 校验改走同一分流（任务卡绑定文件 / AI_Gen_Files 仍静默放行）
- run_python：yolo 模式跳过 py-enabled 开关（记 `py_exec | yolo_bypass`）；ask/strict 维持设置页开关门控
- 前端：ChatPanel 确认弹窗按 `kind="file_access"` 渲染三按钮（`bot_confirm_response` 加 `always` 参，老调用兼容）；SettingsPage 加授权模式三态选择器（yolo 带风险提示）+ 白名单文案改追加语义
- prompt 规则 19 / 安全红线 / TOOLS 描述同步「弹窗授权」措辞
- 测试：PermMode 解析/老配置兼容、`merge_raw_dirs` 追加语义；顺带修 eefa78f 遗留的两个前端测试（设置页出现两组「📤 导出/📥 导入」按钮导致 getByText 二义性，改取第一个 = 任务导入/导出）
- 验证：`cargo test --lib` 407 全绿；`npm test` 107 全绿

## 2026-08-13（周四）M1 脚手架 + M2 看板 + M3 起步

### M1 脚手架
- create-tauri-app react-ts 模板：Tauri 2 + React 19 + TypeScript + Vite 7 + Tailwind v3
- 依赖：@dnd-kit/core、@dnd-kit/utilities、postcss、autoprefixer
- 新拟态样式体系建在 `src/ui/main.css`（`@layer components`）

### M2 看板
- `KanbanBoard.tsx`：dnd-kit 三列（待办/今日/完成），PointerSensor distance:5，列间拖拽
- localStorage 持久化；新建任务自动进标题编辑态
- 卡片结构顺序（固定，别再改）：标题 → 备注 → 标签 → 子任务 → 文件按钮 → 截止时间（**截止永远最底**）
- 列头最终版：全宽 `nm-inset` 凹陷胶囊 + text-lg font-semibold（苹方/SF Pro），标签顶左、计数顶右
- **今日规则**：截止日期=当天 且 列=待办 → 自动进「今日」列；每分钟 + 设 due 时 + 启动时套用；完成列豁免

### M3 文件绑定（起步）
- Rust 命令 `copy_file_with_title(path, title)`：macOS 用 NSPasteboard（public.file-url + NSFilenamesPboardType + 标题文本），Windows 用 CF_HDROP（cfg-gated，未编译验证）
- 前端：📎 绑定文件 / 📁 绑定文件夹（plugin-dialog），📂 打开（plugin-opener），📋 复制文件+标题，× 解绑

## 2026-08-14（周五）M3 收尾 + 归档回收站 + M4 挂件 + 系列迭代

### M3 完成 + 归档 + 回收站
- `copy_file_with_title` 编译通过，粘贴到 Finder/飞书/微信可得文件，文本场景得标题
- **自动归档**：进完成列记 `completedAt`，满 7 天自动 `archived`（`ARCHIVE_AFTER_MS`，每分钟检查）
- 归档页：搜索（标题+备注）+ 标签筛选（计数降序）+ ↩ 恢复；回收站：软删除 + 恢复/彻底删除/清空
- 主窗口头部三视图切换：未选中 `nm-outset` 凸起 / 选中 `nm-inset` 凹陷，文字颜色统一

### M4 侧边磁吸挂件
- Rust 侧创建 `widget` 窗口：transparent + decorations(false) + always_on_top + skip_taskbar + visible_on_all_workspaces（macOS 透明窗口必须 `macOSPrivateApi: true`）
- URL hash 分流：`index.html#/widget` → WidgetApp
- 收起 = 44×220 触发条，悬停展开 320×560 面板，📌 锁定常驻
- **数据同步三保险**：localStorage 同源共享 + `tasks-changed` tauri 事件 + 5s 兜底轮询

### 挂件与主窗口一致性迭代（老板逐条验收）

| 时间 | 变更 |
|---|---|
| 05:42 | 挂件任务卡与主窗口显示一致：新增 `TaskCardContent` + `format.ts`（basename/formatDue 共用） |
| 05:50 | 标题右侧打勾圆圈：新增 `DoneCircle` 共享组件，主窗口+挂件都可一键完成/取消完成 |
| 05:57 | 标题以下折叠：新增 `FoldToggle`，`Task.collapsed` 字段持久化，两窗口同步 |
| 06:02 | 主窗口删除按钮移到截止日期右侧，改小 emoji 🗑️（回收站视图不重复显示） |
| 06:07 | 挂件子任务可勾选 + 📂/📋 文件按钮（与主窗口一致） |
| 06:10 | 头部切换按钮凸/凹按压态（新增 `.nm-outset`） |
| 06:14 | 任务卡标题字体统一 14px/500（新增 `.nm-task-title` 共用类） |
| 06:22 | 挂件圆角处阴影段修复：外阴影漏进透明切角 → 改内阴影 `inset -6px 0 10px -4px` |
| 06:31 | 挂件新建任务 + 标题编辑（新建自动进编辑态） |
| 06:40 | 挂件点标题 → 跳主窗口并进入该任务编辑态（`edit-task` 事件 + 主窗口 setEditingId） |
| 11:07 | 挂件 logo 定 #3 深蓝双方块（老板拍板）：触发条 🗂 表情 → 24px 透明 PNG，面板头部加 20px logo；资产 `src/assets/widget-logo.png`（源 `docs/logo/assets/3/`） |
| 11:12 | 贴顶时触发条竖变横（220×44，flex-row + 水平文字 + 上缘内阴影 `.nm-sidebar-panel-top`）；初始定位与 collapse 按 edge 选择横/竖尺寸 |
| 11:18 | 挂件头部「全部/今日/锁定」按压态：未选中 `nm-outset` 凸起胶囊 → 选中 `nm-inset` 凹陷胶囊（与主窗口头一致） |
| 11:22 | 文件打开/复制按钮（主窗口 TodoCard + 挂件 TaskCardContent）：新增 `.nm-btn` 动作按钮类，默认凸起、`:active` 按下瞬间凹陷（动作按钮用 momentary 按压态，不用常驻切换） |
| 11:24 | 绑定文件/文件夹按钮（📎/📁 幽灵文字 → `nm-btn` 凸起胶囊 + 按压态） |
| 11:27 | 剩余动作按钮统一 `nm-btn`：归档 ↩恢复、回收站 ↩恢复/🗑彻底删除（红字保留）、挂件 +新建任务长条（常驻 nm-inset 仅保留列头/输入框/标签芯片等非动作元素） |
| 11:32 | 出 Windows 包 v2（含 SQLite/挂件 logo/贴顶横条/按钮体系全套改动），飞书发送老板验收 |
| 11:36 | 文档更新：README 全面刷新（SQLite 架构、单写者同步、交叉编译说明、logo 文档指针）+ DEVLOG 待办刷新 |
| 06:47 | 主窗口关闭改为隐藏（CloseRequested prevent_close + Cmd+Q 走 ExitRequested destroy） |
| 07:09 | 挂件新建任务长条移到「全部/今日」下方，新任务从列表顶部出现 |
| 07:28 | 挂件打勾后任务消失（挂件只显示未完成：todo + doing） |
| 07:36 | 挂件自由拖动 + 贴边吸附：右/左/顶 24px 容差、圆角跟随边缘、锚点持久化（`wmessage-widget-pos`） |

### 存储落盘（方案2）+ Logo 定稿 + Windows 交叉编译验证（09:22-10:40）

- **Logo 定稿**：老板 5 张豆包渐变玻璃质感图，裁定以图片为准；去水印/透明底/多尺寸 → `docs/logo/assets/`；规范 `docs/logo/WMessage-LOGO-GUIDELINES.md`；#4 生成全套 Tauri 图标到 `src-tauri/icons/`
- **Windows 剪贴板修复**：`copy_file_windows` 首次编译验证（cargo check 交叉目标），windows 0.61 API 修正见踩坑记录
- **Windows 交叉编译**：产出 9.5MB 独立 exe（静态 CRT），Win10 实测可运行
- **存储落盘（方案2）**：任务数据 localStorage → 应用数据目录 `data.json`（Win: `%APPDATA%\com.renshi.wmessage\data.json`），防清理工具误删。Rust `load_data`/`save_data`（原子写）；单写者：主窗口统一落盘，挂件只读 + `tasks-updated` 携带数据上报；旧 localStorage 首次启动自动迁移；POS_KEY（挂件锚点）仍走 localStorage

### 存储换 SQLite（方案B，10:54 老板拍板）

- 弃 data.json 全量覆盖，上 rusqlite（bundled）行级增量：`db_load`/`db_upsert`/`db_delete` 三命令（`src-tauri/src/db.rs`），表 tasks 单表，tags/subtasks 存 JSON 文本列，WAL + busy_timeout 2s
- 前端统一变更出口 `mutate`（App.tsx）：计算新数组 → diff 出 upserts/deletes → 行级落盘 → 广播 `tasks-changed`；挂件 `applyAndSync` 同样 diff 后经 `tasks-updated` 上报 {upserts, deletes}，主窗口统一落盘（单写者不变）
- 迁移链：SQLite 空库时 Rust 侧自动导入方案2 的 data.json 并删除；更早的 localStorage 数据由主窗口首次启动导入后清除
- 规则（今日/归档）照旧在加载与每分钟重套，diff 后行级落盘

## 踩坑记录（避免重蹈）
- Tailwind `@apply` 不能引用自定义组件类（`.nm-card-hover { @apply nm-card }` 编译报错）
- TodoCard 的 useDraggable 在 DndContext 外会崩 → 归档/回收站页必须包空 `<DndContext>`
- 卡片内裸 `<button>` 是 inline 会并排 → 注意 display（块级化）
- macOS 透明窗口：`tauri.conf.json` 必须 `"macOSPrivateApi": true`
- `WebviewWindow.getByLabel` 返回 Promise，必须 await
- **矩形透明窗口里的圆角面板禁用外阴影**（直边被裁、圆角漏光，形成"一段阴影"）→ 用 inset 内阴影
- 主窗口关闭=销毁会让挂件失去唤起目标 → CloseRequested 拦截改隐藏；Cmd+Q 在 ExitRequested 里 destroy 主窗口
- Rust `get_webview_window` 需要 `use tauri::Manager;`
- Windows 交叉编译链路（macOS → exe）：`brew install llvm lld` + `cargo install cargo-xwin`，然后 `tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc`；缺 llvm-rc 会报 `tauri-winres NotAttempted("llvm-rc")`，缺 lld-link 链接阶段挂
- windows crate 0.61 API 大改：`GlobalAlloc/GlobalLock/GlobalUnlock/GMEM_MOVEABLE` 在 `System::Memory`；`CF_HDROP/CF_UNICODETEXT` 在 `System::Ole` 且为 `CLIPBOARD_FORMAT(u16)` 新类型（传给 `SetClipboardData` 用 `.0 as u32`）；`SetClipboardData` 第二参是 `Option<HANDLE>`（与 `HGLOBAL` 不同新类型，需 `HANDLE(h.0)`）；`GlobalLock` 返回裸指针不是 Result；`BOOL` 只有 `From<bool>`（用 `true.into()`）
- **localStorage 会被清理工具当缓存删**（EBWebView 目录），关键数据必须落盘到 app_data_dir 的 data.json（temp+rename 原子写）
- 落盘架构单写者：主窗口统一写文件，挂件只上报 `tasks-updated`（携带数据）；主窗口 persist 用 `loaded` 门控，否则启动瞬间 async 加载完成前会写空数据覆盖旧档
- 引入 C 依赖（rusqlite bundled）后，Windows 目标裸 `cargo check` 会挂（cc-rs 用宿主 cc 编 sqlite3.c 找不到 stdlib.h），必须 `cargo xwin check --target x86_64-pc-windows-msvc`（cargo-xwin 接管 C 编译器 + SDK 头文件）

## 后续待办

- M5 全局快捷键（已做 ✓）
- M6 打包：交叉编译 exe 已通 ✓（v2 已发验）；剩 NSIS 安装包 + 代码签名 + macOS dmg
- Logo 规范 2.3 单色托盘版（16/32px，现有渐变图缩到托盘尺寸会糊）
- 挂件窗口伸缩平滑动画（当前瞬时伸缩）polish
- ~~深色模式~~ ✓（23:30-23:55 完成，见下节）
- 数据备份/导出（可选；SQLite 文件本身可整体拷贝）

### 深色模式（23:30-23:55，老板逐条验收）

- **主题变量体系**（`src/ui/main.css`）：`:root` / `.dark` 两套独立变量——`--bg` 页面底（深色 #1f242b 中度深灰非纯黑，比卡片表面更暗拉层次）、`--surface` 卡片/按钮表面（#2d333e）、`--sh-low/high`（+strong/hover 档）双阴影（深色专属：更深暗阴影 #13161b + 偏亮弱高光 #4a5464，不复用浅色参数）、`--sh-top` 顶部 1px 内高光（深色玻璃边缘感，浅色 transparent 关闭）、`--edge`、`--input-bg`、`--hover-bg`、文字五级 `--t1..t6`（深色主文字 #f0f3f8 非纯白，逐级降亮但保证可读）、品牌色 `--brand/--brand-strong`（低饱和蓝紫灰，深色提亮 #a3b6dd）、`--success/--danger`
- **nm 组件全部走变量**：nm-card（凸起双阴影+顶部内高光）、nm-inset（内双阴影）、nm-outset、nm-btn（hover 阴影加深/active 内凹），hover/active 两套都有清晰反馈；主题切换平滑过渡（background-color/box-shadow 0.28s，body color 同步）
- **组件颜色全部改变量引用**（`text-[var(--tN)]` 等，脚本批量替换 9 个文件）：看板卡片、折叠卡、归档/回收站、挂件面板、按钮、输入框（含 placeholder 颜色）、checkbox accent 走品牌色
- **主题三态**（`src/theme.ts`）：light/dark/system，localStorage 持久化；system 用 matchMedia 监听系统外观实时切换（含旧 WebView addListener 兼容）；双窗口 storage 事件同步；index.html 预渲染脚本防闪屏；header/挂件按钮三态循环（☀️/🌙/🖥️），Rust 侧 Cmd+Ctrl+T 快捷键快速浅/深切换（emit toggle-theme 只发主窗口，挂件靠 storage 同步防双触发）
- **挂件深色适配**：深蓝 logo 提亮（`.dark .widget-logo` brightness 1.9）
- 验证：tsc/cargo check/npm run build 全过；dev 实例实测浅色正常、快捷键切深色后像素采样+vision 确认（背景中度深灰、卡片与背景层次分明、文字层级可读、双阴影可见、挂件同步变深）

### 平台窗口关闭行为（老板验收，11:42–11:52）

| 时间 | 变更 |
|---|---|
| 11:42 | 主窗口 board 视图「+ 新建任务」按钮 `nm-inset` → `nm-btn`（动作按钮类，凸起/按下凹陷） |
| 11:50 | Windows 关主窗口=隐藏进托盘（不再直接退出）；托盘图标用 **#3**（`docs/logo/assets/3/`）；右键菜单「打开主窗口 / 退出」，**退出才是真退出**（`app.exit(0)` → `ExitRequested` → destroy main）；左键单击/双击托盘恢复主窗口 |
| 11:52 | macOS 行为不变：关闭=隐藏，Cmd+Q 真退出；托盘仅 Windows（`#[cfg(target_os = "windows")]`） |
| 12:17 | 任务卡拖拽手柄：主窗口+挂件标题前拖拽区，悬停才显示 ✋ 小手图标；主窗口 listeners 从整卡移到手柄（只能从手柄拖）；挂件手柄仅展示不参与拖拽 |
| 12:17 | 挂件任务卡标题：单击 → 双击才打开主窗口编辑（openInMain） |
| 13:31 | Win10 缺 WebView2 Runtime 报 webview2loader.dll 找不到：便携包内附微软官方 Evergreen 安装器 MicrosoftEdgeWebview2Setup.exe + README.txt（先装一次再跑 exe；Win11 自带无需装） |
| 12:26 | 任务卡拖拽排序：主窗口三列内排序 + 跨列（@dnd-kit/sortable 多容器 SortableContext + DragOverlay），挂件列表排序（DndContext + SortableTaskCard）；Task 加 `order` 字段（SQLite `ord REAL` 列 + 老库 ALTER 迁移），排序用左右邻居中点（边界 ±1、间隙耗尽全量整数重排，`assignInsertOrder`）；挂件排序后拖拽 click 防误聚焦（200ms 守卫） |

- Cargo.toml tauri features 增加 `tray-icon`、`image-png`；托盘图标生成自 `3-1024.png` → `src-tauri/icons/tray-wm-32.png`（另备 16px）
- 托盘 API 验证：macOS `cargo check` 通过；托盘代码块临时去 cfg 门在 macOS 编译验证 0 错误后恢复
- Windows 交叉 check 被 `libsqlite3-sys` C 交叉编译卡住（本机无 Windows C 工具链），Windows 侧需实机 build 验证

### 便携模式 + 合并导入 + 任务卡交互大改 + 打包定案（13:50-17:25，老板逐条验收）

| 时间 | 变更 |
|---|---|
| 13:50 | **便携模式**：数据库随 exe 走（写探针检测 exe 目录可写性，不可写兜底 app_data_dir，首次启动自动迁移旧库），U 盘拷走数据随行 |
| 14:02 | **合并导入**：「导入数据库」选 wmessage.db 按 id 并集合并，同 id 保留 `updatedAt` 更晚者；Task 加 updatedAt（写路径自动打戳，老数据回填 0）；外部库只读打开、缺 ord/updated_at 列容忍 |
| 15:52 | 「🗂 导入数据库」改名「导入数据」，仅首页显示 |
| 16:00 | 任务卡交互：删隐藏 ✋ 改 ☰ 三横线拖拽手柄（悬浮标题左侧留白）；折叠 chevron 移到标题正下方居中细行 |
| 17:00 | ☰ 手柄太靠边/离标题太近 → 改标题左侧占位，标题行 gap-2 |
| 17:13 | 折叠 chevron 移到标题行内、标题与对勾之间，三角符号加大；标题折叠时单行 truncate（悬停 tooltip 全文），展开才显示全部标题和设置 |
| 17:19 | **拖拽 bug**：待办拖不进空完成列——松手时指针落在卡片自身新位置（over=active）早退丢草稿 → 改为草稿已跨列时按草稿提交 |

- **Windows 打包定案（16:46 老板确认，必须照此执行）**：`cargo clean` 全量重编 + `npx tauri build --target x86_64-pc-windows-gnu --no-bundle` 完整流程；**直接 `cargo build` 出的 exe 缺内置页面资源**（报「无法访问此页面」）
- 交叉编译链路换 mingw-w64 + `x86_64-pc-windows-gnu`（`CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc`，CC/CXX 同理）；NSIS 在 macOS 打不了（makensis 跨平台崩），发绿色版 zip
- **绿色包**：wmessage.exe + WebView2Loader.dll（必带，缺它报「找不到 webview2loader.dll」，与 WebView2 Runtime 无关）+ MicrosoftEdgeWebview2Setup.exe（备用）+ README.txt
- **zip 必须用 Python zipfile 打**：macOS `zip -j` 的 Unix 扩展字段让 Win 资源管理器解压报「位置不可用」

### 归档/回收站三列改造（22:15-22:38）

| 时间 | 变更 |
|---|---|
| 22:15 | 归档页：单列长卡 → **按标签分组的三列小窗口**（标签名作标题 + 计数，任务卡竖排窗口内；新标签自动新建窗口；无标签归「未分类」；搜索过滤隐藏空窗口） |
| 22:28 | 归档窗口标题加 # 前缀（代表标签）+ 窗口可拖拽换列（标题行手柄 + rectSortingStrategy），顺序存 localStorage 持久化；搜索过滤态禁拖 |
| 22:34 | 拖拽激活 bug：`onPointerDown={undefined}` 写在 listeners spread 之后把 dnd-kit 的 onPointerDown 覆盖没了 → 仅无手柄时覆盖 stop |
| 22:38 | 回收站页：单列 → 三列网格（grid-cols-3，从左到右依次填充） |
| 22:41 | 文档更新（本次） |

### M5 全局快捷键（22:53-23:00）

- 引入 `tauri-plugin-global-shortcut = "2"`（Rust 侧注册，无需前端 capabilities）
- **唤起/隐藏主窗口**：macOS `Cmd+Ctrl+W` / Win+Linux `Ctrl+Alt+W`（可见则隐藏，隐藏则 show+unminimize+focus）
- **快速新建任务**：macOS `Cmd+Ctrl+N` / Win+Linux `Ctrl+Alt+N`（唤起主窗口 + emit `quick-add`，前端切回首页 + addTask 进入标题编辑态）
- 键位选择避开冲突：macOS 不用 Cmd+Shift+N（Finder 新建文件夹）、Win 不用 Ctrl+Shift+N（浏览器隐身窗口）；Modifiers 用 cfg 按平台选 SUPER|CONTROL / CONTROL|ALT
- 踩坑：global-hotkey 0.8 `Shortcut::new` 返回 Self 不是 Result（不能 `?`）；`with_handler` 的 setup 闭包引用 hotkey_mods 需加 `move`
### 本地 HTTP API（外部机器人接口，00:06-00:50）

- 老板需求：可选本地 HTTP 接口（默认关闭），Bearer token 鉴权，REST CRUD + SSE，不动现有业务代码，禁止 0.0.0.0 与文件遍历。**明确不写任何大模型调用/function-call 逻辑**
- 实现（`src-tauri/src/api.rs`，tiny_http 0.12 + uuid）：
  - 端口 4763，只绑 127.0.0.1；全部端点 Bearer 鉴权（401 否则）；CORS 全开（本地友好）
  - GET /api/tasks、GET /api/tasks/:id、POST /api/tasks（title 必填，status/note/filePath/fileIsDir/due/tags 可选，status∈todo/doing/done 校验）、PUT /api/tasks/:id（部分更新，进 done 记 completedAt/出 done 清除）、GET /api/events（SSE）
  - 对外 JSON 形状：db::Task 字段 + `status` 别名（=column）
  - 任务变更后：SSE 广播 tasks-changed + emit_to("main","tasks-updated") → 看板自动刷新（复用挂件→主窗口既有通道）
  - token 持久化到数据目录 api-token.txt（uuid v4），`load_or_create_token`；api_start/api_stop/api_status 三个命令；ApiState 托管（默认关闭）
  - 数据访问抽象 TaskStore trait：生产 TauriStore（走 db_load/db_upsert），单测 MemStore
  - **单测**（cargo test --lib api，2 个全过）：401/CRUD/400/404 + SSE 收到 tasks-changed
  - 前端：SettingsPage.tsx（开关 nm-outset/nm-inset 胶囊 + 运行状态 + token 只读框 + 复制按钮 + 接口清单），App.tsx 加「设置」视图与 header 按钮
- **坑**：tiny_http `Response::new` 流式 reader + `req.respond` 会缓冲到连接结束才 flush，SSE 长连首帧出不去 → 改用 `req.upgrade("text/event-stream", resp)` 拿原始 ReadWrite 流直接 write+flush（15s 心跳 `: keepalive`）；`Shortcut`-式教训：tiny_http respond 与 upgrade 不可混用
- 验证：cargo check/test、tsc、npm run build 全过；dev 实例已重建运行，老板在设置页开启后可用 curl 验证

### 本地 HTTP API 升级（07:24-07:40，老板定 P0/P1/P2 全做）

- **P0 功能补齐**：PUT 支持 due/tags/archived/deleted（空串清 due、空数组清 tags）；DELETE /api/tasks/:id 软删进回收站（幂等）；GET /api/tasks 过滤（?status=todo|doing|done、?trash=1、?archived=1、?all=1，默认活跃任务）；开关持久化（api-enabled.flag，重启自动恢复 API）
- **P1 安全**：去 CORS 头与 OPTIONS 预检；请求体 1MB 上限（413）；token 轮换（api_rotate_token 命令 + 设置页「重新生成」按钮）；限流 120 次/分（429）
- **P2 可靠性**：SSE 事件 id + `?since=` 断线重放（EventHub：原子自增 id + 1000 条环形历史）；操作日志 data_dir/api.log；DB_WRITE_LOCK 静态锁包住 db_upsert/db_delete/db_merge（API 线程与主窗口并发写安全）；GET /api/health 免鉴权健康检查
- 测试：cargo test --lib api 2 个全过（覆盖过滤、软删幂等、恢复、due/tags 更新、非法 status 400、health）

### 桌面文件自动迁移清理（09:48-10:15）

- 老板需求：任务完成满 7 天进入归档后，按用户规则表自动迁移其绑定文件（移动到归档目录并更新 filePath，附件链接不断）；规则表可增删改/上传/模版下载，不改代码；保护看板任务附件；delete 规则默认关闭；手动触发 + 定时轮询；异常记日志不崩溃；Win/Mac 兼容
- 实现（`src-tauri/src/migration.rs`）：
  - 规则模型 MigrationRule { id, enabled, keywords, action(move|delete), archiveDir }；规则表存数据目录 cleanup-rules.json（serde camelCase）
  - 引擎 run_migration：阶段一归档到期任务（done 满 7 天 → archived=true，主窗口关闭时也照常）；阶段二对 archived 任务按规则顺序匹配关键字（大小写不敏感）执行 move/delete；成功后 db_upsert 更新 filePath 并 emit tasks-updated（source:"migration"）
  - 保护：仅 archived=true 且未删除的任务参与；看板/回收站附件绝不触碰
  - 归档目录 {year} 占位符；相对路径基于桌面（app.path().desktop_dir()），绝对路径原样；同名冲突自动 `名 (n).ext`
  - move：rename 优先，失败（跨卷）退化 copy+remove；delete：remove_file/remove_dir_all
  - 轮询：spawn_polling 后台线程（启动 60s 后首跑，此后每 10 分钟）；RUNNING AtomicBool 防手动/轮询重入
  - 日志：数据目录 migration.log（[YYYY-MM-DD HH:MM:SS] 行）
  - 命令：migration_rules_load/save/import/template_save/run/status（import 用 dialog 选 JSON，template_save 用 dialog 存模版）
- 前端：MigrationPanel.tsx 设置页新卡片——规则表逐行编辑（启用/关键字/动作/归档目录/删除行）、添加/保存/下载模版/导入规则表/立即执行、上次执行摘要 + 日志尾部展示；切换为 delete 动作默认关闭并显示 danger 提示；types.ts 加 MigrationRule/MigrationReport；App.tsx 监听器 source!=="migration" 才回写落盘
- **踩坑**：MigrationRule 最初没加 `#[serde(rename_all = "camelCase")]`，导入的规则表 archiveDir 字段反序列化成空串 → 迁移把文件移到了桌面根目录（实测发现）；加 rename_all + 专门的 serde 测试防回归。教训：JSON 模型字段名与前端约定必须显式对齐并测序列化
- 验证：cargo test 9 个全过（关键字匹配/大小写/空关键字/冲突命名/年份展开/规则校验/serde camelCase）；tsc + cargo check 过；运行时实测 4 场景——归档任务命中 move 规则被正确移动且 filePath 更新、无命中不动、delete 规则禁用不动、看板任务命中规则也不动（保护逻辑生效）

### 工作区（静态链接收藏，12:27-12:40）

- 老板需求：工作区按钮放主窗口「归档」「回收站」之间、挂件「今日」「锁定」之间；UI 类似任务卡（标题 + 折叠），展开后增删链接，可连接文件/文件夹/网址
- 实现：
  - db.rs：新表 workspace_items（id/title/collapsed/links JSON/ord/updated_at）+ WorkspaceLink/WorkspaceItem 结构（serde camelCase）+ workspace_load/workspace_upsert/workspace_delete 命令（DB_WRITE_LOCK）
  - WorkspacePage.tsx：主窗口视图——卡片标题内联编辑、FoldToggle 折叠、链接行（🔗/📄/📁 图标 + label + target + 悬停删除）、添加链接三入口（网址输入 / 文件对话框 / 文件夹对话框）、新建工作区大长条；链接打开：url → openUrl，file/folder → openPath
  - 挂件：view 加 "workspace"，按钮在今日与锁定之间；工作区视图只读展示卡片 + 链接点击打开；workspace-changed 事件 + 5s 轮询同步；工作区视图隐藏「+ 新建任务」
  - 事件：主窗口编辑后 emit("workspace-changed")；挂件 listen 后重读
- 踩坑：挂件 JSX 条件渲染改坏结构（div 重复）→ tsc 报错，修正；App.tsx 漏 import WorkspacePage → tsc 报 Cannot find name，补 import
- 验证：cargo check + tsc 过；dev 实例自动重建（12:33）；SQLite 探针插入/查询/删除 workspace_items 正常

### 内置机器人聊天（21:18-21:52，第一步：开关 + 聊天窗口本体）

- 老板确认 m-message = WMessage；机器人方案：设置页开关 + 挂件下方聊天区（原挂件窗口 1/2 高度）
- 实现：
  - bot.rs：bot_get_enabled/bot_set_enabled（bot-enabled.flag 持久化，套路同 api-enabled.flag）；bot_get_config/bot_set_config（bot-config.json：baseUrl/apiKey/model，默认 DeepSeek）；bot_chat——OpenAI 兼容流式（reqwest 0.13 + futures-util），delta 经 bot-chat-delta 事件推挂件窗口，带 4 个工具（list/create/complete/delete_task）多轮循环（≤6），工具进程内直改 SQLite，改后广播 tasks-changed + tasks-updated(source:"bot")
  - App.tsx：tasks-updated 监听器 source!=="bot" 才回写（同 api/migration 处理，防异步回写覆盖）
  - WidgetApp.tsx：botOn 状态（挂载读 + bot-changed 事件）；展开高度 = PANEL_H + CHAT_H(280=1/2)；聊天区挂任务列表下方（border-t 分隔）
  - ChatPanel.tsx：消息列表（用户 nm-outset 右 / 助手 nm-inset 左）+ 流式增量追加 + Enter 发送 + 清空对话 + 空态引导
  - SettingsPage.tsx：「设置」卡片改「机器人设置」；开关 + 开启后显示大模型 API 配置（Base URL/API Key/模型 + 保存）；开关切换 emit bot-changed 同步挂件窗口高度
- 验证：cargo check ✓、tsc ✓、dev 实例自动重建（21:51:37）；视觉验证未做（Mac 锁屏，截屏只见壁纸）
- 待验证：老板解锁后——①设置页开关与 API 配置保存 ②挂件展开出现聊天区、高度 840（560+280）③配好 API Key 后发消息流式回复 ④「新建任务/完成某任务」工具调用生效
- 待办：聊天记录持久化（目前仅内存）、文档处理（Excel/Word/PPT/PDF）、Windows 打包

### API Key 迁入系统凭据存储（22:32-22:38，老板拍板：用凭据管理）

- 背景：老板问 key 安全性（是否会流到外网）→ 答复只发给配置的 Base URL；风险点：本地明文文件。老板选 keychain/凭据管理方案
- 实现：
  - Cargo.toml + keyring crate（features: apple-native-keyring-store；windows-native-keyring-store 默认启用）——macOS 钥匙串 / Windows 凭据管理器统一 API
  - bot.rs：KEYRING_SERVICE="wmessage-bot" / USER="api-key"；read/write/has/clear 四个辅助；BotConfig 去掉 apiKey 字段（保留 Option 仅迁移用）；bot_get_config 返回 BotConfigView{baseUrl,model,hasApiKey}（不回传 key 本体）；bot_set_config 新签名（config + api_key: Option，非空才写凭据存储，留空不动旧 key）；新增 bot_clear_api_key；bot_chat 从凭据存储读 key
  - migrate_legacy_key：App 启动（lib.rs setup）+ 设置页读配置时兜底调用——老 bot-config.json 明文 key 迁入凭据存储后从文件清除（凭据存储已有 key 时不覆盖）
  - SettingsPage：key 输入框不回显（hasApiKey 显示 ✓ + placeholder"输入新 Key 可覆盖"）；保存只传输入框内容（空=不动）；「清除已保存的 Key」按钮（confirm 后 bot_clear_api_key）
- 验证：cargo check ✓ tsc ✓；dev 重建（46626, 22:36:49）；实测迁移——重启后 bot-config.json 只剩 baseUrl/model（明文 key 已清），security find-generic-password -s wmessage-bot -a api-key 能读到 key ✓
- 注意：keyring 在 macOS 未签名 dev 构建首次访问钥匙串会弹授权框（正常）；Windows 侧凭据管理器行为待老板 Win 机实测

### 机器人工具扩展：编辑/子任务/绑定文件（22:45-22:50）

- 老板点单：编辑任务、子任务、绑定文件夹
- bot.rs 新增 4 个工具：
  - edit_task：按标题关键词匹配未完成任务，可改 newTitle/note/due/column/tags；空串清字段；列变更补完成语义（进 done 记 completedAt、出 done 清除，与主窗口一致）；无有效字段返回"没有可修改的字段"
  - add_subtask：给任务追加子任务（uuid id、done:false）
  - toggle_subtask：按子任务内容关键词匹配，翻转 done
  - bind_file：isDir=true/false 弹系统文件/文件夹选择框（spawn_blocking + blocking_pick_file/pick_folder，避免卡异步运行时），结果写 filePath/fileIsDir；取消返回"用户取消了选择"
  - 重构：find_task_by_keyword 辅助函数（大小写不敏感 contains）；execute_tool 改 async（bind_file 需要 await spawn_blocking）
  - SYSTEM_PROMPT + TOOLS JSON 同步更新（职责描述 + 8 工具 schema）
- 验证：cargo check ✓ tsc ✓ dev 重建（47308, 22:49:17）
- 待老板实测：编辑/子任务/绑定弹框流程（尤其 macOS 弹框与钥匙串授权体验）

### 挂件内选任务操作（22:59-23:05）

- 老板反馈：打标题费时，想在挂件里直接选任务操作机器人
- 交互设计：聊天区加「🎯 选任务」按钮进入选择模式 → 点任务卡标题切换选中（ring-2 高亮，不再打开主窗口）→ 选中任务以 📌 引用块显示在输入框上方（可单删）→ 输入指令发送 → 消息自动附 [已选任务] 块（id+标题）→ 发送后清空选择退出模式
- Rust 工具侧：complete/delete/edit/add_subtask/toggle_subtask/bind_file 新增 taskId 参数（精确匹配优先），resolve_task 统一定位（taskId → title 关键词）；list_tasks 输出带 id 供模型引用；SYSTEM_PROMPT 加规则：消息含 [已选任务] 时强制用 taskId
- 验证：cargo check ✓ tsc ✓ dev 重建（48070, 23:04:47）运行中
- 待老板实测：选任务模式交互（点击选中/取消、引用块、发送后清理）

### 选任务模式修复（23:08-23:11）

- Bug：选任务模式点不了任务卡——标题绑 onDoubleClick（双击才开主窗口），单击无选中反应
- 修复：SortableTaskCard 加 selectMode/onSelect props；selectMode 下根 div onClick 整卡选中 + cursor-pointer；onTitleClick 传 undefined（暂停双击跳主窗口）；卡内交互按钮均 stopPropagation 不受影响
- 验证：tsc ✓ dev 重建（49146, 23:11:06）；老板实测 ✓ 可批量选中一次标记多个任务完成

### 搜索任务卡工具 search_tasks（23:14-23:16）

- 老板需求：机器人加搜索任务卡功能
- bot.rs 新增第 9 个工具 search_tasks：
  - query 关键词匹配标题/备注/标签/子任务（大小写不敏感 contains）
  - status 参数：active=仅未完成（默认），all=含已完成
  - 结果按 order 排序，带 id（供后续 taskId 精确操作）、状态列、截止时间、标签
  - 无命中返回"没有找到匹配…"
- SYSTEM_PROMPT 加规则：用户想找任务时调用 search_tasks；工具 schema 同步
- 验证：cargo check ✓ dev 重建（49372, 23:15:51）运行中

### 搜索结果可点击跳任务（23:21-23:25）

- 老板反馈：机器人回复的搜索结果想手动点进去
- 实现：
  - bot.rs：TaskRef{id,title} + BotChatResult{text,taskRefs}；execute_tool 返回 (文本, Vec<TaskRef>)，全部 9 个工具在成功时带上涉及的任务引用（list/search 带全部命中）；bot_chat 收集后按 id 去重返回
  - ChatPanel.tsx：invoke 解析 taskRefs，助手消息气泡下方渲染 📌 任务按钮（truncate + title 提示）；点击 → emit edit-task + 聚焦主窗口（主窗口打开该任务编辑态）
- 验证：cargo check ✓ tsc ✓ dev 重建（50331, 23:24:35）运行中
- 待老板实测：搜索后点 📌 按钮是否跳主窗口打开任务

### search_tasks 改为只搜归档（23:31-23:33）

- 老板裁定：搜索工具只搜归档文档，不要搜待办/今日/完成（活跃任务看板挂件直接可见，无需机器人搜）
- 实现：
  - bot.rs：tool_search_tasks 过滤条件 t.archived == Some(true)（原来排除归档，现只搜归档）；删掉 status 参数（schema + 实现同步）；无命中提示改「归档里没有找到匹配…」；SYSTEM_PROMPT 规则 4 改：活跃任务用 list_tasks、归档任务用 search_tasks
  - App.tsx：edit-task 监听判断任务 archived → 跳归档页而非看板（机器人搜归档任务点 📌 按钮落点正确）
- 验证：cargo check ✓ tsc ✓ dev 重建（50916, 23:32:57）运行中

### search_tasks 改回搜索全部任务卡（23:37-23:39）

- 老板再裁定：所有任务卡都能搜索最好（推翻 23:31 的"只搜归档"）
- 实现：
  - bot.rs：tool_search_tasks 过滤改为仅排除回收站软删（deleted_at.is_none()），覆盖待办/进行中/已完成/已归档；结果行对已归档任务加「（已归档）」标记；无命中提示改回「没有找到匹配…」；SYSTEM_PROMPT 规则 4 与 TOOLS schema 描述同步（搜所有任务卡）
  - 保留：App.tsx 点 📌 跳转逻辑（归档→归档页，其余→看板）；resolve_task 仍只定位未完成任务（操作类工具边界不变）
- 验证：cargo check ✓ tsc ✓ dev 重建（51495, 23:38:13）运行中

### 机器人安全优化（23:44-23:50，参考老板发的《Harness 安全网关需求》）

- 老板发来《WMessage 最终完整版 AI 助手 + Function 工具 + Harness 安全网关完整架构需求》，要求参考并优化机器人安全
- 对照七层网关做差距分析后落地 5 项（不打断聊天体验的部分）：
  - 审计日志（第 7 层）：数据目录 bot.log——用户原始指令（截 300 字）、工具名、入参（截 500 字）、执行结果（截 300 字）全留痕；truncate_for_log 按字符数安全截断防刷日志
  - HTTP 超时熔断（第 5 层）：reqwest connect_timeout 15s + 总超时 300s，杜绝请求挂起卡死聊天
  - 参数上限校验（第 3 层）：MAX_TITLE 200 / MAX_NOTE 5000 / MAX_KEYWORD 100 / MAX_SUBTASK_TEXT 200 / MAX_DUE 30 / 标签 ≤10 个且每个 ≤30 字；接入 create/edit/search/add_subtask/toggle_subtask
  - 提示词安全红线（第 1 层）：禁系统命令/修改系统设置、禁批量删除、禁全盘遍历、禁编造路径、禁猜测 id
  - 模块文档注释更新：安全机制清单
  - 已有：工具白名单（第 2 层）、6 轮调用上限（第 5 层）、keyring 凭据存储
- 未落地（待老板定）：第 4 层风险分级弹窗确认（会打断聊天流）、第 6 层文件沙箱（等文档生成功能）、多会话/Markdown 渲染（功能扩展非安全项）
- 验证：cargo check ✓ tsc ✓ dev 重建（52214, 23:49:09）运行中

### 聊天记录持久化（23:53-23:58）

- 老板问：聊天记录持久化怎么做 → 直接实现（SQLite 全量覆盖方案）
- 实现：
  - db.rs：新表 bot_messages（id AUTOINCREMENT/role/content/refs/created_at）；BotMsgRow 结构（role/content/refsJson）；三个命令——bot_history_load（按 id 顺序读全部）、bot_history_save（DELETE 后全量 INSERT，事务式简单可靠）、bot_history_clear
  - lib.rs：注册三命令
  - ChatPanel.tsx：挂载时 bot_history_load 恢复（含 refs JSON 反序列化，📌 按钮也持久化）；send 成功/失败后 persistHistory 全量写入（失败消息 ⚠️ 也存）；「清空对话」按钮同步 bot_history_clear
- 踩坑：edit 工具给 execute_batch 的 SQL 串多塞了 `","` 导致 19 个编译错（prefix REAL/INTEGER unknown）——sed 看现场定位后 python 修复
- 验证：cargo check ✓ tsc ✓ dev 重建（53235, 23:57:03）运行中；sqlite3 探针确认 bot_messages 表已建
- 待老板实测：聊几句 → 重启 App（Cmd+Q 再开）→ 聊天记录应恢复；清空对话 → 记录删除

### 多会话管理（00:00-00:04，老板拍板"可以加"）

- 需求：聊天记录多会话（新建/切换/删除对话，仿主流聊天应用）
- 实现：
  - db.rs：bot_messages 加 session_id 列 + 新表 bot_sessions(id/title/created_at/updated_at)；迁移——老库补列，无 session_id 的孤儿消息自动归入「默认对话」会话
  - 命令：bot_sessions_load（updated_at 倒序）、bot_session_create（uuid id，缺省「新对话」）、bot_session_delete（会话+消息级联删）、bot_session_rename；bot_history_load/save/clear 全部改为按 session_id 维度（save 顺带刷会话活跃时间）
  - lib.rs 注册 4 个新命令
  - ChatPanel.tsx 重写：头部会话切换器（nm-outset 按钮 + 下拉菜单：会话列表/当前高亮/🗑删除/＋新建对话，点击外部关闭）；切换会话重载消息；清空按钮改 🧹（只清当前会话）；首轮发送后「新对话」自动改名用户消息前 20 字；busy 时锁定切换/新建/删除
- 验证：cargo check ✓ tsc ✓ dev 重建（53901, 00:03:30）运行中；sqlite3 探针确认 bot_sessions 表已建（会话由前端挂载时惰性创建）
- 待老板实测：①重启后多个会话都在、各自消息独立 ②切换/新建/删除流程 ③老单会话数据应出现在「默认对话」里

### 文档处理 + Python 编程（00:16-00:31，老板拍板：本机 Python 方案）

- 老板拍板：调用本机 Python；文档处理全走 Python；润色重点是 Word，Excel/PDF/PPT 不润色、参照主流功能（提取/生成）
- 新增 bot_py.rs（Python 执行基础设施 + 固定脚本模板）：
  - 环境：detect_python（macOS python3/python；Windows python/python3/py -3）；py_env_check 返回版本 + openpyxl/docx/pptx/pypdf 可用性
  - 沙箱执行 run_python：独立临时目录 py-runs/<uuid>/、run.py + params.json 传参（永不拼 shell）、默认 60s 超时强杀、stdout/stderr 各截 64KB、双线程读输出防死锁、执行完清临时目录
  - 开关：py-enabled.flag（默认关）；py_exec 未开拒绝执行
  - 审计：py_audit 写 bot.log（脚本摘要/耗时/退出码/输出摘要）
  - 固定脚本 5 个：EXTRACT（docx/xlsx/pptx/pdf 按扩展名提取文本，含表格/工作表/幻灯片结构）、MAKE_DOCX（黑体标题+宋体正文 12pt）、MAKE_XLSX（=开头单元格写原生公式）、MAKE_PDF（reportlab STSong-Light 中文字体+自动折行分页）、MAKE_PPTX（封面+标题要点页）
  - 命令：py_exec、doc_extract（弹框选文件或给定路径）、doc_make_word/excel/pdf/ppt；gen_out_path 输出 AI_Gen_Files/<文件名>，同名自动加 (n) 序号永不覆盖；文件名只取 basename 防路径穿越
- bot.rs：TOOLS schema 加 6 工具（extract_document/create_word/create_excel/create_ppt/create_pdf/run_python）；execute_tool 分发 + 6 个桥接函数（提取文本截 30000 字防爆上下文）；SYSTEM_PROMPT 加文档规则 8-12（先提取→Word 润色后 create_word 新文件不覆盖原文件→公式 Excel→Python 编程→生成文件只落 AI_Gen_Files）
- SettingsPage：机器人设置卡片加「允许机器人执行 Python」开关（默认关、开启 confirm）+「本机 Python 环境」检查按钮（版本+四库状态）
- lib.rs 注册 9 个 bot_py 命令
- 踩坑：ChildStdout/ChildStderr 不能放同一数组循环（类型不同）→ 分开两个 thread；child.stdout.take() 进闭包 partial move → 先 take 再传；edit 误删 refreshBot/loadConfig 定义 → 补回
- 验证：cargo check ✓ tsc ✓；5 个 Python 脚本实测全过（提取 docx 文本 ✓、生成 docx 段落校验 ✓、xlsx 公式 ✓、pdf ✓、pptx ✓）；dev 重建（55633, 00:30:07）运行中
- 本机环境：系统 Python 3.9.6 + openpyxl 3.1.5 + python-docx 1.2.0 + python-pptx 1.0.2 + pypdf 6.10.2 + reportlab 4.5.1 全齐
- 待老板实测：①设置页开关+环境检查 ②让机器人提取 Word 并润色生成新文件 ③Excel 公式 ④run_python

### Word 修订模式（07:02-07:20，老板指令「做修订模式」）

- 需求：Word 润色增加修订模式——生成带修订标记（track changes）的文档，删除内容标删除线、新增标红色下划线，可在 Word「审阅」里逐条接受/拒绝
- 实现：
  - bot_py.rs：新增固定脚本 MAKE_DOCX_REVISIONS_SCRIPT（约 130 行 Python）——原文优先从 originalPath 回读文件（与 EXTRACT 同逻辑：非空段落 + 表格行），提取被截断时回退模型传的 original 行（长度对比判断，保证对比范围一致）；difflib 段落级对齐（autojunk=False）+ 替换段落内字符级 diff；w:ins/w:del XML（author=WMessage AI、date=UTC、id 自增），删除 run 用 w:delText + strike + 红色，新增 run 用 w:u + 红色；新命令 doc_make_word_revisions
  - bot.rs：doc_extract 返回改为 {path, text}（工具结果带 [文档路径] 头）；TOOLS 加 create_word_revisions（originalPath/original/revised/title/filename）；execute_tool 分发 + tool_create_word_revisions；SYSTEM_PROMPT 规则 9 加修订模式分支（含截断时必传 original 的兜底规则）
- 踩坑（重要）：TOOLS JSON 昨晚最后一版有两个括号 bug——extract_document 少一个 `}`、create_excel 多一个 `}`（`},"description"` 提前闭合了 sheets 对象），serde_json::from_str(TOOLS).unwrap() 会直接 panic，bot 聊天整个不可用。逐条解析 + 括号事件追踪定位修复
- 验证：cargo check ✓；Python 脚本单测两条路径全过——①文件回读路径：3 段原文 vs 3 段修订，字符级 diff 正确（del「很好」/ins「晴朗」、ins「三点」「会议」），ins/del 带 author/date/id 序号；②截断回退路径：原文 5 段 + 模型只传 3 段 → 用模型行对比，不产生尾部假删除；接受修订后文本正确；dev 重建（61918, 07:18）运行中
- 待老板实测：让机器人「润色这个Word，用修订模式」→ 打开生成文件看删除线/下划线标记 → 审阅里接受/拒绝全部修订

### 联网工具：web_search + fetch_url（07:46-07:56，老板指令「加最后两个工具，按最优方案」）

- 老板确认机器人配的是 MiniMax 接口 → 搜索首选 MiniMax 自带 web_search 触发 + 客户端执行
- 协议实测（curl 直连 MiniMax M3）：
  - 模型返回 tool_calls（name=web_search, args={query}），**服务端不透明执行**——回传 query 原样无效，模型反复换词重试
  - 客户端执行搜索、把真实结果（标题+链接+摘要文本）作为 tool 消息回传 → 模型正常读取继续（实测读到摘要里的日期信息）
  - 结论：MiniMax 只负责"何时搜、搜什么"，搜索执行在客户端
- 搜索后端选型实测：DuckDuckGo（lite/html 两个端点）从本机连不通（HTTP 000）；cn.bing.com 200 可用且 `<li class="b_algo">` 结构清晰 → 用 Bing 抓取
- 实现：
  - 新模块 bot_web.rs（联网工具，约 300 行）：
    - web_search：Bing 抓取（q + mkt=zh-CN、UA 头、connect 15s/总 30s），解析 b_algo 块（h2>a 标题 + href + p 摘要），去标签 + 实体解码（含数字实体 &#NNN;），最多 8 条、输出截 6000 字
    - fetch_text：URL 校验（url crate，仅 http/https）、本机/内网拦截（IP 字面量 is_private/is_loopback/is_link_local/is_unique_local + localhost/.local/.internal/.lan/.home.arpa 后缀）、2MB 上限、Content-Type 只收 html/xml/text、GB18030/GB2312/GBK/Big5 嗅探解码（encoding_rs）、html2text 转纯文本
  - bot.rs：TOOLS 加 web_search（MiniMax type=web_search 格式）+ fetch_url；execute_tool 两臂 + tool_web_search/tool_fetch_url（fetch 结果截 30000 字）；SYSTEM_PROMPT 规则 13-15（最新信息先搜、给链接用 fetch、来源标注）+ 红线补 fetch_url 只公网
  - Cargo.toml：+html2text 0.13 + encoding_rs 0.8 + url 2（rsproxy 镜像源可用）
- 踩坑：html2text::from_read 返回 Result 不是 String；测试里 futures-util 无 block_on → 用 tauri::async_runtime::block_on；数字实体解码漏吃分号（consumed 未含 ;）→ 修
- 验证：cargo check ✓；单元测试 4 个全过（Bing 解析真实 fixture / 内网拦截清单 / 实体解码 / 真实网络搜索+抓取——SEARCH OK 8 条结果带链接、FETCH OK example.com 175 字、127.0.0.1 和 file:// 被拒）✓；TOOLS 18 工具 JSON 全解析 ✓；dev 重建（64297, 07:55）运行中
- 待老板实测：问机器人实时问题（如"今天北京天气"）→ 应触发搜索并回答带链接；发个链接让总结 → fetch 正文

### 搜索双引擎：Bing + 百度（07:58-08:04，老板指令「再加一个搜索引擎百度」）

- 实测百度可抓（www.baidu.com/s 200、无验证页、22 个 result 容器）
- 百度链接是加密的 /link?url（经典替换表已失效、AES CBC/ECB 末 32 字符 key/iv 方案对齐不上）→ 链接原样给出（浏览器可打开跳转），标题+摘要照常
- 实现：web_search 改双引擎——futures_util::future::join 并行跑 Bing + 百度，按标题去重合并，最多 8 条，单引擎失败不影响另一个（全失败才报错）
- parse_baidu：逐 h3 找 baidu.com/link 标题 + extract_baidu_snippet（h3 后第一个 ≥12 字且无 JSON 垃圾的 span，截 200 字）
- 踩坑：切片 h3end+6000 字节可能切在汉字中间 → is_char_boundary 修
- 验证：单测 5 个全过（新增 parse_baidu_fixture：10 条 Bing + 5 条百度真实结果，摘要 48-200 字）✓；dev 重建（65300, 08:04）
- 待老板实测：中文问题搜索，结果里应混有百度来源；百度跳转链接浏览器可开

### 聊天体验三改（08:13-08:20，老板指令：流式回复效果不好）

- 老板三点：①思考过程做成下拉/折叠 ②工具调用折叠、只发结果 ③结果里的网页/文件给可点链接
- 实测 MiniMax M3 流式：reasoning 以 `<think>…</think>` 标签混在 content 里流式下发（无独立 reasoning 字段）
- 后端 bot.rs：
  - feed_think + tail_prefix_len：`<think>` 标签拆分状态机（标签跨流式块时缓冲前缀），思考 → bot-think-delta 事件，正文 → bot-chat-delta；回合结束冲刷残留并丢弃未闭合标签碎片
  - 工具事件三连：bot-tool（新调用）、bot-tool-name（名字补全）、bot-tool-done（执行完带 args）
- 前端 ChatPanel.tsx：
  - Fold 组件（▸/▾ 折叠块）：💭 思考过程默认收起；🔧 工具行默认收起（执行完标 ✓、展开看入参截 500 字）
  - RichText：正文渲染 http(s) 链接（openUrl）和绝对文件路径（/Users /home /Library 等常见前缀 + Windows 盘符路径，openPath）为可点链接，尾部标点裁剪
  - streamingMeta ref 存流式装饰（思考/工具行），bot_chat 完成后并入最终消息；历史持久化仍只存正文
- 踩坑：tail_prefix_len 比较方向写反（尾部反向对标签正向）→ 改 tag.starts_with(&s[len-k..]) + is_char_boundary 防切汉字；单测暴露
- 验证：cargo check ✓ tsc ✓ think 单测 4 个全过（标签跨块/整块/无标签/多块）✓；dev 重建（66061, 08:19）
- 待老板实测：问个需要思考+工具的问题（如「新建任务：买菜」），看 💭 和 🔧 折叠行；再问实时问题验证链接可点

### 折叠行持久化（08:27-08:28，老板指令：折叠行要留在对话框里，默认收起可点开）

- 老板：思考/工具折叠行要一直留在对话框（换会话、重启后还在），默认折叠、点开可看
- 实现：
  - db.rs：bot_messages 加 thinking/tools 两列（CREATE TABLE + PRAGMA table_info 迁移，沿用 session_id 模式）；BotMsgRow 加 thinking/toolsJson；bot_history_load/save 读写新列
  - ChatPanel.tsx：persistHistory 带 thinking/toolsJson；rowsToMsgs 统一回填（挂载/切会话/删会话三处）；流式监听里 streamingMeta ref 的副作用移出 setMessages updater（StrictMode 下 updater 跑两遍会把思考文本重复累积进 ref——最终消息思考会翻倍）
- 验证：cargo check ✓ tsc ✓；dev 重建（66956, 08:28）
- 待老板实测：问带思考+工具的问题 → 换会话再换回来 / 重启 App → 💭 和 🔧 折叠行还在，点开能看

### 任务卡交给机器人执行（08:47-08:55，老板批准两阶段方案，先落地阶段一）

- 老板拍板：「按照你的意思做」→ A（卡片按钮一键发起）+ B（🎯选卡+聊天说「完成它」）都做，同一执行循环
- 后端 bot.rs：
  - 大重构：bot_chat 的工具循环抽出 run_model_loop(app, msgs, max_rounds)——配置/Key 检查、流式（思考拆分+工具折叠事件）、进程内工具执行全部共用；bot_chat 轮数 6→8
  - 新命令 bot_execute_task(taskId)：任务卡（标题/备注/子任务/截止/绑定文件）组装成 [任务卡执行] 指令块 + EXECUTE_SYSTEM_PROMPT（先读卡→工具执行→link_file_to_task 绑产物→edit_task 写执行摘要→complete_task；线下事务诚实拒绝不标完成），10 轮工具循环；已完成/已归档卡片拒绝执行
  - TOOLS +1（19 个）：link_file_to_task（taskId+path，路径必须真实存在防编造）；extract_document 加可选 path 参数（直读绑定文件，不再只能弹框）
  - SYSTEM_PROMPT 加规则 16：聊天说「完成/执行」带 [已选任务] 引用块 → 同一执行语义
- 前端：
  - TodoCard（主窗口）+ TaskCardContent（挂件）：🤖 交给机器人按钮（截止时间上方，与两处展示一致）
  - 主窗口按钮：emit execute-task 事件 + 唤起挂件窗口；挂件按钮：botOn 时 emit 同一事件（bot 关闭时隐藏）
  - ChatPanel：监听 execute-task → executeTask(taskId)（busyRef 防重入、executeTaskRef 防旧闭包）→ 聊天区显示「🤖 执行任务卡：标题」→ bot_execute_task 流式执行 → 结果消息含 💭/🔧 折叠行 + 📌 任务引用，持久化同普通消息；首轮后会话改名为任务名
- 验证：cargo check ✓ tsc ✓ think 单测 4/4 ✓ TOOLS 19 工具 JSON 合法 ✓；dev 重建（68163, 08:54）
- 待老板实测：①新建一张能机器完成的任务卡（如「搜索一下XX并整理要点」）→ 点 🤖 看全流程 ②🎯 选卡 + 说「完成它」 ③线下任务卡（如「取快递」）→ 应诚实拒绝不标完成 ④产物文件应绑回卡片

### 聊天附件交互（09:06-09:07，老板指令：弹框选文件交互不好，改 ➕ 预添加和消息一起发）

- 老板流程：➕ 添加文件 → 聊天窗口写「润色」→ 一起发送
- 实现（前端为主）：
  - ChatPanel：输入行左侧 ➕ 按钮 → dialog 选文件（multiple，挂件窗口已有 dialog 权限）；已选文件显示为 📎 芯片行（basename + ×移除）
  - send()：附件组装成 [附件文件] 块附在消息前（与 [已选任务] 同模式），发送后清空附件
  - 历史消息：splitAttachments 从内容解析附件块 → UserBubbleContent 渲染 📎 芯片 + 正文（重启/切会话后芯片仍在）
  - 占位提示随附件变化（「输入指令，如：润色这个文件」）；只加附件没文字也可发送
- 后端：SYSTEM_PROMPT 加规则 17——消息带 [附件文件] 块时用 extract_document 的 path 参数直读，不再弹系统选择框
- 验证：cargo check ✓ tsc ✓；dev 重建（68751, 09:07）
- 待老板实测：➕ 加 Word → 输入「润色」→ 发送 → 机器人直读文件润色（不弹框）

### 修订模式链接打不开修复（09:19-09:27，老板反馈：修订完链接打不开、默认程序打不开）

- 排查（两个真凶叠加）：
  1. **老板机器只有 Pages 没有 Word**（/Applications 只有 Pages.app），Pages 打不开 Word track changes 的 docx——原修订模式用 w:ins/w:del 生成，Pages 必然失败；聊天记录里机器人自己也诊断过「Pages 打不开修订标记」
  2. **RichText 正则吞 markdown 反引号**：机器人消息里路径包在反引号里（`/path/docx`），正则字符类没排除反引号 → 链接点出去的是带尾反引号的路径 → openPath 静默失败
- 修复：
  - bot_py.rs MAKE_DOCX_REVISIONS_SCRIPT 重写：track changes（w:ins/w:del）→ **可见修订格式**（删除=红色+删除线、新增=红色+下划线，普通 run；文档开头灰色说明行）——Pages/WPS/Word 通用
  - ChatPanel RichText：路径/URL 字符类排除反引号和 *；openPath 失败兜底 revealItemInDir（Finder 定位，不再静默）
  - bot.rs 文案：工具描述和结果消息去掉「Word 审阅逐条接受/拒绝」，改为「红色删除线=删除、红色下划线=新增，Pages/WPS/Word 通用」
- 验证：新脚本单测（w:ins/w:del 计数 0、strike ×3、下划线 ×3、textutil 正常解析）✓；cargo check ✓ tsc ✓；dev 重建（70090, 09:27）
- 待老板实测：重新用修订模式润色 → 点链接 → Pages 直接打开可见修订对照版

### 修订模式改回 Word 原生 track changes（09:32-09:33，老板裁定：Pages 能打开 Word 修订模式）

- 老板拍板：所有润色/修改都用 Word 原生修订模式（w:ins/w:del，author=WMessage AI），撤销 09:27 的「可见修订」格式
- 恢复：MAKE_DOCX_REVISIONS_SCRIPT 回退 track changes 版本（w:ins/w:del + w:delText + strike/underline 显示 + 自增 id + author/date）；工具描述和结果文案回退「可在 Word 审阅里逐条接受/拒绝」
- 提示词升级：SYSTEM_PROMPT 规则 9 —— Word 润色**默认**用 create_word_revisions 修订模式，用户明确要纯文本版才用 create_word；EXECUTE_SYSTEM_PROMPT 规则 2 同步（任务执行里 Word 修改也走修订模式）
- 保留 09:27 的独立修复：RichText 正则排除反引号（链接吞尾反引号导致点不开的真凶）+ openPath 失败兜底 revealItemInDir
- 验证：脚本单测（w:ins ×3 / w:del ×3 / author=WMessage AI / textutil 解析）✓ cargo check ✓；dev 重建（70711, 09:33）
- 待老板实测：修订模式润色 → 点链接 → Pages 打开 track changes 修订

### 图片附件多模态（09:38-09:41，老板问题：MiniMax 能读图，为什么发的图片提取不出文字）

- 原因：机器人只把图片路径（[附件文件] 块）发给了模型，MiniMax 收到的是路径不是图片本体，自然读不了
- 实现（纯后端，前端无改动）：
  - bot.rs attach_images：解析 [附件文件] 块，图片扩展名（png/jpg/jpeg/webp/gif/bmp）读文件转 base64 data URL，消息 content 变成多模态数组 [text, image_url×N]；无图片时保持纯文本字符串
  - 限制：单张 ≤3MB（base64 后约 4MB）、每条消息最多 4 张、文件不存在/过大静默跳过
  - 范围：最近两条 user 消息的图片附加（追问「再仔细点」时上一张图还在上下文里），更早历史保持纯文本省 token
  - SYSTEM_PROMPT 规则 17 更新：图片附件直接出现在消息里，用视觉能力读取，不要用 extract_document 处理图片；文档附件才走 extract_document
- 新依赖 base64 0.22（rsproxy 镜像）
- 验证：单测 3 个全过（图片→data URL / 非图片→纯文本 / 不存在→跳过）✓；dev 重建（71057, 09:41）
- 待老板实测：➕ 发一张带文字的图片 → 说「提取图片里的文字」→ 应直接读出文字

### 收官两件套：删除确认 + 审计日志入口（09:49-09:51，老板拍板「先做1和3」）

- ① 删除任务弹确认（安全网关第 4 层，只对删除）：
  - bot.rs：CONFIRMS 静态表 + tokio oneshot 通道；ask_user_confirm 发 bot-confirm 事件给挂件并等待，**60s 超时默认拒绝**（安全兜底）；bot_confirm_response 命令回填；tool_delete_task 改 async 先确认后删除（顺手升级为 resolve_task 支持 taskId）；审计日志记录 confirm 请求
  - ChatPanel：bot-confirm 监听 → 挂件内弹窗（⚠️ 机器人要删除任务「XXX」+ 允许/拒绝 + 60 秒自动拒绝提示）
- ③ 审计日志查看入口：
  - bot.rs：bot_log_read 命令（倒序最新在前，默认 200 行、上限 2000）
  - SettingsPage：机器人设置卡加「查看日志」按钮 + 弹窗（pre 滚动显示、刷新/关闭）
- 新依赖 tokio（sync+time，oneshot + timeout；tauri 本就带 tokio 无额外成本）
- 验证：cargo check ✓ tsc ✓ 26 个单测全过 ✓；dev 重建（71717, 09:51）
- 待老板实测：①聊天说「删除任务XXX」→ 挂件弹确认 → 允许/拒绝/不理会（60s 自动拒）②设置页「查看日志」看审计记录

### 定时任务卡（阶段二）（09:58-10:07，老板指令：卡片加「⏰ 定时执行」，到点自动跑）

- 数据：tasks 表加 schedule/sched_last 两列（PRAGMA 迁移）；Task 结构加字段（field-level serde default，老前端数据兼容）；upsert/load/load_external 同步
- 定时格式：daily:HH:MM（每天）/ weekly:D:HH:MM（D=1..7 周一起）/ at:YYYY-MM-DDTHH:MM（一次性，执行完自动清除）
- 后端 bot.rs：
  - bot_execute_task 拆出 execute_task_core（命令与调度共用）
  - start_scheduler：30s tick 扫描到点任务（未删/未归档/未完成，sched_last < 触发点 ≤ now，漏执行会补跑一次）
  - run_scheduled：SCHED_RUNNING 防重入 → 先记 sched_last 防 30s 内重复触发 → 执行核心 → 结果前置「⏰ 自动执行 HH:mm」写进备注（失败也记）；一次性执行完清 schedule；审计日志 sched_run/sched_done
  - occurrence_after 纯函数 + 单测 5 个（daily/weekly/at/非法格式，2026-08-16 是周日基准）
- lib.rs：setup 里 start_scheduler 启动
- 前端：TodoCard（主窗口）+ TaskCardContent（挂件）🤖 旁加 ⏰ 按钮；已定时时显示徽标文案（formatSchedule：每天 09:00 / 每周一 09:00 / 08-17 10:00 一次）；点击展开面板：4 个预设 + 自定义 datetime-local 一次性 + 取消定时；挂件经 onSetSchedule 回调 applyAndSync
- 编译踩坑：容器级 #[serde(default)] 要求 Task: Default（改用字段级）；DateTime.weekday() 要 use chrono::Datelike；MIN_UTC 类型歧义（改 epoch from_timestamp_millis(0)）
- 验证：cargo check ✓ tsc ✓ 30 单测全过（含 sched 5 个）✓；dev 重建（73249, 10:07）
- 待老板实测：给一张卡设「每天 09:00」（或自定义一分钟后的定时一次）→ 到点自动执行 → 备注出现「⏰ 自动执行」记录

### 任务卡归属头像 + 个人资料（10:15-11:05，老板多轮确认规则）

- 规则：人完成 → 用户头像；点「交给机器人」→ 机器人头像（执行结束**无论成败**改回用户头像）；机器人也可上传头像+改名；任务卡只显头像、悬停 tooltip 显姓名
- 模型：一个 `bot_assigned` 布尔（不设 completed_by）；tasks 表 +bot_assigned 列（ALTER 迁移）；execute_task_core 进出 set_bot_assigned（手动 🤖 与 ⏰ 定时共用）；open_db 启动 Once 清残留
- profile.rs 新模块：profile.json 存数据目录、头像文件拷 profile/ 子目录；头像读返回 base64 data URL（绕开 asset protocol/CSP）；profile-changed 事件广播双窗口
- 前端：profile.ts 单例缓存+订阅；ActorAvatar 共享组件（bot 默认 main-logo、用户默认首字圆形）；TodoCard + TaskCardContent 标题行末尾；人完成清 botAssigned；SettingsPage「个人资料」卡片（ProfileRow 用户/机器人两行）
- 验证：cargo check + tsc + vite build + 30 单测；dev 重建 76198

### 聊天斜杠命令四件套（11:20-11:35）

- /stop：StopGuard 全局注册表 + bot_stop 命令；run_model_loop 轮次顶部/流式每 chunk/工具循环前检查，置位提前返回「⏹ 已停止」
- /compact：bot_compact 命令（单次非流式、无工具、60s 超时、≤300 字摘要），历史替换为「📦 上下文已压缩」单条并持久化
- /retry：找最后一条 user 消息，砍掉其后内容重跑；send 核心抽成 runChat(history, renameText) 共用
- /copy：最后一条非空助手回复写剪贴板 + 1.5s「已复制 ✓」（后续按老板要求删除）
- /help：列出全部命令
- 验证：cargo check + 30 单测 + npm build；dev 重建 78025

### 助手回复 Markdown 渲染 + 逐条复制（11:50-12:00）

- react-markdown v10 + remark-gfm + remark-breaks（单换行断行）；MarkdownText.tsx 自定义 a 保留原点击行为（URL→openUrl、绝对路径→openPath 失败 revealItemInDir）；react-markdown 默认转义原始 HTML
- ChatPanel：流式中 RichText（增量纯文本），完成态切 MarkdownText；每条完成回复下 📋 复制按钮 + 🗑 移除按钮
- main.css：md-body 全套样式（段落/标题/列表/行内代码/代码块/引用/GFM 表格/分隔线/任务复选框），主题变量深浅色自适应
- bundle 351→511KB（桌面应用无碍）；dev 重建 79217

### 机器人模块审计两轮清零（13:20-14:00，老板拍板「全修」）

- **第一轮 14 项**（P0×2 / P1×2 / P2×4 / P3×6）：
  - P0-1 一次性定时结果被回滚：清理 schedule 用旧快照 upsert 把执行期间修改整体回滚 → db_load fresh 合并只改目标字段
  - P0-2 API 响应 choices[0] 索引 panic：安全访问 + 流式坏行跳过；连锁修调度器每张卡 spawn 隔离（单卡 panic 不杀调度器）
  - P1 run_python 假沙箱文档诚实化 + kill_tree（Unix 进程组 -PGID / Windows taskkill /T）+ 管道读 take() 硬截断
  - P1 SSRF 三洞（bot_web.rs）：DNS 解析校验（防重绑定）、整数/十六进制/八进制 IPv4 字面量识别、重定向逐跳校验（5 跳上限）
  - P2 TOOLS 单测当场抓到真 bug（web_search 工具定义格式错误，模型侧该工具一直是坏的）+ StopGuard 实例隔离（/stop 不影响后台定时）+ 前端双发 busyRef + 删除确认挂件不可见直接拒绝
  - P3 六项：锁毒恢复、链式 unwrap 消除、api.rs update_task 长度校验、http_client 不退化无超时、bot_compact 20 万字符上限、bot.log/api.log 5MB 轮转
- **第二轮 7 项**（P1×2 / P2×2 / P3×3）：
  - P1 定时 panic 后 sched_running 永久残留 → SchedGuard RAII（Drop 清理）
  - P1 extract_document 可读任意文件 → extract_path_allowed 白名单（任务卡绑定文件 / AI_Gen_Files 目录，canonicalize 防 ../）；create_word_revisions 的 originalPath 同规则
  - P2 机器人开关只管 UI 不管后端 → bot_chat/execute_task_core/run_scheduled 三入口都查 bot_get_enabled（定时跳过时 audit 留痕 sched_skip）
  - P2 api.rs create_task 长度校验补齐（与 update/bot 侧三路对齐）
  - P3：/compact 进度占位、profile 换头像 save 失败回删孤儿文件、executeTask 复用 runChat（execTaskId 参数化，删 80% 重复）、http_client OnceLock 全局复用连接池、sessionIdRef 收尾竞态防护
- 验证：cargo check + 31 单测 + npm build；dev 重建 84239
- 已知边界：run_python 本质仍是用户权限执行，真隔离需 OS 级沙箱

### 设置页「检查环境」无反馈修复（14:30-14:36）

- 根因：py_env_check 是同步 tauri 命令，主线程跑 5 次 python 子进程冻结 UI 几秒；前端 catch 只 console.error 毫无反馈
- 修复：py_env_check 改 async + spawn_blocking；库检测合并单进程一次探测全部 5 库（补上漏掉的 reportlab）；py_exec 拆 py_exec_sync（工具链直调）+ async 命令走 spawn_blocking；前端加 pyEnvErr 可见错误文案
- dev 重建 86605

### 定时面板改造：datetime-local 直输 + 四档周期（15:28-15:32）

- 后端：schedule 新增 `monthly:DD:HH:MM`（当月无该日如 2 月 31 顺延）；occurrence_after 加 monthly 分支（逐月扫描 ≤12）；单测 +2
- 前端（TodoCard + TaskCardContent 两面板同步）：删预设按钮，改 datetime-local 整行输入 + 四档「定时一次/每天/每周/每月」+ 取消；打开回填（at→原值、daily→今天、weekly→最近目标星期、monthly→本月/下月该日）；format.ts 新增 scheduleToDatetime 回填 + formatSchedule monthly 显示
- 验证：32 单测 + npm build；dev 重建 91428

### 定时面板 NaN 链死循环事故（15:36-15:45，老板实测「点每月卡死 App」）

- 数据库实锤坏数据 `monthly:Na:TNaN:`，根因链四层：手动输入不完整 datetime → Invalid Date → weekly 写 `weekly:NaN:...` → 回填 NaN 字符串 → monthly 写坏 → `while (date.getDate() !== NaN)` 死循环卡死
- 四层防御：①面板点档位前校验日期有效性（源头堵死）②scheduleToDatetime 全分支校验+回退今天 09:00+monthly 顺延 for 上限 12 次 ③formatSchedule 坏数据原样返回不展开 ④SQL 清理已有坏数据
- 教训（铁律）：**datetime-local 值不可直接信任；while 循环必须带上限；写库前必须校验日期有效性**
- 验证：32 单测 + npm build；dev 重建 92414

### 定时健壮性全修（15:58-16:00）+ 快捷命令全修（16:16-16:18）

- 定时 3 项：P1 错过的一次性任务不再补执行（at_expired：从未执行且已过期 → 放弃清 schedule）；P2 执行期间用户改定时不被误清（收尾用执行后 fresh.schedule 判断）；P3 记 sched_last 失败放弃执行（防 30s 重复触发）
- 快捷命令 4 项：P2 enterBusy/exitBusy 同步镜像 busyRef（同一帧连按两次 /compact 并发）；P3 三个静默场景加 addHint 本地提示（不持久化不污染上下文）；/help 快照统一；switchSession 用 busyRef
- 33 单测全过；dev 重建 94175、95587

### 桌面清理改造：下载模板卡死修复 + 只读规则表（16:22-16:25）

- **卡死根因**：migration_rules_template_save 是同步命令，主线程调 blocking_save_file() → NSSavePanel 无法弹出死锁。改 async + spawn_blocking（migration_rules_import 同修）
- UI：MigrationPanel 删全部行内编辑，规则表改只读展示；操作区仅「⬇ 下载规则模版」「⬆ 导入规则表」
- 教训：**Tauri 主线程绝不能跑 blocking 对话框**（弹框类命令一律 async + spawn_blocking）
- dev 重建 96166

### 桌面清理：CSV 表格模版 + 迁移日志弹窗（16:32-16:37）

- CSV 模版（老板要求编程小白会用）：csv crate；四列表头「启用/文件名关键字/动作/归档目录」+ 两行示例；UTF-8 BOM（Excel/WPS 双击中文不乱码）
- CSV 导入：宽容表头匹配、关键字中英文逗号/顿号/分号分隔、启用识别 是/true/1、动作识别 移动归档/删除文件 + move/delete；编码 UTF-8 → GBK 兜底；旧 JSON 兼容
- 迁移日志弹窗：migration_log_read 命令（尾部行最新在前，与 bot_log_read 同模式）；📋 查看迁移日志按钮 + 70vh 弹窗
- dev 重建 97119

### 桌面清理审计全修（16:46-16:49）+ 全面审计 11 项（17:12-17:25）+ 最严格审计 9 项（17:30-17:43）

- 桌面清理 10 项：RUNNING 改 MigrationGuard RAII（panic 不锁死）；轮询线程 catch_unwind；migration.log 5MB 轮转；load_rules 坏 JSON 记日志；阶段一归档不参与本轮迁移补注释；CSV/面板标注「规则顺序即优先级」；源文件消失 → 解绑附件不再每轮记 skip 噪音；CSV 编码链补 UTF-16 LE/BE；删死命令 migration_rules_save 与 MigrationStatus 死字段
- 全面审计 11 项（我亲自读全部 13000+ 行）：P1 WorkspacePage 不监听 workspace-changed（挂件改工作区主窗口不刷新）、db.rs load_external 丢新字段 → 动态探测列；P2 formatSchedule at: 分钟丢失、ChatPanel busyRef 统一、挂件工作区直写库改 workspace-updated 上报主窗口代理落盘（单写者架构）；P3 greet 死命令/TrashPage 空壳 DndContext/api 日志时间戳可读/due trim/theme 监听清理/monthly 范围校验
- 最严格审计 9 项（4 类交叉核对：命令注册 × invoke、事件 emit × listen、unwrap 全量、边界精读）：P2 快捷键 `shortcut.register(...)?` 改容错（键位被占不再让 App 启动失败）；P3 bot-chat-done 死事件删、死注册 6 命令删（py_exec/doc_*，工具链走函数直调）、mergeDb 死函数删、Header unwrap 改 expect、sched_last_dt 防御、find_due_tasks O(n) 开库改批量、percent_decode + → 空格、SSE 客户端锁中毒 into_inner 恢复
- 累计四轮审计 42 项全部清零；33 单测 + cargo check 无警告 + npm build；dev 重建 98166、1648、3317

### PPT 技能移植（18:36-18:41，老板：PPT 做得不好，发挥大模型能力）

- 背景：OpenClaw 的 MiniMax 文档技能因架构冲突否决，知识可移植；机器人 PPT 原是固定脚本（封面+标题+要点）
- **双移植**：①提示词技能——SYSTEM_PROMPT 规则 10 重写（大纲先行/每页一个观点/标题即结论/bullets 精炼/数据用表格/主题按场合/页数宁少勿多）；②脚本版式引擎——MAKE_PPTX_SCRIPT 升级为六版式（cover/toc/section/content/table/closing）+ 三主题（blue 商务蓝/dark 深色/green 清新绿），16:9 手工画布（色块+文本框+页码）、bullets 自适应字号（≤5 条 20pt、6-8 条 16pt、8+ 自动双栏）、表格页表头加粗底色
- create_ppt 工具 schema 升级（slide.type + subtitle/items/rows + theme）；doc_make_ppt 加 theme 参数
- 验证：脚本本地 + 嵌入 Rust 后提取端到端两轮测试；cargo check + 33 单测 + npm build；dev 重建 12181
- 踩坑：Python runs 解包（str 列表 vs 元组）、封面 continue 逻辑丢封面（重写主流程）、提取脚本切片偏移

### PPT skill 文档完整移植（18:51-18:53，老板点醒：文档作为提示词可行）

- 结论修正：此前否决 MiniMax PPT skill 只针对 SKILL.md 执行机制（agent 指令 + .NET 依赖），文档内容作提示词完全可行
- 通读 `~/.openclaw/workspace/skills/ppt-orchestra-skill/SKILL.md` 原文：SYSTEM_PROMPT 规则 10 重写为排版手册（版式定位/内容页子类型/数据表格化/主题按场合/生成后自查循环）
- 引擎去 AI 痕迹：删标题下装饰横线（skill Avoid 清单点名）、字号对齐规范（内容页标题 32pt、封面 48pt）
- 验证：端到端 5 页 + cargo check + npm build；dev 重建 13192
- 教训：否机制 ≠ 否内容，skill 知识要读原文移植

### 技能批量移植（19:04-19:07）— 12 个 OpenClaw 技能评估，4 个知识移植

- 评估 12 个技能：移植 4 个知识、否决 8 个（机制冲突/能力重复/需外部 API）
- **已移植**：
  - color-font-skill：18 套配色精选 10 套进 PPT 引擎（blue/navy/teal/forest/wine/sky/plum/coral/dark/green），theme 参数 + TOOLS schema 同步，提示词加「按场合选色」表
  - slide-making-skill：排版纪律（正文不粗体、配色只用所选主题、无渐变）
  - minimax-xlsx：派生值必须写公式不硬编码
  - minimax-docx：Word 按文档类型排版（公文/提案/首行缩进）
- **否决**：OpenXML/.NET 与 XML 直改机制（架构冲突，仅取知识）、vision-analysis（已有多模态读图）、gif-sticker-maker 与 music 系列（需外部生成 API + ffmpeg）、mmx CLI（已直连 MiniMax API）、design-style-skill（面向组件化 PptxGenJS，文字引擎用不上）
- 验证：cargo check + 33 单测 + forest 主题端到端 + npm build；dev 重建 13932

### 技能移植变动审计（19:19-19:23）— 9 项发现全修

- 审计对象：PPT 脚本引擎 + THEMES 10 套配色 + 提示词排版手册（三批移植的产物）
- **P2 对比度/正确性**：navy 深底主题斑马纹硬编码浅灰看不清 → THEMES 加 alt 键；table 表头/section 背景/toc 标题条从 accent 改 band（navy 的 accent 是黄色，黄底白字）；render_cover 此前读 slides[0]，cover 非首位时封面标题错 → 传参
- **P2 提示词与引擎不一致**：提示词承诺 Word 首行缩进但脚本没做 → MAKE_DOCX 补 Pt(24) 缩进；提示词「两个 bullet 组」引擎不支持分组 → 措辞改分条目
- **P3**：textbox runs 非 str 防御；lxml import 死代码清理
- 端到端回归（navy+cover 非首位+数字 bullet+表格）6 页全对；dev 重建 14774
- 教训：提示词与引擎能力必须对齐；深色主题配色对比度逐处验证

### 机器人技能系统（19:39-19:45）— 轻量版 Agent Skills，技能可安装

- 需求：WMessage 机器人能力此前全部硬编码，无法安装技能。调研后照 Anthropic Agent Skills 规范实现轻量版
- bot_skills.rs：技能 = 数据目录 skills/<name>/SKILL.md（frontmatter + 正文）；progressive disclosure——提示词只注入「名称+描述」清单，新工具 use_skill 按需读全文（50KB 上限）；技能名白名单防路径穿越；import 递归拷贝跳符号链接、重名拒绝
- SettingsPage「机器人技能」卡片：导入文件夹 / 删除 / 打开目录 / 列表
- 已把 OpenClaw 的 12 个技能导入 WMessage 数据目录（PPT/文档/配色/视觉分析系列）
- 验证：cargo check + 37 单测 + npm build；dev 重建 16491

### Skill 调度器实现（20:53-20:57）— 运行模型 v1.0

- 设计文档 docs/SKILL-RUNTIME.md（生命周期+状态机+流水线+两种模式+回滚+禁止项+示例）
- 实现范式：调度器监督 + 模型执行——use_skill 即启动（预审→Running），execute_tool 每步过 skill_on_step（计数/超步熔断/超时熔断/暂停拒绝/动作记录），run_model_loop 六出口接 skill_finish，/stop 联动 skill_terminate_all
- 元数据字段全落：risk_level/mode/max_steps/timeout_secs/rollback/enabled/intents（clamp + 非法回退 + high 强制 interactive）；preflight 黑名单拒绝
- 审计：skill_start/skill_failed/skill_completed/skill_terminated
- 单测 +11 → 48 全过；dev 重建 22894

### Skill 调度器优化（21:08-21:11）— 3 个 gap 全修

- resumable 字段全链落地（解析/镜像/审计，默认 false）
- Paused 状态真实入口：ask_user_confirm 联动 skill_mark_paused，bot_confirm_response 联动 skill_confirm_result（恢复/拒绝/超时默认拒）；resumable=false 暂停即终止语义落地
- 回滚建议务实版：失败+可回滚+有动作 → 提取「## 回滚」章节生成建议文本，run_model_loop 六出口拼接回复，模型询问用户后按章节执行逆操作
- 审计 +skill_paused/skill_confirm；单测 48→52；dev 重建 24037

### Skill 调度器核验（21:20-21:24）— 3 项修复

- 老板核验清单驱动，发现并修复：
  - P1 状态机 bug：start_skill 后停在 Loaded 未转 Running，步骤计数/熔断/动作记录全部静默失效 → 预审通过直接 Running
  - P1 单轮 Function 5 次熔断缺失（只有轮数限制，并行 tool_calls 可绕过）→ function_calls_total 独立计数熔断 + 审计
  - P2 歧义意图兜底：技能 ≥3 时提示先列候选向用户确认
- 核验通过：调度器仅编排不可绕过、暂停确认联动、审计、双安全域、preflight
- dev 重建 24037

### Windows 绿色版 v1.0.0 打包（21:43-21:47）

- 元数据：作者张晓峰（Cargo.toml authors + tauri bundle.publisher + package.json + README.txt）；版本 0.1.0 → 1.0.0
- 清理：.DS_Store、旧 zip（WebView2 安装器保留作 Win10 备用）
- mingw 全量重编（cargo clean + tauri build --no-bundle）→ exe 42.8MB；四件套 zip（Python zipfile）→ wmessage-win-x64-v1.0.0.zip 14.7MB
- 验证：exe 内版本/作者/CSP 字符串确认；zip 完整性 OK；飞书交付

### 截止时间改造：日历 + 时间方向键 + 无确认键（22:26-22:40，老板指令）

- 老板需求：看板首页截止日期时间设置不要确认键；日期用日历选，时间手动输入或方向键调整
- 实现：
  - 新组件 `src/components/DuePicker.tsx`：日期按钮（显示 MM-DD）→ 自定义日历弹层（周一开头、月切换 ‹ ›、今天高亮、选中 nm-inset 高亮、「今天」快捷按钮）；点日期**即选即存**并收起
  - 时间输入框：手动输 HH:mm（1-2 位时 + 2 位分，>23:59 忽略）或 **↑↓ 方向键 ±1 分钟**（Shift ±1 小时），即改即存；maxLength 5、placeholder HH:mm、blur 回退已提交值
  - × 移除截止时间；点外部或 Esc 收起编辑态；全部操作**无确认键**
  - TodoCard 截止编辑块：`datetime-local` 整行输入 → DuePicker（归档/回收站页复用 TodoCard 自动生效；挂件截止仅展示不变）
- 防御（沿用定时面板 NaN 事故教训）：parseDueParts/buildDue 严格正则解析组装，无效时间不写库；日历日期由年月日数字直接拼装，不经过 Date 解析，杜绝 Invalid Date → NaN 链
- 兼容：旧数据 date-only（"YYYY-MM-DD"）与 datetime（"…T18:00"）均正常解析；formatDue 已支持两种显示
- 验证：tsc + vite build 过；dev 实例 HMR 已生效

### 截止时间回退 datetime-local + 写库防御（22:41，老板拍板）

- 老板对比新旧后拍板：**回旧版**（datetime-local 单控件，紧凑），放弃 DuePicker 日历+时间框方案（组件已 trash）
- 保留新方案里的关键防御（这是老板最初痛点与历史 NaN 事故的根源）：
  - format.ts 新增 `isValidDateTimeLocal`：格式完整（`^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$`）+ 时 ≤23 / 分 ≤59 + `new Date` 往返日期一致（防 2026-02-31 静默进位）
  - TodoCard 截止 onChange：空串→清 due；完整合法→写库；**不完整/非法值一律不写库**，blur 回退已提交值
- 效果：旧版交互（一行控件、原生渲染）+ 新版安全（坏数据进不了库）
- 验证：tsc + vite build 过；dev 实例 HMR 生效

### 打勾圆圈：完成时间展示 + 取消完成按日期回列（22:55，老板指令）

- 老板需求：完成时间自动生成并**显示在截止日期右边**；截止日期的 × 离近一点腾位置；完成列再点 ✓ → 完成时间的日期是今天 → 回「今日」，否则回「待办」，完成时间删除
- 实现：
  - format.ts：`formatCompletedAt(ms)` → 「完成 MM-DD HH:mm」；`isToday(ms)` 本地时区判今天（模块级 pad 上提，顺带消除局部重复）
  - TodoCard 截止行：due 按钮 + × 收紧为同一 span（gap 0、× 宽 w-4）；完成/归档卡在右侧显示完成时间（column===done 且 completedAt 存在）；取消完成时列回退按 `isToday(completedAt)` 决定 doing/todo，并清 completedAt/archived
  - WidgetApp toggleDone 同步同规则（挂件虽只显示未完成任务，保持两窗口逻辑一致）
- 验证：tsc + vite build 过；dev 实例 HMR 生效

### 完成时间居中显示（23:01，老板指令）

- 完成时间改为 ×（截止移除）与 🗑️（删除任务卡）之间**居中**（flex-1 + text-center + whitespace-nowrap），不再紧跟截止日期；todo/doing 卡布局不变

### 看板功能多角度审计（23:12，老板指令：Section 一验收通过后全面审计）

- 审计范围：主看板 17 项功能（数据一致性/竞态/边界/跨窗口/注入安全）
- 修复 2 项：
  - P3 看板「已归档 N」计数把回收站里的归档卡也算进去 → 过滤条件补 `!t.deletedAt`
  - P2 标题/备注/标签/子任务/挂件标题输入框 Enter 提交无 IME 组合态防护 → 全部加 `!e.nativeEvent.isComposing`（中文输入法回车确认候选词不会误提交；macOS 无感，Windows 发布包重点回归）
- 核对无问题：mutate 单写者同步链无竞态；assignInsertOrder 浮点中点/边界 ±1/间隙耗尽重排正确；拖进空完成列草稿 fallback 正确；完成时间生成/清除/显示条件一致；due 三层校验；React 转义防注入；拖拽 5px 激活与点击不冲突；归档规则老数据补 completedAt 保证完成卡都有完成时间
- 待老板拍板：取消完成回「待办」的任务若 due=今天，今日规则会在 60s 内再拉回「今日」列（既有规则行为，是否需要豁免）

### 取消完成回列判断改截止日期（23:23，老板纠正）

- 老板纠正：完成列再点 ✓ 的回列判断依据是**截止日期**（due 日期部分是今天 → 回「今日」；否则回「待办」），此前误实现为完成时间日期
- 修正：format.ts `isToday(ms)` 替换为 `isDueToday(due)`；TodoCard/WidgetApp toggleDone 同步改；App.tsx 本地 isDueToday 删除、统一 import format.ts 版本（消除三处重复）
- 附带收益：之前审计提出的「取消完成回待办但 due=今天会被今日规则再拉回今日」的拍板问题自然消解——due=今天时本来就直接回「今日」列
- 验证：tsc + vite build 过；dev 实例 HMR 生效

### 归档窗口拖拽换列功能取消（23:37，老板指令）

- 老板裁定：归档页窗口拖拽换列功能取消，归档任务卡不可拖拽（仅展示）
- 代码现状核对：ArchivePage 早已是「搜索 + 标签计数筛选 + 三列网格（order 从左到右填充）」实现，无 DndContext/窗口拖拽/顺序持久化残留——**无代码改动，仅文档同步**
- 同步：README 功能特性行、验收清单二节（删除窗口拖拽项，补充「归档任务卡不可拖拽」）

### 归档卡只读化（23:42，老板指令）

- 老板裁定：归档卡除折叠开关 ▸/▾、绑定文件/文件夹打开（📂，桌面清理迁移后核验）、↩ 恢复外，其余一律不可编辑不可点击
- 实现（TodoCardView 按 archived 分支）：
  - 标题/备注：去掉点击编辑与 cursor-text；「+ 备注」隐藏
  - 标签：× 移除隐藏；「+ 标签」隐藏
  - 子任务：checkbox disabled；× 删除隐藏；「+ 添加子任务」隐藏
  - 文件：只留 📂 打开（📋 复制 / × 解绑隐藏）；无绑定文件时不显示绑定入口
  - 🤖 交给机器人 / ⏰ 定时面板：整体隐藏
  - 截止日期：改只读 span（无点击编辑/× 移除）；「+ 截止时间」隐藏
  - 🗑️ 删除：隐藏（归档卡只能恢复，不能删）
  - 标题自动编辑态（autoEdit）加 !archived 守卫（📌 跳归档只展开不编辑）
  - 保留：FoldToggle、完成时间展示、ActorAvatar、↩ 恢复
- 坑：`{!archived && (` 包裹两个兄弟 JSX 元素报语法错 → 加 fragment `<>`
- 验证：tsc + vite build 过；dev 实例 HMR 生效
- 文档：README 功能行、验收清单二节（新增只读条目）+ 六节 📌 跳转描述同步

### 归档卡保留 📋 复制文件（23:59，老板补充）

- 归档只读基础上，绑定文件区保留 📂 打开 + 📋 复制文件+标题；× 解绑仍隐藏（解绑属编辑）
- 文档同步：验收清单二节只读条目、README 功能行

## 2026-08-17（周一）远程手工验收 polish

> 老板今日通过飞书远程验收，要求「想好了再改，改完同步项目文档、开发文档、MANUAL-ACCEPTANCE.md」。本节为验收过程中通过代码 review 发现并修复的 6 项 polish，全部为前端 TS 代码改动，不动 Rust。

### ChatPanel.tsx 三处修复

1. **聊天输入框 Enter 加 IME 组合态防护**（P2 缺陷）
   - 现状：TodoCard 全部输入框（标题/备注/标签/子任务）已加 `!e.nativeEvent.isComposing`；ChatPanel 聊天输入漏了
   - 影响：中文输入法回车确认候选词会误触发 send，把半成品文本发给模型
   - 修复：与 TodoCard 一致 — `if (e.key === "Enter" && !e.nativeEvent.isComposing) send()`

2. **`messages.map` IIFE + mutation 渲染反模式重构**（P2 缺陷）
   - 现状：原代码在 JSX 渲染条件里用 IIFE `( () => { const fps = extractFilePaths(m.content); (m as Msg & { _fps?: string[] })._fps = fps; return fps.length > 0; } )()`，并把 `m._fps` mutation 放在渲染阶段
   - 影响：违反 React 渲染纯函数原则；StrictMode 下渲染双跑会覆盖 `_fps`；下方 `.map` 又读 `m._fps ?? extractFilePaths(...)` 做兜底，逻辑分散难读
   - 修复：把 `messages.map` 回调从箭头函数 `(m, i) => (...)` 改成块体 `(m, i) => { const fps = ...; const showActions = ...; return (...); }`，顶部一次性算 `fps` + `showActions`，去掉 IIFE 与对 `m` 的 mutation

3. **`/help` 命令改本地提示不持久化**（P3 缺陷）
   - 现状：原代码 `/help` 把「可用快捷命令…」push 到 messages 并 persistHistory，每次发 `/help` 历史里堆一条
   - 影响：与 `/stop`（无进行中）/ `/retry`（无 user 消息）/ `/compact`（消息太少）三个分支用的 `addHint` 模式不一致——这三条只本地提示不持久化
   - 修复：`/help` 也走 `addHint` 不持久化，纯本地展示。理由：help 文本属「参考信息」而非「对话内容」，重启后历史里堆 5-6 条 help 是噪音
   - 注意：实际命令（`/compact` 压缩结果、`/retry` 重跑结果）仍然持久化，行为不变

### WorkspacePage.tsx 三处修复

| 位置 | 触发 | 修复 |
|---|---|---|
| 编辑链接 displayName 输入框 | `commitEditLink()` | 加 `!e.nativeEvent.isComposing` |
| 编辑链接 targetUri 输入框 | `commitEditLink()` | 同上（含中文/空格的路径） |
| 添加链接 targetUri 输入框 | `commitAddLink(it.id)` | 同上（拿半输入 URL 进库） |

### 验收扫描

- 其他组件扫描（MigrationPanel / KanbanBoard / ArchivePage / TrashPage）：无同类 Enter 反模式
- tsc + vite build 全过；改动纯前端，Rust 不动

### 同步

- README.md / SPEC.md 不动（无功能/规格变更，仅内部 polish）
- MANUAL-ACCEPTANCE.md 末尾追加「本次验收过程同步修复（2026-08-17）」小节列明 6 项

### 回收站页只读化 + 彻底删除绑文件弹窗（10:42，老板指令）

**需求**
- 1. 主窗口回收站任务卡：除折叠/📂打开/📋复制/↩恢复/🗑彻底删除外，一律不可编辑不可点击（与归档卡同模式）
- 2. 彻底删除时若绑定了文件/文件夹，弹窗询问是否一并删除本地文件

**Rust 新增 `delete_bound_file` 命令**（`src-tauri/src/bot_skills.rs`，注册到 `lib.rs` invoke_handler）
- 签名 `delete_bound_file(path: String, is_dir: bool) -> Result<(), String>`
- 按 is_dir 分流 `remove_file` / `remove_dir_all`；路径不存在视为成功（幂等，已删则跳过）
- 错误透传给前端（权限不足/路径异常）→ 前端 alert 失败原因，任务行保留在回收站，用户可重试
- 与现有 `open_file_path` / `pick_files_dialog` 同文件就近平铺（文件操作就近原则）

**TodoCardView 改造（`src/components/TodoCard.tsx`）**
- 沿用归档卡模式：所有 `archived` 守卫扩到 `archived || trashed`，包括：
  - `useState(autoEdit && !archived)` → `... && !trashed`（📌 跳回收站只展开不编辑）
  - 标题 cursor + onClick + title 三处守卫
  - 备注 onClick + cursor + 「+ 备注」按钮隐藏
  - 标签 × 移除 + 「+ 标签」按钮隐藏
  - 子任务 checkbox `disabled={archived || trashed}`、× 移除 + 「+ 添加子任务」按钮隐藏
  - 文件 × 解绑 + 「📎/📁 绑定文件/文件夹」按钮隐藏
  - 截止编辑/×移除/「+截止时间」按钮隐藏（保留只读 span）
  - 🤖 + ⏰ 整段隐藏（`{!archived && !trashed && ...}`）
- 保留：折叠开关、📂 打开、📋 复制、↩ 恢复、🗑 彻底删除、完成时间展示、ActorAvatar
- 🗑 彻底删除按钮 onClick 改造（弹窗 + invoke）：
  - 有 `filePath`：confirm「任务卡「X」绑定了文件「Y」。完整路径：/.../Y。是否一并删除本地文件？此操作不可撤销。」
  - 无 `filePath`：confirm「确定彻底删除任务「X」？此操作不可撤销。」
  - 取消：不动
  - 确认 + 有 filePath：先 `invoke('delete_bound_file', { path, isDir })`，成功后再 `onDelete(task.id)`；失败 alert 中止
  - 确认 + 无 filePath：直接 `onDelete(task.id)`

**未动**
- TrashPage / App.tsx 无需改：confirm + invoke 全部封在 TodoCard 内部，`onDelete` 仍是原来的 `hardDeleteTask`（删 SQLite 行），流程干净
- 「清空回收站」不变：老板只说单卡彻底删除，全弹窗体验差不实施

**验证**
- `tsc --noEmit` 过；`cargo check` 过（dev profile 25.56s）
- 同步：MANUAL-ACCEPTANCE.md 三节末尾追加只读化条目 + 彻底删除弹窗条目；README 功能行补一句

### 挂件圆角规则改下半句（11:07，老板指令）

**原规则**（裁定 2026-08-15 21:09）：贴边（右/左/顶）→ 四角全直角贴合屏幕边缘；悬浮 → 四边全 `rounded-2xl`

**新规则**（2026-08-17 11:07）：**贴屏侧直角 + 对侧 `rounded-2xl`**；悬浮四边全 `rounded-2xl`（不变）

老板原话：「贴左缘挂件左边直角右边 `rounded-2xl` 圆角，贴右缘挂件右边直角左边 `rounded-2xl` 圆角，贴顶缘挂件上边直角下边 `rounded-2xl` 圆角」

**动机**：原「贴边全直角」让挂件看起来贴死在屏幕上；新规则让对侧圆角，挂件视觉上「浮起」感更强

**实现**（`src/components/WidgetApp.tsx`）
- 原：`const edgeClass = edge === "float" ? "rounded-2xl" : "";`
- 新：
  ```ts
  const edgeClass =
    edge === "right" ? "rounded-l-2xl" :
    edge === "left" ? "rounded-r-2xl" :
    edge === "top" ? "rounded-b-2xl" :
    "rounded-2xl";
  ```
- 触发条（collapsed）与展开面板（expanded）共用同一 `edgeClass` —— 两者视觉一致
- `.nm-sidebar-panel` / `.nm-sidebar-panel-top` 不设 `border-radius`，Tailwind 工具类完全可控

**验证**
- `tsc --noEmit` 过

**同步**
- MANUAL-ACCEPTANCE.md 五.2 描述按老板原话改写
- README.md 无需改（"圆角跟随边缘" 措辞笼统，新规则仍属跟随边缘）
- MEMORY.md ## Standing Decisions 挂件圆角规则条目：旧规则保留时间戳 + 标注「改下半句」 + 新规则描述 + 实现

### 唤起主窗口强制置顶规则（11:31，老板指令）

**原行为**：WidgetApp `focusMain` / ChatPanel `openTaskInMain` 仅 `main.show() + main.setFocus()`。Windows 上 `setFocus` 不一定把窗口推到 z-order 最顶层（其他窗口抢焦点时主窗口被遮住）

**新规则**（老板 2026-08-17 11:31）：双击唤起后主窗口**必须出现在桌面屏幕最顶层**才能看见

**实现**
- 新增 `src/focus.ts` — 共享 `focusMainWindow()` 工具：
  ```ts
  export async function focusMainWindow(): Promise<void> {
    const main = await WebviewWindow.getByLabel("main");
    if (!main) return;
    await main.show().catch(() => {});       // 防隐藏
    await main.unminimize().catch(() => {});  // 防最小化（Windows 必需）
    await main.setFocus().catch(() => {});    // macOS/Linux 推到 z-order 最前
    await main.setAlwaysOnTop(true).catch(() => {});  // Windows 强制置顶兜底
    await new Promise((r) => setTimeout(r, 80));
    await main.setAlwaysOnTop(false).catch(() => {}); // 立即恢复，不长驻
  }
  ```
- WidgetApp 双击标题 `openInMain` → `emit("edit-task") + focusMainWindow()`
- ChatPanel 点任务引用 `openTaskInMain` → 同样改 `focusMainWindow()`（一致性）
- 删除两处内联的 `WebviewWindow` + `show/setFocus` 重复代码（约 10 行）

**为什么不长期驻顶 alwaysOnTop**：会干扰用户正常使用电脑（盖住其他窗口）；短暂闪烁只在唤起那一瞬生效

**验证**：tsc 过（移除两处未使用的 WebviewWindow import 顺手清掉）

**同步**
- MANUAL-ACCEPTANCE.md 五.5 加「唤起后主窗口必须出现在桌面屏幕最顶层」子项
- README.md 不动（widget 功能行「点任务标题 → 主窗口弹出并进入该任务编辑态」措辞笼统仍适用）

### 唤起主窗口强制置顶 — Rust 端单一真相（11:43，老板追问）

**问题**：之前只在 JS 侧（`src/focus.ts` + `WidgetApp.openInMain` + `ChatPanel.openTaskInMain`）走了 alwaysOnTop 闪烁逻辑；Rust 侧 4 处内联调用仍是裸的 `show + unminimize + set_focus`：

- `lib.rs` line 155-157 全局快捷键 Cmd+Ctrl+W 的 show 分支
- `lib.rs` line 162-164 全局快捷键 Cmd+Ctrl+N（快速新建）
- `lib.rs` line 275-277 托盘菜单「打开主窗口」
- `lib.rs` line 289-291 托盘图标左键/双击

老板 11:43 追问「那程序在 Windows 的托盘里也能跳出主窗口」 — 理论上能（`show` 会取消隐藏），但 Windows 上不一定在 z-order 最顶层（与之前 setFocus 同样的问题）

**改造**
- `src-tauri/src/lib.rs`：新增 `pub fn bring_main_to_front(window: &WebviewWindow)` helper（show + unminimize + set_focus + setAlwaysOnTop 闪烁 80ms）+ `#[tauri::command] fn focus_main_window(app: AppHandle)` 命令包装（供 JS invoke）
- 4 处 Rust 调用点全部改走 `bring_main_to_front(&w)`（删除内联的 show/unminimize/set_focus 三行，约 12 行）
- 注册 `focus_main_window` 到 invoke_handler
- `src/focus.ts`：删掉 JS 端重复实现，改为 `await invoke("focus_main_window")`（单一真相在 Rust）

**为什么 Rust 做单一真相**
- 4 处 Rust + 2 处 JS 调用点如果各做各的，闪烁时长/策略改了要改 6 处
- Rust 端可以用 `std::thread::sleep` 同步阻塞 80ms，JS 端通过 invoke 异步等结果
- 同步 vs 异步：sync 命令在 Tauri 主线程跑，80ms 可接受（用户点击瞬间的小延迟）

**同步阻塞 80ms 的取舍**
- 不长驻 alwaysOnTop 是必须的，否则盖住所有窗口干扰用户
- 短暂闪烁必须有，否则 Windows 上 setFocus 推不到 z-order 最顶层
- 80ms 是经验值，足够 OS 应用 alwaysOnTop 状态再恢复（可在老板实测后调整）

**验证**
- `tsc --noEmit` 过
- `cargo check` 过（1.20s 增量编译）

**同步**
- MANUAL-ACCEPTANCE.md 五.5 子项更新：标注 6 处调用点（2 JS + 4 Rust）都走 `bring_main_to_front` 单一真相
- README.md 不动（措辞已涵盖）

### 挂件任务卡 🤖 + ⏰ 折叠到「操作」下拉键（12:06，老板指令）

**问题**：挂件「全部/今日」视图下，新建任务（`+ 新建任务` 大长条触发，addTask prepend 到列表顶部）会直接空显 `🤖 交给机器人` + `⏰ 定时` 两个按钮，标题还是「新任务」、没填任何内容时就露在外面——视觉杂乱

**老板指令**：改为点下拉键才显示这些操作

**实现**（`src/components/TaskCardContent.tsx`，仅改挂件，主窗口 TodoCard 不动）
- 新增 `adminOpen` state（默认 `false`）
- 把原来的「🤖 交给机器人 + ⏰ 定时 + 定时面板」三段整体包到 `adminOpen && (onBotExecute || onSetSchedule)` 折叠段
- 在折叠段上方加「▸ 操作」按钮（chevron + label），点击切换 `adminOpen`
- ⏰ 按钮点击逻辑补充：`if (!schedOpen) setAdminOpen(true)` —— 点 ⏰ 打开定时面板时自动展开「操作」段，否则面板藏在折叠态看不见（关闭时不强制展开，避免与用户主动收起冲突）

**主窗口 TodoCard 暂不动**
- 老板原话「在挂件」明确指 widget
- 主窗口横向空间大，inline 显两个按钮问题不大
- 后续若要统一改可参照本实现

**验证**
- `tsc --noEmit` 过
- 不动 Rust

**同步**
- MANUAL-ACCEPTANCE.md 五.6（挂件任务卡）补「🤖 + ⏰ 默认折叠在下拉键后面」子项
- README.md 不动（挂件任务卡显示描述是功能级，polish 不需列）

### 挂件任务卡标题永远单行截断（12:14，老板指令）

**问题**：widget 标题 h3 的 `truncate` 类只在 `task.collapsed === true` 时应用，展开态可换行。挂件面板宽 320px - 32px padding - 24px card padding = 264px，标题行内还要扣除 ☰/折叠/打勾/头像 ≈ 96px，标题实际可用宽度仅 ~168px——长标题必换行撑高卡片

**老板原话**「缩略显示和主窗口一样，任务卡保持一行」

**实现**（`src/components/TaskCardContent.tsx`，仅 widget，主窗口 TodoCard 未动）
- 标题 h3 `className` 去掉 `task.collapsed ? "truncate" : ""` 条件，改为永远 `truncate`
- `title` 属性简化为永远 `task.title`（鼠标悬停看完整标题）
- 保留 `cursor-text`（onTitleClick 时）+ 双击唤起主窗口的 onDoubleClick 行为

**主窗口 TodoCard 同样问题没改**（待老板拍板）
- 主窗口 `src/components/TodoCard.tsx` line 207-209 同样的 `${task.collapsed ? "truncate" : ""}` 条件
- 老板原话「和主窗口一样」——但实际两边都只在 collapsed 时截断，主窗口横向空间更大（撑高问题不那么明显）所以视觉上更不易察觉
- 两种处置：
  - A. 仅改 widget（已做）：严格按老板字面要求，但 widget 和主窗口不一致
  - B. 一起改：widget 和主窗口都永远 `truncate`，两边统一；但改了主窗口是额外动作
- 等老板晚上核对时决定

**验证**
- `tsc --noEmit` 过
- 不动 Rust

**同步**
- MANUAL-ACCEPTANCE.md 五.6 挂件任务卡补「标题永远单行截断」子项
- README.md 不动（属于 polish 行为而非功能）

### 挂件标题溢出检测 + 折叠键（12:24，老板细化指令）

**上一轮改法**：上一轮把 widget 标题改为永远 `truncate`，无条件截断
**老板反馈**「不动主窗口，只改挂件，新建任务还是要判断标题名称是否超过了一行，超过一行用缩略显示，增加折叠键，展开态会显示全部标题」

**修正后设计**（`src/components/TaskCardContent.tsx`，仅 widget）
- **JS 运行时检测溢出**：`useLayoutEffect` 测量 `scrollHeight` vs `lineHeight`，超出一行才设 `titleOverflow=true`；短标题保持原样
- **条件截断**：仅 `titleOverflow === true` 且未展开时 `truncate`，避免短标题被无谓截断
- **条件折叠键**：仅 `titleOverflow === true` 时在标题右侧显示 ▾/▴ 按钮，溢出才露控件
- **展开态切换**：点 ▾ → 标题完整显示 + 切换为 ▴；再点 ▴ 回到截断态
- **标题变化重置**：useLayoutEffect 依赖 `task.title`，标题改了重新检测 + 重置 `titleExpanded=false`

**为什么用 `useLayoutEffect` 而不是 `useEffect`**
- useLayoutEffect 在 DOM 更新后、浏览器绘制前同步运行
- 可避免「先渲染完整标题 → useEffect 后检测 → 重渲染截断」的闪烁
- 短标题场景：首次渲染无 truncate → useLayoutEffect 确认无溢出 → 保持无 truncate，无闪烁
- 长标题场景：首次渲染无 truncate → useLayoutEffect 检测溢出 → 第二次渲染加 truncate + 折叠键，可能一帧闪烁（可接受）

**主窗口 TodoCard 未动**（老板明确「不动主窗口」）

**验证**
- `tsc --noEmit` 过
- 不动 Rust

**踩坑（已修）**
- 上一轮手抖把输入框的 `nm-task-title` 误改成 `nm-task-name`（类不存在，会丢失字体样式）—— 已回退

**同步**
- MANUAL-ACCEPTANCE.md 五.6 「标题永远单行截断」条目改写为「标题溢出检测 + 折叠键」（描述新行为 + 主窗口未动）
- README.md 不动

### 挂件任务卡折叠设计回滚到单一 FoldToggle（12:33，老板拍板）

**上一轮设计被否定**：
- 12:06 加 `▸ 操作` 折叠键 + `adminOpen` state 包裹 🤖/⏰/schedule panel
- 12:24 又给标题加溢出检测 + 新折叠键（titleRef/useLayoutEffect/titleOverflow/titleExpanded）

**老板原话**「没有改对，不用重新设计折叠窗口，挂件新建任务时，应该每个都带下拉键折叠窗口，下来后显示交给机器人和定时，这时候会显示标题全名吧」

**真意**：
- 不要新增任何折叠机制，复用现有 FoldToggle
- 每个挂件任务卡（包括新建空任务）都永远显示 FoldToggle
- 折叠态只露标题，展开态露标题完整 + 🤖 + ⏰ + schedule panel + 其他内容

**回滚改造**（`src/components/TaskCardContent.tsx`，仅挂件，主窗口未动）
- 删除 `adminOpen` state（连同其 JSX 引用 + `setAdminOpen` 调用）
- 删除 `titleRef` + `titleOverflow` + `titleExpanded` + `useLayoutEffect` 整套溢出检测
- 删除 `useLayoutEffect` + `useRef` import（仅留 `useEffect` + `useState`）
- 删除 `hasBelow` 常量（已无用处）
- FoldToggle 去掉 `hasBelow &&` 门控，改为 `{onToggleCollapsed && ...}` 永远显示
- 标题 h3 回到原始 `task.collapsed ? "truncate" : ""` 模式（含 `title` 属性 conditional）
- 🤖 + ⏰ 行恢复为水平 flex 布局（去掉 `flex-col` 包装 + `self-start`）
- ⏰ onClick 去掉「点开时确保 admin 段展开」逻辑（adminOpen 不存在了）
- schedule panel 移到独立 block（与 🤖/⏰ 行平级，不再嵌在折叠态里）

**结果**
- 挂件任务卡（包括新建空任务）永远显示一个 FoldToggle（▸/▾）
- 折叠态：只露标题（单行截断）
- 展开态：标题（完整多行）+ 备注/标签/子任务/文件 + 🤖 + ⏰ + 定时面板（点 ⏰ 弹出）+ 截止时间
- 没新增任何折叠机制

**验证**
- `tsc --noEmit` 过
- `grep adminOpen` 0 残留
- 不动 Rust

**同步**
- MANUAL-ACCEPTANCE.md 五.6 补「统一复用现有 FoldToggle」条目，记录两轮设计被否、回滚到单一 FoldToggle 的最终设计
- README.md 不动

### 斜杠命令 autocomplete picker + /help 单一真相（15:51，下午开工）

**老板指令**：聊天窗口用户输入第一个字「/」时，向上浮出所有快捷命令（自动补全面板）

**实现**（`src/components/ChatPanel.tsx`）
- 抽 `SLASH_COMMANDS` 常量（模块顶部，Props 类型后）：
  ```ts
  const SLASH_COMMANDS = [
    { cmd: "/stop", description: "停止当前回复" },
    { cmd: "/compact", description: "压缩对话上下文" },
    { cmd: "/retry", description: "重新生成上一条回复" },
    { cmd: "/help", description: "显示本帮助" },
  ];
  ```
- 加 `slashIdx`（当前选中下标）+ `slashDismissed`（Esc 后不再自动浮出，直到下次输入）
- 派生 `pickerOpen` = `!slashDismissed && input.startsWith("/") && !input.includes(" ")` —— 第一个字是 / 且没空格
- Picker UI（`{!slashDismissed && input.startsWith("/") && !input.includes(" ") && (...)}`）渲染在 input 行**上方**（包裹 input 行的 `<div>` 改成 `flex-col`，picker 是 sibling）
  - 每个命令一行：`<button>` 含命令名 + 描述，hover 高亮 / 选中态 `nm-inset`
  - 前缀过滤：`SLASH_COMMANDS.filter(c => c.cmd.startsWith(input))` —— 输入 `/` 显全部，`/s` 只显 /stop，`/h` 只显 /help，无匹配自动隐藏
- input 行 onKeyDown 新增拦截（picker 开着时）：
  - `ArrowDown` / `ArrowUp`：循环切换选中下标
  - `Tab` / `Enter`：补全为 `cmd + " "`（**注意：picker 开着时 Enter 也走补全**，避免输入半截命令误发送）
  - `Escape`：设 `slashDismissed=true` 关掉 picker（输入保持原样）
- input 行 onChange 同步重置 `slashIdx=0` + `slashDismissed=false`（任何输入都重开 picker）
- `/help` 命令 handler 改用 `SLASH_COMMANDS.map((c) => \`${c.cmd} ${c.description}\`).join("\n")`（单一真相，autocomplete 与 /help 共用同一清单）

**为什么 Tab 和 Enter 都走补全**
- picker 开着意味着用户输入不完整（只有命令名、还没参数），直接发送会变成「/s」被当普通消息发出去——无效
- Enter 走补全 + 关 picker，用户看到自动补的命令名后再按 Enter 才真正发送（命令名 + 空格，会被 runSlashCommand 处理）

**Esc 后怎么重开 picker**
- 任何输入（onChange）都会 `setSlashDismissed(false)`，重新激活自动显示
- 用户可以从空输入框开始重新输入 `/` 触发

**验证**
- `tsc --noEmit` 过（修了一个 map 第三参数 `arr` 未使用的 TS6133）

**同步**
- MANUAL-ACCEPTANCE.md 六.8 斜杠命令条目补「/help 不持久化」+「/ 斜杠命令 autocomplete picker」两段
- README.md 不动

### Skill 调度器 Step 1：后置拦截（原子黑名单硬锁，18:14 老板拍板）

**老板拍板**（Q1-Q4 完整决策）：
- Q1 复合业务清单保留（任务汇总/归档迁移/批量导出Excel/PPT生成/Word修订/文档生成/联网搜索/Python/任务卡执行 等）
- Q2 启用禁止裸调原子 Function 黑名单
- Q3 设立单点白名单，run_python 归入白名单
- Q4 开发顺序：先做后置拦截（小事、见效快）→ 再做前置预路由（核心硬锁）

**本节：Step 1 后置拦截落地**

**新增 `src-tauri/src/tool_guard.rs`**（独立模块）
- `ATOMIC_TOOLS: &[&str]` 黑名单：`create_word_revisions`（Word修订Skill内专用）+ `link_file_to_task`（Skill末尾绑产物专用）
- `is_atomic_tool(name) -> bool` 黑名单识别
- `atomic_block_message(name) -> String` 拦截错误文案（含「请走对应 Skill」引导）
- `is_skill_active() -> bool` 委托 `bot_skills::is_skill_active()`（穿透 Mutex 访问）

**改 `src-tauri/src/bot_skills.rs`**
- 新增 `pub fn is_skill_active() -> bool`：遍历 SKILL_RUNS 全局表查 `state == SkillState::Running`
- Running → 放行原子工具；其他状态（Loaded/Finished/Terminated/Failed/Paused）→ 阻断

**改 `src-tauri/src/lib.rs`**
- 注册 `mod tool_guard;`

**改 `src-tauri/src/bot.rs::execute_tool`**
- 入口加黑名单硬锁（先于 skill_on_step 步骤钩子，避免误计数）：
  ```rust
  if crate::tool_guard::is_atomic_tool(name) && !crate::tool_guard::is_skill_active() {
      let msg = crate::tool_guard::atomic_block_message(name);
      audit_log(app, &format!("tool_blocked_atomic | {name} | 硬锁拦截，提示走对应 Skill"));
      return (msg, Vec::new());
  }
  ```
- 现有 skill_on_step 钩子保留（步骤计数/熔断/动作记录）

**单测**（`tool_guard::tests`，6 个全过）
- `atomic_blacklist_recognizes_create_word_revisions` ✓
- `atomic_blacklist_recognizes_link_file_to_task` ✓
- `single_point_tools_are_not_atomic`（遍历 18 个非原子工具确保不被误判）✓
- `atomic_block_message_mentions_skill`（拦截文案必须引导走 Skill）✓
- `atomic_block_message_unknown_tool_fallback`（未知工具名兜底文案）✓
- `is_skill_active_false_with_no_skills`（无 Skill 时返回 false）✓

**验证**
- `cargo check` 过（0.80s）
- `cargo test --lib tool_guard` 6/6 过
- 整体 `cargo test --lib` 应仍全过（已有 52 个测试）

**黑名单初定 vs 待补**
- 当前：仅 2 个原子（Word修订 + 绑产物）。后续新加原子工具时同步扩 `ATOMIC_TOOLS`
- 没动单点白名单实现（run_python 等保持原有直接调用）

**下一步（Step 2）待老板下令启动**
- 前置预路由：bot_chat 入口前加意图分类；命中复合业务 → 直接 start_skill；未命中 → 放行 LLM
- 意图识别主力：关键词正则 L1；LLM 语义 L2 可选兜底
- 双轨触发：保留 `use_skill`（LLM-driven 兼容）+ 新增预路由直接 start_skill

### Skill 调度器 Step 2：前置预路由（L1 正则硬锁，18:14 老板拍板）

**老板拍板**（Q1）：复合业务清单保留，意图识别主力 = L1 关键词/正则硬锁；LLM 语义 L2 可选兑底（初期不上）

**本节：Step 2 前置预路由落地**

**新增 `src-tauri/src/intent_router.rs`**（独立模块 + 11 单测）
- `RouteAction` 枚举：`Skill(String)` | `PassThrough`
- `INTENT_RULES` 常量：7 条复合业务 → Skill 名映射
  - ppt-orchestra-skill、minimax-docx（修订模式）、minimax-xlsx、minimax-pdf、minimax-web-search、minimax-task-summary、minimax-archive
  - Python 脚本不在规则里（单点白名单 run_python，老板 Q3）
- 正则模式比关键词更灵活：允许中英文混杂 + 中间夹字（如「做一份 XX主题的PPT」中间有「XX主题的」也能命中）
- `once_cell::sync::Lazy` 进程启动时一次性编译所有正则
- 11 单测全过：7 个正向路由 + 4 个负向不路由

**Cargo.toml 加依赖**
- `regex = "1"`（关键词正则匹配）
- `once_cell = "1"`（Lazy 静态编译）

**改 `src-tauri/src/lib.rs`**
- 注册 `mod intent_router;`

**改 `src-tauri/src/bot.rs::bot_chat`**
- 在 `msgs` 构造系统提示词时插入预路由检查：
  - 取用户首条消息文本（`messages.last()`）调 `route_user_input(text)`
  - 命中 `RouteAction::Skill(name)` → 调 `bot_skills::start_skill` 启动 Skill（state → Running，原子黑名单自动放行）
  - 把 Skill body 拼到系统提示词末尾（`## Active Skill: {name}\n\n{body}`）→ LLM 看到 Skill 指令直接执行
  - 写 `bot.log`：`intent_route | {name} | 预路由命中，直接加载 Skill（跳过 LLM 选 Skill）`
  - Skill 未安装 / 加载失败 → 写 log 后放行 LLM（不阻断聊天）
- 命中 `RouteAction::PassThrough` → 原 LLM 路径不变
- **关键**：Skill 选择由路由器决定，LLM 不参与；LLM 仅在 Skill 内部执行时参与工具调用

**与 Step 1 联动**
- pre-router 调用 `start_skill` → state 进入 Running → `is_skill_active()` 返回 true
- Skill 内 LLM 调 `create_word_revisions` / `link_file_to_task` 等原子工具 → 后置拦截放行
- 链路打通：路由命中 → Skill 启动 → 原子工具放行 → Harness 全程监控 → Skill 结束

**整体单测状态**
- 总数 68 passed / 0 failed（原 52 + Step 1 加 6 + Step 2 加 10 — 部分测试并到 intent_router module）
- `cargo test --lib` 全过（api / bot / bot_py / bot_skills / bot_web / db / intent_router / migration / tool_guard 9 个 module）

**预路由 INTENT_RULES 表**（精简版）

| 用户输入关键字（正则） | 加载 Skill |
|---|---|
| 做/生成/搞 + ppt/幻灯片/演示 | `ppt-orchestra-skill` |
| 润色/修订 + word/文档，用修订模式 | `minimax-docx` |
| 做/生成 + excel/xlsx/表格 | `minimax-xlsx` |
| 做/生成 + pdf | `minimax-pdf` |
| 搜/搜索/查 + 联网 | `minimax-web-search` |
| 任务/事项 + 汇总/总结 | `minimax-task-summary` |
| 归档/迁移/清理 + 文件/桌面 | `minimax-archive` |

**未来扩展**（老板 Q1 拍板「L2 可选兑底」）
- 加 LLM 语义判断（用更小模型专门做 intent classification，置信度 < 阈值时回退到 LLM）
- 关键词表扩充（更多业务类型、更多同义口语变体）
- Skill 命名映射（实际 skill 名可能跟 INTENT_RULES 不一致，需要 import 时校对）

**下一步（Step 3，待老板下令）**
- 流程完整闭环：预路由 → Skill 启动 → 原子工具 → Harness 监控 → Skill 结束
- 待补：Skill 结束 / 中途失败的 fallback 策略、调度器审计报告、用户视角的「为什么这次没走 LLM 选 Skill」解释

---

## 2026-08-17（周日）回收站彻底删除·绑文件三选项弹窗

### 改动动机
原弹窗用 `window.confirm` 两选项（确定=文件+任务卡一起删；取消=只删任务卡），歧义大；
老板裁定改为三选项：🗑 全部删除 / � 保留文件删除 / 取消。

### 改动
- `src/components/TodoCard.tsx`
  - 新 state `purgeOpen` / `purgeBusy`
  - 彻底删除按钮 onClick 分支：有 `task.filePath` → `setPurgeOpen(true)` 弹三选项；
    无文件时保持原两选项 `window.confirm`
  - card 主体 return 末尾新增 `{purgeOpen && task.filePath && <div fixed inset-0 z-50 ...>}` 自定义 dialog
- 三按钮语义：
  - 🗑 全部删除（红色）：先 `invoke("delete_bound_file", { path, isDir })`，成功后再 `onDelete(id)`；
    失败 alert（任务卡保留在回收站，可重试或手动删除后再试）
  - 📄 保留文件删除：直接 `onDelete(id)`，本地文件不动
  - 取消：仅 `setPurgeOpen(false)`
- 防误点：`purgeBusy` 锁住三按钮 + 关闭 backdrop 的点击关闭（避免删除中途关掉导致 state 不一致）
- 点 backdrop = 取消（不执行删除）；点 dialog 内 = 阻断冒泡

### 验证
- `npx tsc --noEmit` 零错误
- `npx vite build` 通过（827ms）
- DEVLOG 同步

### Bug fix：回收站彻底删除弹窗 hover 任务卡时显示不正确（21:10 老板报）

- **症状**：鼠标放在任务卡上点 🗑 彻底删除 → 弹窗被裁缩到卡片边界内，标题「🗑 彻底删除任务卡」不可见
- **根因**：TodoCard 容器有两层 transform 创建 CSS 包含块，使 `position: fixed` 子元素不再相对视口定位
  1. `nm-card-hover:hover` 触发 `transform: translateY(-3px) scale(1.01)`（`src/ui/main.css` L96）
  2. dnd-kit `useSortable` 写的 `style={{ transform: CSS.Transform.toString(transform), transition }}`（TodoCard.tsx L791）—— 即使 transform 为 null，`CSS.Transform.toString(null)` 返回 `""`，写成 `transform: ""` 仍是非 none 值
- **修法**：dialog 外层用 `createPortal(<div ...>, document.body)` 渲染到 body 下，彻底脱离 TodoCard DOM 子树；z-index `z-50` → `z-[100]` 覆盖 hover shadow 提升
- **踩坑教训**：任何用了 dnd-kit / framer-motion / CSS hover transform 的卡片，内部弹窗必须 Portal，否则 fixed 失效被裁

### 21:17 老板拍板「删除绑定的文件 → Mac 废纸篓 / Windows 回收站」

- **动机**：原 `std::fs::remove_file` / `remove_dir_all` 是物理删除、不可恢复；改为跨平台 trash crate，误删可救（Mac 废纸篓默认 30 天 / Win 回收站永久直到清空）
- **Cargo.toml**：加 `trash = "5"`（2024 仍在维护，跨平台：macOS NSFileManager.trashItemAtURL / Windows SHFileOperation FOF_ALLOWUNDO / Linux gio trash）
- **src-tauri/src/bot_skills.rs::delete_bound_file**：
  - 不再按 `is_dir` 分流 `remove_file` / `remove_dir_all`，统一 `trash::delete_all(p)`
  - 错误文案按平台 cfg 分发："移到废纸篓失败" / "移到回收站失败" / "移到垃圾箱失败"
  - 错误消息追加「文件可能仍在原位置，任务卡保留可重试」提示
- **src/components/TodoCard.tsx**：
  - alert 错误文案调整：`${e}\n\n任务卡保留在回收站，可重试或手动从废纸篓/回收站清理后再试。`
  - 「全部删除」按钮副标题：`任务卡删除，并把绑定的本地文件夹/文件移到废纸篓/回收站`
- **三类文档同步**：
  - MANUAL-ACCEPTANCE.md L55-56 描述加「+ 21:17 移到废纸篓/回收站」后缀，明示走 trash crate
  - README.md L12 回收站 bullet 改写，明确「移到废纸篓/回收站」+「不是物理删除，误删可恢复」
  - SPEC.md 无相关条目，无需改
- **验证**：
  - `cargo check`：trash v5.2.6 拉下，编译零错（1.85s）
  - `cargo test --lib`：68 passed / 0 failed（既有单测未破）
  - `tsc --noEmit` 零错；`vite build` 827ms 通过
- **风险与边界**：
  - 网络盘文件 `trash` 可能失败 → 错误透传 + 让用户重试或手动处理（沿用 alert 路径）
  - Mac 上若 app 走沙盒会需要 entitlement（本项目 desktop app 不走沙盒，无影响）
  - 「不是物理删除」对审计/合规场景需注意；未来若要「真·不可恢复」，加「绕过废纸篓」勾选开关

## 验收过程 polish（21:42 / 21:52）

### 21:42 修 bug：挂件双击唤起主窗口后标题输入框显示旧值

**症状**：挂件新建任务并改名为「测试」→ 立即双击标题 → 主窗口唤起后那张卡的 input 框显示「**新任务**」而不是「测试」；挂件自己显示正常

**根因**：主窗口 `TodoCardView` 的标题草稿 `const [draft, setDraft] = useState(task.title)`，React useState 初值只在 mount 时算一次。挂件发 `tasks-updated` 后主窗口 `setTasks` 重渲染，但 `<SortableTodoCard key={t.id}>` 的 key 不变 → TodoCardView 不重挂载 → `draft` 停在第一次渲染时的「新任务」。双击唤起后 `edit-task` 事件 → `setEditingId(id)` → autoEdit 由 false→true → useEffect 触发 `setEditing(true)` 但**没同步 draft** → 渲染 `<input value={draft}>` 显示「新任务」

**对比挂件没踩的原因**：`TaskCardContent.tsx` line 47-49 useEffect 有 `if (editingTitle) setDraft(task.title)`，会在切换到编辑态那一瞬同步草稿；主窗口 TodoCard.tsx 缺这一行

**修法**（`src/components/TodoCard.tsx`）：用 `prevAutoEdit` ref 锁住「autoEdit 由 false→true 那一瞬」同步 `setDraft(task.title)`，避免 editing 期间因 `task.title` 进依赖而覆盖用户输入；editing 期间 task.title 不会外部变化（commit 才改，且同时 setEditing(false)），所以 task.title 进依赖安全
```tsx
const prevAutoEdit = useRef(autoEdit);
useEffect(() => {
  if (autoEdit && !archived && !trashed) {
    if (!prevAutoEdit.current) setDraft(task.title);
    setEditing(true);
  } else if (!autoEdit) {
    setEditing(false);
  }
  prevAutoEdit.current = autoEdit;
}, [autoEdit, archived, trashed, task.title]);
```

**三类文档同步**：
- `SPEC.md`：本 bug 暂未单独列功能需求条目（属于既有交互的边界修复，不改 SPEC 的功能列表；MANUAL-ACCEPTANCE 五.挂件新增验收子项「双击唤起后编辑框标题与挂件同步」）
- `README.md`：功能特性挂件 bullet 注明「**双击**」（强调单击不再触发，2026-08-17 12:17 老板拍板；本 bug 修复后行为）
- `docs/MANUAL-ACCEPTANCE.md`：
  - 五.挂件加新子项「双击唤起后编辑框标题与挂件同步」
  - 末尾「本次验收过程同步修复」追加 21:42 段落（症状/根因/修法/关联验收项）

**验证**：
- `tsc --noEmit` 零错
- `vite build` 848ms 通过
- dev 实例手动跑流程：挂件新建任务 + 改名「测试」+ 双击唤起 → 主窗口 input 显示「测试」（不是「新任务」）✅

**教训**：
- React `useState(initial)` 的初值只算一次，prop 后续变化不会重算 → 用 key 强制重挂是重置初值的唯一方法；用 useEffect 同步是另一种（用 ref 锁住「切换瞬间」防覆盖用户输入）
- 主窗口 vs 挂件两边都要写草稿同步逻辑时，记得对齐；这次的根因就是「两边只对齐了 useEffect 进编辑态，没对齐草稿同步」

### 21:52 老板拍板移除机器人聊天 /help 命令

**症状**：`/help` 现在只在本地弹一条提示（addHint 模式，不发后端），但补全面板已上浮（输入 `/` 弹出全部候选命令），`/help` 这个命令本身冗余；老板认为上浮面板已能起到「查命令」作用，`/help` 留着是潜在上下文污染与冗余 UI 入口

**改造**（`src/components/ChatPanel.tsx`）：
- `SLASH_COMMANDS` 列表删除 `{cmd:"/help", description:"显示本帮助"}` 条目；注释改写为「autocomplete picker + runSlashCommand 共享」并加 21:52 移除原因 + 老板指令引用
- `runSlashCommand` 删除 `if (cmd === "/help")` 分支（含原 9 行 addHint + SLASH_COMMANDS.map 拼字符串的代码）
- 空状态占位文本「（/help 查看快捷命令）」→「（输入 / 看可用命令）」——指引用户用 `/` 浮出补全面板查命令

**保留**：`/stop` `/compact` `/retry` 三个本地命令不动（均有副作用：停流/压缩/重试，行为不变）

**三类文档同步**：
- `SPEC.md`：第 10 条「斜杠命令：/stop /compact（≤300 字摘要）/retry」去掉 /help；末尾新增第 13 条「移除 /help 命令」，记录缘由 + 行为变化（用户输入 /help 落到 send() 当普通文本发模型）+ 影响面（仅 src/components/ChatPanel.tsx）
- `README.md`：内置机器人聊天 bullet 里「斜杠命令 /stop /compact /retry /help」→「斜杠命令 /stop /compact /retry」，加 2026-08-17 21:52 移除原因注脚
- `docs/MANUAL-ACCEPTANCE.md`：
  - 六.内置机器人聊天里整条斜杠命令验收重写（去掉 /help 列命令、/help 不持久化；保留 /stop /compact /retry；picker 现在剩 3 条）
  - 末尾「本次验收过程同步修复」追加 21:52 段落（症状/改造/影响面/关联验收项）

**验证**：
- `tsc --noEmit` 零错
- `vite build` 820ms 通过（js 体积从 521.48 kB 降到 521.33 kB，减 150 字节）
- `grep "/help" src/components/ChatPanel.tsx`：只剩注释里的移除原因，无业务代码
- `grep "help\|/help" src-tauri/src/`：Rust 端无 /help 处理（无需改）

**风险与边界**：
- 用户输入 `/help` 现在会被发到模型 → 模型按普通指令回复（可能瞎编或问「你能帮我做什么」）；若老板想加兜底可在 `send()` 入口识别 `/` 开头且不在白名单的斜杠文本时弹本地提示，但当前不做（老板说「直接当普通文本走模型」即可）
- autocomplete picker 的 4 → 3 变化需老板重启 dev 验一遍 UI（之前用过的用户可能不知道 picker 现在少了一条）


## 2026-08-18（周二）C 路径 Phase 1-5（DSL 调度器生产化）+ Phase 5（4 项生产化）

### C 路径概览
DSL 调度器从「解析 + 单次顺序执行」演进到「全链路生产可用 + 可监控 + 可调试」：
- **Phase 1**（07:35 前）：DSL 解析器（`parse_step_heading` / `parse_tool_call` / `parse_skill_steps`）+ `run_skill_scheduler` 入口
- **Phase 2**（07:35）：变量替换 `${stepN.result}` / `${stepN.id}` / `${prev.result}` / `${prev.id}` 接入
- **Phase 3**（05:30-06:12）：13 个 mock Skill 写入（11 真业务 + 2 样板），覆盖 Q1 复合业务清单
- **Phase 4 第 1 项**（06:20）：`dsl_advance_action` 状态机主循环，每 step 前 `advance_dsl` 决策 5 分支
- **Phase 4 第 2 项**（06:25）：嵌套路径 `${stepN.task.id}` / `${prev.list.0.title}` 支持 + `CompletedStep.parsed` 字段
- **Phase 4 第 3 项**（06:36）：端到端 `run_dsl_loop_sync` helper + 4 个 e2e 测试（Run/Finish/AwaitUser/FailWithRollback/Terminate 5 分支）
- **Phase 4 第 4 项**（07:09）：mock Skill 路径迁移，`dev_skills_dir` / `skill_search_paths` / `scan_skill_dirs` 拆分纯函数
- **Phase 4 第 5 项**（07:20）：LLM 兜底路径，`DslOutcome` / `DslFailure` 枚举 + `format_completed_summary` + bot.rs auto-mode 分支 match outcome 注入 system prompt

### Phase 5（4 项生产化）
- **A. 11 个真业务 Skill 端到端 smoke test**（07:30）：新增 `generic_mock_executor` 覆盖 17 个工具 + `smoke_all_real_skills_run_dsl_loop_with_mock_executor` 扫所有 13 个 mock Skill 跑通
- **B. Windows release 打包**（07:35）：Mac 上 mingw 交叉编译 `x86_64-pc-windows-gnu` + Python zipfile 打绿色包 13.19 MB（wmessage.exe 42.4 MB + WebView2Loader.dll 157 KB），修 `unused_mut` warning
- **C. SKILL_DSL.md 编写文档**（07:52）：8941 字节作者视角实操指南，10 节覆盖（概述 / 目录结构 / frontmatter / 步骤语法 / 变量替换 / 状态机 / LLM 兜底 / 4 个完整示例 / 调试测试 / 10 个 FAQ）
- **D. Skill 监控 UI + SQLite 持久化**（08:00）：`skill_outcomes` 表 + `PersistedSkillOutcome` struct + `persist_outcome_quiet` 在 6 个 return 点 + `SettingsPage` 加 `SkillOutcomeBadge`（4 色对应 4 种状态）

### 关键决策
- **变量替换模式**：单段 `${stepN.result/id}` / `${prev.result/id}` + 嵌套 `${stepN.path.to.field}`，4 种模型 + 嵌套路径
- **数据目录优先**：dev 模式 scan_skills 合并 `target/debug/skills/` + `app_data_dir/skills`，数据目录在前（同 name 数据目录版本覆盖 dev mock）
- **LLM 兜底**：FailedButRecoverable 把 `format_completed_summary(ctx)` 注入 system prompt 决策下一步，Terminated（用户 /stop）不接管
- **持久化策略**：bot_skills 每次 run 完写 SQLite `skill_outcomes` 表，重启后历史可追溯

### 踩坑记录（追加）
- **Python parser 对 box-drawing chars（`─` U+2500）有 SyntaxError**：heredoc 写文件失败。绕道：用 ASCII anchor 或 `─` 逃逸
- **edit 工具对 `${...}` 字面量 JSON 解析有 bug**：被嵌套解析成多层对象。绕道：Python 脚本 str.replace 直接改文件
- **Windows 打包增量编译产物问题**：`cargo build` 直出 exe 报「无法访问此页面」（缺内置页面资源嵌入），必须 `npx tauri build --target x86_64-pc-windows-gnu --no-bundle` 完整流程
- **macOS `zip -j` 的 Unix 扩展字段让 Win 资源管理器解压报「位置不可用」**：用 Python `zipfile` 打绿色包
- **`db::open_db` 是 private**，bot_skills 调用 E0603，加 `pub` 修复

### 测试
- cargo test --lib 从 83（Phase 1 起点）→ 132（Phase 5 完工），0 regression
- 端到端覆盖：13 个 mock Skill 跑 run_dsl_loop_sync + 通用 mock executor 验证 parse + 变量替换 + 状态机 + 嵌套路径

## 2026-08-19（周三）Phase 7 Kimi 五批 + 任务卡多文件绑定改造

### Phase 7 五批（Kimi code 串行 5 跑）
- **7a 审计/日志安全**（P2-1/2/15/16/34/35）：token ct_eq、变更日志 title escape、append_line 不 panic、kv 转义、App.tsx 写路径 catch、errorHandler 空 msg 兜底
- **7b DB 事务/迁移**（P2-4/5/6/7/8/19）：老库拷 -wal/-shm、workspace upsert/delete 事务包裹、claim_dst_name TOCTOU、migrate 触发条件 json_len > db_len、save_rules atomic_write、probe_log_dir 三合一
- **7c API/Middleware**（P2-3/13/14/27/28/30）：SSE 通道载荷事件 id 去重、middleware panic catch_unwind、pre_execute 空注册审计、CommandError platform 字段、TaskInvalidState 变体、opener scope 收敛
- **7d 前端 state bug**（P2-17/20/21/22/23/33）：entry_view 头像 5MB cap、diffTaskRows 统一、mutate 落盘后才写 tasksRef、TrashPage 排序、useInlineEdit hook、WidgetApp 5s 兜底轮询弹 alert
- **7e 资源+跨平台**（P2-24/25/26/29/31/32）：ExitRequested cleanup_on_exit、spawn 失败清理、bring_main_to_front 后台化、Cargo.toml 单一版本源、托盘三平台、Linux secret-service 探测
- 累计 30 commit + 5 docs commit；cargo test --lib 314 → 358 pass（+44），vitest 55 → 85 pass（+30）

### 任务卡多文件绑定改造（上限 10）
- 老板 18:37 反馈：单文件绑定不够用，再选覆盖；老板 18:40 拍板直接 Kimi 做、上限 10 个
- **commit `f6fcc17`** 一改到底：
  - **Schema**：`Task.files: Array<{path, isDir}>`，保留 `filePath`/`fileIsDir` 双写过渡；`open_db` 启动迁移：老单绑定自动回填 files（幂等，老列不清）
  - **Rust**：`bind_files`/`bind_file` 命令（`fs::metadata` 判 `isDir`、去重保序、超 10 截断）；LLM `create_task`/`edit_task` 扩展 `files` 字段（Rust 侧硬上限截断+警告）；`tool.return` 审计补 `files_count`/`truncated` kv
  - **文件操作**：`copy_files_with_title`（多文件复制，命名 `{title}-{basename}`，单文件行为不变）；`openFile` 多文件弹 UI 列表选
  - **UI**：TodoCard/TaskCardContent/WidgetApp 多 chip 列表（📁/📎 + basename + 单独 ×）；超 5 折叠「还有 N 个」；`pickFile` 多选追加去重；文件夹仍单选独占（已绑文件夹弹提示不加不替换）
- 老板拍板撤掉修法 B（模块级 chatBusyRef + collapse() busy 检查）—— 当时是 17:56 挂件折叠 bug fix 的深度防御，老板选保持单一修改面，最终 commit `8c6c9d7`

### 测试
- cargo test --lib: 314（Phase 7 起） → 358（Phase 7 末） → 360+（多文件改造后）；新增迁移 + 多文件 + 上限 + 双写过渡 4 类测试
- vitest: 81 → 85（修复 7c/7d/7e + 挂件折叠修法 A）→ 90+（多文件 chip 列表 + × 移除 + 上限 UI）
- tsc --noEmit 零错
- Windows 绿色包 14:27 已发布（43.97 MB → 13.65 MB 第二次重打）供老板压测

### 晚间会话（Kimi code，20:45–23:10）：绑定互斥解除 + 机器人可靠性三连修 + 逐步执行模式
- **绑定文件/文件夹解除互斥**（老板反馈）：`TodoCard` 删掉「已绑文件夹不能再加文件」拦截；`pickFolder` 改追加不替换；绑定区补「📁 绑定文件夹」入口；文件夹仍单选。前端 100 测试全绿
- **「Command bind_files not found」**：不是代码问题——运行中的 app 是凌晨 00:54 起的旧二进制（`bind_files` 19:10 才编译进去），手动 nohup 起的进程不归 tauri dev 管不会自动重启；重启解决
- **Phase A 架构改造**（docs/ARCH-REFACTOR-PLAN.md 勾选清单）：`MutationOrigin` 枚举消灭 source 字符串约定（bot/api/migration 三处 emit 编译期锁死）；前端 `mutationOrigin.ts` 类型守卫，非法 source WARN + 按未落盘处理。顺手修两个预存问题：middleware 加 AppHandle 首参后 11 处集成测试调用点失配、`error.rs` doctest 伪代码块（标 `rust,ignore`）
- **机器人幻觉汇报三连修**（bot.log 实锤：模型零工具调用却回复「已添加子任务」「已移至回收站 🗑️」）：
  1. prompt 点名工具（add_subtask/toggle_subtask/remove_subtask/delete_task 等，M3 对没点名的工具不可靠）+ 规则 8 禁止无工具调用时声称完成
  2. **幻觉守卫**（`bot_model_loop.rs` 循环出口确定性拦截）：声称变更（「已」+8 字窗口内含变更动词，覆盖「已彻底删除」变体）但本轮 0 次变更工具调用 → 注入系统提醒补一轮（每次对话最多一次），记 `hallucination_guard` 审计
  3. **新增 `remove_subtask` 工具**（此前无删除子任务工具，模型无从下手）；`resolve_task` 加 taskId/标题交叉校验防跨卡张冠李戴
- **子任务显示**：主窗口+挂件子任务列表 `divide-y` 分隔线分行 + 长文本单行截断 hover 显示全文；prompt 约束子任务为 ≤15 字动宾短句
- **子任务逐步执行模式**（老板拍板：一个一个做、确认后勾选、不满意重做、全部做完不勾卡完成、定时执行跳过确认）：新模块 `exec_steps.rs` 挂起-恢复链路，每步从 DB 重读重建上下文；「继续」由系统直接落库勾选（不经 LLM）；bot_chat 入口挂起优先路由；/stop 清挂起。状态内存态，重启丢
- **熔断上限 10 → 30 全局**（老板拍板）：22:56「列计划+新增子任务」复合任务真触发熔断实锤 10 不够；软警告 7 → 20
- 测试：cargo 374 pass（+14：mutation/guard/classify），vitest 100 pass，tsc 零错

### 机器人工具升级 Phase 1/2（docs/BOT-TOOLS-UPGRADE-PLAN.md，全部验收通过）
- **Phase 1 本地文件工具**（把「不开」改成「可控地开」）：
  - 新模块 `bot_fs.rs`：`read_text_file`/`grep_files`/`list_files` 三工具 + `resolve_allowed` 白名单守卫（canonicalize 后 starts_with 判定，堵 `..` 逃逸/软链逃逸）；默认白名单 `~/Desktop`/`~/Downloads`/`~/Documents` + 任务卡绑定文件夹；每次访问记审计
  - `BotConfig` 加 `allowed_dirs: Vec<String>`（空=默认），设置页白名单 textarea；`bot.rs::load_config` 改 pub(crate)
  - 红线从「一律拒绝本地文件」改为「白名单外一律拒绝」，prompt 规则 19 约束模型不得擅自换目录
  - 顺手修：`extract_document` 的 `extract_path_allowed` 也过 bot_fs 白名单（之前 docx 附件在绑定文件夹内仍被拒）
  - 只读工具不进 MUTATING_TOOLS；tool_guard 非原子清单同步纳入
- **Phase 2 搜索/网页工具升级**：
  - `bot_web.rs::extract_main_content`：去 script/style/nav/footer 噪声块 → article/main → 语义 class/id 最大 div → 兜底全文；结果 <100 字自动回退整页转换；30KB 截断保留
  - `web_search` 结果清理（空白压缩 + 结尾省略号去除）+ 同域名最多 2 条（百度跳转豁免）+ 来源标注 [Bing]/[百度]/[Tavily]
  - `web_search_with_config(app, query)`：配了 `tavilyKey` 走 Tavily API，失败回退抓取记审计 `web_search.tavily_fallback`；设置页加 Tavily key 输入框（生效需老板申请 key）
- 测试：cargo 377（Phase 1）→ 382（Phase 2）pass，vitest 100 pass，tsc 零错；两阶段手动冒烟均老板验收通过

## 2026-08-20（周四）凌晨：Phase 3 模型可切换 + 聊天窗模型快速切换

### Phase 3（docs/BOT-TOOLS-UPGRADE-PLAN.md，全部验收通过）
- **3.2 兼容性代码核对**：payload（model/messages/tools/stream + Bearer）、SSE 解析（`delta.content` / `delta.tool_calls` 增量拼接）、tool 回填（assistant.tool_calls + role:tool + tool_call_id）全是标准 OpenAI 规范，Kimi/DeepSeek 官方兼容，零代码改动
- **3.3 提供商预设**：`src/lib/providerPresets.ts` 共享常量（设置页按钮组 + 聊天窗菜单共用）；命中判定 baseUrl+model 双匹配（DeepSeek Flash/Pro 同 baseUrl，单靠地址无法区分）
- **聊天窗模型快速切换**（老板提的新需求）：ChatPanel 头部 🧠 按钮（显示当前提供商 label / 自定义显示模型名）→ 菜单选预设即切换，整体回写配置保留白名单/Tavily 字段，apiKey 传 null 不动 keychain；设置页保存广播 `bot-config-changed` 同步刷新标签
- **预设跟随官方更新**：Kimi K2 下线 → K3（`kimi-k3`）；DeepSeek 旧名 `deepseek-chat` 2026-07-24 已废弃 → V4 双档（`deepseek-v4-flash` / `deepseek-v4-pro`），Rust 默认值同步换 v4-flash；菜单加「✏️ 自定义…」内联表单自由填任何 OpenAI 兼容端点
- **踩坑**：切 Kimi 报 401 Invalid Authentication —— 不是代码问题，keychain 里存的还是旧 MiniMax key（设置页 API Key 留空保存 = 保持原 key）；教训：跨提供商切换必须同步换 key，菜单底部已加提示文案
- 测试：cargo 382 pass、vitest 103 pass（+3：设置页预设、聊天窗预设切换、自定义表单）、tsc 零错；Kimi K3 幻觉场景冒烟老板验收通过

### Phase 4 低成本高价值工具（03:05，全部验收通过）
- **`get_current_time`**：返回「现在：yyyy-MM-dd HH:mm:ss 星期X」；prompt 规则 20 强制模型做相对日期判断前先调，杜绝凭训练数据猜日期
- **长期记忆**（`bot_facts` 表：key 主键 upsert / 空 value 删除 / 200 条上限）：
  - 模型主动式设计——不每轮自动注入全量记忆（防白烧 token），靠 prompt 规则 21 引导该回忆时调 `recall_facts`；观察点：模型遵循度不够则下一步改系统提示注入「已有 N 条记忆」提示
  - DB 操作抽成 `fact_upsert`/`fact_delete`/`fact_list`（接受 `&Connection`），内存库单测覆盖全逻辑（open_db 全链路依赖 AppHandle，同 bot_fs 先例不测）
  - `remember_fact` 计入 MUTATING_TOOLS：幻觉守卫覆盖「已记住」话术；`validate_fact_kv` 纯函数管入参边界（key ≤50 字 / value ≤500 字）
- 测试：cargo 382 → 387（+5），vitest 103，tsc 零错；老板冒烟：「记住我喜欢简洁的回复」→ 新会话问偏好，跨会话回忆成功

### 工具对比 Kimi Code 后的四项改进（05:10）
- **create_word 加 tables 参数**：可选表格列表 [{title?, rows}] 追加在段落之后，首行表头加粗（python-docx Table Grid）；schema + doc_make_word 透传（Option<Value> 向后兼容）
- **create_excel 多 sheet 核实无需改**：schema（sheets: [{name, rows}]）+ 脚本循环 + tool 透传早已支持，此前对比误判
- **run_python 超时可配**：三层优先级——工具参数 timeoutSecs（schema 新增，模型按需调）> 设置页「Python 默认超时」（BotConfig.python_timeout_secs）> 内置 60s；硬钳 300s 沿用 resolve_timeout；修复 ChatPanel 模型切换回写丢 pythonTimeoutSecs 的隐患（回写字段补齐）
- **fetch_url Jina Reader 回退**：正文提取 <100 字（JS 渲染 SPA 空壳）时回退 https://r.jina.ai/<url>（服务端渲染返回 markdown，免费无 key）；过 check_public_url + 2MB 上限；失败静默不影响主路径
- **textutil 方案否决**（老板问）：macOS 私有、只覆盖 Word、Windows 无等价物；现 Python 子进程方案跨平台四格式通吃，保留；远期可迁 Rust 原生解析（calamine/lopdf/解 zip）干掉 Python 依赖，单独立项
- 踩坑：TOOLS schema 手写 JSON 嵌套括号错一位（tables items 多关一层），tools_schema_parses 测试当场抓住——这个测试就是干这个的
- 测试：cargo 387 → 388（+1 jina_reader_url），vitest 103，tsc 零错
- **create_ppt customColors**（05:20，老板拍板「骨架固定、皮肤开放」）：可选 {bg/accent/text/sub/band/bandtext/alt} 6 位 hex 覆盖主题配色，脚本侧正则校验非法值忽略保底；版式骨架仍固定（信息架构不开放）
- **extract_document 分页读**（05:20）：30K 硬截断 → offset/limit 字符级分页（默认 30000、硬钳 60000），头部 [位置] offset–end/共 N 字符 + 尾部续读提示；offset 越界明确提示。prompt 规则 10 截断标记措辞同步更新
- **extract_document 代码审查结论**（老板问「有没有问题」）：无功能性 bug；三个已知局限——docx 表格抽在段落后（顺序失真）、xlsx data_only 对未保存过的公式读出空、pptx 漏表格内容
- 测试：cargo 388 → 390（+2 分页测试，jina +1 上一批），vitest 103，tsc 零错
- **extract_document pptx 表格漏读修复**（05:26）：walk 递归组合形状（shape_type 6=GROUP，须先判组再碰 has_table——GroupShape 无该属性），GraphicFrame 表格按行输出「单元格 | 分隔」；本机实测：文本框+表格均提取成功

### 架构改造 Phase A-C + 技能路由动态化（晚间，Kimi Code 执行）
- **Phase A（source 标记类型化）**：`mutation.rs` `MutationOrigin` 枚举取代 `"bot"/"api"/"migration"` 裸字符串；前端 `lib/mutationOrigin.ts` 类型守卫，非法值 WARN 不静默吞
- **Phase B（共享常量单一来源）**：新 `consts.rs` `app_consts` 命令下发 `max_task_files/max_title/max_note/image_exts`；前端 `lib/consts.ts` 启动拉取 + 失败回退硬编码；`MAX_TASK_FILES` 改活绑定 re-export（调用方零改动），ChatPanel 图片扩展名走 `imageExtSet()`
- **Phase C（bot_skills.rs 3091 行拆分）**：纯移动零逻辑改动 → `bot_skills/{mod,files,manage,parse,state,vars,runtime,scheduler}.rs`，全部 ≤1200 行；mod.rs 只留 re-export；仅 7 处可见性放宽到 pub(crate)；cargo 421 测试前后一致
- **技能路由动态化（老板拍板：未安装的技能不得有路由）**：
  - 删除静态 `INTENT_RULES` 7 条硬编码映射——3 条幻影（web-search/task-summary/archive 实体从未存在，功能均有内置替代）、ppt-orchestra-skill 实体与本运行时不兼容（subagent/node compile.js）且能力已内置 SYSTEM_PROMPT 规则 10
  - `intent_router` 重写：路由表 = 已安装技能 SKILL.md frontmatter `intents` 声明；启动 + `skills_import`/`skills_delete` 成功后 `rebuild_routes` 重建；空表恒 PassThrough（fail-open）
  - `parse_meta` intents 支持多行 YAML 列表（单行逗号分隔会切碎 `{0,15}` 量词）；`manage.rs` 新增 `intent_rules_from_dirs`（enabled=false / 无 intents 不产生路由）
  - minimax-docx 实体安装到 dev（target/debug/skills）+ release（App Support）两处，frontmatter 补 intents（含「润色/修订 + .doc/.docx 附件」上下文模式 `(?is)...[\s\S]*\.docx?`——附件以 [附件文件] 块嵌消息文本，无需改 middleware 签名）
  - 冒烟测试语义修正：parse/scheduler 两个扫真 skills 目录的 smoke 跳过非 auto 技能（interactive 技能无 DSL Step 是合法状态，不是解析失败）
  - tests-audit 审计脚本修复：BOT_SKILLS 改读拆分后目录；8 条编排层断言跟 F-6 拆分搬家到 BOT_ALL（bot.rs+bot_chat.rs+bot_model_loop.rs）；cargo_test_count 环境变量缺 HOME 导致恒 -1 的预存 bug 一并修
  - 遗留：xlsx/pdf 技能实体在 ~/.openclaw/workspace/skills 但未安装（要用就装，不用路由自然没有）；「机器人对话里安装技能」入口当前不存在，规则实现后任何安装入口自动生效
- 验证：cargo 421 全绿（lib 392 + 集成 29）、cargo check --release 通过、vitest 105、tsc 零错、pytest 审计 24 过 1 跳

## 2026-09-04（周五）Windows 两处修复：绑定文件打不开 + 多开实例

- **任务卡/工作区点绑定文件（夹）打不开**（老板报 bug）：根因是前端 `openPath`（plugin-opener）受 opener scope 限（capabilities 仅放行 `$HOME/**`、`$APPDATA/**`），Windows 上绑定 D:\ 等非用户目录路径被静默拒绝（catch 是 silent），表现为点击无反应。修复：TodoCard `openOneFile`、WorkspacePage `openLink` 统一改走 Rust 侧 `open_file_path`（与挂件窗口既有方案一致，不受 webview scope 限）；SEC-P1-3 白名单本就含任务卡绑定文件 + 工作区链接，安全边界不变
- **Windows 多次双击 exe 开出多个前端**：引入 `tauri-plugin-single-instance`（Builder 第一个注册，插件要求），二次启动回调 `bring_main_to_front` 唤起已有主窗口后自行退出；顺带消除多实例并发写 SQLite 的隐患
- 验证：vitest 20 文件 182 全过、tsc 零错、cargo check 通过

### 按 AUDIT-API-2026-09-03 修复（P1×2 + P2×9 全清，不换栈沿用 tiny_http）

- **P1-1 accept 循环不再同步等 worker**（api_server.rs）：删 done_tx/done_rx + `recv_timeout(15s)`，`on_error` 包 Arc 传入 worker 自行记 panic 日志；accept spawn 后即回 recv，`api_stop` join 最坏只等 accept 的 400ms tick，不再卡 15s 持锁
- **P1-2 body 滴注 slowloris**（api_handlers.rs `read_body_limited`）：新增 35s 总时长 deadline（8KB 分块读循环内查总时长，vendor 30s 只是单次 read 级管不住滴注）+ Content-Length 预拒超限 body；vendor/tiny_http patch 注释里「与 15s handler 超时语义协调」的不实说法改正（只改注释未动逻辑）
- **P2-1** `api_start` 对已死服务误报成功：先做 `api_status` 同款 `is_finished()` 活性检查 + 尸体清理再重启
- **P2-2** token 文件：`OpenOptions` + `.mode(0o600)` 创建即收紧（unix），消 write→chmod 窗口；覆写/读取存量 0644 文件时补收紧
- **P2-3** 事件 id 落盘换 `db::atomic_write`（tmp+rename），防崩溃半截文件导致重启 id 归 0、客户端 Last-Event-ID 静默丢事件
- **P2-4** T1-1 基线空洞：`db::BASELINE_NULL_ROW = i64::MIN` 哨兵，updated_at 为 NULL 的老行用「行存在性」作基线（被删/被改都 409）；`MemStore::upsert` 补齐基线比对（原先忽略，409 不可测）
- **P2-5** `api_rotate_token` 全程持 `state.0` 锁（拆出 `api_start_locked`/`api_stop_locked`），消「检查→stop→start」间并发 stop 被重新拉起的窗口
- **P2-6** body 读取错误分流：`BodyRead::{Ok,TooLarge,IoFailed}`，读 IO 错误（含超时）回 408，只有超 1MB 回 413
- **P2-7** SSE `?since=` 非法回 400 不再静默当全新连接；环形缓冲 1000 条溢出缺段在 EVENT_HISTORY/sse_connect 注释注明
- **P2-8** `filePath` 补 `API_MAX_FILE_PATH = 1024` 上限（与 title/note 同款 over_limit，超限 400）
- **P2-9** `api_status` 未启用时不再 `load_or_create_token`，不生成 token 文件
- 新增测试 7 条：token 收紧、NULL 行存在性基线三态、409 集成（SabotageStore 模拟插队写，覆盖两种基线）、since 非法 400、filePath 超限 400、Content-Length 预拒
- 验证：cargo test --lib 490 → 497 全绿；cargo check 无新警告；既有 llm_integration 27 + skill_e2e 13 无回归
- 遗留（随换 axum 立项）：header 阶段滴注仍只有单次 read 级 30s 超时；Content-Length 预拒后 keep-alive 连接可能残留未读 body（本地短连接 API，可接受）

### 子任务交互改版（老板 2026-09-04）
- 子任务可编辑：拆出 `SubtaskRow` 组件（每行独立 editing 状态），点击文本进内联编辑，复用 `useInlineEdit`（Enter 提交 / Esc 取消 / blur 提交），空提交保留原文；归档/回收站只读
- 子任务全文显示：去掉 truncate 单行截断，改 `whitespace-pre-wrap break-words` 多行完整显示（checkbox 改 items-start 对齐首行）
- 行间分割线淡化：`divide-[var(--edge)]` → `color-mix(in srgb, var(--edge), transparent 55%)` 半透明
- 验证：TodoCard 测试 21 → 25（新增 4 条：不截断/编辑提交/Esc 取消+空提交/归档只读）、tsc 零错、vite build 过（确认 Tailwind arbitrary class 正确生成 color-mix 规则）
- 未动：挂件 TaskCardContent 子任务仍是只读 + 单行截断（挂件窄卡片场景，未提需求）
