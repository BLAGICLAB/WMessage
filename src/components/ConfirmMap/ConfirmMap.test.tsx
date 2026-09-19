import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";

// Mock @tauri-apps/api/event + core BEFORE import
const listenMock = vi.fn();
const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/event", () => ({
  listen: (...args: unknown[]) => listenMock(...args),
}));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import ConfirmMap from "./ConfirmMap";

type Payload = {
  id: string;
  tool: string;
  detail: string;
  kind: string;
  sessionId: string | null;
};

type Listener = (e: { payload: Payload }) => void;
let listeners: Listener[] = [];

describe("ConfirmMap", () => {
  beforeEach(() => {
    listeners = [];
    listenMock.mockReset();
    listenMock.mockImplementation(async (_event: string, cb: Listener) => {
      listeners.push(cb);
      return () => {
        listeners = listeners.filter((l) => l !== cb);
      };
    });
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(undefined);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  function emit(payload: Payload) {
    for (const cb of listeners) cb({ payload });
  }

  it("默认不渲染（无 pending）", () => {
    render(<ConfirmMap />);
    expect(screen.queryByTestId("confirm-map-modal")).not.toBeInTheDocument();
  });

  it("收到 bot-confirm 事件 → 弹 modal + 倒计时 60s + 详情", async () => {
    render(<ConfirmMap />);
    await act(async () => {
      emit({
        id: "test-id-1",
        tool: "evolution_promote",
        detail: "晋升提案\nproposal_id: p1\nsummary: test",
        kind: "danger",
        sessionId: null,
      });
    });
    expect(screen.getByTestId("confirm-map-modal")).toBeInTheDocument();
    expect(screen.getByTestId("confirm-map-detail").textContent).toContain(
      "晋升提案"
    );
    expect(screen.getByTestId("confirm-map-countdown").textContent).toBe("60s");
    expect(screen.getByText(/高危/)).toBeInTheDocument();
  });

  it("点确认 → 调 invoke('bot_confirm_response', approved=true) + modal 消失", async () => {
    render(<ConfirmMap />);
    await act(async () => {
      emit({
        id: "approve-1",
        tool: "evolution_promote",
        detail: "d",
        kind: "danger",
        sessionId: null,
      });
    });
    fireEvent.click(screen.getByTestId("confirm-map-approve"));
    await act(async () => {});
    expect(invokeMock).toHaveBeenCalledWith("bot_confirm_response", {
      requestId: "approve-1",
      approved: true,
      always: null,
    });
    expect(screen.queryByTestId("confirm-map-modal")).not.toBeInTheDocument();
  });

  it("点拒绝 → approved=false", async () => {
    render(<ConfirmMap />);
    await act(async () => {
      emit({
        id: "deny-1",
        tool: "evolution_reject",
        detail: "d",
        kind: "danger",
        sessionId: null,
      });
    });
    fireEvent.click(screen.getByTestId("confirm-map-deny"));
    await act(async () => {});
    expect(invokeMock).toHaveBeenCalledWith("bot_confirm_response", {
      requestId: "deny-1",
      approved: false,
      always: null,
    });
  });

  it("重复点击确认按钮 → 只 invoke 一次（busy 锁）", async () => {
    render(<ConfirmMap />);
    await act(async () => {
      emit({
        id: "double-1",
        tool: "evolution_promote",
        detail: "d",
        kind: "danger",
        sessionId: null,
      });
    });
    fireEvent.click(screen.getByTestId("confirm-map-approve"));
    fireEvent.click(screen.getByTestId("confirm-map-approve"));
    await act(async () => {});
    const approveCalls = invokeMock.mock.calls.filter(
      (c) => c[0] === "bot_confirm_response" && c[1]?.approved === true
    );
    expect(approveCalls).toHaveLength(1);
  });
});
