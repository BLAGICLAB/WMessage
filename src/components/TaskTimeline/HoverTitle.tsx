import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

/** 悬停完整标题提示：静置 350ms 显示（原生 title 心智），portal 到 body
 *  防 overflow 裁切（池/网格井都 overflow-hidden）。拖拽开始即隐藏。
 *  show/hide 绑到会截断的元素上；chip 定位在元素上方，横向钳制视口内。 */
export function useHoverTitleTip() {
  const [tip, setTip] = useState<{ text: string; x: number; y: number } | null>(
    null,
  );
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );

  const show = (text: string) => (e: React.PointerEvent) => {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(
      () => setTip({ text, x: r.left, y: r.top }),
      350,
    );
  };
  const hide = () => {
    if (timer.current) clearTimeout(timer.current);
    setTip(null);
  };

  const chip = tip
    ? createPortal(
        <div
          className="hover-tip"
          style={{
            left: Math.max(8, Math.min(tip.x, window.innerWidth - 280)),
            top: tip.y - 6,
          }}
        >
          {tip.text}
        </div>,
        document.body,
      )
    : null;

  return { show, hide, chip };
}
