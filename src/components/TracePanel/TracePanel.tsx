// 执行痕迹面板（P2-a，Agent 透明化设计 §5.2）：任务卡的「执行详情」。
//
// 数据源 = P1 的三表（db/trace.rs 查询命令 trace_list/trace_detail）：
// - 摘要头：状态/耗时/轮数/工具数/改文件数/tokens
// - 时间线：每次工具调用一行（入参/结果/耗时/成败，<details> 展开）
// - 文件变更：±行 + unified diff 着色（DiffView）+ 「回滚」按钮（漂移闸拒绝时展示原因）
//
// 弹层用 createPortal 渲染到 body——TodoCard 容器的 hover transform 会创建 CSS
// 包含块，position:fixed 子元素会被裁缩到卡片内（同 purge 弹窗的既有教训）。

import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { FileText, History, TriangleAlert, X } from "lucide-react";
import { basename } from "../../format";
import { DiffView } from "./DiffView";
import {
  fileRollback,
  traceDetail,
  traceListByTask,
  type FileChangeRow,
  type SpanRow,
  type TraceRow,
} from "../../lib/trace";

const STATUS_BADGE: Record<string, { label: string; cls: string }> = {
  done: { label: "完成", cls: "text-[var(--ok,#22c55e)]" },
  failed: { label: "失败", cls: "text-[var(--danger,#ef4444)]" },
  running: { label: "执行中", cls: "text-[var(--info,#3b82f6)]" },
  stopped: { label: "已停止", cls: "text-[var(--t4)]" },
  timeout: { label: "超时", cls: "text-[var(--t4)]" },
};

