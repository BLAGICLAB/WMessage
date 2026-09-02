import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MigrationPanel } from "./MigrationPanel";
import type { MigrationRule } from "../types";

// vi.mock 工厂提升到顶部，共享 mock 变量用 vi.hoisted（同 ChatPanel.test）
const mocks = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  confirmMock: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invokeMock,
}));

const moveRule: MigrationRule = {
  id: "r1",
  enabled: true,
  keywords: ["报表", "周报"],
  action: "move",
  archiveDir: "归档/{year}",
};
const deleteRule: MigrationRule = {
  id: "r2",
  enabled: true,
  keywords: ["临时"],
  action: "delete",
  archiveDir: "",
};

const defaultInvoke = async (cmd: string) => {
  switch (cmd) {
    case "migration_rules_load":
      return { version: 1, rules: [moveRule] };
    case "migration_status":
      return { rules_count: 1, poll_interval_secs: 600 };
    case "migration_run":
      return { ts: 1700000000000, archived: 2, moved: 3, deleted: 0, skipped: 1, log: ["moved a.txt"] };
    case "migration_log_read":
      return "2026-09-01 移动 a.txt → 归档";
    case "migration_rules_import":
      return 3;
    default:
      return null;
  }
};

beforeEach(() => {
  mocks.invokeMock.mockReset();
  mocks.invokeMock.mockImplementation(defaultInvoke);
  mocks.confirmMock.mockReset();
  window.confirm = mocks.confirmMock;
});

// 第二梯队 #8（2026-09-03）：桌面清理面板此前零覆盖。以下覆盖渲染与关键交互：
// 规则表/状态行、删除规则警告、手动迁移 confirm 门、日志弹窗。
describe("MigrationPanel", () => {
  it("初始渲染：加载规则与状态，规则行展示关键字/动作/归档目录", async () => {
    render(<MigrationPanel />);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("migration_rules_load");
    });
    expect(mocks.invokeMock).toHaveBeenCalledWith("migration_status");
    expect(await screen.findByText("报表，周报")).toBeInTheDocument();
    expect(screen.getByText("移动归档")).toBeInTheDocument();
    expect(screen.getByText("归档/{year}")).toBeInTheDocument();
    // 状态行：600s → 10 分钟
    expect(
      screen.getByText(/后台每 10 分钟自动检测一次 · 现有规则 1 条/)
    ).toBeInTheDocument();
  });

  it("空规则：显示「暂无规则」引导，不显示删除警告", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status") return { rules_count: 0, poll_interval_secs: 600 };
      return null;
    });
    render(<MigrationPanel />);
    expect(
      await screen.findByText(/暂无规则：下载表格模版/)
    ).toBeInTheDocument();
    expect(screen.queryByText(/有启用的删除规则/)).not.toBeInTheDocument();
  });

  it("有启用的 delete 规则：显示 ⚠ 删除警告", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "migration_rules_load") return { version: 1, rules: [moveRule, deleteRule] };
      if (cmd === "migration_status") return { rules_count: 2, poll_interval_secs: 600 };
      return null;
    });
    render(<MigrationPanel />);
    expect(await screen.findByText(/⚠ 有启用的删除规则/)).toBeInTheDocument();
    expect(screen.getByText("删除文件")).toBeInTheDocument();
  });

  it("delete 规则停用时不显示删除警告", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "migration_rules_load")
        return { version: 1, rules: [moveRule, { ...deleteRule, enabled: false }] };
      if (cmd === "migration_status") return { rules_count: 2, poll_interval_secs: 600 };
      return null;
    });
    render(<MigrationPanel />);
    expect(await screen.findByText("报表，周报")).toBeInTheDocument();
    expect(screen.queryByText(/⚠ 有启用的删除规则/)).not.toBeInTheDocument();
  });

  it("立即执行迁移：confirm 取消不调 migration_run；确认后执行并展示报告", async () => {
    const user = userEvent.setup();
    render(<MigrationPanel />);
    const runBtn = await screen.findByText("▶ 立即执行迁移");

    // 取消 → 不触发迁移
    mocks.confirmMock.mockReturnValue(false);
    await user.click(runBtn);
    expect(mocks.invokeMock).not.toHaveBeenCalledWith("migration_run");

    // 确认 → 执行并渲染报告
    mocks.confirmMock.mockReturnValue(true);
    await user.click(runBtn);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("migration_run");
    });
    expect(await screen.findByText(/移动 3 · 删除 0 · 跳过 1/)).toBeInTheDocument();
    expect(screen.getByText("moved a.txt")).toBeInTheDocument();
  });

  it("查看迁移日志：点击后弹窗展示 migration_log_read 返回内容，可关闭", async () => {
    const user = userEvent.setup();
    render(<MigrationPanel />);
    await user.click(await screen.findByText("📋 查看迁移日志"));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("migration_log_read", { limit: 500 });
    });
    expect(
      await screen.findByText("2026-09-01 移动 a.txt → 归档")
    ).toBeInTheDocument();
    await user.click(screen.getByText("关闭"));
    expect(
      screen.queryByText("2026-09-01 移动 a.txt → 归档")
    ).not.toBeInTheDocument();
  });

  it("导入规则表：成功后显示「已导入 N 条规则」并重新加载", async () => {
    const user = userEvent.setup();
    render(<MigrationPanel />);
    await user.click(await screen.findByText("⬆ 导入规则表"));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("migration_rules_import");
    });
    expect(await screen.findByText("已导入 3 条规则")).toBeInTheDocument();
    // 导入后 refresh：migration_rules_load 被调两次（初始 + 导入后）
    await waitFor(() => {
      const loads = mocks.invokeMock.mock.calls.filter(
        (c) => c[0] === "migration_rules_load"
      );
      expect(loads).toHaveLength(2);
    });
  });
});
