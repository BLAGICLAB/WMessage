// ChatPanel 子模块：UI hooks（拆分自 ChatPanel.tsx，实现逐字搬移，行为等价）。
// 只收「纯视图态」效果——数据流/流式事件监听仍留在 ChatPanel.tsx（orchestrator）。

import { useEffect, type RefObject } from "react";

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
