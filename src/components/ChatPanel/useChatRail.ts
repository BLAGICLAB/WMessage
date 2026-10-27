/** 聊天面板的「rail」逻辑块（从 ChatPanel 主组件搬出）。
 *
 *  会话栏与右侧上下文栏的开合状态。
 *
 *  搬移是逐字的：语句原样保留、顺序不变；对外接口只有 params 与返回值。
 */
import { useCallback, useState } from "react";
import type { ContextTab, FileSubTab } from "./ContextRail";

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
  /** 文件页的子页：改动（轨迹）/ 交付物（产物）。
   *  从气泡交付物跳过来看的是产物 → 切到交付物页；从「这轮有产物没点名」
   *  的提示跳过去看的是改动 → 切到改动页。跳转方决定落点，栏内不猜。 */
  const [fileSubTab, setFileSubTab] = useState<FileSubTab>("changes");
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
    fileSubTab,
    openSubagentDetail,
    railOpen,
    setCtxOpen,
    setCtxTab,
    setFileSubTab,
    setSubagentDetailId,
    setSubagentStripOpen,
    subagentDetailId,
    subagentStripOpen,
    toggleCtx,
    toggleRail,
  };
}
