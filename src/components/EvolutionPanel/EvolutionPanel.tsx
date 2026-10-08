// EvolutionPanel —  决策面板（前端，老板 16:05 redesign）
//
// 设计（v4.1 + 16:05 ）：
// 1. 单列 list：所有 proposals（不分候选池/回滚区）
// 2. 每条行内：
//    - 右侧：Toggle pill switch（视觉主入口，图片风格红/灰）
//    - 中部：suggestion + 证据 + 影响
//    - 左侧：proposal_id + status badge
//    - 底部：[启用] [停用] [Keep Shadow] 中文按钮（兼容旧 Promote/Reject）
//    - 右下：删除（图标+文字）
// 3. 后端 toggle=true → 写 ChangeRecord + 人工批准执行器落库（ W1：
//    policy 层提案 ON 即生效，幂等）；toggle=false → 移除 pending ChangeRecord
// 4. 后端 delete → 仅删 pending ChangeRecord + proposals 行（只允许删 pending）
// 5. Rollback 区条件显示：仅当 active ChangeRecord 存在时折叠展开
// 6.  W2 头部应用策略二档 radiogroup（auto/confirm，点档即时落盘）；
//    W3 空状态「立即反思」接 memory_consolidate_now + 四指标条 + 行内决策徽标

import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AlertTriangle, RefreshCw, Trash2 } from "lucide-react";
import { handleCommandError } from "../../lib/errorHandler";
import { Toggle } from "../Toggle";
import { IconButton } from "../../ui/IconButton";
import { DeleteConfirmDialog } from "./DeleteConfirmDialog";
import {
  type ApplyPolicy,
  type ChangeRecord,
  type DeriveThresholds,
  type ObserveMetrics,
  type ProposalEntry,
  type ProposalEvidence,
  type ProposalStatus,
  type ProposalTarget,
  APPLY_POLICY_LABELS,
  DERIVE_THRESHOLD_META,
  IMPACT_LABEL,
  LAYER_LABEL,
  ROLLBACK_WARN_THRESHOLD,
  SHADOW_VERDICT_LABEL,
  STATUS_LABEL,
  formatTs,
  suggestedThresholds,
} from "./types";

type StatusFilter = ProposalStatus | null;

const ACTIVE_STATUSES: ReadonlySet<ChangeRecord["status"]> = new Set([
  "pending",
  "shadowing",
  "shadow_passed",
  "approved",
  "canary",
  "active",
]);

/** ：有回滚历史的提案再点 ON 需二次确认 */
const ROLLED_BACK_STATUS: ChangeRecord["status"] = "rolled_back";

/**  W2 应用策略二档（同记忆三档 radiogroup 先例；档位唯一事实源，aria 前缀从 label 派生防漂移） */
const APPLY_POLICY_MODES = APPLY_POLICY_LABELS.map(({ value, label }) => ({
  value,
  label,
  aria: `应用策略：${label}`,
}))

/**  W3：行内决策徽标（优先级：已自动生效 > 你已启用 > 待你决策） */
function decisionBadge(p: ProposalEntry, changes: ChangeRecord[]): string | null {
  const mine = changes.filter((c) => c.proposal_id === p.proposal_id);
  if (mine.some((c) => c.status === "active" && c.approval_source === "auto_applied")) {
    return "已自动生效";
  }
  if (
    mine.some(
      (c) =>
        c.approval_source === "human_approved" &&
        (c.status === "active" || c.status === "pending")
    )
  ) {
    return "你已启用";
  }
  if (p.status === "pooled") return "待你决策";
  return null;
}

