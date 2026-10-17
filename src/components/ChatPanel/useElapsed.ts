// ChatPanel 子模块：把「本地开始时刻」渲染成秒级刷新的耗时（纯展示，无 IO）。

import { useEffect, useState } from "react";

/** 1s 刷新一次的耗时秒数。startedAt 为空（没开始 / 已完成）时返回 null。 */
export function useElapsed(startedAt?: number, active = true): number | null {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active || startedAt == null) return;
    // 1s 粒度足够：ZCode 的 duration 也按秒刷新。更高频只是白烧渲染。
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [startedAt, active]);
  if (startedAt == null || !active) return null;
  return Math.max(0, Math.floor((now - startedAt) / 1000));
}

/** 秒 → 「12s」/「1m 05s」。与 TracePanel 的 fmtMs 同口径的简化版（仅前端计时用）。 */
export function fmtElapsed(secs: number): string {
  if (secs < 60) return `${secs}s`;
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${m}m ${String(s).padStart(2, "0")}s`;
}
