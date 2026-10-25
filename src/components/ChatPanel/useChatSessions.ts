/** 聊天面板的「sessions」逻辑块（从 ChatPanel 主组件搬出）。
 *
 *  会话 / 消息 / 草稿：state、派生值与草稿落盘。
 *
 *  搬移是逐字的：语句原样保留、顺序不变；对外接口只有 params 与返回值。
 */
import { useEffect, useRef, useState } from "react";
import type { Msg, Session, TaskRef } from "./types";
import { EMPTY_DRAFT, readChatDrafts, writeChatDrafts, type ChatDraft } from "./chatDrafts";

export function useChatSessions() {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [messages, setMessages] = useState<Msg[]>([]);
  const [drafts, setDrafts] = useState<Map<string, ChatDraft>>(() =>
    readChatDrafts(),
  );
  const draftKey = sessionId ?? "";
  const draft = drafts.get(draftKey) ?? EMPTY_DRAFT;
  const patchDraft = (key: string, updater: (cur: ChatDraft) => ChatDraft) =>
    setDrafts((prev) => {
      const next = new Map(prev);
      next.set(key, updater(prev.get(key) ?? EMPTY_DRAFT));
      return next;
    });
  const input = draft.text;
  const setInput = (v: string) =>
    patchDraft(draftKey, (d) => ({ ...d, text: v }));
  const files = draft.files;
  const setFiles = (v: string[] | ((prev: string[]) => string[])) =>
    patchDraft(draftKey, (d) => ({
      ...d,
      files: typeof v === "function" ? v(d.files) : v,
    }));
  const refs = draft.refs;
  const setRefs = (v: TaskRef[] | ((prev: TaskRef[]) => TaskRef[])) =>
    patchDraft(draftKey, (d) => ({
      ...d,
      refs: typeof v === "function" ? v(d.refs) : v,
    }));
  useEffect(() => {
    writeChatDrafts(drafts);
  }, [drafts]);
  const sessionIdRef = useRef<string | null>(null);
  const streamHandlersRef = useRef<Record<string, (p: unknown) => void>>({});
  const messagesRef = useRef<Msg[]>([]);

  return {
    files,
    input,
    messages,
    messagesRef,
    refs,
    sessionId,
    sessionIdRef,
    sessions,
    setDrafts,
    setFiles,
    setInput,
    setMessages,
    setRefs,
    setSessionId,
    setSessions,
    streamHandlersRef,
  };
}
