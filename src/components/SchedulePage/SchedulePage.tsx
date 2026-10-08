import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { AlarmClock, Plus } from "lucide-react";
import { useTauriListen } from "../../lib/useTauriListen";
import { handleCommandError } from "../../lib/errorHandler";
import { formatSchedule, relativeTime, untilTime } from "../../format";
import { EmptyState } from "../EmptyState";
import { TracePanel } from "../TracePanel";
import { ScheduleEditorPanel } from "./ScheduleEditorPanel";
import type { ScheduleEntry, ScheduledJobRun, Workflow } from "../../types";

/** * 定时任务模块：列表 = 状态面板（不是设置表单的集合）。
 * 数据单源：schedule_overview（定时作业 + 工作流两源合并，nextRunAt/missed 由 Rust 端
 * 计算——不在前端重写 schedule 解析器，weekly:NaN 双端解析漂移是历史事故）。
 * 两类目标：
 * - 定时作业（kind="job"）：用户写「要做什么」，到点后端新建任务卡并交给机器人执行
 *   （scheduled_job_create/update/delete/fire 四命令，作业即配置，取消 = 删作业）；
 * - 工作流（kind="workflow"）：选已有工作流模板挂定时（workflow_set_schedule）。
 */

/** 行/目标的稳定键（job 与 workflow 的 id 空间不同，必须带 kind 前缀） */
const keyOf = (kind: ScheduleEntry["kind"], id: string) => `${kind}:${id}`;

/** 作业内容上限（与后端 scheduled_job_create 校验一致；超限后端会 reject，前端先拦） */
const JOB_CONTENT_MAX = 500;

/** 执行耗时 → 短文案：<1s 给毫秒（亚秒级任务占比高），否则一位小数秒 */
function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

