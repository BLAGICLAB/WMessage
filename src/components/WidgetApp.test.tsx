import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render } from "@testing-library/react";
import WidgetApp from "./WidgetApp";

// —— Tauri mocks ——
const mocks = vi.hoisted(() => {
  const invokeMock = vi.fn();
  const listenMock = vi.fn(async () => () => {});
  const emitMock = vi.fn(async () => {});
  const winMock = {
    setAlwaysOnTop: vi.fn(async () => {}),
    setSize: vi.fn(async () => {}),
    setPosition: vi.fn(async () => {}),
    outerPosition: vi.fn(async () => ({ x: 100, y: 100 })),
    outerSize: vi.fn(async () => ({ width: 44, height: 220 })),
    scaleFactor: vi.fn(async () => 1),
    onMoved: vi.fn(async () => () => {}),
    startDragging: vi.fn(async () => {}),
  };
  return { invokeMock, listenMock, emitMock, winMock };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invokeMock,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: mocks.listenMock,
  emit: mocks.emitMock,
}));

vi.mock("@tauri-apps/api/window", () => ({
  currentMonitor: vi.fn(async () => null),
  getCurrentWindow: () => mocks.winMock,
  LogicalPosition: class LogicalPosition {
    constructor(
      public x: number,
      public y: number
    ) {}
  },
  LogicalSize: class LogicalSize {
    constructor(
      public width: number,
      public height: number
    ) {}
  },
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn(async () => {}),
}));

const dbErr = { code: "DB_ERROR", message: "database is locked", recoverable: false };
const alertMock = vi.fn();

beforeEach(() => {
  mocks.invokeMock.mockReset();
  mocks.listenMock.mockClear();
  alertMock.mockClear();
  window.alert = alertMock;
  localStorage.clear();
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

// P2-33（2026-08-19）：5s 兜底轮询读失败不再永久静默（catch(()=>{})）——
// 轮询失败弹 alert（widget_poll）；首次加载保持安静，等 tasks-changed / 轮询重试
describe("WidgetApp 轮询报错（P2-33）", () => {
  it("首次加载失败不弹 alert；5s 轮询失败弹 alert", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "db_load") throw dbErr;
      if (cmd === "workspace_load") return [];
      if (cmd === "bot_get_enabled") return false;
      return null;
    });
    render(<WidgetApp />);
    // 首次加载（非轮询）失败：保持安静（flush 微任务让初始 load 跑完）
    await vi.advanceTimersByTimeAsync(0);
    expect(mocks.invokeMock).toHaveBeenCalledWith("db_load");
    expect(alertMock).not.toHaveBeenCalled();
    // 推进 5s 触发兜底轮询：失败必须弹 alert（不静默）
    await vi.advanceTimersByTimeAsync(5000);
    expect(alertMock).toHaveBeenCalled();
    expect(alertMock.mock.calls[0][0]).toContain("database is locked");
  });

  it("轮询连续失败：每个周期都上报（不吞）", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "db_load") throw dbErr;
      if (cmd === "workspace_load") return [];
      if (cmd === "bot_get_enabled") return false;
      return null;
    });
    render(<WidgetApp />);
    await vi.advanceTimersByTimeAsync(5000);
    await vi.advanceTimersByTimeAsync(5000);
    expect(alertMock.mock.calls.length).toBeGreaterThanOrEqual(2);
  });

  it("轮询成功读数不弹 alert", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "db_load")
        return [{ id: "t1", title: "挂件任务", column: "todo", order: 0 }];
      if (cmd === "workspace_load") return [];
      if (cmd === "bot_get_enabled") return false;
      return null;
    });
    render(<WidgetApp />);
    await vi.advanceTimersByTimeAsync(10000);
    expect(alertMock).not.toHaveBeenCalled();
  });
});
