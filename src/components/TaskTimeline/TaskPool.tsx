import { useState } from "react";
import { PenLine } from "lucide-react";
import type { ColumnId, Task } from "../../types";
import { planChip, planColorVar } from "./week";
import { DoneCircle } from "../DoneCircle";

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

/** 任务池开合状态（localStorage 记忆）——页面持有，页头开关与池内 ⇥ 共用 */
export function loadPoolOpen(): boolean {
  return localStorage.getItem(OPEN_KEY) !== "0";
}
export function savePoolOpen(open: boolean) {
  localStorage.setItem(OPEN_KEY, open ? "1" : "0");
}

/** 任务池（毛玻璃悬浮层）：列出全部未完成任务的统一标题块。
 *  块结构（惯例：整块=拖拽面，尾部常驻操作钮）：[色条|标题] [完成圆圈] [✎编辑]——
 *  圆圈快速切换完成态、编辑钮开详情面板，其余区域只拖拽（防误触开编辑窗）。
 *  常驻挂载：开合走 transform+opacity 过渡；evaded = 拖拽离池时整池滑出让出视野；
 *  liftedId = 拖拽中的源块在池内悬浮（升起+光晕）。
 *  onItemPointerDown 供页面发起「池→网格」拖拽；innerRef 供「拖回池清除」落点判定；
 *  doneTasks 折叠在池底「已完成 N」（打勾形式，圆圈可恢复待办）。 */
export function TaskPool({
  tasks,
  doneTasks = [],
  open,
  onToggle,
  onItemPointerDown,
  onSetColumn,
  onEdit,
  innerRef,
  evaded = false,
  liftedId = null,
}: {
  tasks: Task[];
  doneTasks?: Task[];
  open: boolean;
  onToggle: () => void;
  onItemPointerDown?: (task: Task, e: React.PointerEvent) => void;
  onSetColumn?: (taskId: string, col: ColumnId) => void;
  onEdit?: (task: Task) => void;
  innerRef?: React.Ref<HTMLElement>;
  /** 拖拽离池：整池滑出让出视野（回池内滑回） */
  evaded?: boolean;
  /** 拖拽中的源块 id（池内悬浮态） */
  liftedId?: string | null;
}) {
  const [showDone, setShowDone] = useState(false);

  const sorted = sortPoolTasks(tasks);
  const hidden = !open;
  const stopPointer = (e: React.PointerEvent) => e.stopPropagation();
  const itemCls = (t: Task) =>
    `flex h-[34px] touch-none items-center gap-2 rounded-lg border pr-1.5 ${
      liftedId === t.id
        ? "pool-lift relative z-10"
        : "hover:border-[var(--edge-strong)]"
    }`;
  const item = (t: Task) => (
    <div
      key={t.id}
      className={itemCls(t)}
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
      <DoneCircle
        done={false}
        onToggle={() => onSetColumn?.(t.id, "done")}
        title="标记完成"
      />
      <button
        type="button"
        aria-label={`编辑 ${t.title}`}
        title="编辑任务"
        className="nm-icon-btn h-5 w-5 shrink-0 text-[var(--t5)]"
        onPointerDown={stopPointer}
        onClick={(e) => {
          e.stopPropagation();
          onEdit?.(t);
        }}
      >
        <PenLine size={11} aria-hidden />
      </button>
    </div>
  );

  return (
    <aside
      ref={innerRef}
      aria-label="任务池"
      aria-hidden={hidden}
      className={`glass absolute top-3 right-3 bottom-3 z-10 flex w-64 flex-col overflow-hidden ${
        hidden || evaded ? "pointer-events-none opacity-0" : "opacity-100"
      }`}
      style={{
        transform:
          hidden || evaded ? "translateX(calc(100% + 16px))" : "translateX(0)",
        transitionProperty: "transform, opacity",
        transitionDuration: hidden || evaded ? "150ms" : "240ms",
        transitionTimingFunction: hidden || evaded
          ? "cubic-bezier(0.3, 0, 0.8, 0.15)"
          : "cubic-bezier(0.05, 0.7, 0.1, 1)",
      }}
    >
      <div className="flex items-center gap-2 border-b border-[color-mix(in_srgb,var(--edge)_70%,transparent)] px-3.5 pt-3 pb-2.5">
        <b className="text-[13px] font-semibold text-[var(--t1)]">任务池</b>
        <span className="num text-[11px] text-[var(--t5)]">{sorted.length} 个未完成</span>
        <button
          type="button"
          aria-label="折叠任务池"
          onClick={onToggle}
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
          sorted.map(item)
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
                <div
                  key={t.id}
                  className="flex h-[30px] items-center gap-2 rounded-lg px-1.5"
                >
                  <DoneCircle
                    done
                    onToggle={() => onSetColumn?.(t.id, "todo")}
                    title="恢复待办"
                  />
                  <span className="truncate text-xs text-[var(--t2)]">
                    {t.title}
                  </span>
                  <button
                    type="button"
                    aria-label={`编辑 ${t.title}`}
                    title="编辑任务"
                    className="nm-icon-btn ml-auto h-5 w-5 shrink-0 text-[var(--t5)]"
                    onPointerDown={stopPointer}
                    onClick={(e) => {
                      e.stopPropagation();
                      onEdit?.(t);
                    }}
                  >
                    <PenLine size={11} aria-hidden />
                  </button>
                </div>
              ))}
          </div>
        )}
      </div>
    </aside>
  );
}
