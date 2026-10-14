import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, act, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { HomePage } from "./HomePage";
import type { Task } from "../../types";
import type { TraceRow } from "../../lib/trace";
import { writeKpiSnapshot } from "../../lib/homeKpi";

// —— Tauri mocks：invoke 按 command 名路由（trace_list / notifications_list）——
const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}));

vi.mock("../../profile", () => ({
  getProfileCache: vi.fn(() => ({
    user: { name: "测试者", avatarDataUrl: null },
    bot: { name: "机器人", avatarDataUrl: null },
  })),
  loadProfile: vi.fn(async () => ({
    user: { name: "测试者", avatarDataUrl: null },
    bot: { name: "机器人", avatarDataUrl: null },
  })),
  subscribeProfile: vi.fn(() => () => {}),
}));

// 固定时钟：2026-10-11（周日）15:42
const NOW = new Date(2026, 9, 11, 15, 42);
const DAY = 86_400_000;

function task(partial: Partial<Task> & { id: string }): Task {
  return { column: "todo", ...partial } as Task;
}

function trace(partial: Partial<TraceRow> & { id: number }): TraceRow {
  return {
    sessionId: "s",
    origin: "workflow",
    title: "周报生成",
    status: "done",
    startedAt: NOW.getTime() - 3_600_000,
    finishedAt: NOW.getTime() - 3_500_000,
    turnCount: 2,
    toolCalls: 1,
    filesChanged: 0,
    promptTokens: 0,
    completionTokens: 0,
    ...partial,
  };
}

const TASKS: Task[] = [
  task({ id: "p1", title: "首页模块小样", planStart: "2026-10-11T15:15", planEnd: "2026-10-11T16:45", tags: ["设计"] }),
  task({ id: "p2", title: "需求清单梳理", planStart: "2026-10-11T09:00", planEnd: "2026-10-11T10:30", completedAt: NOW.getTime() }),
  task({ id: "p3", title: "周报汇总", planStart: "2026-10-11T19:00", planEnd: "2026-10-11T20:00" }),
  task({ id: "d1", title: "提交验收报告", due: "2026-10-11T18:00" }),
  task({ id: "d2", title: "OCR 分诊复核", due: "2026-10-10" }),
  // 近 7 天每天都有完成（柱状图每桶都有值；值为 0 的桶 recharts 不渲染 rect）
  task({ id: "c0", title: "c0", completedAt: NOW.getTime() - 6 * DAY, column: "done" }),
  task({ id: "c1", title: "c1", completedAt: NOW.getTime() - 5 * DAY, column: "done" }),
  task({ id: "c2", title: "c2", completedAt: NOW.getTime() - 4 * DAY, column: "done" }),
  task({ id: "c3", title: "c3", completedAt: NOW.getTime() - 3 * DAY, column: "done" }),
  task({ id: "c4", title: "c4", completedAt: NOW.getTime() - 2 * DAY, column: "done" }),
  task({ id: "c5", title: "c5", completedAt: NOW.getTime() - 1 * DAY, column: "done" }),
  task({ id: "arch1", title: "归档样例", archived: true, completedAt: NOW.getTime() - 8 * DAY, updatedAt: NOW.getTime() - 1 * DAY, tags: ["开发"], column: "done" }),
  task({ id: "arch2", title: "归档样例 b", archived: true, completedAt: NOW.getTime() - 9 * DAY, updatedAt: NOW.getTime() - 2 * DAY, tags: ["设计"], column: "done" }),
  task({ id: "arch3", title: "归档样例 c", archived: true, completedAt: NOW.getTime() - 10 * DAY, updatedAt: NOW.getTime() - 3 * DAY, column: "done" }),
];

const TRACES: TraceRow[] = [
  trace({ id: 1, status: "running", startedAt: NOW.getTime() - 60_000, finishedAt: null }),
  trace({ id: 2, status: "done", title: "生成交付 PDF" }),
  trace({ id: 3, status: "failed", title: "OCR 分诊复核", startedAt: NOW.getTime() - 2 * DAY }),
];

const NOTIFICATIONS = [
  {
    id: "n1",
    kind: "workflow_question" as const,
    title: "「周报生成」问你：周报收件人是谁？",
    body: "",
    payload: null,
    status: "pending" as const,
    createdAt: NOW.getTime(),
    resolvedAt: null,
  },
];

