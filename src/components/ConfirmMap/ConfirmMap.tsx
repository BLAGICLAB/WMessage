//! ConfirmMap 挂件：监听后端 `bot-confirm` 事件，弹 modal 等用户确认/拒绝。
//!
//! 后端（bot_slash.rs::ask_confirm_inner）emit_to("widget", "bot-confirm", payload)：
//!   payload = { id, tool, detail, kind, sessionId }
//!
//! 前端这里 listen 这个事件 → 弹 modal + 60s 倒计时 + 确认/拒绝按钮 → 调
//!   invoke("bot_confirm_response", { request_id, approved, always })
//!
//! 老板 14:21 拍板：EvolutionPanel Promote/Reject/Keep Shadow 等走 confirm 流程的
//! 危险操作，60s 无响应 → 后端自动拒绝（spec §12.7 fail-safe）。
//! ConfirmMap 是 spec R0 #1 「复用 widget 弹窗」的前端实现。

import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

/// 与后端 emit 的 payload 字段对齐（bot_slash.rs:301 附近）
type BotConfirmPayload = {
  id: string;
  tool: string;
  detail: string;
  kind: string; // "danger" | "file_access" | ...
  sessionId: string | null;
};

const TIMEOUT_SECS = 60;

export default function ConfirmMap() {
  const [pending, setPending] = useState<BotConfirmPayload | null>(null);
  const [remaining, setRemaining] = useState(TIMEOUT_SECS);
  const [busy, setBusy] = useState(false);
  // 闭包内稳定的 responder（避免 useCallback 依赖 pending）
  const pendingRef = useRef<BotConfirmPayload | null>(null);
  pendingRef.current = pending;

  // 监听后端 bot-confirm 事件
  useEffect(() => {
    let unlistenFn: (() => void) | undefined;
    (async () => {
      const u = await listen<BotConfirmPayload>("bot-confirm", (e) => {
        const p = e.payload;
        if (!p || !p.id) return;
        setPending(p);
        setRemaining(TIMEOUT_SECS);
      });
      unlistenFn = u;
    })();
    return () => {
      unlistenFn?.();
    };
  }, []);

  // 60s 倒计时：到 0 自动拒绝（前端防御，后端会兜底）
  useEffect(() => {
    if (!pending) return;
    if (remaining <= 0) {
      void respond(false);
      return;
    }
    const id = window.setTimeout(() => setRemaining((r) => r - 1), 1000);
    return () => window.clearTimeout(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pending, remaining]);

  async function respond(approved: boolean) {
    const cur = pendingRef.current;
    if (!cur || busy) return;
    setBusy(true);
    try {
      await invoke("bot_confirm_response", {
        requestId: cur.id,
        approved,
        always: null,
      });
    } catch (e) {
      // 不阻塞 UI；后端超时也会兜底
      console.error("bot_confirm_response failed:", e);
    } finally {
      setBusy(false);
      setPending(null);
    }
  }

  if (!pending) return null;

  return (
    <div
      className="fixed inset-0 z-[100] flex items-center justify-center bg-black/40 p-6"
      data-testid="confirm-map-modal"
    >
      <div className="nm-card w-full max-w-md p-5 space-y-4">
        <div className="flex items-center justify-between">
          <h3 className="text-base font-semibold text-[var(--t1)]">
            {pending.tool}
            {pending.kind === "danger" && (
              <span className="ml-2 text-xs px-1.5 py-0.5 rounded bg-red-100 text-red-700">
                高危
              </span>
            )}
            {pending.kind === "file_access" && (
              <span className="ml-2 text-xs px-1.5 py-0.5 rounded bg-amber-100 text-amber-700">
                文件访问
              </span>
            )}
          </h3>
          <span
            className={`text-xs tabular-nums ${
              remaining <= 10 ? "text-red-600" : "text-[var(--t3)]"
            }`}
            title="60s 无响应默认拒绝"
            data-testid="confirm-map-countdown"
          >
            {remaining}s
          </span>
        </div>

        <pre
          className="nm-inset text-xs text-[var(--t2)] whitespace-pre-wrap break-words max-h-60 overflow-auto p-3 rounded"
          data-testid="confirm-map-detail"
        >
          {pending.detail}
        </pre>

        <div className="flex gap-2 justify-end">
          <button
            className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)]"
            disabled={busy}
            onClick={() => void respond(false)}
            data-testid="confirm-map-deny"
          >
            拒绝
          </button>
          <button
            className="nm-btn px-3 py-1.5 text-xs text-[var(--t1)] bg-[var(--accent)]"
            disabled={busy}
            onClick={() => void respond(true)}
            data-testid="confirm-map-approve"
          >
            确认
          </button>
        </div>
      </div>
    </div>
  );
}
