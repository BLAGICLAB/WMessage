# AGENTS.md — AI 代理工作约定

给在本仓库工作的编码代理。基线全文见 `SPEC.md`「开发基线」，本文件只放
每次干活都要的第一屏；改了门禁或约定时同步这里（本文件过长即失职）。

## 地图

- `docs/testing.md` — 门禁清单、失败修法、豁免方式
- `docs/rust-bot-architecture.md` — 模块树，现行架构以此为准
- `SPEC.md` — 现行产品规格（功能规格全景）+ 开发基线（准绳）；功能变更随提交更新它
- `DEVLOG.md` — 只写「为什么换方向」；git log 是事实记录
- `docs/archive/graph-audit-2026-10-19.md` — 图谱多角度审计报告（发现项已于 2026-10-21 全部处理，归档备查）
- `docs/agent-audit-2026-10-19.md` — Agent 内核审计的发现清单、已修状态与未修待办
- `docs/agent-audit-2026-10-21.md` — Agent 全量多角度审计报告（Skill/MCP/工具/服务/子 agent/痕迹/契约/门禁七链；1 P0 + 15 P1 + 7 P2，修复按 §六 优先级推进）
- `docs/realmachine-regression-test.md` — 实机变异测试方案：34 项双平台场景卡 + 9 项 Windows 专章（提示词 + 判据 + 变异注入复验），覆盖 2026-10-19 ~ 10-22 这批改动的跨模块接缝；探针 `scripts/regression-probe.sh`（unix）/ `.ps1`（Windows）
- `docs/realmachine-rerun-2026-10-22.md` — 复验手册：换 exe → 先跑修过的那 7 项 → 再跑 S 序列剩余项（含跑法、分段顺序、结果回填口径）
- `docs/realmachine-mutation-test-2026-10-23.md` — 实机变异测试方案：39 项双平台场景卡 + 3 项 Windows 专章，覆盖 2026-10-23 这批 68 个提交（提示词 + 判据 + 变异注入锚点）；实机发现清单在附录 D（D-1 自窗口崩溃 + D-2 聚焦失效 + D-3 密码框敏感闸 + D-4 深度钳制，均已修并实机复验）；已纳入 `audit_doc_claims.py` 对拍门禁
- `docs/realmachine-mutation-test-2026-10-24.md` — 实机变异测试方案：35 项双平台场景卡（M40–M74）+ 3 项 Windows 专章（W04–W06），覆盖 2026-10-23 20:02 → 10-24 10:51 这批 51 个提交（提示词 + 判据 + 变异注入锚点 + 审计事件名总表）；变异编号 V26–V75 接上一份的 V01–V25；已纳入 `audit_doc_claims.py` 对拍门禁
- `docs/realmachine-smoke-test-2026-10-25.md` — 实机冒烟测试方案：18 项主卡（M75–M92）+ 3 项 Windows 专章（W10–W12），覆盖 2026-10-24 10:51 → 10-25 15:36 这批 116 个提交（提示词 + 判据 + 变异注入锚点 V76–V96 + 事件名总表 + 未覆盖诚实清单）；主卡含 P1–P8 八项上下文/工具调用优化的逐条验收；66 个 refactor 由纯搬移核对器兜底、可见性与接线落到两张抽样卡；已纳入 `audit_doc_claims.py` 对拍门禁
- `docs/module-audit-2026-10-22.md` — 分模块代码审计账本（15 模块 / 6 批；批 1 已出 M5 五条 P1）——与实机卡片互补：卡片验「行为对不对」，审计查「闸漏不漏」
- `docs/realmachine-overflow-test.md` — 实机验收方案：工具大结果溢出回灌（边界三连 / 指针读回 / 保留期清扫 / 转存失败兜底）
- `docs/SUBAGENT-HANDOFF-BUDGET-REDESIGN-2026-10-23.md` — 子 agent ↔ 主 agent 衔接与墙钟治理改造方案（预算单一钳制 / 软墙钟收尾窗 / 熔断留产 / 收尾契约三段修复 / digest 补渲染）；含业界口径、不做清单、偏离登记；**其墙钟部分已被 2026-10-28 预算简化方案取代**（全链路撤墙钟，见下方 AGENT-BUDGET-SIMPLIFICATION 条目）

