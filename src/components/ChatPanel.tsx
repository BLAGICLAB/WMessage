import { useEffect, useRef, useState, type ReactNode } from "react";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen, emit } from "@tauri-apps/api/event";

import { openUrl } from "@tauri-apps/plugin-opener";
import { focusMainWindow } from "../focus";
import { handleCommandError, formatCommandError } from "../lib/errorHandler";
import { PROVIDER_PRESETS, matchPreset, type ProviderPreset } from "../lib/providerPresets";
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
 *  2026-08-19 事故：dev 期间挂件 webview 多次重挂载累积了 6 个 execute-task 监听器，
 *  一次点击被投递 6 次 → 同一秒 6 个 bot_execute_task 并发 → 后端防重入拦截 5 个，
 *  每个拒绝都弹「⚠️ 内部错误：该任务卡正在执行中」气泡，用户误以为执行失败。
 *  去重表必须放模块级：放 useEffect 闭包里则每个泄漏监听器各持一份，去重失效。 */
const execTaskDedup = new Map<string, number>();
const EXEC_TASK_DEDUP_MS = 2000;

type TaskRef = { id: string; title: string };

/** 工具调用行：折叠显示，展开可看入参 */
type ToolCall = { id: string; name: string; args?: string; done?: boolean };

/** Skill 失败半成品上下文（仅本会话内存；Phase 4 第 5 项：FailedButRecoverable 兜底）。
 *  后端 run_skill_scheduler 返回 DslOutcome::FailedButRecoverable { reason, completed_summary, rollback_attempted }
 *  时通过 `bot-skill-failed` SSE event 推过来；前端把它存到这条消息上渲染 ⚠️ 折叠行。
 *  - completedSummary: 失败前已成功 step 的摘要（"Step N (tool): output\nStep N (tool): ..."）
 *  - rollbackAttempted: true 表示 rollback 段跑过且无错；false 表示没写或跑挂
 *  - 仅本会话内存，刷新/重启后丢失（DB 持久化要改 src-tauri/，留给后续 subagent） */
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

