import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AppsPage } from "./AppsPage";
import type { App, AppRun, AppRunResult } from "../../lib/apps";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
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

function routeInvoke(cmd: string, args: Record<string, unknown>) {
  switch (cmd) {
    case "apps_list_enabled":
      return Promise.resolve(APPS);
    case "apps_runs_list":
      return Promise.resolve(args.appId === "wage-match" ? [RUN_OK] : []);
    case "apps_run":
      return Promise.resolve(RUN_RESULT);
    default:
      return Promise.resolve(null);
  }
}

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockImplementation(
    (cmd: string, args?: Record<string, unknown>) =>
      routeInvoke(cmd, args ?? {}),
  );
});

describe("AppsPage", () => {
  it("列表渲染：卡片名/输入模式徽章/上次运行状态线", async () => {
    render(<AppsPage />);
    expect(await screen.findByText("工资单匹配")).toBeInTheDocument();
    expect(screen.getByText("日报汇总")).toBeInTheDocument();
    // wage-match 有成功记录（1 小时前）；daily-report 无记录 → 尚未运行
    expect(screen.getByText("上次成功 · 1 小时前")).toBeInTheDocument();
    expect(screen.getByText("尚未运行")).toBeInTheDocument();
    expect(screen.getByText("文件夹")).toBeInTheDocument();
    expect(screen.getByText("表单")).toBeInTheDocument();
    // 头部计数徽章
    expect(screen.getByText("2 个")).toBeInTheDocument();
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
});
