import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, act, fireEvent } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { ChatPanel } from "./ChatPanel";
import * as streamState from "./ChatPanel/streamState";
import { stubScrollNoop } from "../test/scrollNoop";

// ChatPanel 自动滚动依赖 jsdom 未实现的 scrollTo/scrollIntoView（scoped noop）
stubScrollNoop();

// vi.mock 工厂会被提升到顶部，因此共享 mock 变量必须用 vi.hoisted 包裹
const mocks = vi.hoisted(() => {
  const invokeMock = vi.fn();
  const listenMock = vi.fn();
  const emitMock = vi.fn();
  // 捕获 ChatPanel 注册的窗口拖放回调（拖文件进聊天区 → 附件），供用例手动触发
  const dragDropHandlers: Array<
    (e: { payload: Record<string, unknown> }) => unknown
  > = [];
  // 按事件名捕获 listen 回调（chat-open-session 手动触发用）
  const listeners: Record<
    string,
    Array<(e: { payload: Record<string, unknown> }) => void>
  > = {};
  const askMock = vi.fn();
  return { invokeMock, listenMock, emitMock, dragDropHandlers, listeners, askMock };
});

// 默认实现（具名：beforeEach 只 mockClear 不重置 implementation，
// 后跑的用例需显式 mockImplementation(defaultInvoke) 恢复）
const defaultInvoke = async (cmd: string) => {
  switch (cmd) {
    case "bot_sessions_load":
      return [{ id: "s1", title: "默认会话" }];
    case "bot_session_create":
      return { id: "s-new", title: "新对话" };
    case "bot_history_load":
      return [];
    case "bot_history_save":
    case "bot_history_clear":
    case "bot_session_delete":
    case "bot_session_rename":
      return null;
    case "bot_chat":
      return { text: "机器人回复", taskRefs: [] };
    // 双命令：🧠 模型下拉的列表 + 窄口径切换
    case "bot_get_config":
      return {
        baseUrl: "https://api.example.com/v1",
        model: "MiniMax-M3",
        apiProvider: "openai",
        modelsByProvider: {
          openai: [
            {
              id: "m1",
              label: "MiniMax-M3",
              baseUrl: "https://api.example.com/v1",
              model: "MiniMax-M3",
            },
            {
              id: "m2",
              label: "DeepSeek-V3",
              baseUrl: "https://api.deepseek.com",
              model: "deepseek-chat",
            },
          ],
          anthropic: [],
        },
        activeModelId: { openai: "m1", anthropic: null },
      };
    case "bot_set_active_model":
      return {
        model: "deepseek-chat",
        apiProvider: "openai",
        activeModelId: { openai: "m2", anthropic: null },
      };
    case "bot_execute_task":
      return { text: "任务执行结果", taskRefs: [] };
    case "bot_compact":
      return "压缩后的摘要";
    case "bot_stop":
      return null;
    case "pick_files_dialog":
      return [];
    default:
      return null;
  }
};
mocks.invokeMock.mockImplementation(defaultInvoke);
mocks.listenMock.mockImplementation(async () => () => {});
mocks.emitMock.mockImplementation(async () => {});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invokeMock,
  convertFileSrc: vi.fn((p: string) => `tauri://localhost/${p}`),
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: mocks.emitMock,
  listen: mocks.listenMock,
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onDragDropEvent: async (
      cb: (e: { payload: Record<string, unknown> }) => unknown,
    ) => {
      mocks.dragDropHandlers.push(cb);
      return () => {};
    },
    scaleFactor: async () => 1,
  }),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn(async () => {}),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  ask: (...args: unknown[]) => mocks.askMock(...args),
}));

// navigator.clipboard.writeText
const writeTextMock = vi.fn(async () => {});
Object.defineProperty(navigator, "clipboard", {
  configurable: true,
  value: { writeText: writeTextMock },
});

