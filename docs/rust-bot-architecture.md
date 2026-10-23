# wmessage Rust Bot 架构说明

> 项目形态：Tauri 2 桌面应用（任务看板 + AI 助手）。单 crate（`src-tauri/`，lib 名 `wmessage_lib`），`main.rs` 仅 6 行调 `wmessage_lib::run()`。Rust 业务源码共 157 个文件、约 11.9 万行，全在 `src-tauri/src/`；同 crate 里另有 `src-tauri/tests/` 10 个集成测试文件与 `examples/` 1 个。`src-tauri/vendor/tiny_http` 是 vendored 补丁依赖，非业务代码。

## 1. 文件结构 + 职责

```
src-tauri/src/
├── main.rs (6行)        入口，调 wmessage_lib::run()
├── lib.rs (1187)         模块总表、Tauri setup、invoke_handler 注册、托盘/快捷键、退出清理
│                        （setup 里另起清扫：上次被杀留下的 app_runs running 僵尸行）
├── app_state.rs (274)   运行期全局状态单一入口 `AppState`（lib.rs setup `app.manage` 注入）：
│                        8 张执行期表 + 「全局状态总表」文档；详见 §2.1
│                        （原 consts.rs 已并入 app_consts 命令、本节不列）
├── error.rs             CommandError 结构化错误（code=CommandErrorCode 枚举/message/recoverable），全命令统一
├── paths.rs             数据目录解析 + 便携模式判定
├── audit.rs             审计事件写 bot.log（audit_event! 宏、write_event、error_kv 带 code）
├── profile.rs           个人资料：用户/机器人 头像 + 姓名（profile.json + 头像拷进 profile/；读回 base64 data URL，
│                        原子写 + 损坏备份 + 5MB/扩展名校验）
├── mutation.rs          任务变更来源枚举 MutationOrigin（Main/Bot/Api/Migration），tasks-updated 事件分流
├── llm_text.rs          LLM 输出文本小工具：strip_fences 剥 ```json 围栏
│                        （task_autotag / memory::extract / memory::consolidate 共用）
│
│─ 数据层
├── db/                  SQLite（wmessage.db）+ 迁移 + 产物 + 工作区（按表分文件）
│   ├── db/mod.rs      模块声明 + 公共 re-export
│   ├── tasks.rs         Task / Subtask / TaskFile；task_patch 写口校验截止与排期时间格式
│                        （截止只认 YYYY-MM-DD 与 YYYY-MM-DDTHH:mm，解析不了的一律拒）
│   ├── bot_sessions.rs   BotSession / 会话归属
│   ├── bot_history.rs   会话聊天记录
│   ├── migrations.rs     数据库迁移（历史顶层 migration.rs 已并入）
│   ├── paths.rs          data_dir()/gen_dir()（AI_Gen_Files）
│   ├── skill_out.rs      技能产物输出
│   ├── subagents.rs      子 agent 编排持久化（SubagentRow / 六态状态机 transition_allowed /
│   │                     SubagentBudget 预算三硬顶；spawn 插 queued 行，check 幂等轮询）
│   ├── people.rs         成员注册表（多人任务汇总的归属人字典；people_list 命令 +
│   │                     upsert 最新名覆盖 + 占位兜底，见 docs/archive/TASK-GRAPH-DESIGN-2026-10-05.md §1）
│   ├── reminders.rs      机器人提醒（reminders 表）：到点发系统通知，不建卡不执行；
│   │                     定时串与作业同格式，触发点计算共用 bot_scheduler
│   ├── schedule_jobs.rs  内容型定时作业（T1）：XXL-JOB 式调度日志——重试/自动暂停/
│   │                     错过消费 + scheduled_job_runs 执行历史；定时单源本表
│   │                     （open_db 迁移把旧 tasks.schedule 迁入并清空任务卡侧）
│   ├── db/trace.rs        执行痕迹三表 exec_traces/exec_spans/file_changes（Agent 透明化
│   │                     设计 §4.1）：append-only 观测面 + 钳制单源（SPAN_TEXT_MAX 16KB /
│   │                     MAX_DIFF_LINES）+ 保留期清理 retire_traces_before（僵尸 running 同删）；
│   │                     保留期边界（TRACE_RETENTION_MIN/MAX_DAYS）与词元统计可查窗口
│   │                     （USAGE_STATS_MAX_DAYS）各一份常量，命令与设置页公示共用
│   │                     痕迹表也是幻觉守卫的证据源：file_paths_with_receipt（某路径历史上
│   │                     有没有工具写过，按 path COLLATE NOCASE 走索引）/
│   │                     不可证伪写入通道（run_python / MCP 这类自己就能写盘、
│   │                     无从确证的工具）不再查库：让开口径从会话级改成提问级，
│   │                     判在模型循环侧（本次提问跑过就让开那一轮）/
│   │                     session_tool_names（本会话调用过 / 成功过的工具名，两档 →
│   │                     点名判据的排除集：跨提问的重述是真话；派过子 agent 的会话
│   │                     整档让开——子 agent 的 span 记在子 trace 下，join 不成立）
│   ├── db/apps.rs        程序 apps 表 CRUD + app_runs 运行史 + app_versions 版本快照
│   │                     （滚动保留最近 100 版，见 docs/APPS-DESIGN-2026-10-15.md §5）
│   └── workspace.rs      workspace / bind_files
├── task_out.rs          对外 TaskOut（Task flatten + status），api/api_handlers 共享
├── trace_sink.rs        执行痕迹采集管道（Agent 透明化设计 §4.2）：dispatch 工具调用 →
│                        mpsc 后台攒批落库（writer 长连接 + DB_WRITE_LOCK）；trace 生命周期
│                        begin/end_trace（run_task_in_chat_with 首尾）+ TraceCapture 挂钩
├── task_autotag.rs      归档自动打标（一次性 LLM ≤3 标签回写 tags；守卫链幂等 +
│                        校验链截断不整包拒 + 失败静默审计，见任务图谱设计 §2）
├── tag_similar.rs       标签近义（embed 引擎逐标签向量 + 余弦 ≥0.78 对返回；
│                        进程级向量缓存 + 审计，图谱同义聚簇锚点用，任务图谱设计 §3）
│
│─ 模型元数据 meta/（原独立 Python 服务 model-meta-service 的内嵌版，127.0.0.1:8765 已退役）
├── meta/mod.rs          MetaProvider/MetaModel/MetaModelJoined + meta_provider/meta_model
│                        双表 CRUD（DDL 在 db/migrations.rs ensure_meta_tables，open_db 幂等建表）+
│                        8 个 meta_* 命令；user_custom 行永不被同步覆盖（ON CONFLICT WHERE 守卫）
└── meta/sync.rs         models.dev 同步：parse_models_dev 纯函数 + sync_models_dev
                         （shared_llm_client + 30s 超时；lib.rs setup 后台 spawn，失败仅审计不阻断）
