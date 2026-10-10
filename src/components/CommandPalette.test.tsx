import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { CommandPalette } from "./CommandPalette";
import type { Task } from "../types";

// vi.mock 工厂会被提升到顶部，共享 mock 用 vi.hoisted（App.test.tsx 同款模式）
const mocks = vi.hoisted(() => {
  const invokeMock = vi.fn();
  const emitMock = vi.fn();
  return { invokeMock, emitMock };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invokeMock,
}));
vi.mock("@tauri-apps/api/event", () => ({
  emit: mocks.emitMock,
  listen: vi.fn(async () => () => {}),
}));

const TASKS: Task[] = [
  { id: "t1", title: "梳理 WMessage 需求清单", column: "todo" },
  { id: "t2", title: "过一遍扁平分层样式", column: "doing" },
  { id: "t3", title: "完成命令面板原型", column: "done" },
  { id: "t4", title: "已归档任务", column: "done", archived: true },
  { id: "t5", title: "已删除任务", column: "todo", deletedAt: 123 },
];

const SESSIONS = [
  { id: "s1", title: "机器人闲聊" },
  { id: "s2", title: "重构讨论" },
];

// 面板无 open prop：由使用方条件挂载（{open && <CommandPalette/>}），测试同构
function renderPalette(
  overrides: Partial<Parameters<typeof CommandPalette>[0]> = {},
) {
  const onClose = vi.fn();
  const onJumpTask = vi.fn();
  const onJumpSession = vi.fn();
  const result = render(
    <CommandPalette
      onClose={onClose}
      tasks={TASKS}
      onJumpTask={onJumpTask}
      onJumpSession={onJumpSession}
      {...overrides}
    />,
  );
  return { onClose, onJumpTask, onJumpSession, unmount: result.unmount };
}

beforeEach(() => {
  mocks.invokeMock.mockReset();
  mocks.invokeMock.mockImplementation(async (cmd: string) =>
    cmd === "bot_sessions_load" ? SESSIONS : null,
  );
  mocks.emitMock.mockReset();
  mocks.emitMock.mockImplementation(async () => {});
});

describe("CommandPalette", () => {
  it("挂载后渲染任务/会话分组；卸载即消失", async () => {
    const { unmount } = renderPalette();
    // 会话列表为挂载时异步加载（bot_sessions_load），分组头随之出现
    expect(await screen.findByText("机器人闲聊")).toBeInTheDocument();
    expect(screen.getByText("任务")).toBeInTheDocument();
    expect(screen.getByText("会话")).toBeInTheDocument();
    expect(screen.getByText("重构讨论")).toBeInTheDocument();
    unmount();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("输入过滤：只显示匹配的任务/会话；无结果显示空态", async () => {
    const user = userEvent.setup();
    renderPalette();
    await screen.findByText("机器人闲聊");
    const input = screen.getByLabelText("搜索关键词");
    await user.type(input, "需求");
    expect(screen.getByText("梳理 WMessage 需求清单")).toBeInTheDocument();
    expect(screen.queryByText("会话")).not.toBeInTheDocument();
    await user.clear(input);
    await user.type(input, "不存在的关键词xyz");
    expect(screen.getByText("没有匹配的任务或会话")).toBeInTheDocument();
  });

  it("回车跳转任务：按所在位置带出 onJumpTask + onClose", async () => {
    const user = userEvent.setup();
    const { onJumpTask, onClose } = renderPalette();
    const input = screen.getByLabelText("搜索关键词");
    await user.type(input, "需求");
    await user.keyboard("{Enter}");
    expect(onJumpTask).toHaveBeenCalledWith(
      expect.objectContaining({ id: "t1" }),
    );
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("键盘上下选择 + 回车跳会话：onJumpSession 带会话 id（emit 由 App 层发）", async () => {
    const user = userEvent.setup();
    const { onJumpSession } = renderPalette();
    await screen.findByText("机器人闲聊");
    // 5 个任务行（含归档/回收站）后会话行为第 6 项（index 5）；连按 5 次下键
    await user.keyboard(
      "{ArrowDown}{ArrowDown}{ArrowDown}{ArrowDown}{ArrowDown}",
    );
    await user.keyboard("{Enter}");
    expect(onJumpSession).toHaveBeenCalledWith("s1");
  });

  it("Esc 关闭；点击遮罩关闭", async () => {
    const user = userEvent.setup();
    const { onClose } = renderPalette();
    await screen.findByText("机器人闲聊");
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(1);
    // 遮罩 = dialog 的父层（fixed inset-0）
    await user.click(document.querySelector(".fixed.inset-0") as HTMLElement);
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it("任务行带所在位置徽标（待办/今日/完成/归档/回收站）", async () => {
    renderPalette();
    expect(await screen.findByText("待办")).toBeInTheDocument();
    expect(screen.getByText("今日")).toBeInTheDocument();
    expect(screen.getByText("完成")).toBeInTheDocument();
    expect(screen.getByText("归档")).toBeInTheDocument();
    expect(screen.getByText("回收站")).toBeInTheDocument();
  });
});
