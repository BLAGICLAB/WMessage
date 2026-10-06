// SchedulePage 组件测试：mock invoke 按命令名分发（schedule_overview / workflow_list /
// scheduled_job_* / workflow_set_schedule / schedule_set_enabled），验证分组渲染与写通道参数。
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SchedulePage } from "./SchedulePage";
import type { ScheduleEntry } from "../../types";

// 返回值类型按命令而异，用 unknown 宽松签名（mockImplementation 分发各命令桩）
const invokeMock = vi.hoisted(() =>
  vi.fn(async (_cmd: string): Promise<unknown> => null)
);
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
  emit: vi.fn(async () => {}),
}));

const jobEntry: ScheduleEntry = {
  kind: "job",
  targetId: "j1",
  title: "整理昨日进展写站会稿",
  detail: null,
  schedule: "daily:09:00",
  schedLast: null,
  nextRunAt: Date.now() + 2 * 3_600_000,
  missed: false,
  enabled: true,
  lastStatus: null,
  lastError: null,
  retryMax: 0,
  pauseOnFailure: false,
};

const wfEntry: ScheduleEntry = {
  kind: "workflow",
  targetId: "w1",
  title: "周报流水线",
  detail: "汇总本周任务出周报",
  schedule: "weekly:5:18:30",
  schedLast: null,
  nextRunAt: Date.now() + 3 * 86_400_000,
  missed: false,
  enabled: true,
  // workflow 条目恒为 null/0/false（只有 job 有意义）
  lastStatus: null,
  lastError: null,
  retryMax: 0,
  pauseOnFailure: false,
};

/** invoke 默认分发：overview 给入参列表，workflow_list 给一条工作流，其余 null */
function stubInvoke(overview: ScheduleEntry[]) {
  invokeMock.mockImplementation(async (cmd: string) => {
    switch (cmd) {
      case "schedule_overview":
        return overview;
      case "workflow_list":
        return [{ id: "w1", name: "周报流水线", goal: "汇总本周任务出周报" }];
      case "workflow_is_running":
        return false;
      default:
        return null;
    }
  });
}

beforeEach(() => {
  invokeMock.mockReset();
});

describe("SchedulePage 列表渲染", () => {
  it("按定时任务/工作流分组，频率人话 + 下次倒计时 + 即将执行摘要", async () => {
    stubInvoke([jobEntry, wfEntry]);
    render(<SchedulePage />);
    // 分组标题与两条目标（标题会同时出现在「即将执行」摘要条里，按行断言）
    const jobRow = await screen.findByTestId("schedule-row-job:j1");
    const wfRow = screen.getByTestId("schedule-row-workflow:w1");
    expect(within(jobRow).getByText("整理昨日进展写站会稿")).toBeInTheDocument();
    expect(within(wfRow).getByText("周报流水线")).toBeInTheDocument();
    // formatSchedule 人话文案
    expect(within(jobRow).getByText("每天 09:00")).toBeInTheDocument();
    expect(within(wfRow).getByText("每周五 18:30")).toBeInTheDocument();
    // 即将执行摘要：取 nextRunAt 最近一条（作业 2 小时后 < 工作流 3 天后）
    // 倒计时档位随断言时刻漂移（floor），只钉前缀与单位
    expect(screen.getByText(/最近将执行：/)).toHaveTextContent(
      /最近将执行：整理昨日进展写站会稿，\d+ 小时后/
    );
  });

  it("missed →「已错过」警示徽标；enabled=false →「已暂停」置灰徽标", async () => {
    stubInvoke([
      { ...jobEntry, missed: true },
      { ...wfEntry, enabled: false, nextRunAt: null },
    ]);
    render(<SchedulePage />);
    expect(await screen.findByText("已错过")).toBeInTheDocument();
    expect(screen.getByText("已暂停")).toBeInTheDocument();
  });

  it("空态：引导文案 + 新建定时入口", async () => {
    stubInvoke([]);
    render(<SchedulePage />);
    expect(await screen.findByText("暂无定时任务")).toBeInTheDocument();
    expect(screen.getByText(/写下要做什么，到点自动新建任务卡执行/)).toBeInTheDocument();
  });
});

