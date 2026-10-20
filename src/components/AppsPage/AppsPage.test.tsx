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
    timeoutSecs: null,
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
    // 声明了单次运行时长（转写长录音这类活）→ 卡上要看得见
    timeoutSecs: 1800,
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
    timeoutSecs: null,
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

/** 被时长上限砍掉的一次运行：状态是 timeout，文案说清是被砍而不是没起来 */
const RUN_TIMEOUT: AppRun = {
  id: 2,
  appId: "wage-match",
  startedAt: hourAgo,
  finishedAt: hourAgo + 1_800_000,
  durationMs: 1_800_000,
  status: "timeout",
  summary: "执行超时（1800s）已强制终止",
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
    timeoutSecs: null,
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

  it("超时的运行：历史里如实显示被砍，不说成「启动失败」，状态点不是红的", async () => {
    invokeMock.mockImplementation((cmd: string, args: Record<string, unknown>) => {
      if (cmd === "apps_runs_list") {
        return Promise.resolve(
          args.appId === "wage-match" ? [RUN_TIMEOUT, RUN_OK] : [],
        );
      }
      return routeInvoke(cmd, args);
    });
    render(<AppsPage />);
    await screen.findByText("工资单匹配");
    await userEvent.click(screen.getAllByRole("button", { name: "运行历史" })[0]);
    const summary = await screen.findByText("执行超时（1800s）已强制终止");
    expect(summary).toBeInTheDocument();
    expect(screen.queryByText(/启动失败/)).not.toBeInTheDocument();
    // 超时行的状态点走中性色（不是错误红）
    const dot = summary.parentElement?.querySelector("span[aria-hidden]");
    expect(dot?.className).toContain("bg-[var(--t5)]");
    expect(dot?.className).not.toContain("bg-[var(--danger)]");
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

  it("声明了运行时长的程序，卡上看得见「最长 30 分钟」；没声明的不写时长", async () => {
    render(<AppsPage />);
    await screen.findByText("日报汇总");
    expect(screen.getByText("最长 30 分钟")).toBeInTheDocument();
    // 未声明 = 走内置默认，不是这个程序的属性 → 卡上不摆时长
    expect(screen.queryByText("最长 1 分钟")).not.toBeInTheDocument();
  });

  it("上架弹窗：填的运行时长随 apps_create 一起提交", async () => {
    render(<AppsPage />);
    await screen.findByText("工资单匹配");
    await userEvent.click(screen.getAllByRole("button", { name: "上架程序" })[0]);
    await userEvent.type(screen.getByPlaceholderText("wage-match"), "stt");
    await userEvent.type(screen.getByPlaceholderText("工资单匹配"), "语音转写");
    await userEvent.type(
      screen.getByLabelText("单次运行时长（秒）"),
      "1800",
    );
    await userEvent.click(screen.getByRole("button", { name: "确认上架" }));
    await vi.waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "apps_create",
        expect.objectContaining({
          args: expect.objectContaining({ id: "stt", timeoutSecs: 1800 }),
        }),
      );
    });
  });

  it("上架弹窗：非整数时长当场拒（不静默按默认跑）", async () => {
    render(<AppsPage />);
    await screen.findByText("工资单匹配");
    await userEvent.click(screen.getAllByRole("button", { name: "上架程序" })[0]);
    await userEvent.type(screen.getByPlaceholderText("wage-match"), "stt");
    await userEvent.type(screen.getByPlaceholderText("工资单匹配"), "语音转写");
    await userEvent.type(
      screen.getByLabelText("单次运行时长（秒）"),
      "十分钟",
    );
    await userEvent.click(screen.getByRole("button", { name: "确认上架" }));
    expect(
      await screen.findByText(/单次运行时长填非负整数秒/),
    ).toBeInTheDocument();
    expect(
      invokeMock.mock.calls.some((c) => c[0] === "apps_create"),
    ).toBe(false);
  });
  it("程序页的删除键走归档（可逆）：确认后调 apps_archive，卡片从程序页消失", async () => {
    const user = userEvent.setup();
    askMock.mockResolvedValue(true);
    render(<AppsPage />);
    await screen.findByText("工资单匹配");
    await user.click(screen.getByRole("button", { name: "归档程序：工资单匹配" }));
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
    // 程序页这一下是归档不是删除：不碰 apps_delete
    expect(invokeMock).not.toHaveBeenCalledWith(
      "apps_delete",
      expect.anything(),
    );
  });

  it("程序页没有彻底删除键——不可逆的那一下只在归档视图里", async () => {
    render(<AppsPage />);
    await screen.findByText("工资单匹配");
    // 两个垃圾桶摆在一处、一个可逆一个不可逆时，光看图标分不出来：
    // 程序页这侧必须是归档键，彻底删除键不出现
    expect(
      screen.queryByRole("button", { name: /彻底删除程序/ }),
    ).not.toBeInTheDocument();
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
    // 归档视图里不显示运行，不显示归档键（它已经是归档态），只给恢复 + 彻底删除
    expect(screen.queryByRole("button", { name: "运行" })).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "归档程序：旧工具" }),
    ).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "恢复" }));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("apps_unarchive", {
        id: "old-tool",
      }),
    );
  });

  it("归档视图里能彻底删除：确认后调 apps_delete 并重拉列表", async () => {
    const user = userEvent.setup();
    askMock.mockResolvedValue(true);
    let hasArchived = true;
    invokeMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "apps_delete") {
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
    await user.click(
      await screen.findByRole("button", { name: "彻底删除程序：旧工具" }),
    );
    // 不可逆操作必须先过确认，且确认文案要说清会失去什么
    expect(askMock).toHaveBeenCalled();
    const confirmText = JSON.stringify(askMock.mock.calls[0]);
    expect(confirmText).toContain("彻底删除");
    expect(confirmText).toContain("无法恢复");
    expect(confirmText).toContain("input/");
    expect(confirmText).toContain("output/");
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("apps_delete", {
        id: "old-tool",
      }),
    );
    expect(await screen.findByText("没有已归档的程序")).toBeInTheDocument();
  });

  it("取消确认就不该发出删除命令", async () => {
    const user = userEvent.setup();
    askMock.mockResolvedValue(false);
    render(<AppsPage />);
    await screen.findByText("工资单匹配");
    await user.click(screen.getByRole("button", { name: /已归档/ }));
    await user.click(
      await screen.findByRole("button", { name: "彻底删除程序：旧工具" }),
    );
    expect(invokeMock).not.toHaveBeenCalledWith(
      "apps_delete",
      expect.anything(),
    );
    expect(screen.getByText("旧工具")).toBeInTheDocument();
  });
});
