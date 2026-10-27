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
describe("ChatPanel", () => {
  // 缓存命中的回合 input_tokens 只剩增量甚至为 0，token 全在 cache_read 上：
  // 早前那个「input 与 output 都为 0 就跳过」会把整段缓存量丢在账外，
  // 水位条比 trace 弹窗（同口径分档）少算一大截
  it("只带缓存 token 的 usage 增量不被丢弃：水位条计入缓存两档", async () => {
    const listeners = new Map<string, (e: { payload: unknown }) => void>();
    const prevListenImpl = mocks.listenMock.getMockImplementation();
    mocks.listenMock.mockImplementation(
      async (event: string, cb: (e: { payload: unknown }) => void) => {
        listeners.set(event, cb);
        return () => {};
      },
    );
    try {
      render(<ChatPanel {...defaultProps} />);
      await screen.findAllByText("默认会话");
      await act(async () => {
        listeners.get("bot-usage-delta")?.({
          payload: {
            sessionId: "s1",
            inputTokens: 0,
            outputTokens: 0,
            cacheReadTokens: 8000,
            cacheCreationTokens: 500,
          },
        });
      });
      // Σ 含缓存两档（0 + 0 + 8000 + 500），缓存另立一档不并进输入输出
      expect(screen.getByText(/Σ 8\.5K/)).toBeInTheDocument();
      expect(screen.getByText(/缓存 8\.0K\/500/)).toBeInTheDocument();
    } finally {
      mocks.listenMock.mockImplementation(prevListenImpl!);
    }
  });

  it("四项全 0 的 usage 增量不产生水位条（无数据的回合不编数）", async () => {
    const listeners = new Map<string, (e: { payload: unknown }) => void>();
    const prevListenImpl = mocks.listenMock.getMockImplementation();
    mocks.listenMock.mockImplementation(
      async (event: string, cb: (e: { payload: unknown }) => void) => {
        listeners.set(event, cb);
        return () => {};
      },
    );
    try {
      render(<ChatPanel {...defaultProps} />);
      await screen.findAllByText("默认会话");
      await act(async () => {
        listeners.get("bot-usage-delta")?.({
          payload: {
            sessionId: "s1",
            inputTokens: 0,
            outputTokens: 0,
            cacheReadTokens: 0,
            cacheCreationTokens: 0,
          },
        });
      });
      expect(screen.queryByText(/Σ/)).toBeNull();
    } finally {
      mocks.listenMock.mockImplementation(prevListenImpl!);
    }
  });

  it("点回复里的文件路径：右侧栏就地预览，不再调起系统默认程序", async () => {
    const user = userEvent.setup();
    mocks.listenMock.mockImplementation(
      async (
        event: string,
        cb: (e: { payload: Record<string, unknown> }) => void,
      ) => {
        (mocks.listeners[event] ??= []).push(cb);
        return () => {};
      },
    );
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load")
        return [
          {
            role: "assistant",
            content: "调研稿已写入 `/Users/x/调研.md`",
            refsJson: null,
            thinking: null,
            toolsJson: null,
          },
        ];
      if (cmd === "bot_preview_text")
        return {
          kind: "text",
          path: "/Users/x/调研.md",
          content: "# 上海博士后政策\n正文一段",
          truncated: false,
          totalBytes: 20,
          lines: 2,
        };
      return chatDefaultInvoke(cmd);
    });
    try {
      render(<ChatPanel {...defaultProps} />);
      await screen.findAllByText("默认会话");
      await user.click(
        await screen.findByRole("link", { name: "/Users/x/调研.md" }),
      );
      // 预览出现在右侧栏「文件」工作面里
      const rail = within(
        await screen.findByRole("complementary", { name: "上下文侧栏" }),
      );
      expect(await rail.findByLabelText("文件内容")).toHaveTextContent(
        "上海博士后政策",
      );
      // 关键回归：不能顺带调起系统默认程序
      expect(mocks.invokeMock).not.toHaveBeenCalledWith(
        "open_file_path",
        expect.anything(),
      );
    } finally {
      mocks.invokeMock.mockImplementation(chatDefaultInvoke);
      mocks.listenMock.mockImplementation(async () => () => {});
    }
  });

  it("点文件 pill 同样进右侧栏预览（pill 与正文链接是同一份文件的两种入口）", async () => {
    const user = userEvent.setup();
    mocks.listenMock.mockImplementation(
      async (
        event: string,
        cb: (e: { payload: Record<string, unknown> }) => void,
      ) => {
        (mocks.listeners[event] ??= []).push(cb);
        return () => {};
      },
    );
    mocks.invokeMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_preview_text")
        return {
          kind: "text",
          path: "/Users/x/out.md",
          content: "# 产物",
          truncated: false,
          totalBytes: 5,
          lines: 1,
        };
      // 交付物存在性探测：路径在 → 挂上交付物 chip
      if (cmd === "bot_probe_paths")
        return ((args as { paths: string[] } | undefined)?.paths ?? []).map(
          (p) => ({ path: p, exists: true, isDir: false, size: 5 }),
        );
      if (cmd === "bot_chat") return { text: "写到 /Users/x/out.md 了", taskRefs: [] };
      return chatDefaultInvoke(cmd);
    });
    try {
      render(<ChatPanel {...defaultProps} />);
      await screen.findAllByText("默认会话");
      await user.type(
        screen.getByPlaceholderText(/和机器人说点什么/),
        "写个文档",
      );
      await user.keyboard("{Enter}");
      // 交付物 chip（正文点名的路径，过存在性探测后挂上）
      const pill = await screen.findByTitle(
        "打开交付物：/Users/x/out.md",
      );
      await user.click(pill);
      const rail = within(
        await screen.findByRole("complementary", { name: "上下文侧栏" }),
      );
      expect(await rail.findByLabelText("文件内容")).toHaveTextContent("产物");
      expect(mocks.invokeMock).not.toHaveBeenCalledWith(
        "open_file_path",
        expect.anything(),
      );
    } finally {
      mocks.invokeMock.mockImplementation(chatDefaultInvoke);
      mocks.listenMock.mockImplementation(async () => () => {});
    }
  });

  // 时间原先挤在行尾，还得靠 hover 让位给删除键：标题一长就被截成几个字，
  // 扫列表时既认不出是哪段对话、也看不到它是什么时候的。改成两行。
  it("会话条目两行：首行标题、次行相对时间（不因 hover 让位而消失）", async () => {
    const threeHoursAgo = Date.now() - 3 * 3600_000;
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load")
        return [
          { id: "s1", title: "政策调研", updatedAt: threeHoursAgo },
          // 没有 updatedAt 的行不该渲染出空的时间位
          { id: "s2", title: "写周报" },
        ];
      return chatDefaultInvoke(cmd);
    });
    try {
      render(<ChatPanel {...defaultProps} />);
      // 标题在侧栏与头部各渲染一次，按所在区域取侧栏那条
      const title = await screen.findAllByText("政策调研");
      const sideTitle = within(
        screen.getByRole("complementary", { name: "会话列表" }),
      ).getByText("政策调研");
      expect(title.length).toBeGreaterThan(0);
      const row = sideTitle.closest("button");
      expect(row).not.toBeNull();
      // 同一行内：首行标题、次行时间
      const time = within(row as HTMLElement).getByText("3 小时前");
      expect(time).toBeInTheDocument();
      // 次行字号更小
      expect(time.className).toContain("text-[10px]");
      // 不得再靠 hover 才出现——那正是挪到第二行要消掉的抖动
      expect(time.className).not.toContain("group-hover:hidden");
      // 无 updatedAt 的行只有标题
      const bare = rail().getByText("写周报").closest("button");
      expect(within(bare as HTMLElement).queryByText(/前|刚刚/)).toBeNull();
      // 删除键贴在第二行右端（与时间同行），不压首行标题尾部
      const del = rail().getByRole("button", { name: "删除对话 政策调研" });
      expect(del.className).toContain("bottom-1");
      expect(del.className).not.toContain("top-2");
      // 删除键的预留只落在第二行：整行都留白的话首行在没悬停时也白丢 28px
      const rowBtn = sideTitle.closest("button") as HTMLElement;
      expect(rowBtn.className).not.toContain("pr-7");
      const timeLine = time.closest("span")?.parentElement as HTMLElement;
      expect(timeLine.className).toContain("pr-7");
    } finally {
      mocks.invokeMock.mockImplementation(chatDefaultInvoke);
    }
  });

  it("无派发方时子 agent 收尾提示用侧栏徽标同一份措辞（succeeded = 完成，不是「已完成」）", async () => {
    const prevListenImpl = mocks.listenMock.getMockImplementation();
    mocks.listenMock.mockImplementation(
      async (
        event: string,
        cb: (e: { payload: Record<string, unknown> }) => void,
      ) => {
        (mocks.listeners[event] ??= []).push(cb);
        return () => {};
      },
    );
    try {
      render(<ChatPanel {...defaultProps} />);
      await screen.findAllByText("默认会话");
      await act(async () => {
        for (const cb of mocks.listeners["subagent-finished"] ?? []) {
          cb({
            payload: {
              subagentId: "sa_1",
              // 没有派发方主会话 → 没人会替模型转述，提示是唯一出口
              parentSessionId: null,
              status: "succeeded",
              summary: "周报已出",
            },
          });
        }
      });
      expect(await screen.findByText(/子任务完成：周报已出/)).toBeInTheDocument();
    } finally {
      mocks.listenMock.mockImplementation(prevListenImpl!);
    }
  });

  it("有派发方主会话时收尾不再重复提示（后端会在那一轮空闲时自己起汇报轮）", async () => {
    const prevListenImpl = mocks.listenMock.getMockImplementation();
    mocks.listenMock.mockImplementation(
      async (
        event: string,
        cb: (e: { payload: Record<string, unknown> }) => void,
      ) => {
        (mocks.listeners[event] ??= []).push(cb);
        return () => {};
      },
    );
    try {
      render(<ChatPanel {...defaultProps} />);
      await screen.findAllByText("默认会话");
      await act(async () => {
        for (const cb of mocks.listeners["subagent-finished"] ?? []) {
          cb({
            payload: {
              subagentId: "sa_1",
              parentSessionId: "s1",
              status: "failed",
              summary: "周报没跑出来",
            },
          });
        }
      });
      await act(async () => {});
      expect(screen.queryByText(/子任务失败/)).toBeNull();
    } finally {
      mocks.listenMock.mockImplementation(prevListenImpl!);
    }
  });

  it("汇报轮：没有流式气泡时 delta 自建一个，别把整轮回复悄悄丢掉", async () => {
    const prevListenImpl = mocks.listenMock.getMockImplementation();
    mocks.listenMock.mockImplementation(
      async (
        event: string,
        cb: (e: { payload: Record<string, unknown> }) => void,
      ) => {
        (mocks.listeners[event] ??= []).push(cb);
        return () => {};
      },
    );
    const prevInvokeImpl = mocks.invokeMock.getMockImplementation();
    // 会话末尾停在一条用户消息——后端主动起的一轮就落在这个位置
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_history_load")
        return [{ role: "user", content: "查一下上海博士后政策", refsJson: null }];
      return chatDefaultInvoke(cmd);
    });
    try {
      render(<ChatPanel {...defaultProps} />);
      await screen.findAllByText("默认会话");
      expect(await screen.findByText("查一下上海博士后政策")).toBeInTheDocument();
      await act(async () => {
        for (const cb of mocks.listeners["bot-chat-delta"] ?? []) {
          cb({ payload: { sessionId: "s1", text: "子任务跑完了" } });
        }
      });
      expect(await screen.findByText("子任务跑完了")).toBeInTheDocument();
      // 用户原话还在：增量是另起气泡，不是被塞进用户那条
      expect(screen.getByText("查一下上海博士后政策")).toBeInTheDocument();
    } finally {
      mocks.listenMock.mockImplementation(prevListenImpl!);
      mocks.invokeMock.mockImplementation(prevInvokeImpl!);
    }
  });

  it("汇报轮收尾：bot-chat-report 把流式气泡定稿成最终全文", async () => {
    const prevListenImpl = mocks.listenMock.getMockImplementation();
    mocks.listenMock.mockImplementation(
      async (
        event: string,
        cb: (e: { payload: Record<string, unknown> }) => void,
      ) => {
        (mocks.listeners[event] ??= []).push(cb);
        return () => {};
      },
    );
    try {
      render(<ChatPanel {...defaultProps} />);
      await screen.findAllByText("默认会话");
      await act(async () => {
        for (const cb of mocks.listeners["bot-chat-delta"] ?? []) {
          cb({ payload: { sessionId: "s1", text: "子任务跑完了" } });
        }
      });
      await screen.findByText("子任务跑完了");
      await act(async () => {
        for (const cb of mocks.listeners["bot-chat-report"] ?? []) {
          cb({
            payload: { sessionId: "s1", text: "子任务跑完了，结论如下：……" },
          });
        }
      });
      // 正文按后端给的全文校正，不留半截流式内容，也不重复追加一条
      expect(screen.getByText("子任务跑完了，结论如下：……")).toBeInTheDocument();
      expect(screen.queryByText("子任务跑完了")).toBeNull();
    } finally {
      mocks.listenMock.mockImplementation(prevListenImpl!);
    }
  });
  it("删除会话后清掉该会话的水位累计（sessionUsage 条目随会话删除）", async () => {
    const user = userEvent.setup();
    const listeners = new Map<string, (e: { payload: unknown }) => void>();
    mocks.listenMock.mockImplementation(
      async (event: string, cb: (e: { payload: unknown }) => void) => {
        listeners.set(event, cb);
        return () => {};
      },
    );
    let releaseChat!: (v: { text: string; taskRefs: [] }) => void;
    const chatGate = new Promise<{ text: string; taskRefs: [] }>((r) => {
      releaseChat = r;
    });
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load")
        return [
          { id: "s1", title: "默认会话" },
          { id: "s2", title: "另一个" },
        ];
      if (cmd === "bot_chat") return chatGate;
      // 新建会话返回与被删会话相同的 id：模拟观测面上 id 复用，
      // 让「水位条目未清理」变成可见回归（新会话继承旧水位）
      if (cmd === "bot_session_create") return { id: "s2", title: "复生会话" };
      return chatDefaultInvoke(cmd);
    });
    try {
      render(<ChatPanel {...defaultProps} />);
      await screen.findAllByText("默认会话");
      // 切到 s2 发消息（挂起 → inflight），推一轮 usage → 水位条出现
      await user.click(railSession("另一个"));
      const input = screen.getByPlaceholderText(/和机器人说点什么/);
      await user.type(input, "你好");
      await user.keyboard("{Enter}");
      await act(async () => {
        listeners.get("bot-usage-delta")?.({
          payload: { sessionId: "s2", inputTokens: 100, outputTokens: 50 },
        });
      });
      expect(screen.getByText(/Σ 150/)).toBeInTheDocument();
      // 回复收尾（解除 inflight），切回 s1 后删除 s2
      await act(async () => {
        releaseChat({ text: "回复A", taskRefs: [] });
      });
      await screen.findByText("回复A");
      await user.click(railSession("默认会话"));
      const s2Row = railSession("另一个").closest("div");
      await user.hover(s2Row!);
      await user.click(within(s2Row!).getByTitle("删除此对话"));
      await waitFor(() => {
        expect(
          mocks.invokeMock.mock.calls.some(
            (c) => c[0] === "bot_session_delete",
          ),
        ).toBe(true);
      });
      // 侧栏常驻：新建对话（id 复用 s2）→ 陈旧水位不得泄漏进新会话
      await user.click(screen.getByText("新建对话"));
      await screen.findAllByText("复生会话");
      expect(screen.queryByText(/Σ 150/)).toBeNull();
    } finally {
      mocks.listenMock.mockImplementation(async () => () => {});
    }
  });

  it("历史行 refsJson 损坏不抛错：该字段按缺失处理，其余消息照常渲染", async () => {
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load")
        return [
          {
            role: "user",
            content: "损坏引用的问题",
            refsJson: "{oops-not-json",
            thinking: null,
            toolsJson: null,
          },
          {
            role: "assistant",
            content: "引用也损坏的回答",
            refsJson: "[{truncated",
            thinking: null,
            toolsJson: null,
          },
          {
            role: "assistant",
            content: "正常回答",
            refsJson: '[{"id":"t1","title":"引用任务"}]',
            thinking: null,
            toolsJson: null,
          },
        ];
      return chatDefaultInvoke(cmd);
    });
    try {
      render(<ChatPanel {...defaultProps} />);
      // 损坏行不炸整个会话加载：三条消息都渲染出来
      expect(await screen.findByText("损坏引用的问题")).toBeInTheDocument();
      expect(screen.getByText("引用也损坏的回答")).toBeInTheDocument();
      expect(screen.getByText("正常回答")).toBeInTheDocument();
      // 损坏字段按缺失处理（不出引用按钮）；同批合法 refsJson 行为不变（照常出引用）
      expect(screen.queryByTitle("打开任务：引用也损坏的回答")).toBeNull();
      expect(
        screen.getByTitle("打开任务：引用任务"),
      ).toBeInTheDocument();
      // 留痕：解析失败打了带字段标识的错误日志
      expect(errorSpy).toHaveBeenCalledWith(
        expect.stringContaining("refsJson 解析失败"),
        expect.anything(),
      );
    } finally {
      errorSpy.mockRestore();
      mocks.invokeMock.mockImplementation(chatDefaultInvoke);
    }
  });

  it("点选模型调用 bot_set_active_model（窄口径）并广播同步", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(chatDefaultInvoke);
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    const chip = screen.getByTitle("切换模型");
    await waitFor(() => {
      expect(chip).toHaveTextContent("MiniMax-M3");
    });
    await user.click(chip);
    await user.click(await screen.findByText("DeepSeek-V3"));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_set_active_model", {
        modelId: "m2",
      });
    });
    // 标签随返回 view 刷新（active 模型的 model 字段）
    expect(await screen.findByText(/deepseek-chat/)).toBeInTheDocument();
    // 配置变更广播（各页面监听重拉）
    expect(mocks.emitMock).toHaveBeenCalledWith("bot-config-changed", null);
  });

  it("模型下拉双协议同列：两组分组展示，跨协议选中连协议一起切", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_config")
        return {
          baseUrl: "https://a.example/v1",
          model: "glm-5.3-flash",
          apiProvider: "openai",
          modelsByProvider: {
            openai: [
              {
                id: "m1",
                label: "GLM-5.3-Flash",
                baseUrl: "https://a.example/v1",
                model: "glm-5.3-flash",
              },
            ],
            anthropic: [
              {
                id: "m2",
                label: "Claude Sonnet",
                baseUrl: "https://b.example",
                model: "claude-sonnet",
              },
            ],
          },
          activeModelId: { openai: "m1", anthropic: "m2" },
        };
      if (cmd === "bot_set_active_model")
        return {
          model: "claude-sonnet",
          apiProvider: "anthropic",
          activeModelId: { openai: "m1", anthropic: "m2" },
        };
      return chatDefaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    await user.click(screen.getByTitle("切换模型"));
    // 两组都在：分组头 + 各自模型（openai 是当前协议 → GLM 高亮）
    expect(await screen.findByText("OpenAI 兼容")).toBeInTheDocument();
    expect(screen.getByText("Anthropic 兼容")).toBeInTheDocument();
    const glm = screen.getByText("GLM-5.3-Flash");
    expect(glm.className).toContain("nm-inset");
    // 点另一协议的模型 → 窄口径命令原样传 id；后端返回切好的协议 → 🧠 标签刷新
    await user.click(screen.getByText("Claude Sonnet"));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_set_active_model", {
        modelId: "m2",
      });
    });
    expect(await screen.findByText(/claude-sonnet/)).toBeInTheDocument();
  });

  // ── PAR-1：并行回复（每会话独立在途，互不串台）──

  it("S1 回复中切到 S2 发消息——两会话并行回复、各自落库", async () => {
    const user = userEvent.setup();
    let release1!: (v: { text: string; taskRefs: [] }) => void;
    let release2!: (v: { text: string; taskRefs: [] }) => void;
    const gate1 = new Promise<{ text: string; taskRefs: [] }>((r) => {
      release1 = r;
    });
    const gate2 = new Promise<{ text: string; taskRefs: [] }>((r) => {
      release2 = r;
    });
    const chatCalls: Array<Record<string, unknown> | undefined> = [];
    const saves: Array<Record<string, unknown> | undefined> = [];
    const prevInvokeImpl = mocks.invokeMock.getMockImplementation();
    mocks.invokeMock.mockImplementation(
      async (cmd: string, args?: Record<string, unknown>) => {
        switch (cmd) {
          case "bot_sessions_load":
            return [
              { id: "s1", title: "默认会话" },
              { id: "s2", title: "另一个" },
            ];
          case "bot_chat":
            chatCalls.push(args);
            return chatCalls.length === 1 ? gate1 : gate2;
          case "bot_history_load":
            return [];
          case "bot_history_save":
            saves.push(args);
            return null;
          default:
            return chatDefaultInvoke(cmd);
        }
      },
    );
    try {
      render(<ChatPanel {...defaultProps} />);
      await screen.findAllByText("默认会话");
      // S1 发送 → 挂起（在途）
      const input = screen.getByPlaceholderText(/和机器人说点什么/);
      await user.type(input, "问题一");
      await user.keyboard("{Enter}");
      await waitFor(() => {
        expect(chatCalls.length).toBe(1);
        expect(chatCalls[0]?.sessionId).toBe("s1");
      });
      // 切到 S2 → 发送 → 第二个 bot_chat（并行，不拦）
      await user.click(railSession("另一个"));
      const input2 = screen.getByPlaceholderText(/和机器人说点什么/);
      await user.type(input2, "问题二");
      await user.keyboard("{Enter}");
      await waitFor(() => {
        expect(chatCalls.length).toBe(2);
        expect(chatCalls[1]?.sessionId).toBe("s2");
      });
      // 各自落库不串台：S1 的回复不进 S2 的历史
      await act(async () => {
        release1({ text: "答一", taskRefs: [] });
      });
      await waitFor(() => {
        expect(saves.some((sv) => sv?.sessionId === "s1")).toBe(true);
      });
      await act(async () => {
        release2({ text: "答二", taskRefs: [] });
      });
      await waitFor(() => {
        expect(saves.some((sv) => sv?.sessionId === "s2")).toBe(true);
      });
      // 取 S2 最后一次落库（回合收尾那次）：回合开始还会先写一次用户消息
      const s2Save = saves.filter((sv) => sv?.sessionId === "s2").pop() as {
        messages: { role: string; content: string }[];
      };
      expect(s2Save.messages.some((m) => m.content.includes("问题二"))).toBe(
        true,
      );
      expect(s2Save.messages.some((m) => m.content.includes("答二"))).toBe(
        true,
      );
      expect(s2Save.messages.some((m) => m.content.includes("答一"))).toBe(
        false,
      );
    } finally {
      mocks.invokeMock.mockImplementation(prevInvokeImpl!);
    }
  });

  // ── sessionIdRef 镜像同步落：挂载恢复 / 删除末会话重建两条 await 窗口 ──

  it("挂载恢复历史期间已切走：过期历史不得盖掉新会话视图（镜像同步落 + 守卫）", async () => {
    mocks.listenMock.mockImplementation(
      async (
        event: string,
        cb: (e: { payload: Record<string, unknown> }) => void,
      ) => {
        (mocks.listeners[event] ??= []).push(cb);
        return () => {};
      },
    );
    let releaseHistory: (() => void) | null = null;
    mocks.invokeMock.mockImplementation(
      async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "bot_sessions_load")
          return [{ id: "s1", title: "默认会话" }];
        if (cmd === "bot_history_load") {
          if (args?.sessionId === "s2") return [];
          return new Promise((res) => {
            releaseHistory = () =>
              res([
                {
                  role: "user",
                  content: "s1 的迟到历史",
                  refsJson: null,
                  thinking: null,
                  toolsJson: null,
                },
              ]);
          });
        }
        return chatDefaultInvoke(cmd);
      },
    );
    try {
      render(<ChatPanel {...defaultProps} />);
      // 挂载流程停在 history_load 在途（镜像此刻必须已同步落，见 ChatPanel 挂载分支）
      await vi.waitFor(() => expect(releaseHistory).not.toBeNull());
      // 历史在途时跨窗口跳会话（⌘K）：switchSession 同步镜像 ref 到 s2
      await act(async () => {
        for (const cb of mocks.listeners["chat-focus-session"] ?? []) {
          cb({ payload: { sessionId: "s2" } });
        }
      });
      await act(async () => {
        releaseHistory?.();
      });
      // s1 的迟到历史被守卫丢弃，不得盖掉 s2 的视图
      expect(screen.queryByText("s1 的迟到历史")).not.toBeInTheDocument();
    } finally {
      mocks.invokeMock.mockImplementation(chatDefaultInvoke);
      mocks.listenMock.mockImplementation(async () => () => {});
    }
  });

  it("删除末会话触发的重建：创建期间已切走则不抢占视图，镜像落在新会话", async () => {
    const user = userEvent.setup();
    mocks.listenMock.mockImplementation(
      async (
        event: string,
        cb: (e: { payload: Record<string, unknown> }) => void,
      ) => {
        (mocks.listeners[event] ??= []).push(cb);
        return () => {};
      },
    );
    let releaseCreate: ((s: { id: string; title: string }) => void) | null =
      null;
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_session_create") {
        return new Promise((res) => {
          releaseCreate = (s) => res(s);
        });
      }
      return chatDefaultInvoke(cmd);
    });
    try {
      render(<ChatPanel {...defaultProps} />);
      await screen.findAllByText("默认会话");
      mocks.askMock.mockResolvedValueOnce(true);
      const lastRow = railSession("默认会话").closest("div");
      await user.hover(lastRow!);
      await user.click(within(lastRow!).getByTitle("删除此对话"));
      await waitFor(() =>
        expect(mocks.invokeMock).toHaveBeenCalledWith("bot_session_create", {
          title: null,
        }),
      );
      // 创建在途：跨窗口跳到 s2
      await act(async () => {
        for (const cb of mocks.listeners["chat-focus-session"] ?? []) {
          cb({ payload: { sessionId: "s2" } });
        }
      });
      await act(async () => {
        releaseCreate?.({ id: "s4", title: "重建会话" });
      });
      // 守卫生效：重建出来的 s3 不进会话列表（视图已被 s2 接管）
      await waitFor(() =>
        expect(mocks.invokeMock).toHaveBeenCalledWith("bot_history_load", {
          sessionId: "s2",
        }),
      );
      expect(screen.queryByText("重建会话")).not.toBeInTheDocument();
    } finally {
      mocks.invokeMock.mockImplementation(chatDefaultInvoke);
      mocks.listenMock.mockImplementation(async () => () => {});
    }
  });
});