│
│─ 本地 HTTP API（127.0.0.1 微服务，与 bot 解耦，只操作任务数据）
├── api.rs               trait TaskStore + MemStore(测试) + TauriStore(生产)
├── api_server.rs        HTTP server 生命周期、端口绑定、EventHub（SSE 中枢）
├── api_auth.rs          Bearer token（runtime/flags/api-token.txt）、开关持久化（runtime/flags/api-enabled.flag）；
│                        token 文件为空则原子覆写自愈（否则 create_new 恒撞 AlreadyExists，api_start 起不来）
├── api_handlers/        REST /api/tasks CRUD + SSE + api_start/stop 等命令
│   ├── api_handlers/mod.rs  模块声明 + 路由分发
│   ├── handlers.rs      HTTP 路由层
│   ├── api_handlers/commands.rs  tauri 命令层
│   ├── body.rs          请求/响应模型
│   ├── sse.rs           SSE 事件流
│   ├── ratelimit.rs     限流
│   ├── util.rs          工具函数
│   ├── validate.rs      入参校验
│   └── api_handlers/types.rs    共享类型
│
│─ Bot 核心（编排 → 决策 → 工具分发）
├── bot_chat.rs          入口编排：bot_chat / bot_compact / bot_execute_task 命令；五步主流程；
│                        ChatMsg/TaskRef/BotChatResult/TaskExecOrigin；多模态图片组装；ChatGuard 防重入
│                        （prompt 常量已移出，见 prompts/）
│─ 提示词 prompts/（AI 应用的业务逻辑，与代码同等对待；统一从 `crate::prompts::NAME` 取）
├── prompts/mod.rs        模块声明 + `crate::prompts::NAME` 统一出口 + prompt 清单锁测试
│   ├── prompts/subagent.rs 子 Agent 编排提示词（设计 §8）：主 agent 派发职责段/
│   │                     子 agent 通用段/research·coder·general 三 profile/收尾
│   │                     JSON schema 提示；镜像资产 docs/prompts/subagent/
│   prompts/system.rs       SYSTEM_PROMPT（主聊天规则底座 + 安全红线）
│   prompts/summary.rs      SUMMARY_SYSTEM_PROMPT（截断即摘要 ≤200 字）/ COMPACT_SYSTEM_PROMPT（/compact ≤300 字）
│   prompts/reflection.rs   REFLECTION_SYSTEM_PROMPT（多摘要 → 阶段总结）
│   prompts/execute.rs      EXECUTE_SYSTEM_PROMPT（任务卡执行）/ STEPWISE_ADDENDUM（逐步执行附加段）
│   prompts/planner.rs      PLANNER_PROMPT / REPLANNER_PROMPT（bot_plan 动态计划，只输出 JSON）
│   prompts/consolidate.rs  CONSOLIDATE_PROMPT（记忆整理，只输出 JSON ops）
│   prompts/extract.rs      EXTRACT_PROMPT（U16 会话收尾记忆抽取，只输出 JSON 数组）
├── bot_model_loop.rs (5859) LLM 流式调用+工具循环：run_model_loop(薄壳装配)/run_model_loop_core(可注入 mock)；
│                        SSE 解析、思考块拆分；单轮 Function 熔断；**墙钟治理**（WallBudget：软墙钟
│                        收尾提醒 + 墙钟熔断留产，均在轮边界检查）；LlmHttp / ModelLoopDeps 依赖注入
│                        （TOOLS schema 已移出，见 bot/registry.rs）；工具结果回灌上限（字节口径）：
│                        per-tool 预算与设置页 maxToolOutputBytes 取小，超限转存溢出文件、只回灌节选+指针
│                        （tool.output.truncated 审计）+ llm.usage 薄壳累计 + bot-usage-delta 水位条
│                        事件（Anthropic 协议）
├── bot.rs (163)         门面：跨模块 re-export（BotChatResult/TaskRef/model_loop/StopGuard/AuditLevel/db::*）
│                        + 子模块声明；旧 `crate::bot::<item>` 路径 1:1 保持
├── bot/params.rs        Agent 运行参数注册表（P3-a）：PARAMS_TABLE 单源（可编辑 config 驱动 +
│                        硬编码只读两区）+ resolve_* 读取口唯一（max_rounds/history_budget/
│                        subagent 预算/search 条数）+ bot_effective_params 命令；settings 页参数卡同源
│                        （默认值 = resolver 在空配置下的派生值；可填范围 = 钳制常量 min/max；
│                        生效值 = 钳过之后的数 + 来源标注——页面不再自留一份硬编码数字）。
│                        只读行不作为设置项露出，但**可被设置页文案引用**（值从表里取）：
│                        如 trace.retention_days 的可填范围、memory.extract_min_interval_secs
│                        的抽取限频——文案里的数不留手写副本
├── bot/registry.rs (3826)  **工具单源真相（阶段 2）**：`BUILTIN_TOOL_COUNT` 个 schema 常量 + `TOOLS_TABLE`
│                        （name/schema/mutating/call，数目以该常量为准，子 agent 专属工具见 SUBAGENT_ONLY_TOOLS）→
│                        TOOLS JSON（tools_json）/ MUTATING_TOOLS / dispatch 查表三处全派生
├── bot/dispatch.rs (1599)  工具调度核心 execute_tool / execute_tool_impl：**TOOLS_TABLE 查表**（非 match）+
│                        pre_execute 洋葱入口 + skill_on_step 钩子 + tool.return 结构化审计 +
│                        子 agent 软墙钟闸（剩余预算进入收尾窗口即拒绝新调用，不打断在飞工具）
├── bot/hooks.rs           L4 pre_tool 钩子扩展点（design ONION-HOOKS-DESIGN-2026-10-16 §四）：
│                        HookRegistry 进程级注册（OnceLock 单例，同层内按注册序；层序只由
│                        dispatch 调用栈保证，注册表无任何层序配置入口）+ HookLayer 会话级禁用 +
│                        ToolCallView 只读视图（字段全是共享借用，L4 无法改写入参）+ 内置观察钩子
│                        （builtin_observer，计数 + bot.log 留痕）。接线点唯一：
│                        dispatch.rs::run_pre_tool_layer（三道闸之前）
├── bot/mcp/             外部 MCP 服务器接入（stdio/HTTP，设置页 McpPanel 管理）：
│   ├── bot/mcp/mod.rs   模块声明
│   ├── bot/mcp/config.rs        McpServerConfig/校验（stdio 启动器白名单、URL 公网闸、
│   │                            normalize/timeout 钳制）+ 工具命名 mcp_tool_name/tool_prefix
│   ├── bot/mcp/manager.rs       McpManager（OnceLock 单例）：连接槽 ensure_connected/call_tool、
│   │                            指纹懒重连、stderr 环形缓冲、spawn 白名单字面量构造、
│   │                            子进程资源采样（5s TTL 缓存，口径同 bot/procs：
│   │                            RSS / CPU / 监听 socket，整棵进程树合计）
│   ├── bot/mcp/mount.rs         挂载层：连接快照 → 工具 schema 段（撞名消歧/条数/schema 体积
│   │                            上限）+ dispatch miss 反查 execute_mcp_tool（带超时/结果整形）；
│   │                            函数名走进程级登记簿（名字 ↔ 服务器跨断连稳定，不随重连漂移）
│   ├── bot/mcp/secrets.rs       B4-6 机密存储：env/headers blob 走 keyring `mcp:<id>` 条目
│   │                            （Linux 降级单文件 0600）+ 进程内缓存 + load_config 水合 +
│   │                            迁移判据；配置永不明文（skip_serializing 收口）
│   ├── bot/mcp/commands.rs      设置页 tauri 命令：mcp_server_save/delete/toggle、
│   │                            mcp_status（回 builtin 标志 + 子进程 pid/内存/CPU/运行时长/
│   │                            监听 socket）、mcp_server_tools（写路径 CONFIG_WRITE_LOCK 全程持锁；
│   │                            内置条目 delete 拦截）
│   ├── bot/mcp/stdio_transport.rs  stdio 子进程传输（读侧行长上限 + 子进程生命周期托管）：
│   │                            LineCappedReader 每行最多转发 1MB（超出的丢弃，rmcp 的
│   │                            read_until 无上限 → 外部服务器一条长行就能撑爆宿主）；
│   │                            CappedChildProcess 托管子进程（关 stdin → 3s 优雅退出 →
│   │                            超时强杀），不依赖 rmcp 的 TokioChildProcess
│   └── bot/mcp/builtin.rs       内置条目（Playwright 浏览器）：playwright-mcp/ 包目录解析
│                                （env → exe 同目录 → 资源目录 → 仓库根）+ Chromium 内核定位
│                                + 启动时缺失才补写配置（默认停用）
├── bot/procs.rs (1168)      进程树公共模块（受管服务层与 MCP stdio 共用）：
│                        纯进程操作，不碰任何上层登记表——只依赖 pid 空间与系统进程表：
│                        进程快照（pid/ppid/启动标识/RSS/CPU 时间，Linux 读 /proc、
│                        macOS 调 ps）+ ppid 树子孙收集 + 增量比对（PID 复用识别）
│                        + 停止阶梯 stop_tree（SIGINT→100ms→SIGTERM→400ms→SIGKILL，
│                        每级发信号前拍一张快照增量收集子孙，最后统一杀）
│                        + CPU 占用率公式 cpu_usage_pct（两个上层共用一条）
│                        + 监听 socket 枚举 listening_sockets（Linux 读 /proc/<pid>/fd
│                        的 inode 配 /proc/net/tcp{,6}、udp{,6}；macOS 走 lsof；
│                        **Windows 没有**——现有进程快照层在 Windows 上本就退化）
│                        + 平台能力声明 listening_supported（Windows 为 false——
│                        判定层据此返回 Unsupported 而非 Ok）
│   └── bot/procs/tests.rs (591)  进程树纯函数单测：快照解析 / 树收集 / PID 复用 /
│                        整组杀护栏 / 资源采样 / socket 解析（/proc/net 与 lsof 两套格式）
│                        + 真子进程监听端口集成测试（验平台路径真能读到）
├── bot/services.rs (2319) 受管长驻服务层（宿主托管的长驻进程：node MCP server / dev server / watcher）：
│                        登记表（AppState.services）+ 状态机（Starting/Running/Stopping/Exited/Failed）
│                        + 宿主直接 spawn（不继承 run_python 的 rlimit）+ stdout/stderr 落 O_APPEND 文件
│                        + 停止信号阶梯（SIGINT→100ms→SIGTERM→400ms→SIGKILL）+ ppid 树子孙回收
│                        （带 starttime 防 PID 复用）+ 资源观测（整棵进程树 RSS / CPU，
│                        只告警不杀）。与 py/runtime.rs 是两层：前者有界作业、
│                        连坐杀组；本层长驻、宿主知道它、能停它、退出时收干净
│                        + **端口/监听地址闸**（P2-③）：判据 L1 默认只许 loopback
│                        （放开靠独立开关）/ L2 禁占宿主保留端口 / L3 禁特权端口；
│                        判定三态 Ok / Violated / **Unsupported**（Windows 没判≠没事）；
│                        违规记在**正交字段** last_violation（不复用状态机）+
│                        审计 service.port_violation + 面板红色徽章 + 通知中心一条；
│                        启动期加密采样（0/0.5/1/2/5/10s）+ 30s 巡检，发现即终止
│                        + 服务告警进通知中心（资源告警 / 端口违规，条件消失自动撤收）
│   ├── bot/services/watchdog.rs (295)  服务层「宿主死了服务也得死」看门狗：
│   │                            unix 用管道 EOF（宿主一死 → 写端关闭 → 助手 TERM 服务整组）；
│   │                            Windows 复用 py/runtime.rs 的 Job Object（KILL_ON_JOB_CLOSE）
│   └── bot/services/tests.rs (1362)  服务层单测：进程快照纯函数（ppid 树 / starttime / RSS / CPU 解析）+
│                        限额不继承、停止阶梯、setsid 逃逸子孙回收、输出截断无空洞、
│                        资源告警判定（阈值 / 持续 / 去重）、
│                        端口闸（flag 解析 / loopback 判定 / L1-L3 判据 / 保留集）、
│                        服务告警落通知中心（资源告警 / 端口违规）等
├── bot/config/          BotConfig/ApiProvider/PermMode/KeySlot（bot-config.json，key 走系统 keyring）；
│   ├── bot/config/mod.rs  模块声明
│   ├── bot/config/schema.rs        bot-config.json schemaVersion 迁移 + 双协议派生
│   │                               （derive_legacy_fields_from_active / active_model_entry
│   │                               / effective_inference 条目级推理参数解析）+ resolve_max_tokens
│   ├── bot/config/types.rs  ApiProvider / PermMode / KeySlot
│   ├── bot/config/io.rs    配置读写 + 默认值 + read_memory_control（U15 记忆开关轻量读取）
│   ├── keyring.rs        API key 走系统 keyring
│   ├── bot/config/commands.rs  bot_get_config / bot_set_config
│   ├── bot/reasoning.rs  推理强度线上参数映射（RE-1）：EffortLevel 档位 →
│   │                     按模型族/协议的 ReasoningWire（OpenAI effort/GLM thinking/
│   │                     Anthropic budget），resolve 装配 + describe 审计展示
│   └── audit.rs         audit_log
├── bot/tools.rs (2892)    23 个 tool_* 实现（任务卡 CRUD / 子任务 / 文档生成 / 联网 / 时间 / 记忆转发 /
│                        受管服务起停查 start_service / stop_service / list_services）
├── bot_anthropic.rs     Anthropic 协议适配（纯函数）：OpenAI ↔ /v1/messages 双向转换、auth 头、prompt caching
├── bot_plan.rs          PREVR 动态规划：复杂任务先生成计划注入 prompt，失败 Replan（≤2 次）
├── bot_scheduler.rs     定时任务卡调度：30s 扫描 schedule（每日/每周/at:），到点走 run_task_in_chat 自动执行；
│                        SchedGuard 防重入；完成发系统通知；
│                        机器人总开关关着时到点的作业判 Held（不派发、水位不推进，
│                        恢复后仍在补跑窗口内）——否则每 30s 空转一轮、每天几千条跳过审计
├── bot_reminders.rs (562) 机器人提醒 schedule_reminder/list_reminders/cancel_reminder：
│                        到点发系统通知（只说话，不建卡不执行内容）；when 支持绝对时间 /
│                        今天 HH:MM / 相对 +30m；repeat none/daily/weekly/monthly；
│                        定时串与触发点计算复用 bot_scheduler（occurrence_after），
│                        存储与触发路径独立（reminders 表）；与作业同一个 30s tick
├── due_notify.rs        任务卡截止系统通知：30s 扫描活跃任务（仿 bot_scheduler），截止前 1h 一条、
│                        截止时刻一条（超 24h 老任务不补发，防升级/重启后轰炸）；通知失败只记日志
├── bot_slash.rs         StopGuard/StopRegistry（/stop 按执行实例隔离；注册表已迁 app_state::AppState，
│                        见 §2.1）、危险操作确认弹窗、机器人总开关
├── exec_steps.rs        任务卡逐步执行模式：多子任务逐个做、用户确认「继续/重做/停」
├── bot_orchestrator.rs  子 Agent 编排器（方案 B，SUBA 设计 2026-09-27）：受管子 agent 的
│                        spawn/check/cancel 生命周期状态机 + 无任务卡（子 agent 不再建卡，行上
│                        只存 parent_task_id 指向派发主卡，新行 task_id 恒 NULL）+ acceptance
│                        落行上 acceptance_json + 预算钳制；与 bot_execute_task（单发子 agent）入口/存储/事件
│                        各自独立；LLM 工具暴露/runner（SUBA-2）与预算强制/并发排队（SUBA-3）
│                        按设计 §11 分批接入；P4：spawn 预算默认走 bot/params 配置；
│                        **收尾契约**（2026-10-23）：预算单一钳制入口 + 收尾 JSON 三段修复
│                        （本地修复 + 补答一次）+ digest 补渲染（残留产出 / 机检未过项）+
│                        机器可判定条目的派发期形状校验
│                        采样接线（run_task_in_chat 闭包喂 maybe_record_trace）
├── bot_artifacts.rs     产物登记：link_file_to_task → 收尾按 TaskExecOrigin 分流 →
│                        落「通知中心」artifact_bind 消息（原 artifact-batch-ready 挂件弹窗已下线）→
│                        通知页勾选 → confirm_artifact_batch 落库 + notif_resolve_artifact 回写
├── notifications.rs (661) Agent 通知中心：notifications 表持久化消息（INSERT OR IGNORE 幂等，
│                        memory_proposal / artifact_bind / task_question 三类决策事件 +
│                        service_alert 一类服务告警——只告知不决策，条件消失时按
│                        (serviceId, alertType, warning) 三元组自动撤收）+
│                        notifications_list/pending_count/resolve/clear_done 四命令 +
│                        notifications-changed 广播；双向同步（mem approve/reject、
│                        confirm_artifact_batch 尾部回写消息状态）
├── bot_ui.rs (1098) 桌面 UI 自动化工具层（六件套）：list_windows / activate_window /
│                        ui_inspect / ui_click / ui_type / wait_for_element。
│                        四道闸——独立总开关（ui::ui_enabled，默认关闭）/ 窗口级授权
│                        （动作必带 windowId 且窗口真实存在）/ 危险动作确认
│                        （发送、提交、支付、删除类点击前弹窗，60s 无响应默认拒绝；
│                        判不出的（无标题按钮）一律确认）/ 敏感控件禁打（密码、验证码类不代填）；
│                        闸门与实际动作之间还夹两道——窗口身份（pid+标题）比对，
│                        变了就不动；控件身份（NodeIdentity）比对，对不上就不动；
│                        注入全程持全局互斥；每次动作留审计 +
│                        动作前后各一张截图（落 AI_Gen_Files）
├── ui/                 桌面 UI 自动化跨平台层（数据模型 + 总开关 + 平台分发）
│   ├── ui/mod.rs (618) UiWindow / UiNode / NodeIdentity 数据模型 + flag 开关（ui_get_enabled /
│   │                 ui_set_enabled，unix 建即 0600；每次切换写 ui_automation_toggle 审计）+
│   │                 平台分发（list_windows / activate_window / inspect_tree / press_node /
│   │                 focus_node / type_text）+ 前台窗口判定（window_is_foreground）
│   ├── ui/macos.rs (554) macOS 后端：AX 读 UI 树（AXWindows / AXChildren / AXPress，
│   │                 AXValue 取坐标）+ CGEvent 注入（Unicode 键盘事件）；
│   │                 需本进程获「辅助功能」权限（AXIsProcessTrusted）；
│   │                 前台窗口按 NSWorkspace 实测（不恒为 false）
│   └── ui/windows.rs (472) Windows 后端：UIAutomation 读控件树（Invoke 模式优先，
│                     无 Invoke 才退回坐标点击）+ SendInput 注输入；
│                     控件值读 UIA ValuePattern（密码框不读，避免读出明文）；
│                     COM 生命周期同线程成对（CoInitializeEx / CoUninitialize）
├── bot_desktop.rs (647) 电脑辅助 Tier1（N4，看 / 打开 / 把东西递到用户手上）：reveal_path
│                        （opener 定位，白名单闸）/ open_url（公网闸 ensure_public_http_url）/
│                        clipboard_write（官方 clipboard-manager v2）/ copy_file_to_clipboard
│                        （文件进系统剪贴板，读白名单闸，复用 platform::copy_file 的
│                        NSPasteboard / CF_HDROP）/ screenshot（macOS screencapture -x /
│                        Windows PowerShell，落 AI_Gen_Files）；
│                        鼠标键盘注入不在本模块——见 bot_ui.rs / ui/（独立总开关）
├── bot_skills/recommend.rs 语义化技能推荐（N7-②）：技能描述 embed（bge-small-zh 复用
│                        memory/embed）+ 余弦排序截断 top5，embed 不可用降级全清单；
│                        只影响 SkillCatalog 呈现顺序，不接管 IntentRule 路由
├── ask_engine.rs        任务执行问答引擎：ask_user 工具（通知队列 wfq:{qid} + oneshot
│                        等待 + 假设兜底 + 预算扣减/回冲）+ ask_respond 应答命令；
│                        原.workflow_questions，随画布退役中立化保留（手动执行任务卡共用）
│
│─ Bot 工具实现（被 bot.rs 分发调用）
├── bot_fs.rs            只读工具 read_text_file/grep_files/list_files + 文件编辑
│                        edit_file/write_file（N6，Aider 式三级匹配：精确→空白容错→
│                        reflection；写白名单≠读白名单：可写根=AI_Gen_Files+任务卡
│                        绑定文件夹+allowedDirs；白名单外 perm_mode 三分支/覆盖确认）；
│                        路径白名单 + perm_mode(strict/ask/yolo) 授权闸
├── bot_archive.rs (1404) 归档工具 unpack_archive/pack_archive：zip / tar / tar.gz 的解包与打包
│                        （纯 Rust：zip + tar + flate2，不依赖本机 Python）；压缩包是外部
│                        输入——条目名带 .. / 绝对路径 / 盘符 / 软链一律跳过并列出，单文件
│                        64MB、整包 256MB、条目 5000 条上限（超限报「疑似压缩炸弹」），放大比
│                        200:1 封顶；已存在的文件不覆盖（跳过并列出）；解包只认源在读白名单、
│                        落点在写白名单
├── bot_manage_file.rs (394) 文件复制/移动/删除 manage_file：copy 复制（源保留）/
│                        move 移动（同卷 rename、跨卷复制后源入回收站）/ delete 删除
│                        （进系统回收站，删前弹窗确认、60s 无响应默认拒）；源走读
│                        白名单、目标走写白名单；目标已存在一律不覆盖；目录树里的
│                        软链既不跟随也不复制
├── bot_web.rs           web_search（Bing）+ fetch_url（HTML→文本、GBK、SSRF 防护、钉 IP）
├── bot_py.rs (1367)     Python 子进程执行：runtime/flags/py-enabled.flag 开关、独立临时目录、超时/内存/CPU 限额
│                        （rlimit / Job Object）、固定脚本模板（extract/make_docx/xlsx）、自由脚本 run_python
├── ocr.rs               ocr_image：macOS Vision / Windows PP-OCRv6 ONNX；纯本地不上网；
│                        输入有字节与像素上限（解码前只读文件头，解码炸弹面）
├── overflow.rs          工具大结果溢出存储：`data_dir/overflow/<日期>/<uuid>.txt` 转存 +
│                        回灌指针文案 + 保留期清扫（30 天，启动一次 + 每天一次）。
│                        配套 `bot_fs::allowed_dirs` 把本目录放进读白名单（模型靠它分页读回），
│                        回灌上限取 per-tool 预算与设置页 maxToolOutputBytes 的小者（字节口径）
├── bing.rs              必应每日一图（首页小部件数据源）：cn.bing.com 官方 API + 按日落
│                        `{data_dir}/runtime/bing/`（图片先 / 元数据后提交标记；保留 7 天，
│                        过期随拉取清理）；base64 data URL 单 IPC 返回（免配 asset 协议）；
│                        大图先查长度再流式读，边读边计数（超限即中止）
│─ 技能系统 bot_skills/（技能 = 数据目录 skills/<name>/SKILL.md，YAML frontmatter）
├── bot_skills/mod.rs            门面：全量 re-export 子模块
├── bot_skills/parse.rs          SKILL.md frontmatter/正文解析 → SkillMeta（mode/intents/steps）
├── bot_skills/state.rs          SkillRun 运行实例 + 状态机
│                                （Loaded/Running/Paused/Finished/Failed/Terminated）、
│                                SKILL_RUNS 表（已迁 app_state::AppState，访问器需 AppHandle）
├── bot_skills/runtime.rs        生命周期：start_skill 预审/启动、暂停/确认/推进、skill_finish 收尾；
│                                暂停态也算超时（等确认等到超时的按失败收，
│                                否则同名技能被一直占着，别人起不来）
├── bot_skills/scheduler.rs      DSL 步骤调度器 run_skill_scheduler + DslOutcome
│                                （Done/AwaitUser/FailedButRecoverable → LLM 兜底）
├── bot_skills/vars.rs           步骤变量替换（${stepN.field} / ${prev…} 嵌套路径）
├── bot_skills/manage.rs         技能安装/列表/删除，路由表重建触发点
├── bot_skills/remote.rs         远程技能源：install_skill（GitHub 仓库 / 腾讯 SkillHub / zip 直链 →
│                                下载 → 解包 → 定位 SKILL.md → 落盘 + 重建路由）与 search_skills
│                                （SkillHub 公开搜索 API）；下载走公网闸逐跳校验重定向，解包复用
│                                bot_archive 的限额闸（zip slip / 软链 / zip bomb），落盘失败回滚
├── bot_skills/builtin.rs        内置技能预装：`assets/skills/` include_bytes! 烘进二进制，启动时
│                                物化到 builtin_skills/（与用户 skills 同父不同名，导入/删除语义互不干扰）；
│                                物化幂等（Seeded/Updated/Unchanged）；搜索优先级：用户 > 内置 > dev mock
├── bot_skills/files.rs          open_file_path/delete_bound_file 路径白名单校验（防 XSS→RCE）
│
│─ 程序 apps/（聊天里写好的确定性小程序：落盘 app.py + manifest，点一下就跑）
│                         设计见 docs/APPS-DESIGN-2026-10-15.md / docs/APP-DEV-CONVENTIONS.md
├── apps/mod.rs          runner：跑 `{data_dir}/apps/<id>/<script_path>`（脚本路径只放行
│                        app 目录下的单个文件名）、抓 stdout 的 `##RESULT## {json}` 哨兵、
│                        产物落 `output/<run_id>/`、摘要 300 字截断；app_runs 运行史回写
│                        （失败路径都回写终态；进程崩在写回之前留下的 running 僵尸行
│                        由清扫标成失败）
├── apps/commands.rs     程序页 tauri 命令：list_active/get/run/create/update/archive/
│                        runs_list/versions_list/version_restore/preflight/open_output
├── apps/manage.rs       领域核 + 三个 bot 工具（apps_list / apps_create / apps_update_files，
│                        整文件集提交）+ manifest 校验 + 版本快照/回滚/滚动 100 版
│
│─ 路由/守卫
├── intent_router.rs     前置意图路由 RouteAction（Skill/ExecuteTasks/PassThrough）；
│                        L1 正则硬锁（技能 frontmatter intents 驱动，LLM 不参与选 Skill）；
│                        bot_chat pre-step 直调（原 MiddlewareRegistry 洋葱链 2026-10-08 拆除）
├── tool_guard.rs        会话上下文注册表：任务卡执行流程识别 + 子 agent 白名单数据源
│
└─ 记忆系统 memory/（v2，语义嵌入）
├── memory/mod.rs            门面：injection_block 聊天注入记忆块（U15 注入总闸）、
│                            save_summary/apply_reflection、工具 tool_remember_fact/
│                            recall_facts/record_lesson（U15 主动记忆门禁）、MemoryControl
├── memory/store.rs          mem_items 表 CRUD、500 条上限、余弦语义去重（合并/提示）
├── memory/embed.rs          bge-small-zh-v1.5 ONNX 本地嵌入（OnceCell 懒加载，失败全局降级关键词模式）
├── memory/rank.rs           混合打分 0.55 语义 + 0.20 关键词 + 0.15 重要度 + 0.10 新近度
├── memory/consolidate.rs    定时记忆整理：LLM 反思合并/裁决矛盾，10 分钟扫一次
├── memory/panel.rs          设置页记忆库命令：mem_list（混合检索纯读不刷访问计数）/
│                            mem_update（content 变更重算嵌入）/ mem_delete / mem_stats
│                            （统计 + 嵌入引擎状态），MemItemView 不含向量本体；
│                            mem_export/mem_import（JSON 带向量，导入走语义去重只增不删）
├── memory/extract.rs        U16 自动记忆抽取：bot_chat 收尾触发（fire-and-forget，仅交互
│                            会话 + 30 分钟限频）→ LLM 抽取 → parse_extract 容错解析 →
│                            auto 直接入库 / confirm 进 mem_pending 待确认队列
│                            （mem_pending_list/approve/reject，approve 走语义去重入库）；
│                            U19 写入时冲突裁决（两段式）：相似候选（cos≥dedupHint，
│                            只查抽取域 kind）→ LLM 逐条裁决 new/update/skip——改口
│                            update_by_id 更新原条目不堆积，坏输出回退全 new
└── memory/tests.rs + memory/consolidate/tests.rs   记忆单测（假向量，不依赖 ONNX）
```

## 2. 关键类型 / trait 位置

| 类型 | 位置 |
|---|---|
| `RouteAction` / `IntentRule` | intent_router.rs:23 / :100 |
| `TaskStore` trait / `MemStore` / `TauriStore` | api.rs:30 / :48 / :95 |
| `Task` / `Subtask` / `TaskFile` / `BotSession` | db.rs:32 / :13 / :22 / :544 |
| `CommandError` / `CommandErrorCode` / `CommandResult` | error.rs:151 / :36 |
| `ChatMsg` / `TaskRef` / `BotChatResult` / `TaskExecOrigin` | bot_chat.rs:35 / :458 / :466 / :1151 |
| `BotConfig` / `ApiProvider` / `PermMode` / `KeySlot` | bot/config.rs:63 / :203 / :331 / :28 |
| `LlmHttp` / `ModelLoopDeps` / `ParsedChunk` | bot_model_loop.rs:363 / :380 / :146 |
| `SkillRun` / `SkillState` | bot_skills/state.rs |
| `SkillMeta` | bot_skills/parse.rs |
| `DslOutcome` | bot_skills/scheduler.rs |
| `MemItem` | memory/store.rs |
| `MetaProvider` / `MetaModel` / `MetaModelJoined` / `MetaSyncResult` | meta/mod.rs |
| `StopGuard` | bot_slash.rs（注册表 `StopMap` 见 app_state.rs） |
| 确认弹窗 `ConfirmMap` | app_state.rs（`confirms(app)` 访问器） |
| `AppState` / `TOOLS_TABLE` / `ToolDef` | app_state.rs:113 / bot/registry.rs:335 / :36 |

### 2.1 运行期全局状态（阶段 3 收口，2026-09-13）

单一入口 **`app_state.rs`**：`AppState` 由 `lib.rs` setup `app.manage(AppState::default())` 注入，
各模块用 `try_state` 取值（缺失则退回进程级兜底实例，语义不变）。已收口 **8 张执行期表**：

| 字段 | 原位置 | 持有者 / 清理 |
|---|---|---|
| `skill_runs` | bot_skills/state.rs | 状态机 + `skill_terminate_all` / `clear_terminal_skill_runs` |
| `stop_registry` | bot_slash.rs | `StopGuard::drop`（Arc 句柄） |
| `confirm_requests` | bot_slash.rs | 前端应答 / 60s 超时 |
| `chat_running` | bot_chat.rs | `ChatGuard::drop`（Arc 句柄） |
| `exec_running` | bot_chat.rs | `ExecGuard::drop`（Arc 句柄） |
| `sched_running` | bot_scheduler.rs | `SchedGuard::drop`（Arc 句柄） |
| `pending` | exec_steps.rs | 用户应答 / 超时 |
| `artifact_registry` | bot_artifacts.rs | 流程收尾 clear |
| `services` | bot/services.rs | 受管服务状态机 + `stop_service` / 退出时 `services::stop_all` |
| `services_usage` | bot/services.rs | 资源观测的**合计**口径（合计 RSS + 合计告警去重）——跨服务属性，没有归属条目 |

刻意**留档不收口**：`NEXT_STOP_ID`（id 发号器，进程级）、`SESSION_ORIGINS`（session 元数据，
「写一次/读一处/随 session 消亡」，理由与三条测量结论见 `app_state.rs` 总表与 `tool_guard.rs:45`）、
`ROUTES` / API 专属 4 项 / 不可变缓存（理由见 `app_state.rs` 模块头）。
测试隔离：相关用例各自 `manage` 独立实例；两把测试串行锁（`SKILL_RUNS_TEST_LOCK` / `STOP_TEST_LOCK`）
已随迁移撤除。

## 3. 主调用链（收到消息 → 回复）

```
前端 invoke bot_chat(messages, session_id)                [bot_chat.rs]
 → require_bot_enabled + ChatGuard 会话防重入
 → ① exec_steps::resume（有挂起子任务则本条是执行应答，优先返回）
 → ② bot::read_bypass_llm_switch（旧链路降级开关）
 → ③ pre-step 路由（intent_router 固定两步直调）      [intent_router.rs]
     is_chat_execute_trigger → RouteAction::ExecuteTasks → chat_execute_tasks
         （每卡走 run_task_in_chat → run_model_loop，50 轮工具循环）
     route_user_input 正则命中 → bot_skills::start_skill
         ├─ mode=auto → run_skill_scheduler（DSL 逐步执行，
         │   FailedButRecoverable 时带 recovery_hint 落入 ⑤ 让 LLM 兜底）
         └─ mode=interactive → 技能正文拼进 system prompt，继续 ⑤
 → ⑤ bot_model_loop::run_model_loop                      [bot_model_loop.rs]
     薄壳装配 LlmHttp（bot_get_config + keyring key + ApiProvider）+ ModelLoopDeps
     → run_model_loop_core 循环：
         SSE 流式请求（OpenAI，或经 bot_anthropic 转 Anthropic /v1/messages）
         流式片段 emit bot-chat-delta 事件给前端
         模型返回 tool_calls → bot::execute_tool_traced → execute_tool_impl   [bot/dispatch.rs]
             （turn + tool_call_id 随调用下传：tool.call / tool.return / 早退事件都带
               session_id / turn / tool_call_id，可按轮或按 id 整轮回放）
             L4 pre_tool 钩子层（bot/hooks.rs，进程级注册 + 会话级开关；只读，
                 只能拒绝/放行，fail-open）→ per-tool 规则闸（deny/ask）+
                 skill_on_step 钩子 → 子 agent 白名单闸 → 入参 JSON 闸
             **TOOLS_TABLE 查表**分发内置工具，工具执行完、L5 post_tool 钩子层
                 （可改结果，fail-open）之后，才进审计 preview 与结果回灌
                 （拦截职责在工具内部与规则表，原原子黑名单/pre-execute 链已拆除）
             **TOOLS_TABLE 查表**分发内置工具（数目见 registry::BUILTIN_TOOL_COUNT，bot/registry.rs 单一来源）：
                 bot/tools.rs 内部 tool_* / bot_fs / bot_web / bot_py / ocr / memory / bot_skills::use_skill
             后置结构化审计 tool.return + skill_on_step_post
         工具结果回灌 msgs 续聊，直到无 tool_calls 或熔断（MAX_FUNCTION_CALLS_PER_REQUEST）
     bot_plan：复杂任务前置 Planner 注入计划；连续失败触发 Replan
     技能状态：核心只**读**「活动技能快照」（deps.active_skill_run 注入回调），
               收尾/入口清理等写操作留在核心之外（见 §2.1）
 → 记忆：组装 prompt 时 memory::injection_block 注入 top 记忆；
   compact 时 memory::save_summary / apply_reflection
 → 返回 BotChatResult{text, task_refs}