const defaultProps = {
  enabled: true,
  // 右侧上下文栏的任务数据与改写回调（App 侧接 updateTask），
  // 这里只验 ChatPanel 内部的传递，不复现 App 的接线
  tasks: [],
  onUpdateTask: () => {},
};

beforeEach(() => {
  mocks.invokeMock.mockClear();
  mocks.listenMock.mockClear();
  mocks.askMock.mockReset();
  mocks.askMock.mockResolvedValue(true);
  writeTextMock.mockClear();
  mocks.dragDropHandlers.length = 0;
  for (const k of Object.keys(mocks.listeners)) delete mocks.listeners[k];
  // 侧栏/水位档位等视图态持久化在 localStorage，jsdom 同一文件内跨用例共享——
  // 不清会让「默认收起」这类用例被前一个用例写下的展开态污染
  localStorage.clear();
  // 流式在途态在模块 store 里（跨面板挂载存活），同样跨用例共享——
  // 不复位会让上一例的「在途」改写这一例的渲染（占位文案 / 停止键）
  streamState.resetAll();
});
/* 聊天窗滚动（流式期）：用户翻历史时视口不能被流式帧抢回底部。
 *
 * 根因：自动滚动只挂在 messages 上，且无条件 scrollTo 底——流式增量每 16ms
 * 换一次引用，于是每帧都把视口拽回底部：历史看不了，人和程序抢滚动位置 → 抖。 */
