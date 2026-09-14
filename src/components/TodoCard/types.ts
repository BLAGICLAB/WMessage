// TodoCard 子模块：纯类型定义（无 React / 无 IO）。
// 由同目录其他子文件 import，外部不直接引用。

import type { CSSProperties } from "react";
import type {
  DraggableAttributes,
  DraggableSyntheticListeners,
} from "@dnd-kit/core";
import type { Task } from "../../types";

export interface TodoCardViewProps {
  task: Task;
  autoEdit?: boolean;
  onUpdate: (id: string, patch: Partial<Task>) => void;
  onDelete: (id: string) => void;
  /** 归档视图：显示「恢复」按钮 */
  archived?: boolean;
  /** 回收站视图：显示「恢复 / 彻底删除」按钮 */
  trashed?: boolean;
}

/** 拖拽能力由外部 hook（useDraggable / useSortable）注入，View 本体不关心排序上下文 */
export interface CardDrag {
  setNodeRef: (node: HTMLElement | null) => void;
  style?: CSSProperties;
  attributes: DraggableAttributes;
  listeners: DraggableSyntheticListeners | undefined;
  isDragging: boolean;
}

/** 子任务条目（TodoCardView 内部用） */
export interface SubtaskEntry {
  id: string;
  text: string;
  done: boolean;
}
