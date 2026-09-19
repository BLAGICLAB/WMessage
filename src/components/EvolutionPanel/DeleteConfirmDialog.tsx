//! DeleteConfirmDialog — 删除提案确认弹窗（老板 16:35 拍板）
//!
//! 选项：
//! 1. 默认：只删 proposals.jsonl 行 + 级联删 pending ChangeRecord
//! 2. 勾选「连同源记忆一起删」：额外删 mem_items（永久阻断 24h 后重生）
//!
//! 设计权衡（详见老板 16:35 拍板）：
//! - 默认关（保护）：mem_items 可能参与其他 proposal，连带删破坏更广
//! - 勾选后：permanent block，但不可逆

import { useEffect, useState } from "react";
import type { ProposalEntry } from "./types";

type Props = {
  proposal: ProposalEntry;
  busy: boolean;
  onConfirm: (cascadeSource: boolean) => void;
  onCancel: () => void;
};

export function DeleteConfirmDialog({
  proposal,
  busy,
  onConfirm,
  onCancel,
}: Props) {
  const [cascadeSource, setCascadeSource] = useState(false);
  const refsCount = proposal.related_refs.length;

  // Esc 关闭
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, onCancel]);

  return (
    <div
      className="fixed inset-0 z-[90] flex items-center justify-center bg-black/40 p-6"
      data-testid="delete-confirm-modal"
      role="dialog"
      aria-modal="true"
      aria-labelledby="delete-confirm-title"
    >
      <div className="nm-card w-full max-w-md p-5 space-y-4">
        <h3
          id="delete-confirm-title"
          className="text-base font-semibold text-[var(--t1)]"
        >
          🗑️ 彻底删除提案
        </h3>

        <div className="text-xs space-y-1">
          <div className="text-[var(--t3)] font-mono">
            {proposal.proposal_id}
          </div>
          <div className="text-[var(--t2)]">{proposal.suggestion_text}</div>
          {refsCount > 0 && (
            <div className="text-[var(--t4)] mt-1">
              源记忆：{refsCount} 条 mem_items
            </div>
          )}
        </div>

        {refsCount > 0 && (
          <label
            className="flex items-start gap-2 cursor-pointer p-3 rounded hover:bg-[var(--inset)] border border-[var(--border)]"
            data-testid="cascade-source-checkbox"
          >
            <input
              type="checkbox"
              checked={cascadeSource}
              onChange={(e) => setCascadeSource(e.target.checked)}
              disabled={busy}
              className="mt-0.5"
            />
            <div className="text-xs">
              <div className="text-[var(--t2)] font-medium">
                连同源记忆一起删（{refsCount} 条 mem_items）
              </div>
              <div className="text-[var(--t4)] mt-1 leading-relaxed">
                ⚠️ 勾选后永久删除派生源记忆，LLM 无法重新派生。
                <br />
                不勾选：24h dedup 期内不会重生，过期后可能被重新发现。
              </div>
            </div>
          </label>
        )}

        <div className="flex gap-2 justify-end">
          <button
            className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)]"
            onClick={onCancel}
            disabled={busy}
            data-testid="delete-cancel"
          >
            取消
          </button>
          <button
            className="nm-btn px-3 py-1.5 text-xs text-white bg-[var(--danger)] disabled:opacity-50"
            onClick={() => onConfirm(cascadeSource)}
            disabled={busy}
            data-testid="delete-confirm"
          >
            🗑️ 确定删除
          </button>
        </div>
      </div>
    </div>
  );
}
