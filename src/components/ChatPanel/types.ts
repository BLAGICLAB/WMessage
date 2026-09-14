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

export type Session = { id: string; title: string };
