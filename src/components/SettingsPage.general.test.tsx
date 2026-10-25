/// <reference types="node" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
// 共享 mock 装置必须先于被测组件 import：vi.mock 的工厂在被测组件被求值
// 时才跑，那时 mock 实例必须已经就位（import 顺序反了会直接报错）。
import {
  mocks,
} from "./SettingsPage/__testMocks";
import { SettingsPage } from "./SettingsPage";
import type { ThemeSetting } from "../theme";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invokeMock,
  convertFileSrc: vi.fn((p: string) => `tauri://localhost/${p}`),
}));

// 隔离 profile 模块缓存：各测试都需要 fresh load
vi.mock("../profile", () => ({
  getProfileCache: vi.fn(() => null),
  loadProfile: vi.fn(async () => ({
    user: { name: "我", avatarId: null },
    bot: { name: "机器人", avatarId: null },
  })),
  subscribeProfile: vi.fn(() => () => {}),
  setProfileName: vi.fn(async () => ({
    user: { name: "我", avatarId: null },
    bot: { name: "机器人", avatarId: null },
  })),
  setProfileAvatar: vi.fn(async () => ({
    user: { name: "我", avatarId: null },
    bot: { name: "机器人", avatarId: null },
  })),
  removeProfileAvatar: vi.fn(async () => ({
    user: { name: "我", avatarId: null },
    bot: { name: "机器人", avatarId: null },
  })),
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: mocks.emitMock,
  listen: vi.fn(async () => () => {}),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: mocks.openMock,
  save: vi.fn(async () => null),
  ask: mocks.askMock,
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openPath: vi.fn(async () => {}),
  openUrl: mocks.openUrlMock,
}));

const confirmMock = vi.fn(() => true);
const alertMock = vi.fn();
// 模型库（Rust meta_* 命令）mock：默认 invokeMock 对未识别命令返回 null →
// fetchProviders 得 null → 既有用例走「内置预设」降级路径不受影响；
// 模型库用例用 stubMetaInvoke 覆盖 meta_* 命令返回
beforeEach(() => {
  mocks.invokeMock.mockClear();
  mocks.emitMock.mockClear();
  mocks.openMock.mockClear();
  mocks.openUrlMock.mockClear();
  mocks.askMock.mockReset();
  mocks.askMock.mockResolvedValue(true);
  confirmMock.mockClear();
  alertMock.mockClear();
  window.confirm = confirmMock;
  window.alert = alertMock;
});

const defaultProps = {
  theme: "light" as ThemeSetting,
  onThemeChange: vi.fn(),
  onExportTasks: vi.fn(async () => {}),
  onImportTasks: vi.fn(async () => {}),
};