describe("SchedulePage 写通道", () => {
  it("新建定时任务：写内容 + 选频率 → 预览确认 → scheduled_job_create(content, schedule)", async () => {
    const user = userEvent.setup();
    stubInvoke([]);
    render(<SchedulePage />);
    // 空态时顶栏与 EmptyState 各有一个「新建定时」，点哪个都进创建流程
    await user.click((await screen.findAllByText("新建定时"))[0]);
    // 默认类型 = 定时任务，直接写内容
    await user.type(screen.getByLabelText("定时任务内容"), "每天早上收集未完成任务");
    // 面板选频率：每天（时间输入默认回填今天 09:00）
    await user.click(screen.getByText("每天"));
    // 保存前人话预览（说明到点语义：新建任务卡交机器人）
    expect(
      screen.getByText(/每天 09:00，到点自动新建任务卡并交给机器人执行/)
    ).toBeInTheDocument();
    await user.click(screen.getByText("确认保存"));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("scheduled_job_create", {
        content: "每天早上收集未完成任务",
        schedule: "daily:09:00",
        retryMax: 0,
        pauseOnFailure: false,
      })
    );
  });

  it("内容为空时提交禁用", async () => {
    const user = userEvent.setup();
    stubInvoke([]);
    render(<SchedulePage />);
    await user.click((await screen.findAllByText("新建定时"))[0]);
    await user.click(screen.getByText("每天"));
    expect(screen.getByText("确认保存")).toBeDisabled();
  });

  it("新建工作流定时：选模板 + 选频率 → workflow_set_schedule(id, schedule)", async () => {
    const user = userEvent.setup();
    stubInvoke([]);
    render(<SchedulePage />);
    await user.click((await screen.findAllByText("新建定时"))[0]);
    await user.selectOptions(screen.getByLabelText("定时目标类型"), "workflow");
    await user.selectOptions(screen.getByLabelText("定时目标"), "w1");
    await user.click(screen.getByText("每天"));
    expect(screen.getByText(/到点自动执行该工作流/)).toBeInTheDocument();
    await user.click(screen.getByText("确认保存"));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("workflow_set_schedule", {
        id: "w1",
        schedule: "daily:09:00",
      })
    );
  });

  it("取消作业定时：两步确认 → scheduled_job_delete(id)", async () => {
    const user = userEvent.setup();
    stubInvoke([jobEntry]);
    render(<SchedulePage />);
    const row = await screen.findByTestId("schedule-row-job:j1");
    await user.click(within(row).getByText("取消定时"));
    // 第一次点击只 arm，不落库
    expect(invokeMock).not.toHaveBeenCalledWith(
      "scheduled_job_delete",
      expect.anything()
    );
    await user.click(within(row).getByText("确认取消？"));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("scheduled_job_delete", { id: "j1" })
    );
  });

  it("修改作业：编辑内容提交 → scheduled_job_update(id, content, schedule)", async () => {
    const user = userEvent.setup();
    stubInvoke([jobEntry]);
    render(<SchedulePage />);
    const row = await screen.findByTestId("schedule-row-job:j1");
    await user.click(within(row).getByText("修改"));
    const input = within(row).getByLabelText("定时任务内容");
    await user.clear(input);
    await user.type(input, "改成每周汇总");
    // 编辑态预填原规则，不重选频率也能提交
    await user.click(within(row).getByText("确认修改"));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("scheduled_job_update", {
        id: "j1",
        content: "改成每周汇总",
        schedule: "daily:09:00",
        retryMax: 0,
        pauseOnFailure: false,
      })
    );
  });

  it("暂停作业 → schedule_set_enabled(kind=job, id, enabled=false)", async () => {
    const user = userEvent.setup();
    stubInvoke([jobEntry]);
    render(<SchedulePage />);
    const row = await screen.findByTestId("schedule-row-job:j1");
    await user.click(within(row).getByText("暂停"));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("schedule_set_enabled", {
        kind: "job",
        id: "j1",
        enabled: false,
      })
    );
  });

  it("立即执行作业 → scheduled_job_fire(id)；工作流 → workflow_run", async () => {
    const user = userEvent.setup();
    stubInvoke([jobEntry, wfEntry]);
    render(<SchedulePage />);
    const jobRow = await screen.findByTestId("schedule-row-job:j1");
    await user.click(within(jobRow).getByText("立即执行"));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("scheduled_job_fire", { id: "j1" })
    );
    const wfRow = screen.getByTestId("schedule-row-workflow:w1");
    await user.click(within(wfRow).getByText("立即执行"));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("workflow_run", { workflowId: "w1" })
    );
  });
});