```

旁路入口：

- `bot_scheduler::start_scheduler`：30s 扫定时任务 → 同一 `run_task_in_chat` 路径（interactive=false，绕开 exec_steps）
- `bot_slash::bot_stop`：置位 StopGuard 中断在途循环
- 本地 HTTP API（api_* 四件套）与 bot 解耦，只操作任务数据

## 4. 模块依赖主干

```
bot_chat(编排) → bot_model_loop(决策) → bot::execute_tool(分发；表在 bot/registry.rs)
    → bot/tools.rs 内部 tool_* / bot_fs / bot_web / bot_py / ocr / memory / bot_skills(实现)
bot_anthropic  只被 bot_model_loop 边界调用
app_state  运行期表容器（lib.rs 注入；各层 try_state 取）
db / audit / paths / error   全员共享底座
```

## 5. 配置 / prompt / 资源文件

- **运行时数据目录**（`paths::probe_log_dir` 判定，便携模式随 exe 走）：
  `wmessage.db`（SQLite）、`bot-config.json`（bot 配置，API key 已迁系统 keyring，自带 `schemaVersion`）、`bot.log`（审计）、`runtime/flags/`（`api-token.txt` / `api-enabled.flag` / `py-enabled.flag` / `bot-enabled.flag` 等运行期文件，根目录不再散落）、`profile.json` + `profile/`、`cleanup-rules.json`、`skills/<name>/SKILL.md`（用户技能）、`AI_Gen_Files/`（AI 产物唯一入口 `db::gen_dir`）
- **Prompt 集中在 `src-tauri/src/prompts/`**（`&'static str` 编译期常量，无 IO 无状态）：
  `system.rs`（`SYSTEM_PROMPT`）、`summary.rs`（`SUMMARY_SYSTEM_PROMPT` / `COMPACT_SYSTEM_PROMPT`）、
  `reflection.rs`、`execute.rs`（`EXECUTE_SYSTEM_PROMPT` / `STEPWISE_ADDENDUM`）、
  `planner.rs`（`PLANNER_PROMPT` / `REPLANNER_PROMPT`）、`consolidate.rs`；统一从
  `crate::prompts::NAME` 取。**不是** `.md` 资源文件（`.md` + `include_str!` 会引入真实换行 =
  改提示词字节内容，属需单独评估的改动；若将来要换实现，调用方路径不变）。
  工具 schema 单一来源 `TOOLS_TABLE`（bot/registry.rs:716）→ `tools_json()`（:1153，编译期常量原文拼接）
