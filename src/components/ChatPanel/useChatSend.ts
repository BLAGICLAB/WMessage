/** 聊天面板的「send」逻辑块（从 ChatPanel 主组件搬出）。
 *
 *  斜杠命令、发送、附件挑选与消息级操作。
 *
 *  搬移是逐字的：语句原样保留、顺序不变；对外接口只有 params 与返回值。
 */
import { useCallback, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { unlistenSafe } from "../../lib/useTauriListen";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { handleCommandError, formatCommandError } from "../../lib/errorHandler";
import type { Msg, ReasoningLevel, TaskRef } from "./types";
import * as streamState from "./streamState";
import type { Dispatch, RefObject, SetStateAction } from "react";

export type ChatSendParams = {
  addHint: (content: string, actionSessionId?: string | undefined) => void;
  effortBase: ReasoningLevel;
  effortBySession: Map<string, ReasoningLevel>;
  enterChat: (sid: string) => void;
  execWatchRef: RefObject<string | null>;
  exitChat: (sid: string) => void;
  files: string[];
  input: string;
  messages: Msg[];
  messagesRef: RefObject<Msg[]>;
  persistHistory: (sid: string, msgs: Msg[]) => Promise<unknown>;
  refs: TaskRef[];
  rootRef: RefObject<HTMLDivElement | null>;
  runChat: (history: Msg[], renameText?: string | undefined) => Promise<void>;
  sessionId: string | null;
  sessionIdRef: RefObject<string | null>;
  setCopiedIdx: Dispatch<SetStateAction<number | null>>;
  setDragHover: Dispatch<SetStateAction<boolean>>;
  setEffortBySession: Dispatch<SetStateAction<Map<string, ReasoningLevel>>>;
  setEffortMenuOpen: Dispatch<SetStateAction<boolean>>;
  setFiles: (v: string[] | ((prev: string[]) => string[])) => void;
  setInput: (v: string) => void;
  setMessages: Dispatch<SetStateAction<Msg[]>>;
  setOpenTrace: Dispatch<SetStateAction<{ traceId: number; sessionId: string | null; } | null>>;
  setRefs: (v: TaskRef[] | ((prev: TaskRef[]) => TaskRef[])) => void;
  stopCurrent: () => void;
};

export function useChatSend(params: ChatSendParams) {
  const {
    addHint,
    effortBase,
    effortBySession,
    enterChat,
    execWatchRef,
    exitChat,
    files,
    input,
    messages,
    messagesRef,
    persistHistory,
    refs,
    rootRef,
    runChat,
    sessionId,
    sessionIdRef,
    setCopiedIdx,
    setDragHover,
    setEffortBySession,
    setEffortMenuOpen,
    setFiles,
    setInput,
    setMessages,
    setOpenTrace,
    setRefs,
    stopCurrent,
  } = params;

  const runSlashCommand = async (raw: string): Promise<boolean> => {
    const cmd = raw.split(/\s+/)[0].toLowerCase();
    if (cmd === "/stop") {
      setInput("");
      // PAR-1：/stop 停「当前视图会话」的在途回复（并行回复按会话隔离）
      if (streamState.isInflight(sessionIdRef.current ?? "")) {
        stopCurrent();
      } else {
        addHint("当前对话没有进行中的回复");
      }
      return true;
    }
    if (cmd === "/clean") {
      setInput("");
      if (!sessionId) {
        addHint("当前没有活动对话");
        return true;
      }
      // 清空当前 session 消息 + 持久化
      setMessages([]);
      invoke("bot_history_clear", { sessionId }).catch((e) =>
        handleCommandError(e, "bot_history_clear", { silent: true }),
      );
      return true;
    }
    if (cmd === "/compact") {
      setInput("");
      if (streamState.isInflight(sessionIdRef.current ?? "") || !sessionId)
        return true;
      const sid = sessionId;
      const history = messages.filter(
        (m) => !m.streaming && !m.hint && m.content.trim(),
      );
      if (history.length < 2) {
        addHint("消息太少，暂无需压缩");
        return true;
      }
      enterChat(sid);
      // 进度占位：压缩请求期间有可见反馈（否则界面像卡死）
      if (sessionIdRef.current === sid) {
        setMessages((prev) => [
          ...prev,
          { role: "assistant", content: "📦 压缩上下文中…", streaming: true },
        ]);
      }
      try {
        const summary = await invoke<string>("bot_compact", {
          messages: history.map((m) => ({ role: m.role, content: m.content })),
        });
        const compacted: Msg[] = [
          {
            role: "assistant",
            content: `📦 上下文已压缩（原 ${history.length} 条消息）：\n${summary}`,
          },
        ];
        persistHistory(sid, compacted);
        if (sessionIdRef.current === sid) setMessages(compacted);
      } catch (e) {
        const note: Msg = {
          role: "assistant",
          content: `⚠️ 压缩失败：${formatCommandError(e)}`,
        };
        const failed = [...history, note];
        persistHistory(sid, failed);
        if (sessionIdRef.current === sid) setMessages(failed);
        handleCommandError(e, "bot_compact", { silent: true });
      } finally {
        exitChat(sid);
      }
      return true;
    }
    if (cmd === "/retry") {
      setInput("");
      if (streamState.isInflight(sessionIdRef.current ?? "") || !sessionId)
        return true;
      const msgs = messages.filter((m) => !m.streaming && !m.hint);
      // 找到最后一条用户消息，砍掉它之后的所有内容，重新生成回复
      let lastUserIdx = -1;
      for (let i = msgs.length - 1; i >= 0; i--) {
        if (msgs[i].role === "user") {
          lastUserIdx = i;
          break;
        }
      }
      if (lastUserIdx === -1) {
        addHint("没有可重试的消息");
        return true;
      }
      await runChat(msgs.slice(0, lastUserIdx + 1));
      return true;
    }
    return false;
  };
  const send = async () => {
    const text = input.trim();
    // 围观执行会话期拦发送——含 /retry /clean /compact 等斜杠命令（runChat 同样
    // 会往执行中的会话发消息，ChatGuard 只软拒不防交错）；/stop 例外：停执行是
    // 围观期的合法操作
    if (
      execWatchRef.current !== null &&
      execWatchRef.current === sessionId &&
      text !== "/stop"
    ) {
      addHint("⏳ 执行进行中，围观模式暂不能发送");
      return;
    }
    // 斜杠命令优先处理（/stop 在 busy 时也能生效）；未知斜杠文本当普通消息发
    if (text.startsWith("/")) {
      if (await runSlashCommand(text)) return;
    }
    if (!text && !files.length && !refs.length) return;
    // PAR-1：并行回复按会话隔离——本会话在途时拦重复发送（其他会话可自由发送）
    if (sessionId !== null && streamState.isInflight(sessionId)) {
      addHint(
        "⏳ 本对话正在回复中：可切换到其他对话发送，或输入 /stop 停止本条",
      );
      return;
    }
    // 同步检查：state 重渲染前连续两次 Enter 也能拦下重复发送
    if (!sessionId) return;
    setInput("");
    // 附件以 [附件文件] 块附在消息前（模型用 extract_document 的 path 直读，不弹框）
    const attach = files.length
      ? `[附件文件]\n${files.map((f) => `- ${f}`).join("\n")}\n\n`
      : "";
    // 任务引用同理：[已选任务] 块给机器人标题线索，refs 让气泡渲染成可点的卡引用
    // 标记与行格式必须与 intent_router::parse_selected_tasks_block 严格对齐：
    // 少 id 或标记对不上，批量执行路由就静默不触发，模型只能按标题模糊匹配找卡
    const refBlock = refs.length
      ? `[已选任务]\n${refs.map((r) => `- id=${r.id}，标题=${r.title}`).join("\n")}\n\n`
      : "";
    const content = `${attach}${refBlock}${text}`;
    setFiles([]);
    setRefs([]);
    const history: Msg[] = [
      ...messages.filter((m) => !m.streaming && !m.hint),
      { role: "user", content, refs },
    ];
    await runChat(history, text);
  };
  const addFiles = (paths: string[]) => {
    const clean = paths.filter(Boolean);
    if (clean.length) {
      setFiles((prev) => [...prev, ...clean.filter((p) => !prev.includes(p))]);
    }
  };
  const addFilesRef = useRef(addFiles);
  addFilesRef.current = addFiles;
  const pickFiles = async () => {
    try {
      const picked = await invoke<string[]>("pick_files_dialog");
      if (picked.length) addFiles(picked);
    } catch (e) {
      handleCommandError(e, "pick_files_dialog", { silent: true });
    }
  };
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    const insideChat = async (pos: { x: number; y: number }) => {
      const rect = rootRef.current?.getBoundingClientRect();
      if (!rect) return false;
      const scale = await getCurrentWindow()
        .scaleFactor()
        .catch(() => 1);
      const x = pos.x / scale;
      const y = pos.y / scale;
      return (
        x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom
      );
    };
    try {
      getCurrentWindow()
        .onDragDropEvent(async (e) => {
          const p = e.payload;
          if (p.type === "leave") {
            setDragHover(false);
            return;
          }
          if (p.type === "enter" || p.type === "over") {
            setDragHover(await insideChat(p.position));
            return;
          }
          if (p.type === "drop") {
            setDragHover(false);
            if (await insideChat(p.position)) addFilesRef.current(p.paths);
          }
        })
        .then((f) => {
          // 卸载早于 resolve 时自注销，防监听泄漏（同 useTauriListen 的 cancelled 模式）
          if (cancelled) unlistenSafe(f);
          else unlisten = f;
        })
        // 非 Tauri 环境（vitest/jsdom）没有窗口对象，静默忽略
        .catch(() => {});
    } catch {
      // 同上：同步抛错（无 __TAURI_INTERNALS__）也静默忽略
    }
    return () => {
      cancelled = true;
      if (unlisten) unlistenSafe(unlisten);
    };
  }, []);
  const copyMessage = useCallback(async (idx: number, content: string) => {
    try {
      await navigator.clipboard.writeText(content);
      setCopiedIdx(idx);
      setTimeout(() => setCopiedIdx((c) => (c === idx ? null : c)), 1500);
    } catch (e) {
      handleCommandError(e, "clipboard", { silent: true });
    }
  }, []);
  useEffect(() => {
    messagesRef.current = messages;
  }, [messages]);
  const removeMessage = useCallback((idx: number) => {
    const sid = sessionIdRef.current;
    if (!sid || streamState.isInflight(sid)) return;
    const next = messagesRef.current.filter((_, j) => j !== idx);
    setMessages(next);
    persistHistory(sid, next);
  }, []);
  const openTaskInMain = useCallback(async (ref: TaskRef) => {
    emit("edit-task", { id: ref.id }).catch(() => {});
    invoke("focus_main_window").catch(() => {});
  }, []);
  const openTraceFor = useCallback((traceId: number) => {
    setOpenTrace({ traceId, sessionId: sessionIdRef.current });
  }, []);
  const effectiveEffort: ReasoningLevel =
    (sessionId != null ? effortBySession.get(sessionId) : undefined) ??
    effortBase;
  const effortIsOverridden =
    sessionId != null && effortBySession.has(sessionId);
  const setEffortForSession = (level: ReasoningLevel | null) => {
    if (!sessionId) return;
    setEffortBySession((prev) => {
      const next = new Map(prev);
      if (level == null) next.delete(sessionId);
      else next.set(sessionId, level);
      return next;
    });
    setEffortMenuOpen(false);
  };

  return {
    copyMessage,
    effectiveEffort,
    effortIsOverridden,
    openTaskInMain,
    openTraceFor,
    pickFiles,
    removeMessage,
    send,
    setEffortForSession,
  };
}
