import { describe, it, expect, vi, beforeEach } from "vitest";
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
});
