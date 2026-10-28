import { useEffect, useRef } from "react";
import type { RefObject } from "react";

/** 自绘浮层收起：mousedown 落在 refs 全部容器外 → 收起；传 onEscape 时 Esc
 *  也收起（不传则不挂键盘监听——Esc 层级仲裁归调用方，挂了会抢走别的 Esc 语义）。
 *  按钮与下拉分属两个 ref 容器时把两者都放进 refs。
 *  active=false 不挂监听；onClose/refs 每次渲染重建也没关系（走 ref，
 *  监听器只在 active 翻转时重挂，不随父级每帧 render 抖动）。 */
export function useDismissable(
  active: boolean,
  onClose: () => void,
  refs: RefObject<HTMLElement | null>[],
  onEscape?: () => void,
): void {
  const closeRef = useRef(onClose);
  const refsRef = useRef(refs);
  const escapeRef = useRef(onEscape);
  useEffect(() => {
    closeRef.current = onClose;
    refsRef.current = refs;
    escapeRef.current = onEscape;
  });
  useEffect(() => {
    if (!active) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (refsRef.current.every((r) => !r.current?.contains(t)))
        closeRef.current();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") escapeRef.current?.();
    };
    const hasEsc = escapeRef.current != null;
    document.addEventListener("mousedown", onDown);
    if (hasEsc) document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      if (hasEsc) document.removeEventListener("keydown", onKey);
    };
    // close/refs/escape 经 ref 转发取最新值，监听只随 active 翻转重挂
    // eslint-disable-next-line react/exhaustive-deps
  }, [active]);
}
