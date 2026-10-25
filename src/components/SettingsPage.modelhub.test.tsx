/// <reference types="node" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  render,
  screen,
  waitFor,
  fireEvent,
  within,
  cleanup,
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
import { ProviderLogo } from "./SettingsPage/ProviderLogo";
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
  // 模型库（内置 Rust meta 模块）对接
  // meta_* 命令走 invokeMock：默认实现返回 null（fetchProviders 降级内置预设），
  // 下列用例用 stubMetaInvoke 按命令覆盖（包一层当前实现，只接管 meta_*）。

  const metaOpenAI = {
    provider_key: "openai",
    provider_name: "OpenAI",
    logo_url: "https://models.dev/logos/openai.svg",
    fallback_color: "#000000",
    fallback_char: "O",
    default_base_url: "https://api.openai.com/v1",
    timeout: 60,
    source: "models_dev",
  };
  const metaDeepSeek = {
    provider_key: "deepseek",
    provider_name: "DeepSeek",
    logo_url: null,
    fallback_color: "#4d6bfe",
    fallback_char: "D",
    default_base_url: "https://api.deepseek.com",
    timeout: null,
    source: "models_dev",
  };
  const metaOpenAIModels = [
    {
      model_key: "openai/gpt-4o",
      provider_key: "openai",
      display_name: "GPT-4o",
      context_length: 128000,
      temperature: 0.7,
      top_p: 0.95,
      max_tokens: 4096,
      default_system_prompt: "You are helpful.",
      source: "models_dev",
    },
    {
      model_key: "openai/gpt-4o-mini",
      provider_key: "openai",
      display_name: "GPT-4o mini",
      context_length: 204800,
      temperature: null,
      top_p: null,
      max_tokens: null,
      default_system_prompt: null,
      source: "models_dev",
    },
  ];
  const emptyModelConfig = {
    modelsByProvider: { openai: [], anthropic: [] },
    activeModelId: { openai: null, anthropic: null },
    hasApiKey: false,
    bypassLlmOnPreStepHit: true,
  };

  /** 按命令路由的模型库 invoke mock；providers 传 null = meta_list_providers 抛错（降级内置预设）。
   *  包一层当前 invokeMock 实现，只接管 meta_* 命令（其余命令保持调用方已设的实现） */
  const stubMetaInvoke = (
    providers: unknown[] | null,
    modelsByKey: Record<string, unknown[]> = {},
    syncResult: unknown = { ok: true, providers: 2, models: 5 },
  ) => {
    const base = mocks.invokeMock.getMockImplementation()!;
    mocks.invokeMock.mockImplementation(
      async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "meta_list_providers") {
          if (providers === null) throw new Error("meta backend unavailable");
          return providers;
        }
        if (cmd === "meta_models_by_provider") {
          return modelsByKey[String(args?.providerKey)] ?? [];
        }
        if (cmd === "meta_sync_models_dev") return syncResult;
        return base(cmd, args);
      },
    );
  };

  it("模型库可用：添加厂商网格渲染远程服务商（非 8 预设；白名单收敛），搜索过滤生效", async () => {
    const user = userEvent.setup();
    mockVendorConfig(emptyModelConfig);
    stubMetaInvoke([metaOpenAI, metaDeepSeek]);
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "模型设置" }));
    await user.click(screen.getByRole("button", { name: "添加厂商" }));
    // 远程服务商出现；内置预设（MiniMax）不出现
    expect(
      await screen.findByRole("button", { name: /DeepSeek/ }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /OpenAI/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /MiniMax/ })).toBeNull();
    // 本地 lobehub 图标：按 provider_key 解析，渲染 vendor 资产 <img>（不再指向 models.dev）
    const openaiBtn = screen.getByRole("button", { name: /OpenAI/ });
    expectLogoSlug(openaiBtn.querySelector("img"), "openai");
    // 搜索过滤：输入 deepseek → 只剩 DeepSeek
    await user.type(screen.getByLabelText("搜索厂商"), "deepseek");
    expect(screen.queryByRole("button", { name: /OpenAI/ })).toBeNull();
    expect(
      screen.getByRole("button", { name: /DeepSeek/ }),
    ).toBeInTheDocument();
  });

  it("模型库可用：点选远程服务商 → bot_set_config 携带模型列表（首条 active+enabled、model 去前缀、推理参数映射）", async () => {
    const user = userEvent.setup();
    mockVendorConfig(emptyModelConfig);
    stubMetaInvoke([metaOpenAI, metaDeepSeek], { openai: metaOpenAIModels });
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "模型设置" }));
    await user.click(screen.getByRole("button", { name: "添加厂商" }));
    await user.click(await screen.findByRole("button", { name: /OpenAI/ }));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_set_config",
        expect.objectContaining({
          config: expect.objectContaining({ apiProvider: "openai" }),
        }),
      );
    });
    const setCalls = mocks.invokeMock.mock.calls.filter(
      (c) => c[0] === "bot_set_config",
    );
    const arg = setCalls[setCalls.length - 1]![1] as {
      config: {
        apiProvider: string;
        modelsByProvider: {
          openai: Array<Record<string, unknown> & { id: string }>;
        };
        activeModelId: { openai: string | null };
      };
    };
    const list = arg.config.modelsByProvider.openai;
    expect(list).toHaveLength(2);
    // 首条：启用 + active；model 去掉 "openai/" 前缀；推理参数全量映射
    expect(list[0]).toMatchObject({
      label: "GPT-4o",
      model: "gpt-4o",
      baseUrl: "https://api.openai.com/v1",
      vendor: "OpenAI",
      contextK: 128,
      temperature: 0.7,
      topP: 0.95,
      maxTokens: 4096,
      systemPrompt: "You are helpful.",
      enabled: true,
    });
    // 其余：enabled:false；context_length/1000 一位小数（204800 → 204.8）
    expect(list[1]).toMatchObject({
      label: "GPT-4o mini",
      model: "gpt-4o-mini",
      contextK: 204.8,
      enabled: false,
    });
    // 空值推理参数不写字段
    expect(list[1]).not.toHaveProperty("temperature");
    expect(list[1]).not.toHaveProperty("topP");
    expect(list[1]).not.toHaveProperty("maxTokens");
    expect(list[1]).not.toHaveProperty("systemPrompt");
    expect(arg.config.activeModelId.openai).toBe(list[0].id);
  });

  // 每模型推理参数接线

  it("推理参数接线：编辑态填四参数 → 保存落盘 bot_set_config；重载后编辑态回显 + Anthropic 说明行", async () => {
    const user = userEvent.setup();
    let stored: Record<string, unknown> | null = null;
    mocks.invokeMock.mockImplementation(
      async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "bot_get_enabled") return true;
        if (cmd === "bot_get_config") return stored ?? deepseekVendorConfig;
        if (cmd === "bot_set_config") {
          stored = args!.config as Record<string, unknown>;
          return null;
        }
        if (cmd === "bot_test_connection") return { ok: true, status: 200 };
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
    await openVendorPage(user, "DeepSeek");
    // 铅笔进编辑态（非受控输入 + onBlur 提交）
    await user.click(await screen.findByRole("button", { name: "编辑此模型" }));
    const fill = async (label: string, value: string) => {
      const el = screen.getByLabelText(label);
      await user.type(el, value);
      fireEvent.blur(el);
    };
    await fill("temperature", "0.5");
    await fill("top_p", "0.9");
    await fill("max_tokens", "16384");
    await fill("system prompt", "用中文回复");
    // 厂商页「保存配置」→ bot_set_config 落盘
    await user.click(screen.getByRole("button", { name: "保存配置" }));
    await waitFor(() => expect(stored).not.toBeNull());
    const saved = (
      stored as unknown as {
        modelsByProvider: { openai: Array<Record<string, unknown>> };
      }
    ).modelsByProvider.openai[0];
    expect(saved).toMatchObject({
      temperature: 0.5,
      topP: 0.9,
      maxTokens: 16384,
      systemPrompt: "用中文回复",
    });
    // 重载回显：以「盘上」配置重新挂载，编辑态输入框带出已存值
    cleanup();
    render(<SettingsPage {...defaultProps} />);
    await openVendorPage(user, "DeepSeek");
    await user.click(await screen.findByRole("button", { name: "编辑此模型" }));
    expect(screen.getByLabelText("temperature")).toHaveValue("0.5");
    expect(screen.getByLabelText("top_p")).toHaveValue("0.9");
    expect(screen.getByLabelText("max_tokens")).toHaveValue("16384");
    expect(screen.getByLabelText("system prompt")).toHaveValue("用中文回复");
    // 说明行：max_tokens 仅 Anthropic 格式生效（避免 OpenAI 格式用户填了没反应）
    expect(
      screen.getByText(/max_tokens 仅 Anthropic 格式生效/),
    ).toBeInTheDocument();
  });

  it("模型设置页首引导：有带 vendor 条目但 verifiedVendors 为空 → 显示升级引导；已验证或有条目无 vendor 不显示", async () => {
    const user = userEvent.setup();
    // 场景 1：带 vendor 条目 + verifiedVendors 空 → 引导出现
    mockVendorConfig(deepseekVendorConfig);
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "模型设置" }));
    expect(
      await screen.findByRole("note", { name: "厂商可用性引导" }),
    ).toHaveTextContent(/升级后需逐厂商点一次插头恢复可用/);
    cleanup();
    // 场景 2：verifiedVendors 已有厂商 → 不再引导
    mockVendorConfig({
      ...deepseekVendorConfig,
      verifiedVendors: ["DeepSeek"],
    });
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "模型设置" }));
    expect(
      await screen.findByRole("button", { name: /DeepSeek/ }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("note", { name: "厂商可用性引导" })).toBeNull();
    cleanup();
    // 场景 3：条目无 vendor（老配置按协议名兜底分组）→ 不引导
    mockVendorConfig({
      modelsByProvider: {
        openai: [
          {
            id: "m1",
            label: "DeepSeek Chat",
            model: "deepseek-chat",
            baseUrl: "https://api.deepseek.com",
          },
        ],
        anthropic: [],
      },
      activeModelId: { openai: "m1", anthropic: null },
      apiProvider: "openai",
      hasApiKey: true,
      bypassLlmOnPreStepHit: true,
    });
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "模型设置" }));
    await user.click(
      await screen.findByRole("button", { name: "OpenAI 兼容" }),
    );
    expect(screen.queryByRole("note", { name: "厂商可用性引导" })).toBeNull();
  });

  it("模型库同名厂商合并：已存 DeepSeek 条目时点选 provider_name 大小写不同的同厂商 → 归一化查重替换，不并存两条目；厂商名保留既有（keyring 条目不动）", async () => {
    const user = userEvent.setup();
    mockVendorConfig(deepseekVendorConfig);
    // 模型库同名厂商：provider_name 与既有 vendor 仅大小写不同（归一化后同名）
    stubMetaInvoke([{ ...metaDeepSeek, provider_name: "deepseek" }], {
      deepseek: metaOpenAIModels,
    });
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "模型设置" }));
    await user.click(screen.getByRole("button", { name: "添加厂商" }));
    await user.click(await screen.findByRole("button", { name: /deepseek/ }));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_set_config",
        expect.objectContaining({}),
      );
    });
    const setCalls = mocks.invokeMock.mock.calls.filter(
      (c) => c[0] === "bot_set_config",
    );
    const arg = setCalls[setCalls.length - 1]![1] as {
      config: {
        modelsByProvider: {
          openai: Array<Record<string, unknown> & { label: string }>;
        };
      };
    };
    const list = arg.config.modelsByProvider.openai;
    // 合并而非并存：旧「DeepSeek Chat」条目被模型库列表替换，vendor 保留既有名
    expect(list).toHaveLength(metaOpenAIModels.length);
    expect(list.every((m) => m.vendor === "DeepSeek")).toBe(true);
    expect(list.some((m) => m.label === "GPT-4o")).toBe(true);
    expect(list.some((m) => m.label === "DeepSeek Chat")).toBe(false);
  });

  it("模型库不可用：meta_list_providers 抛错 → 回退 8 预设网格 + 提示文案，预设仍可点选", async () => {
    const user = userEvent.setup();
    mockVendorConfig(emptyModelConfig);
    stubMetaInvoke(null);
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "模型设置" }));
    await user.click(screen.getByRole("button", { name: "添加厂商" }));
    expect(
      await screen.findByText(/模型库为空或同步中——启动时自动同步 models\.dev/),
    ).toBeInTheDocument();
    // 内置预设网格保持可用
    expect(
      screen.getByRole("button", { name: /DeepSeek/ }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /MiniMax/ })).toBeInTheDocument();
    // 预设点选链路不破坏：选 DeepSeek → 预设模型行出现
    await user.click(screen.getByRole("button", { name: /DeepSeek/ }));
    expect(await screen.findByText("deepseek-chat")).toBeInTheDocument();
  });

  it("meta_list_providers reject（CommandError 形态）→ 降级内置预设网格", async () => {
    const user = userEvent.setup();
    mockVendorConfig(emptyModelConfig);
    // invoke reject 的非 Error 对象（Tauri CommandError 序列化形态）也必须降级
    const base = mocks.invokeMock.getMockImplementation()!;
    mocks.invokeMock.mockImplementation(
      async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "meta_list_providers")
          throw {
            code: "META_QUERY_FAILED",
            message: "库查询失败",
            recoverable: true,
          };
        return base(cmd, args);
      },
    );
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "模型设置" }));
    await user.click(screen.getByRole("button", { name: "添加厂商" }));
    expect(await screen.findByText(/模型库为空或同步中/)).toBeInTheDocument();
    // 内置预设网格完整呈现
    expect(
      screen.getByRole("button", { name: /DeepSeek/ }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /Anthropic/ }),
    ).toBeInTheDocument();
    // 远程网格不出现；「更新模型库」按钮保留（空库/降级时它是唯一的手动同步入口）
    expect(
      screen.getByRole("button", { name: /更新模型库/ }),
    ).toBeInTheDocument();
    expect(screen.queryByLabelText("搜索厂商")).toBeNull();
  });

  it("厂商页「从模型库添加」：列出未添加的模型，选中追加 enabled:false 条目并落盘", async () => {
    const user = userEvent.setup();
    mockVendorConfig({
      modelsByProvider: {
        openai: [
          {
            id: "m1",
            label: "GPT-4o",
            model: "gpt-4o",
            baseUrl: "https://api.openai.com/v1",
            vendor: "OpenAI",
          },
        ],
        anthropic: [],
      },
      activeModelId: { openai: "m1", anthropic: null },
      apiProvider: "openai",
      hasApiKey: true,
      bypassLlmOnPreStepHit: true,
    });
    stubMetaInvoke([metaOpenAI, metaDeepSeek], { openai: metaOpenAIModels });
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "模型设置" }));
    await user.click(await screen.findByRole("button", { name: "OpenAI" }));
    // 服务可用且厂商在服务中 → 「从模型库添加」入口出现
    await user.click(
      await screen.findByRole("button", { name: /从模型库添加/ }),
    );
    const listbox = await screen.findByRole("listbox", {
      name: "从模型库添加",
    });
    // 已添加的 gpt-4o 不出现，未添加的 GPT-4o mini 出现
    expect(
      within(listbox).queryByRole("option", { name: "GPT-4o" }),
    ).toBeNull();
    const option = within(listbox).getByRole("option", { name: "GPT-4o mini" });
    await user.click(option);
    await waitFor(() => {
      const setCalls = mocks.invokeMock.mock.calls.filter(
        (c) => c[0] === "bot_set_config",
      );
      expect(setCalls.length).toBeGreaterThan(0);
      const arg = setCalls[setCalls.length - 1]![1] as {
        config: {
          modelsByProvider: { openai: Array<Record<string, unknown>> };
        };
      };
      const list = arg.config.modelsByProvider.openai;
      expect(list).toHaveLength(2);
      // 新条目：字段映射同网格点选，enabled:false，baseUrl 继承厂商当前值
      expect(list[1]).toMatchObject({
        label: "GPT-4o mini",
        model: "gpt-4o-mini",
        baseUrl: "https://api.openai.com/v1",
        vendor: "OpenAI",
        contextK: 204.8,
        enabled: false,
      });
    });
  });

  it("「⟳ 更新模型库」：调 meta_sync_models_dev，完成后刷新服务商列表并显示结果", async () => {
    const user = userEvent.setup();
    mockVendorConfig(emptyModelConfig);
    stubMetaInvoke([metaOpenAI], {}, { ok: true, providers: 7, models: 42 });
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "模型设置" }));
    await user.click(screen.getByRole("button", { name: "添加厂商" }));
    await screen.findByRole("button", { name: /OpenAI/ });
    await user.click(screen.getByRole("button", { name: /更新模型库/ }));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("meta_sync_models_dev");
    });
    expect(
      await screen.findByText(/已更新：7 个厂商 \/ 42 个模型/),
    ).toBeInTheDocument();
    // 同步完成后重新拉取服务商列表（首次挂载 1 次 + 同步后刷新 1 次）
    await waitFor(() => {
      const listCalls = mocks.invokeMock.mock.calls.filter(
        (c) => c[0] === "meta_list_providers",
      );
      expect(listCalls.length).toBeGreaterThanOrEqual(2);
    });
  });

  it("ProviderLogo：别名命中彩色图标（moonshotai→kimi）；未命中回退色块+首字母", () => {
    const { container, rerender } = render(
      <ProviderLogo name="Moonshot AI" providerKey="moonshotai" />,
    );
    // providerKey 走别名表 → kimi 彩色图标（本地 lobehub 资产）
    expectLogoSlug(container.querySelector("img"), "kimi");
    // name 直接命中 slug
    rerender(<ProviderLogo name="DeepSeek" />);
    expectLogoSlug(container.querySelector("img"), "deepseek");
    // 带域名括号备注的显示名（models.dev「MiniMax (minimax.cn)」）：归一化后命中
    rerender(<ProviderLogo name="MiniMax (minimax.cn)" />);
    expectLogoSlug(container.querySelector("img"), "minimax");
    // 套餐后缀（models.dev「MiniMax Token Plan (minimax.cn)」）：归一化去 plan 后缀后命中
    rerender(<ProviderLogo name="MiniMax Token Plan (minimax.cn)" />);
    expectLogoSlug(container.querySelector("img"), "minimax");
    // 中文显示名走别名表
    rerender(<ProviderLogo name="阿里云百炼" />);
    expectLogoSlug(container.querySelector("img"), "bailian");
    // 未命中：圆形色块 + fallbackChar
    rerender(
      <ProviderLogo name="我的厂商" fallbackColor="#f55036" fallbackChar="G" />,
    );
    expect(container.querySelector("img")).toBeNull();
    const circle = within(container as HTMLElement).getByText("G");
    expect(circle).toBeInTheDocument();
    expect(circle.style.background).toBeTruthy();
    // 未命中且无 fallbackChar：取名称首字
    rerender(<ProviderLogo name="我的厂商" />);
    expect(container.querySelector("img")).toBeNull();
    expect(
      within(container as HTMLElement).getByText("我"),
    ).toBeInTheDocument();
  });

  it("左栏厂商列表：models.dev 长名称厂商也显示 logo（providerKey 透传 + plan 后缀归一）", async () => {
    const user = userEvent.setup();
    mockVendorConfig({
      modelsByProvider: {
        openai: [],
        anthropic: [
          {
            id: "m1",
            label: "MiniMax-M2",
            model: "MiniMax-M2",
            baseUrl: "https://api.minimax.cn/anthropic/v1",
            vendor: "MiniMax Token Plan (minimax.cn)",
          },
        ],
      },
      activeModelId: { openai: null, anthropic: "m1" },
      apiProvider: "anthropic",
      hasApiKey: false,
      bypassLlmOnPreStepHit: true,
    });
    stubMetaInvoke([
      {
        provider_key: "minimax-cn-coding-plan",
        provider_name: "MiniMax Token Plan (minimax.cn)",
        logo_url: null,
        fallback_color: "#e60033",
        fallback_char: "M",
        default_base_url: "https://api.minimax.cn/anthropic/v1",
        timeout: null,
        source: "models_dev",
      },
    ]);
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "模型设置" }));
    // 左栏厂商行：meta 加载后 providerKey 透传命中 minimax 图标（不再是兜底 M 色块）
    const row = await screen.findByRole("button", {
      name: /MiniMax Token Plan/,
    });
    await waitFor(() => {
      expectLogoSlug(row.querySelector("img"), "minimax");
    });
  });
});
