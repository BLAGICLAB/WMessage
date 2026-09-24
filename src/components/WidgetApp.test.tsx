import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import WidgetApp from "./WidgetApp";
import { stubScrollNoop } from "../test/scrollNoop";

// WidgetApp 面板滚动依赖 jsdom 未实现的 scrollTo/scrollIntoView（scoped noop）
stubScrollNoop();

// —— Tauri mocks ——
const mocks = vi.hoisted(() => {
  const invokeMock = vi.fn();
  // 捕获 listen 回调（按事件名归档），供用例手动 emit 事件（bot-changed 等）
  const listeners: Record<
    string,
    Array<(e: { payload: unknown }) => void>
  > = {};
  const listenMock = vi.fn(
    async (event: string, cb: (e: { payload: unknown }) => void) => {
      if (!listeners[event]) listeners[event] = [];
      listeners[event].push(cb);
      return () => {};
    }
  );
  const emitMock = vi.fn(async () => {});
  const winMock = {
    setAlwaysOnTop: vi.fn(async () => {}),
    setSize: vi.fn(async () => {}),
    setPosition: vi.fn(async () => {}),
    outerPosition: vi.fn(async () => ({ x: 100, y: 100 })),
    outerSize: vi.fn(async () => ({ width: 44, height: 220 })),
    scaleFactor: vi.fn(async () => 1),
    onMoved: vi.fn(async () => () => {}),
    onDragDropEvent: vi.fn(async () => () => {}),
    startDragging: vi.fn(async () => {}),
  };
  return { invokeMock, listenMock, emitMock, winMock, listeners };
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
  for (const k of Object.keys(mocks.listeners)) delete mocks.listeners[k];
  alertMock.mockClear();
  window.alert = alertMock;
  localStorage.clear();
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

// 5s 兜底轮询读失败不再永久静默（catch(()=>{})）——
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

// 回归：挂件折叠不得丢失机器人流式回复。
// ChatPanel 始终挂载（折叠时 display:none 不卸载），messages/busy/流式事件监听跨折叠保留。
describe("挂件折叠不丢聊天（2026-08-19 修复）", () => {
  const setupBotMocks = () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "db_load")
        return [{ id: "t1", title: "挂件任务", column: "todo", order: 0 }];
      if (cmd === "workspace_load") return [];
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_history_save") return null;
      if (cmd === "bot_chat") return { text: "机器人回复", taskRefs: [] };
      return null;
    });
  };

  /** 展开面板（含 ChatPanel 输入框的那个 .nm-sidebar-panel；另一个是触发条） */
  const panelOf = (container: HTMLElement) =>
    Array.from(container.querySelectorAll<HTMLElement>(".nm-sidebar-panel")).find(
      (el) => el.querySelector("input")
    )!;

  /** 触发条（不含输入框的 .nm-sidebar-panel） */
  const stripOf = (container: HTMLElement) =>
    Array.from(container.querySelectorAll<HTMLElement>(".nm-sidebar-panel")).find(
      (el) => !el.querySelector("input")
    )!;

  /** 展开/折叠是多段 await 的异步链，flush 两轮确保 setState 与被动 effect 都落地 */
  const flush = async () => {
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(0);
  };

  it("折叠时 ChatPanel 仍挂载：display:none 隐藏但不卸载", async () => {
    setupBotMocks();
    const { container } = render(<WidgetApp />);
    // ChatPanel 挂载：初始 expanded=false 折叠态。
    // 就用例自己的断言显式等待异步落定（bot_get_enabled → enabled=true →
    // ChatPanel 效应里 bot_sessions_load），不要靠固定次数的微任务 flush——
    // 那样会与相邻用例对 tick 数的要求互相打架（实测同一份 flush 改多改少都会挂另一个用例）。
    await vi.waitFor(() =>
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_sessions_load")
    );
    expect(screen.getByPlaceholderText(/和机器人说点什么/)).toBeInTheDocument();
    // 面板处于隐藏态（display:none），不是被卸载
    expect(panelOf(container).style.display).toBe("none");
  });

  it("折叠-展开循环中 messages 保留（ChatPanel 不卸载、不重新初始化）", async () => {
    setupBotMocks();
    const { container } = render(<WidgetApp />);
    await flush();
    // 鼠标划入触发条 → 展开（React 用 mouseover 合成 mouseenter）
    fireEvent.mouseOver(stripOf(container));
    await flush();
    const panel = panelOf(container);
    expect(panel.style.display).not.toBe("none");
    // 发一条消息并等回复落盘渲染
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    fireEvent.change(input, { target: { value: "你好" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await flush();
    expect(screen.getByText("你好")).toBeInTheDocument();
    expect(screen.getByText("机器人回复")).toBeInTheDocument();
    // 鼠标划出 → 折叠（非 busy，允许）
    fireEvent.mouseOut(panel);
    await flush();
    expect(panel.style.display).toBe("none");
    // 再次展开：消息仍在，且 ChatPanel 未重新挂载（bot_sessions_load 只调一次）
    fireEvent.mouseOver(stripOf(container));
    await flush();
    expect(panel.style.display).not.toBe("none");
    expect(screen.getByText("你好")).toBeInTheDocument();
    expect(screen.getByText("机器人回复")).toBeInTheDocument();
    expect(
      mocks.invokeMock.mock.calls.filter((c) => c[0] === "bot_sessions_load").length
    ).toBe(1);
  });
});

// 回归：bot 开关不再触发 ChatPanel 重挂（16:30 resize 冲突修法）
// ChatPanel 容器始终挂载，关闭时折叠为 0 高度；sessions/历史只加载一次
describe("bot 开关不重挂 ChatPanel（2026-09-12）", () => {
  const setupBotMocks = () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "db_load")
        return [{ id: "t1", title: "挂件任务", column: "todo", order: 0 }];
      if (cmd === "workspace_load") return [];
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_history_save") return null;
      return null;
    });
  };

  const flush = async () => {
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(0);
  };

  /** 手动 emit bot-changed 事件（设置页切换广播，WidgetApp L286 监听） */
  const emitBotChanged = (enabled: boolean) => {
    const cbs = mocks.listeners["bot-changed"];
    expect(cbs).toBeDefined();
    expect(cbs.length).toBeGreaterThan(0);
    cbs[0]({ payload: enabled });
  };

  it("bot on → off → on 切换：ChatPanel 不卸载；重新开启按 [enabled] 依赖重新加载会话", async () => {
    setupBotMocks();
    render(<WidgetApp />);
    const sessionsLoadCalls = () =>
      mocks.invokeMock.mock.calls.filter((c) => c[0] === "bot_sessions_load").length;
    // 初始：bot on。首帧加载是异步的（bot_get_enabled → enabled=true → ChatPanel 效应），
    // 用 vi.waitFor 显式等它落定，不赌固定 tick 数（同「折叠时 ChatPanel 仍挂载」）。
    await vi.waitFor(() => expect(sessionsLoadCalls()).toBe(1));
    expect(screen.getByPlaceholderText(/和机器人说点什么/)).toBeInTheDocument();

    // bot 关闭：ChatPanel 仍在 DOM（容器始终挂载，h-0 折叠），不触发加载
    emitBotChanged(false);
    await flush();
    expect(screen.getByPlaceholderText(/和机器人说点什么/)).toBeInTheDocument();
    expect(sessionsLoadCalls()).toBe(1);

    // bot 重新开启：本用例的要害是 ChatPanel **没有被卸载重挂**（容器一直在、state 保留）；
    // 但它的会话加载 effect 依赖 [enabled]，enabled 由 false→true 会重跑，
    // 因此会话列表会再拉一次——这是当前承认的有意行为，见 ChatPanel.tsx 该 effect 的注释
    // 「重新开启后 useEffect 因为 enabled 变化重跑加载」。
    emitBotChanged(true);
    await vi.waitFor(() => expect(sessionsLoadCalls()).toBe(2));
    expect(screen.getByPlaceholderText(/和机器人说点什么/)).toBeInTheDocument();
    expect(sessionsLoadCalls()).toBe(2);
  });
});
