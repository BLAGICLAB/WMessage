import { useRef } from "react";
import type { PlanSeg } from "./week";
import { DAY_SPAN_MIN, DAY_START_MIN, fmtHours, titleRows } from "./week";
import type { Task } from "../../types";
import { useHoverTitleTip } from "./HoverTitle";
import { ActorAvatar } from "../ActorAvatar";
import { DoneCircle } from "../DoneCircle";

/** 重叠簇第 3 张（lane ≥ 2）起的塌缩档：内容只剩头像（上）+ 对勾（下），
 *  尺寸随块高（≈窗口分钟数）三档。 */
function stackSize(durMin: number): { size: 16 | 12 | 10; cls: string } {
  if (durMin >= 90) return { size: 16, cls: " s-lg" };
  if (durMin >= 40) return { size: 12, cls: " s-md" };
  return { size: 10, cls: " s-sm" };
}

/** 计划块单段渲染（v2 规格）：首行 = 头像最左 + 完成对勾最右（顶隙 4px），
 *  标题第二行起、行数按块高 1/2/4/6 封顶；矮块（< 1 小时）退化为同一行
 *  「头像 · 标题 · 对勾」。时间行 = 时长「N 小时」，位于底部左侧（9px）。
 *  半区卡（lanes > 1）走 narrow 紧凑排版；重叠簇第 3 张起塌缩（collapsed）：
 *  只渲染头像 + 对勾上下排列，无标题无时间。
 *  已完成任务原位保留（done）：整块降饱和，对勾变绿，不划线，点勾恢复待办。
 *  已归档卡（archived）：原位保留但封存——虚线边 + 印章替代对勾，无拖拽
 *  无对勾，仅双击跳归档区（交互收口在 WeekGrid，本组件只管渲染）。 */
export function PlanBlockView({
  seg,
  task,
  color,
  selected = false,
  dimmed = false,
  fresh = false,
  done = false,
  archived = false,
  collapsed = false,
  onPointerDown,
  onDoubleClick,
  onSetColumn,
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
  /** 已完成：原位保留（降饱和 + 绿勾，无划线） */
  done?: boolean;
  /** 已归档：封存态（虚线 + 印章，无对勾无拖拽把手） */
  archived?: boolean;
  /** 重叠簇 lane ≥ 2：塌缩为头像 + 对勾 */
  collapsed?: boolean;
  onPointerDown?: (e: React.PointerEvent) => void;
  /** 双击块身开详情面板（对勾上的双击不算——那是两次完成切换）；
   *  已归档块由 WeekGrid 换成「跳归档区」回调 */
  onDoubleClick?: () => void;
  onSetColumn?: (taskId: string, col: "todo" | "done") => void;
}) {
  const topPct = ((seg.startMin - DAY_START_MIN) / DAY_SPAN_MIN) * 100;
  const hPct = ((seg.endMin - seg.startMin) / DAY_SPAN_MIN) * 100;
  const dur = seg.endMin - seg.startMin;
  const rows = titleRows(seg.startMin, seg.endMin);
  const thin = rows === 1;
  const narrow = seg.lanes > 1;
  const bot = !!task.botAssigned;
  const stack = collapsed ? stackSize(dur) : null;
  const toggle = () => onSetColumn?.(task.id, done ? "todo" : "done");
  const doneBtn = !archived &&
    onSetColumn && (
      <DoneCircle
        done={done}
        onToggle={toggle}
        title={done ? "恢复待办" : "标记完成"}
        size={stack ? stack.size : 12}
      />
    );
  /** 已归档印章（薄块/塌缩簇有独立形态） */
  const stamp = (
    <span className={`plan-stamp${thin ? " is-flat" : ""}`} aria-hidden>
      已归档
    </span>
  );
  const tip = useHoverTitleTip();
  const skipTip = useRef(false);
  const cls = `plan-blk${stack ? ` plan-stack${stack.cls}` : ""}${thin ? " thin" : ""}${narrow && !stack ? " narrow" : ""}${selected ? " sel" : ""}${seg.tail ? " tail" : ""}${seg.head ? " head" : ""}${dimmed ? " opacity-35" : ""}${fresh ? " plan-fresh" : ""}${done ? " is-done" : ""}${archived ? " is-archived" : ""}`;
  return (
    <div
      className={cls}
      style={
        {
          "--c": color,
          top: `${topPct}%`,
          height: `calc(${hPct}% - 2px)`,
          left: `calc(3px + ${seg.lane} * (100% - 6px) / ${seg.lanes})`,
          width: `calc((100% - 6px) / ${seg.lanes} - 4px)`,
        } as React.CSSProperties
      }
      onPointerEnter={(e) => {
        if (!skipTip.current)
          tip.show(archived ? `已归档 · 双击去归档区查看｜${task.title}` : task.title)(e);
      }}
      onPointerLeave={() => {
        skipTip.current = false;
        tip.hide();
      }}
      onPointerDown={(e) => {
        skipTip.current = true;
        tip.hide();
        onPointerDown?.(e);
      }}
      onDoubleClick={(e) => {
        // 完成对勾上的双击 = 两次切换，不开面板（click 已 stopPropagation，
        // 但 dblclick 是独立事件仍会冒泡到块身）
        if ((e.target as HTMLElement).closest("button")) return;
        onDoubleClick?.();
      }}
    >
      <span className="plan-dot plan-dot-s" aria-hidden />
      <span className="plan-dot plan-dot-e" aria-hidden />
      {stack ? (
        <div className="plan-stack-bar">
          <ActorAvatar bot={bot} size={stack.size} />
          {archived ? <span className="plan-stamp-dot" aria-hidden /> : doneBtn}
        </div>
      ) : thin ? (
        <>
          <ActorAvatar bot={bot} size={12} />
          <div className="plan-blk-title">{task.title}</div>
          {archived ? stamp : doneBtn}
        </>
      ) : (
        <>
          <div className="plan-topbar">
            <ActorAvatar bot={bot} size={12} />
            {archived ? stamp : doneBtn}
          </div>
          <div className="plan-blk-title" data-rows={rows}>
            {task.title}
          </div>
          <span className="plan-blk-time num">{fmtHours(dur)} 小时</span>
        </>
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
      {!archived && <span className="plan-grip" aria-hidden data-grip={task.id} />}
      {tip.chip}
    </div>
  );
}
