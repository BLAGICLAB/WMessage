import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { handleCommandError } from "../../lib/errorHandler";
import { formatSchedule, relativeTime, untilTime } from "../../format";
import { ScheduleEntryEditor } from "./ScheduleEntryEditor";
import type { ReminderEntry } from "../../types";

/**
 * 提醒行：到点发一条系统通知，不建卡不执行——所以没有「立即执行 / 预演 /
 * 执行历史 / 执行痕迹 / 会话」，只有暂停/恢复、修改、取消。
 *
 * 一次性提醒（at:）触发即删，没有下次可暂停 → 不给「暂停」钮。
 */
export function ReminderRow({
  entry,
  onChanged,
}: {
  entry: ReminderEntry;
  /** 落库后让页面重拉列表（列表是状态面板，不是设置的集合） */
  onChanged: () => void;
}) {
  const [editing, setEditing] = useState(false);
  /** 取消两步确认：armed 后第二次点击才真删（3s 未确认自动复位） */
  const [cancelArmed, setCancelArmed] = useState(false);
  const armedTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** 暂停/恢复在途（防连点） */
  const [busy, setBusy] = useState(false);

  useEffect(
    () => () => {
      if (armedTimerRef.current) clearTimeout(armedTimerRef.current);
    },
    [],
  );

  const toggleEnabled = async () => {
    if (busy) return;
    setBusy(true);
    try {
      await invoke("reminder_set_enabled", {
        id: entry.targetId,
        enabled: !entry.enabled,
      });
      onChanged();
    } catch (err) {
      handleCommandError(err, entry.enabled ? "暂停提醒" : "恢复提醒");
    } finally {
      setBusy(false);
    }
  };

  const cancel = () => {
    if (!cancelArmed) {
      setCancelArmed(true);
      if (armedTimerRef.current) clearTimeout(armedTimerRef.current);
      armedTimerRef.current = setTimeout(() => setCancelArmed(false), 3000);
      return;
    }
    if (armedTimerRef.current) clearTimeout(armedTimerRef.current);
    setCancelArmed(false);
    void (async () => {
      try {
        const ok = await invoke<boolean>("reminder_delete", {
          id: entry.targetId,
        });
        if (!ok) {
          // 行不存在：多半是刚刚到点触发已被消费掉，不是删除失败
          handleCommandError(
            new Error("提醒已不存在（可能刚刚到点触发）"),
            "取消提醒",
          );
          return;
        }
        onChanged();
      } catch (err) {
        handleCommandError(err, "取消提醒");
      }
    })();
  };

  const save = (v: { content: string; schedule: string }) => {
    void (async () => {
      try {
        await invoke("reminder_update", {
          id: entry.targetId,
          text: v.content,
          schedule: v.schedule,
        });
        setEditing(false);
        onChanged();
      } catch (err) {
        handleCommandError(err, "修改提醒");
      }
    })();
  };

  return (
    <div
      className={`nm-card p-3 ${entry.enabled ? "" : "opacity-60"}`}
      data-testid={`schedule-row-reminder:${entry.targetId}`}
    >
      <div className="flex items-center gap-2">
        <span className="nm-inset shrink-0 px-1.5 py-0.5 text-[10px] text-[var(--t4)]">
          提醒
        </span>
        <p className="min-w-0 flex-1 truncate text-sm font-medium text-[var(--t1)]">
          {entry.title}
        </p>
        {!entry.enabled && (
          <span className="nm-inset shrink-0 px-1.5 py-0.5 text-[10px] text-[var(--t5)]">
            已暂停
          </span>
        )}
      </div>
      <div className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-0.5 text-[11px] text-[var(--t4)]">
        <span className="text-[var(--brand)]">{formatSchedule(entry.schedule)}</span>
        {entry.enabled && entry.nextRunAt !== null && (
          <span>下次 {untilTime(entry.nextRunAt)}</span>
        )}
        {entry.schedLast !== null && <span>上次 {relativeTime(entry.schedLast)}</span>}
        {!entry.oneShot && entry.firedCount !== null && entry.firedCount > 0 && (
          <span>已触发 {entry.firedCount} 次</span>
        )}
      </div>
      <div className="mt-2 flex flex-wrap items-center gap-1.5">
        {/* 一次性提醒触发即删，没有下次可暂停 → 不给这一钮 */}
        {!entry.oneShot && (
          <button
            className="nm-btn px-2 py-0.5 text-[11px] leading-none text-[var(--t3)] disabled:opacity-50"
            disabled={busy}
            title={entry.enabled ? "暂停（保留配置，恢复后继续原节奏）" : "恢复定时"}
            onClick={() => void toggleEnabled()}
          >
            {entry.enabled ? "暂停" : "恢复"}
          </button>
        )}
        <button
          className="nm-btn px-2 py-0.5 text-[11px] leading-none text-[var(--t3)]"
          onClick={() => setEditing((v) => !v)}
        >
          修改
        </button>
        <button
          className={`nm-btn px-2 py-0.5 text-[11px] leading-none ${
            cancelArmed ? "nm-inset text-[var(--danger)]" : "text-[var(--danger)]"
          }`}
          aria-pressed={cancelArmed}
          title={
            cancelArmed
              ? "再点一次确认取消（提醒将被清除）"
              : "取消这条提醒"
          }
          onClick={cancel}
        >
          {cancelArmed ? "确认取消？" : "取消提醒"}
        </button>
      </div>
      {editing && (
        <div className="mt-2">
          <ScheduleEntryEditor
            variant="reminder"
            initialContent={entry.title}
            initialSchedule={entry.schedule}
            submitLabel="确认修改"
            onSubmit={save}
            onCancel={() => setEditing(false)}
          />
        </div>
      )}
    </div>
  );
}
