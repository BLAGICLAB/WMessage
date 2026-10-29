/** 聊天面板的「sessionActions」逻辑块（从 ChatPanel 主组件搬出）。
 *
 *  会话切换 / 新建 / 删除，以及发一轮对话。
 *
 *  搬移是逐字的：语句原样保留、顺序不变；对外接口只有 params 与返回值。
 */
import { useCallback, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTauriListen } from "../../lib/useTauriListen";
import { handleCommandError, formatCommandError } from "../../lib/errorHandler";
import type { Msg, ReasoningLevel, Session, TaskRef } from "./types";
import type { ChatDraft } from "./chatDrafts";
import * as streamState from "./streamState";
import * as planStore from "./planStore";
import * as askStore from "./askStore";
import type { ConfirmOptions } from "../../lib/useConfirm";
import type { Dispatch, RefObject, SetStateAction } from "react";

export type ChatSessionActionsParams = {
  addHint: (content: string, actionSessionId?: string | undefined) => void;
  confirm: (messageOrOptions: string | ConfirmOptions) => Promise<boolean>;
  effortBase: ReasoningLevel;
  effortBySession: Map<string, ReasoningLevel>;
  enterChat: (sid: string) => void;
  exitChat: (sid: string) => void;
  mergeInflight: (rows: Msg[], sid: string) => Msg[];
  messagesRef: RefObject<Msg[]>;
  persistHistory: (sid: string, msgs: Msg[]) => Promise<unknown>;
  resumeInflight: (sid: string) => Promise<void>;
  rowsToMsgs: (rows: { role: string; content: string; refsJson?: string | null | undefined; thinking?: string | null | undefined; toolsJson?: string | null | undefined; turnStatus?: string | null | undefined; }[], sid?: string | null | undefined) => Msg[];
  sessionId: string | null;
  sessionIdRef: RefObject<string | null>;
  sessions: Session[];
  setDrafts: Dispatch<SetStateAction<Map<string, ChatDraft>>>;
  setEffortBySession: Dispatch<SetStateAction<Map<string, ReasoningLevel>>>;
  setMessages: Dispatch<SetStateAction<Msg[]>>;
  setSessionId: Dispatch<SetStateAction<string | null>>;
  setSessionUsage: Dispatch<SetStateAction<Record<string, { input: number; output: number; cacheRead: number; cacheCreation: number; lastInput: number; lastCacheRead: number; }>>>;
  setSessions: Dispatch<SetStateAction<Session[]>>;
  setStopNotice: Dispatch<SetStateAction<{ sid: string; n: number; } | null>>;
  stickBottomRef: RefObject<boolean>;
};

