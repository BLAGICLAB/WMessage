// 执行活动聚合页（Agent 透明化设计 §5.6）：主窗口的「执行活动」——
// 最近全部执行痕迹（手动/定时/批量/工作流/主聊天）列表 + running 实时徽标，
// 行点击看单次 trace（TracePanel traceId 直查模式）；顶部「唤起挂件」围观执行会话。
// 数据源 = exec_traces（trace_list 命令）；10s 轻轮询兜底（有 running 行时才转）。

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { Activity, Bot, ExternalLink } from "lucide-react";
import { TracePanel } from "../TracePanel";
import { formatCommandError } from "../../lib/errorHandler";
import type { TraceRow } from "../../lib/trace";

const ORIGIN_LABEL: Record<string, string> = {
  manual: "手动",
  scheduled: "定时",
  batch: "批量",
  workflow: "工作流",
};

const STATUS_CLS: Record<string, string> = {
  done: "text-[var(--success)]",
  failed: "text-[var(--danger)]",
  running: "text-[var(--brand)]",
  stopped: "text-[var(--t5)]",
  timeout: "text-[var(--t5)]",
};
const STATUS_LABEL: Record<string, string> = {
  done: "完成",
  failed: "失败",
  running: "执行中",
  stopped: "已停止",
  timeout: "超时",
};

function fmtDur(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

export function ActivityPage() {
  const [traces, setTraces] = useState<TraceRow[]>([]);
  const [loaded, setLoaded] = useState(false);
  /** 列表加载失败提示：非空时不渲染「还没有执行痕迹」空态（失败 ≠ 无数据） */
  const [loadError, setLoadError] = useState("");
  /** 唤起挂件失败提示（按钮操作的结果反馈，与列表加载失败分开） */
  const [widgetError, setWidgetError] = useState("");
  const [openTraceId, setOpenTraceId] = useState<number | null>(null);

  // 竞态令牌：10s 轮询 + 事件重入时，慢的旧 trace_list 响应不得覆盖新响应
  const reqIdRef = useRef(0);
  const reload = useCallback(() => {
    const reqId = ++reqIdRef.current;
    invoke<TraceRow[]>("trace_list", { limit: 50 })
      .then((rows) => {
        if (reqId !== reqIdRef.current) return;
        setTraces(rows);
        setLoadError("");
      })
      .catch((e) => {
        if (reqId !== reqIdRef.current) return;
        // 失败必须可见：静默吞掉会让首屏落进「还没有执行痕迹」空态误导用户
        setLoadError(formatCommandError(e));
      })
      .finally(() => {
        if (reqId === reqIdRef.current) setLoaded(true);
      });
  }, []);

  useEffect(() => {
    reload();
  }, [reload]);

  // 有执行中的痕迹时 10s 轻轮询（收尾后自然停转；sched/workflow 事件页各自消费）
  const hasRunning = traces.some((t) => t.status === "running");
  useEffect(() => {
    if (!hasRunning) return;
    const t = setInterval(reload, 10_000);
    return () => clearInterval(t);
  }, [hasRunning, reload]);

  const showWidget = async () => {
    try {
      const w = await WebviewWindow.getByLabel("widget");
      if (!w) {
        setWidgetError("挂件窗口尚未创建，无法唤起");
        return;
      }
      await w.show();
      await w.setFocus();
      setWidgetError("");
    } catch (e) {
      setWidgetError(`唤起挂件失败：${formatCommandError(e)}`);
    }
  };

  return (
    <div className="space-y-4">
      <div className="nm-card p-5">
        <div className="flex items-center justify-between gap-4">
          <div>
            <h2 className="text-lg font-semibold text-[var(--t1)]">执行活动</h2>
            <p className="mt-1 text-xs text-[var(--t5)]">
              最近 50
              次执行的痕迹（对话/任务卡/定时/工作流）；点行看工具时间线与文件
              diff
            </p>
          </div>
          <button
            className="shrink-0 nm-btn px-3 py-1.5 text-xs text-[var(--t3)] inline-flex items-center gap-1"
            title="唤起挂件窗口围观执行会话（流式输出）"
            onClick={() => void showWidget()}
          >
            <ExternalLink size={12} aria-hidden /> 唤起挂件
          </button>
        </div>
        {widgetError && (
          <p className="mt-2 text-xs text-[var(--danger)]">{widgetError}</p>
        )}

        {!loaded && (
          <p className="mt-6 text-center text-xs text-[var(--t5)]">加载中…</p>
        )}
        {loaded && loadError && (
          <p className="mt-6 text-center text-xs text-[var(--danger)]">
            执行痕迹加载失败：{loadError}
          </p>
        )}
        {loaded && !loadError && traces.length === 0 && (
          <p className="mt-6 text-center text-xs text-[var(--t5)]">
            还没有执行痕迹——跑一次对话或交给机器人执行任务卡后，这里会出现执行记录。
          </p>
        )}
        {traces.length > 0 && (
          <ul className="mt-4 flex flex-col divide-y divide-[var(--edge)]">
            {traces.map((t) => (
              <li key={t.id}>
                <button
                  className="flex w-full items-center gap-2 py-2 text-left text-xs hover:bg-[var(--hover-bg)]"
                  onClick={() => setOpenTraceId(t.id)}
                >
                  <span className="w-24 shrink-0 font-mono text-[10px] tabular-nums text-[var(--t5)]">
                    {new Date(t.startedAt).toLocaleString("zh-CN", {
                      month: "2-digit",
                      day: "2-digit",
                      hour: "2-digit",
                      minute: "2-digit",
                    })}
                  </span>
                  <span
                    className="min-w-0 flex-1 truncate text-[var(--t2)]"
                    title={t.title ?? undefined}
                  >
                    {t.taskId ? (
                      <Bot
                        size={11}
                        aria-hidden
                        className="mr-1 inline text-[var(--t4)]"
                      />
                    ) : null}
                    {t.title || t.sessionId}
                  </span>
                  <span className="shrink-0 nm-inset px-1.5 py-0.5 text-[10px] text-[var(--t4)]">
                    {ORIGIN_LABEL[t.origin] ?? t.origin}
                  </span>
                  <span
                    className={`shrink-0 text-[10px] ${STATUS_CLS[t.status] ?? ""}`}
                    title={t.error ?? undefined}
                  >
                    {t.status === "running" && (
                      <Activity
                        size={10}
                        aria-hidden
                        className="mr-1 inline animate-pulse"
                      />
                    )}
                    {STATUS_LABEL[t.status] ?? t.status}
                  </span>
                  <span className="w-16 shrink-0 text-right font-mono text-[10px] tabular-nums text-[var(--t5)]">
                    {t.finishedAt ? fmtDur(t.finishedAt - t.startedAt) : "—"}
                  </span>
                  <span className="w-20 shrink-0 text-right font-mono text-[10px] tabular-nums text-[var(--t5)]">
                    {(t.promptTokens > 0 || t.completionTokens > 0) &&
                      `${((t.promptTokens + t.completionTokens) / 1000).toFixed(1)}K`}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>

      {/* 单次 trace 直查弹层（traceId 模式，无需 taskId 反查） */}
      {openTraceId !== null && (
        <TracePanel
          traceId={openTraceId}
          onClose={() => setOpenTraceId(null)}
        />
      )}
    </div>
  );
}
