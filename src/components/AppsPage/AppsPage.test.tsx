import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AppsPage } from "./AppsPage";
import type { App, AppRun, AppRunResult } from "../../lib/apps";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

// 归档走 useConfirm → Tauri 原生弹窗：mock 成「直接确认」
const askMock = vi.fn(async () => true);
vi.mock("@tauri-apps/plugin-dialog", () => ({
  ask: (...args: unknown[]) => askMock(...(args as [])),
}));

const hourAgo = Date.now() - 3_600_000;

const APPS: App[] = [
  {
    id: "wage-match",
    name: "工资单匹配",
    description: "把银行流水和工资表对上",
    icon: "💰",
    inputSchema: '{"kind":"folder"}',
    scriptPath: "app.py",
    manual: "",
    status: "enabled",
    enabledAt: 1,
    createdAt: 1,
    updatedAt: 1,
  },
  {
    id: "daily-report",
    name: "日报汇总",
    description: "",
    icon: "",
    inputSchema:
      '{"kind":"form","fields":[{"key":"k1","label":"字段1","type":"text"}]}',
    scriptPath: "app.py",
    manual: "",
    status: "enabled",
    enabledAt: 1,
    createdAt: 1,
    updatedAt: 2,
  },
  {
    id: "ai-draft",
    name: "竞品截图归档",
    description: "AI 开发中的草稿",
    icon: "📷",
    inputSchema: '{"kind":"folder"}',
    scriptPath: "app.py",
    manual: "",
    status: "draft",
    enabledAt: null,
    createdAt: 1,
    updatedAt: 3,
  },
];

const RUN_OK: AppRun = {
  id: 1,
  appId: "wage-match",
  startedAt: hourAgo,
  finishedAt: hourAgo + 1200,
  durationMs: 1200,
  status: "ok",
  summary: "匹配 32 行",
};

const RUN_RESULT: AppRunResult = {
  runId: 9,
  appId: "wage-match",
  outputDir: "/data/apps/wage-match/output/9/",
  resultJson: null,
  summary: "匹配 32 行",
  stdout: "",
  stderr: "",
  exitCode: 0,
  truncated: false,
  durationMs: 1200,
};

const ARCHIVED: App[] = [
  {
    id: "old-tool",
    name: "旧工具",
    description: "上季度用的，现在不用了",
    icon: "🗄",
    inputSchema: '{"kind":"folder"}',
    scriptPath: "app.py",
    manual: "",
    status: "archived",
    enabledAt: 1,
    createdAt: 1,
    updatedAt: 1,
  },
];

function routeInvoke(cmd: string, args: Record<string, unknown>) {
  switch (cmd) {
    case "apps_list_active":
      return Promise.resolve(APPS);
    case "apps_list_archived":
      return Promise.resolve(ARCHIVED);
    case "apps_runs_list":
      return Promise.resolve(args.appId === "wage-match" ? [RUN_OK] : []);
    case "apps_versions_list":
      return Promise.resolve(
        args.appId === "wage-match"
          ? [
              {
                id: 2,
                appId: "wage-match",
                version: 2,
                name: "工资单匹配",
                icon: "💰",
                source: "update",
                createdAt: hourAgo,
              },
              {
                id: 1,
                appId: "wage-match",
                version: 1,
                name: "工资单匹配",
                icon: "💰",
                source: "create",
                createdAt: hourAgo - 86_400_000,
              },
            ]
          : [],
      );
    case "apps_run":
      return Promise.resolve(RUN_RESULT);
    default:
      return Promise.resolve(null);
  }
}

beforeEach(() => {
  invokeMock.mockReset();
  askMock.mockClear();
  askMock.mockResolvedValue(true);
  invokeMock.mockImplementation(
    (cmd: string, args?: Record<string, unknown>) =>
      routeInvoke(cmd, args ?? {}),
  );
});