export function EvolutionPanel() {
  const [proposals, setProposals] = useState<ProposalEntry[]>([]);
  const [changes, setChanges] = useState<ChangeRecord[]>([]);
  const [filter, setFilter] = useState<StatusFilter>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [info, setInfo] = useState("");
  // 删除确认弹窗状态（）
  const [deleteTarget, setDeleteTarget] = useState<ProposalEntry | null>(null);
  //  W2 应用策略档位（缺字段/读失败 = auto 显示，与后端同口径）
  const [applyPolicy, setApplyPolicy] = useState<ApplyPolicy>("auto");
  // 点档 in-flight guard（防连点 auto→confirm→auto 乱序回滚）
  const [policyBusy, setPolicyBusy] = useState(false);
  //  W3 四指标（读失败 = 不显示，不阻塞面板）
  const [metrics, setMetrics] = useState<ObserveMetrics | null>(null);
  // 决策证据（影子判定 + 冲突标注；读失败 = 空表，面板照常工作）
  const [evidence, setEvidence] = useState<Record<string, ProposalEvidence>>({});
  // 立即反思进行中（LLM 调用秒级，与列表 busy 分开避免整板禁用）
  const [reflecting, setReflecting] = useState(false);
  // 提案派生门槛（读失败 = 默认 2/2/1）；memTotal 用于建议值分档
  const [thresholds, setThresholds] = useState<DeriveThresholds>({
    mergeMinIds: 2,
    distillMinIds: 2,
    contradictionMinIds: 1,
  });
  const [thresholdsBusy, setThresholdsBusy] = useState(false);
  const [thresholdsMsg, setThresholdsMsg] = useState("");
  const [memTotal, setMemTotal] = useState<number | null>(null);

  const refresh = useCallback(async () => {
    try {
      const statusArg = filter ?? undefined;
      const p = await invoke<ProposalEntry[]>("evolution_list_proposals", {
        status: statusArg,
      });
      setProposals(p ?? []);
      const c = await invoke<ChangeRecord[]>("evolution_list_changes");
      setChanges(c ?? []);
      const ev = await invoke<ProposalEvidence[]>("evolution_proposal_evidence");
      setEvidence(
        Object.fromEntries((ev ?? []).map((e) => [e.proposalId, e]))
      );
      setError("");
    } catch (e) {
      handleCommandError(e, "evolution-panel", { silent: true });
      setError(String(e));
    }
  }, [filter]);

  useEffect(() => {
    // 挂载/筛选变更后刷新提案列表（外部数据同步；setState 发生在 async 内部）
    // oxlint-disable-next-line react/set-state-in-effect
    void refresh();
  }, [refresh]);

  useEffect(() => {
    // 挂载时拉一次治理开关与四指标（读失败静默降级，开关按 auto 显示）
    void (async () => {
      try {
        const p = await invoke<ApplyPolicy>("evolution_get_apply_policy");
        if (p === "auto" || p === "confirm") setApplyPolicy(p);
      } catch {
        /* 读失败按 auto 显示（后端同口径），不打断面板 */
      }
      try {
        const m = await invoke<ObserveMetrics>("evolution_metrics");
        setMetrics(m ?? null);
      } catch {
        setMetrics(null);
      }
      try {
        const t = await invoke<DeriveThresholds>("evolution_get_thresholds");
        if (t) setThresholds(t);
      } catch {
        /* 读失败保持默认 2/2/1（后端同口径） */
      }
      try {
        const st = await invoke<{ total: number } | null>("mem_stats");
        if (st) setMemTotal(st.total);
      } catch {
        setMemTotal(null);
      }
    })();
  }, []);

  // 提案派生门槛保存（服务端 clamped 兜底；同点档先例先改 state 再落盘）
  const onSaveThresholds = async () => {
    setThresholdsBusy(true);
    setThresholdsMsg("");
    try {
      await invoke("evolution_set_thresholds", { thresholds });
      setThresholdsMsg("已保存");
    } catch (e) {
      setThresholdsMsg(String(e));
    } finally {
      setThresholdsBusy(false);
      setTimeout(() => setThresholdsMsg(""), 4000);
    }
  };

  //  W2：点档即时落盘（同记忆三档先例：先改 state 再落盘，失败回滚+报错；
  // in-flight 期间忽略并发点档，防乱序回滚）
  const onApplyPolicyChange = (next: ApplyPolicy) => {
    if (policyBusy || next === applyPolicy) return;
    const prev = applyPolicy;
    setApplyPolicy(next);
    setPolicyBusy(true);
    setError("");
    setInfo("");
    void (async () => {
      try {
        await invoke("evolution_set_apply_policy", { policy: next });
      } catch (e) {
        setApplyPolicy(prev);
        handleCommandError(e, "evolution-panel", { silent: true });
        setError(String(e));
      } finally {
        setPolicyBusy(false);
      }
    })();
  };

  //  W3：空状态「立即反思」——对记忆库跑一轮反思，产出可决策的提案
  const onReflectNow = async () => {
    setReflecting(true);
    setError("");
    setInfo("");
    try {
      const r = await invoke<{
        merged: number;
        distilled: number;
        contradictions: number;
      }>("memory_consolidate_now");
      setInfo(`反思完成：合并 ${r.merged} · 提炼 ${r.distilled} · 裁决 ${r.contradictions}`);
      await refresh();
    } catch (e) {
      handleCommandError(e, "evolution-panel", { silent: true });
      setError(String(e));
    } finally {
      setReflecting(false);
    }
  };

  const runCmd = useCallback(
    async (
      cmd: string,
      args: Record<string, unknown>,
      successMsg: string
    ): Promise<boolean> => {
      setBusy(true);
      setError("");
      setInfo("");
      try {
        await invoke(cmd, args);
        setInfo(successMsg);
        await refresh();
        return true;
      } catch (e) {
        handleCommandError(e, "evolution-panel", { silent: true });
        setError(String(e));
        return false;
      } finally {
        setBusy(false);
      }
    },
    [refresh]
  );

  // Toggle 入口（的主操作）
  // 有回滚历史的提案再点 ON → 二次确认「上次已回滚」——
  // 禁止会锁死重试路径，静默放行易误点循环，二次确认兼顾。
  const onToggle = (id: string, enabled: boolean) => {
    if (
      enabled &&
      changes.some((c) => c.proposal_id === id && c.status === ROLLED_BACK_STATUS)
    ) {
      if (!window.confirm(`提案 ${id} 上次已回滚，确认再次启用？`)) return;
    }
    return void runCmd(
      "evolution_toggle_proposal",
      { proposalId: id, enabled },
      enabled ? `已启用 ${id}` : `已停用 ${id}`
    );
  };

  // 删除 emoji（后端口径：proposals 行无论 status 一律删，changes 仅级联删 pending）
  // 老板 16:35：弹 DeleteConfirmDialog（可勾选 cascade_source）
  const onDeleteClick = (p: ProposalEntry) => setDeleteTarget(p);
  const onDeleteConfirm = async (cascadeSource: boolean) => {
    if (!deleteTarget) return;
    const id = deleteTarget.proposal_id;
    const target = deleteTarget;
    setDeleteTarget(null);
    await runCmd(
      "evolution_delete_proposal",
      { proposalId: id, cascadeSource },
      cascadeSource
        ? `已删除 ${id}（连同 ${target.related_refs.length} 条源记忆）`
        : `已删除 ${id}`
    );
  };

  // 兼容旧 UI：Promote/Reject 按钮走 toggle 语义（已改后端）
  const onPromote = (id: string) =>
    void runCmd(
      "evolution_promote_proposal",
      { proposalId: id, interactive: true, sessionId: null },
      `已启用 ${id}`
    );
  const onReject = (id: string) =>
    void runCmd(
      "evolution_reject_proposal",
      { proposalId: id, interactive: true, sessionId: null },
      `已停用 ${id}`
    );
  const onKeepShadow = (id: string) =>
    void runCmd(
      "evolution_keep_shadow",
      { proposalId: id },
      `已延长 shadow 期：${id}`
    );
  const onRollback = (changeId: string) =>
    void runCmd(
      "evolution_rollback_change",
      { changeId, interactive: true, sessionId: null },
      `已回滚 ${changeId}`
    );

  // Toggle 状态：proposal 是否在 changes.jsonl 流水线中
  const isToggleOn = useCallback(
    (proposalId: string): boolean => {
      return changes.some(
        (c) =>
          c.proposal_id === proposalId &&
          ACTIVE_STATUSES.has(c.status)
      );
    },
    [changes]
  );

  const activeChanges = changes.filter((c) => c.status === "active");
  const hasActive = activeChanges.length > 0;

  return (
    <div className="nm-card p-6 space-y-6">
      <div className="flex items-center justify-between">
        <h2 className="text-xl font-semibold text-[var(--t2)]">
          自进化决策面板
        </h2>
        {/* 头部轻操作对齐全仓 IconButton 家族（同 MemoryPanel 头部刷新键） */}
        <IconButton
          aria-label="刷新提案与变更"
          title="刷新提案与变更"
          className="text-[var(--t5)] hover:text-[var(--t2)]"
          onClick={() => void refresh()}
          disabled={busy}
        >
          <RefreshCw size={13} aria-hidden />
        </IconButton>
      </div>

      {info && (
        <p
          className="text-sm text-[var(--success)] bg-[var(--inset)] px-3 py-2 rounded-xl"
          data-testid="evolution-info"
        >
          {info}
        </p>
      )}
      {error && (
        <p
          className="text-sm text-[var(--danger)] bg-[var(--inset)] px-3 py-2 rounded-xl"
          data-testid="evolution-error"
        >
          {error}
        </p>
      )}

      {/*  W2：应用策略二档（头部 radiogroup，同记忆三档样式；点档即时落盘） */}
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div className="space-y-1.5">
          <p className="text-[11px] text-[var(--t3)]">应用策略</p>
          <p className="text-[11px] text-[var(--t5)]">
            「自动生效」= 反思产出的记忆建议直接写入，不用逐条过目；「需我确认」= 一律留在下方列表，等你拨开关才生效
          </p>
          <div className="flex gap-2" role="radiogroup" aria-label="自进化应用策略">
            {APPLY_POLICY_MODES.map(({ value, label, aria }) => {
              const selected = applyPolicy === value;
              return (
                <button
                  key={value}
                  type="button"
                  role="radio"
                  aria-checked={selected}
                  aria-label={aria}
                  className={`px-3 py-1.5 text-xs text-[var(--t3)] ${
                    selected ? "nm-inset" : "nm-outset"
                  } ${policyBusy ? "opacity-50" : ""}`}
                  onClick={() => onApplyPolicyChange(value)}
                  disabled={busy || policyBusy}
                >
                  {label}
                </button>
              );
            })}
          </div>
        </div>
        {/*  W3：四指标条（读失败不显示；窗口天数取后端字段防漂移） */}
        {metrics && (
          <p
            className="text-[11px] text-[var(--t5)]"
            data-testid="evolution-metrics"
          >
            近{Math.round(metrics.observation_window_days)}天：候选{" "}
            {metrics.candidate_generation_rate.toFixed(1)} 条/天 · 通过{" "}
            {(metrics.approval_rate * 100).toFixed(0)}% · 回滚{" "}
            {(metrics.rollback_rate * 100).toFixed(0)}% · 存活{" "}
            {metrics.pollution_survival_days.toFixed(1)} 天
          </p>
        )}
        {/* 回滚预警：观察窗口内回滚过阈值 → 建议切手动档 */}
        {metrics && metrics.rolled_back_count >= ROLLBACK_WARN_THRESHOLD && (
          <p
            className="inline-flex items-center gap-1 text-[11px] text-[var(--danger)]"
            data-testid="evolution-rollback-warning"
          >
            <AlertTriangle size={11} aria-hidden />
            近{Math.round(metrics.observation_window_days)}天回滚{" "}
            {metrics.rolled_back_count} 条（≥{ROLLBACK_WARN_THRESHOLD}）：
            建议把应用策略切到「需我确认」档
          </p>
        )}
      </div>

      {/* 提案派生门槛（可设置项；按记忆总量给建议值） */}
      <div className="nm-inset p-4 space-y-2" data-testid="evolution-thresholds-card">
        <div className="flex items-center justify-between gap-3 flex-wrap">
          <p className="text-[11px] text-[var(--t3)]">提案派生门槛</p>
          {memTotal != null && (
            <p className="text-[11px] text-[var(--t5)]" data-testid="thresholds-suggestion">
              记忆 {memTotal} 条（{suggestedThresholds(memTotal).tier} 档）→
              建议 合并 {suggestedThresholds(memTotal).merge} / 提炼{" "}
              {suggestedThresholds(memTotal).distill} / 矛盾{" "}
              {suggestedThresholds(memTotal).contradiction}
            </p>
          )}
        </div>
        <div className="grid grid-cols-1 gap-2 sm:grid-cols-3">
          {DERIVE_THRESHOLD_META.map((f) => (
            <label key={f.key} className="block">
              <span className="text-[11px] text-[var(--t5)]">{f.label}</span>
              <input
                type="number"
                min={f.min}
                max={f.max}
                value={thresholds[f.key]}
                onChange={(e) => {
                  const n = Number(e.target.value);
                  if (Number.isFinite(n))
                    setThresholds((t) => ({ ...t, [f.key]: n }));
                }}
                data-testid={`thresholds-${f.key}`}
                className="nm-inset mt-0.5 w-full px-2 py-1 text-xs text-[var(--t2)]"
              />
              <span className="block mt-0.5 text-[10px] leading-snug text-[var(--t6)]">
                {f.hint}
              </span>
            </label>
          ))}
        </div>
        <div className="flex items-center gap-2">
          <button
            className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)] disabled:opacity-50"
            onClick={() => void onSaveThresholds()}
            disabled={busy || thresholdsBusy}
            data-testid="btn-thresholds-save"
          >
            保存门槛
          </button>
          {thresholdsMsg && (
            <span className="text-[11px] text-[var(--t5)]">{thresholdsMsg}</span>
          )}
        </div>
      </div>

      {/* 候选池列表 */}
      <section>
        <div className="flex items-center justify-between mb-3">
          <h3 className="text-base font-semibold text-[var(--t2)]">
            自进化提案（{proposals.length}）
          </h3>
          <div className="flex gap-1">
            <FilterBtn
              active={filter === null}
              onClick={() => setFilter(null)}
              label="全部"
            />
            {(["pooled", "promoted", "expired", "rejected"] as ProposalStatus[]).map(
              (s) => (
                <FilterBtn
                  key={s}
                  active={filter === s}
                  onClick={() => setFilter(s)}
                  label={STATUS_LABEL[s]}
                />
              )
            )}
          </div>
        </div>

        {proposals.length === 0 ? (
          <div className="px-3 py-4 space-y-2.5">
            <p className="text-sm text-[var(--t4)]">
              （空 — 当前筛选下没有提案）
            </p>
            {/*  W3：空状态「立即反思」——记忆库没提案时手动跑一轮反思 */}
            <div className="flex items-center gap-3">
              <button
                className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)] disabled:opacity-50"
                onClick={() => void onReflectNow()}
                disabled={busy || reflecting}
                data-testid="btn-reflect-now"
              >
                立即反思
              </button>
              <p className="text-[11px] text-[var(--t5)]">
                对记忆库跑一轮反思（合并 / 提炼 / 裁决），反思产出会出现在这里等你决策
              </p>
            </div>
          </div>
        ) : (
          <div className="space-y-3">
            {proposals.map((p) => (
              <ProposalCard
                key={p.proposal_id}
                proposal={p}
                busy={busy}
                toggleOn={isToggleOn(p.proposal_id)}
                decision={decisionBadge(p, changes)}
                evidence={evidence[p.proposal_id]}
                onToggle={onToggle}
                onDelete={onDeleteClick}
                onPromote={onPromote}
                onReject={onReject}
                onKeepShadow={onKeepShadow}
              />
            ))}
          </div>
        )}
      </section>

      {/* 回滚区（仅当有 active ChangeRecord 时展开，老板 16:05 ） */}
      {hasActive && (
        <section>
          <h3 className="text-base font-semibold text-[var(--t2)] mb-3">
            回滚（active ChangeRecord：{activeChanges.length}）
          </h3>
          <div className="space-y-2">
            {activeChanges.map((c) => (
              <ChangeRow
                key={c.change_id}
                change={c}
                busy={busy}
                onRollback={onRollback}
              />
            ))}
          </div>
        </section>
      )}

      {/* 删除确认弹窗 */}
      {deleteTarget && (
        <DeleteConfirmDialog
          proposal={deleteTarget}
          busy={busy}
          onConfirm={onDeleteConfirm}
          onCancel={() => setDeleteTarget(null)}
        />
      )}
    </div>
  );
}

