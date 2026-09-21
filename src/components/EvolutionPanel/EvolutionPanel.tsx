// EvolutionPanel — R5 决策面板（前端，老板 16:05 redesign）
//
// 老板设计（v4.1 + 16:05 拍板）：
// 1. 单列 list：所有 proposals（不分候选池/回滚区）
// 2. 每条行内：
//    - 右侧：Toggle pill switch（视觉主入口，图片风格红/灰）
//    - 中部：suggestion + 证据 + 影响
//    - 左侧：proposal_id + status badge
//    - 底部：[启用] [停用] [Keep Shadow] 中文按钮（兼容旧 Promote/Reject）
//    - 右下：🗑️ 删除 emoji
// 3. 后端 toggle=true → 写 ChangeRecord(pending)；toggle=false → 移除 ChangeRecord
// 4. 后端 delete → 仅删 pending ChangeRecord + proposals 行（老板拍板只允许删 pending）
// 5. Rollback 区条件显示：仅当 active ChangeRecord 存在时折叠展开
// 6. applied.jsonl 完全不动（与 toggle 解耦，auto_apply 路径独立）

import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { handleCommandError } from "../../lib/errorHandler";
import { Toggle } from "../Toggle";
import { DeleteConfirmDialog } from "./DeleteConfirmDialog";
import {
  type ChangeRecord,
  type ProposalEntry,
  type ProposalStatus,
  type ProposalTarget,
  IMPACT_LABEL,
  LAYER_LABEL,
  STATUS_LABEL,
  formatTs,
} from "./types";

type StatusFilter = ProposalStatus | null;

const ACTIVE_STATUSES = new Set([
  "pending",
  "shadowing",
  "shadow_passed",
  "approved",
  "canary",
  "active",
]);

export function EvolutionPanel() {
  const [proposals, setProposals] = useState<ProposalEntry[]>([]);
  const [changes, setChanges] = useState<ChangeRecord[]>([]);
  const [filter, setFilter] = useState<StatusFilter>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [info, setInfo] = useState("");
  // 删除确认弹窗状态（老板 16:35 拍板）
  const [deleteTarget, setDeleteTarget] = useState<ProposalEntry | null>(null);

  const refresh = useCallback(async () => {
    try {
      const statusArg = filter ?? undefined;
      const p = await invoke<ProposalEntry[]>("evolution_list_proposals", {
        status: statusArg,
      });
      setProposals(p ?? []);
      const c = await invoke<ChangeRecord[]>("evolution_list_changes");
      setChanges(c ?? []);
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

  // Toggle 入口（老板 16:05 拍板的主操作）
  const onToggle = (id: string, enabled: boolean) =>
    void runCmd(
      "evolution_toggle_proposal",
      { proposalId: id, enabled },
      enabled ? `已启用 ${id}` : `已停用 ${id}`
    );

  // 删除 emoji（仅允许删 pending 的，老板拍板）
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
          ACTIVE_STATUSES.has(c.status as string)
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
        <button
          className="nm-btn px-3 py-1.5 text-sm text-[var(--t3)]"
          onClick={() => void refresh()}
          disabled={busy}
        >
          🔄 刷新
        </button>
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

      {/* 候选池列表（老板 16:05：去掉独立分区，单列表所有提案） */}
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
          <p className="text-sm text-[var(--t4)] px-3 py-4">
            （空 — 当前筛选下没有提案）
          </p>
        ) : (
          <div className="space-y-3">
            {proposals.map((p) => (
              <ProposalCard
                key={p.proposal_id}
                proposal={p}
                busy={busy}
                toggleOn={isToggleOn(p.proposal_id)}
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

      {/* 回滚区（仅当有 active ChangeRecord 时展开，老板 16:05 拍板） */}
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

      {/* 删除确认弹窗（老板 16:35 拍板：cascade_source 复选框） */}
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
  onToggle,
  onDelete,
  onPromote,
  onReject,
  onKeepShadow,
}: {
  proposal: ProposalEntry;
  busy: boolean;
  toggleOn: boolean;
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
        {/* Toggle pill switch（老板 16:05 拍板） */}
        <div className="flex flex-col items-end gap-1 shrink-0">
          <Toggle
            checked={toggleOn}
            onChange={(next) => onToggle(proposal.proposal_id, next)}
            disabled={busy}
            ariaLabel={`切换 ${proposal.proposal_id}`}
          />
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
          ✅ 启用
        </button>
        <button
          className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)] disabled:opacity-50"
          onClick={() => onReject(proposal.proposal_id)}
          disabled={busy || proposal.status === "rejected"}
          title="停用（ConfirmMap 弹窗确认）"
          data-testid={`btn-disable-${proposal.proposal_id}`}
        >
          ⛔ 停用
        </button>
        <button
          className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)] disabled:opacity-50"
          onClick={() => onKeepShadow(proposal.proposal_id)}
          disabled={busy || !isPending}
          title={isPending ? "延长 shadow 期（重置 TTL）" : "非 pooled 状态不可 keep shadow"}
          data-testid={`btn-keep-shadow-${proposal.proposal_id}`}
        >
          ⏳ 延长 shadow
        </button>
        <div className="flex-1" />
        <button
          className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)] hover:text-[var(--danger)] disabled:opacity-50"
          onClick={() => onDelete(proposal)}
          disabled={busy}
          title="彻底删除（仅 status=pending 可删，其他用回滚）"
          data-testid={`btn-delete-${proposal.proposal_id}`}
        >
          🗑️ 删除
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
