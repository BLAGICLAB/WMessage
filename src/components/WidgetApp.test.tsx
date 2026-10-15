import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
import WidgetApp from "./WidgetApp";
import { stubScrollNoop } from "../test/scrollNoop";
import { ERROR_DIALOG_EVENT } from "../lib/errorHandler";

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
    },
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
      public y: number,
    ) {}
  },
  LogicalSize: class LogicalSize {
    constructor(
      public width: number,
      public height: number,
    ) {}
  },
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn(async () => {}),
}));

const dbErr = {
  code: "DB_ERROR",
  message: "database is locked",
  recoverable: false,
};
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
describe("WidgetApp 轮询报错", () => {
  it("首次加载失败不弹窗；5s 轮询失败弹错误弹窗（批 4 起为应用内 dialog）", async () => {
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
    expect(screen.queryByRole("alertdialog")).toBeNull();
    // 推进 5s 触发兜底轮询：失败必须弹窗（不静默）。
    // 本套件是 fake timers：act 包裹推进让 React 状态刷新，断言用同步查询
    //（findByRole 的 waitFor 依赖真实计时器，fake 下会死锁）
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5000);
    });
    const dialog = screen.getByRole("alertdialog");
    expect(dialog.textContent).toContain("database is locked");
  });

  it("轮询连续失败：每个周期都上报（不吞，按弹窗请求数计）", async () => {
    let dialogRequests = 0;
    const countHandler = () => {
      dialogRequests++;
    };
    window.addEventListener(ERROR_DIALOG_EVENT, countHandler);
    try {
      mocks.invokeMock.mockImplementation(async (cmd: string) => {
        if (cmd === "db_load") throw dbErr;
        if (cmd === "workspace_load") return [];
        if (cmd === "bot_get_enabled") return false;
        return null;
      });
      render(<WidgetApp />);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(5000);
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(5000);
      });
      expect(dialogRequests).toBeGreaterThanOrEqual(2);
      expect(screen.getByRole("alertdialog")).toBeTruthy();
    } finally {
      window.removeEventListener(ERROR_DIALOG_EVENT, countHandler);
    }
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

  /** 展开面板（含 ChatPanel 输入框的那个 .nm-sidebar-panel；另一个是触发条）
   *  （ChatPanel 输入框是 textarea，不再有 input） */
  const panelOf = (container: HTMLElement) =>
    Array.from(
      container.querySelectorAll<HTMLElement>(".nm-sidebar-panel"),
    ).find((el) => el.querySelector("textarea"))!;

  /** 触发条（不含输入框的 .nm-sidebar-panel） */
  const stripOf = (container: HTMLElement) =>
    Array.from(
      container.querySelectorAll<HTMLElement>(".nm-sidebar-panel"),
    ).find((el) => !el.querySelector("textarea"))!;

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
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_sessions_load"),
    );
    expect(screen.getByPlaceholderText(/和机器人说点什么/)).toBeInTheDocument();
    // 面板处于隐藏态（display:none），不是被卸载
    expect(panelOf(container).style.display).toBe("none");
  });

  it("折叠-展开循环中 messages 保留（ChatPanel 不卸载、不重新初始化）", async () => {
    setupBotMocks();
    const { container } = render(<WidgetApp />);
    await flush();
    // 点击触发条 → 展开（2026-10-15 起改点击展开 + 持久化，鼠标划入不再触发）
    fireEvent.click(stripOf(container));
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
    // 点「收起挂件」→ 折叠（鼠标划出不再自动折叠）
    fireEvent.click(screen.getByTitle("收起挂件"));
    await flush();
    expect(panel.style.display).toBe("none");
    // 再次点击触发条展开：消息仍在，且 ChatPanel 未重新挂载（bot_sessions_load 只调一次）
    fireEvent.click(stripOf(container));
    await flush();
    expect(panel.style.display).not.toBe("none");
    expect(screen.getByText("你好")).toBeInTheDocument();
    expect(screen.getByText("机器人回复")).toBeInTheDocument();
    expect(
      mocks.invokeMock.mock.calls.filter((c) => c[0] === "bot_sessions_load")
        .length,
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
      mocks.invokeMock.mock.calls.filter((c) => c[0] === "bot_sessions_load")
        .length;
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

// —— 任务卡口径对齐主窗口（2026-10-12 验收）：今日=今天排期、池口径排序、
// 完成恢复恒回待办、新建恒进待办。do/undo 循环里这些行为曾经各说各话。 ——
describe("WidgetApp 任务卡口径对齐主窗口", () => {
  const pad = (n: number) => String(n).padStart(2, "0");
  /** 相对今天偏移 offset 天的本地日期键 YYYY-MM-DD（与组件内 dayKey 同口径） */
  const dayKeyOf = (offset: number) => {
    const d = new Date();
    d.setDate(d.getDate() + offset);
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
  };

  const setupTaskMocks = (rows: unknown[]) => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "db_load") return rows;
      if (cmd === "workspace_load") return [];
      if (cmd === "bot_get_enabled") return false;
      return null;
    });
  };

  const titles = (container: HTMLElement) =>
    Array.from(container.querySelectorAll("h3")).map((h) => h.textContent);

  /** 初始加载/交互后的微任务 flush：act 包裹让 React 状态刷新落 DOM（套件既有模式） */
  const flush = async () => {
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
  };

  /** 展开面板（点击触发条 → expand）：getByRole 只看可见元素，
   *  折叠态面板 display:none 里的按钮对角色查询不可见 */
  const expandPanel = async (container: HTMLElement) => {
    const strip = Array.from(
      container.querySelectorAll<HTMLElement>(".nm-sidebar-panel"),
    ).find((el) => !el.querySelector("textarea"))!;
    fireEvent.click(strip);
    await flush();
    await flush();
  };

  it("今日视图 = 今天排期（planStart），不再是 doing 列", async () => {
    setupTaskMocks([
      { id: "a", title: "doing列未排期", column: "doing", order: 0 },
      {
        id: "b",
        title: "今天排期",
        column: "todo",
        order: 1,
        planStart: `${dayKeyOf(0)}T09:00`,
        planEnd: `${dayKeyOf(0)}T10:00`,
      },
      {
        id: "c",
        title: "明天排期",
        column: "todo",
        order: 2,
        planStart: `${dayKeyOf(1)}T09:00`,
      },
    ]);
    const { container } = render(<WidgetApp />);
    await flush();
    await expandPanel(container);
    fireEvent.click(screen.getByTitle("今日"));
    await flush();
    expect(titles(container)).toEqual(["今天排期"]);
  });

  it("待办视图排序同任务池：已排期 planStart 升序在前，未排期 updatedAt 降序在后", async () => {
    setupTaskMocks([
      { id: "u1", title: "未排期旧", column: "todo", order: 0, updatedAt: 1000 },
      {
        id: "s2",
        title: "已排期下午",
        column: "todo",
        order: 1,
        planStart: `${dayKeyOf(0)}T15:00`,
      },
      { id: "u2", title: "未排期新", column: "todo", order: 2, updatedAt: 2000 },
      {
        id: "s1",
        title: "已排期上午",
        column: "doing",
        order: 3,
        planStart: `${dayKeyOf(0)}T09:00`,
      },
    ]);
    const { container } = render(<WidgetApp />);
    await flush();
    expect(titles(container)).toEqual([
      "已排期上午",
      "已排期下午",
      "未排期新",
      "未排期旧",
    ]);
  });

  it("完成卡恢复恒回待办（不看截止是否今天），走 task_set_column", async () => {
    setupTaskMocks([
      {
        id: "d1",
        title: "今天截止的完成卡",
        column: "done",
        order: 0,
        due: dayKeyOf(0),
        completedAt: 123,
      },
    ]);
    const { container } = render(<WidgetApp />);
    await flush();
    await expandPanel(container);
    fireEvent.click(screen.getByTitle("完成"));
    await flush();
    fireEvent.click(screen.getByRole("button", { name: "取消完成" }));
    await flush();
    expect(mocks.invokeMock).toHaveBeenCalledWith("task_set_column", {
      id: "d1",
      col: "todo",
    });
  });

  it("今日视图新建任务：恒进待办列并切回待办视图让新卡可见", async () => {
    setupTaskMocks([
      {
        id: "b",
        title: "今天排期",
        column: "todo",
        order: 0,
        planStart: `${dayKeyOf(0)}T09:00`,
      },
    ]);
    render(<WidgetApp />);
    await flush();
    fireEvent.click(screen.getByTitle("今日"));
    await flush();
    fireEvent.click(screen.getByText("+ 新建任务"));
    await flush();
    // 上报主窗口的新任务恒为 todo 列（不再因今日视图进 doing）
    const emitCalls = mocks.emitMock.mock.calls as unknown as Array<
      [string, { upserts: Array<{ column: string }> }]
    >;
    const upsertCall = emitCalls.find((c) => c[0] === "tasks-updated");
    expect(upsertCall).toBeDefined();
    expect(upsertCall![1].upserts[0].column).toBe("todo");
    // 视图已切回待办：新任务卡进入编辑态可见（今日视图里未排期卡本不显示）
    expect(screen.getByDisplayValue("新任务")).toBeInTheDocument();
  });
});

