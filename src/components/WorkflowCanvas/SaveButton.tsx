//! SaveButton — 圆形保存按钮（图标 only，与删除键同尺寸）
//!
//! 交互：
//! - 圆形 36×36，Save 图标；有未保存改动时右上角一个品牌色小圆点
//! - 按下 → 轻微凹陷（is-pressed）
//! - 松开 → 保存（is-saving，Loader 自旋）
//! - 成功 → 对勾 + 「已保存」（is-done，按钮撑宽、泛绿）；~1.4s 后回 idle
//! - 失败（onSave 返回 false）→ 回 idle；错误文案由父级 handleCommandError 兜底
//! - 键盘 Enter/Space 等价点击
//!
//! data-phase 暴露状态给测试：idle | pressed | saving | done

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type KeyboardEvent as RKeyboardEvent,
  type PointerEvent as RPointerEvent,
} from "react";
import { Check, Loader2, Save } from "lucide-react";

/** 「已保存」展示时长（毫秒） */
const DONE_HOLD_MS = 1400;
/** 按下态最短展示时长（毫秒）——快速点击也能看到凹陷反馈 */
const MIN_PRESS_MS = 130;

type Phase = "idle" | "pressed" | "saving" | "done";

type Props = {
  /** 有未保存改动（决定可点 + 右上角小圆点） */
  dirty: boolean;
  /** 父级保存进行中（外部 busy） */
  saving: boolean;
  /** 触发保存；返回 false 表示失败（不显示「已保存」） */
  onSave: () => unknown | Promise<unknown>;
  title?: string;
  className?: string;
  ariaLabel?: string;
};

const delay = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

export function SaveButton({
  dirty,
  saving,
  onSave,
  title = "保存工作流",
  className = "",
  ariaLabel = "保存工作流",
}: Props) {
  const [phase, setPhase] = useState<Phase>("idle");
  // refs：闭包读最新值，避免过期
  const phaseRef = useRef<Phase>("idle");
  const pressedAtRef = useRef<number | null>(null);
  const doneTimerRef = useRef<number | null>(null);
  const busyRef = useRef(false);
  const mountedRef = useRef(true);
  const dirtyRef = useRef(dirty);
  const savingRef = useRef(saving);
  const onSaveRef = useRef(onSave);

  useEffect(() => {
    phaseRef.current = phase;
  }, [phase]);
  useEffect(() => {
    dirtyRef.current = dirty;
  }, [dirty]);
  useEffect(() => {
    savingRef.current = saving;
  }, [saving]);
  useEffect(() => {
    onSaveRef.current = onSave;
  }, [onSave]);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      if (doneTimerRef.current) window.clearTimeout(doneTimerRef.current);
    };
  }, []);

  const disabled = !dirty || saving || phase === "saving" || phase === "done";

  const runSave = useCallback(async () => {
    if (busyRef.current) return;
    if (!dirtyRef.current || savingRef.current) {
      setPhase("idle");
      return;
    }
    busyRef.current = true;
    // 保证「按下凹陷」至少可见 MIN_PRESS_MS
    const held =
      pressedAtRef.current != null ? performance.now() - pressedAtRef.current : MIN_PRESS_MS;
    if (held < MIN_PRESS_MS) await delay(MIN_PRESS_MS - held);
    if (!mountedRef.current) {
      busyRef.current = false;
      return;
    }
    setPhase("saving");
    let ok: unknown = true;
    try {
      ok = await onSaveRef.current();
    } catch {
      ok = false;
    }
    if (!mountedRef.current) {
      busyRef.current = false;
      return;
    }
    if (ok === false) {
      setPhase("idle");
      busyRef.current = false;
      return;
    }
    setPhase("done");
    if (doneTimerRef.current) window.clearTimeout(doneTimerRef.current);
    doneTimerRef.current = window.setTimeout(() => {
      if (!mountedRef.current) return;
      setPhase("idle");
      busyRef.current = false;
      pressedAtRef.current = null;
    }, DONE_HOLD_MS);
  }, []);

  const releaseCapture = (el: HTMLButtonElement, pointerId: number) => {
    try {
      if (el.hasPointerCapture(pointerId)) el.releasePointerCapture(pointerId);
    } catch {
      /* 旧 webview 可能不支持 */
    }
  };

  const onPointerDown = (e: RPointerEvent<HTMLButtonElement>) => {
    if (disabled) return;
    if (e.button !== 0) return; // 仅主键
    e.preventDefault();
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      /* ignore */
    }
    pressedAtRef.current = performance.now();
    setPhase("pressed");
  };
  const onPointerUp = (e: RPointerEvent<HTMLButtonElement>) => {
    if (phaseRef.current !== "pressed") return;
    releaseCapture(e.currentTarget, e.pointerId);
    void runSave();
  };
  const onPointerAbort = (e: RPointerEvent<HTMLButtonElement>) => {
    if (phaseRef.current !== "pressed") return;
    releaseCapture(e.currentTarget, e.pointerId);
    setPhase("idle");
  };
  const onKeyDown = (e: RKeyboardEvent<HTMLButtonElement>) => {
    if (disabled) return;
    if (e.key !== "Enter" && e.key !== " ") return;
    e.preventDefault();
    pressedAtRef.current = performance.now();
    setPhase("pressed");
    void runSave();
  };

  const showDot = dirty && phase === "idle" && !saving;

  return (
    <button
      type="button"
      className={`save-btn ${phase === "pressed" ? "is-pressed" : ""} ${
        phase === "saving" ? "is-saving" : ""
      } ${phase === "done" ? "is-done" : ""} ${className}`}
      onPointerDown={onPointerDown}
      onPointerUp={onPointerUp}
      onPointerLeave={onPointerAbort}
      onPointerCancel={onPointerAbort}
      onKeyDown={onKeyDown}
      disabled={disabled}
      title={title}
      aria-label={ariaLabel}
      aria-busy={phase === "saving"}
      data-testid="save-btn"
      data-phase={phase}
    >
      <span className="save-btn__face" aria-hidden>
        {phase === "done" ? (
          <Check size={16} />
        ) : phase === "saving" ? (
          <Loader2 size={16} className="save-btn__spin" />
        ) : (
          <Save size={16} />
        )}
      </span>
      {phase === "done" && <span className="save-btn__label">已保存</span>}
      {showDot && <span className="save-btn__dot" data-testid="save-btn-dot" aria-hidden />}
    </button>
  );
}
