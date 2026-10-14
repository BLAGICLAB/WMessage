// WidgetApp 子模块：挂件任务卡（静态渲染）。
//
// 列表顺序随主窗口任务池口径（已排期在前按 planStart 升序，未排期按 updatedAt
// 降序），不存在用户自定义顺序——因此没有卡内拖拽排序。
// 左缘色条与主窗口任务池/首页任务卡同源：首标签哈希 → 计划色板（planColorVar）。

import { TaskCardContent } from "../TaskCardContent";
import { planColorVar } from "../TaskTimeline/week";
import type { Task } from "../../types";

export function TaskCard({
  task,
  dueOverdue = false,
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
  onOpenSubtaskCard,
}: {
  task: Task;
  /** 截止时间已过且未完成：截止徽标标红（与首页到期横条同一判定口径） */
  dueOverdue?: boolean;
  editingTitle: boolean;
  selected: boolean;
  /** 选任务模式：整卡单击切换选中 */
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
  /** 子任务行已生成任务卡时显示 ↗；点击跳主窗口编辑页 */
  onOpenSubtaskCard?: (cardId: string) => void;
}) {
  return (
    <div
      onClick={
        selectMode
          ? () => {
              onSelect?.();
            }
          : undefined
      }
      className={`group nm-card flex gap-2 py-2 pl-2.5 pr-3 text-left shrink-0 ${
        selected ? "ring-2 ring-[var(--brand)]" : ""
      } ${selectMode ? "cursor-pointer" : ""}`}
    >
      <span
        aria-hidden
        className="w-[3px] shrink-0 self-stretch rounded-full"
        style={{ background: planColorVar(task.tags) }}
      />
      <div className="min-w-0 flex-1">
        <TaskCardContent
          task={task}
          dueOverdue={dueOverdue}
          editingTitle={editingTitle}
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
          onOpenSubtaskCard={onOpenSubtaskCard}
        />
      </div>
    </div>
  );
}
