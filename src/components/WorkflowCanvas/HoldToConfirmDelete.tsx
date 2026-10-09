//! HoldToConfirmDelete — 长按确认圆形删除按钮
//!
//! 交互：
//! - 圆形 trash 图标按钮 + 外圈进度环（SVG circle + stroke-dashoffset 动画）
//! - pointerdown 启动 1.5s 计时，进度环顺时针走满
//! - 中途 pointerup/pointerleave/pointercancel：进度环平滑倒退归零
//! - 走满 → 左右震动一次（CSS keyframe）→ 渲染对勾 + "已删除" 文本 → 触发 onConfirm
//!
//! 设计：
//! - 用 rAF 推进进度，时间戳驱动；不依赖 CSS 动画时长（动画只管显示）
//! - 反向动画用同一 rAF 回路：phase 切到 "releasing"，从当前 progress 倒推到 0
//! - 走满→震动→done 的过渡用 window.setTimeout；句柄 ref 跟踪，卸载时清
//! - 键盘 Enter/Space：单次直接确认（键盘无"持续按住"语义，不强加手势）
//! - "done" 状态 ~1.2s 后自动复位 idle，给父组件以未卸载时回退的机会

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { Check, Trash2 } from "lucide-react";

/** 长按总时长（毫秒）。用户约定 1.5s */
export const HOLD_MS = 1500;
/** 走满后震动动画时长（毫秒），与 CSS keyframe 360ms 同步 */
const SHAKE_MS = 360;
/** "已删除" 状态展示时长（毫秒） */
const DONE_HOLD_MS = 1200;
/** 进度环 SVG 半径（32×32 viewBox）——环中心线直径 30px，贴 36px 按钮内侧边缘（内圈约 1px 间隙） */
const RING_R = 15;
/** 进度环周长（用于 strokeDasharray） */
const RING_CIRC = 2 * Math.PI * RING_R;
/** 进度环 SVG 显示边长（px），缩到按钮内侧 */
const RING_SIZE = 32;
/** 进度环 viewBox 中心坐标 */
const RING_C = 16;

type Phase = "idle" | "pressing" | "releasing" | "done";

type Props = {
  /** 长按走满时触发（同步或异步）。父组件做实际删除动作 */
  onConfirm: () => void | Promise<void>;
  /** 禁用：禁用时不响应 pointer 事件，呈现灰态 */
  disabled?: boolean;
  /** 鼠标悬停提示 */
  title?: string;
  /** 自定义类名（覆盖布局/位置） */
  className?: string;
  /** 命中测试/无障碍标签；默认「长按删除工作流」 */
  ariaLabel?: string;
};