- **打包资源**（tauri.conf.json resources）：
  `bge-small-zh-v1.5/`（嵌入模型 tokenizer+ONNX，项目根）、`pp-ocr-v6/`（Windows OCR 模型，scripts/fetch_ocr_models.sh 下载）、`src-tauri/icons/`、权限文件 `src-tauri/capabilities/default.json`（opener scope 收窄，有回归测试锁死）
- **设计文档**：`docs/`（BOT-MEMORY-V2-DESIGN.md、SKILL-RUNTIME.md、SKILL-DSL.md、TASK-CHAT-EXECUTION-DESIGN.md、ARCH-REFACTOR-PLAN.md 及多份审计报告）
- **Rust 侧测试**：`src-tauri/tests/`（llm_integration、task_chat_exec、mock_llm 等，走 `run_model_loop_core` 可注入路径）

## 6. Agent 运行步骤及逻辑图（Mermaid）

### 6.1 主流程（bot_chat 五步）

```mermaid
flowchart TD
    A[前端 invoke bot_chat<br/>messages + session_id] --> B{bot 总开关?<br/>require_bot_enabled}
    B -- 关 --> B1[返回拒绝]
    B -- 开 --> C{ChatGuard<br/>会话防重入}
    C -- 已在聊 --> C1[返回繁忙]
    C -- 空闲 --> D{① exec_steps::resume<br/>有挂起子任务?}
    D -- 有 --> D1[本条作为执行应答<br/>继续/重做/停] --> Z
    D -- 无 --> E{② bypass_llm 开关?}
    E -- 开 --> E1[旧链路降级处理] --> Z
    E -- 关 --> F[③ pre-step 路由<br/>intent_router 固定两步直调]

    F --> G{is_chat_execute_trigger<br/>命中任务卡?}
    G -- ExecuteTasks --> G1[chat_execute_tasks<br/>每卡 run_task_in_chat<br/>interactive=true] --> L
    G -- 通过 --> H{route_user_input<br/>L1 正则硬锁}
    H -- 命中技能 --> I{技能 mode?}
    I -- auto --> J[run_skill_scheduler<br/>DSL 逐步执行]
    J --> J1{DslOutcome}
    J1 -- Done/AwaitUser --> Z
    J1 -- FailedButRecoverable --> K[带 recovery_hint<br/>落入步骤⑤ LLM 兜底]
    I -- interactive --> K2[技能正文拼入 system prompt] --> L
    H -- PassThrough --> L

    L[⑤ bot_model_loop::run_model_loop<br/>装配 LlmHttp + ModelLoopDeps] --> M[run_model_loop_core 循环<br/>≤50 轮]
    M --> N{bot_plan: 复杂任务?}
    N -- 是 --> N1[Planner 生成计划注入 prompt]
    N -- 否 --> O
    N1 --> O[SSE 流式请求 LLM<br/>OpenAI 或 bot_anthropic 转 Anthropic]
    O --> O1[流式片段 emit bot-chat-delta → 前端]
    O --> P{返回 tool_calls?}
    P -- 有 --> Q[bot::execute_tool → execute_tool_impl<br/>前置闸（工具内部按 session 判）+ skill 钩子]
    Q --> R[TOOLS_TABLE 查表分发内置工具（数目见 registry::BUILTIN_TOOL_COUNT）:<br/>bot/tools.rs 内部 tool_* / bot_fs /<br/>bot_web / bot_py / ocr / memory / bot_skills]
    R --> R1[L5 post_tool 钩子层（可改结果）] --> R1b[后置审计 tool.return] --> R2[结果回灌 msgs] --> M
    P -- 无 --> S{熔断或结束}
    S --> S1[L6 post_turn 钩子层（可改最终回复）]
    S1 --> T{连续失败?}
    T -- 是 --> T1[bot_plan Replan ≤2 次] --> M
    T -- 否 --> U[返回 BotChatResult<br/>text + task_refs]
    U --> Z([结束])

    M -.->|任意时刻| ST[/stop: StopGuard 置位<br/>中断在途循环/]
    ST --> Z
```

