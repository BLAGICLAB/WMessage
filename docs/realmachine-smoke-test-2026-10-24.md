# 真机冒烟测试报告：2026-10-24

对 `8887e50e`（10-24 10:51，工作区干净）这一版在 macOS 实机做的一轮**冒烟**：只回答「这版能不能正常启动、核心链路通不通」，不做 38 卡全量、不做变异注入（本轮口径：不改代码）。

**结论：全绿，0 发现项。** 启动、迁移、界面渲染、真实 LLM 往返、退出清理共 14 项检查全部 PASS；1 条测试手段伪影（非应用问题）已查明并记录；3 个面留给全量卡。

| 项 | 值 |
|---|---|
| 提交 | `8887e50e` docs(realmachine)（工作区 clean） |
| 构建 | `npm run tauri dev`（dev 必须：诊断走 stderr），21s 编译，运行于 `target/debug/wmessage` |
| 数据目录 | `~/Library/Application Support/com.renshi.wmessage`（dev 下 exe 目录写探针失败 → 兜底，见「观察」①） |
| 观测窗口 | bot.log 字节 `3244259` → `3246313`（探针 mark/report） |
| 驱动方式 | 截图 + cliclick/osascript GUI 自动化 + 探针 + 只读 sqlite（`file:...?mode=ro`） |

---

## 一、检查项与结果（14/14 PASS）

### 启动与迁移

| # | 检查 | 结果 | 证据 |
|---|---|---|---|
| 1 | 应用启动无 panic | PASS | dev stderr 仅 vendored tiny_http 2 条警告（既有）；`[memory] 嵌入引擎加载成功` |
| 2 | error.log 零增量 | PASS | 水位前后均 193 行，一条未新增 |
| 3 | **M61 迁移列在位** | PASS | 启动前 `subagents` 表 17 列无 usage 列 → 启动后 `prompt_tokens` / `completion_tokens` / `cache_read_tokens` 三列补齐（本批 95b3ea90 的 `ensure_usage_columns` 首次在真库生效） |
| 4 | 库完整性 | PASS | `PRAGMA integrity_check` = ok（只读连接）；关键表齐全（tasks 47 行、app_versions、notifications、workspaces 等 27 表） |
| 5 | 探针 mark/report 闭环 | PASS | 水位落盘、report 正常出报告（事件区间为空，见「观察」③） |

### 界面渲染（截图 4 张，`/tmp/smoke-{main,tasks,chat,settings,skills}.png`）

| # | 检查 | 结果 | 证据 |
|---|---|---|---|
| 6 | 首页仪表盘 | PASS | 问候语、KPI 三池（待办 29 / 完成 1 / 归档 16）、完成率 37%、**每日一图加载成功**（网络通）、周计划柱图、图谱速览、本周日程 |
| 7 | 任务页周时间网格 | PASS | 10月19–25 周网格 + 泳道计划块 + 任务池面板（29 张未完成卡） |
| 8 | 聊天页 | PASS | 会话列表（17 个）、右侧工作面 任务/文件/子Agent 三页签、输入区模式条（弹授权/简洁/MiniMax-M3/中·默认） |
| 9 | 设置页 + 技能面板 | PASS | 基础设置整页渲染；技能面板 **ppt-generator 行尾出现启用拨杆（开启态）**——M47 的 UI 已上真机；**列表无 pptx-design 僵尸**（与 M48 文件面互证）；子 agent 三配置档（probe/validator/writereditor）正常列出 |

### 行为链路（真实 LLM，两轮往返）

