// 执行痕迹面板（Agent 透明化设计 §5.2）：任务卡的「执行详情」。
//
// 数据源 =  的三表（db/trace.rs 查询命令 trace_list/trace_detail）：
// - 摘要头：状态/耗时/轮数/工具数/改文件数/tokens
// - 时间线：每次工具调用一行（入参/结果/耗时/成败，<details> 展开）
// - 文件变更：±行 + unified diff 着色（DiffView）+ 「回滚」按钮（漂移闸拒绝时展示原因）
//
// 弹层 portal 到 body——TodoCard 容器的 hover transform 会创建 CSS
// 包含块，position:fixed 子元素会被裁缩到卡片内（Modal 的 portal prop）。

import { useEffect, useRef, useState } from "react";
import { Download, FileText, History, TriangleAlert, X } from "lucide-react";
import { Modal } from "../../ui/Modal";
import { stopPointer } from "../../lib/stopPointer";
import { basename, formatMonthDayTime } from "../../format";
import { DiffView } from "./DiffView";
import {
  fileRollback,
  traceDetail,
  traceExport,
  traceListByOrigin,
  traceListBySession,
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
};

/** span 失败分类（后端 audit::classify_error_class 的九类）→ 中文短词。
 *  值域跟后端分类器一一对应，新增一类必须在此补词；未登记值不退化成统一
 *  「失败」——那样新分类上线后前端看不出差别，兜底走「失败（原值）」 */
const ERROR_CLASS_LABEL: Record<string, string> = {
  unknown_tool: "工具不存在",
  gate_denied: "门禁拦截",
  user_denied: "用户拒绝",
  skill_paused: "技能暂停",
  fused: "已熔断",
  timeout: "超时",
  not_found: "未找到",
  permission: "权限不足",
  exec_error: "执行失败",
};

/** span 行尾的失败短词。errorClass 为空（旧行/成功侧未落）走通用「失败」 */
function spanFailText(s: SpanRow): string {
  const cls = s.errorClass;
  if (!cls) return "失败";
  return ERROR_CLASS_LABEL[cls] ?? `失败（${cls}）`;
}

