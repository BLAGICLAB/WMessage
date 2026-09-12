import { useEffect, useRef, useState, type ReactNode } from "react";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen, emit } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { focusMainWindow } from "../focus";
import { handleCommandError, formatCommandError, isCommandError } from "../lib/errorHandler";
import {
  LINK_OR_PATH_RE,
  extractFilePaths,
  isHttpUrl,
  openTarget,
} from "../lib/openTarget";
import type { Task } from "../types";
import { basename } from "../format";
import { imageExtSet } from "../lib/consts";
import { MarkdownText } from "./MarkdownText";

/** 路径后缀是否图片类型（大小写不敏感）。无后缀或未知后缀按文件处理。
 *  扩展名清单由后端 consts::app_consts 下发（真相在 bot_chat.rs::IMAGE_EXTS，
 *  后端 attach_images 按同一列表判断是否转 base64 image_url）。 */
function isImagePath(p: string): boolean {
  const m = p.toLowerCase().match(/\.([a-z0-9]+)$/);
  return m ? imageExtSet().has(m[1]) : false;
}

/** execute-task 事件去重（模块级，跨组件实例/HMR 泄漏监听器共享）：
 *  dev 期间挂件 webview 多次重挂载会累积多个 execute-task 监听器，
 *  一次点击被投递多次 → 同一秒多个 bot_execute_task 并发 → 后端防重入拦截，
 *  每个拒绝都弹「⚠️ 内部错误：该任务卡正在执行中」气泡，用户误以为执行失败。
 *  去重表必须放模块级：放 useEffect 闭包里则每个泄漏监听器各持一份，去重失效。 */
const execTaskDedup = new Map<string, number>();
const EXEC_TASK_DEDUP_MS = 2000;

type TaskRef = { id: string; title: string };

/** 工具调用行：折叠显示，展开可看入参 */
type ToolCall = { id: string; name: string; args?: string; done?: boolean };

/** Skill 失败半成品上下文（仅本会话内存；FailedButRecoverable 兜底）。
 *  后端 run_skill_scheduler 返回 DslOutcome::FailedButRecoverable { reason, completed_summary, rollback_attempted }
 *  时通过 `bot-skill-failed` SSE event 推过来；前端把它存到这条消息上渲染 ⚠️ 折叠行。
 *  - completedSummary: 失败前已成功 step 的摘要（"Step N (tool): output\nStep N (tool): ..."）
 *  - rollbackAttempted: true 表示 rollback 段跑过且无错；false 表示没写或跑挂
 *  - 仅本会话内存，刷新/重启后丢失（DB 持久化要改 src-tauri/） */
type SkillFailure = {
  skillName: string;
  reason: string;
  completedSummary: string;
  rollbackAttempted: boolean;
};

type Msg = {
  role: "user" | "assistant";
  content: string;
  streaming?: boolean;
  /** 本轮涉及的任务（渲染成可点击按钮，点击去主窗口打开该任务） */
  refs?: TaskRef[];
  /** 模型思考过程（折叠显示，不持久化） */
  thinking?: string;
  /** 本轮工具调用行（折叠显示） */
  tools?: ToolCall[];
  /** Skill 失败半成品上下文（折叠显示 ⚠️ 行；见 SkillFailure 说明） */
  skillFailure?: SkillFailure;
  /** 「查看执行对话」跳转按钮（任务执行聊天化：busy 时执行跳转排队，
   *  忙完提示 + 点击切到该执行会话） */
  actionSessionId?: string;
};

type Session = { id: string; title: string };

