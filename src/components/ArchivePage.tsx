import { useState } from "react";
import { DndContext } from "@dnd-kit/core";
import type { Task } from "../types";
import { TodoCard } from "./TodoCard";

export function ArchivePage({
  tasks,
  onUpdate,
  onDelete,
}: {
  tasks: Task[];
  onUpdate: (id: string, patch: Partial<Task>) => void;
  onDelete: (id: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [activeTag, setActiveTag] = useState<string | null>(null);

  const archived = tasks.filter((t) => t.archived && !t.deletedAt);

  // 所有标签 + 计数（按归档任务统计，按计数降序）
  const tagCounts = new Map<string, number>();
  for (const t of archived) {
    for (const tag of t.tags ?? []) {
      tagCounts.set(tag, (tagCounts.get(tag) ?? 0) + 1);
    }
  }
  const tags = [...tagCounts.entries()].sort((a, b) => b[1] - a[1]);

  const q = query.trim().toLowerCase();
  const filtered = archived.filter((t) => {
    if (activeTag && !(t.tags ?? []).includes(activeTag)) return false;
    if (q && !`${t.title} ${t.note ?? ""}`.toLowerCase().includes(q)) return false;
    return true;
  });

  return (
    <DndContext>
      <div className="mx-auto max-w-2xl">
        {/* 搜索 */}
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="搜索归档内容…"
          className="nm-inset w-full rounded-xl px-4 py-2.5 text-sm text-gray-700 outline-none"
        />

        {/* 标签列表（点击筛选同标签） */}
        {tags.length > 0 && (
          <div className="mt-3 flex flex-wrap items-center gap-1.5">
            {tags.map(([tag, count]) => (
              <button
                key={tag}
                onClick={() => setActiveTag(activeTag === tag ? null : tag)}
                className={`nm-inset px-2.5 py-1 text-xs flex items-center gap-1 ${
                  activeTag === tag ? "text-gray-900" : "text-gray-500"
                }`}
              >
                {tag}
                <span className="tabular-nums text-gray-400">{count}</span>
              </button>
            ))}
          </div>
        )}

        {/* 归档列表 */}
        <div className="mt-4 flex flex-col gap-3">
          {filtered.length === 0 ? (
            <p className="py-10 text-center text-sm text-gray-400">
              {archived.length === 0 ? "暂无归档内容" : "没有匹配的归档"}
            </p>
          ) : (
            filtered.map((t) => (
              <TodoCard
                key={t.id}
                task={t}
                onUpdate={onUpdate}
                onDelete={onDelete}
                archived
              />
            ))
          )}
        </div>
      </div>
    </DndContext>
  );
}
