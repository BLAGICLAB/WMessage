// 即将到期横条：每日一图下方的单行小组件，高度填满与周统计卡之间的缺口。
// 一行只显一条最近到期的任务，多条时每 4s 轮播（悬停暂停；reduced-motion
// 下只换内容不做位移动画）；空态显示提示文案。点行跳任务页编辑态。

import { useEffect, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import type { Task } from "../../types";
import { dueChipOf, dueSoonTasks } from "./homeData";
import { planColorVar } from "../TaskTimeline/week";

const ROTATE_MS = 4_000;

export function DueTicker({
  tasks,
  now,
  onOpenTask,
}: {
  tasks: Task[];
  now: Date;
  onOpenTask: (t: Task) => void;
}) {
  const reduceMotion = useReducedMotion();
  const due = dueSoonTasks(tasks, now);
  const [idx, setIdx] = useState(0);
  const [paused, setPaused] = useState(false);
  const safeIdx = due.length ? idx % due.length : 0;

  useEffect(() => {
    if (due.length < 2 || paused) return;
    const iv = setInterval(() => setIdx((i) => (i + 1) % due.length), ROTATE_MS);
    return () => clearInterval(iv);
  }, [due.length, paused]);

  const current = due[safeIdx];

  return (
    <section
      className="nm-card flex h-full min-h-[44px] items-center gap-2.5 px-4"
      aria-label="即将到期"
      data-testid="due-ticker"
      onMouseEnter={() => setPaused(true)}
      onMouseLeave={() => setPaused(false)}
    >
      <span className="shrink-0 text-xs font-semibold text-[var(--t4)]">
        即将到期
      </span>
      {current ? (
        <>
          {due.length > 1 && (
            <span
              className="num shrink-0 rounded-md bg-[var(--inset-bg)] px-1.5 py-0.5 text-[10.5px] text-[var(--t4)]"
              title={`共 ${due.length} 条`}
            >
              {safeIdx + 1}/{due.length}
            </span>
          )}
          <button
            type="button"
            onClick={() => onOpenTask(current)}
            className="flex min-w-0 flex-1 items-center gap-2 text-left"
          >
            <AnimatePresence mode="popLayout" initial={false}>
              <motion.span
                key={current.id}
                className="flex min-w-0 flex-1 items-center gap-2"
                initial={reduceMotion ? false : { opacity: 0, y: 8 }}
                animate={{ opacity: 1, y: 0 }}
                exit={reduceMotion ? { opacity: 0 } : { opacity: 0, y: -8 }}
                transition={{ duration: 0.25 }}
              >
                <span
                  aria-hidden
                  className="h-[7px] w-[7px] shrink-0 rounded-full"
                  style={{ background: planColorVar(current.tags) }}
                />
                <span className="min-w-0 flex-1 truncate text-[13px] text-[var(--t2)]">
                  {current.title}
                </span>
              </motion.span>
            </AnimatePresence>
          </button>
          {(() => {
            const chip = dueChipOf(current, now);
            return (
              <span
                className={`num shrink-0 rounded-md border px-1.5 py-0.5 text-[11px] ${
                  chip.danger
                    ? "border-[color-mix(in_srgb,var(--danger)_40%,transparent)] font-semibold text-[var(--danger)]"
                    : "border-[var(--edge)] text-[var(--t4)]"
                }`}
              >
                {chip.text}
              </span>
            );
          })()}
        </>
      ) : (
        <span className="min-w-0 flex-1 truncate text-[13px] text-[var(--t5)]">
          未来 7 天没有要撞线的任务。
        </span>
      )}
    </section>
  );
}