function fmtMs(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
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
          <span className="font-mono text-[11px] text-[var(--t3)]">
            {s.name}
          </span>
          <span className="ml-1.5 text-[10px] text-[var(--t5)]">
            第{s.turn}轮{s.durationMs != null && ` · ${fmtMs(s.durationMs)}`}
            {!s.ok && (
              <span className="text-[var(--danger,#ef4444)]">
                {" "}
                · {spanFailText(s)}
              </span>
            )}
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
function FileChangeItem({
  c,
  onRolledBack,
}: {
  c: FileChangeRow;
  onRolledBack: () => void;
}) {
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
        <span
          className="min-w-0 flex-1 truncate font-mono text-[11px] text-[var(--t3)]"
          title={c.path}
        >
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
          <summary className="cursor-pointer select-none text-[10px] text-[var(--t4)]">
            查看 diff
          </summary>
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
  traceId,
  sessionId,
  origin,
  onClose,
}: {
  /** 按任务卡查（执行历史列表 → 选一条）；与 sessionId/origin/traceId 互斥 */
  taskId?: string;
  taskTitle?: string;
  /**  直查模式：活动页按 trace id 直接打开单条 */
  traceId?: number;
  /** 按会话查（同会话的多次执行 → 选一条）；聊天回合跳执行轨迹用它。
   *  与 traceId 同时给 = 列表模式并预选这一条 */
  sessionId?: string;
  /** 按来源查（手动/定时/批量/聊天） */
  origin?: string;
  onClose: () => void;
}) {
  const [traces, setTraces] = useState<TraceRow[]>([]);
  const [selected, setSelected] = useState<TraceRow | null>(null);
  const [detail, setDetail] =
    useState<Awaited<ReturnType<typeof traceDetail>>>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // JSONL 导出（按钮态 + 结果路径/错误展示）
  const [exportBusy, setExportBusy] = useState(false);
  const [exportMsg, setExportMsg] = useState<string | null>(null);

  // 竞态令牌：快速连点历史条目时，慢的旧 traceDetail 响应不得覆盖新选中的详情
  const detailSeqRef = useRef(0);

  // 列表来源三选一：会话 → 来源 → 任务卡
  const loadList = () =>
    sessionId
      ? traceListBySession(sessionId)
      : origin
        ? traceListByOrigin(origin)
        : traceListByTask(taskId ?? "");

  const reload = (pickId: number | null) => {
    // 开头即作废在途响应：列表模式下慢的旧 reload 不得覆盖新选中的详情
    const seq = ++detailSeqRef.current;
    setLoading(true);
    setError(null);
    // 直查模式（只给了 traceId）：单条结果即完整 detail，直接复用不再重复查询；
    // 列表模式（任务卡/会话/来源）拉执行历史列表后按选中项再取详情
    if (traceId != null && !sessionId && !origin) {
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
    loadList()
      .then(async (list) => {
        if (seq !== detailSeqRef.current) return;
        setTraces(list);
        const pick =
          pickId != null
            ? (list.find((t) => t.id === pickId) ?? null)
            : (list[0] ?? null);
        if (pick) {
          setSelected(pick);
          const d = await traceDetail(pick.id);
          // 二段 await 期间可能已有新选中/新 reload：过期即弃
          if (seq !== detailSeqRef.current) return;
          setDetail(d);
          return;
        }
        if (pickId != null) {
          // 目标执行不在本页（会话执行多、已翻出 limit 之外）：直查单条，
          // 不回退到列表里的其它执行——回退等于给用户看错的那一次
          const d = await traceDetail(pickId);
          if (seq !== detailSeqRef.current) return;
          setSelected(d);
          setDetail(d);
          return;
        }
        setSelected(null);
        setDetail(null);
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
    // eslint-disable-next-line react/exhaustive-deps -- 列表来源挂载期一次性加载；回滚后手动 reload
  }, [taskId, traceId, sessionId, origin]);

  const statusOf = (t: TraceRow | null) =>
    t ? (STATUS_BADGE[t.status] ?? null) : null;
  const duration = (t: TraceRow): string =>
    t.finishedAt ? fmtMs(t.finishedAt - t.startedAt) : "进行中";

  return (
    <Modal
      className="fixed inset-0 z-[100] flex items-center justify-center bg-black/40 p-6"
      onClose={onClose}
      portal
    >
      <div
        className="nm-card flex max-h-[80vh] w-full max-w-2xl flex-col p-4"
        onPointerDown={stopPointer}
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
        {exportMsg && (
          <p className="mt-1 break-all text-[10px] text-[var(--t4)]">
            {exportMsg}
          </p>
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
                    selected?.id === t.id
                      ? "text-[var(--t1)]"
                      : "text-[var(--t4)]"
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
                  {formatMonthDayTime(t.startedAt)}
                  {badge && (
                    <span className={`ml-1 ${badge.cls}`}>{badge.label}</span>
                  )}
                </button>
              );
            })}
          </div>
        )}

        {loading && (
          <p className="mt-4 text-center text-xs text-[var(--t5)]">加载中…</p>
        )}
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
                  <span className={`nm-inset px-2 py-0.5 ${badge.cls}`}>
                    {badge.label}
                  </span>
                ) : null;
              })()}
              <span className="nm-inset px-2 py-0.5 tabular-nums">
                {duration(detail)}
              </span>
              <span className="nm-inset px-2 py-0.5 tabular-nums">
                {detail.turnCount} 轮 · {detail.toolCalls} 次工具 ·{" "}
                {detail.filesChanged} 个文件
              </span>
              {(detail.promptTokens > 0 ||
                detail.completionTokens > 0 ||
                detail.cacheReadTokens > 0 ||
                detail.cacheWriteTokens > 0) && (
                <span
                  className="nm-inset px-2 py-0.5 tabular-nums"
                  title={
                    detail.cacheReadTokens > 0 || detail.cacheWriteTokens > 0
                      ? `tokens 输入 ${detail.promptTokens} / 输出 ${detail.completionTokens} · 缓存读 ${detail.cacheReadTokens} / 缓存写 ${detail.cacheWriteTokens}（缓存档单价不同，不并入上面前两项）`
                      : undefined
                  }
                >
                  tokens {detail.promptTokens}/{detail.completionTokens}
                  {(detail.cacheReadTokens > 0 || detail.cacheWriteTokens > 0) &&
                    ` · 缓存 ${detail.cacheReadTokens}/${detail.cacheWriteTokens}`}
                </span>
              )}
              {detail.origin && (
                <span className="nm-inset px-2 py-0.5">{detail.origin}</span>
              )}
              {detail.model && (
                <span
                  className="nm-inset px-2 py-0.5"
                  title="本轮实际调用的模型"
                >
                  {detail.model}
                </span>
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
                    <FileChangeItem
                      key={c.id}
                      c={c}
                      onRolledBack={() => reload(detail.id)}
                    />
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
    </Modal>
  );
}
