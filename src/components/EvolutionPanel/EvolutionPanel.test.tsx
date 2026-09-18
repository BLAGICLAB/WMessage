// EvolutionPanel vitest 测试
//
// 覆盖：
// - 加载 proposals / changes（mock invoke）
// - 过滤切换
// - Promote / Reject / Keep Shadow / Rollback 按钮调用 invoke
// - HumanApproved / AutoApplied 区分（硬约束 ②）

import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import { EvolutionPanel } from "./EvolutionPanel";
import type { ChangeRecord, ProposalEntry } from "./types";

function mkProposal(
  id: string,
  overrides: Partial<ProposalEntry> = {}
): ProposalEntry {
  return {
    proposal_id: id,
    change_id: `chg-${id}`,
    layer: "policy",
    impact: "medium",
    origin: "consolidation_reflection",
    target: { kind: "memory_policy", policy: "test" },
    suggestion_text: `text-${id}`,
    mem_key: `evo:${id}`,
    related_refs: [],
    summary: `summary-${id}`,
    occurrence_count: 1,
    window_hours: 24,
    created_at_ms: 1_700_000_000_000,
    expires_at_ms: 1_700_000_000_000 + 86_400_000 * 14,
    status: "pooled",
    ...overrides,
  };
}

function mkChange(id: string, overrides: Partial<ChangeRecord> = {}): ChangeRecord {
  return {
    change_id: id,
    parent_id: null,
    schema_version: 1,
    layer: "policy",
    origin: "consolidation_reflection",
    proposal_id: id.replace(/^chg-/, ""),
    target: { kind: "memory_policy", policy: "test" },
    suggestion_text: `text-${id}`,
    mem_key: `evo:${id.replace(/^chg-/, "")}`,
    impact: "medium",
    eval_before: null,
    eval_after: null,
    status: "active",
    hard_constraint_compliance: true,
    approval_source: "auto_applied",
    human_approver: null,
    created_at_ms: 1_700_000_000_000,
    rolled_back_at: null,
    rollback_reason: null,
    ...overrides,
  };
}

beforeEach(() => {
  invokeMock.mockReset();
});

