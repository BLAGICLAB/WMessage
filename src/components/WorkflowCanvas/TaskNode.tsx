import { memo } from "react";
import { Handle, Position } from "@xyflow/react";
import { Trash2 } from "lucide-react";
import type { Task } from "../../types";
import { TaskCardContent } from "../TaskCardContent";

/** 工作流画布任务节点（W1-CANVAS 设计 §5.2）：
 *  内容复用挂件卡 TaskCardContent（nm-card 视觉统一），外面包 React Flow 手柄。
 *  未保存节点（无真实任务绑定）渲染极简草稿卡：仅标题输入。
 *  索引签名：React Flow v12 要求 node data 满足 Record<string, unknown> */
export interface TaskNodeData extends Record<string, unknown> {
  task: Task | undefined;
  /** 草稿标题（未保存节点用） */
  draftTitle: string;
  onDraftTitleCommit: (localId: string, title: string) => void;
  onDelete: (localId: string) => void;
  onToggleDone?: (taskId: string) => void;
  onCommitTitle?: (taskId: string, title: string) => void;
  onToggleSubtask?: (taskId: string, subtaskId: string) => void;
}

export const TaskNode = memo(function TaskNode({
  id,
  data,
}: {
  id: string;
  data: TaskNodeData;
}) {
  const { task, draftTitle } = data;
  const isDone = task?.column === "done";
  const isDoing = task?.column === "doing" && task?.botAssigned;
  const failed =
    task?.column === "done" &&
    task?.result?.status != null &&
    task.result.status !== "success";
  const border = isDoing
    ? "ring-2 ring-[var(--info,#3b82f6)]"
    : isDone
      ? failed
        ? "ring-2 ring-[var(--danger,#ef4444)]"
        : "ring-2 ring-[var(--ok,#22c55e)]"
      : "";
  return (
    <div className={`nm-card w-[340px] px-1 pt-1 pb-2 ${border}`}>
      {/* 连线手柄：上=下游入（被依赖），下=上游出（依赖别人）——
          视觉上"从卡底拉到卡顶"与图流向（上→下）一致 */}
      <Handle type="target" position={Position.Top} />
      <Handle type="source" position={Position.Bottom} />
      <button
        aria-label="删除节点"
        title="删除节点（连带清理相关连线，保存后生效）"
        onClick={(e) => {
          e.stopPropagation();
          data.onDelete(id);
        }}
        className="absolute -right-2 -top-2 z-10 rounded-full border border-[var(--edge)] bg-[var(--bg)] p-1 text-[var(--t5)] hover:text-[var(--danger,#ef4444)]"
      >
        <Trash2 size={12} aria-hidden />
      </button>
      {task ? (
        <TaskCardContent
          task={task}
          onTitleClick={() => {}}
          onCommitTitle={(t) => data.onCommitTitle?.(task.id, t)}
          onCancelTitle={() => {}}
          onToggleDone={() => data.onToggleDone?.(task.id)}
          onToggleSubtask={(sid) => data.onToggleSubtask?.(task.id, sid)}
        />
      ) : (
        <input
          aria-label="新任务标题"
          className="w-full rounded-[var(--r-sm)] bg-transparent px-2 py-1.5 text-sm text-[var(--t1)] outline-none placeholder:text-[var(--t5)]"
          value={draftTitle}
          placeholder="新任务"
          onChange={(e) => data.onDraftTitleCommit(id, e.target.value)}
        />
      )}
    </div>
  );
});
