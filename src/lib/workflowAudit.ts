import { invoke } from "@tauri-apps/api/core";

/** 审计条目（与 Rust db::workflow_audit::AuditEntry 对应；按 run 分组展示） */
export interface WorkflowAuditEntry {
  id: number;
  workflowId: string;
  runStartedAt: number;
  nodeTaskId: string | null;
  kind: string;
  level: string;
  payload: Record<string, unknown> | null;
  createdAt: number;
}

/** 工作流设置（与 Rust db::workflow_settings::WorkflowSettingsView 对应） */
export interface WorkflowSettings {
  nodeAcceptance: boolean;
  auditRetentionRuns: number;
  /** 轻量评审模型（模型库条目 id）；空串 = 跟随全局 active */
  reviewModel: string;
  /** ask_user 提问预算（每次任务执行/工作流节点，1–5，默认 3） */
  askBudget: number;
}

/** 审计列表（新→旧，limit 缺省 200 上限 500） */
export function listWorkflowAudit(
  workflowId: string,
  limit?: number,
): Promise<WorkflowAuditEntry[]> {
  return invoke<WorkflowAuditEntry[]>("workflow_audit_list", {
    workflowId,
    limit: limit ?? null,
  });
}

/** 清空全部工作流审计；返回删除行数 */
export function clearAllWorkflowAudit(): Promise<number> {
  return invoke<number>("workflow_audit_clear_all", {});
}

/** 审计导出 JSON 到指定路径；返回条数 */
export function exportWorkflowAudit(
  workflowId: string,
  path: string,
): Promise<number> {
  return invoke<number>("workflow_audit_export", { workflowId, path });
}

/** 读工作流设置 */
export function getWorkflowSettings(): Promise<WorkflowSettings> {
  return invoke<WorkflowSettings>("workflow_settings_get", {});
}

/** 写工作流设置（两参均可选 = 只改传了的）；返回写后全量 */
export function setWorkflowSettings(patch: {
  nodeAcceptance?: boolean;
  auditRetentionRuns?: number;
  reviewModel?: string;
  askBudget?: number;
}): Promise<WorkflowSettings> {
  return invoke<WorkflowSettings>("workflow_settings_set", {
    nodeAcceptance: patch.nodeAcceptance ?? null,
    auditRetentionRuns: patch.auditRetentionRuns ?? null,
    reviewModel: patch.reviewModel ?? null,
    askBudget: patch.askBudget ?? null,
  });
}
