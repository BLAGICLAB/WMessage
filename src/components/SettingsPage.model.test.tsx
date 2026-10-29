/// <reference types="node" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  render,
  screen,
  waitFor,
  fireEvent,
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
  it("大模型 API 配置：初始无模型→ 厂商列表空 + 显示引导", async () => {
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
    // 导航到「模型设置」分类（hidden section 的元素查不到 role，先导航）
    fireEvent.click(screen.getByRole("button", { name: "模型设置" }));
    //  厂商中心：未选厂商显示引导；点「添加厂商」出预设网格
    expect(await screen.findByText(/从左侧选择一个厂商/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /添加厂商/ }));
    expect(screen.getByText("MiniMax")).toBeInTheDocument();
    expect(screen.getByText("DeepSeek")).toBeInTheDocument();
  });

  it("大模型 API 配置：点「添加厂商」选预设 → 添加模型自动 active（厂商首条）", async () => {
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
    // 导航到「模型设置」→ 添加厂商 → 选 Anthropic 预设 → 厂商页
    await user.click(screen.getByRole("button", { name: "模型设置" }));
    await user.click(screen.getByRole("button", { name: "添加厂商" }));
    await user.click(screen.getByRole("button", { name: /Anthropic/ }));
    // 预设自带 claude 模型行
    expect(
      await screen.findByText("claude-sonnet-4-20250514"),
    ).toBeInTheDocument();
    // 点「＋ 添加模型」→ 新空行加入（紧凑行「（未命名）」）
    await user.click(screen.getByRole("button", { name: /添加模型/ }));
    expect(screen.getAllByText("（未命名）").length).toBe(1);
  });

  it("大模型 API 配置：已有在用模型时点预设 → 只加条目不抢 active（未验证厂商不得接管全局）", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "bot_get_config")
        return {
          // 在用：anthropic MiniMax（已验证 active）；点 openai DeepSeek 预设不得顶掉
          modelsByProvider: {
            openai: [],
            anthropic: [
              {
                id: "m-mm",
                label: "MiniMax",
                baseUrl: "https://api.minimaxi.com/anthropic",
                model: "MiniMax-M3",
                vendor: "MiniMax",
                enabled: true,
              },
            ],
          },
          activeModelId: { openai: null, anthropic: "m-mm" },
          apiProvider: "anthropic",
          verifiedVendors: ["MiniMax"],
          hasApiKey: true,
          bypassLlmOnPreStepHit: true,
        };
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
    await user.click(screen.getByRole("button", { name: "模型设置" }));
    await user.click(screen.getByRole("button", { name: "添加厂商" }));
    await user.click(screen.getByRole("button", { name: "DeepSeek" }));
    // 预设立即落盘：条目进 openai 列表，但协议与 active 仍指 MiniMax
    await waitFor(() => {
      const setCalls = mocks.invokeMock.mock.calls.filter(
        (c) => c[0] === "bot_set_config",
      );
      expect(setCalls.length).toBeGreaterThan(0);
      const arg = setCalls[setCalls.length - 1]![1] as {
        config: {
          apiProvider?: string;
          modelsByProvider: {
            openai: { vendor?: string; enabled?: boolean }[];
            anthropic: { id: string }[];
          };
          activeModelId: { openai: string | null; anthropic: string | null };
        };
      };
      expect(arg.config.apiProvider).toBe("anthropic");
      expect(arg.config.activeModelId.anthropic).toBe("m-mm");
      expect(arg.config.activeModelId.openai).toBeNull();
      expect(
        arg.config.modelsByProvider.openai.some(
          (m) => m.vendor === "DeepSeek" && m.enabled,
        ),
      ).toBe(true);
      expect(arg.config.modelsByProvider.anthropic.map((m) => m.id)).toEqual([
        "m-mm",
      ]);
    });
  });

  it("大模型 API 配置：空配置点预设 → 照旧接管协议与 active（首接语义不变）", async () => {
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
    await user.click(screen.getByRole("button", { name: "模型设置" }));
    await user.click(screen.getByRole("button", { name: "添加厂商" }));
    await user.click(screen.getByRole("button", { name: /Anthropic/ }));
    await waitFor(() => {
      const setCalls = mocks.invokeMock.mock.calls.filter(
        (c) => c[0] === "bot_set_config",
      );
      expect(setCalls.length).toBeGreaterThan(0);
      const arg = setCalls[setCalls.length - 1]![1] as {
        config: {
          apiProvider?: string;
          activeModelId: { anthropic: string | null };
          modelsByProvider: { anthropic: { vendor?: string }[] };
        };
      };
      expect(arg.config.apiProvider).toBe("anthropic");
      expect(arg.config.activeModelId.anthropic).toBeTruthy();
      expect(arg.config.modelsByProvider.anthropic[0]!.vendor).toBe(
        "Anthropic",
      );
    });
  });

  it("设为当前使用：未验证厂商菜单项禁用（先测试连接）；已验证厂商可切换", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "bot_get_config")
        return {
          // DeepSeek 未验证却占着 active（存量坏指针）；Kimi 已验证未启用
          modelsByProvider: {
            openai: [
              {
                id: "m-ds",
                label: "DS",
                baseUrl: "https://api.deepseek.com/v1",
                model: "deepseek-v4-flash",
                vendor: "DeepSeek",
                enabled: true,
              },
            ],
            anthropic: [
              {
                id: "m-kimi",
                label: "K2",
                baseUrl: "https://api.moonshot.cn/anthropic",
                model: "kimi-k3",
                vendor: "Kimi",
                enabled: true,
              },
            ],
          },
          activeModelId: { openai: "m-ds", anthropic: null },
          apiProvider: "openai",
          verifiedVendors: ["Kimi"],
          hasApiKey: true,
          bypassLlmOnPreStepHit: true,
        };
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
    await user.click(screen.getByRole("button", { name: "模型设置" }));
    // 左栏进 DeepSeek 厂商页（未验证）
    await user.click(await screen.findByRole("button", { name: "DeepSeek" }));
    expect(await screen.findByText("DS")).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "厂商 DeepSeek 更多操作" }),
    );
    // DeepSeek 是当前使用厂商但未验证 → 菜单项禁用（重接管也要先过测试连接）
    expect(
      screen.getByRole("menuitem", { name: "当前使用 ✓" }),
    ).toBeDisabled();
    // 切到 Kimi（已验证）→ 菜单项可点，点击后接管全局（状态更新为「当前使用 ✓」）
    await user.click(screen.getByRole("button", { name: "Kimi" }));
    expect(await screen.findByText("K2")).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "厂商 Kimi 更多操作" }),
    );
    const item = screen.getByRole("menuitem", { name: "设为当前使用" });
    expect(item).toBeEnabled();
    await user.click(item);
    await user.click(
      screen.getByRole("button", { name: "厂商 Kimi 更多操作" }),
    );
    expect(
      await screen.findByRole("menuitem", { name: "当前使用 ✓" }),
    ).toBeInTheDocument();
  });

  it("大模型 API 配置：切协议 → 列表整体切换（OpenAI 模型不在 Anthropic 协议下显示）", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "bot_get_config")
        return {
          modelsByProvider: {
            openai: [
              {
                id: "m1",
                label: "DeepSeek",
                baseUrl: "https://api.deepseek.com/v1",
                model: "deepseek-v4-flash",
              },
              {
                id: "m2",
                label: "Kimi",
                baseUrl: "https://api.moonshot.cn/v1",
                model: "kimi-k3",
              },
            ],
            anthropic: [
              {
                id: "m3",
                label: "Claude Sonnet",
                baseUrl: "https://api.anthropic.com",
                model: "claude-sonnet-4-5",
              },
            ],
          },
          activeModelId: { openai: "m1", anthropic: "m3" },
          hasApiKey: true,
          bypassLlmOnPreStepHit: true,
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
      return null;
    });
    render(<SettingsPage {...defaultProps} />);
    // 导航到「模型设置」分类；老条目无 vendor → 按协议名兜底为两个厂商行
    await user.click(screen.getByRole("button", { name: "模型设置" }));
    await user.click(screen.getByRole("button", { name: "OpenAI 兼容" }));
    // OpenAI 兼容厂商页：DeepSeek + Kimi 都在（紧凑行显示模型名文本），Claude 不在
    expect(await screen.findByText("DeepSeek")).toBeInTheDocument();
    expect(screen.getByText("Kimi")).toBeInTheDocument();
    expect(screen.queryByText("Claude Sonnet")).toBeNull();
    // 切到 Anthropic 兼容厂商页
    await user.click(screen.getByRole("button", { name: "Anthropic 兼容" }));
    // 现在 Anthropic 兼容页：只有 Claude
    expect(await screen.findByText("Claude Sonnet")).toBeInTheDocument();
    expect(screen.queryByText("DeepSeek")).toBeNull();
    expect(screen.queryByText("Kimi")).toBeNull();
    // 切回 OpenAI 兼容厂商页
    await user.click(screen.getByRole("button", { name: "OpenAI 兼容" }));
    // DeepSeek/Kimi 又回来了
    expect(await screen.findByText("DeepSeek")).toBeInTheDocument();
    expect(screen.getByText("Kimi")).toBeInTheDocument();
  });

  it("大模型 API 配置：删除模型 → 确认后立即落盘（active 被删由列表第一个顶替；删完列表空）", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "bot_get_config")
        return {
          modelsByProvider: {
            openai: [
              {
                id: "m1",
                label: "DeepSeek",
                baseUrl: "https://api.deepseek.com/v1",
                model: "deepseek-v4-flash",
              },
              {
                id: "m2",
                label: "Kimi",
                baseUrl: "https://api.moonshot.cn/v1",
                model: "kimi-k3",
              },
            ],
            anthropic: [],
          },
          activeModelId: { openai: "m1", anthropic: null },
          hasApiKey: true,
          bypassLlmOnPreStepHit: true,
        };
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
    // 导航到「模型设置」分类；进 OpenAI 兼容厂商页
    await user.click(screen.getByRole("button", { name: "模型设置" }));
    await user.click(screen.getByRole("button", { name: "OpenAI 兼容" }));
    expect(await screen.findByText("DeepSeek")).toBeInTheDocument();
    // 删 m1 → 进编辑态点删除（删除钮收在编辑态；确认框全局 mock 为 true）
    await user.click(screen.getAllByRole("button", { name: "编辑此模型" })[0]);
    await user.click(screen.getAllByTitle("删除此模型")[0]);
    // DeepSeek 没了，Kimi 还在；且立即落盘（bot_set_config 载荷只剩 Kimi，active 顶替为 m2）
    await waitFor(() => {
      expect(screen.queryByText("DeepSeek")).toBeNull();
      expect(screen.getByText("Kimi")).toBeInTheDocument();
    });
    await waitFor(() => {
      const setCalls = mocks.invokeMock.mock.calls.filter(
        (c) => c[0] === "bot_set_config",
      );
      expect(setCalls.length).toBeGreaterThan(0);
      const arg = setCalls[setCalls.length - 1]![1] as {
        config: {
          modelsByProvider: { openai: { id: string }[] };
          activeModelId: { openai: string | null };
        };
      };
      expect(arg.config.modelsByProvider.openai.map((m) => m.id)).toEqual([
        "m2",
      ]);
      expect(arg.config.activeModelId.openai).toBe("m2");
    });
    // 删 m2 → 进编辑态点删除，列表空回到「暂无模型」空态（厂商页保留）
    await user.click(screen.getAllByRole("button", { name: "编辑此模型" })[0]);
    await user.click(screen.getAllByTitle("删除此模型")[0]);
    expect(await screen.findByText(/暂无模型/)).toBeInTheDocument();
  });

  it("大模型 API 配置：保存 → bot_set_config 透传新结构（modelsByProvider + activeModelId + apiProvider），老 baseUrl/model 字段不再传", async () => {
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
        };
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
    // 导航到「模型设置」分类；添加厂商选「Anthropic」预设（携带 vendor 字段）
    await user.click(screen.getByRole("button", { name: "模型设置" }));
    await user.click(screen.getByRole("button", { name: "添加厂商" }));
    await user.click(screen.getByRole("button", { name: /Anthropic/ }));
    // 添加大模型（厂商页内）
    await user.click(screen.getByRole("button", { name: /添加模型/ }));
    // 填 label新行为紧凑行，点铅笔进编辑态后出现输入框）
    await user.click(
      screen.getAllByRole("button", { name: "编辑此模型" }).slice(-1)[0],
    );
    const labelInput = await screen.findByPlaceholderText(/DeepSeek \/ Kimi/);
    await user.type(labelInput, "Claude Sonnet");
    // 保存 → bot_set_config 透传新结构（aria-label 恒定，不受「已保存 ✓」瞬态影响）
    const saveBtns = await screen.findAllByRole("button", { name: "保存配置" });
    await user.click(saveBtns[saveBtns.length - 1]);
    await waitFor(() => {
      // 预设落盘也会发一次 bot_set_config；这里取「保存按钮」触发的最后一次
      const setCalls = mocks.invokeMock.mock.calls.filter(
        (c) => c[0] === "bot_set_config",
      );
      expect(setCalls.length).toBeGreaterThan(0);
      const setCall = setCalls[setCalls.length - 1];
      const arg = setCall![1] as {
        config: {
          modelsByProvider: {
            anthropic: Array<{ label: string; id: string; vendor?: string }>;
          };
          activeModelId: { anthropic: string | null };
          apiProvider: string;
        };
        apiKey: string | null;
        tavilyKey: string | null;
        braveKey: string | null;
      };
      // 预设自带 claude 行 + 新加的 Claude Sonnet 行，共两条
      expect(arg.config.modelsByProvider.anthropic).toHaveLength(2);
      expect(arg.config.modelsByProvider.anthropic[1].label).toBe(
        "Claude Sonnet",
      );
      // activeModelId.anthropic 仍是预设首条（添加不抢 active）
      expect(arg.config.modelsByProvider.anthropic[1].vendor).toBe("Anthropic");
      expect(arg.config.activeModelId.anthropic).toBe(
        arg.config.modelsByProvider.anthropic[0].id,
      );
      // apiProvider 透传
      expect(arg.config.apiProvider).toBe("anthropic");
      // 全局 maxTokens 兜底层已退役（每模型编辑里都有）：payload 不再带该键
      expect("maxTokens" in arg.config).toBe(false);
      // 顶层 key 字段仍按以前模式传 null
      expect(arg.apiKey).toBeNull();
      expect(arg.tavilyKey).toBeNull();
      expect(arg.braveKey).toBeNull();
    });
  });
});
