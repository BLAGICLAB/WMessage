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
- `docs/module-audit-2026-10-22.md` — 分模块代码审计账本（15 模块 / 6 批；批 1 已出 M5 五条 P1）——与实机卡片互补：卡片验「行为对不对」，审计查「闸漏不漏」
- `docs/realmachine-overflow-test.md` — 实机验收方案：工具大结果溢出回灌（边界三连 / 指针读回 / 保留期清扫 / 转存失败兜底）
- `docs/SUBAGENT-HANDOFF-BUDGET-REDESIGN-2026-10-23.md` — 子 agent ↔ 主 agent 衔接与墙钟治理改造方案（预算单一钳制 / 软墙钟收尾窗 / 熔断留产 / 收尾契约三段修复 / digest 补渲染）；含业界口径、不做清单、偏离登记

- `docs/SUBAGENT-EFFICIENCY-TRANSPARENCY-DESIGN-2026-10-24.md` — 子 agent 效率与透明度改造（已落地）：小文件一次读完红线 / 预算触顶收尾窗只拒读放行写 / read_own_card 实时余量 / 右侧栏动作流实时上屏 / usage 三列记账；含实机账（PPT 素材稿 100 调用零产出）与实施偏离登记
- `docs/MCP-ON-DEMAND-LIFECYCLE-DESIGN-2026-10-24.md` — MCP 外部服务器按需启停方案（静默待命 / 调用期拉起 / 调用完回收 / 工具清单落盘解耦可见性）+ 业界口径调研（VS Code autostart、mcp-proxy lazy、rho lazy）+ 踩坑记录
- `docs/SKILL-ENABLE-TOGGLE-DESIGN-2026-10-24.md` — 技能启用开关方案（拨杆 UI / 状态写技能自身 frontmatter 的 enabled 键 / 关掉即三闸全拦 / 内置技能跨版本保留禁用态）+ 业界口径调研（Claude Code 的 frontmatter 属性与 skillOverrides、VS Code 1.109 同源）+ 踩坑记录
- `docs/AGENT-REMEDIATION-PLAN-2026-10-24.md` — Agent 评分整改方案（2026-10-24 评分 8.2/10 的 13 项 → 12 批次 + 1 拍板项）：eval 层 / 巨型函数拆分 / 溢出漏斗存活变异补测 / 实机脚本进仓 / 审计 API 与双路径收敛 / MCP 审计口径 / 留底加固 / Windows 验证口径 / 模块审计补批；含不做清单、逐批验收与偏离登记
## 命令

- 快检：`bash scripts/test-fast.sh`（pre-commit 自动跑，按改动智能跳过）
- 全量：`bash scripts/test-all.sh`（pre-push 自动跑：cargo nextest + vitest）
- Rust 单测：`cd src-tauri && cargo test --lib`；lint：`cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`（**已在快门禁内**；clippy 是唯一能发现「测试压根没被收集运行」的信号，没装 = 门禁失败，不静默 skip）
- 前端：`npm test`；lint：`npx oxlint src`；类型：`npx tsc --noEmit`

## 硬性禁令（门禁会拦，人也退）

- 注释不带工单号/日期决策/拍板记录（`W8-ATTACH`、`OCR C5`、`B0-1`、`AUDIT-*`、「拍板」）——这些只进 DEVLOG；不画横幅分隔线。注释只写代码说不出的约束。
- 不吞错误：禁空 catch、禁 `unwrap()`/`expect()` 绕错、禁 `as any` 逃逸类型——要么处理要么上抛。
- 不留死代码：本次改动孤儿化的函数/导入/依赖随同一提交删除；未要求的无关遗留不顺手重构。
- 不加投机抽象：trait/中间件/config 开关等，第二个调用方出现前不抽象（YAGNI）。
- 密钥只走系统 keyring / 环境变量，不进源码、不进仓库。

## 工作方式

- 动手前工作区是干净 commit（checkpoint）；小步提交，一个逻辑变更一个 commit。
- 行为变更换测试；修 bug 先写复现测试再修。
- 新增依赖先给一行理由：stdlib / 现有依赖为何不行。
- 提交前人眼看一遍 diff——门禁只懂规则，不懂意图。
- **不主动出 Windows 包**：修完先让用户在现有环境过一遍，有问题他们直接反馈；出包只在明确要求时做。出包不只是几分钟的事，换 exe 还牵连 SHA256 与路径两处记账（写进手册后很快又会过期）。
