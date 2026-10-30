/** 聊天面板的「history」逻辑块（从 ChatPanel 主组件搬出）。
 *
 *  历史落盘与读回：写盘、解析、断点续传。
 *
 *  搬移是逐字的：语句原样保留、顺序不变；对外接口只有 params 与返回值。
 */
import { useCallback, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { handleCommandError } from "../../lib/errorHandler";
import type { Msg, Session, TaskRef, ToolCall } from "./types";
import * as streamState from "./streamState";
import { parseTurnStatus } from "./parse";
import type { Dispatch, RefObject, SetStateAction } from "react";

export type ChatHistoryParams = {
  enabled: boolean;
  mergeInflight: (rows: Msg[], sid: string) => Msg[];
  sessionIdRef: RefObject<string | null>;
  setMessages: Dispatch<SetStateAction<Msg[]>>;
  setSessionId: Dispatch<SetStateAction<string | null>>;
  setSessions: Dispatch<SetStateAction<Session[]>>;
  streamHandlersRef: RefObject<Record<string, (p: unknown) => void>>;
};

export function useChatHistory(params: ChatHistoryParams) {
  const {
    enabled,
    mergeInflight,
    sessionIdRef,
    setMessages,
    setSessionId,
    setSessions,
    streamHandlersRef,
  } = params;

  const persistHistory = (sid: string, msgs: Msg[]) =>
    invoke("bot_history_save", {
      sessionId: sid,
      messages: msgs
        .filter((m) => !m.hint)
        .map((m) => ({
          role: m.role,
          content: m.content,
          refsJson: m.refs?.length ? JSON.stringify(m.refs) : null,
          thinking: m.thinking ?? null,
          toolsJson: m.tools?.length ? JSON.stringify(m.tools) : null,
          turnStatus: m.turnStatus ?? null,
        })),
    }).catch((e) =>
      // 后台持久化失败：UI 还能跑，不打断当前对话
      handleCommandError(e, "bot_history_save", { silent: true }),
    );
  const parseHistoryJson = <T,>(
    raw: string | null | undefined,
    field: string,
    rowIdx: number,
    sid: string | null,
  ): T[] | undefined => {
    if (!raw) return undefined;
    try {
      const v = JSON.parse(raw) as T[];
      return Array.isArray(v) ? v : undefined;
    } catch (e) {
      console.error(
        `[chat] 历史行 ${field} 解析失败（会话 ${sid ?? "?"} 第 ${rowIdx + 1} 行），按缺失处理:`,
        e,
      );
      return undefined;
    }
  };
  const rowsToMsgs = (
    rows: {
      role: string;
      content: string;
      refsJson?: string | null;
      thinking?: string | null;
      toolsJson?: string | null;
      turnStatus?: string | null;
    }[],
    sid?: string | null,
  ): Msg[] =>
    rows.map((r, i) => ({
      role: r.role === "user" ? "user" : "assistant",
      content: r.content,
      refs: parseHistoryJson<TaskRef>(r.refsJson, "refsJson", i, sid ?? null),
      thinking: r.thinking ?? undefined,
      tools: parseHistoryJson<ToolCall>(r.toolsJson, "toolsJson", i, sid ?? null),
      turnStatus: parseTurnStatus(r.turnStatus),
    }));
  const loadFinalText = useCallback(async (sid: string, attempt = 0) => {
    try {
      const rows = await invoke<
        {
          role: string;
          content: string;
          refsJson?: string | null;
          thinking?: string | null;
          toolsJson?: string | null;
        }[]
      >("bot_history_load", { sessionId: sid });
      const msgs = rowsToMsgs(rows ?? [], sid);
      const last = msgs[msgs.length - 1];
      if (last?.role === "user" && attempt < 6) {
        window.setTimeout(() => void loadFinalText(sid, attempt + 1), 300);
        return;
      }
      if (sessionIdRef.current === sid) setMessages(msgs);
    } catch (e) {
      handleCommandError(e, "bot_history_load", { silent: true });
    }
  }, []);
  const resumeInflight = useCallback(
    async (sid: string) => {
      if (!enabled) return;
      // 同一会话的两次回灌不能并发（会灌两遍）：判定与置位连着做，中间不能有 await
      if (streamState.isResuming(sid)) return;
      streamState.markResuming(sid);
      try {
        const r = await streamState.fetchReplay(sid, 0);
        if (!r.inflight) {
          // 后端没在跑 → 这一轮已经收尾，正文以库为准（历史已加载）。
          // 本地若还有上一轮的残迹（终态事件丢过才会走到这）一并清掉，
          // 当前视图按库刷一遍；invoke 驱动的轮例外，收尾权在 runChat。
          if (
            !streamState.isInvoked(sid) &&
            (streamState.seqOf(sid) > 0 || streamState.isInflight(sid))
          ) {
            streamState.dropSession(sid);
            if (sessionIdRef.current === sid) void loadFinalText(sid);
          }
          return;
        }
        // 后端在跑。本地游标与回放窗口**连续**（本地一直跟着这一轮）→ 按
        // seq 增量补，重叠段被去重闸挡掉；**不连续**（游标属于上一轮 / 缓冲
        // 头部被裁）→ 本地状态对不上这一轮，清干净整轮重建——上一轮的残文
        // 混进这一轮比缺一段更糟。本地没有游标（= 0）时无可混的旧文，缓冲
        // 有多少灌多少（缺头是缓冲裁剪所致，终态以库为准收口）。
        const local = streamState.seqOf(sid);
        const first = r.events[0]?.seq;
        const continuous =
          local === 0 ||
          (!r.truncated && first !== undefined && first <= local + 1);
        if (!continuous) {
          streamState.setMeta(sid, {});
          streamState.resetCursor(sid);
          // 视图里若还挂着旧内容的流式气泡（mergeInflight 按旧 meta 建的），
          // 正文清空等回放重灌——不清就是新旧两段拼一条
          if (sessionIdRef.current === sid) {
            setMessages((prev) => {
              const last = prev[prev.length - 1];
              if (!last?.streaming) return prev;
              const copy = [...prev];
              copy[copy.length - 1] = { ...last, content: "" };
              return copy;
            });
          }
        }
        // 补上在途标记：处理器按「非当前会话且不在途」过滤，不置位就接不住
        streamState.markInflight(sid);
        // 标记为「回灌接回」：这轮没有 invoke 结果可等，终态要自己取最终
        // 正文。invoke 驱动的轮（切走再切回）不标：收尾权在 runChat。
        if (!streamState.isInvoked(sid)) streamState.markResumed(sid);
        // 在途气泡先挂上（围观执行会话此前没被标过在途，本函数之前的
        // mergeInflight 接不住半截正文；已有流式气泡则不动）——**必须在
        // 回放之前**：回放出来的增量经队列 flush 往气泡上追加，先建快照
        // 再流增量恰好各计一次；先放增量再补快照就把同一段文字算两遍。
        // 快照值在 setState 之外取：函数式更新的 updater 渲染时才执行，
        // 在里面读 meta 会读到回放灌完之后的「未来」值。
        if (sessionIdRef.current === sid) {
          const m = streamState.metaOf(sid);
          const snapshot = m?.text ?? "";
          const startedAt = m?.turnStartedAt ?? Date.now();
          setMessages((prev) => {
            const last = prev[prev.length - 1];
            if (last?.streaming) return prev;
            return [
              ...prev,
              {
                role: "assistant" as const,
                content: snapshot,
                streaming: true,
                turnStatus: "running" as const,
                turnStartedAt: startedAt,
              },
            ];
          });
        }
        const handlers = streamHandlersRef.current;
        for (const ev of r.events) {
          handlers[ev.event]?.(ev.payload);
        }
      } catch (e) {
        handleCommandError(e, "bot_stream_replay", { silent: true });
      } finally {
        streamState.clearResuming(sid);
      }
    },
    [enabled, loadFinalText, sessionIdRef, setMessages],
  );
  const resumeAllInflight = useCallback(async () => {
    try {
      const runs = await streamState.fetchInflightRuns();
      // 各会话的回灌互不依赖（各自的 meta 与游标），并行即可
      await Promise.all(runs.map((r) => resumeInflight(r.sessionId)));
    } catch {
      // 问不到在途列表就当没有（后端老版本没有这个命令时同理）
    }
  }, [resumeInflight]);
  useEffect(() => {
    if (!enabled) return;
    (async () => {
      try {
        let list = await invoke<Session[]>("bot_sessions_load");
        if (!list.length) {
          const s = await invoke<Session>("bot_session_create", {
            title: null,
          });
          list = [s];
        }
        setSessions(list);
        const cur = list[0]; // 按 updated_at 倒序，第一个即最近会话
        setSessionId(cur.id);
        // 镜像同步落（同 openExecSession/switchSession/deleteSession）：
        // 流式事件过滤与过期守卫读的是 ref，不能等渲染后的 effect 补
        sessionIdRef.current = cur.id;
        const rows = await invoke<
          {
            role: string;
            content: string;
            refsJson?: string | null;
            thinking?: string | null;
            toolsJson?: string | null;
          }[]
        >("bot_history_load", { sessionId: cur.id });
        // 历史加载期间用户已切走（如 ⌘K 跨窗口跳会话）：过期历史不得盖掉新会话视图
        if (sessionIdRef.current !== cur.id) return;
        const msgs = mergeInflight(rowsToMsgs(rows ?? [], cur.id), cur.id);
        if (rows?.length || msgs.length) {
          setMessages(msgs);
        }
        // 历史到位后再回灌：回放出来的气泡要接在历史后面（顺序反了会被历史盖掉）
        void resumeInflight(cur.id);
        // 后端在跑的其他会话也接回来（并行回复的半截内容别丢）
        void resumeAllInflight();
      } catch (e) {
        handleCommandError(e, "chat init", { silent: true });
      }
    })();
  }, [enabled, mergeInflight, resumeInflight, resumeAllInflight]);

  return {
    loadFinalText,
    persistHistory,
    resumeInflight,
    rowsToMsgs,
  };
}