describe("SchedulePage 最近状态与执行历史", () => {
  it("lastStatus=fail → 失败徽标（title 带 lastError）；ok → 成功徽标；null 不显示", async () => {
    stubInvoke([
      { ...jobEntry, lastStatus: "fail", lastError: "模型超时" },
      { ...wfEntry },
    ]);
    render(<SchedulePage />);
    const row = await screen.findByTestId("schedule-row-job:j1");
    const failBadge = within(row).getByText("失败");
    expect(failBadge).toBeInTheDocument();
    expect(failBadge).toHaveAttribute("title", "模型超时");
    // 工作流条目恒无状态徽标
    const wfRow = screen.getByTestId("schedule-row-workflow:w1");
    expect(within(wfRow).queryByText("成功")).not.toBeInTheDocument();
    expect(within(wfRow).queryByText("失败")).not.toBeInTheDocument();
  });

  it("lastStatus=ok → 成功徽标", async () => {
    stubInvoke([{ ...jobEntry, lastStatus: "ok" }]);
    render(<SchedulePage />);
    const row = await screen.findByTestId("schedule-row-job:j1");
    expect(within(row).getByText("成功")).toBeInTheDocument();
  });

  it("展开历史 → scheduled_job_history 调用 + 渲染状态/耗时/摘要；空历史提示", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "schedule_overview":
          return [jobEntry];
        case "scheduled_job_history":
          return [
            {
              id: "r2",
              jobId: "j1",
              firedAt: Date.now() - 3_600_000,
              status: "fail",
              durationMs: 3200,
              summary: "模型超时",
              cardId: "c2",
            },
            {
              id: "r1",
              jobId: "j1",
              firedAt: Date.now() - 86_400_000,
              status: "ok",
              durationMs: 800,
              summary: null,
              cardId: "c1",
            },
          ];
        default:
          return null;
      }
    });
    const user = userEvent.setup();
    render(<SchedulePage />);
    const row = await screen.findByTestId("schedule-row-job:j1");
    await user.click(within(row).getByText("历史"));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("scheduled_job_history", { id: "j1" })
    );
    expect(await within(row).findByText("模型超时")).toBeInTheDocument();
    expect(within(row).getByText("3.2s")).toBeInTheDocument();
    expect(within(row).getByText("800ms")).toBeInTheDocument();
    // 历史列表内的成功/失败（行内还有 lastStatus 徽标位，故只钉存在性）
    expect(within(row).getAllByText("失败").length).toBeGreaterThan(0);
    expect(within(row).getAllByText("成功").length).toBeGreaterThan(0);
  });

  it("空历史：显示「还没有执行记录」", async () => {
    stubInvoke([jobEntry]);
    const user = userEvent.setup();
    render(<SchedulePage />);
    const row = await screen.findByTestId("schedule-row-job:j1");
    await user.click(within(row).getByText("历史"));
    expect(await within(row).findByText("还没有执行记录")).toBeInTheDocument();
  });
});

describe("SchedulePage 失败策略配置", () => {
  it("新建带 retryMax=2 + 自动暂停 → create 参数携带 retryMax/pauseOnFailure", async () => {
    const user = userEvent.setup();
    stubInvoke([]);
    render(<SchedulePage />);
    await user.click((await screen.findAllByText("新建定时"))[0]);
    await user.type(screen.getByLabelText("定时任务内容"), "每周汇总失败也要通知我");
    await user.click(screen.getByText("每天"));
    // 失败重试选 2 次、打开自动暂停
    await user.selectOptions(screen.getByLabelText("失败重试次数"), "2");
    await user.click(screen.getByLabelText("失败自动暂停"));
    await user.click(screen.getByText("确认保存"));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("scheduled_job_create", {
        content: "每周汇总失败也要通知我",
        schedule: "daily:09:00",
        retryMax: 2,
        pauseOnFailure: true,
      })
    );
  });

  it("编辑作业时回填 retryMax/pauseOnFailure", async () => {
    const user = userEvent.setup();
    stubInvoke([{ ...jobEntry, retryMax: 3, pauseOnFailure: true }]);
    render(<SchedulePage />);
    const row = await screen.findByTestId("schedule-row-job:j1");
    await user.click(within(row).getByText("修改"));
    expect(within(row).getByLabelText("失败重试次数")).toHaveValue("3");
    expect(within(row).getByLabelText("失败自动暂停")).toBeChecked();
    // 不改任何值直接提交：配置原样带回
    await user.click(within(row).getByText("确认修改"));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("scheduled_job_update", {
        id: "j1",
        content: "整理昨日进展写站会稿",
        schedule: "daily:09:00",
        retryMax: 3,
        pauseOnFailure: true,
      })
    );
  });
});
