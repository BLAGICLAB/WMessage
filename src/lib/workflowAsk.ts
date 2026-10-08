import { invoke } from "@tauri-apps/api/core";

/** 澄清问题（与 Rust workflow_clarify::ClarifyQuestion 对应；default = AI 假设，红线：必非空） */
export interface ClarifyQuestion {
  id: string;
  question: string;
  why: string | null;
  options: string[];
  /** AI 推荐假设——用户不答时按它继续 */
  default: string | null;
}

/** 澄清结果（questions 空 = 信息足够，直接拆解） */
export interface ClarifyResult {
  questions: ClarifyQuestion[];
  attempts: number;
}

/** 澄清问答对（随 workflow_decompose 回传注入 prompt） */
export interface Clarification {
  question: string;
  answer: string;
}

/** 工作流执行提问通知 payload（notifications 表 kind=workflow_question） */
export interface WorkflowQuestionPayload {
  questionId: string;
  workflowId: string;
  workflowName?: string;
  taskId?: string;
  nodeTitle?: string;
  question: string;
  why?: string;
  options?: string[];
  assumption?: string;
}

/** 拆解前澄清：失败时服务端已降级空 questions——增强非闸门 */
export function clarifyWorkflow(goal: string, attachments: string[]): Promise<ClarifyResult> {
  return invoke<ClarifyResult>("workflow_clarify", {
    goal,
    attachments: attachments.length ? attachments : null,
  });
}

/** 回答工作流执行提问：answer（富回答）/ assume（按 AI 假设继续）/ dismiss（忽略≈按假设） */
export function respondWorkflowQuestion(
  questionId: string,
  action: "answer" | "assume" | "dismiss",
  answer?: string
): Promise<void> {
  return invoke("workflow_question_respond", {
    questionId,
    action,
    answer: answer ?? null,
  });
}
