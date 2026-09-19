//! Toggle 组件：iOS 风格 pill switch（红色 ON / 灰色 OFF）
//!
//! 老板 16:05 拍板：EvolutionPanel 改造，原按钮点击模式 → 图示 toggle 模式。
//! 受控组件：父组件管状态，Toggle 只负责显示 + 触发 onChange。
//! 视觉规则：disabled 时透明度降；aria-pressed 表达语义。

import { useId } from "react";

type Props = {
  checked: boolean;
  onChange: (next: boolean) => void;
  disabled?: boolean;
  /** ARIA 标签（Tauri webview / screen reader 用） */
  ariaLabel?: string;
  /** 尺寸：sm=24px，md=28px（默认） */
  size?: "sm" | "md";
};

export function Toggle({
  checked,
  onChange,
  disabled = false,
  ariaLabel,
  size = "md",
}: Props) {
  const id = useId();
  // md: w-11 h-6 (44x24px)；sm: w-9 h-5 (36x20px)
  const dims =
    size === "sm"
      ? "w-9 h-5"
      : "w-11 h-6";
  const knobChecked =
    size === "sm"
      ? "translate-x-[18px]"
      : "translate-x-[22px]";

  return (
    <button
      type="button"
      role="switch"
      id={id}
      aria-checked={checked}
      aria-label={ariaLabel}
      disabled={disabled}
      onClick={() => !disabled && onChange(!checked)}
      className={[
        "relative inline-flex shrink-0 items-center rounded-full transition-colors duration-150",
        dims,
        checked ? "bg-[#ef4444]" : "bg-[var(--border)]",
        disabled ? "opacity-50 cursor-not-allowed" : "cursor-pointer",
      ].join(" ")}
      data-testid="toggle"
    >
      <span
        aria-hidden
        className={[
          "inline-block transform rounded-full bg-white shadow transition-transform duration-150",
          size === "sm" ? "h-4 w-4" : "h-5 w-5",
          checked ? knobChecked : "translate-x-0.5",
        ].join(" ")}
      />
    </button>
  );
}
