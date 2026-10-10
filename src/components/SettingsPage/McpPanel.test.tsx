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
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listenMock }));

beforeEach(() => {
  mocks.invokeMock.mockClear();
  mocks.listenMock.mockClear();
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
    const dot = screen.getByTitle("已连接");
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
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    render(<McpPanel />);
    await flush();
    fireEvent.click(screen.getAllByTitle("删除")[0]);
    await flush();
    expect(confirmSpy).toHaveBeenCalled();
    expect(mocks.invokeMock).toHaveBeenCalledWith("mcp_server_delete", {
      id: "srv-1",
    });
    confirmSpy.mockRestore();
  });

  it("启停：点复选框按 id 调 mcp_server_toggle", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "mcp_status" ? SAMPLE_STATUS : [],
    );
    render(<McpPanel />);
    await flush();
    const boxes = screen.getAllByRole("checkbox") as HTMLInputElement[];
    // 第一个 = filesystem（enabled=true），点击取消勾选
    expect(boxes[0].checked).toBe(true);
    fireEvent.click(boxes[0]);
    await flush();
    expect(mocks.invokeMock).toHaveBeenCalledWith("mcp_server_toggle", {
      id: "srv-1",
      enabled: false,
    });
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
});
