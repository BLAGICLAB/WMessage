import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { HomePage } from "./HomePage";
import type { Task } from "../../types";
import type { TraceRow } from "../../lib/trace";

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
  task({ id: "arch1", title: "归档样例", archived: true, completedAt: NOW.getTime() - 8 * DAY, column: "done" }),
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
  invokeMock.mockReset();
  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === "trace_list") return TRACES;
    if (cmd === "notifications_list") return NOTIFICATIONS;
    return null;
  });
});

/** framer-motion useScroll 要求已水合的容器元素（未挂载的占位元素即可） */
function makeScrollRef(): React.RefObject<HTMLElement | null> {
  return { current: document.createElement("main") };
}

function renderHome() {
  return render(
    <HomePage
      tasks={TASKS}
      scrollRef={makeScrollRef()}
      onNavigate={vi.fn()}
      onOpenTask={vi.fn()}
      onNewTask={vi.fn()}
      now={NOW}
    />,
  );
}

/** KPI 大数字：KPI 标签在 text-right 块里（阶段条也有「已完成」字样，按容器区分） */
function kpiValue(label: string): string {
  const el = screen
    .getAllByText(label)
    .find((e) => e.closest(".text-right"));
  expect(el).toBeDefined();
  return el!.previousElementSibling!.textContent ?? "";
}

describe("HomePage 渲染", () => {
  it("欢迎区：问候带个人资料姓名 + 小字只统计今日计划/完成", () => {
    renderHome();
    expect(screen.getByText("下午好，测试者")).toBeInTheDocument();
    expect(screen.getByText("10月11日 周日")).toBeInTheDocument();
    expect(screen.getByText("今日计划 3 段，已完成 1")).toBeInTheDocument();
  });

  it("今日完成率进度条 = 今日已完成/今日计划（1/3 → 33%）", () => {
    renderHome();
    const fill = document.querySelector(".home-progress-fill") as HTMLElement;
    expect(fill).not.toBeNull();
    expect(fill.style.width).toBe("33%");
    expect(fill.textContent).toBe("33%");
  });

  it("KPI 三池真实计数：待办 4 / 完成 7 / 归档 1", () => {
    renderHome();
    expect(kpiValue("待办")).toBe("4");
    expect(kpiValue("完成")).toBe("7");
    expect(kpiValue("归档")).toBe("1");
  });

  it("今日时间带：3 个今日排期块（1 个已完成）", () => {
    renderHome();
    const lane = screen.getByTestId("ribbon-lane");
    const blocks = lane.querySelectorAll(".home-ribbon-blk");
    expect(blocks).toHaveLength(3);
    expect(lane.querySelector(".home-ribbon-blk.done")).not.toBeNull();
  });

  it("即将到期：逾期在前（OCR 分诊复核 逾期 1 天），已完成任务不出现", () => {
    renderHome();
    expect(screen.getByText("OCR 分诊复核")).toBeInTheDocument();
    expect(screen.getByText("逾期 1 天")).toBeInTheDocument();
    expect(screen.getByText("提交验收报告")).toBeInTheDocument();
    expect(screen.queryByText("done-old")).not.toBeInTheDocument();
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
});

describe("Linked Hover 联动", () => {
  it("悬停日期签：两张图共用参考线，其余柱降透明，卡头读数切到该天", async () => {
    const user = userEvent.setup();
    renderHome();
    // 悬停前：无参考线，卡头显示本周合计
    expect(document.querySelectorAll(".recharts-reference-line")).toHaveLength(0);
    expect(screen.getAllByText(/本周合计/)).toHaveLength(2);

    await user.hover(screen.getByTestId("trend-chip-2"));

    // 两张图各一条竖向参考线（共用同一天）
    expect(document.querySelectorAll(".recharts-reference-line")).toHaveLength(2);
    // 非悬停柱降透明，悬停柱不降
    const bar0 = screen.getByTestId("trend-bar-0") as HTMLElement;
    const bar2 = screen.getByTestId("trend-bar-2") as HTMLElement;
    expect(bar0.style.opacity).toBe("0.22");
    expect(bar2.style.opacity).toBe("1");
    // 两张卡的读数都切到悬停天（10月7日）
    expect(screen.getAllByText("10月7日")).toHaveLength(2);
    // 日期签高亮
    expect(screen.getByTestId("trend-chip-2")).toHaveAttribute("data-active");
  });

  it("移开后还原：参考线消失、透明度回满", async () => {
    const user = userEvent.setup();
    renderHome();
    await user.hover(screen.getByTestId("trend-chip-3"));
    expect(document.querySelectorAll(".recharts-reference-line")).toHaveLength(2);
    await user.hover(screen.getByText("今日时间带"));
    // 离开日期签容器 → 清除（onMouseLeave）
    expect(document.querySelectorAll(".recharts-reference-line")).toHaveLength(0);
    const bar0 = screen.getByTestId("trend-bar-0") as HTMLElement;
    expect(bar0.style.opacity).toBe("1");
  });

  it("键盘可达：focus 日期签同样联动（a11y 等价交互面）", () => {
    renderHome();
    fireEvent.focus(screen.getByTestId("trend-chip-4"));
    expect(document.querySelectorAll(".recharts-reference-line")).toHaveLength(2);
    expect(screen.getByTestId("trend-chip-4")).toHaveAttribute("data-active");
  });
});
