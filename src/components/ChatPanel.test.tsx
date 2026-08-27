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
  return { invokeMock, listenMock, emitMock };
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
};

beforeEach(() => {
  mocks.invokeMock.mockClear();
  mocks.listenMock.mockClear();
  writeTextMock.mockClear();
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
    // P1-8（2026-08-27 审计）：/stop 按会话停止——携带当前会话 id
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

  it("模型快速切换：🧠 按钮开菜单，点 Kimi K3 → bot_set_config 回写（保留白名单/Tavily）且标签更新", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_get_config")
        return {
          baseUrl: "https://api.minimaxi.com/v1",
          model: "MiniMax-M3",
          hasApiKey: true,
          bypassLlmOnPreStepHit: true,
          allowedDirs: ["/tmp/x"],
          tavilyKey: "tvly-test",
        };
      if (cmd === "bot_set_config") return null;
      return null;
    });
    render(<ChatPanel {...defaultProps} />);
    // 头部按钮显示当前提供商（baseUrl + model 双匹配命中预设 label）
    const btn = await screen.findByText(/🧠 MiniMax/);
    await user.click(btn);
    // 菜单出现，点 Kimi K3
    await user.click(screen.getByText("Kimi K3"));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_set_config",
        expect.objectContaining({
          apiKey: null, // 不动 keychain
          config: expect.objectContaining({
            baseUrl: "https://api.moonshot.cn/v1",
            model: "kimi-k3",
            allowedDirs: ["/tmp/x"], // 保留既有字段
            tavilyKey: "tvly-test",
          }),
        })
      );
    });
    // 标签更新 + 本地提示
    expect(await screen.findByText(/🧠 Kimi K3/)).toBeInTheDocument();
    expect(await screen.findByText(/✅ 已切换到 Kimi K3/)).toBeInTheDocument();
  });

  it("模型菜单自定义：填 Base URL + 模型名 → 回写自定义配置，头部显示模型名", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_sessions_load") return [{ id: "s1", title: "默认会话" }];
      if (cmd === "bot_history_load") return [];
      if (cmd === "bot_get_config")
        return {
          baseUrl: "https://api.minimaxi.com/v1",
          model: "MiniMax-M3",
          hasApiKey: true,
        };
      if (cmd === "bot_set_config") return null;
      return null;
    });
    render(<ChatPanel {...defaultProps} />);
    await user.click(await screen.findByText(/🧠 MiniMax/));
    // 展开自定义表单（预填当前配置）
    await user.click(screen.getByText("✏️ 自定义…"));
    const baseInput = await screen.findByPlaceholderText(/Base URL/);
    const modelInput = screen.getByPlaceholderText(/模型名/);
    await user.clear(baseInput);
    await user.type(baseInput, "https://api.example.com/v1");
    await user.clear(modelInput);
    await user.type(modelInput, "my-model");
    await user.click(screen.getByText("使用此模型"));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_set_config",
        expect.objectContaining({
          apiKey: null,
          config: expect.objectContaining({
            baseUrl: "https://api.example.com/v1",
            model: "my-model",
          }),
        })
      );
    });
    // 自定义配置命中不了预设 → 头部显示模型名
    expect(await screen.findByText(/🧠 my-model/)).toBeInTheDocument();
  });
});
