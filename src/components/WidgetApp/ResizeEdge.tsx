// WidgetApp 子模块：四边拖拽热区（老板拍板 A+C 风格）。
//
// 平时 1px 暗示线 + 拖动时高亮（opacity 0.3 → 0.8 → 1.0）。

import { getCurrentWindow } from "@tauri-apps/api/window";

import { setWidgetDragActive } from "./storage";

export function ResizeEdge({
  side,
  onDelta,
}: {
  side: "n" | "s" | "e" | "w";
  onDelta: (
    dx: number,
    dy: number,
    startPos: { x: number; y: number; w: number; h: number; sw: number; sh: number }
  ) => void;
}) {
  const isH = side === "n" || side === "s";
  const pos =
    side === "n"
      ? "top-0 left-0 right-0 h-2 cursor-n-resize"
      : side === "s"
      ? "bottom-0 left-0 right-0 h-2 cursor-s-resize"
      : side === "e"
      ? "top-0 right-0 bottom-0 w-2 cursor-e-resize"
      : "top-0 left-0 bottom-0 w-2 cursor-w-resize";
  const linePos = isH
    ? "left-0 right-0 top-1/2 -translate-y-1/2 h-px"
    : "top-0 bottom-0 left-1/2 -translate-x-1/2 w-px";

  const onDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const sx = e.clientX, sy = e.clientY;
    const el = e.currentTarget as HTMLElement;
    try { el.setPointerCapture(e.pointerId); } catch { /* ignore */ }
    // 起始外框位置/尺寸/屏幕尺寸(异步读取;未就绪前的 move 一律忽略)
    let startPos: { x: number; y: number; w: number; h: number; sw: number; sh: number } | null = null;
    const win = getCurrentWindow();
    void Promise.all([
      win.outerPosition(),
      win.outerSize(),
      win.scaleFactor(),
      import("./storage").then((s) => s.screenSize()),
    ]).then(([p, s, sc, ss]) => {
      startPos = {
        x: p.x / sc,
        y: p.y / sc,
        w: s.width / sc,
        h: s.height / sc,
        sw: ss.w,
        sh: ss.h,
      };
    });
    setWidgetDragActive(true);
    // 监听器同步注册(不等异步读取):避免快速点按时漏掉 pointerup 造成监听残留
    const move = (ev: PointerEvent) => {
      const sp = startPos;
      if (!sp) return;
      onDelta(ev.clientX - sx, ev.clientY - sy, sp);
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
    <div className={`absolute z-30 ${pos} bg-transparent`} onPointerDown={onDown}>
      <div className={`absolute ${linePos} bg-[var(--edge)] opacity-30 hover:opacity-80 active:opacity-100 transition-opacity`} />
    </div>
  );
}
