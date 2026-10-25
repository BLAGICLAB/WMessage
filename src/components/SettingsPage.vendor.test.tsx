/// <reference types="node" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
// 共享 mock 装置必须先于被测组件 import：vi.mock 的工厂在被测组件被求值
// 时才跑，那时 mock 实例必须已经就位（import 顺序反了会直接报错）。
import {
  mocks,
  expectLogoSlug,
  deepseekVendorConfig,
  mockVendorConfig,
  openVendorPage,
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
  // 厂商详情页复刻改造（厂商头开关/⋯菜单、Key 显隐、获取 Key 外链、
  // 连接测试、能力徽标、ProviderLogo、updateModel 跨协议修复）

  it("厂商总开关：切换 → bot_set_config 写入 disabledVendors；禁用后模型行变淡 + 开关禁用", async () => {
    const user = userEvent.setup();
    mockVendorConfig(deepseekVendorConfig);
    render(<SettingsPage {...defaultProps} />);
    await openVendorPage(user, "DeepSeek");
    const vendorToggle = await screen.findByRole("switch", {
      name: "启用厂商 DeepSeek",
    });
    expect(vendorToggle).toHaveAttribute("aria-checked", "true");
    await user.click(vendorToggle);
    // 点击即落盘（skipReload）：payload 带 disabledVendors
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_set_config",
        expect.objectContaining({
          config: expect.objectContaining({ disabledVendors: ["DeepSeek"] }),
        }),
      );
    });
    await waitFor(() => {
      expect(
        screen.getByRole("switch", { name: "启用厂商 DeepSeek" }),
      ).toHaveAttribute("aria-checked", "false");
    });
    // 禁用后：模型行整行变淡、行内启用开关禁用
    const row = screen.getByText("DeepSeek Chat").closest("div");
    expect(row?.className).toContain("opacity-50");
    const rowToggle = row?.querySelector('[data-testid="toggle"]');
    expect(rowToggle).toBeDisabled();
  });

  it("删除厂商：确认后立即落盘（bot_set_config 不含该厂商条目），厂商从列表消失", async () => {
    const user = userEvent.setup();
    mockVendorConfig(deepseekVendorConfig);
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    render(<SettingsPage {...defaultProps} />);
    await openVendorPage(user, "DeepSeek");
    // 「⋯」菜单 → 删除厂商（确认框放行）
    await user.click(
      await screen.findByRole("button", { name: "厂商 DeepSeek 更多操作" }),
    );
    await user.click(screen.getByRole("menuitem", { name: "删除厂商" }));
    // 立即落盘：payload 双协议列表均已剔除 DeepSeek 条目
    await waitFor(() => {
      const calls = mocks.invokeMock.mock.calls.filter(
        (c) => c[0] === "bot_set_config",
      );
      expect(calls.length).toBeGreaterThan(0);
      const payload = calls[calls.length - 1][1] as {
        config: {
          modelsByProvider: { openai: unknown[]; anthropic: unknown[] };
        };
      };
      expect(payload.config.modelsByProvider.openai).toEqual([]);
      expect(payload.config.modelsByProvider.anthropic).toEqual([]);
    });
    // 厂商从左栏消失（回到引导态）
    await waitFor(() => {
      expect(
        screen.queryByRole("button", { name: "DeepSeek" }),
      ).not.toBeInTheDocument();
    });
    confirmSpy.mockRestore();
  });

  it("API Key 显隐：眼睛按钮在 password ↔ text 间往返", async () => {
    const user = userEvent.setup();
    mockVendorConfig({ ...deepseekVendorConfig, hasApiKey: false });
    render(<SettingsPage {...defaultProps} />);
    await openVendorPage(user, "DeepSeek");
    const input = await screen.findByPlaceholderText("sk-…");
    expect(input).toHaveAttribute("type", "password");
    await user.click(screen.getByRole("button", { name: "显示 API Key" }));
    expect(input).toHaveAttribute("type", "text");
    await user.click(screen.getByRole("button", { name: "隐藏 API Key" }));
    expect(input).toHaveAttribute("type", "password");
  });

  it("API Key 按厂商分存：vendorKeys 命中显示「已存入 ✓」；输入保存走 vendorKey 参数；清除调 bot_clear_vendor_key", async () => {
    const user = userEvent.setup();
    mockVendorConfig({ ...deepseekVendorConfig, vendorKeys: ["DeepSeek"] });
    render(<SettingsPage {...defaultProps} />);
    await openVendorPage(user, "DeepSeek");
    // 已存标志按厂商显示 + 清除入口
    expect(await screen.findByText(/已存入系统凭据存储/)).toBeInTheDocument();
    const input = screen.getByPlaceholderText("已保存（输入新 Key 可覆盖）");
    // 输入新 key → 保存 → vendorKey 参数携带厂商名，apiKey 旧槽位固定 null
    await user.type(input, "sk-deepseek-new");
    const saveBtns = await screen.findAllByRole("button", { name: "保存配置" });
    await user.click(saveBtns[saveBtns.length - 1]);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_set_config",
        expect.objectContaining({
          apiKey: null,
          vendorKey: { vendor: "DeepSeek", key: "sk-deepseek-new" },
        }),
      );
    });
    // 清除 → bot_clear_vendor_key（确认框全局 mock 为 true）
    await user.click(screen.getByText("清除已保存的 Key"));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_clear_vendor_key", {
        vendor: "DeepSeek",
      });
    });
  });

  it("API Key 未存的厂商：无「已存入」标志，placeholder 为 sk-…", async () => {
    const user = userEvent.setup();
    mockVendorConfig({ ...deepseekVendorConfig, vendorKeys: [] });
    render(<SettingsPage {...defaultProps} />);
    await openVendorPage(user, "DeepSeek");
    expect(await screen.findByPlaceholderText("sk-…")).toBeInTheDocument();
    expect(screen.queryByText(/已存入系统凭据存储/)).toBeNull();
    expect(screen.queryByText("清除已保存的 Key")).toBeNull();
  });

  it("API 格式切换：MiniMax 从 Anthropic 切 OpenAI → Base URL 自动换成 /v1；切回恢复记忆值", async () => {
    const user = userEvent.setup();
    mockVendorConfig({
      modelsByProvider: {
        openai: [],
        anthropic: [
          {
            id: "m1",
            label: "MiniMax-M2",
            model: "MiniMax-M2",
            baseUrl: "https://api.minimaxi.com/anthropic",
            vendor: "MiniMax",
          },
        ],
      },
      activeModelId: { openai: null, anthropic: "m1" },
      apiProvider: "anthropic",
      hasApiKey: false,
      bypassLlmOnPreStepHit: true,
    });
    render(<SettingsPage {...defaultProps} />);
    await openVendorPage(user, "MiniMax");
    const baseUrlInput = await screen.findByPlaceholderText(
      "https://api.example.com/v1",
    );
    expect(baseUrlInput).toHaveValue("https://api.minimaxi.com/anthropic");
    // 切到 OpenAI 格式 → 已知双协议表命中，Base URL 自动换成 /v1
    await user.click(
      screen.getByRole("button", { name: /Anthropic Messages/ }),
    );
    await user.click(
      await screen.findByRole("option", { name: "OpenAI Chat Completions" }),
    );
    await waitFor(() => {
      expect(baseUrlInput).toHaveValue("https://api.minimaxi.com/v1");
    });
    // 切回 Anthropic → 恢复切换前记住的 URL
    await user.click(
      screen.getByRole("button", { name: /OpenAI Chat Completions/ }),
    );
    await user.click(
      await screen.findByRole("option", { name: "Anthropic Messages" }),
    );
    await waitFor(() => {
      expect(baseUrlInput).toHaveValue("https://api.minimaxi.com/anthropic");
    });
  });

  it("「获取 API Key」：预设厂商显示外链并调 opener.openUrl；自定义厂商不显示", async () => {
    const user = userEvent.setup();
    mockVendorConfig({
      ...deepseekVendorConfig,
      modelsByProvider: {
        openai: [
          ...deepseekVendorConfig.modelsByProvider.openai,
          {
            id: "m9",
            label: "自建模型",
            model: "my-model",
            baseUrl: "https://llm.example.com/v1",
            vendor: "我的厂商",
          },
        ],
        anthropic: [],
      },
    });
    render(<SettingsPage {...defaultProps} />);
    await openVendorPage(user, "DeepSeek");
    await user.click(
      await screen.findByRole("button", { name: "获取 API Key" }),
    );
    expect(mocks.openUrlMock).toHaveBeenCalledWith(
      "https://platform.deepseek.com/api_keys",
    );
    // 自定义厂商（无预设 keyUrl）→ 不显示外链
    await user.click(screen.getByRole("button", { name: "我的厂商" }));
    expect(screen.queryByRole("button", { name: "获取 API Key" })).toBeNull();
  });

  it("连接测试：ok → 插头变成功态；error → 失败态且 title 显示错误", async () => {
    const user = userEvent.setup();
    let testResult: unknown = { ok: true, status: 200 };
    mockVendorConfig(deepseekVendorConfig, (cmd) =>
      cmd === "bot_test_connection" ? testResult : undefined,
    );
    render(<SettingsPage {...defaultProps} />);
    await openVendorPage(user, "DeepSeek");
    const plug = await screen.findByRole("button", { name: "测试连接" });
    await user.click(plug);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_test_connection", {
        baseUrl: "https://api.deepseek.com",
        apiFormat: "openai",
        vendor: "DeepSeek",
      });
    });
    await waitFor(() => {
      expect(plug.className).toContain("text-[var(--success)]");
      expect(plug.title).toContain("连接成功");
    });
    // 失败路径：HTTP 401 + 错误文案
    testResult = { ok: false, status: 401, error: "未授权" };
    await user.click(plug);
    await waitFor(() => {
      expect(plug.className).toContain("text-[var(--danger)]");
      expect(plug.title).toContain("未授权");
    });
  });

  it("连接测试通过 → 左栏提示点变绿（verified_vendors 落盘 + reload）；key 未验证的厂商保持灰点", async () => {
    const user = userEvent.setup();
    // 配置可变：连接测试「成功」后模拟后端已把 DeepSeek 写进 verifiedVendors
    let verified: string[] = [];
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_test_connection") return { ok: true, status: 200 };
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "bot_get_config")
        return { ...deepseekVendorConfig, verifiedVendors: verified };
      if (cmd === "bot_set_config") return null;
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
    });
    render(<SettingsPage {...defaultProps} />);
    await openVendorPage(user, "DeepSeek");
    // 初始：未验证 → 灰点
    const row = await screen.findByRole("button", { name: "DeepSeek" });
    const dot = () => row.querySelector("span.rounded-full");
    expect(dot()?.className).toContain("bg-[var(--t6)]");
    // 点插头测试 → 后端写盘 verified_vendors；onTested 触发 reload 后绿点
    verified = ["DeepSeek"];
    await user.click(await screen.findByRole("button", { name: "测试连接" }));
    await waitFor(() => {
      expect(dot()?.className).toContain("bg-[var(--success)]");
    });
  });

  it("厂商页保存配置 → 有开启的模型自动跑厂商级连接测试并显示结果；全部关闭则不测", async () => {
    const user = userEvent.setup();
    mockVendorConfig(deepseekVendorConfig, (cmd) =>
      cmd === "bot_test_connection" ? { ok: true, status: 200 } : undefined,
    );
    render(<SettingsPage {...defaultProps} />);
    await openVendorPage(user, "DeepSeek");
    const saveBtns = await screen.findAllByRole("button", { name: "保存配置" });
    await user.click(saveBtns[saveBtns.length - 1]);
    // 自动探测：厂商级参数（共享 Base URL + 协议 + 厂商名）
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_test_connection", {
        baseUrl: "https://api.deepseek.com",
        apiFormat: "openai",
        vendor: "DeepSeek",
      });
    });
    expect(await screen.findByText("连接测试通过 ✓")).toBeInTheDocument();
  });

  it("厂商页保存配置：模型开关全部关闭 → 不自动连接测试", async () => {
    const user = userEvent.setup();
    mockVendorConfig({
      ...deepseekVendorConfig,
      modelsByProvider: {
        openai: [
          {
            ...deepseekVendorConfig.modelsByProvider.openai[0],
            enabled: false,
          },
        ],
        anthropic: [],
      },
    });
    render(<SettingsPage {...defaultProps} />);
    await openVendorPage(user, "DeepSeek");
    const saveBtns = await screen.findAllByRole("button", { name: "保存配置" });
    await user.click(saveBtns[saveBtns.length - 1]);
    // 保存完成（bot_set_config 被调）后仍不发起探测
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_set_config",
        expect.anything(),
      );
    });
    expect(
      mocks.invokeMock.mock.calls.filter((c) => c[0] === "bot_test_connection"),
    ).toHaveLength(0);
  });

  it("插头颜色持久化：厂商在 verifiedVendors 里 → 未点测试插头也显绿", async () => {
    const user = userEvent.setup();
    mockVendorConfig({
      ...deepseekVendorConfig,
      verifiedVendors: ["DeepSeek"],
    });
    render(<SettingsPage {...defaultProps} />);
    await openVendorPage(user, "DeepSeek");
    const plug = await screen.findByRole("button", { name: "测试连接" });
    // 未点测试：持久化名单命中即绿
    expect(plug.className).toContain("text-[var(--success)]");
    expect(plug.title).toContain("已通过");
  });

  it("失效的「视觉能力」设置已下线：不再渲染能力徽标与编辑态 checkbox", async () => {
    const user = userEvent.setup();
    mockVendorConfig({
      modelsByProvider: {
        openai: [],
        anthropic: [
          {
            id: "m1",
            label: "MiniMax-M3",
            model: "MiniMax-M3",
            baseUrl: "https://api.minimaxi.com/anthropic",
            vendor: "MiniMax",
            contextK: 1000,
            // 老配置里的残留字段：视觉能力对任何运行时行为都不设闸，
            // 设置页既不再展示徽标也不让它进 UI
            capabilities: ["视觉"],
          },
          {
            id: "m2",
            label: "MiniMax-M2",
            model: "MiniMax-M2",
            baseUrl: "https://api.minimaxi.com/anthropic",
            vendor: "MiniMax",
            contextK: 204.8,
          },
        ],
      },
      activeModelId: { openai: null, anthropic: "m1" },
      apiProvider: "anthropic",
      hasApiKey: true,
      bypassLlmOnPreStepHit: true,
    });
    render(<SettingsPage {...defaultProps} />);
    await openVendorPage(user, "MiniMax");
    // 老配置残留的 capabilities 不再渲染成徽标
    expect(screen.queryAllByText("视觉")).toHaveLength(0);
    // 上下文徽标保持既有行为（编辑态把模型名换成输入框，故先断言徽标）
    const row1 = screen.getByText("MiniMax-M3").closest("div")!;
    const row2 = screen.getByText("MiniMax-M2").closest("div")!;
    expect(within(row1).getByText("1000K")).toBeInTheDocument();
    expect(within(row2).getByText("204.8K")).toBeInTheDocument();
    // 编辑态的「视觉能力」checkbox 一并下线
    const editBtns = screen.getAllByRole("button", { name: "编辑此模型" });
    await user.click(editBtns[0]);
    expect(screen.queryByLabelText("视觉能力")).toBeNull();
    expect(screen.queryByText("视觉能力")).toBeNull();
  });

  it("厂商 Logo：预设厂商渲染本地 lobehub <img>，自定义厂商回退色块+首字母", async () => {
    const user = userEvent.setup();
    mockVendorConfig({
      ...deepseekVendorConfig,
      modelsByProvider: {
        openai: [
          ...deepseekVendorConfig.modelsByProvider.openai,
          {
            id: "m9",
            label: "自建模型",
            model: "my-model",
            baseUrl: "https://llm.example.com/v1",
            vendor: "我的厂商",
          },
        ],
        anthropic: [],
      },
    });
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "模型设置" }));
    // 左栏：DeepSeek（预设）有本地 lobehub <img> logo；我的厂商（自定义）回退色块+首字母
    const dsBtn = await screen.findByRole("button", { name: "DeepSeek" });
    expectLogoSlug(dsBtn.querySelector("img"), "deepseek");
    const customBtn = screen.getByRole("button", { name: "我的厂商" });
    expect(customBtn.querySelector("img")).toBeNull();
    expect(within(customBtn).getByText("我")).toBeInTheDocument();
    // 厂商详情页头同样用 <img>
    await user.click(dsBtn);
    const header = await screen.findByRole("heading", { name: "DeepSeek" });
    expect(header.parentElement?.querySelector("img")).not.toBeNull();
  });

  it("updateModel 跨协议回归：apiProvider=openai 时编辑 anthropic 厂商条目写回 anthropic 列表", async () => {
    const user = userEvent.setup();
    mockVendorConfig({
      modelsByProvider: {
        openai: [
          {
            id: "o1",
            label: "DeepSeek Chat",
            model: "deepseek-chat",
            baseUrl: "https://api.deepseek.com",
            vendor: "DeepSeek",
          },
        ],
        anthropic: [
          {
            id: "a1",
            label: "Claude",
            model: "claude-sonnet-4-5",
            baseUrl: "https://api.anthropic.com",
            vendor: "Anthropic",
          },
        ],
      },
      activeModelId: { openai: "o1", anthropic: "a1" },
      // 全局协议停在 openai，但编辑 anthropic 厂商的条目——
      // 旧实现按 apiProvider 定位列表会写不进/写错列表
      apiProvider: "openai",
      hasApiKey: true,
      bypassLlmOnPreStepHit: true,
    });
    render(<SettingsPage {...defaultProps} />);
    await openVendorPage(user, "Anthropic");
    await user.click(await screen.findByRole("button", { name: "编辑此模型" }));
    const labelInput = await screen.findByPlaceholderText(/DeepSeek \/ Kimi/);
    await user.clear(labelInput);
    await user.type(labelInput, "Claude Sonnet X");
    // 受控输入能更新 = patch 落到了条目实际所在的 anthropic 列表
    expect(labelInput).toHaveValue("Claude Sonnet X");
    // 落盘 payload：anthropic 条目改名，openai 条目原样
    const saveBtns = screen.getAllByRole("button", { name: "保存配置" });
    await user.click(saveBtns[saveBtns.length - 1]);
    await waitFor(() => {
      const setCalls = mocks.invokeMock.mock.calls.filter(
        (c) => c[0] === "bot_set_config",
      );
      expect(setCalls.length).toBeGreaterThan(0);
      const arg = setCalls[setCalls.length - 1]![1] as {
        config: {
          modelsByProvider: {
            openai: Array<{ label: string }>;
            anthropic: Array<{ label: string }>;
          };
        };
      };
      expect(arg.config.modelsByProvider.anthropic[0].label).toBe(
        "Claude Sonnet X",
      );
      expect(arg.config.modelsByProvider.openai[0].label).toBe("DeepSeek Chat");
    });
  });
});
