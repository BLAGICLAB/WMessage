// ChatPanel 子模块：消息列表（ 拆分自 ChatPanel.tsx，JSX 逐字搬移）。
// MsgBubble 用 React.memo 做流式性能边界：流式增量每帧只替换最后一
// 条消息的对象引用，历史气泡 props 全等 → memo 跳过重渲染（MarkdownText/
// RichText 的 markdown 解析是重活，N 条消息 × 每帧一次的全列表重渲染是
// 长会话掉帧主因）。回调句柄由父级 useCallback 固定，memo 浅比较即可命中。

import { memo, useMemo, useState, type ComponentType } from "react";
import {
  Check,
  Copy,
  FileText,
  MessageCircle,
  MessagesSquare,
  Pin,
  Trash2,
  TriangleAlert,
} from "lucide-react";
import { basename } from "../../format";
import { extractFilePaths, openTarget } from "../../lib/openTarget";
import { MarkdownText } from "../MarkdownText";
import { Fold } from "./Fold";
import { RichText } from "./RichText";
import { UserBubbleContent } from "./UserBubbleContent";
import type {
  FileChangeLite,
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
};

function MsgBubbleBase({
  msg: m,
  idx,
  isCopied,
  busy,
  verboseLevel,
  onCopy,
  onRemove,
  onOpenTask,
  onOpenExecSession,
}: MsgBubbleProps) {
  // 一次性算文件路径 + 是否显示操作行（避免在渲染条件里 IIFE + mutation m._fps 的反模式）；
  // 流式末条气泡每帧重渲染，useMemo 让路径正则只在内容变化时重跑
  const fps = useMemo(() => extractFilePaths(m.content), [m.content]);
  const hasContent = !!m.content.trim();
  const showActions =
    m.role === "assistant" &&
    !m.streaming &&
    (hasContent || (m.refs?.length ?? 0) > 0 || fps.length > 0);
  /** 气泡正文：用户消息走浅底渲染、流式走轻量富文本、完成走 markdown（拆分前的
   *  嵌套三元提取成函数，渲染输出逐字不变） */
  function bubbleBody(msg: Msg) {
    if (!msg.content) return msg.streaming ? "…" : "";
    if (msg.role === "user") return <UserBubbleContent content={msg.content} />;
    if (msg.streaming) return <RichText text={msg.content} />;
    return <MarkdownText text={msg.content} />;
  }
  return (
    <div
      className={
        m.role === "user" ? "max-w-[85%] ml-auto" : "max-w-full mr-auto"
      }
    >
      <div
        className={`chat-bubble text-xs leading-relaxed whitespace-pre-wrap break-words ${
          m.role === "user" ? "chat-bubble-user" : "chat-bubble-bot"
        }`}
      >
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
          <FileSummary fps={fps} changes={m.fileChanges} />
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
  );
}

/** memo 边界：流式更新时历史气泡 props 全等 → 跳过重渲染（见文件头注释） */
const MsgBubble = memo(MsgBubbleBase);

