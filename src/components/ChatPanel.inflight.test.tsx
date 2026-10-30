import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { chatDefaultInvoke } from "../test/chatTestMocks";
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

mocks.invokeMock.mockImplementation(chatDefaultInvoke);
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
        return chatDefaultInvoke(cmd);
      },
    );
  };

  beforeEach(() => {
    db.clear();
  });
  afterEach(() => {
    mocks.invokeMock.mockImplementation(chatDefaultInvoke);
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
      return chatDefaultInvoke(cmd);
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
      return chatDefaultInvoke(cmd);
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
      return chatDefaultInvoke(cmd);
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
      return chatDefaultInvoke(cmd);
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

/* 后端发起的轮（围观执行会话 / 回灌接回）没有 invoke 收尾，终态事件就是
 * 唯一的清场时机：终态不清场，下一轮的正文会接在旧文本上、二次回灌被
 * 游标挡住（切走再切回丢正文的根）。 */
describe("后端发起轮：终态清场与回灌重建", () => {
  afterEach(() => {
    mocks.invokeMock.mockImplementation(chatDefaultInvoke);
    mocks.listenMock.mockImplementation(async () => () => {});
  });

  it("回灌轮收到终态 → 游标与权威副本一并清掉，下一轮从零累积", async () => {
    const fire: Record<
      string,
      (e: { payload: Record<string, unknown> }) => void
    > = {};
    mocks.listenMock.mockImplementation(async (event: string, cb: unknown) => {
      fire[event] = cb as (e: { payload: Record<string, unknown> }) => void;
      return () => {};
    });
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_stream_replay")
        return {
          events: [
            {
              seq: 1,
              event: "bot-chat-delta",
              payload: { text: "上一轮的尾巴", sessionId: "s1", seq: 1 },
            },
          ],
          lastSeq: 1,
          truncated: false,
          inflight: true,
        };
      return chatDefaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    await screen.findByText("上一轮的尾巴");
    await act(async () => {
      fire["bot-turn-status"]?.({
        payload: { status: "completed", sessionId: "s1" },
      });
    });
    await act(async () => {});
    expect(streamState.seqOf("s1")).toBe(0);
    expect(streamState.metaOf("s1")).toBeUndefined();
    expect(streamState.isInflight("s1")).toBe(false);
    expect(streamState.isResumed("s1")).toBe(false);
    // 下一轮的增量从零累积，不接旧尾巴
    await act(async () => {
      fire["bot-chat-delta"]?.({
        payload: { text: "新一轮", sessionId: "s1", seq: 2 },
      });
    });
    expect(streamState.metaOf("s1")?.text).toBe("新一轮");
  });

  it("二次回灌窗口与本地游标不连续 → 清残迹整轮重建，旧文不混入", async () => {
    const user = userEvent.setup();
    let s1Calls = 0;
    mocks.invokeMock.mockImplementation(
      async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "bot_sessions_load")
          return [
            { id: "s1", title: "会话一" },
            { id: "s2", title: "会话二" },
          ];
        if (cmd === "bot_stream_replay") {
          if ((args as { sessionId?: string })?.sessionId !== "s1")
            return { events: [], lastSeq: 0, truncated: false, inflight: false };
          s1Calls += 1;
          if (s1Calls === 1)
            return {
              events: [
                {
                  seq: 1,
                  event: "bot-chat-delta",
                  payload: { text: "上一轮", sessionId: "s1", seq: 1 },
                },
              ],
              lastSeq: 1,
              truncated: false,
              inflight: true,
            };
          // 缓冲头部已被裁（truncated）：新一轮事件从 seq 5 起，
          // 本地游标还停在 1——中间对不上，必须清掉重建
          return {
            events: [
              {
                seq: 5,
                event: "bot-chat-delta",
                payload: { text: "新一轮开头", sessionId: "s1", seq: 5 },
              },
            ],
            lastSeq: 5,
            truncated: true,
            inflight: true,
          };
        }
        return chatDefaultInvoke(cmd);
      },
    );
    render(<ChatPanel {...defaultProps} />);
    await screen.findByText("上一轮");
    // 切走再切回，触发第二次回灌
    await user.click(railSession("会话二"));
    await waitFor(() => expect(currentTitle()).toHaveTextContent("会话二"));
    await user.click(railSession("会话一"));
    await waitFor(() => expect(currentTitle()).toHaveTextContent("会话一"));
    await waitFor(() => {
      expect(streamState.metaOf("s1")?.text).toBe("新一轮开头");
    });
    expect(streamState.metaOf("s1")?.text).not.toContain("上一轮");
  });

  it("切会话卡在合并窗里：旧会话的增量不打进新会话的视图", async () => {
    const user = userEvent.setup();
    let delta: ((e: { payload: Record<string, unknown> }) => void) | null = null;
    mocks.listenMock.mockImplementation(async (event: string, cb: unknown) => {
      if (event === "bot-chat-delta")
        delta = cb as typeof delta;
      return () => {};
    });
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load")
        return [
          { id: "s1", title: "会话一" },
          { id: "s2", title: "会话二" },
        ];
      return chatDefaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    await waitFor(() => expect(currentTitle()).toHaveTextContent("会话一"));
    // 增量入队（rAF 合并窗挂着）后立刻切走
    await act(async () => {
      delta?.({ payload: { text: "旧会话的尾巴", sessionId: "s1", seq: 1 } });
    });
    await user.click(railSession("会话二"));
    await waitFor(() => expect(currentTitle()).toHaveTextContent("会话二"));
    // 放合并窗落地：这批增量属于 s1，不得进 s2 的视图
    await act(async () => {
      await new Promise((r) => setTimeout(r, 40));
    });
    expect(screen.queryByText("旧会话的尾巴")).not.toBeInTheDocument();
  });

  it("dead-man 看门狗：在途会话长时间无事件 → 判死回收并叫停后端", async () => {
    vi.useFakeTimers();
    try {
      render(<ChatPanel {...defaultProps} />);
      await act(async () => {});
      streamState.markInflight("s1");
      streamState.rewindActivityForTest("s1", 700_000);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(16_000);
      });
      expect(streamState.isInflight("s1")).toBe(false);
      expect(
        mocks.invokeMock.mock.calls.filter((c) => c[0] === "bot_stop"),
      ).not.toHaveLength(0);
      expect(screen.getByText(/长时间无任何流式事件/)).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });
});

