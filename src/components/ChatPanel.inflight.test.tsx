import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, act, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ChatPanel } from "./ChatPanel";
import * as streamState from "./ChatPanel/streamState";
import { stubScrollNoop } from "../test/scrollNoop";

// ChatPanel 自动滚动依赖 jsdom 未实现的 scrollTo/scrollIntoView（scoped noop）
stubScrollNoop();

/** 会话标题在两处渲染：常驻侧栏（导航）与聊天头部（上下文指示）。
 *  用例要定位时按所在区域限定作用域，否则 findByText 会命中两处报歧义。 */
const rail = () =>
  within(screen.getByRole("complementary", { name: "会话列表" }));
/** 侧栏内按标题找会话行（切会话的点击目标） */
const railSession = (title: string | RegExp) => rail().getByText(title);
/** 头部标题：确认「当前在看哪一段对话」 */
const currentTitle = () => screen.getByRole("heading", { level: 2 });

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
/* 在途回合不丢：发出去的消息，切走再切回来必须还在。
 *
 * 根因：普通对话的历史只有前端在**回合收尾时**整段写库——回合进行中，
 * 用户消息只活在 React 内存里。任何一次「按会话重载历史」（切会话 /
 * 跨窗口跳会话 / 执行会话围观）都用库里旧历史整段替换视图，在途的用户
 * 消息当场消失；下一回合收尾再把这份残缺视图整段写回库，丢失就变永久。 */
