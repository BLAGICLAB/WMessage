import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { LogsPanel } from "./LogsPanel";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

const clipboardMock = vi.hoisted(() => vi.fn(async () => undefined));

const installClipboard = () =>
  Object.defineProperty(navigator, "clipboard", {
    value: { writeText: clipboardMock },
    configurable: true,
  });

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue("（暂无日志）");
  clipboardMock.mockClear();
  installClipboard();
});

describe("LogsPanel 日志模块", () => {
  it("默认读错误日志，倒序展示返回的行", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "error_log_read") {
        return "[2026-10-16 10:00:00.1] ERROR | panic | msg=x\n[2026-10-16 09:00:00.0] WARN | degraded";
      }
      if (cmd === "logs_bundle_text") return "bundle";
      return null;
    });
    render(<LogsPanel />);
    const pre = await screen.findByText(/ERROR \| panic/);
    // 最新在前：panic 行先于 WARN 行出现
    expect(pre.textContent).toMatch(/^.*ERROR \| panic[\s\S]*WARN \| degraded$/);
    expect(screen.queryByText("暂无记录")).not.toBeInTheDocument();
  });

  it("占位文案 → 「暂无记录」空态", async () => {
    render(<LogsPanel />);
    await screen.findByText("暂无记录");
  });

  it("切换来源读对应命令：机器人审计 / 文件迁移", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_log_read") return "bot-line";
      if (cmd === "migration_log_read") return "migration-line";
      return "（暂无日志）";
    });
    const user = userEvent.setup();
    render(<LogsPanel />);
    await screen.findByText("暂无记录");

    await user.click(screen.getByRole("radio", { name: "机器人审计" }));
    expect(await screen.findByText("bot-line")).toBeInTheDocument();
    expect(invokeMock).toHaveBeenCalledWith("bot_log_read", { limit: 200 });

    await user.click(screen.getByRole("radio", { name: "文件迁移" }));
    expect(await screen.findByText("migration-line")).toBeInTheDocument();
    expect(invokeMock).toHaveBeenCalledWith("migration_log_read", {
      limit: 200,
    });
  });

  it("复制全部日志 → 取打包文本写剪贴板，按钮变「已复制」", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "logs_bundle_text") return "===== error.log =====\nALL";
      return "（暂无日志）";
    });
    const user = userEvent.setup();
    // userEvent.setup() 会安装自家剪贴板 stub 覆盖 beforeEach 的注入——setup 后再注入
    installClipboard();
    render(<LogsPanel />);
    await screen.findByText("暂无记录");
    await user.click(screen.getByRole("button", { name: /复制全部/ }));
    await waitFor(() => {
      expect(clipboardMock).toHaveBeenCalledWith("===== error.log =====\nALL");
    });
    expect(screen.getByText("已复制")).toBeInTheDocument();
  });

  it("读取失败 → 行内错误提示，不炸面板", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "error_log_read") throw new Error("disk gone");
      return "（暂无日志）";
    });
    render(<LogsPanel />);
    await screen.findByText(/disk gone/);
  });
});
