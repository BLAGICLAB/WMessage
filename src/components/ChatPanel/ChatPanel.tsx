// ChatPanel 主组件（orchestrator）。
// 1862 → 拆分：SessionList / MessageList / InputArea / useChatUi 到同目录子文件
// 行为等价拆分——消息数据流/斜杠命令语义/Tauri 命令调用零改动），
// 本文件保留所有 useState / useRef / useEffect / handlers / 数据流监听 / 装配 JSX。
// MsgBubble 的 React.memo 流式性能边界见 MessageList.tsx 头注释。
//
// 公开 import 路径保持稳定：外部仍 `import { ChatPanel } from "./components/ChatPanel"`，
// Vite 解析到 `./ChatPanel/index.tsx` → 透传 `./ChatPanel`。

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, emit } from "@tauri-apps/api/event";
import {
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
} from "lucide-react";
import { unlistenSafe, useTauriListen } from "../../lib/useTauriListen";
import { getBotConfig } from "../../lib/botConfig";
import type { Task } from "../../types";
import { useConfirm } from "../../lib/useConfirm";
import { getCurrentWindow } from "@tauri-apps/api/window";

import {
  handleCommandError,
  formatCommandError,
  isCommandError,
} from "../../lib/errorHandler";

import type {
  ChatModelEntry,
  ModelItem,
  Msg,
  PermMode,
  ReasoningLevel,
  Session,
  SkillFailure,
  SubagentRow,
  TaskRef,
  ToolCall,
  TurnStatus,
  VerboseLevel,
} from "./types";
import { DELTA_BATCH_MS, execTaskDedup, finishedExecTasks, SUBAGENT_POLL_MS, SUBAGENT_STATUS } from "./constants";
import {
  EMPTY_DRAFT,
  readChatDrafts,
  writeChatDrafts,
  type ChatDraft,
} from "./chatDrafts";
import { useAutoGrow, useOutsideClose } from "./useChatUi";
import * as streamState from "./streamState";
import { SessionRail } from "./SessionRail";
import { ContextRail, type ContextTab, type SubagentRun } from "./ContextRail";
import { MessageList } from "./MessageList";
import { UsageMeter } from "./UsageMeter";
import { InputArea } from "./InputArea";
import { TracePanel } from "../TracePanel";

/** 子 agent 行里的契约/结果字段都是 JSON 字符串，详情视图要结构化渲染。
 *  解析失败或形状对不上（验收标准不是字符串数组、结果不是对象）都落回原文——
 *  原文本身可能就是排查线索，吞掉等于把「子 agent 到底写了什么」变成无法回答。 */
function parseJsonField(
  raw?: string | null,
): string | string[] | Record<string, unknown> | null {
  if (!raw) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return raw;
  }
  if (Array.isArray(parsed)) {
    return parsed.every((x) => typeof x === "string")
      ? (parsed as string[])
      : raw;
  }
  if (typeof parsed === "object" && parsed !== null) {
    return parsed as Record<string, unknown>;
  }
  return raw;
}

/** 回合状态取值域（与后端 TurnStatus::as_str 同源）。
 *  库里出现未知值按缺失处理：旧版本写入/库被手改都不能让整段会话加载炸掉 */
const TURN_STATUS_VALUES: readonly string[] = [
  "running",
  "settling",
  "completed",
  "interrupted",
  "failed",
];
const parseTurnStatus = (raw?: string | null): TurnStatus | undefined =>
  raw && TURN_STATUS_VALUES.includes(raw) ? (raw as TurnStatus) : undefined;

/** 贴底阈值（px）：离底这么近就算「还贴着底」。
 *  流式追加让内容高度每帧变，差几像素就当没贴底 → 视口在底部反复抽搐。 */
const STICK_BOTTOM_PX = 32;

/** 模型下拉条目集合协议：types.ModelItem 的来源协议收窄用 */
type Props = {
  /** 机器人是否开启（主窗口读 bot_get_enabled + bot-changed 广播）。   *  false 时跳过会话加载/历史恢复与所有对外 invoke，容器仍挂载——主窗口里
   *  聊天页与其它页面靠 hidden 切换可见性，卸载会连带丢掉 messages 与全部监听器 */
  enabled: boolean;
  /** 右侧上下文栏的任务池（App 侧已过滤归档/删除/他人任务） */
  tasks: Task[];
  /** 勾完成等任务改写（走 App 的 updateTask，后端广播 tasks-updated） */
  onUpdateTask: (taskId: string, patch: Partial<Task>) => void;
};

