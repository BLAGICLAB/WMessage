import { useMemo, useState } from "react";
import type { Task } from "../types";
import { sortByOrder } from "../storage";
import { TodoCard } from "./TodoCard";

/**
 * 归档页：搜索 → 标签计数（点击筛选，与搜索 AND 叠加）→ 三列卡片网格。
 * 归档任务按 order 从左列到右列依次排布，卡片默认折叠（展开后保持展开）。
 */
export function ArchivePage({
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
  const [query, setQuery] = useState("");
  const [activeTag, setActiveTag] = useState<string | null>(null);

  const archived = tasks.filter((t) => t.archived && !t.deletedAt);

  // 标签计数（降序）：多标签任务在每个标签下各计一次
  const tagCounts = useMemo(() => {
    const counts = new Map<string, number>();
    for (const t of archived) {
      for (const tag of t.tags ?? []) {
        counts.set(tag, (counts.get(tag) ?? 0) + 1);
      }
    }
    return [...counts.entries()].sort((a, b) => b[1] - a[1]);
  }, [archived]);

  // 过滤：标签筛选 + 搜索 AND 叠加；按 order 排序（从左到右依次填充三列）
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return sortByOrder(
      archived.filter((t) => {
        if (activeTag && !(t.tags ?? []).includes(activeTag)) return false;
        if (!q) return true;
        return `${t.title} ${t.note ?? ""}`.toLowerCase().includes(q);
      })
    );
  }, [archived, query, activeTag]);

  /** 归档卡片默认折叠（collapsed 为 false 时说明用户已展开，保持展开）；
   *  编辑态命中（机器人 📌 跳转）时展开，否则折叠卡片里看不到跳转效果。
   *  仅真正翻转（collapsed 未定义）才产副本——已折叠的复用引用，保 memo/引用相等 */
  const displayTask = (t: Task): Task =>
    t.collapsed !== undefined || t.id === editingId
      ? t
      : { ...t, collapsed: true };

  return (
    <div>
      {/* 搜索 */}
      {archived.length > 0 && (
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="搜索归档内容…"
          className="nm-inset w-full rounded-xl px-4 py-2.5 text-sm text-[var(--t2)] outline-none"
        />
      )}

      {/* 标签计数（点击筛选，再点取消） */}
      {tagCounts.length > 0 && (
        <div className="mt-3 flex flex-wrap gap-1.5">
          {tagCounts.map(([tag, count]) => (
            <button
              key={tag}
              className={`px-2.5 py-1 text-xs rounded-full transition-colors ${
                activeTag === tag
                  ? "nm-inset text-[var(--t1)] font-medium"
                  : "nm-outset text-[var(--t4)] hover:text-[var(--t2)]"
              }`}
              onClick={() => setActiveTag(activeTag === tag ? null : tag)}
            >
              # {tag}
              <span className="ml-1 tabular-nums text-[var(--t5)]">{count}</span>
            </button>
          ))}
        </div>
      )}

      {archived.length === 0 ? (
        <p className="py-16 text-center text-sm text-[var(--t5)]">暂无归档内容</p>
      ) : filtered.length === 0 ? (
        <p className="py-10 text-center text-sm text-[var(--t5)]">没有匹配的归档</p>
      ) : (
        <div className="mt-4 grid grid-cols-3 gap-4 items-start">
          {filtered.map((t) => (
            <TodoCard
              key={t.id}
              task={displayTask(t)}
              autoEdit={t.id === editingId}
              onUpdate={onUpdate}
              onDelete={onDelete}
              archived
            />
          ))}
        </div>
      )}
    </div>
  );
}
