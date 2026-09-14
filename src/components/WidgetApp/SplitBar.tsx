// WidgetApp 子模块：Splitter 双箭头 + 拖拽条（任务区/聊天区分隔）。
//
// ▲ = 聊天区变大(任务区变小)
// ▼ = 任务区变大(聊天区变小)

import { setWidgetDragActive } from "./storage";

export function SplitBar({
  onSplit,
  onArrow,
}: {
  onSplit: (delta: number) => void;
  onArrow: (delta: number) => void;
}) {
  const onDown = (e: React.PointerEvent) => {
    if ((e.target as HTMLElement).closest("button")) return;
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const sy = e.clientY;
    let acc = 0;
    const el = e.currentTarget as HTMLElement;
    try { el.setPointerCapture(e.pointerId); } catch { /* ignore */ }
    setWidgetDragActive(true);
    const move = (ev: PointerEvent) => {
      const dy = ev.clientY - sy;
      const inc = dy - acc;
      acc = dy;
      onSplit(inc);
    };
    const cleanup = (ev: PointerEvent) => {
      try { el.releasePointerCapture(ev.pointerId); } catch { /* ignore */ }
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", cleanup);
      window.removeEventListener("pointercancel", cleanup);
      setWidgetDragActive(false);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", cleanup);
    window.addEventListener("pointercancel", cleanup);
  };

  return (
    <div
      className="relative h-4 shrink-0 flex items-center justify-center cursor-row-resize hover:bg-[var(--hover-bg)] group select-none"
      onPointerDown={onDown}
    >
      <div className="absolute inset-x-0 top-1/2 -translate-y-1/2 h-px bg-[var(--edge)] opacity-40 group-hover:opacity-80" />
      <div className="relative flex items-center gap-3 px-2 py-0.5 rounded-full bg-[var(--bg)] border border-[var(--edge)]">
        <button
          onClick={() => onArrow(-40)}
          title="聊天区变大"
          className="text-[10px] text-[var(--t5)] hover:text-[var(--t2)] leading-none px-0.5"
        >▲</button>
        <button
          onClick={() => onArrow(+40)}
          title="任务区变大"
          className="text-[10px] text-[var(--t5)] hover:text-[var(--t2)] leading-none px-0.5"
        >▼</button>
      </div>
    </div>
  );
}
