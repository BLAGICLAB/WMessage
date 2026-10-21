// ChatPanel 子模块：纯类型定义（无 React / 无 IO）。

/** 回合状态（后端 TurnStatus 的 camelCase 镜像，见 bot_model_loop::TurnStatus） */
export type TurnStatus = "running" | "settling" | "completed" | "interrupted" | "failed";

export type TaskRef = { id: string; title: string };

/** 工具调用行：折叠显示，展开可看入参；bot-tool-done 携带
 *  result（截 2000 字）/ms/ok（成败口径同后端 tool_call_failed）。
 *  - startedAt: bot-tool-running 到达时的本地时刻（ms），用于未完成时秒级计时；
 *    完成后以 done 事件的 ms 为准，不计前端计时（避免两端重复计） */
export type ToolCall = {
  id: string;
  name: string;
  args?: string;
  done?: boolean;
  result?: string;
  ms?: number;
  ok?: boolean;
  /** 本地开始时刻（Date.now()）；仅未完成态用于计时 */
  startedAt?: number;
  /** 入参预览（bot-tool-running 带的截断版，用于长工具的「正在跑什么」提示） */
  argsPreview?: string;
};

/** 结构化文件变更：bot-file-changed 事件载荷的会话内累积条目，
 *  替代原先从正文正则抽路径的近似（正则保留为兜底） */
export type FileChangeLite = {
  path: string;
  kind: "create" | "modify" | "delete";
  added: number;
  deleted: number;
};

/** 执行过程详细度（verbose 三档）：简洁=只看工具名；详细=可展开入参/结果；
 *  调试=默认展开+耗时+轮次。localStorage 持久化，仅影响展示不改数据流 */
export type VerboseLevel = "concise" | "detailed" | "debug";

/** Skill 失败半成品上下文（仅本会话内存；FailedButRecoverable 兜底）。
 *  后端 run_skill_scheduler 返回 DslOutcome::FailedButRecoverable { reason, completed_summary, rollback_attempted }
 *  时通过 `bot-skill-failed` SSE event 推过来；前端把它存到这条消息上渲染 ⚠️ 折叠行。
 *  - completedSummary: 失败前已成功 step 的摘要（"Step N (tool): output\nStep N (tool): ..."）
 *  - rollbackAttempted: true 表示 rollback 段跑过且无错；false 表示没写或跑挂
 *  - 仅本会话内存，刷新/重启后丢失（DB 持久化要改 src-tauri/） */
export type SkillFailure = {
  skillName: string;
  reason: string;
  completedSummary: string;
  rollbackAttempted: boolean;
};

export type Msg = {
  role: "user" | "assistant";
  content: string;
  streaming?: boolean;
  /** 回合状态机（借鉴 ZCode turn view model）：running=在跑；settling=正文已出、
   *  收尾中（终稿自检等），此时**仍算进行中**——这是「说完了但还在动」的唯一表达；
   *  completed/interrupted/failed 为终态。前端据此决定是否显示活体指示。 */
  turnStatus?: TurnStatus;
  /** 当前阶段文案（「正在思考 · 第 N 轮」/「收尾中 · 校验答复」等） */
  turnDetail?: string;
  /** 本轮开始的本地时刻（ms），用于「持续了 N 秒」秒级刷新 */
  turnStartedAt?: number;
  /** 本轮涉及的任务（渲染成可点击按钮，点击去主窗口打开该任务） */
  refs?: TaskRef[];
  /** 模型思考过程（折叠显示，不持久化） */
  thinking?: string;
  /** 本轮工具调用行（折叠显示） */
  tools?: ToolCall[];
  /** 本轮结构化文件变更（bot-file-changed 累积；FileSummary 优先用，
   *  空时回退正文正则抽取） */
  fileChanges?: FileChangeLite[];
  /** Skill 失败半成品上下文（折叠显示 ⚠️ 行；见 SkillFailure 说明） */
  skillFailure?: SkillFailure;
  /** 「查看执行对话」跳转按钮（任务执行聊天化：busy 时执行跳转排队，
   *  忙完提示 + 点击切到该执行会话） */
  actionSessionId?: string;
  /** 本地瞬态提示（addHint 造）：只显示，不落库、不进模型上下文。
   *  过滤点四处：send / retry / compact 的历史构造 + persistHistory 收口 */
  hint?: boolean;
  /** 本回合的 trace id（后端 bot_chat 回传）：把这条答复关联到它的执行轨迹。
   *  null = 本回合没开 trace。暂不落库（bot_messages 无此列），刷新后不保留 */
  traceId?: number | null;
};

