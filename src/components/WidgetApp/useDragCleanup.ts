// WidgetApp 子模块：拖动清理登记槽 hook。
//
// mid-drag unmount 兜底：拖动开始时把 cleanup 装入返回的 ref，
// 组件卸载时强制调用（摘 window 监听 + 复位拖动标记 + 释放 pointer capture）。
// ResizeEdge / SplitBar 共用，保证卸载安全不变量只有一份实现。

import { useEffect, useRef } from "react";

export function useDragCleanup() {
  const dragCleanupRef = useRef<(() => void) | null>(null);
  useEffect(() => {
    return () => {
      dragCleanupRef.current?.();
    };
  }, []);
  return dragCleanupRef;
}
