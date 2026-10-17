// EvolutionPanel vitest 测试
//
// 覆盖：
// - 加载 proposals / changes（mock invoke）
// - 过滤切换
// - Promote / Reject / Rollback 按钮调用 invoke
// - HumanApproved / AutoApplied 区分（硬约束 ②）

import { describe, it, expect, beforeEach, vi } from "vitest";
import {
  render,
  screen,
  waitFor,
  act,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invokeMock = vi.fn();
const askMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  ask: (...args: unknown[]) => askMock(...args),
}));

import { EvolutionPanel } from "./EvolutionPanel";
import type { ChangeRecord, ProposalEntry } from "./types";

function mkProposal(
  id: string,
  overrides: Partial<ProposalEntry> = {},
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

function mkChange(
  id: string,
  overrides: Partial<ChangeRecord> = {},
): ChangeRecord {
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
  askMock.mockReset();
  askMock.mockResolvedValue(true);
});

// ── ：有回滚历史的提案再点 ON 需二次确认 ──

function renderWithRollbackHistory() {
  const p = mkProposal("p-rb", { status: "promoted" });
  const rolled = mkChange("chg-p-rb", {
    proposal_id: "p-rb",
    status: "rolled_back",
  });
  invokeMock.mockImplementation((cmd: string) => {
    if (cmd === "evolution_list_proposals") return [p];
    if (cmd === "evolution_list_changes") return [rolled];
    if (cmd === "evolution_toggle_proposal") return null;
    return null;
  });
  return p;
}

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
        expect.objectContaining({ status: undefined }),
      );
      expect(invokeMock).toHaveBeenCalledWith("evolution_list_changes");
    });
    expect(screen.getByText(/自进化决策面板/)).toBeTruthy();
    expect(screen.getByText(/自进化提案（0）/)).toBeTruthy();
    // Rollback 区条件显示：0 active 时不渲染（）
    expect(screen.queryByText(/active ChangeRecord：/)).toBeNull();
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
        expect.objectContaining({ status: undefined }),
      ),
    );
    invokeMock.mockClear();

    const user = userEvent.setup();
    const pooledBtn = screen.getAllByRole("button", { name: "候选" })[0];
    await user.click(pooledBtn);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "evolution_list_proposals",
        expect.objectContaining({ status: "pooled" }),
      );
    });
  });

  it("换筛选后慢的旧筛选响应不覆盖新列表（乱序丢弃）", async () => {
    const pAll = mkProposal("p-all", { suggestion_text: "全量提案" });
    const pPooled = mkProposal("p-pool", { suggestion_text: "候选提案" });
    // 按 status 参数挂起响应：模拟旧筛选响应晚于新筛选响应到达
    const gates = new Map<string, (v: ProposalEntry[]) => void>();
    invokeMock.mockImplementation(
      (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "evolution_list_proposals") {
          const key = String(args?.status ?? "all");
          return new Promise<ProposalEntry[]>((resolve) =>
            gates.set(key, resolve),
          );
        }
        if (cmd === "evolution_list_changes") return [];
        return null;
      },
    );
    render(<EvolutionPanel />);
    await waitFor(() => expect(gates.has("all")).toBe(true));
    const user = userEvent.setup();
    await user.click(screen.getAllByRole("button", { name: "候选" })[0]);
    await waitFor(() => expect(gates.has("pooled")).toBe(true));
    // 新筛选的响应先到并渲染
    await act(async () => {
      gates.get("pooled")!([pPooled]);
    });
    expect(await screen.findByText("候选提案")).toBeInTheDocument();
    // 旧筛选（全量）的慢响应后到：不得覆盖新筛选列表
    await act(async () => {
      gates.get("all")!([pAll]);
    });
    expect(screen.getByText("候选提案")).toBeInTheDocument();
    expect(screen.queryByText("全量提案")).toBeNull();
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
      expect(screen.queryByText(p.suggestion_text)).toBeTruthy(),
    );

    const user = userEvent.setup();
    invokeMock.mockClear();
    const promoteBtn = screen.getByRole("button", { name: /启用/ });
    await user.click(promoteBtn);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "evolution_promote_proposal",
        expect.objectContaining({
          proposalId: "p-promo",
          interactive: true,
          sessionId: null,
        }),
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
      expect(screen.queryByText(p.suggestion_text)).toBeTruthy(),
    );

    const user = userEvent.setup();
    invokeMock.mockClear();
    const rejectBtn = screen.getByRole("button", { name: /停用/ });
    await user.click(rejectBtn);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "evolution_reject_proposal",
        expect.objectContaining({ proposalId: "p-rej" }),
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
      expect(screen.getByText(/active ChangeRecord：1/)).toBeTruthy(),
    );

    const user = userEvent.setup();
    invokeMock.mockClear();
    const rbBtn = screen.getByRole("button", { name: /回滚/ });
    await user.click(rbBtn);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "evolution_rollback_change",
        expect.objectContaining({ changeId: "chg-rb" }),
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
      expect(screen.queryByText(p.suggestion_text)).toBeTruthy(),
    );

    const promoteBtn = screen.getByRole("button", { name: /启用/ });
    expect(promoteBtn).toBeDisabled();
  });

  it("distinguishes human_approved from auto_applied in change list", async () => {
    const c1 = mkChange("chg-a", {
      approval_source: "auto_applied",
      human_approver: null,
    });
    const c2 = mkChange("chg-h", {
      approval_source: "human_approved",
      human_approver: "boss",
    });
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "evolution_list_proposals") return [];
      if (cmd === "evolution_list_changes") return [c1, c2];
      return null;
    });
    render(<EvolutionPanel />);
    await waitFor(() => expect(screen.getByText("chg-a")).toBeTruthy());
    expect(screen.getByText(/by boss/)).toBeTruthy();
  });

  it("rollback-confirm: 有回滚历史时启用前 confirm", async () => {
    renderWithRollbackHistory();
    render(<EvolutionPanel />);
    const switchBtn = await screen.findByRole("switch", { name: "切换 p-rb" });
    askMock.mockResolvedValueOnce(true);
    const user = userEvent.setup();
    await user.click(switchBtn);
    expect(askMock).toHaveBeenCalledWith(
      expect.stringContaining("上次已回滚"),
      expect.anything(),
    );
  });

  it("rollback-confirm: 接受时调 evolution_toggle_proposal", async () => {
    renderWithRollbackHistory();
    render(<EvolutionPanel />);
    const switchBtn = await screen.findByRole("switch", { name: "切换 p-rb" });
    askMock.mockResolvedValueOnce(true);
    const user = userEvent.setup();
    await user.click(switchBtn);
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "evolution_toggle_proposal",
        expect.objectContaining({ proposalId: "p-rb", enabled: true }),
      );
    });
  });

  it("rollback-confirm: 取消时不调 toggle", async () => {
    renderWithRollbackHistory();
    render(<EvolutionPanel />);
    const switchBtn = await screen.findByRole("switch", { name: "切换 p-rb" });
    askMock.mockResolvedValueOnce(false);
    const user = userEvent.setup();
    await user.click(switchBtn);
    expect(askMock).toHaveBeenCalled();
    expect(
      invokeMock.mock.calls.some(([c]) => c === "evolution_toggle_proposal"),
    ).toBe(false);
  });

  it("rollback-confirm: 无回滚历史时跳过 confirm", async () => {
    const p = mkProposal("p-clean", { status: "pooled" });
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "evolution_list_proposals") return [p];
      if (cmd === "evolution_list_changes") return [];
      if (cmd === "evolution_toggle_proposal") return null;
      return null;
    });
    render(<EvolutionPanel />);
    const switchBtn = await screen.findByRole("switch", {
      name: "切换 p-clean",
    });
    const user = userEvent.setup();
    await user.click(switchBtn);
    expect(askMock).not.toHaveBeenCalled();
    expect(invokeMock).toHaveBeenCalledWith(
      "evolution_toggle_proposal",
      expect.objectContaining({ proposalId: "p-clean", enabled: true }),
    );
  });
});

