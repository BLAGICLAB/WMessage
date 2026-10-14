import { useEffect, useState } from "react";

/** 周期时钟：每 intervalMs 刷新一次 Date（分钟粒度 UI 的单一时间源）。
 *  intervalMs=null 不走表——外部时钟注入口（首页测试确定性场景）。
 *  首个 tick 在下一个间隔到达，挂载时不补一跳。 */
export function useNowTick(intervalMs: number | null = 60_000): Date {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    if (intervalMs === null) return;
    const id = setInterval(() => setNow(new Date()), intervalMs);
    return () => clearInterval(id);
  }, [intervalMs]);
  return now;
}
