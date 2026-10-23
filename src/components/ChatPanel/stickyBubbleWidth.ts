// ChatPanel 子模块：机器人气泡宽度只增不减。
//
// 气泡是「内容自适应」宽度——所在列是 flex 项，会收缩到内容宽度。工具折叠行里
// 「正在跑 xxx」出现就把气泡撑宽，那行在工具跑完后变短又缩回去，同一条回复在
// 流式过程中宽度反复跳。这里把见过的最宽宽度记成下限，之后只增不减；上限是
// 所在行的可用宽度，窗口收放让可用宽度变了，下限按新上限收回（否则会溢出）。
//
// 用 ResizeObserver 而不是每次渲染都量：观测回调发生在布局之后、绘制之前，
// 浏览器本来就要布局，省掉的是同步强制重排（流式期每帧一次强制重排是长会话
// 掉帧的主因之一）。代价是宽度被收窄的那一帧要多走一次布局来写回下限。
//
// 容器隐藏时量到的宽高全是 0：这时没有可用宽度可言，别拿它改下限——聊天页切走
// 是隐藏不卸载（见 SPEC「聊天常挂载」），切回来时下限应当还在。

import { useEffect, useRef } from "react";

/** 宽度下限：只增不减 */
export function stickyFloor(current: number, measured: number): number {
  return measured > current ? measured : current;
}

/** 行内可用宽度：行右缘减去列左缘（列左缘 = 头像 + 间距，位置固定） */
export function availWidth(rowRight: number, colLeft: number): number {
  return Math.max(0, rowRight - colLeft);
}

/**
 * 让气泡所在列的宽度只增不减。
 *
 * @param bubbleRef 气泡元素（量它的实际宽度）
 * @param colRef 气泡所在列（下限写在这一列的 min-width 上）
 */
export function useStickyBubbleWidth(
  bubbleRef: React.RefObject<HTMLElement | null>,
  colRef: React.RefObject<HTMLElement | null>,
): void {
  /** 见过的最宽宽度（记忆，只增） */
  const widestRef = useRef(0);
  /** 上一次的行内可用宽度（变了说明窗口收放过） */
  const availRef = useRef(0);
  /** 已写进样式的下限 */
  const appliedRef = useRef(0);

  useEffect(() => {
    const bubble = bubbleRef.current;
    const col = colRef.current;
    // 能力守卫：jsdom 等环境没有 ResizeObserver，缺了只是宽度照旧跳
    if (!bubble || !col || typeof ResizeObserver !== "function") return;
    const row = col.parentElement;
    if (!row) return;

    const write = (floor: number) => {
      if (appliedRef.current === floor) return;
      appliedRef.current = floor;
      // 向上取整：下限只管宽、不管窄，抹掉小数像素的零头
      col.style.minWidth = floor > 0 ? `${Math.ceil(floor)}px` : "";
    };

    const ro = new ResizeObserver(() => {
      const rowRect = row.getBoundingClientRect();
      const colRect = col.getBoundingClientRect();
      const avail = availWidth(rowRect.right, colRect.left);
      // 容器隐藏：量到的全是 0，没有可用宽度可言，保持现状
      if (avail <= 0) return;
      if (avail !== availRef.current) {
        availRef.current = avail;
        widestRef.current = Math.min(widestRef.current, avail);
        // 旧下限可能超出新的可用宽度：先收回（写样式会改布局，下一轮观察再校准）
        const capped = Math.min(widestRef.current, avail);
        if (appliedRef.current > capped) {
          write(capped);
          return;
        }
      }
      const measured = bubble.getBoundingClientRect().width;
      if (measured > widestRef.current) widestRef.current = measured;
      const floor = Math.min(widestRef.current, avail);
      // 现在就比下限宽：不必写样式（流式期绝大多数帧走这条，不额外触发布局）
      if (measured >= floor) return;
      write(floor);
    });
    ro.observe(bubble);
    ro.observe(row);
    return () => ro.disconnect();
  }, [bubbleRef, colRef]);
}
