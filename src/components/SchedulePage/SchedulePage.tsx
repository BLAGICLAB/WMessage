import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { isPermissionGranted } from "@tauri-apps/plugin-notification";
import { AlarmClock, Bell, Plus } from "lucide-react";
import { useTauriListen } from "../../lib/useTauriListen";
import { handleCommandError, formatCommandError } from "../../lib/errorHandler";
import { formatSchedule, relativeTime, untilTime } from "../../format";
import { useRaceSeq } from "../../lib/useRaceSeq";
import { EmptyState } from "../EmptyState";
import { TracePanel } from "../TracePanel";
import { ScheduleEntryEditor } from "./ScheduleEntryEditor";
import { ReminderRow } from "./ReminderRow";
import { JobRunList } from "./JobRunList";
import type {
  FinishedJob,
  JobEntry,
  ReminderEntry,
  ScheduleEntry,
  ScheduledJobRun,
} from "../../types";

/**
 * 定时模块：列表 = 状态面板（不是设置表单的集合）。
 * 数据单源：schedule_overview（nextRunAt/missed 由 Rust 端计算——不在前端重写
 * schedule 解析器，weekly:NaN 双端解析漂移是历史事故）。
 *
 * 两类条目用标签切换分开看（不用分页：分页在本仓的语义是「换时间段」，作业与
 * 提醒之间没有时间关系）：
 * - 定时任务（kind="job"）：用户写「要做什么」，到点后端新建任务卡并交给机器人执行
 *   （scheduled_job_create/update/delete/fire 四命令，作业即配置，取消 = 删作业）。
 * - 提醒（kind="reminder"）：到点只发一条系统通知，不建卡不执行
 *   （reminder_create/update/delete/set_enabled 四命令）。
 */

/** 行/目标的稳定键（带 kind 前缀，与后端条目键一致） */
const keyOf = (kind: ScheduleEntry["kind"], id: string) => `${kind}:${id}`;

