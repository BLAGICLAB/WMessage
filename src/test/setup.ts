import "@testing-library/jest-dom/vitest";
import { afterEach, vi } from "vitest";
import { cleanup } from "@testing-library/react";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

// React 19 + @testing-library：子组件异步状态更新（如 ActorAvatar useProfile
// 的 loadProfile promise）会产生未包装 act 的警告；测试结果不受影响，仅净化日志。
const originalError = console.error;
console.error = (...args: unknown[]) => {
  const msg = typeof args[0] === "string" ? args[0] : "";
  if (msg.includes("not wrapped in act")) return;
  originalError(...args);
};

// jsdom 不实现 matchMedia，theme.ts 的 subscribeSystem 要用到；返回基础 mock
if (typeof window !== "undefined" && !window.matchMedia) {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: vi.fn().mockImplementation((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: vi.fn(),
      removeListener: vi.fn(),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })),
  });
}

// jsdom 不实现 HTMLDivElement.scrollTo / scrollIntoView；ChatPanel 自动滚动会用到
if (typeof Element !== "undefined") {
  if (!Element.prototype.scrollTo) {
    Element.prototype.scrollTo = function () {
      // noop：测试不需要真实滚动
    };
  }
  if (!Element.prototype.scrollIntoView) {
    Element.prototype.scrollIntoView = function () {
      // noop
    };
  }
}

// crypto.randomUUID 在 jsdom 中默认存在，但部分老环境下缺失；这里兜底
if (typeof globalThis.crypto === "undefined") {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).crypto = {};
}
if (typeof globalThis.crypto.randomUUID !== "function") {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis.crypto as any).randomUUID = () =>
    `uuid-${Math.random().toString(36).slice(2)}-${Date.now()}`;
}