describe("在途回合（切走再切回不丢消息）", () => {
  /** 仿库的存/取：bot_history_save 整段覆盖写，bot_history_load 原样读回 */
  const db = new Map<string, { role: string; content: string }[]>();
  const sessions = [
    { id: "s1", title: "会话一" },
    { id: "s2", title: "会话二" },
  ];
  /** 机器人一直不回复：回合保持在途，正好复现「消息发出去还没回来」的时刻 */
  const useDb = () => {
    mocks.invokeMock.mockImplementation(
      async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "bot_sessions_load") return sessions;
        if (cmd === "bot_history_load")
          return db.get(String(args?.sessionId)) ?? [];
        if (cmd === "bot_history_save") {
          db.set(
            String(args?.sessionId),
            (args?.messages ?? []) as { role: string; content: string }[],
          );
          return null;
        }
        if (cmd === "bot_chat") return new Promise(() => {});
        return defaultInvoke(cmd);
      },
    );
  };

  beforeEach(() => {
    db.clear();
  });
  afterEach(() => {
    mocks.invokeMock.mockImplementation(defaultInvoke);
    mocks.listenMock.mockImplementation(async () => () => {});
  });

  it("发出去的消息：切到别的会话再切回来，这条消息必须还在", async () => {
    const user = userEvent.setup();
    useDb();
    render(<ChatPanel {...defaultProps} />);
    expect(await screen.findAllByText("会话一")).not.toHaveLength(0);
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "是不是Python撞钟墙了？");
    await user.keyboard("{Enter}");
    expect(
      await screen.findByText("是不是Python撞钟墙了？"),
    ).toBeInTheDocument();
    // 切到另一个会话
    await user.click(railSession("会话二"));
    await waitFor(() => expect(currentTitle()).toHaveTextContent("会话二"));
    // 再切回来：这一轮还没回完，视图里不能凭空少掉用户发的那条
    await user.click(railSession("会话一"));
    await waitFor(() => expect(currentTitle()).toHaveTextContent("会话一"));
    expect(
      await screen.findByText("是不是Python撞钟墙了？"),
    ).toBeInTheDocument();
  });

  it("已流出的回复正文：切走再切回，占位气泡要接着这段文字", async () => {
    const user = userEvent.setup();
    let deltaHandler:
      | ((e: { payload: Record<string, unknown> }) => void)
      | null = null;
    mocks.listenMock.mockImplementation(async (event: string, cb: unknown) => {
      if (event === "bot-chat-delta")
        deltaHandler = cb as typeof deltaHandler;
      return () => {};
    });
    useDb();
    render(<ChatPanel {...defaultProps} />);
    expect(await screen.findAllByText("会话一")).not.toHaveLength(0);
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "继续");
    await user.keyboard("{Enter}");
    await act(async () => {
      (
        deltaHandler as unknown as (e: {
          payload: Record<string, unknown>;
        }) => void
      )({ payload: { text: "已经说了一半", sessionId: "s1" } });
    });
    expect(await screen.findByText("已经说了一半")).toBeInTheDocument();
    await user.click(railSession("会话二"));
    await waitFor(() => expect(currentTitle()).toHaveTextContent("会话二"));
    await user.click(railSession("会话一"));
    await waitFor(() => expect(currentTitle()).toHaveTextContent("会话一"));
    // 已读到的半截回复要接得上，不凭空少一半
    expect(await screen.findByText("已经说了一半")).toBeInTheDocument();
  });

  // 应用重启 / 面板重挂载后接回在途回复：后端说这一轮还在跑、本地没有它的
  // 状态 → 把这一轮的事件按序补齐（补齐走的是与实时订阅同一批处理器）。
  it("在途回灌：后端说还在跑 → 这一轮的事件补进气泡", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_stream_replay")
        return {
          events: [
            {
              seq: 1,
              event: "bot-chat-delta",
              payload: { text: "补齐的半截", sessionId: "s1", seq: 1 },
            },
          ],
          lastSeq: 1,
          truncated: false,
          inflight: true,
        };
      return defaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    expect(await screen.findByText("补齐的半截")).toBeInTheDocument();
  });

  // 补齐与实时会重叠：同一条事件两边各来一次时，第二次必须被序号闸挡住——
  // 不挡就是「重说了一遍」（正文叠成两半）。
  it("在途回灌：同号事件再从实时通道来一次 → 不重复叠加", async () => {
    let delta: ((e: { payload: Record<string, unknown> }) => void) | null = null;
    mocks.listenMock.mockImplementation(async (event: string, cb: unknown) => {
      if (event === "bot-chat-delta")
        delta = cb as typeof delta;
      return () => {};
    });
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_stream_replay")
        return {
          events: [
            {
              seq: 1,
              event: "bot-chat-delta",
              payload: { text: "只该有一遍", sessionId: "s1", seq: 1 },
            },
          ],
          lastSeq: 1,
          truncated: false,
          inflight: true,
        };
      return defaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    expect(await screen.findByText("只该有一遍")).toBeInTheDocument();
    // 实时通道把同一条（seq=1）再送一次 → 序号闸挡住，正文不叠
    await act(async () => {
      (
        delta as unknown as (e: { payload: Record<string, unknown> }) => void
      )({
        payload: { text: "只该有一遍", sessionId: "s1", seq: 1 },
      });
    });
    await new Promise((r) => setTimeout(r, 50));
    const hits = screen.getAllByText("只该有一遍");
    expect(hits).toHaveLength(1);
  });

  // 后端说没在跑 → 不回灌（这一轮已经收尾，正文以库为准）
  it("在途回灌：后端说已收尾 → 不补任何东西", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_stream_replay")
        return { events: [], lastSeq: 0, truncated: false, inflight: false };
      return defaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    await waitFor(() => {
      expect(
        mocks.invokeMock.mock.calls.filter((c) => c[0] === "bot_stream_replay"),
      ).not.toHaveLength(0);
    });
    expect(screen.queryByText("补齐的半截")).not.toBeInTheDocument();
  });

  // 挂载时把后端在跑的那些会话都接回来——不止当前会话：并行回复（PAR-1）
  // 的那些会话的半截内容也在，别让它们只剩最终正文。
  it("挂载时接回在途列表里的其他会话", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "bot_inflight_list")
        return [
          { sessionId: "s2", startedAt: 1, lastSeq: 1, eventCount: 1 },
        ];
      if (cmd === "bot_stream_replay" && (args as { sessionId?: string })?.sessionId === "s2")
        return {
          events: [
            {
              seq: 1,
              event: "bot-chat-delta",
              payload: { text: "另一会话的半截", sessionId: "s2", seq: 1 },
            },
          ],
          lastSeq: 1,
          truncated: false,
          inflight: true,
        };
      return defaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    // s2 不在当前视图 → 内容进的是权威副本（切过去就能接上）
    await waitFor(() => {
      expect(streamState.metaOf("s2")?.text).toBe("另一会话的半截");
    });
    expect(streamState.isInflight("s2")).toBe(true);
  });

  it("回合开始即落库：在途时用户消息已经进库（崩了也不丢）", async () => {
    const user = userEvent.setup();
    useDb();
    render(<ChatPanel {...defaultProps} />);
    expect(await screen.findAllByText("会话一")).not.toHaveLength(0);
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "先记下来");
    await user.keyboard("{Enter}");
    // 回合还没结束，库里已经有这条用户消息
    await waitFor(() => {
      const saved = mocks.invokeMock.mock.calls.find(
        (c) => c[0] === "bot_history_save" && c[1]?.sessionId === "s1",
      );
      expect(saved).toBeTruthy();
      expect(saved?.[1]?.messages).toEqual([
        expect.objectContaining({ role: "user", content: "先记下来" }),
      ]);
    });
  });
});
