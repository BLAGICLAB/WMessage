import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { TaskTimelinePage } from "./TaskTimelinePage";
import { startOfWeek } from "./week";
import type { Task } from "../../types";

const MON = startOfWeek(new Date());
const day = (i: number) => {
  const d = new Date(MON);
  d.setDate(d.getDate() + i);
  return d;
};
const dt = (d: Date, hm: string) =>
  `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}T${hm}`;

const mk = (over: Partial<Task> & { id: string; title: string }): Task => ({
  column: "todo",
  updatedAt: 1000,
  ...over,
});

const TASKS: Task[] = [
  mk({ id: "t1", title: "已排期任务甲", tags: ["设计"], planStart: dt(day(2), "09:00"), planEnd: dt(day(2), "10:00") }),
  mk({ id: "t2", title: "未排期任务乙", updatedAt: 2000 }),
  mk({ id: "t3", title: "已完成任务丙", column: "done", completedAt: 1 }),
  mk({ id: "t4", title: "已删除任务丁", deletedAt: 5 }),
  mk({ id: "t5", title: "已归档任务戊", archived: true }),
  mk({ id: "t6", title: "已排期任务己", tags: ["开发"], planStart: dt(day(0), "14:00"), planEnd: dt(day(0), "15:00") }),
];

beforeEach(() => {
  localStorage.clear();
});

