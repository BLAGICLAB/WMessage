// ChatPanel 子模块：文件改动视图（右侧栏「文件」工作面内的详情页）。
//
// 为什么默认看改动而不是全文：用户点「本会话文件改动」几乎都是为了「确认刚才
// 改成了什么样」——全文得自己找变化点，改动视图直接给改动块 + 少量上下文。
// 全文仍然留一条路（「看全文」按钮），不是把全文藏起来。
//
// 口径由后端定（累计 / 逐次 / 无行级 diff 三档），这里只负责如实呈现：降级原因
// 与漂移提示必须显示——口径变了却不说，用户无从察觉，只会当成「就改了这些」。

import { ArrowLeft, FileText, Loader2, TriangleAlert } from "lucide-react";
import { DiffView } from "../TracePanel/DiffView";
import type { SessionFileDiff } from "../../lib/sessionFileChanges";
import { basename } from "../../format";

export function FileDiffView({
  path,
  detail,
  loading,
  error,
  onShowFull,
  onBack,
}: {
  path: string;
  detail: SessionFileDiff | null;
  loading: boolean;
  error: string | null;
  /** 切到全文视图（只读快照） */
  onShowFull: () => void;
  onBack: () => void;
}) {
  const name = basename(path);
  return (
    <div className="flex min-h-0 flex-col">
      {/* 返回行与子 Agent 详情同款：详情是一层，返回才回列表 */}
      <button
        type="button"
        onClick={onBack}
        className="mb-1 inline-flex items-center gap-1 text-[11px] text-[var(--t4)] hover:text-[var(--t2)]"
      >
        <ArrowLeft size={11} aria-hidden /> 返回文件列表
      </button>
      <div className="mb-1.5 flex items-start gap-1.5 px-0.5">
        <FileText
          size={12}
          aria-hidden
          className="mt-0.5 shrink-0 text-[var(--t5)]"
        />
        <div className="min-w-0 flex-1">
          <p className="truncate text-[11px] font-medium text-[var(--t2)]" title={path}>
            {name}
          </p>
          <p className="truncate text-[10px] text-[var(--t5)]" title={path}>
            {path}
          </p>
          {detail && (
            <p className="font-mono text-[10px] text-[var(--t5)]">
              <span className="text-[var(--success)]">+{detail.added}</span>{" "}
              <span className="text-[var(--danger)]">−{detail.deleted}</span>
              {detail.changes > 1 && (
                <span className="font-sans">
                  {" "}
                  · 本会话改了 {detail.changes} 次
                </span>
              )}
            </p>
          )}
        </div>
        <button
          type="button"
          onClick={onShowFull}
          title="查看文件全文（只读快照）"
          className="shrink-0 rounded px-1 py-0.5 text-[10px] text-[var(--t4)] hover:bg-[var(--hover-bg)] hover:text-[var(--t2)]"
        >
          看全文
        </button>
      </div>

      {error && (
        <p className="flex items-start gap-1 px-0.5 text-[11px] text-[var(--danger)]">
          <TriangleAlert size={11} aria-hidden className="mt-0.5 shrink-0" />
          {error}
        </p>
      )}
      {/* 降级原因 / 漂移提示：口径变了必须说出来 */}
      {detail?.note && (
        <p className="mb-1 rounded-md px-1.5 py-1 text-[10px] leading-relaxed text-[var(--t4)] nm-inset">
          {detail.note}
        </p>
      )}
      {!error && loading && (
        <p className="inline-flex items-center gap-1 px-0.5 text-[11px] text-[var(--t5)]">
          <Loader2 size={11} aria-hidden className="wmsg-activity" />
          读取改动…
        </p>
      )}

      {detail?.mode === "session" &&
        (detail.diff ? (
          <div className="min-h-0 flex-1 overflow-y-auto">
            <DiffView diff={detail.diff} truncated={detail.truncated} />
          </div>
        ) : (
          <p className="px-0.5 text-[11px] leading-relaxed text-[var(--t3)]">
            这个文件相对本会话开始时没有净变化。
          </p>
        ))}

      {detail?.mode === "perChange" && (
        <div className="min-h-0 flex-1 space-y-2 overflow-y-auto">
          {detail.perChange.map((c, i) => (
            <div key={c.id}>
              <p className="px-0.5 text-[10px] text-[var(--t5)]">
                第 {i + 1} 次改动 ·{" "}
                <span className="text-[var(--success)]">+{c.added}</span>{" "}
                <span className="text-[var(--danger)]">−{c.deleted}</span>
              </p>
              {c.diff ? (
                <DiffView diff={c.diff} truncated={c.truncated} />
              ) : (
                <p className="px-0.5 text-[10px] text-[var(--t5)]">
                  这次改动没有行级 diff
                </p>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
