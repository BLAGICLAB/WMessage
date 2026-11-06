import { describe, it, expect, vi, beforeEach } from "vitest";
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
mocks.listenMock.mockImplementation(async (event: string, cb: unknown) => {
  ((mocks.listeners[event] ??= []) as Array<typeof cb>).push(
    cb as (e: { payload: Record<string, unknown> }) => void,
  );
  return () => {};
});
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
describe("ChatPanel", () => {
  it("空消息时输入框可以打字，回车后调用 bot_chat 并展示用户消息", async () => {
    const user = userEvent.setup();
    render(<ChatPanel {...defaultProps} />);
    // 等待初始加载完成
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_sessions_load");
    });
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "你好");
    expect(input).toHaveValue("你好");
    await user.keyboard("{Enter}");
    // 消息气泡出现
    expect(await screen.findByText("你好")).toBeInTheDocument();
    // 触发 bot_chat RPC（真源化契约：只发本轮意图 + 预生成 clientMsgId）
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_chat",
        expect.objectContaining({
          sessionId: "s1",
          content: "你好",
          clientMsgId: expect.any(String),
        }),
      );
    });
    // 真源化：终稿行经 bot-msg-upserted 事件回显（后端落行，前端投影）
    await act(async () => {
      for (const cb of mocks.listeners["bot-msg-upserted"] ?? []) {
        cb({
          payload: {
            sessionId: "s1",
            msgId: "m1",
            role: "assistant",
            content: "机器人回复",
            turnStatus: "completed",
          },
        });
      }
    });
    expect(await screen.findByText("机器人回复")).toBeInTheDocument();
  });

  it("输入 /stop 后按发送：本会话无在途回复 → 提示「当前对话没有进行中的回复」", async () => {
    const user = userEvent.setup();
    render(<ChatPanel {...defaultProps} />);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_sessions_load");
    });
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    // 直接以带空格的形式输入（picker 不会因含空格弹出，send 路径走 runSlashCommand）
    await user.type(input, "/stop ");
    // 点击「发送」按钮触发 send → runSlashCommand → /stop
    // （UI-1 圆形发送键：图标按钮以 aria-label 定位）
    await user.click(screen.getByRole("button", { name: "发送" }));
    // 本会话无在途回复 → addHint："当前对话没有进行中的回复"，bot_stop 不调用
    await waitFor(() => {
      expect(screen.getByText(/当前对话没有进行中的回复/)).toBeInTheDocument();
    });
    expect(mocks.invokeMock).not.toHaveBeenCalledWith("bot_stop");
  });

  it("本地提示不进历史库、不进模型上下文（/stop 无在途 → 提示 → 再发一条）", async () => {
    const user = userEvent.setup();
    render(<ChatPanel {...defaultProps} />);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_sessions_load");
    });
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    // 触发提示：/stop 且本会话无在途回复
    await user.type(input, "/stop ");
    await user.click(screen.getByRole("button", { name: "发送" }));
    await waitFor(() => {
      expect(screen.getByText(/当前对话没有进行中的回复/)).toBeInTheDocument();
    });
    // 再发一条正常消息：提示不得进入落库载荷与模型载荷
    await user.type(input, "第二个问题");
    await user.keyboard("{Enter}");
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_chat",
        expect.objectContaining({
          content: "第二个问题",
          clientMsgId: expect.any(String),
        }),
      );
    });
    // 正常回合前端零落库（终稿行归后端）：提示无从混进落库载荷
    const saveCalls = mocks.invokeMock.mock.calls.filter(
      (c) => c[0] === "bot_history_save",
    );
    expect(saveCalls.length).toBe(0);
    const chatCalls = mocks.invokeMock.mock.calls.filter(
      (c) => c[0] === "bot_chat",
    );
    const chatCall = chatCalls[chatCalls.length - 1];
    expect(JSON.stringify(chatCall?.[1])).not.toContain("当前对话没有进行中的回复");
  });

  it("移除一条回复：按行删（bot_history_drop），提示不混进任何载荷", async () => {
    const user = userEvent.setup();
    render(<ChatPanel {...defaultProps} />);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_sessions_load");
    });
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    // 先跑完一轮，让列表里有一条可移除的助手回复
    await user.type(input, "你好");
    await user.keyboard("{Enter}");
    await act(async () => {
      for (const cb of mocks.listeners["bot-msg-upserted"] ?? []) {
        cb({
          payload: {
            sessionId: "s1",
            msgId: "m1",
            role: "assistant",
            content: "机器人回复",
            turnStatus: "completed",
          },
        });
      }
    });
    expect(await screen.findByText("机器人回复")).toBeInTheDocument();
    // 再造一条本地提示（/stop 且本会话无在途回复）
    await user.type(input, "/stop ");
    await user.click(screen.getByRole("button", { name: "发送" }));
    await waitFor(() => {
      expect(screen.getByText(/当前对话没有进行中的回复/)).toBeInTheDocument();
    });
    // 移除助手回复 → 按行删（真源化批 4：全量快照写权已断代）
    const bubbleRow = screen
      .getByText("机器人回复")
      .closest("div.chat-bubble")?.parentElement;
    expect(bubbleRow).not.toBeNull();
    await user.click(
      within(bubbleRow as HTMLElement).getByTitle(
        "移除这条回复（不满意时删除，不再作为上下文）",
      ),
    );
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_history_drop", {
        sessionId: "s1",
        msgId: "m1",
      });
    });
    // 全量保存通道已退役：提示无从混进任何落库载荷
    const saveCalls = mocks.invokeMock.mock.calls.filter(
      (c) => c[0] === "bot_history_save",
    );
    expect(saveCalls.length).toBe(0);
    expect(screen.queryByText("机器人回复")).not.toBeInTheDocument();
  });

  it("bot_chat 回传 traceId：答复照常呈现（trace 归属跟着消息走）", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_chat") {
        return { text: "带 trace 的答复", taskRefs: [], traceId: 123 };
      }
      return chatDefaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_sessions_load");
    });
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "你好");
    await user.keyboard("{Enter}");
    await act(async () => {
      for (const cb of mocks.listeners["bot-msg-upserted"] ?? []) {
        cb({
          payload: {
            sessionId: "s1",
            msgId: "m1",
            role: "assistant",
            content: "带 trace 的答复",
            turnStatus: "completed",
            traceId: 123,
          },
        });
      }
    });
    expect(await screen.findByText("带 trace 的答复")).toBeInTheDocument();
  });

  it("回合带 traceId：气泡上有「执行轨迹」入口，点开按会话查执行历史", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_chat") {
        return { text: "带 trace 的答复", taskRefs: [], traceId: 123 };
      }
      if (cmd === "trace_list") return [];
      return chatDefaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_sessions_load");
    });
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "你好");
    await user.keyboard("{Enter}");
    await act(async () => {
      for (const cb of mocks.listeners["bot-msg-upserted"] ?? []) {
        cb({
          payload: {
            sessionId: "s1",
            msgId: "m1",
            role: "assistant",
            content: "带 trace 的答复",
            turnStatus: "completed",
            traceId: 123,
          },
        });
      }
    });
    const btn = await screen.findByRole("button", { name: /执行轨迹/ });
    await user.click(btn);
    // 按会话维度查（同会话的多次执行列出来），并预选这一轮那条
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("trace_list", {
        sessionId: "s1",
        limit: 20,
      });
    });
  });

  it("回合终态跟着消息走：历史行带的 turnStatus 在保存回程原样带出", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_history_load") {
        return [
          {
            role: "user",
            content: "问",
            refsJson: null,
            thinking: null,
            toolsJson: null,
            turnStatus: null,
          },
          {
            role: "assistant",
            content: "被打断的答复",
            refsJson: null,
            thinking: null,
            toolsJson: null,
            turnStatus: "interrupted",
          },
        ];
      }
      return chatDefaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    // 读进来的终态行渲染成终态（不亮假活体指示）
    expect(await screen.findByText("被打断的答复")).toBeInTheDocument();
    expect(screen.queryByText("正在思考")).not.toBeInTheDocument();
    // 再发一条 → 终稿行事件回显 → 移除该回复触发整段保存：读进来的
    // interrupted 终态必须原样带出（persistHistory 全字段回程）
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "再问一句");
    await user.keyboard("{Enter}");
    await act(async () => {
      for (const cb of mocks.listeners["bot-msg-upserted"] ?? []) {
        cb({
          payload: {
            sessionId: "s1",
            msgId: "m2",
            role: "assistant",
            content: "新的答复",
            turnStatus: "completed",
          },
        });
      }
    });
    expect(await screen.findByText("新的答复")).toBeInTheDocument();
    // 读回的终态行随事件回显的行一样是可删实体：按行删（全量重写已断代，
    // interrupted 的库级往返由后端锚点⑰钉住）
    const bubbleRow = screen
      .getByText("新的答复")
      .closest("div.chat-bubble")?.parentElement;
    expect(bubbleRow).not.toBeNull();
    await user.click(
      within(bubbleRow as HTMLElement).getByTitle(
        "移除这条回复（不满意时删除，不再作为上下文）",
      ),
    );
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_history_drop", {
        sessionId: "s1",
        msgId: "m2",
      });
    });
  });

  it("会话锁被占：拒答不当成模型答复——只显示本地提示，不落库", async () => {
    const user = userEvent.setup();
    // 后端回 busy=true：本轮压根没跑，text 是拒答文案不是模型答复
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_chat") {
        return {
          text: "这条会话正在处理上一条消息，请稍候再发。",
          taskRefs: [],
          busy: true,
        };
      }
      return chatDefaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_sessions_load");
    });
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "你好");
    await user.keyboard("{Enter}");
    // ① 本地提示可见（末条是提示，不是模型答复）
    await waitFor(() => {
      expect(screen.getByText(/模型正忙/)).toBeInTheDocument();
    });
    // ② 拒答文案不当成答复展示；占位气泡已摘除（活体指示不再亮）
    expect(
      screen.queryByText(/这条会话正在处理上一条消息/),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("正在思考")).not.toBeInTheDocument();
    // ③ 零落库（真源化）：用户行落库权归后端且 busy 判定在前——拒答路径
    // 连用户行都不写（后端锚点⑨），前端不再回合开始存快照，全程零 save
    const saveCalls = mocks.invokeMock.mock.calls.filter(
      (c) => c[0] === "bot_history_save",
    );
    expect(saveCalls.length).toBe(0);
  });

  it("回复中发送键变为红框停止键：点击调 bot_stop", async () => {
    const user = userEvent.setup();
    // bot_chat 挂起不返回 → busy 保持 true
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_chat") return new Promise(() => {});
      if (cmd === "bot_stop") return null;
      return null;
    });
    render(<ChatPanel {...defaultProps} />);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_sessions_load");
    });
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "你好");
    await user.keyboard("{Enter}");
    // busy=true：发送键消失，红描边圆形停止键出现
    const stopBtn = await screen.findByTitle("停止当前回复");
    expect(
      screen.queryByRole("button", { name: "发送" }),
    ).not.toBeInTheDocument();
    await user.click(stopBtn);
    // /stop 按会话停止——携带当前会话 id
    expect(mocks.invokeMock).toHaveBeenCalledWith("bot_stop", {
      sessionId: "s1",
    });
  });

  it("停止后回显置位条数：bot_stop 返回几条就显示「已停 N 条」", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_chat") return new Promise(() => {});
      if (cmd === "bot_stop") return 2;
      return null;
    });
    render(<ChatPanel {...defaultProps} />);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_sessions_load");
    });
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "你好");
    await user.keyboard("{Enter}");
    const stopBtn = await screen.findByTitle("停止当前回复");
    await user.click(stopBtn);
    expect(await screen.findByText(/已停 2 条/)).toBeInTheDocument();
  });

  it("一条都没停到时不报数（点了停止却什么都没发生，报「已停 0 条」更让人困惑）", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_chat") return new Promise(() => {});
      if (cmd === "bot_stop") return 0;
      return null;
    });
    render(<ChatPanel {...defaultProps} />);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_sessions_load");
    });
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "你好");
    await user.keyboard("{Enter}");
    const stopBtn = await screen.findByTitle("停止当前回复");
    await user.click(stopBtn);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_stop", {
        sessionId: "s1",
      });
    });
    expect(screen.queryByText(/已停/)).not.toBeInTheDocument();
  });

  it("推理强度：未覆盖按钮叫「默认」且发送带基线档位；会话覆盖只影响本会话且不回写（RE-1）", async () => {
    const user = userEvent.setup();
    // 上一个用例把 bot_chat 换成挂起 Promise，这里恢复默认实现（chatDefaultInvoke）
    mocks.invokeMock.mockImplementation(chatDefaultInvoke);
    render(<ChatPanel {...defaultProps} />);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_sessions_load");
    });
    // bot_get_config 未带 reasoningEffort → 基线 medium；未覆盖时按钮就叫「默认」
    expect(await screen.findByText("默认")).toBeInTheDocument();

    // 发送第一条：生效档位 medium 随 bot_chat 传出；等回复落盘（busy 清除）
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "你好");
    await user.keyboard("{Enter}");
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_chat",
        expect.objectContaining({ reasoningEffort: "medium" }),
      );
    });
    await act(async () => {
      for (const cb of mocks.listeners["bot-msg-upserted"] ?? []) {
        cb({
          payload: {
            sessionId: "s1",
            msgId: "m1",
            role: "assistant",
            content: "机器人回复",
            turnStatus: "completed",
          },
        });
      }
    });
    expect(await screen.findByText("机器人回复")).toBeInTheDocument();

    // 打开覆盖下拉选「高」→ 按钮改显所选档位
    await user.click(
      screen.getByTitle("推理强度：只影响当前会话的发送，不写入设置"),
    );
    await user.click(await screen.findByText("高"));
    expect(screen.getByText("高")).toBeInTheDocument();

    // 第二条发送：reasoningEffort 变为 high
    await user.type(
      screen.getByPlaceholderText(/和机器人说点什么/),
      "再来一条",
    );
    await user.keyboard("{Enter}");
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_chat",
        expect.objectContaining({ reasoningEffort: "high" }),
      );
    });
    await act(async () => {
      for (const cb of mocks.listeners["bot-msg-upserted"] ?? []) {
        cb({
          payload: {
            sessionId: "s1",
            msgId: "m1",
            role: "assistant",
            content: "机器人回复",
            turnStatus: "completed",
          },
        });
      }
    });


    // 覆盖不回写后台：全程不应出现 bot_set_config
    expect(mocks.invokeMock).not.toHaveBeenCalledWith(
      "bot_set_config",
      expect.anything(),
    );
  });

  it("输入窗口锁定对话：草稿按会话隔离，切走再切回各自保留、发送不串台（DRAFT-1）", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load")
        return [
          { id: "s1", title: "默认会话" },
          { id: "s2", title: "另一个" },
        ];
      return chatDefaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");

    // S1 打草稿 → 切到 S2：输入框清空（不带走 S1 的内容）
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "给一会的草稿");
    await user.click(railSession("另一个"));
    const input2 = screen.getByPlaceholderText(/和机器人说点什么/);
    expect(input2).toHaveValue("");

    // S2 打自己的草稿 → 切回 S1：草稿原样恢复
    await user.type(input2, "给二会的草稿");
    await user.click(railSession("默认会话"));
    const input3 = screen.getByPlaceholderText(/和机器人说点什么/);
    expect(input3).toHaveValue("给一会的草稿");

    // 直接回车发送：进的是 S1、内容是 S1 的草稿（A 的字不会误发给 B）
    await user.click(input3);
    await user.keyboard("{Enter}");
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_chat",
        expect.objectContaining({
          sessionId: "s1",
          content: "给一会的草稿",
          clientMsgId: expect.any(String),
        }),
      );
    });
  });

  // 设置页在 App 里是提前 return（整棵主树被替换掉），聊天面板随之卸载——
  // 草稿放组件 state 里的话，切一次设置输入框就空了（用户报的「输入不持久」）。
  // 这里用「卸载 → 重挂载」等效复现那条路径。
  it("草稿跨挂载存活：面板卸载再挂载（切设置的等效动作），输入框内容还在", async () => {
    const user = userEvent.setup();
    const { unmount } = render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");

    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "没发出去的半句话");
    expect(input).toHaveValue("没发出去的半句话");

    // 卸载 = 设置页接管主树；重挂载 = 点「返回」
    unmount();
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    expect(screen.getByPlaceholderText(/和机器人说点什么/)).toHaveValue(
      "没发出去的半句话",
    );
  });

  it("草稿落盘：写进 localStorage（换窗口 / 应用重启后回来还在）", async () => {
    const user = userEvent.setup();
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");

    await user.type(
      screen.getByPlaceholderText(/和机器人说点什么/),
      "留着下次接着写",
    );
    await waitFor(() => {
      expect(localStorage.getItem("wmessage-chat-drafts")).toContain(
        "留着下次接着写",
      );
    });
    // 发出去即清空：空草稿不落盘（不堆垃圾条目）
    await user.keyboard("{Enter}");
    await waitFor(() => {
      const raw = localStorage.getItem("wmessage-chat-drafts") ?? "";
      expect(raw).not.toContain("留着下次接着写");
    });
  });

  it("确认弹窗不再由聊天窗呈现：bot-confirm 只有 ConfirmMap 一个监听者", async () => {
    // 两个呈现者同窗挂载时同一请求会弹两次：ConfirmMap 答完，
    // 聊天窗那份 z-50 残影还在，点它只会静默失败
    const events: string[] = [];
    mocks.listenMock.mockImplementation(async (event: string) => {
      events.push(event);
      return () => {};
    });
    render(<ChatPanel {...defaultProps} />);
    expect(await screen.findAllByText("默认会话")).not.toHaveLength(0);
    await act(async () => {});
    expect(events).not.toContain("bot-confirm");
    expect(events).toContain("bot-chat-delta");
  });
});