- `docs/SUBAGENT-EFFICIENCY-TRANSPARENCY-DESIGN-2026-10-24.md` — 子 agent 效率与透明度改造（已落地）：小文件一次读完红线 / 预算触顶收尾窗只拒读放行写 / read_own_card 实时余量 / 右侧栏动作流实时上屏 / usage 三列记账；含实机账（PPT 素材稿 100 调用零产出）与实施偏离登记
- `docs/MCP-ON-DEMAND-LIFECYCLE-DESIGN-2026-10-24.md` — MCP 外部服务器按需启停方案（静默待命 / 调用期拉起 / 调用完回收 / 工具清单落盘解耦可见性）+ 业界口径调研（VS Code autostart、mcp-proxy lazy、rho lazy）+ 踩坑记录
- `docs/SKILL-ENABLE-TOGGLE-DESIGN-2026-10-24.md` — 技能启用开关方案（拨杆 UI / 状态写技能自身 frontmatter 的 enabled 键 / 关掉即三闸全拦 / 内置技能跨版本保留禁用态）+ 业界口径调研（Claude Code 的 frontmatter 属性与 skillOverrides、VS Code 1.109 同源）+ 踩坑记录
- `docs/AGENT-REMEDIATION-PLAN-2026-10-24.md` — Agent 评分整改方案（2026-10-24 评分 8.2/10 的 13 项 → 12 批次 + 1 拍板项）：eval 层 / 巨型函数拆分 / 溢出漏斗存活变异补测 / 实机脚本进仓 / 审计 API 与双路径收敛 / MCP 审计口径 / 留底加固 / Windows 验证口径 / 模块审计补批；含不做清单、逐批验收与偏离登记
- `docs/FILE-SPLIT-PLAN-2026-10-25.md` — 18 个超限大文件的拆分计划（12 批）：A 类测试外移 8 条 / B 类生产切分 7 条 / C 类特殊形态 3 条；含实测基线、三条门禁约束（行数棘轮 / 模块地图 / 命名空间）、冻结守卫与 `pub(super)` 下移陷阱、并行 worktree 策略与逐批收尾清单
- `docs/FILE-SPLIT-ROUND2-2026-10-25.md` — 第二轮：贴线（1300–1500）的 10 个文件拆分计划——6 条测试外移 / 2 条纯测试文件按节切 / 2 条生产切分；含逐文件切法、执行顺序依赖与本轮特有的四个坑
- `docs/FILE-SPLIT-FRONTEND-2026-10-25.md` — 第三轮（前端，**已执行完毕**）：5 个大文件（SettingsPage 4068 / ChatPanel.test 3105 / SettingsPage.test 2985 / ChatPanel 2270 / GraphCanvas 1245）的逐文件切法——A 类纯搬移 / B 类搬移+接线六条白名单 / 前端版核对器规格 / effect 顺序与 mock 作用域等十个坑 / 可粘贴的提示词；§十二 进度回填与 §十三 落地结果（含偏离登记与「exhaustive-deps 认作用域不认值」等五条教训）
- `docs/CONTEXT-TOOLCALL-OPTIMIZATION-DESIGN-2026-10-25.md` — 上下文成本与工具调用优化方案（8 项 P1–P8，5 个批次）：缓存前缀解封 / 截断滞回 / `lookup_receipt` 召回工具 / 并行调用引导 / schema 示例 / 思考段回灌开关 / 轮内工具结果滑动窗；含业界实证口径（Anthropic PTC -37%、Context Editing +29%）、不做清单与逐项验收口径
- `docs/AGENT-BUDGET-SIMPLIFICATION-DESIGN-2026-10-28.md` — Agent 预算与风控简化方案（**已执行完毕**，7 批 + 批 7 两段，2026-10-29 收官）：取代「三档 × 六预算 × 60/20/15 + 墙钟」模型——无墙钟 / 无档位 / 子 agent 无独立预算（停滞超时 600s 接替）/ 安全网常量 200 轮 400 工具主子共用 / 回填 128KB 固定 / 参数卡退役、参数表缩三行 / 回灌「发了就回」；§10 八条拍板记录与业界口径调研（Claude Code / Cursor / Gemini CLI / OpenAI SDK / LangGraph / AutoGen / Devin）在录
- `docs/BOT-EVENT-ENVELOPE-DESIGN-2026-10-27.md` — 流式事件信封方案：会话号只在统一出口盖章（`emit_recorded` 唯一盖章处，调用方不许自带同名键）+ 计划状态外置到模块级 store；含业界口径调研（ACP / CloudEvents / Kafka headers / 行业 AI 流式）、根因链、踩坑记录与落地结果表
- `docs/ASK-USER-CHAT-MULTICHOICE-RESEARCH-2026-10-30.md` — ask_user 聊天放开 + 选择题提问调研（**方案 B 已落地**，拍板七项全按推荐）：五家大厂口径（Claude Code AskUserQuestion / Codex request_user_input / Cursor / Cline-Roo / Kiro）+ MCP elicitation + 现状盘点（聊天曾被 schema 描述与引擎双闸挡死）+ 三方案对比；§十一 落地结果（bot-ask/bot-ask-resolved 事件、AskReply 动作随行、useSyncExternalStore 空态引用坑）
- `docs/RUN-PYTHON-NETWORK-POLICY-2026-10-30.md` — run_python 联网口径方案（**方向已定为「不联网」，未落地**）：初稿主张「可联网 + 联网优先专用工具」被三处硬伤推翻（优先序是弱约束且自开「批量请求」口子 / schema 里的「可联网」是许可证不是披露 / 失败模式不对称且定时作业无人值守）；必要性论证（bot 需要联网 ≠ run_python 需要联网）+ **子 agent 三段式分工这条硬依据**（联网本属 recon 阶段，coder/executor 无联网工具，`run_python` 在执行阶段开的是暗道）+ OCR 隐私红线与脚本联网不能同时成立；6 条拍板（不联网 / 联网能力收进专用工具 / 目的地=白名单+白名单外按次弹窗 / apps 契约改交付约定 / 超时统一 120s / 技术闸另立项目）；**三处覆盖硬要求**（主聊天提示词 + 子 agent profile + 工具描述，只改主聊天则暗道留着）；能力前置 C1–C4（POST/JSON/鉴权头、下载到产物目录、目的地白名单与按次授权、工具形态待拍板）+ 五批提交切分 + 五个锚点测试；超时漂移共 7 处（含 `SCHEMA_AST_INSPECT` 这处模型直接读的文案）
- `docs/HTTP-REQUEST-TOOL-DESIGN-2026-10-30.md` — 专用 HTTP 工具与目的地机制方案（**方案已出，未落地**，是上一份「断网」方案的能力前置 C1–C4）：新起 `http_request` 工具（方法/请求体/自定义头/下载落产物目录，`fetch_url` 保持读正文语义不动）+ 目的地闸（产品自有出口 vs 用户目的地分层；用户目的地走 `allowed_domains` 白名单 + 白名单外按次弹窗、仅本次生效、复用 strict/ask/yolo 三档）；含凭据处理硬要求（不回显/不进审计/不进产物）+ 代码落点（`bot_web/destination.rs`、`bot_web/http.rs`、`bot/tools/http.rs`）+ 审计事件 + 五批切分 + 七个锚点测试；**待拍板 D1：`fetch_url` 是否也纳入目的地闸**（推荐纳入，否则 GET 查询串外传是已知缺口）
- `docs/TOOL-EXECUTION-FACT-SHEET-2026-10-30.md` — 架构事实表（**现状盘点，非方案**）：65 个工具逐条的跑位 / 数据边界 / 闸 / 强制层（13 组）+ 闸序（dispatch 的 8 级：tool.call → L4 pre_tool 钩子 → 技能 allowed-tools → per-tool 规则表 → skill 熔断 → 子 agent 白名单 → 入参 JSON → 工具本体闸）+ py 沙箱能力矩阵（管资源与生命周期、不管能力）+ **强制缺口清单**（按「能力面 × 强制缺口」排：start_service 与 MCP stdio 排在 run_python 前面）+ 三条结论（本仓尚无能力维度沙箱 / 闸序集中但边界分散是口径漂移的结构性来源 / 两处 fail-open）；含维护规则（改工具表、闸序、跑位、闸、沙箱限额任一项须同步本文）
- `docs/ACCEPTANCE-PLAN-2026-10-30.md` — 整体验收工作方案（**现行**）：四层验收模型（门禁 / 单测 / 实机 / 发布）+ SPEC 全部 114 条功能的追踪矩阵（`docs/acceptance/matrix.csv`）+ 逐条验收卡（`docs/acceptance/cards-*.md`，每功能一张）+ 事件名白名单（`docs/acceptance/event-names.md`）+ Windows 专章（`cards-90-windows.md`）；门禁 `tests-audit/audit_acceptance_matrix.py` 对拍 SPEC ↔ 矩阵 ↔ 卡片（改了 SPEC 功能条目必须同步矩阵与卡）
- `docs/AGENT-SANDBOX-NETWORK-RESEARCH-2026-10-30.md` — 大厂代码执行类工具的联网与安全边界调研（**方案已出，未落地**）：控制点分层（执行环境出网/目的地策略/运行期授权/凭据信任域）＋四家官方文档对照（Claude Code 沙箱默认关、Codex permission profiles、Gemini profile 矩阵与沙箱扩张请求、VS Code 企业托管设置）＋失效证据（Ona 三层绕过链、PromptArmor 注入、Cursor YOLO）＋沙箱基础设施网络策略语义（allow 恒优先 deny、白名单须配兜底拒绝、只控 TCP、缺 sidecar 时静默失效、凭据代持注入）＋能力分离与出网 DLP；含一手/二手证据分层与时效边界
- `docs/RUN-PYTHON-SECURITY-REFIT-PLAN-2026-10-31.md` — run_python 安全改造与整理能力整合方案（**现行施工总纲，未落地**；整合上游四文档并修订之，冲突处以本文为准）：三处上游修订（回灌定死 30K 切片/目的地闸重定向每跳都判/凭据三小件——set-cookie 脱敏、CRLF 靠 reqwest 类型解析、`args_preview` 对 http_request 裁成 method+host）＋六项新增（授权粒度=host×会话防审批疲劳终点 yolo/macOS seatbelt 出网硬闸提级——deny 出入站、自证式探测、fail-closed 无放行开关、apps 同享乘数/`keyring:` 前缀凭据引用一期做/无人值守快速拒/子 agent 白名单外直接拒不弹窗/MCP 出口并入判定表）＋整理能力四接缝（saveAs 直读/产出收口引导/64KB 大清单引导/档位 research+general）＋拍板项 B1–B9＋七批切分与 23 条锚点测试；D1（fetch_url 接闸）定缓、SPEC 记缺口

