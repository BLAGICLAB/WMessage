import { useCallback, useEffect, useRef, useState } from "react";
import { Bell, BrainCircuit, FolderInput, HelpCircle, Sparkles } from "lucide-react";
import type { ReactNode } from "react";
import { EmptyState } from "../EmptyState";
import { handleCommandError } from "../../lib/errorHandler";
import {
  respondWorkflowQuestion,
  type WorkflowQuestionPayload,
} from "../../lib/workflowAsk";
import {
  NOTIFICATIONS_CHANGED_EVENT,
  approveMemoryProposals,
  clearDoneNotifications,
  confirmArtifactBatch,
  dismissNotification,
  enableEvolutionProposal,
  listNotifications,
  rejectMemoryProposals,
} from "../../lib/notifications";
import type { NotificationItem } from "../../lib/notifications";
import { useTauriListen } from "../../lib/useTauriListen";

/** 时间显示：MM-DD HH:mm（通知消息不需要跨年精度） */
function fmtTime(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

const KIND_META: Record<NotificationItem["kind"], { icon: ReactNode; label: string }> = {
  memory_proposal: {
    icon: <BrainCircuit size={15} aria-hidden />,
    label: "记忆提案",
  },
  evolution_proposal: {
    icon: <Sparkles size={15} aria-hidden />,
    label: "自进化提案",
  },
  artifact_bind: {
    icon: <FolderInput size={15} aria-hidden />,
    label: "产物绑定",
  },
  workflow_question: {
    icon: <HelpCircle size={15} aria-hidden />,
    label: "工作流提问",
  },
};

/** 记忆提案 ids（payload.ids: number[]） */
function memoryIds(n: NotificationItem): number[] {
  const ids = n.payload?.ids;
  return Array.isArray(ids) ? ids.filter((v): v is number => typeof v === "number") : [];
}

/** 自进化提案 proposalId（payload.proposalId: string） */
function evolutionProposalId(n: NotificationItem): string | null {
  const id = n.payload?.proposalId;
  return typeof id === "string" ? id : null;
}

/** 产物绑定批次（payload.taskId/taskTitle/paths） */
function artifactBatch(n: NotificationItem): { taskId: string; paths: string[] } | null {
  const taskId = n.payload?.taskId;
  const paths = n.payload?.paths;
  if (typeof taskId !== "string" || !Array.isArray(paths)) return null;
  return { taskId, paths: paths.filter((p): p is string => typeof p === "string") };
}

function StatusBadge({ status }: { status: NotificationItem["status"] }) {
  if (status === "pending") return null;
  const [text, cls] =
    status === "done"
      ? ["已处理", "text-[var(--t5)]"]
      : ["已忽略", "text-[var(--t5)]"];
  return (
    <span className={`shrink-0 text-[11px] ${cls}`}>{text}</span>
  );
}

/** 通用操作按钮组 */
function CardActions({ children }: { children: ReactNode }) {
  return <div className="mt-3 flex items-center justify-end gap-2">{children}</div>;
}

const btnPrimary =
  "nm-btn rounded-[var(--r-sm)] px-3 py-1.5 text-xs text-[var(--t1)] disabled:opacity-50";
const btnGhost =
  "rounded-[var(--r-sm)] px-3 py-1.5 text-xs text-[var(--t3)] hover:bg-[var(--hover-bg)] disabled:opacity-50";

/** 记忆提案卡：全部收下 / 忽略（走 mem_pending_approve/reject，后端回写消息状态） */
function MemoryCard({ item }: { item: NotificationItem }) {
  const ids = memoryIds(item);
  const [busy, setBusy] = useState(false);
  const act = async (approve: boolean) => {
    setBusy(true);
    try {
      if (approve) await approveMemoryProposals(ids);
      else await rejectMemoryProposals(ids);
    } catch (e) {
      handleCommandError(e, approve ? "收下记忆提案" : "忽略记忆提案");
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <CardActions>
        <button type="button" className={btnGhost} disabled={busy || ids.length === 0} onClick={() => act(false)}>
          忽略
        </button>
        <button type="button" className={btnPrimary} disabled={busy || ids.length === 0} onClick={() => act(true)}>
          全部收下（{ids.length}）
        </button>
      </CardActions>
    </>
  );
}

/** 自进化提案卡：启用（toggle ON，立即落 lesson）/ 忽略 */
function EvolutionCard({ item }: { item: NotificationItem }) {
  const proposalId = evolutionProposalId(item);
  const [busy, setBusy] = useState(false);
  const enable = async () => {
    if (!proposalId) return;
    setBusy(true);
    try {
      await enableEvolutionProposal(proposalId);
    } catch (e) {
      handleCommandError(e, "启用自进化提案");
    } finally {
      setBusy(false);
    }
  };
  const dismiss = async () => {
    setBusy(true);
    try {
      await dismissNotification(item.id);
    } catch (e) {
      handleCommandError(e, "忽略自进化提案");
    } finally {
      setBusy(false);
    }
  };
  return (
    <CardActions>
      <button type="button" className={btnGhost} disabled={busy} onClick={dismiss}>
        忽略
      </button>
      <button type="button" className={btnPrimary} disabled={busy || !proposalId} onClick={enable}>
        启用
      </button>
    </CardActions>
  );
}

/** 产物绑定卡：内嵌复选列表（默认全选）+ 绑定选中 / 跳过 */
function ArtifactCard({ item }: { item: NotificationItem }) {
  const batch = artifactBatch(item);
  const [selected, setSelected] = useState<Set<string>>(() => new Set(batch?.paths ?? []));
  const [busy, setBusy] = useState(false);
  if (!batch) return <p className="mt-2 text-xs text-[var(--t4)]">消息数据不完整，可忽略</p>;
  const toggle = (p: string) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(p)) next.delete(p);
      else next.add(p);
      return next;
    });
  const allChecked = batch.paths.length > 0 && batch.paths.every((p) => selected.has(p));
  const bind = async () => {
    if (selected.size === 0) return;
    setBusy(true);
    try {
      await confirmArtifactBatch(batch.taskId, Array.from(selected));
    } catch (e) {
      handleCommandError(e, "绑定产物到任务卡");
    } finally {
      setBusy(false);
    }
  };
  const skip = async () => {
    setBusy(true);
    try {
      await dismissNotification(item.id);
    } catch (e) {
      handleCommandError(e, "跳过产物绑定");
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <div className="mt-3 flex items-center gap-2">
        <input
          id={`all-${item.id}`}
          type="checkbox"
          checked={allChecked}
          onChange={() =>
            setSelected(allChecked ? new Set() : new Set(batch.paths))
          }
          disabled={busy}
        />
        <label htmlFor={`all-${item.id}`} className="text-xs text-[var(--t3)]">
          全选
        </label>
        <span className="ml-auto text-[11px] text-[var(--t5)]">
          已选 {selected.size} / {batch.paths.length}
        </span>
      </div>
      <ul className="mt-2 space-y-1.5">
        {batch.paths.map((p) => (
          <li key={p} className="flex items-start gap-2">
            <input
              type="checkbox"
              checked={selected.has(p)}
              onChange={() => toggle(p)}
              disabled={busy}
              className="mt-0.5"
              aria-label={p}
            />
            <span className="min-w-0 flex-1 break-all font-mono text-xs text-[var(--t3)]">{p}</span>
          </li>
        ))}
      </ul>
      <CardActions>
        <button type="button" className={btnGhost} disabled={busy} onClick={skip}>
          跳过
        </button>
        <button type="button" className={btnPrimary} disabled={busy || selected.size === 0} onClick={bind}>
          {busy ? "绑定中…" : `绑定选中（${selected.size}）`}
        </button>
      </CardActions>
    </>
  );
}

