// 全局 JS 错误上报：window error / unhandledrejection → frontend_event_report
//（Rust 侧签名去重 + error.log 镜像）。生产 WebView 控制台没人看，脚本崩溃
// 与未处理 rejection 从此可反推。与 main.tsx 的 CSP 上报同款纪律：每会话
// 上限 20 条、上报失败静默（审计通道自身不得产生用户可见副作用）、只装一次。

import { invoke } from "@tauri-apps/api/core";

const MAX_REPORTS = 20;
let reports = 0;
let installed = false;

/** ResizeObserver 循环噪音：观察回调内改布局触发下一帧重排，规范自带的
 * 「通知延后投递」提示，不是应用错误——Chromium 已把它移出控制台，
 * Sentry 等上报器的标配 ignoreErrors。零诊断价值，只会挤占 Rust 侧 32
 * 签名去重表和 error.log 镜像。两种历史文案（Chrome 新旧版本）都滤，
 * 且在配额计数前丢弃（噪音不得占用真错误的名额）。 */
const IGNORED_ERROR_PATTERN =
  /^ResizeObserver loop (completed with undelivered notifications|limit exceeded)/;

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
    if (IGNORED_ERROR_PATTERN.test(ev.message)) return;
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
