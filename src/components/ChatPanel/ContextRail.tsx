// ChatPanel 子模块：右侧上下文栏（Agent workbench 式「对话锚点 + 工作面」）。
// 三类工作面切换：任务池 / 文件 / 子 Agent。栏宽占聊天区 1/3——固定 px 在
// 大窗口下显得局促，而这一栏要看的是任务标题与文件路径，宽一点才读得完。
//
// 「终端」「审查」不在此列：项目内无后端命令也无数据源，留空壳入口会变成
// 永远点不动的死 UI（那是从零做 PTY / diff 模型的事，另有工程）。

import { useEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Bot,
  Check,
  FileText,
  FolderOpen,
  ListTodo,
  Loader2,
  Paperclip,
} from "lucide-react";
import { basename } from "../../format";
import { formatCommandError, handleCommandError } from "../../lib/errorHandler";
import { FilePreview } from "./FilePreview";
import { FileDiffView } from "./FileDiffView";
import { mergeDeliverables, collectDeclaredPaths } from "../../lib/deliverables";
import { useProbedDeliverables } from "../../lib/useDeliverables";
import {
  sessionFileChangeDetail,
  sessionFileChangeList,
  type SessionFileChangeItem,
  type SessionFileDiff,
} from "../../lib/sessionFileChanges";
import { useTauriListen } from "../../lib/useTauriListen";
import {
  applyBinding,
  filesPatch,
  taskFiles,
  type TaskFile,
} from "../../lib/taskFiles";
import type { Task } from "../../types";
import {
  SubagentCancelButton,
  SubagentElapsed,
} from "./SubagentRowLine";
import { SUBAGENT_STATUS } from "./constants";
import { SubagentDetailPanel } from "./SubagentDetailPanel";
import type { SubagentActivityItem } from "../../lib/subagentActivity";

/** 三类工作面。key 用窄口径字符串，不另立枚举 */
export type ContextTab = "tasks" | "files" | "agents";

type Tab = ContextTab;

/** 文件页的两个子页：改动（轨迹）/ 交付物（产物） */
export type FileSubTab = "changes" | "deliverables";

export type SubagentRun = {
  /** 子 agent id（sa_ 前缀）——点开查详情/取消都用它 */
  id: string;
  title: string;
  /** 行上六态原值 */
  status: "queued" | "running" | "succeeded" | "failed" | "cancelled" | "budget_exceeded";
  running: boolean;
  error?: string | null;
  /** 已跑轮数；在途才有（终态守卫注销后读不到，报 null 不报 0） */
  roundsUsed?: number | null;
  /** 轮数硬顶，与轮数同报 */
  roundsBudget?: number | null;
  /** 已执行工具调用数与硬顶，口径同轮数（收尾窗从 N/N 起开） */
  toolCallsUsed?: number | null;
  toolCallsBudget?: number | null;
  /** 毫秒时间戳；排队期未开跑故 startedAt 可空 */
  startedAt?: number | null;
  finishedAt?: number | null;
  /** 派它出来的子 agent 身份与契约原文（详情视图用；列表不消费） */
  profile?: string;
  contextSummary?: string | null;
  /** 验收标准：库里是 JSON 数组字符串，映射时解析；解析失败/形状不符落回原文，
   *  故类型含 Record 分支（渲染侧按非数组兜底转文本） */
  acceptance?: string | string[] | Record<string, unknown> | null;
  /** 收尾结构化结果：库里是 JSON 对象字符串，同上（含数组/原文兜底） */
  result?: string | string[] | Record<string, unknown> | null;
  traceId?: string | null;
  /** 最近动作流（useChatSubagents 只给在途行挂数据；事件即焚不落库）——
   *  详情页「执行详情」窗口消费，列表行不渲染 */
  activity?: SubagentActivityItem[];
};