describe("TaskTimelinePage", () => {
  it("渲染周网格骨架：周一~周日列头、8:00/18:00 刻度、任务池", () => {
    render(<TaskTimelinePage tasks={TASKS} />);
    expect(screen.getByText("周一")).toBeInTheDocument();
    expect(screen.getByText("周日")).toBeInTheDocument();
    expect(screen.getByText("08:00")).toBeInTheDocument();
    expect(screen.getByText("18:00")).toBeInTheDocument();
    expect(screen.getByText("任务池")).toBeInTheDocument();
  });

  it("池列出全部未完成（含已排期），done/软删/归档不上池", () => {
    render(<TaskTimelinePage tasks={TASKS} />);
    const pool = within(screen.getByLabelText("任务池"));
    expect(pool.getByText("未排期任务乙")).toBeInTheDocument();
    expect(pool.getByText("已排期任务甲")).toBeInTheDocument();
    expect(pool.queryByText("已完成任务丙")).not.toBeInTheDocument();
    expect(pool.queryByText("已删除任务丁")).not.toBeInTheDocument();
    expect(pool.queryByText("已归档任务戊")).not.toBeInTheDocument();
    // 已排期带时间角标
    expect(pool.getByText("三 09:00")).toBeInTheDocument();
  });

  it("池排序：已排期按 planStart 升序在前，未排期按 updatedAt 降序在后", () => {
    render(<TaskTimelinePage tasks={TASKS} />);
    const pool = within(screen.getByLabelText("任务池"));
    const titles = pool
      .getAllByText(/已排期任务|未排期任务/)
      .map((el) => el.textContent);
    expect(titles).toEqual(["已排期任务己", "已排期任务甲", "未排期任务乙"]);
  });

  it("已排期任务在网格上渲染计划块（池与网格各出现一次）", () => {
    render(<TaskTimelinePage tasks={TASKS} />);
    expect(screen.getAllByText("已排期任务甲").length).toBe(2);
  });

  it("折叠任务池：列表收成右缘把手，状态记忆进 localStorage；展开复原", async () => {
    const user = userEvent.setup();
    render(<TaskTimelinePage tasks={TASKS} />);
    await user.click(screen.getByRole("button", { name: "折叠任务池" }));
    expect(screen.queryByText("未排期任务乙")).not.toBeInTheDocument();
    expect(localStorage.getItem("wm-task-pool-open")).toBe("0");
    await user.click(screen.getByRole("button", { name: /展开任务池/ }));
    expect(screen.getByText("未排期任务乙")).toBeInTheDocument();
    expect(localStorage.getItem("wm-task-pool-open")).toBe("1");
  });

  it("周导航：‹ 翻到上周出现「今天」，点「今天」回当前周；当前周不显示「今天」", async () => {
    const user = userEvent.setup();
    render(<TaskTimelinePage tasks={[]} />);
    const label = () => screen.getByText(/月.*日 –/);
    const initial = label().textContent;
    expect(screen.queryByText("今天")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "上一周" }));
    expect(label().textContent).not.toBe(initial);
    expect(screen.getByText("今天")).toBeInTheDocument();
    await user.click(screen.getByText("今天"));
    expect(label().textContent).toBe(initial);
    expect(screen.queryByText("今天")).not.toBeInTheDocument();
  });

  it("空任务：页面骨架照常渲染（网格 + 空池提示）", () => {
    render(<TaskTimelinePage tasks={[]} />);
    expect(screen.getByText("任务池")).toBeInTheDocument();
    expect(screen.getByText(/没有未完成的任务/)).toBeInTheDocument();
  });

  it("显式 null 计划字段（拖回池/清除计划写入）不上网格、不炸渲染", () => {
    render(
      <TaskTimelinePage
        tasks={[
          mk({ id: "n1", title: "已清除计划的任务", planStart: null, planEnd: null }),
        ]}
        onUpdate={vi.fn()}
        onSetColumn={vi.fn()}
        onDelete={vi.fn()}
      />,
    );
    // 池里照常可见（未完成），网格无块（只有池 + 面板外共 1 处文本）
    expect(screen.getAllByText("已清除计划的任务")).toHaveLength(1);
  });

  it("onNewTask：页头新建按钮透传回调", async () => {
    const user = userEvent.setup();
    let called = 0;
    render(<TaskTimelinePage tasks={[]} onNewTask={() => called++} />);
    await user.click(screen.getByRole("button", { name: "＋ 新建任务" }));
    expect(called).toBe(1);
  });

  it("点池项（按下即松，无位移）打开详情面板；Esc 关闭", () => {
    render(
      <TaskTimelinePage
        tasks={TASKS}
        onUpdate={vi.fn()}
        onSetColumn={vi.fn()}
        onDelete={vi.fn()}
      />,
    );
    expect(screen.queryByLabelText("任务详情")).not.toBeInTheDocument();
    // 按下 → 原位松开 = 点击（无位移不构成拖拽）
    fireEvent.pointerDown(screen.getByText("未排期任务乙"), { button: 0 });
    fireEvent.pointerUp(screen.getByText("未排期任务乙"));
    const panel = screen.getByLabelText("任务详情");
    expect(panel).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByLabelText("任务详情")).not.toBeInTheDocument();
  });

  it("从池拖出时池淡出让出视野（露出周末列），松手淡回", () => {
    render(
      <TaskTimelinePage
        tasks={TASKS}
        onUpdate={vi.fn()}
        onSetColumn={vi.fn()}
        onDelete={vi.fn()}
      />,
    );
    const pool = screen.getByLabelText("任务池");
    expect(pool.className).not.toContain("opacity-0");
    // 按下池项 + 移动 = 拖拽开始 → 池淡出
    fireEvent.pointerDown(screen.getByText("未排期任务乙"), { button: 0 });
    fireEvent.pointerMove(window, { clientX: 420, clientY: 320 });
    expect(pool.className).toContain("opacity-0");
    // 松手落位 → 淡回
    fireEvent.pointerUp(window, { clientX: 420, clientY: 320 });
    expect(pool.className).not.toContain("opacity-0");
  });

  it("editingId 变化：面板打开并聚焦标题（⌘N 新建路径）", () => {
    const { rerender } = render(
      <TaskTimelinePage tasks={TASKS} onUpdate={vi.fn()} onSetColumn={vi.fn()} onDelete={vi.fn()} />,
    );
    expect(screen.queryByLabelText("任务详情")).not.toBeInTheDocument();
    rerender(
      <TaskTimelinePage
        tasks={[...TASKS, mk({ id: "new1", title: "新任务" })]}
        onUpdate={vi.fn()}
        onSetColumn={vi.fn()}
        onDelete={vi.fn()}
        editingId="new1"
      />,
    );
    const panel = screen.getByLabelText("任务详情");
    const title = within(panel).getByLabelText("任务标题") as HTMLInputElement;
    expect(title.value).toBe("新任务");
    expect(document.activeElement).toBe(title);
  });

  it("已完成折叠区：点开列 done 任务，点行进面板可恢复待办", async () => {
    const user = userEvent.setup();
    const onSetColumn = vi.fn();
    render(
      <TaskTimelinePage
        tasks={[...TASKS, mk({ id: "t9", title: "完成任务庚", column: "done", completedAt: 1 })]}
        onUpdate={vi.fn()}
        onSetColumn={onSetColumn}
        onDelete={vi.fn()}
      />,
    );
    const pool = within(screen.getByLabelText("任务池"));
    expect(pool.queryByText("完成任务庚")).not.toBeInTheDocument();
    await user.click(pool.getByText(/已完成/));
    await user.click(pool.getByText("完成任务庚"));
    const panel = screen.getByLabelText("任务详情");
    await user.click(within(panel).getByRole("button", { name: "✓ 恢复待办" }));
    expect(onSetColumn).toHaveBeenCalledWith("t9", "todo");
  });
});
