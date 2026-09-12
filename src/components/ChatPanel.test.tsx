import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ChatPanel } from "./ChatPanel";
import type { Task } from "../types";

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
  const listeners: Record<string, Array<(e: { payload: Record<string, unknown> }) => void>> = {};
  return { invokeMock, listenMock, emitMock, dragDropHandlers, listeners };
});

// 默认实现
mocks.invokeMock.mockImplementation(async (cmd: string) => {
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
});
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
      cb: (e: { payload: Record<string, unknown> }) => unknown
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

vi.mock("../focus", () => ({
  focusMainWindow: vi.fn(async () => {}),
}));

// navigator.clipboard.writeText
const writeTextMock = vi.fn(async () => {});
Object.defineProperty(navigator, "clipboard", {
  configurable: true,
  value: { writeText: writeTextMock },
});

const defaultProps = {
  selecting: false,
  onToggleSelecting: vi.fn(),
  selectedTasks: [] as Task[],
  onRemoveSelected: vi.fn(),
  onFinishSelection: vi.fn(),
  enabled: true,
};

beforeEach(() => {
  mocks.invokeMock.mockClear();
  mocks.listenMock.mockClear();
  writeTextMock.mockClear();
  mocks.dragDropHandlers.length = 0;
  for (const k of Object.keys(mocks.listeners)) delete mocks.listeners[k];
});

