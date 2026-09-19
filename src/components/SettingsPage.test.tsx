/// <reference types="node" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SettingsPage } from "./SettingsPage";
import type { ThemeSetting } from "../theme";

// vi.mock 工厂会被提升到顶部，因此共享 mock 变量必须用 vi.hoisted 包裹
const mocks = vi.hoisted(() => {
  const invokeMock = vi.fn();
  const emitMock = vi.fn();
  const openMock = vi.fn();
  return { invokeMock, emitMock, openMock };
});

mocks.invokeMock.mockImplementation(async (cmd: string) => {
  switch (cmd) {
    case "profile_get":
      return {
        user: { name: "我", avatarDataUrl: null },
        bot: { name: "机器人", avatarDataUrl: null },
      };
    case "bot_get_enabled":
      return false;
    case "bot_get_config":
      return {
        baseUrl: "",
        model: "",
        hasApiKey: false,
        bypassLlmOnPreStepHit: true,
        // 字体大小：默认 small（老板拍板「目前字号为小」）
        uiFontSize: "small",
      };
    case "py_get_enabled":
      return false;
    case "py_env_check":
      return { available: false, python: "", version: "", libs: [] };
    case "api_status":
      // 模拟 disabled 状态:后端用 skip_serializing_if 剔除 token 字段
      return { enabled: false, port: 4763 };
    case "skills_list":
      return [];
    case "migration_rules_load":
      return { version: 1, rules: [] };
    case "migration_status":
      return { rules_count: 0, poll_interval_secs: 600 };
    case "migration_log_read":
      return "";
    // 开机自启动：默认关闭；enable/disable 幂等返 null
    case "plugin:autostart|is_enabled":
      return false;
    case "plugin:autostart|enable":
    case "plugin:autostart|disable":
      return null;
    default:
      return null;
  }
});
mocks.emitMock.mockImplementation(async () => {});
mocks.openMock.mockImplementation(async () => null);

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invokeMock,
  convertFileSrc: vi.fn((p: string) => `tauri://localhost/${p}`),
}));

// 隔离 profile 模块缓存：各测试都需要 fresh load
vi.mock("../profile", () => ({
  getProfileCache: vi.fn(() => null),
  loadProfile: vi.fn(async () => ({
    user: { name: "我", avatarDataUrl: null },
    bot: { name: "机器人", avatarDataUrl: null },
  })),
  subscribeProfile: vi.fn(() => () => {}),
  setProfileName: vi.fn(async () => ({
    user: { name: "我", avatarDataUrl: null },
    bot: { name: "机器人", avatarDataUrl: null },
  })),
  setProfileAvatar: vi.fn(async () => ({
    user: { name: "我", avatarDataUrl: null },
    bot: { name: "机器人", avatarDataUrl: null },
  })),
  removeProfileAvatar: vi.fn(async () => ({
    user: { name: "我", avatarDataUrl: null },
    bot: { name: "机器人", avatarDataUrl: null },
  })),
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: mocks.emitMock,
  listen: vi.fn(async () => () => {}),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: mocks.openMock,
  save: vi.fn(async () => null),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openPath: vi.fn(async () => {}),
}));