describe("SettingsPage", () => {
  it("通用设置：开机自动启动开关 → 点调 plugin:autostart|enable / disable，状态联动", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_enabled") return true;
      // 初始关闭；enable/disable 幂等返 null
      if (cmd === "plugin:autostart|is_enabled") return false;
      if (cmd === "plugin:autostart|enable") return null;
      if (cmd === "plugin:autostart|disable") return null;
      if (cmd === "api_status")
        return { enabled: false, port: 4763, token: "" };
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarId: null },
          bot: { name: "机器人", avatarId: null },
        };
      if (cmd === "py_get_enabled" || cmd === "service_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status")
        return { rules_count: 0, poll_interval_secs: 600 };
      if (cmd === "migration_log_read") return "";
      if (cmd === "bot_get_config")
        return {
          hasApiKey: false,
          bypassLlmOnPreStepHit: true,
          modelsByProvider: { openai: [], anthropic: [] },
          activeModelId: { openai: null, anthropic: null },
        };
      return null;
    });
    render(<SettingsPage {...defaultProps} />);
    // 「开机自动启动 wmessage」是独有文案，不会和 Tavily/Brave/机器人/Python 的「已关闭」撞
    const title = await screen.findByText("开机自动启动 wmessage");
    // 向上走到外层 flex 行（title 所在 div 的 parentElement）
    const row = title.closest("div")?.parentElement;
    const toggle = row?.querySelector("button");
    expect(toggle).not.toBeNull();
    if (!toggle) return;
    // 初始文字「已关闭」（autostart 关闭）
    expect(toggle.textContent).toMatch(/已关闭/);
    // 点 → enable
    await user.click(toggle);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("plugin:autostart|enable");
    });
    // 文字翻转「已开启」
    await waitFor(() => {
      expect(toggle.textContent).toMatch(/已开启/);
    });
    // 再点 → disable
    await user.click(toggle);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("plugin:autostart|disable");
    });
    await waitFor(() => {
      expect(toggle.textContent).toMatch(/已关闭/);
    });
  });

  it("通用设置：开机自启动 enable 失败 → 显示错误，不漂状态", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "plugin:autostart|is_enabled") return false;
      if (cmd === "plugin:autostart|enable")
        throw {
          code: "AUTOSTART_FAILED",
          message: "系统拒绝写入",
          recoverable: false,
        };
      if (cmd === "api_status")
        return { enabled: false, port: 4763, token: "" };
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarId: null },
          bot: { name: "机器人", avatarId: null },
        };
      if (cmd === "py_get_enabled" || cmd === "service_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status")
        return { rules_count: 0, poll_interval_secs: 600 };
      if (cmd === "migration_log_read") return "";
      if (cmd === "bot_get_config")
        return {
          hasApiKey: false,
          bypassLlmOnPreStepHit: true,
          modelsByProvider: { openai: [], anthropic: [] },
          activeModelId: { openai: null, anthropic: null },
        };
      return null;
    });
    render(<SettingsPage {...defaultProps} />);
    const title = await screen.findByText("开机自动启动 wmessage");
    const row = title.closest("div")?.parentElement;
    const toggle = row?.querySelector("button");
    expect(toggle).not.toBeNull();
    if (!toggle) return;
    await user.click(toggle);
    // 错误可见（与 toggle 同 row 渲染）
    expect(await screen.findByText(/系统拒绝写入/)).toBeInTheDocument();
    // 状态保持为已关闭（拉取回真值仍是 false）
    expect(toggle.textContent).toMatch(/已关闭/);
  });

  it("通用设置：字体大小四档 → 点选即套 data-attr + 立即落盘（与外观点选一致，2026-09-08 老板）", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "plugin:autostart|is_enabled") return false;
      if (cmd === "api_status")
        return { enabled: false, port: 4763, token: "" };
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarId: null },
          bot: { name: "机器人", avatarId: null },
        };
      if (cmd === "py_get_enabled" || cmd === "service_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status")
        return { rules_count: 0, poll_interval_secs: 600 };
      if (cmd === "migration_log_read") return "";
      if (cmd === "bot_get_config")
        return {
          hasApiKey: false,
          bypassLlmOnPreStepHit: true,
          modelsByProvider: { openai: [], anthropic: [] },
          activeModelId: { openai: null, anthropic: null },
          // 初始 small（默认）
          uiFontSize: "small",
        };
      if (cmd === "bot_set_config") return null;
      return null;
    });
    render(<SettingsPage {...defaultProps} />);
    // 初始：small 高亮（nm-inset），4 档按钮
    // 「字体大小」p 的下一个 sibling 就是字体按钮所在 div（避免 closest 抓到整张卡的 3+4 个按钮）
    const title = await screen.findByText("字体大小");
    const row = title.nextElementSibling as HTMLElement;
    expect(row).not.toBeNull();
    if (!row) return;
    const btns = row.querySelectorAll("button");
    // 4 档：小 / 标准 / 大 / 特大
    expect(btns).toHaveLength(4);
    expect(btns[0].textContent).toBe("小");
    expect(btns[1].textContent).toBe("标准");
    expect(btns[2].textContent).toBe("大");
    expect(btns[3].textContent).toBe("特大");
    // 初始 small 高亮
    expect(btns[0].className).toContain("nm-inset");
    // 点「标准」→ data-attr 变 + active 跳到第二档 + 立即落盘（与外观一致）
    await user.click(btns[1]);
    expect(document.documentElement.dataset.fontSize).toBe("standard");
    // saveConfig 是 async fire-and-forget，触发重渲染 → waitFor 等 active class 跳位
    await waitFor(() => {
      expect(btns[1].className).toContain("nm-inset");
      expect(btns[0].className).not.toContain("nm-inset");
    });
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_set_config",
        expect.objectContaining({
          config: expect.objectContaining({
            uiFontSize: "standard",
          }),
        }),
      );
    });
    // 再点「特大」→ 同样立即落盘
    await user.click(btns[3]);
    expect(document.documentElement.dataset.fontSize).toBe("xlarge");
    await waitFor(() => {
      expect(btns[3].className).toContain("nm-inset");
    });
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_set_config",
        expect.objectContaining({
          config: expect.objectContaining({
            uiFontSize: "xlarge",
          }),
        }),
      );
    });
  });

  // Bugfix 回归：原 main.css 只覆盖 text-[10/11/12px] 三个任意值类，
  // 设置页按钮 / chat 输入框用的 text-xs 不在覆盖范围 → 切档无视觉差异。
  // 补全覆盖后必须能命中。vitest config css:false 不加载 CSS，读文件做静态断言。
  it("字体大小覆盖：main.css 含 text-xs / text-sm / text-base + input 各档规则", async () => {
    const fs = await import("node:fs/promises");
    const path = await import("node:path");
    const url = await import("node:url");
    const cssPath = path.resolve(
      path.dirname(url.fileURLToPath(import.meta.url)),
      "../ui/main.css",
    );
    const css = await fs.readFile(cssPath, "utf-8");
    // xlarge → text-xs: 16px；large → text-sm: 16px；standard → text-base: 17px
    expect(css).toMatch(
      /\[data-font-size="xlarge"\]\s+\.text-xs\s*\{\s*font-size:\s*16px/,
    );
    expect(css).toMatch(
      /\[data-font-size="large"\]\s+\.text-sm\s*\{\s*font-size:\s*16px/,
    );
    expect(css).toMatch(
      /\[data-font-size="standard"\]\s+\.text-base\s*\{\s*font-size:\s*17px/,
    );
    // input/textarea 防 macOS Safari 自动 zoom（xlarge 档 19px）
    expect(css).toMatch(
      /\[data-font-size="xlarge"\]\s+input[\s\S]*?font-size:\s*19px/,
    );
    // small 档保持不动（「目前字号为小」）
    expect(css).not.toMatch(/\[data-font-size="small"\]\s+\.text-xs/);
  });
});
