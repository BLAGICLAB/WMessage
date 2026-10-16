// 全局 JS 错误上报：window error / unhandledrejection → frontend_event_report
//（Rust 侧签名去重 + error.log 镜像）。生产 WebView 控制台没人看，脚本崩溃
// 与未处理 rejection 从此可反推。与 main.tsx 的 CSP 上报同款纪律：每会话
// 上限 20 条、上报失败静默（审计通道自身不得产生用户可见副作用）、只装一次。

import { invoke } from "@tauri-apps/api/core";

const MAX_REPORTS = 20;
let reports = 0;
let installed = false;

function report(event: string, detail: string): void {
  if (reports >= MAX_REPORTS) return;
  reports += 1;
  invoke("frontend_event_report", {
    event,
    detail: detail.slice(0, 200),
  }).catch(() => {});
}

/** reason → 单行摘要：Error 取栈顶两行（含消息+首帧），其余 String 化 */
export function detailFromReason(reason: unknown): string {
  if (reason instanceof Error) {
    if (reason.stack) return reason.stack.split("\n").slice(0, 2).join(" ");
    return reason.message;
  }
  return String(reason);
}

/** 安装全局监听（幂等）；资源加载失败（img/script，error 事件无 message）
 * 不上报——瞬时噪音会挤占 Rust 侧 32 签名去重表，把真错误挤出去。
 * target 参数化（生产 = window）：模块态 + 全局监听让测试无法用 resetModules
 * 隔离（旧监听器仍挂在共享 window 上），测试注入全新 EventTarget 即零泄漏 */
export function installGlobalErrorReport(target: Window | EventTarget = window): void {
  if (installed) return;
  installed = true;
  target.addEventListener("error", (e) => {
    const ev = e as ErrorEvent;
    if (!ev.message) return;
    report(
      "js_error",
      `${ev.filename || "?"}:${ev.lineno ?? 0}:${ev.colno ?? 0} ${ev.message}`,
    );
  });
  target.addEventListener("unhandledrejection", (e) => {
    const reason = (e as PromiseRejectionEvent).reason;
    report("js_unhandled_rejection", detailFromReason(reason));
  });
}
