// 首页三列面板：即将到期（任务）/ 机器人动态（执行痕迹）/ 通知中心。
// 行为约定：到期/动态行点击跳对应模块，通知「去处理」跳通知中心——
// 首页只做聚合与跳转，完整操作（验收/回复/提案裁决）留在各自模块页。

import type { Task } from "../../types";
import type { TraceRow } from "../../lib/trace";
import type { NotificationItem } from "../../lib/notifications";
import { dueTimeOf, dueSoonTasks } from "./homeData";
import { relativeTime } from "../../format";
import { planColorVar } from "../TaskTimeline/week";

const WEEKDAY = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];

function fmtHM(min: number): string {
  return `${String(Math.floor(min / 60)).padStart(2, "0")}:${String(min % 60).padStart(2, "0")}`;
}

/** 截止角标文案：逾期 n 天（红）/ 今天 / 明天 / 周X（7 天窗内） */
function dueChipOf(t: Task, now: Date): { text: string; danger: boolean } {
  const due = dueTimeOf(t)!;
  const startOfToday = new Date(now);
  startOfToday.setHours(0, 0, 0, 0);
  const dayOffset = Math.floor((due - startOfToday.getTime()) / 86_400_000);
  const showTime = (t.due?.length ?? 0) > 10;
  const hm = (ms: number) => {
    const d = new Date(ms);
    return fmtHM(d.getHours() * 60 + d.getMinutes());
  };
  if (due < startOfToday.getTime()) {
    const overdueDays = Math.max(1, -dayOffset);
    return { text: `逾期 ${overdueDays} 天`, danger: true };
  }
  if (dayOffset === 0)
    return { text: `今天${showTime ? ` ${hm(due)}` : ""}`, danger: false };
  if (dayOffset === 1)
    return { text: `明天${showTime ? ` ${hm(due)}` : ""}`, danger: false };
  return {
    text: `${WEEKDAY[new Date(due).getDay()]}${showTime ? ` ${hm(due)}` : ""}`,
    danger: false,
  };
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

function fmtDur(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

export function DuePanel({
  tasks,
  now,
  onMore,
  onOpenTask,
}: {
  tasks: Task[];
  now: Date;
  onMore: () => void;
  onOpenTask: (t: Task) => void;
}) {
  const due = dueSoonTasks(tasks, now);
  const rows = due.slice(0, 5);
  return (
    <Panel
      dot="var(--plan-blue)"
      title="即将到期"
      moreLabel="全部任务"
      onMore={onMore}
      footer={`截止日=今天的任务会自动进「今日」`}
    >
      {rows.length === 0 ? (
        <p className="py-4 text-center text-[13px] text-[var(--t5)]">
          未来 7 天没有要撞线的任务。
        </p>
      ) : (
        rows.map((t) => {
          const chip = dueChipOf(t, now);
          return (
            <Row key={t.id} onClick={() => onOpenTask(t)}>
              <span
                aria-hidden
                className="h-[7px] w-[7px] shrink-0 rounded-full"
                style={{ background: planColorVar(t.tags) }}
              />
              <span className="min-w-0 flex-1 truncate text-[13px] text-[var(--t2)]">
                {t.title}
              </span>
              <span
                className={`num shrink-0 rounded-md border px-1.5 py-0.5 text-[11px] ${
                  chip.danger
                    ? "border-[color-mix(in_srgb,var(--danger)_40%,transparent)] font-semibold text-[var(--danger)]"
                    : "border-[var(--edge)] text-[var(--t4)]"
                }`}
              >
                {chip.text}
              </span>
            </Row>
          );
        })
      )}
    </Panel>
  );
}

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
        <div className="flex h-full flex-col justify-evenly">
          {rows.map((n) => (
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
          ))}
        </div>
      )}
    </Panel>
  );
}
