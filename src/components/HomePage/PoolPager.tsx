// 任务卡分页器：映射任务页任务池——本周 7 天各一页 + 未排期池一页，共 8 页。
// 页码三层模型：默认页 = 今日（跨天自动跟随）；联动悬停与 ‹ › 手动翻页都是
// 临时偏移——恢复触发 = 指针离开本组件区域，或联动结束且指针不在本组件内
// （行间移动指针时组件内不触发）。页内任务行 hover 上抛（图谱节点同步高亮）；
// 点行跳任务页编辑态。

import { useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { ChevronLeft, ChevronRight, Inbox } from "lucide-react";
import { dayKey, plannedTasksByDay, unplannedTasks } from "./homeData";
import { planColorVar, parsePlanDT, fmtMin } from "../TaskTimeline/week";
import type { Task } from "../../types";

const PAGE_COUNT = 8;

export function PoolPager({
  tasks,
  now,
  activeDay,
  activeTaskId,
  onHoverTask,
  onOpenTask,
  onMore,
}: {
  tasks: Task[];
  now: Date;
  activeDay: number | null;
  activeTaskId: string | null;
  onHoverTask: (task: Task | null) => void;
  onOpenTask: (t: Task) => void;
  onMore: () => void;
}) {
  const reduceMotion = useReducedMotion();
  const { days, keys } = plannedTasksByDay(tasks, now);
  const unplanned = unplannedTasks(tasks);
  const todayIdx = Math.max(0, keys.indexOf(dayKey(now)));
  // null = 停在默认页（今日）；数字 = 用户手动翻到的临时页
  const [tempPage, setTempPage] = useState<number | null>(null);
  // 指针是否在本组件内：行间移动会短暂清掉共享 hover（activeDay→null），
  // 指针仍在组件内时不得触发恢复，否则翻页后在行间移动就弹回今日
  const pointerInRef = useRef(false);
  const page = activeDay !== null ? activeDay : tempPage ?? todayIdx;
  // 联动结束（activeDay 非空 → null）且指针不在本组件内：恢复今日
  useEffect(() => {
    if (activeDay === null && !pointerInRef.current) setTempPage(null);
  }, [activeDay]);
  const setPage = (p: number) => {
    setTempPage((p + PAGE_COUNT) % PAGE_COUNT);
  };
  const restore = () => {
    pointerInRef.current = false;
    setTempPage(null);
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
    <section
      className="nm-card flex h-full flex-col p-4"
      aria-label="任务卡"
      onMouseEnter={() => {
        pointerInRef.current = true;
      }}
      onMouseLeave={restore}
    >
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

      {/* 页内容淡入淡出：联动翻页/恢复今日的切换不生硬（reduced-motion 直切） */}
      <div className="mt-2 min-h-0 flex-1 overflow-y-auto" data-testid="pool-page-list">
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.div
            key={page}
            className="space-y-1"
            initial={reduceMotion ? false : { opacity: 0, y: 6 }}
            animate={{ opacity: 1, y: 0 }}
            exit={reduceMotion ? { opacity: 0 } : { opacity: 0, y: -6 }}
            transition={{ duration: 0.18 }}
          >
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
            const rowActive = activeTaskId === t.id;
            return (
              <button
                key={t.id}
                type="button"
                onClick={() => onOpenTask(t)}
                onMouseEnter={() => onHoverTask(t)}
                onMouseLeave={() => onHoverTask(null)}
                data-active={rowActive || undefined}
                className={`flex w-full items-center gap-2 rounded-md border px-2 py-1.5 text-left transition-colors ${
                  rowActive
                    ? "border-[var(--accent)] bg-[color-mix(in_srgb,var(--accent)_10%,transparent)]"
                    : "border-[var(--edge)] hover:border-[var(--edge-strong)] hover:bg-[var(--hover-bg)]"
                }`}
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
          </motion.div>
        </AnimatePresence>
      </div>
    </section>
  );
}
