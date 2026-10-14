import { useEffect, useMemo, useRef, useState } from "react";
import { Archive, Tags } from "lucide-react";
import type { Task } from "../types";
import { sortByOrder } from "../storage";
import { getArchiveAfterDays } from "../lib/archiveRule";
import { isArchived } from "../lib/taskScope";
import { TodoCard } from "./TodoCard";
import { EmptyState } from "./EmptyState";
import { TagConsolidateModal } from "./TagConsolidateModal";

/**
 * 归档页：搜索 → 标签计数（点击筛选，与搜索 AND 叠加）→ 三列卡片网格。
 * 归档任务按 order 从左列到右列依次排布，卡片默认折叠（展开后保持展开）。
 * 标签行默认折叠（「标签不常看」——展开需要点 details）；
 * 「整理标签」按钮始终显示，单击开 TagConsolidateModal 弹窗。
 */
export function ArchivePage({
  tasks,
  onUpdate,
  onDelete,
  onOpenSubtaskCard,
  highlightTaskId,
  onClearHighlight,
}: {
  tasks: Task[];
  onUpdate: (id: string, patch: Partial<Task>) => void;
  onDelete: (id: string) => void;
  /** 子任务行已生成任务卡时显示 ↗；点击跳主窗口编辑页 */
  onOpenSubtaskCard?: (cardId: string) => void;
  /** 时间泳道双击归档卡的定位目标：滚动居中 + 紫边框标记（点卡/改筛选/切页即清） */
  highlightTaskId?: string | null;
  /** 任一归档区交互后清除定位态（App 侧清 archiveFocus） */
  onClearHighlight?: () => void;
}) {
  const [query, setQuery] = useState("");
  const [activeTag, setActiveTag] = useState<string | null>(null);
  const [consolidateOpen, setConsolidateOpen] = useState(false);
  const locateRef = useRef<HTMLDivElement | null>(null);

  const archived = tasks.filter(isArchived);

  // 跳转定位：目标卡被当前搜索/标签筛选隐藏时清掉对应筛选，保证卡可见。
  // 用渲染期派生调整（React 推荐模式）而非 effect——effect 里 setState 会级联渲染；
  // 清筛选后的滚动由下方 effect 在下一轮 commit 补跑
  const target = highlightTaskId
    ? archived.find((x) => x.id === highlightTaskId)
    : undefined;
  const q = query.trim().toLowerCase();
  const targetHidden =
    !!target &&
    ((q !== "" && !`${target.title} ${target.note ?? ""}`.toLowerCase().includes(q)) ||
      (activeTag !== null && !(target.tags ?? []).includes(activeTag)));
  const [prevFocus, setPrevFocus] = useState<string | null>(null);
  const focusKey = highlightTaskId ?? null;
  if (focusKey !== prevFocus) {
    setPrevFocus(focusKey);
    if (focusKey && targetHidden) {
      setQuery("");
      setActiveTag(null);
    }
  }
  useEffect(() => {
    if (highlightTaskId) {
      locateRef.current?.scrollIntoView({ behavior: "smooth", block: "center" });
    }
  }, [highlightTaskId, query, activeTag]);

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
      }),
    );
  }, [archived, query, activeTag]);

  /** 归档卡片默认折叠（collapsed 为 false 时说明用户已展开，保持展开）；
   *  仅未定义时产副本打默认值——其余复用引用，保 memo/引用相等 */
  const displayTask = (t: Task): Task =>
    t.collapsed !== undefined ? t : { ...t, collapsed: true };

  return (
    <div>
      {/* 搜索 + 整理标签按钮（同一行） */}
      {archived.length > 0 && (
        <div className="flex items-center gap-2">
          <input
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              onClearHighlight?.();
            }}
            placeholder="搜索归档内容…"
            className="nm-inset flex-1 rounded-xl px-4 py-2.5 text-sm text-[var(--t2)] outline-none"
          />
          <button
            type="button"
            onClick={() => setConsolidateOpen(true)}
            className="nm-btn shrink-0 px-3 py-2.5 text-xs text-[var(--t3)] flex items-center gap-1.5 whitespace-nowrap"
            title="整理归档卡的相似标签"
          >
            <Tags size={12} aria-hidden />
            整理标签
          </button>
        </div>
      )}

      {/* 标签计数（点击筛选，再点取消）—— 默认折叠，按需展开 */}
      {tagCounts.length > 0 && (
        <details className="mt-3 group">
          <summary className="text-[11px] text-[var(--t5)] cursor-pointer select-none hover:text-[var(--t3)] flex items-center gap-1.5">
            <span className="inline-block transition-transform group-open:rotate-90">
              ▶
            </span>
            标签（{tagCounts.length}）{activeTag && (
              <span className="ml-2 text-[var(--t2)]">
                · 当前筛选 # {activeTag}
                <button
                  type="button"
                  className="ml-1 text-[var(--t5)] hover:text-[var(--t2)]"
                  onClick={(e) => {
                    e.preventDefault();
                    setActiveTag(null);
                  }}
                >
                  ×
                </button>
              </span>
            )}
          </summary>
          <div className="mt-2 flex flex-wrap gap-1.5">
            {tagCounts.map(([tag, count]) => (
              <button
                key={tag}
                className={`px-2.5 py-1 text-xs rounded-full transition-colors ${
                  activeTag === tag
                    ? "nm-inset text-[var(--t1)] font-medium"
                    : "nm-outset text-[var(--t4)] hover:text-[var(--t2)]"
                }`}
                onClick={() => {
                  setActiveTag(activeTag === tag ? null : tag);
                  onClearHighlight?.();
                }}
              >
                # {tag}
                <span className="ml-1 tabular-nums text-[var(--t5)]">
                  {count}
                </span>
              </button>
            ))}
          </div>
        </details>
      )}

      {archived.length === 0 ? (
        <EmptyState
          icon={<Archive size={18} aria-hidden />}
          title="暂无归档内容"
          description={`任务完成后满 ${getArchiveAfterDays()} 天会自动归档到这里`}
        />
      ) : filtered.length === 0 ? (
        <p className="py-10 text-center text-sm text-[var(--t5)]">
          没有匹配的归档
        </p>
      ) : (
        <div className="mt-4 grid grid-cols-3 gap-4 items-start">
          {filtered.map((t) => {
            const located = t.id === highlightTaskId;
            return (
              <div
                key={t.id}
                data-task-id={t.id}
                ref={located ? locateRef : undefined}
                className={located ? "archive-locate" : undefined}
                onClick={onClearHighlight}
              >
                <TodoCard
                  task={displayTask(t)}
                  onUpdate={onUpdate}
                  onDelete={onDelete}
                  onOpenSubtaskCard={onOpenSubtaskCard}
                  archived
                />
              </div>
            );
          })}
        </div>
      )}

      {consolidateOpen && (
        <TagConsolidateModal onClose={() => setConsolidateOpen(false)} />
      )}
    </div>
  );
}
