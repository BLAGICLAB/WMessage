import type { ButtonHTMLAttributes, ReactNode } from "react";

type IconButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  /** 单个 lucide 图标（stroke 线性） */
  children: ReactNode;
};

/** 图标按钮基件：.nm-icon-btn 扁平材质（无底色，hover 淡底提字）， *  尺寸/圆角/语义色由使用处 className 补充（如 rounded-full、hover:text-[var(--danger)]） */
export function IconButton({
  className = "",
  type = "button",
  children,
  ...rest
}: IconButtonProps) {
  return (
    <button
      type={type}
      className={className ? `nm-icon-btn ${className}` : "nm-icon-btn"}
      {...rest}
    >
      {children}
    </button>
  );
}
