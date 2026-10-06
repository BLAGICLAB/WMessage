// UsageStatsCard 测试：mock usage_stats_daily / usage_stats_by_model /
// usage_stats_daily_by_model，覆盖汇总行（含连续天数）/热力图（模式切换、月份标签）/
// 时间范围切换/按模型趋势图/模型用量/空态/刷新。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi, beforeEach } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import {
  UsageStatsCard,
  streakDays,
  buildTrendSeries,
  type UsageDay,
  type UsageModelRow,
  type UsageDayModelRow,
} from "./UsageStatsCard";

// 近 7 天窗口：开头 09-30~10-02 连续活跃 3 天（最长），尾部 10-05~06 连续 2 天（当前）
const DAYS: UsageDay[] = [
  { day: "2026-09-30", promptTokens: 500, completionTokens: 500, runs: 1, toolCalls: 1 },
  { day: "2026-10-01", promptTokens: 500, completionTokens: 500, runs: 1, toolCalls: 1 },
  { day: "2026-10-02", promptTokens: 500, completionTokens: 500, runs: 1, toolCalls: 1 },
  { day: "2026-10-03", promptTokens: 0, completionTokens: 0, runs: 0, toolCalls: 0 },
  { day: "2026-10-04", promptTokens: 0, completionTokens: 0, runs: 0, toolCalls: 0 },
  { day: "2026-10-05", promptTokens: 900, completionTokens: 100, runs: 2, toolCalls: 5 },
  { day: "2026-10-06", promptTokens: 4000, completionTokens: 1000, runs: 3, toolCalls: 8 },
];

const MODELS: UsageModelRow[] = [
  { model: "glm-5.3", promptTokens: 4900, completionTokens: 1100, runs: 5 },
  { model: null, promptTokens: 0, completionTokens: 0, runs: 0 },
];

const MODEL_DAYS: UsageDayModelRow[] = [
  { day: "2026-10-05", model: "glm-5.3", promptTokens: 900, completionTokens: 100 },
  { day: "2026-10-06", model: "glm-5.3", promptTokens: 4000, completionTokens: 1000 },
  { day: "2026-10-05", model: "kimi-k3", promptTokens: 50, completionTokens: 50 },
];

function mockOk() {
  invokeMock.mockImplementation((cmd: string) => {
    if (cmd === "usage_stats_daily") return Promise.resolve(DAYS);
    if (cmd === "usage_stats_by_model") return Promise.resolve(MODELS);
    if (cmd === "usage_stats_daily_by_model") return Promise.resolve(MODEL_DAYS);
    return Promise.reject(new Error(`unexpected command: ${cmd}`));
  });
}

