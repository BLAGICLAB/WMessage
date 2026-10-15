// ChatPanel 子模块：UI hooks（拆分自 ChatPanel.tsx，实现逐字搬移，行为等价）。
// 只收「纯视图态」效果——数据流/流式事件监听仍留在 ChatPanel.tsx（orchestrator）。

import { useEffect, useRef, type RefObject } from "react";

/** 点击下拉区域外关闭：按钮与下拉不在同一个 ref 容器里，需同时检测两者（双 ref）。
 *  open 为 false 时不挂监听。close/refs 仅在订阅时捕获（经 ref 转发取最新值），
 *  订阅节拍与拆分前一致——只在 open 翻转时挂/摘，不随父级每帧 render 抖动 */
export function useOutsideClose(
  open: boolean,
  close: () => void,
  ...refs: RefObject<HTMLElement | null>[]
): void {
  const closeRef = useRef(close);
  const refsRef = useRef(refs);
  useEffect(() => {
    closeRef.current = close;
    refsRef.current = refs;
  });
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (refsRef.current.every((r) => !r.current?.contains(t))) {
        closeRef.current();
      }
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open]);
}

/** 输入卡 textarea 自动增高：随内容长到 max-h-40 后内部滚动；发送清空后缩回。
 *  dep = 输入内容（内容变化时重算高度） */
export function useAutoGrow(
  taRef: RefObject<HTMLTextAreaElement | null>,
  dep: string,
): void {
  useEffect(() => {
    const ta = taRef.current;
    if (!ta) return;
    ta.style.height = "auto";
    ta.style.height = `${ta.scrollHeight}px`;
  }, [dep, taRef]);
}
