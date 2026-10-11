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
  getProfileCache,
  loadProfile,
  subscribeProfile,
} from "../../profile";
import {
  dailyTrend,
  poolCounts,
  planPhases,
} from "./homeData";
import { HomeHeader } from "./HomeHeader";
import { TodayRibbon } from "./TodayRibbon";
import { TrendCharts } from "./TrendCharts";
import { BotPanel, DuePanel, NotifyPanel } from "./HomePanels";

const REFRESH_MS = 30_000;

export function HomePage({
  tasks,
  scrollRef,
  onNavigate,
  onOpenTask,
  now: nowProp,
}: {
  tasks: Task[];
  scrollRef: React.RefObject<HTMLElement | null>;
  onNavigate: (v: "board" | "activity" | "notifications") => void;
  onOpenTask: (t: Task) => void;
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

  // 用户名（设置里个人资料）：缓存优先，变更广播同步；加载失败已由
  // loadProfile 内 errorHandler 上报，这里降级为不带姓名的问候
  const [userName, setUserName] = useState(
    () => getProfileCache()?.user.name ?? "",
  );
  useEffect(() => {
    let alive = true;
    loadProfile()
      .then((p) => {
        if (alive) setUserName(p.user.name);
      })
      .catch(() => setUserName(getProfileCache()?.user.name ?? ""));
    const un = subscribeProfile(() => {
      setUserName(getProfileCache()?.user.name ?? "");
    });
    return () => {
      alive = false;
      un();
    };
  }, []);

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
  const pools = useMemo(() => poolCounts(tasks), [tasks]);
  const trend = useMemo(
    () => dailyTrend(tasks, traces ?? [], now, 7),
    [tasks, traces, now],
  );

  return (
    <div className="pb-10" data-testid="home-page">
      <HomeHeader
        name={userName}
        phases={phases}
        pools={pools}
        now={now}
        scrollRef={scrollRef}
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
