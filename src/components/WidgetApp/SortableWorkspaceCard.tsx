// WidgetApp 子模块：挂件工作区可排序卡片（useSortable 注入）。
//
// ☰ 手柄 listeners 经 render prop 交给标题行；外部决定如何呈现（WidgetApp 主组件 JSX）。

import type { ReactNode } from "react";
import { CSS } from "@dnd-kit/utilities";
import type { DraggableSyntheticListeners } from "@dnd-kit/core";
import { useSortable } from "@dnd-kit/sortable";

import type { WorkspaceItem } from "../../types";

export function SortableWorkspaceCard({
  it,
  children,
}: {
  it: WorkspaceItem;
  children: (listeners: DraggableSyntheticListeners | undefined) => ReactNode;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } =
    useSortable({ id: it.id });
  const style = { transform: CSS.Transform.toString(transform), transition };
  return (
    <div
      ref={setNodeRef}
      style={style}
      {...attributes}
      className={`group nm-card p-3 ${isDragging ? "opacity-70" : ""}`}
    >
      {children(listeners)}
    </div>
  );
}