// ── U20-EVOGOV：应用策略二档 / 决策徽标 / 立即反思 / 四指标条 ──

describe("EvolutionPanel U20 governance", () => {
  it("W2: apply policy radiogroup defaults to auto and persists on click", async () => {
    const p = mkProposal("p-gov");
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "evolution_list_proposals") return [p];
      if (cmd === "evolution_list_changes") return [];
      if (cmd === "evolution_get_apply_policy") return "auto";
      if (cmd === "evolution_set_apply_policy") return null;
      return null;
    });
    render(<EvolutionPanel />);
    const group = await screen.findByRole("radiogroup", {
      name: "自进化应用策略",
    });
    expect(group).toBeTruthy();
    expect(
      screen.getByRole("radio", { name: "应用策略：自动生效" }),
    ).toHaveAttribute("aria-checked", "true");
    expect(
      screen.getByRole("radio", { name: "应用策略：需我确认" }),
    ).toHaveAttribute("aria-checked", "false");

    const user = userEvent.setup();
    invokeMock.mockClear();
    await user.click(screen.getByRole("radio", { name: "应用策略：需我确认" }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("evolution_set_apply_policy", {
        policy: "confirm",
      });
    });
    expect(
      screen.getByRole("radio", { name: "应用策略：需我确认" }),
    ).toHaveAttribute("aria-checked", "true");
  });

  it("W2: reading confirm from backend selects the confirm tier", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "evolution_list_proposals") return [];
      if (cmd === "evolution_list_changes") return [];
      if (cmd === "evolution_get_apply_policy") return "confirm";
      return null;
    });
    render(<EvolutionPanel />);
    await waitFor(() => {
      expect(
        screen.getByRole("radio", { name: "应用策略：需我确认" }),
      ).toHaveAttribute("aria-checked", "true");
    });
  });

  it("W3: shows decision badges 已自动生效 / 你已启用 / 待你决策", async () => {
    const pAuto = mkProposal("p-auto", { status: "promoted" });
    const pHuman = mkProposal("p-human", { status: "promoted" });
    const pPooled = mkProposal("p-pooled", { status: "pooled" });
    const crAuto = mkChange("chg-p-auto", {
      proposal_id: "p-auto",
      status: "active",
      approval_source: "auto_applied",
    });
    const crHuman = mkChange("chg-p-human", {
      proposal_id: "p-human",
      status: "active",
      approval_source: "human_approved",
      human_approver: "boss",
    });
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "evolution_list_proposals") return [pAuto, pHuman, pPooled];
      if (cmd === "evolution_list_changes") return [crAuto, crHuman];
      return null;
    });
    render(<EvolutionPanel />);
    expect(
      await screen.findByTestId("decision-badge-p-auto"),
    ).toHaveTextContent("已自动生效");
    expect(screen.getByTestId("decision-badge-p-human")).toHaveTextContent(
      "你已启用",
    );
    expect(screen.getByTestId("decision-badge-p-pooled")).toHaveTextContent(
      "待你决策",
    );
  });

  it("W3: empty state shows 立即反思 and reports consolidate result", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "evolution_list_proposals") return [];
      if (cmd === "evolution_list_changes") return [];
      if (cmd === "memory_consolidate_now")
        return { merged: 2, distilled: 1, contradictions: 3 };
      return null;
    });
    render(<EvolutionPanel />);
    const btn = await screen.findByTestId("btn-reflect-now");
    const user = userEvent.setup();
    invokeMock.mockClear();
    await user.click(btn);
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("memory_consolidate_now");
    });
    await waitFor(() => {
      expect(screen.getByTestId("evolution-info")).toHaveTextContent(
        "反思完成：合并 2 · 提炼 1 · 裁决 3",
      );
    });
  });

  it("evidence: 影子判定行 + 冲突徽标按后端数据渲染", async () => {
    const p = mkProposal("p-ev");
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "evolution_list_proposals") return [p];
      if (cmd === "evolution_list_changes") return [];
      if (cmd === "evolution_proposal_evidence")
        return [
          {
            proposalId: "p-ev",
            shadow: { verdict: "pass", note: "would_enter_top3" },
            conflicts: [
              { with: "pool:p-other", kind: "pooled" },
              { with: "active:chg-1", kind: "active" },
            ],
          },
        ];
      return null;
    });
    render(<EvolutionPanel />);
    await screen.findByText("text-p-ev");
    expect(screen.getByTestId("shadow-judgment-p-ev")).toHaveTextContent(
      /采纳后会进 lesson 槽位 top-3/,
    );
    const badge = screen.getByTestId("conflict-badge-p-ev");
    expect(badge).toHaveTextContent(/同目标冲突 ×2/);
    expect(badge).toHaveAttribute(
      "title",
      expect.stringContaining("pool:p-other"),
    );
  });

  it("evidence: 证据缺失时不渲染判定行（后端不可用降级）", async () => {
    const p = mkProposal("p-noev");
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "evolution_list_proposals") return [p];
      if (cmd === "evolution_list_changes") return [];
      if (cmd === "evolution_proposal_evidence") return [];
      return null;
    });
    render(<EvolutionPanel />);
    await screen.findByText("text-p-noev");
    expect(screen.queryByTestId("shadow-judgment-p-noev")).toBeNull();
  });
});
