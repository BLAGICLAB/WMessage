import { useEffect } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/**
 * 反注册统一走这里：Tauri 注入的 unlisten_js_script 无洞守卫
 * （tauri 2.11.5 src/event/mod.rs unlisten_js_script——listeners[eventName]
 * 数组存在但 listeners[eventId] 槽位未写入/已清空时直接 TypeError），
 * unlisten 与注册表填充存在竞态（主窗口冒烟实见一条 Unhandled Promise
 * Rejection，见 docs/CSP-TIGHTEN-VERIFY-2026-10-07.md §6）。卸载路径上
 * 失败无行动点，静默。
 */
export function unlistenSafe(u: UnlistenFn | Promise<UnlistenFn>): void {
  Promise.resolve(u)
    .then((f) => f())
    .catch(() => {});
}

/**
 * Tauri 事件订阅 hook（组件挂载期单次订阅）。
 *
 * cancelled flag 防「卸载早于 listen resolve」泄漏：cleanup 先跑完时，
 * 注册完成后立即自注销，listener 不悬挂；catch 防 unhandled rejection。
 * handler 只在订阅时捕获一次——调用方只应传 setState 类稳定语义回调。
 */
export function useTauriListen<T>(
  event: string,
  handler: (payload: T) => void,
): void {
  useEffect(() => {
    let cancelled = false;
    let unlistenFn: UnlistenFn | undefined;
    listen<T>(event, (e) => handler(e.payload))
      .then((u) => {
        if (cancelled) unlistenSafe(u);
        else unlistenFn = u;
      })
      .catch((e) => console.error(`${event} listen failed:`, e));
    return () => {
      cancelled = true;
      if (unlistenFn) unlistenSafe(unlistenFn);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [event]);
}
