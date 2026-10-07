// ChatPanel 主组件（orchestrator）。
// 1862 → 拆分：SessionList / MessageList / InputArea / useChatUi 到同目录子文件
// （U3a，行为等价拆分——消息数据流/斜杠命令语义/Tauri 命令调用零改动），
// 本文件保留所有 useState / useRef / useEffect / handlers / 数据流监听 / 装配 JSX。
// MsgBubble 的 React.memo 流式性能边界见 MessageList.tsx 头注释（B5-2）。
//
// 公开 import 路径保持稳定：外部仍 `import { ChatPanel } from "./components/ChatPanel"`，
// Vite 解析到 `./ChatPanel/index.tsx` → 透传 `./ChatPanel`。

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, emit } from "@tauri-apps/api/event";
import { Pin } from "lucide-react";
import { useTauriListen } from "../../lib/useTauriListen";
import { getCurrentWindow } from "@tauri-apps/api/window";


import { handleCommandError, formatCommandError, isCommandError } from "../../lib/errorHandler";
import type { Task } from "../../types";

import type {
  ChatModelEntry,
  FileChangeLite,
  ModelItem,
  Msg,
  PermMode,
  ReasoningLevel,
  Session,
  SkillFailure,
  TaskRef,
  ToolCall,
  VerboseLevel,
} from "./types";
import {
  DELTA_BATCH_MS,
  execTaskDedup,
  finishedExecTasks,
} from "./constants";
import { useAutoGrow, useDropdownTop, useOutsideClose } from "./useChatUi";
import { SessionList } from "./SessionList";
import { MessageList } from "./MessageList";
import { UsageMeter } from "./UsageMeter";
import { InputArea } from "./InputArea";

