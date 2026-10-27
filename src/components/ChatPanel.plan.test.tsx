import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ChatPanel } from "./ChatPanel";
import * as planStore from "./ChatPanel/planStore";
import * as streamState from "./ChatPanel/streamState";
import { stubScrollNoop } from "../test/scrollNoop";

/* 计划浮层的实时刷新：事件到了，浮层当场就变。
 *
 * 这条守的是修好之后的那条路。此前后端 `bot-plan` 事件载荷没带会话号，
 * 前端按会话过滤的闸门把每条实时更新都丢了——浮层只在重新挂载
 * （关掉再点开拉快照）时才更新，用户看着像「不实时刷新」。
 */
stubScrollNoop();

const mocks = vi.hoisted(() => {
  const invokeMock = vi.fn();
  const listenMock = vi.fn();
  const emitMock = vi.fn();
  // 按事件名捕获 listen 回调（手动触发用）
  const listeners: Record<
    string,
    Array<(e: { payload: Record<string, unknown> }) => void>
  > = {};
  const dragDropHandlers: Array<
    (e: { payload: Record<string, unknown> }) => unknown
  > = [];
  const askMock = vi.fn();
  return { invokeMock, listenMock, emitMock, listeners, dragDropHandlers, askMock };
});

const defaultInvoke = async (cmd: string) => {
  switch (cmd) {
    case "bot_sessions_load":
      return [{ id: "s1", title: "默认会话" }];
    case "bot_history_load":
      return [];
    case "bot_plan_state":
      return null;
    case "bot_session_file_changes":
      return { items: [] };
    case "bot_inflight_list":
      return [];
    case "bot_stream_replay":
      return { events: [], lastSeq: 0, truncated: false, inflight: false };
    case "bot_get_config":
      return {
        baseUrl: "https://api.example.com/v1",
        model: "MiniMax-M3",
        apiProvider: "openai",
        modelsByProvider: {
          openai: [
            { id: "m1", label: "MiniMax-M3", baseUrl: "https://api.example.com/v1", model: "MiniMax-M3" },
          ],
        },
      };
    default:
      return null;
  }
};

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
  tasks: [],
  onUpdateTask: () => {},
};

/** 触发一个已注册的事件回调（模拟后端广播到达） */
async function fire(event: string, payload: Record<string, unknown>) {
  const cbs = mocks.listeners[event] ?? [];
  await act(async () => {
    for (const cb of cbs) cb({ payload });
  });
}

/** 打开计划浮层 */
async function openPlanPanel(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("button", { name: "展开计划面板" }));
  await screen.findByRole("complementary", { name: "执行计划面板" });
}

beforeEach(() => {
  mocks.invokeMock.mockClear();
  mocks.listenMock.mockClear();
  mocks.emitMock.mockClear();
  mocks.emitMock.mockImplementation(async () => {});
  mocks.listenMock.mockImplementation(
    async (
      event: string,
      cb: (e: { payload: Record<string, unknown> }) => void,
    ) => {
      (mocks.listeners[event] ??= []).push(cb);
      return () => {};
    },
  );
  mocks.invokeMock.mockImplementation(defaultInvoke);
  mocks.askMock.mockReset();
  mocks.askMock.mockResolvedValue(true);
  writeTextMock.mockClear();
  mocks.dragDropHandlers.length = 0;
  for (const k of Object.keys(mocks.listeners)) delete mocks.listeners[k];
  // 模块级 store 跨用例存活（面板重挂载要接得住），不复位会串味
  streamState.resetAll();
  planStore.resetAll();
});

describe("计划浮层：实时刷新", () => {
  it("bot-plan 事件到达 → 浮层当场刷新（不用关掉再点开）", async () => {
    const user = userEvent.setup();
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    await openPlanPanel(user);
    expect(screen.getByText(/本会话暂无计划/)).toBeInTheDocument();

    // 后端统一出口盖了信封：载荷带 sessionId，前端按会话过滤后认领
    await fire("bot-plan", {
      sessionId: "s1",
      steps: [{ content: "改代码", status: "in_progress" }],
      updatedAt: 200,
      turnStartedAt: 100,
    });

    expect(await screen.findByText("改代码")).toBeInTheDocument();
    // 浮层一直开着（没关过）：内容是这次事件带来的
    expect(
      screen.getByRole("complementary", { name: "执行计划面板" }),
    ).toBeInTheDocument();
  });

  it("别的会话的事件不落到当前浮层上", async () => {
    const user = userEvent.setup();
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    await openPlanPanel(user);

    await fire("bot-plan", {
      sessionId: "s2",
      steps: [{ content: "别的会话的步骤", status: "pending" }],
      updatedAt: 300,
      turnStartedAt: 100,
    });

    expect(screen.queryByText("别的会话的步骤")).not.toBeInTheDocument();
  });

  // 回放补齐与实时订阅走**同一批**处理器：回放出来的计划事件也要落到浮层，
  // 否则重连后浮层还停在旧状态。
  it("在途回灌：回放里的 bot-plan 事件也要落到浮层", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_stream_replay")
        return {
          events: [
            {
              seq: 1,
              event: "bot-plan",
              payload: {
                sessionId: "s1",
                seq: 1,
                steps: [{ content: "补齐的步骤", status: "completed" }],
                updatedAt: 200,
                turnStartedAt: 100,
              },
            },
          ],
          lastSeq: 1,
          truncated: false,
          inflight: true,
        };
      return defaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    // 回灌在挂载时跑完，浮层后开也要看得见这份计划（状态在 store 里，不在浮层里）
    await waitFor(() => {
      expect(planStore.planOf("s1")?.steps[0].content).toBe("补齐的步骤");
    });
    await openPlanPanel(user);
    expect(screen.getByText("补齐的步骤")).toBeInTheDocument();
  });
});
