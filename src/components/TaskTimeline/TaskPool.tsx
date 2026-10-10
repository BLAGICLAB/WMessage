import { useEffect, useState } from "react";
import type { Task } from "../../types";
import { planChip, planColorVar } from "./week";

const OPEN_KEY = "wm-task-pool-open";

/** 池内排序：已排期按 planStart 升序在前，未排期按 updatedAt 降序在后 */
export function sortPoolTasks(tasks: Task[]): Task[] {
  return [...tasks].sort((a, b) => {
    if (a.planStart && b.planStart) return a.planStart.localeCompare(b.planStart);
    if (a.planStart) return -1;
    if (b.planStart) return 1;
    return (b.updatedAt ?? 0) - (a.updatedAt ?? 0);
  });
}

/** 任务池（毛玻璃悬浮层）：列出全部未完成任务的统一标题块，可折叠成右缘把手。
 *  onItemPointerDown 供页面发起「池→网格」拖拽；innerRef 供页面做「拖回池清除」
 *  落点判定；doneTasks 折叠在池底「已完成 N」里（点开可选中进详情面板）；
 *  faded = 拖拽进行中让出视野（从池拖出时淡出，拖完淡回）。 */
export function TaskPool({
  tasks,
  doneTasks = [],
  onItemPointerDown,
  onTaskClick,
  innerRef,
  faded = false,
}: {
  tasks: Task[];
  doneTasks?: Task[];
  onItemPointerDown?: (task: Task, e: React.PointerEvent) => void;
  onTaskClick?: (task: Task) => void;
  innerRef?: React.Ref<HTMLElement>;
  faded?: boolean;
}) {
  const [showDone, setShowDone] = useState(false);
  const fadeCls = faded ? "opacity-0 pointer-events-none" : "opacity-100";
  const [open, setOpen] = useState(() => localStorage.getItem(OPEN_KEY) !== "0");
  useEffect(() => {
    localStorage.setItem(OPEN_KEY, open ? "1" : "0");
  }, [open]);

  const sorted = sortPoolTasks(tasks);
  if (!open) {
    return (
      <button
        type="button"
        aria-label={`展开任务池（${sorted.length} 个未完成）`}
        onClick={() => setOpen(true)}
        className={`glass absolute top-1/2 right-0 z-10 flex -translate-y-1/2 flex-row-reverse items-center gap-2 rounded-r-none border-r-0 px-[7px] py-3.5 text-xs tracking-[0.08em] text-[var(--t3)] transition-opacity duration-200 ${fadeCls}`}
      >
        任务池
        <span className="num inline-flex h-4 min-w-4 items-center justify-center rounded-full bg-[var(--accent)] px-1 text-[10px] font-semibold text-white dark:text-[#101228]">
          {sorted.length}
        </span>
      </button>
    );
  }
  return (
    <aside
      ref={innerRef}
      aria-label="任务池"
      className={`glass absolute top-3 right-3 bottom-3 z-10 flex w-64 flex-col overflow-hidden transition-opacity duration-200 ${fadeCls}`}
    >
      <div className="flex items-center gap-2 border-b border-[color-mix(in_srgb,var(--edge)_70%,transparent)] px-3.5 pt-3 pb-2.5">
        <b className="text-[13px] font-semibold text-[var(--t1)]">任务池</b>
        <span className="num text-[11px] text-[var(--t5)]">{sorted.length} 个未完成</span>
        <button
          type="button"
          aria-label="折叠任务池"
          onClick={() => setOpen(false)}
          className="nm-icon-btn ml-auto h-6 w-6 text-xs"
        >
          ⇥
        </button>
      </div>
      <div className="flex min-h-0 flex-1 flex-col gap-1.5 overflow-y-auto p-2.5">
        {sorted.length === 0 && doneTasks.length === 0 ? (
          <p className="my-auto text-center text-xs text-[var(--t5)]">
            没有未完成的任务，⌘N 新建
          </p>
        ) : (
          sorted.map((t) => (
            <div
              key={t.id}
              className="flex h-[34px] touch-none items-center gap-2 rounded-lg border pr-2.5"
              style={{
                borderColor: "color-mix(in srgb, var(--edge) 80%, transparent)",
                background: "color-mix(in srgb, var(--surface-raised) 72%, transparent)",
              }}
              onPointerDown={(e) => {
                if (e.button !== 0) return;
                onItemPointerDown?.(t, e);
              }}
            >
              <span
                className="h-full w-[3px] shrink-0 rounded-l-lg"
                style={{ background: planColorVar(t.tags) }}
                aria-hidden
              />
              <span className="truncate text-xs text-[var(--t2)]">{t.title}</span>
              {t.planStart && (
                <span className="num ml-auto shrink-0 rounded-full bg-[var(--inset-bg)] px-1.5 py-px text-[10px] text-[var(--t5)]">
                  {planChip(t.planStart)}
                </span>
              )}
            </div>
          ))
        )}
        {doneTasks.length > 0 && (
          <div className="mt-auto border-t border-[color-mix(in_srgb,var(--edge)_70%,transparent)] pt-1.5">
            <button
              type="button"
              className="flex w-full items-center gap-1 px-1 py-1 text-[11px] text-[var(--t5)] hover:text-[var(--t3)]"
              onClick={() => setShowDone((v) => !v)}
              aria-expanded={showDone}
            >
              {showDone ? "▾" : "▸"} 已完成 {doneTasks.length}
            </button>
            {showDone &&
              doneTasks.map((t) => (
                <button
                  key={t.id}
                  type="button"
                  className="flex h-[30px] w-full items-center gap-2 rounded-lg px-2 text-left hover:bg-[var(--hover-bg)]"
                  onClick={() => onTaskClick?.(t)}
                >
                  <span className="truncate text-xs text-[var(--t5)] line-through">
                    {t.title}
                  </span>
                </button>
              ))}
          </div>
        )}
      </div>
    </aside>
  );
}
