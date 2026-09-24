// WidgetApp 子模块：挂件工作区可排序卡片（useSortable 注入）。
//
// ☰ 手柄 listeners 经 render prop 交给标题行；外部决定如何呈现（WidgetApp 主组件 JSX）。

import type { ReactNode } from "react";
import { CSS } from "@dnd-kit/utilities";
import type { DraggableAttributes, DraggableSyntheticListeners } from "@dnd-kit/core";
import { useSortable } from "@dnd-kit/sortable";

import type { WorkspaceItem } from "../../types";

// 手柄合并 props：listeners（含 KeyboardSensor 激活）+ attributes（role/tabIndex/aria-*）。
// 键盘可达性要求两者落在同一个可聚焦元素上——由 caller 铺到手柄（如 <span {...h}>☰</span>）。
// 注：DraggableSyntheticListeners 的 index signature（Function）与 attributes 的 role: string
// 不能干净相交，构造处用 as 断言（运行时就是普通 props 对象）。
export type SortableHandleProps =
  | (DraggableAttributes & DraggableSyntheticListeners)
  | undefined;

export function SortableWorkspaceCard({
  it,
  children,
}: {
  it: WorkspaceItem;
  children: (handleProps: SortableHandleProps) => ReactNode;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } =
    useSortable({ id: it.id });
  const style = { transform: CSS.Transform.toString(transform), transition };
  return (
    <div
      ref={setNodeRef}
      style={style}
      className={`group nm-card p-3 ${isDragging ? "opacity-70" : ""}`}
    >
      {children(
        listeners
          ? ({ ...attributes, ...listeners } as NonNullable<SortableHandleProps>)
          : undefined
      )}
    </div>
  );
}
