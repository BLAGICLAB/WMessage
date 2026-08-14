import {
  DndContext,
  DragEndEvent,
  PointerSensor,
  useDroppable,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import type { ColumnId, Task } from "../types";
import { TodoCard } from "./TodoCard";

export const COLUMNS: { id: ColumnId; label: string }[] = [
  { id: "todo", label: "待办" },
  { id: "doing", label: "今日" },
  { id: "done", label: "完成" },
];

function Column({
  id,
  label,
  tasks,
  editingId,
  onUpdate,
  onDelete,
  footer = null,
}: {
  id: ColumnId;
  label: string;
  tasks: Task[];
  editingId: string | null;
  onUpdate: (taskId: string, patch: Partial<Task>) => void;
  onDelete: (taskId: string) => void;
  footer?: React.ReactNode;
}) {
  const { setNodeRef, isOver } = useDroppable({ id });
  return (
    <div className="flex-1 min-w-60 flex flex-col">
      <div className="nm-inset mb-3 px-4 py-2 flex items-baseline justify-between">
        <h2 className="text-lg font-semibold tracking-wide text-gray-800">{label}</h2>
        <span className="text-xs font-medium tabular-nums text-gray-400">
          {tasks.length}
        </span>
      </div>
      <div
        ref={setNodeRef}
        className={`flex-1 rounded-2xl p-3 flex flex-col gap-3 min-h-32 transition-shadow ${
          isOver ? "nm-inset" : ""
        }`}
      >
        {tasks.map((t) => (
          <TodoCard
            key={t.id}
            task={t}
            autoEdit={t.id === editingId}
            onUpdate={onUpdate}
            onDelete={onDelete}
          />
        ))}
      </div>
      {footer}
    </div>
  );
}

export function KanbanBoard({
  tasks,
  editingId,
  onMove,
  onUpdate,
  onDelete,
  onOpenArchive,
}: {
  tasks: Task[];
  editingId: string | null;
  onMove: (taskId: string, column: ColumnId) => void;
  onUpdate: (taskId: string, patch: Partial<Task>) => void;
  onDelete: (taskId: string) => void;
  onOpenArchive: () => void;
}) {
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } })
  );

  const archivedCount = tasks.filter((t) => t.column === "done" && t.archived).length;

  const handleDragEnd = (event: DragEndEvent) => {
    const { active, over } = event;
    if (!over) return;
    const target = COLUMNS.some((c) => c.id === over.id) ? (over.id as ColumnId) : undefined;
    if (target) onMove(String(active.id), target);
  };

  return (
    <DndContext sensors={sensors} onDragEnd={handleDragEnd}>
      <div className="flex gap-4 p-4 items-start">
        {COLUMNS.map((c) => (
          <Column
            key={c.id}
            {...c}
            tasks={tasks.filter(
              (t) =>
                t.column === c.id &&
                !t.deletedAt &&
                (c.id !== "done" || !t.archived)
            )}
            editingId={editingId}
            onUpdate={onUpdate}
            onDelete={onDelete}
            footer={
              c.id === "done" && archivedCount > 0 ? (
                <button
                  className="mt-2 text-xs text-gray-400 hover:text-gray-600"
                  onClick={onOpenArchive}
                >
                  🗄 已归档 {archivedCount}
                </button>
              ) : null
            }
          />
        ))}
      </div>
    </DndContext>
  );
}
