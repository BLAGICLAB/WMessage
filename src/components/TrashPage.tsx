import { Trash2 } from "lucide-react";
import type { Task } from "../types";
import { TodoCard } from "./TodoCard";
import { EmptyState } from "./EmptyState";
import { isTrashed } from "../lib/taskScope";

export function TrashPage({
  tasks,
  onUpdate,
  onDelete,
  onOpenSubtaskCard,
}: {
  tasks: Task[];
  onUpdate: (id: string, patch: Partial<Task>) => void;
  onDelete: (id: string) => void;
  /** 子任务行已生成任务卡时显示 ↗；点击跳主窗口编辑页 */
  onOpenSubtaskCard?: (cardId: string) => void;
}) {
  // 回收站按删除时间倒序（新删的在前面），原 filter 不打排序
  // 导致顺序依赖任务数组原始 order，删除时间与展示位置对不上
  const trashed = tasks
    .filter(isTrashed)
    .sort((a, b) => (b.deletedAt ?? 0) - (a.deletedAt ?? 0));

  return (
    <div>
      {trashed.length === 0 ? (
        <EmptyState
          icon={<Trash2 size={18} aria-hidden />}
          title="回收站是空的"
          description="软删除的任务会进回收站，可恢复或彻底删除"
        />
      ) : (
        <div className="mt-3 grid grid-cols-3 gap-4 items-start">
          {trashed.map((t) => (
            <TodoCard
              key={t.id}
              task={t}
              onUpdate={onUpdate}
              onDelete={onDelete}
              onOpenSubtaskCard={onOpenSubtaskCard}
              trashed
            />
          ))}
        </div>
      )}
    </div>
  );
}
