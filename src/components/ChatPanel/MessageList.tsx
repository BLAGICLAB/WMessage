// ChatPanel 子模块：消息列表（ 拆分自 ChatPanel.tsx，JSX 逐字搬移）。
// MsgBubble 用 React.memo 做流式性能边界：流式增量每帧只替换最后一
// 条消息的对象引用，历史气泡 props 全等 → memo 跳过重渲染（MarkdownText/
// RichText 的 markdown 解析是重活，N 条消息 × 每帧一次的全列表重渲染是
// 长会话掉帧主因）。回调句柄由父级 useCallback 固定，memo 浅比较即可命中。

import { memo, useMemo, useRef, useState, type ComponentType } from "react";
import {
  Bot,
  Check,
  Copy,
  FileText,
  History,
  Loader2,
  MessageCircle,
  MessagesSquare,
  Pin,
  Trash2,
  TriangleAlert,
  Wrench,
} from "lucide-react";
import { basename, formatMsShort } from "../../format";
import { mergeDeliverables } from "../../lib/deliverables";
import { extractFilePaths, openTarget } from "../../lib/openTarget";
import { useProbedDeliverables } from "../../lib/useDeliverables";
import { ActorAvatar } from "../ActorAvatar";
import { MarkdownText } from "../MarkdownText";
import { BUBBLE_MAX_W_PCT } from "./constants";
import type { SubagentRun } from "./ContextRail";
import { Fold } from "./Fold";
import { LiveIndicator } from "./LiveIndicator";
import { RichText } from "./RichText";
import { useStickyBubbleWidth } from "./stickyBubbleWidth";
import { SubagentStrip } from "./SubagentStrip";
import { UserBubbleContent } from "./UserBubbleContent";
import { fmtElapsed, useElapsed } from "./useElapsed";
import type {
  Msg,
  TaskRef,
  ToolCall,
  VerboseLevel,
} from "./types";

/** 气泡回调（父级 useCallback 固定引用，memo 浅比较依赖这一点） */
type BubbleCallbacks = {
  /** 逐条复制回复内容（idx 用于「已复制」反馈定位） */
  onCopy: (idx: number, content: string) => void;
  /** 移除一条回复（不再作为上下文发给模型） */
  onRemove: (idx: number) => void;
  /** 点任务引用按钮：去主窗口打开该任务 */
  onOpenTask: (ref: TaskRef) => void;
  /** 「查看执行对话」跳转：切到该执行会话 */
  onOpenExecSession: (sid: string) => void;
  /** 「执行轨迹」：打开这一轮的执行详情（trace 归属跟着消息走） */
  onOpenTrace?: (traceId: number) => void;
};

/** 子 agent 内嵌区。整组可选：没子 agent 的会话不传就不渲染。
 *  刻意打包成一个对象而不是摊平成四个 prop——它只会整体挂在**最后一条**
 *  助手回复上，其余气泡恒为 undefined，memo 浅比较照样命中。 */
type SubagentSlot = {
  runs: SubagentRun[];
  /** 条带是否展开（折叠时只列在途） */
  expanded: boolean;
  onOpen: (id: string) => void;
  onCancel: (id: string) => void;
  onToggleExpanded: () => void;
};

type MsgBubbleProps = BubbleCallbacks & {
  msg: Msg;
  idx: number;
  /** 该消息的「已复制」反馈是否点亮（父级按 copiedIdx 算好传入） */
  isCopied: boolean;
  /** 视图会话是否有在途回复（驱动移除按钮禁用态） */
  busy: boolean;
  /** 执行过程详细度：控制工具徽章展开级别（简洁/详细/调试） */
  verboseLevel: VerboseLevel;
  /** 子 agent 内嵌区；仅挂在最后一条助手回复上，其余气泡为 undefined */
  subagentSlot?: SubagentSlot;
  /** 点文件（pill / 正文路径链接）→ 右侧栏就地预览。父级用 useCallback 固定，
   *  memo 浅比较才不会因新句柄把历史气泡全部重渲染一遍 */
  onPreviewFile?: (path: string) => void;
  /** 交付物区空态提示的跳转：去右侧栏看本轮改动过的文件 */
  onShowFileChanges?: () => void;
};

