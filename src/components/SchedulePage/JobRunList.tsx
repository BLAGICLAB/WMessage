import { relativeTime, formatMsShort } from "../../format";
import type { ScheduledJobRun } from "../../types";

/** 执行结果的中文标签与配色：ok / fail / preview 三态。
 *  preview = 预演档（到点只建提案卡不执行）——它既不是成功也不是失败，
 *  按「非 ok 即失败」渲染会把它说成失败（老渲染的毛病）。 */
function statusOf(status: string): {
  label: string;
  cls: string;
} {
  switch (status) {
    case "ok":
      return { label: "成功", cls: "text-[var(--success)]" };
    case "preview":
      return { label: "预演", cls: "text-[var(--brand)]" };
    default:
      return { label: "失败", cls: "text-[var(--danger)]" };
  }
}

type Props = {
  runs: ScheduledJobRun[];
  /** 执行会话 id（跳聊天页围观）：只有本次会话内跑过才有 */
  sessionId?: string | null;
  /** 「痕迹」按钮：本次执行新建的任务卡 id */
  onTrace?: (cardId: string) => void;
  /** 「会话」按钮：跳聊天页围观 */
  onJumpSession?: (sessionId: string) => void;
};

/** 执行记录列表（倒序，最新在前）：作业行的「历史」区与「已结束」区共用。
 *
 *  两处展示的是同一份数据（scheduled_job_runs），形态必须一致——分成两套
 *  渲染，日后一处改了另一处没改，状态标签与徽标就会说不同的谎。
 */
export function JobRunList({
  runs,
  sessionId,
  onTrace,
  onJumpSession,
}: Props) {
  if (runs.length === 0) {
    return (
      <p className="mt-1 text-[11px] text-[var(--t5)]">还没有执行记录</p>
    );
  }
  return (
    <ul className="mt-1 flex flex-col divide-y divide-[color-mix(in_srgb,var(--edge),transparent_55%)]">
      {runs.map((r) => {
        const st = statusOf(r.status);
        return (
          <li
            key={r.id}
            className="flex items-center gap-2 py-1 text-[11px] text-[var(--t4)]"
          >
            <span className={`shrink-0 ${st.cls}`}>{st.label}</span>
            <span className="shrink-0">{relativeTime(r.firedAt)}</span>
            {/* 预演没有耗时（durationMs 为 null）：显示占位而不是 0ms */}
            <span className="shrink-0 tabular-nums">
              {r.durationMs === null ? "—" : formatMsShort(r.durationMs)}
            </span>
            {r.summary && (
              <span className="min-w-0 flex-1 truncate" title={r.summary}>
                {r.summary}
              </span>
            )}
            {r.cardId && onTrace && (
              <button
                className="nm-btn shrink-0 px-1.5 py-0.5 text-[10px] text-[var(--t3)]"
                title="查看本次执行痕迹（工具时间线 / 文件 diff / 回滚）"
                onClick={() => onTrace(r.cardId!)}
              >
                痕迹
              </button>
            )}
            {sessionId && onJumpSession && (
              <button
                className="nm-btn shrink-0 px-1.5 py-0.5 text-[10px] text-[var(--t3)]"
                title="跳转聊天页，围观本次执行会话"
                onClick={() => onJumpSession(sessionId)}
              >
                会话
              </button>
            )}
          </li>
        );
      })}
    </ul>
  );
}
