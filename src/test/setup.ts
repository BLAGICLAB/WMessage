import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach, vi } from "vitest";
import { cleanup } from "@testing-library/react";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  // errorHandler 的降级分支用例会显式 delete __TAURI_INTERNALS__（模拟纯浏览器），
  // 删除会残留到同文件后续用例——这里复位基线，保证每个用例都从 Tauri 宿主起步
  if (typeof window !== "undefined") {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  }
});

// 模拟 Tauri 宿主：产品运行环境是 Tauri webview（__TAURI_INTERNALS__ 恒在），
// errorHandler 的弹窗降级只应在纯浏览器触发——测试环境统一按宿主模拟，
// 降级分支由 errorHandler.test 显式 delete 后单测（afterEach 统一复位，见上）
if (typeof window !== "undefined") {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
}

// React 19 + @testing-library：子组件异步状态更新（如 ActorAvatar useProfile
// 的 loadProfile promise）会产生未包装 act 的警告；测试结果不受影响，仅净化日志。
// 真身在模块加载期捕获一次（spy 恢复与否不影响）；正则须带 act( 括号防误吞。
const realConsoleError = console.error;
beforeEach(() => {
  vi.spyOn(console, "error").mockImplementation((...args: unknown[]) => {
    const msg = typeof args[0] === "string" ? args[0] : "";
    if (/not wrapped in act\(/.test(msg)) return;
    realConsoleError(...args);
  });
});

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

// Element.scrollTo / scrollIntoView 的 jsdom noop 已按文件 scope 收进
// src/test/scrollNoop.ts（渲染 ChatPanel 的测试文件各自引入，文件末清掉）——
// 不在全局 setup 里常驻原型补丁（原型链安全规则）；文件内也不按用例摘除：
// 摘掉到重装之间有空档，passive effect 撞上就是随机某个用例的红。

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

// jsdom 没有 WebGL/WebGL2 上下文。Three.js 在 WebGLRenderer 构造时才探测
// 能力，而 GraphPage.test 已整块 mock 掉 GraphCanvas、HomePage 的速览卡走
// 2D SVG——真正需要上下文的路径在 jsdom 里从没跑到。补两个空构造函数只是
// 让「有没有 WebGL2」这类能力探测有个确定答案（返回未定义），不改任何渲染行为。
if (typeof globalThis.WebGL2RenderingContext === "undefined") {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).WebGL2RenderingContext =
    function WebGL2RenderingContext() {};
}
if (typeof globalThis.WebGLRenderingContext === "undefined") {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).WebGLRenderingContext =
    function WebGLRenderingContext() {};
}

// Node 26 自带 `localStorage` 全局，未传 `--localstorage-file` 时其值为 undefined，
// 会在 jsdom 环境里遮蔽 jsdom 的实现；theme 等测试依赖它
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