/** 会话（bot_sessions_load 载荷 = Rust BotSession camelCase）。
 *  时间戳可选：chat-open-session 前端补行的会话不带（按无时间渲染）。
 *  后端载荷里的 isSubagent 位恒为 false（列表已过滤子 agent 会话），
 *  这边没有消费者——不按它路由 */
export type Session = {
  id: string;
  title: string;
  createdAt?: number;
  updatedAt?: number;
};

/** 子 agent 执行行（list_subagents 命令 = subagents 表行 camelCase + 内存态轮数）。
 *  子 agent 不再建任务卡也不建独立会话——它的可见性完全落在聊天记录里，
 *  这一行就是那条记录的数据源。taskId 仅老数据有（无卡行为 null）。
 *  时间戳是毫秒（后端 now_ms），前端直接与 Date.now() 相减算运行时长。
 *  rounds* 只在途时有值：轮数活在 tool_guard 注册表，终态守卫注销即读不到，
 *  报 null 而非 0——凭空造数比不报更坏。 */
export type SubagentRow = {
  id: string;
  status: "queued" | "running" | "succeeded" | "failed" | "cancelled" | "budget_exceeded";
  profile: string;
  /** per-spawn 透传的模型名；null = 回退当前 active model */
  model?: string | null;
  parentSessionId?: string | null;
  sessionId?: string | null;
  taskId?: string | null;
  objective: string;
  error?: string | null;
  contextSummary?: string | null;
  /** 验收标准 JSON 数组原文；详情视图解析，解析失败落回原文显示 */
  acceptanceJson?: string | null;
  /** 收尾结构化结果 JSON 原文，同上 */
  resultJson?: string | null;
  /** SubagentBudget 的 JSON 原文（NOT NULL 列：所有子 agent 行都有值） */
  budgetJson?: string | null;
  /** 贯穿 bot.log 的串链键（NOT NULL 列） */
  traceId: string;
  /** 毫秒时间戳（NOT NULL 列） */
  createdAt: number;
  startedAt?: number | null;
  finishedAt?: number | null;
  /** 已跑轮数；在途才有 */
  roundsUsed?: number | null;
  /** 轮数硬顶；与轮数同报，10/20 与 10/200 的紧迫度完全不同 */
  roundsBudget?: number | null;
};

/** 🧠 模型下拉条目（bot_get_config 的 modelsByProvider 当前协议子列表）。
 *  与设置页 ModelEntry 同形（id/label/model），但独立声明——SettingsPage/types.ts
 *  头部注明「外部不直接引用」，ChatPanel 不跨目录 import。 */
export type ChatModelEntry = {
  id: string;
  label: string;
  model: string;
  /** 所属厂商名：命中设置页 disabledVendors 时从 🧠 下拉过滤 */
  vendor?: string;
  /** 启用开关（设置页模型行）：false = 未启用，🧠 下拉不显示；缺省视为启用 */
  enabled?: boolean;
  /** 上下文窗口（千 token， 徽标同源）： 水位条分母；缺省 = 不显示百分比 */
  contextK?: number;
};

/** 模型下拉条目（双协议同列）：在 ChatModelEntry 上带来源协议，供分组展示 */
export type ModelItem = ChatModelEntry & { provider: "openai" | "anthropic" };

/** 推理强度抽象档位：与后端 EffortLevel::from_cfg 的合法值一一对应 */
export type ReasoningLevel = "off" | "low" | "medium" | "high";

/** 授权模式（只读展示）：与后端 PermMode::from_cfg 的合法值一一对应，None = ask；
 *  auto = 白名单内自动（含覆盖写免确认）、白名单外 ask（acceptEdits 语义） */
export type PermMode = "ask" | "strict" | "auto" | "yolo";
