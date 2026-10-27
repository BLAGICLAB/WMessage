// ChatPanel 主组件（orchestrator）。
// 1862 → 拆分：SessionList / MessageList / InputArea / useChatUi 到同目录子文件
// 行为等价拆分——消息数据流/斜杠命令语义/Tauri 命令调用零改动），
// 本文件保留所有 useState / useRef / useEffect / handlers / 数据流监听 / 装配 JSX。
// MsgBubble 的 React.memo 流式性能边界见 MessageList.tsx 头注释。
//
// 公开 import 路径保持稳定：外部仍 `import { ChatPanel } from "./components/ChatPanel"`，
// Vite 解析到 `./ChatPanel/index.tsx` → 透传 `./ChatPanel`。

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, emit } from "@tauri-apps/api/event";
import { PanelLeftClose, PanelLeftOpen, PanelRightClose, PanelRightOpen, ListTodo } from "lucide-react";
import { unlistenSafe, useTauriListen } from "../../lib/useTauriListen";
import { getBotConfig } from "../../lib/botConfig";
import type { Task } from "../../types";
import { useConfirm } from "../../lib/useConfirm";
import { handleCommandError, formatCommandError, isCommandError } from "../../lib/errorHandler";
import type { ChatModelEntry, Msg, PermMode, Session, SubagentRow, VerboseLevel } from "./types";
import { execTaskDedup, finishedExecTasks, SUBAGENT_STATUS } from "./constants";
import { useAutoGrow, useOutsideClose } from "./useChatUi";
import * as streamState from "./streamState";
import { SessionRail } from "./SessionRail";
import { ContextRail } from "./ContextRail";
import { MessageList } from "./MessageList";
import { PlanPanel } from "./PlanPanel";
import { UsageMeter } from "./UsageMeter";
import { InputArea } from "./InputArea";
import { STICK_BOTTOM_PX } from "./constants";
import { TracePanel } from "../TracePanel";
import { useChatSessions } from "./useChatSessions";
import { useChatTrace } from "./useChatTrace";
import { useModelPicker } from "./useModelPicker";
import { useEffortMenu } from "./useEffortMenu";
import { usePermMenu } from "./usePermMenu";
import { useChatRail } from "./useChatRail";
import { useChatSubagents } from "./useChatSubagents";
import { useChatHistory } from "./useChatHistory";
import { useChatStream } from "./useChatStream";
import { useChatSessionActions } from "./useChatSessionActions";
import { useChatSend } from "./useChatSend";

/** 模型下拉条目集合协议：types.ModelItem 的来源协议收窄用 */
type Props = {
  /** 机器人是否开启（主窗口读 bot_get_enabled + bot-changed 广播）。   *  false 时跳过会话加载/历史恢复与所有对外 invoke，容器仍挂载——主窗口里
   *  聊天页与其它页面靠 hidden 切换可见性，卸载会连带丢掉 messages 与全部监听器 */
  enabled: boolean;
  /** 右侧上下文栏的任务池（App 侧已过滤归档/删除/他人任务） */
  tasks: Task[];
  /** 勾完成等任务改写（走 App 的 updateTask，后端广播 tasks-updated） */
  onUpdateTask: (taskId: string, patch: Partial<Task>) => void;
  /** 更上层浮层开着（命令面板等）：Esc 让位给它，计划面板不抢这个键 */
  escBlocked?: boolean;
};

