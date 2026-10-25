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
  it("记忆权限双开关：点选即时落盘 bot_set_config.memoryControl；缺字段默认全开", async () => {
    const user = userEvent.setup();
    const stored: Array<Record<string, unknown>> = [];
    // 有状态 mock：bot_set_config 存下的 memoryControl 在 bot_get_config 回读
    //（真实后端语义——saveConfig 完成后会 loadConfig 刷新界面）
    let savedCtrl: Record<string, unknown> | null = null;
    mocks.invokeMock.mockImplementation(
      async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "bot_get_enabled") return true;
        if (cmd === "bot_get_config") {
          return {
            hasApiKey: false,
            bypassLlmOnPreStepHit: true,
            // 初始不带 memoryControl → 老配置语义，前端按全开显示
            ...(savedCtrl ? { memoryControl: savedCtrl } : {}),
          };
        }
        if (cmd === "bot_set_config") {
          const cfg = args!.config as {
            memoryControl?: Record<string, unknown>;
          };
          savedCtrl = cfg.memoryControl ?? null;
          stored.push(cfg);
          return null;
        }
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
        return null;
      },
    );
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "记忆" }));
    // 记忆权限卡：双开关缺字段默认开启（role=switch + aria-checked，同厂商总开关语义）
    expect(screen.getByText("记忆权限")).toBeInTheDocument();
    const injectBtn = screen.getByRole("switch", { name: "聊天注入记忆" });
    const autoBtn = screen.getByRole("switch", { name: "模型自动记忆" });
    expect(injectBtn).toHaveAttribute("aria-checked", "true");
    expect(autoBtn).toHaveAttribute("aria-checked", "true");
    // 关「聊天注入记忆」→ 即时落盘
    await user.click(injectBtn);
    await waitFor(() => expect(stored.length).toBeGreaterThan(0));
    const first = stored[0] as {
      memoryControl: { injectionEnabled: boolean; autoWriteEnabled: boolean };
    };
    expect(first.memoryControl).toEqual({
      injectionEnabled: false,
      autoWriteEnabled: true,
      autoExtract: "off",
    });
    // 回读后开关态持久（有状态 mock 保存了保存值）
    await waitFor(() =>
      expect(
        screen.getByRole("switch", { name: "聊天注入记忆" }),
      ).toHaveAttribute("aria-checked", "false"),
    );
    // 再关「模型自动记忆」→ 双关落盘
    await user.click(screen.getByRole("switch", { name: "模型自动记忆" }));
    await waitFor(() => {
      const last = stored[stored.length - 1] as {
        memoryControl: { injectionEnabled: boolean; autoWriteEnabled: boolean };
      };
      expect(last.memoryControl).toEqual({
        injectionEnabled: false,
        autoWriteEnabled: false,
        autoExtract: "off",
      });
    });
  });

  it("U16 自动记忆抽取三档：点选即时落盘 autoExtract；缺字段默认关闭", async () => {
    const user = userEvent.setup();
    const stored: Array<Record<string, unknown>> = [];
    let savedCtrl: Record<string, unknown> | null = null;
    mocks.invokeMock.mockImplementation(
      async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "bot_get_enabled") return true;
        if (cmd === "bot_get_config") {
          return {
            hasApiKey: false,
            bypassLlmOnPreStepHit: true,
            ...(savedCtrl ? { memoryControl: savedCtrl } : {}),
          };
        }
        if (cmd === "bot_set_config") {
          const cfg = args!.config as {
            memoryControl?: Record<string, unknown>;
          };
          savedCtrl = cfg.memoryControl ?? null;
          stored.push(cfg);
          return null;
        }
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
        return null;
      },
    );
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "记忆" }));
    // 三档选择器：radio 语义；缺字段默认「关闭」高亮
    const autoBtn = screen.getByRole("radio", {
      name: "自动记忆抽取：自动入库",
    });
    expect(autoBtn).toHaveAttribute("aria-checked", "false");
    // 切到「自动入库」→ 即时落盘
    await user.click(autoBtn);
    await waitFor(() => expect(stored.length).toBeGreaterThan(0));
    const first = stored[0] as {
      memoryControl: { autoExtract: string };
    };
    expect(first.memoryControl.autoExtract).toBe("auto");
    // 回读后高亮跟随（有状态 mock 持久化）
    await waitFor(() => expect(autoBtn.className).toContain("nm-inset"));
    // 再切「需确认」
    await user.click(
      screen.getByRole("radio", { name: "自动记忆抽取：需确认" }),
    );
    await waitFor(() => {
      const last = stored[stored.length - 1] as {
        memoryControl: { autoExtract: string };
      };
      expect(last.memoryControl.autoExtract).toBe("confirm");
    });
    // 总闸联动：关「模型自动记忆」后三档禁用（后端总闸优先，档位是静默 no-op）
    await user.click(screen.getByRole("switch", { name: "模型自动记忆" }));
    await waitFor(() =>
      expect(
        screen.getByRole("radio", { name: "自动记忆抽取：自动入库" }),
      ).toBeDisabled(),
    );
  });
});
