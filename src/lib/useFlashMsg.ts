import { useCallback, useEffect, useRef, useState } from "react";

/** 一次性操作提示：flash(msg) 置文案并自动清空。重设前清旧计时器
 *  （防连续操作时旧计时提前清掉新提示），卸载时清理。
 *  ms 缺省用 defaultMs；传 0 表示持续显示（错误提示等需要驻留的场景）。 */
export function useFlashMsg(
  defaultMs = 3000,
): [string, (msg: string, ms?: number) => void] {
  const [msg, setMsg] = useState("");
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );
  const flash = useCallback(
    (m: string, ms?: number) => {
      setMsg(m);
      if (timer.current) clearTimeout(timer.current);
      const wait = ms ?? defaultMs;
      if (wait > 0) timer.current = setTimeout(() => setMsg(""), wait);
    },
    [defaultMs],
  );
  return [msg, flash];
}
