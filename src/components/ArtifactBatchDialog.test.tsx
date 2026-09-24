import { describe, expect, it, vi, beforeEach } from "vitest";
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

import { ArtifactBatchDialog } from "./ArtifactBatchDialog";

type Payload = {
  taskId: string;
  taskTitle: string;
  sessionId: string;
  origin: "manual" | "scheduled" | "batch";
  paths: string[];
};

type Listener = (e: { payload: Payload }) => void;
let listeners: Listener[] = [];

const mkBatch = (taskId: string, title: string): Payload => ({
  taskId,
  taskTitle: title,
  sessionId: `s-${taskId}`,
  origin: "manual",
  paths: [`/tmp/${taskId}-a.txt`, `/tmp/${taskId}-b.txt`],
});

describe("ArtifactBatchDialog", () => {
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
    invokeMock.mockResolvedValue(2);
  });

  function emit(payload: Payload) {
    for (const cb of listeners) cb({ payload });
  }

  it("收到 artifact-batch-ready → 弹窗显示批次标题与路径", async () => {
    render(<ArtifactBatchDialog />);
    await act(async () => {
      emit(mkBatch("t1", "甲批次任务"));
    });
    expect(screen.getByText(/甲批次任务/)).toBeInTheDocument();
    expect(screen.getByText("/tmp/t1-a.txt")).toBeInTheDocument();
  });

  it("confirm 挂起期间新批次到达 → 旧 confirm 完成后新批次弹窗仍在（快照守卫）", async () => {
    let resolveInvoke: ((v: number) => void) | undefined;
    invokeMock.mockImplementation(
      () => new Promise<number>((r) => (resolveInvoke = r))
    );
    render(<ArtifactBatchDialog />);
    await act(async () => {
      emit(mkBatch("t1", "甲批次任务"));
    });
    fireEvent.click(screen.getByRole("button", { name: /绑定选中/ }));
    // confirm A 在飞 → 新批次 B 事件覆盖弹窗
    await act(async () => {
      emit(mkBatch("t2", "乙批次任务"));
    });
    expect(screen.getByText(/乙批次任务/)).toBeInTheDocument();
    // A 的 invoke 完成：不得把 B 的弹窗杀掉
    await act(async () => {
      resolveInvoke?.(2);
    });
    expect(screen.getByText(/乙批次任务/)).toBeInTheDocument();
    // 且只对 A 发了一次 confirm
    const calls = invokeMock.mock.calls.filter(
      (c) => c[0] === "confirm_artifact_batch"
    );
    expect(calls).toHaveLength(1);
    expect(calls[0][1]).toMatchObject({ taskId: "t1" });
  });

  it("confirm 挂起期间同 tick 双击 → invoke 仅一次（busyRef 同步守卫）", async () => {
    let resolveInvoke: ((v: number) => void) | undefined;
    invokeMock.mockImplementation(
      () => new Promise<number>((r) => (resolveInvoke = r))
    );
    render(<ArtifactBatchDialog />);
    await act(async () => {
      emit(mkBatch("t1", "甲批次任务"));
    });
    const btn = screen.getByRole("button", { name: /绑定选中/ });
    fireEvent.click(btn);
    fireEvent.click(btn);
    await act(async () => {
      resolveInvoke?.(2);
    });
    const calls = invokeMock.mock.calls.filter(
      (c) => c[0] === "confirm_artifact_batch"
    );
    expect(calls).toHaveLength(1);
  });

  it("unmount 早于 listen resolve → resolve 后立即自注销（不泄漏监听器）", async () => {
    let resolveListen: ((u: () => void) => void) | undefined;
    const unlistenSpy = vi.fn();
    listenMock.mockImplementation(
      () => new Promise<() => void>((r) => (resolveListen = r))
    );
    const { unmount } = render(<ArtifactBatchDialog />);
    unmount();
    // cleanup 先跑完，listen 才 resolve：cancelled 后注册必须立即自注销
    await act(async () => {
      resolveListen?.(unlistenSpy);
    });
    expect(unlistenSpy).toHaveBeenCalledTimes(1);
  });
});