function fmtMs(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

function fmtTs(ts: number): string {
  const d = new Date(ts);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** 单条 span：一行摘要 + <details> 展开入参/结果 */
function SpanItem({ s }: { s: SpanRow }) {
  return (
    <div className="flex items-start gap-1.5">
      <span
        aria-hidden
        className={`mt-1 h-1.5 w-1.5 shrink-0 rounded-full ${s.ok ? "bg-[var(--ok,#22c55e)]" : "bg-[var(--danger,#ef4444)]"}`}
      />
      <details className="min-w-0 flex-1">
        <summary className="cursor-pointer select-none list-none">
          <span className="font-mono text-[11px] text-[var(--t3)]">{s.name}</span>
          <span className="ml-1.5 text-[10px] text-[var(--t5)]">
            第{s.turn}轮
            {s.durationMs != null && ` · ${fmtMs(s.durationMs)}`}
            {!s.ok && <span className="text-[var(--danger,#ef4444)]"> · 失败</span>}
          </span>
        </summary>
        {s.args && (
          <pre className="mt-0.5 whitespace-pre-wrap break-words font-mono text-[10px] text-[var(--t4)]">
            {s.args}
          </pre>
        )}
        {s.result && (
          <pre className="mt-0.5 whitespace-pre-wrap break-words font-mono text-[10px] text-[var(--t5)]">
            {s.result.length > 800 ? s.result.slice(0, 800) + "…" : s.result}
          </pre>
        )}
      </details>
    </div>
  );
}

/** 单条文件变更：路径 + ±行 + diff（可展开）+ 回滚（仅 modify 且有快照） */
function FileChangeItem({ c, onRolledBack }: { c: FileChangeRow; onRolledBack: () => void }) {
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const canRollback = c.kind === "modify" && !!c.beforeRef;
  const rollback = async () => {
    if (busy) return;
    setBusy(true);
    setMsg(null);
    try {
      setMsg(await fileRollback(c.id));
      onRolledBack();
    } catch (e) {
      // 漂移闸拒绝等业务错误：原文展示（包含「请人工核对」指引）
      setMsg(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="rounded-lg nm-inset px-2 py-1.5">
      <div className="flex items-center gap-1.5">
        <FileText size={11} aria-hidden className="shrink-0 text-[var(--t4)]" />
        <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-[var(--t3)]" title={c.path}>
          {basename(c.path)}
        </span>
        <span className="shrink-0 font-mono text-[10px] tabular-nums">
          <span className="text-[var(--ok,#22c55e)]">+{c.added}</span>{" "}
          <span className="text-[var(--danger,#ef4444)]">−{c.deleted}</span>
        </span>
        {canRollback && (
          <button
            className="nm-btn shrink-0 rounded-lg px-2 py-0.5 text-[10px] text-[var(--t3)] disabled:opacity-50"
            disabled={busy}
            title="恢复此文件到本次 AI 修改前（文件被手动改过会被拒）"
            onClick={rollback}
          >
            回滚
          </button>
        )}
      </div>
      {msg && <p className="mt-1 text-[10px] text-[var(--t3)]">{msg}</p>}
      {c.diff && (
        <details className="mt-1">
          <summary className="cursor-pointer select-none text-[10px] text-[var(--t4)]">查看 diff</summary>
          <div className="mt-1">
            <DiffView diff={c.diff} truncated={c.truncated} />
          </div>
        </details>
      )}
    </div>
  );
}

export function TracePanel({
  taskId,
  taskTitle,
  onClose,
}: {
  taskId: string;
  taskTitle?: string;
  onClose: () => void;
}) {
  const [traces, setTraces] = useState<TraceRow[]>([]);
  const [selected, setSelected] = useState<TraceRow | null>(null);
  const [detail, setDetail] = useState<Awaited<ReturnType<typeof traceDetail>>>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const reload = (traceId: number | null) => {
    setLoading(true);
    setError(null);
    traceListByTask(taskId)
      .then(async (list) => {
        setTraces(list);
        const pick = list.find((t) => t.id === traceId) ?? list[0] ?? null;
        setSelected(pick);
        setDetail(pick ? await traceDetail(pick.id) : null);
      })
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    reload(null);
    // eslint-disable-next-line react/exhaustive-deps -- taskId 不变的挂载期一次性加载；回滚后手动 reload
  }, [taskId]);

  const statusOf = (t: TraceRow | null) => (t ? STATUS_BADGE[t.status] ?? null : null);
  const duration = (t: TraceRow): string =>
    t.finishedAt ? fmtMs(t.finishedAt - t.startedAt) : "进行中";

  return createPortal(
    <div
      className="fixed inset-0 z-[100] flex items-center justify-center bg-black/40 p-6"
      onPointerDown={onClose}
    >
      <div
        className="nm-card flex max-h-[80vh] w-full max-w-2xl flex-col p-4"
        onPointerDown={(e) => e.stopPropagation()}
      >
        {/* 头部：标题 + 关闭 */}
        <div className="flex items-center gap-2">
          <History size={14} aria-hidden className="text-[var(--t4)]" />
          <h2 className="min-w-0 flex-1 truncate text-sm font-medium text-[var(--t1)]">
            执行详情{taskTitle ? ` · ${taskTitle}` : ""}
          </h2>
          <button
            className="nm-btn rounded-lg p-1 text-[var(--t4)] hover:text-[var(--t2)]"
            aria-label="关闭执行详情"
            onClick={onClose}
          >
            <X size={14} aria-hidden />
          </button>
        </div>

        {/* 执行历史切换（同卡多次执行） */}
        {traces.length > 1 && (
          <div className="mt-2 flex flex-wrap gap-1">
            {traces.map((t) => {
              const badge = STATUS_BADGE[t.status];
              return (
                <button
                  key={t.id}
                  className={`nm-btn rounded-lg px-2 py-0.5 text-[10px] ${
                    selected?.id === t.id ? "text-[var(--t1)]" : "text-[var(--t4)]"
                  }`}
                  onClick={() => {
                    setSelected(t);
                    setDetail(null);
                    traceDetail(t.id)
                      .then(setDetail)
                      .catch((e) => setError(String(e)));
                  }}
                >
                  {fmtTs(t.startedAt)}
                  {badge && <span className={`ml-1 ${badge.cls}`}>{badge.label}</span>}
                </button>
              );
            })}
          </div>
        )}

        {loading && <p className="mt-4 text-center text-xs text-[var(--t5)]">加载中…</p>}
        {error && (
          <p className="mt-4 text-xs text-[var(--danger,#ef4444)]">
            <TriangleAlert size={11} aria-hidden className="mr-1 inline" />
            {error}
          </p>
        )}

        {!loading && !error && !detail && (
          <p className="mt-6 text-center text-xs text-[var(--t5)]">
            这张卡还没有执行痕迹——交给机器人跑一次后，这里会展示每次工具调用与文件修改。
          </p>
        )}

        {!loading && detail && (
          <>
            {/* 摘要头 */}
            <div className="mt-3 flex flex-wrap items-center gap-1.5 text-[10px] text-[var(--t4)]">
              {(() => {
                const badge = statusOf(detail);
                return badge ? (
                  <span className={`nm-inset px-2 py-0.5 ${badge.cls}`}>{badge.label}</span>
                ) : null;
              })()}
              <span className="nm-inset px-2 py-0.5 tabular-nums">{duration(detail)}</span>
              <span className="nm-inset px-2 py-0.5 tabular-nums">
                {detail.turnCount} 轮 · {detail.toolCalls} 次工具 · {detail.filesChanged} 个文件
              </span>
              {(detail.promptTokens > 0 || detail.completionTokens > 0) && (
                <span className="nm-inset px-2 py-0.5 tabular-nums">
                  tokens {detail.promptTokens}/{detail.completionTokens}
                </span>
              )}
              {detail.origin && (
                <span className="nm-inset px-2 py-0.5">{detail.origin}</span>
              )}
            </div>
            {detail.error && (
              <p className="mt-2 text-xs text-[var(--danger,#ef4444)]">
                <TriangleAlert size={11} aria-hidden className="mr-1 inline" />
                {detail.error}
              </p>
            )}

            {/* 文件变更（有 diff + 回滚，放时间线前面——「改了什么」是第一诉求） */}
            {detail.fileChanges.length > 0 && (
              <div className="mt-3">
                <p className="text-[10px] font-medium uppercase tracking-wide text-[var(--t5)]">
                  文件修改
                </p>
                <div className="mt-1 space-y-1.5">
                  {detail.fileChanges.map((c) => (
                    <FileChangeItem key={c.id} c={c} onRolledBack={() => reload(detail.id)} />
                  ))}
                </div>
              </div>
            )}

            {/* 工具时间线 */}
            <div className="mt-3 min-h-0 flex-1 overflow-y-auto">
              <p className="text-[10px] font-medium uppercase tracking-wide text-[var(--t5)]">
                工具时间线
              </p>
              <div className="mt-1 space-y-1.5">
                {detail.spans.length === 0 ? (
                  <p className="text-[10px] text-[var(--t5)]">
                    本次执行没有工具调用记录。
                  </p>
                ) : (
                  detail.spans.map((s) => <SpanItem key={s.id} s={s} />)
                )}
              </div>
            </div>
          </>
        )}
      </div>
    </div>,
    document.body
  );
}
