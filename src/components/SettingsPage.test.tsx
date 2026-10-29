/// <reference types="node" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  render,
  screen,
  waitFor,
  fireEvent,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
// 共享 mock 装置必须先于被测组件 import：vi.mock 的工厂在被测组件被求值
// 时才跑，那时 mock 实例必须已经就位（import 顺序反了会直接报错）。
import {
  mocks,
  paramViews,
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
    case "service_get_enabled":
    case "service_get_expose_net_allowed":
      return false;
    case "services_list":
      return { services: [], totalRssMb: 0, warnings: [], sampledAt: null };
    case "services_stop":
      return { serviceId: "", stopped: true };
    case "env_check":
      return {
        failed: false,
        python: { available: false, python: "", version: "", packages: [] },
        node: {
          available: false,
          node: "",
          version: "",
          npm: "",
          npmVersion: "",
          packages: [],
        },
        launchers: [],
      };
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
    fireEvent.click(screen.getByRole("button", { name: "本地服务" }));
    expect(screen.getByText("受管服务")).toBeInTheDocument();
    expect(screen.getByText("暂无受管服务")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "任务图谱" }));
    expect(
      screen.getByRole("switch", { name: "只看我的任务" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("radiogroup", { name: "星点大小" }),
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
    // 上限放宽到 30 秒：这条要把整个设置页挂起来再逐个分类点一遍，是全仓
    // 最重的 UI 用例；机器负载高时 5 秒默认上限会把它饿死（报超时，不是断言不达标）
  }, 30_000);

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
      if (cmd === "py_get_enabled" || cmd === "service_get_enabled") return false;
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
      if (cmd === "py_get_enabled" || cmd === "service_get_enabled") return false;
      if (cmd === "env_check")
        return {
          failed: false,
          python: { available: false, python: "", version: "", packages: [] },
          node: {
            available: false,
            node: "",
            version: "",
            npm: "",
            npmVersion: "",
            packages: [],
          },
          launchers: [],
        };
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

  it("Python 默认超时：默认秒数与上限取自后端参数表", async () => {
    // 前端此前手写「留空 = 60 秒 / 上限 N 秒」——后端常量一改页面就撒谎
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

  it("一键预设「放开」：轮数、工具调用与 Python 超时全部取参数表上界", async () => {
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
            // 上界来自参数表（mock：轮数/超时上界都是 1000、调用数上界 1200，非
            // 真值——写死字面量时钳制不会把它拉到上界，断言就能分辨）
            maxRounds: 1000,
            maxFunctionCalls: 1200,
            pythonTimeoutSecs: 1000,
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
      if (cmd === "py_get_enabled" || cmd === "service_get_enabled") return false;
      if (cmd === "env_check")
        return {
          failed: false,
          python: { available: false, python: "", version: "", packages: [] },
          node: {
            available: false,
            node: "",
            version: "",
            npm: "",
            npmVersion: "",
            packages: [],
          },
          launchers: [],
        };
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

  it("受管服务开关：默认关闭；开启要二次确认并写明凭证继承", async () => {
    const user = userEvent.setup();
    const setMock = vi.fn(async (_payload?: unknown) => true);
    mocks.invokeMock.mockImplementation(async (cmd: string, payload?: unknown) => {
      if (cmd === "service_set_enabled") return setMock(payload);
      return agentParamsMock(cmd);
    });
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "机器人" }));
    const title = await screen.findByText("允许机器人启动本地服务");
    const row = title.parentElement?.parentElement as HTMLElement;
    // 默认关闭
    expect(within(row).getByRole("button").textContent).toContain("已关闭");
    await user.click(within(row).getByRole("button"));
    // 二次确认走 Tauri 原生 dialog
    await waitFor(() => expect(mocks.askMock).toHaveBeenCalled());
    const text = String(mocks.askMock.mock.calls[0]?.[0] ?? "");
    // 凭证继承这条必须写在确认框里——用户是在授权「代码能读我机器上的密钥」
    expect(text).toContain("环境变量");
    expect(text).toContain("密钥");
    expect(setMock).toHaveBeenCalledWith({ enabled: true });
  });

  it("非 loopback 暴露开关：默认关闭；开启要二次确认并写明局域网可连", async () => {
    const user = userEvent.setup();
    const setMock = vi.fn(async (_payload?: unknown) => true);
    mocks.invokeMock.mockImplementation(async (cmd: string, payload?: unknown) => {
      if (cmd === "service_set_expose_net_allowed") return setMock(payload);
      return agentParamsMock(cmd);
    });
    render(<SettingsPage {...defaultProps} />);
    await user.click(screen.getByRole("button", { name: "机器人" }));
    const title = await screen.findByText("允许服务暴露到非 loopback");
    const row = title.parentElement?.parentElement as HTMLElement;
    // 默认关闭
    expect(within(row).getByRole("button").textContent).toContain("已关闭");
    await user.click(within(row).getByRole("button"));
    // 二次确认走 Tauri 原生 dialog
    await waitFor(() => expect(mocks.askMock).toHaveBeenCalled());
    const text = String(mocks.askMock.mock.calls[0]?.[0] ?? "");
    // 确认框要写明后果：局域网能连上它
    expect(text).toContain("局域网");
    expect(setMock).toHaveBeenCalledWith({ allowed: true });
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
      if (cmd === "py_get_enabled" || cmd === "service_get_enabled") return false;
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
      if (cmd === "py_get_enabled" || cmd === "service_get_enabled") return false;
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
      if (cmd === "py_get_enabled" || cmd === "service_get_enabled") return false;
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
      if (cmd === "py_get_enabled" || cmd === "service_get_enabled") return false;
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
});
