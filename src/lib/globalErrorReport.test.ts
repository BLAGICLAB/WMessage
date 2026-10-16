import { describe, expect, it, vi, beforeEach } from "vitest";

// 模块态（reports 计数 / installed 标志）——每测试 resetModules + 动态 import 隔离；
// 监听目标注入全新 EventTarget：resetModules 后旧模块的监听器仍挂在共享 window
// 上（reset不掉），注入式目标是唯一的零泄漏隔离方式
const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

const importMod = () => import("./globalErrorReport");

function resetAll() {
  vi.resetModules();
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(null);
}

/** 派发辅助：错误事件（带定位）；资源类（无 message）走 message: "" */
function fireError(target: EventTarget, message: string, at = "app.js:3:7") {
  const [filename, lineno, colno] = at.split(":");
  target.dispatchEvent(
    new ErrorEvent("error", {
      message,
      filename,
      lineno: Number(lineno),
      colno: Number(colno),
    }),
  );
}

describe("detailFromReason", () => {
  beforeEach(resetAll);

  it("Error 带 stack → 消息 + 首帧拼单行；无 stack → message", async () => {
    const { detailFromReason } = await importMod();
    const err = new Error("boom");
    err.stack = "Error: boom\n    at fn (a.js:1:2)\n    at caller (b.js:3:4)";
    // 单行拼接：日志行内不落原始换行（Rust 侧还得转义，客户端一步到位）
    expect(detailFromReason(err)).toBe("Error: boom     at fn (a.js:1:2)");
    const noStack = new Error("plain");
    noStack.stack = "";
    expect(detailFromReason(noStack)).toBe("plain");
  });

  it("非 Error reason → String 化", async () => {
    const { detailFromReason } = await importMod();
    expect(detailFromReason("直接字符串")).toBe("直接字符串");
    expect(detailFromReason(42)).toBe("42");
  });
});

describe("installGlobalErrorReport", () => {
  beforeEach(resetAll);

  it("error 事件带 message → 上报 js_error（文件:行:列 消息）", async () => {
    const { installGlobalErrorReport } = await importMod();
    const target = new EventTarget();
    installGlobalErrorReport(target);
    fireError(target, "boom");
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith("frontend_event_report", {
      event: "js_error",
      detail: "app.js:3:7 boom",
    });
  });

  it("无 message 的 error（资源加载失败）不上报", async () => {
    const { installGlobalErrorReport } = await importMod();
    const target = new EventTarget();
    installGlobalErrorReport(target);
    target.dispatchEvent(new ErrorEvent("error", { message: "" }));
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("unhandledrejection → 上报 js_unhandled_rejection（Error 取消息+首帧单行）", async () => {
    const { installGlobalErrorReport } = await importMod();
    const target = new EventTarget();
    installGlobalErrorReport(target);
    const err = new Error("async boom");
    err.stack = "Error: async boom\n    at task (t.js:9:1)";
    target.dispatchEvent(
      new PromiseRejectionEvent("unhandledrejection", {
        reason: err,
        promise: Promise.resolve(),
      }),
    );
    expect(invokeMock).toHaveBeenCalledWith("frontend_event_report", {
      event: "js_unhandled_rejection",
      detail: "Error: async boom     at task (t.js:9:1)",
    });
  });

  it("每会话 20 条上限：第 21 条不发送", async () => {
    const { installGlobalErrorReport } = await importMod();
    const target = new EventTarget();
    installGlobalErrorReport(target);
    for (let i = 0; i < 25; i++) {
      fireError(target, `unique-${i}`, "a.js:1:1");
    }
    expect(invokeMock).toHaveBeenCalledTimes(20);
  });

  it("重复安装幂等：一次派发只发一条（StrictMode 双挂载防线）", async () => {
    const { installGlobalErrorReport } = await importMod();
    const target = new EventTarget();
    installGlobalErrorReport(target);
    installGlobalErrorReport(target);
    fireError(target, "once");
    expect(invokeMock).toHaveBeenCalledTimes(1);
  });
});
