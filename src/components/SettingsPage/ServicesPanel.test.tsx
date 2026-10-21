import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, act, fireEvent } from "@testing-library/react";
import { ServicesPanel } from "./ServicesPanel";

/** 一个运行中的服务 + 一个已退出的服务 */
const SNAPSHOT = {
  services: [
    {
      serviceId: "svc_aaa11111",
      name: "前端 dev server",
      command: "npx",
      args: ["vite"],
      cwd: "/tmp/web",
      pid: 4321,
      state: "running" as const,
      startedAt: 1,
      uptimeSecs: 125,
      exitCode: null,
      sessionId: null,
      rssMb: 145.2,
      cpuPct: 12.5,
      warnings: [],
      stdoutTail: ["VITE v7.3.6 ready in 312 ms", "Local: http://127.0.0.1:5173/"],
    },
    {
      serviceId: "svc_bbb22222",
      name: "",
      command: "python3",
      args: ["-c", "pass"],
      cwd: "/tmp",
      pid: 4322,
      state: "exited" as const,
      startedAt: 1,
      uptimeSecs: 3,
      exitCode: 0,
      sessionId: null,
      warnings: ["rss_over_limit"],
      stdoutTail: [],
    },
  ],
  totalRssMb: 145.2,
  warnings: [],
  sampledAt: 1000,
};

const mocks = vi.hoisted(() => ({
  // 返回类型放宽成 unknown：各用例的 mockImplementation 形状不同
  // （空列表 / 带告警 / 抛错），写死窄类型会让后写的用例类型不匹配
  invokeMock: vi.fn(
    async (cmd: string, args?: Record<string, unknown>): Promise<unknown> => {
      if (cmd === "services_list") return structuredClone(SNAPSHOT);
      if (cmd === "services_stop")
        return { serviceId: args?.serviceId, stopped: true };
      return null;
    },
  ),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invokeMock }));

beforeEach(() => {
  mocks.invokeMock.mockClear();
});

async function flush() {
  await act(async () => {});
}

describe("ServicesPanel", () => {
  it("列出服务：状态 / pid / 运行时长 / 资源占用都看得见", async () => {
    render(<ServicesPanel />);
    await flush();
    expect(screen.getByText("受管服务")).toBeInTheDocument();
    expect(screen.getByText("运行中")).toBeInTheDocument();
    expect(screen.getByText("前端 dev server")).toBeInTheDocument();
    expect(screen.getByText("pid 4321")).toBeInTheDocument();
    expect(screen.getByText("2 分 5 秒")).toBeInTheDocument();
    expect(screen.getByText("内存 145 MB")).toBeInTheDocument();
    expect(screen.getByText("CPU 13%")).toBeInTheDocument();
    // 已退出的服务不显示停止按钮，但保留历史
    expect(screen.getByText("已退出")).toBeInTheDocument();
    expect(screen.getByText("退出码 0")).toBeInTheDocument();
  });

  it("合计口径与告警显示在顶部", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "services_list") {
        return {
          ...structuredClone(SNAPSHOT),
          warnings: ["total_rss_over_limit"],
          totalRssMb: 1200,
        };
      }
      return null;
    });
    render(<ServicesPanel />);
    await flush();
    expect(screen.getByText(/合计内存 1200 MB/)).toBeInTheDocument();
    expect(screen.getAllByText(/全部服务合计内存超阈值/).length).toBeGreaterThan(0);
  });

  it("点停止 → 调 services_stop 并带 serviceId", async () => {
    render(<ServicesPanel />);
    await flush();
    fireEvent.click(screen.getByLabelText("停止服务 前端 dev server"));
    await flush();
    const stopCall = mocks.invokeMock.mock.calls.find(
      (c) => c[0] === "services_stop",
    );
    expect(stopCall).toBeTruthy();
    expect(stopCall?.[1]).toEqual({ serviceId: "svc_aaa11111" });
  });

  it("空态：没有服务时给引导", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "services_list") {
        return { services: [], totalRssMb: 0, warnings: [], sampledAt: null };
      }
      return null;
    });
    render(<ServicesPanel />);
    await flush();
    expect(screen.getByText("暂无受管服务")).toBeInTheDocument();
  });

  it("列表接口报错 → 面板显示错误信息", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "services_list") throw new Error("boom");
      return null;
    });
    render(<ServicesPanel />);
    await flush();
    expect(screen.getByText(/boom/)).toBeInTheDocument();
  });

  it("定时刷新（5s 一轮）且卸载后不留计时器", async () => {
    // 假定时器必须在 render 之前装：定时器是挂载时创建的，
    // 之后再装 fake timers 管不到已经建好的那个
    vi.useFakeTimers();
    try {
      const { unmount } = render(<ServicesPanel />);
      await flush();
      const before = mocks.invokeMock.mock.calls.length;
      await act(async () => {
        vi.advanceTimersByTime(5000);
      });
      expect(mocks.invokeMock.mock.calls.length).toBeGreaterThan(before);
      unmount();
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });
});
