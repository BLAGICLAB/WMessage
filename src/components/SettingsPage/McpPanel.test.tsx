// McpPanel 测试：列表渲染 / 添加确认流/ 删除 / 启停。

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, act, fireEvent } from "@testing-library/react";
import { McpPanel } from "./McpPanel";

const mocks = vi.hoisted(() => ({
  invokeMock: vi.fn(async (cmd: string) => {
    if (cmd === "mcp_status") return [];
    if (cmd === "bot_get_config") return {};
    if (cmd === "mcp_server_save") return [];
    return null;
  }),
  listenMock: vi.fn(async () => () => {}),
  askMock: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listenMock }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ ask: mocks.askMock }));

beforeEach(() => {
  mocks.invokeMock.mockClear();
  mocks.listenMock.mockClear();
  mocks.askMock.mockReset();
  mocks.askMock.mockResolvedValue(true);
});

async function flush() {
  await act(async () => {});
}

const SAMPLE_STATUS = [
  {
    id: "srv-1",
    name: "filesystem",
    transport: "stdio",
    enabled: true,
    state: "connected",
    error: null,
    toolCount: 3,
  },
  {
    id: "srv-2",
    name: "remote",
    transport: "http",
    enabled: false,
    state: "absent",
    error: null,
    toolCount: 0,
  },
];

const BUILTIN_STATUS = [
  SAMPLE_STATUS[0],
  {
    id: "builtin-playwright",
    name: "Playwright 浏览器",
    transport: "stdio",
    enabled: false,
    state: "absent",
    error: null,
    toolCount: 0,
    builtin: true,
  },
];

/** stdio 连接且子进程在跑：带资源字段 */
const STATUS_WITH_RES = [
  {
    id: "srv-1",
    name: "filesystem",
    transport: "stdio",
    enabled: true,
    state: "connected",
    error: null,
    toolCount: 3,
    pid: 4321,
    rssMb: 128.4,
    cpuPct: 12.6,
    cpuSeconds: 1.5,
    uptimeSecs: 125,
  },
];

