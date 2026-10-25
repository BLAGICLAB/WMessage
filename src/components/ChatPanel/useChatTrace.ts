/** 聊天面板的「trace」逻辑块（从 ChatPanel 主组件搬出）。
 *
 *  追踪面板：复制反馈、执行收尾提示与追踪展开态。
 *
 *  搬移是逐字的：语句原样保留、顺序不变；对外接口只有 params 与返回值。
 */
import { useState } from "react";
import type { TaskRef } from "./types";

export type ChatTraceParams = {
  setRefs: (v: TaskRef[] | ((prev: TaskRef[]) => TaskRef[])) => void;
};

export function useChatTrace(params: ChatTraceParams) {
  const { setRefs } = params;

  const [copiedIdx, setCopiedIdx] = useState<number | null>(null);
  const [stopNotice, setStopNotice] = useState<{
    sid: string;
    n: number;
  } | null>(null);
  const [openTrace, setOpenTrace] = useState<{
    traceId: number;
    sessionId: string | null;
  } | null>(null);
  const refTask = (t: TaskRef) => {
    setRefs((prev) =>
      prev.some((x) => x.id === t.id) ? prev : [...prev, t],
    );
  };

  return {
    copiedIdx,
    openTrace,
    refTask,
    setCopiedIdx,
    setOpenTrace,
    setStopNotice,
    stopNotice,
  };
}
