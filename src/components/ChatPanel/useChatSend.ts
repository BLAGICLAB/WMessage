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
import { newMsgId } from "../../lib/msgId";
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
  refs: TaskRef[];
  rootRef: RefObject<HTMLDivElement | null>;
  runChat: (
    history: Msg[],
    renameText?: string | undefined,
    opts?: { optimistic?: boolean },
  ) => Promise<void>;
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
  stopCurrent: () => Promise<number>;
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
      // PAR-1：/stop 停「当前视图会话」的在途回复（并行回复按会话隔离）。
      // 前端 streamState 只跟踪主聊流式，任务卡执行等后端在途（exec_traces
      // running）不在其中——预判会把 exec 会话的 /stop 挡成「没有进行中的
      // 回复」；裁决统一交给后端 bot_stop（置位数 0 = 本会话无可停实例）。
      void stopCurrent().then((n) => {
        if (n === 0) addHint("当前对话没有进行中的回复");
      });
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
      // 摘要范围取当前历史行的身份（真源化：落库归后端，前端只报范围）
      const msgIds = history.map((m) => m.id).filter((v): v is string => !!v);
      if (msgIds.length < 2) {
        addHint("消息还没有身份记录（旧数据），先刷新对话再压缩");
        exitChat(sid);
        return true;
      }
      // 进度占位：压缩请求期间有可见反馈（否则界面像卡死）
      if (sessionIdRef.current === sid) {
        setMessages((prev) => [
          ...prev,
          { role: "assistant", content: "📦 压缩上下文中…", streaming: true },
        ]);
      }
      try {
        // 后端读库总结（旧摘要行也参与——幂等再压缩）
        const summary = await invoke<string>("bot_compact", {
          sessionId: sid,
        });
        const label = `📦 上下文已压缩（原 ${msgIds.length} 条消息）：\n${summary}`;
        // 摘要行换血（真源侧操作）：指定行换成一条摘要行，事件回显对账
        const summaryRow = await invoke<{ msgId: string | null }>(
          "bot_history_replace_summary",
          { sessionId: sid, msgIds, content: label },
        );
        const compacted: Msg = {
          id: summaryRow.msgId ?? undefined,
          role: "assistant",
          content: label,
        };
        if (sessionIdRef.current === sid) setMessages([compacted]);
      } catch (e) {
        // 压缩失败不动库（原历史完好），只提示
        const note: Msg = {
          role: "assistant",
          content: `⚠️ 压缩失败：${formatCommandError(e)}`,
        };
        if (sessionIdRef.current === sid) setMessages([note]);
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
      const target = msgs[lastUserIdx];
      // 真源化 retry：先截尾（库里该用户行之后的失败回复等残行必须清掉，
      // 否则后端重建的上下文会把它们摆回模型面前），再重放同一用户行
      // （同 id upsert 命中旧行，保位保首插时刻）
      if (!target.id) {
        addHint("该消息还没有身份记录（旧数据），先刷新对话再重试");
        return true;
      }
      try {
        await invoke("bot_history_drop_after", {
          sessionId,
          msgId: target.id,
        });
      } catch (e) {
        // 截尾失败就起轮 = 带残行上下文重跑，宁可不做
        handleCommandError(e, "bot_history_drop_after");
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
      // 乐观用户行出生即有身份：id 随 bot_chat 传后端落库（client_msg_id），
      // bot-msg-upserted 事件回显同 id 对账
      { id: newMsgId(), role: "user", content, refs },
    ];
    await runChat(history, text, { optimistic: true });
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
    const target = messagesRef.current[idx];
    // 真源化：删行走真源侧操作（bot_history_drop），本视图即时摘除
    if (!target?.id) {
      handleCommandError("该消息还没有身份记录（旧数据），先刷新对话再移除", "removeMessage");
      return;
    }
    const next = messagesRef.current.filter((_, j) => j !== idx);
    setMessages(next);
    invoke("bot_history_drop", { sessionId: sid, msgId: target.id }).catch(
      (e) => handleCommandError(e, "bot_history_drop"),
    );
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