describe("ChatPanel", () => {
  it("初始渲染：加载默认会话后显示空消息提示", async () => {
    render(<ChatPanel {...defaultProps} />);
    // 初始 useEffect 加载会话
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_sessions_load");
    });
    expect(
      await screen.findByText(/跟我说：新建任务|输入 \/ 看可用命令/, { exact: false })
    ).toBeInTheDocument();
    // 会话标题
    expect(screen.getByText("🤖 默认会话")).toBeInTheDocument();
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
        })
      );
    });
    // 流式完成后机器人回复渲染
    expect(await screen.findByText("机器人回复")).toBeInTheDocument();
  });

  it("输入 /stop 后按发送：busy=false → 提示「当前没有进行中的回复」", async () => {
    const user = userEvent.setup();
    render(<ChatPanel {...defaultProps} />);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_sessions_load");
    });
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    // 直接以带空格的形式输入（picker 不会因含空格弹出，send 路径走 runSlashCommand）
    await user.type(input, "/stop ");
    // 点击「发送」按钮触发 send → runSlashCommand → /stop
    await user.click(screen.getByText("发送"));
    // 当前没有进行中的回复 → addHint："当前没有进行中的回复"，bot_stop 不调用
    await waitFor(() => {
      expect(screen.getByText(/当前没有进行中的回复/)).toBeInTheDocument();
    });
    expect(mocks.invokeMock).not.toHaveBeenCalledWith("bot_stop");
  });

  it("回复中发送键变为红框停止键：点击调 bot_stop（2026-08-26 发送/停止一体键）", async () => {
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
    // busy=true：发送键消失，红框正方形停止键出现
    const stopBtn = await screen.findByTitle("停止当前回复");
    expect(screen.queryByText("发送")).not.toBeInTheDocument();
    await user.click(stopBtn);
    // /stop 按会话停止——携带当前会话 id
    expect(mocks.invokeMock).toHaveBeenCalledWith("bot_stop", { sessionId: "s1" });
  });

  it("确认弹窗按会话过滤：别的会话的 bot-confirm 不弹窗（2026-08-26 会话隔离）", async () => {
    // 捕获 ChatPanel 注册的 bot-confirm 监听器
    let confirmHandler: ((e: { payload: Record<string, unknown> }) => void) | null = null;
    mocks.listenMock.mockImplementation(async (event: string, cb: unknown) => {
      if (event === "bot-confirm") confirmHandler = cb as typeof confirmHandler;
      return () => {};
    });
    render(<ChatPanel {...defaultProps} />);
    // 等初始会话加载完成（标题渲染 = sessionId 已 set 且 sessionIdRef 已同步），
    // 否则 fire 时 sessionIdRef.current 还是 null，「本会话」用例会被误过滤（全量跑时序敏感）
    expect(await screen.findByText("🤖 默认会话")).toBeInTheDocument();
    expect(confirmHandler).not.toBeNull();
    const fire = (payload: Record<string, unknown>) =>
      (confirmHandler as unknown as (e: { payload: Record<string, unknown> }) => void)({ payload });
    // 别的会话（sessionId 不匹配）→ 不弹
    fire({ id: "c1", tool: "delete_task", detail: "别会话任务", sessionId: "other-session" });
    expect(screen.queryByText(/别会话任务/)).not.toBeInTheDocument();
    // 无 sessionId（后台任务）→ 不弹
    fire({ id: "c2", tool: "delete_task", detail: "后台任务", sessionId: null });
    expect(screen.queryByText(/后台任务/)).not.toBeInTheDocument();
    // 当前会话（s1，初始加载的默认会话）→ 弹
    fire({ id: "c3", tool: "delete_task", detail: "本会话任务", sessionId: "s1" });
    expect(await screen.findByText(/本会话任务/)).toBeInTheDocument();
  });

  it("流式事件按会话过滤：别会话的 bot-chat-delta 被忽略，本会话的才追加（2026-08-28 批次3审计 P0-2）", async () => {
    const user = userEvent.setup();
    // 捕获 ChatPanel 注册的 bot-chat-delta 监听器
    let deltaHandler: ((e: { payload: Record<string, unknown> }) => void) | null = null;
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
    expect(await screen.findByText("🤖 默认会话")).toBeInTheDocument();
    expect(deltaHandler).not.toBeNull();
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "你好");
    await user.keyboard("{Enter}");
    const fire = (payload: Record<string, unknown>) =>
      (deltaHandler as unknown as (e: { payload: Record<string, unknown> }) => void)({ payload });
    // 别的会话的增量 → 忽略（streaming 气泡内容不变）
    await act(async () => fire({ text: "别会话输出", sessionId: "other-session" }));
    expect(screen.queryByText("别会话输出")).not.toBeInTheDocument();
    // 无归属（后台任务不推流，双保险）→ 忽略
    await act(async () => fire({ text: "无归属输出", sessionId: null }));
    expect(screen.queryByText("无归属输出")).not.toBeInTheDocument();
    // 本会话（s1）的增量 → 追加进 streaming 气泡
    await act(async () => fire({ text: "本会话输出", sessionId: "s1" }));
    expect(await screen.findByText("本会话输出")).toBeInTheDocument();
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
            toolsJson: JSON.stringify([{ id: "t1", name: "search", args: '{}', done: true }]),
          },
        ];
      }
      return null;
    });
    render(<ChatPanel {...defaultProps} />);
    // 思考与工具默认折叠，title 包含「思考过程」/「search」
    expect(await screen.findByText(/💭 思考过程/)).toBeInTheDocument();
    expect(await screen.findByText(/🔧 search/)).toBeInTheDocument();
    // 点击思考 toggle，展开内容
    await userEvent.setup().click(screen.getByText(/💭 思考过程/));
    expect(await screen.findByText("我在想……")).toBeInTheDocument();
  });

  it("错误展示：bot_chat 抛错时气泡显示 ⚠️ 错误信息", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_chat") throw { code: "LLM_API_ERROR", message: "API 配额超限", recoverable: true };
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

  it("任务卡执行被 TASK_INVALID_STATE 拒绝：按结构化 code 识别，⏳ 本地提示而非 ⚠️ 气泡（批次7 P2-2）", async () => {
    // 捕获 listen 回调以手动触发 execute-task 事件
    const listeners = new Map<string, (e: { payload: unknown }) => void>();
    mocks.listenMock.mockImplementation(
      async (event: string, cb: (e: { payload: unknown }) => void) => {
        listeners.set(event, cb);
        return () => {};
      }
    );
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_execute_task")
        throw {
          code: "TASK_INVALID_STATE",
          message: "任务状态不允许该操作：该任务卡正在执行中，请等待完成后再触发",
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
        listeners.get("execute-task")?.({ payload: { id: "t-reentry", title: "测试任务" } });
      });
      // 本地 ⏳ 提示（透传后端 message），不产生 ⚠️ 错误气泡
      expect(
        await screen.findByText(/⏳ 任务状态不允许该操作：该任务卡正在执行中/)
      ).toBeInTheDocument();
      expect(screen.queryByText(/⚠️/)).not.toBeInTheDocument();
    } finally {
      // mockClear 不复位实现，手工恢复默认，防泄漏到后续用例
      mocks.listenMock.mockImplementation(async () => () => {});
    }
  });

  it("拖文件进聊天区：enter 显示提示层，drop 落在聊天区内加入附件（区外忽略、去重）", async () => {
    render(<ChatPanel {...defaultProps} />);
    expect(await screen.findByText("🤖 默认会话")).toBeInTheDocument();
    const handler = mocks.dragDropHandlers[mocks.dragDropHandlers.length - 1];
    expect(handler).toBeDefined();
    // jsdom 的 getBoundingClientRect 全 0：position (0,0) 视为聊天区内，(10,10) 视为区外
    // enter 悬停 → 提示层出现
    await act(async () => {
      await handler({
        payload: { type: "enter", paths: ["/tmp/a.docx"], position: { x: 0, y: 0 } },
      });
    });
    expect(await screen.findByText("松开以添加文件")).toBeInTheDocument();
    // drop 在区内 → 加入附件芯片，提示层消失
    await act(async () => {
      await handler({
        payload: { type: "drop", paths: ["/tmp/a.docx"], position: { x: 0, y: 0 } },
      });
    });
    expect(await screen.findByText(/a\.docx/)).toBeInTheDocument();
    expect(screen.queryByText("松开以添加文件")).not.toBeInTheDocument();
    // 同一路径再拖一次 → 去重，仍只有一个芯片
    await act(async () => {
      await handler({
        payload: { type: "drop", paths: ["/tmp/a.docx"], position: { x: 0, y: 0 } },
      });
    });
    expect(screen.getAllByText(/a\.docx/)).toHaveLength(1);
    // drop 在聊天区外（任务列表区）→ 不添加
    await act(async () => {
      await handler({
        payload: { type: "drop", paths: ["/tmp/b.pdf"], position: { x: 10, y: 10 } },
      });
    });
    expect(screen.queryByText(/b\.pdf/)).not.toBeInTheDocument();
  });

  // ───────── 任务执行聊天化：chat-open-session 跳转/排队 ─────────

  it("chat-open-session 非 busy：直接切换到执行会话并加载其历史", async () => {
    // 前面的用例会把 listenMock 恢复成不记录的默认实现，这里显式重设
    mocks.listenMock.mockImplementation(
      async (event: string, cb: (e: { payload: Record<string, unknown> }) => void) => {
        (mocks.listeners[event] ??= []).push(cb);
        return () => {};
      }
    );
    mocks.invokeMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load")
        return args?.sessionId === "s-exec"
          ? [{ role: "user", content: "[任务卡执行]\nid=t1\n标题：写报告", refsJson: null, thinking: null, toolsJson: null }]
          : [];
      return null;
    });
    render(<ChatPanel {...defaultProps} />);
    expect(await screen.findByText("🤖 默认会话")).toBeInTheDocument();
    await act(async () => {
      for (const cb of mocks.listeners["chat-open-session"] ?? []) {
        cb({ payload: { sessionId: "s-exec", taskId: "t1", title: "📋 任务：写报告", origin: "manual" } });
      }
    });
    // 切到执行会话：加载其历史，任务块渲染出来
    await waitFor(() =>
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_history_load", { sessionId: "s-exec" })
    );
    expect(await screen.findByText(/\[任务卡执行\]/)).toBeInTheDocument();
    // 会话列表已含新会话
    expect(screen.getByText(/📋 任务：写报告/)).toBeInTheDocument();
  });

  it("chat-open-session busy：跳转排队，当前轮结束后出现「查看执行对话」按钮，点击切换", async () => {
    mocks.listenMock.mockImplementation(
      async (event: string, cb: (e: { payload: Record<string, unknown> }) => void) => {
        (mocks.listeners[event] ??= []).push(cb);
        return () => {};
      }
    );
    const user = userEvent.setup();
    let releaseChat: (v: { text: string; taskRefs: never[] }) => void = () => {};
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
    expect(await screen.findByText("🤖 默认会话")).toBeInTheDocument();
    const input = screen.getByPlaceholderText(/和机器人说点什么/);
    await user.type(input, "你好");
    await user.keyboard("{Enter}");
    // busy 中收到 chat-open-session → 不切换、不读执行会话历史
    await act(async () => {
      for (const cb of mocks.listeners["chat-open-session"] ?? []) {
        cb({ payload: { sessionId: "s-exec", taskId: "t1", title: "📋 任务：写报告", origin: "scheduled" } });
      }
    });
    expect(screen.getByText("🤖 默认会话")).toBeInTheDocument();
    expect(mocks.invokeMock).not.toHaveBeenCalledWith("bot_history_load", { sessionId: "s-exec" });
    // 当前轮结束 → 排队跳转兑现为提示 + 按钮
    await act(async () => {
      releaseChat({ text: "回复", taskRefs: [] });
    });
    expect(await screen.findByText(/已在新会话执行/)).toBeInTheDocument();
    const btn = await screen.findByText("💬 查看执行对话");
    await user.click(btn);
    await waitFor(() =>
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_history_load", { sessionId: "s-exec" })
    );
  });
});
