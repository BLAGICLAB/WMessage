import { expect } from "vitest";
import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

/** 各测试文件共用的 mock 装置与夹具。
 *
 *  为什么单独成文件：SettingsPage 的用例散在 6 个测试文件里，每份都自带
 *  一份 300 行的 invoke 分发器既重复又没有共同真相——参数表一改要改六处。
 *  这里只放**跨文件共用**的东西：mock 实例、invoke 的默认分发、参数表假数据、
 *  厂商配置夹具。vi.mock 的工厂仍留在各测试文件（mock 按文件作用域生效，
 *  搬走就失效）。
 *
 *  导入顺序有讲究：使用方必须先 import 本模块，再 import 被测组件——
 *  vi.mock 的工厂在被测组件被求值时才跑，那时 mock 实例必须已经就位。
 */

// vi.mock 工厂会被提升到顶部，因此共享 mock 变量必须用 vi.hoisted 包裹
const mocks = vi.hoisted(() => {
  const invokeMock = vi.fn();
  const emitMock = vi.fn();
  const openMock = vi.fn();
  const openUrlMock = vi.fn();
  const askMock = vi.fn();
  return { invokeMock, emitMock, openMock, openUrlMock, askMock };
});
export { mocks };

/** invoke 的默认分发：按命令名返回各配置（未识别的命令返 null）。 */
const defaultInvoke = async (cmd: string) => {
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
    case "service_get_enabled":
    case "service_get_expose_net_allowed":
      return false;
    case "services_list":
      return { services: [], totalRssMb: 0, warnings: [], sampledAt: null };
    case "services_stop":
      return { serviceId: "", stopped: true };
    // Agent 运行参数卡的元数据（默认值/可填范围/当前生效值）
    case "bot_effective_params":
      return paramViews;
    case "env_check":
      return {
        failed: false,
        python: {
          available: false,
          python: "",
          version: "",
          packages: [],
          install: null,
        },
        node: {
          available: false,
          node: "",
          version: "",
          npm: "",
          npmVersion: "",
          packages: [],
          install: null,
        },
        launchers: [],
      };
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
};

mocks.invokeMock.mockImplementation(defaultInvoke);
mocks.emitMock.mockImplementation(async () => {});
mocks.openMock.mockImplementation(async () => null);
mocks.openUrlMock.mockImplementation(async () => {});

/** logo src 断言：vite 把小 SVG 内联为 data URI（解码后含 slug），大资产为 lobe/ 路径 */
export const expectLogoSlug = (img: Element | null, slug: string) => {
  const src = img?.getAttribute("src") ?? "";
  expect(src).not.toBe("");
  const hay = src.startsWith("data:") ? decodeURIComponent(src) : src;
  expect(hay.toLowerCase()).toContain(slug);
};

/** bot_effective_params 的返回体（后端 bot::params::ParamView 的线格式）。
 *  前端不再自留一份默认数字——这些值就是页面显示「默认/可填范围/当前生效」
 *  的唯一来源。 */
export const paramViews = [
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
    label: "主会话工具调用安全网",
    category: "loop",
    hint: "主会话的工具调用熔断上限；默认取共享安全网常量",
    value: "400",
    default: "400",
    source: "default",
    editable: true,
    min: 1,
    max: 1200,
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
    // Python 超时：value/default 是 mock 自造值（真值在后端 py::runtime 常量），
    // 测试断言的就是 mock 给的数——页面若手写真值就露馅。
    // 上界故意用 1000（真值是后端 MAX_TIMEOUT_SECS）：预设「拉到上限」若写死就露馅（同上）
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

/** 厂商详情页用例的公共 mock：bot 开启 + 指定 bot_get_config 视图；
 *  extra 可覆盖个别命令（如 bot_test_connection） */
export const mockVendorConfig = (
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
    if (cmd === "py_get_enabled" || cmd === "service_get_enabled") return false;
    if (cmd === "skills_list") return [];
    if (cmd === "migration_rules_load") return { version: 1, rules: [] };
    if (cmd === "migration_status")
      return { rules_count: 0, poll_interval_secs: 600 };
    if (cmd === "migration_log_read") return "";
    return null;
  });
};

export const deepseekVendorConfig = {
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
export const openVendorPage = async (
  user: ReturnType<typeof userEvent.setup>,
  vendor: string,
) => {
  await user.click(screen.getByRole("button", { name: "模型设置" }));
  await user.click(await screen.findByRole("button", { name: vendor }));
};
