// ErrorDialogHost —— 应用内错误弹窗宿主（批 4：替代原生 alert/confirm）。
//
// 原生 alert/confirm 无法富文本：❌💡🔁 只能靠 emoji 当视觉语言。本组件挂载后
// 接管 errorHandler 发出的 ERROR_DIALOG_EVENT（preventDefault 表示已接管），
// 用 nm 卡片渲染：CircleX 标题图标 + Lightbulb 提示行 + RotateCcw 重试键。
// 未挂载 Host 的窗口走 errorHandler 的原生兜底（行为不变）。
//
// 主窗口与挂件窗口各挂一个（App.tsx / WidgetApp.tsx）；window 事件天然按
// webview 隔离，互不串扰。

import { useEffect, useRef, useState } from "react";
import { CircleX, Lightbulb, RotateCcw } from "lucide-react";
import {
  ERROR_DIALOG_EVENT,
  type ErrorDialogRequest,
} from "../lib/errorHandler";

export function ErrorDialogHost() {
  const [req, setReq] = useState<ErrorDialogRequest | null>(null);
  // 当前请求的同步镜像：新请求顶掉旧请求前先把旧的 resolve(false)，
  // 否则旧调用方的 await 永久悬挂（resolve 不依赖 React 提交时机）
  const reqRef = useRef<ErrorDialogRequest | null>(null);

  useEffect(() => {
    const handler = (ev: Event) => {
      // preventDefault = 声明接管（errorHandler 据此决定是否走原生兜底）
      ev.preventDefault();
      const next = (ev as CustomEvent<ErrorDialogRequest>).detail;
      const prev = reqRef.current;
      reqRef.current = next;
      prev?.resolve(false);
      setReq(next);
    };
    window.addEventListener(ERROR_DIALOG_EVENT, handler);
    return () => window.removeEventListener(ERROR_DIALOG_EVENT, handler);
  }, []);

  if (!req) return null;

  const finish = (retry: boolean) => {
    try {
      req.resolve(retry);
    } finally {
      reqRef.current = null;
      setReq(null);
    }
  };

  return (
    <div
      className="fixed inset-0 z-[100] flex items-center justify-center bg-black/40 p-6"
      onClick={() => finish(false)}
      data-testid="error-dialog-overlay"
    >
      <div
        className="nm-card w-full max-w-md p-5 space-y-3"
        onClick={(e) => e.stopPropagation()}
        role="alertdialog"
        aria-label={req.retryable ? "错误（可重试）" : "错误"}
      >
        <p className="flex items-center gap-2 text-base font-semibold text-[var(--danger)]">
          <CircleX size={18} aria-hidden />
          {req.retryable ? "出错了，要重试吗？" : "出错了"}
        </p>
        <p className="text-sm text-[var(--t2)] whitespace-pre-wrap break-words">
          {req.message}
        </p>
        {req.hint && (
          <p className="flex items-start gap-1.5 text-xs text-[var(--t4)]">
            <Lightbulb
              size={12}
              aria-hidden
              className="shrink-0 mt-0.5 text-[var(--brand)]"
            />
            {req.hint}
          </p>
        )}
        <div className="flex justify-end gap-2 pt-1">
          {req.retryable && (
            <button
              type="button"
              className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)] inline-flex items-center gap-1 whitespace-nowrap"
              onClick={() => finish(false)}
            >
              取消
            </button>
          )}
          <button
            type="button"
            className="nm-btn px-3 py-1.5 text-xs text-[var(--t1)] inline-flex items-center gap-1 whitespace-nowrap"
            data-testid="error-dialog-primary"
            onClick={() => finish(req.retryable)}
          >
            {req.retryable && <RotateCcw size={12} aria-hidden />}
            {req.retryable ? "重试" : "确定"}
          </button>
        </div>
      </div>
    </div>
  );
}
