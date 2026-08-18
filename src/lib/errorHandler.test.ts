// P0-6B（2026-08-18）：hint 与后端 is_recoverable() 对齐 + recoverable 驱动重试 UI
import { describe, it, expect, vi, beforeEach } from "vitest";
import { handleCommandError } from "./errorHandler";

/** 构造一个 CommandErrorPayload 形状的对象 */
const ce = (code: string, recoverable: boolean, message = "出错了") => ({
  code,
  message,
  recoverable,
});

describe("handleCommandError hint 文案（P0-6B）", () => {
  let alertSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    alertSpy = vi.spyOn(window, "alert").mockImplementation(() => {});
  });

  it("KEYRING_ERROR hint 不再误导「可重试」", () => {
    handleCommandError(ce("KEYRING_ERROR", false));
    expect(alertSpy).toHaveBeenCalledTimes(1);
    const text = alertSpy.mock.calls[0][0] as string;
    expect(text).not.toContain("可重试");
    expect(text).toContain("不可自动重试");
    expect(text).toContain("凭据存储");
  });

  it("UNKNOWN_TOOL / DB_ERROR / IO_ERROR / INTERNAL hint 均不再误导「可重试」", () => {
    for (const code of ["UNKNOWN_TOOL", "DB_ERROR", "IO_ERROR", "INTERNAL"]) {
      alertSpy.mockClear();
      handleCommandError(ce(code, false));
      expect(alertSpy).toHaveBeenCalledTimes(1);
      const text = alertSpy.mock.calls[0][0] as string;
      expect(text).not.toContain("可重试");
      expect(text).toContain("不可自动重试");
    }
  });
});

describe("handleCommandError recoverable 驱动重试 UI（P0-6B）", () => {
  let alertSpy: ReturnType<typeof vi.spyOn>;
  let confirmSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    alertSpy = vi.spyOn(window, "alert").mockImplementation(() => {});
    confirmSpy = vi.spyOn(window, "confirm").mockImplementation(() => true);
  });

  it("recoverable=true + 传了 onRetry + 用户确认 → 调用 onRetry", () => {
    const onRetry = vi.fn();
    handleCommandError(ce("BOT_DISABLED", true), "test", { onRetry });
    expect(confirmSpy).toHaveBeenCalledTimes(1);
    expect(onRetry).toHaveBeenCalledTimes(1);
    expect(alertSpy).not.toHaveBeenCalled();
  });

  it("recoverable=true 但用户在 confirm 取消 → 不调用 onRetry", () => {
    const onRetry = vi.fn();
    confirmSpy.mockImplementation(() => false);
    handleCommandError(ce("API_KEY_MISSING", true), "test", { onRetry });
    expect(confirmSpy).toHaveBeenCalledTimes(1);
    expect(onRetry).not.toHaveBeenCalled();
  });

  it("recoverable=false → 不弹 confirm、不调用 onRetry（只 alert 引导反馈日志）", () => {
    const onRetry = vi.fn();
    handleCommandError(ce("DB_ERROR", false), "test", { onRetry });
    expect(confirmSpy).not.toHaveBeenCalled();
    expect(onRetry).not.toHaveBeenCalled();
    expect(alertSpy).toHaveBeenCalledTimes(1);
  });

  it("recoverable=true 但未传 onRetry → 普通 alert（无 confirm）", () => {
    handleCommandError(ce("BOT_DISABLED", true), "test");
    expect(confirmSpy).not.toHaveBeenCalled();
    expect(alertSpy).toHaveBeenCalledTimes(1);
  });

  it("silent=true 时不弹任何对话框、不调用 onRetry", () => {
    const onRetry = vi.fn();
    handleCommandError(ce("BOT_DISABLED", true), "test", {
      silent: true,
      onRetry,
    });
    expect(confirmSpy).not.toHaveBeenCalled();
    expect(alertSpy).not.toHaveBeenCalled();
    expect(onRetry).not.toHaveBeenCalled();
  });
});
