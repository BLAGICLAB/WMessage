import { useState } from "react";
import { ScheduleEditorPanel } from "./ScheduleEditorPanel";
import { formatSchedule } from "../../format";

/**
 * 定时条目编辑器（定时作业 / 提醒共用）：内容 + 频率 + 预览确认。
 *
 * 频率四档两类逐档同构（同一套定时串格式），面板直接复用；差异只有两处：
 * 失败策略（提醒不做事，没有失败可言）与到点语义的预览文案。
 */
export interface ScheduleEntryValues {
  /** 条目内容（作业=要做什么；提醒=提醒什么） */
  content: string;
  /** 规范定时串（daily:/weekly:/monthly:/at:） */
  schedule: string;
  /** 仅作业：失败重试次数上限 */
  retryMax?: number;
  /** 仅作业：重试用尽仍失败时自动暂停 */
  pauseOnFailure?: boolean;
}

/** 内容上限（与后端一致：作业 500 字 / 提醒 200 字；超限后端会 reject，前端先拦） */
const CONTENT_MAX = { job: 500, reminder: 200 } as const;

export function ScheduleEntryEditor({
  variant,
  initialContent = "",
  initialSchedule = null,
  initialRetryMax = 0,
  initialPauseOnFailure = false,
  submitLabel,
  onSubmit,
  onCancel,
}: {
  variant: "job" | "reminder";
  /** 编辑时回填已有内容；新建为空 */
  initialContent?: string;
  /** 编辑时回填已有规则；新建为 null（必须重新选频率才能提交） */
  initialSchedule?: string | null;
  /** 编辑时回填重试上限；新建默认 0（不重试） */
  initialRetryMax?: number;
  /** 编辑时回填自动暂停开关；新建默认关 */
  initialPauseOnFailure?: boolean;
  submitLabel: string;
  onSubmit: (v: ScheduleEntryValues) => void;
  onCancel: () => void;
}) {
  const [content, setContent] = useState(initialContent);
  // 编辑态预填原规则：只改内容不换频率也能直接提交
  const [draftSchedule, setDraftSchedule] = useState<string | null>(initialSchedule);
  const [retryMax, setRetryMax] = useState(initialRetryMax);
  const [pauseOnFailure, setPauseOnFailure] = useState(initialPauseOnFailure);
  const canSubmit = content.trim().length > 0 && draftSchedule !== null;
  const isJob = variant === "job";
  const max = CONTENT_MAX[variant];

  return (
    <div className="space-y-2">
      <textarea
        aria-label={isJob ? "定时任务内容" : "提醒内容"}
        className="nm-inset w-full resize-none rounded-lg px-2 py-1.5 text-xs text-[var(--t2)] outline-none placeholder:text-[var(--t5)]"
        rows={2}
        maxLength={max}
        placeholder={
          isJob
            ? `要做什么？（${max} 字以内，到点自动新建任务卡交给机器人）`
            : `提醒我什么？（${max} 字以内，到点发一条系统通知）`
        }
        value={content}
        onChange={(e) => setContent(e.target.value)}
      />
      <ScheduleEditorPanel
        initial={initialSchedule}
        onApply={setDraftSchedule}
        onCancel={onCancel}
      />
      {/* 失败策略（Temporal pauseOnFailure 式）：两个独立旋钮——只有作业会失败 */}
      {isJob && (
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
            失败后 5 分钟自动重试；重试用尽仍失败时
            {pauseOnFailure ? "暂停此任务并通知" : "保持原节奏"}
          </p>
        </div>
      )}
      {/* 保存前预览：人话总结先确认再落库——到点语义必须说清（作业做事，提醒只说话） */}
      {draftSchedule && (
        <div className="space-y-2">
          <p className="nm-inset rounded-lg px-3 py-2 text-xs text-[var(--t3)]">
            {isJob
              ? `${formatSchedule(draftSchedule)}，到点自动新建任务卡并交给机器人执行`
              : `${formatSchedule(draftSchedule)}，到点发一条系统通知提醒你（只说话，不做事）`}
          </p>
          <div className="flex gap-2">
            <button
              className="nm-btn px-3 py-1 text-xs text-[var(--t1)] disabled:opacity-50"
              disabled={!canSubmit}
              title={content.trim() ? undefined : isJob ? "先填写要做什么" : "先填写提醒什么"}
              onClick={() =>
                onSubmit(
                  isJob
                    ? {
                        content: content.trim(),
                        schedule: draftSchedule,
                        retryMax,
                        pauseOnFailure,
                      }
                    : { content: content.trim(), schedule: draftSchedule },
                )
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
