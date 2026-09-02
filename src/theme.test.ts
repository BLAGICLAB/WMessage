import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  getSetting,
  effectiveTheme,
  applySetting,
  cycleSetting,
  toggleTheme,
  subscribeTheme,
  subscribeSystem,
} from "./theme";

const THEME_KEY = "wmessage-theme";

/** 覆盖 setup.ts 的默认 matchMedia（默认 matches=false），指定系统深浅色 */
const mockSystemDark = (dark: boolean) => {
  vi.spyOn(window, "matchMedia").mockImplementation(
    (query: string) =>
      ({
        matches: dark,
        media: query,
        onchange: null,
        addListener: vi.fn(),
        removeListener: vi.fn(),
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        dispatchEvent: vi.fn(),
      }) as unknown as MediaQueryList
  );
};

beforeEach(() => {
  localStorage.clear();
  document.documentElement.classList.remove("dark");
});

// 第二梯队 #8（2026-09-03）：主题三态切换/解析纯函数此前零覆盖。
describe("theme 设置读写与解析", () => {
  it("getSetting：无存储默认 light；dark/system 读回；非法值归一 light", () => {
    expect(getSetting()).toBe("light");
    localStorage.setItem(THEME_KEY, "dark");
    expect(getSetting()).toBe("dark");
    localStorage.setItem(THEME_KEY, "system");
    expect(getSetting()).toBe("system");
    localStorage.setItem(THEME_KEY, "weird");
    expect(getSetting()).toBe("light");
  });

  it("getSetting：localStorage 抛错（隐私模式等）兜底 light", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("denied");
    });
    expect(getSetting()).toBe("light");
  });

  it("effectiveTheme：light/dark 直返；system 跟随 matchMedia", () => {
    expect(effectiveTheme("light")).toBe("light");
    expect(effectiveTheme("dark")).toBe("dark");
    mockSystemDark(true);
    expect(effectiveTheme("system")).toBe("dark");
    mockSystemDark(false);
    expect(effectiveTheme("system")).toBe("light");
  });

  it("effectiveTheme：matchMedia 不存在（旧 WebView）时 system 兜底 light", () => {
    const orig = window.matchMedia;
    // @ts-expect-error 模拟无 matchMedia 的环境
    window.matchMedia = undefined;
    try {
      expect(effectiveTheme("system")).toBe("light");
    } finally {
      window.matchMedia = orig;
    }
  });

  it("applySetting：持久化到 localStorage 并切换 html.dark class", () => {
    applySetting("dark");
    expect(localStorage.getItem(THEME_KEY)).toBe("dark");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    applySetting("light");
    expect(document.documentElement.classList.contains("dark")).toBe(false);
  });
});

describe("theme 切换", () => {
  it("cycleSetting：light → dark → system → light 三态循环，每步持久化", () => {
    expect(cycleSetting()).toBe("dark");
    expect(localStorage.getItem(THEME_KEY)).toBe("dark");
    expect(cycleSetting()).toBe("system");
    expect(localStorage.getItem(THEME_KEY)).toBe("system");
    expect(cycleSetting()).toBe("light");
    expect(localStorage.getItem(THEME_KEY)).toBe("light");
  });

  it("toggleTheme：light ↔ dark 来回切", () => {
    expect(toggleTheme()).toBe("dark");
    expect(toggleTheme()).toBe("light");
  });

  it("toggleTheme：system 态按当前生效值取反，落到具体模式", () => {
    localStorage.setItem(THEME_KEY, "system");
    mockSystemDark(true); // 系统深色 → 生效 dark → 取反落 light
    expect(toggleTheme()).toBe("light");
    localStorage.setItem(THEME_KEY, "system");
    mockSystemDark(false);
    expect(toggleTheme()).toBe("dark");
  });
});

describe("theme 订阅", () => {
  it("subscribeTheme：storage 事件同步其他窗口变更；无关 key 忽略；非法值归一 light；退订生效", () => {
    mockSystemDark(false);
    const cb = vi.fn();
    const off = subscribeTheme(cb);
    window.dispatchEvent(
      new StorageEvent("storage", { key: THEME_KEY, newValue: "dark" })
    );
    expect(cb).toHaveBeenCalledWith("dark");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    // 无关 key 不触发
    window.dispatchEvent(
      new StorageEvent("storage", { key: "other", newValue: "system" })
    );
    expect(cb).toHaveBeenCalledTimes(1);
    // 非法 newValue 归一 light，class 同步撤掉
    window.dispatchEvent(
      new StorageEvent("storage", { key: THEME_KEY, newValue: "weird" })
    );
    expect(cb).toHaveBeenLastCalledWith("light");
    expect(document.documentElement.classList.contains("dark")).toBe(false);
    // 退订后不再回调
    off();
    window.dispatchEvent(
      new StorageEvent("storage", { key: THEME_KEY, newValue: "dark" })
    );
    expect(cb).toHaveBeenCalledTimes(2);
  });

  it("subscribeSystem：system 设置下系统外观变化触发回调并套用 class；非 system 不回调", () => {
    let handler: (() => void) | null = null;
    const mql = {
      matches: true,
      media: "(prefers-color-scheme: dark)",
      onchange: null,
      addListener: vi.fn(),
      removeListener: vi.fn(),
      addEventListener: vi.fn((_e: string, f: () => void) => {
        handler = f;
      }),
      removeEventListener: vi.fn(),
      dispatchEvent: vi.fn(),
    };
    vi.spyOn(window, "matchMedia").mockReturnValue(
      mql as unknown as MediaQueryList
    );

    // 非 system：系统变化不回调
    localStorage.setItem(THEME_KEY, "light");
    const cb1 = vi.fn();
    subscribeSystem(cb1);
    handler!();
    expect(cb1).not.toHaveBeenCalled();

    // system：回调 + 套用 dark class；退订走 removeEventListener
    localStorage.setItem(THEME_KEY, "system");
    const cb2 = vi.fn();
    const off = subscribeSystem(cb2);
    handler!();
    expect(cb2).toHaveBeenCalledWith("system");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    off();
    expect(mql.removeEventListener).toHaveBeenCalledWith("change", handler);
  });

  it("subscribeSystem 旧 WebView 分支：无 addEventListener 退回 addListener，清理调 removeListener", () => {
    const mql = {
      matches: false,
      media: "(prefers-color-scheme: dark)",
      onchange: null,
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
      // 故意不提供 addEventListener/removeEventListener
    };
    vi.spyOn(window, "matchMedia").mockReturnValue(
      mql as unknown as MediaQueryList
    );
    localStorage.setItem(THEME_KEY, "system");
    const off = subscribeSystem(vi.fn());
    expect(mql.addListener).toHaveBeenCalled();
    off();
    expect(mql.removeListener).toHaveBeenCalled();
  });
});