describe("EvolutionPanel", () => {
  it("renders and loads proposals + changes on mount", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "evolution_list_proposals") return [];
      if (cmd === "evolution_list_changes") return [];
      return null;
    });
    render(<EvolutionPanel />);
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "evolution_list_proposals",
        expect.objectContaining({ status: undefined })
      );
      expect(invokeMock).toHaveBeenCalledWith("evolution_list_changes");
    });
    expect(screen.getByText(/自进化决策面板/)).toBeTruthy();
    expect(screen.getByText(/候选池（0）/)).toBeTruthy();
    expect(screen.getByText(/active ChangeRecord：0/)).toBeTruthy();
  });

  it("renders proposals with evidence / impact info", async () => {
    const p1 = mkProposal("p1", {
      suggestion_text: "合并相似记忆条目",
      summary: "工具 A 失败 N 次",
      occurrence_count: 5,
      impact: "high",
    });
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "evolution_list_proposals") return [p1];
      if (cmd === "evolution_list_changes") return [];
      return null;
    });
    render(<EvolutionPanel />);
    expect(await screen.findByText("合并相似记忆条目")).toBeTruthy();
    expect(screen.getByText(/工具 A 失败 N 次/)).toBeTruthy();
    expect(screen.getByText(/出现 5 次/)).toBeTruthy();
    expect(screen.getByText(/高/)).toBeTruthy(); // impact label
  });

  it("switches status filter and refetches", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "evolution_list_proposals") return [];
      if (cmd === "evolution_list_changes") return [];
      return null;
    });
    render(<EvolutionPanel />);
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith(
        "evolution_list_proposals",
        expect.objectContaining({ status: undefined })
      )
    );
    invokeMock.mockClear();

    const user = userEvent.setup();
    const pooledBtn = screen.getAllByRole("button", { name: "候选" })[0];
    await user.click(pooledBtn);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "evolution_list_proposals",
        expect.objectContaining({ status: "pooled" })
      );
    });
  });

  it("calls evolution_promote_proposal on Promote click", async () => {
    const p = mkProposal("p-promo", { status: "pooled" });
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "evolution_list_proposals") return [p];
      if (cmd === "evolution_list_changes") return [];
      if (cmd === "evolution_promote_proposal") return {};
      return null;
    });
    render(<EvolutionPanel />);
    await waitFor(() =>
      expect(screen.queryByText(p.suggestion_text)).toBeTruthy()
    );

    const user = userEvent.setup();
    invokeMock.mockClear();
    const promoteBtn = screen.getByRole("button", { name: /Promote/ });
    await user.click(promoteBtn);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "evolution_promote_proposal",
        expect.objectContaining({
          proposalId: "p-promo",
          interactive: true,
          sessionId: null,
        })
      );
    });
  });

  it("calls evolution_reject_proposal on Reject click", async () => {
    const p = mkProposal("p-rej", { status: "pooled" });
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "evolution_list_proposals") return [p];
      if (cmd === "evolution_list_changes") return [];
      return null;
    });
    render(<EvolutionPanel />);
    await waitFor(() =>
      expect(screen.queryByText(p.suggestion_text)).toBeTruthy()
    );

    const user = userEvent.setup();
    invokeMock.mockClear();
    const rejectBtn = screen.getByRole("button", { name: /Reject/ });
    await user.click(rejectBtn);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "evolution_reject_proposal",
        expect.objectContaining({ proposalId: "p-rej" })
      );
    });
  });

  it("calls evolution_keep_shadow on Keep Shadow click", async () => {
    const p = mkProposal("p-shadow", { status: "pooled" });
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "evolution_list_proposals") return [p];
      if (cmd === "evolution_list_changes") return [];
      return null;
    });
    render(<EvolutionPanel />);
    await waitFor(() =>
      expect(screen.queryByText(p.suggestion_text)).toBeTruthy()
    );

    const user = userEvent.setup();
    invokeMock.mockClear();
    const keepBtn = screen.getByRole("button", { name: /Keep Shadow/ });
    await user.click(keepBtn);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "evolution_keep_shadow",
        expect.objectContaining({ proposalId: "p-shadow" })
      );
    });
  });

  it("calls evolution_rollback_change on Rollback click", async () => {
    const c = mkChange("chg-rb", { status: "active" });
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "evolution_list_proposals") return [];
      if (cmd === "evolution_list_changes") return [c];
      return null;
    });
    render(<EvolutionPanel />);
    await waitFor(() =>
      expect(screen.getByText(/active ChangeRecord：1/)).toBeTruthy()
    );

    const user = userEvent.setup();
    invokeMock.mockClear();
    const rbBtn = screen.getByRole("button", { name: /回滚/ });
    await user.click(rbBtn);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "evolution_rollback_change",
        expect.objectContaining({ changeId: "chg-rb" })
      );
    });
  });

  it("disables Promote button for non-pooled proposals", async () => {
    const p = mkProposal("p-done", { status: "promoted" });
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "evolution_list_proposals") return [p];
      if (cmd === "evolution_list_changes") return [];
      return null;
    });
    render(<EvolutionPanel />);
    await waitFor(() =>
      expect(screen.queryByText(p.suggestion_text)).toBeTruthy()
    );

    const promoteBtn = screen.getByRole("button", { name: /Promote/ });
    expect(promoteBtn).toBeDisabled();
  });

  it("distinguishes human_approved from auto_applied in change list", async () => {
    const c1 = mkChange("chg-a", { approval_source: "auto_applied", human_approver: null });
    const c2 = mkChange("chg-h", { approval_source: "human_approved", human_approver: "boss" });
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "evolution_list_proposals") return [];
      if (cmd === "evolution_list_changes") return [c1, c2];
      return null;
    });
    render(<EvolutionPanel />);
    await waitFor(() => expect(screen.getByText("chg-a")).toBeTruthy());
    expect(screen.getByText(/by boss/)).toBeTruthy();
  });
});