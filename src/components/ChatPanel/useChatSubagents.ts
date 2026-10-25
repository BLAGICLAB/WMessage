/** 聊天面板的「subagents」逻辑块（从 ChatPanel 主组件搬出）。
 *
 *  子 agent 行、活动流与派生视图。
 *
 *  搬移是逐字的：语句原样保留、顺序不变；对外接口只有 params 与返回值。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTauriListen } from "../../lib/useTauriListen";
import { applyActivityEvent, SubagentActivityEvent, SubagentActivityItem } from "../../lib/subagentActivity";
import { handleCommandError } from "../../lib/errorHandler";
import type { SubagentRow } from "./types";
import { SUBAGENT_POLL_MS } from "./constants";
import type { SubagentRun } from "./ContextRail";
import { parseJsonField } from "./parse";
import type { Dispatch, RefObject, SetStateAction } from "react";

export type ChatSubagentsParams = {
  enabled: boolean;
  openSubagentDetail: (id: string) => void;
  sessionId: string | null;
  sessionIdRef: RefObject<string | null>;
  setSubagentStripOpen: Dispatch<SetStateAction<boolean>>;
  subagentStripOpen: boolean;
};

export function useChatSubagents(params: ChatSubagentsParams) {
  const {
    enabled,
    openSubagentDetail,
    sessionId,
    sessionIdRef,
    setSubagentStripOpen,
    subagentStripOpen,
  } = params;

  const [subagents, setSubagents] = useState<SubagentRow[]>([]);
  const refreshSubagentsRef = useRef<() => void>(() => {});
  const refreshSubagents = useCallback(() => {
    if (!enabled || !sessionId) {
      setSubagents([]);
      return;
    }
    invoke<SubagentRow[] | null>("list_subagents", { sessionId, limit: 20 })
      .then((rows) => {
        // 竞态：await 期间用户已切走，慢响应不得盖掉新会话的列表。
        // 非数组（命令失败/旧版后端返回 null）当空列表，不让渲染炸掉
        if (sessionIdRef.current === sessionId) {
          setSubagents(Array.isArray(rows) ? rows : []);
        }
      })
      .catch(() => setSubagents([]));
  }, [enabled, sessionId]);
  useEffect(() => {
    refreshSubagentsRef.current = refreshSubagents;
    refreshSubagents();
  }, [refreshSubagents]);
  const [subagentActivity, setSubagentActivity] = useState<
    Record<string, SubagentActivityItem[]>
  >({});
  useEffect(() => {
    setSubagentActivity({});
  }, [sessionId]);
  useTauriListen<SubagentActivityEvent>("subagent-activity", (payload) => {
    setSubagentActivity((prev) => ({
      ...prev,
      [payload.subagentId]: applyActivityEvent(
        prev[payload.subagentId] ?? [],
        payload,
      ),
    }));
  });
  const hasPendingSubagent = subagents.some(
    (r) => r.status === "running" || r.status === "queued",
  );
  useEffect(() => {
    if (!enabled || !sessionId || !hasPendingSubagent) return;
    const t = window.setInterval(() => {
      refreshSubagentsRef.current();
    }, SUBAGENT_POLL_MS);
    return () => window.clearInterval(t);
  }, [enabled, sessionId, hasPendingSubagent]);
  const subagentRuns = useMemo<SubagentRun[]>(
    () =>
      subagents.map((r) => ({
        id: r.id,
        title: r.objective,
        status: r.status,
        running: r.status === "running" || r.status === "queued",
        error: r.error ?? null,
        // 轮数与时间戳原样带下去：单行摘要要显示「已跑 N/M 轮 · 运行时长」，
        // 在这里压掉就得让组件自己再拉一次行，等于把一次 IPC 复制成 N 次
        roundsUsed: r.roundsUsed ?? null,
        roundsBudget: r.roundsBudget ?? null,
        toolCallsUsed: r.toolCallsUsed ?? null,
        toolCallsBudget: r.toolCallsBudget ?? null,
        // 动作流只在途挂（事件即焚，终态行没有新动作）
        activity:
          r.status === "running" || r.status === "queued"
            ? subagentActivity[r.id]
            : undefined,
        startedAt: r.startedAt ?? null,
        finishedAt: r.finishedAt ?? null,
        profile: r.profile,
        contextSummary: r.contextSummary ?? null,
        acceptance: parseJsonField(r.acceptanceJson),
        result: parseJsonField(r.resultJson),
        traceId: r.traceId,
      })),
    [subagents, subagentActivity],
  );
  const cancelSubagent = useCallback((id: string) => {
    invoke("cancel_subagent", { key: id }).catch((e) =>
      handleCommandError(e, "cancel_subagent"),
    );
  }, []);
  const toggleSubagentStrip = useCallback(
    () => setSubagentStripOpen((v) => !v),
    [],
  );
  const subagentSlot = useMemo(
    () =>
      subagentRuns.length === 0
        ? undefined
        : {
            runs: subagentRuns,
            expanded: subagentStripOpen,
            onOpen: openSubagentDetail,
            onCancel: cancelSubagent,
            onToggleExpanded: toggleSubagentStrip,
          },
    [
      subagentRuns,
      subagentStripOpen,
      openSubagentDetail,
      cancelSubagent,
      toggleSubagentStrip,
    ],
  );

  return {
    cancelSubagent,
    refreshSubagentsRef,
    subagentRuns,
    subagentSlot,
  };
}