function MsgBubbleBase({
  msg: m,
  idx,
  isCopied,
  busy,
  verboseLevel,
  subagentSlot,
  onPreviewFile,
  onShowFileChanges,
  onCopy,
  onRemove,
  onOpenTask,
  onOpenExecSession,
  onOpenTrace,
}: MsgBubbleProps) {
  // 一次性算文件路径 + 是否显示操作行（避免在渲染条件里 IIFE + mutation m._fps 的反模式）；
  // 流式末条气泡每帧重渲染，useMemo 让路径正则只在内容变化时重跑
  const fps = useMemo(() => extractFilePaths(m.content), [m.content]);
  // 交付物清单的另一半：本轮写盘产生的文件。写盘通道记的是「动过的文件」，
  // 里面混着生成脚本等中间产物形态，合并时过形态过滤（见 mergeDeliverables）。
  const producedPaths = useMemo(
    () => (m.fileChanges ?? []).map((c) => c.path),
    [m.fileChanges],
  );
  // 气泡宽度只增不减：工具行「正在跑 xxx」撑宽、跑完缩回，同一条回复宽度反复跳
  const bubbleRef = useRef<HTMLDivElement | null>(null);
  const colRef = useRef<HTMLDivElement | null>(null);
  useStickyBubbleWidth(bubbleRef, colRef);
  const hasContent = !!m.content.trim();
  const showActions =
    m.role === "assistant" &&
    !m.streaming &&
    (hasContent ||
      (m.refs?.length ?? 0) > 0 ||
      fps.length > 0 ||
      producedPaths.length > 0 ||
      m.traceId != null);
  /** 活体指示的显示条件：本条还在流式、且回合未落终态。
   *  turnStatus 缺省按 running 算（与 ChatPanel 旧的派生口径一致）——
   *  已完成的历史气泡若 turnStatus 缺失，不该又冒出假活体指示。 */
  const liveTurn =
    m.role === "assistant" &&
    !!m.streaming &&
    m.turnStatus !== "completed" &&
    m.turnStatus !== "interrupted" &&
    m.turnStatus !== "failed";
  /** 气泡正文：用户消息走浅底渲染、流式走轻量富文本、完成走 markdown（拆分前的
   *  嵌套三元提取成函数，渲染输出逐字不变） */
  function bubbleBody(msg: Msg) {
    if (!msg.content) return msg.streaming ? "…" : "";
    if (msg.role === "user") return <UserBubbleContent content={msg.content} />;
    if (msg.streaming) return <RichText text={msg.content} onPathClick={onPreviewFile} />;
    return <MarkdownText text={msg.content} onPathClick={onPreviewFile} />;
  }
  return (
    <div
      className={
        // 头像写在 DOM 首位，用户侧再翻 row-reverse：视觉上被甩到最右。
        // 一处顺序写清，头像与气泡不会在两条分支里写歪
        m.role === "user"
          ? "flex flex-row-reverse items-start gap-2"
          : "flex items-start gap-2"
      }
    >
      {/* 归属头像：谁说的这条消息，悬停出姓名（ActorAvatar 自带 title） */}
      <ActorAvatar bot={m.role === "assistant"} size={24} />
      <div
        ref={m.role === "assistant" ? colRef : undefined}
        className="min-w-0"
        // 限宽挂在列容器上（不挂气泡）：头像不占正文宽度。
        // 用户与机器人同一条上限——机器人气泡满宽时长回复会伸到用户头像底下
        style={{ maxWidth: `${BUBBLE_MAX_W_PCT}%` }}
      >
        <div
          ref={m.role === "assistant" ? bubbleRef : undefined}
          className={`chat-bubble text-xs leading-relaxed whitespace-pre-wrap break-words ${
            m.role === "user" ? "chat-bubble-user" : "chat-bubble-bot"
          }`}
        >
          {/* 活体指示贴在气泡顶部：回合状态是「这条回复」的属性，头部那一条
              讲的是整个会话——滚动看旧消息时，头部指示与眼前内容对不上号 */}
          {liveTurn && (
            <div className="mb-1.5">
              <LiveIndicator
                status={m.turnStatus ?? "running"}
                detail={m.turnDetail}
                startedAt={m.turnStartedAt}
              />
            </div>
          )}
          {m.role === "assistant" && (m.thinking?.length ?? 0) > 0 && (
            <Fold
              title={
                <span>
                  <MessageCircle
                    size={11}
                    aria-hidden
                    className="inline-block align-[-2px]"
                  />{" "}
                  思考过程{m.streaming ? " …" : ""}
                </span>
              }
            >
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
                  <TriangleAlert
                    size={11}
                    aria-hidden
                    className="inline-block align-[-2px]"
                  />{" "}
                  Skill 失败：{m.skillFailure.skillName}
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
                    <TriangleAlert
                      size={11}
                      aria-hidden
                      className="inline-block align-[-2px]"
                    />{" "}
                    已完成步骤未回滚，请检查任务卡状态。
                  </div>
                )}
              </div>
            </Fold>
          )}
          {/* 工具调用（对齐截图）：mono pill 徽章行 + 可折叠「进程 N/M」详情 */}
          {(m.tools?.length ?? 0) > 0 && (
            <ToolBadges tools={m.tools!} verboseLevel={verboseLevel} />
          )}
          {/* 子 agent 区域：挂在最后一条助手回复里、工具调用下方。
              跟着派发它的那一轮走，而不是钉在对话顶部——对话顶部那块地属于会话，
              「谁派的、派去做什么」属于某一轮，钉在顶上就成了无主浮层。 */}
          {subagentSlot && subagentSlot.runs.length > 0 && (
            <SubagentStrip
              subagents={subagentSlot.runs}
              onOpen={subagentSlot.onOpen}
              onCancel={subagentSlot.onCancel}
              expanded={subagentSlot.expanded}
              onToggleExpanded={subagentSlot.onToggleExpanded}
            />
          )}
          {bubbleBody(m)}
          {/* 「查看执行对话」跳转（任务执行聊天化：busy 时跳转排队，
              忙完 hint + 按钮切换到执行会话） */}
          {m.actionSessionId && (
            <button
              className="nm-btn mt-1 inline-flex items-center gap-1 rounded-lg px-2 py-0.5 text-[10px] text-[var(--t2)]"
              onClick={() => onOpenExecSession(m.actionSessionId!)}
            >
              <MessagesSquare size={11} aria-hidden />
              查看执行对话
            </button>
          )}
        </div>
        {/* 回复完成后的操作行：复制按钮 + 任务引用按钮（点击去主窗口打开该任务） */}
        {showActions && (
          <div className="mt-1.5 flex flex-wrap items-center gap-1">
            {hasContent && (
              <button
                className="nm-btn inline-flex items-center gap-1 rounded-lg px-2 py-0.5 text-[10px] text-[var(--t3)]"
                title="复制回复内容"
                onClick={() => onCopy(idx, m.content)}
              >
                {isCopied ? (
                  <>
                    <Check size={12} aria-hidden /> 已复制
                  </>
                ) : (
                  <>
                    <Copy size={11} aria-hidden />
                    复制
                  </>
                )}
              </button>
            )}
            {hasContent && (
              <button
                className="nm-btn inline-flex items-center gap-1 rounded-lg px-2 py-0.5 text-[10px] text-[var(--t4)] hover:text-[var(--danger)]"
                title="移除这条回复（不满意时删除，不再作为上下文）"
                onClick={() => onRemove(idx)}
                disabled={busy}
              >
                <Trash2 size={11} aria-hidden />
                移除
              </button>
            )}
            {m.traceId != null && onOpenTrace && (
              <button
                className="nm-btn inline-flex items-center gap-1 rounded-lg px-2 py-0.5 text-[10px] text-[var(--t3)]"
                title="查看这一轮的工具调用与文件改动"
                onClick={() => onOpenTrace(m.traceId!)}
              >
                <History size={11} aria-hidden />
                执行轨迹
              </button>
            )}
            <DeliverableArea
              declared={fps}
              produced={producedPaths}
              streaming={!!m.streaming}
              onOpen={onPreviewFile ?? openTarget}
              onShowChanges={onShowFileChanges}
            />
            {(m.refs ?? []).map((r) => (
              <button
                key={r.id}
                className="nm-btn inline-flex items-center gap-1 rounded-lg px-2 py-0.5 text-[10px] text-[var(--t3)] max-w-full"
                title={`打开任务：${r.title}`}
                onClick={() => onOpenTask(r)}
              >
                <Pin size={11} aria-hidden className="shrink-0" />
                <span className="truncate max-w-[260px]">{r.title}</span>
              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

/** memo 边界：流式更新时历史气泡 props 全等 → 跳过重渲染（见文件头注释） */
const MsgBubble = memo(MsgBubbleBase);

/** 单个工具徽章的状态标记（✕ / ✓ / spinner+计时）。
 *  徽章行与折叠标题共用，避免两处各写一份后改一处漏一处。 */
function ToolStatusMark({ t }: { t: ToolCall }) {
  const running = !t.done && t.ok !== false;
  const secs = useElapsed(t.startedAt, running);
  const tone = t.ok === false
    ? "text-[var(--danger)]"
    : t.done
      ? "text-[var(--success)]"
      : "";
  if (!running) {
    return (
      <span aria-hidden className={tone}>
        {t.ok === false ? "✕" : <Check size={10} strokeWidth={3} />}
      </span>
    );
  }
  // 未完成：Loader2 转圈 + 前端本地计时。静态「…」在 300s 级的工具里
  // 与「已结束」在视觉上无区别——这是用户看不出 agent 在不在跑的主因。
  return (
    <span aria-hidden className="inline-flex items-center gap-1 text-[var(--brand)]">
      <Loader2 size={10} className="wmsg-activity" />
      {secs != null && (
        <span className="tabular-nums text-[var(--t4)]">{fmtElapsed(secs)}</span>
      )}
    </span>
  );
}

/** 工具调用：一行可折叠的摘要（🔧 调用工具 N/M），展开才是 mono pill 徽章行
 *  + 逐工具入参/结果。
 *
 *  徽章行为什么也收进折叠里：一次任务动辄十几个工具，pill 全平铺会占掉
 *  四五行、把正文挤到折叠线以下——正文才是用户要读的东西。
 *  折叠不等于静默——标题行带着「N/M」进度与**当前正在跑的工具名**，
 *  「亮着」也真有信息（否则用户会以为卡死）。
 *
 *  verbose 三档决定展开后看到多少：简洁=只有徽章；详细=再带入参/结果；
 *  调试=默认展开 + 耗时。
 *  memo：文本流式 tick 不改 tools 引用，跳过整块重渲染。 */
const ToolBadges = memo(function ToolBadges({
  tools,
  verboseLevel,
}: {
  tools: ToolCall[];
  verboseLevel: VerboseLevel;
}) {
  const done = tools.filter((t) => t.done).length;
  const failed = tools.filter((t) => t.ok === false).length;
  const totalMs = tools.reduce((acc, t) => acc + (t.ms ?? 0), 0);
  const runningTool = tools.find((t) => !t.done && t.ok !== false);
  return (
    <div className="mb-1">
      <Fold
        title={
          <span className="inline-flex items-center gap-1">
            <Wrench size={11} aria-hidden className="shrink-0" />
            <span className="font-mono">
              调用工具 {done}/{tools.length}
              {failed > 0 && (
                <span className="text-[var(--danger)]">
                  {" "}
                  · {failed} 失败
                </span>
              )}
              {verboseLevel === "debug" &&
                totalMs > 0 &&
                ` · Σ ${formatMsShort(totalMs)}`}
            </span>
            {runningTool && (
              <span className="inline-flex items-center gap-1 text-[var(--brand)]">
                <Loader2 size={9} className="wmsg-activity" />
                正在跑 {runningTool.name || "tool"}
              </span>
            )}
          </span>
        }
        defaultOpen={verboseLevel === "debug"}
      >
        <div className="flex flex-wrap gap-1">
          {tools.map((t) => (
            <span
              key={t.id}
              className="inline-flex items-center gap-1 rounded-full border border-[var(--edge)] px-2 py-0.5 font-mono text-[10px] leading-4 text-[var(--t4)]"
            >
              {t.name || "tool"}
              <ToolStatusMark t={t} />
            </span>
          ))}
        </div>
        {verboseLevel !== "concise" && (
          <div className="mt-1 space-y-1">
            {tools.map((t) => (
              <div key={t.id}>
                <span className="font-mono text-[10px] text-[var(--t4)]">
                  {t.name || "tool"}
                  {t.ms != null && (
                    <span className="ml-1 text-[var(--t5)]">{formatMsShort(t.ms)}</span>
                  )}
                </span>
                {t.args ? (
                  <pre className="opacity-80 whitespace-pre-wrap break-words font-mono text-[10px]">
                    {t.args.length > 500 ? t.args.slice(0, 500) + "…" : t.args}
                  </pre>
                ) : null}
                {t.result ? (
                  <pre className="whitespace-pre-wrap break-words font-mono text-[10px] text-[var(--t5)]">
                    {t.result.length > 800
                      ? t.result.slice(0, 800) + "…"
                      : t.result}
                  </pre>
                ) : null}
              </div>
            ))}
          </div>
        )}
      </Fold>
    </div>
  );
});

/**
 * 交付物区：这一轮的**最终产物**。
 *
 * 为什么不再摆执行过程文件：写盘通道记的是「动过的文件」，里面混着生成
 * 脚本等中间产物——它们挂在交付物位上，真正要拿的那份反被淹没。改动文件
 * 归右侧栏「改动」页与执行轨迹面板（那里才有 ±行 / diff / 回滚）。
 *
 * 两种空态要分得开：
 * - 确实没产出 → 什么都不显示
 * - 写了文件但机器人没在正文点名 → 给一行提示并可跳去看改动（正文自报
 *   才是交付物清单的来源，模型漏写就等于没交付，用户该知道去哪找）
 *
 * 探测状态（存在性校验）持有在本组件内，**不放在气泡里**：探测结果落地
 * 会重渲染整个气泡，正文里的链接 DOM 节点随之被替换——用户正好在那一刻
 * 点正文路径，点的是已被换掉的旧节点，事件冒泡不到 React 根，点击就丢了。
 */
function DeliverableArea({
  declared,
  produced,
  streaming,
  onOpen,
  onShowChanges,
}: {
  /** 正文自报的路径（模型点名的交付物，不过形态过滤） */
  declared: string[];
  /** 本轮写盘产生的文件（合并时过形态过滤，脚本等中间产物挡掉） */
  produced: string[];
  /** 消息还在流式输出：正文每 token 都变，探一次就是每 token 一次 IPC */
  streaming: boolean;
  /** 点交付物：跳右侧栏预览 */
  onOpen: (path: string) => void;
  /** 空态提示的跳转：去右侧栏看本轮改动过的文件 */
  onShowChanges?: () => void;
}) {
  const merged = useMemo(
    () => mergeDeliverables(declared, produced),
    [declared, produced],
  );
  const probed = useProbedDeliverables(merged, !streaming);
  const items = probed.items;
  const [open, setOpen] = useState(false);
  if (items.length === 0) {
    // 还没探完：先不渲染。探完才知道是「这轮没交付物」还是「在探」，
    // 现在就报「没产出」会把刚写出来的文件说没了。
    if (!probed.ready) return null;
    // 空态提示里的数字取 produced 总数（含被形态过滤掉的脚本形态）：
    // 用户要的是「有东西没给我」的总数，不是筛完剩几个
    if (produced.length === 0) return null;
    return (
      <button
        type="button"
        className="nm-btn inline-flex items-center gap-1 rounded-lg px-2 py-0.5 text-[10px] text-[var(--t3)]"
        title="机器人没有在回复里列出这些产物：点开看本轮改动过的文件"
        onClick={() => onShowChanges?.()}
      >
        <FileText size={11} aria-hidden />
        本轮有 {produced.length} 个产物未在回复中列出
      </button>
    );
  }
  const chips = items.map((d) => (
    <button
      key={d.path}
      type="button"
      className="nm-btn inline-flex items-center gap-1 rounded-lg px-2 py-0.5 text-[10px] text-[var(--t3)] max-w-full"
      title={`打开交付物：${d.path}`}
      onClick={() => onOpen(d.path)}
    >
      <FileText size={11} aria-hidden className="shrink-0 text-[var(--t4)]" />
      <span className="truncate max-w-[280px]">{basename(d.path)}</span>
    </button>
  ));
  // 统一 DOM 形状（w-full 容器），≤2 与 >2 只是容器内子元素不同
  return (
    <div className="w-full">
      {items.length <= 2 ? (
        <div className="flex flex-wrap gap-1">{chips}</div>
      ) : (
        <>
          <button
            type="button"
            className="nm-btn inline-flex items-center gap-1 rounded-lg px-2 py-0.5 font-mono text-[10px] text-[var(--t3)]"
            aria-expanded={open}
            title={open ? "收起交付物列表" : "展开交付物列表"}
            onClick={() => setOpen((v) => !v)}
          >
            <FileText size={11} aria-hidden />
            交付物 {items.length}
            <span aria-hidden>{open ? "▴" : "▾"}</span>
          </button>
          {open && <div className="mt-1 flex flex-wrap gap-1">{chips}</div>}
        </>
      )}
    </div>
  );
}

type MessageListProps = BubbleCallbacks & {
  /** 滚动容器 ref（父级「新消息自动滚到底」用） */
  scrollRef: React.RefObject<HTMLDivElement | null>;
  messages: Msg[];
  /** 视图会话是否有在途回复 */
  viewedBusy: boolean;
  /** 「已复制」反馈的消息下标 */
  copiedIdx: number | null;
  /** 执行过程详细度（透传给 ToolBadges） */
  verboseLevel: VerboseLevel;
  /** 空状态起始短语点击：把短语填进当前会话输入框（不自动发送） */
  onPickStarter: (text: string) => void;
  /** 子 agent 内嵌区；不传 = 本会话没有子 agent */
  subagentSlot?: SubagentSlot;
  /** 点文件 → 右侧栏就地预览（不传 = 调起系统默认程序） */
  onPreviewFile?: (path: string) => void;
  /** 交付物区空态提示的跳转：去右侧栏看本轮改动过的文件 */
  onShowFileChanges?: () => void;
};

/** 空状态：一行灰字把「能做什么」全压在读者眼前最弱的位置。
 *  改成引导卡 + 可点击起始短语——空会话是唯一能低成本教用户上手的时刻，
 *  短语点一下直接落进输入框（不自动发送，措辞仍由用户过目） */
const STARTERS = [
  "列出我当前的任务",
  "把今天的安排整理成清单",
  "帮我写一条周报",
];

function EmptyState({ onPick }: { onPick: (text: string) => void }) {
  return (
    <div className="flex flex-col items-center px-4 pt-16 text-center">
      <span className="nm-card mb-3 flex h-11 w-11 items-center justify-center rounded-2xl">
        <Bot size={18} aria-hidden className="text-[var(--brand)]" />
      </span>
      <p className="text-sm font-medium text-[var(--t2)]">和机器人说点什么</p>
      <p className="mt-1 text-xs text-[var(--t5)]">
        问我任务、进度、安排，或直接让我动手（输入 / 看可用命令）
      </p>
      <div className="mt-4 flex flex-wrap justify-center gap-1.5">
        {STARTERS.map((s) => (
          <button
            key={s}
            type="button"
            className="nm-btn px-2.5 py-1 text-xs text-[var(--t3)]"
            onClick={() => onPick(s)}
          >
            {s}
          </button>
        ))}
      </div>
    </div>
  );
}

/** 渲染体共享（memo / 非 memo 两个入口同一 JSX，性能测试做对照） */
function renderMessageList(
  {
    scrollRef,
    messages,
    viewedBusy,
    copiedIdx,
    verboseLevel,
    subagentSlot,
    onPreviewFile,
    onShowFileChanges,
    onPickStarter,
    onCopy,
    onRemove,
    onOpenTask,
    onOpenExecSession,
    onOpenTrace,
  }: MessageListProps,
  Bubble: ComponentType<MsgBubbleProps>,
) {
  // 子 agent 区域只挂一条：最后一条助手回复（= 最近一轮的落点）。其余气泡
  // 拿到 undefined，memo 浅比较继续命中。全程只有一条助手消息时自然是它。
  const hostIdx = (() => {
    for (let i = messages.length - 1; i >= 0; i--) {
      if (messages[i].role === "assistant") return i;
    }
    return -1;
  })();
  return (
    // chat-scroll：关掉浏览器滚动锚定（理由见 main.css 同名类）。
    // 流式气泡在视口内长高时，锚定会改 scrollTop 补偿，程序化滚动又写一次 ——
    // 两个控制器抢同一份滚动位置，表现为抖动。滚动语义由应用自己接管（贴底意图跟踪）。
    <div
      ref={scrollRef}
      data-testid="chat-scroll"
      className="chat-scroll min-h-0 flex-1 overflow-y-auto"
    >
      {/* 内容限宽 760px 居中：通栏时正文行长在宽窗口下会拉长到难以扫读的行，
          这里把阅读带约束在舒适行长内，滚动条仍占满容器宽 */}
      <div className="mx-auto w-full max-w-[760px] space-y-2.5 px-0.5 pb-1">
        {messages.length === 0 ? (
          <EmptyState onPick={onPickStarter} />
        ) : (
          messages.map((m, i) => {
            return (
              // 消息无稳定 id：流式追加语义依赖数组序（合成 id 会改消息数据流，
              // 违反拆分红线），数组序即渲染序
              <Bubble
                // oxlint-disable-next-line react/no-array-index-key
                key={i}
                msg={m}
                idx={i}
                isCopied={copiedIdx === i}
                busy={viewedBusy}
                verboseLevel={verboseLevel}
                subagentSlot={i === hostIdx ? subagentSlot : undefined}
                onPreviewFile={onPreviewFile}
                onShowFileChanges={onShowFileChanges}
                onCopy={onCopy}
                onRemove={onRemove}
                onOpenTask={onOpenTask}
                onOpenExecSession={onOpenExecSession}
                onOpenTrace={onOpenTrace}
              />
            );
          })
        )}
      </div>
    </div>
  );
}

/** 生产入口：气泡带 memo 边界 */
export function MessageList(props: MessageListProps) {
  return renderMessageList(props, MsgBubble);
}

/** 无 memo 对照（仅供 MessageList.perf.test.tsx 做前后对比；生产代码不消费） */
export function MessageListUnmemoized(props: MessageListProps) {
  return renderMessageList(props, MsgBubbleBase);
}
