// 子 agent 只读详情的渲染契约：内容取全、时间线正确、以及 JSON 兜底不崩。

import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SubagentDetailPanel } from "./SubagentDetailPanel";
import type { SubagentRun } from "./ContextRail";

function run(over: Partial<SubagentRun> = {}): SubagentRun {
  return {
    id: "sa_1",
    title: "调研 ZCode turn 状态机",
    status: "succeeded",
    running: false,
    profile: "research",
    ...over,
  };
}

describe("SubagentDetailPanel 子 agent 只读详情", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("展示完整目标（不截断）——列表截断正是为了点进来能看全文", () => {
    const long = "调研 ".repeat(60).trim();
    render(<SubagentDetailPanel run={run({ title: long })} onBack={() => {}} />);
    expect(screen.getByText(long)).toBeTruthy();
  });

  it("结构化结果优先摊开 summary，而不是甩一堆花括号", () => {
    render(
      <SubagentDetailPanel
        run={run({ result: { summary: "状态机在 turn-state.ts 与 turn-machine.ts" } })}
        onBack={() => {}}
      />,
    );
    expect(screen.getByText("状态机在 turn-state.ts 与 turn-machine.ts")).toBeTruthy();
  });

  it("结果里混着别的字段时保留全文，不丢字段", () => {
    render(
      <SubagentDetailPanel
        run={run({ result: { summary: "一句话结论", artifacts: ["a.md"] } })}
        onBack={() => {}}
      />,
    );
    expect(screen.getByText(/一句话结论/)).toBeTruthy();
    expect(screen.getByText(/artifacts/)).toBeTruthy();
  });

  it("解析失败的原文照样显示，不吞字段", () => {
    render(
      <SubagentDetailPanel run={run({ result: "{不是合法 JSON" })} onBack={() => {}} />,
    );
    expect(screen.getByText("{不是合法 JSON")).toBeTruthy();
  });

  it("验收标准逐条编号", () => {
    render(
      <SubagentDetailPanel
        run={run({ acceptance: ["给出定位依据", "标注推断项"] })}
        onBack={() => {}}
      />,
    );
    expect(screen.getByText(/1\. 给出定位依据/)).toBeTruthy();
    expect(screen.getByText(/2\. 标注推断项/)).toBeTruthy();
  });

  it("终态用时定死在 finishedAt-startedAt，并显示起止时刻与 trace", () => {
    const t0 = Date.now();
    vi.setSystemTime(t0);
    render(
      <SubagentDetailPanel
        run={run({ startedAt: t0 - 148_001, finishedAt: t0, traceId: "trc_abc" })}
        onBack={() => {}}
      />,
    );
    expect(screen.getByText("2m 28s")).toBeTruthy();
    expect(screen.getByText(/trc_abc/)).toBeTruthy();
    act(() => {
      vi.advanceTimersByTime(10_000);
    });
    expect(screen.getByText("2m 28s"), "终态用时不该继续跳").toBeTruthy();
  });

  it("在途详情的用时仍每秒刷新（详情页也要是活的）", () => {
    const t0 = Date.now();
    vi.setSystemTime(t0);
    render(
      <SubagentDetailPanel
        run={run({ status: "running", running: true, startedAt: t0, finishedAt: null })}
        onBack={() => {}}
      />,
    );
    expect(screen.getByText("0s")).toBeTruthy();
    act(() => {
      vi.advanceTimersByTime(2000);
    });
    expect(screen.getByText("2s")).toBeTruthy();
  });

  it("返回按钮回列表页", () => {
    const onBack = vi.fn();
    render(<SubagentDetailPanel run={run()} onBack={onBack} />);
    act(() => {
      screen.getByLabelText("返回子 agent 列表").click();
    });
    expect(onBack).toHaveBeenCalledTimes(1);
  });
});
