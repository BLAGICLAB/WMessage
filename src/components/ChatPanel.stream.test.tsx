import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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
describe("ChatPanel", () => {
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

  it("用户叫停：回合终态后到达的迟到增量不再开一个「正在思考」的气泡（活体指示不该永远转）", async () => {
    const user = userEvent.setup();
    const handlers: Record<
      string,
      (e: { payload: Record<string, unknown> }) => void
    > = {};
    mocks.listenMock.mockImplementation(
      async (event: string, cb: (e: { payload: unknown }) => void) => {
        handlers[event] = cb as (e: { payload: Record<string, unknown> }) => void;
        return () => {};
      },
    );
    // bot_chat 挂起：回合在跑，等后端收尾才 resolve
    let settleChat: ((v: { text: string; taskRefs: unknown[] }) => void) | null =
      null;
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_chat")
        return new Promise((resolve) => {
          settleChat = resolve as typeof settleChat;
        });
      return null;
    });
    render(<ChatPanel {...defaultProps} />);
    expect(await screen.findAllByText("默认会话")).not.toHaveLength(0);
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "做个 ppt");
    await user.keyboard("{Enter}");
    // 真机上的事件顺序（后端先推最后一块在途数据，再落终态，最后命令返回）：
    // ① 迟到的思考增量 ② 回合终态 ③ bot_chat 返回最终答复
    await act(async () => {
      handlers["bot-think-delta"]?.({
        payload: { text: "还在想", sessionId: "s1" },
      });
      handlers["bot-turn-status"]?.({
        payload: { status: "interrupted", detail: "", sessionId: "s1" },
      });
      settleChat?.({ text: "⏹ 已停止", taskRefs: [] });
      // 等过合并窗口（flush 落在下一帧）：增量此时已被处理完
      await new Promise((r) => setTimeout(r, 60));
    });
    expect(await screen.findByText("⏹ 已停止")).toBeInTheDocument();
    // 判据：不得多出第二个「正在思考」气泡——回合已终，不会再有终态事件
    // 来收它，活体指示会一直转到用户以为还在跑
    expect(screen.queryByText("正在思考")).not.toBeInTheDocument();
  });

  it("后端主动起的一轮（子任务收尾汇报）：无预置气泡时增量照样开出活体气泡", async () => {
    const user = userEvent.setup();
    const handlers: Record<
      string,
      (e: { payload: Record<string, unknown> }) => void
    > = {};
    mocks.listenMock.mockImplementation(
      async (event: string, cb: (e: { payload: unknown }) => void) => {
        handlers[event] = cb as (e: { payload: Record<string, unknown> }) => void;
        return () => {};
      },
    );
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_chat") return { text: "机器人回复", taskRefs: [] };
      return null;
    });
    render(<ChatPanel {...defaultProps} />);
    expect(await screen.findAllByText("默认会话")).not.toHaveLength(0);
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "你好");
    await user.keyboard("{Enter}");
    expect(await screen.findByText("机器人回复")).toBeInTheDocument();
    // 后端主动起的一轮：先来一条 running 状态（回合状态机口径：本轮在跑），
    // 再来增量——前端没有预置气泡，增量要能自己开一个
    await act(async () => {
      handlers["bot-turn-status"]?.({
        payload: { status: "running", detail: "正在思考 · 第 1 轮", sessionId: "s1" },
      });
      handlers["bot-chat-delta"]?.({
        payload: { text: "汇报正文", sessionId: "s1" },
      });
      await new Promise((r) => setTimeout(r, 60));
    });
    expect(screen.getByText("汇报正文")).toBeInTheDocument();
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
});
