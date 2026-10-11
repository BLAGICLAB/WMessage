// 首页：单机个人任务助手的落地仪表盘。
// 结构 = 吸顶收缩头部（滚动驱动，HomeHeader）→ 今日时间带 → 近 7 天联动
// 图表（Linked Hover）→ 三列面板（到期/机器人/通知）。
// 数据：tasks 由 App 下发；执行痕迹/通知本页拉取（30s 轻轮询 + 变更广播）。

import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Task } from "../../types";
import type { TraceRow } from "../../lib/trace";
import {
  listNotifications,
  NOTIFICATIONS_CHANGED_EVENT,
  type NotificationItem,
} from "../../lib/notifications";
import { useTauriListen } from "../../lib/useTauriListen";
import { formatCommandError } from "../../lib/errorHandler";
import {
  dailyTrend,
  dueSoonTasks,
  dueTimeOf,
  planPhases,
} from "./homeData";
import { HomeHeader, type HomeKpi } from "./HomeHeader";
import { TodayRibbon } from "./TodayRibbon";
import { TrendCharts } from "./TrendCharts";
import { BotPanel, DuePanel, NotifyPanel } from "./HomePanels";

const REFRESH_MS = 30_000;

export function HomePage({
  tasks,
  scrollRef,
  onNavigate,
  onOpenTask,
  onNewTask,
  now: nowProp,
}: {
  tasks: Task[];
  scrollRef: React.RefObject<HTMLElement | null>;
  onNavigate: (v: "board" | "activity" | "notifications") => void;
  onOpenTask: (t: Task) => void;
  onNewTask: () => void;
  /** 时钟注入口（测试确定性；生产缺省走真实时间 + 30s 轮询） */
  now?: Date;
}) {
  const [nowState, setNowState] = useState(() => new Date());
  const now = nowProp ?? nowState;
  useEffect(() => {
    if (nowProp) return;
    const iv = setInterval(() => setNowState(new Date()), REFRESH_MS);
    return () => clearInterval(iv);
  }, [nowProp]);

  // 执行痕迹：trace_list 全量来源（手动/定时/工作流/批量）；失败必须可见
  const [traces, setTraces] = useState<TraceRow[] | null>(null);
  const [traceError, setTraceError] = useState("");
  const loadTraces = useCallback(() => {
    invoke<TraceRow[]>("trace_list", { limit: 200 })
      .then((rows) => {
        setTraces(rows);
        setTraceError("");
      })
      .catch((e) => setTraceError(formatCommandError(e)));
  }, []);
  useEffect(() => {
    loadTraces();
    const iv = setInterval(loadTraces, REFRESH_MS);
    return () => clearInterval(iv);
  }, [loadTraces]);

  // 通知：pending 列表；后端任何写入/解决都会广播变更事件
  const [notifications, setNotifications] = useState<NotificationItem[] | null>(
    null,
  );
  const [notifError, setNotifError] = useState("");
  const loadNotifications = useCallback(() => {
    listNotifications("pending")
      .then((rows) => {
        setNotifications(rows);
        setNotifError("");
      })
      .catch((e) => setNotifError(formatCommandError(e)));
  }, []);
  useEffect(() => {
    loadNotifications();
  }, [loadNotifications]);
  useTauriListen(NOTIFICATIONS_CHANGED_EVENT, loadNotifications);

  const phases = useMemo(() => planPhases(tasks, now), [tasks, now]);
  const overdueCount = useMemo(
    () =>
      dueSoonTasks(tasks, now).filter(
        (t) => (dueTimeOf(t) ?? Infinity) < now.getTime(),
      ).length,
    [tasks, now],
  );
  const botRunsToday = useMemo(() => {
    if (traces === null) return 0;
    const start = new Date(now);
    start.setHours(0, 0, 0, 0);
    return traces.filter((t) => t.startedAt >= start.getTime()).length;
  }, [traces, now]);
  const trend = useMemo(
    () => dailyTrend(tasks, traces ?? [], now, 7),
    [tasks, traces, now],
  );

  const kpis: HomeKpi[] = [
    { value: phases.done + phases.doing + phases.todo, label: "今日计划" },
    { value: phases.done, label: "已完成" },
    { value: overdueCount, label: "已逾期", danger: true },
    { value: botRunsToday, label: "机器人执行" },
  ];

  return (
    <div className="pb-10" data-testid="home-page">
      <HomeHeader
        phases={phases}
        kpis={kpis}
        now={now}
        scrollRef={scrollRef}
        onNewTask={onNewTask}
      />

      <div className="mt-4 flex flex-col gap-3.5">
        <TodayRibbon tasks={tasks} now={now} onOpenTask={onOpenTask} />
        <TrendCharts data={trend} />
        <div className="grid grid-cols-1 gap-3.5 lg:grid-cols-3">
          <DuePanel
            tasks={tasks}
            now={now}
            onMore={() => onNavigate("board")}
            onOpenTask={onOpenTask}
          />
          <BotPanel
            traces={traces}
            traceError={traceError}
            now={now}
            onMore={() => onNavigate("activity")}
          />
          <NotifyPanel
            notifications={notifications}
            notifError={notifError}
            onMore={() => onNavigate("notifications")}
          />
        </div>
      </div>
    </div>
  );
}