const confirmMock = vi.fn(() => true);
const alertMock = vi.fn();
beforeEach(() => {
  mocks.invokeMock.mockClear();
  mocks.emitMock.mockClear();
  mocks.openMock.mockClear();
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
  it("初始渲染：所有主要 section 都出现", async () => {
    render(<SettingsPage {...defaultProps} />);
    // 各 section 标题
    expect(screen.getByText("个人资料")).toBeInTheDocument();
    expect(screen.getByText("通用设置")).toBeInTheDocument();
    expect(screen.getByText("任务数据管理")).toBeInTheDocument();
    expect(screen.getByText("机器人设置")).toBeInTheDocument();
    expect(screen.getByText("机器人技能")).toBeInTheDocument();
    // 主题三按钮
    expect(screen.getByText("☀️ 浅色")).toBeInTheDocument();
    expect(screen.getByText("🌙 深色")).toBeInTheDocument();
    expect(screen.getByText("🖥️ 跟随系统")).toBeInTheDocument();
    // 等待初始 invoke 异步加载完成
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_get_enabled");
    });
  });

  it("主题三态：依次点击 → onThemeChange 收到 light / dark / system", async () => {
    const onThemeChange = vi.fn();
    const user = userEvent.setup();
    render(<SettingsPage {...defaultProps} onThemeChange={onThemeChange} />);
    await user.click(screen.getByText("☀️ 浅色"));
    expect(onThemeChange).toHaveBeenLastCalledWith("light");
    await user.click(screen.getByText("🌙 深色"));
    expect(onThemeChange).toHaveBeenLastCalledWith("dark");
    await user.click(screen.getByText("🖥️ 跟随系统"));
    expect(onThemeChange).toHaveBeenLastCalledWith("system");
    expect(onThemeChange).toHaveBeenCalledTimes(3);
  });

  it("主题激活态：当前 theme 对应按钮带 nm-inset 类（视觉反馈）", async () => {
    render(<SettingsPage {...defaultProps} theme="dark" />);
    const darkBtn = screen.getByText("🌙 深色");
    const lightBtn = screen.getByText("☀️ 浅色");
    // 当前选中的按钮包含 nm-inset，未选中的用 nm-outset
    expect(darkBtn.className).toContain("nm-inset");
    expect(lightBtn.className).toContain("nm-outset");
  });

  it("保存按钮 disabled 语义：ProfileRow 空姓名时点保存 → 提示「姓名不能为空」", async () => {
    const user = userEvent.setup();
    render(<SettingsPage {...defaultProps} />);
    // 等待用户 ProfileRow 的 input 出现（profile 加载后用 useEffect 回填）
    const userInput = (await screen.findAllByDisplayValue("我"))[0];
    await user.clear(userInput);
    // 两个 ProfileRow 都有「保存」按钮，点击第一个（用户 ProfileRow）
    const saveBtns = screen.getAllByText("保存");
    await user.click(saveBtns[0]);
    // 提示「姓名不能为空」
    expect(await screen.findByText("姓名不能为空")).toBeInTheDocument();
    // 不应触发 profile_set_name
    expect(mocks.invokeMock).not.toHaveBeenCalledWith("profile_set_name");
  });

  it("机器人聊天开关：点击 → bot_set_enabled 调用 + 按钮文案变成「已开启」", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_enabled") return false;
      if (cmd === "bot_set_enabled") return true;
      if (cmd === "bot_get_config")
        return {
          baseUrl: "",
          model: "",
          hasApiKey: false,
          bypassLlmOnPreStepHit: true,
        };
      if (cmd === "api_status") return { enabled: false, port: 4763, token: "" };
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarDataUrl: null },
          bot: { name: "机器人", avatarDataUrl: null },
        };
      if (cmd === "py_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status") return { rules_count: 0, poll_interval_secs: 600 };
      if (cmd === "migration_log_read") return "";
      return null;
    });
    render(<SettingsPage {...defaultProps} />);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_get_enabled");
    });
    // 「开启机器人聊天」行的描述文案作为锚点找所在行的 button
    const botDesc = screen.getByText("在挂件下方显示聊天窗口，用大模型管理任务");
    const row = botDesc.closest("div")?.parentElement;
    const toggle = row?.querySelector("button");
    expect(toggle).not.toBeNull();
    if (!toggle) return;
    await user.click(toggle);
    // 调用了 bot_set_enabled
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_set_enabled",
        expect.objectContaining({ enabled: true })
      );
    });
    // 调用后开关按钮文案变成「已开启」
    expect(toggle.textContent).toMatch(/已开启/);
  });

  it("Tavily 开关：点击 → bot_set_config 持久化 tavilyEnabled；key 在 keyring 不动（2026-09-05）", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "bot_set_enabled") return true;
      if (cmd === "bot_get_config")
        return {
          baseUrl: "https://api.minimaxi.com/v1",
          model: "MiniMax-M3",
          hasApiKey: true,
          bypassLlmOnPreStepHit: true,
          allowedDirs: [],
          // view 只给 has 标志，key 本体在系统凭据存储
          hasTavilyKey: true,
          tavilyEnabled: false, // 显式关闭：配了 key 也应走双引擎
          pythonTimeoutSecs: null,
        };
      if (cmd === "bot_set_config") return null;
      if (cmd === "api_status") return { enabled: false, port: 4763, token: "" };
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarDataUrl: null },
          bot: { name: "机器人", avatarDataUrl: null },
        };
      if (cmd === "py_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status") return { rules_count: 0, poll_interval_secs: 600 };
      if (cmd === "migration_log_read") return "";
      return null;
    });
    render(<SettingsPage {...defaultProps} />);
    // 「Tavily 搜索」行出现（开关初始为关）
    const title = await screen.findByText("Tavily 搜索");
    const row = title.closest("div")?.parentElement;
    const toggle = row?.querySelector("button");
    expect(toggle).not.toBeNull();
    if (!toggle) return;
    expect(toggle.textContent).toMatch(/已关闭/);
    await user.click(toggle);
    // 点击即持久化：tavilyEnabled 翻转为 true；config 里 key 字段固定 null（不落明文），
    // 顶层 tavilyKey 参数 null（输入框为空 → 后端不动 keyring 里已存的 key）
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_set_config",
        expect.objectContaining({
          tavilyKey: null,
          braveKey: null,
          config: expect.objectContaining({
            tavilyEnabled: true,
            tavilyKey: null,
            braveKey: null,
          }),
        })
      );
    });
  });

  it("Tavily 开关：开启但没填 key → 显示缺 key 提示（不静默走百度）", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "bot_get_config")
        return {
          baseUrl: "",
          model: "",
          hasApiKey: false,
          bypassLlmOnPreStepHit: true,
          allowedDirs: [],
          // 缺 key 的判定看 has 标志（keyring 里没有）
          hasTavilyKey: false,
          tavilyEnabled: true, // 开了但没 key
          pythonTimeoutSecs: null,
        };
      if (cmd === "api_status") return { enabled: false, port: 4763, token: "" };
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarDataUrl: null },
          bot: { name: "机器人", avatarDataUrl: null },
        };
      if (cmd === "py_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status") return { rules_count: 0, poll_interval_secs: 600 };
      if (cmd === "migration_log_read") return "";
      return null;
    });
    render(<SettingsPage {...defaultProps} />);
    expect(
      await screen.findByText(/已开启但未填 key/)
    ).toBeInTheDocument();
  });

  it("Tavily key 输入：不回填已存 key；输入新 key 保存 → 顶层参数透传 + config 字段 null + 输入框清空", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "bot_get_config")
        return {
          baseUrl: "https://api.minimaxi.com/v1",
          model: "MiniMax-M3",
          hasApiKey: true,
          bypassLlmOnPreStepHit: true,
          allowedDirs: [],
          hasTavilyKey: true, // 已存 key：输入框不应回填任何值
          tavilyEnabled: true,
          pythonTimeoutSecs: null,
        };
      if (cmd === "bot_set_config") return null;
      if (cmd === "api_status") return { enabled: false, port: 4763, token: "" };
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarDataUrl: null },
          bot: { name: "机器人", avatarDataUrl: null },
        };
      if (cmd === "py_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status") return { rules_count: 0, poll_interval_secs: 600 };
      if (cmd === "migration_log_read") return "";
      return null;
    });
    render(<SettingsPage {...defaultProps} />);
    // 已存 key：placeholder 提示 + 输入框不回填
    const input = await screen.findByPlaceholderText(/已存入系统凭据存储/);
    expect(input).toHaveValue("");
    // 输入新 key → 保存配置
    await user.type(input, "tvly-new-key");
    await user.click(screen.getByText("保存配置"));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_set_config",
        expect.objectContaining({
          // 新 key 走顶层参数（后端写系统凭据存储）
          tavilyKey: "tvly-new-key",
          braveKey: null,
          apiKey: null,
          config: expect.objectContaining({
            // config 对象里的 key 字段固定 null（不落明文，后端强制置 None 双保险）
            tavilyKey: null,
            braveKey: null,
          }),
        })
      );
    });
    // 保存成功后输入框清空（与主 keyInput 同模式）
    await waitFor(() => {
      expect(screen.getByPlaceholderText(/已存入系统凭据存储/)).toHaveValue("");
    });
  });

  it("导出按钮：点击 → 调用 onExportTasks prop", async () => {
    const onExportTasks = vi.fn(async () => {});
    const user = userEvent.setup();
    render(<SettingsPage {...defaultProps} onExportTasks={onExportTasks} />);
    // eefa78f 起页面有两个「📤 导出」（任务导出 + 工作区导出），取第一个（任务导出）
    await user.click(screen.getAllByText("📤 导出")[0]);
    await waitFor(() => {
      expect(onExportTasks).toHaveBeenCalledTimes(1);
    });
  });

  // ───────── 双协议下的大模型列表（老板拍板改版）─────────
  // 老「提供商预设」3 个测试 + 1 个「API 协议」测试全部重写：新行为是每协议独立
  // 一份 ModelEntry 列表，点「添加大模型」自己加，切换协议时列表整体切换。

  it("大模型 API 配置：初始无模型（老板要求「不设置默认厂商」）→ 列表为空 + 显示「暂无大模型」", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "bot_get_config")
        return {
          // 新结构：两协议都是空数组
          modelsByProvider: { openai: [], anthropic: [] },
          activeModelId: { openai: null, anthropic: null },
          hasApiKey: false,
          bypassLlmOnPreStepHit: true,
        };
      if (cmd === "api_status") return { enabled: false, port: 4763, token: "" };
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarDataUrl: null },
          bot: { name: "机器人", avatarDataUrl: null },
        };
      if (cmd === "py_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status") return { rules_count: 0, poll_interval_secs: 600 };
      if (cmd === "migration_log_read") return "";
      return null;
    });
    render(<SettingsPage {...defaultProps} />);
    // 空态：列表为空提示 + 添加按钮（用 role=button 避免和空态描述里“添加大模型”同款文字冲突）
    expect(await screen.findByText(/暂无大模型/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /添加大模型/ })).toBeInTheDocument();
    // 老「提供商预设」UI 已经全部拿掉
    expect(screen.queryByText("MiniMax")).toBeNull();
    expect(screen.queryByText("DeepSeek")).toBeNull();
    expect(screen.queryByText(/自定义/)).toBeNull();
  });

  it("大模型 API 配置：点「添加大模型」→ 列表加一行 + 自动 active（空列表首次添加）", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "bot_get_config")
        return {
          modelsByProvider: { openai: [], anthropic: [] },
          activeModelId: { openai: null, anthropic: null },
          hasApiKey: false,
          bypassLlmOnPreStepHit: true,
        };
      if (cmd === "api_status") return { enabled: false, port: 4763, token: "" };
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarDataUrl: null },
          bot: { name: "机器人", avatarDataUrl: null },
        };
      if (cmd === "py_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status") return { rules_count: 0, poll_interval_secs: 600 };
      if (cmd === "migration_log_read") return "";
      return null;
    });
    render(<SettingsPage {...defaultProps} />);
    // 切到 Anthropic 后再点添加，验证新行落到 anthropic 协议下
    const trigger = await screen.findByRole("button", { name: /OpenAI 兼容/ });
    await user.click(trigger);
    await user.click(screen.getByRole("button", { name: /Anthropic 兼容/ }));
    expect(await screen.findByText(/暂无大模型/)).toBeInTheDocument();
    // 点「添加大模型」（用 role=button 避免和空态描述里同款文字冲突）
    await user.click(screen.getByRole("button", { name: /添加大模型/ }));
    // 列表里出现一行（用 ModelRow 的 label placeholder 定位）
    const labelInput = await screen.findByPlaceholderText(/DeepSeek \/ Kimi/);
    expect(labelInput).toBeInTheDocument();
    // 这一行自动 active——radio 按钮 title 走到「当前选中」分支
    expect(screen.getByTitle("当前选中（点其它条目可切换）")).toBeInTheDocument();
    // OpenAI 协议下仍然是空（列表不串协议）
    const trigger2 = screen.getByRole("button", { name: /Anthropic 兼容/ });
    await user.click(trigger2);
    await user.click(screen.getByRole("button", { name: /OpenAI 兼容/ }));
    expect(screen.getByText(/暂无大模型/)).toBeInTheDocument();
  });

  it("大模型 API 配置：切协议 → 列表整体切换（OpenAI 模型不在 Anthropic 协议下显示）", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "bot_get_config")
        return {
          modelsByProvider: {
            openai: [
              { id: "m1", label: "DeepSeek", baseUrl: "https://api.deepseek.com/v1", model: "deepseek-v4-flash" },
              { id: "m2", label: "Kimi", baseUrl: "https://api.moonshot.cn/v1", model: "kimi-k3" },
            ],
            anthropic: [
              { id: "m3", label: "Claude Sonnet", baseUrl: "https://api.anthropic.com", model: "claude-sonnet-4-5" },
            ],
          },
          activeModelId: { openai: "m1", anthropic: "m3" },
          hasApiKey: true,
          bypassLlmOnPreStepHit: true,
        };
      if (cmd === "api_status") return { enabled: false, port: 4763, token: "" };
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarDataUrl: null },
          bot: { name: "机器人", avatarDataUrl: null },
        };
      if (cmd === "py_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status") return { rules_count: 0, poll_interval_secs: 600 };
      if (cmd === "migration_log_read") return "";
      return null;
    });
    render(<SettingsPage {...defaultProps} />);
    // 默认 OpenAI 协议：DeepSeek + Kimi 都在，Claude 不在
    await screen.findByDisplayValue("DeepSeek");
    expect(screen.getByDisplayValue("Kimi")).toBeInTheDocument();
    expect(screen.queryByDisplayValue("Claude Sonnet")).toBeNull();
    // 切到 Anthropic
    const trigger = screen.getByRole("button", { name: /OpenAI 兼容/ });
    await user.click(trigger);
    await user.click(screen.getByRole("button", { name: /Anthropic 兼容/ }));
    // 现在 Anthropic 协议：只有 Claude
    await screen.findByDisplayValue("Claude Sonnet");
    expect(screen.queryByDisplayValue("DeepSeek")).toBeNull();
    expect(screen.queryByDisplayValue("Kimi")).toBeNull();
    // 切回 OpenAI
    const trigger2 = screen.getByRole("button", { name: /Anthropic 兼容/ });
    await user.click(trigger2);
    await user.click(screen.getByRole("button", { name: /OpenAI 兼容/ }));
    // DeepSeek/Kimi 又回来了
    await screen.findByDisplayValue("DeepSeek");
    expect(screen.getByDisplayValue("Kimi")).toBeInTheDocument();
    // 且 DeepSeek（m1）仍是 active——回到原协议时 active 模型是协议级记忆
    expect(screen.getByTitle("当前选中（点其它条目可切换）")).toBeInTheDocument();
  });

  it("大模型 API 配置：删除 active → 列表第一个顶替 active（删完 active=null，列表空）", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "bot_get_config")
        return {
          modelsByProvider: {
            openai: [
              { id: "m1", label: "DeepSeek", baseUrl: "https://api.deepseek.com/v1", model: "deepseek-v4-flash" },
              { id: "m2", label: "Kimi", baseUrl: "https://api.moonshot.cn/v1", model: "kimi-k3" },
            ],
            anthropic: [],
          },
          activeModelId: { openai: "m1", anthropic: null },
          hasApiKey: true,
          bypassLlmOnPreStepHit: true,
        };
      if (cmd === "api_status") return { enabled: false, port: 4763, token: "" };
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarDataUrl: null },
          bot: { name: "机器人", avatarDataUrl: null },
        };
      if (cmd === "py_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status") return { rules_count: 0, poll_interval_secs: 600 };
      if (cmd === "migration_log_read") return "";
      return null;
    });
    render(<SettingsPage {...defaultProps} />);
    await screen.findByDisplayValue("DeepSeek");
    // 初始：m1(DeepSeek) active，m2(Kimi) 不是
    expect(screen.getByTitle("当前选中（点其它条目可切换）")).toBeInTheDocument();
    expect(screen.getByTitle("点此切换为当前模型")).toBeInTheDocument();
    // 删 m1 → 列表里只剩 m2(Kimi)，应该顶替成 active
    const delButtons1 = screen.getAllByTitle("删除此模型");
    await user.click(delButtons1[0]);
    await waitFor(() => {
      // Kimi 现在 active
      const radios = screen.getAllByTitle("当前选中（点其它条目可切换）");
      expect(radios).toHaveLength(1);
    });
    // DeepSeek 没了，Kimi 还在
    expect(screen.queryByDisplayValue("DeepSeek")).toBeNull();
    expect(screen.getByDisplayValue("Kimi")).toBeInTheDocument();
    // 删 m2 → 列表空，回到「暂无大模型」提示
    const delButtons2 = screen.getAllByTitle("删除此模型");
    await user.click(delButtons2[0]);
    expect(await screen.findByText(/暂无大模型/)).toBeInTheDocument();
  });

  it("大模型 API 配置：保存 → bot_set_config 透传新结构（modelsByProvider + activeModelId + apiProvider + maxTokens），老 baseUrl/model 字段不再传", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "bot_get_config")
        return {
          modelsByProvider: { openai: [], anthropic: [] },
          activeModelId: { openai: null, anthropic: null },
          hasApiKey: true,
          bypassLlmOnPreStepHit: true,
          apiProvider: "openai",
          maxTokens: null,
        };
      if (cmd === "bot_set_config") return null;
      if (cmd === "api_status") return { enabled: false, port: 4763, token: "" };
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarDataUrl: null },
          bot: { name: "机器人", avatarDataUrl: null },
        };
      if (cmd === "py_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status") return { rules_count: 0, poll_interval_secs: 600 };
      if (cmd === "migration_log_read") return "";
      return null;
    });
    render(<SettingsPage {...defaultProps} />);
    // 切到 Anthropic
    const trigger = await screen.findByRole("button", { name: /OpenAI 兼容/ });
    await user.click(trigger);
    await user.click(screen.getByRole("button", { name: /Anthropic 兼容/ }));
    // 添加大模型（用 role=button 避免和空态描述里同款文字冲突）
    await user.click(screen.getByRole("button", { name: /添加大模型/ }));
    // 填 label
    const labelInput = await screen.findByPlaceholderText(/DeepSeek \/ Kimi/);
    await user.type(labelInput, "Claude Sonnet");
    // 填 max_tokens（Anthropic 模式出现）
    const maxTokensInput = screen.getByPlaceholderText("8192");
    await user.type(maxTokensInput, "4096");
    // 保存 → bot_set_config 透传新结构
    await user.click(screen.getByText("保存配置"));
    await waitFor(() => {
      const setCall = mocks.invokeMock.mock.calls.find(
        (c) => c[0] === "bot_set_config",
      );
      expect(setCall).toBeDefined();
      const arg = setCall![1] as {
        config: {
          modelsByProvider: { anthropic: Array<{ label: string; id: string }> };
          activeModelId: { anthropic: string | null };
          apiProvider: string;
          maxTokens: number | null;
        };
        apiKey: string | null;
        tavilyKey: string | null;
        braveKey: string | null;
      };
      // 新结构：modelsByProvider.anthropic 有刚加的那一条
      expect(arg.config.modelsByProvider.anthropic).toHaveLength(1);
      expect(arg.config.modelsByProvider.anthropic[0].label).toBe("Claude Sonnet");
      // activeModelId.anthropic 是该条目的 id
      expect(arg.config.activeModelId.anthropic).toBe(
        arg.config.modelsByProvider.anthropic[0].id,
      );
      // apiProvider 透传
      expect(arg.config.apiProvider).toBe("anthropic");
      // maxTokens 透传
      expect(arg.config.maxTokens).toBe(4096);
      // 顶层 key 字段仍按以前模式传 null
      expect(arg.apiKey).toBeNull();
      expect(arg.tavilyKey).toBeNull();
      expect(arg.braveKey).toBeNull();
    });
  });

  it("通用设置：开机自动启动开关 → 点调 plugin:autostart|enable / disable，状态联动", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_enabled") return true;
      // 初始关闭；enable/disable 幂等返 null
      if (cmd === "plugin:autostart|is_enabled") return false;
      if (cmd === "plugin:autostart|enable") return null;
      if (cmd === "plugin:autostart|disable") return null;
      if (cmd === "api_status") return { enabled: false, port: 4763, token: "" };
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarDataUrl: null },
          bot: { name: "机器人", avatarDataUrl: null },
        };
      if (cmd === "py_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status") return { rules_count: 0, poll_interval_secs: 600 };
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
        throw { code: "AUTOSTART_FAILED", message: "系统拒绝写入", recoverable: false };
      if (cmd === "api_status") return { enabled: false, port: 4763, token: "" };
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarDataUrl: null },
          bot: { name: "机器人", avatarDataUrl: null },
        };
      if (cmd === "py_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status") return { rules_count: 0, poll_interval_secs: 600 };
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

  it("通用设置：字体大小四档 → 点选即套 data-attr + 立即落盘（与外观点选一致，2026-09-08 老板拍板）", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "plugin:autostart|is_enabled") return false;
      if (cmd === "api_status") return { enabled: false, port: 4763, token: "" };
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarDataUrl: null },
          bot: { name: "机器人", avatarDataUrl: null },
        };
      if (cmd === "py_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status") return { rules_count: 0, poll_interval_secs: 600 };
      if (cmd === "migration_log_read") return "";
      if (cmd === "bot_get_config")
        return {
          hasApiKey: false,
          bypassLlmOnPreStepHit: true,
          modelsByProvider: { openai: [], anthropic: [] },
          activeModelId: { openai: null, anthropic: null },
          // 初始 small（老板拍板默认）
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
        })
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
        })
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
    // small 档保持不动（老板拍板「目前字号为小」）
    expect(css).not.toMatch(/\[data-font-size="small"\]\s+\.text-xs/);
  });
});