### 6.2 记忆与调度旁路

```mermaid
flowchart LR
    subgraph 记忆 memory/
        M1[injection_block<br/>组装 prompt 时注入 top 记忆] 
        M2[embed.rs bge-small-zh ONNX<br/>失败降级关键词]
        M3[rank.rs 混合打分<br/>0.55语义+0.20关键词+0.15重要度+0.10新近]
        M4[consolidate.rs 每10分钟<br/>LLM 反思合并/裁决矛盾]
        M1 --> M2 --> M3
        M4 --> M2
    end

    subgraph 定时调度 bot_scheduler
        S1[30s 扫描 schedule<br/>每日/每周/at:] --> S2[到点 run_task_in_chat<br/>interactive=false 绕开 exec_steps]
        S2 --> S3[完成发系统通知]
    end

    subgraph 本地 API 与 bot 解耦
        H1[api_server 127.0.0.1<br/>Bearer token] --> H2[api_handlers<br/>/api/tasks CRUD + SSE]
        H2 --> H3[(db/ SQLite)]
    end

    BC[bot_chat 主流程] --> M1
    BC2[bot_compact] --> M5[save_summary / apply_reflection]
    S2 -.同一执行路径.-> BC
```

### 6.3 分层依赖

```mermaid
flowchart TD
    编排[bot_chat 编排层] --> 决策[bot_model_loop 决策层<br/>+ bot_anthropic 协议适配]
    决策 --> 分发[bot/dispatch.rs execute_tool 分发层<br/>表在 bot/registry.rs]
    分发 --> 工具[工具实现层<br/>bot_fs / bot_web / bot_py / ocr / memory / bot_skills]
    闸[intent_router / tool_guard<br/>前置路由 / 会话闸] -.切入.-> 编排
    闸 -.切入.-> 分发
    底座[共享底座 db / audit / paths / error] -.被全员依赖.-> 编排
    底座 -.-> 决策
    底座 -.-> 工具
```