describe("UsageStatsCard", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    mockOk();
  });

  it("汇总行聚合累计/峰值/执行/连续天数，热力图与按模型趋势图渲染", async () => {
    const { container } = render(<UsageStatsCard />);
    await waitFor(() => expect(screen.getByText("9.0K")).toBeTruthy()); // 累计 (500+500)×3+1000+5000
    expect(screen.getByText("5.0K")).toBeTruthy(); // 峰值日（10-06 = 5000）
    expect(screen.getByText("8")).toBeTruthy(); // 执行次数 1+1+1+2+3
    expect(screen.getByText("2 天")).toBeTruthy(); // 当前连续（10-05~06）
    expect(screen.getByText("3 天")).toBeTruthy(); // 最长连续（09-30~10-02 游程为 3）
    expect(screen.getByText("累计 Token 数")).toBeTruthy();
    expect(screen.getByText("最长连续天数")).toBeTruthy();
    // 热力图：活跃日的格子都在（title 即 tooltip）
    expect(screen.getByTitle(/2026-10-06 · 5.0K tokens/)).toBeTruthy();
    // 热力图底部月份标签：7 天窗口跨 9/30~10/06，仅月首 9月 获得标签（10月 距离过近被跳过）
    expect(screen.getByText("9月")).toBeTruthy();
    // 趋势图 SVG + 图例按模型命名（窗口内两个模型）
    expect(container.querySelector("svg[aria-label=\"每日 Token 趋势图\"]")).toBeTruthy();
    expect(screen.getAllByText("glm-5.3").length).toBeGreaterThanOrEqual(1); // 图例 + 模型用量榜
    expect(screen.getByText("kimi-k3")).toBeTruthy();
    // 模型用量榜
    expect(screen.getByText("未知模型（旧数据）")).toBeTruthy();
  });

  it("热力图 每日/每周/累计 切换选中态互换", async () => {
    render(<UsageStatsCard />);
    await waitFor(() => expect(screen.getByText("9.0K")).toBeTruthy());
    expect(screen.getByText("每日").className).toContain("nm-inset");
    fireEvent.click(screen.getByText("累计"));
    await waitFor(() => expect(screen.getByText("累计").className).toContain("nm-inset"));
    expect(screen.getByText("每日").className).toContain("nm-outset");
  });

  it("时间范围切换：近7日/近30日选中态互换，并按新窗口重拉按模型序列", async () => {
    render(<UsageStatsCard />);
    await waitFor(() => expect(screen.getByText("9.0K")).toBeTruthy());
    expect(screen.getByText("近30日").className).toContain("nm-inset");
    fireEvent.click(screen.getByText("近7日"));
    await waitFor(() => expect(screen.getByText("近7日").className).toContain("nm-inset"));
    expect(screen.getByText("近30日").className).toContain("nm-outset");
    await waitFor(() => {
      const calls = invokeMock.mock.calls.filter((c) => c[0] === "usage_stats_daily_by_model");
      expect(calls.some((c) => c[1]?.days === 7)).toBe(true);
    });
  });

  it("无数据时空态文案", async () => {
    invokeMock.mockImplementation((cmd: string) =>
      cmd === "usage_stats_daily"
        ? Promise.resolve([])
        : cmd === "usage_stats_by_model"
          ? Promise.resolve([])
          : Promise.resolve([]),
    );
    render(<UsageStatsCard />);
    await waitFor(() => expect(screen.getByText(/还没有词元用量/)).toBeTruthy());
  });

  it("刷新按钮重拉全部三个统计命令", async () => {
    render(<UsageStatsCard />);
    await waitFor(() => expect(screen.getByText("刷新")).toBeTruthy());
    fireEvent.click(screen.getByText("刷新"));
    await waitFor(() => {
      expect(invokeMock.mock.calls.filter((c) => c[0] === "usage_stats_daily").length).toBe(2);
      expect(invokeMock.mock.calls.filter((c) => c[0] === "usage_stats_by_model").length).toBe(2);
      expect(invokeMock.mock.calls.filter((c) => c[0] === "usage_stats_daily_by_model").length).toBe(2);
    });
  });

  it("streakDays：尾部空档从昨天起算，最长游程取窗口最大", () => {
    const mk = (day: string, tokens: number): UsageDay => ({
      day,
      promptTokens: tokens,
      completionTokens: 0,
      runs: 0,
      toolCalls: 0,
    });
    const days = [
      mk("2026-10-01", 10),
      mk("2026-10-02", 10),
      mk("2026-10-03", 0),
      mk("2026-10-04", 10),
      mk("2026-10-05", 10),
      mk("2026-10-06", 0), // 今天不活跃
    ];
    expect(streakDays(days)).toEqual([2, 2]);
  });

  it("buildTrendSeries：缺日记 0、按 tokens 降序取前 N、NULL 模型显示未知", () => {
    const rows: UsageDayModelRow[] = [
      { day: "2026-10-06", model: "a-model", promptTokens: 300, completionTokens: 0 },
      { day: "2026-10-06", model: null, promptTokens: 100, completionTokens: 0 },
    ];
    const series = buildTrendSeries(DAYS, rows, 5);
    expect(series.length).toBe(2);
    expect(series[0].name).toBe("a-model"); // 300 > 100 降序在前
    expect(series[0].values[6]).toBe(300);
    expect(series[0].values[5]).toBe(0); // 缺日记 0
    expect(series[1].name).toBe("未知模型");
    expect(series[1].color).not.toBe(series[0].color);
  });
});