/** 文本渲染：http(s) 链接和绝对文件路径可点击 */
function RichText({ text }: { text: string }) {
  const parts: ReactNode[] = [];
  // URL | Windows 绝对路径 | 常见 unix 绝对路径（/Users /home /Library 等）
  const re =
    /(https?:\/\/[^\s<>"'，。；、（）【】！!？?`*]+)|([A-Za-z]:[\\/][^\s<>"'，。；、（）【】！!？?`*]+)|(\/(?:Users|home|var|tmp|Library|Applications|opt)\b[^\s<>"'，。；、（）【】！!？?`*]*)/g;
  let last = 0;
  let key = 0;
  for (const m of text.matchAll(re)) {
    const idx = m.index ?? 0;
    const token = m[0];
    if (idx > last) parts.push(<span key={key++}>{text.slice(last, idx)}</span>);
    const isUrl = token.startsWith("http");
    const clean = isUrl ? token.replace(/[.,;:!?]+$/, "") : token;
    parts.push(
      <a
        key={key++}
        className="text-[var(--brand)] underline decoration-dotted underline-offset-2 cursor-pointer break-all"
        title={isUrl ? "在浏览器打开" : "打开文件/文件夹"}
        onClick={(e) => {
          e.preventDefault();
          if (isUrl) {
            openUrl(clean).catch(() => {});
          } else {
            // 打开失败（如路径不存在）时退到在 Finder 中显示，不静默
            invoke("open_file_path", { path: clean }).catch((e) =>
              handleCommandError(e, "open_file_path", { silent: true })
            );
          }
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
              <span className="truncate max-w-[150px]">
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
};

/** 斜杠命令清单（单一真相：autocomplete picker + runSlashCommand 共享）
 *  老板 2026-08-17 15:51 指令：输入「/」浮出补全列表
 *  老板 2026-08-17 21:52 指令：移除 /help（上浮全面板后 /help 还在 LLM 上下文里白白占 token） */
const SLASH_COMMANDS = [
  { cmd: "/stop", description: "停止当前回复" },
  { cmd: "/compact", description: "压缩对话上下文" },
  { cmd: "/retry", description: "重新生成上一条回复" },
];

/** 挂件聊天区：位于任务列表下方，机器人开关开启时显示；支持多会话 */
export function ChatPanel({
  selecting,
  onToggleSelecting,
  selectedTasks,
  onRemoveSelected,
  onFinishSelection,
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
  /** 已添加的附件文件路径（➕ 添加，随消息一起发送） */
  const [files, setFiles] = useState<string[]>([]);
  /** 危险操作确认请求（机器人删任务前弹窗）；kind="file_access" 时为文件访问授权（三按钮） */
  const [confirmReq, setConfirmReq] = useState<{
    id: string;
    tool: string;
    detail: string;
    kind?: string;
  } | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  /** 模型切换菜单（头部 🧠 按钮）：label 显示当前提供商/模型 */
  const [modelMenuOpen, setModelMenuOpen] = useState(false);
  const [modelLabel, setModelLabel] = useState("…");
  const modelMenuRef = useRef<HTMLDivElement>(null);
  /** 菜单内「自定义…」内联表单：自由填 Base URL + 模型名 */
  const [customOpen, setCustomOpen] = useState(false);
  const [customBaseUrl, setCustomBaseUrl] = useState("");
  const [customModel, setCustomModel] = useState("");
  /** 流式过程中的装饰（思考/工具行/Skill 失败），bot_chat 完成后并入最终消息 */
  const streamingMeta = useRef<{
    thinking?: string;
    tools?: ToolCall[];
    skillFailure?: SkillFailure;
  }>({});
  /** busy 镜像：供事件监听里同步判断。
   *  ⚠️ 必须与 setBusy 同步更新（useEffect 在重渲染后才跑，同一帧内连按会有并发窗口，审计 P2） */
  const busyRef = useRef(false);
  useEffect(() => {
    busyRef.current = busy;
  }, [busy]);
  const enterBusy = () => {
    busyRef.current = true;
    setBusy(true);
  };
  const exitBusy = () => {
    busyRef.current = false;
    setBusy(false);
  };
  /** 本地提示消息：只显示不持久化（不污染上下文） */
  const addHint = (content: string) => {
    setMessages((prev) => [...prev, { role: "assistant", content }]);
  };
  /** sessionId 镜像：异步收尾时判断会话是否已切换（二次审计 P3 竞态防护） */
  const sessionIdRef = useRef<string | null>(null);
  useEffect(() => {
    sessionIdRef.current = sessionId;
  }, [sessionId]);

/** 从消息文本提取文件路径（绝对路径：/Users/...、盘符路径），去重保序。
 *  机器人产物的路径可能被反引号包裹（Markdown code），正则穿过反引号提取后清理。 */
function extractFilePaths(content: string): string[] {
  const re =
    /(?:[A-Za-z]:[\\/][^\s<>"'，。；、（）【】`*]+|\/(?:Users|home|var|tmp|Library|Applications|opt)\b[^\s<>"'，。；、（）【】`*]*)/g;
  const out: string[] = [];
  for (const m of content.matchAll(re)) {
    let p = m[0].replace(/[`.,;:!?]+$/, "").trim();
    if (!p || p.startsWith("http")) continue;
    if (out.includes(p)) continue;
    out.push(p);
  }
  return out;
}

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
  useEffect(() => {
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
  }, []);

  // 点击会话菜单外关闭
  useEffect(() => {
    if (!sessionMenuOpen) return;
    const onDown = (e: MouseEvent) => {
      if (!menuRef.current?.contains(e.target as Node)) setSessionMenuOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [sessionMenuOpen]);

  // 挂载：读当前模型配置，头部 🧠 按钮显示当前提供商（自定义地址显示模型名）；
  // 设置页保存配置后广播 bot-config-changed，这里同步刷新
  useEffect(() => {
    const reload = () =>
      invoke<{ baseUrl?: string; model?: string }>("bot_get_config")
        .then((c) =>
          setModelLabel(
            matchPreset(c.baseUrl ?? "", c.model ?? "")?.label ?? (c.model || "未配置")
          )
        )
        .catch(() => setModelLabel("未配置"));
    reload();
    const un = listen("bot-config-changed", reload);
    return () => {
      un.then((f) => f());
    };
  }, []);

  // 点击模型菜单外关闭
  useEffect(() => {
    if (!modelMenuOpen) return;
    const onDown = (e: MouseEvent) => {
      if (!modelMenuRef.current?.contains(e.target as Node)) setModelMenuOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [modelMenuOpen]);

  /** 回写模型配置：整体写回（保留白名单/Tavily 等字段），apiKey 传 null 保持 keychain 现有 key */
  const applyModelConfig = async (baseUrl: string, model: string, label: string) => {
    setModelMenuOpen(false);
    setCustomOpen(false);
    try {
      const c = await invoke<{
        bypassLlmOnPreStepHit?: boolean;
        allowedDirs?: string[];
        tavilyKey?: string;
        tavilyEnabled?: boolean | null;
        pythonTimeoutSecs?: number | null;
      }>("bot_get_config");
      await invoke("bot_set_config", {
        config: {
          baseUrl,
          model,
          bypassLlmOnPreStepHit: c.bypassLlmOnPreStepHit ?? true,
          allowedDirs: c.allowedDirs ?? [],
          tavilyKey: c.tavilyKey ?? null,
          tavilyEnabled: c.tavilyEnabled ?? null,
          pythonTimeoutSecs: c.pythonTimeoutSecs ?? null,
        },
        apiKey: null,
      });
      setModelLabel(label);
      addHint(`✅ 已切换到 ${label}（${model}）`);
    } catch (e) {
      handleCommandError(e, "bot_set_config", { silent: true });
      addHint(`⚠️ 切换模型失败：${formatCommandError(e)}`);
    }
  };

  const switchModel = (p: ProviderPreset) => applyModelConfig(p.baseUrl, p.model, p.label);

  /** 自定义模型：菜单内联表单提交（baseUrl/model 必填） */
  const applyCustomModel = () => {
    const b = customBaseUrl.trim();
    const m = customModel.trim();
    if (!b || !m) {
      addHint("⚠️ 自定义模型需要填写 Base URL 和模型名");
      return;
    }
    applyModelConfig(b, m, m);
  };

  // 流式增量：追加到最后一条 streaming 中的助手消息
  useEffect(() => {
    const unlisten = listen<{ text?: string }>("bot-chat-delta", (e) => {
      const t = e.payload?.text;
      if (!t) return;
      setMessages((prev) => {
        const last = prev[prev.length - 1];
        if (!last || !last.streaming) return prev;
        const copy = [...prev];
        copy[copy.length - 1] = { ...last, content: last.content + t };
        return copy;
      });
    });
    const unThink = listen<{ text?: string }>("bot-think-delta", (e) => {
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
    });
    const unTool = listen<{ id?: string; name?: string }>("bot-tool", (e) => {
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
    });
    const unToolName = listen<{ id?: string; name?: string }>("bot-tool-name", (e) => {
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
    });
    const unToolDone = listen<{ id?: string; name?: string; args?: string }>(
      "bot-tool-done",
      (e) => {
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
      }
    );
    // Skill 失败半成品（Phase 4 第 5 项 + 审计 P2）：后端 run_skill_scheduler 返回
    // FailedButRecoverable 时 emit `bot-skill-failed` event；前端把它挂到当前流式消息上
    // 渲染 ⚠️ 折叠行，让用户看到哪步成功哪步失败 + 是否回滚。
    // 注意：失败时同一条流式消息后续还会有 LLM 兜底回复，所以不要清空 streamingMeta。
    const unSkillFailed = listen<{
      skillName?: string;
      reason?: string;
      completedSummary?: string;
      rollbackAttempted?: boolean;
    }>("bot-skill-failed", (e) => {
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

  // 任务卡交给机器人执行（execute-task 事件：主窗口/挂件卡片 🤖 按钮触发）
  const executeTask = async (id: string, title: string) => {
    if (busyRef.current || !sessionId) return;
    const userMsg: Msg = {
      role: "user",
      content: `🤖 执行任务卡：${title || "未命名任务"}`,
    };
    const history: Msg[] = [...messages.filter((m) => !m.streaming), userMsg];
    // 复用 runChat 核心逻辑（二次审计 P3 去重）；首轮会话改名用任务名
    await runChat(history, title || "任务执行", id);
  };

  // executeTask 经 ref 暴露给事件监听（避免闭包旧状态）
  const executeTaskRef = useRef<(id: string, title: string) => void>(() => {});
  executeTaskRef.current = (id, title) => {
    executeTask(id, title);
  };
  useEffect(() => {
    // 去重表在模块级 execTaskDedup（泄漏的监听器实例间共享才有效，见文件头注释）
    const unExec = listen<{ id?: string; title?: string }>("execute-task", (e) => {
      const { id, title } = e.payload ?? {};
      if (!id) return;
      const now = Date.now();
      if (now - (execTaskDedup.get(id) ?? 0) < EXEC_TASK_DEDUP_MS) return;
      execTaskDedup.set(id, now);
      executeTaskRef.current(id, title ?? "");
    });
    return () => {
      unExec.then((f) => f());
    };
  }, []);

  // 危险操作确认：机器人删任务前弹窗（60s 无响应后端自动拒绝）；
  // kind="file_access"（2026-08-26）：文件访问授权，三按钮（允许一次/始终允许该目录/拒绝）
  useEffect(() => {
    const unConfirm = listen<{ id?: string; tool?: string; detail?: string; kind?: string }>(
      "bot-confirm",
      (e) => {
        const { id, tool, detail, kind } = e.payload ?? {};
        if (id) setConfirmReq({ id, tool: tool ?? "", detail: detail ?? "", kind });
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

  /** 核心执行：发送历史并流式收尾（send / /retry / executeTask 共用）。
   *  execTaskId 存在时走 bot_execute_task（🤖 任务卡执行），否则 bot_chat。
   *  收尾时校验会话未切换才更新 UI；持久化始终按 sid 写（写的是正确会话） */
  const runChat = async (
    history: Msg[],
    renameText?: string,
    execTaskId?: string
  ) => {
    const sid = sessionId;
    if (!sid) return;
    enterBusy();
    setMessages([...history, { role: "assistant", content: "", streaming: true }]);
    streamingMeta.current = {};
    try {
      const full = await invoke<{ text: string; taskRefs?: TaskRef[] }>(
        execTaskId ? "bot_execute_task" : "bot_chat",
        execTaskId
          ? { taskId: execTaskId }
          : { messages: history.map((m) => ({ role: m.role, content: m.content })) }
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
      // 防重入拒绝（这张卡真在跑）不是错误：不持久化 ⚠️ 气泡、不污染会话历史，
      // 只恢复触发前消息 + 本地提示（2026-08-19：幻影重复触发留下的错误气泡
      // 让用户以为执行失败，实际任务在正常跑）
      if (execTaskId && formatCommandError(e).includes("正在执行中")) {
        if (sessionIdRef.current === sid) setMessages(history.slice(0, -1));
        addHint("⏳ 这张卡正在执行中，跑完会实时更新；完成后再触发");
      } else {
        const failed: Msg[] = [
          ...history,
          { role: "assistant", content: `⚠️ ${formatCommandError(e)}` },
        ];
        persistHistory(sid, failed);
        if (sessionIdRef.current === sid) setMessages(failed);
      }
      // 流式错误已经写进消息气泡了，不重复弹 alert
      handleCommandError(e, execTaskId ? "bot_execute_task" : "bot_chat", { silent: true });
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
        invoke("bot_stop").catch((e) =>
          handleCommandError(e, "bot_stop", { silent: true })
        );
      } else {
        addHint("当前没有进行中的回复");
      }
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
      // 进度占位：压缩请求期间有可见反馈（二次审计 P3：原界面像卡死）
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
    // busyRef 同步检查：state 重渲染前连续两次 Enter 也能拦下重复发送（审计 P2）
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

  // ➕ 添加附件：选文件（文档或图片均可），与消息一起发送（如：加 Word 后输入「润色」）。
  // Rust 侧弹框：此前前端 dialog.open 在挂件窗口不弹框（点击无反应）
  // 注意：pick_files_dialog 当前 Rust 实现未暴露 filter 参数，不做硬过滤；
  //   图片识别靠 isImagePath(path)（后端 IMAGE_EXTS 同步），图片走多模态，文档走 extract_document。
  const pickFiles = async () => {
    try {
      const picked = await invoke<string[]>("pick_files_dialog");
      if (picked.length) {
        setFiles((prev) => [...prev, ...picked.filter((p) => !prev.includes(p))]);
      }
    } catch (e) {
      handleCommandError(e, "pick_files_dialog", { silent: true });
    }
  };

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
  // （与挂件双击标题同一规则：老板 2026-08-17 11:31「主窗口要出现在桌面屏幕最顶层」）
  const openTaskInMain = async (ref: TaskRef) => {
    emit("edit-task", { id: ref.id }).catch(() => {});
    focusMainWindow();
  };

  const currentTitle =
    sessions.find((s) => s.id === sessionId)?.title ?? "新对话";

  return (
    <div className="relative flex flex-col shrink-0 h-full min-h-0">
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
      <div className="flex items-center justify-between mb-2 shrink-0 gap-1">
        {/* 会话切换器 */}
        <div className="relative min-w-0 flex-1" ref={menuRef}>
          <button
            className="nm-outset w-full flex items-center gap-1 rounded-lg px-2 py-1 text-xs text-[var(--t2)] min-w-0"
            title="切换会话"
            onClick={() => setSessionMenuOpen((v) => !v)}
          >
            <span className="truncate flex-1 text-left">🤖 {currentTitle}</span>
            <span className="shrink-0 text-[10px] text-[var(--t5)]">
              {sessionMenuOpen ? "▴" : "▾"}
            </span>
          </button>
          {sessionMenuOpen && (
            <div className="absolute left-0 top-full mt-1 w-full nm-card p-1 rounded-xl z-50 max-h-40 overflow-y-auto">
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
        </div>
        <div className="flex items-center gap-1 shrink-0">
          {/* 模型快速切换（不用进设置页）；跨提供商切换后需在设置页更新对应 API Key */}
          <div className="relative shrink-0" ref={modelMenuRef}>
            <button
              className="px-2 py-0.5 text-[10px] nm-outset text-[var(--t5)] hover:text-[var(--t3)] max-w-[110px] truncate"
              title={`切换模型（当前：${modelLabel}）`}
              onClick={() => setModelMenuOpen((v) => !v)}
            >
              🧠 {modelLabel}
            </button>
            {modelMenuOpen && (
              <div className="absolute right-0 top-full mt-1 w-52 nm-card p-1 rounded-xl z-50">
                {PROVIDER_PRESETS.map((p) => (
                  <button
                    key={p.label}
                    className={`w-full text-left px-2 py-1 rounded-lg text-xs flex items-center justify-between gap-1 ${
                      modelLabel === p.label
                        ? "nm-inset text-[var(--t1)] font-medium"
                        : "text-[var(--t3)] hover:bg-[var(--hover-bg)]"
                    }`}
                    onClick={() => switchModel(p)}
                  >
                    <span>{p.label}</span>
                    <span className="text-[9px] text-[var(--t5)] truncate">{p.model}</span>
                  </button>
                ))}
                {/* 自定义：自由填 Base URL + 模型名（任何 OpenAI 兼容端点） */}
                <button
                  className={`w-full text-left px-2 py-1 rounded-lg text-xs ${
                    customOpen
                      ? "nm-inset text-[var(--t1)] font-medium"
                      : "text-[var(--t3)] hover:bg-[var(--hover-bg)]"
                  }`}
                  onClick={() => {
                    if (!customOpen) {
                      // 展开时预填当前配置，方便在原基础上改
                      invoke<{ baseUrl?: string; model?: string }>("bot_get_config")
                        .then((c) => {
                          setCustomBaseUrl(c.baseUrl ?? "");
                          setCustomModel(c.model ?? "");
                        })
                        .catch(() => {});
                    }
                    setCustomOpen((v) => !v);
                  }}
                >
                  ✏️ 自定义…
                </button>
                {customOpen && (
                  <div className="px-1.5 py-1 space-y-1">
                    <input
                      value={customBaseUrl}
                      onChange={(e) => setCustomBaseUrl(e.target.value)}
                      placeholder="Base URL，如 https://api.example.com/v1"
                      className="nm-inset w-full rounded-lg px-2 py-1 text-[10px] text-[var(--t3)] outline-none"
                    />
                    <input
                      value={customModel}
                      onChange={(e) => setCustomModel(e.target.value)}
                      placeholder="模型名，如 my-model"
                      className="nm-inset w-full rounded-lg px-2 py-1 text-[10px] text-[var(--t3)] outline-none"
                    />
                    <button
                      className="nm-btn w-full px-2 py-1 text-[10px] text-[var(--t2)]"
                      onClick={applyCustomModel}
                    >
                      使用此模型
                    </button>
                  </div>
                )}
                <p className="px-2 py-1 text-[9px] leading-snug text-[var(--t6)]">
                  跨提供商切换后，需到设置页更新对应的 API Key
                </p>
              </div>
            )}
          </div>
          <button
            className={`px-2 py-0.5 text-[10px] ${
              selecting
                ? "nm-inset text-[var(--t1)] font-medium"
                : "nm-outset text-[var(--t5)] hover:text-[var(--t3)]"
            }`}
            onClick={onToggleSelecting}
            title="选任务模式：点击下方任务卡选中，再输入操作指令"
          >
            🎯
          </button>
          <button
            className="px-2 py-0.5 text-[10px] nm-outset text-[var(--t5)] hover:text-[var(--t3)]"
            title="清空当前对话"
            onClick={() => {
              if (!sessionId) return;
              setMessages([]);
              invoke("bot_history_clear", { sessionId }).catch((e) =>
                handleCommandError(e, "bot_history_clear", { silent: true })
              );
            }}
          >
            🧹
          </button>
        </div>
      </div>

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
              <span className="truncate max-w-[180px]">📌 {t.title}</span>
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
                {/* Skill 失败半成品（Phase 4 第 5 项 + 审计 P2 优化项）。
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
                          // Rust 侧打开（挂件窗口 openPath 前端权限可能被拒，点击无反应）
                          invoke("open_file_path", { path: f }).catch((e) =>
                            handleCommandError(e, "open_file_path", { silent: true })
                          );
                        }}
                      >
                        <span className="truncate max-w-[180px]">📄 {basename(f)}</span>
                      </button>
                    ))}
                    {(m.refs ?? []).map((r) => (
                      <button
                        key={r.id}
                        className="nm-btn inline-flex items-center gap-1 rounded-lg px-2 py-0.5 text-[10px] text-[var(--t3)] max-w-full"
                        title={`打开任务：${r.title}`}
                        onClick={() => openTaskInMain(r)}
                      >
                        <span className="truncate max-w-[160px]">📌 {r.title}</span>
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
                  <span className="truncate max-w-[180px]">
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
                    className="pointer-events-none absolute bottom-full left-0 mb-1 hidden group-hover:block max-w-[160px] max-h-[120px] rounded-lg border border-[var(--border)] bg-[var(--bg)] shadow-lg object-contain z-10"
                  />
                )}
              </div>
            );
          })}
        </div>
      )}

      {/* 斜杠命令 autocomplete：第一个字是 / 且无空格时浮出 picker
     （老板 2026-08-17 15:51 指令）。点选 / Tab / ↑↓ 选 / Esc 关 */}
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
        {/* 发送/停止一体键（2026-08-26 老板拍板）：回复中变为红框正方形停止键，
            点击即 bot_stop 中断本次运行；中断/回答结束自动变回发送键 */}
        {busy ? (
          <button
            className="nm-btn shrink-0 px-3 py-1.5 text-xs text-[var(--danger)] flex items-center"
            title="停止当前回复"
            onClick={() =>
              invoke("bot_stop").catch((e) =>
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