function FilterBtn({
  active,
  onClick,
  label,
}: {
  active: boolean;
  onClick: () => void;
  label: string;
}) {
  return (
    <button
      className={`px-2 py-1 text-xs ${
        active ? "nm-inset" : "nm-outset"
      } text-[var(--t3)]`}
      onClick={onClick}
    >
      {label}
    </button>
  );
}

function ProposalCard({
  proposal,
  busy,
  toggleOn,
  decision,
  evidence,
  onToggle,
  onDelete,
  onPromote,
  onReject,
  onKeepShadow,
}: {
  proposal: ProposalEntry;
  busy: boolean;
  toggleOn: boolean;
  /**  W3 行内决策徽标（null = 不显示） */
  decision: string | null;
  /** 决策证据（影子判定 + 冲突标注；undefined = 后端不可用，不渲染） */
  evidence?: ProposalEvidence;
  onToggle: (id: string, enabled: boolean) => void;
  onDelete: (p: ProposalEntry) => void;
  onPromote: (id: string) => void;
  onReject: (id: string) => void;
  onKeepShadow: (id: string) => void;
}) {
  const isPending = proposal.status === "pooled";
  return (
    <div
      className="nm-inset p-4 space-y-2"
      data-testid={`proposal-card-${proposal.proposal_id}`}
    >
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0 flex-1">
          <div className="text-sm font-mono text-[var(--t3)]">
            {proposal.proposal_id}
          </div>
          <div className="text-base text-[var(--t2)] mt-1 break-words">
            {proposal.suggestion_text}
          </div>
        </div>
        {/* Toggle pill switch */}
        <div className="flex flex-col items-end gap-1 shrink-0">
          <Toggle
            checked={toggleOn}
            onChange={(next) => onToggle(proposal.proposal_id, next)}
            disabled={busy}
            ariaLabel={`切换 ${proposal.proposal_id}`}
          />
          {decision && (
            <span
              className="nm-tag text-xs"
              data-testid={`decision-badge-${proposal.proposal_id}`}
            >
              {decision}
            </span>
          )}
          {evidence && evidence.conflicts.length > 0 && (
            <span
              className="nm-tag inline-flex items-center gap-1 text-xs text-[var(--danger)]"
              data-testid={`conflict-badge-${proposal.proposal_id}`}
              title={`同目标对象：${evidence.conflicts.map((c) => c.with).join("、")}`}
            >
              <AlertTriangle size={11} aria-hidden />
              同目标冲突 ×{evidence.conflicts.length}
            </span>
          )}
          <span
            className="nm-tag text-xs"
            data-testid={`status-badge-${proposal.proposal_id}`}
          >
            {STATUS_LABEL[proposal.status]}
          </span>
          <span className="nm-tag text-xs">{LAYER_LABEL[proposal.layer]}</span>
          <span className="nm-tag text-xs">
            {IMPACT_LABEL[proposal.impact]}
          </span>
        </div>
      </div>

      {/* 证据 + 影响 */}
      <div className="text-xs text-[var(--t4)] space-y-1">
        <div>
          <span className="font-semibold">证据：</span>
          {proposal.summary}
          {proposal.occurrence_count > 1 &&
            `（出现 ${proposal.occurrence_count} 次 / ${proposal.window_hours}h）`}
        </div>
        <div>
          <span className="font-semibold">target：</span>
          {formatTarget(proposal.target)}
        </div>
        <div>
          <span className="font-semibold">created_at：</span>
          {formatTs(proposal.created_at_ms)}
          <span className="ml-2 font-semibold">expires_at：</span>
          {formatTs(proposal.expires_at_ms)}
        </div>
        {proposal.related_refs.length > 0 && (
          <div>
            <span className="font-semibold">refs：</span>
            {proposal.related_refs.join(", ")}
          </div>
        )}
        {evidence && (
          <div data-testid={`shadow-judgment-${proposal.proposal_id}`}>
            <span className="font-semibold">影子判定：</span>
            {SHADOW_VERDICT_LABEL[evidence.shadow.verdict]}
          </div>
        )}
      </div>

      {/* 操作行：中文按钮（兼容）+ 删除 emoji */}
      <div className="flex items-center gap-2 pt-2">
        <button
          className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)] disabled:opacity-50"
          onClick={() => onPromote(proposal.proposal_id)}
          disabled={busy || !isPending}
          title={isPending ? "启用（ConfirmMap 弹窗确认）" : "非 pooled 状态不可启用"}
          data-testid={`btn-enable-${proposal.proposal_id}`}
        >
          启用
        </button>
        <button
          className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)] disabled:opacity-50"
          onClick={() => onReject(proposal.proposal_id)}
          disabled={busy || proposal.status === "rejected"}
          title="停用（ConfirmMap 弹窗确认）"
          data-testid={`btn-disable-${proposal.proposal_id}`}
        >
          停用
        </button>
        <button
          className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)] disabled:opacity-50"
          onClick={() => onKeepShadow(proposal.proposal_id)}
          disabled={busy || !isPending}
          title={isPending ? "延长 shadow 期（重置 TTL）" : "非 pooled 状态不可 keep shadow"}
          data-testid={`btn-keep-shadow-${proposal.proposal_id}`}
        >
          延长 shadow
        </button>
        <div className="flex-1" />
        <button
          className="nm-btn inline-flex items-center gap-1 px-3 py-1.5 text-xs text-[var(--t3)] hover:text-[var(--danger)] disabled:opacity-50"
          onClick={() => onDelete(proposal)}
          disabled={busy}
          title="彻底删除（仅 status=pending 可删，其他用回滚）"
          data-testid={`btn-delete-${proposal.proposal_id}`}
        >
          <Trash2 size={11} aria-hidden />
          删除
        </button>
      </div>
    </div>
  );
}