export function SchedulePage() {
  const [entries, setEntries] = useState<ScheduleEntry[]>([]);
  const [loaded, setLoaded] = useState(false);
  /** 新建流程的目标下拉数据源（工作流全量，含 schedule 用于「已定时」禁用） */
  const [workflows, setWorkflows] = useState<Workflow[]>([]);
  /** 工作流执行中状态（「立即执行」禁用依据） */
  const [wfRunning, setWfRunning] = useState<Record<string, boolean>>({});
  /** 行内修改面板展开目标（keyOf 形式） */
  const [editKey, setEditKey] = useState<string | null>(null);
  /** 执行历史展开目标（job id；展开时拉一次，不自动刷新——历史是排障视角，不是监控大盘） */
  const [historyOpenId, setHistoryOpenId] = useState<string | null>(null);
  const [history, setHistory] = useState<Record<string, ScheduledJobRun[]>>({});
  /** 加载中的作业 id（按 job 记：连开两个作业的历史时，先完成的不得清掉后一个的加载态） */
  const [historyLoadingId, setHistoryLoadingId] = useState<string | null>(null);
  /** 取消定时两步确认：armed 的 key，第二次点击才真清（3s 复位，WorkflowPage 同模式） */
  const [cancelArmed, setCancelArmed] = useState<string | null>(null);
  const armedTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** 立即执行在途（防连点；后端另有防重入兜底） */
  const [busyKeys, setBusyKeys] = useState<Set<string>>(new Set());
  /** 新建流程：选类型 → （作业：写内容 / 工作流：选模板）→ 面板 → 预览确认 */
  const [creating, setCreating] = useState<{
    kind: "job" | "workflow";
    targetId: string;
    draftSchedule: string | null;
  } | null>(null);
  //  执行透明：sched-status 事件驱动的「执行中」作业（started 入集，done/failed 出集）
  const [liveJobIds, setLiveJobIds] = useState<Set<string>>(new Set());
  /** job id → 最近一次执行会话（sched-status 携带；「会话」按钮跳挂件围观） */
  const [jobSessions, setJobSessions] = useState<Record<string, string>>({});
  /** 执行痕迹弹层目标（本次执行新建的任务卡 id） */
  const [traceCardId, setTraceCardId] = useState<string | null>(null);

  useEffect(
    () => () => {
      if (armedTimerRef.current) clearTimeout(armedTimerRef.current);
    },
    []
  );

  // 竞态令牌：reload 由挂载/tasks-changed/sched-status 并发触发，慢的旧响应
  // 不得覆盖新响应（NotificationsPage reloadSeqRef 同款模式）
  const reloadSeqRef = useRef(0);
  const reload = useCallback(async () => {
    const seq = ++reloadSeqRef.current;
    try {
      const list = await invoke<ScheduleEntry[]>("schedule_overview");
      if (seq !== reloadSeqRef.current) return;
      setEntries(list);
      setLoaded(true);
      // 工作流执行态逐个回查（数量少，逐个问比后端加聚合接口简单）
      const wfIds = list.filter((e) => e.kind === "workflow").map((e) => e.targetId);
      const running: Record<string, boolean> = {};
      await Promise.all(
        wfIds.map(async (id) => {
          running[id] = await invoke<boolean>("workflow_is_running", {
            workflowId: id,
          }).catch(() => false);
        })
      );
      if (seq !== reloadSeqRef.current) return;
      setWfRunning(running);
    } catch (e) {
      if (seq !== reloadSeqRef.current) return;
      handleCommandError(e, "读取定时任务列表", { silent: true });
      setLoaded(true);
    }
    invoke<Workflow[]>("workflow_list")
      .then((w) => {
        if (seq === reloadSeqRef.current) setWorkflows(w);
      })
      .catch((e) => handleCommandError(e, "读取工作流列表", { silent: true }));
  }, []);

  useEffect(() => {
    // 首次挂载拉列表：setState 都在 invoke await 之后，lint 误报同步 setState
    // oxlint-disable-next-line react/set-state-in-effect
    void reload();
  }, [reload]);
  // 到点执行会新建任务卡 → tasks-changed 广播，顺手刷新「上次执行」等状态
  useTauriListen("tasks-changed", () => {
    void reload();
  });

  //  执行透明：sched-status 实时驱动「执行中」徽标 + 会话跳转锚点；
  // done/failed 时刷新列表（lastStatus/sched_last 已在任务行更新）
  useTauriListen<{
    jobId?: string;
    phase?: "started" | "done" | "failed" | "skipped";
    sessionId?: string | null;
  }>("sched-status", (payload) => {
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

  /** 「会话」跳转：发 chat-focus-session 给挂件聊天区并唤起挂件窗口 */
  const jumpToSession = (sessionId: string) => {
    emit("chat-focus-session", { sessionId }).catch(() => {});
    WebviewWindow.getByLabel("widget")
      .then((w) => {
        if (w) {
          w.show()
            .then(() => w.setFocus())
            .catch(() => {});
        } else {
          // 挂件窗口不存在（重启后未再开启）：必须给可见反馈，不能点了没反应
          handleCommandError(
            new Error("挂件窗口未开启，请先打开挂件窗口"),
            "跳转执行会话"
          );
        }
      })
      .catch(() => {});
  };

  // 行操作

  /** 工作流定时落库（新建/修改/取消共用）：schedule=null 即取消 */
  const setWorkflowSchedule = async (targetId: string, schedule: string | null) => {
    await invoke("workflow_set_schedule", { id: targetId, schedule }).catch((e) =>
      handleCommandError(e, "设置工作流定时")
    );
    void reload();
  };

  const runNow = async (e: ScheduleEntry) => {
    const key = keyOf(e.kind, e.targetId);
    if (busyKeys.has(key)) return;
    setBusyKeys((prev) => new Set(prev).add(key));
    try {
      if (e.kind === "job") {
        // 与到点同款链路：后端新建任务卡并交给机器人跑
        await invoke("scheduled_job_fire", { id: e.targetId });
      } else {
        await invoke("workflow_run", { workflowId: e.targetId });
        setWfRunning((prev) => ({ ...prev, [e.targetId]: true }));
      }
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

  const toggleEnabled = async (e: ScheduleEntry) => {
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

  /** 取消定时：两步确认，3s 未确认自动复位。作业 = 删作业；工作流 = 清 schedule */
  const cancelSchedule = (e: ScheduleEntry) => {
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
      if (e.kind === "job") {
        await invoke("scheduled_job_delete", { id: e.targetId }).catch((err) =>
          handleCommandError(err, "删除定时任务")
        );
        void reload();
      } else {
        await setWorkflowSchedule(e.targetId, null);
      }
    })();
  };

  // 新建流程

  // 已有定时的工作流标「已定时」禁用——防同一目标双配置
  const scheduledWfIds = new Set(
    entries.filter((e) => e.kind === "workflow").map((e) => e.targetId)
  );

  const startCreate = () =>
    setCreating({ kind: "job", targetId: "", draftSchedule: null });

  /** 作业落库（新建/编辑共用）：content+schedule+重试配置都实传 */
  const saveJob = async (
    id: string | null,
    content: string,
    schedule: string,
    retryMax: number,
    pauseOnFailure: boolean
  ): Promise<boolean> => {
    try {
      if (id === null) {
        await invoke("scheduled_job_create", { content, schedule, retryMax, pauseOnFailure });
      } else {
        await invoke("scheduled_job_update", { id, content, schedule, retryMax, pauseOnFailure });
      }
      setCreating(null);
      setEditKey(null);
      void reload();
      return true;
    } catch (e) {
      handleCommandError(e, id === null ? "新建定时任务" : "修改定时任务");
      return false;
    }
  };

  const saveWorkflowCreate = async () => {
    if (!creating || creating.kind !== "workflow") return;
    if (!creating.targetId || !creating.draftSchedule) return;
    await setWorkflowSchedule(creating.targetId, creating.draftSchedule);
    setCreating(null);
  };

  // 渲染

  const jobEntries = entries.filter((e) => e.kind === "job");
  const wfEntries = entries.filter((e) => e.kind === "workflow");
  // 即将执行摘要：启用项按 nextRunAt 升序取第一条
  const upcoming = entries
    .filter((e) => e.enabled && e.nextRunAt !== null)
    .sort((a, b) => a.nextRunAt! - b.nextRunAt!)[0];

  const renderEntry = (e: ScheduleEntry) => {
    const key = keyOf(e.kind, e.targetId);
    const busy = busyKeys.has(key) || (e.kind === "workflow" && !!wfRunning[e.targetId]);
    return (
      <div
        key={key}
        data-testid={`schedule-row-${key}`}
        className={`nm-card p-3 ${e.enabled ? "" : "opacity-60"}`}
      >
        <div className="flex items-center gap-2">
          <span className="nm-inset shrink-0 px-1.5 py-0.5 text-[10px] text-[var(--t4)]">
            {e.kind === "job" ? "定时任务" : "工作流"}
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
          {e.kind === "workflow" && wfRunning[e.targetId] && (
            <span className="nm-inset shrink-0 px-1.5 py-0.5 text-[10px] text-[var(--brand)]">
              执行中
            </span>
          )}
          {/* ：作业执行中（sched-status started → done/failed 移除） */}
          {e.kind === "job" && liveJobIds.has(e.targetId) && (
            <span className="nm-inset shrink-0 px-1.5 py-0.5 text-[10px] text-[var(--brand)]">
              执行中…
            </span>
          )}
        </div>
        {e.detail && (
          <p className="mt-1 truncate text-[11px] text-[var(--t5)]" title={e.detail}>
            {e.detail}
          </p>
        )}
        <div className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-0.5 text-[11px] text-[var(--t4)]">
          <span className="text-[var(--brand)]">{formatSchedule(e.schedule)}</span>
          {e.enabled && e.nextRunAt !== null && (
            <span>下次 {untilTime(e.nextRunAt)}</span>
          )}
          {e.schedLast !== null && <span>上次 {relativeTime(e.schedLast)}</span>}
          {/* 最近执行结果徽标（仅 job 有此字段；null=从未跑不显示）：
              静默失败是调度器最大敌人，结果必须在列表一眼可见 */}
          {e.kind === "job" && e.lastStatus === "ok" && (
            <span className="nm-inset px-1.5 py-0.5 text-[10px] text-[var(--success)]">
              成功
            </span>
          )}
          {e.kind === "job" && e.lastStatus === "fail" && (
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
            title={e.enabled ? "暂停（保留配置，恢复后继续原节奏）" : "恢复定时"}
            onClick={() => void toggleEnabled(e)}
          >
            {e.enabled ? "暂停" : "恢复"}
          </button>
          <button
            className="nm-btn px-2 py-0.5 text-[11px] leading-none text-[var(--t3)]"
            onClick={() => setEditKey(editKey === key ? null : key)}
          >
            修改
          </button>
          {e.kind === "job" && (
            <button
              className="nm-btn px-2 py-0.5 text-[11px] leading-none text-[var(--t3)]"
              onClick={() => void toggleHistory(e.targetId)}
            >
              {historyOpenId === e.targetId ? "收起历史" : "历史"}
            </button>
          )}
          <button
            className={`nm-btn px-2 py-0.5 text-[11px] leading-none ${
              cancelArmed === key
                ? "nm-inset text-[var(--danger)]"
                : "text-[var(--danger)]"
            }`}
            aria-pressed={cancelArmed === key}
            title={cancelArmed === key ? "再点一次确认取消（配置将被清除）" : "清除这条定时"}
            onClick={() => cancelSchedule(e)}
          >
            {cancelArmed === key ? "确认取消？" : "取消定时"}
          </button>
        </div>
        {editKey === key &&
          (e.kind === "job" ? (
            <div className="mt-2">
              {/* 作业编辑：内容 + 频率 + 重试配置都可改，一次提交 */}
              <JobEditor
                initialContent={e.title}
                initialSchedule={e.schedule}
                initialRetryMax={e.retryMax}
                initialPauseOnFailure={e.pauseOnFailure}
                submitLabel="确认修改"
                onSubmit={(content, schedule, retryMax, pauseOnFailure) =>
                  void saveJob(e.targetId, content, schedule, retryMax, pauseOnFailure)
                }
                onCancel={() => setEditKey(null)}
              />
            </div>
          ) : (
            <div className="mt-2">
              <ScheduleEditorPanel
                initial={e.schedule}
                onApply={(schedule) => {
                  setEditKey(null);
                  void setWorkflowSchedule(e.targetId, schedule);
                }}
                onCancel={() => setEditKey(null)}
              />
            </div>
          ))}
        {/* 执行历史（XXL-JOB 调度日志式排障视角）：倒序，最新在前 */}
        {e.kind === "job" && historyOpenId === e.targetId && (
          <div className="mt-2 nm-inset rounded-lg p-2">
            <p className="text-[10px] text-[var(--t5)]">执行历史（最近 20 条）</p>
            {historyLoadingId === e.targetId && !history[e.targetId] ? (
              <p className="mt-1 text-[11px] text-[var(--t5)]">加载中…</p>
            ) : (history[e.targetId] ?? []).length === 0 ? (
              <p className="mt-1 text-[11px] text-[var(--t5)]">还没有执行记录</p>
            ) : (
              <ul className="mt-1 flex flex-col divide-y divide-[color-mix(in_srgb,var(--edge),transparent_55%)]">
                {(history[e.targetId] ?? []).map((r) => (
                  <li
                    key={r.id}
                    className="flex items-center gap-2 py-1 text-[11px] text-[var(--t4)]"
                  >
                    <span
                      className={`shrink-0 ${
                        r.status === "ok" ? "text-[var(--success)]" : "text-[var(--danger)]"
                      }`}
                    >
                      {r.status === "ok" ? "成功" : "失败"}
                    </span>
                    <span className="shrink-0">{relativeTime(r.firedAt)}</span>
                    <span className="shrink-0 tabular-nums">{formatDuration(r.durationMs)}</span>
                    {r.summary && (
                      <span className="min-w-0 flex-1 truncate" title={r.summary}>
                        {r.summary}
                      </span>
                    )}
                    {/* ：执行痕迹（本次执行新建的卡）+ 会话跳转（挂件围观 ⏰ 会话） */}
                    {r.cardId && (
                      <button
                        className="nm-btn shrink-0 px-1.5 py-0.5 text-[10px] text-[var(--t3)]"
                        title="查看本次执行痕迹（工具时间线 / 文件 diff / 回滚）"
                        onClick={() => setTraceCardId(r.cardId!)}
                      >
                        痕迹
                      </button>
                    )}
                    {jobSessions[e.targetId] && (
                      <button
                        className="nm-btn shrink-0 px-1.5 py-0.5 text-[10px] text-[var(--t3)]"
                        title="跳转挂件，围观本次执行会话"
                        onClick={() => jumpToSession(jobSessions[e.targetId])}
                      >
                        会话
                      </button>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}
      </div>
    );
  };

  return (
    <div className="flex flex-col gap-4">
      {/* 顶栏：标题 + 新建入口（风格对齐 WorkflowPage 工具栏） */}
      <div className="flex shrink-0 flex-wrap items-center gap-2 pb-1">
        <h2 className="text-base font-semibold text-[var(--t1)]">定时任务</h2>
        <div className="flex-1" />
        <button
          className="flex h-8 items-center gap-1.5 whitespace-nowrap rounded-[var(--r-sm)] px-3 text-sm text-[var(--t3)] nm-outset"
          onClick={startCreate}
        >
          <Plus size={14} aria-hidden /> 新建定时
        </button>
      </div>

      {/* 即将执行摘要条：只高亮最近一条（可见性 = 对自动执行信任的来源） */}
      {upcoming && (
        <p className="nm-inset rounded-lg px-3 py-2 text-xs text-[var(--t3)]">
          最近将执行：<span className="text-[var(--t1)]">{upcoming.title}</span>
          ，{untilTime(upcoming.nextRunAt!)}
        </p>
      )}

      {/* 新建流程：选类型 →（作业写内容 / 工作流选模板）→ 设频率 → 预览确认 */}
      {creating && (
        <div className="nm-card p-3">
          <p className="text-xs font-medium text-[var(--t2)]">新建定时</p>
          <div className="mt-2 flex flex-wrap items-center gap-2">
            <select
              aria-label="定时目标类型"
              className="h-8 rounded-[var(--r-sm)] bg-transparent px-2 text-sm text-[var(--t2)] nm-outset"
              value={creating.kind}
              onChange={(e) =>
                setCreating({
                  kind: e.target.value as "job" | "workflow",
                  targetId: "",
                  draftSchedule: null,
                })
              }
            >
              <option value="job">定时任务</option>
              <option value="workflow">工作流</option>
            </select>
            {creating.kind === "workflow" && (
              <select
                aria-label="定时目标"
                className="h-8 min-w-48 rounded-[var(--r-sm)] bg-transparent px-2 text-sm text-[var(--t2)] nm-outset"
                value={creating.targetId}
                onChange={(e) =>
                  setCreating({ ...creating, targetId: e.target.value })
                }
              >
                <option value="">（选择工作流）</option>
                {workflows.map((w) => (
                  <option
                    key={w.id}
                    value={w.id}
                    disabled={scheduledWfIds.has(w.id)}
                  >
                    {w.name}
                    {scheduledWfIds.has(w.id) ? "（已定时）" : ""}
                  </option>
                ))}
              </select>
            )}
          </div>
          {creating.kind === "job" ? (
            <div className="mt-2">
              <JobEditor
                submitLabel="确认保存"
                onSubmit={(content, schedule, retryMax, pauseOnFailure) =>
                  void saveJob(null, content, schedule, retryMax, pauseOnFailure)
                }
                onCancel={() => setCreating(null)}
              />
            </div>
          ) : (
            <>
              {creating.targetId && (
                <div className="mt-2">
                  <ScheduleEditorPanel
                    onApply={(schedule) =>
                      setCreating({ ...creating, draftSchedule: schedule })
                    }
                    onCancel={() => setCreating(null)}
                  />
                </div>
              )}
              {/* 保存前预览：人话总结先确认再落库 */}
              {creating.draftSchedule && (
                <div className="mt-2 space-y-2">
                  <p className="nm-inset rounded-lg px-3 py-2 text-xs text-[var(--t3)]">
                    {formatSchedule(creating.draftSchedule)}，到点自动执行该工作流
                  </p>
                  <div className="flex gap-2">
                    <button
                      className="nm-btn px-3 py-1 text-xs text-[var(--t1)]"
                      onClick={() => void saveWorkflowCreate()}
                    >
                      确认保存
                    </button>
                    <button
                      className="nm-btn px-3 py-1 text-xs text-[var(--t4)]"
                      onClick={() => setCreating(null)}
                    >
                      放弃
                    </button>
                  </div>
                </div>
              )}
            </>
          )}
        </div>
      )}

      {loaded && entries.length === 0 && !creating ? (
        <EmptyState
          icon={<AlarmClock size={18} aria-hidden />}
          title="暂无定时任务"
          description="写下要做什么，到点自动新建任务卡执行；也可以给已有工作流挂定时。"
          action={{ label: "新建定时", onClick: startCreate }}
        />
      ) : (
        <>
          {jobEntries.length > 0 && (
            <section>
              <p className="mb-2 text-xs font-medium text-[var(--t5)]">定时任务</p>
              <div className="flex flex-col gap-2">{jobEntries.map(renderEntry)}</div>
            </section>
          )}
          {wfEntries.length > 0 && (
            <section>
              <p className="mb-2 text-xs font-medium text-[var(--t5)]">工作流</p>
              <div className="flex flex-col gap-2">{wfEntries.map(renderEntry)}</div>
            </section>
          )}
        </>
      )}
      {/* ：执行痕迹弹层（按本次执行新建的任务卡查 trace；TracePanel 自带 portal） */}
      {traceCardId && (
        <TracePanel taskId={traceCardId} onClose={() => setTraceCardId(null)} />
      )}
    </div>
  );
}

/** * 定时作业编辑器（新建/行内修改共用）：内容输入 + 频率面板 + 重试配置 + 预览确认。
 * 到点语义：后端按内容新建任务卡并交给机器人执行——预览文案必须说清这一点。
 */
function JobEditor({
  initialContent = "",
  initialSchedule = null,
  initialRetryMax = 0,
  initialPauseOnFailure = false,
  submitLabel,
  onSubmit,
  onCancel,
}: {
  /** 编辑时回填已有内容；新建为空 */
  initialContent?: string;
  /** 编辑时回填已有规则；新建为 null（必须重新选频率才能提交） */
  initialSchedule?: string | null;
  /** 编辑时回填重试上限；新建默认 0（不重试） */
  initialRetryMax?: number;
  /** 编辑时回填自动暂停开关；新建默认关 */
  initialPauseOnFailure?: boolean;
  submitLabel: string;
  onSubmit: (
    content: string,
    schedule: string,
    retryMax: number,
    pauseOnFailure: boolean
  ) => void;
  onCancel: () => void;
}) {
  const [content, setContent] = useState(initialContent);
  // 编辑态预填原规则：只改内容不换频率也能直接提交
  const [draftSchedule, setDraftSchedule] = useState<string | null>(initialSchedule);
  const [retryMax, setRetryMax] = useState(initialRetryMax);
  const [pauseOnFailure, setPauseOnFailure] = useState(initialPauseOnFailure);
  const canSubmit = content.trim().length > 0 && draftSchedule !== null;

  return (
    <div className="space-y-2">
      <textarea
        aria-label="定时任务内容"
        className="nm-inset w-full resize-none rounded-lg px-2 py-1.5 text-xs text-[var(--t2)] outline-none placeholder:text-[var(--t5)]"
        rows={2}
        maxLength={JOB_CONTENT_MAX}
        placeholder={`要做什么？（${JOB_CONTENT_MAX} 字以内，到点自动新建任务卡交给机器人）`}
        value={content}
        onChange={(e) => setContent(e.target.value)}
      />
      <ScheduleEditorPanel
        initial={initialSchedule}
        onApply={setDraftSchedule}
        onCancel={onCancel}
      />
      {/* 失败策略（Temporal pauseOnFailure 式）：重试与自动暂停两个独立旋钮 */}
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
        <label className="flex items-center gap-1.5 text-[11px] text-[var(--t4)]">
          失败重试
          <select
            aria-label="失败重试次数"
            className="h-7 rounded-[var(--r-sm)] bg-transparent px-1.5 text-xs text-[var(--t2)] nm-outset"
            value={retryMax}
            onChange={(e) => setRetryMax(Number(e.target.value))}
          >
            <option value={0}>不重试</option>
            <option value={1}>1 次</option>
            <option value={2}>2 次</option>
            <option value={3}>3 次</option>
          </select>
        </label>
        <label className="flex items-center gap-1.5 text-[11px] text-[var(--t4)] cursor-pointer">
          <input
            type="checkbox"
            aria-label="失败自动暂停"
            className="w-3.5 h-3.5 accent-[var(--brand)]"
            checked={pauseOnFailure}
            onChange={(e) => setPauseOnFailure(e.target.checked)}
          />
          失败自动暂停
        </label>
        <p className="w-full text-[10px] text-[var(--t5)]">
          失败后 5 分钟自动重试；重试用尽仍失败时{pauseOnFailure ? "暂停此任务并通知" : "保持原节奏"}
        </p>
      </div>
      {/* 保存前预览：人话总结先确认再落库 */}
      {draftSchedule && (
        <div className="space-y-2">
          <p className="nm-inset rounded-lg px-3 py-2 text-xs text-[var(--t3)]">
            {formatSchedule(draftSchedule)}，到点自动新建任务卡并交给机器人执行
          </p>
          <div className="flex gap-2">
            <button
              className="nm-btn px-3 py-1 text-xs text-[var(--t1)] disabled:opacity-50"
              disabled={!canSubmit}
              title={content.trim() ? undefined : "先填写要做什么"}
              onClick={() =>
                onSubmit(content.trim(), draftSchedule, retryMax, pauseOnFailure)
              }
            >
              {submitLabel}
            </button>
            <button
              className="nm-btn px-3 py-1 text-xs text-[var(--t4)]"
              onClick={onCancel}
            >
              放弃
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
