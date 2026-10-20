// ChatPanel 子模块：活体执行指示（借鉴 ZCode 的 turn status 呈现）。
//
// 为什么需要它：原先 header 只有一个 animate-pulse 小圆点 + 「正在回复」四个字，
// 它由 viewedBusy 驱动、整个 turn 恒为 true —— 于是它在工具执行期**确实亮着
// 但完全不变**，300s 的 pip install 与 0.1s 的 list_files 长得一模一样。
// 「亮着」不等于「有信息」，静默一样会让人以为卡死。
//
// ZCode 的做法（docs/chat/reasoning-streaming-auto-collapse.md）是：
// 渐变文字 + 流式状态摘要 + 每秒刷新的 duration。本组件照此实现。

import { Loader2 } from "lucide-react";
import type { TurnStatus } from "./types";
import { fmtElapsed, useElapsed } from "./useElapsed";

/** 渐变文字：CSS 动画渐变裁切到字面（ZCode 的 animated-gradient-text 同款思路）。
 *  渐变本身用内联 style——tailwind 任意值类装不下这条 color-mix 渐变；
 *  但流光动画必须写在 CSS 类里（.wmsg-live-label）：内联 style 压过样式表规则，
 *  main.css 里那条 reduced-motion 覆盖就永远不生效。 */
const GRADIENT_TEXT: React.CSSProperties = {
  backgroundImage:
    "linear-gradient(90deg, var(--brand) 0%, var(--brand) 35%, color-mix(in oklab, var(--brand) 55%, transparent) 50%, var(--brand) 65%, var(--brand) 100%)",
  backgroundSize: "250% 100%",
  WebkitBackgroundClip: "text",
  backgroundClip: "text",
  color: "transparent",
};

export function LiveIndicator({
  status,
  detail,
  startedAt,
}: {
  status: TurnStatus;
  detail?: string;
  startedAt?: number;
}) {
  // settling 也要计：它正是「正文说完了但还在跑」的那段，秒数在这里最有用
  const secs = useElapsed(startedAt, status === "running" || status === "settling");
  const label =
    status === "settling"
      ? (detail ?? "收尾中")
      : status === "running"
        ? (detail ?? "正在思考")
        : "正在回复";
  return (
    <span className="flex shrink-0 items-center gap-1.5 text-[10px]">
      <Loader2 size={10} className="animate-spin text-[var(--brand)]" aria-hidden />
      <span style={GRADIENT_TEXT} className="wmsg-live-label tabular-nums">
        {label}
      </span>
      {secs != null && <span className="tabular-nums text-[var(--t4)]">{fmtElapsed(secs)}</span>}
    </span>
  );
}
