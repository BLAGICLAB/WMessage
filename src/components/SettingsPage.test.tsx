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
      };
    case "py_get_enabled":
      return false;
    case "py_env_check":
      return { available: false, python: "", version: "", libs: [] };
    case "api_status":
      return { enabled: false, port: 4763, token: "" };
    case "skills_list":
      return [];
    case "migration_rules_load":
      return { version: 1, rules: [] };
    case "migration_status":
      return { rules_count: 0, poll_interval_secs: 600 };
    case "migration_log_read":
      return "";
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
    expect(screen.getByText("深浅色模式")).toBeInTheDocument();
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
          // 2026-09-05 起 view 只给 has 标志，key 本体在系统凭据存储
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
          // 2026-09-05 起缺 key 的判定看 has 标志（keyring 里没有）
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

  it("提供商预设：点击 Kimi → Base URL / 模型自动填充且按钮高亮", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "bot_get_config")
        return {
          baseUrl: "https://api.minimaxi.com/v1",
          model: "MiniMax-M3",
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
    // 等待配置加载：Base URL 输入框回填 MiniMax 地址
    const baseUrlInput = await screen.findByDisplayValue("https://api.minimaxi.com/v1");
    // MiniMax 预设高亮（baseUrl 匹配）
    expect(screen.getByText("MiniMax").className).toContain("nm-inset");
    // 点击 Kimi → 自动填充
    await user.click(screen.getByText("Kimi"));
    await waitFor(() => {
      expect(baseUrlInput).toHaveValue("https://api.moonshot.cn/v1");
    });
    expect(screen.getByPlaceholderText("deepseek-v4-flash")).toHaveValue("kimi-k3");
    expect(screen.getByText("Kimi").className).toContain("nm-inset");
    expect(screen.getByText("MiniMax").className).toContain("nm-outset");
    // 命中供应商时「自定义」不高亮
    expect(screen.getByText("✏️ 自定义").className).toContain("nm-outset");
    // 点「自定义」→ 清空 Base URL/模型进入自定义填写态，按钮高亮
    await user.click(screen.getByText("✏️ 自定义"));
    expect(baseUrlInput).toHaveValue("");
    expect(screen.getByPlaceholderText("deepseek-v4-flash")).toHaveValue("");
    expect(screen.getByText("✏️ 自定义").className).toContain("nm-inset");
    expect(screen.getByText("Kimi").className).toContain("nm-outset");
  });

  it("提供商预设：同供应商手改模型（deepseek-v4-pro）→ DeepSeek 按钮保持高亮", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "bot_get_config")
        return {
          baseUrl: "https://api.deepseek.com/v1",
          model: "deepseek-v4-pro",
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
    await screen.findByDisplayValue("https://api.deepseek.com/v1");
    // 预设只看供应商（baseUrl）：模型手改成 pro 档 DeepSeek 仍高亮，自定义不高亮
    expect(screen.getByText("DeepSeek").className).toContain("nm-inset");
    expect(screen.getByText("✏️ 自定义").className).toContain("nm-outset");
  });

  it("提供商预设：自定义地址/模型（不命中任何预设）→「自定义」按钮高亮", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "bot_get_config")
        return {
          baseUrl: "https://api.example.com/v1",
          model: "my-model",
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
    await screen.findByDisplayValue("https://api.example.com/v1");
    expect(screen.getByText("✏️ 自定义").className).toContain("nm-inset");
    expect(screen.getByText("MiniMax").className).toContain("nm-outset");
  });

  it("API 协议：切到 Anthropic → placeholder 联动 + max_tokens 出现，保存时透传 apiProvider/maxTokens", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "bot_get_config")
        return {
          baseUrl: "https://api.deepseek.com/v1",
          model: "deepseek-v4-flash",
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
    // 配置加载后协议下拉出现（自绘下拉，2026-09-05：原生 select 弹系统菜单不跟随主题），默认 OpenAI 兼容；max_tokens 不显示
    const trigger = await screen.findByRole("button", { name: /OpenAI 兼容/ });
    expect(screen.queryByText("max_tokens")).toBeNull();
    // 切到 Anthropic：点触发钮展开浮层 → 点选项；placeholder 联动 + max_tokens 输入框出现
    await user.click(trigger);
    await user.click(screen.getByRole("button", { name: /Anthropic 兼容/ }));
    expect(screen.getByPlaceholderText("https://api.anthropic.com")).toBeInTheDocument();
    expect(screen.getByPlaceholderText("claude-sonnet-4-5")).toBeInTheDocument();
    const maxTokensInput = screen.getByPlaceholderText("8192");
    await user.type(maxTokensInput, "4096");
    // 保存 → bot_set_config 透传 apiProvider/maxTokens
    await user.click(screen.getByText("保存配置"));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_set_config",
        expect.objectContaining({
          config: expect.objectContaining({
            apiProvider: "anthropic",
            maxTokens: 4096,
          }),
        })
      );
    });
  });
});
