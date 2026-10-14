// 弹层模板：backdrop（fixed inset-0 全屏遮罩）+ 面板元素（children 单元素）。
// 收口各处 modal 的 backdrop + stopPropagation 样板；z 档/尺寸各异，类串归调用方。
import { cloneElement, isValidElement, useEffect } from "react";
import { createPortal } from "react-dom";

type ModalProps = Omit<React.ComponentProps<"div">, "children"> & {
  /** 面板的**单个元素**（nm-card 及 role/aria/data-testid 留在调用方元素上）；
   * Modal 会注入 onPointerDown stopPropagation，使面板内点击不触发 backdrop 关闭，
   * 面板自有的 onPointerDown 会先被调用 */
  children: React.ReactElement<React.HTMLAttributes<HTMLDivElement>>;
  /** backdrop 按下（onPointerDown）即关；与 closeOnOutside 同控，缺省开 */
  onClose?: () => void;
  /** backdrop 按下关闭开关；缺省开（关闭策略各异的弹窗置 false 并自带关闭入口） */
  closeOnOutside?: boolean;
  /** Esc 关闭（挂 window keydown 监听）；缺省关 */
  closeOnEsc?: boolean;
  /** true = createPortal 到 document.body：卡片容器有 transform 包含块
   * （fixed 被裁缩到卡片边界）时必须开 */
  portal?: boolean;
};

export function Modal({
  onClose,
  closeOnOutside = true,
  closeOnEsc = false,
  portal = false,
  className,
  children,
  ...rest
}: ModalProps) {
  useEffect(() => {
    if (!(closeOnEsc && onClose)) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [closeOnEsc, onClose]);

  const panel = isValidElement<React.HTMLAttributes<HTMLDivElement>>(children)
    ? cloneElement(children, {
        onPointerDown: (e: React.PointerEvent<HTMLDivElement>) => {
          children.props.onPointerDown?.(e);
          e.stopPropagation();
        },
      })
    : children;

  const node = (
    <div
      {...rest}
      className={className}
      onPointerDown={closeOnOutside && onClose ? onClose : undefined}
    >
      {panel}
    </div>
  );
  // className 由调用方给全：z 档/padding 各异，且 Tailwind JIT 只扫描源码
  // 字面量，prop 变量拼出的类不会被生成
  return portal ? createPortal(node, document.body) : node;
}
