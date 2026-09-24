import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";

/**
 * Tauri 事件订阅 hook（组件挂载期单次订阅）。
 *
 * cancelled flag 防「卸载早于 listen resolve」泄漏：cleanup 先跑完时，
 * 注册完成后立即自注销，listener 不悬挂；catch 防 unhandled rejection。
 * handler 只在订阅时捕获一次——调用方只应传 setState 类稳定语义回调。
 */
export function useTauriListen<T>(
  event: string,
  handler: (payload: T) => void
): void {
  useEffect(() => {
    let cancelled = false;
    let unlistenFn: (() => void) | undefined;
    listen<T>(event, (e) => handler(e.payload))
      .then((u) => {
        if (cancelled) u();
        else unlistenFn = u;
      })
      .catch((e) => console.error(`${event} listen failed:`, e));
    return () => {
      cancelled = true;
      unlistenFn?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [event]);
}
