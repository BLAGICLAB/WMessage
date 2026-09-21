// hint 与后端 is_recoverable() 对齐 + recoverable 驱动重试 UI
import { describe, it, expect, vi, beforeEach } from "vitest";
import { handleCommandError, ALL_COMMAND_ERROR_CODES } from "./errorHandler";

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

describe("空 message 兜底（P2-35）", () => {  let alertSpy: ReturnType<typeof vi.spyOn>;
  let confirmSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    alertSpy = vi.spyOn(window, "alert").mockImplementation(() => {});
    confirmSpy = vi.spyOn(window, "confirm").mockImplementation(() => true);
  });

  it("CommandError message 为空 → alert 显示 code 而非空串", () => {
    handleCommandError(ce("DB_ERROR", false, ""));
    expect(alertSpy).toHaveBeenCalledTimes(1);
    const text = alertSpy.mock.calls[0][0] as string;
    expect(text).toContain("DB_ERROR");
    expect(text).not.toBe("❌ ");
  });

  it("CommandError message 与 code 均空 → 兜底「未知错误」", () => {
    handleCommandError(ce("", false, ""));
    expect(alertSpy).toHaveBeenCalledTimes(1);
    expect(alertSpy.mock.calls[0][0]).toContain("未知错误");
  });

  it("recoverable=true + 空 message + onRetry → confirm 重试 UI 同样用兜底文案", () => {
    const onRetry = vi.fn();
    handleCommandError(ce("IO_ERROR", true, ""), "test", { onRetry });
    expect(confirmSpy).toHaveBeenCalledTimes(1);
    const text = confirmSpy.mock.calls[0][0] as string;
    expect(text).toContain("IO_ERROR");
    expect(text).toContain("是否重试");
  });

  it("非结构化空 msg（null）→ alert 兜底「未知错误」，不静默跳过", () => {
    handleCommandError(null, "test");
    expect(alertSpy).toHaveBeenCalledTimes(1);
    expect(alertSpy.mock.calls[0][0]).toBe("❌ 未知错误");
  });
});

describe("hintForCode 全覆盖（每个 code 必须有专属 hint）", () => {
  let alertSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    alertSpy = vi.spyOn(window, "alert").mockImplementation(() => {});
  });

  // code 全集来自 errorHandler.ts 的 ALL_COMMAND_ERROR_CODES——它与 Rust
  // CommandErrorCode 由 tests-audit/audit_error_codes.py 跨语言对齐。
  // 后端加新 code 时这里自动跟上，本测试迫使前端同步补 hint
  //（防再次出现 TASK_INVALID_STATE 落默认分支的漏配）。
  it("ALL_COMMAND_ERROR_CODES 全部有专属 hint（💡 行），无 code 落默认分支", () => {
    expect(ALL_COMMAND_ERROR_CODES.length).toBe(24);
    for (const code of ALL_COMMAND_ERROR_CODES) {
      alertSpy.mockClear();
      handleCommandError(ce(code, false));
      expect(alertSpy).toHaveBeenCalledTimes(1);
      const text = alertSpy.mock.calls[0][0] as string;
      expect(text).toContain("💡");
    }
  });

  it("TASK_INVALID_STATE hint 说明业务状态语义", () => {
    handleCommandError(ce("TASK_INVALID_STATE", true, "任务状态不允许该操作：…"));
    const text = alertSpy.mock.calls[0][0] as string;
    expect(text).toContain("任务当前状态不允许该操作");
  });

  it("DOMAIN_RULE hint 给出「按提示调整后重试」指引", () => {
    handleCommandError(ce("DOMAIN_RULE", true, "[web] 已拒绝访问本机/内网地址"));
    const text = alertSpy.mock.calls[0][0] as string;
    expect(text).toContain("💡");
    expect(text).toContain("按提示调整后重试");
  });
});
