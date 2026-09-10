import { useEffect, useRef, useState } from "react";

/**
 * 内联文本编辑统一行为：草稿 state + Enter 提交 / Escape 取消 /
 * Blur 提交。TaskCardContent（挂件）与 TodoCard（主窗口）的标题内联编辑原先各写
 * 一份，行为漂移过 E4 类 bug（Escape 取消后 blur 又把草稿提交）；现统一走本 hook。
 *
 * 行为约定：
 * - 进入编辑态（editing false→true）时草稿重置为当前已提交值；
 *   编辑中外部值变化不覆盖用户输入；
 * - Enter（非 IME 组词中）提交草稿；
 * - Escape 恢复草稿为已提交值并调 onCancel；若随后 blur 仍触发（编辑态由父组件
 *   控制、input 尚未卸载），本次 blur 必须跳过提交；
 * - Blur（未被 Escape 取消）提交草稿。
 */
export function useInlineEdit({
  value,
  editing,
  onCommit,
  onCancel,
}: {
  /** 当前已提交值（进入编辑态时同步为草稿初值） */
  value: string;
  /** 是否处于编辑态 */
  editing: boolean;
  /** 提交草稿（Enter / 未取消的 Blur） */
  onCommit: (draft: string) => void;
  /** 取消编辑（Escape）；可选 */
  onCancel?: () => void;
}) {
  const [draft, setDraft] = useState(value);
  // 进入编辑态（false→true）那一瞬重置草稿为当前值
  const prevEditing = useRef(editing);
  useEffect(() => {
    if (editing && !prevEditing.current) setDraft(value);
    prevEditing.current = editing;
    // value 不进依赖：编辑中外部值变化不得覆盖用户输入
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editing]);
  // E4：Escape 取消标记——取消后若 blur 仍触发，必须跳过本次提交；
  // 提交/跳过后重置标记，不污染下一轮编辑
  const cancelledRef = useRef(false);

  const commit = () => {
    cancelledRef.current = false; // 显式提交，重置取消标记
    onCommit(draft);
  };
  const cancel = () => {
    cancelledRef.current = true;
    setDraft(value);
    onCancel?.();
  };
  /** blur 提交：Escape 已取消则跳过并重置标记 */
  const onBlur = () => {
    if (cancelledRef.current) {
      cancelledRef.current = false;
      return;
    }
    onCommit(draft);
  };
  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter" && !e.nativeEvent.isComposing) commit();
    if (e.key === "Escape") cancel();
  };

  return { draft, setDraft, onBlur, onKeyDown };
}
