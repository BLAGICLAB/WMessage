// 任务卡分页器：映射任务页任务池——本周 7 天各一页 + 未排期池一页，共 8 页。
// 联动：周图/日程指向某天时自动翻到该页；页内任务行 hover 上抛（图谱节点
// 同步高亮）；‹ › 手动翻页；点行跳任务页编辑态。

import { useEffect, useState } from "react";
import { ChevronLeft, ChevronRight, Inbox } from "lucide-react";
import { plannedTasksByDay, unplannedTasks } from "./homeData";
import { planColorVar, parsePlanDT, fmtMin } from "../TaskTimeline/week";
import type { Task } from "../../types";

const PAGE_COUNT = 8;

export function PoolPager({
  tasks,
  now,
  activeDay,
  onActiveDay,
  onHoverTask,
  onOpenTask,
  onMore,
}: {
  tasks: Task[];
  now: Date;
  activeDay: number | null;
  onActiveDay: (day: number | null) => void;
  onHoverTask: (task: Task | null) => void;
  onOpenTask: (t: Task) => void;
  onMore: () => void;
}) {
  const { days, keys } = plannedTasksByDay(tasks, now);
  const unplanned = unplannedTasks(tasks);
  // 联动翻页：外部指向某天时显示该天页；无联动时用本地页码
  const [localPage, setLocalPage] = useState(0);
  const page = activeDay !== null ? activeDay : localPage;
  useEffect(() => {
    setLocalPage((p) => Math.min(p, PAGE_COUNT - 1));
  }, []);
  const setPage = (p: number) => {
    const wrapped = (p + PAGE_COUNT) % PAGE_COUNT;
    setLocalPage(wrapped);
    onActiveDay(wrapped <= 6 ? wrapped : null);
  };

  const label = (p: number) => {
    if (p <= 6) {
      const d = new Date(keys[p]);
      return `${["周一", "周二", "周三", "周四", "周五", "周六", "周日"][p]} ${d.getMonth() + 1}/${d.getDate()}`;
    }
    return "未排期池";
  };
  const pageTasks: Task[] = page <= 6 ? days[page] : unplanned;

  return (
    <section className="nm-card flex h-full flex-col p-4" aria-label="任务卡">
      <div className="flex items-center gap-2">
        <span
          aria-hidden
          className="h-[7px] w-[7px] shrink-0 rounded-full"
          style={{ background: "var(--plan-amber)" }}
        />
        <h3 className="text-[13px] font-semibold text-[var(--t3)]">任务卡</h3>
        <span
          className="num ml-auto rounded-md bg-[var(--inset-bg)] px-1.5 py-0.5 text-[10.5px] text-[var(--t4)]"
          data-testid="pool-pager-counter"
        >
          {page + 1}/{PAGE_COUNT}
        </span>
        <button
          onClick={onMore}
          className="rounded-md px-2 py-0.5 text-xs text-[var(--t4)] transition-colors hover:bg-[var(--hover-bg)] hover:text-[var(--t2)]"
        >
          任务页
        </button>
      </div>

      <div className="mt-2 flex items-center justify-between">
        <button
          onClick={() => setPage(page - 1)}
          aria-label="上一页"
          className="rounded-md border border-[var(--edge)] px-1.5 py-0.5 text-[var(--t4)] transition-colors hover:bg-[var(--hover-bg)] hover:text-[var(--t2)]"
        >
          <ChevronLeft size={13} aria-hidden />
        </button>
        <span className="text-xs font-medium text-[var(--t2)]" data-testid="pool-page-label">
          {label(page)}
        </span>
        <button
          onClick={() => setPage(page + 1)}
          aria-label="下一页"
          className="rounded-md border border-[var(--edge)] px-1.5 py-0.5 text-[var(--t4)] transition-colors hover:bg-[var(--hover-bg)] hover:text-[var(--t2)]"
        >
          <ChevronRight size={13} aria-hidden />
        </button>
      </div>

      <div className="mt-2 min-h-0 flex-1 space-y-1 overflow-y-auto" data-testid="pool-page-list">
        {pageTasks.length === 0 ? (
          <div className="flex h-full flex-col items-center justify-center gap-2 py-4 text-center">
            <Inbox size={18} className="text-[var(--t5)]" aria-hidden />
            <p className="text-[12.5px] text-[var(--t5)]">
              {page <= 6 ? "这一天没有排期任务。" : "任务池是空的——新建的任务会先落到这里。"}
            </p>
          </div>
        ) : (
          pageTasks.map((t) => {
            const seg = t.planStart ? parsePlanDT(t.planStart) : null;
            return (
              <button
                key={t.id}
                type="button"
                onClick={() => onOpenTask(t)}
                onMouseEnter={() => onHoverTask(t)}
                onMouseLeave={() => onHoverTask(null)}
                className="flex w-full items-center gap-2 rounded-md border border-[var(--edge)] px-2 py-1.5 text-left transition-colors hover:border-[var(--edge-strong)] hover:bg-[var(--hover-bg)]"
              >
                <span
                  aria-hidden
                  className="h-4 w-1 shrink-0 rounded-full"
                  style={{ background: planColorVar(t.tags) }}
                />
                <span
                  className={`min-w-0 flex-1 truncate text-[12.5px] text-[var(--t2)] ${
                    t.completedAt ? "line-through opacity-60" : ""
                  }`}
                >
                  {t.title}
                </span>
                {seg && (
                  <span className="num shrink-0 text-[11px] text-[var(--t5)]">
                    {fmtMin(seg.min)}
                  </span>
                )}
              </button>
            );
          })
        )}
      </div>
    </section>
  );
}
