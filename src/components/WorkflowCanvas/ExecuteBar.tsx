//! ExecuteBar — 工作流执行「液面罐子」长条按钮
//!
//! 视觉：长条圆角「罐子」内装绿色液体，液面宽度 = 已完成任务 / 总任务卡。
//! 每执行完一张任务卡（doneCount 递增），液面前进一格（width 过渡 + 斜纹流光）。
//! 任务卡未执行完 / 液体未满 → 可继续执行；两者都满 → 禁用。
//!
//! 文案：
//! - 无任务卡          "请先添加任务卡"（禁用）
//! - 未选工作流        "先选择工作流"（禁用）
//! - 有未保存改动      "请先保存"（禁用）
//! - 全部完成/满罐     "已完成"（禁用，液体满 + 轻脉动）
//! - 执行中            "执行中…"（点击 = 停止）
//! - 否则              doneCount>0 ? "继续执行" : "开始执行"

import type { CSSProperties } from "react";
import { Check, Play, Square } from "lucide-react";

type Props = {
  /** 已完成任务卡数 */
  doneCount: number;
  /** 任务卡总数（画布节点数） */
  total: number;
  /** 执行中 */
  running: boolean;
  /** 有未保存改动（禁用执行） */
  dirty: boolean;
  /** 是否已选中/打开工作流 */
  hasActive: boolean;
  onStart: () => void;
  onStop: () => void;
  className?: string;
};

export function ExecuteBar({
  doneCount,
  total,
  running,
  dirty,
  hasActive,
  onStart,
  onStop,
  className = "",
}: Props) {
  const pct = total > 0 ? Math.min(100, Math.max(0, (doneCount / total) * 100)) : 0;
  const empty = total === 0;
  const isFull = total > 0 && doneCount >= total;

  const blockedIdle = empty || !hasActive || dirty || isFull;
  // 执行中可点（= 停止）；其余非 blocked 可点（= 开始/继续）
  const disabled = !running && blockedIdle;

  let label: string;
  let phase: string;
  if (empty) {
    label = "请先添加任务卡";
    phase = "empty";
  } else if (running) {
    label = "执行中…";
    phase = "running";
  } else if (!hasActive) {
    label = "先选择工作流";
    phase = "blocked";
  } else if (dirty) {
    label = "请先保存";
    phase = "dirty";
  } else if (isFull) {
    label = "已完成";
    phase = "full";
  } else if (doneCount > 0) {
    label = "继续执行";
    phase = "continue";
  } else {
    label = "开始执行";
    phase = "ready";
  }

  const handleClick = () => {
    if (running) {
      onStop();
      return;
    }
    if (blockedIdle) return;
    onStart();
  };

  // 液面裁剪：白色文案副本按液体比例裁切，保证跨越液面时文字对比度稳定
  const clip: CSSProperties = {
    clipPath: `inset(0 ${100 - pct}% 0 0)`,
    WebkitClipPath: `inset(0 ${100 - pct}% 0 0)`,
  };

  const icon = running ? (
    <Square size={13} className="exec-bar__icon" aria-hidden />
  ) : isFull ? (
    <Check size={15} className="exec-bar__icon" aria-hidden />
  ) : (
    <Play size={14} className="exec-bar__icon" aria-hidden />
  );
  const content = (
    <>
      {icon}
      <span className="exec-bar__label">{label}</span>
      {!empty && (
        <span className="exec-bar__count">
          {doneCount}/{total}
        </span>
      )}
    </>
  );

  return (
    <button
      type="button"
      className={`exec-bar ${running ? "is-running" : ""} ${isFull ? "is-full" : ""} ${
        pct > 0 ? "has-liquid" : ""
      } ${className}`}
      onClick={handleClick}
      disabled={disabled}
      title={label}
      aria-label={label}
      data-testid="exec-bar"
      data-phase={phase}
      data-pct={pct.toFixed(2)}
    >
      <span className="exec-bar__liquid" style={{ width: `${pct}%` }} aria-hidden>
        <span className="exec-bar__flow" />
      </span>
      <span className="exec-bar__inner">{content}</span>
      <span className="exec-bar__inner exec-bar__inner--over" style={clip} aria-hidden>
        {content}
      </span>
    </button>
  );
}
