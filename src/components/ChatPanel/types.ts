// ChatPanel 子模块：纯类型定义（无 React / 无 IO）。

export type TaskRef = { id: string; title: string };

/** 工具调用行：折叠显示，展开可看入参 */
export type ToolCall = { id: string; name: string; args?: string; done?: boolean };

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
  /** 本轮涉及的任务（渲染成可点击按钮，点击去主窗口打开该任务） */
  refs?: TaskRef[];
  /** 模型思考过程（折叠显示，不持久化） */
  thinking?: string;
  /** 本轮工具调用行（折叠显示） */
  tools?: ToolCall[];
  /** Skill 失败半成品上下文（折叠显示 ⚠️ 行；见 SkillFailure 说明） */
  skillFailure?: SkillFailure;
  /** 「查看执行对话」跳转按钮（任务执行聊天化：busy 时执行跳转排队，
   *  忙完提示 + 点击切到该执行会话） */
  actionSessionId?: string;
};

/** 会话（bot_sessions_load 载荷 = Rust BotSession camelCase）。
 *  时间戳可选：chat-open-session 前端补行的会话不带（按无时间渲染）。
 *  isSubagent：子 agent 执行会话标记（U20D 批 5 结构化路由；旧会话无此字段
 *  = undefined，按旧标题 🧩 前缀兜底判定） */
export type Session = {
  id: string;
  title: string;
  createdAt?: number;
  updatedAt?: number;
  isSubagent?: boolean;
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
};

/** 模型下拉条目（MP-02 双协议同列）：在 ChatModelEntry 上带来源协议，供分组展示 */
export type ModelItem = ChatModelEntry & { provider: "openai" | "anthropic" };

/** 推理强度抽象档位（RE-1）：与后端 EffortLevel::from_cfg 的合法值一一对应 */
export type ReasoningLevel = "off" | "low" | "medium" | "high";

/** 授权模式（只读展示）：与后端 PermMode::from_cfg 的合法值一一对应，None = ask */
export type PermMode = "ask" | "strict" | "yolo";