beforeEach(() => {
  localStorage.clear();
  invokeMock.mockReset();
  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === "trace_list") return TRACES;
    if (cmd === "notifications_list") return NOTIFICATIONS;
    if (cmd === "bing_daily")
      return {
        title: "秋冬菜单静物",
        copyright: "© 摄影师（Bing 中国）",
        dataUrl: "data:image/jpeg;base64,anBlZ2J5dGVz",
        imageUrl: "https://cn.bing.com/th?id=OHR.X",
        date: "2026-10-11",
      };
    return null;
  });
});

/** framer-motion useScroll 要求已水合的容器元素（未挂载的占位元素即可） */
function makeScrollRef(): React.RefObject<HTMLElement | null> {
  return { current: document.createElement("main") };
}

/** 每天都有排期的任务集（0 值柱 recharts 不渲染 rect，周图联动断言要用它） */
const WEEK_TASKS: Task[] = TASKS.concat(
  Array.from({ length: 6 }, (_, i) => {
    const d = new Date(2026, 9, 5 + i);
    const p = (n: number) => String(n).padStart(2, "0");
    return task({
      id: `w${i}`,
      title: `w${i}`,
      planStart: `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T09:00`,
    });
  }),
  // 周六第二行：行间移动（不弹回今日）断言需要同页两行
  [task({ id: "w5b", title: "w5b", planStart: "2026-10-10T14:00" })],
).flat();

function renderHome(tasks: Task[] = TASKS) {
  return render(
    <HomePage
      tasks={tasks}
      scrollRef={makeScrollRef()}
      onNavigate={vi.fn()}
      onOpenTask={vi.fn()}
      now={NOW}
    />,
  );
}