// —— settle 位置钳制（拖动停止 500ms 后吸附）：拖出屏底的 y 必须钳回屏内，
// 与挂载恢复同口径（按窗口自身高度收口）——否则悬浮/侧贴边吸附只修正 x，
// 整窗留在屏外永远够不着。screenSize 走 currentMonitor=null 的回退屏 1920×1080。 ——
describe("WidgetApp settle 位置钳制", () => {
  it("拖出屏底（y=5000）后 settle 落点钳回屏底内：y = 屏高 1080 - 窗高 220", async () => {
    const prevPos = mocks.winMock.outerPosition.getMockImplementation();
    const prevOnMoved = mocks.winMock.onMoved.getMockImplementation();
    let moved: (() => void) | undefined;
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "db_load") return [];
      if (cmd === "workspace_load") return [];
      if (cmd === "bot_get_enabled") return false;
      return null;
    });
    mocks.winMock.outerPosition.mockResolvedValue({ x: 100, y: 5000 });
    mocks.winMock.onMoved.mockImplementation(async (...args: unknown[]) => {
      moved = args[0] as () => void;
      return () => {};
    });
    try {
      render(<WidgetApp />);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      if (typeof moved !== "function") throw new Error("onMoved 未注册");
      const fireMoved = moved;
      mocks.winMock.setPosition.mockClear();
      act(() => {
        fireMoved();
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(600); // settle 去抖 500ms + 余量
      });
      // LogicalPosition 类实例（测试内 mock 构造器只存 x/y）；mock 声明无参，
      // 调用参数形状按实际断言需要收窄
      const calls = mocks.winMock.setPosition.mock.calls as unknown as Array<
        [{ x: number; y: number }]
      >;
      expect(calls.length).toBeGreaterThan(0);
      const last = calls[calls.length - 1][0];
      expect(last.x).toBe(100); // 悬浮（不贴边）：x 不动
      expect(last.y).toBe(860); // 1080 - 220：钳回屏内
    } finally {
      // 复位默认实现，不向相邻用例泄漏超界 y / onMoved 捕获
      mocks.winMock.outerPosition.mockImplementation(prevPos!);
      mocks.winMock.onMoved.mockImplementation(prevOnMoved!);
    }
  });
});
