// 子 agent 条带在窄栏下的分块契约：标题守住首行、进度段整块换行、停止键钉在首行。
//
// 回归背景：中栏最窄时整行内容硬塞一行——标题被挤成 0 宽、进度溢出卡片、
// 停止键压在时长上。jsdom 不排版，这里钉住「能按块换行」的结构。

import { render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SubagentStrip } from "./SubagentStrip";
import { FIRST_LINE_SLOT } from "./SubagentRowLine";
import type { SubagentRun } from "./ContextRail";

const T0 = new Date("2026-11-09T10:00:00Z").getTime();
const TITLE = "调研上海出发的三天旅行候选目的地";

function run(over: Partial<SubagentRun> = {}): SubagentRun {
  return {
    id: "sa_1",
    title: TITLE,
    status: "running",
    running: true,
    roundsUsed: 30,
    toolCallsUsed: 62,
    startedAt: T0 - 293_000,
    ...over,
  };
}

function renderStrip(runs: SubagentRun[]) {
  render(
    <SubagentStrip
      subagents={runs}
      onOpen={vi.fn()}
      onCancel={vi.fn()}
      expanded
      onToggleExpanded={vi.fn()}
    />,
  );
}

describe("子 agent 条带：窄栏按块换行", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(T0);
  });
  afterEach(() => vi.useRealTimers());

  it("标题单独成块（长了截断、全文在悬停提示），进度段是同一可换行容器里的另一块", () => {
    renderStrip([run()]);
    const title = screen.getByText(TITLE);
    expect(title.className).toContain("truncate");
    expect(screen.getByTitle(TITLE).contains(title)).toBe(true);
    const progress = screen.getByText("30 轮").parentElement as HTMLElement;
    expect(progress.contains(title)).toBe(false);
    expect(progress.parentElement).toBe(title.parentElement);
    expect(title.parentElement?.className).toContain("flex-wrap");
    expect(progress.className).toContain("flex-wrap");
  });

  it("时长与「详情 ›」绑成一块：折行时一起走，「详情 ›」不单独落成一行", () => {
    renderStrip([run()]);
    expect(screen.getByText("4m 53s").parentElement).toBe(
      screen.getByText("详情 ›").parentElement,
    );
  });

  it("停止键在「看详情」按钮之外，放在与标题那行等高的槽里（行声明了对应行高）", () => {
    renderStrip([run()]);
    const stop = screen.getByRole("button", { name: `取消子 agent：${TITLE}` });
    const open = screen.getByTitle(TITLE);
    expect(open.contains(stop)).toBe(false);
    expect(stop.parentElement).toBe(open.parentElement);
    expect(stop.className).toContain(FIRST_LINE_SLOT);
    // 槽高 1.625em 依赖行上的 leading-relaxed：两者分家，停止键就对不齐标题行
    expect(stop.parentElement?.className).toContain("leading-relaxed");
  });

  it("标题行：「子 Agent」与计数各自不断行，放不下时计数整块折到下一行", () => {
    renderStrip([run()]);
    const label = screen.getByText("子 Agent");
    const count = screen.getByText("1 个执行中");
    expect(label.className).toContain("whitespace-nowrap");
    expect(count.className).toContain("whitespace-nowrap");
    expect(count.parentElement).toBe(label.parentElement);
    expect(label.parentElement?.className).toContain("flex-wrap");
  });
});
