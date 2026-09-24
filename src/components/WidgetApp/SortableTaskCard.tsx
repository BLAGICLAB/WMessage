// WidgetApp 子模块：挂件可排序任务卡（useSortable 注入 + TaskCardContent 渲染）。
//
// 手柄 ☰ 在 TaskCardContent 内部；selectMode 时整卡单击切换选中。

import { useRef } from "react";
import { CSS } from "@dnd-kit/utilities";
import { useDndMonitor } from "@dnd-kit/core";
import { useSortable } from "@dnd-kit/sortable";

import { TaskCardContent } from "../TaskCardContent";
import type { Task } from "../../types";

export function SortableTaskCard({
  task,
  editingTitle,
  selected,
  selectMode,
  onSelect,
  onTitleClick,
  onCommitTitle,
  onCancelTitle,
  onToggleDone,
  onToggleCollapsed,
  onToggleSubtask,
  onOpenFilePath,
  onCopyFilePath,
  onRemoveFile,
  onBotExecute,
  onSetSchedule,
}: {
  task: Task;
  editingTitle: boolean;
  selected: boolean;
  /** 选任务模式：整卡单击选中（内部交互按钮除外），标题双击去主窗口暂停 */
  selectMode: boolean;
  onSelect?: () => void;
  onTitleClick?: () => void;
  onCommitTitle: (title: string) => void;
  onCancelTitle: () => void;
  onToggleDone: () => void;
  onToggleCollapsed: () => void;
  onToggleSubtask: (subtaskId: string) => void;
  onOpenFilePath: (path: string) => void;
  onCopyFilePath: (path: string) => void;
  onRemoveFile: (path: string) => void;
  onBotExecute?: () => void;
  onSetSchedule?: (schedule: string | undefined) => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } =
    useSortable({ id: task.id });
  // 拖拽守卫：拖拽结束后浏览器会在同一元素派发 click——selectMode 下不能误切选中。
  // end/cancel 用 setTimeout(0) 复位：click 在 pointerup 后、timeout 前派发，时序上守卫有效。
  const wasDragging = useRef(false);
  const resetIfSelf = (id: string | number) => {
    if (id === task.id) setTimeout(() => { wasDragging.current = false; }, 0);
  };
  useDndMonitor({
    onDragStart(e) {
      if (e.active.id === task.id) wasDragging.current = true;
    },
    onDragEnd(e) {
      resetIfSelf(e.active.id);
    },
    onDragCancel(e) {
      resetIfSelf(e.active.id);
    },
  });
  const style = { transform: CSS.Transform.toString(transform), transition };
  return (
    <div
      ref={setNodeRef}
      style={style}
      {...attributes}
      onClick={
        selectMode
          ? () => {
              if (!wasDragging.current) onSelect?.();
            }
          : undefined
      }
      className={`group nm-card px-3 py-2 text-left shrink-0 ${
        isDragging ? "opacity-70" : ""
      } ${selected ? "ring-2 ring-[var(--brand)]" : ""} ${
        selectMode ? "cursor-pointer" : ""
      }`}
    >
      <TaskCardContent
        task={task}
        editingTitle={editingTitle}
        handleListeners={listeners}
        onTitleClick={onTitleClick}
        onCommitTitle={onCommitTitle}
        onCancelTitle={onCancelTitle}
        onToggleDone={onToggleDone}
        onToggleCollapsed={onToggleCollapsed}
        onToggleSubtask={onToggleSubtask}
        onOpenFilePath={onOpenFilePath}
        onCopyFilePath={onCopyFilePath}
        onRemoveFile={onRemoveFile}
        onBotExecute={onBotExecute}
        onSetSchedule={onSetSchedule}
      />
    </div>
  );
}
