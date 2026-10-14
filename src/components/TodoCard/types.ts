// TodoCard 子模块：纯类型定义（无 React / 无 IO）。
// 由同目录其他子文件 import，外部不直接引用。

import type { CSSProperties } from "react";
import type {
  DraggableAttributes,
  DraggableSyntheticListeners,
} from "@dnd-kit/core";
import type { ColumnId, Task } from "../../types";

export interface TodoCardViewProps {
  task: Task;
  onUpdate: (id: string, patch: Partial<Task>) => void;
  onDelete: (id: string) => void;
  /** TP-1：列状态定向迁移（服务端命令，免快照竞态）；缺省回落 onUpdate 整行写 */
  onSetColumn?: (taskId: string, col: ColumnId) => void;
  /** 归档视图：显示「恢复」按钮 */
  archived?: boolean;
  /** 回收站视图：显示「恢复 / 彻底删除」按钮 */
  trashed?: boolean;
  /** 子任务行已生成任务卡（sub.taskId 在场）时显示 ↗；点击跳主窗口编辑页。
   *  归档/回收站卡的只读动作（同点开/复制），缺省不显示 ↗ */
  onOpenSubtaskCard?: (cardId: string) => void;
}

/** 拖拽能力由外部 hook（useDraggable / useSortable）注入，View 本体不关心排序上下文 */
export interface CardDrag {
  setNodeRef: (node: HTMLElement | null) => void;
  style?: CSSProperties;
  attributes: DraggableAttributes;
  listeners: DraggableSyntheticListeners | undefined;
  isDragging: boolean;
}
