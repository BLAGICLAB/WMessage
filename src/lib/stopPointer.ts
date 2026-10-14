import type React from "react";

/** 阻止 pointer 事件冒泡到卡片/浮层的拖拽与收起层（卡内按钮共用）。 */
export function stopPointer(e: React.PointerEvent): void {
  e.stopPropagation();
}
