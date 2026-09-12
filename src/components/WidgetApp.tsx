import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import {
  DndContext,
  DragEndEvent,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import type { DraggableSyntheticListeners } from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import {
  LogicalPosition,
  LogicalSize,
  currentMonitor,
  getCurrentWindow,
} from "@tauri-apps/api/window";
import { listen, emit } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { openTarget } from "../lib/openTarget";
import { linkDisplayName } from "./WorkspacePage";
import { focusMainWindow } from "../focus";
import { handleCommandError } from "../lib/errorHandler";
import { isDueToday } from "../format";
import { loadTasksFromDb, loadWorkspaceFromDb, sortByOrder, assignInsertOrder, diffTaskRows } from "../storage";
import { applySetting, getSetting, subscribeSystem, subscribeTheme } from "../theme";
import type { ThemeSetting } from "../theme";
import type { Task, WorkspaceItem } from "../types";
import { taskFiles, filesPatch } from "../lib/taskFiles";
import { TaskCardContent } from "./TaskCardContent";
import { FoldToggle } from "./FoldToggle";
import { ChatPanel } from "./ChatPanel";
import { ArtifactBatchDialog } from "./ArtifactBatchDialog";
import widgetLogo from "../assets/widget-logo.png";

// 收起为触发条 / 展开为侧边面板
const STRIP_W = 44;
const STRIP_H = 220;
const PANEL_W = 480;
const PANEL_H = 560;
const TOP_Y = 140; // 默认贴右缘的初始 Y

// 老板拍板：边框调整边界 + Splitter 上下限
const PANEL_W_MIN = 400, PANEL_W_MAX = 800;
const PANEL_H_MIN = 800, PANEL_H_MAX = 900;
const TASK_H_MIN = 0, CHAT_H_MIN = 120;
const SPLITTER_H = 16;
const DEFAULT_TASK_H = 280;

const SIZE_KEY = "***";
interface WidgetSize { w: number; h: number }
function loadSize(): WidgetSize | null {
  try {
    const raw = localStorage.getItem(SIZE_KEY);
    if (raw) {
      const s = JSON.parse(raw) as WidgetSize;
      if (s.w >= PANEL_W_MIN && s.w <= PANEL_W_MAX &&
          s.h >= PANEL_H_MIN && s.h <= PANEL_H_MAX) return s;
    }
  } catch { /* ignore */ }
  return null;
}
function saveSize(s: WidgetSize) {
  try { localStorage.setItem(SIZE_KEY, JSON.stringify(s)); } catch { /* ignore */ }
}

// 从 CSS 变量读/写任务区高度(Splitter 拖动用,不入 React state)
// 注意:拖到顶部时值为 "0px",0 是合法状态,不能当无效值回退成默认高度
function getTaskH(): number {
  const raw = document.documentElement.style.getPropertyValue("--task-h");
  if (!raw) return DEFAULT_TASK_H; // 从未设置过(首次)
  const v = parseInt(raw, 10);
  return Number.isFinite(v) ? Math.max(0, v) : DEFAULT_TASK_H;
}
function setTaskH(h: number) {
  document.documentElement.style.setProperty("--task-h", h + "px");
}

// 拖拽进行中标记:拖动调整期间禁止 mouseleave 触发折叠(防止拖到一半「缩回去」)
let widgetDragActive = false;

// 挂件位置持久化（与任务数据分开的 key）
const POS_KEY = "wmessage-widget-pos";

type Edge = "right" | "left" | "top" | "float";

interface Anchor {
  x: number;
  y: number;
  edge: Edge;
}

async function screenSize(): Promise<{ w: number; h: number }> {
  try {
    const m = await currentMonitor();
    if (m)
      return { w: m.size.width / m.scaleFactor, h: m.size.height / m.scaleFactor };
  } catch {
    /* ignore */
  }
  return { w: 1920, h: 1080 };
}

function loadAnchor(): Anchor | null {
  try {
    const raw = localStorage.getItem(POS_KEY);
    if (raw) return JSON.parse(raw) as Anchor;
  } catch {
    /* ignore */
  }
  return null;
}

function saveAnchor(a: Anchor) {
  try {
    localStorage.setItem(POS_KEY, JSON.stringify(a));
  } catch {
    /* ignore */
  }
}

// 按窗口当前矩形推算：贴边检测（右/左/顶，24px 容差）+ 触发条锚点
function anchorFromRect(
  x: number,
  y: number,
  w: number,
  sw: number
): { anchor: { x: number; y: number }; edge: Edge } {
  const MARGIN = 24;
  if (x + w >= sw - MARGIN) return { anchor: { x: sw - STRIP_W, y }, edge: "right" };
  if (x <= MARGIN) return { anchor: { x: 0, y }, edge: "left" };
  if (y <= MARGIN) return { anchor: { x, y: 0 }, edge: "top" };
  return { anchor: { x, y }, edge: "float" };
}

// 四边拖拽热区：老板拍板 A+C（平时 1px 暗示线 + 拖动高亮）
function ResizeEdge({
  side,
  onDelta,
}: {
  side: "n" | "s" | "e" | "w";
  onDelta: (dx: number, dy: number, startPos: { x: number; y: number; w: number; h: number; sw: number; sh: number }) => void;
}) {
  const isH = side === "n" || side === "s";
  const pos =
    side === "n" ? "top-0 left-0 right-0 h-2 cursor-n-resize" :
    side === "s" ? "bottom-0 left-0 right-0 h-2 cursor-s-resize" :
    side === "e" ? "top-0 right-0 bottom-0 w-2 cursor-e-resize" :
                   "top-0 left-0 bottom-0 w-2 cursor-w-resize";
  const linePos = isH
    ? "left-0 right-0 top-1/2 -translate-y-1/2 h-px"
    : "top-0 bottom-0 left-1/2 -translate-x-1/2 w-px";

  const onDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const sx = e.clientX, sy = e.clientY;
    const el = e.currentTarget as HTMLElement;
    try { el.setPointerCapture(e.pointerId); } catch { /* ignore */ }
    // 起始外框位置/尺寸/屏幕尺寸(异步读取;未就绪前的 move 一律忽略)
    let startPos: { x: number; y: number; w: number; h: number; sw: number; sh: number } | null = null;
    const win = getCurrentWindow();
    void Promise.all([win.outerPosition(), win.outerSize(), win.scaleFactor(), screenSize()]).then(
      ([p, s, sc, { w: sw, h: sh }]) => {
        startPos = { x: p.x / sc, y: p.y / sc, w: s.width / sc, h: s.height / sc, sw, sh };
      }
    );
    widgetDragActive = true;
    // 监听器同步注册(不等异步读取):避免快速点按时漏掉 pointerup 造成监听残留
    const move = (ev: PointerEvent) => {
      const sp = startPos;
      if (!sp) return;
      onDelta(ev.clientX - sx, ev.clientY - sy, sp);
    };
    const cleanup = (ev: PointerEvent) => {
      try { el.releasePointerCapture(ev.pointerId); } catch { /* ignore */ }
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", cleanup);
      window.removeEventListener("pointercancel", cleanup);
      widgetDragActive = false;
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", cleanup);
    window.addEventListener("pointercancel", cleanup);
  };

  return (
    <div className={`absolute z-30 ${pos} bg-transparent`} onPointerDown={onDown}>
      <div className={`absolute ${linePos} bg-[var(--edge)] opacity-30 hover:opacity-80 active:opacity-100 transition-opacity`} />
    </div>
  );
}

// Splitter：任务区/聊天区分割条，双箭头 + 可拖拽
// ▲ = 聊天区变大(任务区变小)、▼ = 任务区变大(聊天区变小)
function SplitBar({ onSplit, onArrow }: {
  onSplit: (delta: number) => void;
  onArrow: (delta: number) => void;
}) {
  const onDown = (e: React.PointerEvent) => {
    if ((e.target as HTMLElement).closest("button")) return;
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const sy = e.clientY;
    let acc = 0;
    const el = e.currentTarget as HTMLElement;
    try { el.setPointerCapture(e.pointerId); } catch { /* ignore */ }
    widgetDragActive = true;
    const move = (ev: PointerEvent) => {
      const dy = ev.clientY - sy;
      const inc = dy - acc;
      acc = dy;
      onSplit(inc);
    };
    const cleanup = (ev: PointerEvent) => {
      try { el.releasePointerCapture(ev.pointerId); } catch { /* ignore */ }
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", cleanup);
      window.removeEventListener("pointercancel", cleanup);
      widgetDragActive = false;
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", cleanup);
    window.addEventListener("pointercancel", cleanup);
  };

  return (
    <div
      className="relative h-4 shrink-0 flex items-center justify-center cursor-row-resize hover:bg-[var(--hover-bg)] group select-none"
      onPointerDown={onDown}
    >
      <div className="absolute inset-x-0 top-1/2 -translate-y-1/2 h-px bg-[var(--edge)] opacity-40 group-hover:opacity-80" />
      <div className="relative flex items-center gap-3 px-2 py-0.5 rounded-full bg-[var(--bg)] border border-[var(--edge)]">
        <button
          onClick={() => onArrow(-40)}
          title="聊天区变大"
          className="text-[10px] text-[var(--t5)] hover:text-[var(--t2)] leading-none px-0.5"
        >▲</button>
        <button
          onClick={() => onArrow(+40)}
          title="任务区变大"
          className="text-[10px] text-[var(--t5)] hover:text-[var(--t2)] leading-none px-0.5"
        >▼</button>
      </div>
    </div>
  );
}

export default function WidgetApp() {
  const [tasks, setTasks] = useState<Task[]>([]);
  const tasksRef = useRef<Task[]>([]);
  const [theme, setTheme] = useState<ThemeSetting>(getSetting);
  const [expanded, setExpanded] = useState(false);
  const [locked, setLocked] = useState(false);
  const [view, setView] = useState<"all" | "today" | "done" | "workspace">("all");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [edge, setEdge] = useState<Edge>("right");
  const [botOn, setBotOn] = useState(false);

  // 老板拍板：挂件边框尺寸(唯一尺寸源,只控制外框；内部任务/聊天区 flex 自适应)
  const [size, setSize] = useState<WidgetSize>(() => loadSize() ?? {
    w: PANEL_W, h: PANEL_H
  });
  // bot 关时面板 = 任务区 + splitter(不包含聊天区)；bot 开时 = 边框高度
  const panelH = (bot: boolean) => bot ? size.h : getTaskH() + SPLITTER_H;
  const [selecting, setSelecting] = useState(false);
  const [selectedTasks, setSelectedTasks] = useState<Task[]>([]);

  // 机器人开关状态：挂载时读 + 监听设置页切换广播
  useEffect(() => {
    invoke<boolean>("bot_get_enabled")
      .then(setBotOn)
      .catch((e) =>
        // 自动读取机器人状态：失败只是默认 false，不打扰
        handleCommandError(e, "bot_get_enabled", { silent: true })
      );
    const unlisten = listen<boolean>("bot-changed", (e) => setBotOn(!!e.payload));
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // bot 开/关 → 同步外框高度(bot 关时面板只留任务区+splitter)
  useEffect(() => {
    if (!expanded) return;
    getCurrentWindow()
      .setSize(new LogicalSize(size.w, panelH(botOn)))
      .catch(() => {});
  }, [botOn, expanded]);

  // 字体大小同步（老板拍板）：挂件 webview 独立 document
  // 需要拉 config 设 data-font-size；设置页保存后广播 bot-config-changed，
  // Tauri emit 全局广播挂件也能收到。失败也套默认 small，避免渲染前空抳。
  useEffect(() => {
    let unlistenCfg: (() => void) | undefined;
    (async () => {
      const applyFromConfig = async () => {
        try {
          const c = await invoke<{ uiFontSize?: string | null }>("bot_get_config");
          const v =
            c.uiFontSize === "standard" ||
            c.uiFontSize === "large" ||
            c.uiFontSize === "xlarge"
              ? c.uiFontSize
              : "small";
          document.documentElement.dataset.fontSize = v;
        } catch {
          document.documentElement.dataset.fontSize = "small";
        }
      };
      await applyFromConfig();
      const u = await listen("bot-config-changed", applyFromConfig);
      unlistenCfg = u;
    })();
    return () => unlistenCfg?.();
  }, []);

  // 机器人开关变化时：展开状态下同步调整窗口高度
  useEffect(() => {
    if (!expanded) return;
    getCurrentWindow()
      .setSize(new LogicalSize(size.w, panelH(botOn)))
      .catch(() => {});
  }, [botOn, expanded]);

  // 主题：启动时应用 + 监听主窗口切换 + 跟随系统模式监听系统外观变化
  useEffect(() => {
    applySetting(theme);
  }, [theme]);
  useEffect(() => subscribeTheme(setTheme), []);
  useEffect(() => subscribeSystem(setTheme), []);
  const anchorRef = useRef<Anchor>({ x: 0, y: TOP_Y, edge: "right" });
  const listRef = useRef<HTMLDivElement>(null);
  const chatAreaRef = useRef<HTMLDivElement>(null);
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } })
  );

  // 任务数据同步：主窗口统一落盘 SQLite，挂件只读。三层：初始读取 + tasks-changed + 5s 兜底轮询
  useEffect(() => {
    // 5s 兜底轮询读失败不再永久静默——轮询连续失败说明
    // 主窗口/DB 异常，弹 alert 让用户知情；首次加载与 tasks-changed 触发保持安静
    // （任务为空/瞬态失败等下次重试即可，不打扰）
    const load = async (fromPoll = false) => {
      try {
        const res = await loadTasksFromDb();
        if (!res.ok) {
          if (fromPoll) handleCommandError(res.error, "widget_poll");
          return; // 读失败：保持现状，等下次 tasks-changed / 轮询重试
        }
        const list = sortByOrder(res.tasks);
        // 空列表只在「当前本就为空」时跳过（首载防空闪）——
        // 原先无条件跳过：主窗口删光任务后挂件永久显示旧数据，点勾选还会把已删任务复活回库
        if (!list.length && tasksRef.current.length === 0) return;
        setTasks((prev) => {
          const same = JSON.stringify(list) === JSON.stringify(prev);
          if (!same) tasksRef.current = list;
          return same ? prev : list;
        });
      } catch (e) {
        if (fromPoll) handleCommandError(e, "widget_poll");
      }
    };
    load();
    const unlisten = listen("tasks-changed", () => {
      load().catch(() => {});
    });
    const id = setInterval(() => {
      load(true).catch((e) => handleCommandError(e, "widget_poll"));
    }, 5000);
    return () => {
      unlisten.then((f) => f());
      clearInterval(id);
    };
  }, []);

  // 工作区同步：只读展示（编辑在主窗口）；初始读取 + workspace-changed + 5s 兜底轮询
  const [workspace, setWorkspace] = useState<WorkspaceItem[]>([]);
  useEffect(() => {
    const load = async () => {
      try {
        const list = await loadWorkspaceFromDb();
        setWorkspace((prev) => {
          const same = JSON.stringify(list) === JSON.stringify(prev);
          return same ? prev : list;
        });
      } catch {
        /* ignore */
      }
    };
    load();
    const unlisten = listen("workspace-changed", () => {
      load().catch(() => {});
    });
    const id = setInterval(() => {
      load().catch(() => {});
    }, 5000);
    return () => {
      unlisten.then((f) => f());
      clearInterval(id);
    };
  }, []);

  // 统一走 openTarget——按内容判定 URL/路径（不信任存储的 kind，
  // 历史数据可能把「C:\...」误存成 url 被 openUrl scope 拒），失败弹错不静默
  const openLink = (link: WorkspaceItem["links"][number]) => {
    openTarget(link.targetUri);
  };

  /** 挂件工作区折叠切换：与任务卡一致走「上报主窗口落盘」（单写者架构），
   *  不再直接写库（此前挂件直接 workspace_upsert 与主窗口并发写，违反挂件只读约定） */
  const toggleWsCollapsed = (it: WorkspaceItem) => {
    const next = workspace.map((w) =>
      w.id === it.id ? { ...w, collapsed: !w.collapsed, updatedAt: Date.now() } : w
    );
    setWorkspace(next);
    const updated = next.find((w) => w.id === it.id);
    if (updated) {
      emit("workspace-updated", { upserts: [updated] }).catch(() => {});
    }
  };

  /** 挂件工作区上下排序：arrayMove 换位 → assignInsertOrder 分配 order → 上报主窗口落盘 */
  const handleWsDragEnd = (e: DragEndEvent) => {
    const { active, over } = e;
    if (!over || active.id === over.id) return;
    const ids = workspace.map((w) => w.id);
    const from = ids.indexOf(String(active.id));
    const to = ids.indexOf(String(over.id));
    if (from < 0 || to < 0) return;
    const byId = new Map(workspace.map((w) => [w.id, w]));
    const next = assignInsertOrder(
      arrayMove(ids, from, to).map((id) => byId.get(id)!),
      String(active.id)
    );
    setWorkspace(next);
    const prevMap = new Map(workspace.map((w) => [w.id, w]));
    const changed = next.filter(
      (w) => JSON.stringify(w) !== JSON.stringify(prevMap.get(w.id))
    );
    const now = Date.now();
    changed.forEach((w) => (w.updatedAt = now));
    if (changed.length) {
      emit("workspace-updated", { upserts: changed }).catch(() => {});
    }
  };

  // 初始：透明背景 + 置顶 + 恢复到上次位置（默认贴右缘）
  useEffect(() => {
    // 透明窗口：html/body 都必须透明，否则圆角外的缺口被底色填满，看起来像直角
    document.documentElement.style.background = "transparent";
    document.body.style.background = "transparent";
    const win = getCurrentWindow();
    win.setAlwaysOnTop(true).catch(() => {});
    (async () => {
      const { w: sw } = await screenSize();
      const saved = loadAnchor();
      const anchor: Anchor = saved ?? { x: sw - STRIP_W, y: TOP_Y, edge: "right" };
      anchorRef.current = anchor;
      setEdge(anchor.edge);
      // 贴顶时触发条为横条（220×44），其余边缘为竖条（44×220）
      const top = anchor.edge === "top";
      await win.setSize(
        new LogicalSize(top ? STRIP_H : STRIP_W, top ? STRIP_W : STRIP_H)
      );
      await win.setPosition(new LogicalPosition(anchor.x, anchor.y));
    })();
  }, []);

  // 拖动结束（停止移动 500ms）后：贴边吸附 + 记录锚点（圆角随贴边/悬浮切换，见 edgeClass）
  useEffect(() => {
    const win = getCurrentWindow();
    let timer: number | undefined;
    const settle = async () => {
      const pos = await win.outerPosition();
      const size = await win.outerSize();
      const scale = await win.scaleFactor();
      const { w: sw } = await screenSize();
      const x = pos.x / scale;
      const y = pos.y / scale;
      const w = size.width / scale;
      const { anchor, edge: e2 } = anchorFromRect(x, y, w, sw);
      // 贴边吸附
      let nx = x;
      let ny = y;
      if (e2 === "right") nx = sw - w;
      else if (e2 === "left") nx = 0;
      else if (e2 === "top") ny = 0;
      if (nx !== x || ny !== y) await win.setPosition(new LogicalPosition(nx, ny));
      anchorRef.current = { ...anchor, edge: e2 };
      setEdge(e2);
      saveAnchor({ ...anchor, edge: e2 });
    };
    const unlisten = win.onMoved(() => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        settle().catch(() => {});
      }, 500);
    });
    return () => {
      unlisten.then((f) => f());
      window.clearTimeout(timer);
    };
  }, []);

  const expand = async () => {
    const win = getCurrentWindow();
    const { x, y, edge: e } = anchorRef.current;
    // 贴右缘：向左展开；其余情况从锚点向右/向下展开
    const px = e === "right" ? x - (size.w - STRIP_W) : x;
    if (e === "right") await win.setPosition(new LogicalPosition(px, y));
    await win.setSize(new LogicalSize(size.w, panelH(botOn)));
    await win.setPosition(new LogicalPosition(px, y));
    setExpanded(true);
  };

  const collapse = async () => {
    if (locked || widgetDragActive) return; // 拖动调整中不折叠
    const win = getCurrentWindow();
    // 拖动后 settle 可能还没跑：按当前窗口位置实时推算锚点，避免贴回旧位置
    const pos = await win.outerPosition();
    const size = await win.outerSize();
    const scale = await win.scaleFactor();
    const { w: sw } = await screenSize();
    const { anchor, edge: e2 } = anchorFromRect(
      pos.x / scale,
      pos.y / scale,
      size.width / scale,
      sw
    );
    anchorRef.current = { ...anchor, edge: e2 };
    setEdge(e2);
    saveAnchor({ ...anchor, edge: e2 });
    await win.setPosition(new LogicalPosition(anchor.x, anchor.y));
    // 贴顶横条 / 其余竖条
    await win.setSize(
      new LogicalSize(
        e2 === "top" ? STRIP_H : STRIP_W,
        e2 === "top" ? STRIP_W : STRIP_H
      )
    );
    setExpanded(false);
  };

  // 产物绑定弹窗（ArtifactBatchDialog）挂在挂件窗口：面板始终挂载（收起只是
  // display:none），事件监听不丢；收起成触发条时收到 artifact-batch-ready
  // 自动展开让弹窗可见，锁定/展开时直接弹出等待确认
  const expandedRef = useRef(expanded);
  expandedRef.current = expanded;
  useEffect(() => {
    const un = listen("artifact-batch-ready", () => {
      if (!expandedRef.current) expand().catch(() => {});
    });
    return () => {
      un.then((f) => f());
    };
  }, [botOn]);

  // 面板头部为拖动手柄：按住拖动挂件到任意位置
  const startDrag = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    getCurrentWindow().startDragging().catch(() => {});
  };

  // 四边拖拽边框：尺寸/位置全部按「拖动起点 + 累计位移」绝对计算(不做增量叠加,避免暴涨)
  const resizePanel = (
    dx: number,
    dy: number,
    side: "n" | "s" | "e" | "w",
    startPos: { x: number; y: number; w: number; h: number; sw: number; sh: number },
  ) => {
    // 尺寸 = 起点尺寸 ± 位移
    let w = startPos.w;
    let h = startPos.h;
    if (side === "e") w = startPos.w + dx;
    else if (side === "w") w = startPos.w - dx;
    else if (side === "s") h = startPos.h + dy;
    else if (side === "n") h = startPos.h - dy;
    // 「拖到屏幕边就停」:不超出屏幕可用范围
    if (side === "e") w = Math.min(w, startPos.sw - startPos.x);
    else if (side === "w") w = Math.min(w, startPos.x + startPos.w);
    if (side === "s") h = Math.min(h, startPos.sh - startPos.y);
    else if (side === "n") h = Math.min(h, startPos.y + startPos.h);
    w = Math.round(Math.min(PANEL_W_MAX, Math.max(PANEL_W_MIN, w)));
    h = Math.round(Math.min(PANEL_H_MAX, Math.max(PANEL_H_MIN, h)));
    const next: WidgetSize = { w, h };
    saveSize(next);
    setSize(next);

    // 位置:固定"没被拖的那条边"(拖左→右固定;拖上→底固定;拖右/下→左/顶固定)
    let nx = startPos.x;
    let ny = startPos.y;
    if (side === "w") nx = startPos.x + startPos.w - w;
    else if (side === "n") ny = startPos.y + startPos.h - h;

    // 窗口尺寸:bot 开=边框高度,直接用本次算出的 h(不读 state,避免旧值导致抖动)
    const winH = botOn ? h : getTaskH() + SPLITTER_H;
    void (async () => {
      try {
        await getCurrentWindow().setSize(new LogicalSize(w, winH));
        await getCurrentWindow().setPosition(new LogicalPosition(nx, ny));
      } catch { /* ignore */ }
    })();
  };

  // Splitter 拖动：只改 CSS 变量 --task-h，聊天区 flex-1 自动反向伸缩(外框不变)
  // 向下拖的停止线按实时布局测量：聊天区至少保留 CHAT_H_MIN(不再写死头部/按钮/内边距估算)
  const moveSplit = (deltaY: number) => {
    if (!botOn) return; // bot 关时面板只有任务区+Splitter,没聊天区可挤
    const cur = getTaskH();
    const chatEl = chatAreaRef.current;
    // 还能向任务区让出的高度 = 聊天区当前高度 - 聊天区最小高度
    const maxTask = chatEl
      ? cur + Math.max(0, chatEl.offsetHeight - CHAT_H_MIN)
      : Math.max(TASK_H_MIN, size.h - 136 - CHAT_H_MIN); // 兜底估算:内边距32+头部44+按钮44+分割条16
    const next =
      deltaY >= 0
        ? Math.min(Math.max(cur, maxTask), cur + deltaY) // 已到极限时下拉不动(不回缩)
        : Math.max(TASK_H_MIN, cur + deltaY);
    setTaskH(Math.round(next));
  };
  const arrowSplit = (deltaY: number) => moveSplit(deltaY);

  // 挂件端改动的统一出口：更新本地 state + 行级 diff 上报主窗口（主窗口负责落盘 SQLite）
  const applyAndSync = (fn: (prev: Task[]) => Task[]) => {
    const prev = tasksRef.current;
    const next = fn(prev);
    tasksRef.current = next;
    // 打最后修改时间戳（合并导入按此比较同 id 取舍）；
    // 纯排序变更保留原 updatedAt（diffTaskRows 内部判定）
    const { upserts, deletes } = diffTaskRows(prev, next, Date.now());
    setTasks(next);
    if (upserts.length || deletes.length) {
      // emit 失败不再静默吞——挂件不落盘，上报失败意味着
      // 本地改动永不持久化（≤5s 后轮询回滚，用户视角「编辑神秘消失」），必须让用户知情
      emit("tasks-updated", { upserts, deletes }).catch((e) =>
        handleCommandError(e, "tasks-updated")
      );
    }
  };

  // 点圆圈完成/取消完成：与主窗口行为一致（完成 → 记时间；取消 → 截止日期是今天回今日、否则回待办，完成时间删除）
  const toggleDone = (t: Task) =>
    applyAndSync((prev) =>
      prev.map((x): Task => {
        if (x.id !== t.id) return x;
        return x.column === "done"
          ? {
              ...x,
              column: isDueToday(x.due) ? "doing" : "todo",
              completedAt: undefined,
              archived: undefined,
            }
          : // 人完成：清机器人标记 → 显示用户头像
            { ...x, column: "done", completedAt: Date.now(), archived: false, botAssigned: undefined };
      })
    );

  // 标题以下折叠/展开
  const toggleCollapsed = (t: Task) =>
    applyAndSync((prev) =>
      prev.map((x) => (x.id === t.id ? { ...x, collapsed: !x.collapsed } : x))
    );

  // 新建任务：今日视图直接进 doing（今日列），否则进 todo；创建后立即进入标题编辑态。
  // 新任务插到列表顶部（从顶部出来）
  const addTask = () => {
    const id = crypto.randomUUID();
    applyAndSync((prev) => {
      const min = prev.reduce((m, t) => Math.min(m, t.order ?? 0), 0);
      return [
        {
          id,
          title: "新任务",
          column: view === "today" ? "doing" : "todo",
          order: min - 1,
        },
        ...prev,
      ];
    });
    setEditingId(id);
  };

  // 标题编辑提交（Enter/blur）；清空则回退原标题
  const commitTitle = (t: Task, title: string) => {
    const next = title.trim() || t.title;
    applyAndSync((prev) =>
      prev.map((x) => (x.id === t.id ? { ...x, title: next } : x))
    );
    setEditingId(null);
  };

  // 定时执行规则更新（⏰ 面板；同步主窗口）
  const setSchedule = (t: Task, schedule: string | undefined) =>
    applyAndSync((prev) =>
      prev.map((x) => (x.id === t.id ? { ...x, schedule } : x))
    );

  const cancelTitle = () => setEditingId(null);

  // 双击标题 → 通知主窗口打开该任务编辑态 + 唤起主窗口并强制置顶
  // （老板规则：双击唤起后主窗口必须出现在桌面屏幕最顶层）
  const openInMain = (t: Task) => {
    emit("edit-task", { id: t.id }).catch(() => {});
    focusMainWindow();
  };

  // 选任务模式：点标题切换选中（不打开主窗口）
  const toggleSelectTask = (t: Task) => {
    setSelectedTasks((prev) =>
      prev.some((x) => x.id === t.id)
        ? prev.filter((x) => x.id !== t.id)
        : [...prev, t]
    );
  };

  // 发送完成：清空选择 + 退出选任务模式
  const finishSelection = () => {
    setSelectedTasks([]);
    setSelecting(false);
  };

  // 进入编辑态时滚回顶部，保证新建任务输入框可见（新任务在列表顶部）
  useEffect(() => {
    if (editingId) {
      listRef.current?.scrollTo({ top: 0, behavior: "smooth" });
    }
  }, [editingId]);

  // 子任务勾选/取消（与主窗口一致）
  const toggleSubtask = (t: Task, subtaskId: string) =>
    applyAndSync((prev) =>
      prev.map((x) =>
        x.id !== t.id
          ? x
          : {
              ...x,
              subtasks: (x.subtasks ?? []).map((s) =>
                s.id === subtaskId ? { ...s, done: !s.done } : s
              ),
            }
      )
    );

  // 打开绑定文件/文件夹（点文件名直开，不再有 📂 按钮；
  // 失败弹错不静默——点了没反应无从排查）
  const openFilePath = (path: string) => {
    openTarget(path);
  };

  // 复制单个绑定文件+标题（chip 内「复制」字样；与原 📋 单文件行为一致）
  const copyFilePath = (t: Task, path: string) => {
    invoke("copy_file_with_title", { path, title: t.title }).catch((e) =>
      handleCommandError(e, "copy_file_with_title", { silent: true })
    );
  };

  // 移除单个绑定文件（chip ×；同步主窗口落盘）
  const removeFile = (t: Task, path: string) =>
    applyAndSync((prev) =>
      prev.map((x) =>
        x.id !== t.id
          ? x
          : {
              ...x,
              ...filesPatch(taskFiles(x).filter((f) => f.path !== path)),
            }
      )
    );

  // 挂件可见列表排序结束：重建全局顺序，只给被拖任务分配 order（行级同步主窗口）
  const handleDragEnd = (e: DragEndEvent) => {
    const { active, over } = e;
    if (!over || active.id === over.id) return;
    const ids = list.map((t) => t.id);
    const from = ids.indexOf(String(active.id));
    const to = ids.indexOf(String(over.id));
    if (from < 0 || to < 0) return;
    const newIds = arrayMove(ids, from, to);
    applyAndSync((prev) => {
      const visibleSet = new Set(ids);
      const byId = new Map(prev.map((t) => [t.id, t]));
      const arr: Task[] = [];
      let cursor = 0;
      for (const t of prev) {
        if (visibleSet.has(t.id)) arr.push(byId.get(newIds[cursor++])!);
        else arr.push(t);
      }
      return assignInsertOrder(arr, String(active.id));
    });
  };

  // 挂件「待办 / 今日」显示未完成（todo + doing）；「完成」显示已完成列
  const visible = tasks.filter(
    (t) => !t.archived && !t.deletedAt && t.column !== "done"
  );
  const doneList = tasks
    .filter((t) => !t.archived && !t.deletedAt && t.column === "done")
    .slice()
    .sort((a, b) => (b.completedAt ?? 0) - (a.completedAt ?? 0));
  const list =
    view === "today"
      ? visible.filter((t) => t.column === "doing")
      : view === "done"
      ? doneList
      : visible;

  // 圆角：贴屏幕那侧直角，对侧 rounded-2xl；悬浮（不贴边）四边全圆角
  // （老板定的规则：贴屏侧直角 + 对侧圆角）
  const edgeClass =
    edge === "right"
      ? "rounded-l-2xl"
      : edge === "left"
      ? "rounded-r-2xl"
      : edge === "top"
      ? "rounded-b-2xl"
      : "rounded-2xl";

  return (
    <div className="w-screen h-screen bg-transparent overflow-hidden">
      {!expanded && (
        /* 触发条：贴边隐藏状态（顶缘为横条，其余为竖条） */
        <div
          className={`${
            edge === "top" ? "nm-sidebar-panel-top" : "nm-sidebar-panel"
          } ${edgeClass} !p-2 w-full h-full flex ${
            edge === "top" ? "flex-row" : "flex-col"
          } items-center justify-center gap-2 cursor-pointer select-none`}
          onMouseEnter={expand}
        >
          <img
            src={widgetLogo}
            className="widget-logo w-6 h-6"
            alt="WMessage"
            draggable={false}
          />
          <span
            className="text-[var(--t4)] text-xs tracking-widest"
            style={edge === "top" ? undefined : { writingMode: "vertical-rl" }}
          >
            WMessage
          </span>
        </div>
      )}
      {/* 展开面板：始终挂载，折叠时 display:none 隐藏而不卸载——保住 ChatPanel 的 messages /
          busy / streamingMeta / bot-chat-delta 事件监听，折叠-展开循环不丢流式消息 */}
      {
        <div
          className={`nm-sidebar-panel ${edgeClass} flex flex-col relative overflow-hidden`}
          style={{
            width: size.w,
            height: panelH(botOn),
            ...(expanded ? undefined : { display: "none" }),
          }}
          onMouseLeave={collapse}
        >
          {/* 四边拖拽热区：贴边时不可拖贴边侧（老板拍板 3）；float 状态4边全可拖 */}
          {expanded && (edge === "right" || edge === "left") && (
            <>
              <ResizeEdge side="n" onDelta={(dx, dy, sp) => resizePanel(dx, dy, "n", sp)} />
              <ResizeEdge side="s" onDelta={(dx, dy, sp) => resizePanel(dx, dy, "s", sp)} />
              {edge === "right" && (
                <ResizeEdge side="w" onDelta={(dx, dy, sp) => resizePanel(dx, dy, "w", sp)} />
              )}
              {edge === "left" && (
                <ResizeEdge side="e" onDelta={(dx, dy, sp) => resizePanel(dx, dy, "e", sp)} />
              )}
            </>
          )}
          {expanded && edge === "top" && (
            <ResizeEdge side="s" onDelta={(dx, dy, sp) => resizePanel(dx, dy, "s", sp)} />
          )}
          {expanded && edge === "float" && (
            <>
              <ResizeEdge side="n" onDelta={(dx, dy, sp) => resizePanel(dx, dy, "n", sp)} />
              <ResizeEdge side="s" onDelta={(dx, dy, sp) => resizePanel(dx, dy, "s", sp)} />
              <ResizeEdge side="e" onDelta={(dx, dy, sp) => resizePanel(dx, dy, "e", sp)} />
              <ResizeEdge side="w" onDelta={(dx, dy, sp) => resizePanel(dx, dy, "w", sp)} />
            </>
          )}
          {/* 头部：按住拖动挂件（按钮区不触发拖动） */}
          <div
            className="flex items-center justify-between mb-3 cursor-grab active:cursor-grabbing"
            title="按住拖动挂件"
            onPointerDown={startDrag}
          >
            <div className="flex items-center gap-1.5">
              <img
                src={widgetLogo}
                className="widget-logo w-5 h-5"
                alt=""
                draggable={false}
              />
              <span className="text-sm font-semibold text-[var(--t2)]">WMessage</span>
            </div>
            <div
              className="flex items-center gap-1"
              onPointerDown={(e) => e.stopPropagation()}
            >
              <button
                className={`w-8 h-8 flex items-center justify-center ${
                  view === "all" ? "nm-inset text-[var(--t1)] font-medium" : "nm-outset text-[var(--t4)]"
                }`}
                title="待办"
                onClick={() => setView("all")}
              >
                <svg viewBox="0 0 64 64" width="20" height="20">
                  <rect x="14" y="13" width="34" height="38" rx="2" fill="#ffffff" stroke="#2460b3" strokeWidth="2" />
                  <rect x="46" y="17" width="6" height="30" rx="1" fill="#ffffff" stroke="#2460b3" strokeWidth="2" />
                  <rect x="19" y="19" width="5" height="5" fill="#ffffff" stroke="#2460b3" strokeWidth="2" />
                  <rect x="19" y="27" width="5" height="5" fill="#ffffff" stroke="#2460b3" strokeWidth="2" />
                  <rect x="19" y="35" width="5" height="5" fill="#ffffff" stroke="#2460b3" strokeWidth="2" />
                  <line x1="27" y1="21.5" x2="43" y2="21.5" stroke="#2460b3" strokeWidth="1.3" />
                  <line x1="27" y1="29.5" x2="43" y2="29.5" stroke="#2460b3" strokeWidth="1.3" />
                  <line x1="27" y1="37.5" x2="43" y2="37.5" stroke="#2460b3" strokeWidth="1.3" />
                  <rect x="26" y="11" width="10" height="4" rx="1.5" fill="#f28522" stroke="#f28522" strokeWidth="1" />
                </svg>
              </button>
              <button
                className={`w-8 h-8 flex items-center justify-center ${
                  view === "today" ? "nm-inset text-[var(--t1)] font-medium" : "nm-outset text-[var(--t4)]"
                }`}
                title="今日"
                onClick={() => setView("today")}
              >
                <svg viewBox="0 0 64 64" width="20" height="20">
                  <rect x="13" y="20" width="38" height="28" rx="2" fill="#ffffff" stroke="#2460b3" strokeWidth="2" />
                  <rect x="21" y="15" width="6" height="7" rx="1" fill="#ffffff" stroke="#2460b3" strokeWidth="2" />
                  <rect x="37" y="15" width="6" height="7" rx="1" fill="#ffffff" stroke="#2460b3" strokeWidth="2" />
                  <line x1="13" y1="26" x2="51" y2="26" stroke="#2460b3" strokeWidth="2" />
                  <text x="22" y="41" fontFamily="Arial, sans-serif" fontSize="11" fontWeight="700" fill="#f28522">07</text>
                  <circle cx="44" cy="38" r="6" fill="#ffffff" stroke="#f28522" strokeWidth="1.8" />
                  <line x1="44" y1="29" x2="44" y2="31" stroke="#f28522" strokeWidth="1.6" />
                  <line x1="44" y1="45" x2="44" y2="47" stroke="#f28522" strokeWidth="1.6" />
                  <line x1="35" y1="38" x2="37" y2="38" stroke="#f28522" strokeWidth="1.6" />
                  <line x1="51" y1="38" x2="53" y2="38" stroke="#f28522" strokeWidth="1.6" />
                </svg>
              </button>
              <button
                className={`w-8 h-8 flex items-center justify-center ${
                  view === "done" ? "nm-inset text-[var(--t1)] font-medium" : "nm-outset text-[var(--t4)]"
                }`}
                title="完成"
                onClick={() => setView("done")}
              >
                <svg viewBox="0 0 64 64" width="20" height="20">
                  <circle cx="32" cy="30" r="14" fill="#ffffff" stroke="#2460b3" strokeWidth="2.4" />
                  <circle cx="32" cy="30" r="9" fill="#ffffff" stroke="#2460b3" strokeWidth="1.6" />
                  <polyline points="27,30 30,33 37,26" fill="none" stroke="#f28522" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" />
                  <path d="M21 42 L18 52 L24 48 L30 52 Z" fill="#ffffff" stroke="#2460b3" strokeWidth="2" />
                  <path d="M43 42 L40 52 L46 48 L52 52 Z" fill="#ffffff" stroke="#2460b3" strokeWidth="2" />
                </svg>
              </button>
              <button
                className={`w-8 h-8 flex items-center justify-center ${
                  view === "workspace" ? "nm-inset text-[var(--t1)] font-medium" : "nm-outset text-[var(--t4)]"
                }`}
                title="工作区"
                onClick={() => setView("workspace")}
              >
                <svg viewBox="0 0 64 64" width="20" height="20">
                  <rect x="10" y="24" width="44" height="24" rx="2" fill="#ffffff" stroke="#2460b3" strokeWidth="2" />
                  <rect x="14" y="28" width="36" height="16" fill="#ffffff" stroke="#2460b3" strokeWidth="1.4" />
                  <line x1="18" y1="33" x2="46" y2="33" stroke="#2460b3" strokeWidth="1.2" />
                  <line x1="18" y1="37" x2="42" y2="37" stroke="#2460b3" strokeWidth="1.2" />
                  <line x1="18" y1="41" x2="38" y2="41" stroke="#2460b3" strokeWidth="1.2" />
                  <rect x="42" y="14" width="14" height="12" rx="1" fill="#ffffff" stroke="#2460b3" strokeWidth="2" />
                  <line x1="44" y1="18" x2="54" y2="18" stroke="#f28522" strokeWidth="1.3" />
                  <line x1="44" y1="22" x2="52" y2="22" stroke="#f28522" strokeWidth="1.3" />
                  <path d="M8 48 L56 48 L52 54 L12 54 Z" fill="#ffffff" stroke="#2460b3" strokeWidth="2" />
                </svg>
              </button>
              <button
                className={`w-8 h-8 flex items-center justify-center text-[16px] ${
                  locked ? "nm-inset text-[var(--t1)]" : "nm-outset text-[var(--t4)]"
                }`}
                title={locked ? "取消常驻" : "常驻锁定"}
                onClick={() => setLocked((v) => !v)}
              >
                {locked ? "🔒" : "📌"}
              </button>
            </div>
          </div>

          {/* 新建任务大长条：仅「待办 / 今日」显示，完成 / 工作区视图隐藏 */}
          {view !== "workspace" && view !== "done" && (
            <button
              className="nm-btn w-full mb-2 py-2 text-sm text-[var(--t3)] shrink-0"
              onClick={addTask}
            >
              + 新建任务
            </button>
          )}

          <div
            ref={listRef}
            className="overflow-hidden overflow-y-auto flex flex-col gap-2 pr-0.5 shrink-0"
            style={{ height: "var(--task-h)" }}
          >
            {view === "workspace" ? (
              workspace.length === 0 ? (
                <p className="text-xs text-[var(--t5)] text-center mt-8">
                  暂无工作区
                </p>
              ) : (
                <DndContext
                  sensors={sensors}
                  collisionDetection={closestCenter}
                  onDragEnd={handleWsDragEnd}
                >
                  <SortableContext
                    items={workspace.map((w) => w.id)}
                    strategy={verticalListSortingStrategy}
                  >
                    {workspace.map((it) => (
                      <SortableWorkspaceCard key={it.id} it={it}>
                        {(listeners) => (
                          <div>
                            <div className="flex items-center gap-2">
                              <span
                                {...listeners}
                                title="拖拽排序"
                                className="shrink-0 w-4 h-4 flex items-center justify-center text-[12px] leading-none text-[var(--t5)] rounded hover:bg-[var(--hover-bg)] opacity-0 group-hover:opacity-100 transition-opacity cursor-grab active:cursor-grabbing"
                              >
                                ☰
                              </span>
                              <p className="min-w-0 flex-1 truncate text-xs font-medium text-[var(--t1)]">
                                {it.title}
                              </p>
                              <FoldToggle
                                collapsed={!!it.collapsed}
                                onToggle={() => toggleWsCollapsed(it)}
                                alwaysVisible
                              />
                            </div>
                            {!it.collapsed &&
                              (it.links.length === 0 ? (
                                <p className="mt-2 text-[10px] text-[var(--t5)]">还没有链接</p>
                              ) : (
                                <div className="mt-2 space-y-1">
                                  {it.links.map((link) => (
                                    <button
                                      key={link.id}
                                      className="nm-inset w-full flex items-center gap-2 rounded-lg px-2 py-1.5 text-left"
                                      title={`${linkDisplayName(link)}\n${link.targetUri}`}
                                      onClick={() => openLink(link)}
                                    >
                                      <span className="shrink-0 text-[10px]">
                                        {link.kind === "url"
                                          ? "🔗"
                                          : link.kind === "folder"
                                            ? "📁"
                                            : "📄"}
                                      </span>
                                      <span className="min-w-0 flex-1 truncate text-xs text-[var(--t2)]">
                                        {linkDisplayName(link)}
                                      </span>
                                    </button>
                                  ))}
                                </div>
                              ))}
                          </div>
                        )}
                      </SortableWorkspaceCard>
                    ))}
                  </SortableContext>
                </DndContext>
              )
            ) : list.length === 0 ? (
              <p className="text-xs text-[var(--t5)] text-center mt-8">
                {view === "today"
                  ? "今日暂无任务"
                  : view === "done"
                  ? "暂无已完成任务"
                  : "暂无任务"}
              </p>
            ) : (
              <DndContext
                sensors={sensors}
                collisionDetection={closestCenter}
                onDragEnd={handleDragEnd}
              >
                <SortableContext
                  items={list.map((t) => t.id)}
                  strategy={verticalListSortingStrategy}
                >
                  {list.map((t) => (
                    <SortableTaskCard
                      key={t.id}
                      task={t}
                      editingTitle={editingId === t.id}
                      selected={selectedTasks.some((x) => x.id === t.id)}
                      selectMode={selecting}
                      onSelect={() => toggleSelectTask(t)}
                      onTitleClick={selecting ? undefined : () => openInMain(t)}
                      onCommitTitle={(title) => commitTitle(t, title)}
                      onCancelTitle={cancelTitle}
                      onToggleDone={() => toggleDone(t)}
                      onToggleCollapsed={() => toggleCollapsed(t)}
                      onToggleSubtask={(sid) => toggleSubtask(t, sid)}
                      onOpenFilePath={openFilePath}
                      onCopyFilePath={(p) => copyFilePath(t, p)}
                      onRemoveFile={(p) => removeFile(t, p)}
                      onBotExecute={
                        botOn
                          ? () =>
                              emit("execute-task", {
                                id: t.id,
                                title: t.title,
                              }).catch(() => {})
                          : undefined
                      }
                      onSetSchedule={(sched) => setSchedule(t, sched)}
                    />
                  ))}
                </SortableContext>
              </DndContext>
            )}
          </div>

          {/* Splitter：任务区/聊天区分隔条(bot 开时显示) */}
          {botOn && <SplitBar onSplit={moveSplit} onArrow={arrowSplit} />}

          {/* 聊天区：bot 开时在 Splitter 下方,flex-1 自动填满剩余空间 */}
          {botOn && (
            <div ref={chatAreaRef} className="flex-1 min-h-[120px] overflow-hidden">
              <ChatPanel
                selecting={selecting}
                onToggleSelecting={() => setSelecting((v) => !v)}
                selectedTasks={selectedTasks}
                onRemoveSelected={(id) =>
                  setSelectedTasks((prev) => prev.filter((x) => x.id !== id))
                }
                onFinishSelection={finishSelection}
              />
            </div>
          )}

          {/* 产物绑定弹窗：挂在挂件窗口（面板始终挂载，收起时不卸载不丢事件） */}
          <ArtifactBatchDialog />
        </div>
      }
    </div>
  );
}

