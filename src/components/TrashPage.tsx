import { DndContext } from "@dnd-kit/core";
import type { Task } from "../types";
import { TodoCard } from "./TodoCard";

export function TrashPage({
  tasks,
  onUpdate,
  onDelete,
  onClearAll,
}: {
  tasks: Task[];
  onUpdate: (id: string, patch: Partial<Task>) => void;
  onDelete: (id: string) => void;
  onClearAll: () => void;
}) {
  const trashed = tasks.filter((t) => t.deletedAt);

  return (
    <DndContext>
      <div className="mx-auto max-w-2xl">
        {trashed.length > 0 && (
          <div className="flex justify-end">
            <button
              className="text-xs text-gray-400 hover:text-red-500"
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

        <div className="mt-3 flex flex-col gap-3">
          {trashed.length === 0 ? (
            <p className="py-10 text-center text-sm text-gray-400">回收站是空的</p>
          ) : (
            trashed.map((t) => (
              <TodoCard
                key={t.id}
                task={t}
                onUpdate={onUpdate}
                onDelete={onDelete}
                trashed
              />
            ))
          )}
        </div>
      </div>
    </DndContext>
  );
}