## 6.4 源文件清单（完整列表，由 `tests-audit/audit_module_map.py` 对拍）

`src-tauri/src/` 实际 157 个 `.rs` 文件（按模块分组）。本节由 `audit_module_map.py`
对拍：每条必须指向真实文件，每个源文件必须在本节列出。新增模块请同步登记。

### 顶层
- `api.rs`
- `api_auth.rs`
- `api_server.rs`
- `app_state.rs`
- `ask_engine.rs`
- `audit.rs`
- `bing.rs`
- `bot.rs`
- `bot_anthropic.rs`
- `bot_archive.rs`
- `bot_artifacts.rs`
- `bot_chat.rs`
- `bot_desktop.rs`
- `bot_fs.rs`
- `bot_manage_file.rs`
- `bot_model_loop.rs`
- `bot_onions.rs`
- `bot_orchestrator.rs`
- `bot_plan.rs`
- `bot_py.rs`
- `bot_reminders.rs`
- `bot_scheduler.rs`
- `bot_slash.rs`
- `bot_ui.rs`
- `bot_web.rs`
- `due_notify.rs`
- `error.rs`
- `exec_steps.rs`
- `intent_router.rs`
- `lib.rs`
- `llm_text.rs`
- `main.rs`
- `mutation.rs`
- `notifications.rs`
- `ocr.rs`
- `overflow.rs`
- `paths.rs`
- `profile.rs`
- `prompt_builder.rs`
- `tag_similar.rs`
- `task_autotag.rs`
- `task_out.rs`
- `tool_guard.rs`
- `trace_sink.rs`