describe("聊天窗滚动（流式期）", () => {
  /** jsdom 没有布局：scrollHeight/clientHeight 恒 0，滚动语义测不出来。
   *  给容器造一份可滚的几何，scrollTop 可写可读。 */
  const stubGeometry = (
    el: HTMLElement,
    { height = 1000, view = 400 } = {},
  ) => {
    let top = 0;
    Object.defineProperty(el, "scrollHeight", {
      value: height,
      configurable: true,
    });
    Object.defineProperty(el, "clientHeight", {
      value: view,
      configurable: true,
    });
    Object.defineProperty(el, "scrollTop", {
      get: () => top,
      set: (v: number) => {
        top = v;
      },
      configurable: true,
    });
  };

  /** 起一个「机器人正在跑」的会话：bot_chat 挂起不返回，气泡保持 streaming */
  const startStreaming = async (user: UserEvent) => {
    let deltaHandler:
      | ((e: { payload: Record<string, unknown> }) => void)
      | null = null;
    mocks.listenMock.mockImplementation(async (event: string, cb: unknown) => {
      if (event === "bot-chat-delta")
        deltaHandler = cb as typeof deltaHandler;
      return () => {};
    });
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_chat") return new Promise(() => {}); // 挂起不返回
      return null;
    });
    render(<ChatPanel {...defaultProps} />);
    expect(await screen.findAllByText("默认会话")).not.toHaveLength(0);
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "你好");
    await user.keyboard("{Enter}");
    const fire = (text: string) =>
      (
        deltaHandler as unknown as (e: {
          payload: Record<string, unknown>;
        }) => void
      )({ payload: { text, sessionId: "s1" } });
    return { fire, list: screen.getByTestId("chat-scroll") };
  };

  it("用户往上翻历史后，流式增量不再把视口拽回底部", async () => {
    const user = userEvent.setup();
    const { fire, list } = await startStreaming(user);
    stubGeometry(list);
    const scrollTo = vi.spyOn(Element.prototype, "scrollTo");
    // 用户往上翻（离底 500px）
    list.scrollTop = 100;
    fireEvent.scroll(list);
    scrollTo.mockClear();
    // 机器人继续吐字 → 视口必须留在用户翻到的位置
    await act(async () => fire("继续输出"));
    expect(await screen.findByText("继续输出")).toBeInTheDocument();
    expect(scrollTo).not.toHaveBeenCalled();
    scrollTo.mockRestore();
  });

  it("贴着底时，流式增量仍然自动滚到底（跟随不丢）", async () => {
    const user = userEvent.setup();
    const { fire, list } = await startStreaming(user);
    stubGeometry(list);
    const scrollTo = vi.spyOn(Element.prototype, "scrollTo");
    scrollTo.mockClear();
    await act(async () => fire("新输出"));
    expect(await screen.findByText("新输出")).toBeInTheDocument();
    expect(scrollTo).toHaveBeenCalledWith({ top: 1000 });
    scrollTo.mockRestore();
  });

  it("翻过历史后发消息：视口回到底部跟着这一轮", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_chat") return { text: "机器人回复", taskRefs: [] };
      return null;
    });
    render(<ChatPanel {...defaultProps} />);
    expect(await screen.findAllByText("默认会话")).not.toHaveLength(0);
    const list = screen.getByTestId("chat-scroll");
    stubGeometry(list);
    const scrollTo = vi.spyOn(Element.prototype, "scrollTo");
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "你好");
    await user.keyboard("{Enter}");
    // 用户翻历史
    list.scrollTop = 100;
    fireEvent.scroll(list);
    scrollTo.mockClear();
    // 再发一条 → 用户主动发的，视口必须跟到底部
    await user.type(input, "再来一条");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(scrollTo).toHaveBeenCalledWith({ top: 1000 }));
    scrollTo.mockRestore();
  });

  it("切走再切回聊天页：视口回到底部（display:none 会清零滚动口）", async () => {
    // jsdom 没有 ResizeObserver：补一个可控替身，模拟「容器从隐藏变可见」
    const observers: Array<() => void> = [];
    class FakeResizeObserver {
      constructor(cb: () => void) {
        observers.push(cb);
      }
      observe() {}
      disconnect() {}
    }
    (
      globalThis as unknown as { ResizeObserver?: unknown }
    ).ResizeObserver = FakeResizeObserver;
    try {
      const user = userEvent.setup();
      const { list } = await startStreaming(user);
      const scrollTo = vi.spyOn(Element.prototype, "scrollTo");
      scrollTo.mockClear();
      // 聊天页切走：容器 display:none → 没有滚动口，浏览器把滚动位置清零
      stubGeometry(list, { height: 0, view: 0 });
      observers.forEach((cb) => cb());
      // 切回来：容器重新有高度 → 视口必须回到底部（否则停在历史开头，
      // 再来一帧流式增量又被拽下去，看着像吞内容）
      stubGeometry(list);
      observers.forEach((cb) => cb());
      expect(scrollTo).toHaveBeenCalledWith({ top: 1000 });
      scrollTo.mockRestore();
    } finally {
      delete (globalThis as unknown as { ResizeObserver?: unknown })
        .ResizeObserver;
    }
  });

  it("机器人报错：已流出的正文不被吞掉（错误接在正文后面）", async () => {
    const user = userEvent.setup();
    let deltaHandler:
      | ((e: { payload: Record<string, unknown> }) => void)
      | null = null;
    mocks.listenMock.mockImplementation(async (event: string, cb: unknown) => {
      if (event === "bot-chat-delta")
        deltaHandler = cb as typeof deltaHandler;
      return () => {};
    });
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_chat")
        return new Promise((_, reject) =>
          setTimeout(() => reject(new Error("boom")), 60),
        );
      return null;
    });
    render(<ChatPanel {...defaultProps} />);
    expect(await screen.findAllByText("默认会话")).not.toHaveLength(0);
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "你好");
    await user.keyboard("{Enter}");
    await act(async () => {
      (
        deltaHandler as unknown as (e: {
          payload: Record<string, unknown>;
        }) => void
      )({ payload: { text: "已经说了一半", sessionId: "s1" } });
    });
    const list = screen.getByTestId("chat-scroll");
    expect(await screen.findByText("已经说了一半")).toBeInTheDocument();
    // 收尾报错：正文必须还在（整段替换 = 用户读到的回复凭空消失）
    await waitFor(() => expect(list.textContent).toContain("boom"));
    expect(list.textContent).toContain("已经说了一半");
  });

});
