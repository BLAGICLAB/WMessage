// 首页面板：归档脉搏（映射归档页统计）/ 最近归档（归档卡列表）/
// 机器人动态（执行痕迹）/ 通知中心。
// 行为约定：面板只做聚合与跳转，完整操作（恢复/验收/提案裁决）留在各自模块页。

import type { Task } from "../../types";
import type { TraceRow } from "../../lib/trace";
import type { NotificationItem } from "../../lib/notifications";
import { relativeTime } from "../../format";
import { planColorVar } from "../TaskTimeline/week";
import type { ArchivePulse } from "./homeData";

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
  task_question: "任务提问",
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
        ) : notifications === null ? (
          // 加载中与 body 同口径：pending 用 null 表达未加载，不能落到「都处理完了」
          "加载中…"
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

/** 归档脉搏：本周归档 / 自动打标率 / 归档阈值 + 标签分布 chips（映射归档页统计） */
export function ArchivePulsePanel({
  pulse,
  onMore,
}: {
  pulse: ArchivePulse;
  onMore: () => void;
}) {
  return (
    <Panel
      dot="var(--plan-slate)"
      title="归档脉搏"
      moreLabel="归档页"
      onMore={onMore}
      footer={
        pulse.lastBatchDaysAgo === null
          ? "还没有归档过任务"
          : `最近一批 ${
              pulse.lastBatchDaysAgo === 0 ? "今天" : `${pulse.lastBatchDaysAgo} 天前`
            }归档，可在归档页恢复`
      }
    >
      <div className="flex gap-5">
        {[
          { v: String(pulse.weekCount), l: "本周归档" },
          {
            v: pulse.autoTagPct === null ? "—" : `${pulse.autoTagPct}%`,
            l: "自动打标",
          },
          { v: `${pulse.thresholdDays} 天`, l: "归档阈值" },
        ].map((s) => (
          <div key={s.l}>
            <div className="num text-[19px] font-semibold leading-none text-[var(--t1)]">
              {s.v}
            </div>
            <div className="mt-1 text-[11px] text-[var(--t5)]">{s.l}</div>
          </div>
        ))}
      </div>
      {pulse.topTags.length > 0 && (
        <div className="mt-3 flex flex-wrap gap-1.5">
          {pulse.topTags.map(({ tag, count }) => (
            <span
              key={tag}
              className="rounded-lg px-2 py-0.5 text-[11.5px] text-[var(--t2)]"
              style={
                {
                  "--c": planColorVar([tag]),
                  background: "color-mix(in srgb, var(--c) 10%, transparent)",
                  border: "1px solid color-mix(in srgb, var(--c) 32%, transparent)",
                } as React.CSSProperties
              }
            >
              {tag} <b className="num font-semibold">{count}</b>
            </span>
          ))}
        </div>
      )}
    </Panel>
  );
}

/** 最近归档：最近 10 张归档任务卡（按归档时间近似倒序），点行跳归档页编辑态 */
export function RecentArchivedPanel({
  pulse,
  onOpenTask,
  onMore,
}: {
  pulse: ArchivePulse;
  onOpenTask: (t: Task) => void;
  onMore: () => void;
}) {
  return (
    <Panel
      dot="var(--plan-slate)"
      title="最近归档"
      moreLabel="归档页"
      onMore={onMore}
      footer={`归档区共 ${pulse.total} 张卡`}
    >
      {pulse.recent.length === 0 ? (
        <p className="py-4 text-center text-[13px] text-[var(--t5)]">
          还没有归档的任务卡。
        </p>
      ) : (
        <div className="space-y-1" data-testid="recent-archived-list">
          {pulse.recent.map((t) => (
            <Row key={t.id} onClick={() => onOpenTask(t)}>
              <span
                aria-hidden
                className="h-[7px] w-[7px] shrink-0 rounded-full"
                style={{ background: planColorVar(t.tags) }}
              />
              <span className="min-w-0 flex-1 truncate text-[13px] text-[var(--t2)]">
                {t.title}
              </span>
              <span className="num shrink-0 text-[11px] text-[var(--t5)]">
                {t.updatedAt ? relativeTime(t.updatedAt) : ""}
              </span>
            </Row>
          ))}
        </div>
      )}
    </Panel>
  );
}
