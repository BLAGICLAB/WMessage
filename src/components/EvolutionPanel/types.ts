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

/**  W2：自进化应用策略二档（后端缺字段/非法值 = auto = 自动生效） */
export type ApplyPolicy = "auto" | "confirm";

/** 档位中文标签（本模块内构建 LABELS 用，外部只消费 APPLY_POLICY_LABELS） */
const APPLY_POLICY_LABEL: Record<ApplyPolicy, string> = {
  auto: "自动生效",
  confirm: "需我确认",
};

/** radiogroup 渲染用档位清单（label 唯一事实源的派生形态） */
export const APPLY_POLICY_LABELS = (
  Object.keys(APPLY_POLICY_LABEL) as ApplyPolicy[]
).map((value) => ({ value, label: APPLY_POLICY_LABEL[value] }));

/**  W3：observe::compute_metrics 的四指标快照（snake_case 镜像后端 serde） */
export interface ObserveMetrics {
  candidate_generation_rate: number;
  approval_rate: number;
  rollback_rate: number;
  pollution_survival_days: number;
  proposal_total: number;
  promoted_count: number;
  rolled_back_count: number;
  active_lessons: number;
  observation_window_days: number;
  observation_window_start_ms: number;
  observation_window_end_ms: number;
  evaluated_at_ms: number;
}

// 决策证据（镜像后端 panel/evidence.rs，camelCase）

export type ShadowVerdict = "pass" | "fail" | "skipped";

interface ShadowJudgment {
  verdict: ShadowVerdict;
  /** no_baseline / would_enter_top3 / below_top3_threshold / top3_diff_but_candidate_absent */
  note: string;
}

type ConflictKind = "pooled" | "active";

interface ConflictRef {
  /** `pool:<proposal_id>` / `active:<change_id>` */
  with: string;
  kind: ConflictKind;
}

export interface ProposalEvidence {
  proposalId: string;
  shadow: ShadowJudgment;
  conflicts: ConflictRef[];
}

/** 影子判定中文标签（决策参考：静态近似，真实注入随 query 混合打分变化） */
export const SHADOW_VERDICT_LABEL: Record<ShadowVerdict, string> = {
  pass: "采纳后会进 lesson 槽位 top-3",
  fail: "采纳后进不了 top-3（重要性不够或被现有记忆覆盖）",
  skipped: "记忆库还没有 lesson，无对比基准",
};

/** 回滚预警阈值：观察窗口内回滚达到此数建议切手动档（沿袭 14 天观察期停止条件口径） */
export const ROLLBACK_WARN_THRESHOLD = 5;
// ───────────────────────── 提案派生门槛（镜像后端 derive.rs，camelCase） ─────────────────────────

export interface DeriveThresholds {
  mergeMinIds: number;
  distillMinIds: number;
  contradictionMinIds: number;
}

export const DERIVE_THRESHOLD_META: {
  key: keyof DeriveThresholds;
  label: string;
  hint: string;
  min: number;
  max: number;
}[] = [
  {
    key: "mergeMinIds",
    label: "合并提案门槛",
    hint: "一次合并至少涉及几条记忆，才生成「合并类提案」",
    min: 2,
    max: 20,
  },
  {
    key: "distillMinIds",
    label: "提炼提案门槛",
    hint: "一次提炼至少汇总几条记忆，才生成「规律类提案」（Low 档，进池不自动生效）",
    min: 2,
    max: 20,
  },
  {
    key: "contradictionMinIds",
    label: "矛盾提案门槛",
    hint: "矛盾裁决恒涉及 2 条（留一删一）；默认 1 = 任何矛盾都产提案，调到 3 以上 ≈ 关闭",
    min: 1,
    max: 99,
  },
];

/** 按记忆总量给建议值（4 档）：小库操作天然小、大库单轮整理涉及面广。
 *  矛盾门槛恒 1——矛盾天然只涉及 2 条且都是真教训，抬门槛 ≈ 关闭。 */
export function suggestedThresholds(total: number): {
  merge: number;
  distill: number;
  contradiction: number;
  tier: string;
} {
  if (total < 100) return { merge: 2, distill: 2, contradiction: 1, tier: "0–100" };
  if (total < 500) return { merge: 3, distill: 3, contradiction: 1, tier: "100–500" };
  if (total < 2000) return { merge: 3, distill: 4, contradiction: 1, tier: "500–2000" };
  return { merge: 4, distill: 5, contradiction: 1, tier: "2000+" };
}