| # | 检查 | 结果 | 证据 |
|---|---|---|---|
| 10 | 第一轮：发送 → 回复 | PASS | 界面回复「冒烟通过」；审计链完整：`user.message` → `trace.start trace_id=143` → `llm.request model=MiniMax-M3 provider=anthropic` → `llm.response status=200` → `llm.usage cache_hit_pct=100` → `trace.complete status=done ms=4996` |
| 11 | 第二轮（连续对话） | PASS | 回复「收到」；trace_id=144，`msgs_count=5`（多轮上下文正确累积），ms=1755；顶栏用量条 Σ132→Σ315、缓存 128→309 实时更新 |
| 12 | Enter 发送口径 | PASS | 代码口径「Enter 发送、Shift+Enter 换行、组合态不发送」（`InputArea.tsx:302`）；osascript 合成回车实测可发送。首次 cliclick 回车未发送为**测试伪影**，见「观察」④ |
| 13 | 优雅退出清理链 | PASS | Cmd+Q → `app_exit_cleanup \| api_stopped=true \| py_killed=0 \| services_stopped=0 \| mcp_stopped=0 \| exec_stopped=0 \| exec_drained=true`，进程退出 |
| 14 | 文件面静态核验 | PASS | `builtin_skills/` 无退役残留（M48）；技能目录无 `.tmp` 残渣（M47 原子写副证）；`runtime/flags/` 四旗标齐（bot/py/services/ui）；MCP 无子进程、无工具快照文件（`builtin-playwright` 处于停用 → `absent`，符合口径）；技能 frontmatter 无 `enabled` 键 = 缺省启用，与拨杆开启态一致（M47「缺省不误关」） |

---

## 二、观察与说明（非发现项）

1. **`data_dir_fallback` WARN 属 dev 既有口径**：dev 的 exe 在 `target/debug`，写探针失败 → 兜底到 App Support。上一轮运行（03:48/03:52）同样有这两行，非本批引入。
2. **error.log 老错误不属于本窗口**：最后三条（03:06–03:11）是上一轮会话遗留——`extract_document 提取失败：内部错误：提取失败：`（错误原因为空串，值得在全量卡时看一眼是否吞了 detail）、一次 offset 越界 WARN、一次 `subagent.machine_check_failed`。本轮启动后零新增。
3. **探针 report 事件区间为空是预期**：两轮冒烟是纯文本对话（tools=0），不产生探针所统计的工具/调度/技能域事件；原始审计链在「行为链路」表中逐行列出。
4. **Enter 伪影的定位过程**：cliclick `kp:return` 合成回车在有字输入框中未触发发送（粘贴与点击均正常）；换 osascript `keystroke return` 立即发送成功 → 合成键被输入法层消费的测试手段问题，应用侧 Enter 处理正常（代码与实测双向印证）。**复跑手册若用脚本驱动中文输入，建议剪贴板 + osascript 回车，不用 cliclick 的 kp。**

## 三、本轮未覆盖（留给 38 卡全量）

- **M40–M74 行为卡**：本冒烟只验了 M47/M48/M61 的「静态面」（拨杆 UI 在、僵尸清了、迁移列在）；三道闸（清单/路由/预审）、收尾窗、五态槽位等**行为面**需按 `docs/realmachine-mutation-test-2026-10-24.md` 逐卡跑。
- **MCP 按需启停（M50/M51/W04）**：现配服务器 + 真起子进程才能验，冒烟未现配。
- **变异注入**：本轮口径不改代码，50 条注入全部未做。

## 四、痕迹

- 冒烟产生的新数据：1 个会话（「冒烟测试：请只回复四个字…」，2 轮对话）、bot.log 约 2 KB、`subagents` 表三列迁移（结构性，无行级数据）。按「测试阶段不做数据迁移」口径未清理，同库继续用。
- 截图证据：`/tmp/smoke-boot.png`、`smoke-main.png`、`smoke-tasks.png`、`smoke-chat.png`、`smoke-typed.png`、`smoke-reply2.png`、`smoke-reply3.png`、`smoke-settings.png`、`smoke-skills.png`（临时目录，重启后失效）。
- 应用已退出、dev 进程树已清、工作区无代码改动（`git status` clean + 本报告一个文件）。