type ContextRailProps = {
  tasks: Task[];
  onUpdateTask: (taskId: string, patch: Partial<Task>) => void;
  /** 点任务标题 = 把它引用进当前消息（同附件：摆进对话让机器人看着办），
   *  不是「立刻让机器人去做」——那是任务卡 🤖 的活 */
  onRefTask: (t: { id: string; title: string }) => void;
  /** 已在待发引用里的任务 id（标出点过的项） */
  refIds: string[];
  /** 本会话 id：文件改动清单按会话查后端痕迹表。
   *  口径由后端定（累计 / 逐次 / 无行级 diff 三档），前端不自己累加——前端那份
   *  是「后者覆盖前者」的单次写入数字，与详情里的累计 diff 对不上。 */
  sessionId: string | null;
  /** 子 agent 运行列表（含在途与刚结束），按新到旧 */
  subagents: SubagentRun[];
  /** 点子 agent 条目 → 右侧栏切到它的详情视图 */
  onOpenSubagent: (id: string) => void;
  /** 取消在途子 agent */
  onCancelSubagent: (id: string) => void;
  /** 当前钉住详情的子 agent（null = 停在列表页）。取不到对应行时回落列表，
   *  详情页不该因为一次重拉把用户困在没有返回按钮的空白里。 */
  subagentDetailId?: string | null;
  onCloseSubagentDetail?: () => void;
  /** 当前工作面（受控）：内嵌子 agent 区域要点侧栏切类别，状态得在父级 */
  tab: Tab;
  onTabChange: (t: Tab) => void;
  /** 预览请求（受控，null = 停在列表页）：打开哪个文件、看哪一面。
   *  对话正文里的路径链接与气泡交付物区也要能打开它，预览的归属方因此不能
   *  只在本组件内。与子 Agent 详情同款：详情是一层，返回才回列表。
   *
   *  **每次打开都是一个新对象**：父级重新发请求 = 用户又点了一次，栏内据此
   *  重置「看内容 / 看改动」。用两个字段（路径 + 模式）时，同一个文件从不同
   *  入口再点一次就分不清是哪次的意图了。 */
  previewReq?: PreviewRequest | null;
  onPreviewReq?: (req: PreviewRequest | null) => void;
  /** 文件页当前子页：改动（轨迹）/ 交付物（产物） */
  fileSubTab?: FileSubTab;
  onFileSubTabChange?: (t: FileSubTab) => void;
  /** 本会话的助手消息正文（会话级交付物清单的「正文自报」那半来源） */
  sessionMessages?: { role: string; content: string }[];
};

/** 预览请求：打开哪个文件 + 看哪一面 */
export type PreviewRequest = {
  /** 要预览的文件路径 */
  path: string;
  /** 看内容还是看改动：由点击来源决定（交付物/正文路径 → 内容，改动清单 → 差异） */
  mode: "content" | "diff";
};

const TABS: { key: Tab; label: string; icon: ReactNode }[] = [
  { key: "tasks", label: "任务", icon: <ListTodo size={14} /> },
  { key: "files", label: "文件", icon: <FileText size={14} /> },
  { key: "agents", label: "子 Agent", icon: <Bot size={14} /> },
];

/** `sessionMessages` 的缺省值：模块级常量。每次渲染新建数组会破引用相等，
 *  会话级交付物清单的 memo 会跟着每帧失效。 */
const NO_SESSION_MESSAGES: { role: string; content: string }[] = [];

function EmptyHint({ children }: { children: ReactNode }) {
  return <p className="px-1.5 py-2 text-[11px] text-[var(--t5)]">{children}</p>;
}

/** 文件类型铭牌：行首 24px 方块亮出扩展名，是文件页唯一的身份设备。
 *  两档功能色——文档（md/pdf/office）brand、数据（json/csv/表格）success，
 *  代码与未知走中性灰不抢戏；改动页每行挂文件夹图标的老毛病顺手修掉 */
const FILE_BADGE_TONES: Record<string, CSSProperties> = {
  doc: {
    background: "color-mix(in srgb, var(--brand) 15%, transparent)",
    color: "var(--brand)",
  },
  data: {
    background: "color-mix(in srgb, var(--success) 14%, transparent)",
    color: "var(--success)",
  },
  other: { background: "var(--hover-bg)", color: "var(--t4)" },
};
const FILE_DOC_EXTS = new Set([
  "md", "txt", "pdf", "doc", "docx", "ppt", "pptx", "rtf",
]);
const FILE_DATA_EXTS = new Set([
  "json", "csv", "tsv", "xlsx", "xls", "yml", "yaml", "toml",
]);

