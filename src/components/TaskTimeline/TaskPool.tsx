import { useState } from "react";
import type { ColumnId, Task } from "../../types";
import { planChip, planColorVar, sortPoolTasks } from "./week";
import { DoneCircle } from "../DoneCircle";
import { useHoverTitleTip } from "./HoverTitle";

const OPEN_KEY = "wm-task-pool-open";

/** 任务池开合状态（localStorage 记忆）——页面持有，页头开关与池内 ⇥ 共用 */
export function loadPoolOpen(): boolean {
  return localStorage.getItem(OPEN_KEY) !== "0";
}
export function savePoolOpen(open: boolean) {
  localStorage.setItem(OPEN_KEY, open ? "1" : "0");
}

/** 任务池（毛玻璃悬浮层）：列出全部未完成任务的统一标题块。
 *  块结构（惯例：整块=拖拽面）：[色条|标题] [完成圆圈]——圆圈快速切换完成态，
 *  双击块开详情面板（单击/拖拽不开，防误触）。
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
  /** 双击池项开详情面板 */
  onEdit?: (task: Task) => void;
  innerRef?: React.Ref<HTMLElement>;
  /** 拖拽离池：整池滑出让出视野（回池内滑回） */
  evaded?: boolean;
  /** 拖拽中的源块 id（池内悬浮态） */
  liftedId?: string | null;
}) {
  const [showDone, setShowDone] = useState(false);
  const tip = useHoverTitleTip();

  const sorted = sortPoolTasks(tasks);
  const hidden = !open;
  const itemCls = (t: Task) =>
    `flex h-[34px] touch-none items-center gap-2 rounded-lg border pr-1.5 ${
      liftedId === t.id
        ? "pool-lift relative z-10"
        : "hover:border-[var(--edge-strong)]"
    }`;
  // 对勾上的双击 = 两次完成切换，不开面板（click 已 stopPropagation，
  // 但 dblclick 是独立事件仍会冒泡到块身）
  const openOnDblClick = (t: Task) => (e: React.MouseEvent) => {
    if ((e.target as HTMLElement).closest("button")) return;
    onEdit?.(t);
  };
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
        tip.hide();
        onItemPointerDown?.(t, e);
      }}
      onDoubleClick={openOnDblClick(t)}
    >
      <span
        className="h-full w-[3px] shrink-0 rounded-l-lg"
        style={{ background: planColorVar(t.tags) }}
        aria-hidden
      />
      <span
        className="min-w-0 flex-1 truncate text-xs text-[var(--t2)]"
        onPointerEnter={tip.show(t.title)}
        onPointerLeave={tip.hide}
        onPointerDown={tip.hide}
      >
        {t.title}
      </span>
      {t.planStart && (
        <span className="num shrink-0 rounded-full bg-[var(--inset-bg)] px-1.5 py-px text-[10px] text-[var(--t5)]">
          {planChip(t.planStart)}
        </span>
      )}
      <DoneCircle
        done={false}
        onToggle={() => onSetColumn?.(t.id, "done")}
        title="标记完成"
      />
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
        transitionTimingFunction: "var(--ease-out)",
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
                  onDoubleClick={openOnDblClick(t)}
                >
                  <span
                    className="min-w-0 flex-1 truncate text-xs text-[var(--t2)]"
                    onPointerEnter={tip.show(t.title)}
                    onPointerLeave={tip.hide}
                  >
                    {t.title}
                  </span>
                  <DoneCircle
                    done
                    onToggle={() => onSetColumn?.(t.id, "todo")}
                    title="恢复待办"
                  />
                </div>
              ))}
          </div>
        )}
      </div>
      {tip.chip}
    </aside>
  );
}
