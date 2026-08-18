import { describe, it, expect, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TodoCard, TodoCardView } from "./TodoCard";
import type { Task } from "../types";

// —— Tauri mocks ——
// 任务卡虽然本身不直接调 invoke（除复制/删绑定文件外），但子组件 ActorAvatar 会调用
// profile API + listen；mock 掉避免 Profile 初始化抛错。
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => null),
  convertFileSrc: vi.fn((p: string) => `tauri://localhost/${p}`),
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => null),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openPath: vi.fn(async () => {}),
}));

vi.mock("@tauri-apps/api/webviewWindow", () => ({
  WebviewWindow: { getByLabel: vi.fn(async () => null) },
}));

// profile 子模块：返回基本用户/机器人资料即可
vi.mock("../profile", () => ({
  getProfileCache: vi.fn(() => ({
    user: { name: "Test", avatarDataUrl: null },
    bot: { name: "Bot", avatarDataUrl: null },
  })),
  loadProfile: vi.fn(async () => ({
    user: { name: "Test", avatarDataUrl: null },
    bot: { name: "Bot", avatarDataUrl: null },
  })),
  subscribeProfile: vi.fn(() => () => {}),
}));

const baseTask: Task = {
  id: "t1",
  title: "默认任务",
  column: "todo",
  order: 0,
};

const dueTask: Task = {
  ...baseTask,
  id: "t2",
  title: "有截止时间",
  due: "2026-08-18T15:30",
};

const completedTask: Task = {
  ...baseTask,
  id: "t3",
  title: "已完成",
  column: "done",
  completedAt: Date.now(),
};

describe("TodoCardView", () => {
  it("默认渲染：标题、DoneCircle、FoldToggle 都可见", () => {
    render(
      <TodoCardView task={baseTask} onUpdate={vi.fn()} onDelete={vi.fn()} />
    );
    expect(screen.getByText("默认任务")).toBeInTheDocument();
    expect(screen.getByTitle("标记完成")).toBeInTheDocument();
    expect(screen.getByTitle("收起")).toBeInTheDocument();
  });

  it("点击标题进入编辑态：input 显示当前 title，按 Esc 取消恢复", async () => {
    const user = userEvent.setup();
    const onUpdate = vi.fn();
    render(<TodoCardView task={baseTask} onUpdate={onUpdate} onDelete={vi.fn()} />);
    await user.click(screen.getByText("默认任务"));
    const input = screen.getByDisplayValue("默认任务");
    expect(input).toBeInTheDocument();
    // Esc 取消：保留旧 title，关闭编辑态
    await user.keyboard("{Escape}");
    expect(screen.getByText("默认任务")).toBeInTheDocument();
    expect(onUpdate).not.toHaveBeenCalled();
  });

  it("编辑态输入新标题按 Enter 提交，trim 后非空才回调 onUpdate", async () => {
    const user = userEvent.setup();
    const onUpdate = vi.fn();
    render(<TodoCardView task={baseTask} onUpdate={onUpdate} onDelete={vi.fn()} />);
    await user.click(screen.getByText("默认任务"));
    const input = screen.getByDisplayValue("默认任务");
    await user.clear(input);
    await user.type(input, "  新标题  ");
    await user.keyboard("{Enter}");
    expect(onUpdate).toHaveBeenCalledWith("t1", { title: "新标题" });
  });

  it("点击 FoldToggle 触发折叠回调：传入 collapsed: true", async () => {
    const user = userEvent.setup();
    const onUpdate = vi.fn();
    render(<TodoCardView task={baseTask} onUpdate={onUpdate} onDelete={vi.fn()} />);
    await user.click(screen.getByTitle("收起"));
    expect(onUpdate).toHaveBeenCalledWith("t1", { collapsed: true });
  });

  it("点击 DoneCircle 触发完成回调：未完成 → column=done + completedAt + 清 botAssigned", async () => {
    const user = userEvent.setup();
    const onUpdate = vi.fn();
    const taskWithBot: Task = { ...baseTask, botAssigned: true };
    render(
      <TodoCardView
        task={taskWithBot}
        onUpdate={onUpdate}
        onDelete={vi.fn()}
      />
    );
    await user.click(screen.getByTitle("标记完成"));
    const call = onUpdate.mock.calls[0];
    expect(call[0]).toBe("t1");
    expect(call[1].column).toBe("done");
    expect(typeof call[1].completedAt).toBe("number");
    expect(call[1].botAssigned).toBeUndefined();
  });

  it("截止日期显示：formatDue 输出「截止 MM-DD HH:mm」", () => {
    render(<TodoCardView task={dueTask} onUpdate={vi.fn()} onDelete={vi.fn()} />);
    expect(screen.getByText(/截止 08-18 15:30/)).toBeInTheDocument();
  });
});

describe("TodoCard (归档/回收站版)", () => {
  it("归档态：渲染「恢复」按钮，隐藏 DoneCircle 与删除按钮", () => {
    const onUpdate = vi.fn();
    const onDelete = vi.fn();
    render(
      <TodoCard task={baseTask} archived onUpdate={onUpdate} onDelete={onDelete} />
    );
    expect(screen.getByText("↩ 恢复")).toBeInTheDocument();
    expect(screen.queryByTitle("标记完成")).not.toBeInTheDocument();
    // 标题行右侧的删除按钮（🗑️）在归档态不渲染
    expect(screen.queryByTitle("删除任务")).not.toBeInTheDocument();
  });

  it("回收站态：渲染「恢复」+「彻底删除」按钮", () => {
    const onUpdate = vi.fn();
    const onDelete = vi.fn();
    render(
      <TodoCard task={baseTask} trashed onUpdate={onUpdate} onDelete={onDelete} />
    );
    expect(screen.getByText("↩ 恢复")).toBeInTheDocument();
    expect(screen.getByText(/彻底删除/)).toBeInTheDocument();
  });

  it("完成态：归档态显示完成时间（formatCompletedAt 在 due 区域右侧）", () => {
    render(
      <TodoCard task={completedTask} archived onUpdate={vi.fn()} onDelete={vi.fn()} />
    );
    expect(screen.getByText(/完成 \d{2}-\d{2} \d{2}:\d{2}/)).toBeInTheDocument();
  });
});

// 让 dnd-kit useDraggable 在 jsdom 下不报错；TodoCard 直接调用 useDraggable 但不影响按钮交互
describe("TodoCard useDraggable 集成", () => {
  it("渲染 TodoCard 不会抛错（useDraggable 在 jsdom 中可工作）", () => {
    expect(() =>
      render(<TodoCard task={baseTask} onUpdate={vi.fn()} onDelete={vi.fn()} />)
    ).not.toThrow();
    // 验证整体结构存在
    expect(screen.getByText("默认任务")).toBeInTheDocument();
  });
});

// 兜底：测试用具，sanity check jsdom + @testing-library/jest-dom
describe("TodoCard 测试环境", () => {
  it("toBeInTheDocument matcher 可用", () => {
    const { container } = render(
      <TodoCardView task={baseTask} onUpdate={vi.fn()} onDelete={vi.fn()} />
    );
    expect(within(container).getByText("默认任务")).toBeInTheDocument();
  });
});
