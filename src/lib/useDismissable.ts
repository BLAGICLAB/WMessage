import { useEffect, useRef } from "react";
import type { RefObject } from "react";

/** 自绘浮层两点收起：mousedown 落在 rootRef 外或按 Esc → 收起。
 *  active=false 不挂监听；onClose 每次渲染重建也没关系（走 ref，
 *  监听器只在 open 翻转时重挂，与原各处手写 effect 同生命周期）。 */
export function useDismissable(
  active: boolean,
  rootRef: RefObject<HTMLElement | null>,
  onClose: () => void,
  /** Esc 路径的收起动作；缺省同 onClose。用于收起外还要做事的场景
   *  （如 ApiProviderSelect 把焦点归还触发钮——点外部收起不归还） */
  onEscape?: () => void,
): void {
  const closeRef = useRef(onClose);
  const escapeRef = useRef(onEscape ?? onClose);
  useEffect(() => {
    closeRef.current = onClose;
    escapeRef.current = onEscape ?? onClose;
  });
  useEffect(() => {
    if (!active) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node))
        closeRef.current();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") escapeRef.current();
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [active, rootRef]);
}
