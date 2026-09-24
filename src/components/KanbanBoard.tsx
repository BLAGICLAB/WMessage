import { useState } from "react";
import {
  DndContext,
  DragEndEvent,
  DragOverEvent,
  DragOverlay,
  DragStartEvent,
  PointerSensor,
  closestCorners,
  useDroppable,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import {
  SortableContext,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import type { ColumnId, Task } from "../types";
import { SortableTodoCard, TodoCardView } from "./TodoCard";

const COLUMNS: { id: ColumnId; label: string }[] = [
  { id: "todo", label: "待办" },
  { id: "doing", label: "今日" },
  { id: "done", label: "完成" },
];

const isColumnId = (id: string): id is ColumnId =>
  COLUMNS.some((c) => c.id === id);

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
    <div className="flex-1 min-w-56 flex flex-col">
      <div className="nm-inset mb-3 px-4 py-2 flex items-baseline justify-between">
        <h2 className="text-lg font-semibold tracking-wide text-[var(--t1)]">{label}</h2>
        <span className="text-xs font-medium tabular-nums text-[var(--t5)]">
          {tasks.length}
        </span>
      </div>
      <div
        ref={setNodeRef}
        className={`flex-1 rounded-2xl py-3 flex flex-col gap-3 min-h-32 transition-shadow ${
          isOver ? "nm-inset" : ""
        }`}
      >
        <SortableContext
          items={tasks.map((t) => t.id)}
          strategy={verticalListSortingStrategy}
        >
          {tasks.map((t) => (
            <SortableTodoCard
              key={t.id}
              task={t}
              autoEdit={t.id === editingId}
              onUpdate={onUpdate}
              onDelete={onDelete}
            />
          ))}
        </SortableContext>
      </div>
      {footer}
    </div>
  );
}

/** 在扁平数组中移除 active 并插入到 over 位置（跨列时同步改 column）。导出仅供单测直测 */
export function spliceMove(
  flat: Task[],
  activeId: string,
  targetCol: ColumnId,
  overId: string,
  below: boolean
): Task[] {
  const moved = flat.find((t) => t.id === activeId);
  if (!moved) return flat;
  const rest = flat.filter((t) => t.id !== activeId);
  const updated: Task = { ...moved, column: targetCol };
  let insertAt: number;
  if (isColumnId(overId)) {
    // 落到列空白区：插到该列最后一个任务之后
    let last = -1;
    for (let i = rest.length - 1; i >= 0; i--) {
      if (rest[i].column === targetCol) {
        last = i;
        break;
      }
    }
    insertAt = last === -1 ? rest.length : last + 1;
  } else {
    const idx = rest.findIndex((t) => t.id === overId);
    insertAt = idx === -1 ? rest.length : idx + (below ? 1 : 0);
  }
  rest.splice(insertAt, 0, updated);
  return rest;
}

export function KanbanBoard({
  tasks,
  editingId,
  onReorder,
  onUpdate,
  onDelete,
  onOpenArchive,
}: {
  tasks: Task[];
  editingId: string | null;
  /** 拖拽结束后提交最终顺序（含跨列变更），order 由上层统一分配 */
  onReorder: (activeId: string, next: Task[]) => void;
  onUpdate: (taskId: string, patch: Partial<Task>) => void;
  onDelete: (taskId: string) => void;
  onOpenArchive: () => void;
}) {
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } })
  );

  // 拖拽期间的本地草稿：跨列实时移动走 draft，松手才提交
  const [draft, setDraft] = useState<Task[] | null>(null);
  const [activeId, setActiveId] = useState<string | null>(null);
  const live = draft ?? tasks;

  const columnOf = (id: string): ColumnId | null => {
    if (isColumnId(id)) return id;
    return live.find((t) => t.id === id)?.column ?? null;
  };

  const handleDragStart = (e: DragStartEvent) => {
    setActiveId(String(e.active.id));
    setDraft(tasks);
  };

  const handleDragOver = (e: DragOverEvent) => {
    const { active, over } = e;
    if (!over || !draft) return;
    const activeCol = columnOf(String(active.id));
    const overCol = columnOf(String(over.id));
    if (!activeCol || !overCol || activeCol === overCol) return;
    // 跨列：实时把卡片挪到目标列（完成语义在上层 commit 时统一处理）
    setDraft((prev) =>
      prev
        ? spliceMove(prev, String(active.id), overCol, String(over.id), false)
        : prev
    );
  };

  const handleDragEnd = (e: DragEndEvent) => {
    const { active, over } = e;
    setActiveId(null);
    setDraft(null);
    // 松手时指针常落在被拖卡片自己的新位置（尤其拖进空列时 dnd-kit 会把自身作为 over），
    // 或落在可拖区外（over 为 null）。此时若草稿已发生跨列移动，按草稿提交，否则卡片会弹回原列。
    if (!over || active.id === over.id) {
      const moved = draft?.find((t) => t.id === String(active.id));
      const original = tasks.find((t) => t.id === String(active.id));
      if (draft && moved && original && moved.column !== original.column) {
        onReorder(String(active.id), draft);
      }
      return;
    }
    const activeCol = columnOf(String(active.id));
    const overCol = columnOf(String(over.id));
    if (!activeCol || !overCol) return;
    const translated =
      active.rect.current?.translated ?? active.rect.current?.initial;
    const below =
      translated && over.rect
        ? translated.top + translated.height / 2 >
          over.rect.top + over.rect.height / 2
        : false;
    onReorder(
      String(active.id),
      spliceMove(tasks, String(active.id), overCol, String(over.id), below)
    );
  };

  const handleDragCancel = () => {
    setActiveId(null);
    setDraft(null);
  };

  const archivedCount = tasks.filter(
    (t) => t.column === "done" && t.archived && !t.deletedAt
  ).length;
  const activeTask = activeId ? live.find((t) => t.id === activeId) : undefined;

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCorners}
      onDragStart={handleDragStart}
      onDragOver={handleDragOver}
      onDragEnd={handleDragEnd}
      onDragCancel={handleDragCancel}
    >
      <div className="flex gap-4 items-start">
        {COLUMNS.map((c) => (
          <Column
            key={c.id}
            {...c}
            tasks={live.filter(
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
                  className="mt-2 text-xs text-[var(--t5)] hover:text-[var(--t2)]"
                  onClick={onOpenArchive}
                >
                  🗄 已归档 {archivedCount}
                </button>
              ) : null
            }
          />
        ))}
      </div>
      <DragOverlay>
        {activeTask ? (
          <div className="w-64">
            <TodoCardView
              task={activeTask}
              autoEdit={false}
              onUpdate={onUpdate}
              onDelete={onDelete}
            />
          </div>
        ) : null}
      </DragOverlay>
    </DndContext>
  );
}