export function SchedulePage({ onOpenChat }: { onOpenChat: () => void }) {
  const [entries, setEntries] = useState<ScheduleEntry[]>([]);
  const [loaded, setLoaded] = useState(false);
  /** 首次列表加载失败信息（null=正常）：失败不能伪装成「暂无定时任务」空态 */
  const [loadError, setLoadError] = useState<string | null>(null);
  /** 行内修改面板展开目标（keyOf 形式） */
  const [editKey, setEditKey] = useState<string | null>(null);
  /** 执行历史展开目标（job id；展开时拉一次，不自动刷新——历史是排障视角，不是监控大盘） */
  const [historyOpenId, setHistoryOpenId] = useState<string | null>(null);
  const [history, setHistory] = useState<Record<string, ScheduledJobRun[]>>({});
  /** 加载中的作业 id（按 job 记：连开两个作业的历史时，先完成的不得清掉后一个的加载态） */
  const [historyLoadingId, setHistoryLoadingId] = useState<string | null>(null);
  /** 取消定时两步确认：armed 的 key，第二次点击才真清（3s 复位） */
  const [cancelArmed, setCancelArmed] = useState<string | null>(null);
  const armedTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** 立即执行在途（防连点；后端另有防重入兜底） */
  const [busyKeys, setBusyKeys] = useState<Set<string>>(new Set());
  /** 新建流程展开态（写内容 → 面板 → 预览确认） */
  const [creating, setCreating] = useState<boolean>(false);
  /** 分组标签：定时任务 / 提醒。默认作业——作业会自己做事，失败与错过需要人看；
   *  提醒只是响一声。标签不记忆：每次进来都回到作业 tab，作业的失败不会被漏看。 */
  const [tab, setTab] = useState<"jobs" | "reminders">("jobs");
  /** 系统通知未授权（提醒到点会静默失败）：只在提醒 tab 提示——作业不依赖系统通知 */
  const [notifyBlocked, setNotifyBlocked] = useState(false);
  //  执行透明：sched-status 事件驱动的「执行中」作业（started 入集，done/failed 出集）
  const [liveJobIds, setLiveJobIds] = useState<Set<string>>(new Set());
  /** job id → 最近一次执行会话（sched-status 携带；「会话」按钮跳聊天页围观） */
  const [jobSessions, setJobSessions] = useState<Record<string, string>>({});
  /** 执行痕迹弹层目标（本次执行新建的任务卡 id） */
  const [traceCardId, setTraceCardId] = useState<string | null>(null);
  /** 已结束的一次性作业（作业行已被消费掉、执行记录留档的） */
  const [finished, setFinished] = useState<FinishedJob[]>([]);
  const [finishedError, setFinishedError] = useState<string | null>(null);
  /** 已结束区展开目标（作业 id） */
  const [finishedOpenId, setFinishedOpenId] = useState<string | null>(null);

  useEffect(
    () => () => {
      if (armedTimerRef.current) clearTimeout(armedTimerRef.current);
    },
    [],
  );

  // 竞态令牌：reload 由挂载/tasks-changed/sched-status 并发触发，慢的旧响应
  // 不得覆盖新响应
  const race = useRaceSeq();
  const reload = useCallback(async () => {
    const seq = race.next();
    try {
      const list = await invoke<ScheduleEntry[]>("schedule_overview");
      if (!race.isCurrent(seq)) return;
      setEntries(list);
      setLoaded(true);
      setLoadError(null);
    } catch (e) {
      if (!race.isCurrent(seq)) return;
      handleCommandError(e, "读取定时任务列表", { silent: true });
      // 失败不置 loaded：留错误行 + 重试入口，不渲染成「暂无定时任务」空态
      setLoadError(formatCommandError(e));
    }
  }, [race]);

  useEffect(() => {
    // 首次挂载拉列表：setState 都在 invoke await 之后，lint 误报同步 setState
    // oxlint-disable-next-line react/set-state-in-effect
    void reload();
  }, [reload]);

  /** 已结束的一次性作业：作业行被系统消费掉后不在列表里了，它的执行记录
   *  留档 30 天——这批留档单独拉。失败不吞：留着错误行，比显示成「没有已结束
   *  的作业」诚实（空态会被读成「从没跑过」）。 */
  const reloadFinished = useCallback(async () => {
    try {
      // ?? []：命令回空/异常形态时不得让整页崩（列表区读 length 会炸）
      const list = await invoke<FinishedJob[]>("scheduled_job_finished");
      setFinished(list ?? []);
      setFinishedError(null);
    } catch (e) {
      handleCommandError(e, "读取已结束的定时任务", { silent: true });
      setFinishedError(formatCommandError(e));
    }
  }, []);
  useEffect(() => {
    // oxlint-disable-next-line react/set-state-in-effect
    void reloadFinished();
  }, [reloadFinished]);
  // 到点执行会新建任务卡 → tasks-changed 广播，顺手刷新「上次执行」等状态
  useTauriListen("tasks-changed", () => {
    void reload();
  });
  // 提醒到点触发/删除 → reminders-changed 广播。提醒是唯一在用户视线外自己变化的
  // 一类条目（一次性提醒触发后行会消失、周期提醒触发次数会涨），不订阅就停在旧快照上。
  useTauriListen("reminders-changed", () => {
    void reload();
  });

  /** 系统通知授权状态：提醒到点的唯一效果就是那声通知，未授权时静默失败，
   *  用户只会觉得「提醒坏了」。查不到状态 ≠ 未授权（插件不可用时不报假警）。 */
  const checkNotifyPermission = useCallback(async () => {
    try {
      setNotifyBlocked(!(await isPermissionGranted()));
    } catch {
      setNotifyBlocked(false);
    }
  }, []);
  useEffect(() => {
    if (tab !== "reminders") return;
    // setState 在 await 之后（授权查询是异步的），lint 误报同步 setState
    // oxlint-disable-next-line react/set-state-in-effect
    void checkNotifyPermission();
  }, [tab, checkNotifyPermission]);

  //  执行透明：sched-status 实时驱动「执行中」徽标 + 会话跳转锚点；
  // done/failed 时刷新列表（lastStatus/sched_last 已在任务行更新）
  useTauriListen<{
    jobId?: string;
    phase?: "started" | "done" | "failed" | "skipped";
    sessionId?: string | null;
  }>("sched-status", (payload) => {
    // 作业被消费（done/failed）→ 已结束的一次性作业列表会多一条，跟着刷新。
    // 放在 jobId 判定之前：消费事件未必带 jobId
    if (payload.phase === "done" || payload.phase === "failed") {
      void reloadFinished();
    }
    const jobId = payload.jobId;
    if (!jobId) return;
    if (payload.phase === "started") {
      // oxlint-disable-next-line react/set-state-in-effect
      setLiveJobIds((prev) => new Set(prev).add(jobId));
      return;
    }
    setLiveJobIds((prev) => {
      const next = new Set(prev);
      next.delete(jobId);
      return next;
    });
    if (payload.sessionId) {
      // oxlint-disable-next-line react/set-state-in-effect
      setJobSessions((prev) => ({ ...prev, [jobId]: payload.sessionId! }));
    }
    void reload();
  });

  /** 「会话」跳转：切到主窗口聊天页，由 ChatPanel 切到该会话 */
  const jumpToSession = (sessionId: string) => {
    onOpenChat();
    emit("chat-focus-session", { sessionId }).catch(() => {});
  };

  // 行操作

  const runNow = async (e: JobEntry) => {
    const key = keyOf(e.kind, e.targetId);
    if (busyKeys.has(key)) return;
    setBusyKeys((prev) => new Set(prev).add(key));
    try {
      // 与到点同款链路：后端新建任务卡并交给机器人跑
      await invoke("scheduled_job_fire", { id: e.targetId });
    } catch (err) {
      handleCommandError(err, "立即执行");
    } finally {
      setBusyKeys((prev) => {
        const next = new Set(prev);
        next.delete(key);
        return next;
      });
    }
  };

  const toggleEnabled = async (e: JobEntry) => {
    try {
      await invoke("schedule_set_enabled", {
        kind: e.kind,
        id: e.targetId,
        enabled: !e.enabled,
      });
      void reload();
    } catch (err) {
      handleCommandError(err, e.enabled ? "暂停定时" : "恢复定时");
    }
  };

  /** 执行档切换：auto ↔ preview（预演 = 到点只建提案卡+通知，确认后「立即执行」真跑） */
  const toggleMode = async (e: JobEntry) => {
    const next = e.mode === "preview" ? "auto" : "preview";
    try {
      await invoke("scheduled_job_update", { id: e.targetId, mode: next });
      void reload();
    } catch (err) {
      handleCommandError(
        err,
        next === "preview" ? "切到预演档" : "切到自动档",
      );
    }
  };

  /** 展开/收起执行历史：展开时拉一次（倒序，后端每作业最多 20 条） */
  const toggleHistory = async (jobId: string) => {
    if (historyOpenId === jobId) {
      setHistoryOpenId(null);
      return;
    }
    setHistoryOpenId(jobId);
    if (history[jobId]) return; // 已拉过：直接展示缓存
    setHistoryLoadingId(jobId);
    try {
      const runs = await invoke<ScheduledJobRun[]>("scheduled_job_history", {
        id: jobId,
      });
      setHistory((prev) => ({ ...prev, [jobId]: runs }));
    } catch (e) {
      handleCommandError(e, "读取执行历史", { silent: true });
    } finally {
      // 只清「自己这条」：期间又点了别的作业时，其加载态不能被顺手清掉
      setHistoryLoadingId((cur) => (cur === jobId ? null : cur));
    }
  };

  /** 取消定时：两步确认，3s 未确认自动复位。作业 = 删作业 */
  const cancelSchedule = (e: JobEntry) => {
    const key = keyOf(e.kind, e.targetId);
    if (cancelArmed !== key) {
      setCancelArmed(key);
      if (armedTimerRef.current) clearTimeout(armedTimerRef.current);
      armedTimerRef.current = setTimeout(() => setCancelArmed(null), 3000);
      return;
    }
    if (armedTimerRef.current) clearTimeout(armedTimerRef.current);
    setCancelArmed(null);
    void (async () => {
      await invoke("scheduled_job_delete", { id: e.targetId }).catch((err) =>
        handleCommandError(err, "删除定时任务"),
      );
      void reload();
    })();
  };

  // 新建流程

  const startCreate = () => setCreating(true);

  /** 作业落库（新建/编辑共用）：content+schedule+重试配置都实传 */
  const saveJob = async (
    id: string | null,
    content: string,
    schedule: string,
    retryMax: number,
    pauseOnFailure: boolean,
  ): Promise<boolean> => {
    try {
      if (id === null) {
        await invoke("scheduled_job_create", {
          content,
          schedule,
          retryMax,
          pauseOnFailure,
        });
      } else {
        await invoke("scheduled_job_update", {
          id,
          content,
          schedule,
          retryMax,
          pauseOnFailure,
        });
      }
      setCreating(false);
      setEditKey(null);
      void reload();
      return true;
    } catch (e) {
      handleCommandError(e, id === null ? "新建定时任务" : "修改定时任务");
      return false;
    }
  };

  /** 新建提醒（落库后重拉列表） */
  const saveReminder = async (content: string, schedule: string) => {
    try {
      await invoke("reminder_create", { text: content, schedule });
      setCreating(false);
      void reload();
    } catch (e) {
      handleCommandError(e, "新建提醒");
    }
  };

  /** 切标签：收起新建面板与行内编辑——面板形态随标签变，半途切会把作业内容写成提醒 */
  const switchTab = (k: "jobs" | "reminders") => {
    setTab(k);
    setCreating(false);
    setEditKey(null);
  };

  // 渲染

  const jobEntries = entries.filter((e): e is JobEntry => e.kind === "job");
  const remEntries = entries.filter((e): e is ReminderEntry => e.kind === "reminder");
  // 摘要条只取当前标签的条目：作业「最近将执行」、提醒「最近会提醒」——
  // 混在一起会出现「最近将执行：站起来活动一下」，文案撒谎（提醒不执行）。
  const shownEntries = tab === "jobs" ? jobEntries : remEntries;
  const upcoming = shownEntries
    .filter((e) => e.enabled && e.nextRunAt !== null)
    .sort((a, b) => a.nextRunAt! - b.nextRunAt!)[0];

  const renderJobRow = (e: JobEntry) => {
    const key = keyOf("job", e.targetId);
    const busy = busyKeys.has(key);
    return (
      <div
        key={key}
        data-testid={`schedule-row-${key}`}
        className={`nm-card p-3 ${e.enabled ? "" : "opacity-60"}`}
      >
        <div className="flex items-center gap-2">
          <span className="nm-inset shrink-0 px-1.5 py-0.5 text-[10px] text-[var(--t4)]">
            定时任务
          </span>
          <p className="min-w-0 flex-1 truncate text-sm font-medium text-[var(--t1)]">
            {e.title}
          </p>
          {e.missed && (
            <span className="nm-inset shrink-0 px-1.5 py-0.5 text-[10px] text-[var(--danger)]">
              已错过
            </span>
          )}
          {!e.enabled && (
            <span className="nm-inset shrink-0 px-1.5 py-0.5 text-[10px] text-[var(--t5)]">
              已暂停
            </span>
          )}
          {/* 作业执行中（sched-status started → done/failed 移除） */}
          {liveJobIds.has(e.targetId) && (
            <span className="nm-inset shrink-0 px-1.5 py-0.5 text-[10px] text-[var(--brand)]">
              执行中…
            </span>
          )}
        </div>
        {e.detail && (
          <p
            className="mt-1 truncate text-[11px] text-[var(--t5)]"
            title={e.detail}
          >
            {e.detail}
          </p>
        )}
        <div className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-0.5 text-[11px] text-[var(--t4)]">
          <span className="text-[var(--brand)]">
            {formatSchedule(e.schedule)}
          </span>
          {e.enabled && e.nextRunAt !== null && (
            <span>下次 {untilTime(e.nextRunAt)}</span>
          )}
          {e.schedLast !== null && (
            <span>上次 {relativeTime(e.schedLast)}</span>
          )}
          {/* 最近执行结果徽标（仅 job 有此字段；null=从未跑不显示）：
              静默失败是调度器最大敌人，结果必须在列表一眼可见 */}
          {e.lastStatus === "ok" && (
            <span className="nm-inset px-1.5 py-0.5 text-[10px] text-[var(--success)]">
              成功
            </span>
          )}
          {e.lastStatus === "fail" && (
            <span
              className="nm-inset px-1.5 py-0.5 text-[10px] text-[var(--danger)]"
              title={e.lastError ?? undefined}
            >
              失败
            </span>
          )}
        </div>
        <div className="mt-2 flex flex-wrap items-center gap-1.5">
          <button
            className="nm-btn px-2 py-0.5 text-[11px] leading-none text-[var(--t3)] disabled:opacity-50"
            disabled={busy}
            title={busy ? "执行中" : "不等定时，立即试跑一次"}
            onClick={() => void runNow(e)}
          >
            立即执行
          </button>
          <button
            className="nm-btn px-2 py-0.5 text-[11px] leading-none text-[var(--t3)]"
            title={
              e.enabled ? "暂停（保留配置，恢复后继续原节奏）" : "恢复定时"
            }
            onClick={() => void toggleEnabled(e)}
          >
            {e.enabled ? "暂停" : "恢复"}
          </button>
          {
            <button
              className={`nm-btn px-2 py-0.5 text-[11px] leading-none ${
                e.mode === "preview"
                  ? "nm-inset text-[var(--t1)]"
                  : "text-[var(--t3)]"
              }`}
              title={
                e.mode === "preview"
                  ? "预演档：到点只建提案卡并通知，不执行；点此切回自动执行"
                  : "自动档：到点直接执行；点此切到预演（先看后跑）"
              }
              onClick={() => void toggleMode(e)}
            >
              {e.mode === "preview" ? "预演" : "自动"}
            </button>
          }
          <button
            className="nm-btn px-2 py-0.5 text-[11px] leading-none text-[var(--t3)]"
            onClick={() => setEditKey(editKey === key ? null : key)}
          >
            修改
          </button>
          {
            <button
              className="nm-btn px-2 py-0.5 text-[11px] leading-none text-[var(--t3)]"
              onClick={() => void toggleHistory(e.targetId)}
            >
              {historyOpenId === e.targetId ? "收起历史" : "历史"}
            </button>
          }
          <button
            className={`nm-btn px-2 py-0.5 text-[11px] leading-none ${
              cancelArmed === key
                ? "nm-inset text-[var(--danger)]"
                : "text-[var(--danger)]"
            }`}
            aria-pressed={cancelArmed === key}
            title={
              cancelArmed === key
                ? "再点一次确认取消（配置将被清除）"
                : "清除这条定时"
            }
            onClick={() => cancelSchedule(e)}
          >
            {cancelArmed === key ? "确认取消？" : "取消定时"}
          </button>
        </div>
        {editKey === key && (
          <div className="mt-2">
            {/* 作业编辑：内容 + 频率 + 重试配置都可改，一次提交 */}
            <ScheduleEntryEditor
              variant="job"
              initialContent={e.title}
              initialSchedule={e.schedule}
              initialRetryMax={e.retryMax}
              initialPauseOnFailure={e.pauseOnFailure}
              submitLabel="确认修改"
              onSubmit={(v) =>
                void saveJob(
                  e.targetId,
                  v.content,
                  v.schedule,
                  v.retryMax ?? 0,
                  v.pauseOnFailure ?? false,
                )
              }
              onCancel={() => setEditKey(null)}
            />
          </div>
        )}
        {/* 执行历史（XXL-JOB 调度日志式排障视角）：倒序，最新在前 */}
        {historyOpenId === e.targetId && (
          <div className="mt-2 nm-inset rounded-lg p-2">
            <p className="text-[10px] text-[var(--t5)]">
              执行历史（最近 20 条）
            </p>
            {historyLoadingId === e.targetId && !history[e.targetId] ? (
              <p className="mt-1 text-[11px] text-[var(--t5)]">加载中…</p>
            ) : (
              <JobRunList
                runs={history[e.targetId] ?? []}
                sessionId={jobSessions[e.targetId] ?? null}
                onTrace={(cardId) => setTraceCardId(cardId)}
                onJumpSession={jumpToSession}
              />
            )}
          </div>
        )}
      </div>
    );
  };

  return (
    <div className="flex flex-col gap-4">
      {/* 顶栏：标题 + 新建入口（新建按钮文案随标签变） */}
      <div className="flex shrink-0 flex-wrap items-center gap-2 pb-1">
        <h2 className="text-base font-semibold text-[var(--t1)]">定时</h2>
        <div className="flex-1" />
        <button
          className="flex h-8 items-center gap-1.5 whitespace-nowrap rounded-[var(--r-sm)] px-3 text-sm text-[var(--t3)] nm-outset"
          onClick={startCreate}
        >
          <Plus size={14} aria-hidden /> {tab === "jobs" ? "新建定时" : "新建提醒"}
        </button>
      </div>

      {/* 分组标签：两类条目语义互斥（做事 vs 说话），各带一套操作 → 分开看。
          不用分页：分页在本仓的语义是「换时间段」，两类之间没有时间关系。
          标签带计数——提醒不在当前屏时，计数是「别忘了它」的唯一提示。 */}
      <div
        role="tablist"
        aria-label="定时类型"
        className="flex items-center gap-1 border-b border-[var(--edge)] pb-2"
      >
        {(
          [
            ["jobs", "定时任务", jobEntries.length],
            ["reminders", "提醒", remEntries.length],
          ] as const
        ).map(([key, label, n]) => (
          <button
            key={key}
            type="button"
            role="tab"
            aria-selected={tab === key}
            onClick={() => switchTab(key)}
            className={`rounded-[var(--r-sm)] px-2.5 py-1 text-xs transition-colors duration-[var(--dur-chip)] ease-[var(--ease-out)] ${
              tab === key
                ? "nm-inset text-[var(--t1)]"
                : "text-[var(--t3)] hover:bg-[var(--hover-bg)]"
            }`}
          >
            {label}
            <span className="num ml-1 text-[10px] text-[var(--t5)]">{n}</span>
          </button>
        ))}
      </div>

      {/* 摘要条：只高亮当前标签最近的一条（可见性 = 对自动执行信任的来源） */}
      {upcoming && (
        <p className="nm-inset rounded-lg px-3 py-2 text-xs text-[var(--t3)]">
          {tab === "jobs" ? "最近将执行：" : "最近会提醒："}
          <span className="text-[var(--t1)]">{upcoming.title}</span>
          ，{untilTime(upcoming.nextRunAt!)}
        </p>
      )}

      {/* 通知未授权提示：只在提醒 tab 出现（作业不靠系统通知）。提醒到点只有
          那声通知，未授权 = 静默失败，用户只会以为功能坏了。 */}
      {tab === "reminders" && notifyBlocked && (
        <div className="nm-inset flex items-center gap-2 rounded-lg px-3 py-2 text-xs text-[var(--t2)]">
          <span className="min-w-0 flex-1">
            系统通知未授权——提醒到点不会响。请到系统设置的通知面板里允许本应用发送通知。
          </span>
          <button
            className="nm-btn shrink-0 px-2 py-0.5 text-[11px] text-[var(--t3)]"
            onClick={() => void checkNotifyPermission()}
          >
            重新检查
          </button>
        </div>
      )}

      {/* 新建流程：写内容 → 设频率 → 预览确认 */}
      {creating && (
        <div className="nm-card p-3">
          <p className="text-xs font-medium text-[var(--t2)]">
            {tab === "jobs" ? "新建定时" : "新建提醒"}
          </p>
          <div className="mt-2">
            <ScheduleEntryEditor
              variant={tab === "jobs" ? "job" : "reminder"}
              submitLabel="确认保存"
              onSubmit={(v) =>
                tab === "jobs"
                  ? void saveJob(
                      null,
                      v.content,
                      v.schedule,
                      v.retryMax ?? 0,
                      v.pauseOnFailure ?? false,
                    )
                  : void saveReminder(v.content, v.schedule)
              }
              onCancel={() => setCreating(false)}
            />
          </div>
        </div>
      )}

      {/* 加载失败：错误行 + 重试入口（失败绝不能渲染成「暂无定时任务」空态） */}
      {loadError && (
        <div className="nm-inset flex items-center gap-2 rounded-lg px-3 py-2 text-xs text-[var(--danger)]">
          <span className="min-w-0 flex-1 truncate" title={loadError}>
            定时任务列表加载失败：{loadError}
          </span>
          <button
            className="nm-btn shrink-0 px-2 py-0.5 text-[11px] text-[var(--t3)]"
            onClick={() => void reload()}
          >
            重试
          </button>
        </div>
      )}

      {loaded && !loadError && !creating && shownEntries.length === 0 ? (
        tab === "jobs" ? (
          <EmptyState
            icon={<AlarmClock size={18} aria-hidden />}
            title="暂无定时任务"
            description="写下要做什么，到点自动新建任务卡执行。"
            action={{ label: "新建定时", onClick: startCreate }}
          />
        ) : (
          <EmptyState
            icon={<Bell size={18} aria-hidden />}
            title="还没有提醒"
            description="设一个到点响一声的事——到点发系统通知，不建任务卡、不执行任何内容。"
            action={{ label: "新建提醒", onClick: startCreate }}
          />
        )
      ) : tab === "jobs" ? (
        jobEntries.length > 0 && (
          <div className="flex flex-col gap-2">{jobEntries.map(renderJobRow)}</div>
        )
      ) : (
        remEntries.length > 0 && (
          <div className="flex flex-col gap-2">
            {remEntries.map((r) => (
              <ReminderRow
                key={r.targetId}
                entry={r}
                onChanged={() => void reload()}
              />
            ))}
          </div>
        )
      )}
      {/* 已结束的一次性作业：作业行被系统消费掉后不在列表里了，但它的执行
          记录留档 30 天——「它跑过什么」要查得到。只在作业 tab 出现（提醒
          没有这类记录）。失败留错误行，不渲染成「没有已结束的作业」。 */}
      {tab === "jobs" && (finishedError || (finished?.length ?? 0) > 0) && (
        <section className="mt-1">
          <p className="mb-1 text-[11px] text-[var(--t5)]">
            已结束的一次性作业（执行记录留档 30 天）
          </p>
          {finishedError && (
            <div className="nm-inset flex items-center gap-2 rounded-lg px-3 py-2 text-xs text-[var(--danger)]">
              <span className="min-w-0 flex-1 truncate" title={finishedError}>
                已结束的定时任务读取失败：{finishedError}
              </span>
              <button
                className="nm-btn shrink-0 px-2 py-0.5 text-[11px] text-[var(--t3)]"
                onClick={() => void reloadFinished()}
              >
                重试
              </button>
            </div>
          )}
          {!finishedError &&
            (finished ?? []).map((f) => {
              const open = finishedOpenId === f.jobId;
              return (
                <div
                  key={f.jobId}
                  data-testid={`finished-job-${f.jobId}`}
                  className="nm-card mt-1 p-3 opacity-80"
                >
                  <div className="flex items-center gap-2">
                    <span className="nm-inset shrink-0 px-1.5 py-0.5 text-[10px] text-[var(--t4)]">
                      已结束
                    </span>
                    <p className="min-w-0 flex-1 truncate text-sm font-medium text-[var(--t2)]">
                      {f.content}
                    </p>
                    <span
                      className={`nm-inset shrink-0 px-1.5 py-0.5 text-[10px] ${
                        f.lastStatus === "ok"
                          ? "text-[var(--success)]"
                          : f.lastStatus === "preview"
                            ? "text-[var(--brand)]"
                            : "text-[var(--danger)]"
                      }`}
                    >
                      {f.lastStatus === "ok"
                        ? "成功"
                        : f.lastStatus === "preview"
                          ? "预演"
                          : "失败"}
                    </span>
                  </div>
                  <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-0.5 text-[11px] text-[var(--t4)]">
                    {f.schedule && <span>{formatSchedule(f.schedule)}</span>}
                    <span>最后执行 {relativeTime(f.lastFiredAt)}</span>
                    <span>共 {f.runs.length} 次尝试</span>
                  </div>
                  <div className="mt-2 flex flex-wrap items-center gap-1.5">
                    <button
                      className="nm-btn px-2 py-0.5 text-[11px] leading-none text-[var(--t3)]"
                      onClick={() => setFinishedOpenId(open ? null : f.jobId)}
                    >
                      {open ? "收起记录" : "执行记录"}
                    </button>
                  </div>
                  {open && (
                    <div className="mt-2 nm-inset rounded-lg p-2">
                      <JobRunList
                        runs={f.runs}
                        onTrace={(cardId) => setTraceCardId(cardId)}
                      />
                    </div>
                  )}
                </div>
              );
            })}
        </section>
      )}
      {/* 执行痕迹弹层（按本次执行新建的任务卡查 trace；TracePanel 自带 portal） */}
      {traceCardId && (
        <TracePanel taskId={traceCardId} onClose={() => setTraceCardId(null)} />
      )}
    </div>
  );
}
