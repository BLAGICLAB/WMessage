import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { DiagnosticsCard } from "./DiagnosticsCard";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

const clipboardMock = vi.hoisted(() => vi.fn(async () => undefined));

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue("（暂无日志）");
  clipboardMock.mockClear();
  // jsdom 的 navigator.clipboard 是 getter-only，必须 defineProperty 覆盖
  Object.defineProperty(navigator, "clipboard", {
    value: { writeText: clipboardMock },
    configurable: true,
  });
});

describe("DiagnosticsCard 诊断卡", () => {
  it("error_log_read 返回错误行 → 倒序展示", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "error_log_read") {
        return "[2026-10-16 10:00:00.1] ERROR | panic | msg=x\n[2026-10-16 09:00:00.0] WARN | degraded";
      }
      if (cmd === "logs_bundle_text") return "bundle";
      return null;
    });
    render(<DiagnosticsCard />);
    const pre = await screen.findByText(/ERROR \| panic/);
    // 最新在前：panic 行先于 WARN 行出现
    expect(pre.textContent).toMatch(/^.*ERROR \| panic[\s\S]*WARN \| degraded$/);
    expect(screen.queryByText("暂无错误记录")).not.toBeInTheDocument();
  });

  it("占位文案 → 「暂无错误记录」空态", async () => {
    render(<DiagnosticsCard />);
    await screen.findByText("暂无错误记录");
  });

  it("复制全部日志 → 取打包文本写剪贴板，按钮变「已复制」", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "logs_bundle_text") return "===== error.log =====\nALL";
      return "（暂无日志）";
    });
    const user = userEvent.setup();
    // userEvent.setup() 会安装自家剪贴板 stub 覆盖 beforeEach 的注入——setup 后再注入
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: clipboardMock },
      configurable: true,
    });
    render(<DiagnosticsCard />);
    await screen.findByText("暂无错误记录");
    await user.click(screen.getByRole("button", { name: /复制全部日志/ }));
    await waitFor(() => {
      expect(clipboardMock).toHaveBeenCalledWith("===== error.log =====\nALL");
    });
    expect(screen.getByText("已复制")).toBeInTheDocument();
  });

  it("读取失败 → 行内错误提示，不炸卡片", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "error_log_read") throw new Error("disk gone");
      return "（暂无日志）";
    });
    render(<DiagnosticsCard />);
    await screen.findByText(/disk gone/);
  });
});