/* 终态清场权与赛跑防线：invoke 轮的终态事件不得代 runChat 清场（收尾要
 * 从权威副本并 thinking/tools）；回放快照过期不得把死轮复活；世代守卫
 * 防旧轮迟到的收尾误清新轮——三条都是「删掉守卫、现有别处不红」的关键闸。 */
describe("终态清场权与赛跑防线", () => {
  afterEach(() => {
    mocks.invokeMock.mockImplementation(chatDefaultInvoke);
    mocks.listenMock.mockImplementation(async () => () => {});
  });

  it("invoke 轮的终态不清场：meta 存活到 runChat 收尾才清", async () => {
    const user = userEvent.setup();
    const fire: Record<
      string,
      (e: { payload: Record<string, unknown> }) => void
    > = {};
    mocks.listenMock.mockImplementation(async (event: string, cb: unknown) => {
      fire[event] = cb as (e: { payload: Record<string, unknown> }) => void;
      return () => {};
    });
    const db = new Map<string, { role: string; content: string }[]>();
    let release: ((v: { text: string; taskRefs?: unknown[] }) => void) | null =
      null;
    mocks.invokeMock.mockImplementation(
      async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "bot_sessions_load")
          return [{ id: "s1", title: "会话一" }];
        if (cmd === "bot_history_load")
          return db.get(String(args?.sessionId)) ?? [];
        if (cmd === "bot_history_save") {
          db.set(
            String(args?.sessionId),
            (args?.messages ?? []) as { role: string; content: string }[],
          );
          return null;
        }
        if (cmd === "bot_chat")
          return new Promise<{ text: string; taskRefs?: unknown[] }>((res) => {
            release = res;
          });
        return chatDefaultInvoke(cmd);
      },
    );
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("会话一");
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "发一轮");
    await user.keyboard("{Enter}");
    await act(async () => {
      fire["bot-chat-delta"]?.({
        payload: { text: "流出的正文", sessionId: "s1", seq: 1 },
      });
    });
    // 终态事件先于 invoke 落地到达：不得清 invoke 轮的权威副本
    await act(async () => {
      fire["bot-turn-status"]?.({
        payload: { status: "completed", sessionId: "s1", seq: 2 },
      });
    });
    expect(streamState.metaOf("s1")?.text).toBe("流出的正文");
    expect(streamState.isInflight("s1")).toBe(true);
    expect(streamState.isResumed("s1")).toBe(false);
    // 放行 invoke：runChat 的 finally 才统一清场
    await act(async () => {
      release?.({ text: "最终答复", taskRefs: [] });
    });
    await waitFor(() =>
      expect(streamState.metaOf("s1")).toBeUndefined(),
    );
    expect(streamState.isInflight("s1")).toBe(false);
  });

  it("回放快照过期（fetch 期间终态已到）→ 不得把死轮重新标在途", async () => {
    const fire: Record<
      string,
      (e: { payload: Record<string, unknown> }) => void
    > = {};
    mocks.listenMock.mockImplementation(async (event: string, cb: unknown) => {
      fire[event] = cb as (e: { payload: Record<string, unknown> }) => void;
      return () => {};
    });
    let resolveReplay: ((v: unknown) => void) | null = null;
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_stream_replay")
        return new Promise((res) => {
          resolveReplay = res;
        });
      if (cmd === "bot_history_load") return [];
      return chatDefaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    await waitFor(() => expect(resolveReplay).toBeTruthy());
    // fetch 挂起期间终态事件先处理（代际 bump）
    await act(async () => {
      fire["bot-turn-status"]?.({
        payload: { status: "completed", sessionId: "s1" },
      });
    });
    // 旧快照随后返回：inflight=true、事件里没有终态（取自终态记录之前）
    await act(async () => {
      resolveReplay?.({
        events: [
          {
            seq: 1,
            event: "bot-chat-delta",
            payload: { text: "旧半截", sessionId: "s1", seq: 1 },
          },
        ],
        lastSeq: 1,
        truncated: false,
        inflight: true,
      });
    });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 30));
    });
    expect(
      streamState.isInflight("s1"),
      "死轮不得复活（会卡 busy 到看门狗兜底）",
    ).toBe(false);
    expect(streamState.metaOf("s1")).toBeUndefined();
    const loads = mocks.invokeMock.mock.calls.filter(
      (c) => c[0] === "bot_history_load",
    ).length;
    expect(loads, "过期快照按收尾收口（loadFinalText 刷库）").toBeGreaterThan(
      1,
    );
  });

  it("世代守卫：旧轮迟到的收尾不得清掉新轮状态", () => {
    streamState.markInflight("sx");
    const staleTurn = streamState.beginTurn("sx");
    // 看门狗判死 → 状态清空 → 用户开新轮
    streamState.dropSession("sx");
    streamState.markInflight("sx");
    const freshTurn = streamState.beginTurn("sx");
    expect(streamState.endTurn("sx", staleTurn)).toBe(false);
    expect(streamState.isInflight("sx")).toBe(true);
    expect(streamState.endTurn("sx", freshTurn)).toBe(true);
    streamState.dropSession("sx");
  });

  it("markInflight 刷新活动时刻：新轮不继承旧时间戳", () => {
    streamState.markInflight("sy");
    streamState.rewindActivityForTest("sy", 700_000);
    expect(Date.now() - streamState.lastActivityOf("sy")).toBeGreaterThan(
      660_000,
    );
    streamState.markInflight("sy");
    expect(Date.now() - streamState.lastActivityOf("sy")).toBeLessThan(1_000);
    streamState.dropSession("sy");
  });

  it("终态代际是墓碑：dropSession 之后仍可对出「已收尾」", () => {
    const e0 = streamState.termEpochOf("sz");
    streamState.bumpTermEpoch("sz");
    expect(streamState.termEpochOf("sz")).toBeGreaterThan(e0);
    streamState.dropSession("sz");
    expect(
      streamState.termEpochOf("sz"),
      "终态代际不随 dropSession 清除，否则过期快照重新对上号",
    ).toBeGreaterThan(e0);
  });
});