/** 工具调用（对齐截图）：mono pill 徽章行（名称 + ✓/✗/… 状态 + 调试档耗时），
 *  折叠「进程 N/M」承载逐工具入参/结果详情。
 *  verbose 三档：简洁=隐藏详情折叠；详细=现状；调试=默认展开+耗时。
 *  memo：文本流式 tick 不改 tools 引用，跳过徽章行重渲染 */
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
  if (verboseLevel === "concise") {
    return (
      <div className="mb-1 flex flex-wrap gap-1">
        {tools.map((t) => (
          <span
            key={t.id}
            className="inline-flex items-center gap-1 rounded-full border border-[var(--edge)] px-2 py-0.5 font-mono text-[10px] leading-4 text-[var(--t4)]"
          >
            {t.name || "tool"}
            <span
              aria-hidden
              className={
                t.ok === false
                  ? "text-[var(--danger,#ef4444)]"
                  : t.done
                    ? "text-[var(--success)]"
                    : ""
              }
            >
              {t.ok === false ? (
                "✕"
              ) : t.done ? (
                <Check size={10} strokeWidth={3} />
              ) : (
                "…"
              )}
            </span>
          </span>
        ))}
      </div>
    );
  }
  return (
    <div className="mb-1 space-y-1">
      <div className="flex flex-wrap gap-1">
        {tools.map((t) => (
          <span
            key={t.id}
            className="inline-flex items-center gap-1 rounded-full border border-[var(--edge)] px-2 py-0.5 font-mono text-[10px] leading-4 text-[var(--t4)]"
          >
            {t.name || "tool"}
            <span
              aria-hidden
              className={
                t.ok === false
                  ? "text-[var(--danger,#ef4444)]"
                  : t.done
                    ? "text-[var(--success)]"
                    : ""
              }
            >
              {t.ok === false ? (
                "✕"
              ) : t.done ? (
                <Check size={10} strokeWidth={3} />
              ) : (
                "…"
              )}
            </span>
          </span>
        ))}
      </div>
      <Fold
        title={
          <span className="font-mono">
            进程 {done}/{tools.length}
            {failed > 0 && (
              <span className="text-[var(--danger,#ef4444)]">
                {" "}
                · {failed} 失败
              </span>
            )}
            {verboseLevel === "debug" &&
              totalMs > 0 &&
              ` · Σ ${fmtMs(totalMs)}`}
          </span>
        }
        defaultOpen={verboseLevel === "debug"}
      >
        <div className="space-y-1">
          {tools.map((t) => (
            <div key={t.id}>
              <span className="font-mono text-[10px] text-[var(--t4)]">
                {t.name || "tool"}
                {t.ms != null && (
                  <span className="ml-1 text-[var(--t5)]">{fmtMs(t.ms)}</span>
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
      </Fold>
    </div>
  );
});

function fmtMs(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

/** 文件变更摘要条（对齐截图）：≤2 个文件平铺 pill，更多则折叠为
 *  「📄 N 个文件」摘要条（点击展开）。
 *  P2-b：优先用结构化 fileChanges（bot-file-changed 事件，带 ±行统计）；
 *  无结构化数据时回退正文正则抽取（主聊天无 trace，行为不变） */
function FileSummary({
  fps,
  changes,
}: {
  fps: string[];
  changes?: FileChangeLite[];
}) {
  const [open, setOpen] = useState(false);
  const structured = changes ?? [];
  const pillOf = (f: string) => (
    <button
      key={f}
      type="button"
      className="nm-btn inline-flex items-center gap-1 rounded-lg px-2 py-0.5 text-[10px] text-[var(--t3)] max-w-full"
      title={`打开文件：${f}`}
      onClick={() => {
        // Rust 侧打开（挂件窗口 openPath 前端权限可能被拒）；失败弹错不静默
        openTarget(f);
      }}
    >
      <span className="truncate max-w-[280px]">
        <FileText
          size={11}
          aria-hidden
          className="mr-1 inline text-[var(--t4)]"
        />
        {basename(f)}
      </span>
    </button>
  );
  const structuredPillOf = (c: FileChangeLite) => (
    <button
      key={c.path}
      type="button"
      className="nm-btn inline-flex items-center gap-1 rounded-lg px-2 py-0.5 text-[10px] text-[var(--t3)] max-w-full"
      title={`打开文件：${c.path}`}
      onClick={() => openTarget(c.path)}
    >
      <span className="truncate max-w-[240px]">
        <FileText
          size={11}
          aria-hidden
          className="mr-1 inline text-[var(--t4)]"
        />
        {basename(c.path)}
      </span>
      <span className="shrink-0 font-mono tabular-nums">
        <span className="text-[var(--ok,#22c55e)]">+{c.added}</span>{" "}
        <span className="text-[var(--danger,#ef4444)]">−{c.deleted}</span>
      </span>
    </button>
  );
  const items =
    structured.length > 0 ? structured.map(structuredPillOf) : fps.map(pillOf);
  // 统一 DOM 形状（w-full 容器），≤2 与 >2 只是容器内子元素不同
  return (
    <div className="w-full">
      {items.length <= 2 ? (
        <div className="flex flex-wrap gap-1">{items}</div>
      ) : (
        <>
          <button
            type="button"
            className="nm-btn inline-flex items-center gap-1 rounded-lg px-2 py-0.5 font-mono text-[10px] text-[var(--t3)]"
            aria-expanded={open}
            title={open ? "收起文件列表" : "展开文件列表"}
            onClick={() => setOpen((v) => !v)}
          >
            <FileText size={11} aria-hidden />
            {items.length} 个文件
            <span aria-hidden>{open ? "▴" : "▾"}</span>
          </button>
          {open && <div className="mt-1 flex flex-wrap gap-1">{items}</div>}
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
};

/** 渲染体共享（memo / 非 memo 两个入口同一 JSX，性能测试做对照） */
function renderMessageList(
  {
    scrollRef,
    messages,
    viewedBusy,
    copiedIdx,
    verboseLevel,
    onCopy,
    onRemove,
    onOpenTask,
    onOpenExecSession,
  }: MessageListProps,
  Bubble: ComponentType<MsgBubbleProps>,
) {
  return (
    <div
      ref={scrollRef}
      className="flex-1 min-h-0 overflow-y-auto space-y-2.5 pr-0.5"
    >
      {messages.length === 0 ? (
        <p className="text-xs text-[var(--t5)] text-center mt-6">
          跟我说：新建任务、列出任务、完成某任务…（输入 / 看可用命令）
        </p>
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
              onCopy={onCopy}
              onRemove={onRemove}
              onOpenTask={onOpenTask}
              onOpenExecSession={onOpenExecSession}
            />
          );
        })
      )}
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