describe("AppsPage", () => {
  it("列表渲染：卡片名/输入模式徽章/上次运行状态线/草稿徽章", async () => {
    render(<AppsPage />);
    expect(await screen.findByText("工资单匹配")).toBeInTheDocument();
    expect(screen.getByText("日报汇总")).toBeInTheDocument();
    // wage-match 有成功记录（1 小时前）；daily-report / ai-draft 无记录 → 尚未运行
    expect(screen.getByText("上次成功 · 1 小时前")).toBeInTheDocument();
    expect(screen.getAllByText("尚未运行")).toHaveLength(2);
    expect(screen.getAllByText("文件夹")).toHaveLength(2);
    expect(screen.getByText("表单")).toBeInTheDocument();
    // draft 程序带「草稿」徽章（M2 草稿可试跑）
    expect(screen.getByText("草稿")).toBeInTheDocument();
    // 头部计数徽章
    expect(screen.getByText("3 个")).toBeInTheDocument();
  });

  it("点「运行」调 apps_run 并显示完成横幅（含耗时与摘要）", async () => {
    render(<AppsPage />);
    await screen.findByText("工资单匹配");
    await userEvent.click(screen.getAllByRole("button", { name: "运行" })[0]);
    expect(await screen.findByText(/运行完成/)).toBeInTheDocument();
    expect(screen.getByText("「工资单匹配」运行完成")).toBeInTheDocument();
    expect(screen.getByText("1.2s")).toBeInTheDocument();
    expect(invokeMock).toHaveBeenCalledWith("apps_run", expect.anything());
  });

  it("点「运行历史」展开历史面板列出记录", async () => {
    render(<AppsPage />);
    await screen.findByText("工资单匹配");
    await userEvent.click(screen.getAllByRole("button", { name: "运行历史" })[0]);
    expect(await screen.findByText("匹配 32 行")).toBeInTheDocument();
    // 历史行时间走 MM-DD HH:mm
    expect(screen.getByText(/\d{2}-\d{2} \d{2}:\d{2}/)).toBeInTheDocument();
  });

  it("版本页签：列出快照，最新版无回滚钮、旧版可回滚", async () => {
    render(<AppsPage />);
    await screen.findByText("工资单匹配");
    await userEvent.click(screen.getAllByRole("button", { name: "运行历史" })[0]);
    await userEvent.click(await screen.findByRole("button", { name: "版本" }));
    expect(await screen.findByText("v2")).toBeInTheDocument();
    expect(screen.getByText("v1")).toBeInTheDocument();
    expect(screen.getByText("创建")).toBeInTheDocument();
    // 最新版（v2）就是当前内容不提供回滚；v1 有一个回滚钮
    expect(screen.getAllByRole("button", { name: "回滚" })).toHaveLength(1);
  });

  it("无程序时空态引导上架（头部 CTA + 空态行动钮）", async () => {
    invokeMock.mockImplementation(() => Promise.resolve([]));
    render(<AppsPage />);
    expect(await screen.findByText("还没有程序")).toBeInTheDocument();
    expect(
      screen.getAllByRole("button", { name: "上架程序" }),
    ).toHaveLength(2);
  });

  it("上架弹窗：填表提交 apps_create", async () => {
    render(<AppsPage />);
    await screen.findByText("工资单匹配");
    await userEvent.click(screen.getAllByRole("button", { name: "上架程序" })[0]);
    const dialog = await screen.findByRole("dialog", { name: "上架程序" });
    expect(dialog).toBeInTheDocument();
    await userEvent.type(screen.getByPlaceholderText("wage-match"), "new-app");
    await userEvent.type(screen.getByPlaceholderText("工资单匹配"), "新程序");
    await userEvent.click(screen.getByRole("button", { name: "确认上架" }));
    await vi.waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "apps_create",
        expect.objectContaining({
          args: expect.objectContaining({ id: "new-app", name: "新程序" }),
        }),
      );
    });
  });
  it("删除按钮走归档：确认后调 apps_archive，卡片从程序页消失", async () => {
    const user = userEvent.setup();
    askMock.mockResolvedValue(true);
    render(<AppsPage />);
    await screen.findByText("工资单匹配");
    await user.click(screen.getByRole("button", { name: "删除程序：工资单匹配" }));
    expect(askMock).toHaveBeenCalled();
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("apps_archive", {
        id: "wage-match",
      }),
    );
    // 归档后重新拉两个列表（测试夹具里 active 恒有卡，故只断言命令被调）
    await waitFor(() =>
      expect(
        invokeMock.mock.calls.filter((c) => c[0] === "apps_list_active").length,
      ).toBeGreaterThan(1),
    );
  });

  it("脚本失败（exit_code!=0）弹红色失败横幅，不谎报运行完成", async () => {
    invokeMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "apps_run")
        return Promise.resolve({
          ...RUN_RESULT,
          exitCode: 2,
          summary: "❌ input/本月.xlsx 不存在",
        });
      return routeInvoke(cmd, args ?? {});
    });
    render(<AppsPage />);
    await screen.findByText("工资单匹配");
    await userEvent.click(screen.getAllByRole("button", { name: "运行" })[0]);
    expect(await screen.findByText("「工资单匹配」运行失败")).toBeInTheDocument();
    expect(screen.getByText(/❌ input\/本月\.xlsx 不存在/)).toBeInTheDocument();
    expect(screen.queryByText("「工资单匹配」运行完成")).not.toBeInTheDocument();
  });

  it("恢复最后一条归档后仍能切回程序列表（视图开关不消失）", async () => {
    const user = userEvent.setup();
    let hasArchived = true;
    invokeMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "apps_unarchive") {
        hasArchived = false;
        return Promise.resolve(null);
      }
      if (cmd === "apps_list_archived")
        return Promise.resolve(hasArchived ? ARCHIVED : []);
      return routeInvoke(cmd, args ?? {});
    });
    render(<AppsPage />);
    await screen.findByText("工资单匹配");
    await user.click(screen.getByRole("button", { name: /已归档/ }));
    await user.click(await screen.findByRole("button", { name: "恢复" }));
    // 恢复后归档列表清空：空态出现，但「已归档」开关必须还在（否则回不去）
    expect(await screen.findByText("没有已归档的程序")).toBeInTheDocument();
    const toggle = screen.getByRole("button", { name: /已归档/ });
    await user.click(toggle);
    expect(await screen.findByText("工资单匹配")).toBeInTheDocument();
  });

  it("「已归档」视图：列出归档程序，点恢复调 apps_unarchive", async () => {
    const user = userEvent.setup();
    render(<AppsPage />);
    await screen.findByText("工资单匹配");
    await user.click(screen.getByRole("button", { name: /已归档/ }));
    expect(screen.getByText("旧工具")).toBeInTheDocument();
    // 归档视图里不显示运行/删除，只给恢复
    expect(screen.queryByRole("button", { name: "运行" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "恢复" }));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("apps_unarchive", {
        id: "old-tool",
      }),
    );
  });
});
