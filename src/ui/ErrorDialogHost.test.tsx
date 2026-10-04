// ErrorDialogHost vitest —— 批 4：应用内错误弹窗接管 + 原生兜底双路径。
//
// 接管路径：mount Host 后 handleCommandError 发事件 → Host preventDefault
// 接管 → nm 卡片渲染（图标由 Host 提供），点重试/取消触发 resolve 回调。
// 兜底路径：不挂 Host → errorHandler 走原生 alert/confirm 旧格式（行为不变，
// 存量 errorHandler.test.ts 锁的就是这条路径）。

import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { ErrorDialogHost } from "./ErrorDialogHost";
import {
  handleCommandError,
  isCommandError,
  type CommandErrorPayload,
} from "../lib/errorHandler";

function ce(code: string, recoverable: boolean, message = "测试错误"): CommandErrorPayload {
  return { code, message, recoverable };
}

beforeEach(() => {
  vi.restoreAllMocks();
});

describe("ErrorDialogHost 接管路径（Host 已挂载）", () => {
  it("不可恢复错误：渲染图标化卡片（message + hint 行 + 确定），无重试键", async () => {
    render(<ErrorDialogHost />);
    handleCommandError(ce("API_KEY_MISSING", false, "缺少 API Key"));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog.textContent).toContain("缺少 API Key");
    expect(dialog.textContent).toContain("请先到设置页填写 API Key");
    // 图标化口径：不再是 emoji 文案
    expect(dialog.textContent).not.toContain("💡");
    expect(dialog.textContent).not.toContain("❌");
    expect(screen.getByTestId("error-dialog-primary").textContent).toBe("确定");
    expect(screen.queryByText("取消")).toBeNull();
  });

  it("可恢复错误 + onRetry：点重试 → onRetry 恰好一次、弹窗关闭", async () => {
    const onRetry = vi.fn();
    render(<ErrorDialogHost />);
    handleCommandError(ce("HTTP_START_FAILED", true, "端口被占用"), "test", {
      onRetry,
    });
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog.textContent).toContain("出错了，要重试吗？");
    expect(screen.getByText("取消")).toBeTruthy();
    const user = userEvent.setup();
    await user.click(screen.getByTestId("error-dialog-primary"));
    expect(onRetry).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });

  it("可恢复错误：点取消 → onRetry 不被调用", async () => {
    const onRetry = vi.fn();
    render(<ErrorDialogHost />);
    handleCommandError(ce("LLM_REQUEST_FAILED", true, "请求失败"), "test", {
      onRetry,
    });
    await screen.findByRole("alertdialog");
    const user = userEvent.setup();
    await user.click(screen.getByText("取消"));
    expect(onRetry).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });

  it("onRetry 同步抛错 → 回收进 errorHandler（silent，不二次弹窗）", async () => {
    const consoleSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const onRetry = () => {
      throw isCommandError(ce("DB_ERROR", false)) ? ce("DB_ERROR", false) : new Error("boom");
    };
    render(<ErrorDialogHost />);
    handleCommandError(ce("HTTP_START_FAILED", true, "端口被占用"), "test", {
      onRetry,
    });
    await screen.findByRole("alertdialog");
    // 重试抛错被回收后 silent：弹窗链终止（无新弹窗），console 留痕
    fireEvent.click(screen.getByTestId("error-dialog-primary"));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(
      consoleSpy.mock.calls.some((c) => String(c[0]).includes("DB_ERROR"))
    ).toBe(true);
    consoleSpy.mockRestore();
  });
});

describe("兜底路径（未挂 Host）", () => {
  it("无 Host → 原生 alert 旧 emoji 格式（行为不变，errorHandler.test 同口径）", () => {
    const alertSpy = vi.spyOn(window, "alert").mockImplementation(() => {});
    // 本测试文件不渲染 Host——但同文件上方用例已挂过 Host？不会：每个用例
    // 独立 render/unmount，这里确保无 Host 残留后再断言兜底
    handleCommandError(ce("DB_ERROR", false, "数据库坏了"));
    expect(alertSpy).toHaveBeenCalledTimes(1);
    const text = alertSpy.mock.calls[0][0] as string;
    expect(text).toContain("❌ 数据库坏了");
    expect(text).toContain("💡");
    alertSpy.mockRestore();
  });
});
