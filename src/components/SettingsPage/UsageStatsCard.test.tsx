// UsageStatsCard 测试（P3-b）：mock usage_stats_daily，覆盖汇总/空态/条形渲染。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi, beforeEach } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import { UsageStatsCard, type UsageDay } from "./UsageStatsCard";

const DAYS: UsageDay[] = [
  { day: "2026-10-05", promptTokens: 900, completionTokens: 100, runs: 2, toolCalls: 5 },
  { day: "2026-10-06", promptTokens: 4000, completionTokens: 1000, runs: 3, toolCalls: 8 },
];

describe("UsageStatsCard", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "usage_stats_daily") return Promise.resolve(DAYS);
      return Promise.reject(new Error(`unexpected command: ${cmd}`));
    });
  });

  it("汇总行聚合全期 tokens/执行/工具数，按日行渲染", async () => {
    render(<UsageStatsCard />);
    await waitFor(() => expect(screen.getByText("4.9K")).toBeTruthy()); // 输入 900+4000
    expect(screen.getByText("1.1K")).toBeTruthy(); // 输出 100+1000
    expect(screen.getByText("5")).toBeTruthy(); // 执行次数 2+3
    expect(screen.getByText("13")).toBeTruthy(); // 工具调用 5+8
    expect(screen.getByText("10-06")).toBeTruthy(); // 按日行
    expect(screen.getByText("5.0K · 3次")).toBeTruthy(); // 当日总量与执行次数
  });

  it("无数据时空态文案", async () => {
    invokeMock.mockImplementation((cmd: string) =>
      cmd === "usage_stats_daily" ? Promise.resolve([]) : Promise.reject(new Error("x"))
    );
    render(<UsageStatsCard />);
    await waitFor(() => expect(screen.getByText(/还没有执行记录/)).toBeTruthy());
  });

  it("刷新按钮重拉数据", async () => {
    render(<UsageStatsCard />);
    await waitFor(() => expect(screen.getByText("刷新")).toBeTruthy());
    fireEvent.click(screen.getByText("刷新"));
    await waitFor(() =>
      expect(invokeMock.mock.calls.filter((c) => c[0] === "usage_stats_daily").length).toBe(2)
    );
  });
});
