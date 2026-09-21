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

// Node 26 自带 `localStorage` 全局，未传 `--localstorage-file` 时其值为 undefined，
// 会在 jsdom 环境里遮蔽 jsdom 的实现；theme / WidgetApp 等测试依赖它
// （beforeEach 里 `localStorage.clear()`）。这里按需补一个内存实现，
// 覆盖 Web Storage 的最小可用子集。
if (typeof globalThis.localStorage === "undefined") {
  const store = new Map<string, string>();
  const localStorageShim: Storage = {
    get length() {
      return store.size;
    },
    clear: () => store.clear(),
    getItem: (key: string) => (store.has(key) ? store.get(key)! : null),
    key: (index: number) => Array.from(store.keys())[index] ?? null,
    removeItem: (key: string) => void store.delete(key),
    setItem: (key: string, value: string) => void store.set(key, String(value)),
  };
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    writable: true,
    value: localStorageShim,
  });
}
