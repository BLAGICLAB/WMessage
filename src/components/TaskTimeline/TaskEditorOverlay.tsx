import { useEffect, useRef, useState } from "react";
import type { ColumnId, Task } from "../../types";
import { TaskDetailPanel } from "./TaskDetailPanel";

/** 浮窗左缘 = 导航栏 w-52(208px) + 4px 间距；滑出屏幕的位移要同时
 *  越过自身宽度(-100%)和左缘偏移，两个值都从 LEFT 推导保持同步 */
const LEFT = 212;
const SLIDE_MS = 240;
/** 退场卸载兜底：略大于 SLIDE_MS（transitionend 事件在 jsdom/降级路径不触发） */
const EXIT_MS = 280;

/** 全局任务编辑浮窗（毛玻璃上层窗）：fixed 悬浮在当前页面之上，从左滑入、
 *  向左滑出，任何导航页都可打开（新建/编辑统一入口）。
 *  纯展示宿主：开合动画由 closing 驱动（transform 过渡），退场完成后回调
 *  onClosed 由接线方卸载；任务切换（closing=false 期间换 id）原位换内容不重播动画。 */
export function TaskEditorOverlay({
  task,
  autoFocusTitle = false,
  closing = false,
  onClosed,
  onClose,
  onUpdate,
  onSetColumn,
  onDelete,
  onCreateSubtaskCard,
  onOpenTask,
  onDeleteSubtask,
}: {
  task: Task;
  /** 新建任务打开时聚焦标题并全选（自动进入编辑态） */
  autoFocusTitle?: boolean;
  /** true = 滑出退场中（结束后回调 onClosed 一次） */
  closing?: boolean;
  onClosed: () => void;
  onClose: () => void;
  onUpdate: (patch: Partial<Task>) => void;
  onSetColumn: (col: ColumnId) => void;
  onDelete: () => void;
  onCreateSubtaskCard?: (subId: string, title: string) => void;
  onOpenTask?: (id: string) => void;
  onDeleteSubtask?: (subId: string) => void;
}) {
  // 挂载帧先落在屏外（translateX 隐藏位），下一帧再滑入——首帧不闪现
  const [entered, setEntered] = useState(false);
  useEffect(() => {
    const raf = requestAnimationFrame(() => setEntered(true));
    return () => cancelAnimationFrame(raf);
  }, []);
  // onClosed 走 ref：接线方每次渲染换函数引用也不重置退场定时器
  // （先于 closing 定时器 effect 声明，保证定时器读到的是最新回调）
  const closedRef = useRef(onClosed);
  useEffect(() => {
    closedRef.current = onClosed;
  }, [onClosed]);
  useEffect(() => {
    if (!closing) return;
    const t = setTimeout(() => closedRef.current(), EXIT_MS);
    return () => clearTimeout(t);
  }, [closing]);

  const shown = entered && !closing;
  return (
    <div
      className="fixed inset-y-3 z-40 w-[300px]"
      style={{
        left: LEFT,
        transform: shown ? "translateX(0)" : `translateX(calc(-100% - ${LEFT}px))`,
        opacity: shown ? 1 : 0,
        transition: `transform ${SLIDE_MS}ms cubic-bezier(0.05, 0.7, 0.1, 1), opacity 180ms ease`,
      }}
    >
      <TaskDetailPanel
        task={task}
        autoFocusTitle={autoFocusTitle}
        onUpdate={onUpdate}
        onSetColumn={onSetColumn}
        onDelete={onDelete}
        onClose={onClose}
        onCreateSubtaskCard={onCreateSubtaskCard}
        onOpenTask={onOpenTask}
        onDeleteSubtask={onDeleteSubtask}
      />
    </div>
  );
}
