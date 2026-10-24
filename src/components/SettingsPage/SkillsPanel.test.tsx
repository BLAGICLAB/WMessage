import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, act, fireEvent } from "@testing-library/react";
import { SkillsPanel } from "./SkillsPanel";

const mocks = vi.hoisted(() => ({
  // 返回值统一收成 unknown：各用例按需 mock 出不同形状的列表
  invokeMock: vi.fn(async (cmd: string): Promise<unknown> => {
    if (cmd === "skills_list") return [];
    if (cmd === "skills_import") return "测试技能";
    return null;
  }),
  openMock: vi.fn(async () => "/tmp/some-skill"),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invokeMock }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.openMock }));

beforeEach(() => {
  mocks.invokeMock.mockClear();
  mocks.openMock.mockClear();
});

async function flush() {
  await act(async () => {});
}

describe("SkillsPanel notice 计时器生命周期", () => {
  it("导入成功 → notice 出现；3s 后消退；3s 内二次导入计时重置", async () => {
    render(<SkillsPanel />);
    await flush(); // refresh 初始加载
    vi.useFakeTimers();
    try {
      fireEvent.click(screen.getByText("导入技能文件夹"));
      await flush();
      expect(screen.getByText(/已安装/)).toBeInTheDocument();
      // 2.5s 后二次导入 → 计时器重置
      act(() => {
        vi.advanceTimersByTime(2500);
      });
      expect(screen.getByText(/已安装/)).toBeInTheDocument();
      fireEvent.click(screen.getByText("导入技能文件夹"));
      await flush();
      // 距第二次导入 2.5s（若旧计时器没清，此刻已被误清）
      act(() => {
        vi.advanceTimersByTime(2500);
      });
      expect(screen.getByText(/已安装/)).toBeInTheDocument();
      act(() => {
        vi.advanceTimersByTime(600);
      });
      expect(screen.queryByText(/已安装/)).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("卸载后计时器清理（无残留）", async () => {
    const { unmount } = render(<SkillsPanel />);
    await flush();
    vi.useFakeTimers();
    try {
      fireEvent.click(screen.getByText("导入技能文件夹"));
      await flush();
      expect(screen.getByText(/已安装/)).toBeInTheDocument();
      unmount();
      act(() => {
        vi.advanceTimersByTime(5000);
      });
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });
});

const SAMPLE_SKILLS = [
  {
    name: "sample-skill",
    description: "PPT 设计规范",
    version: "1.0.1",
    enabled: true,
  },
  {
    name: "off-skill",
    description: "已关掉的技能",
    enabled: false,
  },
];

describe("SkillsPanel 启用开关", () => {
  it("启停：拨杆按 name 调 skills_set_enabled", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "skills_list" ? SAMPLE_SKILLS : null,
    );
    render(<SkillsPanel />);
    await flush();
    const switches = screen.getAllByRole("switch") as HTMLElement[];
    expect(switches[0].getAttribute("aria-checked")).toBe("true");
    expect(switches[1].getAttribute("aria-checked")).toBe("false");
    fireEvent.click(switches[0]);
    await flush();
    expect(mocks.invokeMock).toHaveBeenCalledWith("skills_set_enabled", {
      name: "sample-skill",
      enabled: false,
    });
    // 关掉已关的 → 重新启用
    fireEvent.click(switches[1]);
    await flush();
    expect(mocks.invokeMock).toHaveBeenCalledWith("skills_set_enabled", {
      name: "off-skill",
      enabled: true,
    });
  });

  it("拨杆形态：解释文字走悬停 tooltip，按钮后不跟文字", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "skills_list" ? SAMPLE_SKILLS : null,
    );
    render(<SkillsPanel />);
    await flush();
    const sw = screen.getAllByRole("switch")[0] as HTMLElement;
    expect(sw.tagName).toBe("BUTTON");
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    const tip = sw.getAttribute("title") ?? "";
    expect(tip).toMatch(/进机器人清单|看不到也调不到/);
    // 开关本体不承载可见文字
    expect(sw.textContent).toBe("");
  });

  it("后端没下发 enabled 字段 → 视为启用（缺省不误关）", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "skills_list" ? [{ name: "legacy", description: "旧数据" }] : null,
    );
    render(<SkillsPanel />);
    await flush();
    const sw = screen.getAllByRole("switch")[0];
    expect(sw.getAttribute("aria-checked")).toBe("true");
  });

  it("关闭态：技能名置灰（视觉上与启用态可区分）", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) =>
      cmd === "skills_list" ? SAMPLE_SKILLS : null,
    );
    render(<SkillsPanel />);
    await flush();
    const on = screen.getByText("sample-skill");
    const off = screen.getByText("off-skill");
    expect(on.className).toContain("text-[var(--t2)]");
    expect(off.className).toContain("text-[var(--t5)]");
  });
});