## 命令

- 快检：`bash scripts/test-fast.sh`（pre-commit 自动跑，按改动智能跳过）
- 全量：`bash scripts/test-all.sh`（pre-push 自动跑：cargo nextest + vitest）
- Rust 单测：`cd src-tauri && cargo test --lib`；lint：`cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`（**已在快门禁内**；clippy 是唯一能发现「测试压根没被收集运行」的信号，没装 = 门禁失败，不静默 skip）
- 前端：`npm test`；lint：`npx oxlint src`；类型：`npx tsc --noEmit`
- 文件行数：`python3 -m pytest tests-audit/audit_file_size.py -q`（已在全量门禁内；**前后端统一口径**——扫描 `src-tauri/` 的 `.rs` 与仓库根 `src/` 的 `.ts/.tsx`，上限同为 1500）
- 验收对拍：`python3 -m pytest tests-audit/audit_acceptance_matrix.py -q`（已在全量门禁内；查 SPEC 功能条目 ↔ 追踪矩阵 ↔ 验收卡三方是否还对得住——**改 SPEC 功能条目时必须同步矩阵与卡**）
- 拆大文件后核「纯搬移」：Rust 用 `python3 scripts/verify_pure_move.py <原文件> --manifest <清单> --base-dir <目录>`（清单每行 `<相对路径>:<起>-<止>`，行号指原文件；`--strip-prefix 'pub(super) '` 声明可见性改写，`--allow-chunk` 声明已知获准改动）；前端用 `python3 scripts/verify_pure_move_tsx.py <原文件> --manifest <清单> --base-dir <目录>`（逐行 lstrip 后必须完全相同；前端无格式化器，比 Rust 版更硬）

