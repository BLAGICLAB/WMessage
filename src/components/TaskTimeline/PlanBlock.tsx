import type { PlanSeg } from "./week";
import { DAY_SPAN_MIN, DAY_START_MIN, fmtMin } from "./week";
import type { Task } from "../../types";

/** 计划块单段渲染：seg 决定几何（父列内绝对定位，百分比制），task 决定内容与颜色。
 *  lane/lanes 来自 layoutLanes，多栏重叠时横向均分且只显示开始时间。 */
export function PlanBlockView({
  seg,
  task,
  color,
  selected = false,
  dimmed = false,
  fresh = false,
  onPointerDown,
}: {
  seg: PlanSeg & { lane: number; lanes: number };
  task: Task;
  /** CSS color 值（planColorVar 产出），注入 --c 驱动 main.css 的块材质 */
  color: string;
  selected?: boolean;
  /** 拖拽中的原块降透明（位置由 ghost/slot 占位） */
  dimmed?: boolean;
  /** 刚落位的块播弹入动画 */
  fresh?: boolean;
  onPointerDown?: (e: React.PointerEvent) => void;
}) {
  const topPct = ((seg.startMin - DAY_START_MIN) / DAY_SPAN_MIN) * 100;
  const hPct = ((seg.endMin - seg.startMin) / DAY_SPAN_MIN) * 100;
  const tiny = seg.endMin - seg.startMin < 45;
  const multi = seg.lanes > 1;
  return (
    <div
      className={`plan-blk${selected ? " sel" : ""}${seg.tail ? " tail" : ""}${seg.head ? " head" : ""}${dimmed ? " opacity-35" : ""}${fresh ? " plan-fresh" : ""}`}
      style={
        {
          "--c": color,
          top: `${topPct}%`,
          height: `calc(${hPct}% - 2px)`,
          left: `calc(3px + ${seg.lane} * (100% - 6px) / ${seg.lanes})`,
          width: `calc((100% - 6px) / ${seg.lanes} - 4px)`,
        } as React.CSSProperties
      }
      title={task.title}
      onPointerDown={onPointerDown}
    >
      <span className="plan-dot plan-dot-s" aria-hidden />
      <span className="plan-dot plan-dot-e" aria-hidden />
      <div className="plan-blk-title">{task.title}</div>
      {!tiny && (
        <span className="plan-blk-time num">
          {fmtMin(seg.startMin)}
          {multi ? "" : ` – ${fmtMin(seg.endMin)}`}
        </span>
      )}
      {seg.tail && (
        <span className="plan-cont" aria-hidden>
          »
        </span>
      )}
      {seg.head && (
        <span className="plan-cont" aria-hidden>
          «
        </span>
      )}
      <span className="plan-grip" aria-hidden data-grip={task.id} />
    </div>
  );
}