/** 机器人聊天区：主窗口左导航「Agent 能力 → 聊天」整页；支持多会话 */
export function ChatPanel({ enabled, tasks, onUpdateTask, escBlocked }: Props) {
  const {
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
  } = useChatSessions();
  const taRef = useRef<HTMLTextAreaElement>(null);
  const confirm = useConfirm();
  const [slashIdx, setSlashIdx] = useState(0);
  const [slashDismissed, setSlashDismissed] = useState(false);
  const [inflightSids, setInflightSids] = useState<ReadonlySet<string>>(
    () => new Set(streamState.inflightSessions()),
  );
  useEffect(
    () => streamState.subscribeInflight((sids) => setInflightSids(new Set(sids))),
    [],
  );
  const {
    copiedIdx,
    openTrace,
    refTask,
    setCopiedIdx,
    setOpenTrace,
    setStopNotice,
    stopNotice,
  } = useChatTrace({
    setRefs,
  });
  const [dragHover, setDragHover] = useState(false);
  /** 执行计划浮层开合（右上角按钮切换；浮层不占右侧栏位置） */
  const [planOpen, setPlanOpen] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const stickBottomRef = useRef(true);
  const lastScrollTopRef = useRef(0);
  const lastSessionRef = useRef<string | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const {
    activeModelId,
    apiProvider,
    modelBtnRef,
    modelDropdownRef,
    modelLabel,
    modelMenuOpen,
    models,
    setActiveModelId,
    setApiProvider,
    setModelLabel,
    setModelMenuOpen,
    setModels,
  } = useModelPicker();
  const {
    effortBase,
    effortBtnRef,
    effortBySession,
    effortDropdownRef,
    effortMenuOpen,
    setEffortBase,
    setEffortBySession,
    setEffortMenuOpen,
  } = useEffortMenu();
  const {
    permBtnRef,
    permDropdownRef,
    permMenuOpen,
    permMode,
    permModeRef,
    selectPermMode,
    setPermMenuOpen,
    setPermMode,
  } = usePermMenu();
  const [sessionUsage, setSessionUsage] = useState<
    Record<
      string,
      {
        input: number;
        output: number;
        cacheRead: number;
        cacheCreation: number;
        lastInput: number;
        lastCacheRead: number;
      }
    >
  >({});
  const [verboseLevel, setVerboseLevelState] = useState<VerboseLevel>(() => {
    const v = localStorage.getItem("chat-verbose-level");
    return v === "concise" || v === "debug" ? v : "detailed";
  });
  const setVerboseLevel = (l: VerboseLevel) => {
    setVerboseLevelState(l);
    localStorage.setItem("chat-verbose-level", l);
  };
  const {
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
  } = useChatRail();
  /**
   * 预览请求：打开哪个文件、看哪一面。
   *
   * 看哪一面由**点击来源**决定，不由「这个路径在不在改动清单里」决定——
   * 交付物也可能是本轮写出来的（配套说明文档就是），按在不在清单里判会把
   * 交付物打开成 diff，用户要看的是内容。
   *
   * 用一个对象而不是两个字段：每次打开都是新对象，栏内据此重置「看内容/
   * 看改动」；用两个字段时，同一个文件从不同入口再点一次就分不清是哪次
   * 请求的意图了。
   */
  const [previewReq, setPreviewReq] = useState<{
    path: string;
    mode: "content" | "diff";
  } | null>(null);
  const openFilePreview = useCallback((path: string) => {
    setCtxOpen(true);
    localStorage.setItem("chat-ctx-open", "1");
    setCtxTab("files");
    // 落点是交付物页：点进来的是交付物，清单就在这一页
    setFileSubTab("deliverables");
    setPreviewReq({ path, mode: "content" });
  }, []);
  /** 去右侧栏看本轮改动过的文件（交付物区空态提示的跳转） */
  const openFileChanges = useCallback(() => {
    setCtxOpen(true);
    localStorage.setItem("chat-ctx-open", "1");
    setCtxTab("files");
    // 落点是改动页：那里才是改动文件的完整清单
    setFileSubTab("changes");
  }, []);
  useAutoGrow(taRef, input);
  useEffect(() => {
    // 挂载式 mount-refresh：复位瞬时 UI 态（可改写为派生但会换执行时机，拆分批保行为）
    // oxlint-disable-next-line react/set-state-in-effect
    setSlashIdx(0);
    // oxlint-disable-next-line react/set-state-in-effect
    setSlashDismissed(false);
    // 子 agent 详情是按会话归属的：不复位的话，切到另一个会话侧栏还钉着
    // 上一世的条目，点「返回」才看得出列表已经换人了
    // oxlint-disable-next-line react/set-state-in-effect
    setSubagentDetailId(null);
  }, [sessionId]);
  const metaFor = (sid: string | null) => {
    if (!sid) return undefined;
    return streamState.ensureMeta(sid);
  };
  const mergeInflight = useCallback((rows: Msg[], sid: string): Msg[] => {
    if (!streamState.isInflight(sid)) return rows;
    const meta = streamState.metaOf(sid);
    const userMsg = meta?.userMsg;
    const last = rows[rows.length - 1];
    // 库里已经有这条用户消息（回合开始即落库）就不再补，避免重复一条
    const inDb =
      !!userMsg && last?.role === "user" && last.content === userMsg.content;
    const base = userMsg && !inDb ? [...rows, userMsg] : rows;
    return [
      ...base,
      {
        role: "assistant",
        content: meta?.text ?? "",
        streaming: true,
        turnStatus: "running",
        turnStartedAt: meta?.turnStartedAt ?? Date.now(),
      },
    ];
  }, []);
  const pendingExecRef = useRef<{ sid: string; title: string } | null>(null);
  const execSessionByTaskRef = useRef<Map<string, string>>(new Map());
  const enterChat = (sid: string) => {
    streamState.markInflight(sid);
  };
  const exitChat = (sid: string) => {
    streamState.markFinished(sid);
    // 全部会话回复结束 → 兑现排队中的执行会话跳转提示（不打断刚结束的对话）
    if (streamState.inflightSessions().size === 0) {
      const pending = pendingExecRef.current;
      if (pending) {
        pendingExecRef.current = null;
        addHint(
          `${pending.title || "任务"}已在新会话执行，点击查看执行对话`,
          pending.sid,
        );
      }
    }
  };
  const viewedBusy = sessionId !== null && inflightSids.has(sessionId);
  const {
    cancelSubagent,
    refreshSubagentsRef,
    subagentRuns,
    subagentSlot,
  } = useChatSubagents({
    enabled,
    openSubagentDetail,
    sessionId,
    sessionIdRef,
    setSubagentStripOpen,
    subagentStripOpen,
  });
  const addHint = (content: string, actionSessionId?: string) => {
    setMessages((prev) => [
      ...prev,
      { role: "assistant", content, actionSessionId, hint: true },
    ]);
  };
  useEffect(() => {
    sessionIdRef.current = sessionId;
  }, [sessionId]);
  useTauriListen<{
    subagentId: string;
    parentSessionId: string | null;
    status: SubagentRow["status"];
    summary: string;
  }>("subagent-finished", (payload) => {
    // 有派发方主会话时不打这条提示：后端会在那一轮空闲时自己起一轮汇报，
    // 同一件事说两遍比不说更糟。没有任何派发方（后台/定时派发）时才靠提示告知。
    if (!payload.parentSessionId) {
      const label = SUBAGENT_STATUS[payload.status]?.label ?? payload.status;
      addHint(`子任务${label}：${payload.summary || payload.subagentId}`);
    }
    // 子 agent 行只有这里可见（不再有任务卡兜底）：不重拉就一直停在
    // running/queued——派发方会话里既看不到刚跑完的，也看不到刚派出的
    refreshSubagentsRef.current();
  });
  useTauriListen<{ sessionId: string; text: string }>("bot-chat-report", (payload) => {
    if (payload.sessionId !== sessionIdRef.current) return;
    setMessages((prev) => {
      const last = prev[prev.length - 1];
      if (last?.streaming) {
        return [
          ...prev.slice(0, -1),
          { ...last, content: payload.text, streaming: false, turnStatus: "completed" as const },
        ];
      }
      return [...prev, { role: "assistant" as const, content: payload.text, turnStatus: "completed" as const }];
    });
  });
  useTauriListen<{ subagentId: string }>("subagent-spawned", () => {
    refreshSubagentsRef.current();
  });
  const {
    loadFinalText,
    persistHistory,
    resumeInflight,
    rowsToMsgs,
  } = useChatHistory({
    enabled,
    mergeInflight,
    sessionIdRef,
    setMessages,
    setSessionId,
    setSessions,
    streamHandlersRef,
  });
  useOutsideClose(
    modelMenuOpen,
    () => setModelMenuOpen(false),
    modelBtnRef,
    modelDropdownRef,
  );
  useOutsideClose(
    effortMenuOpen,
    () => setEffortMenuOpen(false),
    effortBtnRef,
    effortDropdownRef,
  );
  useOutsideClose(
    permMenuOpen,
    () => setPermMenuOpen(false),
    permBtnRef,
    permDropdownRef,
  );
  useEffect(() => {
    const reload = () =>
      getBotConfig()
        .then((c) => {
          setModelLabel(c.model || "未配置");
          const isOpenai = (c.apiProvider ?? "openai") !== "anthropic";
          setApiProvider(isOpenai ? "openai" : "anthropic");
          const mbp = c.modelsByProvider ?? {};
          // 过滤：厂商总开关命中的厂商、启用开关关闭的模型、未通过连接测试的厂商
          // 都不进下拉；无 vendor 的老条目/自定义条目没有厂商验证概念，不受影响
          const disabled = new Set(c.disabledVendors ?? []);
          const verified = new Set(c.verifiedVendors ?? []);
          const visible = (m: ChatModelEntry) =>
            m.enabled !== false &&
            (!m.vendor || (!disabled.has(m.vendor) && verified.has(m.vendor)));
          // 双协议同列——两组模型都列出（带协议分组标签），
          // 跨协议选中由后端 apply_active_model_switch 连协议一起切
          setModels([
            ...(mbp.openai ?? [])
              .filter(visible)
              .map((m) => ({ ...m, provider: "openai" as const })),
            ...(mbp.anthropic ?? []).filter(visible).map((m) => ({
              ...m,
              provider: "anthropic" as const,
            })),
          ]);
          setActiveModelId(
            (c.activeModelId ?? { openai: null, anthropic: null })[
              isOpenai ? "openai" : "anthropic"
            ] ?? null,
          );
          // RE-1 推理强度后台默认：缺字段/非法值回 medium（与后端 from_cfg 一致）。
          // 只刷新基线——已选过覆盖的会话保持覆盖（验收 3），没覆盖过的即时跟随新默认
          const raw = c.reasoningEffort;
          setEffortBase(
            raw === "off" || raw === "low" || raw === "high" ? raw : "medium",
          );
          // 授权模式：非法/缺省回 ask（与后端 PermMode::from_cfg 一致）
          const pm = c.permMode;
          const next: PermMode =
            pm === "strict" || pm === "yolo" || pm === "auto" ? pm : "ask";
          permModeRef.current = next;
          setPermMode(next);
        })
        .catch(() => setModelLabel("未配置"));
    reload();
    const un = listen("bot-config-changed", reload);
    return () => {
      unlistenSafe(un);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const switchModel = async (id: string) => {
    try {
      const c = await invoke<{
        model?: string;
        apiProvider?: string | null;
        activeModelId?: {
          openai: string | null;
          anthropic: string | null;
        } | null;
      }>("bot_set_active_model", { modelId: id });
      setModelLabel(c.model || "未配置");
      // 跨协议选中时后端已连协议一起切，这里同步本地协议态
      const isOpenai = (c.apiProvider ?? "openai") !== "anthropic";
      setApiProvider(isOpenai ? "openai" : "anthropic");
      setActiveModelId(
        (c.activeModelId ?? { openai: null, anthropic: null })[
          isOpenai ? "openai" : "anthropic"
        ] ?? null,
      );
      setModelMenuOpen(false);
      emit("bot-config-changed", null).catch(() => {});
    } catch (e) {
      handleCommandError(e, "切换模型");
    }
  };
  useChatStream({
    exitChat,
    loadFinalText,
    metaFor,
    sessionIdRef,
    setMessages,
    setSessionUsage,
    streamHandlersRef,
  });
  const execWatchRef = useRef<string | null>(null);
  const openExecSession = async (sid: string, watch = true) => {
    setSessionId(sid);
    // 镜像同步落（effect 渲染后才补）：await 期间守卫读到的必须已是新会话，
    // 否则快响应会被「过期」守卫误丢
    sessionIdRef.current = sid;
    streamState.setMeta(sid, {});
    // 围观守卫：记录当前围观的执行会话——执行期间拦 Send
    //（防用户输入与执行响应交错 + 被收尾 history_load 冲掉）；切换会话自由。
    // watch=false 的迟到查看不挂守卫：执行已结束，没有需要隔离的响应流
    if (watch) execWatchRef.current = sid;
    try {
      const rows = await invoke<Parameters<typeof rowsToMsgs>[0]>(
        "bot_history_load",
        { sessionId: sid },
      );
      // 围观期间用户已切走：丢弃过期历史，防旧会话消息刷进当前视图
      if (sessionIdRef.current !== sid) return;
      setMessages(mergeInflight(rowsToMsgs(rows, sid), sid));
      // 历史到位后再回灌（顺序反了会被历史盖掉）
      void resumeInflight(sid);
    } catch (e) {
      handleCommandError(e, "bot_history_load", { silent: true });
    }
  };
  const openExecSessionRef = useRef<(sid: string, watch?: boolean) => void>(
    () => {},
  );
  openExecSessionRef.current = (sid, watch) => {
    openExecSession(sid, watch);
  };
  useEffect(() => {
    // 去重表在模块级 execTaskDedup（泄漏的监听器实例间共享才有效，见文件头注释）
    const unExec = listen<{ id?: string; title?: string }>(
      "execute-task",
      (e) => {
        const { id } = e.payload ?? {};
        if (!id) return;
        if (execTaskDedup.shouldSkip(id, Date.now())) return;
        invoke("bot_execute_task", { taskId: id, sessionId: null })
          .then(() => {
            // 收尾登记（迟到 chat-open-session 防复活用）
            finishedExecTasks.set(id, "success");
            // 执行收尾：刷新会话列表（新会话入列）；若正围观该执行会话，
            // 重载历史替换流式占位气泡为最终落库内容
            const sid = execSessionByTaskRef.current.get(id);
            execSessionByTaskRef.current.delete(id);
            invoke<Session[]>("bot_sessions_load")
              .then(setSessions)
              .catch(() => {});
            // 收尾 history_load 无条件发起：围观守卫的解除必须等它完成（早清会
            // 重新打开「输入被最终历史冲掉」的窗）；UI 更新仅当用户仍在该会话
            const load = sid
              ? invoke<Parameters<typeof rowsToMsgs>[0]>("bot_history_load", {
                  sessionId: sid,
                })
              : null;
            if (load) {
              load
                .then((rows) => {
                  if (sessionIdRef.current === sid)
                    setMessages(rowsToMsgs(rows, sid ?? undefined));
                })
                .catch(() => {});
              // 独立订阅清守卫：先 catch 再 finally，避免 finally 链 unhandled rejection
              load
                .catch(() => {})
                .finally(() => {
                  // 围观守卫解除——不论用户当前是否仍在该会话（切换后回来不得被永久拦截）
                  if (execWatchRef.current === sid) execWatchRef.current = null;
                });
            }
          })
          .catch((err) => {
            // 收尾登记（含失败/业务拒）：失败任务的迟到跳转不自动切，停下让用户决定
            finishedExecTasks.set(id, "failed");
            // TASK_INVALID_STATE（执行中重复触发/已完成/已归档）按业务状态提示而非错误
            addHint(
              `${isCommandError(err) && err.code === "TASK_INVALID_STATE" ? "⏳" : "⚠️"} ${formatCommandError(err)}`,
            );
            // 执行失败同样解除围观守卫（chat-open-session 已置值、.then 不会跑，
            // 不清则该会话被永久拦 Send）
            const sid = execSessionByTaskRef.current.get(id);
            execSessionByTaskRef.current.delete(id);
            if (sid && execWatchRef.current === sid)
              execWatchRef.current = null;
          });
      },
    );
    return () => {
      unlistenSafe(unExec);
    };
  }, []);
  useEffect(() => {
    const un = listen<{
      sessionId?: string;
      taskId?: string;
      title?: string;
      origin?: string;
    }>("chat-open-session", (e) => {
      const { sessionId: sid, taskId, title } = e.payload ?? {};
      if (!sid) return;
      if (taskId) execSessionByTaskRef.current.set(taskId, sid);
      // 新会话入列（后端已建库，前端列表补一行即可，不必整表重载）
      setSessions((prev) =>
        prev.some((s) => s.id === sid)
          ? prev
          : [{ id: sid, title: title ?? "执行" }, ...prev],
      );
      // 迟到防复活：execute-task 已收尾的任务不再以围观模式重开（重挂守卫会把
      // 已结束的会话永久拦输入）。成功 → 切过去只读查看；失败 → 不自动
      // 跳转，停下来让用户决定是否重试/查看（从会话列表点入不受拦）
      const outcome = taskId ? finishedExecTasks.get(taskId) : undefined;
      if (outcome) {
        if (outcome === "success") openExecSessionRef.current(sid, false);
        return;
      }
      if (streamState.isInflight(sessionIdRef.current ?? "")) {
        pendingExecRef.current = { sid, title: title ?? "" };
        return;
      }
      openExecSessionRef.current(sid);
    });
    return () => {
      unlistenSafe(un);
    };
  }, []);
  useLayoutEffect(() => {
    const el = scrollRef.current;
    // 可选链只挡 ref 为空，挡不住方法缺失——缺 scrollTo 的环境里这行会抛，
    // 抛在 effect 里就变成随机某个用例的红
    if (!el || typeof el.scrollTo !== "function") return;
    // 换会话 = 换一段对话：上一段的滚动位置不跟着带过来
    if (lastSessionRef.current !== sessionId) {
      lastSessionRef.current = sessionId;
      stickBottomRef.current = true;
    }
    if (!stickBottomRef.current) return;
    el.scrollTo({ top: el.scrollHeight });
  }, [messages, sessionId]);
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => {
      lastScrollTopRef.current = el.scrollTop;
      const gap = el.scrollHeight - el.scrollTop - el.clientHeight;
      stickBottomRef.current = gap <= STICK_BOTTOM_PX;
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, []);
  useEffect(() => {
    const el = scrollRef.current;
    // 能力守卫：jsdom 等环境没有 ResizeObserver，缺了只是不做自动复位
    if (!el || typeof ResizeObserver !== "function") return;
    let visible = el.clientHeight > 0;
    const ro = new ResizeObserver(() => {
      const nowVisible = el.clientHeight > 0;
      const becameVisible = nowVisible && !visible;
      visible = nowVisible;
      if (typeof el.scrollTo !== "function") return;
      if (becameVisible) {
        // 刚从隐藏态回来：滚动口被清零，按贴底意图复位
        if (stickBottomRef.current) el.scrollTo({ top: el.scrollHeight });
        else el.scrollTop = lastScrollTopRef.current;
        return;
      }
      if (stickBottomRef.current) el.scrollTo({ top: el.scrollHeight });
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const {
    currentTitle,
    deleteSession,
    newSession,
    runChat,
    stopCurrent,
    switchSession,
  } = useChatSessionActions({
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
  });
  const {
    copyMessage,
    effectiveEffort,
    effortIsOverridden,
    openTaskInMain,
    openTraceFor,
    pickFiles,
    removeMessage,
    send,
    setEffortForSession,
  } = useChatSend({
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
  });
  return (
    <div
      ref={rootRef}
      className="relative flex h-full min-h-0 shrink-0 gap-4"
    >
      {railOpen && (
        <SessionRail
          sessions={sessions}
          sessionId={sessionId}
          onSelect={switchSession}
          onNew={newSession}
          onDelete={deleteSession}
        />
      )}
      {/* flex-[2]：与右侧栏的 flex-1 按 2:1 分剩余空间 —— 侧栏恰为聊天区的
          1/3。写百分比（w-[33.333%]）的基准是根容器（含左侧会话栏），
          与「聊天区的 1/3」不是一回事 */}
      <div className="relative flex min-w-0 flex-[2] flex-col">
      {/* 拖放提示层：文件悬停在聊天区上时显示，松开即加入附件 */}
      {dragHover && (
        <div className="pointer-events-none absolute inset-0 z-40 flex items-center justify-center rounded-2xl border-2 border-dashed border-[var(--brand)] bg-black/20">
          <span className="text-xs text-[var(--brand)]">松开以添加文件</span>
        </div>
      )}
      {/* 聊天主体头部：当前会话标题常驻可见——切到历史会话后，用户靠这一行
          确认自己在哪一段对话里，而不是靠侧栏高亮反推。
          两端各一个折叠钮，是各自侧栏的唯一开关——侧栏内不再放第二个。
          头部块是 relative：计划浮层挂在它下面（top-full），
          贴着聊天区右上角展开，不去挤右侧栏。 */}
      <div className="relative mb-2 shrink-0 border-b border-[var(--edge)] pb-2">
        <header className="flex items-center gap-2">
          <button
            type="button"
            className="nm-icon-btn h-7 w-7"
            aria-expanded={railOpen}
            aria-controls="chat-session-rail"
            aria-label={railOpen ? "收起会话栏" : "展开会话栏"}
            title={railOpen ? "收起会话栏" : "展开会话栏"}
            onClick={toggleRail}
          >
            {railOpen ? (
              <PanelLeftClose size={15} aria-hidden />
            ) : (
              <PanelLeftOpen size={15} aria-hidden />
            )}
          </button>
          <h2 className="min-w-0 flex-1 truncate text-sm font-medium text-[var(--t1)]">
            <span className="sr-only">当前会话：</span>
            {currentTitle}
          </h2>
          <button
            type="button"
            className="nm-icon-btn h-7 w-7"
            aria-expanded={planOpen}
            aria-controls="chat-plan-panel"
            aria-label={planOpen ? "收起计划面板" : "展开计划面板"}
            title={planOpen ? "收起计划面板" : "展开计划面板"}
            onClick={() => setPlanOpen((v) => !v)}
          >
            <ListTodo size={15} aria-hidden />
          </button>
          <button
            type="button"
            className="nm-icon-btn h-7 w-7"
            aria-expanded={ctxOpen}
            aria-controls="chat-context-rail"
            title={ctxOpen ? "收起右侧栏" : "展开右侧栏"}
            aria-label={ctxOpen ? "收起右侧栏" : "展开右侧栏"}
            onClick={toggleCtx}
          >
            {ctxOpen ? (
              <PanelRightClose size={15} aria-hidden />
            ) : (
              <PanelRightOpen size={15} aria-hidden />
            )}
          </button>
        </header>
        {planOpen && (
          <div id="chat-plan-panel" className="absolute right-0 top-full mt-2 z-40">
            <PlanPanel
              sessionId={sessionId}
              subagents={subagentRuns}
              onClose={() => setPlanOpen(false)}
              escBlocked={escBlocked}
            />
          </div>
        )}
      </div>

      {/* 上下文水位条（Anthropic 协议回合级 usage 累计；contextK 来自 active 模型条目） */}
      <UsageMeter
        input={sessionUsage[sessionId ?? ""]?.input ?? 0}
        output={sessionUsage[sessionId ?? ""]?.output ?? 0}
        cacheRead={sessionUsage[sessionId ?? ""]?.cacheRead ?? 0}
        cacheCreation={sessionUsage[sessionId ?? ""]?.cacheCreation ?? 0}
        lastInput={sessionUsage[sessionId ?? ""]?.lastInput ?? 0}
        lastCacheRead={sessionUsage[sessionId ?? ""]?.lastCacheRead ?? 0}
        contextK={models.find((m) => m.id === activeModelId)?.contextK}
      />

      <MessageList
        scrollRef={scrollRef}
        messages={messages}
        viewedBusy={viewedBusy}
        copiedIdx={copiedIdx}
        verboseLevel={verboseLevel}
        subagentSlot={subagentSlot}
        onPreviewFile={openFilePreview}
        onShowFileChanges={openFileChanges}
        onPickStarter={setInput}
        onCopy={copyMessage}
        onRemove={removeMessage}
        onOpenTask={openTaskInMain}
        onOpenExecSession={switchSession}
        onOpenTrace={openTraceFor}
      />
      {/* 停止反馈（bot_stop 回传置位条数）：贴在输入区上方，不进消息流 */}
      {stopNotice && stopNotice.sid === sessionId && (
        <p
          className="shrink-0 px-1 py-1 text-[11px] text-[var(--t4)]"
          title="本会话被置停止标志的执行条数"
        >
          ⏹ 已停 {stopNotice.n} 条
        </p>
      )}
      <InputArea
        verbose={{ level: verboseLevel, setLevel: setVerboseLevel }}
        input={input}
        setInput={setInput}
        files={files}
        setFiles={setFiles}
        taskRefs={refs}
        setTaskRefs={setRefs}
        taRef={taRef}
        slashIdx={slashIdx}
        setSlashIdx={setSlashIdx}
        slashDismissed={slashDismissed}
        setSlashDismissed={setSlashDismissed}
        send={send}
        pickFiles={pickFiles}
        onStop={stopCurrent}
        viewedBusy={viewedBusy}
        perm={{
          mode: permMode,
          menuOpen: permMenuOpen,
          setMenuOpen: setPermMenuOpen,
          onSelect: selectPermMode,
        }}
        permBtnRef={permBtnRef}
        permDropdownRef={permDropdownRef}
        modelBtnRef={modelBtnRef}
        modelDropdownRef={modelDropdownRef}
        effortBtnRef={effortBtnRef}
        effortDropdownRef={effortDropdownRef}
        model={{
          models,
          provider: apiProvider,
          activeId: activeModelId,
          label: modelLabel,
          menuOpen: modelMenuOpen,
          setMenuOpen: setModelMenuOpen,
          onSwitch: switchModel,
        }}
        effort={{
          base: effortBase,
          effective: effectiveEffort,
          isOverridden: effortIsOverridden,
          menuOpen: effortMenuOpen,
          setMenuOpen: setEffortMenuOpen,
          setForSession: setEffortForSession,
        }}
      />
      </div>
      {/* 右侧上下文栏：聊天主体列的兄弟节点（不是 header 的子节点）——
          只有与主体列同级，w-[33.333%] 才参照整个聊天区宽度；塞进 header
          会变成 header 的 flex item，百分比参照 header 而把整行拉垮 */}
      {ctxOpen && (
        <ContextRail
          tasks={tasks}
          onUpdateTask={onUpdateTask}
          onRefTask={refTask}
          refIds={refs.map((r) => r.id)}
          sessionId={sessionId}
          subagents={subagentRuns}
          onOpenSubagent={openSubagentDetail}
          onCancelSubagent={cancelSubagent}
          subagentDetailId={subagentDetailId}
          onCloseSubagentDetail={closeSubagentDetail}
          tab={ctxTab}
          onTabChange={setCtxTab}
          fileSubTab={fileSubTab}
          onFileSubTabChange={setFileSubTab}
          sessionMessages={messages}
          previewReq={previewReq}
          onPreviewReq={setPreviewReq}
        />
      )}
      {/* 执行详情弹层：按回合的 trace 打开（TracePanel 自带 portal） */}
      {openTrace && (
        <TracePanel
          traceId={openTrace.traceId}
          sessionId={openTrace.sessionId ?? undefined}
          onClose={() => setOpenTrace(null)}
        />
      )}
    </div>
  );
}
