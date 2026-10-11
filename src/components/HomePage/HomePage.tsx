// 首页：单机个人任务助手的落地仪表盘。
// 结构 = 吸顶收缩头部（HomeHeader）→ 首行（每日一图+到期横条｜本周计划完成
// 柱状图｜通知）→ 第二行（图谱速览｜本周日程｜任务卡分页器）→ 机器人动态。
// 四个小组件按「日期 + 任务卡」做 Linked Hover：共享 hover 状态（activeDay /
// activeTaskId），周图橙色竖线、日程列参考线、任务卡自动翻页、图谱节点高亮。
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
  poolCounts,
  planPhases,
  weeklyBuckets,
  plannedTasksByDay,
} from "./homeData";
import { HomeHeader } from "./HomeHeader";
import { BingCard } from "./BingCard";
import { DueTicker } from "./DueTicker";
import { WeeklyChart } from "./WeeklyChart";
import { GraphMini } from "./GraphMini";
import { DayGridMini } from "./DayGridMini";
import { PoolPager } from "./PoolPager";
import { BotPanel, NotifyPanel } from "./HomePanels";

const REFRESH_MS = 30_000;

/** 跨小组件联动状态：指向的周内天（0=周一）与具体任务卡（可空） */
export type HomeHover = { day: number | null; taskId: string | null };

export function HomePage({
  tasks,
  scrollRef,
  onNavigate,
  onOpenTask,
  now: nowProp,
}: {
  tasks: Task[];
  scrollRef: React.RefObject<HTMLElement | null>;
  onNavigate: (v: "board" | "activity" | "notifications" | "graph") => void;
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
  const week = useMemo(() => weeklyBuckets(tasks, now), [tasks, now]);
  const { keys: dayKeys } = useMemo(
    () => plannedTasksByDay(tasks, now),
    [tasks, now],
  );

  // Linked Hover 共享状态：日期 + 任务卡（周图/日程/任务卡/图谱四组件共用）
  const [hover, setHover] = useState<HomeHover>({ day: null, taskId: null });
  const hoverDay = (day: number | null) => setHover({ day, taskId: null });
  const hoverTask = (task: Task | null) => {
    if (!task) return setHover({ day: null, taskId: null });
    const day = task.planStart
      ? dayKeys.findIndex((k) => k === task.planStart!.slice(0, 10))
      : -1;
    setHover({ day: day === -1 ? null : day, taskId: task.id });
  };

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
        {/* 三行网格：r1 图+通知，r2 到期横条+（周图跨 r1-r2），r3 图谱+日程。
            右列整列是一个纵向 flex（通知 + 任务卡 flex-1）：任务卡上沿贴通知
            下沿、下沿与周图/日程底边齐平，中段空档由任务卡吃掉 */}
        <div className="grid grid-cols-1 gap-3.5 lg:grid-cols-[minmax(0,37fr)_minmax(0,37fr)_minmax(0,26fr)] lg:grid-rows-[auto_minmax(44px,auto)_auto]">
          <div className="lg:col-start-1 lg:row-start-1">
            <BingCard />
          </div>
          <div className="lg:col-start-1 lg:row-start-2">
            <DueTicker tasks={tasks} now={now} onOpenTask={onOpenTask} />
          </div>
          <div className="lg:col-start-1 lg:row-start-3">
            <GraphMini
              tasks={tasks}
              dayKeys={dayKeys}
              activeDay={hover.day}
              activeTaskId={hover.taskId}
              onHoverTask={hoverTask}
              onOpenTask={onOpenTask}
              onMore={() => onNavigate("graph")}
            />
          </div>
          <div className="lg:col-start-2 lg:row-start-1 lg:row-span-2">
            <WeeklyChart
              data={week}
              activeDay={hover.day}
              onActiveDay={hoverDay}
            />
          </div>
          <div className="lg:col-start-2 lg:row-start-3">
            <DayGridMini
              tasks={tasks}
              now={now}
              activeDay={hover.day}
              activeTaskId={hover.taskId}
              onActiveDay={hoverDay}
              onHoverTask={hoverTask}
              onOpenTask={onOpenTask}
              onMore={() => onNavigate("board")}
            />
          </div>
          <div className="flex flex-col gap-3.5 lg:col-start-3 lg:row-start-1 lg:row-span-3">
            <NotifyPanel
              notifications={notifications}
              notifError={notifError}
              onMore={() => onNavigate("notifications")}
            />
            <div className="min-h-0 flex-1">
              <PoolPager
                tasks={tasks}
                now={now}
                activeDay={hover.day}
                activeTaskId={hover.taskId}
                onActiveDay={hoverDay}
                onHoverTask={hoverTask}
                onOpenTask={onOpenTask}
                onMore={() => onNavigate("board")}
              />
            </div>
          </div>
        </div>

        <BotPanel
          traces={traces}
          traceError={traceError}
          now={now}
          onMore={() => onNavigate("activity")}
        />
      </div>
    </div>
  );
}
