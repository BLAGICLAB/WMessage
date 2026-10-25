/** 聊天面板的「rail」逻辑块（从 ChatPanel 主组件搬出）。
 *
 *  会话栏与右侧上下文栏的开合状态。
 *
 *  搬移是逐字的：语句原样保留、顺序不变；对外接口只有 params 与返回值。
 */
import { useCallback, useState } from "react";
import type { ContextTab } from "./ContextRail";

export function useChatRail() {
  const [railOpen, setRailOpen] = useState(
    () => localStorage.getItem("chat-rail-open") !== "0",
  );
  const toggleRail = () => {
    setRailOpen((v) => {
      localStorage.setItem("chat-rail-open", v ? "0" : "1");
      return !v;
    });
  };
  const [ctxOpen, setCtxOpen] = useState(
    () => localStorage.getItem("chat-ctx-open") === "1",
  );
  const [ctxTab, setCtxTab] = useState<ContextTab>("tasks");
  const [subagentStripOpen, setSubagentStripOpen] = useState(false);
  const toggleCtx = () => {
    setCtxOpen((v) => {
      localStorage.setItem("chat-ctx-open", v ? "0" : "1");
      return !v;
    });
  };
  const [subagentDetailId, setSubagentDetailId] = useState<string | null>(null);
  const openSubagentDetail = useCallback((id: string) => {
    setCtxOpen(true);
    localStorage.setItem("chat-ctx-open", "1");
    setCtxTab("agents");
    setSubagentDetailId(id);
  }, []);
  const closeSubagentDetail = useCallback(() => setSubagentDetailId(null), []);

  return {
    closeSubagentDetail,
    ctxOpen,
    ctxTab,
    openSubagentDetail,
    railOpen,
    setCtxOpen,
    setCtxTab,
    setSubagentDetailId,
    setSubagentStripOpen,
    subagentDetailId,
    subagentStripOpen,
    toggleCtx,
    toggleRail,
  };
}
