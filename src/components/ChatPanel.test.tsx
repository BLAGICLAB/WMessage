import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
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
});
