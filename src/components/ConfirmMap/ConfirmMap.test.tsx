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

  it("确认 invoke 挂起 + 倒计时冲零 → 只 invoke 一次（busyRef 挡 respond(false)）", async () => {
    vi.useFakeTimers();
    let resolveInvoke: (() => void) | undefined;
    invokeMock.mockImplementation(
      () => new Promise<void>((r) => (resolveInvoke = r))
    );
    render(<ConfirmMap />);
    await act(async () => {
      emit({
        id: "race-1",
        tool: "evolution_promote",
        detail: "d",
        kind: "danger",
        sessionId: null,
      });
    });
    // 点击确认（invoke 挂起不返回），倒计时冲到 0 → effect 触发 respond(false)
    fireEvent.click(screen.getByTestId("confirm-map-approve"));
    await act(async () => {
      vi.advanceTimersByTime(61000);
    });
    await act(async () => {
      resolveInvoke?.();
    });
    const calls = invokeMock.mock.calls.filter(
      (c) => c[0] === "bot_confirm_response"
    );
    expect(calls).toHaveLength(1);
    expect(calls[0][1]).toMatchObject({ requestId: "race-1", approved: true });
  });

  it("响应 invoke 挂起期间新请求到达 → 旧响应完成后新弹窗仍在（快照清理）", async () => {
    let resolveInvoke: (() => void) | undefined;
    invokeMock.mockImplementation(
      () => new Promise<void>((r) => (resolveInvoke = r))
    );
    render(<ConfirmMap />);
    await act(async () => {
      emit({
        id: "old-1",
        tool: "evolution_promote",
        detail: "old",
        kind: "danger",
        sessionId: null,
      });
    });
    fireEvent.click(screen.getByTestId("confirm-map-approve"));
    // invoke 挂起期间新请求覆盖 pending
    await act(async () => {
      emit({
        id: "new-1",
        tool: "evolution_reject",
        detail: "new detail",
        kind: "file_access",
        sessionId: null,
      });
    });
    expect(screen.getByTestId("confirm-map-detail").textContent).toContain(
      "new detail"
    );
    // 旧 invoke 完成：不得清掉用户正在看的新请求弹窗
    await act(async () => {
      resolveInvoke?.();
    });
    expect(screen.getByTestId("confirm-map-detail").textContent).toContain(
      "new detail"
    );
    const calls = invokeMock.mock.calls.filter(
      (c) => c[0] === "bot_confirm_response"
    );
    expect(calls).toHaveLength(1);
  });
});
