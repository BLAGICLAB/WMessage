import type { Task } from "../types";
import { TodoCard } from "./TodoCard";

export function TrashPage({
  tasks,
  editingId,
  onUpdate,
  onDelete,
  onClearAll,
}: {
  tasks: Task[];
  /** 机器人 📌 引用跳转：命中任务卡自动进入标题编辑态 */
  editingId?: string | null;
  onUpdate: (id: string, patch: Partial<Task>) => void;
  onDelete: (id: string) => void;
  onClearAll: () => void;
}) {
  const trashed = tasks.filter((t) => t.deletedAt);

  return (
    <div>
        {trashed.length > 0 && (
          <div className="flex justify-end">
            <button
              className="text-xs text-[var(--t5)] hover:text-[var(--danger)]"
              onClick={() => {
                if (
                  window.confirm(
                    `确定清空回收站？将永久删除 ${trashed.length} 个任务`
                  )
                )
                  onClearAll();
              }}
            >
              清空回收站
            </button>
          </div>
        )}

        {trashed.length === 0 ? (
          <p className="py-10 text-center text-sm text-[var(--t5)]">回收站是空的</p>
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