/** 模型下拉条目集合协议（MP-02）：types.ModelItem 的来源协议收窄用 */
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
  // 输入窗口锁定对话（DRAFT-1）：草稿/附件按会话隔离——切走再切回各自保留，
  // 会话 A 打的字切到 B 不会误发给 B。input/files 是「当前会话」的派生值，
  // setInput/setFiles 伪装成 React setter，原有调用点（onChange/send/斜杠补全/
  // 发送清空/附件增删）语义不变。覆盖只在内存，不落盘（会话历史由后端管）。
  const [drafts, setDrafts] = useState<Map<string, string>>(new Map());
  const draftKey = sessionId ?? "";
  const input = drafts.get(draftKey) ?? "";
  const setInput = (v: string) =>
    setDrafts((prev) => {
      const next = new Map(prev);
      next.set(draftKey, v);
      return next;
    });
  /** 输入卡 textarea 引用：自动增高用（UI-1 单行 input 升级为多行 textarea） */
  const taRef = useRef<HTMLTextAreaElement>(null);
  /** 斜杠命令 autocomplete 选中项下标（输入「/」浮出后用 ↑↓ / Tab / 点选） */
  const [slashIdx, setSlashIdx] = useState(0);
  /** 斜杠命令 picker 被 Esc 关掉后，输入未变化前不再自动浮出 */
  const [slashDismissed, setSlashDismissed] = useState(false);
  /** PAR-1 并行回复：每个会话独立在途。inflightSids 驱动 UI（按视图会话判定），
   *  inflightRef 供事件监听同步判断（⚠️ 必须与 setInflightSids 同步更新，
   *  useEffect 镜像在同一帧内连按会有并发窗口） */
  const [inflightSids, setInflightSids] = useState<ReadonlySet<string>>(new Set());
  /** 逐条复制按钮的反馈：记录当前“已复制”的消息下标 */
  const [copiedIdx, setCopiedIdx] = useState<number | null>(null);
  /** 已添加的附件文件路径（➕ 或拖入，随消息一起发送）——按会话隔离（DRAFT-1） */
  const [filesBySession, setFilesBySession] = useState<Map<string, string[]>>(
    new Map()
  );
  const files = filesBySession.get(draftKey) ?? [];
  const setFiles = (v: string[] | ((prev: string[]) => string[])) =>
    setFilesBySession((prev) => {
      const next = new Map(prev);
      next.set(
        draftKey,
        typeof v === "function" ? v(prev.get(draftKey) ?? []) : v
      );
      return next;
    });
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
  // 🧠 模型下拉（MP-01）：同会话菜单三件套——开合 + 按钮 ref + 下拉 ref，
  // 另带当前协议的模型列表与 active id（reload 里一并维护）
  const [modelMenuOpen, setModelMenuOpen] = useState(false);
  const [models, setModels] = useState<ModelItem[]>([]);
  // 当前协议（MP-02）：双协议同列展示后，active 高亮只认本协议的 active 模型
  const [apiProvider, setApiProvider] = useState<"openai" | "anthropic">("openai");
  const [activeModelId, setActiveModelId] = useState<string | null>(null);
  const modelBtnRef = useRef<HTMLButtonElement>(null);
  const modelDropdownRef = useRef<HTMLDivElement>(null);
  /** 输入卡底栏 🧠 标签：显示当前模型（设置页维护，聊天面板只读展示） */
  const [modelLabel, setModelLabel] = useState("…");
  // ⚡ 推理强度（RE-1）：后台默认基线（bot-config-changed 时刷新）+ 会话级覆盖。
  // 覆盖只存内存 Map，绝不回写 bot-config.json；会话切换后无覆盖的会话直接跟随后台。
  const [effortBase, setEffortBase] = useState<ReasoningLevel>("medium");
  const [effortBySession, setEffortBySession] = useState<Map<string, ReasoningLevel>>(
    new Map()
  );
  const [effortMenuOpen, setEffortMenuOpen] = useState(false);
  const effortBtnRef = useRef<HTMLButtonElement>(null);
  const effortDropdownRef = useRef<HTMLDivElement>(null);
  /** 🛡 授权模式（U3b 只读展示）：bot-config 的 permMode，设置页维护 */
  const [permMode, setPermMode] = useState<PermMode>("ask");
  // P4 上下文水位条：bot-usage-delta 按会话累计（Anthropic 协议回合级 usage）。
  // lastInput = 最近一轮请求的 input（即当前上下文 prompt 体量）——每轮 input 是
  // 完整重发的 prompt，跨轮累加会随轮数近似平方膨胀，水位百分比必须用最近一轮。
  const [sessionUsage, setSessionUsage] = useState<
    Record<string, { input: number; output: number; lastInput: number }>
  >({});
  // P2-b 执行过程详细度（verbose 三档）：localStorage 持久化，仅影响 ToolBadges 展示档位
  const [verboseLevel, setVerboseLevelState] = useState<VerboseLevel>(() => {
    const v = localStorage.getItem("chat-verbose-level");
    return v === "concise" || v === "debug" ? v : "detailed";
  });
  const setVerboseLevel = (l: VerboseLevel) => {
    setVerboseLevelState(l);
    localStorage.setItem("chat-verbose-level", l);
  };
  // 顶部行引用 + 下拉 top 定位（紧贴 🤖 按钮底部，0 间距）
  const topBarRef = useRef<HTMLDivElement>(null);
  const dropdownTop = useDropdownTop(rootRef, menuRef);
  /** 输入卡 textarea 自动增高：随内容长到 max-h-40 后内部滚动；发送清空后缩回 */
  useAutoGrow(taRef, input);
  /** 切会话：斜杠 picker 状态复位（草稿/附件已按会话隔离，picker 是瞬时 UI 态） */
  useEffect(() => {
    // 挂载式 mount-refresh：复位瞬时 UI 态（可改写为派生但会换执行时机，拆分批保行为）
    // oxlint-disable-next-line react/set-state-in-effect
    setSlashIdx(0);
    // oxlint-disable-next-line react/set-state-in-effect
    setSlashDismissed(false);
  }, [sessionId]);
  /** PAR-1：每会话流式元数据（thinking/tools/skillFailure 权威副本，按 sid 隔离；
   *  并行回复各自累积互不串台，收尾并入最终消息后删除条目） */
  const streamingMetaMapRef = useRef<
    Map<
      string,
      {
        thinking?: string;
        tools?: ToolCall[];
        skillFailure?: SkillFailure;
        fileChanges?: FileChangeLite[];
      }
    >
  >(new Map());
  const metaFor = (sid: string | null) => {
    if (!sid) return undefined;
    let m = streamingMetaMapRef.current.get(sid);
    if (!m) {
      m = {};
      streamingMetaMapRef.current.set(sid, m);
    }
    return m;
  };
  /** 在途会话集合（同步镜像，见 inflightSids 注释） */
  const inflightRef = useRef<Set<string>>(new Set());
  /** 任务执行聊天化：围观流式回复中的会话时，到达的执行会话跳转排队（只留最新一个）；
   *  执行本身不排队——后端已在新会话开跑 */
  const pendingExecRef = useRef<{ sid: string; title: string } | null>(null);
  /** 任务 id → 执行会话 id（chat-open-session 事件建立；invoke 收尾时按它刷新历史） */
  const execSessionByTaskRef = useRef<Map<string, string>>(new Map());
  const enterChat = (sid: string) => {
    inflightRef.current.add(sid);
    setInflightSids(new Set(inflightRef.current));
  };
  const exitChat = (sid: string) => {
    inflightRef.current.delete(sid);
    setInflightSids(new Set(inflightRef.current));
    // 全部会话回复结束 → 兑现排队中的执行会话跳转提示（不打断刚结束的对话）
    if (inflightRef.current.size === 0) {
      const pending = pendingExecRef.current;
      if (pending) {
        pendingExecRef.current = null;
        addHint(
          `${pending.title || "任务"}已在新会话执行，点击查看执行对话`,
          pending.sid
        );
      }
    }
  };
  /** 视图会话是否有在途回复（驱动停止键/placeholder/禁用态） */
  const viewedBusy = sessionId !== null && inflightSids.has(sessionId);
  /** 本地提示消息：只显示不持久化（不污染上下文） */
  const addHint = (content: string, actionSessionId?: string) => {
    setMessages((prev) => [...prev, { role: "assistant", content, actionSessionId }]);
  };
  /** sessionId 镜像：异步收尾时判断会话是否已切换（竞态防护） */
  const sessionIdRef = useRef<string | null>(null);
  useEffect(() => {
    sessionIdRef.current = sessionId;
  }, [sessionId]);

  // SUBA-3：子 agent 收尾事件——派发者主会话里轻提示（围观过滤沿用 PAR-1 的
  // sessionId 流式隔离；这里补「后台子任务完成」的可见性）。
  // 复用 useTauriListen（OCR r1 采纳：与手写 listen 生命周期等价且更稳）。
  const subagentStatusLabel: Record<string, string> = {
    succeeded: "已完成",
    failed: "失败",
    cancelled: "已取消",
    budget_exceeded: "预算耗尽",
  };
  useTauriListen<{
    subagentId: string;
    parentSessionId: string | null;
    status: string;
    summary: string;
  }>("subagent-finished", (payload) => {
    const parent = payload.parentSessionId;
    if (parent && parent === sessionIdRef.current) {
      const label = subagentStatusLabel[payload.status] ?? payload.status;
      addHint(`子任务${label}：${payload.summary || payload.subagentId}`);
    }
  });

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
  useOutsideClose(sessionMenuOpen, () => setSessionMenuOpen(false), menuRef, sessionDropdownRef);
  // 点击模型下拉外关闭（镜像会话菜单：按钮与下拉不在同一 ref 容器，双 ref 检测）
  useOutsideClose(modelMenuOpen, () => setModelMenuOpen(false), modelBtnRef, modelDropdownRef);
  // 点击推理强度下拉外关闭（镜像模型菜单：双 ref 检测）
  useOutsideClose(effortMenuOpen, () => setEffortMenuOpen(false), effortBtnRef, effortDropdownRef);

  // 挂载：读当前模型配置，输入卡底栏 🧠 下拉展示当前协议的模型列表 + active id；
  // 设置页/另一窗口保存配置后广播 bot-config-changed，这里同步刷新
  useEffect(() => {
    const reload = () =>
      invoke<{
        baseUrl?: string;
        model?: string;
        apiProvider?: string | null;
        modelsByProvider?: {
          openai?: ChatModelEntry[];
          anthropic?: ChatModelEntry[];
        } | null;
        activeModelId?: { openai: string | null; anthropic: string | null } | null;
        /** 被禁用的厂商名列表：其模型不进 🧠 下拉 */
        disabledVendors?: string[] | null;
        /** 连接测试通过的厂商名单：带 vendor 的模型仅当厂商已验证才进下拉 */
        verifiedVendors?: string[] | null;
        reasoningEffort?: string | null;
        /** 授权模式（U3b 只读展示）：strict/ask/yolo，None = ask。
         *  ⚠️ BotConfigView 序列化为 camelCase（rename_all），线字段是 permMode */
        permMode?: string | null;
        /** P4 水位条：模型条目 contextK（千 token）随 modelsByProvider 条目透传 */
      }>("bot_get_config")
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
          // MP-02：双协议同列——两组模型都列出（带协议分组标签），
          // 跨协议选中由后端 apply_active_model_switch 连协议一起切
          setModels([
            ...(mbp.openai ?? []).filter(visible).map((m) => ({ ...m, provider: "openai" as const })),
            ...(mbp.anthropic ?? []).filter(visible).map((m) => ({
              ...m,
              provider: "anthropic" as const,
            })),
          ]);
          setActiveModelId(
            (c.activeModelId ?? { openai: null, anthropic: null })[
              isOpenai ? "openai" : "anthropic"
            ] ?? null
          );
          // RE-1 推理强度后台默认：缺字段/非法值回 medium（与后端 from_cfg 一致）。
          // 只刷新基线——已选过覆盖的会话保持覆盖（验收 3），没覆盖过的即时跟随新默认
          const raw = c.reasoningEffort;
          setEffortBase(
            raw === "off" || raw === "low" || raw === "high" ? raw : "medium"
          );
          // 授权模式只读展示（U3b）：非法/缺省回 ask（与后端 PermMode::from_cfg 一致）
          const pm = c.permMode;
          setPermMode(
            pm === "strict" || pm === "yolo" || pm === "auto" ? pm : "ask"
          );
        })
        .catch(() => setModelLabel("未配置"));
    reload();
    const un = listen("bot-config-changed", reload);
    return () => {
      un.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 🧠 下拉选中：走窄口径命令（后端读盘最新配置，只动 active + 派生老字段，
  // 不做整份写回——避免旧快照覆盖设置页并发修改）。成功后广播 bot-config-changed，
  // 另一窗口（主窗口 ↔ 挂件）的 ChatPanel 靠既有 listener 刷新
  const switchModel = async (id: string) => {
    try {
      const c = await invoke<{
        model?: string;
        apiProvider?: string | null;
        activeModelId?: { openai: string | null; anthropic: string | null } | null;
      }>("bot_set_active_model", { modelId: id });
      setModelLabel(c.model || "未配置");
      // MP-02：跨协议选中时后端已连协议一起切，这里同步本地协议态
      const isOpenai = (c.apiProvider ?? "openai") !== "anthropic";
      setApiProvider(isOpenai ? "openai" : "anthropic");
      setActiveModelId(
        (c.activeModelId ?? { openai: null, anthropic: null })[
          isOpenai ? "openai" : "anthropic"
        ] ?? null
      );
      setModelMenuOpen(false);
      emit("bot-config-changed", null).catch(() => {});
    } catch (e) {
      handleCommandError(e, "切换模型");
    }
  };

  // 流式增量：追加到最后一条 streaming 中的助手消息
  // 会话过滤：六个流式事件 payload 均带 sessionId
  // （交互实例专属；后台任务不推流），只消费属于当前会话的增量，
  // 否则两个会话并行跑时输出会互相串台
  //
  // 16ms 小批量合并（rAF；无 rAF 的环境退化为同长定时器）：SSE 每个 chunk 一个事件，
  // 逐条 setMessages 会在一帧内做多次「整数组拷贝 + 重渲染」。合并后一帧最多写一次，
  // 观感不变而 CPU 更稳。
  // **只延迟不丢**：同一窗口内的片段按到达顺序拼接后一次性写入，不会少字/错序；
  // 另有两层兜底——正文最终内容由 bot_chat 返回值 full.text 在收尾时整体覆盖，
  // thinking 的权威副本是各会话的 streamingMetaMapRef 条目（事件到达即累加，不受缓冲影响）。
  useEffect(() => {
    const pending = { text: "", think: false };
    let frame: number | null = null;
    const rafOk = typeof requestAnimationFrame === "function";
    const flush = () => {
      frame = null;
      const add = pending.text;
      const thinkTouched = pending.think;
      pending.text = "";
      pending.think = false;
      if (!add && !thinkTouched) return;
      setMessages((prev) => {
        const last = prev[prev.length - 1];
        if (!last || !last.streaming) return prev;
        const copy = [...prev];
        copy[copy.length - 1] = {
          ...last,
          content: add ? last.content + add : last.content,
          thinking: thinkTouched
            ? streamingMetaMapRef.current.get(sessionIdRef.current ?? "")?.thinking
            : last.thinking,
        };
        return copy;
      });
    };
    const schedule = () => {
      if (frame !== null) return;
      frame = rafOk ? requestAnimationFrame(flush) : window.setTimeout(flush, DELTA_BATCH_MS);
    };
    const unlisten = listen<{ text?: string; sessionId?: string | null }>(
      "bot-chat-delta",
      (e) => {
        const sid = e.payload?.sessionId ?? null;
        if (sid !== sessionIdRef.current) return;
        const t = e.payload?.text;
        if (!t) return;
        pending.text += t;
        schedule();
      }
    );
    const unThink = listen<{ text?: string; sessionId?: string | null }>(
      "bot-think-delta",
      (e) => {
        const sid = e.payload?.sessionId ?? null;
        const t = e.payload?.text;
        if (!t) return;
        // 权威副本按「回复所属会话」累加（SWITCH-1：不随视图切换丢失），
        // 仅展示走同帧合并且只进当前视图
        if ((sid !== null && inflightRef.current.has(sid)) || sid === sessionIdRef.current) {
          const m = metaFor(sid);
          if (m) m.thinking = (m.thinking ?? "") + t;
        }
        if (sid !== sessionIdRef.current) return;
        pending.think = true;
        schedule();
      }
    );
    const unTool = listen<{ id?: string; name?: string; sessionId?: string | null }>(
      "bot-tool",
      (e) => {
        const sid = e.payload?.sessionId ?? null;
        // 元数据按回复所属会话累加（SWITCH-1）；视觉更新仅当前视图
        if (sid === null) return;
        if (sid !== sessionIdRef.current && !inflightRef.current.has(sid)) return;
        const { id, name } = e.payload ?? {};
        if (!id) return;
        const m = metaFor(sid);
        if (!m) return;
        const tools = [...(m.tools ?? [])];
        if (!tools.some((x) => x.id === id)) tools.push({ id, name: name ?? "" });
        m.tools = tools;
        if (sid !== sessionIdRef.current) return;
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
        if (sid === null) return;
        if (sid !== sessionIdRef.current && !inflightRef.current.has(sid)) return;
        const { id, name } = e.payload ?? {};
        if (!id || !name) return;
        const m = metaFor(sid);
        if (!m) return;
        const tools = (m.tools ?? []).map((x) =>
          x.id === id ? { ...x, name } : x
        );
        m.tools = tools;
        if (sid !== sessionIdRef.current) return;
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
      result?: string | null;
      ms?: number | null;
      ok?: boolean;
      sessionId?: string | null;
    }>("bot-tool-done", (e) => {
      const sid = e.payload?.sessionId ?? null;
      if (sid === null) return;
      if (sid !== sessionIdRef.current && !inflightRef.current.has(sid)) return;
      const { id, name, args, result, ms, ok } = e.payload ?? {};
      if (!id) return;
      const m = metaFor(sid);
      if (!m) return;
      const tools = (m.tools ?? []).map((x) =>
        x.id === id
          ? { ...x, name: name ?? x.name, args, done: true, result: result ?? undefined, ms: ms ?? undefined, ok }
          : x
      );
      m.tools = tools;
      if (sid !== sessionIdRef.current) return;
      setMessages((prev) => {
        const last = prev[prev.length - 1];
        if (!last || !last.streaming) return prev;
        const copy = [...prev];
        copy[copy.length - 1] = { ...last, tools };
        return copy;
      });
    });
    // P2-b 结构化文件变更：dispatch 在有 trace 的执行会话里 emit（全窗口）——
    // 每次落盘修改一条，按会话累积进 meta，收尾并入最终消息给 FileSummary 用。
    // 主聊天（无 trace）不产生事件 → FileSummary 走正则兜底，行为不变。
    const unFileChanged = listen<{
      path?: string;
      kind?: "create" | "modify" | "delete";
      added?: number;
      deleted?: number;
      sessionId?: string | null;
    }>("bot-file-changed", (e) => {
      const sid = e.payload?.sessionId ?? null;
      if (sid === null) return;
      if (sid !== sessionIdRef.current && !inflightRef.current.has(sid)) return;
      const p = e.payload ?? {};
      if (!p.path) return;
      const m = metaFor(sid);
      if (!m) return;
      const changes = [
        ...(m.fileChanges ?? []),
        {
          path: p.path,
          kind: p.kind ?? "modify",
          added: p.added ?? 0,
          deleted: p.deleted ?? 0,
        },
      ];
      m.fileChanges = changes;
      if (sid !== sessionIdRef.current) return;
      setMessages((prev) => {
        const last = prev[prev.length - 1];
        if (!last || !last.streaming) return prev;
        const copy = [...prev];
        copy[copy.length - 1] = { ...last, fileChanges: changes };
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
      if (sid === null) return;
      if (sid !== sessionIdRef.current && !inflightRef.current.has(sid)) return;
      const p = e.payload ?? {};
      if (!p.skillName) return;
      const failure: SkillFailure = {
        skillName: p.skillName,
        reason: p.reason ?? "(无原因)",
        completedSummary: p.completedSummary ?? "(无已完成步骤)",
        rollbackAttempted: !!p.rollbackAttempted,
      };
      const m = metaFor(sid);
      if (!m) return;
      m.skillFailure = failure;
      if (sid !== sessionIdRef.current) return;
      setMessages((prev) => {
        const last = prev[prev.length - 1];
        if (!last || !last.streaming) return prev;
        const copy = [...prev];
        copy[copy.length - 1] = { ...last, skillFailure: failure };
        return copy;
      });
    });
    // P4 水位条数据源：Anthropic 回合级 usage 按会话累计（当前视图 + 在途会话，
    // 与流式事件同过滤口径——切走的会话继续累计，切回可见）
    const unUsage = listen<{
      inputTokens?: number;
      outputTokens?: number;
      sessionId?: string | null;
    }>("bot-usage-delta", (e) => {
      const sid = e.payload?.sessionId ?? null;
      if (sid === null) return;
      if (sid !== sessionIdRef.current && !inflightRef.current.has(sid)) return;
      const i = e.payload?.inputTokens ?? 0;
      const o = e.payload?.outputTokens ?? 0;
      if (i <= 0 && o <= 0) return;
      setSessionUsage((prev) => {
        const cur = prev[sid] ?? { input: 0, output: 0, lastInput: 0 };
        return {
          ...prev,
          [sid]: { input: cur.input + i, output: cur.output + o, lastInput: i },
        };
      });
    });
    return () => {
      if (frame !== null) {
        if (rafOk) cancelAnimationFrame(frame);
        else window.clearTimeout(frame);
        frame = null;
      }
      unlisten.then((f) => f());
      unThink.then((f) => f());
      unTool.then((f) => f());
      unToolName.then((f) => f());
      unToolDone.then((f) => f());
      unFileChanged.then((f) => f());
      unUsage.then((f) => f());
      unSkillFailed.then((f) => f());
    };
  }, []);

  // 围观中的执行会话 sid；执行收尾（execute-task 的 history_load 完成）后清除
  const execWatchRef = useRef<string | null>(null);
  /** 切到执行会话围观：加载已落库历史 + 末尾 streaming 占位气泡承接后续流式增量。
   * watch=false（收尾后迟到的跳转）：只读查看不挂围观守卫，输入立即可用 */
  const openExecSession = async (sid: string, watch = true) => {
    setSessionMenuOpen(false);
    setSessionId(sid);
    // 镜像同步落（effect 渲染后才补）：await 期间守卫读到的必须已是新会话，
    // 否则快响应会被「过期」守卫误丢
    sessionIdRef.current = sid;
    streamingMetaMapRef.current.set(sid, {});
    // 围观守卫（拍板 #22=B）：记录当前围观的执行会话——执行期间拦 Send
    //（防用户输入与执行响应交错 + 被收尾 history_load 冲掉）；切换会话自由。
    // watch=false 的迟到查看不挂守卫：执行已结束，没有需要隔离的响应流
    if (watch) execWatchRef.current = sid;
    try {
      const rows = await invoke<Parameters<typeof rowsToMsgs>[0]>(
        "bot_history_load",
        { sessionId: sid }
      );
      // 围观期间用户已切走：丢弃过期历史，防旧会话消息刷进当前视图
      if (sessionIdRef.current !== sid) return;
      setMessages([
        ...rowsToMsgs(rows),
        { role: "assistant", content: "", streaming: true },
      ]);
    } catch (e) {
      handleCommandError(e, "bot_history_load", { silent: true });
    }
  };
  // openExecSession 经 ref 暴露给事件监听（避免闭包旧状态）
  const openExecSessionRef = useRef<(sid: string, watch?: boolean) => void>(() => {});
  // latest-ref 模式：事件监听闭包要调到最新一帧实现（React 官方推荐转发法；
  // render 期写 ref 为既有语义，拆分批不改时机）
  // oxlint-disable-next-line react/refs
  openExecSessionRef.current = (sid, watch) => {
    openExecSession(sid, watch);
  };

  // 任务卡交给机器人执行（execute-task 事件：主窗口/挂件卡片 🤖 按钮触发）
  // 任务执行聊天化：不再在当前会话执行——后端建新会话跑（run_task_in_chat），
  // 前端收 chat-open-session 切过去围观。执行本身不吃 busy 锁（会话隔离天然并发），
  // 只有「自动跳转查看」在围观流式回复时排队（exitChat 清空在途时 hint 提示）。
  useEffect(() => {
    // 去重表在模块级 execTaskDedup（泄漏的监听器实例间共享才有效，见文件头注释）
    const unExec = listen<{ id?: string; title?: string }>("execute-task", (e) => {
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
            ? invoke<Parameters<typeof rowsToMsgs>[0]>("bot_history_load", { sessionId: sid })
            : null;
          if (load) {
            load
              .then((rows) => {
                if (sessionIdRef.current === sid) setMessages(rowsToMsgs(rows));
              })
              .catch(() => {});
            // 独立订阅清守卫：先 catch 再 finally，避免 finally 链 unhandled rejection
            load.catch(() => {}).finally(() => {
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
            `${isCommandError(err) && err.code === "TASK_INVALID_STATE" ? "⏳" : "⚠️"} ${formatCommandError(err)}`
          );
          // 执行失败同样解除围观守卫（chat-open-session 已置值、.then 不会跑，
          // 不清则该会话被永久拦 Send）
          const sid = execSessionByTaskRef.current.get(id);
          execSessionByTaskRef.current.delete(id);
          if (sid && execWatchRef.current === sid) execWatchRef.current = null;
        });
    });
    return () => {
      unExec.then((f) => f());
    };
  }, []);

  // chat-open-session：后端新建执行会话后广播——
  // 非 busy 直接切过去围观（流式增量按 sessionId 过滤自动落到新会话的占位气泡）；
  // 围观流式回复中不打断，跳转排队，全部在途回复结束后弹「点击查看」提示。
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
      // 迟到防复活：execute-task 已收尾的任务不再以围观模式重开（重挂守卫会把
      // 已结束的会话永久拦输入）。拍板：成功 → 切过去只读查看；失败 → 不自动
      // 跳转，停下来让用户决定是否重试/查看（从会话列表点入不受拦）
      const outcome = taskId ? finishedExecTasks.get(taskId) : undefined;
      if (outcome) {
        if (outcome === "success") openExecSessionRef.current(sid, false);
        return;
      }
      if (inflightRef.current.has(sessionIdRef.current ?? "")) {
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

  const switchSession = useCallback(async (sid: string) => {
    // SWITCH-1/PAR-1：busy 期允许实时切换——回复仍按发起会话落库/流式路由（inflightRef），
    // 切走不影响它在后台完成
    if (sid === sessionId) {
      setSessionMenuOpen(false);
      return;
    }
    setSessionId(sid);
    // 镜像同步落（同 openExecSession）：守卫在 await 之后读 ref，不能等渲染
    sessionIdRef.current = sid;
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
      // 等待期间用户已切到别的会话：丢弃过期响应（后到的慢响应不得盖掉新会话的加载结果）
      if (sessionIdRef.current !== sid) return;
      let msgs = rowsToMsgs(rows);
      // 切回正在回复的会话：补一个流式占位气泡承接后续增量（已错过的增量段
      // 由收尾 full.text 整体校正，不会串进其他会话）
      if (inflightRef.current.has(sid)) {
        msgs = [...msgs, { role: "assistant", content: "", streaming: true }];
      }
      setMessages(msgs);
    } catch (e) {
      handleCommandError(e, "bot_history_load", { silent: true });
    }
  }, [sessionId]);

  const newSession = async () => {
    // SWITCH-1：busy 期允许新建对话（进行中的回复继续落在原会话）
    try {
      const s = await invoke<Session>("bot_session_create", { title: null });
      setSessions((prev) => [s, ...prev]);
      setSessionMenuOpen(false);
      setSessionId(s.id);
      setMessages([]);
      // DRAFT-1：不再清 input——草稿按会话隔离，新会话天然是空的，
      // 原会话的草稿切回去还在（旧实现的全局清空会误删原会话草稿）
    } catch (e) {
      handleCommandError(e, "bot_session_create", { silent: true });
    }
  };

  // ⌘K 跨窗口跳会话（U2 命令面板）：主窗口发 chat-focus-session，复用 switchSession
  // 走完整切换（清消息 + bot_history_load + inflight 占位），仅 setSessionId 不会加载历史；
  // 面板展开由 WidgetApp 负责。ref 写放 effect（render 期写 ref 会新增 lint warn）
  const switchSessionRef = useRef<(sid: string) => void>(() => {});
  useEffect(() => {
    switchSessionRef.current = switchSession;
  });
  useTauriListen<{ sessionId?: string }>("chat-focus-session", (payload) => {
    if (payload.sessionId) switchSessionRef.current(payload.sessionId);
  });

  const deleteSession = async (sid: string) => {
    // SWITCH-1：只拦「删除正在回复的会话」（回复落库目标不能被删）；其他会话随便删
    if (inflightRef.current.has(sid)) {
      addHint("⏳ 该对话正在回复中，完成后才能删除");
      return;
    }
    const target = sessions.find((s) => s.id === sid);
    if (!window.confirm(`删除对话「${target?.title ?? "未命名"}」？消息记录一并删除。`)) return;
    try {
      await invoke("bot_session_delete", { id: sid });
      const rest = sessions.filter((s) => s.id !== sid);
      setSessions(rest);
      // DRAFT-1：被删会话的草稿/附件/推理强度覆盖一并清掉（内存 Map 防积攒）
      setDrafts((prev) => {
        const next = new Map(prev);
        next.delete(sid);
        return next;
      });
      setFilesBySession((prev) => {
        const next = new Map(prev);
        next.delete(sid);
        return next;
      });
      setEffortBySession((prev) => {
        const next = new Map(prev);
        next.delete(sid);
        return next;
      });
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
    if (!sid || inflightRef.current.has(sid)) return;
    enterChat(sid);
    setMessages([...history, { role: "assistant", content: "", streaming: true }]);
    streamingMetaMapRef.current.set(sid, {});
    // RE-1 推理强度：生效档位 = 本会话覆盖 ?? 后台默认（具体值传后端，
    // 覆盖本身只存前端内存，不回写 bot-config.json）
    const effort = effortBySession.get(sid) ?? effortBase;
    try {
      const full = await invoke<{ text: string; taskRefs?: TaskRef[] }>(
        "bot_chat",
        {
          messages: history.map((m) => ({ role: m.role, content: m.content })),
          sessionId: sid,
          reasoningEffort: effort,
        }
      );
      // 把流式过程中累积的思考/工具行并入最终消息
      const meta = streamingMetaMapRef.current.get(sid) ?? {};
      streamingMetaMapRef.current.delete(sid);
      const done: Msg[] = [
        ...history,
        {
          role: "assistant",
          content: full.text || "",
          refs: full.taskRefs ?? [],
          thinking: meta.thinking,
          tools: meta.tools,
          fileChanges: meta.fileChanges,
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
      exitChat(sid);
    }
  };

  // 斜杠快捷命令（/stop 在机器人回复中也能生效）
  const runSlashCommand = async (raw: string): Promise<boolean> => {
    const cmd = raw.split(/\s+/)[0].toLowerCase();
    if (cmd === "/stop") {
      setInput("");
      // PAR-1：/stop 停「当前视图会话」的在途回复（并行回复按会话隔离）
      if (inflightRef.current.has(sessionIdRef.current ?? "")) {
        if (isSubagentSession) {
          // SUBA-3：子 agent 会话 → cancel_subagent（状态机置 cancelled 并硬停）
          invoke("cancel_subagent", { key: sessionIdRef.current })
            .catch((e) => handleCommandError(e, "cancel_subagent", { silent: true }))
            .finally(() => invoke("bot_stop", { sessionId: sessionIdRef.current }).catch(() => {}));
        } else {
          invoke("bot_stop", { sessionId: sessionIdRef.current }).catch((e) =>
            handleCommandError(e, "bot_stop", { silent: true })
          );
        }
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
        handleCommandError(e, "bot_history_clear", { silent: true })
      );
      return true;
    }
    if (cmd === "/compact") {
      setInput("");
      if (inflightRef.current.has(sessionIdRef.current ?? "") || !sessionId) return true;
      const sid = sessionId;
      const history = messages.filter((m) => !m.streaming && m.content.trim());
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
        const note: Msg = { role: "assistant", content: `⚠️ 压缩失败：${formatCommandError(e)}` };
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
      if (inflightRef.current.has(sessionIdRef.current ?? "") || !sessionId) return true;
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
    // 围观执行会话期拦发送——含 /retry /clean /compact 等斜杠命令（runChat 同样
    // 会往执行中的会话发消息，ChatGuard 只软拒不防交错）；/stop 例外：停执行是
    // 围观期的合法操作（拍板 #22=B）
    if (execWatchRef.current !== null && execWatchRef.current === sessionId && text !== "/stop") {
      addHint("⏳ 执行进行中，围观模式暂不能发送");
      return;
    }
    // 斜杠命令优先处理（/stop 在 busy 时也能生效）；未知斜杠文本当普通消息发
    if (text.startsWith("/")) {
      if (await runSlashCommand(text)) return;
    }
    if (!text && !files.length) return;
    // PAR-1：并行回复按会话隔离——本会话在途时拦重复发送（其他会话可自由发送）
    if (sessionId !== null && inflightRef.current.has(sessionId)) {
      addHint("⏳ 本对话正在回复中：可切换到其他对话发送，或输入 /stop 停止本条");
      return;
    }
    // 同步检查：state 重渲染前连续两次 Enter 也能拦下重复发送
    if (!sessionId) return;
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
  // 拖放回调只在挂载时注册一次，闭包会捕获 mount 时的 addFiles（其 draftKey
  // 彼时 sessionId 还是 null）——用 latest ref 指到最新实现，落点才是当前会话
  const addFilesRef = useRef(addFiles);
  // latest-ref 模式：同 openExecSessionRef（render 期写 ref 为既有语义，拆分批不改时机）
  // oxlint-disable-next-line react/refs
  addFilesRef.current = addFiles;

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
            if (await insideChat(p.position)) addFilesRef.current(p.paths);
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

  // 逐条复制回复内容（按钮反馈在消息下方）；useCallback 固定句柄供 MsgBubble memo
  const copyMessage = useCallback(async (idx: number, content: string) => {
    try {
      await navigator.clipboard.writeText(content);
      setCopiedIdx(idx);
      setTimeout(() => setCopiedIdx((c) => (c === idx ? null : c)), 1500);
    } catch (e) {
      handleCommandError(e, "clipboard", { silent: true });
    }
  }, []);

  // 移除一条回复：不满意时删掉，不再作为上下文发给模型（全量持久化）。
  // 句柄必须稳定（MsgBubble memo 依赖）：messages 走 latest ref——流式期间
  // messages 每帧换引用，进 deps 会每帧重建回调、击穿全部气泡的 memo 边界
  const messagesRef = useRef<Msg[]>([]);
  useEffect(() => {
    messagesRef.current = messages;
  }, [messages]);
  const removeMessage = useCallback((idx: number) => {
    const sid = sessionIdRef.current;
    if (!sid || inflightRef.current.has(sid)) return;
    const next = messagesRef.current.filter((_, j) => j !== idx);
    setMessages(next);
    persistHistory(sid, next);
  }, []);

  // 点任务引用按钮：通知主窗口打开该任务编辑态 + 唤起主窗口并强制置顶
  // （与挂件双击标题同一规则：主窗口要出现在桌面屏幕最顶层）
  const openTaskInMain = useCallback(async (ref: TaskRef) => {
    emit("edit-task", { id: ref.id }).catch(() => {});
    invoke("focus_main_window").catch(() => {});
  }, []);

  const currentSession = sessions.find((s) => s.id === sessionId);
  const currentTitle = currentSession?.title ?? "新对话";
  // SUBA-3：视图会话是子 agent 执行会话——runner 建会话时写
  // bot_sessions.is_subagent 结构化标记（U20D），停止按钮/斜杠 /stop 走
  // cancel_subagent（与任务卡停止按钮同 API），而非 bot_stop。
  // 数据清空后无旧前缀会话，无需标题兜底（U20D-A）
  const isSubagentSession = currentSession?.isSubagent === true;

  // 停止键点击（发送/停止一体键的 busy 分支）：子 agent 会话走 cancel_subagent，
  // 普通会话走 bot_stop——与 /stop 斜杠命令同一套分派
  const stopCurrent = useCallback(() => {
    if (isSubagentSession) {
      // SUBA-3：子 agent 会话停止键 = cancel_subagent（与任务卡按钮同 API）
      invoke("cancel_subagent", { key: sessionIdRef.current })
        .catch((e) =>
          handleCommandError(e, "cancel_subagent", { silent: true })
        )
        .finally(() =>
          invoke("bot_stop", { sessionId: sessionIdRef.current }).catch(() => {})
        );
    } else {
      invoke("bot_stop", { sessionId: sessionIdRef.current }).catch((e) =>
        handleCommandError(e, "bot_stop", { silent: true })
      );
    }
  }, [isSubagentSession]);

  // RE-1 推理强度：底栏按钮显示的生效档位 = 本会话覆盖 ?? 后台默认；
  // effortIsOverridden 决定要不要缀「·默认」角标
  const effectiveEffort: ReasoningLevel =
    (sessionId != null ? effortBySession.get(sessionId) : undefined) ??
    effortBase;
  const effortIsOverridden = sessionId != null && effortBySession.has(sessionId);
  /** 选档：写入会话覆盖 Map（内存）；选「后台默认」= 清除覆盖跟随后台 */
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
      <SessionList
        sessions={sessions}
        sessionId={sessionId}
        currentTitle={currentTitle}
        sessionMenuOpen={sessionMenuOpen}
        onToggleMenu={() => setSessionMenuOpen((v) => !v)}
        topBarRef={topBarRef}
        menuRef={menuRef}
        dropdownRef={sessionDropdownRef}
        dropdownTop={dropdownTop}
        selecting={selecting}
        onToggleSelecting={onToggleSelecting}
        onSelect={switchSession}
        onNew={newSession}
        onDelete={deleteSession}
      />

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
              <span className="truncate max-w-[280px]">
                <Pin size={10} aria-hidden className="inline-block align-[-1px]" /> {t.title}
              </span>
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
      {/* P4 上下文水位条（Anthropic 协议回合级 usage 累计；contextK 来自 active 模型条目） */}
      <UsageMeter
        input={sessionUsage[sessionId ?? ""]?.input ?? 0}
        output={sessionUsage[sessionId ?? ""]?.output ?? 0}
        lastInput={sessionUsage[sessionId ?? ""]?.lastInput ?? 0}
        contextK={models.find((m) => m.id === activeModelId)?.contextK}
      />

      <MessageList
        scrollRef={scrollRef}
        messages={messages}
        viewedBusy={viewedBusy}
        copiedIdx={copiedIdx}
        verboseLevel={verboseLevel}
        onCopy={copyMessage}
        onRemove={removeMessage}
        onOpenTask={openTaskInMain}
        onOpenExecSession={switchSession}
      />
      <InputArea
        verbose={{ level: verboseLevel, setLevel: setVerboseLevel }}
        input={input}
        setInput={setInput}
        files={files}
        setFiles={setFiles}
        taRef={taRef}
        slashIdx={slashIdx}
        setSlashIdx={setSlashIdx}
        slashDismissed={slashDismissed}
        setSlashDismissed={setSlashDismissed}
        send={send}
        pickFiles={pickFiles}
        onStop={stopCurrent}
        viewedBusy={viewedBusy}
        selecting={selecting}
        isSubagentSession={isSubagentSession}
        permMode={permMode}
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
  );
}
