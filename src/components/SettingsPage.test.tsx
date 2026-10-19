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
import { SettingsPage } from "./SettingsPage";
import { ProviderLogo } from "./SettingsPage/ProviderLogo";
import type { ThemeSetting } from "../theme";

/** logo src 断言：vite 把小 SVG 内联为 data URI（解码后含 slug），大资产为 lobe/ 路径 */
const expectLogoSlug = (img: Element | null, slug: string) => {
  const src = img?.getAttribute("src") ?? "";
  expect(src).not.toBe("");
  const hay = src.startsWith("data:") ? decodeURIComponent(src) : src;
  expect(hay.toLowerCase()).toContain(slug);
};

// vi.mock 工厂会被提升到顶部，因此共享 mock 变量必须用 vi.hoisted 包裹
const mocks = vi.hoisted(() => {
  const invokeMock = vi.fn();
  const emitMock = vi.fn();
  const openMock = vi.fn();
  const openUrlMock = vi.fn();
  const askMock = vi.fn();
  return { invokeMock, emitMock, openMock, openUrlMock, askMock };
});

mocks.invokeMock.mockImplementation(async (cmd: string) => {
  switch (cmd) {
    case "profile_get":
      return {
        user: { name: "我", avatarId: null },
        bot: { name: "机器人", avatarId: null },
      };
    case "bot_get_enabled":
      return false;
    case "bot_get_config":
      return {
        baseUrl: "",
        model: "",
        hasApiKey: false,
        bypassLlmOnPreStepHit: true,
        // 字体大小：默认 small（「目前字号为小」）
        uiFontSize: "small",
      };
    case "py_get_enabled":
      return false;
    // Agent 运行参数卡的元数据（默认值/可填范围/当前生效值）
    case "bot_effective_params":
      return paramViews;
    case "py_env_check":
      return { available: false, python: "", version: "", libs: [] };
    case "api_status":
      // 模拟 disabled 状态:后端用 skip_serializing_if 剔除 token 字段
      return { enabled: false, port: 4763 };
    case "skills_list":
      return [];
    case "onions_list":
      return [];
    case "error_log_read":
      return "（暂无日志）";
    case "logs_bundle_text":
      return "===== error.log =====\n";
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
mocks.openUrlMock.mockImplementation(async () => {});

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

/** bot_effective_params 的返回体（后端 bot::params::ParamView 的线格式）。
 *  前端不再自留一份默认数字——这些值就是页面显示「默认/可填范围/当前生效」
 *  的唯一来源。 */
const paramViews = [
  {
    key: "loop.max_rounds",
    label: "模型循环最大轮数",
    category: "loop",
    hint: "单次对话/任务执行的 LLM 轮数上限；调大=更复杂任务但失控时更费 token",
    value: "50",
    default: "50",
    source: "default",
    editable: true,
    // 上界故意用 1000（真值 200）：预设里的「拉到上限」若写死字面量就露馅——
    // 钳制只会把超出的往下拉，写死 200 落盘还是 200，派生则落盘 1000
    min: 5,
    max: 1000,
  },
  {
    key: "loop.max_function_calls",
    label: "单次请求工具调用熔断",
    category: "loop",
    hint: "全域工具调用熔断上限（子 agent 预算同源此值）",
    value: "100",
    default: "100",
    source: "default",
    editable: true,
    min: 1,
    max: 500,
  },
  {
    key: "tools.max_output_bytes",
    label: "工具结果截断字节数",
    category: "tools",
    hint: "单条工具结果回填模型上下文的截断上限（字节，UTF-8）；设 0 = 显式关闭截断",
    value: "30000",
    default: "30000",
    source: "default",
    editable: true,
    min: 0,
    max: 200000,
  },
  {
    // 故意用 45 而不是真实的 30：页面显示的数必须来自后端参数表，
    // 前端自留一份「30」的话这条就会露馅
    key: "trace.retention_days",
    label: "执行痕迹保留期（天）",
    category: "tools",
    hint: "痕迹三表与回滚快照的保留期：启动即清理一次，此后每天一次",
    value: "45",
    default: "45",
    source: "hardcoded",
    editable: false,
    // 范围也故意用非真值（真值 1/365）：title 里的「N–M」必须跟着参数表走
    min: 2,
    max: 400,
  },
  {
    // 记忆抽取限频（秒）：真值 1800（30 分钟），这里故意给 900（15 分钟）——
    // 页面文案里的分钟数只能从后端取，写死「30 分钟」这条就会露馅
    key: "memory.extract_min_interval_secs",
    label: "记忆抽取最小间隔（秒）",
    category: "memory",
    hint: "同一会话两次自动抽取的最小间隔",
    value: "900",
    default: "900",
    source: "hardcoded",
    editable: false,
    min: null,
    max: null,
  },
  {
    // 全局 max_tokens：真默认 32768（后端 schema::DEFAULT_MAX_TOKENS），
    // 范围 256..=200000
    key: "model.max_tokens",
    label: "全局 max_tokens",
    category: "model",
    hint: "仅 Anthropic 协议发送；OpenAI 兼容模式不发送",
    value: "32768",
    default: "32768",
    source: "default",
    editable: true,
    min: 256,
    max: 200000,
  },
  {
    // Python 超时：默认 60 是后端真值（py::runtime 常量），前端此前手写这两个数。
    // 上界故意用 1000（真值 300）：预设「拉到上限」若写死就露馅（同上）
    key: "tools.python_timeout_secs",
    label: "Python 执行超时（秒）",
    category: "tools",
    hint: "run_python 默认超时；工具参数可单次覆盖",
    value: "60",
    default: "60",
    source: "default",
    editable: true,
    min: 1,
    max: 1000,
  },
];

/** 机器人分区的 Agent 参数卡：进分区 + 拿到参数表 */
async function openAgentParams(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("button", { name: "机器人" }));
  return screen.findByText("Agent 运行参数");
}

/** 机器人分区的 invoke mock（标准命令 + Agent 参数表）。
 *  必须自带 bot_effective_params：前一个用例覆写过 invoke 实现，未识别的
 *  命令一律返 null——参数表拉不到时页面会走降级渲染（不显示默认值/范围）。 */
const agentParamsMock = async (cmd: string): Promise<unknown> => {
  switch (cmd) {
    case "bot_effective_params":
      return paramViews;
    case "bot_get_config":
      return {
        baseUrl: "",
        model: "",
        hasApiKey: false,
        bypassLlmOnPreStepHit: true,
        uiFontSize: "small",
      };
    case "bot_set_config":
      return null;
    case "profile_get":
      return { user: { name: "我", avatarId: null }, bot: { name: "机器人", avatarId: null } };
    case "bot_get_enabled":
      return false;
    case "py_get_enabled":
      return false;
    case "py_env_check":
      return { available: false, python: "", version: "", libs: [] };
    case "api_status":
      return { enabled: false, port: 4763 };
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
};

describe("SettingsPage", () => {
  it("初始渲染：所有主要 section 都出现", async () => {
    render(<SettingsPage {...defaultProps} />);
    //  十分类：逐分类导航断言各面板标题（惰性挂载，未激活不渲染）
    expect(screen.getByText("个人资料")).toBeInTheDocument();
    // 侧栏分类名与面板标题同文（「基础设置」两处），取全量断言
    expect(screen.getAllByText("基础设置").length).toBeGreaterThan(0);
    expect(screen.getByText("浅色")).toBeInTheDocument();
    expect(screen.getByText("深色")).toBeInTheDocument();
    expect(screen.getByText("跟随系统")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "数据管理" }));
    expect(screen.getByText("任务数据管理")).toBeInTheDocument();
    expect(screen.getByText("工作区管理")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "机器人" }));
    expect(screen.getByText("机器人设置")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "模型设置" }));
    fireEvent.click(screen.getByRole("button", { name: "记忆" }));
    expect(screen.getByText("记忆整理")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "技能" }));
    expect(screen.getByText("机器人技能")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "MCP 服务" }));
    expect(screen.getByText("MCP 服务器（外部工具）")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "任务图谱" }));
    expect(
      screen.getByRole("switch", { name: "只看我的任务" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("radiogroup", { name: "布局松散度" }),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "记忆" }));
    expect(screen.getByText("记忆库")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "文件整理" }));
    expect(screen.getByText("整理来源")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "词元统计" }));
    // 侧栏分类名与空态标题同文，取全量断言
    expect(screen.getAllByText("词元统计").length).toBeGreaterThan(0);
    // 等待初始 invoke 异步加载完成
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_get_enabled");
    });
  });

  it("任务卡归档时间（数据管理）：默认 7 天，改 30 保存 → bot_set_config 携带 archiveAfterDays=30", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_config")
        return {
          baseUrl: "",
          model: "",
          hasApiKey: false,
          bypassLlmOnPreStepHit: true,
          archiveAfterDays: 7,
        };
      if (cmd === "bot_set_config") return null;
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarId: null },
          bot: { name: "机器人", avatarId: null },
        };
      if (cmd === "bot_get_enabled") return false;
      if (cmd === "py_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status")
        return { rules_count: 0, poll_interval_secs: 600 };
      if (cmd === "migration_log_read") return "";
      return null;
    });
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "数据管理" }));
    // 默认显示 7（后端返 7；后端 None 亦显示 7）
    const input = await screen.findByLabelText("任务卡归档天数");
    expect(input).toHaveValue("7");
    await user.clear(input);
    await user.type(input, "30");
    // 本卡「保存配置」钮（aria-label 恒定，可见文本随状态变）
    await user.click(screen.getByLabelText("保存配置"));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_set_config",
        expect.objectContaining({
          config: expect.objectContaining({ archiveAfterDays: 30 }),
        }),
      );
    });
    // 越界钳制：0 视为未配置（null → 后端回默认 7）；500 钳到 365
    await user.clear(input);
    await user.type(input, "500");
    await user.click(screen.getByLabelText("保存配置"));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_set_config",
        expect.objectContaining({
          config: expect.objectContaining({ archiveAfterDays: 365 }),
        }),
      );
    });
  });

  it("工具调用触发器关键词：删词保存 → bot_set_config 携带改后的词表", async () => {
    // 这条盯的是「整体替换写」：漏传 guardClaimKeywords = 用户词表被清空。
    // 后端另有 preserve_guard_keywords 兜底，但前端该带的必须带。
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bot_get_config")
        return {
          baseUrl: "",
          model: "",
          hasApiKey: false,
          bypassLlmOnPreStepHit: true,
          guardClaimKeywords: ["执行完毕", "跑完了", "已执行"],
          guardClaimDefaultKeywords: ["执行完毕", "跑完了", "已执行"],
        };
      if (cmd === "bot_set_config") return null;
      if (cmd === "profile_get")
        return { user: { name: "我", avatarId: null }, bot: { name: "机器人", avatarId: null } };
      if (cmd === "bot_get_enabled") return false;
      if (cmd === "py_get_enabled") return false;
      if (cmd === "py_env_check")
        return { available: false, python: "", version: "", libs: [] };
      // 机器人分区有张卡直接读 status.enabled，兜底 null 会渲染崩
      if (cmd === "api_status") return { enabled: false, port: 4763 };
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status") return { rules_count: 0, poll_interval_secs: 600 };
      if (cmd === "migration_log_read") return "";
      return null;
    });
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "机器人" }));
    const ta = (await screen.findByPlaceholderText(
      /执行完毕/,
    )) as HTMLTextAreaElement;
    expect(ta.value).toBe("执行完毕\n跑完了\n已执行");
    await user.clear(ta);
    await user.type(ta, "搞定{Enter}执行完毕");
    await user.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_set_config",
        expect.objectContaining({
          config: expect.objectContaining({
            guardClaimKeywords: ["搞定", "执行完毕"],
          }),
        }),
      );
    });
  });

  it("Agent 运行参数卡：默认值/可填范围/当前生效值来自后端参数表", async () => {
    // 这条盯的是「页面显示的数 = 真正生效的数」：默认值与范围此前是前端
    // 自留的一份硬编码，后端改了默认页面还写着旧数。
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(agentParamsMock);
    render(<SettingsPage {...defaultProps} />);
    await openAgentParams(user);
    const desc = await screen.findByText(/默认 50/);
    expect(desc.textContent).toContain("可填 5–1000");
    expect(desc.textContent).toContain("当前生效 50");
    expect(desc.textContent).toContain("内置默认");
    // 输入框的 placeholder = 后端给的默认值（留空 = 用内置默认）
    const input = screen.getByLabelText(
      "模型循环最大轮数",
    ) as HTMLInputElement;
    expect(input.value).toBe("");
    expect(input.placeholder).toBe("50");
  });

  it("Agent 运行参数卡：改数值保存 → 落盘钳到后端给的可填范围", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(agentParamsMock);
    render(<SettingsPage {...defaultProps} />);
    await openAgentParams(user);
    const input = (await screen.findByLabelText(
      "模型循环最大轮数",
    )) as HTMLInputElement;
    await user.clear(input);
    await user.type(input, "9999");
    await user.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_set_config",
        expect.objectContaining({
          config: expect.objectContaining({ maxRounds: 1000 }),
        }),
      );
    });
  });

  it("工具结果截断字节数填 0 → 落盘 0（显式关闭截断），不是当成未配置", async () => {
    // 0 是这一项的合法第三态（关闭截断）。此前前端 `n > 0` 把它判成 null，
    // 用户填 0 存下去还是默认 30000 字节——设了等于没设。
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(agentParamsMock);
    render(<SettingsPage {...defaultProps} />);
    await openAgentParams(user);
    const input = (await screen.findByLabelText(
      "工具结果截断字节数",
    )) as HTMLInputElement;
    await user.clear(input);
    await user.type(input, "0");
    await user.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_set_config",
        expect.objectContaining({
          config: expect.objectContaining({ maxToolOutputBytes: 0 }),
        }),
      );
    });
  });

  it("Python 默认超时：默认秒数与上限取自后端参数表", async () => {
    // 前端此前手写「留空 = 60 秒 / 上限 300 秒」——后端常量一改页面就撒谎
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(agentParamsMock);
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "机器人" }));
    expect(
      await screen.findByText(/留空 = 60 秒；大计算可调大，上限 1000 秒/),
    ).toBeInTheDocument();
  });

  it("Python 默认超时：填 9999 保存 → 落盘钳到后端给的上限", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(agentParamsMock);
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "机器人" }));
    const input = (await screen.findByLabelText(
      "Python 默认超时（秒）",
    )) as HTMLInputElement;
    await user.clear(input);
    await user.type(input, "9999");
    await user.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_set_config",
        expect.objectContaining({
          config: expect.objectContaining({ pythonTimeoutSecs: 1000 }),
        }),
      );
    });
  });

  it("一键预设「保守」：轮数/调用数/超时全部留空（用后端内置默认），不钉成字面量", async () => {
    // 预设里写死 50/100/60 = 把后端默认值抄成前端副本：后端改默认，预设还是
    // 旧数，页面说「保守档」实际按另一套跑。留空才是「交给后端默认」。
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(agentParamsMock);
    render(<SettingsPage {...defaultProps} />);
    await openAgentParams(user);
    await user.click(await screen.findByRole("button", { name: "保守" }));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_set_config",
        expect.objectContaining({
          config: expect.objectContaining({
            maxRounds: null,
            maxFunctionCalls: null,
            pythonTimeoutSecs: null,
          }),
        }),
      );
    });
  });

  it("一键预设「放开」：轮数与 Python 超时取参数表上界，调用数是产品档位值 300", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(agentParamsMock);
    render(<SettingsPage {...defaultProps} />);
    await openAgentParams(user);
    await user.click(await screen.findByRole("button", { name: "放开" }));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_set_config",
        expect.objectContaining({
          config: expect.objectContaining({
            // 上界来自参数表（mock：轮数/超时上界都是 1000，非真值——
            // 写死字面量时钳制不会把它拉到上界，断言就能分辨）
            maxRounds: 1000,
            pythonTimeoutSecs: 1000,
            // 这一项后端没有对应常量（硬顶 500、默认 100），是产品选的中间档
            maxFunctionCalls: 300,
          }),
        }),
      );
    });
  });

  it("执行痕迹保留期（数据管理）：默认天数取自后端参数表，前端不自留一份", async () => {
    // 那个「30」此前在仓库里有 6 份副本（后端常量 / 参数表字面量 / 前端文案 /
    // 前端输入框初值 / 前端 title / 前端兜底）。现在页面只从后端取。
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(agentParamsMock);
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "数据管理" }));
    // mock 里的保留期是 45（真实值是 30）——显示 45 才说明数来自后端
    expect(await screen.findByText(/默认保留 45 天/)).toBeInTheDocument();
    const input = (await screen.findByLabelText("保留天数")) as HTMLInputElement;
    expect(input.value).toBe("");
    expect(input.placeholder).toBe("45");
  });

  it("执行痕迹清理：保留天数留空 → 传 null 由后端按内置默认处理", async () => {
    // 前端不再自己兜一个 30：空值直接交回后端（trace_clear_before 的 None 分支
    // 走 TRACE_RETENTION_DAYS），前端那份副本删掉后就不会漂
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "trace_clear_before") return 3;
      if (cmd === "bot_effective_params") return paramViews;
      if (cmd === "bot_get_config")
        return {
          baseUrl: "",
          model: "",
          hasApiKey: false,
          bypassLlmOnPreStepHit: true,
          uiFontSize: "small",
        };
      if (cmd === "profile_get")
        return { user: { name: "我", avatarId: null }, bot: { name: "机器人", avatarId: null } };
      if (cmd === "bot_get_enabled") return false;
      if (cmd === "py_get_enabled") return false;
      if (cmd === "py_env_check")
        return { available: false, python: "", version: "", libs: [] };
      if (cmd === "api_status") return { enabled: false, port: 4763 };
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status") return { rules_count: 0, poll_interval_secs: 600 };
      if (cmd === "migration_log_read") return "";
      return null;
    });
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "数据管理" }));
    const input = (await screen.findByLabelText("保留天数")) as HTMLInputElement;
    expect(input.value).toBe("");
    await user.click(screen.getByRole("button", { name: "清理" }));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("trace_clear_before", {
        days: null,
      });
    });
  });

  it("执行痕迹保留期（数据管理）：title 里的范围取自参数表，前端不写死 1–365", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(agentParamsMock);
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "数据管理" }));
    const input = (await screen.findByLabelText("保留天数")) as HTMLInputElement;
    // mock 的范围是 2–400（真值 1/365）：写死就露馅
    expect(input.title).toBe("保留天数（2–400，默认 45）");
  });

  it("自动记忆抽取（记忆分区）：限频分钟数取自参数表，前端不写死 30 分钟", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(agentParamsMock);
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "记忆" }));
    // mock 给 900 秒 = 15 分钟（真值 1800 = 30 分钟）
    expect(await screen.findByText(/≥15 分钟一次/)).toBeInTheDocument();
  });

  it("Python 开关（机器人分区）：开启确认框里的超时取自参数表，前端不写死 60 秒", async () => {
    const user = userEvent.setup();
    // 生效超时故意给 120（真值 60）：写死「60 秒超时」那条就会露馅
    const views = paramViews.map((p) =>
      p.key === "tools.python_timeout_secs" ? { ...p, value: "120" } : p,
    );
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "bot_effective_params" ? views : agentParamsMock(cmd),
    );
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "机器人" }));
    const title = await screen.findByText("允许机器人执行 Python");
    // 开关按钮与标题同一行（标题 <p> → 文字 div → flex 行）
    const row = title.parentElement?.parentElement as HTMLElement;
    await user.click(within(row).getByRole("button"));
    // 二次确认走 Tauri 原生 dialog（useConfirm → ask），不是 window.confirm
    await waitFor(() => expect(mocks.askMock).toHaveBeenCalled());
    const text = String(mocks.askMock.mock.calls[0]?.[0] ?? "");
    // 120 秒 → 「2 分钟」（走 fmtDurationSecs），不是手写的「60 秒」
    expect(text).toContain("2 分钟");
    expect(text).not.toContain("60 秒");
  });

  it("Python 开关：参数表拉不到时确认框不写秒数（不留手写副本）", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      // 参数表拉不到 → 整块降级，此时文案里不能出现任何秒数
      if (cmd === "bot_effective_params") return null;
      return agentParamsMock(cmd);
    });
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "机器人" }));
    const title = await screen.findByText("允许机器人执行 Python");
    const row = title.parentElement?.parentElement as HTMLElement;
    await user.click(within(row).getByRole("button"));
    await waitFor(() => expect(mocks.askMock).toHaveBeenCalled());
    const text = String(mocks.askMock.mock.calls[0]?.[0] ?? "");
    expect(text).toContain("限时执行");
    expect(text).not.toMatch(/\d+\s*秒/);
  });

  it("主题三态：依次点击 → onThemeChange 收到 light / dark / system", async () => {
    const onThemeChange = vi.fn();
    const user = userEvent.setup();
    render(<SettingsPage {...defaultProps} onThemeChange={onThemeChange} />);
    await user.click(screen.getByText("浅色"));
    expect(onThemeChange).toHaveBeenLastCalledWith("light");
    await user.click(screen.getByText("深色"));
    expect(onThemeChange).toHaveBeenLastCalledWith("dark");
    await user.click(screen.getByText("跟随系统"));
    expect(onThemeChange).toHaveBeenLastCalledWith("system");
    expect(onThemeChange).toHaveBeenCalledTimes(3);
  });

  it("主题激活态：当前 theme 对应按钮带 nm-inset 类（视觉反馈）", async () => {
    render(<SettingsPage {...defaultProps} theme="dark" />);
    const darkBtn = screen.getByText("深色");
    const lightBtn = screen.getByText("浅色");
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
      if (cmd === "api_status")
        return { enabled: false, port: 4763, token: "" };
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarId: null },
          bot: { name: "机器人", avatarId: null },
        };
      if (cmd === "py_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status")
        return { rules_count: 0, poll_interval_secs: 600 };
      if (cmd === "migration_log_read") return "";
      return null;
    });
    render(<SettingsPage {...defaultProps} />);
    // 导航到「机器人」分类
    await user.click(screen.getByRole("button", { name: "机器人" }));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_get_enabled");
    });
    // 「开启机器人聊天」行的描述文案作为锚点找所在行的 button
    const botDesc = screen.getByText(
      "在主窗口「聊天」页与大模型对话，用它管理任务",
    );
    const row = botDesc.closest("div")?.parentElement;
    const toggle = row?.querySelector("button");
    expect(toggle).not.toBeNull();
    if (!toggle) return;
    await user.click(toggle);
    // 调用了 bot_set_enabled
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "bot_set_enabled",
        expect.objectContaining({ enabled: true }),
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
      if (cmd === "api_status")
        return { enabled: false, port: 4763, token: "" };
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarId: null },
          bot: { name: "机器人", avatarId: null },
        };
      if (cmd === "py_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status")
        return { rules_count: 0, poll_interval_secs: 600 };
      if (cmd === "migration_log_read") return "";
      return null;
    });
    render(<SettingsPage {...defaultProps} />);
    // 导航到「MCP 服务」分类（搜索引擎设置所在）
    await user.click(screen.getByRole("button", { name: "MCP 服务" }));
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
        }),
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
      if (cmd === "api_status")
        return { enabled: false, port: 4763, token: "" };
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarId: null },
          bot: { name: "机器人", avatarId: null },
        };
      if (cmd === "py_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status")
        return { rules_count: 0, poll_interval_secs: 600 };
      if (cmd === "migration_log_read") return "";
      return null;
    });
    render(<SettingsPage {...defaultProps} />);
    // 导航到「MCP 服务」分类
    fireEvent.click(screen.getByRole("button", { name: "MCP 服务" }));
    expect(await screen.findByText(/已开启但未填 key/)).toBeInTheDocument();
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
      if (cmd === "api_status")
        return { enabled: false, port: 4763, token: "" };
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarId: null },
          bot: { name: "机器人", avatarId: null },
        };
      if (cmd === "py_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status")
        return { rules_count: 0, poll_interval_secs: 600 };
      if (cmd === "migration_log_read") return "";
      return null;
    });
    render(<SettingsPage {...defaultProps} />);
    // 导航到「MCP 服务」分类
    await user.click(screen.getByRole("button", { name: "MCP 服务" }));
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
        }),
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
    // 导航到「数据管理」分类（导出按钮所在面板）
    await user.click(screen.getByRole("button", { name: "数据管理" }));
    // eefa78f 起页面有两个「导出」（任务导出 + 工作区导出），取第一个（任务导出）；U20B 图标化后为纯文字
    await user.click(screen.getAllByText("导出")[0]);
    await waitFor(() => {
      expect(onExportTasks).toHaveBeenCalledTimes(1);
    });
  });

  // 双协议下的大模型列表（改版）
  // 老「提供商预设」3 个测试 + 1 个「API 协议」测试全部重写：新行为是每协议独立
  // 一份 ModelEntry 列表，点「添加大模型」自己加，切换协议时列表整体切换。

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
      if (cmd === "py_get_enabled") return false;
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
      if (cmd === "py_get_enabled") return false;
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
      if (cmd === "py_get_enabled") return false;
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
      if (cmd === "py_get_enabled") return false;
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
      if (cmd === "py_get_enabled") return false;
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
      if (cmd === "py_get_enabled") return false;
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
      if (cmd === "py_get_enabled") return false;
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
          maxTokens: null,
        };
      if (cmd === "bot_set_config") return null;
      if (cmd === "api_status")
        return { enabled: false, port: 4763, token: "" };
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarId: null },
          bot: { name: "机器人", avatarId: null },
        };
      if (cmd === "py_get_enabled") return false;
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
          maxTokens: number | null;
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
      // maxTokens 兜底层无 UI 入口（每模型编辑里都有）：未配置原样透传 null
      expect(arg.config.maxTokens).toBeNull();
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
      if (cmd === "api_status")
        return { enabled: false, port: 4763, token: "" };
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarId: null },
          bot: { name: "机器人", avatarId: null },
        };
      if (cmd === "py_get_enabled") return false;
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
      if (cmd === "py_get_enabled") return false;
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
      if (cmd === "py_get_enabled") return false;
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

  // 厂商详情页复刻改造（厂商头开关/⋯菜单、Key 显隐、获取 Key 外链、
  // 连接测试、能力徽标、ProviderLogo、updateModel 跨协议修复）

  /** 厂商详情页用例的公共 mock：bot 开启 + 指定 bot_get_config 视图；
   *  extra 可覆盖个别命令（如 bot_test_connection） */
  const mockVendorConfig = (
    config: Record<string, unknown>,
    extra?: (cmd: string) => unknown,
  ) => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      const r = extra?.(cmd);
      if (r !== undefined) return r;
      if (cmd === "bot_get_enabled") return true;
      if (cmd === "bot_get_config") return config;
      if (cmd === "bot_set_config") return null;
      if (cmd === "api_status")
        return { enabled: false, port: 4763, token: "" };
      if (cmd === "profile_get")
        return {
          user: { name: "我", avatarId: null },
          bot: { name: "机器人", avatarId: null },
        };
      if (cmd === "py_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status")
        return { rules_count: 0, poll_interval_secs: 600 };
      if (cmd === "migration_log_read") return "";
      return null;
    });
  };

  const deepseekVendorConfig = {
    modelsByProvider: {
      openai: [
        {
          id: "m1",
          label: "DeepSeek Chat",
          model: "deepseek-chat",
          baseUrl: "https://api.deepseek.com",
          vendor: "DeepSeek",
        },
      ],
      anthropic: [],
    },
    activeModelId: { openai: "m1", anthropic: null },
    apiProvider: "openai",
    hasApiKey: true,
    bypassLlmOnPreStepHit: true,
  };

  /** 导航到「模型设置」并打开指定厂商详情页 */
  const openVendorPage = async (
    user: ReturnType<typeof userEvent.setup>,
    vendor: string,
  ) => {
    await user.click(screen.getByRole("button", { name: "模型设置" }));
    await user.click(await screen.findByRole("button", { name: vendor }));
  };

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
      if (cmd === "py_get_enabled") return false;
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
        if (cmd === "py_get_enabled") return false;
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
        if (cmd === "py_get_enabled") return false;
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
        if (cmd === "py_get_enabled") return false;
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