function ChangeRow({
  change,
  busy,
  onRollback,
}: {
  change: ChangeRecord;
  busy: boolean;
  onRollback: (id: string) => void;
}) {
  return (
    <div className="nm-inset p-3 flex items-center justify-between">
      <div>
        <div className="text-sm font-mono text-[var(--t3)]">
          {change.change_id}
        </div>
        <div className="text-xs text-[var(--t4)]">
          proposal_id: {change.proposal_id} · status: {change.status} · approval:{" "}
          {change.approval_source}
          {change.human_approver && ` · by ${change.human_approver}`}
        </div>
        <div className="text-xs text-[var(--t2)] mt-1">
          {change.suggestion_text}
        </div>
      </div>
      <button
        className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)] disabled:opacity-50"
        onClick={() => onRollback(change.change_id)}
        disabled={busy}
        title="回滚：删 mem_item + status=RolledBack"
      >
        回滚
      </button>
    </div>
  );
}

/** 格式化 ProposalTarget 为可读字符串 */
function formatTarget(t: ProposalTarget): string {
  switch (t.kind) {
    case "prompt_section":
      return `prompt_section: ${t.name}`;
    case "tool_schema":
      return `tool_schema: ${t.tool_name}`;
    case "skill_dsl":
      return `skill_dsl: ${t.skill_name}`;
    case "memory_policy":
      return `memory_policy: ${t.policy}`;
  }
}
