import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, act, fireEvent } from "@testing-library/react";
import { SkillsPanel } from "./SkillsPanel";

const mocks = vi.hoisted(() => ({
  invokeMock: vi.fn(async (cmd: string) => {
    if (cmd === "skills_list") return [];
    if (cmd === "skills_import") return "测试技能";
    return null;
  }),
  openMock: vi.fn(async () => "/tmp/some-skill"),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invokeMock }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.openMock }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openPath: vi.fn(async () => {}) }));

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
      fireEvent.click(screen.getByText("⬆ 导入技能文件夹"));
      await flush();
      expect(screen.getByText(/已安装/)).toBeInTheDocument();
      // 2.5s 后二次导入 → 计时器重置
      act(() => {
        vi.advanceTimersByTime(2500);
      });
      expect(screen.getByText(/已安装/)).toBeInTheDocument();
      fireEvent.click(screen.getByText("⬆ 导入技能文件夹"));
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
      fireEvent.click(screen.getByText("⬆ 导入技能文件夹"));
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