describe("McpPanel", () => {
  it("空列表：显示占位文案 + 添加按钮", async () => {
    render(<McpPanel />);
    await flush();
    expect(screen.getByText(/暂无服务器/)).toBeInTheDocument();
    expect(screen.getByText("＋ 添加服务器")).toBeInTheDocument();
  });

  it("列表渲染：状态点 tooltip / 名称 / 工具数 / 禁用态", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "mcp_status" ? SAMPLE_STATUS : null,
    );
    render(<McpPanel />);
    await flush();
    expect(screen.getByText("filesystem")).toBeInTheDocument();
    expect(screen.getByText(/3 个工具/)).toBeInTheDocument();
    expect(screen.getByText(/已禁用/)).toBeInTheDocument();
    const dot = screen.getByTitle("已连接（本次调用中）");
    expect(dot.className).toContain("bg-emerald-500");
  });

  it("添加流：填表单 → 确认弹窗展示完整命令 → 确认后按 payload 调 mcp_server_save", async () => {
    render(<McpPanel />);
    await flush();
    fireEvent.click(screen.getByText("＋ 添加服务器"));
    fireEvent.change(screen.getByPlaceholderText("如 filesystem"), {
      target: { value: "fs" },
    });
    fireEvent.change(screen.getByPlaceholderText(/一行一个参数/), {
      target: { value: "-y\n@modelcontextprotocol/server-filesystem" },
    });
    fireEvent.change(screen.getByPlaceholderText(/KEY=VALUE/), {
      target: { value: "HOME=/tmp/x" },
    });
    fireEvent.click(screen.getByText("下一步：确认并保存"));
    await flush();
    // 确认弹窗：完整命令行（命令 + 参数 + env 值）必须全部可见
    expect(
      screen.getByText(/npx -y @modelcontextprotocol\/server-filesystem/),
    ).toBeInTheDocument();
    // env 在表单 textarea 和确认弹窗各出现一次（≥2 = 弹窗也展示了）
    expect(screen.getAllByText(/HOME=\/tmp\/x/).length).toBeGreaterThanOrEqual(
      2,
    );
    fireEvent.click(screen.getByText("确认，保存并连接"));
    await flush();
    expect(mocks.invokeMock).toHaveBeenCalledWith("mcp_server_save", {
      server: expect.objectContaining({
        id: "",
        name: "fs",
        transport: "stdio",
        command: "npx",
        args: ["-y", "@modelcontextprotocol/server-filesystem"],
        env: { HOME: "/tmp/x" },
        enabled: true,
      }),
    });
  });

  it("空名称校验：不弹确认框、报错提示", async () => {
    render(<McpPanel />);
    await flush();
    fireEvent.click(screen.getByText("＋ 添加服务器"));
    fireEvent.click(screen.getByText("下一步：确认并保存"));
    await flush();
    expect(screen.getByText("名称不能为空")).toBeInTheDocument();
    expect(
      mocks.invokeMock.mock.calls.some(([c]) => c === "mcp_server_save"),
    ).toBe(false);
  });

  it("确认弹窗：含空白的参数加引号展示（B0 评审：展示 token 与实际 argv 对齐）", async () => {
    render(<McpPanel />);
    await flush();
    fireEvent.click(screen.getByText("＋ 添加服务器"));
    fireEvent.change(screen.getByPlaceholderText("如 filesystem"), {
      target: { value: "fs" },
    });
    fireEvent.change(screen.getByPlaceholderText(/一行一个参数/), {
      target: { value: "hello world" },
    });
    fireEvent.click(screen.getByText("下一步：确认并保存"));
    await flush();
    expect(screen.getByText(/npx "hello world"/)).toBeInTheDocument();
  });

  it("超时非法值（小数/越界）：不弹确认框、报 5–600 整数提示（B0 回归）", async () => {
    render(<McpPanel />);
    await flush();
    fireEvent.click(screen.getByText("＋ 添加服务器"));
    fireEvent.change(screen.getByPlaceholderText("如 filesystem"), {
      target: { value: "fs" },
    });
    fireEvent.change(screen.getByPlaceholderText(/默认 60/), {
      target: { value: "3.5" },
    });
    fireEvent.click(screen.getByText("下一步：确认并保存"));
    await flush();
    expect(screen.getByText("超时须为 5–600 的整数秒")).toBeInTheDocument();
    expect(
      mocks.invokeMock.mock.calls.some(([c]) => c === "mcp_server_save"),
    ).toBe(false);
  });

  it("删除：confirm 后按 id 调 mcp_server_delete", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "mcp_status" ? SAMPLE_STATUS : [],
    );
    render(<McpPanel />);
    await flush();
    fireEvent.click(screen.getAllByTitle("删除")[0]);
    await flush();
    expect(mocks.askMock).toHaveBeenCalled();
    expect(mocks.invokeMock).toHaveBeenCalledWith("mcp_server_delete", {
      id: "srv-1",
    });
  });

  it("启停：拨杆开关按 id 调 mcp_server_toggle", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "mcp_status" ? SAMPLE_STATUS : [],
    );
    render(<McpPanel />);
    await flush();
    const switches = screen.getAllByRole("switch") as HTMLElement[];
    // 第一个 = filesystem（enabled=true），点击即取消启用
    expect(switches[0].getAttribute("aria-checked")).toBe("true");
    fireEvent.click(switches[0]);
    await flush();
    expect(mocks.invokeMock).toHaveBeenCalledWith("mcp_server_toggle", {
      id: "srv-1",
      enabled: false,
    });
  });

  it("启停开关：拨杆形态，解释文字走悬停 tooltip（按钮后不挂文字）", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "mcp_status" ? SAMPLE_STATUS : [],
    );
    render(<McpPanel />);
    await flush();
    const sw = screen.getAllByRole("switch")[0];
    // 开关本体是 role=switch 的按钮（不再是 checkbox）
    expect(sw.tagName).toBe("BUTTON");
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    // 解释在 title 里（悬停可见），且不是行内可见文本
    const tip = sw.getAttribute("title") ?? "";
    expect(tip).toMatch(/静默待命|不进机器人清单/);
    expect(sw.textContent).toBe("");
  });

  it("工具展开：点「工具」拉取 mcp_server_tools 并显示清单", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "mcp_status") return SAMPLE_STATUS;
      if (cmd === "mcp_server_tools")
        return [{ name: "read_file", description: "读文件" }];
      return null;
    });
    render(<McpPanel />);
    await flush();
    fireEvent.click(screen.getAllByText("工具")[0]);
    await flush();
    expect(screen.getByText("read_file")).toBeInTheDocument();
    expect(screen.getByText(/读文件/)).toBeInTheDocument();
  });

  it("内置条目：标「内置」、不给删除按钮，开关照常可点", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "mcp_status" ? BUILTIN_STATUS : [],
    );
    render(<McpPanel />);
    await flush();
    expect(screen.getByText("Playwright 浏览器")).toBeInTheDocument();
    expect(screen.getByText("内置")).toBeInTheDocument();
    // 两台服务器，但只有自建那台有删除按钮
    expect(screen.getAllByTitle("删除")).toHaveLength(1);
    // 内置条目默认停用，开关可打开（第二张卡 = 内置那台）
    const switches = screen.getAllByRole("switch") as HTMLElement[];
    expect(switches[1].getAttribute("aria-checked")).toBe("false");
    fireEvent.click(switches[1]);
    await flush();
    expect(mocks.invokeMock).toHaveBeenCalledWith("mcp_server_toggle", {
      id: "builtin-playwright",
      enabled: true,
    });
  });

  it("资源行：有子进程时显示 pid / 内存 / CPU / 运行时长", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "mcp_status" ? STATUS_WITH_RES : [],
    );
    render(<McpPanel />);
    await flush();
    expect(screen.getByText("pid 4321")).toBeInTheDocument();
    expect(screen.getByText("内存 128 MB")).toBeInTheDocument();
    expect(screen.getByText("CPU 13%")).toBeInTheDocument();
    expect(screen.getByText("已运行 2 分 5 秒")).toBeInTheDocument();
  });

  it("说明行：标清 MCP 子进程不在本地服务表里", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "mcp_status" ? STATUS_WITH_RES : [],
    );
    render(<McpPanel />);
    await flush();
    expect(
      screen.getByText("此进程由 MCP 客户端按需拉起，不在本地服务表中"),
    ).toBeInTheDocument();
  });

  it("本平台采不到：明说「不支持采集」，不把空值显示成没占用", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "mcp_status"
        ? [
            {
              ...STATUS_WITH_RES[0],
              rssMb: null,
              cpuPct: null,
              cpuSeconds: null,
              listeningSupported: false,
              resourceSupported: false,
            },
          ]
        : [],
    );
    render(<McpPanel />);
    await flush();
    expect(
      screen.getByText(/本平台暂不支持进程资源采集/),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/本平台暂不支持监听端口采集/),
    ).toBeInTheDocument();
  });

  it("本平台能采集：不出「不支持采集」提示", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "mcp_status"
        ? [
            {
              ...STATUS_WITH_RES[0],
              listeningSupported: true,
              resourceSupported: true,
            },
          ]
        : [],
    );
    render(<McpPanel />);
    await flush();
    expect(screen.queryByText(/本平台暂不支持/)).not.toBeInTheDocument();
  });

  it("首采无占用率：退化成累计 CPU 时间", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "mcp_status"
        ? [{ ...STATUS_WITH_RES[0], cpuPct: null, cpuSeconds: 1.5 }]
        : [],
    );
    render(<McpPanel />);
    await flush();
    expect(screen.getByText("累计 CPU 1.5s")).toBeInTheDocument();
    expect(screen.queryByText(/^CPU /)).not.toBeInTheDocument();
  });

  it("无子进程（HTTP 连接）：不显示资源行与说明行", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "mcp_status" ? SAMPLE_STATUS : [],
    );
    render(<McpPanel />);
    await flush();
    expect(screen.queryByText(/^pid /)).not.toBeInTheDocument();
    expect(
      screen.queryByText("此进程由 MCP 客户端按需拉起，不在本地服务表中"),
    ).not.toBeInTheDocument();
  });

  it("监听端口：显示子进程在监听的 socket", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "mcp_status"
        ? [
            {
              ...STATUS_WITH_RES[0],
              listening: [
                { protocol: "TCP", address: "127.0.0.1", port: 5173 },
                { protocol: "TCP", address: "0.0.0.0", port: 8080 },
              ],
            },
          ]
        : [],
    );
    render(<McpPanel />);
    await flush();
    expect(screen.getByText(/监听 TCP 127\.0\.0\.1:5173/)).toBeInTheDocument();
    // 非 loopback 的绑定也要照实显示（判 L1 是 P2-③ 的事，这里只管显示）
    expect(screen.getByText(/TCP 0\.0\.0\.0:8080/)).toBeInTheDocument();
  });

  it("编辑表单：机密提示跨两列（不占标签列），参数占位示例不超框", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "mcp_status") return SAMPLE_STATUS;
      if (cmd === "mcp_server_secrets_masked")
        return { env: { TOKEN: "••••••" }, headers: {}, mask: "••••••" };
      if (cmd === "bot_get_config")
        return {
          mcpServers: [
            {
              id: "srv-1",
              name: "filesystem",
              transport: "stdio",
              command: "npx",
              args: ["-y", "pkg"],
              env: {},
              headers: {},
              timeoutSecs: 60,
              enabled: true,
            },
          ],
        };
      return null;
    });
    const { container } = render(<McpPanel />);
    await flush();
    fireEvent.click(screen.getAllByLabelText("编辑")[0]);
    await flush();

    // 机密提示是网格子项，必须跨两列：否则它落进标签列（auto 宽度被这句长文撑开），
    // 还会把「超时（秒）」的标签挤到同一行、输入框掉到下一行第一列（实机截图：附录 D-12）
    const hint = screen.getByText(/已设置的机密以/).closest("p");
    expect(hint).not.toBeNull();
    expect(hint?.className).toContain("col-span-2");
    const grid = container.querySelector(".grid-cols-\\[auto_1fr\\]");
    expect(grid).not.toBeNull();
    expect(hint?.parentElement).toBe(grid);

    // 参数框的占位示例不能超过框高（rows=3）：第四行会被裁掉，用户读不全
    const args = screen.getByPlaceholderText(/一行一个参数/) as HTMLTextAreaElement;
    expect(args.rows).toBe(3);
    expect(args.placeholder.split("\n").length).toBeLessThanOrEqual(3);
  });

  it("无监听：不显示监听行", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "mcp_status"
        ? [{ ...STATUS_WITH_RES[0], listening: [] }]
        : [],
    );
    render(<McpPanel />);
    await flush();
    expect(screen.queryByText(/监听/)).not.toBeInTheDocument();
  });
});