export function HoldToConfirmDelete({
  onConfirm,
  disabled = false,
  title = "长按删除（按住 1.5 秒确认）",
  className = "",
  ariaLabel = "长按删除工作流",
}: Props) {
  const [phase, setPhase] = useState<Phase>("idle");
  const [progress, setProgress] = useState(0); // 0..1
  const [shaking, setShaking] = useState(false);

  // rAF / 时间戳：跨阶段复用同一推进循环，phase 决定推进方向
  const rafRef = useRef<number | null>(null);
  const startedAtRef = useRef<number | null>(null);
  const startProgressRef = useRef(0); // 反向时记录的起点进度
  // 走满→震动→done 的过渡计时器
  const shakeTimerRef = useRef<number | null>(null);
  // 已触发的 onConfirm 引用：避免 done→idle 复位路径里重复触发
  const confirmFiredRef = useRef(false);

  const cancelRaf = useCallback(() => {
    if (rafRef.current !== null) {
      cancelAnimationFrame(rafRef.current);
      rafRef.current = null;
    }
  }, []);

  const clearShakeTimer = useCallback(() => {
    if (shakeTimerRef.current !== null) {
      window.clearTimeout(shakeTimerRef.current);
      shakeTimerRef.current = null;
    }
  }, []);

  /** 推进循环：pressing → 顺时针到 1；releasing → 从 startProgress 倒退到 0 */
  const tick = useCallback(
    (now: number) => {
      if (startedAtRef.current === null) return;
      const elapsed = now - startedAtRef.current;
      const cur = phaseRef.current;
      if (cur === "pressing") {
        const p = Math.min(1, elapsed / HOLD_MS);
        setProgress(p);
        if (p >= 1) {
          // 走满：取消循环、进入震动态
          cancelRaf();
          startedAtRef.current = null;
          setProgress(1);
          setShaking(true);
          // 震动后切到 done + 触发 onConfirm
          clearShakeTimer();
          shakeTimerRef.current = window.setTimeout(() => {
            shakeTimerRef.current = null;
            setShaking(false);
            setPhase("done");
            if (!confirmFiredRef.current) {
              confirmFiredRef.current = true;
              void onConfirm();
            }
          }, SHAKE_MS);
          return;
        }
      } else if (cur === "releasing") {
        // 反向：从 startProgress 倒推到 0（与填充同速率，反向总时长 = startProgress * HOLD_MS）
        const p = Math.max(0, startProgressRef.current - elapsed / HOLD_MS);
        setProgress(p);
        if (p <= 0) {
          cancelRaf();
          startedAtRef.current = null;
          setPhase("idle");
          setProgress(0);
          return;
        }
      } else {
        // 防御性：其他 phase 不该在 tick 里
        cancelRaf();
        startedAtRef.current = null;
        return;
      }
      rafRef.current = requestAnimationFrame(tick);
    },
    [cancelRaf, clearShakeTimer, onConfirm]
  );

  // phaseRef 镜像：tick 闭包避免每帧重建
  const phaseRef = useRef<Phase>("idle");
  useEffect(() => {
    phaseRef.current = phase;
  }, [phase]);

  // 启动推进：统一入口
  const startTick = useCallback(
    (initialPhase: Phase, startProgress: number) => {
      cancelRaf();
      startedAtRef.current = performance.now();
      startProgressRef.current = startProgress;
      setPhase(initialPhase);
      rafRef.current = requestAnimationFrame(tick);
    },
    [cancelRaf, tick]
  );

  // pointer 处理
  const handlePointerDown = (e: ReactPointerEvent<HTMLButtonElement>) => {
    if (disabled) return;
    // 仅主键（鼠标左键 / 触摸 / 笔）触发——避免右键/中间键
    // 触摸/笔的 button 恒为 0，鼠标右键=2 中键=1，故单一按 button 判断足够
    if (e.button !== 0) return;
    if (phaseRef.current === "done") return; // done 态期间不重开
    // 防止文字被选中 / 滚动
    e.preventDefault();
    try {
      (e.currentTarget as HTMLButtonElement).setPointerCapture(e.pointerId);
    } catch {
      // 旧 webview 上 setPointerCapture 可能抛——吞掉
    }
    startTick("pressing", progress);
  };

  const handlePointerRelease = (e: ReactPointerEvent<HTMLButtonElement>) => {
    if (disabled) return;
    if (phaseRef.current !== "pressing") return;
    try {
      if ((e.currentTarget as HTMLButtonElement).hasPointerCapture(e.pointerId)) {
        (e.currentTarget as HTMLButtonElement).releasePointerCapture(e.pointerId);
      }
    } catch {
      // ignore
    }
    // 启动反向：保留当前进度作为起点
    startTick("releasing", progress);
  };

  // 卸载/disabled 变化：清理
  useEffect(() => {
    return () => {
      cancelRaf();
      clearShakeTimer();
    };
  }, [cancelRaf, clearShakeTimer]);

  useEffect(() => {
    if (disabled) {
      cancelRaf();
      clearShakeTimer();
      startedAtRef.current = null;
      if (phaseRef.current !== "done") {
        setShaking(false);
        setPhase("idle");
        setProgress(0);
      }
    }
  }, [disabled, cancelRaf, clearShakeTimer]);

  // done 状态展示 DONE_HOLD_MS 后复位 idle
  useEffect(() => {
    if (phase !== "done") return;
    const t = window.setTimeout(() => {
      setPhase("idle");
      setProgress(0);
      confirmFiredRef.current = false;
    }, DONE_HOLD_MS);
    return () => window.clearTimeout(t);
  }, [phase]);

  // 键盘 Enter/Space：直接确认（无指针"持续"语义；不做两步确认以免
  // 给键盘用户额外摩擦——破坏性动作已通过必须先选工作流来 gate）。
  const handleKeyDown = (e: ReactKeyboardEvent<HTMLButtonElement>) => {
    if (disabled) return;
    if (e.key !== "Enter" && e.key !== " ") return;
    e.preventDefault();
    if (phaseRef.current === "done" || confirmFiredRef.current) return;
    confirmFiredRef.current = true;
    setProgress(1);
    setPhase("done");
    // 跳过震动——键盘用户不需要
    window.setTimeout(() => void onConfirm(), 80);
  };

  // 计算 ring stroke-dashoffset（进度 0 = 全空；1 = 全满）
  const dashOffset = RING_CIRC * (1 - progress);

  const ringStyle: CSSProperties = {
    strokeDasharray: RING_CIRC,
    strokeDashoffset: dashOffset,
  };

  // 渲染
  const isActive = phase === "pressing" || phase === "releasing";
  return (
    <button
      type="button"
      onPointerDown={handlePointerDown}
      onPointerUp={handlePointerRelease}
      onPointerLeave={handlePointerRelease}
      onPointerCancel={handlePointerRelease}
      onKeyDown={handleKeyDown}
      disabled={disabled}
      title={title}
      aria-label={ariaLabel}
      aria-busy={phase === "done"}
      data-testid="hold-confirm-delete"
      data-phase={phase}
      data-progress={progress.toFixed(3)}
      className={`hold-confirm ${shaking ? "hold-confirm-shake-once" : ""} ${
        phase === "done" ? "is-done" : ""
      } ${isActive ? "is-active" : ""} ${className}`}
    >
      <svg
        className="hold-confirm-ring"
        viewBox={`0 0 ${RING_SIZE} ${RING_SIZE}`}
        width={RING_SIZE}
        height={RING_SIZE}
        aria-hidden
      >
        {/* 底层轨道（淡灰） */}
        <circle
          className="hold-confirm-ring-track"
          cx={RING_C}
          cy={RING_C}
          r={RING_R}
        />
        {/* 进度环 */}
        <circle
          className="hold-confirm-ring-fg"
          cx={RING_C}
          cy={RING_C}
          r={RING_R}
          style={ringStyle}
        />
      </svg>
      <span className="hold-confirm-face" aria-hidden>
        {phase === "done" ? (
          <Check size={16} className="hold-confirm-check" />
        ) : (
          <Trash2 size={16} />
        )}
      </span>
      {phase === "done" && (
        <span className="hold-confirm-label">已删除</span>
      )}
    </button>
  );
}