### `api_handlers/`
- `api_handlers/body.rs`
- `api_handlers/commands.rs`
- `api_handlers/handlers.rs`
- `api_handlers/mod.rs`
- `api_handlers/ratelimit.rs`
- `api_handlers/sse.rs`
- `api_handlers/types.rs`
- `api_handlers/util.rs`
- `api_handlers/validate.rs`

### `apps/`
- `apps/commands.rs`
- `apps/manage.rs`
- `apps/mod.rs`

### `bot/`
- `bot/compaction.rs`
- `bot/config/audit.rs`
- `bot/config/commands.rs`
- `bot/config/io.rs`
- `bot/config/keyring.rs`
- `bot/config/mod.rs`
- `bot/config/schema.rs`
- `bot/config/types.rs`
- `bot/corrections.rs`
- `bot/dispatch.rs`
- `bot/format.rs`
- `bot/hooks.rs`
- `bot/log_sanitize.rs`
- `bot/mcp/builtin.rs`
- `bot/mcp/commands.rs`
- `bot/mcp/config.rs`
- `bot/mcp/manager.rs`
- `bot/mcp/mod.rs`
- `bot/mcp/mount.rs`
- `bot/mcp/secrets.rs`
- `bot/mcp/stdio_transport.rs`
- `bot/params.rs`
- `bot/procs.rs`
- `bot/procs/tests.rs`
- `bot/reasoning.rs`
- `bot/registry.rs`
- `bot/services.rs`
- `bot/services/tests.rs`
- `bot/services/watchdog.rs`
- `bot/tags_consolidate.rs`
- `bot/tools.rs`

