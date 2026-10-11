import { describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@testing-library/react";
import { DndContext } from "@dnd-kit/core";
import { SortableContext } from "@dnd-kit/sortable";

vi.mock("../TaskCardContent", () => ({
  TaskCardContent: () => <div data-testid="tcc" />,
}));

import { SortableWorkspaceCard } from "./SortableWorkspaceCard";
import { TaskCard } from "./TaskCard";
import type { Task, WorkspaceItem } from "../../types";

const wsItem: WorkspaceItem = { id: "w1", title: "默认", links: [] };
const task = { id: "t1" } as unknown as Task;

function wrap(items: string[], node: React.ReactNode) {
  return render(
    <DndContext>
      <SortableContext items={items}>{node}</SortableContext>
    </DndContext>,
  );
}

describe("SortableWorkspaceCard 键盘可达性", () => {
  it("手柄（render prop 合并 props）带 tabIndex/role/aria，wrapper 不带", () => {
    const { getByTestId } = wrap(
      ["w1"],
      <SortableWorkspaceCard it={wsItem}>
        {(h) => (
          <span data-testid="handle" {...h}>
            ☰
          </span>
        )}
      </SortableWorkspaceCard>,
    );
    const handle = getByTestId("handle");
    expect(handle).toHaveAttribute("role", "button");
    expect(handle).toHaveAttribute("tabindex", "0");
    expect(handle).toHaveAttribute("aria-roledescription");
    const wrapper = handle.parentElement as HTMLElement;
    expect(wrapper).not.toHaveAttribute("role");
    expect(wrapper).not.toHaveAttribute("tabindex");
  });
});

// 挂件任务卡已无拖拽排序（列表顺序随主窗口任务池口径），selectMode 整卡单击选中等行为保留
describe("TaskCard 选任务模式", () => {
  const base = {
    task,
    editingTitle: false,
    selected: false,
    onCommitTitle: () => {},
    onCancelTitle: () => {},
    onToggleDone: () => {},
    onToggleCollapsed: () => {},
    onToggleSubtask: () => {},
    onOpenFilePath: () => {},
    onCopyFilePath: () => {},
    onRemoveFile: () => {},
  };

  it("selectMode 下整卡单击调 onSelect", () => {
    const onSelect = vi.fn();
    const { container } = render(
      <TaskCard {...base} selectMode onSelect={onSelect} />,
    );
    fireEvent.click(container.firstChild as HTMLElement);
    expect(onSelect).toHaveBeenCalledTimes(1);
  });

  it("selectMode 关闭时不挂 onClick", () => {
    const onSelect = vi.fn();
    const { container } = render(
      <TaskCard {...base} selectMode={false} onSelect={onSelect} />,
    );
    fireEvent.click(container.firstChild as HTMLElement);
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("左缘色条颜色 = planColorVar(首标签)，选中态带 ring", () => {
    const { container } = render(
      <TaskCard
        {...base}
        selectMode
        selected
        task={{ ...task, tags: ["设计"] }}
      />,
    );
    const bar = (container.firstChild as HTMLElement).querySelector(
      "span[aria-hidden]",
    ) as HTMLElement;
    expect(bar.style.background).toContain("--plan-");
    expect(container.firstChild).toHaveClass("ring-2");
  });
});