describe("McpPanel 轮询（资源行是活的量）", () => {
  // 假定时器必须在 render 之前装：轮询定时器是挂载时创建的，
  // 之后再装 fake timers 管不到已经建好的那个
  beforeEach(() => {
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "mcp_status" ? STATUS_WITH_RES : [],
    );
  });

  it("可见时每 5s 拉一次：已运行时长会走（不再停在打开那一刻的 0 秒）", async () => {
    vi.useFakeTimers();
    try {
      let uptime = 0;
      mocks.invokeMock.mockImplementation(async (cmd: string) =>
        cmd === "mcp_status"
          ? [{ ...STATUS_WITH_RES[0], uptimeSecs: uptime }]
          : [],
      );
      render(<McpPanel />);
      await flush();
      expect(screen.getByText("已运行 0 秒")).toBeInTheDocument();

      uptime = 65;
      await act(async () => {
        await vi.advanceTimersByTimeAsync(5000);
      });
      await flush();
      expect(screen.getByText("已运行 1 分 5 秒")).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("不可见（active=false）：不轮询、不发请求（面板惰性挂载不卸载，别在后台空转）", async () => {
    vi.useFakeTimers();
    try {
      render(<McpPanel active={false} />);
      await flush();
      const before = mocks.invokeMock.mock.calls.length;
      await act(async () => {
        await vi.advanceTimersByTimeAsync(15000);
      });
      expect(mocks.invokeMock.mock.calls.length).toBe(before);
    } finally {
      vi.useRealTimers();
    }
  });

  it("切回可见：立刻补一次刷新（面板不卸载，数值不能停在离开那刻）", async () => {
    vi.useFakeTimers();
    try {
      const { rerender } = render(<McpPanel active={false} />);
      await flush();
      const before = mocks.invokeMock.mock.calls.length;
      rerender(<McpPanel active />);
      await flush();
      expect(mocks.invokeMock.mock.calls.length).toBeGreaterThan(before);
    } finally {
      vi.useRealTimers();
    }
  });

  it("卸载后不留计时器", async () => {
    vi.useFakeTimers();
    try {
      const { unmount } = render(<McpPanel />);
      await flush();
      unmount();
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });
});