/** 机器人聊天区：主窗口左导航「Agent 能力 → 聊天」整页；支持多会话 */
export function ChatPanel({ enabled, tasks, onUpdateTask }: Props) {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [messages, setMessages] = useState<Msg[]>([]);
  // 输入窗口锁定对话（DRAFT-1）：草稿/附件按会话隔离——切走再切回各自保留，
  // 会话 A 打的字切到 B 不会误发给 B。input/files/refs 是「当前会话」的派生值，
  // setInput/setFiles/setRefs 伪装成 React setter，原有调用点（onChange/send/斜杠补全/
  // 发送清空/附件增删）语义不变。
  //
  // 落脚点在组件外（chatDrafts.ts，localStorage）：设置页在 App 里是提前 return，
  // 主树整体被替换 → 聊天面板卸载 → 草稿放组件 state 里就归零（切一次设置，
  // 输入框里的字就没了）。现在 state 仍是用起来的那一份，写下去/读回来交给 chatDrafts。
  const [drafts, setDrafts] = useState<Map<string, ChatDraft>>(() =>
    readChatDrafts(),
  );
  const draftKey = sessionId ?? "";
  const draft = drafts.get(draftKey) ?? EMPTY_DRAFT;
  /** 改某会话的草稿（函数式更新：拿最新一份，不是渲染时的快照） */
  const patchDraft = (key: string, updater: (cur: ChatDraft) => ChatDraft) =>
    setDrafts((prev) => {
      const next = new Map(prev);
      next.set(key, updater(prev.get(key) ?? EMPTY_DRAFT));
      return next;
    });
  const input = draft.text;
  const setInput = (v: string) =>
    patchDraft(draftKey, (d) => ({ ...d, text: v }));
  /** 已添加的附件文件路径（➕ 或拖入，随消息一起发送）——按会话隔离（DRAFT-1） */
  const files = draft.files;
  const setFiles = (v: string[] | ((prev: string[]) => string[])) =>
    patchDraft(draftKey, (d) => ({
      ...d,
      files: typeof v === "function" ? v(d.files) : v,
    }));
  /** 待发任务引用（右侧栏点任务加入，随消息一起发送）——按会话隔离，同附件口径。
   *  语义是「把这张卡摆到对话里让机器人看着办」，不是「立刻让机器人去做」：
   *  后者是任务卡 🤖 的活（runWithBot），两者的时机与代价完全不同 */
  const refs = draft.refs;
  const setRefs = (v: TaskRef[] | ((prev: TaskRef[]) => TaskRef[])) =>
    patchDraft(draftKey, (d) => ({
      ...d,
      refs: typeof v === "function" ? v(d.refs) : v,
    }));
  /** 草稿落盘：改动后写一次（面板被卸载/重挂载后读回来，切设置不再丢输入） */
  useEffect(() => {
    writeChatDrafts(drafts);
  }, [drafts]);
  /** 输入卡 textarea 引用：自动增高用（UI-1 单行 input 升级为多行 textarea） */
  const taRef = useRef<HTMLTextAreaElement>(null);
  /** 斜杠命令 autocomplete 选中项下标（输入「/」浮出后用 ↑↓ / Tab / 点选） */
  const confirm = useConfirm();
  const [slashIdx, setSlashIdx] = useState(0);
  /** 斜杠命令 picker 被 Esc 关掉后，输入未变化前不再自动浮出 */
  const [slashDismissed, setSlashDismissed] = useState(false);
  /** PAR-1 并行回复：每个会话独立在途。在途集合的真源在模块 store
   *  （streamState），这里只是它给 UI 的镜像——事件监听与 UI 读同一份，
   *  不再是 ref 与 state 两份副本各自更新（同帧连按会分叉）。 */
  const [inflightSids, setInflightSids] = useState<ReadonlySet<string>>(
    () => new Set(streamState.inflightSessions()),
  );
  useEffect(
    () => streamState.subscribeInflight((sids) => setInflightSids(new Set(sids))),
    [],
  );
  /** 逐条复制按钮的反馈：记录当前“已复制”的消息下标 */
  const [copiedIdx, setCopiedIdx] = useState<number | null>(null);
  /** 停止反馈：bot_stop 回传的置位条数。
   *  放在消息流之外——回合收尾会整表替换消息数组，放在流里的提示会被那一下
   *  抹掉，用户根本来不及看。它是本地瞬态，不落库、不进模型上下文。 */
  const [stopNotice, setStopNotice] = useState<{
    sid: string;
    n: number;
  } | null>(null);
  /** 执行详情弹层：回合回传的 traceId + 所属会话（会话维度列出同会话的多次执行） */
  const [openTrace, setOpenTrace] = useState<{
    traceId: number;
    sessionId: string | null;
  } | null>(null);
  /** 右侧栏点任务 = 把它摆进当前消息（同附件）。与「立刻让机器人去做」
   *  是两回事：后者会在新会话里开跑，这里只是让机器人看着这张卡回答。
   *  重复点同一张卡不叠加——引用是集合不是列表。
   *  不包 useCallback：setRefs 每次渲染都是新函数（它按 draftKey 定位会话），
   *  空依赖的 useCallback 会把它固定在首帧那版，把引用写进错误的会话 key */
  const refTask = (t: TaskRef) => {
    setRefs((prev) =>
      prev.some((x) => x.id === t.id) ? prev : [...prev, t],
    );
  };
  /** 拖放悬停：文件拖到聊天区上时显示提示层 */
  const [dragHover, setDragHover] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  /** 贴底意图：用户往上翻了历史就别再抢视口（自动滚动处的说明） */
  const stickBottomRef = useRef(true);
  /** 上次的滚动位置：容器从隐藏态回来时浏览器会清零滚动口，用它还原 */
  const lastScrollTopRef = useRef(0);
  /** 上一个会话 id：换会话要把视口甩回底部（上一段的滚动位置不跟着带过来） */
  const lastSessionRef = useRef<string | null>(null);
  /** 聊天区根节点：拖放落点判定用（只接收落在聊天区矩形内的文件） */
  const rootRef = useRef<HTMLDivElement>(null);
  // 🧠 模型下拉：同会话菜单三件套——开合 + 按钮 ref + 下拉 ref，
  // 另带当前协议的模型列表与 active id（reload 里一并维护）
  const [modelMenuOpen, setModelMenuOpen] = useState(false);
  const [models, setModels] = useState<ModelItem[]>([]);
  // 当前协议：双协议同列展示后，active 高亮只认本协议的 active 模型
  const [apiProvider, setApiProvider] = useState<"openai" | "anthropic">(
    "openai",
  );
  const [activeModelId, setActiveModelId] = useState<string | null>(null);
  const modelBtnRef = useRef<HTMLButtonElement>(null);
  const modelDropdownRef = useRef<HTMLDivElement>(null);
  /** 输入卡底栏 🧠 标签：显示当前模型（设置页维护，聊天面板只读展示） */
  const [modelLabel, setModelLabel] = useState("…");
  // ⚡ 推理强度（RE-1）：后台默认基线（bot-config-changed 时刷新）+ 会话级覆盖。
  // 覆盖只存内存 Map，绝不回写 bot-config.json；会话切换后无覆盖的会话直接跟随后台。
  const [effortBase, setEffortBase] = useState<ReasoningLevel>("medium");
  const [effortBySession, setEffortBySession] = useState<
    Map<string, ReasoningLevel>
  >(new Map());
  const [effortMenuOpen, setEffortMenuOpen] = useState(false);
  const effortBtnRef = useRef<HTMLButtonElement>(null);
  const effortDropdownRef = useRef<HTMLDivElement>(null);
  /** 🛡 授权模式：全局档位（bot-config 的 permMode），输入卡底栏可直接改。
   *  改的是「机器人敢做多少事」——这个恰恰要在动手前一刻就地决定，
   *  让人跑去设置页等它跑起来再切是来不及的。 */
  const [permMode, setPermMode] = useState<PermMode>("ask");
  /** 授权档位的当前值（回滚要读它）：state 更新是异步的，回滚时不能靠读 state */
  const permModeRef = useRef<PermMode>("ask");
  const [permMenuOpen, setPermMenuOpen] = useState(false);
  const permBtnRef = useRef<HTMLButtonElement>(null);
  const permDropdownRef = useRef<HTMLDivElement>(null);
  /** 选定授权档：乐观更新 → 窄命令落盘。失败回滚并弹错——
   *  留着「界面显示已放行、实际没落盘」的假绿比报错更危险（用户会以为白名单外
   *  不会弹窗，实际还在弹）。副作用不写在 setState 的 updater 里（StrictMode 下
   *  updater 会跑两次，invoke 就会发两次）。 */
  const selectPermMode = useCallback((m: PermMode) => {
    setPermMenuOpen(false);
    const prev = permModeRef.current;
    if (prev === m) return;
    permModeRef.current = m;
    setPermMode(m);
    invoke("bot_set_perm_mode", { mode: m })
      .then(() => {})
      .catch((e) => {
        permModeRef.current = prev;
        setPermMode(prev);
        handleCommandError(e, "bot_set_perm_mode");
      });
  }, []);
  //  上下文水位条：bot-usage-delta 按会话累计（Anthropic 协议回合级 usage）。
  // lastInput = 最近一轮请求的 input（即当前上下文 prompt 体量）——每轮 input 是
  // 完整重发的 prompt，跨轮累加会随轮数近似平方膨胀，水位百分比必须用最近一轮。
  // 缓存两档独立累计（协议口径见 UsageMeter 头部：Anthropic 的 cache_read 不含在
  // input_tokens 里，折进 input 会与 trace 弹窗的分档展示对不上）
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
  //  执行过程详细度（verbose 三档）：localStorage 持久化，仅影响 ToolBadges 展示档位
  const [verboseLevel, setVerboseLevelState] = useState<VerboseLevel>(() => {
    const v = localStorage.getItem("chat-verbose-level");
    return v === "concise" || v === "debug" ? v : "detailed";
  });
  const setVerboseLevel = (l: VerboseLevel) => {
    setVerboseLevelState(l);
    localStorage.setItem("chat-verbose-level", l);
  };
  // 会话栏折叠态：localStorage 持久化（与 verboseLevel 同范式）。
  // 默认展开——侧栏的价值就在于「历史会话始终可见」，收起来只为窄窗/专注阅读
  const [railOpen, setRailOpen] = useState(
    () => localStorage.getItem("chat-rail-open") !== "0",
  );
  const toggleRail = () => {
    setRailOpen((v) => {
      localStorage.setItem("chat-rail-open", v ? "0" : "1");
      return !v;
    });
  };
  // 右侧上下文栏：默认收起。左侧栏已占 200px，再常驻 220px 会把 1200px 窗口的
  // 聊天正文挤到不足 500px——需要看任务/文件时按需展开
  const [ctxOpen, setCtxOpen] = useState(
    () => localStorage.getItem("chat-ctx-open") === "1",
  );
  /** 当前工作面（tasks / files / agents）。提到父级是为了让聊天内嵌的子 agent
   *  区域能驱动侧栏切到自己的类别——两处指同一块工作面 */
  const [ctxTab, setCtxTab] = useState<ContextTab>("tasks");
  /** 子 agent 内嵌区是否展开（折叠时只列在途，收起的执行记录不占对话空间） */
  const [subagentStripOpen, setSubagentStripOpen] = useState(false);
  const toggleCtx = () => {
    setCtxOpen((v) => {
      localStorage.setItem("chat-ctx-open", v ? "0" : "1");
      return !v;
    });
  };
  /** 侧栏子 agent 工作面当前钉住的详情条目（null = 停在列表页）。
   *  归属在这里而不是 ContextRail 内部：聊天内嵌区也能点开同一个详情，
   *  谁触发的打开谁持有 id，内层存就丢了「点的是哪个」这条信息。 */
  const [subagentDetailId, setSubagentDetailId] = useState<string | null>(null);
  /** 点子 agent（内嵌区域或侧栏条目）→ 展开右侧栏、切到「子 Agent」工作面，
   *  并把详情页钉在**被点的那个**上。id 必须一路带到详情：只切工作面的话，
   *  用户点谁看到的都是同一份列表，等于点了没反应。 */
  const openSubagentDetail = useCallback((id: string) => {
    setCtxOpen(true);
    localStorage.setItem("chat-ctx-open", "1");
    setCtxTab("agents");
    setSubagentDetailId(id);
  }, []);
  const closeSubagentDetail = useCallback(() => setSubagentDetailId(null), []);
  /** 侧栏「文件」工作面当前预览的路径（null = 停在列表页）。归属在父级同
   *  subagentDetailId：助手回复里的路径链接与文件 pill 也要能打开它，而那些
   *  点击发生在消息区、不在侧栏内部。 */
  const [previewPath, setPreviewPath] = useState<string | null>(null);
  /** 点文件 = 展开右侧栏、切到「文件」工作面、就地预览。侧栏可能正收起着，
   *  不顺手展开的话点了等于没反应——用户要的是「看内容」，不必先摆弄面板。 */
  const openFilePreview = useCallback((path: string) => {
    setCtxOpen(true);
    localStorage.setItem("chat-ctx-open", "1");
    setCtxTab("files");
    setPreviewPath(path);
  }, []);
  /** 输入卡 textarea 自动增高：随内容长到 max-h-40 后内部滚动；发送清空后缩回 */
  useAutoGrow(taRef, input);
  /** 切会话：斜杠 picker 状态复位（草稿/附件已按会话隔离，picker 是瞬时 UI 态） */
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
  /** PAR-1：每会话流式元数据（thinking/tools/skillFailure 权威副本，按 sid 隔离；   *  并行回复各自累积互不串台，收尾并入最终消息后删除条目）。
   *  落脚点在组件外（streamState）：状态要跨面板挂载存活，且必须与「已应用到
   *  哪一号事件」的游标同生共死——游标跑在状态前面，补齐就会跳过本该重建的
   *  那一段（接出来的是半截）。 */
  const metaFor = (sid: string | null) => {
    if (!sid) return undefined;
    return streamState.ensureMeta(sid);
  };
  /** 重建视图时接回在途回合：按会话重载历史拿到的是「回合收尾时」的库内容，
   *  库里还没有这一轮——不接回来，用户发出去的消息与已读到的半截回复就没了。
   *  用户消息在回合开始即落库，这里补的是库尚未落地的窗口与在途正文。 */
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
  /** 任务执行聊天化：围观流式回复中的会话时，到达的执行会话跳转排队（只留最新一个）；   *  执行本身不排队——后端已在新会话开跑 */
  const pendingExecRef = useRef<{ sid: string; title: string } | null>(null);
  /** 任务 id → 执行会话 id（chat-open-session 事件建立；invoke 收尾后按它刷新历史） */
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
  /** 视图会话是否有在途回复（驱动停止键/placeholder/禁用态） */
  const viewedBusy = sessionId !== null && inflightSids.has(sessionId);
  /** 子 agent 运行列表（右侧栏「子 Agent」工作面 + 聊天内嵌区域的同一份数据）。
   *  随会话切换重拉：子 agent 是按 parent_session_id 归属的 */
  const [subagents, setSubagents] = useState<SubagentRow[]>([]);
  // 事件回调注册在 render 期，只能走 ref 拿最新实现，否则闭包冻在首帧的
  // sessionId 上（切会话后事件会刷到旧会话的列表）
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
  /** 在途行要显示实时轮数，而轮数只活在 tool_guard 的内存仪表里（不落库，
   *  落库就得每轮写一次），只能重拉才拿得到。3 秒一拍：子 agent 一轮最短也要
   *  几秒（要调模型），再密只是白烧 IPC；终态后子集为空，定时器自己停。 */
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
  /** 侧栏/内嵌区消费的视图模型：标题取 objective（子 agent 的目标即其身份）。
   *  六态原样带下去——压成 running/error 两桶会让「已取消」「预算耗尽」都
   *  显示成红色失败（子 agent 解绑任务卡后这两态不再有卡可对照） */
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
        startedAt: r.startedAt ?? null,
        finishedAt: r.finishedAt ?? null,
        profile: r.profile,
        contextSummary: r.contextSummary ?? null,
        acceptance: parseJsonField(r.acceptanceJson),
        result: parseJsonField(r.resultJson),
        traceId: r.traceId,
      })),
    [subagents],
  );
  /** 用户主动取消子 agent：子 agent 会话不在会话列表里（is_subagent=1 被
   *  排除），停止键那条路永远够不着它，卡片删除那条取消入口又随解绑一起没了
   *  ——这里是用户唯一的取消入口。key 传 subagent_id（sa_ 前缀直查行） */
  const cancelSubagent = useCallback((id: string) => {
    invoke("cancel_subagent", { key: id }).catch((e) =>
      handleCommandError(e, "cancel_subagent"),
    );
  }, []);
  /** 子 agent 内嵌区（挂在最后一条助手回复的工具调用下方）。
   *  打包成一个对象传给 MessageList：它只挂一条气泡，其余气泡恒为 undefined，
   *  memo 浅比较才不会被每帧新建的对象打穿。toggle 也要稳定引用——内联箭头
   *  函数每帧换新，3s 轮询一次就白白重渲那条气泡。 */
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
  /** 本地提示消息：只显示不持久化（不污染上下文） */
  const addHint = (content: string, actionSessionId?: string) => {
    setMessages((prev) => [
      ...prev,
      { role: "assistant", content, actionSessionId, hint: true },
    ]);
  };
  /** sessionId 镜像：异步收尾时判断会话是否已切换（竞态防护） */
  const sessionIdRef = useRef<string | null>(null);
  useEffect(() => {
    sessionIdRef.current = sessionId;
  }, [sessionId]);

  // 子 agent 收尾事件——没有派发方主会话时（如后台/定时派发）才轻提示，
  // 有派发方时由汇报轮发言，见下面 bot-chat-report。
  // 复用 useTauriListen（与手写 listen 生命周期等价且更稳）。
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
  // 子任务收尾汇报轮的收尾：后端已把整段回复追加落库，这里只把流式气泡定稿
  // （正文用后端给的全文覆盖，与 runChat 同一套「full.text 校正」口径）
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
  // 子 agent 派发事件：行建好（queued）即到——只重拉列表，不打扰对话
  // （收尾提示只走 subagent-finished）。没有这条，新派发的行在收尾前
  // 全程隐身，长跑任务期间无处取消
  useTauriListen<{ subagentId: string }>("subagent-spawned", () => {
    refreshSubagentsRef.current();
  });

  /** 历史整段落库（覆盖写）。返回 promise：回合开始处要 await 它——
   *  用户消息先入库，库里才是这轮对话的真源。 */
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

  /** 历史行 JSON 列安全解析：损坏数据（旧版本写入/库被手改）不得炸掉整个
   *  会话加载——该字段按缺失处理，console.error 留痕（带字段名/行序/会话）定位 */
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

  /** 历史行 → 消息（回填思考/工具折叠行） */
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

  /** 终态后把最终正文取回来（回灌接回的那一轮没有 invoke 结果可等）。
   *
   *  终态事件比落库早到一点点：模型循环收尾 → 之后才写库。所以最后一次
   *  取到的历史可能还停在用户消息那条——那就等一下再取，别把半截当最终态。 */
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

  /** 在途回灌：后端说这个会话还有回合在跑、本地却没有这一轮的状态时，
   *  把这一轮的事件按序补齐——应用重启 / 面板重挂载后接回在途回复。
   *
   *  补齐走的是与实时订阅**同一批**处理函数（streamHandlersRef），口径一致；
   *  序号去重闸保证补齐与实时重叠时不会把正文叠成两半。 */
  const resumeInflight = useCallback(
    async (sid: string) => {
      if (!enabled) return;
      // 本地已有这一轮的内容 → 不用补（补一遍就是「重说了一遍」）。
      // 判据是游标而不是在途标记：游标清零才说明本地没有这一轮的状态
      // （一轮结束时连同 meta 一起清掉了）。
      if (streamState.seqOf(sid) > 0) return;
      // 同一会话的两次回灌不能并发（会灌两遍）：判定与置位连着做，中间不能有 await
      if (streamState.isResuming(sid)) return;
      streamState.markResuming(sid);
      try {
        const r = await streamState.fetchReplay(sid, 0);
        // 后端说没在跑 → 这一轮已经收尾，正文以库为准（历史已加载）
        if (!r.inflight || !r.events.length) return;
        // 本地那份是上一轮的残留：清干净再灌（否则正文接成两段）
        streamState.setMeta(sid, {});
        streamState.resetCursor(sid);
        // 补上在途标记：处理器按「非当前会话且不在途」过滤，不置位就接不住
        streamState.markInflight(sid);
        // 标记为「回灌接回」：这轮没有 invoke 结果可等，终态要自己取最终正文
        streamState.markResumed(sid);
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
    [enabled],
  );

  /** 挂载时把后端在跑的那些会话都接回来——不止当前会话：
   *  并行回复（PAR-1）的那些会话的半截内容也在，别让它们只剩最终正文。 */
  const resumeAllInflight = useCallback(async () => {
    try {
      const runs = await streamState.fetchInflightRuns();
      // 各会话的回灌互不依赖（各自的 meta 与游标），并行即可
      await Promise.all(runs.map((r) => resumeInflight(r.sessionId)));
    } catch {
      // 问不到在途列表就当没有（后端老版本没有这个命令时同理）
    }
  }, [resumeInflight]);

  // 挂载：加载会话列表（空则新建一个），选中最近更新的会话并恢复其消息
  // enabled=false 时直接跳过（机器人关闭，容器仍挂载但内部不工作；
  // 重新开启后 useEffect 因为 enabled 变化重跑加载）
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

  // 点击模型下拉外关闭（镜像会话菜单：按钮与下拉不在同一 ref 容器，双 ref 检测）
  useOutsideClose(
    modelMenuOpen,
    () => setModelMenuOpen(false),
    modelBtnRef,
    modelDropdownRef,
  );
  // 点击推理强度下拉外关闭（镜像模型菜单：双 ref 检测）
  useOutsideClose(
    effortMenuOpen,
    () => setEffortMenuOpen(false),
    effortBtnRef,
    effortDropdownRef,
  );
  // 点击授权模式下拉外关闭（同镜像：双 ref 检测）
  useOutsideClose(
    permMenuOpen,
    () => setPermMenuOpen(false),
    permBtnRef,
    permDropdownRef,
  );

  // 挂载：读当前模型配置，输入卡底栏 🧠 下拉展示当前协议的模型列表 + active id；
  // 设置页/另一窗口保存配置后广播 bot-config-changed，这里同步刷新
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

  // 🧠 下拉选中：走窄口径命令（后端读盘最新配置，只动 active + 派生老字段，
  // 不做整份写回——避免旧快照覆盖设置页并发修改）。成功后广播 bot-config-changed，
  // 设置页与本组件共用既有 listener 同步
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
  // thinking 的权威副本是各会话的 streamState 条目（事件到达即累加，不受缓冲影响）。
  /** 事件处理器表（按事件名）：回放补齐按事件名取**同一批**处理函数来调——
   *  两套口径迟早分家（回放出来的正文与实时出来的对不上，用户看到的是「重说了一遍」）。 */
  const streamHandlersRef = useRef<Record<string, (p: unknown) => void>>({});
  useEffect(() => {
    const pending = { text: "", think: false };
    let frame: number | null = null;
    const rafOk = typeof requestAnimationFrame === "function";
    // 事件处理器登记处：实时订阅与「回放补齐」走**同一批**处理函数。
    // 两套口径迟早分家——回放出来的正文与实时出来的对不上，用户看到的是
    // 「重说了一遍」。所以这里登记一份，回放时按事件名取同一份来调。
    const handlers: Record<string, (p: unknown) => void> = {};
    const reg = <T,>(name: string, cb: (p: T) => void) => {
      handlers[name] = cb as (p: unknown) => void;
      return listen<T>(name, (e) => cb(e.payload));
    };
    // 处理器表交给组件（回放补齐按事件名取同一份）
    streamHandlersRef.current = handlers;
    const flush = () => {
      frame = null;
      const add = pending.text;
      const thinkTouched = pending.think;
      pending.text = "";
      pending.think = false;
      if (!add && !thinkTouched) return;
      setMessages((prev) => {
        const last = prev[prev.length - 1];
        if (!last || !last.streaming) {
          // 后端主动起的一轮（子任务收尾汇报）没有前端预置的气泡，数组末尾是
          // 用户消息。原来在这里直接 return prev，增量被静默丢掉，用户得重新
          // 加载会话才看得见这段回复——这里补一个气泡接住它。
          return [
            ...prev,
            {
              role: "assistant" as const,
              content: add ?? "",
              streaming: true,
              turnStatus: "running" as const,
              turnStartedAt: Date.now(),
            },
          ];
        }
        const copy = [...prev];
        copy[copy.length - 1] = {
          ...last,
          content: add ? last.content + add : last.content,
          thinking: thinkTouched
            ? streamState.metaOf(sessionIdRef.current ?? "")?.thinking
            : last.thinking,
        };
        return copy;
      });
    };
    const schedule = () => {
      if (frame !== null) return;
      frame = rafOk
        ? requestAnimationFrame(flush)
        : window.setTimeout(flush, DELTA_BATCH_MS);
    };
    const unlisten = reg<{
      text?: string;
      sessionId?: string | null;
      seq?: number;
    }>("bot-chat-delta", (p) => {
        const sid = p?.sessionId ?? null;
        const t = p?.text;
        if (!t) return;
        // 权威副本按「回复所属会话」累加（切走再切回时占位气泡要回填这半截）
        if (
          sid !== null &&
          (streamState.isInflight(sid) || sid === sessionIdRef.current)
        ) {
          // 序号去重：补齐与实时会重叠（同一条事件两边各来一次）
          if (!streamState.noteSeq(sid, p?.seq)) return;
          const m = metaFor(sid);
          if (m) m.text = (m.text ?? "") + t;
        }
        if (sid !== sessionIdRef.current) return;
        pending.text += t;
        schedule();
      });
    const unThink = reg<{
      text?: string;
      sessionId?: string | null;
      seq?: number;
    }>("bot-think-delta", (p) => {
        const sid = p?.sessionId ?? null;
        const t = p?.text;
        if (!t) return;
        // 权威副本按「回复所属会话」累加（SWITCH-1：不随视图切换丢失），
        // 仅展示走同帧合并且只进当前视图
        if (
          (sid !== null && streamState.isInflight(sid)) ||
          sid === sessionIdRef.current
        ) {
          const m = metaFor(sid);
          if (m) m.thinking = (m.thinking ?? "") + t;
        }
        if (sid !== sessionIdRef.current) return;
        pending.think = true;
        schedule();
      },
    );
    const unTool = reg<{
      id?: string;
      name?: string;
      sessionId?: string | null;
      seq?: number;
    }>("bot-tool", (p) => {
      const sid = p?.sessionId ?? null;
      // 元数据按回复所属会话累加（SWITCH-1）；视觉更新仅当前视图
      if (sid === null) return;
      if (sid !== sessionIdRef.current && !streamState.isInflight(sid)) return;
      // 序号去重：补齐与实时会重叠（同一条事件两边各来一次）
      if (!streamState.noteSeq(sid, p?.seq)) return;
      const { id, name } = p ?? {};
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
    });
    const unToolName = reg<{
      id?: string;
      name?: string;
      sessionId?: string | null;
      seq?: number;
    }>("bot-tool-name", (p) => {
      const sid = p?.sessionId ?? null;
      if (sid === null) return;
      if (sid !== sessionIdRef.current && !streamState.isInflight(sid)) return;
      // 序号去重：补齐与实时会重叠（同一条事件两边各来一次）
      if (!streamState.noteSeq(sid, p?.seq)) return;
      const { id, name } = p ?? {};
      if (!id || !name) return;
      const m = metaFor(sid);
      if (!m) return;
      const tools = (m.tools ?? []).map((x) =>
        x.id === id ? { ...x, name } : x,
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
    const unToolDone = reg<{
      id?: string;
      name?: string;
      args?: string;
      result?: string | null;
      ms?: number | null;
      ok?: boolean;
      sessionId?: string | null;
      seq?: number;
    }>("bot-tool-done", (p) => {
      const sid = p?.sessionId ?? null;
      if (sid === null) return;
      if (sid !== sessionIdRef.current && !streamState.isInflight(sid)) return;
      // 序号去重：补齐与实时会重叠（同一条事件两边各来一次）
      if (!streamState.noteSeq(sid, p?.seq)) return;
      const { id, name, args, result, ms, ok } = p ?? {};
      if (!id) return;
      const m = metaFor(sid);
      if (!m) return;
      const tools = (m.tools ?? []).map((x) =>
        x.id === id
          ? {
              ...x,
              name: name ?? x.name,
              args,
              done: true,
              result: result ?? undefined,
              ms: ms ?? undefined,
              ok,
            }
          : x,
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
    const unToolRunning = reg<{
      id?: string;
      name?: string;
      args_preview?: string;
      sessionId?: string | null;
      seq?: number;
    }>("bot-tool-running", (p) => {
      const sid = p?.sessionId ?? null;
      if (sid === null) return;
      if (sid !== sessionIdRef.current && !streamState.isInflight(sid)) return;
      // 序号去重：补齐与实时会重叠（同一条事件两边各来一次）
      if (!streamState.noteSeq(sid, p?.seq)) return;
      const { id, name, args_preview: argsPreview } = p ?? {};
      if (!id) return;
      const m = metaFor(sid);
      if (!m) return;
      // 记本地开始时刻：工具返回后以 done 事件的 ms 为准，前端计时只在
      // 「bot-tool 已收到但还没回」这段窗口里显示，不与后端耗时重复计。
      const tools = (m.tools ?? []).map((x) =>
        x.id === id ? { ...x, name: name ?? x.name, argsPreview, startedAt: Date.now() } : x,
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
    //  回合状态机（借鉴 ZCode turn view model）：running/settling 都算进行中。
    //  settling 是关键——正文已输出完但终稿自检还在跑，此时若把气泡置为
    //  「完成」，用户看到的正是一句「说完就停」的假静止。
    const unTurnStatus = reg<{
      status?: TurnStatus;
      detail?: string;
      sessionId?: string | null;
      seq?: number;
      startedAt?: number;
    }>("bot-turn-status", (p) => {
      const sid = p?.sessionId ?? null;
      if (sid === null) return;
      if (sid !== sessionIdRef.current && !streamState.isInflight(sid)) return;
      // 序号去重：补齐与实时会重叠（同一条事件两边各来一次）
      if (!streamState.noteSeq(sid, p?.seq)) return;
      const status = p?.status;
      if (!status) return;
      const m = metaFor(sid);
      if (m) {
        m.turnStatus = status;
        m.turnDetail = p?.detail ?? undefined;
        // 终态到来前记下开始时刻：running/settling 期间的秒表从这里起。
        // 起点取后端记的本轮起始时刻——回灌接回时「本地此刻」是补齐那一刻，
        // 拿它当起点秒表会从零开始（回复其实已经跑了一会儿）
        if (status === "running" && m.turnStartedAt == null) {
          m.turnStartedAt = p?.startedAt ?? Date.now();
        }
      }
      // 回灌接回的那一轮：终态到了要把最终正文取回来——这轮没有 invoke
      // 结果可等（发起它的那个窗口已经没了），只能自己去取库里的最终态
      if (
        status !== "running" &&
        status !== "settling" &&
        streamState.isResumed(sid)
      ) {
        streamState.clearResumed(sid);
        exitChat(sid);
        void loadFinalText(sid);
      }
      if (sid !== sessionIdRef.current) return;
      setMessages((prev) => {
        const last = prev[prev.length - 1];
        if (!last || !last.streaming) return prev;
        const copy = [...prev];
        copy[copy.length - 1] = {
          ...last,
          turnStatus: status,
          turnDetail: p?.detail ?? undefined,
          turnStartedAt:
            status === "running"
              ? (m?.turnStartedAt ?? last.turnStartedAt ?? Date.now())
              : last.turnStartedAt,
        };
        return copy;
      });
    });
    //  结构化文件变更：dispatch 在有 trace 的执行会话里 emit（全窗口）——
    // 每次落盘修改一条，按会话累积进 meta，收尾并入最终消息给 FileSummary 用。
    // 主聊天（无 trace）不产生事件 → FileSummary 走正则兜底，行为不变。
    const unFileChanged = reg<{
      path?: string;
      kind?: "create" | "modify" | "delete";
      added?: number;
      deleted?: number;
      sessionId?: string | null;
      seq?: number;
    }>("bot-file-changed", (p) => {
      const sid = p?.sessionId ?? null;
      if (sid === null) return;
      if (sid !== sessionIdRef.current && !streamState.isInflight(sid)) return;
      // 序号去重：补齐与实时会重叠（同一条事件两边各来一次）
      if (!streamState.noteSeq(sid, p?.seq)) return;
      if (!p?.path) return;
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
    const unSkillFailed = reg<{
      skillName?: string;
      reason?: string;
      completedSummary?: string;
      rollbackAttempted?: boolean;
      sessionId?: string | null;
      seq?: number;
    }>("bot-skill-failed", (p) => {
      const sid = p?.sessionId ?? null;
      if (sid === null) return;
      if (sid !== sessionIdRef.current && !streamState.isInflight(sid)) return;
      // 序号去重：补齐与实时会重叠（同一条事件两边各来一次）
      if (!streamState.noteSeq(sid, p?.seq)) return;
      if (!p?.skillName) return;
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
    //  水位条数据源：Anthropic 回合级 usage 按会话累计（当前视图 + 在途会话，
    // 与流式事件同过滤口径——切走的会话继续累计，切回可见）
    const unUsage = listen<{
      inputTokens?: number;
      outputTokens?: number;
      cacheReadTokens?: number;
      cacheCreationTokens?: number;
      sessionId?: string | null;
    }>("bot-usage-delta", (e) => {
      const sid = e.payload?.sessionId ?? null;
      if (sid === null) return;
      if (sid !== sessionIdRef.current && !streamState.isInflight(sid)) return;
      const i = e.payload?.inputTokens ?? 0;
      const o = e.payload?.outputTokens ?? 0;
      const cr = e.payload?.cacheReadTokens ?? 0;
      const cc = e.payload?.cacheCreationTokens ?? 0;
      // 四项全空才跳过：缓存命中的回合 input/output 极小甚至为 0，只看前两项
      // 会把整段缓存量丢在账外（与 trace 弹窗的分档展示对不上）
      if (i <= 0 && o <= 0 && cr <= 0 && cc <= 0) return;
      setSessionUsage((prev) => {
        const cur = prev[sid] ?? {
          input: 0,
          output: 0,
          cacheRead: 0,
          cacheCreation: 0,
          lastInput: 0,
          lastCacheRead: 0,
        };
        return {
          ...prev,
          [sid]: {
            input: cur.input + i,
            output: cur.output + o,
            cacheRead: cur.cacheRead + cr,
            cacheCreation: cur.cacheCreation + cc,
            lastInput: i,
            lastCacheRead: cr,
          },
        };
      });
    });
    return () => {
      if (frame !== null) {
        if (rafOk) cancelAnimationFrame(frame);
        else window.clearTimeout(frame);
        frame = null;
      }
      unlistenSafe(unlisten);
      unlistenSafe(unThink);
      unlistenSafe(unTool);
      unlistenSafe(unToolName);
      unlistenSafe(unToolDone);
      unlistenSafe(unToolRunning);
      unlistenSafe(unTurnStatus);
      unlistenSafe(unFileChanged);
      unlistenSafe(unUsage);
      unlistenSafe(unSkillFailed);
      streamHandlersRef.current = {};
    };
  }, [loadFinalText]);

  // 围观中的执行会话 sid；执行收尾（execute-task 的 history_load 完成）后清除
  const execWatchRef = useRef<string | null>(null);
  /** 切到执行会话围观：加载已落库历史 + 末尾 streaming 占位气泡承接后续流式增量。   * watch=false（收尾后迟到的跳转）：只读查看不挂围观守卫，输入立即可用 */
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
  // openExecSession 经 ref 暴露给事件监听（避免闭包旧状态）
  const openExecSessionRef = useRef<(sid: string, watch?: boolean) => void>(
    () => {},
  );
  // latest-ref 模式：事件监听闭包要调到最新一帧实现（React 官方推荐转发法；
  // render 期写 ref 为既有语义，拆分批不改时机）
  // oxlint-disable-next-line react/refs
  openExecSessionRef.current = (sid, watch) => {
    openExecSession(sid, watch);
  };

  // 任务卡交给机器人执行（execute-task 事件：任务卡 🤖 按钮触发）
  // 任务执行聊天化：不再在当前会话执行——后端建新会话跑（run_task_in_chat），
  // 前端收 chat-open-session 切过去围观。执行本身不吃 busy 锁（会话隔离天然并发），
  // 只有「自动跳转查看」在围观流式回复时排队（exitChat 清空在途时 hint 提示）。
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

  // 危险操作确认由 ConfirmMap 全窗独现（主窗口任何视图下都要看得见，
  // 聊天窗常挂载只切 hidden，把弹窗放这里会在别的视图里不可见）。
  // 本组件不再 listen bot-confirm——两个呈现者会为同一请求各弹一次。

  /* 新消息自动滚到底——只在「还贴着底」时跟。
   *
   * 无条件滚底是聊天窗最重的坑：流式增量每 16ms 换一次 messages 引用，
   * 每帧都无条件 scrollTo 底 → 用户往上翻历史，下一帧就被拽回去（历史根本看不了），
   * 人和程序抢同一个滚动位置 → 视口来回抽搐（抖）。
   *
   * 用 layout effect（绘制前）而不是 useEffect（绘制后）：后者每帧晚一帧才滚，
   * 观感是「内容先跳一下、再跟一下」，16ms 一帧攒起来就是抖。
   * 贴底意图由滚动容器自己的 scroll 事件判定（离底多远是最终要满足的不变量），
   * 滚回底部自动恢复跟随；换会话/发新消息强制回到底部。 */
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

  /* 贴底意图跟踪：滚动容器的 scroll 事件是「用户想看哪」的唯一可靠信号。
   * 不用 wheel/touch 判意图——惯性滚动与程序化滚动都会混进 scroll 事件，
   * 而「离底多远」才是最终要满足的不变量。 */
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

  /* 容器尺寸变化 → 视口复位。
   *  聊天页切走时容器是 display:none（没有滚动口，浏览器把滚动位置清零），
   *  切回来不复位就停在历史开头，再来一帧流式增量又被拽到底——看着像吞内容。
   *  上下文水位条出现等布局变化同理：贴着底就跟到底，不打断正在读的内容。 */
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

  // ⌘K 跳会话（命令面板）：主窗口发 chat-focus-session，复用 switchSession
  // 走完整切换（清消息 + bot_history_load + inflight 占位），仅 setSessionId 不会加载历史。
  // ref 写放 effect（render 期写 ref 会新增 lint warn）
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

  /** 核心执行：发送历史并流式收尾（send / /retry 共用）。   *  任务执行聊天化：bot_execute_task 不再走这里（任务卡执行由后端
   *  建新会话跑，前端收 chat-open-session 切换围观）。
   *  收尾时校验会话未切换才更新 UI；持久化始终按 sid 写（写的是正确会话） */
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
  // 子 agent 执行会话不进会话列表（bot_sessions_load 过滤 is_subagent=1，
  // 只以聊天内嵌区域呈现），视图会话恒为普通对话——停止统一走 bot_stop；
  // 子 agent 的取消入口在条带/侧栏的行上（cancelSubagent）

  // 停止分派唯一实现（发送/停止一体键的 busy 分支 + /stop 斜杠命令共用）
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

  // 斜杠快捷命令（/stop 在机器人回复中也能生效）
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
  // Rust 侧弹框：走原生对话框，前端 dialog.open 在部分平台不弹框
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
  // 转成 CSS 像素后与聊天区矩形比对。
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
    if (!sid || streamState.isInflight(sid)) return;
    const next = messagesRef.current.filter((_, j) => j !== idx);
    setMessages(next);
    persistHistory(sid, next);
  }, []);

  // 点任务引用按钮：打开该任务编辑态 + 把主窗口唤起并强制置顶
  // （主窗口要出现在桌面屏幕最顶层才看得见浮窗）
  const openTaskInMain = useCallback(async (ref: TaskRef) => {
    emit("edit-task", { id: ref.id }).catch(() => {});
    invoke("focus_main_window").catch(() => {});
  }, []);

  // 「执行轨迹」入口：按回合回传的 trace 打开执行详情。会话 id 走 ref
  // 取当前视图会话——气泡的 memo 浅比较依赖句柄稳定，不能把 sessionId
  // 闭进回调里（那样每换会话就换一次句柄，历史气泡全量重渲）。
  const openTraceFor = useCallback((traceId: number) => {
    setOpenTrace({ traceId, sessionId: sessionIdRef.current });
  }, []);

  // RE-1 推理强度：底栏按钮显示的生效档位 = 本会话覆盖 ?? 后台默认；
  // effortIsOverridden 决定要不要缀「·默认」角标
  const effectiveEffort: ReasoningLevel =
    (sessionId != null ? effortBySession.get(sessionId) : undefined) ??
    effortBase;
  const effortIsOverridden =
    sessionId != null && effortBySession.has(sessionId);
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
          两端各一个折叠钮，是各自侧栏的唯一开关——侧栏内不再放第二个 */}
      <header className="mb-2 flex shrink-0 items-center gap-2 border-b border-[var(--edge)] pb-2">
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
          previewPath={previewPath}
          onPreviewPath={setPreviewPath}
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
