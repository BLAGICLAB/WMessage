// EvolutionPanel TS 类型 — 镜像后端 serde schema
//
// 后端约定：所有 struct 字段用 #[serde(rename_all = "snake_case")]；
// 枚举标签用 #[serde(tag = "kind", rename_all = "snake_case")]（ProposalTarget）。
// Tauri 2 invoke 自动把 JS camelCase 转为 Rust snake_case（命令参数名）。

export type ProposalStatus = "pooled" | "promoted" | "expired" | "rejected";

export type EvolutionLayer =
  | "parameter"
  | "policy"
  | "prompt_hint"
  | "tool_schema"
  | "skill"
  | "code";

export type ImpactLevel = "low" | "medium" | "high";

type ProposalOrigin =
  | "consolidation_reflection"
  | "scheduler_audit"
  | "user_triggered";

export type ProposalTarget =
  | { kind: "prompt_section"; name: string }
  | { kind: "tool_schema"; tool_name: string }
  | { kind: "skill_dsl"; skill_name: string }
  | { kind: "memory_policy"; policy: string };

export interface ProposalEntry {
  proposal_id: string;
  change_id: string;
  layer: EvolutionLayer;
  impact: ImpactLevel;
  origin: ProposalOrigin;
  target: ProposalTarget;
  suggestion_text: string;
  mem_key: string;
  related_refs: string[];
  summary: string;
  occurrence_count: number;
  window_hours: number;
  created_at_ms: number;
  expires_at_ms: number;
  status: ProposalStatus;
}

type ChangeStatus =
  | "pending"
  | "shadowing"
  | "shadow_passed"
  | "approved"
  | "canary"
  | "active"
  | "rejected"
  | "rolled_back"
  | "expired";

type ApprovalSource =
  | "pending"
  | "auto_applied"
  | "human_approved"
  | "system_rejected"
  | "human_rejected";

interface EvalResult {
  task_success_rate: number;
  tool_call_efficiency: number;
  behavior_deviation: number;
  rollback_rate: number;
  pollution_survival_days: number;
  evaluated_at_ms: number;
}

export interface ChangeRecord {
  change_id: string;
  parent_id: string | null;
  schema_version: number;
  layer: EvolutionLayer;
  origin: ProposalOrigin;
  proposal_id: string;
  target: ProposalTarget;
  suggestion_text: string;
  mem_key: string;
  impact: ImpactLevel;
  eval_before: EvalResult | null;
  eval_after: EvalResult | null;
  status: ChangeStatus;
  hard_constraint_compliance: boolean;
  approval_source: ApprovalSource;
  human_approver: string | null;
  created_at_ms: number;
  rolled_back_at: number | null;
  rollback_reason: string | null;
}

/** 后端相对时间戳（epoch ms） → 可读字符串 */
export function formatTs(ms: number): string {
  return new Date(ms).toLocaleString();
}

/** ProposalStatus 中文标签 */
export const STATUS_LABEL: Record<ProposalStatus, string> = {
  pooled: "候选",
  promoted: "已晋升",
  expired: "过期",
  rejected: "已拒绝",
};

/** EvolutionLayer 中文标签 */
export const LAYER_LABEL: Record<EvolutionLayer, string> = {
  parameter: "参数",
  policy: "策略",
  prompt_hint: "PromptHint",
  tool_schema: "ToolSchema",
  skill: "技能",
  code: "代码",
};

/** ImpactLevel 中文标签 */
export const IMPACT_LABEL: Record<ImpactLevel, string> = {
  low: "低",
  medium: "中",
  high: "高",
};