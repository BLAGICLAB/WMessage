// ChatPanel 子模块：输入卡底栏的窄宽度三档降级（full / compact / mini）。
//
// 分档宽度量的是输入卡本身，不是窗口：卡片在 760px 阅读带内，左侧会话栏、
// 右侧上下文栏的开合与伸缩都在改变卡片可用宽，窗口断点（lg: 这类）量不出
// 这个落差。ResizeObserver 观测卡片内容盒（底栏 flex 行的实际可用宽），
// 观测回调发生在布局之后、绘制之前，拖窗过程逐帧跟随、无同步强制重排。
//
// 三档语义：
//   full    ≥ ICONIFY_BELOW ：底栏控件全文案（原始形态）
//   compact ≥ COLLAPSE_BELOW：推理强度图标化（文案退到 tooltip 与 aria-label）
//   mini    < COLLAPSE_BELOW：推理强度收进 ⋯ 菜单，授权模式也图标化
//
// 容器隐藏时量到的是 0（聊天页切走是隐藏不卸载，见 SPEC「聊天常挂载」）：
// 没有可用宽度可言，回落 full 维持原渲染，切回来自然恢复分档。
// jsdom 等环境没有 ResizeObserver：同样回落 full（与降级前同渲染），不抛错。

import { useEffect, useState } from "react";
import type { RefObject } from "react";

export type ComposerTier = "full" | "compact" | "mini";

/** 低于此宽度（px）：全文案底栏开始放不下，推理强度去文案只留图标 */
const ICONIFY_BELOW = 620;
/** 低于此宽度（px）：图标化也放不下，两控件收进 ⋯ 菜单、授权模式一并图标化 */
const COLLAPSE_BELOW = 470;

export function tierOf(width: number): ComposerTier {
  if (width <= 0) return "full";
  if (width < COLLAPSE_BELOW) return "mini";
  if (width < ICONIFY_BELOW) return "compact";
  return "full";
}

/** 观测输入卡宽度并折算成降级档位 */
export function useComposerTier(
  cardRef: RefObject<HTMLElement | null>,
): ComposerTier {
  const [tier, setTier] = useState<ComposerTier>("full");
  useEffect(() => {
    const card = cardRef.current;
    if (!card || typeof ResizeObserver !== "function") return;
    const ro = new ResizeObserver((entries) => {
      const width = entries[entries.length - 1]?.contentRect.width ?? 0;
      setTier(tierOf(width));
    });
    ro.observe(card);
    return () => ro.disconnect();
  }, [cardRef]);
  return tier;
}
