import { Trash2 } from "lucide-react";
import type { Task } from "../types";
import { TodoCard } from "./TodoCard";
import { EmptyState } from "./EmptyState";

export function TrashPage({
  tasks,
  editingId,
  onUpdate,
  onDelete,
}: {
  tasks: Task[];
  /** 机器人 📌 引用跳转：命中任务卡自动进入标题编辑态 */
  editingId?: string | null;
  onUpdate: (id: string, patch: Partial<Task>) => void;
  onDelete: (id: string) => void;
}) {
  // 回收站按最后修改时间倒序（新删的在前面），原 filter 不打排序
  // 导致顺序依赖任务数组原始 order，删除时间与展示位置对不上
  const trashed = tasks
    .filter((t) => t.deletedAt)
    .sort((a, b) => (b.updatedAt ?? 0) - (a.updatedAt ?? 0));

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
                autoEdit={t.id === editingId}
                onUpdate={onUpdate}
                onDelete={onDelete}
                trashed
              />
            ))}
          </div>
        )}
    </div>
  );
}
