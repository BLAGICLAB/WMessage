import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, act } from "@testing-library/react";
import { LiveIndicator } from "./LiveIndicator";
import { fmtElapsed } from "./useElapsed";
import type { Msg } from "./types";

// 活体执行指示的渲染与计时。
//
// 回归背景：原先 header 只有一个 animate-pulse 小圆点 + 固定文案「正在回复」，
// 由 viewedBusy 驱动、整个 turn 恒亮——300s 的 pip install 与 0.1s 的
// list_files 视觉上无差别，用户看不出 agent 是否还在跑。

describe("LiveIndicator", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-10-17T10:00:00Z"));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("running 时显示文案并按秒刷新计时", () => {
    const start = Date.now();
    render(<LiveIndicator status="running" detail="正在思考 · 第 1 轮" startedAt={start} />);
    expect(screen.getByText("正在思考 · 第 1 轮")).toBeTruthy();
    // 起步时不显示耗时（0s 无信息量），过 3 秒应出现
    expect(screen.queryByText("3s")).toBeNull();
    act(() => {
      vi.advanceTimersByTime(3000);
    });
    expect(screen.getByText("3s")).toBeTruthy();
  });

  it("超过 60 秒换成 m 分秒格式", () => {
    render(<LiveIndicator status="running" detail="正在思考" startedAt={Date.now()} />);
    act(() => {
      vi.advanceTimersByTime(65_000);
    });
    expect(screen.getByText("1m 05s")).toBeTruthy();
  });

  it("settling（正文说完但还在收尾）必须仍显示活体指示与计时", () => {
    // 这是 ZCode settling 状态存在的理由：正文已出、自检还在跑，
    // 此时若不显示，用户看到的就是「说完就停」的假静止。
    render(<LiveIndicator status="settling" detail="收尾中 · 校验答复" startedAt={Date.now()} />);
    expect(screen.getByText("收尾中 · 校验答复")).toBeTruthy();
    act(() => {
      vi.advanceTimersByTime(8000);
    });
    expect(screen.getByText("8s")).toBeTruthy();
  });

  it("终态不再计时（秒数停住）", () => {
    render(<LiveIndicator status="completed" detail="x" startedAt={Date.now()} />);
    expect(screen.queryByText(/s$/)).toBeNull();
  });

  it("无起点时只显示文案不显示秒数", () => {
    render(<LiveIndicator status="running" detail="正在思考" />);
    act(() => {
      vi.advanceTimersByTime(5000);
    });
    expect(screen.queryByText("5s")).toBeNull();
  });
});

describe("useElapsed", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-10-17T10:00:00Z"));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("fmtElapsed 分档", () => {
    expect(fmtElapsed(0)).toBe("0s");
    expect(fmtElapsed(59)).toBe("59s");
    expect(fmtElapsed(60)).toBe("1m 00s");
    expect(fmtElapsed(3661)).toBe("61m 01s");
  });
});

describe("工具徽章的未完成态", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-10-17T10:00:00Z"));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("未完成工具显示秒级计时而非静态省略号", async () => {
    const { MessageList } = await import("./MessageList");
    const msg: Msg = {
      role: "assistant",
      content: "正在装依赖",
      streaming: true,
      tools: [{ id: "t1", name: "run_python", startedAt: Date.now() }],
    };
    render(
      <MessageList
        messages={[msg]}
        verboseLevel="concise"
        scrollRef={{ current: null }}
        viewedBusy={false}
        copiedIdx={null}
        onPickStarter={() => {}}
        onCopy={() => {}}
        onRemove={() => {}}
        onOpenTask={() => {}}
        onOpenExecSession={() => {}}
      />,
    );
    expect(screen.getByText("run_python")).toBeTruthy();
    // 静态「…」不该再出现——它与「已完成」在视觉上无法区分
    expect(screen.queryByText("…")).toBeNull();
    act(() => {
      vi.advanceTimersByTime(4200);
    });
    expect(screen.getByText("4s")).toBeTruthy();
  });
});
