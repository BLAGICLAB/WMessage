import { useState } from "react";
import { scheduleToDatetime } from "../../format";

/** * 定时编辑面板（SchedulePage 新建/修改共用）。
 * 抽出自主窗口 TodoCard 原 ⏰ 面板：datetime-local 输入 + 一次/每天/每周/每月四档，
 * 保留 NaN 防御——无效日期一律不回调（历史事故：weekly:NaN 双端解析漂移死循环）。
 */
export interface ScheduleEditorPanelProps {
  /** 编辑时回填已有规则（daily:/weekly:/monthly:/at: 串）；新建不传 */
  initial?: string | null;
  /** 应用：拼装好的 daily:/weekly:/monthly:/at: 串 */
  onApply: (schedule: string) => void;
  /** 面板关闭（不落库） */
  onCancel?: () => void;
  disabled?: boolean;
}

export function ScheduleEditorPanel({
  initial,
  onApply,
  onCancel,
  disabled = false,
}: ScheduleEditorPanelProps) {
  // 受控草稿：初始值由 scheduleToDatetime 全程防御（坏数据回退今天 09:00）
  const [dt, setDt] = useState(() => scheduleToDatetime(initial));

  const apply = (kind: "once" | "daily" | "weekly" | "monthly") => {
    if (!dt) return;
    const v = dt.slice(0, 16);
    // 防御：无效日期（手动输入不完整等）不回调，防 NaN 进 schedule
    // （历史事故：Invalid Date → NaN 写进 schedule → 面板回填死循环卡死 App）
    if (isNaN(new Date(v).getTime())) return;
    const hm = v.slice(11, 16);
    if (kind === "once") {
      onApply(`at:${v}`);
    } else if (kind === "daily") {
      onApply(`daily:${hm}`);
    } else if (kind === "weekly") {
      // 取所选日期的星期几（1=周一 ... 7=周日）
      const wd = ((new Date(v).getDay() + 6) % 7) + 1;
      onApply(`weekly:${wd}:${hm}`);
    } else {
      onApply(`monthly:${v.slice(8, 10)}:${hm}`);
    }
  };

  return (
    <div className="nm-inset rounded-lg p-2 space-y-1.5">
      <p className="text-[10px] text-[var(--t5)]">
        选一个时间，再点频率应用（任务卡结果写进备注）
      </p>
      <input
        type="datetime-local"
        aria-label="定时时间"
        value={dt}
        disabled={disabled}
        onChange={(e) => setDt(e.target.value)}
        className="nm-inset w-full rounded-lg px-2 py-1 text-xs text-[var(--t3)] outline-none"
      />
      <div className="flex flex-wrap gap-1">
        {(
          [
            ["一次", "once"],
            ["每天", "daily"],
            ["每周", "weekly"],
            ["每月", "monthly"],
          ] as [string, "once" | "daily" | "weekly" | "monthly"][]
        ).map(([label, kind]) => (
          <button
            key={kind}
            className="nm-btn px-2 py-0.5 text-[10px] text-[var(--t3)] disabled:opacity-50"
            disabled={disabled}
            onClick={() => apply(kind)}
          >
            {label}
          </button>
        ))}
        {onCancel && (
          <button
            className="nm-btn px-2 py-0.5 text-[10px] text-[var(--t4)]"
            onClick={onCancel}
          >
            关闭
          </button>
        )}
      </div>
    </div>
  );
}
