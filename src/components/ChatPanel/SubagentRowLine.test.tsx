// 子 agent 单行摘要的渲染契约：轮数口径、时长刷新与停表。

import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SubagentProgress, SubagentRowLine } from "./SubagentRowLine";
import type { SubagentRun } from "./ContextRail";

function run(over: Partial<SubagentRun> = {}): SubagentRun {
  return {
    id: "sa_1",
    title: "调研 ZCode 流式模式",
    status: "running",
    running: true,
    ...over,
  };
}

describe("SubagentRowLine 子 agent 单行摘要", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("在途行显示「已跑/预算 轮」与运行时长", () => {
    const t0 = Date.now();
    vi.setSystemTime(t0);
    render(<SubagentRowLine run={run({ startedAt: t0 - 4000 })} />);
    expect(screen.getByText("调研 ZCode 流式模式")).toBeTruthy();
    // 没有轮数数据时不凭空补分母
    expect(screen.queryByText(/\d+\/\d+ 轮/)).toBeNull();
    expect(screen.getByText("4s")).toBeTruthy();
  });

  it("只报已跑数：分母与主会话共用安全网，子 agent 行不单报", () => {
    render(<SubagentProgress run={run({ roundsUsed: 10, toolCallsUsed: 100 })} />);
    expect(screen.getByText("10 轮")).toBeTruthy();
    expect(screen.getByText("100 次")).toBeTruthy();
    expect(screen.queryByText(/\d+\/\d+ 轮/)).toBeNull();
    expect(screen.queryByText(/\d+\/\d+ 次/)).toBeNull();
  });

  it("只有已跑工具数时只报工具数", () => {
    render(<SubagentProgress run={run({ toolCallsUsed: 7 })} />);
    expect(screen.getByText("7 次")).toBeTruthy();
  });

  it("只有已跑轮数时只报轮数", () => {
    render(<SubagentProgress run={run({ roundsUsed: 7 })} />);
    expect(screen.getByText("7 轮")).toBeTruthy();
    expect(screen.queryByText(/\d+\/\d+ 轮/)).toBeNull();
  });

  it("读不到仪表（终态）时整段轮数消失", () => {
    render(
      <SubagentProgress
        run={run({ status: "succeeded", running: false, roundsUsed: null })}
      />,
    );
    expect(screen.queryByText(/轮$/)).toBeNull();
  });

  it("在途时长每秒刷新", () => {
    const t0 = Date.now();
    vi.setSystemTime(t0);
    render(<SubagentProgress run={run({ startedAt: t0 })} />);
    expect(screen.getByText("0s")).toBeTruthy();
    act(() => {
      vi.advanceTimersByTime(3000);
    });
    expect(screen.getByText("3s")).toBeTruthy();
  });

  it("终态时长定死在 finishedAt-startedAt，不再跳秒", () => {
    const t0 = Date.now();
    vi.setSystemTime(t0);
    render(
      <SubagentProgress
        run={run({
          status: "succeeded",
          running: false,
          startedAt: t0 - 90_000,
          finishedAt: t0,
        })}
      />,
    );
    expect(screen.getByText("1m 30s")).toBeTruthy();
    act(() => {
      vi.advanceTimersByTime(5000);
    });
    expect(
      screen.getByText("1m 30s"),
      "终态还跳秒数等于让用户以为它还在跑",
    ).toBeTruthy();
  });

  it("排队期没有 startedAt，两端都没有就不显示时长", () => {
    render(<SubagentProgress run={run({ status: "queued", running: true })} />);
    expect(screen.queryByText(/\d+s$/)).toBeNull();
  });

  it("时长跨 60 秒自动分档", () => {
    const t0 = Date.now();
    vi.setSystemTime(t0);
    render(<SubagentProgress run={run({ startedAt: t0 - 125_000 })} />);
    expect(screen.getByText("2m 05s")).toBeTruthy();
  });
});
