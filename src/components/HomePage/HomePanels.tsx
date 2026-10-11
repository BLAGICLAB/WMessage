// 首页面板：机器人动态（执行痕迹）/ 通知中心。
// 行为约定：动态行点击跳对应模块，通知「去处理」跳通知中心——
// 首页只做聚合与跳转，完整操作（验收/回复/提案裁决）留在各自模块页。

import type { TraceRow } from "../../lib/trace";
import type { NotificationItem } from "../../lib/notifications";
import { relativeTime } from "../../format";

function fmtDur(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

function Panel({
  dot,
  title,
  moreLabel,
  onMore,
  children,
  footer,
}: {
  dot: string;
  title: string;
  moreLabel: string;
  onMore: () => void;
  children: React.ReactNode;
  footer: React.ReactNode;
}) {
  return (
    <section className="nm-card flex flex-col p-4">
      <div className="mb-3 flex items-center gap-2">
        <span
          aria-hidden
          className="h-[7px] w-[7px] shrink-0 rounded-full"
          style={{ background: dot }}
        />
        <h3 className="text-[13px] font-semibold text-[var(--t3)]">{title}</h3>
        <button
          onClick={onMore}
          className="ml-auto rounded-md px-2 py-0.5 text-xs text-[var(--t4)] transition-colors hover:bg-[var(--hover-bg)] hover:text-[var(--t2)]"
        >
          {moreLabel}
        </button>
      </div>
      <div className="flex-1">{children}</div>
      <div className="mt-3 border-t border-[var(--edge)] pt-2.5 text-xs text-[var(--t5)]">
        {footer}
      </div>
    </section>
  );
}

function Row({
  onClick,
  children,
}: {
  onClick?: () => void;
  children: React.ReactNode;
}) {
  const Tag = onClick ? "button" : "div";
  return (
    <Tag
      {...(onClick ? { onClick, type: "button" as const } : {})}
      className="flex w-full items-center gap-2.5 rounded-[var(--r-sm)] px-1.5 py-2 text-left transition-colors first:pt-1 hover:bg-[var(--hover-bg)]"
    >
      {children}
    </Tag>
  );
}

const PILL_CLS: Record<TraceRow["status"], string> = {
  running: "text-[var(--accent)] bg-[color-mix(in_srgb,var(--accent)_13%,transparent)]",
  done: "text-[var(--success)] bg-[color-mix(in_srgb,var(--success)_13%,transparent)]",
  failed: "text-[var(--danger)] bg-[color-mix(in_srgb,var(--danger)_13%,transparent)]",
  stopped: "text-[var(--t5)] bg-[var(--inset-bg)]",
  timeout: "text-[var(--t5)] bg-[var(--inset-bg)]",
};
const PILL_LABEL: Record<TraceRow["status"], string> = {
  running: "执行中",
  done: "完成",
  failed: "失败",
  stopped: "已停止",
  timeout: "超时",
};
const ORIGIN_LABEL: Record<string, string> = {
  manual: "手动",
  scheduled: "定时",
  batch: "批量",
  workflow: "工作流",
};

export function BotPanel({
  traces,
  traceError,
  now,
  onMore,
}: {
  traces: TraceRow[] | null;
  traceError: string;
  now: Date;
  onMore: () => void;
}) {
  const sorted = (traces ?? [])
    .slice()
    .sort((a, b) => {
      if ((a.status === "running") !== (b.status === "running"))
        return a.status === "running" ? -1 : 1;
      return b.startedAt - a.startedAt;
    })
    .slice(0, 4);
  const weekAgo = now.getTime() - 7 * 86_400_000;
  const week = (traces ?? []).filter((t) => t.startedAt >= weekAgo);
  const okCount = week.filter((t) => t.status === "done").length;
  const rate = week.length ? Math.round((okCount / week.length) * 100) : null;
  return (
    <Panel
      dot="var(--plan-violet)"
      title="机器人动态"
      moreLabel="活动页"
      onMore={onMore}
      footer={
        traceError ? (
          <span className="text-[var(--danger)]">{traceError}</span>
        ) : week.length ? (
          <>近 7 天执行 {week.length} 次{rate !== null ? ` · 成功率 ${rate}%` : ""}</>
        ) : (
          "近 7 天没有机器人执行"
        )
      }
    >
      {traces === null && !traceError ? (
        <p className="py-4 text-center text-[13px] text-[var(--t5)]">加载中…</p>
      ) : sorted.length === 0 ? (
        <p className="py-4 text-center text-[13px] text-[var(--t5)]">
          还没有执行痕迹——把任务交给机器人跑一次。
        </p>
      ) : (
        sorted.map((tr) => (
          <Row key={tr.id} onClick={onMore}>
            <span
              className={`shrink-0 rounded-md px-1.5 py-0.5 text-[10.5px] font-semibold ${PILL_CLS[tr.status]}`}
            >
              {PILL_LABEL[tr.status]}
            </span>
            <span className="min-w-0 flex-1 truncate text-[13px] text-[var(--t2)]">
              {tr.title
                ? `${ORIGIN_LABEL[tr.origin] ?? tr.origin} · ${tr.title}`
                : ORIGIN_LABEL[tr.origin] ?? tr.origin}
            </span>
            <span className="num shrink-0 text-[11px] text-[var(--t5)]">
              {relativeTime(tr.startedAt, now.getTime())}
              {tr.finishedAt ? ` · ${fmtDur(tr.finishedAt - tr.startedAt)}` : ""}
            </span>
          </Row>
        ))
      )}
    </Panel>
  );
}

const KIND_LABEL: Record<NotificationItem["kind"], string> = {
  memory_proposal: "记忆提案",
  evolution_proposal: "自进化提案",
  artifact_bind: "产物绑定",
  workflow_question: "工作流提问",
};

export function NotifyPanel({
  notifications,
  notifError,
  onMore,
}: {
  notifications: NotificationItem[] | null;
  notifError: string;
  onMore: () => void;
}) {
  const rows = (notifications ?? []).slice(0, 3);
  const pending = notifications?.length ?? 0;
  return (
    <Panel
      dot="var(--plan-amber)"
      title="通知"
      moreLabel="通知中心"
      onMore={onMore}
      footer={
        notifError ? (
          <span className="text-[var(--danger)]">{notifError}</span>
        ) : pending ? (
          `${pending} 条待处理`
        ) : (
          "都处理完了"
        )
      }
    >
      {notifications === null && !notifError ? (
        <p className="py-4 text-center text-[13px] text-[var(--t5)]">加载中…</p>
      ) : rows.length === 0 ? (
        <p className="py-4 text-center text-[13px] text-[var(--t5)]">
          没有待处理的消息。
        </p>
      ) : (
        rows.map((n) => (
          <Row key={n.id}>
            <span aria-hidden className="h-[7px] w-[7px] shrink-0 rounded-full bg-[var(--accent)]" />
            <span className="min-w-0 flex-1 truncate text-[13px] text-[var(--t2)]">
              {KIND_LABEL[n.kind]} · {n.title}
            </span>
            <button
              onClick={onMore}
              className="shrink-0 rounded-md border border-[var(--edge-strong)] px-2 py-0.5 text-[11px] text-[var(--t2)] transition-colors hover:bg-[var(--hover-bg)]"
            >
              去处理
            </button>
          </Row>
        ))
      )}
    </Panel>
  );
}
