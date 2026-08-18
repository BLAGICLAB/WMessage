import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { WorkspacePage } from "./WorkspacePage";

// vi.mock 工厂会被提升到顶部，因此共享 mock 变量必须用 vi.hoisted 包裹
const mocks = vi.hoisted(() => {
  const invokeMock = vi.fn();
  const listenMock = vi.fn();
  const emitMock = vi.fn();
  const openMock = vi.fn();
  return { invokeMock, listenMock, emitMock, openMock };
});

// —— Tauri mocks ——
// 默认：workspace_load 返空列表
mocks.invokeMock.mockImplementation(async (cmd: string) => {
  if (cmd === "workspace_load") return [];
  if (cmd === "workspace_upsert") return null;
  if (cmd === "workspace_delete") return null;
  return null;
});
mocks.listenMock.mockImplementation(async () => () => {});
mocks.emitMock.mockImplementation(async () => {});
mocks.openMock.mockImplementation(async () => null);

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invokeMock,
  convertFileSrc: vi.fn((p: string) => `tauri://localhost/${p}`),
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: mocks.emitMock,
  listen: mocks.listenMock,
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: mocks.openMock,
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openPath: vi.fn(async () => {}),
  openUrl: vi.fn(async () => {}),
}));

const confirmMock = vi.fn(() => true);
beforeEach(() => {
  mocks.invokeMock.mockClear();
  mocks.listenMock.mockClear();
  mocks.emitMock.mockClear();
  mocks.openMock.mockClear();
  confirmMock.mockClear();
  window.confirm = confirmMock;
});

describe("WorkspacePage", () => {
  it("空列表渲染：显示「+ 新建工作区」+ 暂无工作区提示", async () => {
    render(<WorkspacePage />);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("workspace_load");
    });
    expect(screen.getByText("+ 新建工作区")).toBeInTheDocument();
    expect(
      screen.getByText(/暂无工作区：新建一个/)
    ).toBeInTheDocument();
  });

  it("点击 + 新建工作区：调用 workspace_upsert，新工作区标题「新工作区」进入编辑态", async () => {
    const user = userEvent.setup();
    render(<WorkspacePage />);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("workspace_load");
    });
    const callsBefore = mocks.invokeMock.mock.calls.filter(
      (c) => c[0] === "workspace_upsert"
    ).length;
    await user.click(screen.getByText("+ 新建工作区"));
    // 标题行进入编辑态（input value = "新工作区"）
    const titleInput = await screen.findByDisplayValue("新工作区");
    expect(titleInput).toBeInTheDocument();
    // workspace_upsert 至少被调用一次
    await waitFor(() => {
      const calls = mocks.invokeMock.mock.calls.filter(
        (c) => c[0] === "workspace_upsert"
      );
      expect(calls.length).toBeGreaterThan(callsBefore);
    });
  });

  it("折叠切换：点击 FoldToggle → workspace_upsert 携带 collapsed: true", async () => {
    const user = userEvent.setup();
    // 模拟有一项已展开的工作区
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "workspace_load")
        return [
          {
            id: "w1",
            title: "测试工作区",
            collapsed: false,
            links: [],
            order: 0,
            updatedAt: Date.now(),
          },
        ];
      if (cmd === "workspace_upsert") return null;
      return null;
    });
    render(<WorkspacePage />);
    await waitFor(() => {
      expect(screen.getByText("测试工作区")).toBeInTheDocument();
    });
    // 触发折叠
    await user.click(screen.getByTitle("收起"));
    // workspace_upsert 收到 items[0].collapsed = true
    await waitFor(() => {
      const upserts = mocks.invokeMock.mock.calls.filter(
        (c) => c[0] === "workspace_upsert"
      );
      // 最后一次 upsert 的 items 中应包含 collapsed: true
      const lastArgs = upserts[upserts.length - 1][1] as
        | { items: { collapsed: boolean }[] }
        | undefined;
      expect(lastArgs?.items?.[0]?.collapsed).toBe(true);
    });
  });

  it("删除工作区：点 🗑️ → confirm 接受 → workspace_delete 调用 + items 从 UI 移除", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "workspace_load")
        return [
          {
            id: "w1",
            title: "待删工作区",
            collapsed: true,
            links: [],
            order: 0,
            updatedAt: Date.now(),
          },
        ];
      if (cmd === "workspace_upsert") return null;
      if (cmd === "workspace_delete") return null;
      return null;
    });
    // confirm 返回 true（接受删除）
    confirmMock.mockReturnValue(true);
    render(<WorkspacePage />);
    await waitFor(() => {
      expect(screen.getByText("待删工作区")).toBeInTheDocument();
    });
    // 点击删除按钮（🗑️ title="删除工作区"）
    await user.click(screen.getByTitle("删除工作区"));
    // confirm 被调用
    expect(confirmMock).toHaveBeenCalled();
    // workspace_delete 收到 ["w1"]
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "workspace_delete",
        expect.objectContaining({ ids: ["w1"] })
      );
    });
    // 工作区从 UI 移除
    await waitFor(() => {
      expect(screen.queryByText("待删工作区")).not.toBeInTheDocument();
    });
  });

  it("删除工作区：confirm 取消 → 不调用 workspace_delete", async () => {
    const user = userEvent.setup();
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "workspace_load")
        return [
          {
            id: "w1",
            title: "保留工作区",
            collapsed: true,
            links: [],
            order: 0,
            updatedAt: Date.now(),
          },
        ];
      if (cmd === "workspace_upsert") return null;
      if (cmd === "workspace_delete") return null;
      return null;
    });
    confirmMock.mockReturnValue(false); // 取消
    render(<WorkspacePage />);
    await waitFor(() => {
      expect(screen.getByText("保留工作区")).toBeInTheDocument();
    });
    await user.click(screen.getByTitle("删除工作区"));
    // confirm 被调用
    expect(confirmMock).toHaveBeenCalled();
    // workspace_delete 不应被调用
    expect(mocks.invokeMock).not.toHaveBeenCalledWith(
      "workspace_delete",
      expect.anything()
    );
    // 工作区还在
    expect(screen.getByText("保留工作区")).toBeInTheDocument();
  });
});
