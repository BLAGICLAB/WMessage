import { invoke } from "@tauri-apps/api/core";

/** 任务执行提问通知 payload（notifications 表 kind=workflow_question，遗留名含库内
 *  历史行；旧行可能带 workflowId/workflowName，按可选容错） */
export interface TaskQuestionPayload {
  questionId: string;
  workflowId?: string | null;
  workflowName?: string;
  taskId?: string;
  nodeTitle?: string;
  question: string;
  why?: string;
  options?: string[];
  assumption?: string;
}

/** 回答任务执行提问：answer（富回答）/ assume（按 AI 假设继续）/ dismiss（忽略≈按假设） */
export function respondTaskQuestion(
  questionId: string,
  action: "answer" | "assume" | "dismiss",
  answer?: string,
): Promise<void> {
  return invoke("ask_respond", {
    questionId,
    action,
    answer: answer ?? null,
  });
}
