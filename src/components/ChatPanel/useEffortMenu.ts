/** 聊天面板的「effort」逻辑块（从 ChatPanel 主组件搬出）。
 *
 *  推理强度档位：全局档位 + 按会话覆盖。
 *
 *  搬移是逐字的：语句原样保留、顺序不变；对外接口只有 params 与返回值。
 */
import { useRef, useState } from "react";
import type { ReasoningLevel } from "./types";

export function useEffortMenu() {
  const [effortBase, setEffortBase] = useState<ReasoningLevel>("medium");
  const [effortBySession, setEffortBySession] = useState<
    Map<string, ReasoningLevel>
  >(new Map());
  const [effortMenuOpen, setEffortMenuOpen] = useState(false);
  const effortBtnRef = useRef<HTMLButtonElement>(null);
  const effortDropdownRef = useRef<HTMLDivElement>(null);

  return {
    effortBase,
    effortBtnRef,
    effortBySession,
    effortDropdownRef,
    effortMenuOpen,
    setEffortBase,
    setEffortBySession,
    setEffortMenuOpen,
  };
}
