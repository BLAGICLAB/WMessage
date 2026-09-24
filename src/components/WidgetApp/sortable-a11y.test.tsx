import { describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@testing-library/react";
import { DndContext } from "@dnd-kit/core";
import { SortableContext } from "@dnd-kit/sortable";

vi.mock("../TaskCardContent", () => ({
  TaskCardContent: () => <div data-testid="tcc" />,
}));

import { SortableWorkspaceCard } from "./SortableWorkspaceCard";
import { SortableTaskCard } from "./SortableTaskCard";
import type { Task, WorkspaceItem } from "../../types";

const wsItem: WorkspaceItem = { id: "w1", title: "默认", links: [] };
const task = { id: "t1" } as unknown as Task;

function wrap(items: string[], node: React.ReactNode) {
  return render(
    <DndContext>
      <SortableContext items={items}>{node}</SortableContext>
    </DndContext>
  );
}

describe("SortableWorkspaceCard 键盘可达性", () => {
  it("手柄（render prop 合并 props）带 tabIndex/role/aria，wrapper 不带", () => {
    const { getByTestId } = wrap(["w1"], (
      <SortableWorkspaceCard it={wsItem}>
        {(h) => (
          <span data-testid="handle" {...h}>
            ☰
          </span>
        )}
      </SortableWorkspaceCard>
    ));
    const handle = getByTestId("handle");
    expect(handle).toHaveAttribute("role", "button");
    expect(handle).toHaveAttribute("tabindex", "0");
    expect(handle).toHaveAttribute("aria-roledescription");
    const wrapper = handle.parentElement as HTMLElement;
    expect(wrapper).not.toHaveAttribute("role");
    expect(wrapper).not.toHaveAttribute("tabindex");
  });
});

describe("SortableTaskCard 拖拽守卫", () => {
  it("selectMode 下普通 click（非拖拽衍生）仍调 onSelect——守卫不误伤", () => {
    const onSelect = vi.fn();
    const { container } = wrap(["t1"], (
      <SortableTaskCard
        task={task}
        editingTitle={false}
        selected={false}
        selectMode
        onSelect={onSelect}
        onCommitTitle={() => {}}
        onCancelTitle={() => {}}
        onToggleDone={() => {}}
        onToggleCollapsed={() => {}}
        onToggleSubtask={() => {}}
        onOpenFilePath={() => {}}
        onCopyFilePath={() => {}}
        onRemoveFile={() => {}}
      />
    ));
    fireEvent.click(container.firstChild as HTMLElement);
    expect(onSelect).toHaveBeenCalledTimes(1);
  });

  it("selectMode 关闭时不挂 onClick", () => {
    const onSelect = vi.fn();
    const { container } = wrap(["t1"], (
      <SortableTaskCard
        task={task}
        editingTitle={false}
        selected={false}
        selectMode={false}
        onSelect={onSelect}
        onCommitTitle={() => {}}
        onCancelTitle={() => {}}
        onToggleDone={() => {}}
        onToggleCollapsed={() => {}}
        onToggleSubtask={() => {}}
        onOpenFilePath={() => {}}
        onCopyFilePath={() => {}}
        onRemoveFile={() => {}}
      />
    ));
    fireEvent.click(container.firstChild as HTMLElement);
    expect(onSelect).not.toHaveBeenCalled();
  });
});