function FileTypeBadge({ path }: { path: string }) {
  // 扩展名从显示名取（与行内 basename 同源），/a.b/报告 这类无扩展名路径不误判
  const name = basename(path);
  const ext = name.includes(".")
    ? name.slice(name.lastIndexOf(".") + 1).toLowerCase()
    : "";
  const tone = FILE_DOC_EXTS.has(ext)
    ? FILE_BADGE_TONES.doc
    : FILE_DATA_EXTS.has(ext)
      ? FILE_BADGE_TONES.data
      : FILE_BADGE_TONES.other;
  return (
    <span
      aria-hidden
      className="grid h-6 w-6 shrink-0 place-items-center rounded-md font-mono text-[8px] font-medium tracking-wide"
      style={tone}
    >
      {ext ? ext.slice(0, 4).toUpperCase() : "?"}
    </span>
  );
}

/** 文件页空态：虚线框 + 主行 + 一句去向——空屏是邀请，不是死文本 */
function FilePageEmpty({ main, sub }: { main: string; sub: string }) {
  return (
    <div className="rounded-xl border border-dashed border-[var(--edge)] px-3 py-4 text-center">
      <p className="text-[11px] text-[var(--t4)]">{main}</p>
      <p className="mt-0.5 text-[10px] text-[var(--t6)]">{sub}</p>
    </div>
  );
}

/**
 * 文件页的子页切换钮（下划线式）。
 *
 * 与一级类别钮（药丸式）刻意分开：子页嵌在类别里，两层都用药丸会看出
 * 层级。计数为 null 时**不显示数字**——探测没回来时的「0」会被读成
 * 「确实没有交付物」。
 */
function FileSubTabButton({
  label,
  count,
  active,
  onClick,
}: {
  label: string;
  count: number | null;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      className={`-mb-px flex items-center gap-1 border-b-2 pb-1 text-[11px] transition-colors duration-[var(--dur-chip)] ease-[var(--ease-out)] ${
        active
          ? "border-[var(--brand)] text-[var(--t1)]"
          : "border-transparent text-[var(--t3)] hover:text-[var(--t2)]"
      }`}
      onClick={onClick}
    >
      {label}
      {count != null && (
        <span className="font-mono text-[10px] text-[var(--t5)]">{count}</span>
      )}
    </button>
  );
}

