import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, act, waitFor } from "@testing-library/react";
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
        tool: "apps_publish",
        detail: "上架程序\napp_id: a1\nname: test",
        kind: "danger",
        sessionId: null,
      });
    });
    expect(screen.getByTestId("confirm-map-modal")).toBeInTheDocument();
    expect(screen.getByTestId("confirm-map-detail").textContent).toContain(
      "上架程序",
    );
    expect(screen.getByTestId("confirm-map-countdown").textContent).toBe("60s");
    expect(screen.getByText(/高危/)).toBeInTheDocument();
  });

  it("点确认 → 调 invoke('bot_confirm_response', approved=true) + modal 消失", async () => {
    render(<ConfirmMap />);
    await act(async () => {
      emit({
        id: "approve-1",
        tool: "apps_publish",
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
        tool: "apps_discard",
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
        tool: "apps_publish",
        detail: "d",
        kind: "danger",
        sessionId: null,
      });
    });
    fireEvent.click(screen.getByTestId("confirm-map-approve"));
    fireEvent.click(screen.getByTestId("confirm-map-approve"));
    await act(async () => {});
    const approveCalls = invokeMock.mock.calls.filter(
      (c) => c[0] === "bot_confirm_response" && c[1]?.approved === true,
    );
    expect(approveCalls).toHaveLength(1);
  });

  it("确认 invoke 挂起 + 倒计时冲零 → 只 invoke 一次（busyRef 挡 respond(false)）", async () => {
    vi.useFakeTimers();
    let resolveInvoke: (() => void) | undefined;
    invokeMock.mockImplementation(
      () => new Promise<void>((r) => (resolveInvoke = r)),
    );
    render(<ConfirmMap />);
    await act(async () => {
      emit({
        id: "race-1",
        tool: "apps_publish",
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
      (c) => c[0] === "bot_confirm_response",
    );
    expect(calls).toHaveLength(1);
    expect(calls[0][1]).toMatchObject({ requestId: "race-1", approved: true });
  });

  it("响应 invoke 挂起期间新请求到达 → 旧响应完成后新弹窗仍在（快照清理）", async () => {
    let resolveInvoke: (() => void) | undefined;
    invokeMock.mockImplementation(
      () => new Promise<void>((r) => (resolveInvoke = r)),
    );
    render(<ConfirmMap />);
    await act(async () => {
      emit({
        id: "old-1",
        tool: "apps_publish",
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
        tool: "apps_discard",
        detail: "new detail",
        kind: "file_access",
        sessionId: null,
      });
    });
    expect(screen.getByTestId("confirm-map-detail").textContent).toContain(
      "new detail",
    );
    // 旧 invoke 完成：不得清掉用户正在看的新请求弹窗
    await act(async () => {
      resolveInvoke?.();
    });
    expect(screen.getByTestId("confirm-map-detail").textContent).toContain(
      "new detail",
    );
    const calls = invokeMock.mock.calls.filter(
      (c) => c[0] === "bot_confirm_response",
    );
    expect(calls).toHaveLength(1);
  });

  it("新 confirm 到达 → 未响应旧 id 自动拒旧（approved=false）+ 新 confirm 上屏", async () => {
    render(<ConfirmMap />);
    await act(async () => {
      emit({
        id: "stale-1",
        tool: "apps_publish",
        detail: "old request",
        kind: "danger",
        sessionId: null,
      });
    });
    expect(screen.getByTestId("confirm-map-modal")).toBeInTheDocument();
    await act(async () => {
      emit({
        id: "fresh-2",
        tool: "apps_publish",
        detail: "new request",
        kind: "danger",
        sessionId: null,
      });
    });
    // 旧 id 立即被拒（不靠 60s 超时兜底）
    const rejectCalls = invokeMock.mock.calls.filter(
      (c) => c[0] === "bot_confirm_response",
    );
    expect(rejectCalls).toHaveLength(1);
    expect(rejectCalls[0][1]).toMatchObject({
      requestId: "stale-1",
      approved: false,
    });
    // 新 confirm 正常上屏
    expect(screen.getByTestId("confirm-map-detail").textContent).toContain(
      "new request",
    );
  });
  it("file_access 三选一：始终允许该目录 → always=true", async () => {
    render(<ConfirmMap />);
    await act(async () => {
      emit({
        id: "fa-1",
        tool: "read_text_file",
        detail: "/Users/me/outside.txt",
        kind: "file_access",
        sessionId: null,
      });
    });
    fireEvent.click(screen.getByTestId("confirm-map-always"));
    await waitFor(() => {
      const calls = invokeMock.mock.calls.filter(
        (c) => c[0] === "bot_confirm_response",
      );
      expect(calls).toHaveLength(1);
      expect(calls[0][1]).toMatchObject({
        requestId: "fa-1",
        approved: true,
        always: true,
      });
    });
  });

  it("file_access 三选一：允许一次 → always=false", async () => {
    render(<ConfirmMap />);
    await act(async () => {
      emit({
        id: "fa-2",
        tool: "read_text_file",
        detail: "/Users/me/outside.txt",
        kind: "file_access",
        sessionId: "s1",
      });
    });
    fireEvent.click(screen.getByTestId("confirm-map-approve"));
    await waitFor(() => {
      const calls = invokeMock.mock.calls.filter(
        (c) => c[0] === "bot_confirm_response",
      );
      expect(calls).toHaveLength(1);
      expect(calls[0][1]).toMatchObject({
        requestId: "fa-2",
        approved: true,
        always: null,
      });
    });
  });

  /* 挂载还原（webview 重载后按后端确认表重查）：倒计时用后端剩余秒数
   * （deadline 不随还原重置），已到期的不还原直接拒收，多条并发还原最早
   * 那条、其余拒收。 */
  it("挂载还原：带后端剩余时长的第一条上屏，过期与其余的拒收", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_pending_confirms")
        return [
          {
            id: "expired-1",
            tool: "删除任务",
            detail: "早已到期",
            kind: "danger",
            sessionId: "s1",
            remainingSecs: 0,
          },
          {
            id: "old-1",
            tool: "删除任务",
            detail: "50 秒前入表",
            kind: "danger",
            sessionId: "s1",
            remainingSecs: 9,
          },
          {
            id: "fresh-1",
            tool: "apps_publish",
            detail: "刚入表",
            kind: "danger",
            sessionId: "s1",
            remainingSecs: 59,
          },
        ];
      return undefined;
    });
    render(<ConfirmMap />);
    // 还原最早那条（old-1），倒计时是后端剩余 9s 而不是重置的 60s
    expect(await screen.findByTestId("confirm-map-modal")).toBeInTheDocument();
    expect(screen.getByTestId("confirm-map-countdown").textContent).toBe("9s");
    expect(screen.getByTestId("confirm-map-detail").textContent).toContain(
      "50 秒前入表",
    );
    // 过期的与较新的按 stale 口径拒收
    await waitFor(() => {
      const calls = invokeMock.mock.calls.filter(
        (c) => c[0] === "bot_confirm_response",
      );
      const ids = calls.map((c) => (c[1] as { requestId: string }).requestId);
      expect(ids).toContain("expired-1");
      expect(ids).toContain("fresh-1");
      expect(ids).not.toContain("old-1");
    });
  });

  it("挂载还原：全部已到期 → 不弹窗，全部拒收", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_pending_confirms")
        return [
          {
            id: "dead-1",
            tool: "删除任务",
            detail: "d",
            kind: "danger",
            sessionId: null,
            remainingSecs: 0,
          },
        ];
      return undefined;
    });
    render(<ConfirmMap />);
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10));
    });
    expect(screen.queryByTestId("confirm-map-modal")).not.toBeInTheDocument();
    await waitFor(() => {
      const calls = invokeMock.mock.calls.filter(
        (c) => c[0] === "bot_confirm_response",
      );
      expect(calls).toHaveLength(1);
      expect(calls[0][1]).toMatchObject({ requestId: "dead-1", approved: false });
    });
  });
});
