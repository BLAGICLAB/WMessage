// 执行痕迹面板（Agent 透明化设计 §5.2）：任务卡的「执行详情」。
//
// 数据源 =  的三表（db/trace.rs 查询命令 trace_list/trace_detail）：
// - 摘要头：状态/耗时/轮数/工具数/改文件数/tokens
// - 时间线：每次工具调用一行（入参/结果/耗时/成败，<details> 展开）
// - 文件变更：±行 + unified diff 着色（DiffView）+ 「回滚」按钮（漂移闸拒绝时展示原因）
//
// 弹层用 createPortal 渲染到 body——TodoCard 容器的 hover transform 会创建 CSS
// 包含块，position:fixed 子元素会被裁缩到卡片内（同 purge 弹窗的既有教训）。

import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Download, FileText, History, ListChecks, TriangleAlert, X } from "lucide-react";
import { basename } from "../../format";
import { listWorkflowAudit, type WorkflowAuditEntry } from "../../lib/workflowAudit";
import { DiffView } from "./DiffView";
import {
  fileRollback,
  traceDetail,
  traceExport,
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

/** W10：审计 kind → 展示标签（契约外值原样显示，不炸渲染） */
const AUDIT_KIND_LABEL: Record<string, string> = {
  run_start: "▶ 开始",
  node_start: "· 节点开始",
  node_result: "✓ 节点结果",
  acceptance_check: "🔎 验收",
  rework: "↻ 验收返工",
  review: "📋 收尾评审",
  rework_round: "↻ 评审返工轮",
  run_done: "■ 结束",
  stop: "⏹ 停止",
  question_asked: "❓ 提问",
  question_answered: "💬 回答",
};

/** 审计 payload → 一行摘要（各 kind 取关键字段；缺失静默省略） */
function auditPayloadSummary(r: WorkflowAuditEntry): string {
  const p = r.payload ?? {};
  const parts: string[] = [];
  const push = (label: string, v: unknown) => {
    if (typeof v === "string" && v) parts.push(`${label} ${v}`);
    else if (typeof v === "number") parts.push(`${label} ${v}`);
  };
  switch (r.kind) {
    case "run_start":
      push("触发", p.trigger);
      push("节点", p.total);
      break;
    case "node_result":
      push("状态", p.status);
      push("attempt", p.attempt);
      push("耗时", typeof p.ms === "number" ? `${p.ms}ms` : undefined);
      break;
    case "acceptance_check":
      push("裁决", p.verdict);
      push("返工", p.reworkUsed);
      if (typeof p.evidence === "string" && p.evidence) parts.push(p.evidence);
      break;
    case "node_start":
      push("attempt", p.attempt);
      break;
    case "stop":
      push("by", p.by);
      break;
    case "rework":
      push("原因", p.reason);
      push("第", p.reworkUsed);
      push("attempt", p.attempt);
      if (typeof p.evidence === "string" && p.evidence) parts.push(p.evidence);
      break;
    case "review":
      push("verdict", p.verdict);
      push("issues", p.issues);
      break;
    case "rework_round":
      push("返工节点", p.reworked);
      push("verdict", p.verdict);
      break;
    case "run_done":
      push("完成", p.done);
      push("失败", p.failed);
      push("跳过", p.skipped);
      break;
    case "question_asked":
      if (typeof p.question === "string") parts.push(p.question);
      if (typeof p.assumption === "string" && p.assumption) parts.push(`假设 ${p.assumption}`);
      break;
    case "question_answered":
      if (typeof p.question === "string") parts.push(p.question);
      if (typeof p.answer === "string") parts.push(`→ ${p.answer}`);
      push("动作", p.action);
      break;
    default:
      break;
  }
  return parts.join(" · ");
}

export function TracePanel({
  taskId,
  taskTitle,
  traceId,
  workflowId,
  acceptanceInfo,
  onClose,
}: {
  /** 按任务卡查（执行历史列表 → 选一条）；与 traceId 二选一 */
  taskId?: string;
  taskTitle?: string;
  /**  直查模式：活动页按 trace id 直接打开单条 */
  traceId?: number;
  /** W10：所属工作流（有值才显示「运行审计」页签；看板任务无审计） */
  workflowId?: string;
  /** W10：本卡验收结论（节点级验收写入 result；有值显示验收行） */
  acceptanceInfo?: { verdict: string; evidence: string } | null;
  onClose: () => void;
}) {
  const [traces, setTraces] = useState<TraceRow[]>([]);
  const [selected, setSelected] = useState<TraceRow | null>(null);
  const [detail, setDetail] = useState<Awaited<ReturnType<typeof traceDetail>>>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // JSONL 导出（按钮态 + 结果路径/错误展示）
  const [exportBusy, setExportBusy] = useState(false);
  const [exportMsg, setExportMsg] = useState<string | null>(null);
  // W10：页签（trace=执行痕迹 / audit=运行审计）+ 审计数据
  const [tab, setTab] = useState<"trace" | "audit">("trace");
  const [auditRows, setAuditRows] = useState<WorkflowAuditEntry[] | null>(null);
  const [auditErr, setAuditErr] = useState<string | null>(null);

  // 竞态令牌：快速连点历史条目时，慢的旧 traceDetail 响应不得覆盖新选中的详情
  const detailSeqRef = useRef(0);

  const reload = (pickId: number | null) => {
    // 开头即作废在途响应：taskId 模式下慢的旧 reload 不得覆盖新选中的详情
    const seq = ++detailSeqRef.current;
    setLoading(true);
    setError(null);
    // 直查模式（traceId）：单条结果即完整 detail，直接复用不再重复查询；
    // 任务卡模式拉执行历史列表后按选中项再取详情
    if (traceId != null) {
      traceDetail(traceId)
        .then((d) => {
          if (seq !== detailSeqRef.current) return;
          const list = d ? [d] : [];
          setTraces(list);
          const pick = list.find((t) => t.id === pickId) ?? list[0] ?? null;
          setSelected(pick);
          setDetail(d);
        })
        .catch((e) => {
          if (seq === detailSeqRef.current) setError(String(e));
        })
        .finally(() => {
          if (seq === detailSeqRef.current) setLoading(false);
        });
      return;
    }
    traceListByTask(taskId ?? "")
      .then(async (list) => {
        if (seq !== detailSeqRef.current) return;
        setTraces(list);
        const pick = list.find((t) => t.id === pickId) ?? list[0] ?? null;
        setSelected(pick);
        const d = pick ? await traceDetail(pick.id) : null;
        // 二段 await 期间可能已有新选中/新 reload：过期即弃
        if (seq !== detailSeqRef.current) return;
        setDetail(d);
      })
      .catch((e) => {
        if (seq === detailSeqRef.current) setError(String(e));
      })
      .finally(() => {
        if (seq === detailSeqRef.current) setLoading(false);
      });
  };

  useEffect(() => {
    reload(traceId ?? null);
    // eslint-disable-next-line react/exhaustive-deps -- taskId/traceId 挂载期一次性加载；回滚后手动 reload
  }, [taskId, traceId]);

  // W10：运行审计按需加载（切到审计页签才拉，非工作流卡不拉）
  const loadAudit = () => {
    if (!workflowId) return;
    setAuditErr(null);
    listWorkflowAudit(workflowId, 300)
      .then(setAuditRows)
      .catch((e) => setAuditErr(String(e)));
  };

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
        {/* 头部：标题 + 导出 + 关闭 */}
        <div className="flex items-center gap-2">
          <History size={14} aria-hidden className="text-[var(--t4)]" />
          <h2 className="min-w-0 flex-1 truncate text-sm font-medium text-[var(--t1)]">
            执行详情{taskTitle ? ` · ${taskTitle}` : ""}
          </h2>
          {detail && (
            <button
              className="nm-btn rounded-lg px-2 py-1 text-[10px] text-[var(--t4)] hover:text-[var(--t2)] disabled:opacity-50"
              disabled={exportBusy}
              title="导出为 JSONL 文件（数据目录 exports/）"
              onClick={async () => {
                setExportBusy(true);
                setExportMsg(null);
                try {
                  setExportMsg(await traceExport(detail.id));
                } catch (e) {
                  setExportMsg(String(e));
                } finally {
                  setExportBusy(false);
                }
              }}
            >
              <Download size={11} aria-hidden className="mr-1 inline" />
              {exportBusy ? "导出中…" : "导出"}
            </button>
          )}
          <button
            className="nm-btn rounded-lg p-1 text-[var(--t4)] hover:text-[var(--t2)]"
            aria-label="关闭执行详情"
            onClick={onClose}
          >
            <X size={14} aria-hidden />
          </button>
        </div>
        {exportMsg && <p className="mt-1 break-all text-[10px] text-[var(--t4)]">{exportMsg}</p>}

        {/* W10：页签（工作流卡才有运行审计） */}
        {workflowId && (
          <div className="mt-2 flex gap-1">
            <button
              className={`nm-btn rounded-lg px-3 py-1 text-[11px] ${
                tab === "trace" ? "text-[var(--t1)] nm-inset" : "text-[var(--t4)]"
              }`}
              onClick={() => setTab("trace")}
            >
              执行痕迹
            </button>
            <button
              className={`nm-btn rounded-lg px-3 py-1 text-[11px] ${
                tab === "audit" ? "text-[var(--t1)] nm-inset" : "text-[var(--t4)]"
              }`}
              onClick={() => {
                setTab("audit");
                loadAudit(); // 每次进入页签都刷新（不设 null 守卫——防跨工作流残留旧数据）
              }}
            >
              <ListChecks size={11} aria-hidden className="mr-1 inline" />
              运行审计
            </button>
          </div>
        )}

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
                    // 切换即清上次加载错误并进加载态：避免旧错误残留 + 空态文案闪现
                    setError(null);
                    setLoading(true);
                    const seq = ++detailSeqRef.current;
                    traceDetail(t.id)
                      .then((d) => {
                        if (seq !== detailSeqRef.current) return;
                        setDetail(d);
                      })
                      .catch((e) => {
                        if (seq !== detailSeqRef.current) return;
                        setError(String(e));
                      })
                      .finally(() => {
                        if (seq === detailSeqRef.current) setLoading(false);
                      });
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

        {!loading && (!workflowId || tab === "trace") && !error && !detail && (
          <p className="mt-6 text-center text-xs text-[var(--t5)]">
            这张卡还没有执行痕迹——交给机器人跑一次后，这里会展示每次工具调用与文件修改。
          </p>
        )}

        {/* W10：运行审计视图（仅工作流卡 + audit 页签——无 workflowId 时
            auditRows 恒 null 会卡在"加载中"，必须与页签按钮同条件门控） */}
        {workflowId && tab === "audit" && (
          <div className="mt-3 min-h-0 flex-1 overflow-y-auto">
            {auditErr && (
              <p className="text-xs text-[var(--danger,#ef4444)]">
                <TriangleAlert size={11} aria-hidden className="mr-1 inline" />
                {auditErr}
              </p>
            )}
            {!auditErr && auditRows === null && (
              <p className="mt-2 text-center text-xs text-[var(--t5)]">加载中…</p>
            )}
            {auditRows?.length === 0 && (
              <p className="mt-2 text-center text-xs text-[var(--t5)]">
                还没有运行审计记录——执行一次工作流后这里会按 run 展示调度/验收/评审时间线。
              </p>
            )}
            {auditRows && auditRows.length > 0 && (
              <ol className="mt-1 space-y-1">
                {auditRows.map((r) => (
                  <li
                    key={r.id}
                    className="flex items-start gap-2 rounded-[var(--r-sm)] px-2 py-1 text-[11px] nm-inset"
                  >
                    <span className="shrink-0 tabular-nums text-[var(--t5)]">
                      {new Date(r.createdAt).toLocaleTimeString()}
                    </span>
                    <span
                      className={`shrink-0 ${
                        r.level === "warn"
                          ? "text-[var(--warn,#eab308)]"
                          : "text-[var(--t3)]"
                      }`}
                    >
                      {AUDIT_KIND_LABEL[r.kind] ?? r.kind}
                    </span>
                    <span className="min-w-0 flex-1 break-all text-[var(--t4)]">
                      {auditPayloadSummary(r)}
                    </span>
                  </li>
                ))}
              </ol>
            )}
          </div>
        )}

        {!loading && (!workflowId || tab === "trace") && detail && (
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
            {/* W10：节点级验收结论（引擎写 result，这里只读展示） */}
            {acceptanceInfo && (
              <p className="mt-2 rounded-[var(--r-sm)] px-2 py-1 text-[11px] nm-inset">
                <span
                  className={
                    acceptanceInfo.verdict === "pass"
                      ? "text-[var(--ok,#22c55e)]"
                      : acceptanceInfo.verdict === "partial"
                        ? "text-[var(--warn,#eab308)]"
                        : "text-[var(--danger,#ef4444)]"
                  }
                >
                  验收：{acceptanceInfo.verdict}
                </span>
                {acceptanceInfo.evidence && (
                  <span className="ml-2 text-[var(--t4)]">{acceptanceInfo.evidence}</span>
                )}
              </p>
            )}
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