export function ContextRail({
  tasks,
  onUpdateTask,
  onRefTask,
  refIds,
  sessionId,
  subagents,
  onOpenSubagent,
  onCancelSubagent,
  subagentDetailId,
  onCloseSubagentDetail,
  tab,
  onTabChange,
  previewReq = null,
  onPreviewReq,
  fileSubTab = "changes",
  onFileSubTabChange,
  sessionMessages = NO_SESSION_MESSAGES,
}: ContextRailProps) {
  const runningCount = subagents.filter((s) => s.running).length;

  // ── 预览视图（父级发请求，栏内自己切面）────────────────────────────
  // 父级发来的是**请求**（打开哪个文件 + 看哪一面），栏内把它落成当前视图。
  // 视图是栏内的：预览里的「看全文 / 看改动」是同一次打开内部的动作，不该
  // 反过来改父级状态；栏内自己点开别的文件时，再把请求回报父级。
  //
  // 父级每次「打开」都发一个**新对象**，栏内按对象身份判定「这是一次新的
  // 打开」并丢掉上一次留下的视图——否则「点交付物看内容」之后在预览里切到
  // 「看改动」，再点一次同一个交付物就还是差异视图，用户第二次点击的意图
  // 被上一次残留吃掉了。
  const [view, setView] = useState<PreviewRequest | null>(previewReq);
  const [lastReq, setLastReq] = useState<PreviewRequest | null>(previewReq);
  if (previewReq !== lastReq) {
    setLastReq(previewReq);
    setView(previewReq);
  }
  /** 打开一个文件：栏内立刻切视图，同时把请求回报父级（父级据此记「当前打开谁」） */
  const openPreview = (path: string, mode: "content" | "diff") => {
    setView({ path, mode });
    onPreviewReq?.({ path, mode });
  };
  /** 同一份预览内部换一面（看全文 ↔ 看改动）：不惊动父级 */
  const switchFace = (mode: "content" | "diff") => {
    if (!view) return;
    setView({ path: view.path, mode });
  };
  /** 回到文件列表 */
  const closePreview = () => {
    setView(null);
    onPreviewReq?.(null);
  };
  // 最新 tasks 镜像：绑定对话框挂起期间 files 可能被别处改动（另一窗口/机器人），
  // 合并基线必须取对话框返回后的现值，而非点击时的快照（ref 写放 effect）
  const tasksRef = useRef(tasks);
  useEffect(() => {
    tasksRef.current = tasks;
  });

  // ── 文件改动工作面（清单 + 详情）────────────────────────────────────
  // 清单口径在后端（累计 ±）；前端只负责取数与呈现，不自己累加。
  const [fileItems, setFileItems] = useState<SessionFileChangeItem[]>([]);
  const [fileListError, setFileListError] = useState<string | null>(null);
  const [fileListLoading, setFileListLoading] = useState(false);
  /** 清单已经取回的是哪个会话（null = 还没取过）：判断「这个路径改没改」要用 */
  const [listLoadedFor, setListLoadedFor] = useState<string | null>(null);
  const [fileDetail, setFileDetail] = useState<SessionFileDiff | null>(null);
  const [fileDetailError, setFileDetailError] = useState<string | null>(null);
  const [fileDetailLoading, setFileDetailLoading] = useState(false);
  /** 最新 sessionId 镜像：事件回调只捕获一次（useTauriListen 的约定），读 ref 拿当前值 */
  const sessionIdRef = useRef(sessionId);
  useEffect(() => {
    sessionIdRef.current = sessionId;
  }, [sessionId]);
  /** 落盘事件计数：AI 又改了文件 → 清单要跟着长 */
  const [fileChangeTick, setFileChangeTick] = useState(0);
  useTauriListen<{ sessionId?: string | null }>("bot-file-changed", (payload) => {
    const sid = payload.sessionId ?? null;
    if (sid && sid === sessionIdRef.current) setFileChangeTick((t) => t + 1);
  });

  // 清单：切到「文件」页 / 切会话 / AI 又改了文件 → 重取。
  // 防抖 200ms：一次回合里连着落盘十来个文件时，别把每个事件都变成一次全量重算。
  useEffect(() => {
    if (tab !== "files" || !sessionId) return;
    let alive = true;
    const timer = setTimeout(() => {
      setFileListLoading(true);
      sessionFileChangeList(sessionId)
        .then((r) => {
          if (!alive) return;
          setFileItems(r?.items ?? []);
          setFileListError(null);
        })
        .catch((e) => {
          if (!alive) return;
          handleCommandError(e, "读取会话文件改动", { silent: true });
          setFileListError(formatCommandError(e) || "读取失败");
        })
        .finally(() => {
          if (!alive) return;
          setFileListLoading(false);
          // 清单回来了（成功或失败）——失败也标上：这时该退到全文视图，
          // 而不是一直卡在「读取中」
          setListLoadedFor(sessionId);
        });
    }, 200);
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [tab, sessionId, fileChangeTick]);

  // ── 产物目录入口（打开 AI_Gen_Files）────────────────────────────────
  // 路径由后端解析后回传：便携模式下数据目录跟着 exe 走，前端拼不出来。
  const [genDir, setGenDir] = useState<string | null>(null);
  const [genDirError, setGenDirError] = useState<string | null>(null);
  const openGenDir = async () => {
    try {
      const p = await invoke<string>("bot_open_gen_dir");
      setGenDir(p);
      setGenDirError(null);
    } catch (e) {
      handleCommandError(e, "打开产物文件夹", { silent: true });
      setGenDirError(formatCommandError(e) || "打开失败");
    }
  };

  // ── 会话级交付物清单（右侧栏「交付物」页）──────────────────────────
  // 每次渲染都算：贵的那步（逐条正文正则）由 collectDeclaredPaths 的
  // 消息对象缓存吃掉——流式输出只重扫正在流的那一条，历史消息不重扫。
  // 剩下的只是 O(消息数 + 路径数) 的查表与去重。
  const sessionDeliverables = useMemo(() => {
    const declared = collectDeclaredPaths(sessionMessages);
    const produced = fileItems.map((f) => f.path);
    return mergeDeliverables(declared, produced);
  }, [sessionMessages, fileItems]);
  const sessionProbed = useProbedDeliverables(sessionDeliverables, true);

  // 详情：点开某个改过的文件 → 取它的累计 diff（未改过的路径不在清单里，
  // 由渲染处直接走全文视图，不发这次请求）
  useEffect(() => {
    const path = view?.path;
    if (!path || !sessionId) return;
    const hit = fileItems.some((i) => i.path === path);
    if (!hit) return;
    let alive = true;
    setFileDetailLoading(true);
    setFileDetailError(null);
    sessionFileChangeDetail(sessionId, path)
      .then((r) => {
        if (!alive) return;
        setFileDetail(r.detail ?? null);
      })
      .catch((e) => {
        if (!alive) return;
        handleCommandError(e, "读取文件改动", { silent: true });
        setFileDetailError(formatCommandError(e) || "读取失败");
      })
      .finally(() => {
        if (alive) setFileDetailLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [view?.path, sessionId, fileItems]);

  // 换文件就清掉上一份的差异详情：那是上一份文件的遗留态。
  // 看哪一面（内容/改动）不在这里重置——由点击来源决定（交付物 → 内容，
  // 改动清单 → 差异），重置会把用户的意图冲掉。
  useEffect(() => {
    setFileDetail(null);
  }, [view?.path]);

  /** 聊天窗口直接把本地文件绑到任务卡：选文件 → bind_files 探测 isDir →
   *  统一内核合并（去重/文件夹槽替换/10 上限）→ 走 onUpdateTask 既有落库通道。
   *  绑定一律加法，不动卡上已有绑定 */
  const bindFilesToTask = async (t: Task) => {
    try {
      const picked = await invoke<string[]>("pick_files_dialog");
      if (!picked?.length) return;
      const added = await invoke<TaskFile[]>("bind_files", { paths: picked });
      // 对话框挂起期间别的路径（卡片 / 详情面板）可能改了绑定：合并基线取
      // 「对话框返回后的现值」，别拿点击时的快照把那批整列覆盖掉
      const cur = tasksRef.current.find((x) => x.id === t.id) ?? t;
      applyBinding(taskFiles(cur), added, (files) =>
        onUpdateTask(t.id, filesPatch(files)),
      );
    } catch (e) {
      handleCommandError(e, "bind files to task", { silent: true });
    }
  };

  return (
    <aside
      id="chat-context-rail"
      aria-label="上下文侧栏"
      // flex-1 与聊天主体的 flex-[2] 按 1:2 分剩余空间 → 侧栏 = 聊天区的 1/3；
      // min/max 只在极端窗口兜底（窄窗不让侧栏挤成缝，宽窗不让正文消失）
      className="flex min-w-[240px] max-w-[420px] flex-1 flex-col overflow-y-auto pl-3"
    >
      {/* 类别切换：切类别不打断对话（工作面在侧栏，主对话留在原位） */}
      <div className="mb-2 flex shrink-0 gap-1">
          {TABS.map((t) => {
            const on = tab === t.key;
            return (
              <button
                key={t.key}
                type="button"
                aria-pressed={on}
                className={`flex min-w-0 flex-1 items-center justify-center gap-1 rounded-lg px-2 py-1.5 text-[11px] transition-colors duration-[var(--dur-chip)] ease-[var(--ease-out)] ${
                  on
                    ? "nm-inset text-[var(--t1)]"
                    : "text-[var(--t3)] hover:bg-[var(--hover-bg)]"
                }`}
                onClick={() => onTabChange(t.key)}
              >
                {t.icon}
                <span className="truncate">{t.label}</span>
                {/* 在途计数常驻：切走类别也能看到后台还在跑几个 */}
                {t.key === "agents" && runningCount > 0 && (
                  <span
                    aria-hidden
                    className="h-1.5 w-1.5 shrink-0 wmsg-activity-pulse rounded-full bg-[var(--brand)]"
                  />
                )}
              </button>
            );
          })}
      </div>

      {tab === "tasks" && (
        <section aria-label="任务池">
          <div className="mb-1 flex items-center gap-1.5 px-1.5">
            <span className="text-[11px] font-medium text-[var(--t2)]">
              待办任务
            </span>
            <span className="font-mono text-[10px] text-[var(--t5)]">
              {tasks.length}
            </span>
          </div>
          {tasks.length === 0 ? (
            <EmptyHint>没有待办任务</EmptyHint>
          ) : (
            <div className="space-y-0.5">
              {tasks.map((t) => {
                const referenced = refIds.includes(t.id);
                return (
                <div key={t.id} className="group flex items-center gap-1">
                  <button
                    type="button"
                    aria-label={`引用任务：${t.title}`}
                    title={`引用「${t.title}」到对话`}
                    className={`min-w-0 flex-1 truncate rounded-md py-1.5 pl-1.5 pr-1 text-left text-xs hover:bg-[var(--hover-bg)] ${
                      referenced
                        ? "text-[var(--t1)]"
                        : "text-[var(--t3)] hover:text-[var(--t2)]"
                    }`}
                    aria-pressed={referenced}
                    onClick={() => onRefTask({ id: t.id, title: t.title })}
                  >
                    {t.title}
                  </button>
                  <button
                    type="button"
                    className="shrink-0 rounded p-1 text-[var(--t5)] hover:bg-[var(--hover-bg)] hover:text-[var(--success)]"
                    title="标记完成"
                    aria-label={`完成 ${t.title}`}
                    onClick={() => onUpdateTask(t.id, { column: "done" })}
                  >
                    <Check size={12} aria-hidden />
                  </button>
                  <button
                    type="button"
                    className="shrink-0 rounded p-1 text-[var(--t5)] hover:bg-[var(--hover-bg)] hover:text-[var(--brand)]"
                    title="绑定文件到此卡"
                    aria-label={`绑定文件到 ${t.title}`}
                    onClick={() => void bindFilesToTask(t)}
                  >
                    <Paperclip size={12} aria-hidden />
                  </button>
                </div>
                );
              })}
            </div>
          )}
        </section>
      )}

      {tab === "files" && (() => {
        // 预览态：整栏换成文件内容（与子 Agent 详情同级），返回才回列表。
        // 看内容还是看改动由「请求」给的（点击来源决定）：改动清单点进来
        // 看差异，交付物点进来看内容——交付物也可能是本轮写出来的（配套说明
        // 文档就是），按「在不在改动清单里」判会把交付物打开成 diff。
        if (view) {
          const hit = fileItems.find((f) => f.path === view.path);
          // 差异视图要先知道这个路径改没改：清单没回来时不猜（否则会闪一下
          // 全文再跳到改动视图，看着像两次加载）。内容视图不用等清单。
          if (view.mode === "diff" && sessionId && listLoadedFor !== sessionId) {
            return (
              <section aria-label="文件改动">
                <p className="inline-flex items-center gap-1 px-1.5 py-2 text-[11px] text-[var(--t5)]">
                  <Loader2 size={11} aria-hidden className="wmsg-activity" />
                  读取改动…
                </p>
              </section>
            );
          }
          if (view.mode === "diff" && hit) {
            return (
              <section aria-label="文件改动">
                <FileDiffView
                  path={view.path}
                  detail={fileDetail}
                  loading={fileDetailLoading}
                  error={fileDetailError}
                  onShowFull={() => switchFace("content")}
                  onBack={closePreview}
                />
              </section>
            );
          }
          return (
            <section aria-label="文件预览">
              <FilePreview
                // key 绑定 path：切文件即重挂载，预览状态天然清零
                key={view.path}
                path={view.path}
                added={hit?.added}
                deleted={hit?.deleted}
                // 改过的文件在全文视图里也给一条「看改动」回去的路
                onShowDiff={
                  hit ? () => switchFace("diff") : undefined
                }
                // 预览正文里再点一层路径：那是另一个文件的内容，看内容不看差异
                onOpenPath={(p) => openPreview(p, "content")}
                onBack={closePreview}
              />
            </section>
          );
        }
        return (
        <div>
          {/* 产物目录入口：交付物落在 AI_Gen_Files，路径由后端解析后回传。
              通栏细边框幽灵钮——它是这页的锚，不是一条普通链接 */}
          <button
            type="button"
            className="mb-1.5 flex w-full items-center gap-1.5 rounded-lg border border-[var(--edge)] px-2 py-1.5 text-[11px] text-[var(--t3)] transition-colors duration-[var(--dur-chip)] ease-[var(--ease-out)] hover:bg-[var(--hover-bg)] hover:text-[var(--t1)]"
            title={
              genDir
                ? `打开产物文件夹：${genDir}`
                : "打开产物文件夹（AI_Gen_Files）"
            }
            onClick={() => void openGenDir()}
          >
            <FolderOpen
              size={12}
              aria-hidden
              className="shrink-0 text-[var(--brand)]"
            />
            <span className="truncate">打开产物文件夹</span>
          </button>
          {genDirError && (
            <p className="mb-1.5 truncate px-1 text-[10px] text-[var(--danger)]">
              {genDirError}
            </p>
          )}
          {/* 子页切换：改动（轨迹）/ 交付物（产物）。两类东西不混在一张清单里 */}
          <div
            role="tablist"
            aria-label="文件分类"
            className="mb-1.5 flex gap-3 border-b border-[var(--border)]"
          >
            <FileSubTabButton
              label="改动"
              count={fileItems.length}
              active={fileSubTab === "changes"}
              onClick={() => onFileSubTabChange?.("changes")}
            />
            <FileSubTabButton
              label="交付物"
              count={sessionProbed.ready ? sessionProbed.items.length : null}
              active={fileSubTab === "deliverables"}
              onClick={() => onFileSubTabChange?.("deliverables")}
            />
          </div>
          {fileSubTab === "deliverables" ? (
        <section role="tabpanel" aria-label="交付物">
          {/* 名字与计数已在页签上，区头不再复读一遍 */}
          {sessionProbed.items.length === 0 ? (
            sessionProbed.ready ? (
              <FilePageEmpty
                main="本会话还没有交付物"
                sub="机器人产出的文件会汇总在这里，点开即预览"
              />
            ) : (
              <EmptyHint>读取交付物中…</EmptyHint>
            )
          ) : (
            <div className="space-y-0.5">
              {sessionProbed.items.map((d) => (
                <button
                  key={d.path}
                  type="button"
                  className="group flex w-full items-center gap-2 rounded-lg px-1.5 py-1.5 text-left hover:bg-[var(--hover-bg)]"
                  title={`${d.path}（${d.source === "declared" ? "回复中点名" : "本会话新写"}）· 点开预览`}
                  onClick={() => openPreview(d.path, "content")}
                >
                  <FileTypeBadge path={d.path} />
                  <span className="min-w-0 flex-1 truncate text-xs text-[var(--t3)] transition-colors duration-[var(--dur-chip)] ease-[var(--ease-out)] group-hover:text-[var(--t1)]">
                    {basename(d.path)}
                  </span>
                </button>
              ))}
            </div>
          )}
        </section>
          ) : (
        <section role="tabpanel" aria-label="文件变更">
          {fileListError ? (
            <p className="px-1.5 py-2 text-[11px] text-[var(--danger)]">
              {fileListError}
            </p>
          ) : fileItems.length === 0 ? (
            fileListLoading ? (
              <EmptyHint>读取改动中…</EmptyHint>
            ) : (
              <FilePageEmpty
                main="本会话还没有文件改动"
                sub="机器人改动过的文件会汇总在这里"
              />
            )
          ) : (
            <div className="space-y-0.5">
              {fileItems.map((f) => (
                <button
                  key={f.path}
                  type="button"
                  className="group flex w-full items-center gap-2 rounded-lg px-1.5 py-1.5 text-left hover:bg-[var(--hover-bg)]"
                  title={`${f.path}（${f.mode === "noLineDiff" ? "无行级 diff" : `+${f.added} −${f.deleted}`}）· 点开看改动${f.note ? `｜${f.note}` : ""}`}
                  onClick={() => openPreview(f.path, "diff")}
                >
                  <FileTypeBadge path={f.path} />
                  <span className="min-w-0 flex-1 truncate text-xs text-[var(--t3)] transition-colors duration-[var(--dur-chip)] ease-[var(--ease-out)] group-hover:text-[var(--t1)]">
                    {basename(f.path)}
                  </span>
                  {f.mode === "noLineDiff" ? (
                    <span className="shrink-0 text-[10px] text-[var(--t5)]">
                      无行级 diff
                    </span>
                  ) : (
                    <>
                      {/* 逐次档的行数是各次之和（不是净变化）——标出来，别让人误读 */}
                      {f.mode === "perChange" && (
                        <span className="shrink-0 text-[10px] text-[var(--t5)]">
                          逐次
                        </span>
                      )}
                      <span className="shrink-0 font-mono text-[10px] text-[var(--success)]">
                        +{f.added}
                      </span>
                      <span className="shrink-0 font-mono text-[10px] text-[var(--danger)]">
                        −{f.deleted}
                      </span>
                    </>
                  )}
                </button>
              ))}
            </div>
          )}
        </section>
          )}
        </div>
        );
      })()}

      {tab === "agents" && (() => {
        const detail = subagents.find((s) => s.id === subagentDetailId);
        if (detail && onCloseSubagentDetail) {
          return (
            <section aria-label="子 Agent 详情">
              <SubagentDetailPanel run={detail} onBack={onCloseSubagentDetail} />
            </section>
          );
        }
        return (
        <section aria-label="子 Agent">
          <div className="mb-1 flex items-center gap-1.5 px-1.5">
            <span className="text-[11px] font-medium text-[var(--t2)]">
              子 Agent
            </span>
            {runningCount > 0 && (
              <span className="text-[10px] text-[var(--brand)]">
                {runningCount} 个执行中
              </span>
            )}
          </div>
          {subagents.length === 0 ? (
            <EmptyHint>当前没有子 agent 执行</EmptyHint>
          ) : (
            <div className="space-y-0.5">
              {subagents.map((s) => (
                <div key={s.id} className="rounded-md hover:bg-[var(--hover-bg)]">
                  {/* 列表行只留「名称 + 用时 + 停止键」：进度读数与逐步动作
                      都在详情页，列表塞满数字就成了仪表盘。状态由图标带——
                      在途转圈，终态看颜色。 */}
                  <div className="flex w-full items-center gap-1.5 rounded-md px-1.5 py-1.5 text-xs text-[var(--t3)]">
                    <button
                      type="button"
                      title={s.title}
                      className="flex min-w-0 flex-1 items-center gap-1.5 text-left hover:text-[var(--t2)]"
                      onClick={() => onOpenSubagent(s.id)}
                    >
                      {s.running ? (
                        <Loader2
                          size={12}
                          aria-hidden
                          className="shrink-0 wmsg-activity text-[var(--brand)]"
                        />
                      ) : (
                        <Bot
                          size={12}
                          aria-hidden
                          className={`shrink-0 ${SUBAGENT_STATUS[s.status].color}`}
                        />
                      )}
                      <span className="min-w-0 flex-1 truncate">{s.title}</span>
                      <SubagentElapsed run={s} />
                    </button>
                    <SubagentCancelButton
                      run={s}
                      onCancel={onCancelSubagent}
                    />
                  </div>
                </div>
              ))}
            </div>
          )}
        </section>
        );
      })()}
    </aside>
  );
}

