// EvolutionPanel — R5 决策面板（前端）
//
// 功能：
// 1. 列出候选池（按 status 过滤：pooled / promoted / expired / rejected）
// 2. 每条展示：证据（summary + occurrence_count）/ 影响（impact + layer）/ 操作
// 3. 操作：Promote / Reject / Keep Shadow（每条 3 按钮 + status=Active 的回滚按钮）
// 4. 后端复用 ConfirmMap（spec R0 #1 默认 A：复用 widget 弹窗）

import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { handleCommandError } from "../../lib/errorHandler";
import {
  type ChangeRecord,
  type ProposalEntry,
  type ProposalStatus,
  IMPACT_LABEL,
  LAYER_LABEL,
  STATUS_LABEL,
  formatTs,
} from "./types";

type StatusFilter = ProposalStatus | null;

export function EvolutionPanel() {
  const [proposals, setProposals] = useState<ProposalEntry[]>([]);
  const [changes, setChanges] = useState<ChangeRecord[]>([]);
  const [filter, setFilter] = useState<StatusFilter>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [info, setInfo] = useState("");

  const refresh = useCallback(async () => {
    try {
      const statusArg = filter ?? undefined;
      const p = await invoke<ProposalEntry[]>("evolution_list_proposals", {
        status: statusArg,
      });
      setProposals(p);
      const c = await invoke<ChangeRecord[]>("evolution_list_changes");
      setChanges(c);
      setError("");
    } catch (e) {
      handleCommandError(e, "evolution-panel", { silent: true });
      setError(String(e));
    }
  }, [filter]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

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

  const onPromote = (id: string) =>
    void runCmd(
      "evolution_promote_proposal",
      { proposalId: id, interactive: true, sessionId: null },
      `已晋升 ${id}`
    );

  const onReject = (id: string) =>
    void runCmd(
      "evolution_reject_proposal",
      { proposalId: id, interactive: true, sessionId: null },
      `已拒绝 ${id}`
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

  const activeChanges = changes.filter((c) => c.status === "active");

  return (
    <div className="nm-card p-6 space-y-6">
      <div className="flex items-center justify-between">
        <h2 className="text-xl font-semibold text-[var(--t2)]">
          自进化决策面板
        </h2>
        <button
          className="nm-btn px-3 py-1.5 text-sm text-[var(--t3)]"
          onClick={() => void refresh()}
          disabled={busy}
        >
          🔄 刷新
        </button>
      </div>

      {info && (
        <p className="text-sm text-[var(--success)] bg-[var(--inset)] px-3 py-2 rounded-xl">
          {info}
        </p>
      )}
      {error && (
        <p className="text-sm text-[var(--danger)] bg-[var(--inset)] px-3 py-2 rounded-xl">
          {error}
        </p>
      )}

      {/* 候选池列表 */}
      <section>
        <div className="flex items-center justify-between mb-3">
          <h3 className="text-base font-semibold text-[var(--t2)]">
            候选池（{proposals.length}）
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
          <p className="text-sm text-[var(--t4)] px-3 py-4">
            （空 — 当前筛选下没有候选）
          </p>
        ) : (
          <div className="space-y-3">
            {proposals.map((p) => (
              <ProposalCard
                key={p.proposal_id}
                proposal={p}
                busy={busy}
                onPromote={onPromote}
                onReject={onReject}
                onKeepShadow={onKeepShadow}
              />
            ))}
          </div>
        )}
      </section>

      {/* 回滚区（Active ChangeRecord） */}
      <section>
        <h3 className="text-base font-semibold text-[var(--t2)] mb-3">
          回滚（active ChangeRecord：{activeChanges.length}）
        </h3>
        {activeChanges.length === 0 ? (
          <p className="text-sm text-[var(--t4)] px-3 py-4">（无 active 记录）</p>
        ) : (
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
        )}
      </section>
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
  onPromote,
  onReject,
  onKeepShadow,
}: {
  proposal: ProposalEntry;
  busy: boolean;
  onPromote: (id: string) => void;
  onReject: (id: string) => void;
  onKeepShadow: (id: string) => void;
}) {
  const isPending = proposal.status === "pooled";
  return (
    <div className="nm-inset p-4 space-y-2">
      <div className="flex items-start justify-between">
        <div>
          <div className="text-sm font-mono text-[var(--t3)]">
            {proposal.proposal_id}
          </div>
          <div className="text-base text-[var(--t2)] mt-1">
            {proposal.suggestion_text}
          </div>
        </div>
        <div className="flex flex-col items-end gap-1">
          <span className="nm-tag text-xs">
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
      </div>

      {/* 操作 */}
      <div className="flex gap-2 pt-2">
        <button
          className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)] disabled:opacity-50"
          onClick={() => onPromote(proposal.proposal_id)}
          disabled={busy || !isPending}
          title={isPending ? "晋升（ConfirmMap 弹窗确认）" : "非 pooled 状态不可晋升"}
        >
          ✅ Promote
        </button>
        <button
          className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)] disabled:opacity-50"
          onClick={() => onReject(proposal.proposal_id)}
          disabled={busy || proposal.status === "rejected"}
          title="拒绝"
        >
          ⛔ Reject
        </button>
        <button
          className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)] disabled:opacity-50"
          onClick={() => onKeepShadow(proposal.proposal_id)}
          disabled={busy || !isPending}
          title={isPending ? "延长 shadow 期（重置 TTL）" : "非 pooled 状态不可 keep shadow"}
        >
          ⏳ Keep Shadow
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
        ↩️ 回滚
      </button>
    </div>
  );
}

/** 格式化 ProposalTarget 为可读字符串 */
function formatTarget(t: import("./types").ProposalTarget): string {
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