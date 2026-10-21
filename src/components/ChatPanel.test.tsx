import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, act, within, fireEvent } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { ChatPanel } from "./ChatPanel";
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
});

describe("ChatPanel", () => {
  it("初始渲染：加载默认会话后显示空消息提示", async () => {
    render(<ChatPanel {...defaultProps} />);
    // 初始 useEffect 加载会话
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_sessions_load");
    });
    expect(
      await screen.findByText("和机器人说点什么", { exact: false }),
    ).toBeInTheDocument();
    // 会话标题在侧栏与头部各渲染一处
    expect(railSession("默认会话")).toBeInTheDocument();
    expect(currentTitle()).toHaveTextContent("默认会话");
  });

  it("空状态起始短语：点一下填进输入框，不自动发送", async () => {
    const user = userEvent.setup();
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.click(screen.getByRole("button", { name: "列出我当前的任务" }));
    expect(input).toHaveValue("列出我当前的任务");
    // 措辞留用户过目：填入不等于发送
    expect(
      mocks.invokeMock.mock.calls.some((c) => c[0] === "bot_chat"),
    ).toBe(false);
  });

  it("聊天内嵌子 agent 区域：按主会话拉子 agent，点条目展开右侧栏到子 Agent 类别", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load")
        return [{ id: "s1", title: "默认会话" }];
      // 子 agent 区域挂在**最后一条助手回复**里（工具调用下方），所以会话得
      // 有一条助手消息当宿主，否则条带无处可挂
      if (cmd === "bot_history_load")
        return [{ role: "assistant", content: "已派发", refsJson: null }];
      if (cmd === "list_subagents")
        return [
          { id: "sa_1", status: "running", objective: "整理周报" },
          { id: "sa_2", status: "succeeded", objective: "改文档" },
        ];
      return defaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    // 按当前会话拉取：子 agent 靠 parent_session_id 归属
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("list_subagents", {
        sessionId: "s1",
        limit: 20,
      });
    });
    // 折叠态只显示在途的
    expect(screen.getByText("子 Agent")).toBeInTheDocument();
    expect(screen.getByText("1 个执行中")).toBeInTheDocument();
    // 展开后能看到已结束的
    await user.click(screen.getByRole("button", { name: /子 Agent/ }));
    expect(screen.getByText("改文档")).toBeInTheDocument();
    // 点条目 → 侧栏展开且切到子 Agent 类别（不切视图，对话留在原位）
    // 行内还有「取消」钮，其 aria-label 带同一标题，用 ^ 锚到详情钮本身
    await user.click(screen.getByRole("button", { name: /^整理周报/ }));
    const rail = within(
      screen.getByRole("complementary", { name: "上下文侧栏" }),
    );
    expect(rail.getByRole("button", { name: /子 Agent/ })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByRole("button", { name: "收起右侧栏" })).toBeVisible();
  });

  it("点子 agent 条目把 id 带进详情页（只切类别不换视图等于点了没反应）", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load")
        return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load")
        return [{ role: "assistant", content: "已派发", refsJson: null }];
      if (cmd === "list_subagents")
        return [
          {
            id: "sa_1",
            status: "succeeded",
            objective: "整理周报",
            profile: "research",
            resultJson: JSON.stringify({ summary: "已汇总 3 条任务" }),
            acceptanceJson: JSON.stringify(["给出结论"]),
            traceId: "trc_x1",
            startedAt: 1_700_000_000_000,
            finishedAt: 1_700_000_148_001,
          },
          {
            id: "sa_2",
            status: "succeeded",
            objective: "另一个子 agent",
          },
        ];
      return defaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    // strip 要等 list_subagents 那次 invoke 回来才进 DOM，跟会话列表不是同一拍
    await user.click(await screen.findByRole("button", { name: /子 Agent/ }));
    await user.click(screen.getByRole("button", { name: /^整理周报/ }));

    const rail = within(
      screen.getByRole("complementary", { name: "上下文侧栏" }),
    );
    // 详情页真的渲染了，且渲染的是**被点的那个**（另一个条目不该混进来）
    expect(rail.getByLabelText("子 Agent 详情")).toBeInTheDocument();
    expect(rail.getByText("已汇总 3 条任务")).toBeInTheDocument();
    expect(rail.getByText(/1\. 给出结论/)).toBeInTheDocument();
    expect(rail.queryByText("另一个子 agent")).toBeNull();
    // 终态用时定死（148.001s）
    expect(rail.getByText("2m 28s")).toBeInTheDocument();
    // 能退回列表
    await user.click(rail.getByLabelText("返回子 agent 列表"));
    expect(screen.queryByLabelText("子 Agent 详情")).toBeNull();
    expect(rail.getByText("另一个子 agent")).toBeInTheDocument();
  });

  it("subagent-finished 到达就重拉子 agent 行（行只有这块可见，不重拉永远停在 running）", async () => {
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
    let finished = false;
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load")
        return [{ role: "assistant", content: "已派发", refsJson: null }];
      if (cmd === "list_subagents")
        return finished
          ? [{ id: "sa_1", status: "succeeded", objective: "整理周报" }]
          : [{ id: "sa_1", status: "running", objective: "整理周报" }];
      return defaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    // 子 agent 行是异步拉回来的：先等条带出现再展开（全量跑比单文件慢，
    // getByRole 会抢在 list_subagents 返回前）
    await user.click(await screen.findByRole("button", { name: /子 Agent/ }));
    expect(screen.getByText("1 个执行中")).toBeInTheDocument();
    finished = true;
    await act(async () => {
      for (const cb of mocks.listeners["subagent-finished"] ?? []) {
        cb({
          payload: {
            subagentId: "sa_1",
            parentSessionId: "s1",
            status: "succeeded",
            summary: "周报已出",
          },
        });
      }
    });
    // 计数从「1 个执行中」变成「1 个」= 列表真重拉了
    await waitFor(() =>
      expect(screen.queryByText("1 个执行中")).not.toBeInTheDocument(),
    );
  });

  it("subagent-spawned 到达就重拉：新派发的行收尾前就可见可取消", async () => {
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
    let spawned = false;
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "list_subagents")
        return spawned
          ? [{ id: "sa_2", status: "queued", objective: "整理周报" }]
          : [];
      return defaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    // 空列表时条带不渲染，从右侧栏进「子 Agent」工作面
    await user.click(screen.getByRole("button", { name: "展开右侧栏" }));
    await user.click(await screen.findByRole("button", { name: /子 Agent/ }));
    // 派发前：列表为空（侧栏显示 0 个执行中），没有这行的任何痕迹
    expect(screen.queryByText("整理周报")).not.toBeInTheDocument();
    spawned = true;
    await act(async () => {
      for (const cb of mocks.listeners["subagent-spawned"] ?? []) {
        cb({ payload: { subagentId: "sa_2" } });
      }
    });
    // queued 也算执行中：行到达即显示并带取消入口（条带与侧栏各一份计数/按钮）
    expect((await screen.findAllByText("1 个执行中")).length).toBeGreaterThan(0);
    expect(
      (await screen.findAllByRole("button", { name: /取消子 agent：整理周报/ }))
        .length,
    ).toBeGreaterThan(0);
  });

  it("在途子 agent 有取消入口：点它调 cancel_subagent 且 key 传子 agent id", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load")
        return [{ role: "assistant", content: "已派发", refsJson: null }];
      if (cmd === "list_subagents")
        return [{ id: "sa_9", status: "running", objective: "整理周报" }];
      return defaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    await user.click(await screen.findByRole("button", { name: /子 Agent/ }));
    await user.click(
      await screen.findByRole("button", { name: /取消子 agent：整理周报/ }),
    );
    expect(mocks.invokeMock).toHaveBeenCalledWith("cancel_subagent", {
      key: "sa_9",
    });
  });

  it("活体指示在助手气泡顶部，不在聊天头部（回合状态属于某一条回复）", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      // 回合不收尾（bot_chat 悬着）：要看到的是**流式中**那条气泡的指示
      if (cmd === "bot_chat") return new Promise(() => {});
      return defaultInvoke(cmd);
    });
    mocks.listenMock.mockImplementation(async (event: string, cb: unknown) => {
      if (event === "bot-chat-delta" || event === "bot-turn-status") {
        ((mocks.listeners[event] ??= []) as Array<typeof cb>).push(
          cb as (e: { payload: Record<string, unknown> }) => void,
        );
      }
      return () => {};
    });
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    await user.type(screen.getByPlaceholderText(/和机器人说点什么/), "跑一下");
    await user.keyboard("{Enter}");
    await act(async () => {
      for (const cb of mocks.listeners["bot-chat-delta"] ?? []) {
        cb({ payload: { sessionId: "s1", text: "开工了" } });
      }
      for (const cb of mocks.listeners["bot-turn-status"] ?? []) {
        cb({
          payload: {
            sessionId: "s1",
            status: "running",
            detail: "正在思考 · 第 7 轮",
          },
        });
      }
    });
    expect(await screen.findByText("开工了")).toBeInTheDocument();
    const indicator = await screen.findByText("正在思考 · 第 7 轮");
    // 挂在气泡里：回合状态是「这条回复」的属性，头部那一条讲的是整个会话
    expect(indicator.closest(".chat-bubble-bot")).not.toBeNull();
    // 头部不再有它（塞在 header 会让「在读哪段对话」和「在跑第几轮」互相干扰）
    expect(document.querySelector("header")?.textContent ?? "").not.toContain(
      "正在思考",
    );
  });

  it("子 agent 区域在最后一条助手回复里、工具调用下方（不钉在对话顶部）", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load")
        return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load")
        return [
          { role: "user", content: "整理周报", refsJson: null },
          {
            role: "assistant",
            content: "已派发",
            refsJson: null,
            toolsJson: JSON.stringify([
              { id: "t1", name: "spawn_subagent", done: true },
            ]),
          },
        ];
      if (cmd === "list_subagents")
        return [{ id: "sa_1", status: "running", objective: "整理周报" }];
      return defaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    const strip = await screen.findByRole("button", { name: /子 Agent/ });
    // 落在助手气泡里，且在可滚动的消息列表内（原先是钉在列表上方的浮层，
    // 不随内容滚动、也不属于任何一轮）
    const bubble = strip.closest(".chat-bubble-bot");
    expect(bubble).not.toBeNull();
    expect(strip.closest(".overflow-y-auto")).not.toBeNull();

    // 位置在工具调用块下方、正文上方：跟着派发它的那一轮走
    const stripBlock = strip.closest("div") as HTMLElement;
    const toolBlock = screen
      .getByText(/调用工具 1\/1/)
      .closest(".chat-bubble-bot > div") as HTMLElement;
    const siblings = [...(bubble as HTMLElement).children];
    expect(siblings.indexOf(stripBlock)).toBeGreaterThan(
      siblings.indexOf(toolBlock),
    );
  });

  it("右侧栏是聊天主体的兄弟节点，不在 header 内（百分比宽度才参照聊天区）", async () => {
    const user = userEvent.setup();
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    await user.click(screen.getByRole("button", { name: "展开右侧栏" }));
    const rail = screen.getByRole("complementary", { name: "上下文侧栏" });
    // 塞进 header（横向 flex）时，侧栏会变成 header 的 flex item、把整行拉垮
    expect(rail.closest("header")).toBeNull();
    // 与会话栏、聊天主体同属根容器那层
    expect(rail.parentElement?.tagName).toBe("DIV");
    const sibling = rail.parentElement as HTMLElement;
    expect(sibling.querySelector("header")).not.toBeNull();
  });

  it("右侧栏点任务是「引用进消息」而非「立刻执行」", async () => {
    const user = userEvent.setup();
    const onUpdateTask = vi.fn();
    render(
      <ChatPanel
        {...defaultProps}
        tasks={[
          { id: "t1", title: "写周报", column: "todo" },
          { id: "t2", title: "改文档", column: "todo" },
        ]}
        onUpdateTask={onUpdateTask}
      />,
    );
    await screen.findAllByText("默认会话");
    // 默认收起：侧栏不在 DOM 里，入口在头部
    expect(
      screen.queryByRole("complementary", { name: "上下文侧栏" }),
    ).toBeNull();
    await user.click(screen.getByRole("button", { name: "展开右侧栏" }));
    const rail = within(
      screen.getByRole("complementary", { name: "上下文侧栏" }),
    );
    expect(rail.getByText("写周报")).toBeInTheDocument();
    // 折叠钮只有头部这一个，侧栏内不放第二个
    expect(screen.getAllByRole("button", { name: "收起右侧栏" })).toHaveLength(
      1,
    );
    // 勾完成 → 走宿主回调（App 侧接 updateTask），组件自己不碰后端
    await user.click(rail.getByRole("button", { name: "完成 写周报" }));
    expect(onUpdateTask).toHaveBeenCalledWith("t1", { column: "done" });
    // 点标题 = 引用进消息：芯片落在输入区，且不发 bot_chat（不立刻执行）
    await user.click(rail.getByRole("button", { name: "引用任务：改文档" }));
    expect(screen.getByTitle("改文档")).toBeInTheDocument();
    expect(
      mocks.invokeMock.mock.calls.some((c) => c[0] === "bot_execute_task"),
    ).toBe(false);
    // 发送时带出引用块与 refs，正文含任务标题
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "这两张卡先做哪张");
    await user.keyboard("{Enter}");
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_chat",
        expect.objectContaining({
          messages: expect.arrayContaining([
            expect.objectContaining({
              role: "user",
              content: expect.stringContaining("改文档"),
            }),
          ]),
        }),
      );
    });
    // 引用块必须是 intent_router 能解析的形态：标记 + 每行带 id。
    // 少标记或少 id 时批量执行路由静默不触发，模型只能按标题模糊匹配找卡。
    const chatCalls = mocks.invokeMock.mock.calls.filter((c) => c[0] === "bot_chat");
    const sentMsgs =
      chatCalls[chatCalls.length - 1]?.[1]?.messages ?? [];
    const sentContent = [...sentMsgs]
      .reverse()
      .find((m: { role: string }) => m.role === "user")?.content;
    expect(sentContent).toContain("[已选任务]");
    expect(sentContent).toContain("- id=t2，标题=改文档");
    // 位置也是协议的一部分：块在消息头、用户正文在块之后。
    // 解析器只认消息头的块，触发词只从块之后的正文里找——顺序反了两边都落空。
    expect(sentContent.indexOf("[已选任务]")).toBe(0);
    expect(sentContent.indexOf("这两张卡先做哪张")).toBeGreaterThan(
      sentContent.indexOf("- id=t2，标题=改文档"),
    );
    // 发送后芯片清空（与附件同口径）
    expect(screen.queryByTitle("改文档")).toBeNull();
  });

  it("会话侧栏可折叠：收起后侧栏消失，入口仍在头部，再点展开", async () => {
    const user = userEvent.setup();
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    expect(railSession("默认会话")).toBeInTheDocument();
    // 收起：侧栏整体卸载
    await user.click(screen.getByRole("button", { name: "收起会话栏" }));
    expect(
      screen.queryByRole("complementary", { name: "会话列表" }),
    ).toBeNull();
    // 头部标题不受折叠影响——它是「当前在哪段对话」的唯一指示
    expect(currentTitle()).toHaveTextContent("默认会话");
    // 入口仍在，展开后侧栏回来
    const expandBtn = screen.getByRole("button", { name: "展开会话栏" });
    expect(expandBtn).toHaveAttribute("aria-expanded", "false");
    await user.click(expandBtn);
    expect(railSession("默认会话")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "收起会话栏" })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    // 折叠钮只此一个：侧栏内不再放第二个，否则同一动作有两处入口
    expect(screen.getAllByRole("button", { name: "收起会话栏" })).toHaveLength(1);
  });

  it("会话侧栏常驻：切换会话不用先展开菜单，当前会话有 aria-current 标记", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load")
        return [
          { id: "s1", title: "默认会话" },
          { id: "s2", title: "另一个" },
        ];
      return defaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    // 当前会话在侧栏里被标记出来，无需任何展开动作
    expect(railSession("默认会话").closest("button")).toHaveAttribute(
      "aria-current",
      "page",
    );
    expect(railSession("另一个").closest("button")).not.toHaveAttribute(
      "aria-current",
    );
    // 一步直点即切换
    await user.click(railSession("另一个"));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_history_load", {
        sessionId: "s2",
      });
    });
    await waitFor(() => {
      expect(railSession("另一个").closest("button")).toHaveAttribute(
        "aria-current",
        "page",
      );
    });
    expect(currentTitle()).toHaveTextContent("另一个");
  });

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
    // 触发 bot_chat RPC
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_chat",
        expect.objectContaining({
          messages: expect.arrayContaining([
            expect.objectContaining({ role: "user", content: "你好" }),
          ]),
        }),
      );
    });
    // 流式完成后机器人回复渲染
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
          messages: expect.arrayContaining([
            expect.objectContaining({ role: "user", content: "第二个问题" }),
          ]),
        }),
      );
    });
    const saveCalls = mocks.invokeMock.mock.calls.filter(
      (c) => c[0] === "bot_history_save",
    );
    const saveCall = saveCalls[saveCalls.length - 1];
    expect(JSON.stringify(saveCall?.[1])).not.toContain("当前对话没有进行中的回复");
    const chatCalls = mocks.invokeMock.mock.calls.filter(
      (c) => c[0] === "bot_chat",
    );
    const chatCall = chatCalls[chatCalls.length - 1];
    expect(JSON.stringify(chatCall?.[1])).not.toContain("当前对话没有进行中的回复");
  });

  it("移除一条回复时的整段落库也不带本地提示（persistHistory 收口）", async () => {
    const user = userEvent.setup();
    render(<ChatPanel {...defaultProps} />);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_sessions_load");
    });
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    // 先跑完一轮，让列表里有一条可移除的助手回复
    await user.type(input, "你好");
    await user.keyboard("{Enter}");
    expect(await screen.findByText("机器人回复")).toBeInTheDocument();
    // 再造一条本地提示（/stop 且本会话无在途回复）
    await user.type(input, "/stop ");
    await user.click(screen.getByRole("button", { name: "发送" }));
    await waitFor(() => {
      expect(screen.getByText(/当前对话没有进行中的回复/)).toBeInTheDocument();
    });
    // 移除助手回复 → 整段历史重新落库：提示不得混进去
    const bubbleRow = screen
      .getByText("机器人回复")
      .closest("div.chat-bubble")?.parentElement;
    expect(bubbleRow).not.toBeNull();
    await user.click(
      within(bubbleRow as HTMLElement).getByTitle(
        "移除这条回复（不满意时删除，不再作为上下文）",
      ),
    );
    const saveCalls = mocks.invokeMock.mock.calls.filter(
      (c) => c[0] === "bot_history_save",
    );
    const saveCall = saveCalls[saveCalls.length - 1];
    expect(JSON.stringify(saveCall?.[1])).not.toContain("当前对话没有进行中的回复");
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

  it("推理强度：默认跟随后台（中·默认）随发送传入；会话覆盖只影响本会话且不回写（RE-1）", async () => {
    const user = userEvent.setup();
    // 上一个用例把 bot_chat 换成挂起 Promise，这里恢复默认实现（见 defaultInvoke 注释）
    mocks.invokeMock.mockImplementation(defaultInvoke);
    render(<ChatPanel {...defaultProps} />);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_sessions_load");
    });
    // bot_get_config 未带 reasoningEffort → 基线 medium；按钮显示「中·默认」
    expect(await screen.findByText(/中·默认/)).toBeInTheDocument();

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
    expect(await screen.findByText("机器人回复")).toBeInTheDocument();

    // 打开覆盖下拉选「高」→ 按钮去掉「·默认」角标
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
      return defaultInvoke(cmd);
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
          messages: expect.arrayContaining([
            expect.objectContaining({ role: "user", content: "给一会的草稿" }),
          ]),
        }),
      );
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

  it("流式事件按会话过滤：别会话的 bot-chat-delta 被忽略，本会话的才追加", async () => {
    const user = userEvent.setup();
    // 捕获 ChatPanel 注册的 bot-chat-delta 监听器
    let deltaHandler:
      ((e: { payload: Record<string, unknown> }) => void) | null = null;
    mocks.listenMock.mockImplementation(async (event: string, cb: unknown) => {
      if (event === "bot-chat-delta") deltaHandler = cb as typeof deltaHandler;
      return () => {};
    });
    // bot_chat 挂起不返回 → streaming 气泡保持，便于观察增量
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_chat") return new Promise(() => {});
      return null;
    });
    render(<ChatPanel {...defaultProps} />);
    // 等初始会话加载完成（sessionIdRef 同步后再 fire，否则「本会话」用例会被误过滤）
    expect(await screen.findAllByText("默认会话")).not.toHaveLength(0);
    expect(deltaHandler).not.toBeNull();
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "你好");
    await user.keyboard("{Enter}");
    const fire = (payload: Record<string, unknown>) =>
      (
        deltaHandler as unknown as (e: {
          payload: Record<string, unknown>;
        }) => void
      )({ payload });
    // 别的会话的增量 → 忽略（streaming 气泡内容不变）
    await act(async () =>
      fire({ text: "别会话输出", sessionId: "other-session" }),
    );
    expect(screen.queryByText("别会话输出")).not.toBeInTheDocument();
    // 无归属（后台任务不推流，双保险）→ 忽略
    await act(async () => fire({ text: "无归属输出", sessionId: null }));
    expect(screen.queryByText("无归属输出")).not.toBeInTheDocument();
    // 本会话（s1）的增量 → 追加进 streaming 气泡
    await act(async () => fire({ text: "本会话输出", sessionId: "s1" }));
    expect(await screen.findByText("本会话输出")).toBeInTheDocument();
  });

  it("自动滚动：环境没有 scrollTo 时不炸（可选链挡不住方法缺失）", async () => {
    const user = userEvent.setup();
    // jsdom 本身不实现 scrollTo，测试基建按 suite scope 临时补上——补的窗口期
    // 里它可能已经不在。可选链只挡 ref 为空，挡不住方法缺失：缺方法时这行
    // 抛 TypeError，落在 passive effect 里就成了随机某个用例的红
    const saved = Element.prototype.scrollTo;
    delete (Element.prototype as unknown as Record<string, unknown>).scrollTo;
    try {
      render(<ChatPanel {...defaultProps} />);
      expect(await screen.findAllByText("默认会话")).not.toHaveLength(0);
      const input = screen.getByPlaceholderText(/和机器人说点什么/);
      await user.type(input, "你好");
      await user.keyboard("{Enter}");
      await act(async () => {});
    } finally {
      (Element.prototype as unknown as Record<string, unknown>).scrollTo = saved;
    }
  });

  it("流式增量 16ms 合并：同帧多条 delta 攒成一次写入，内容不丢且保持到达顺序", async () => {
    const user = userEvent.setup();
    let deltaHandler:
      ((e: { payload: Record<string, unknown> }) => void) | null = null;
    mocks.listenMock.mockImplementation(async (event: string, cb: unknown) => {
      if (event === "bot-chat-delta") deltaHandler = cb as typeof deltaHandler;
      return () => {};
    });
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_chat") return new Promise(() => {}); // 挂起不返回，气泡保持 streaming
      return null;
    });
    render(<ChatPanel {...defaultProps} />);
    expect(await screen.findAllByText("默认会话")).not.toHaveLength(0);
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "你好");
    await user.keyboard("{Enter}");
    const fire = (payload: Record<string, unknown>) =>
      (
        deltaHandler as unknown as (e: {
          payload: Record<string, unknown>;
        }) => void
      )({ payload });

    // 同一帧内连发三条：合并窗口内不立刻写 state（这是「合并」的直接证据）
    await act(async () => {
      fire({ text: "第一段", sessionId: "s1" });
      fire({ text: "第二段", sessionId: "s1" });
      fire({ text: "第三段", sessionId: "s1" });
    });
    expect(screen.queryByText(/第一段/)).not.toBeInTheDocument();

    // flush 后一次到位：三条按到达顺序拼接，不丢字
    expect(await screen.findByText("第一段第二段第三段")).toBeInTheDocument();
  });

  it("助手消息可折叠：thinking + tools Fold 子组件渲染", async () => {
    // 自定义 invoke 返回带 thinking + tools 的历史
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") {
        return [
          {
            role: "assistant",
            content: "思考后的答案",
            refsJson: null,
            thinking: "我在想……",
            toolsJson: JSON.stringify([
              { id: "t1", name: "search", args: "{}", done: true },
            ]),
          },
        ];
      }
      return null;
    });
    render(<ChatPanel {...defaultProps} />);
    // 思考与工具都默认折叠：思考 title 含「思考过程」；工具收在
    // 「🔧 调用工具 N/M」一行里——十几个 pill 平铺会把正文挤出视野
    expect(await screen.findByText(/思考过程/)).toBeInTheDocument();
    expect(await screen.findByText(/调用工具 1\/1/)).toBeInTheDocument();
    expect(screen.queryByText("search")).not.toBeInTheDocument();
    // 展开工具块才看到 mono pill 徽章行（名称 + ✓ 状态）；详细档下同一工具
    // 在详情区还会再出现一次（徽章 + 入参行头），故用 findAll
    await userEvent.setup().click(screen.getByText(/调用工具 1\/1/));
    expect((await screen.findAllByText("search")).length).toBeGreaterThan(0);
    // 点击思考 toggle，展开内容
    await userEvent.setup().click(screen.getByText(/思考过程/));
    expect(await screen.findByText("我在想……")).toBeInTheDocument();
  });

  it("错误展示：bot_chat 抛错时气泡显示 ⚠️ 错误信息", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_chat")
        throw {
          code: "LLM_API_ERROR",
          message: "API 配额超限",
          recoverable: true,
        };
      return null;
    });
    render(<ChatPanel {...defaultProps} />);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_sessions_load");
    });
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "测试错误");
    await user.keyboard("{Enter}");
    // 流式错误写入气泡：⚠️ + formatCommandError 提取的 message
    expect(await screen.findByText(/⚠️ API 配额超限/)).toBeInTheDocument();
  });

  it("runChat 失败后清理本会话的流式元数据条目（不滞留到下次收尾或重启）", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_history_save") return null;
      if (cmd === "bot_chat")
        throw {
          code: "LLM_API_ERROR",
          message: "API 配额超限",
          recoverable: true,
        };
      return null;
    });
    // streamingMetaMap 是组件内部 ref，UI 无观测面。借 Map.prototype.delete
    // 记录调用，证明失败路径也执行了与流开始 set(sid,{}) 对称的清理。
    // inflightRef 是 Set（走 Set.prototype.delete），不会混进记录。
    const deletes: unknown[][] = [];
    const origDelete = Map.prototype.delete;
    const spy = vi
      .spyOn(Map.prototype, "delete")
      .mockImplementation(function (
        this: Map<unknown, unknown>,
        key: unknown,
      ) {
        deletes.push([key]);
        return origDelete.call(this, key);
      });
    try {
      render(<ChatPanel {...defaultProps} />);
      await screen.findAllByText("默认会话");
      const mark = deletes.length;
      const input = screen.getByPlaceholderText(/和机器人说点什么/);
      await user.type(input, "会失败的一条");
      await user.keyboard("{Enter}");
      // 失败收尾完成（⚠️ 气泡渲染 = catch + finally 已跑完）
      expect(await screen.findByText(/⚠️ API 配额超限/)).toBeInTheDocument();
      expect(
        deletes.slice(mark).some(([key]) => key === "s1"),
      ).toBe(true);
    } finally {
      spy.mockRestore();
    }
  });

  it("任务卡执行被 TASK_INVALID_STATE 拒绝：按结构化 code 识别，⏳ 本地提示而非 ⚠️ 气泡", async () => {
    // 捕获 listen 回调以手动触发 execute-task 事件
    const listeners = new Map<string, (e: { payload: unknown }) => void>();
    mocks.listenMock.mockImplementation(
      async (event: string, cb: (e: { payload: unknown }) => void) => {
        listeners.set(event, cb);
        return () => {};
      },
    );
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_execute_task")
        throw {
          code: "TASK_INVALID_STATE",
          message:
            "任务状态不允许该操作：该任务卡正在执行中，请等待完成后再触发",
          recoverable: true,
        };
      return null;
    });
    try {
      render(<ChatPanel {...defaultProps} />);
      await waitFor(() => {
        expect(mocks.invokeMock).toHaveBeenCalledWith("bot_sessions_load");
      });
      await act(async () => {
        listeners.get("execute-task")?.({
          payload: { id: "t-reentry", title: "测试任务" },
        });
      });
      // 本地 ⏳ 提示（透传后端 message），不产生 ⚠️ 错误气泡
      expect(
        await screen.findByText(/⏳ 任务状态不允许该操作：该任务卡正在执行中/),
      ).toBeInTheDocument();
      expect(screen.queryByText(/⚠️/)).not.toBeInTheDocument();
    } finally {
      // mockClear 不复位实现，手工恢复默认，防泄漏到后续用例
      mocks.listenMock.mockImplementation(async () => () => {});
    }
  });

  it("拖文件进聊天区：enter 显示提示层，drop 落在聊天区内加入附件（区外忽略、去重）", async () => {
    render(<ChatPanel {...defaultProps} />);
    expect(await screen.findAllByText("默认会话")).not.toHaveLength(0);
    const handler = mocks.dragDropHandlers[mocks.dragDropHandlers.length - 1];
    expect(handler).toBeDefined();
    // jsdom 的 getBoundingClientRect 全 0：position (0,0) 视为聊天区内，(10,10) 视为区外
    // enter 悬停 → 提示层出现
    await act(async () => {
      await handler({
        payload: {
          type: "enter",
          paths: ["/tmp/a.docx"],
          position: { x: 0, y: 0 },
        },
      });
    });
    expect(await screen.findByText("松开以添加文件")).toBeInTheDocument();
    // drop 在区内 → 加入附件芯片，提示层消失
    await act(async () => {
      await handler({
        payload: {
          type: "drop",
          paths: ["/tmp/a.docx"],
          position: { x: 0, y: 0 },
        },
      });
    });
    expect(await screen.findByText(/a\.docx/)).toBeInTheDocument();
    expect(screen.queryByText("松开以添加文件")).not.toBeInTheDocument();
    // 同一路径再拖一次 → 去重，仍只有一个芯片
    await act(async () => {
      await handler({
        payload: {
          type: "drop",
          paths: ["/tmp/a.docx"],
          position: { x: 0, y: 0 },
        },
      });
    });
    expect(screen.getAllByText(/a\.docx/)).toHaveLength(1);
    // drop 在聊天区外（任务列表区）→ 不添加
    await act(async () => {
      await handler({
        payload: {
          type: "drop",
          paths: ["/tmp/b.pdf"],
          position: { x: 10, y: 10 },
        },
      });
    });
    expect(screen.queryByText(/b\.pdf/)).not.toBeInTheDocument();
  });

  // 任务执行聊天化：chat-open-session 跳转/排队

  it("chat-open-session 非 busy：直接切换到执行会话并加载其历史", async () => {
    // 前面的用例会把 listenMock 恢复成不记录的默认实现，这里显式重设
    mocks.listenMock.mockImplementation(
      async (
        event: string,
        cb: (e: { payload: Record<string, unknown> }) => void,
      ) => {
        (mocks.listeners[event] ??= []).push(cb);
        return () => {};
      },
    );
    mocks.invokeMock.mockImplementation(
      async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "bot_sessions_load")
          return [{ id: "s1", title: "默认会话" }];
        if (cmd === "bot_history_load")
          return args?.sessionId === "s-exec"
            ? [
                {
                  role: "user",
                  content: "[任务卡执行]\nid=t1\n标题：写报告",
                  refsJson: null,
                  thinking: null,
                  toolsJson: null,
                },
              ]
            : [];
        return null;
      },
    );
    render(<ChatPanel {...defaultProps} />);
    expect(await screen.findAllByText("默认会话")).not.toHaveLength(0);
    await act(async () => {
      for (const cb of mocks.listeners["chat-open-session"] ?? []) {
        cb({
          payload: {
            sessionId: "s-exec",
            taskId: "t1",
            title: "📋 任务：写报告",
            origin: "manual",
          },
        });
      }
    });
    // 切到执行会话：加载其历史，任务块渲染出来
    await waitFor(() =>
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_history_load", {
        sessionId: "s-exec",
      }),
    );
    expect(await screen.findByText(/\[任务卡执行\]/)).toBeInTheDocument();
    // 会话列表已含新会话
    expect(railSession(/📋 任务：写报告/)).toBeInTheDocument();
    expect(currentTitle()).toHaveTextContent("📋 任务：写报告");
  });

  it("chat-open-session 迟到（execute-task 已收尾）：失败任务不自动跳转让用户决定；成功任务切过去只读查看", async () => {
    mocks.listenMock.mockImplementation(
      async (
        event: string,
        cb: (e: { payload: Record<string, unknown> }) => void,
      ) => {
        (mocks.listeners[event] ??= []).push(cb);
        return () => {};
      },
    );
    mocks.invokeMock.mockImplementation(
      async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "bot_sessions_load")
          return [{ id: "s1", title: "默认会话" }];
        if (cmd === "bot_execute_task") {
          if (args?.taskId === "t-fail")
            throw {
              code: "TASK_INVALID_STATE",
              message: "任务失败",
            } as unknown;
          return null;
        }
        if (cmd === "bot_history_load")
          return args?.sessionId === "s-done"
            ? [
                {
                  role: "user",
                  content: "[任务卡执行]\nid=t-done\n标题：已完成",
                  refsJson: null,
                  thinking: null,
                  toolsJson: null,
                },
              ]
            : [];
        return null;
      },
    );
    render(<ChatPanel {...defaultProps} />);
    expect(await screen.findAllByText("默认会话")).not.toHaveLength(0);
    // 两张卡都收尾（一成一败）——收尾顺序在迟到事件之前
    await act(async () => {
      for (const cb of mocks.listeners["execute-task"] ?? []) {
        cb({ payload: { id: "t-done", title: "成功任务" } });
        cb({ payload: { id: "t-fail", title: "失败任务" } });
      }
    });
    await act(async () => {});
    // 迟到的失败任务 open-session：不自动切（历史不加载，用户自行决定是否重试/查看）
    await act(async () => {
      for (const cb of mocks.listeners["chat-open-session"] ?? []) {
        cb({
          payload: { sessionId: "s-fail", taskId: "t-fail", title: "失败任务" },
        });
      }
    });
    expect(mocks.invokeMock).not.toHaveBeenCalledWith("bot_history_load", {
      sessionId: "s-fail",
    });
    // 迟到的成功任务 open-session：切过去查看最终历史
    await act(async () => {
      for (const cb of mocks.listeners["chat-open-session"] ?? []) {
        cb({
          payload: { sessionId: "s-done", taskId: "t-done", title: "成功任务" },
        });
      }
    });
    await waitFor(() =>
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_history_load", {
        sessionId: "s-done",
      }),
    );
    expect(await screen.findByText(/\[任务卡执行\]/)).toBeInTheDocument();
  });

  it("chat-open-session busy：跳转排队，当前轮结束后出现「查看执行对话」按钮，点击切换", async () => {
    mocks.listenMock.mockImplementation(
      async (
        event: string,
        cb: (e: { payload: Record<string, unknown> }) => void,
      ) => {
        (mocks.listeners[event] ??= []).push(cb);
        return () => {};
      },
    );
    const user = userEvent.setup();
    let releaseChat: (v: {
      text: string;
      taskRefs: never[];
    }) => void = () => {};
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_chat")
        return new Promise((r) => {
          releaseChat = r as typeof releaseChat;
        });
      return null;
    });
    render(<ChatPanel {...defaultProps} />);
    expect(await screen.findAllByText("默认会话")).not.toHaveLength(0);
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "你好");
    await user.keyboard("{Enter}");
    // busy 中收到 chat-open-session → 不切换、不读执行会话历史
    await act(async () => {
      for (const cb of mocks.listeners["chat-open-session"] ?? []) {
        cb({
          payload: {
            sessionId: "s-exec",
            taskId: "t1",
            title: "📋 任务：写报告",
            origin: "scheduled",
          },
        });
      }
    });
    expect(railSession("默认会话")).toBeInTheDocument();
    expect(mocks.invokeMock).not.toHaveBeenCalledWith("bot_history_load", {
      sessionId: "s-exec",
    });
    // 当前轮结束 → 排队跳转兑现为提示 + 按钮
    await act(async () => {
      releaseChat({ text: "回复", taskRefs: [] });
    });
    expect(await screen.findByText(/已在新会话执行/)).toBeInTheDocument();
    const btn = await screen.findByText("查看执行对话");
    await user.click(btn);
    await waitFor(() =>
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_history_load", {
        sessionId: "s-exec",
      }),
    );
  });

  // ── 双命令：🧠 模型下拉 ──

  it("右侧栏文件工作面：点文件就地预览内容，能返回列表", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      // 回合不收尾（bot_chat 悬着）：文件改动是流式回合里才有的事件
      if (cmd === "bot_chat") return new Promise(() => {});
      if (cmd === "bot_preview_text")
        return {
          kind: "text",
          path: "/tmp/报告.md",
          content: "# 标题\n\n正文一段",
          truncated: false,
          totalBytes: 14,
          lines: 3,
        };
      return defaultInvoke(cmd);
    });
    mocks.listenMock.mockImplementation(async (event: string, cb: unknown) => {
      ((mocks.listeners[event] ??= []) as Array<typeof cb>).push(
        cb as (e: { payload: Record<string, unknown> }) => void,
      );
      return () => {};
    });
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    await user.type(screen.getByPlaceholderText(/和机器人说点什么/), "改文件");
    await user.keyboard("{Enter}");
    await act(async () => {
      for (const cb of mocks.listeners["bot-file-changed"] ?? []) {
        cb({
          payload: {
            sessionId: "s1",
            path: "/tmp/报告.md",
            kind: "modify",
            added: 3,
            deleted: 1,
          },
        });
      }
    });
    // 打开右侧栏 → 文件工作面
    await user.click(screen.getByRole("button", { name: "展开右侧栏" }));
    const rail = within(
      await screen.findByRole("complementary", { name: "上下文侧栏" }),
    );
    await user.click(rail.getByRole("button", { name: /文件/ }));
    // 文件行可点（以前是只读 div）
    const row = await rail.findByTitle(/报告\.md/);
    expect(row.tagName).toBe("BUTTON");
    await user.click(row);
    // 内容在侧栏里就地渲染，不跳出去
    expect(mocks.invokeMock).toHaveBeenCalledWith("bot_preview_text", {
      path: "/tmp/报告.md",
    });
    expect(await rail.findByLabelText("文件内容")).toBeInTheDocument();
    expect(rail.getByText("标题")).toBeInTheDocument();
    // 能退回列表
    await user.click(rail.getByText(/返回文件列表/));
    await waitFor(() =>
      expect(rail.queryByLabelText("文件内容")).toBeNull(),
    );
  });

  it("授权模式可点选：写入走窄命令 bot_set_perm_mode，落盘失败回滚", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_set_perm_mode") throw { code: "IO", message: "只读磁盘" };
      return defaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    // 默认档是 ask（bot_get_config 没给 permMode 时按安全默认归一）
    const pill = await waitFor(() =>
      screen.getByRole("button", { name: /弹授权/ }),
    );
    // 以前这里是个只读 span，点不动——正是要改的地方
    expect(pill.tagName).toBe("BUTTON");
    await user.click(pill);
    const opt = await screen.findByRole("option", { name: /全放行/ });
    await user.click(opt);
    // 写盘走窄命令（只动 perm_mode），不是整份 bot_set_config 回写
    expect(mocks.invokeMock).toHaveBeenCalledWith("bot_set_perm_mode", {
      mode: "yolo",
    });
    expect(
      mocks.invokeMock.mock.calls.some((c) => c[0] === "bot_set_config"),
    ).toBe(false);
    // 落盘失败 → 回滚到 ask 并弹错，不留「界面显示已放行」的假绿
    await waitFor(() =>
      expect(screen.getByRole("button", { name: /弹授权/ })).toBeInTheDocument(),
    );
  });

  it("授权模式成功落盘后 pill 切换到新档位（yolo 走警示色）", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_config")
        return { ...((await defaultInvoke("bot_get_config")) as object), permMode: "ask" };
      if (cmd === "bot_set_perm_mode") return defaultInvoke("bot_get_config");
      return defaultInvoke(cmd);
    });
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    const pill = await screen.findByRole("button", { name: /弹授权/ });
    await user.click(pill);
    await user.click(await screen.findByRole("option", { name: /自动/ }));
    const next = await screen.findByRole("button", { name: /自动/ });
    expect(mocks.invokeMock).toHaveBeenCalledWith("bot_set_perm_mode", {
      mode: "auto",
    });
    // 下拉选完自动收起（选完还开着会挡住下一次点发送）
    await waitFor(() =>
      expect(screen.queryByRole("option", { name: /全放行/ })).toBeNull(),
    );
    expect(next).toBeInTheDocument();
  });

  it("点 🧠 弹出当前协议模型列表，再点收起", async () => {
    const user = userEvent.setup();
    // 前面用例换过的 mockImplementation 会跨用例残留（beforeEach 只 mockClear），
    // 先恢复默认实现拿到 bot_get_config / bot_set_active_model 的 mock
    mocks.invokeMock.mockImplementation(defaultInvoke);
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    // 等 bot_get_config reload 完成（chip 标签与 setModels 同一个 .then 刷新）；
    // chip 与下拉项同名，一律在 chip 按钮内查询（findByText 作用于按钮本身）
    const chip = screen.getByTitle("切换模型");
    await waitFor(() => {
      expect(chip).toHaveTextContent("MiniMax-M3");
    });
    await user.click(chip);
    expect(await screen.findByText("DeepSeek-V3")).toBeInTheDocument();
    // chip 与下拉项同名：chip 用 toHaveTextContent，下拉项按 class 限定
    expect(chip).toHaveTextContent("MiniMax-M3");
    // 再点收起
    await user.click(chip);
    await waitFor(() => {
      expect(screen.queryByText("DeepSeek-V3")).toBeNull();
    });
  });

  it("模型下拉可用性过滤：带厂商但未通过连接测试的模型不显示，已验证的显示", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_config")
        return {
          baseUrl: "https://api.deepseek.com",
          model: "deepseek-chat",
          apiProvider: "openai",
          modelsByProvider: {
            openai: [
              {
                id: "m1",
                label: "DeepSeek Chat",
                baseUrl: "https://api.deepseek.com",
                model: "deepseek-chat",
                vendor: "DeepSeek",
              },
              {
                id: "m2",
                label: "Kimi K3",
                baseUrl: "https://api.moonshot.cn/v1",
                model: "kimi-k3",
                vendor: "Kimi",
              },
              {
                id: "m3",
                label: "自建直连",
                baseUrl: "https://llm.example.com/v1",
                model: "my-model",
              },
            ],
            anthropic: [],
          },
          activeModelId: { openai: "m1", anthropic: null },
          // 只有 Kimi 通过了连接测试
          verifiedVendors: ["Kimi"],
        };
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      return null;
    });
    render(<ChatPanel {...defaultProps} />);
    await screen.findAllByText("默认会话");
    const chip = screen.getByTitle("切换模型");
    await user.click(chip);
    // Kimi 已验证 → 显示；DeepSeek 未验证 → 不显示；无 vendor 的自建条目不受影响
    expect(await screen.findByText("Kimi K3")).toBeInTheDocument();
    expect(screen.queryByText("DeepSeek Chat")).toBeNull();
    expect(screen.getByText("自建直连")).toBeInTheDocument();
  });

  // ── SWITCH-1：busy 期实时切换对话，回复不串台 ──

  it("busy 中切换/新建/删除（拦回复中会话），回复仍落原会话", async () => {
    const user = userEvent.setup();
    // 捕获事件监听器（手动触发流式增量用）
    const evListeners: Record<string, ((e: { payload: unknown }) => void)[]> =
      {};
    const prevListenImpl = mocks.listenMock.getMockImplementation();
    mocks.listenMock.mockImplementation(
      async (ev: string, cb: (e: { payload: unknown }) => void) => {
        (evListeners[ev] ??= []).push(cb);
        return () => {};
      },
    );
    const historyLoads: unknown[] = [];
    const saves: Array<Record<string, unknown> | undefined> = [];
    let releaseChat!: (v: { text: string; taskRefs: [] }) => void;
    const chatGate = new Promise<{ text: string; taskRefs: [] }>((r) => {
      releaseChat = r;
    });
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
            return chatGate;
          case "bot_history_load":
            historyLoads.push(args?.sessionId);
            return [];
          case "bot_history_save":
            saves.push(args);
            return null;
          case "bot_session_create":
            return { id: "s-new", title: "新对话" };
          default:
            return defaultInvoke(cmd);
        }
      },
    );
    try {
      render(<ChatPanel {...defaultProps} />);
      await screen.findAllByText("默认会话");
      // 发送 → bot_chat 挂起 → busy
      const input = screen.getByPlaceholderText(/和机器人说点什么/);
      await user.type(input, "你好");
      await user.keyboard("{Enter}");
      await waitFor(() => {
        expect(mocks.invokeMock).toHaveBeenCalledWith(
          "bot_chat",
          expect.objectContaining({ sessionId: "s1" }),
        );
      });
      // busy 中切换到 s2 → 允许（回归钉 1：不再被 busy 拦截）
      await user.click(railSession("另一个"));
      await waitFor(() => {
        expect(historyLoads).toContain("s2");
      });
      // 向 s1（后台回复中）发流式增量 → 不得污染当前 s2 视图
      await act(async () => {
        for (const cb of evListeners["bot-chat-delta"] ?? []) {
          cb({ payload: { sessionId: "s1", text: "串台密文" } });
        }
      });
      expect(screen.queryByText(/串台密文/)).toBeNull();
      // busy 中删除正在回复的 s1 → 拦截（不弹 confirm、不调 delete）
      mocks.askMock.mockReset();
      const s1Row = railSession("默认会话").closest("div");
      // 删除键悬停才显形（列表不留白抖动），先 hover 再点
      await user.hover(s1Row!);
      await user.click(within(s1Row!).getByTitle("删除此对话"));
      expect(
        mocks.invokeMock.mock.calls.some((c) => c[0] === "bot_session_delete"),
      ).toBe(false);
      expect(mocks.askMock).not.toHaveBeenCalled();
      // 删除守卫返回后侧栏仍在：busy 中新建对话 → 允许（回归钉 2）
      await user.click(screen.getByText("新建对话"));
      await waitFor(() => {
        expect(mocks.invokeMock).toHaveBeenCalledWith("bot_session_create", {
          title: null,
        });
      });
      // 切回 s1（回复仍在进行）→ 允许并补占位气泡（s1 的 history_load 比挂载时 +1）
      const s1LoadsBefore = historyLoads.filter((x) => x === "s1").length;
      await user.click(railSession("默认会话"));
      expect(historyLoads.filter((x) => x === "s1").length).toBe(
        s1LoadsBefore + 1,
      );
      // 收尾：回复落回原会话 s1（不串台核心钉 3），绝不写 s2
      await act(async () => {
        releaseChat({ text: "回复A", taskRefs: [] });
      });
      await waitFor(() => {
        expect(saves.length).toBeGreaterThan(0);
      });
      for (const sv of saves) {
        expect(sv?.sessionId).toBe("s1");
      }
      const lastSave = saves[saves.length - 1] as {
        sessionId: string;
        messages: { role: string; content: string }[];
      };
      expect(lastSave.messages[lastSave.messages.length - 1].content).toBe(
        "回复A",
      );
    } finally {
      mocks.invokeMock.mockImplementation(prevInvokeImpl!);
      mocks.listenMock.mockImplementation(prevListenImpl!);
    }
  });

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
      return defaultInvoke(cmd);
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
      mocks.invokeMock.mockImplementation(defaultInvoke);
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
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
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
      if (cmd === "bot_chat") return { text: "写到 /Users/x/out.md 了", taskRefs: [] };
      return defaultInvoke(cmd);
    });
    try {
      render(<ChatPanel {...defaultProps} />);
      await screen.findAllByText("默认会话");
      await user.type(
        screen.getByPlaceholderText(/和机器人说点什么/),
        "写个文档",
      );
      await user.keyboard("{Enter}");
      // 文件 pill（正文无改动的正则兜底路径）
      const pill = await screen.findByTitle(
        "在右侧栏预览：/Users/x/out.md",
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
      mocks.invokeMock.mockImplementation(defaultInvoke);
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
      return defaultInvoke(cmd);
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
      mocks.invokeMock.mockImplementation(defaultInvoke);
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
      return defaultInvoke(cmd);
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
      return defaultInvoke(cmd);
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
      return defaultInvoke(cmd);
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
      mocks.invokeMock.mockImplementation(defaultInvoke);
    }
  });

  it("点选模型调用 bot_set_active_model（窄口径）并广播同步", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(defaultInvoke);
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
      return defaultInvoke(cmd);
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
            return defaultInvoke(cmd);
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
        return defaultInvoke(cmd);
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
      mocks.invokeMock.mockImplementation(defaultInvoke);
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
      return defaultInvoke(cmd);
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
      mocks.invokeMock.mockImplementation(defaultInvoke);
      mocks.listenMock.mockImplementation(async () => () => {});
    }
  });
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

