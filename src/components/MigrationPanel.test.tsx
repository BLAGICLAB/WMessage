import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MigrationPanel } from "./MigrationPanel";
import type { MigrationRule, ScanSource } from "../types";

// vi.mock 工厂提升到顶部，共享 mock 变量用 vi.hoisted（同 ChatPanel.test）
const mocks = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  askMock: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invokeMock,
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  ask: mocks.askMock,
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

const desktopSource: ScanSource = {
  kind: "desktop",
  path: "",
  archiveRoot: "",
  enabled: true,
};
const customSource: ScanSource = {
  kind: "custom",
  path: "/Users/t/Downloads",
  archiveRoot: "",
  enabled: true,
};

const defaultInvoke = async (cmd: string) => {
  switch (cmd) {
    case "migration_rules_load":
      return { version: 1, rules: [moveRule] };
    case "migration_status":
      return { rules_count: 1, poll_interval_secs: 600 };
    case "migration_sources_load":
      return { sources: [desktopSource] };
    case "migration_run":
      return {
        ts: 1700000000000,
        archived: 2,
        moved: 3,
        deleted: 0,
        skipped: 1,
        log: ["moved a.txt"],
      };
    case "migration_scan":
      return {
        ts: 1700000000001,
        archived: 0,
        moved: 2,
        deleted: 0,
        skipped: 1,
        log: ["[扫描]已移动 b.txt"],
      };
    case "migration_log_read":
      return "2026-09-01 移动 a.txt → 归档";
    case "migration_rules_import":
      return 3;
    case "migration_sources_add_folder":
      return { sources: [desktopSource, customSource] };
    default:
      return null;
  }
};

beforeEach(() => {
  mocks.invokeMock.mockReset();
  mocks.invokeMock.mockImplementation(defaultInvoke);
  mocks.askMock.mockReset();
  mocks.askMock.mockResolvedValue(true);
});