export function useChatSessionActions(params: ChatSessionActionsParams) {
  const {
    addHint,
    confirm,
    effortBase,
    effortBySession,
    enterChat,
    exitChat,
    mergeInflight,
    messagesRef,
    persistHistory,
    resumeInflight,
    rowsToMsgs,
    sessionId,
    sessionIdRef,
    sessions,
    setDrafts,
    setEffortBySession,
    setMessages,
    setSessionId,
    setSessionUsage,
    setSessions,
    setStopNotice,
    stickBottomRef,
  } = params;

  const switchSession = useCallback(
    async (sid: string) => {
      // SWITCH-1/PAR-1：busy 期允许实时切换——回复仍按发起会话落库/流式路由（在途集合），
      // 切走不影响它在后台完成
      if (sid === sessionId) {
        return;
      }
      setSessionId(sid);
      // 镜像同步落（同 openExecSession）：守卫在 await 之后读 ref，不能等渲染
      sessionIdRef.current = sid;
      setMessages([]);
      // 停止反馈是上一段会话的事：切走就收起来
      setStopNotice(null);
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
        // 等待期间用户已切到别的会话：丢弃过期响应（后到的慢响应不得盖掉新会话的加载结果）
        if (sessionIdRef.current !== sid) return;
        let msgs = rowsToMsgs(rows, sid);
        // 切回正在回复的会话：接回在途回合（用户消息 + 已流出的正文）——
        // 库里只有上一轮收尾时的内容，不接回来用户会以为消息被吞了
        msgs = mergeInflight(msgs, sid);
        setMessages(msgs);
        // 历史到位后再回灌：后端还跑着的那一轮，按序补齐（顺序反了会被历史盖掉）
        void resumeInflight(sid);
      } catch (e) {
        handleCommandError(e, "bot_history_load", { silent: true });
      }
    },
    [sessionId, mergeInflight, resumeInflight],
  );
  const newSession = async () => {
    // SWITCH-1：busy 期允许新建对话（进行中的回复继续落在原会话）
    try {
      const s = await invoke<Session>("bot_session_create", { title: null });
      setSessions((prev) => [s, ...prev]);
      setSessionId(s.id);
      setMessages([]);
      // DRAFT-1：不再清 input——草稿按会话隔离，新会话天然是空的，
      // 原会话的草稿切回去还在（旧实现的全局清空会误删原会话草稿）
    } catch (e) {
      handleCommandError(e, "bot_session_create", { silent: true });
    }
  };
  const switchSessionRef = useRef<(sid: string) => void>(() => {});
  useEffect(() => {
    switchSessionRef.current = switchSession;
  });
  useTauriListen<{ sessionId?: string }>("chat-focus-session", (payload) => {
    if (payload.sessionId) switchSessionRef.current(payload.sessionId);
  });
  const deleteSession = async (sid: string) => {
    // SWITCH-1：只拦「删除正在回复的会话」（回复落库目标不能被删）；其他会话随便删
    if (streamState.isInflight(sid)) {
      addHint("⏳ 该对话正在回复中，完成后才能删除");
      return;
    }
    const target = sessions.find((s) => s.id === sid);
    if (
      !(await confirm(
        `删除对话「${target?.title ?? "未命名"}」？消息记录一并删除。`,
      ))
    )
      return;
    try {
      await invoke("bot_session_delete", { id: sid });
      const rest = sessions.filter((s) => s.id !== sid);
      setSessions(rest);
      // DRAFT-1：被删会话的草稿/附件/任务引用/推理强度覆盖一并清掉
      // （草稿这份 Map 现在同时装文本/附件/引用，删一条即全清；防积攒、
      // 也防同 id 会话重建后误继承旧草稿）
      setDrafts((prev) => {
        const next = new Map(prev);
        next.delete(sid);
        return next;
      });
      setEffortBySession((prev) => {
        const next = new Map(prev);
        next.delete(sid);
        return next;
      });
      // 水位累计与草稿/附件同族：被删会话的条目一并清掉，防内存积攒
      //（也防同 id 会话重建后误继承旧水位）
      setSessionUsage((prev) => {
        const next = { ...prev };
        delete next[sid];
        return next;
      });
      // 计划挂在会话上：被删会话的计划一并清掉（防同 id 重建后继承旧计划）
      planStore.dropSession(sid);
      // 提问卡同上：挂在会话上，删会话一并清（防同 id 重建后继承旧卡）
      askStore.dropSession(sid);
      if (sid === sessionId) {
        // 删的是当前会话：切到剩余第一个；没有则新建
        if (rest.length) {
          const targetSid = rest[0].id;
          setSessionId(targetSid);
          // 镜像同步落（openExecSession 同款）：await 期间守卫必须读到新会话
          sessionIdRef.current = targetSid;
          const rows = await invoke<
            {
              role: string;
              content: string;
              refsJson?: string | null;
              thinking?: string | null;
              toolsJson?: string | null;
            }[]
          >("bot_history_load", { sessionId: targetSid });
          // 等待期用户已切走：丢弃过期历史，防旧会话消息刷进新视图
          if (sessionIdRef.current !== targetSid) return;
          // 目标会话可能正在回复：接回在途回合，别把在途内容抹掉
          setMessages(mergeInflight(rowsToMsgs(rows, targetSid), targetSid));
        } else {
          // await 前记基线：创建期间用户已切走（如 chat-focus-session）则放弃
          // 抢占视图——新建结果不强行切屏
          const sidBefore = sessionIdRef.current;
          const s = await invoke<Session>("bot_session_create", {
            title: null,
          });
          if (sessionIdRef.current !== sidBefore) return;
          setSessions([s]);
          setSessionId(s.id);
          // 镜像同步落（同 openExecSession）：监听器守卫在渲染前就要读到新会话
          sessionIdRef.current = s.id;
          setMessages([]);
        }
      }
    } catch (e) {
      handleCommandError(e, "bot_session_delete");
    }
  };
  const runChat = async (history: Msg[], renameText?: string) => {
    const sid = sessionId;
    if (!sid || streamState.isInflight(sid)) return;
    enterChat(sid);
    // 新的回合开始：上一轮的停止反馈作废（它说的是上一轮的事）
    setStopNotice(null);
    // 用户主动发消息 → 视口回到底部跟着这一轮（哪怕刚才翻过历史）
    stickBottomRef.current = true;
    setMessages([
      ...history,
      { role: "assistant", content: "", streaming: true, turnStatus: "running", turnStartedAt: Date.now() },
    ]);
    // 本轮在途内容的权威副本（按 sid 隔离，收尾时随消息并入最终气泡）：
    // 用户消息兜住「库尚未落地」的窗口，已流出的正文供切回时回填占位气泡
    const lastMsg = history[history.length - 1];
    streamState.setMeta(sid, {
      text: "",
      userMsg: lastMsg?.role === "user" ? lastMsg : undefined,
    });
    // RE-1 推理强度：生效档位 = 本会话覆盖 ?? 后台默认（具体值传后端，
    // 覆盖本身只存前端内存，不回写 bot-config.json）
    const effort = effortBySession.get(sid) ?? effortBase;
    try {
      // 回合开始即落库：用户消息先进库，之后任何一次「按会话重载历史」
      // 都拿得到它——在途回合不再只活在内存里（切走再切回不丢消息的根因修复）
      await persistHistory(sid, history);
      const full = await invoke<{
        text: string;
        taskRefs?: TaskRef[];
        busy?: boolean;
        traceId?: number | null;
      }>("bot_chat", {
        messages: history.map((m) => ({ role: m.role, content: m.content })),
        sessionId: sid,
        reasoningEffort: effort,
      });
      // 会话锁被占（别的回合在跑）：本轮压根没跑，full.text 是拒答文案。
      // 当成模型答复落库 = 给用户发了一条假答复，还进了下一轮上下文。
      if (full.busy) {
        if (sessionIdRef.current === sid) {
          // 先 reset 摘掉本轮塞的空占位气泡（它带 streaming，活体指示会一直亮），
          // 再补提示——顺序反了提示会被 reset 冲掉
          setMessages(history);
          // 文案要自洽：用户消息已在上一步落库，历史里会留一条孤立用户消息
          addHint("⏳ 模型正忙，本次未生成回复，请稍后重试");
        }
        return;
      }
      // 把流式过程中累积的思考/工具行并入最终消息
      const meta = streamState.metaOf(sid) ?? {};
      const done: Msg[] = [
        ...history,
        {
          role: "assistant",
          content: full.text || "",
          refs: full.taskRefs ?? [],
          // 本回合的 trace 归属：跟着消息走，跳转执行轨迹时用得上
          traceId: full.traceId ?? null,
          thinking: meta.thinking,
          tools: meta.tools,
          fileChanges: meta.fileChanges,
          skillFailure: meta.skillFailure,
          // 终态随消息一起留档：turnStatus 缺失时 LiveIndicator 会退回
          // 「正在思考」文案，历史气泡复用时就会出现假活体指示
          turnStatus: meta.turnStatus ?? "completed",
          turnStartedAt: meta.turnStartedAt,
        },
      ];
      // 持久化始终按 sid 写；UI 只在会话未切换时更新（防旧会话消息渲染进新会话）
      persistHistory(sid, done);
      if (sessionIdRef.current === sid) setMessages(done);
      // 首轮对话后把「新对话」标题改为用户消息前 20 字（仿主流聊天应用）
      const sess = sessions.find((s) => s.id === sid);
      if (sess?.title === "新对话" && renameText) {
        const short = renameText.slice(0, 20);
        invoke("bot_session_rename", { id: sid, title: short })
          .then(() =>
            setSessions((prev) =>
              prev.map((s) => (s.id === sid ? { ...s, title: short } : s)),
            ),
          )
          .catch((e) =>
            handleCommandError(e, "bot_session_rename", { silent: true }),
          );
      }
    } catch (e) {
      /* 失败/中断：保住已经流出来的正文。
       * 整段替换成一行「⚠️ …」= 用户读到一半的回复凭空消失（内容被吞），
       * 而这一轮往往正是最需要留着的部分。已流出的正文照旧，错误接在后面。 */
      const viewMsgs =
        sessionIdRef.current === sid ? messagesRef.current : [];
      // 权威副本是本轮累积的流式正文（切走了也在）；视图只是它的近似
      const partial =
        streamState.metaOf(sid)?.text ||
        viewMsgs[viewMsgs.length - 1]?.content ||
        "";
      const note = `⚠️ ${formatCommandError(e)}`;
      const failed: Msg[] = [
        ...history,
        {
          role: "assistant",
          content: partial ? `${partial}\n\n${note}` : note,
        },
      ];
      persistHistory(sid, failed);
      if (sessionIdRef.current === sid) setMessages(failed);
      // 流式错误已经写进消息气泡了，不重复弹 alert
      handleCommandError(e, "bot_chat", { silent: true });
    } finally {
      // 成功已并入消息、失败已丢弃——两种收尾条目都失效，统一在此清理，
      // 防失败路径的累积 thinking 滞留到下次 runChat 或重启
      streamState.dropSession(sid);
      exitChat(sid);
    }
  };
  const currentSession = sessions.find((s) => s.id === sessionId);
  const currentTitle = currentSession?.title ?? "新对话";
  const stopCurrent = useCallback(() => {
    const sid = sessionIdRef.current ?? "";
    invoke<number>("bot_stop", { sessionId: sessionIdRef.current })
      .then((n) => {
        // 一条都没停到就不报数：点了停止却什么都没发生，报「已停 0 条」
        // 比不报更让人困惑（真停到了才说明停止确实生效了）
        if (n > 0) setStopNotice({ sid, n });
      })
      .catch((e) => handleCommandError(e, "bot_stop", { silent: true }));
  }, []);

  return {
    currentTitle,
    deleteSession,
    newSession,
    runChat,
    stopCurrent,
    switchSession,
  };
}