## 硬性禁令（门禁会拦，人也退）

- 注释不带工单号/日期决策/拍板记录（`W8-ATTACH`、`OCR C5`、`B0-1`、`AUDIT-*`、「拍板」）——这些只进 DEVLOG；不画横幅分隔线。注释只写代码说不出的约束。
- 不吞错误：禁空 catch、禁 `unwrap()`/`expect()` 绕错、禁 `as any` 逃逸类型——要么处理要么上抛。
- 不留死代码：本次改动孤儿化的函数/导入/依赖随同一提交删除；未要求的无关遗留不顺手重构。
- 不加投机抽象：trait/中间件/config 开关等，第二个调用方出现前不抽象（YAGNI）。
- 新文件 ≤1500 行：超了先拆子模块；存量超限文件的白名单（`tests-audit/audit_file_size.py` 的 `WHITELIST`）已清空——现在任何文件超 1500 行都当场 fail（真出现存量超限文件时再登记，只许变小不许变大）。
- 密钥只走系统 keyring / 环境变量，不进源码、不进仓库。

## 工作方式

- 动手前工作区是干净 commit（checkpoint）；小步提交，一个逻辑变更一个 commit。
- 行为变更换测试；修 bug 先写复现测试再修。
- 新增依赖先给一行理由：stdlib / 现有依赖为何不行。
- 提交前人眼看一遍 diff——门禁只懂规则，不懂意图。
- **不主动出 Windows 包**：修完先让用户在现有环境过一遍，有问题他们直接反馈；出包只在明确要求时做。出包不只是几分钟的事，换 exe 还牵连 SHA256 与路径两处记账（写进手册后很快又会过期）。