// 文件整理面板覆盖渲染与关键交互：
// 来源列表/规则表/状态行、删除规则警告、绑定整理与扫描整理双入口 confirm 门、
// 来源启停与添加文件夹、共享整理记录弹窗。
describe("MigrationPanel", () => {
  it("初始渲染：加载规则、状态与来源，规则行展示关键字/动作/归档目录", async () => {
    render(<MigrationPanel />);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("migration_rules_load");
    });
    expect(mocks.invokeMock).toHaveBeenCalledWith("migration_status");
    expect(mocks.invokeMock).toHaveBeenCalledWith("migration_sources_load");
    expect(await screen.findByText("报表，周报")).toBeInTheDocument();
    expect(screen.getByText("移动归档")).toBeInTheDocument();
    expect(screen.getByText("归档/{year}")).toBeInTheDocument();
    expect(screen.getByText("桌面（预设）")).toBeInTheDocument();
    // 状态行：600s → 10 分钟；扫描整理仅手动
    expect(
      screen.getByText(
        /绑定整理后台每 10 分钟随归档自动执行 · 扫描整理仅手动 · 现有规则 1 条/,
      ),
    ).toBeInTheDocument();
  });

  it("空规则：显示「暂无规则」引导，不显示删除警告", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      if (cmd === "migration_status")
        return { rules_count: 0, poll_interval_secs: 600 };
      if (cmd === "migration_sources_load")
        return { sources: [desktopSource] };
      return null;
    });
    render(<MigrationPanel />);
    expect(
      await screen.findByText(/暂无规则：下载表格模版/),
    ).toBeInTheDocument();
    expect(screen.queryByText(/有启用的删除规则/)).not.toBeInTheDocument();
  });

  it("有启用的 delete 规则：显示 ⚠ 删除警告", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "migration_rules_load")
        return { version: 1, rules: [moveRule, deleteRule] };
      if (cmd === "migration_status")
        return { rules_count: 2, poll_interval_secs: 600 };
      if (cmd === "migration_sources_load")
        return { sources: [desktopSource] };
      return null;
    });
    render(<MigrationPanel />);
    expect(await screen.findByText(/有启用的删除规则/)).toBeInTheDocument();
    expect(screen.getByText("删除文件")).toBeInTheDocument();
  });

  it("delete 规则停用时不显示删除警告", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "migration_rules_load")
        return {
          version: 1,
          rules: [moveRule, { ...deleteRule, enabled: false }],
        };
      if (cmd === "migration_status")
        return { rules_count: 2, poll_interval_secs: 600 };
      if (cmd === "migration_sources_load")
        return { sources: [desktopSource] };
      return null;
    });
    render(<MigrationPanel />);
    expect(await screen.findByText("报表，周报")).toBeInTheDocument();
    expect(screen.queryByText(/有启用的删除规则/)).not.toBeInTheDocument();
  });

  it("立即执行绑定整理：confirm 取消不调 migration_run；确认后执行并展示报告", async () => {
    const user = userEvent.setup();
    render(<MigrationPanel />);
    const runBtn = await screen.findByText("立即执行绑定整理");

    // 取消 → 不触发迁移
    mocks.askMock.mockResolvedValue(false);
    await user.click(runBtn);
    expect(mocks.invokeMock).not.toHaveBeenCalledWith("migration_run");

    // 确认 → 执行并渲染报告
    mocks.askMock.mockResolvedValue(true);
    await user.click(runBtn);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("migration_run");
    });
    expect(
      await screen.findByText(/移动 3 · 删除 0 · 跳过 1/),
    ).toBeInTheDocument();
    expect(screen.getByText("moved a.txt")).toBeInTheDocument();
  });

  it("扫描整理：confirm 取消不调 migration_scan；确认后执行并展示扫描报告", async () => {
    const user = userEvent.setup();
    render(<MigrationPanel />);
    const scanBtn = await screen.findByText("扫描整理");

    mocks.askMock.mockResolvedValue(false);
    await user.click(scanBtn);
    expect(mocks.invokeMock).not.toHaveBeenCalledWith("migration_scan");

    mocks.askMock.mockResolvedValue(true);
    await user.click(scanBtn);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("migration_scan");
    });
    expect(
      await screen.findByText(/上次扫描整理.*移动 2 · 删除 0 · 跳过 1/s),
    ).toBeInTheDocument();
    expect(screen.getByText("[扫描]已移动 b.txt")).toBeInTheDocument();
  });

  it("切换来源启用状态：调 migration_sources_save 提交翻转后的列表", async () => {
    const user = userEvent.setup();
    render(<MigrationPanel />);
    const toggle = await screen.findByTitle("已启用，点击停用");
    await user.click(toggle);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("migration_sources_save", {
        sources: {
          sources: [{ ...desktopSource, enabled: false }],
        },
      });
    });
  });

  it("添加文件夹：调 migration_sources_add_folder 并渲染新来源行", async () => {
    const user = userEvent.setup();
    render(<MigrationPanel />);
    await user.click(await screen.findByText("添加文件夹"));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith(
        "migration_sources_add_folder",
      );
    });
    expect(await screen.findByText("Downloads")).toBeInTheDocument();
    expect(screen.getByText("/Users/t/Downloads")).toBeInTheDocument();
  });

  it("查看整理记录：点击后弹窗展示 migration_log_read 返回内容，可关闭", async () => {
    const user = userEvent.setup();
    render(<MigrationPanel />);
    await user.click(await screen.findByText("查看整理记录"));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("migration_log_read", {
        limit: 500,
      });
    });
    expect(
      await screen.findByText("2026-09-01 移动 a.txt → 归档"),
    ).toBeInTheDocument();
    await user.click(screen.getByText("关闭"));
    expect(
      screen.queryByText("2026-09-01 移动 a.txt → 归档"),
    ).not.toBeInTheDocument();
  });

  it("导入规则表：成功后显示「已导入 N 条规则」并重新加载", async () => {
    const user = userEvent.setup();
    render(<MigrationPanel />);
    await user.click(await screen.findByText("导入规则表"));
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("migration_rules_import");
    });
    expect(await screen.findByText("已导入 3 条规则")).toBeInTheDocument();
    // 导入后 refresh：migration_rules_load 被调两次（初始 + 导入后）
    await waitFor(() => {
      const loads = mocks.invokeMock.mock.calls.filter(
        (c) => c[0] === "migration_rules_load",
      );
      expect(loads).toHaveLength(2);
    });
  });
});