/** 挂件可排序任务卡：useSortable 注入，手柄在 TaskCardContent 的 ☰ 上 */
function SortableTaskCard({
  task,
  editingTitle,
  selected,
  selectMode,
  onSelect,
  onTitleClick,
  onCommitTitle,
  onCancelTitle,
  onToggleDone,
  onToggleCollapsed,
  onToggleSubtask,
  onOpenFilePath,
  onCopyFilePath,
  onRemoveFile,
  onBotExecute,
  onSetSchedule,
}: {
  task: Task;
  editingTitle: boolean;
  selected: boolean;
  /** 选任务模式：整卡单击选中（内部交互按钮除外），标题双击去主窗口暂停 */
  selectMode: boolean;
  onSelect?: () => void;
  onTitleClick?: () => void;
  onCommitTitle: (title: string) => void;
  onCancelTitle: () => void;
  onToggleDone: () => void;
  onToggleCollapsed: () => void;
  onToggleSubtask: (subtaskId: string) => void;
  onOpenFilePath: (path: string) => void;
  onCopyFilePath: (path: string) => void;
  onRemoveFile: (path: string) => void;
  onBotExecute?: () => void;
  onSetSchedule?: (schedule: string | undefined) => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } =
    useSortable({ id: task.id });
  const style = { transform: CSS.Transform.toString(transform), transition };
  return (
    <div
      ref={setNodeRef}
      style={style}
      {...attributes}
      onClick={selectMode ? onSelect : undefined}
      className={`group nm-card px-3 py-2 text-left shrink-0 ${
        isDragging ? "opacity-70" : ""
      } ${selected ? "ring-2 ring-[var(--brand)]" : ""} ${
        selectMode ? "cursor-pointer" : ""
      }`}
    >
      <TaskCardContent
        task={task}
        editingTitle={editingTitle}
        handleListeners={listeners}
        onTitleClick={onTitleClick}
        onCommitTitle={onCommitTitle}
        onCancelTitle={onCancelTitle}
        onToggleDone={onToggleDone}
        onToggleCollapsed={onToggleCollapsed}
        onToggleSubtask={onToggleSubtask}
        onOpenFilePath={onOpenFilePath}
        onCopyFilePath={onCopyFilePath}
        onRemoveFile={onRemoveFile}
        onBotExecute={onBotExecute}
        onSetSchedule={onSetSchedule}
      />
    </div>
  );
}

/** 挂件工作区可排序卡片：useSortable 注入，☰ 手柄 listeners 经 render prop 交给标题行 */
function SortableWorkspaceCard({
  it,
  children,
}: {
  it: WorkspaceItem;
  children: (listeners: DraggableSyntheticListeners | undefined) => ReactNode;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } =
    useSortable({ id: it.id });
  const style = { transform: CSS.Transform.toString(transform), transition };
  return (
    <div
      ref={setNodeRef}
      style={style}
      {...attributes}
      className={`group nm-card p-3 ${isDragging ? "opacity-70" : ""}`}
    >
      {children(listeners)}
    </div>
  );
}