/** 工作流提问 payload（payload.questionId 必有，缺失 = 数据不完整可忽略） */
function workflowQuestion(n: NotificationItem): WorkflowQuestionPayload | null {
  const p = n.payload;
  if (!p || typeof p.questionId !== "string") return null;
  return p as unknown as WorkflowQuestionPayload;
}

/** 工作流提问卡（W9-ASK）：选项 chips + 自由输入 + 按假设继续——
 *  红线：忽略问题工作流也能走（assume 返回问题自带假设） */
function WorkflowQuestionCard({ item }: { item: NotificationItem }) {
  const q = workflowQuestion(item);
  const [picked, setPicked] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  if (!q) return <p className="mt-2 text-xs text-[var(--t4)]">消息数据不完整，可忽略</p>;
  const options = Array.isArray(q.options) ? q.options : [];
  const answerText = text.trim() || picked;
  const respond = async (action: "answer" | "assume") => {
    setBusy(true);
    try {
      await respondWorkflowQuestion(
        q.questionId,
        action,
        action === "answer" ? answerText || undefined : undefined
      );
    } catch (e) {
      handleCommandError(e, "回答工作流提问");
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      {q.why && <p className="mt-2 text-[11px] text-[var(--t5)]">问这个：{q.why}</p>}
      {q.nodeTitle && (
        <p className="mt-1 text-[11px] text-[var(--t5)]">提问节点：{q.nodeTitle}</p>
      )}
      {options.length > 0 && (
        <div className="mt-2 flex flex-wrap gap-1.5">
          {options.map((opt) => (
            <button
              key={opt}
              aria-pressed={picked === opt}
              className={`rounded-full px-3 py-1 text-xs ${
                picked === opt
                  ? "nm-outset text-[var(--t1)]"
                  : "nm-inset text-[var(--t3)]"
              }`}
              disabled={busy}
              onClick={() => {
                setPicked(opt);
                setText("");
              }}
            >
              {opt}
            </button>
          ))}
        </div>
      )}
      <input
        aria-label="自定义回答"
        className="mt-2 w-full rounded-[var(--r-sm)] px-3 py-1.5 text-xs nm-inset text-[var(--t1)] outline-none placeholder:text-[var(--t5)]"
        maxLength={200}
        placeholder={options.length ? "或输入其他回答" : "输入你的回答"}
        value={text}
        disabled={busy}
        onChange={(e) => {
          setText(e.target.value);
          setPicked(null);
        }}
      />
      <CardActions>
        <button
          type="button"
          className={btnGhost}
          disabled={busy}
          title={q.assumption ? `不回答，按 AI 假设继续：${q.assumption}` : "不回答，按继续处理"}
          onClick={() => void respond("assume")}
        >
          按假设继续
        </button>
        <button
          type="button"
          className={btnPrimary}
          disabled={busy || !answerText}
          onClick={() => void respond("answer")}
        >
          {busy ? "提交中…" : "回答并继续"}
        </button>
      </CardActions>
    </>
  );
}

/** Agent 通知中心：三类决策消息（记忆提案 / 自进化提案 / 产物绑定）的持久化收件箱 */
export function NotificationsPage() {
  const [items, setItems] = useState<NotificationItem[]>([]);
  const [tab, setTab] = useState<"pending" | "all">("pending");
  const [loadError, setLoadError] = useState("");

  // 竞态令牌：挂载 + 事件风暴并发 reload 时，慢的旧响应不得覆盖新响应
  const reloadSeqRef = useRef(0);
  const reload = useCallback(() => {
    const seq = ++reloadSeqRef.current;
    listNotifications()
      .then((list) => {
        if (seq !== reloadSeqRef.current) return;
        setItems(list);
        setLoadError("");
      })
      .catch((e) => {
        if (seq !== reloadSeqRef.current) return;
        setLoadError(String(e));
        console.error("[notifications] 列表加载失败:", e);
      });
  }, []);
  useEffect(() => {
    void reload();
  }, [reload]);
  useTauriListen(NOTIFICATIONS_CHANGED_EVENT, reload);

  const shown = tab === "pending" ? items.filter((n) => n.status === "pending") : items;
  const pendingCount = items.filter((n) => n.status === "pending").length;

  const clearDone = async () => {
    try {
      await clearDoneNotifications();
    } catch (e) {
      handleCommandError(e, "清空已处理通知");
    }
  };

  return (
    <div className="mx-auto max-w-3xl">
      <div className="flex items-center gap-3">
        <h1 className="text-xl font-semibold text-[var(--t1)]">通知</h1>
        <span className="text-xs text-[var(--t4)]">{pendingCount} 条待处理</span>
        <div className="flex-1" />
        {tab === "all" && items.some((n) => n.status !== "pending") && (
          <button type="button" className={btnGhost} onClick={clearDone}>
            清空已处理
          </button>
        )}
        <div role="tablist" aria-label="通知筛选" className="flex items-center gap-1">
          {(
            [
              ["pending", "待处理"],
              ["all", "全部"],
            ] as const
          ).map(([key, label]) => (
            <button
              key={key}
              type="button"
              role="tab"
              aria-selected={tab === key}
              onClick={() => setTab(key)}
              className={`rounded-[var(--r-sm)] px-2.5 py-1 text-xs transition-colors duration-100 ${
                tab === key
                  ? "nm-inset text-[var(--t1)]"
                  : "text-[var(--t3)] hover:bg-[var(--hover-bg)]"
              }`}
            >
              {label}
            </button>
          ))}
        </div>
      </div>

      {loadError && (
        <p className="mt-4 text-sm text-red-400">通知加载失败：{loadError}</p>
      )}

      <div className="mt-4 space-y-3">
        {shown.length === 0 ? (
          <EmptyState
            icon={<Bell size={18} aria-hidden />}
            title={tab === "pending" ? "没有待处理的通知" : "还没有通知"}
            description="Agent 的记忆提案、自进化提案与产物绑定会以消息形式出现在这里"
          />
        ) : (
          shown.map((n) => {
            const meta = KIND_META[n.kind];
            return (
              <article
                key={n.id}
                className={`nm-card p-4 ${n.status !== "pending" ? "opacity-60" : ""}`}
                aria-label={`${meta.label}：${n.title}`}
              >
                <header className="flex items-center gap-2">
                  <span className="text-[var(--t3)]">{meta.icon}</span>
                  <span className="text-[11px] text-[var(--t5)]">{meta.label}</span>
                  <span className="text-[11px] text-[var(--t5)]">· {fmtTime(n.createdAt)}</span>
                  <span className="flex-1" />
                  <StatusBadge status={n.status} />
                </header>
                <h2 className="mt-2 text-sm font-medium text-[var(--t1)]">{n.title}</h2>
                {n.body && <p className="mt-1 text-xs text-[var(--t3)]">{n.body}</p>}
                {n.status === "pending" &&
                  (n.kind === "memory_proposal" ? (
                    <MemoryCard item={n} />
                  ) : n.kind === "evolution_proposal" ? (
                    <EvolutionCard item={n} />
                  ) : n.kind === "workflow_question" ? (
                    <WorkflowQuestionCard item={n} />
                  ) : (
                    <ArtifactCard item={n} />
                  ))}
              </article>
            );
          })
        )}
      </div>
    </div>
  );
}