### `bot_skills/`
- `bot_skills/builtin.rs`
- `bot_skills/files.rs`
- `bot_skills/manage.rs`
- `bot_skills/mod.rs`
- `bot_skills/parse.rs`
- `bot_skills/recommend.rs`
- `bot_skills/remote.rs`
- `bot_skills/runtime.rs`
- `bot_skills/scheduler.rs`
- `bot_skills/state.rs`
- `bot_skills/vars.rs`

### `db/`
- `db/apps.rs`
- `db/bot_history.rs`
- `db/bot_sessions.rs`
- `db/lock.rs`
- `db/migrations.rs`
- `db/mod.rs`
- `db/paths.rs`
- `db/people.rs`
- `db/reminders.rs`
- `db/schedule_jobs.rs`
- `db/skill_out.rs`
- `db/subagents.rs`
- `db/tasks.rs`
- `db/trace.rs`
- `db/workspace.rs`

### `memory/`
- `memory/consolidate.rs`
- `memory/consolidate/tests.rs`
- `memory/embed.rs`
- `memory/extract.rs`
- `memory/mod.rs`
- `memory/panel.rs`
- `memory/rank.rs`
- `memory/store.rs`
- `memory/tests.rs`

### `meta/`
- `meta/mod.rs`
- `meta/sync.rs`

### `migration/`
- `migration/commands.rs`
- `migration/journal.rs`
- `migration/mod.rs`
- `migration/ops.rs`
- `migration/recovery.rs`
- `migration/rules.rs`
- `migration/run.rs`
- `migration/scan.rs`
- `migration/sources.rs`
- `migration/types.rs`

### `platform/`
- `platform/copy_file.rs`
- `platform/mod.rs`

### `prompts/`
- `prompts/consolidate.rs`
- `prompts/execute.rs`
- `prompts/extract.rs`
- `prompts/mod.rs`
- `prompts/planner.rs`
- `prompts/selfcheck.rs`
- `prompts/subagent.rs`
- `prompts/summary.rs`
- `prompts/system.rs`

### `py/`
- `py/ast.rs`
- `py/audit.rs`
- `py/commands.rs`
- `py/document.rs`
- `py/env.rs`
- `py/harvest.rs`
- `py/io.rs`
- `py/mod.rs`
- `py/runtime.rs`

### `ui/`
- `ui/macos.rs`
- `ui/mod.rs`
- `ui/windows.rs`