describe("HomePage 渲染", () => {
  it("欢迎区：问候带个人资料姓名 + 小字只统计今日计划/完成", () => {
    renderHome();
    expect(screen.getByText("下午好，测试者")).toBeInTheDocument();
    expect(screen.getByText("10月11日 周日")).toBeInTheDocument();
    expect(screen.getByText("今日计划 3，已完成 1")).toBeInTheDocument();
    expect(screen.queryByText("新建任务")).not.toBeInTheDocument();
  });

  it("今日完成率进度条 = 今日已完成/今日计划（1/3 → 33%）", () => {
    renderHome();
    const fill = document.querySelector(".home-progress-fill") as HTMLElement;
    expect(fill).not.toBeNull();
    expect(fill.style.width).toBe("33%");
    expect(fill.textContent).toBe("33%");
  });

  it("首页 KPI 默认 4 项真实计数：完成率 10/14、大型任务族 0", () => {
    renderHome();
    // 14 张有效卡（未删，含归档）里 10 张有完成时间；fixture 无任何关联关系
    expect(screen.getByTestId("kpi-value-completionRate")).toHaveTextContent("71.4%");
    expect(screen.getByTestId("kpi-value-largeCount")).toHaveTextContent("0");
    expect(screen.getByTestId("kpi-value-largeRatio")).toHaveTextContent("0%");
    expect(screen.getByTestId("kpi-value-largeDoneRate")).toHaveTextContent("—");
  });

  it("三池计数回到首行右侧：待办 4 / 完成 7 / 归档 3", () => {
    renderHome();
    expect(screen.getByTestId("pool-todo")).toHaveTextContent("4");
    expect(screen.getByTestId("pool-done")).toHaveTextContent("7");
    expect(screen.getByTestId("pool-archived")).toHaveTextContent("3");
  });

  it("首页 KPI 涨跌角标：对比上一次变动前的快照基线，向好=绿", () => {
    const last = {
      completionRate: 70,
      largeCount: 0,
      largeRatio: 0,
      largeDoneRate: 0,
      onTimeRate: 0,
      overdueCount: 0,
      planRate: 0,
      shelfDuration: null,
    };
    writeKpiSnapshot({ last, prev: null });
    renderHome();
    const badge = screen.getByTestId("kpi-trend-completionRate");
    expect(badge).toHaveAttribute("title", "较上次变动 +1.4%");
    expect(badge.className).toContain("text-[var(--success)]");
    // 数值没变的指标不出角标
    expect(screen.queryByTestId("kpi-trend-largeCount")).not.toBeInTheDocument();
  });

  it("归档脉搏：本周归档/打标率/阈值 + 标签 chips + 最近批次脚注", () => {
    renderHome();
    expect(screen.getByText("归档脉搏")).toBeInTheDocument();
    expect(screen.getByText("2")).toBeInTheDocument(); // 本周归档
    expect(screen.getByText("67%")).toBeInTheDocument(); // 自动打标 2/3
    expect(screen.getByText("7 天")).toBeInTheDocument(); // bot_get_config 阈值
    expect(screen.getByText(/开发/)).toBeInTheDocument();
    expect(screen.getByText(/设计/)).toBeInTheDocument();
    expect(
      screen.getByText("最近一批 1 天前归档，可在归档页恢复"),
    ).toBeInTheDocument();
  });

  it("最近归档：按归档时间倒序的卡列表 + 总数脚注", () => {
    renderHome();
    expect(screen.getByText("最近归档")).toBeInTheDocument();
    const list = screen.getByTestId("recent-archived-list");
    const titles = within(list)
      .getAllByText(/归档样例/)
      .map((el) => el.textContent);
    expect(titles).toEqual(["归档样例", "归档样例 b", "归档样例 c"]);
    expect(screen.getByText("归档区共 3 张卡")).toBeInTheDocument();
  });

  it("必应每日一图卡：成功态铺图 + 标题版权", async () => {
    renderHome();
    await screen.findByText("秋冬菜单静物");
    expect(screen.getByText(/© 摄影师/)).toBeInTheDocument();
    expect(document.querySelector("img[alt='秋冬菜单静物']")).not.toBeNull();
  });

  it("必应每日一图卡：失败显示友好占位 + 重试", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "bing_daily") throw new Error("网络不可达");
      if (cmd === "trace_list") return TRACES;
      if (cmd === "notifications_list") return NOTIFICATIONS;
      return null;
    });
    renderHome();
    await screen.findByText("今日壁纸没拉到");
    expect(screen.getByText("重试")).toBeInTheDocument();
    expect(screen.queryByText("秋冬菜单静物")).not.toBeInTheDocument();
  });

  it("本周计划与完成：周一..周日两组柱（14 根）+ 图例", async () => {
    // 每天都有排期（0 值柱 recharts 不渲染 rect，避免计数失真）
    renderHome(WEEK_TASKS);
    await screen.findByText("本周计划与完成");
    // 7 天 × 计划/完成 两根柱（以 Cell 的 testid 计数，绕开包装层类名）
    expect(
      document.querySelectorAll("[data-testid^='week-plan-'], [data-testid^='week-done-']"),
    ).toHaveLength(14);
  });

  it("即将到期横条：逾期任务在前（1/2 计数），未轮到的任务不渲染", () => {
    renderHome();
    const ticker = within(screen.getByTestId("due-ticker"));
    expect(ticker.getByText("即将到期")).toBeInTheDocument();
    expect(ticker.getByText("OCR 分诊复核")).toBeInTheDocument();
    expect(ticker.getByText("逾期 1 天")).toBeInTheDocument();
    expect(ticker.getByText("1/2")).toBeInTheDocument();
    expect(ticker.queryByText("提交验收报告")).not.toBeInTheDocument();
  });

  it("即将到期横条：4s 轮播到下一条（悬停暂停逻辑见组件）", async () => {
    vi.useFakeTimers();
    try {
      renderHome();
      const ticker = within(screen.getByTestId("due-ticker"));
      expect(ticker.getByText("OCR 分诊复核")).toBeInTheDocument();
      // act 包裹：interval 里的 setIdx 必须在 act 内 flush 才落 DOM
      await act(async () => {
        await vi.advanceTimersByTimeAsync(4_000);
      });
      expect(ticker.getByText("提交验收报告")).toBeInTheDocument();
      expect(ticker.getByText("今天 18:00")).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("机器人动态：执行中排最前，失败行也在", async () => {
    renderHome();
    await screen.findByText("工作流 · 周报生成");
    expect(screen.getAllByText("执行中")).toHaveLength(1);
    expect(screen.getByText("失败")).toBeInTheDocument();
  });

  it("通知：pending 消息渲染 + 待处理数脚注", async () => {
    renderHome();
    await screen.findByText(/「周报生成」问你/);
    expect(screen.getByText("去处理")).toBeInTheDocument();
    expect(screen.getByText("1 条待处理")).toBeInTheDocument();
  });

  it("通知加载中：footer 与 body 同口径，不提前显示「都处理完了」", () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "trace_list") return TRACES;
      if (cmd === "notifications_list") return new Promise(() => {}); // 挂起 = 加载中
      if (cmd === "bing_daily")
        return {
          title: "秋冬菜单静物",
          copyright: "© 摄影师（Bing 中国）",
          dataUrl: "data:image/jpeg;base64,anBlZ2J5dGVz",
          imageUrl: "https://cn.bing.com/th?id=OHR.X",
          date: "2026-10-11",
        };
      return null;
    });
    renderHome();
    const panel = screen.getByText("通知", { selector: "h3" }).closest("section")!;
    // body 与 footer 同口径：都是加载中，不提前报「都处理完了」
    expect(within(panel).getAllByText("加载中…")).toHaveLength(2);
    expect(within(panel).queryByText("都处理完了")).not.toBeInTheDocument();
  });

  it("通知与执行痕迹同模式 30s 轻轮询：挂载窗口期丢事件可自愈", async () => {
    vi.useFakeTimers();
    try {
      renderHome();
      const notifCalls = () =>
        invokeMock.mock.calls.filter(([cmd]) => cmd === "notifications_list")
          .length;
      expect(notifCalls()).toBe(1); // 挂载即拉一次
      await act(async () => {
        await vi.advanceTimersByTimeAsync(30_000);
      });
      expect(notifCalls()).toBe(2); // 30s 后再拉（轮询自愈）
      await act(async () => {
        await vi.advanceTimersByTimeAsync(30_000);
      });
      expect(notifCalls()).toBe(3);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("Linked Hover 联动（周图 / 日程 / 任务卡 / 图谱）", () => {
  it("默认页 = 今日（fixture 今天是周日 → 7/8）", () => {
    renderHome(WEEK_TASKS);
    expect(screen.getByTestId("pool-pager-counter")).toHaveTextContent("7/8");
    expect(screen.getByTestId("pool-page-label")).toHaveTextContent("周日 10/11");
  });

  it("悬停日程列头：周图出橙色参考线、非当日柱降透明，任务卡翻到该天页", async () => {
    const user = userEvent.setup();
    renderHome(WEEK_TASKS);
    // 悬停前：无参考线，任务卡停在今日页
    expect(document.querySelectorAll(".recharts-reference-line")).toHaveLength(0);
    expect(screen.getByTestId("pool-pager-counter")).toHaveTextContent("7/8");

    await user.hover(screen.getByTestId("daygrid-head-2"));

    // 周图：橙色竖向参考线落在该天，非当日柱降透明
    expect(document.querySelectorAll(".recharts-reference-line")).toHaveLength(1);
    const bar0 = screen.getByTestId("week-plan-0") as HTMLElement;
    const bar2 = screen.getByTestId("week-plan-2") as HTMLElement;
    expect(bar0.style.opacity).toBe("0.25");
    expect(bar2.style.opacity).toBe("1");
    // 任务卡自动翻到该天页（周三 10/7）
    expect(screen.getByTestId("pool-pager-counter")).toHaveTextContent("3/8");
    expect(screen.getByTestId("pool-page-label")).toHaveTextContent("周三 10/7");
    // 列头高亮
    expect(screen.getByTestId("daygrid-head-2")).toHaveAttribute("data-active");
  });

  it("移开后还原：参考线消失、任务卡回到今日页、柱透明度回满", async () => {
    const user = userEvent.setup();
    renderHome(WEEK_TASKS);
    await user.hover(screen.getByTestId("daygrid-head-3"));
    expect(document.querySelectorAll(".recharts-reference-line")).toHaveLength(1);
    // 指针移出日程卡 → 清除（onMouseLeave）
    await user.hover(screen.getByTestId("due-ticker"));
    expect(document.querySelectorAll(".recharts-reference-line")).toHaveLength(0);
    expect(screen.getByTestId("pool-pager-counter")).toHaveTextContent("7/8");
    const bar0 = screen.getByTestId("week-plan-0") as HTMLElement;
    expect(bar0.style.opacity).toBe("1");
  });

  it("键盘可达：聚焦日程列头同样联动（a11y 等价交互面）", () => {
    renderHome();
    fireEvent.focus(screen.getByTestId("daygrid-head-4"));
    expect(document.querySelectorAll(".recharts-reference-line")).toHaveLength(1);
    expect(screen.getByTestId("pool-pager-counter")).toHaveTextContent("5/8");
  });

  it("手动翻页是临时偏移：指针离开卡片恢复今日", () => {
    renderHome(WEEK_TASKS);
    // 从今日（7/8）‹ 翻一页到周六
    fireEvent.click(screen.getByLabelText("上一页"));
    expect(screen.getByTestId("pool-pager-counter")).toHaveTextContent("6/8");
    expect(screen.getByTestId("pool-page-label")).toHaveTextContent("周六 10/10");
    // 指针离开卡片 → 恢复今日
    fireEvent.mouseLeave(screen.getByTestId("pool-page-list").closest("section")!);
    expect(screen.getByTestId("pool-pager-counter")).toHaveTextContent("7/8");
  });

  it("联动中移向 ‹（离开日程卡回今日），点 ‹ 从今日起翻，离开任务卡恢复今日", async () => {
    const user = userEvent.setup();
    renderHome(WEEK_TASKS);
    await user.hover(screen.getByTestId("daygrid-head-2"));
    expect(screen.getByTestId("pool-pager-counter")).toHaveTextContent("3/8");
    // 指针移向 ‹：先离开日程卡 → 联动清除，任务卡回到今日页
    await user.hover(screen.getByLabelText("上一页"));
    expect(screen.getByTestId("pool-pager-counter")).toHaveTextContent("7/8");
    // 从今日 ‹ 翻一页 → 周六
    fireEvent.click(screen.getByLabelText("上一页"));
    expect(screen.getByTestId("pool-pager-counter")).toHaveTextContent("6/8");
    // 指针离开任务卡 → 恢复今日
    await user.hover(screen.getByTestId("due-ticker"));
    expect(screen.getByTestId("pool-pager-counter")).toHaveTextContent("7/8");
  });

  it("未排期池是第 8 页，从今日 › 一次可达", () => {
    renderHome(WEEK_TASKS);
    fireEvent.click(screen.getByLabelText("下一页"));
    expect(screen.getByTestId("pool-pager-counter")).toHaveTextContent("8/8");
    expect(screen.getByTestId("pool-page-label")).toHaveTextContent("未排期池");
    // 再 › 环绕回第 1 页（周一）
    fireEvent.click(screen.getByLabelText("下一页"));
    expect(screen.getByTestId("pool-pager-counter")).toHaveTextContent("1/8");
  });

  it("图谱速览：任务节点渲染，联动指向节点时无关节点降透明", () => {
    renderHome();
    const nodes = document.querySelectorAll("[data-testid^='graph-node-']");
    expect(nodes.length).toBeGreaterThanOrEqual(5);
    fireEvent.mouseEnter(nodes[0]);
    // 指向节点后，图谱内出现降透明的无关节点（Dim）
    const dimmed = document.querySelectorAll(
      "[data-testid^='graph-node-'][opacity='0.22']",
    );
    expect(dimmed.length).toBeGreaterThanOrEqual(1);
    fireEvent.mouseLeave(nodes[0]);
  });

  it("任务卡级联动：日程块与任务卡行互相高亮", () => {
    renderHome(WEEK_TASKS);
    // 翻到周日页（列头联动）
    fireEvent.mouseEnter(screen.getByTestId("daygrid-head-6"));
    const list = screen.getByTestId("pool-page-list");
    const row = within(list).getByText("首页模块小样").closest("button")!;
    // 悬停任务卡行 → 日程里该任务块描边高亮
    fireEvent.mouseEnter(row);
    expect(screen.getByTestId("daygrid-block-p1")).toHaveAttribute("data-active");
    expect(row).toHaveAttribute("data-active");
    // 悬停日程块 → 任务卡行同样高亮（同一联动状态）
    fireEvent.mouseLeave(row);
    fireEvent.mouseEnter(screen.getByTestId("daygrid-block-p1"));
    expect(row).toHaveAttribute("data-active");
    fireEvent.mouseLeave(screen.getByTestId("daygrid-block-p1"));
  });

  it("手动翻页后在行间移动指针不弹回今日", async () => {
    const user = userEvent.setup();
    renderHome(WEEK_TASKS);
    fireEvent.click(screen.getByLabelText("上一页")); // 今日(7/8) ‹ → 周六 6/8
    const list = screen.getByTestId("pool-page-list");
    // 行间移动：relatedTarget 指向卡内下一行，组件 mouseLeave 不触发
    await user.hover(within(list).getByText("w5").closest("button")!);
    await user.hover(within(list).getByText("w5b").closest("button")!);
    expect(screen.getByTestId("pool-pager-counter")).toHaveTextContent("6/8");
    // 指针离开小组件 → 恢复今日
    await user.hover(screen.getByTestId("due-ticker"));
    expect(screen.getByTestId("pool-pager-counter")).toHaveTextContent("7/8");
  });
});