/** 折叠块：标题 + 点击展开的内容 */
function Fold({
  title,
  children,
}: {
  title: ReactNode;
  children?: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  return (
    <div>
      <button
        className="inline-flex items-center gap-1 max-w-full text-[10px] text-[var(--t5)] hover:text-[var(--t3)]"
        onClick={() => setOpen((v) => !v)}
      >
        <span className="w-2 shrink-0 inline-block">{open ? "▾" : "▸"}</span>
        <span className="truncate">{title}</span>
      </button>
      {open && children ? (
        <div className="mt-0.5 pl-3 text-[10px] leading-relaxed text-[var(--t4)] whitespace-pre-wrap break-words max-h-40 overflow-y-auto">
          {children}
        </div>
      ) : null}
    </div>
  );
}

/** 文本渲染：http(s) 链接和绝对文件路径可点击（识别规则见 lib/openTarget，
 *  带空格路径不会被截断成「C:\Program」） */
function RichText({ text }: { text: string }) {
  const parts: ReactNode[] = [];
  let last = 0;
  let key = 0;
  for (const m of text.matchAll(LINK_OR_PATH_RE)) {
    const idx = m.index ?? 0;
    const token = m[0];
    if (idx > last) parts.push(<span key={key++}>{text.slice(last, idx)}</span>);
    const isUrl = isHttpUrl(token);
    const clean = isUrl ? token.replace(/[.,;:!?]+$/, "") : token;
    parts.push(
      <a
        key={key++}
        className="text-[var(--brand)] underline decoration-dotted underline-offset-2 cursor-pointer break-all"
        title={isUrl ? "在浏览器打开" : "打开文件/文件夹"}
        onClick={(e) => {
          e.preventDefault();
          openTarget(clean);
        }}
      >
        {clean}
      </a>
    );
    last = idx + token.length;
  }
  if (last < text.length) parts.push(<span key={key++}>{text.slice(last)}</span>);
  return <>{parts}</>;
}

/** 从消息内容里拆出 [附件文件] 块（历史消息恢复附件芯片显示用） */
function splitAttachments(content: string): { files: string[]; text: string } {
  const m = content.match(/^\[附件文件\]\n((?:- .+\n)+)\n/);
  if (!m) return { files: [], text: content };
  const files = m[1]
    .split("\n")
    .filter((l) => l.startsWith("- "))
    .map((l) => l.slice(2));
  return { files, text: content.slice(m[0].length) };
}

/** 用户消息气泡：附件块渲染成 📎/🖼️ 芯片 + 正文。图片用 🖼️ 标记（与后端 IMAGE_EXTS 对齐）。 */
function UserBubbleContent({ content }: { content: string }) {
  const { files, text } = splitAttachments(content);
  return (
    <>
      {files.length > 0 && (
        <div className="flex flex-wrap gap-1 mb-1">
          {files.map((f) => (
            <span
              key={f}
              className="nm-inset inline-flex items-center gap-1 rounded-lg px-1.5 py-0.5 text-[10px] text-[var(--t4)] max-w-full"
              title={f}
            >
              <span className="truncate max-w-[220px]">
                {isImagePath(f) ? "🖼️" : "📎"} {basename(f)}
              </span>
            </span>
          ))}
        </div>
      )}
      {text}
    </>
  );
}

type Props = {
  /** 是否处于选任务模式（点任务卡切换选中） */
  selecting: boolean;
  onToggleSelecting: () => void;
  /** 已选中的任务（显示为可移除的引用块，发送时以 [已选任务] 附在消息后） */
  selectedTasks: Task[];
  onRemoveSelected: (id: string) => void;
  /** 发送完成：清空选择 + 退出选任务模式 */
  onFinishSelection: () => void;
  /** 机器人是否开启（WidgetApp 透传 botOn）。
   *  false 时跳过会话加载/历史恢复与所有对外 invoke，容器仍挂载以保住折叠展开循环里的 messages
   *  与 listeners（详见 WidgetApp 折叠不丢聊天回归测试 + 16:30 bot 开关 resize 修法） */
  enabled: boolean;
};

/** 斜杠命令清单（单一真相：autocomplete picker + runSlashCommand 共享）。
 *  没有 /help：上浮全面板后 /help 还在 LLM 上下文里白白占 token */
const SLASH_COMMANDS = [
  { cmd: "/stop", description: "停止当前回复" },
  { cmd: "/compact", description: "压缩对话上下文" },
  { cmd: "/clean", description: "清空当前对话（替代顶部 🧹 按键）" },
  { cmd: "/retry", description: "重新生成上一条回复" },
];

/** 挂件聊天区：位于任务列表下方，机器人开关开启时显示；支持多会话 */
export function ChatPanel({
  selecting,
  onToggleSelecting,
  selectedTasks,
  onRemoveSelected,
  onFinishSelection,
  enabled,
}: Props) {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [sessionMenuOpen, setSessionMenuOpen] = useState(false);
  const [messages, setMessages] = useState<Msg[]>([]);
  const [input, setInput] = useState("");
  /** 斜杠命令 autocomplete 选中项下标（输入「/」浮出后用 ↑↓ / Tab / 点选） */
  const [slashIdx, setSlashIdx] = useState(0);
  /** 斜杠命令 picker 被 Esc 关掉后，输入未变化前不再自动浮出 */
  const [slashDismissed, setSlashDismissed] = useState(false);
  const [busy, setBusy] = useState(false);
  /** 逐条复制按钮的反馈：记录当前“已复制”的消息下标 */
  const [copiedIdx, setCopiedIdx] = useState<number | null>(null);
  /** 已添加的附件文件路径（➕ 或拖入，随消息一起发送） */
  const [files, setFiles] = useState<string[]>([]);
  /** 拖放悬停：文件拖到聊天区上时显示提示层 */
  const [dragHover, setDragHover] = useState(false);
  /** 危险操作确认请求（机器人删任务前弹窗）；kind="file_access" 时为文件访问授权（三按钮） */
  const [confirmReq, setConfirmReq] = useState<{
    id: string;
    tool: string;
    detail: string;
    kind?: string;
  } | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  /** 聊天区根节点：拖放落点判定用（只接收落在聊天区矩形内的文件） */
  const rootRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLButtonElement>(null);
  const sessionDropdownRef = useRef<HTMLDivElement>(null);
  /** 头部 🧠 标签：显示当前模型（设置页维护，聊天面板只读展示） */
  const [modelLabel, setModelLabel] = useState("…");
  // 顶部行引用 + 下拉 top 定位（紧贴 🤖/🧠 按钮底部，0 间距）
  const topBarRef = useRef<HTMLDivElement>(null);
  const [dropdownTop, setDropdownTop] = useState(0);
  useEffect(() => {
    const update = () => {
      const root = rootRef.current;
      const btn = menuRef.current;
      if (!root || !btn) return;
      const br = btn.getBoundingClientRect();
      const rr = root.getBoundingClientRect();
      setDropdownTop(br.bottom - rr.top);
    };
    update();
    if (typeof ResizeObserver !== "undefined" && rootRef.current) {
      const ro = new ResizeObserver(update);
      ro.observe(rootRef.current);
      return () => ro.disconnect();
    }
    window.addEventListener("resize", update);
    return () => {
      window.removeEventListener("resize", update);
    };
  }, []);
  /** 流式过程中的装饰（思考/工具行/Skill 失败），bot_chat 完成后并入最终消息 */
  const streamingMeta = useRef<{
    thinking?: string;
    tools?: ToolCall[];
    skillFailure?: SkillFailure;
  }>({});
  /** busy 镜像：供事件监听里同步判断。
   *  ⚠️ 必须与 setBusy 同步更新（useEffect 在重渲染后才跑，同一帧内连按会有并发窗口） */
  const busyRef = useRef(false);
  useEffect(() => {
    busyRef.current = busy;
  }, [busy]);
  /** 任务执行聊天化：busy 时到达的执行会话跳转排队（只留最新一个）；
   *  执行本身不排队——后端已在新会话开跑，这里只排「自动跳转查看」 */
  const pendingExecRef = useRef<{ sid: string; title: string } | null>(null);
  /** 任务 id → 执行会话 id（chat-open-session 事件建立；invoke 收尾时按它刷新历史） */
  const execSessionByTaskRef = useRef<Map<string, string>>(new Map());
  const enterBusy = () => {
    busyRef.current = true;
    setBusy(true);
  };
  const exitBusy = () => {
    busyRef.current = false;
    setBusy(false);
    // 忙碌期排队的执行会话跳转：忙完提示「已执行，点击查看」（不打断刚结束的对话）
    const pending = pendingExecRef.current;
    if (pending) {
      pendingExecRef.current = null;
      addHint(
        `${pending.title || "任务"}已在新会话执行，点击查看执行对话`,
        pending.sid
      );
    }
  };
  /** 本地提示消息：只显示不持久化（不污染上下文） */
  const addHint = (content: string, actionSessionId?: string) => {
    setMessages((prev) => [...prev, { role: "assistant", content, actionSessionId }]);
  };
  /** sessionId 镜像：异步收尾时判断会话是否已切换（竞态防护） */
  const sessionIdRef = useRef<string | null>(null);
  useEffect(() => {
    sessionIdRef.current = sessionId;
  }, [sessionId]);

  const persistHistory = (sid: string, msgs: Msg[]) => {
    invoke("bot_history_save", {
      sessionId: sid,
      messages: msgs.map((m) => ({
        role: m.role,
        content: m.content,
        refsJson: m.refs?.length ? JSON.stringify(m.refs) : null,
        thinking: m.thinking ?? null,
        toolsJson: m.tools?.length ? JSON.stringify(m.tools) : null,
      })),
    }).catch((e) =>
      // 后台持久化失败：UI 还能跑，不打断当前对话
      handleCommandError(e, "bot_history_save", { silent: true })
    );
  };

  /** 历史行 → 消息（回填思考/工具折叠行） */
  const rowsToMsgs = (
    rows: {
      role: string;
      content: string;
      refsJson?: string | null;
      thinking?: string | null;
      toolsJson?: string | null;
    }[]
  ): Msg[] =>
    rows.map((r) => ({
      role: r.role === "user" ? "user" : "assistant",
      content: r.content,
      refs: r.refsJson ? (JSON.parse(r.refsJson) as TaskRef[]) : undefined,
      thinking: r.thinking ?? undefined,
      tools: r.toolsJson ? (JSON.parse(r.toolsJson) as ToolCall[]) : undefined,
    }));

  // 挂载：加载会话列表（空则新建一个），选中最近更新的会话并恢复其消息
  // enabled=false 时直接跳过（机器人关闭，容器仍挂载但内部不工作；
  // 重新开启后 useEffect 因为 enabled 变化重跑加载）
  useEffect(() => {
    if (!enabled) return;
    (async () => {
      try {
        let list = await invoke<Session[]>("bot_sessions_load");
        if (!list.length) {
          const s = await invoke<Session>("bot_session_create", { title: null });
          list = [s];
        }
        setSessions(list);
        const cur = list[0]; // 按 updated_at 倒序，第一个即最近会话
        setSessionId(cur.id);
        const rows = await invoke<
          {
            role: string;
            content: string;
            refsJson?: string | null;
            thinking?: string | null;
            toolsJson?: string | null;
          }[]
        >("bot_history_load", { sessionId: cur.id });
        if (rows?.length) {
          setMessages(rowsToMsgs(rows));
        }
      } catch (e) {
        handleCommandError(e, "chat init", { silent: true });
      }
    })();
  }, [enabled]);

  // 点击会话菜单外关闭（下拉与按钮不在同一个 ref 容器里，需同时检测两者）
  useEffect(() => {
    if (!sessionMenuOpen) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (
        !menuRef.current?.contains(t) &&
        !sessionDropdownRef.current?.contains(t)
      )
        setSessionMenuOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [sessionMenuOpen]);

  // 挂载：读当前模型配置，头部 🧠 按钮显示当前提供商（自定义地址显示模型名）；
  // 设置页保存配置后广播 bot-config-changed，这里同步刷新
  useEffect(() => {
    const reload = () =>
      invoke<{ baseUrl?: string; model?: string }>("bot_get_config")
        .then((c) => setModelLabel(c.model || "未配置"))
        .catch(() => setModelLabel("未配置"));
    reload();
    const un = listen("bot-config-changed", reload);
    return () => {
      un.then((f) => f());
    };
  }, []);

  // 流式增量：追加到最后一条 streaming 中的助手消息
  // 会话过滤：六个流式事件 payload 均带 sessionId
  // （交互实例专属；后台任务不推流），只消费属于当前会话的增量，
  // 否则两个会话并行跑时输出会互相串台
  useEffect(() => {
    const unlisten = listen<{ text?: string; sessionId?: string | null }>(
      "bot-chat-delta",
      (e) => {
        const sid = e.payload?.sessionId ?? null;
        if (sid !== sessionIdRef.current) return;
        const t = e.payload?.text;
        if (!t) return;
        setMessages((prev) => {
          const last = prev[prev.length - 1];
          if (!last || !last.streaming) return prev;
          const copy = [...prev];
          copy[copy.length - 1] = { ...last, content: last.content + t };
          return copy;
        });
      }
    );
    const unThink = listen<{ text?: string; sessionId?: string | null }>(
      "bot-think-delta",
      (e) => {
        const sid = e.payload?.sessionId ?? null;
        if (sid !== sessionIdRef.current) return;
        const t = e.payload?.text;
        if (!t) return;
        const thinking = (streamingMeta.current.thinking ?? "") + t;
        streamingMeta.current.thinking = thinking;
        setMessages((prev) => {
          const last = prev[prev.length - 1];
          if (!last || !last.streaming) return prev;
          const copy = [...prev];
          copy[copy.length - 1] = { ...last, thinking };
          return copy;
        });
      }
    );
    const unTool = listen<{ id?: string; name?: string; sessionId?: string | null }>(
      "bot-tool",
      (e) => {
        const sid = e.payload?.sessionId ?? null;
        if (sid !== sessionIdRef.current) return;
        const { id, name } = e.payload ?? {};
        if (!id) return;
        const tools = [...(streamingMeta.current.tools ?? [])];
        if (!tools.some((x) => x.id === id)) tools.push({ id, name: name ?? "" });
        streamingMeta.current.tools = tools;
        setMessages((prev) => {
          const last = prev[prev.length - 1];
          if (!last || !last.streaming) return prev;
          const copy = [...prev];
          copy[copy.length - 1] = { ...last, tools };
          return copy;
        });
      }
    );
    const unToolName = listen<{ id?: string; name?: string; sessionId?: string | null }>(
      "bot-tool-name",
      (e) => {
        const sid = e.payload?.sessionId ?? null;
        if (sid !== sessionIdRef.current) return;
        const { id, name } = e.payload ?? {};
        if (!id || !name) return;
        const tools = (streamingMeta.current.tools ?? []).map((x) =>
          x.id === id ? { ...x, name } : x
        );
        streamingMeta.current.tools = tools;
        setMessages((prev) => {
          const last = prev[prev.length - 1];
          if (!last || !last.streaming) return prev;
          const copy = [...prev];
          copy[copy.length - 1] = { ...last, tools };
          return copy;
        });
      }
    );
    const unToolDone = listen<{
      id?: string;
      name?: string;
      args?: string;
      sessionId?: string | null;
    }>("bot-tool-done", (e) => {
      const sid = e.payload?.sessionId ?? null;
      if (sid !== sessionIdRef.current) return;
      const { id, name, args } = e.payload ?? {};
      if (!id) return;
      const tools = (streamingMeta.current.tools ?? []).map((x) =>
        x.id === id ? { ...x, name: name ?? x.name, args, done: true } : x
      );
      streamingMeta.current.tools = tools;
      setMessages((prev) => {
        const last = prev[prev.length - 1];
        if (!last || !last.streaming) return prev;
        const copy = [...prev];
        copy[copy.length - 1] = { ...last, tools };
        return copy;
      });
    });
    // Skill 失败半成品：后端 run_skill_scheduler 返回
    // FailedButRecoverable 时 emit `bot-skill-failed` event；前端把它挂到当前流式消息上
    // 渲染 ⚠️ 折叠行，让用户看到哪步成功哪步失败 + 是否回滚。
    // 注意：失败时同一条流式消息后续还会有 LLM 兜底回复，所以不要清空 streamingMeta。
    const unSkillFailed = listen<{
      skillName?: string;
      reason?: string;
      completedSummary?: string;
      rollbackAttempted?: boolean;
      sessionId?: string | null;
    }>("bot-skill-failed", (e) => {
      const sid = e.payload?.sessionId ?? null;
      if (sid !== sessionIdRef.current) return;
      const p = e.payload ?? {};
      if (!p.skillName) return;
      const failure: SkillFailure = {
        skillName: p.skillName,
        reason: p.reason ?? "(无原因)",
        completedSummary: p.completedSummary ?? "(无已完成步骤)",
        rollbackAttempted: !!p.rollbackAttempted,
      };
      streamingMeta.current.skillFailure = failure;
      setMessages((prev) => {
        const last = prev[prev.length - 1];
        if (!last || !last.streaming) return prev;
        const copy = [...prev];
        copy[copy.length - 1] = { ...last, skillFailure: failure };
        return copy;
      });
    });
    return () => {
      unlisten.then((f) => f());
      unThink.then((f) => f());
      unTool.then((f) => f());
      unToolName.then((f) => f());
      unToolDone.then((f) => f());
      unSkillFailed.then((f) => f());
    };
  }, []);

  /** 切到执行会话围观：加载已落库历史 + 末尾 streaming 占位气泡承接后续流式增量 */
  const openExecSession = async (sid: string) => {
    setSessionMenuOpen(false);
    setSessionId(sid);
    streamingMeta.current = {};
    try {
      const rows = await invoke<Parameters<typeof rowsToMsgs>[0]>(
        "bot_history_load",
        { sessionId: sid }
      );
      setMessages([
        ...rowsToMsgs(rows),
        { role: "assistant", content: "", streaming: true },
      ]);
    } catch (e) {
      handleCommandError(e, "bot_history_load", { silent: true });
    }
  };
  // openExecSession 经 ref 暴露给事件监听（避免闭包旧状态）
  const openExecSessionRef = useRef<(sid: string) => void>(() => {});
  openExecSessionRef.current = (sid) => {
    openExecSession(sid);
  };

  // 任务卡交给机器人执行（execute-task 事件：主窗口/挂件卡片 🤖 按钮触发）
  // 任务执行聊天化：不再在当前会话执行——后端建新会话跑（run_task_in_chat），
  // 前端收 chat-open-session 切过去围观。执行本身不吃 busy 锁（会话隔离天然并发），
  // 只有「自动跳转查看」在 busy 时排队（exitBusy 时 hint 提示）。
  useEffect(() => {
    // 去重表在模块级 execTaskDedup（泄漏的监听器实例间共享才有效，见文件头注释）
    const unExec = listen<{ id?: string; title?: string }>("execute-task", (e) => {
      const { id } = e.payload ?? {};
      if (!id) return;
      const now = Date.now();
      if (now - (execTaskDedup.get(id) ?? 0) < EXEC_TASK_DEDUP_MS) return;
      execTaskDedup.set(id, now);
      invoke("bot_execute_task", { taskId: id, sessionId: null })
        .then(() => {
          // 执行收尾：刷新会话列表（新会话入列）；若正围观该执行会话，
          // 重载历史替换流式占位气泡为最终落库内容
          const sid = execSessionByTaskRef.current.get(id);
          execSessionByTaskRef.current.delete(id);
          invoke<Session[]>("bot_sessions_load")
            .then(setSessions)
            .catch(() => {});
          if (sid && sessionIdRef.current === sid) {
            invoke<Parameters<typeof rowsToMsgs>[0]>("bot_history_load", { sessionId: sid })
              .then((rows) => setMessages(rowsToMsgs(rows)))
              .catch(() => {});
          }
        })
        .catch((err) =>
          // TASK_INVALID_STATE（执行中重复触发/已完成/已归档）按业务状态提示而非错误
          addHint(
            `${isCommandError(err) && err.code === "TASK_INVALID_STATE" ? "⏳" : "⚠️"} ${formatCommandError(err)}`
          )
        );
    });
    return () => {
      unExec.then((f) => f());
    };
  }, []);

  // chat-open-session：后端新建执行会话后广播——
  // 非 busy 直接切过去围观（流式增量按 sessionId 过滤自动落到新会话的占位气泡）；
  // busy 不打断当前对话，跳转排队，当前轮结束后 exitBusy 弹「点击查看」提示。
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
          : [{ id: sid, title: title ?? "执行" }, ...prev]
      );
      if (busyRef.current) {
        pendingExecRef.current = { sid, title: title ?? "" };
        return;
      }
      openExecSessionRef.current(sid);
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  // 危险操作确认：机器人删任务前弹窗（60s 无响应后端自动拒绝）；
  // kind="file_access"：文件访问授权，三按钮（允许一次/始终允许该目录/拒绝）；
  // sessionId 归属过滤（会话隔离）：只弹属于当前会话的确认，
  // 别的会话/后台任务的确认不弹（后端 60s 超时自动拒绝兜底）
  useEffect(() => {
    const unConfirm = listen<{ id?: string; tool?: string; detail?: string; kind?: string; sessionId?: string | null }>(
      "bot-confirm",
      (e) => {
        const { id, tool, detail, kind, sessionId: sid } = e.payload ?? {};
        if (!id) return;
        // 只弹明确属于当前会话的确认；别的会话 / 无归属（后台任务）不弹，
        // 后端 60s 超时自动拒绝兜底
        if (sid == null || sid !== sessionIdRef.current) return;
        setConfirmReq({ id, tool: tool ?? "", detail: detail ?? "", kind });
      }
    );
    return () => {
      unConfirm.then((f) => f());
    };
  }, []);

  const answerConfirm = (approved: boolean, always = false) => {
    if (!confirmReq) return;
    invoke("bot_confirm_response", {
      requestId: confirmReq.id,
      approved,
      always,
    }).catch((e) =>
      handleCommandError(e, "bot_confirm_response", { silent: true })
    );
    setConfirmReq(null);
  };

  // 新消息自动滚到底
  useEffect(() => {
    scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight });
  }, [messages]);

  const switchSession = async (sid: string) => {
    if (busyRef.current || sid === sessionId) {
      setSessionMenuOpen(false);
      return;
    }
    setSessionId(sid);
    setSessionMenuOpen(false);
    setMessages([]);
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
      setMessages(rowsToMsgs(rows));
    } catch (e) {
      handleCommandError(e, "bot_history_load", { silent: true });
    }
  };

  const newSession = async () => {
    if (busyRef.current) return;
    try {
      const s = await invoke<Session>("bot_session_create", { title: null });
      setSessions((prev) => [s, ...prev]);
      setSessionMenuOpen(false);
      setSessionId(s.id);
      setMessages([]);
      setInput("");
    } catch (e) {
      handleCommandError(e, "bot_session_create", { silent: true });
    }
  };

  const deleteSession = async (sid: string) => {
    if (busyRef.current) return;
    const target = sessions.find((s) => s.id === sid);
    if (!window.confirm(`删除对话「${target?.title ?? "未命名"}」？消息记录一并删除。`)) return;
    try {
      await invoke("bot_session_delete", { id: sid });
      const rest = sessions.filter((s) => s.id !== sid);
      setSessions(rest);
      if (sid === sessionId) {
        // 删的是当前会话：切到剩余第一个；没有则新建
        if (rest.length) {
          setSessionId(rest[0].id);
          const rows = await invoke<
            {
              role: string;
              content: string;
              refsJson?: string | null;
              thinking?: string | null;
              toolsJson?: string | null;
            }[]
          >("bot_history_load", { sessionId: rest[0].id });
          setMessages(rowsToMsgs(rows));
        } else {
          const s = await invoke<Session>("bot_session_create", { title: null });
          setSessions([s]);
          setSessionId(s.id);
          setMessages([]);
        }
      }
    } catch (e) {
      handleCommandError(e, "bot_session_delete");
    }
  };

  /** 核心执行：发送历史并流式收尾（send / /retry 共用）。
   *  任务执行聊天化：bot_execute_task 不再走这里（任务卡执行由后端
   *  建新会话跑，前端收 chat-open-session 切换围观）。
   *  收尾时校验会话未切换才更新 UI；持久化始终按 sid 写（写的是正确会话） */
  const runChat = async (history: Msg[], renameText?: string) => {
    const sid = sessionId;
    if (!sid) return;
    enterBusy();
    setMessages([...history, { role: "assistant", content: "", streaming: true }]);
    streamingMeta.current = {};
    try {
      const full = await invoke<{ text: string; taskRefs?: TaskRef[] }>(
        "bot_chat",
        { messages: history.map((m) => ({ role: m.role, content: m.content })), sessionId: sid }
      );
      // 把流式过程中累积的思考/工具行并入最终消息
      const meta = streamingMeta.current;
      streamingMeta.current = {};
      const done: Msg[] = [
        ...history,
        {
          role: "assistant",
          content: full.text || "",
          refs: full.taskRefs ?? [],
          thinking: meta.thinking,
          tools: meta.tools,
          skillFailure: meta.skillFailure,
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
              prev.map((s) => (s.id === sid ? { ...s, title: short } : s))
            )
          )
          .catch((e) =>
            handleCommandError(e, "bot_session_rename", { silent: true })
          );
      }
      onFinishSelection();
    } catch (e) {
      const failed: Msg[] = [
        ...history,
        { role: "assistant", content: `⚠️ ${formatCommandError(e)}` },
      ];
      persistHistory(sid, failed);
      if (sessionIdRef.current === sid) setMessages(failed);
      // 流式错误已经写进消息气泡了，不重复弹 alert
      handleCommandError(e, "bot_chat", { silent: true });
    } finally {
      exitBusy();
    }
  };

  // 斜杠快捷命令（/stop 在机器人回复中也能生效）
  const runSlashCommand = async (raw: string): Promise<boolean> => {
    const cmd = raw.split(/\s+/)[0].toLowerCase();
    if (cmd === "/stop") {
      setInput("");
      if (busyRef.current) {
        // /stop 按会话停止——只停当前会话的执行实例
        invoke("bot_stop", { sessionId: sessionIdRef.current }).catch((e) =>
          handleCommandError(e, "bot_stop", { silent: true })
        );
      } else {
        addHint("当前没有进行中的回复");
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
        handleCommandError(e, "bot_history_clear", { silent: true })
      );
      return true;
    }
    if (cmd === "/compact") {
      setInput("");
      if (busyRef.current || !sessionId) return true;
      const sid = sessionId;
      const history = messages.filter((m) => !m.streaming && m.content.trim());
      if (history.length < 2) {
        addHint("消息太少，暂无需压缩");
        return true;
      }
      enterBusy();
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
        const note: Msg = { role: "assistant", content: `⚠️ 压缩失败：${formatCommandError(e)}` };
        const failed = [...history, note];
        persistHistory(sid, failed);
        if (sessionIdRef.current === sid) setMessages(failed);
        handleCommandError(e, "bot_compact", { silent: true });
      } finally {
        exitBusy();
      }
      return true;
    }
    if (cmd === "/retry") {
      setInput("");
      if (busyRef.current || !sessionId) return true;
      const msgs = messages.filter((m) => !m.streaming);
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
    // 斜杠命令优先处理（/stop 在 busy 时也能生效）；未知斜杠文本当普通消息发
    if (text.startsWith("/")) {
      if (await runSlashCommand(text)) return;
    }
    // busyRef 同步检查：state 重渲染前连续两次 Enter 也能拦下重复发送
    if ((!text && !files.length) || busyRef.current || !sessionId) return;
    setInput("");
    // 已选任务以 [已选任务] 引用块附在消息后，模型按 taskId 精确操作
    const block = selectedTasks
      .map((t) => `- id=${t.id}，标题=${t.title}`)
      .join("\n");
    // 附件以 [附件文件] 块附在消息前（模型用 extract_document 的 path 直读，不弹框）
    const attach = files.length
      ? `[附件文件]\n${files.map((f) => `- ${f}`).join("\n")}\n\n`
      : "";
    const content = `${attach}${text}${
      selectedTasks.length ? `\n\n[已选任务]\n${block}` : ""
    }`;
    setFiles([]);
    const history: Msg[] = [
      ...messages.filter((m) => !m.streaming),
      { role: "user", content },
    ];
    await runChat(history, text);
  };

  /** 附件去重追加（➕ 选文件 / 拖入文件共用） */
  const addFiles = (paths: string[]) => {
    const clean = paths.filter(Boolean);
    if (clean.length) {
      setFiles((prev) => [...prev, ...clean.filter((p) => !prev.includes(p))]);
    }
  };

  // ➕ 添加附件：选文件（文档或图片均可），与消息一起发送（如：加 Word 后输入「润色」）。
  // Rust 侧弹框：此前前端 dialog.open 在挂件窗口不弹框（点击无反应）
  // 注意：pick_files_dialog 当前 Rust 实现未暴露 filter 参数，不做硬过滤；
  //   图片识别靠 isImagePath(path)（后端 IMAGE_EXTS 同步），图片走多模态，文档走 extract_document。
  const pickFiles = async () => {
    try {
      const picked = await invoke<string[]>("pick_files_dialog");
      if (picked.length) addFiles(picked);
    } catch (e) {
      handleCommandError(e, "pick_files_dialog", { silent: true });
    }
  };

  // 拖文件进聊天区 → 加入附件（等同 ➕ 选文件）。
  // Tauri 窗口 dragDropEnabled 默认开启：OS 级拖放不触发 HTML5 drop，
  // 走窗口级 tauri 事件（onDragDropEvent）；position 为物理像素，需除缩放系数
  // 转成 CSS 像素后与聊天区矩形比对——拖到挂件任务列表区的文件不归聊天管。
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    const insideChat = async (pos: { x: number; y: number }) => {
      const rect = rootRef.current?.getBoundingClientRect();
      if (!rect) return false;
      const scale = await getCurrentWindow()
        .scaleFactor()
        .catch(() => 1);
      const x = pos.x / scale;
      const y = pos.y / scale;
      return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
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
            if (await insideChat(p.position)) addFiles(p.paths);
          }
        })
        .then((f) => {
          unlisten = f;
        })
        // 非 Tauri 环境（vitest/jsdom）没有窗口对象，静默忽略
        .catch(() => {});
    } catch {
      // 同上：同步抛错（无 __TAURI_INTERNALS__）也静默忽略
    }
    return () => unlisten?.();
  }, []);

  // 逐条复制回复内容（按钮反馈在消息下方）
  const copyMessage = async (idx: number, content: string) => {
    try {
      await navigator.clipboard.writeText(content);
      setCopiedIdx(idx);
      setTimeout(() => setCopiedIdx((c) => (c === idx ? null : c)), 1500);
    } catch (e) {
      handleCommandError(e, "clipboard", { silent: true });
    }
  };

  // 移除一条回复：不满意时删掉，不再作为上下文发给模型（全量持久化）
  const removeMessage = (idx: number) => {
    if (busyRef.current || !sessionId) return;
    const next = messages.filter((_, j) => j !== idx);
    setMessages(next);
    persistHistory(sessionId, next);
  };

  // 点任务引用按钮：通知主窗口打开该任务编辑态 + 唤起主窗口并强制置顶
  // （与挂件双击标题同一规则：主窗口要出现在桌面屏幕最顶层）
  const openTaskInMain = async (ref: TaskRef) => {
    emit("edit-task", { id: ref.id }).catch(() => {});
    focusMainWindow();
  };

  const currentTitle =
    sessions.find((s) => s.id === sessionId)?.title ?? "新对话";

  return (
    <div ref={rootRef} className="relative flex flex-col shrink-0 h-full min-h-0">
      {/* 拖放提示层：文件悬停在聊天区上时显示，松开即加入附件 */}
      {dragHover && (
        <div className="pointer-events-none absolute inset-0 z-40 flex items-center justify-center rounded-2xl border-2 border-dashed border-[var(--brand)] bg-black/20">
          <span className="text-xs text-[var(--brand)]">松开以添加文件</span>
        </div>
      )}
      {/* 危险操作确认弹窗：机器人删任务前等老板拍板；file_access = 文件访问授权（三按钮） */}
      {confirmReq && (
        <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/30 rounded-2xl">
          <div className="nm-card p-3 w-64">
            <p className="text-xs font-medium text-[var(--t2)] mb-1.5">
              {confirmReq.kind === "file_access"
                ? "📂 机器人要访问白名单外路径"
                : `⚠️ 机器人要${confirmReq.tool === "delete_task" ? "删除任务" : "执行危险操作"}`}
            </p>
            <p className="text-xs text-[var(--t3)] mb-1 break-words">
              「{confirmReq.detail}」
            </p>
            <p className="text-[10px] text-[var(--t5)] mb-3">
              60 秒内不回复将自动拒绝
            </p>
            {confirmReq.kind === "file_access" ? (
              <div className="flex flex-col gap-2">
                <button
                  className="nm-btn w-full px-2 py-1 text-xs text-[var(--t3)]"
                  onClick={() => answerConfirm(true, false)}
                >
                  允许一次
                </button>
                <button
                  className="nm-btn w-full px-2 py-1 text-xs text-[var(--t3)]"
                  onClick={() => answerConfirm(true, true)}
                >
                  始终允许该目录
                </button>
                <button
                  className="nm-btn w-full px-2 py-1 text-xs text-[var(--danger)]"
                  onClick={() => answerConfirm(false)}
                >
                  拒绝
                </button>
              </div>
            ) : (
              <div className="flex gap-2">
                <button
                  className="nm-btn flex-1 px-2 py-1 text-xs text-[var(--t3)]"
                  onClick={() => answerConfirm(true)}
                >
                  允许
                </button>
                <button
                  className="nm-btn flex-1 px-2 py-1 text-xs text-[var(--danger)]"
                  onClick={() => answerConfirm(false)}
                >
                  拒绝
                </button>
              </div>
            )}
          </div>
        </div>
      )}
      <div ref={topBarRef} className="flex items-center mb-2 shrink-0 gap-1">
        {/* 左侧：🤖 会话 + 🧠 模型 两块平分空间 */}
        <div className="flex items-center gap-1 flex-1 min-w-0">
          {/* 🤖 会话切换器（平分第一块） */}
          <button
            ref={menuRef}
            className="nm-outset flex-1 min-w-0 flex items-center gap-1 rounded-lg px-2 py-1 text-xs text-[var(--t2)]"
            title="切换会话"
            onClick={() => setSessionMenuOpen((v) => !v)}
          >
            <span className="truncate flex-1 text-left">🤖 {currentTitle}</span>
            <span className="shrink-0 text-[10px] text-[var(--t5)]">
              {sessionMenuOpen ? "▴" : "▾"}
            </span>
          </button>
          {/* 🧠 当前模型标签（只读；模型在设置页维护）
              宽度按内容收窄：flex-1 → shrink-0 max-w-fit，剩余空间全部让给 🤖 会话按钮 */}
          <span
            className="nm-outset shrink-0 max-w-fit flex items-center gap-1 rounded-lg px-2 py-1 text-xs text-[var(--t2)]"
            title={`当前模型：${modelLabel}（设置页切换）`}
          >
            <span className="truncate text-left">🧠 {modelLabel}</span>
          </span>
        </div>
        {/* 右侧：🎯 移到原 🧹 位置（最右；外框 px-2 py-1 跟 🤖/🧠 等高，emoji 内部 16px 免受字体档位影响） */}
        <button
          className={`shrink-0 px-2 py-1 flex items-center justify-center ${
            selecting
              ? "nm-inset text-[var(--t1)] font-medium"
              : "nm-outset text-[var(--t4)]"
          }`}
          onClick={onToggleSelecting}
          title="选任务模式：点击下方任务卡选中，再输入操作指令"
        >
          <span className="text-[16px] leading-none">🎯</span>
        </button>
      </div>
      {/* 会话下拉：作为 rootRef 直接子元素，absolute 横跨整个 panel 宽度
          （左对齐 🤖、右对齐 🎯） */}
      {sessionMenuOpen && (
        <div
          ref={sessionDropdownRef}
          className="absolute left-0 right-0 nm-card p-1 rounded-xl z-50 max-h-40 overflow-y-auto"
          style={{ top: dropdownTop > 0 ? `${dropdownTop}px` : undefined }}
        >
          {sessions.map((s) => (
            <div key={s.id} className="flex items-center gap-1">
              <button
                className={`flex-1 min-w-0 text-left px-2 py-1 rounded-lg text-xs truncate ${
                  s.id === sessionId
                    ? "nm-inset text-[var(--t1)] font-medium"
                    : "text-[var(--t3)] hover:bg-[var(--hover-bg)]"
                }`}
                onClick={() => switchSession(s.id)}
                title={s.title}
              >
                {s.title}
              </button>
              <button
                className="shrink-0 w-5 h-5 flex items-center justify-center rounded text-[10px] text-[var(--t5)] hover:text-[var(--danger)] hover:bg-[var(--hover-bg)]"
                title="删除此对话"
                onClick={() => deleteSession(s.id)}
              >
                🗑
              </button>
            </div>
          ))}
          <button
            className="w-full text-left px-2 py-1 rounded-lg text-xs text-[var(--brand)] hover:bg-[var(--hover-bg)]"
            onClick={newSession}
          >
            ＋ 新建对话
          </button>
        </div>
      )}

      {selecting && (
        <p className="mb-1.5 text-[10px] text-[var(--brand)] shrink-0">
          点击下方任务卡选择要操作的任务，选完输入指令（如：标记完成）
        </p>
      )}

      {/* 已选任务引用块 */}
      {selectedTasks.length > 0 && (
        <div className="mb-1.5 flex flex-wrap gap-1 shrink-0">
          {selectedTasks.map((t) => (
            <span
              key={t.id}
              className="nm-inset inline-flex items-center gap-1 rounded-lg px-2 py-0.5 text-[10px] text-[var(--t3)] max-w-full"
            >
              <span className="truncate max-w-[280px]">📌 {t.title}</span>
              <button
                className="text-[var(--t5)] hover:text-[var(--danger)]"
                onClick={() => onRemoveSelected(t.id)}
                title="移除"
              >
                ×
              </button>
            </span>
          ))}
        </div>
      )}

      <div ref={scrollRef} className="flex-1 min-h-0 overflow-y-auto space-y-1.5 pr-0.5">
        {messages.length === 0 ? (
          <p className="text-xs text-[var(--t5)] text-center mt-6">
            跟我说：新建任务、列出任务、完成某任务…（输入 / 看可用命令）
          </p>
        ) : (
          messages.map((m, i) => {
            // 一次性算文件路径 + 是否显示操作行（避免在渲染条件里 IIFE + mutation m._fps 的反模式）
            const fps = extractFilePaths(m.content);
            const showActions =
              m.role === "assistant" &&
              !m.streaming &&
              (m.content.trim() || (m.refs?.length ?? 0) > 0 || fps.length > 0);
            return (
            <div
              key={i}
              className={`max-w-[85%] ${
                m.role === "user" ? "ml-auto" : "mr-auto"
              }`}
            >
              <div
                className={`px-2.5 py-1.5 text-xs leading-relaxed whitespace-pre-wrap break-words ${
                  m.role === "user"
                    ? "nm-outset rounded-xl text-[var(--t2)]"
                    : "nm-inset rounded-xl text-[var(--t3)]"
                }`}
              >
                {m.role === "assistant" && (m.thinking?.length ?? 0) > 0 && (
                  <Fold title={<span>💭 思考过程{m.streaming ? " …" : ""}</span>}>
                    {m.thinking}
                  </Fold>
                )}
                {/* Skill 失败半成品。
                 *  区别于 🔧 工具折叠行：用 ⚠️ 标记，视觉上提示「这不是普通工具调用」；
                 *  默认折叠，点开才看哪个 step 成功 + 是否回滚。 */}
                {m.role === "assistant" && m.skillFailure && (
                  <Fold
                    title={
                      <span
                        className={
                          m.skillFailure.rollbackAttempted
                            ? "text-[var(--t4)]"
                            : "text-[var(--danger)]"
                        }
                      >
                        ⚠️ Skill 失败：{m.skillFailure.skillName}
                        {m.skillFailure.rollbackAttempted
                          ? "（已回滚）"
                          : "（未回滚，请人工核对）"}
                      </span>
                    }
                  >
                    <div className="space-y-1">
                      <div>
                        <span className="opacity-70">原因：</span>
                        <span>{m.skillFailure.reason}</span>
                      </div>
                      <div>
                        <span className="opacity-70">已完成步骤：</span>
                        <pre className="opacity-80 whitespace-pre-wrap break-words">
                          {m.skillFailure.completedSummary}
                        </pre>
                      </div>
                      {!m.skillFailure.rollbackAttempted && (
                        <div className="text-[var(--danger)]">
                          ⚠️ 已完成步骤未回滚，请检查任务卡状态。
                        </div>
                      )}
                    </div>
                  </Fold>
                )}
                {(m.tools?.length ?? 0) > 0 && (
                  <div className="mt-0.5 space-y-0.5">
                    {m.tools!.map((t) => (
                      <Fold
                        key={t.id}
                        title={
                          <span>
                            🔧 {t.name || "工具调用"}
                            {t.done ? " ✓" : " …"}
                          </span>
                        }
                      >
                        {t.args ? (
                          <code className="opacity-80">
                            {t.args.length > 500
                              ? t.args.slice(0, 500) + "…"
                              : t.args}
                          </code>
                        ) : null}
                      </Fold>
                    ))}
                  </div>
                )}
                {m.content ? (
                  m.role === "user" ? (
                    <UserBubbleContent content={m.content} />
                  ) : m.streaming ? (
                    <RichText text={m.content} />
                  ) : (
                    <MarkdownText text={m.content} />
                  )
                ) : m.streaming ? (
                  "…"
                ) : (
                  ""
                )}
                {/* 「查看执行对话」跳转（任务执行聊天化：busy 时跳转排队，
                    忙完 hint + 按钮切换到执行会话） */}
                {m.actionSessionId && (
                  <button
                    className="nm-btn mt-1 inline-flex items-center gap-1 rounded-lg px-2 py-0.5 text-[10px] text-[var(--t2)]"
                    onClick={() => switchSession(m.actionSessionId!)}
                  >
                    💬 查看执行对话
                  </button>
                )}
              </div>
              {/* 回复完成后的操作行：复制按钮 + 任务引用按钮（点击去主窗口打开该任务） */}
              {showActions && (
                  <div className="mt-1 flex flex-wrap items-center gap-1">
                    {m.content.trim() && (
                      <button
                        className="nm-btn inline-flex items-center gap-1 rounded-lg px-2 py-0.5 text-[10px] text-[var(--t3)]"
                        title="复制回复内容"
                        onClick={() => copyMessage(i, m.content)}
                      >
                        {copiedIdx === i ? "✓ 已复制" : "📋 复制"}
                      </button>
                    )}
                    {m.content.trim() && (
                      <button
                        className="nm-btn inline-flex items-center gap-1 rounded-lg px-2 py-0.5 text-[10px] text-[var(--t4)] hover:text-[var(--danger)]"
                        title="移除这条回复（不满意时删除，不再作为上下文）"
                        onClick={() => removeMessage(i)}
                        disabled={busy}
                      >
                        🗑 移除
                      </button>
                    )}
                    {fps.map((f) => (
                      <button
                        key={f}
                        className="nm-btn inline-flex items-center gap-1 rounded-lg px-2 py-0.5 text-[10px] text-[var(--t3)] max-w-full"
                        title={`打开文件：${f}`}
                        onClick={() => {
                          // Rust 侧打开（挂件窗口 openPath 前端权限可能被拒）；失败弹错不静默
                          openTarget(f);
                        }}
                      >
                        <span className="truncate max-w-[280px]">📄 {basename(f)}</span>
                      </button>
                    ))}
                    {(m.refs ?? []).map((r) => (
                      <button
                        key={r.id}
                        className="nm-btn inline-flex items-center gap-1 rounded-lg px-2 py-0.5 text-[10px] text-[var(--t3)] max-w-full"
                        title={`打开任务：${r.title}`}
                        onClick={() => openTaskInMain(r)}
                      >
                        <span className="truncate max-w-[260px]">📌 {r.title}</span>
                      </button>
                    ))}
                  </div>
                )}
            </div>
            );
          })
        )}
      </div>
      {/* 已添加附件：随消息一起发送 */}
      {files.length > 0 && (
        <div className="mb-1.5 flex flex-wrap gap-1 shrink-0">
          {files.map((f) => {
            const isImg = isImagePath(f);
            return (
              <div key={f} className="relative group">
                <span
                  className="nm-inset inline-flex items-center gap-1 rounded-lg px-2 py-0.5 text-[10px] text-[var(--t3)] max-w-full"
                  title={f}
                >
                  <span className="truncate max-w-[280px]">
                    {isImg ? "🖼️" : "📎"} {basename(f)}
                  </span>
                  <button
                    className="text-[var(--t5)] hover:text-[var(--danger)]"
                    onClick={() => setFiles((prev) => prev.filter((x) => x !== f))}
                    title="移除"
                  >
                    ×
                  </button>
                </span>
                {/* 图片附件悬停缩略图：convertFileSrc 走 asset:// 协议。
                    tauri.conf.json 启用 assetProtocol.enable 后可读图；
                    未启用时 <img> 加载失败自然隐藏，chip 行为不变。 */}
                {isImg && (
                  <img
                    src={convertFileSrc(f)}
                    alt={basename(f)}
                    loading="lazy"
                    onError={(e) => {
                      // 资产协议未启用（403）时隐藏占位元素
                      (e.currentTarget as HTMLImageElement).style.display = "none";
                    }}
                    className="pointer-events-none absolute bottom-full left-0 mb-1 hidden group-hover:block max-w-[220px] max-h-[120px] rounded-lg border border-[var(--border)] bg-[var(--bg)] shadow-lg object-contain z-10"
                  />
                )}
              </div>
            );
          })}
        </div>
      )}

      {/* 斜杠命令 autocomplete：第一个字是 / 且无空格时浮出 picker。
          点选 / Tab / ↑↓ 选 / Esc 关 */}
      {!slashDismissed && input.startsWith("/") && !input.includes(" ") && (
        <div className="mb-1.5 nm-card rounded-xl p-1 max-h-40 overflow-y-auto shrink-0">
          {SLASH_COMMANDS.filter((c) => c.cmd.startsWith(input)).map(
            (c, i) => (
              <button
                key={c.cmd}
                className={`w-full flex flex-col items-start gap-0 px-2 py-1 rounded-lg text-left ${
                  i === slashIdx ? "nm-inset" : "hover:bg-[var(--hover-bg)]"
                }`}
                onMouseEnter={() => setSlashIdx(i)}
                onClick={(e) => {
                  e.stopPropagation();
                  setInput(c.cmd + " ");
                  setSlashIdx(0);
                  setSlashDismissed(false);
                }}
              >
                <span className="text-xs text-[var(--t2)] font-medium">
                  {c.cmd}
                </span>
                <span className="text-[10px] text-[var(--t4)]">
                  {c.description}
                </span>
              </button>
            )
          )}
        </div>
      )}

      <div className="flex gap-1.5 shrink-0">
        <button
          className="nm-btn shrink-0 px-2.5 py-1.5 text-xs text-[var(--t3)]"
          title="添加文件或图片，和消息一起发送（如：添加 Word 后输入「润色」；图片发给机器人识别：png / jpg / jpeg / webp / gif / bmp，最大 3MB/张、最多 4 张/消息）"
          onClick={pickFiles}
          disabled={busy}
        >
          ➕
        </button>
        <input
          value={input}
          onChange={(e) => {
            setInput(e.target.value);
            setSlashIdx(0);
            setSlashDismissed(false);
          }}
          onKeyDown={(e) => {
            // 斜杠 picker 开着时拦截上下/Tab/Esc
            const pickerOpen =
              !slashDismissed && input.startsWith("/") && !input.includes(" ");
            const matches = SLASH_COMMANDS.filter((c) => c.cmd.startsWith(input));
            if (pickerOpen && e.key === "ArrowDown" && matches.length) {
              e.preventDefault();
              setSlashIdx((slashIdx + 1) % matches.length);
              return;
            }
            if (pickerOpen && e.key === "ArrowUp" && matches.length) {
              e.preventDefault();
              setSlashIdx((slashIdx - 1 + matches.length) % matches.length);
              return;
            }
            if (pickerOpen && (e.key === "Tab" || e.key === "Enter") && matches.length) {
              // Tab 总是补全；Enter 在 picker 开着时也补全（避免输入半截命令误发送）
              e.preventDefault();
              setInput(matches[slashIdx].cmd + " ");
              setSlashIdx(0);
              return;
            }
            if (pickerOpen && e.key === "Escape") {
              e.preventDefault();
              setSlashDismissed(true);
              setSlashIdx(0);
              return;
            }
            // 与 TodoCard 一致：中文输入法组合态下回车确认候选词不应触发发送
            if (e.key === "Enter" && !e.nativeEvent.isComposing) send();
          }}
          placeholder={
            busy
              ? "回复中…（点右侧 ■ 或输入 /stop 可停止）"
              : files.length
                ? "输入指令，如：润色这个文件"
                : selecting
                  ? "输入操作指令，如：标记完成"
                  : "和机器人说点什么"
          }
          className="nm-inset flex-1 min-w-0 rounded-xl px-3 py-1.5 text-xs text-[var(--t3)] outline-none placeholder:text-[var(--t5)]"
        />
        {/* 发送/停止一体键：回复中变为红框正方形停止键，
            点击即 bot_stop 中断本次运行；中断/回答结束自动变回发送键 */}
        {busy ? (
          <button
            className="nm-btn shrink-0 px-3 py-1.5 text-xs text-[var(--danger)] flex items-center"
            title="停止当前回复"
            onClick={() =>
              invoke("bot_stop", { sessionId: sessionIdRef.current }).catch((e) =>
                handleCommandError(e, "bot_stop", { silent: true })
              )
            }
          >
            ■
          </button>
        ) : (
          <button
            className="nm-btn shrink-0 px-3 py-1.5 text-xs text-[var(--t3)]"
            onClick={send}
          >
            发送
          </button>
        )}
      </div>
    </div>
  );
}
