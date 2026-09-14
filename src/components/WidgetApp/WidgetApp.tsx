// WidgetApp 主组件（orchestrator，default export）。
// 1260 → 900 行：constants / storage / ResizeEdge / SplitBar / SortableTaskCard /
// SortableWorkspaceCard 拆到同目录子文件，本文件保留 hooks + handlers + JSX render。
//
// 公开 import 路径保持稳定：外部 `import WidgetApp from "./components/WidgetApp"`
// （默认导入），Vite 解析到 `./WidgetApp/index.tsx` → 透传 `./WidgetApp.tsx` 的 default。

import { useEffect, useRef, useState } from "react";
import {
  DndContext,
  DragEndEvent,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { LogicalPosition, LogicalSize, getCurrentWindow } from "@tauri-apps/api/window";
import { listen, emit } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { openTarget } from "../../lib/openTarget";
import { linkDisplayName } from "../WorkspacePage";

import { handleCommandError } from "../../lib/errorHandler";
import { isDueToday } from "../../format";
import { loadTasksFromDb, loadWorkspaceFromDb, sortByOrder, assignInsertOrder, diffTaskRows } from "../../storage";
import { applySetting, getSetting, subscribeSystem, subscribeTheme } from "../../theme";
import type { ThemeSetting } from "../../theme";
import type { Task, WorkspaceItem } from "../../types";
import { taskFiles, filesPatch } from "../../lib/taskFiles";
import { ChatPanel } from "../ChatPanel";
import { FoldToggle } from "../FoldToggle";
import { ArtifactBatchDialog } from "../ArtifactBatchDialog";
import widgetLogo from "../../assets/widget-logo.png";

import {
  PANEL_H,
  PANEL_H_MAX,
  PANEL_H_MIN,
  PANEL_W,
  PANEL_W_MAX,
  PANEL_W_MIN,
  SPLITTER_H,
  STRIP_H,
  STRIP_W,
  TOP_Y,
  CHAT_H_MIN,
} from "./constants";
import {
  WidgetSize,
  Anchor,
  Edge,
  anchorFromRect,
  getTaskH,
  loadAnchor,
  loadSize,
  saveAnchor,
  saveSize,
  screenSize,
  setTaskH,
  widgetDragActive,
} from "./storage";
import { ResizeEdge } from "./ResizeEdge";
import { SplitBar } from "./SplitBar";
import { SortableTaskCard } from "./SortableTaskCard";
import { SortableWorkspaceCard } from "./SortableWorkspaceCard";


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
  const [size, setSize] = useState<WidgetSize>(() => loadSize() ?? { w: PANEL_W, h: PANEL_H });
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
            c.uiFontSize === "standard" || c.uiFontSize === "large" || c.uiFontSize === "xlarge"
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

  const openLink = (link: WorkspaceItem["links"][number]) => {
    openTarget(link.targetUri);
  };

  /** 挂件工作区折叠切换：与任务卡一致走「上报主窗口落盘」（单写者架构） */
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
      const top = anchor.edge === "top";
      await win.setSize(
        new LogicalSize(top ? STRIP_H : STRIP_W, top ? STRIP_W : STRIP_H)
      );
      await win.setPosition(new LogicalPosition(anchor.x, anchor.y));
    })();
  }, []);

  // 拖动结束（停止移动 500ms）后：贴边吸附 + 记录锚点
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
    const px = e === "right" ? x - (size.w - STRIP_W) : x;
    if (e === "right") await win.setPosition(new LogicalPosition(px, y));
    await win.setSize(new LogicalSize(size.w, panelH(botOn)));
    await win.setPosition(new LogicalPosition(px, y));
    setExpanded(true);
  };

  const collapse = async () => {
    if (locked || widgetDragActive) return;
    const win = getCurrentWindow();
    const pos = await win.outerPosition();
    const size = await win.outerSize();
    const scale = await win.scaleFactor();
    const { w: sw } = await screenSize();
    const { anchor, edge: e2 } = anchorFromRect(
      pos.x / scale, pos.y / scale, size.width / scale, sw
    );
    anchorRef.current = { ...anchor, edge: e2 };
    setEdge(e2);
    saveAnchor({ ...anchor, edge: e2 });
    await win.setPosition(new LogicalPosition(anchor.x, anchor.y));
    await win.setSize(
      new LogicalSize(
        e2 === "top" ? STRIP_H : STRIP_W,
        e2 === "top" ? STRIP_W : STRIP_H
      )
    );
    setExpanded(false);
  };

  // 产物绑定弹窗挂在挂件窗口：面板始终挂载（收起只是 display:none），事件监听不丢
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

  const startDrag = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    getCurrentWindow().startDragging().catch(() => {});
  };

  // 四边拖拽边框：尺寸/位置全部按「拖动起点 + 累计位移」绝对计算
  const resizePanel = (
    dx: number, dy: number,
    side: "n" | "s" | "e" | "w",
    startPos: { x: number; y: number; w: number; h: number; sw: number; sh: number },
  ) => {
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

    let nx = startPos.x;
    let ny = startPos.y;
    if (side === "w") nx = startPos.x + startPos.w - w;
    else if (side === "n") ny = startPos.y + startPos.h - h;

    const winH = botOn ? h : getTaskH() + SPLITTER_H;
    void (async () => {
      try {
        await getCurrentWindow().setSize(new LogicalSize(w, winH));
        await getCurrentWindow().setPosition(new LogicalPosition(nx, ny));
      } catch { /* ignore */ }
    })();
  };

  // Splitter 拖动：只改 CSS 变量 --task-h，聊天区 flex-1 自动反向伸缩
  const moveSplit = (deltaY: number) => {
    if (!botOn) return;
    const cur = getTaskH();
    const chatEl = chatAreaRef.current;
    const maxTask = chatEl
      ? cur + Math.max(0, chatEl.offsetHeight - CHAT_H_MIN)
      : Math.max(0, size.h - 136 - CHAT_H_MIN);
    const next =
      deltaY >= 0
        ? Math.min(Math.max(cur, maxTask), cur + deltaY)
        : Math.max(0, cur + deltaY);
    setTaskH(Math.round(next));
  };
  const arrowSplit = (deltaY: number) => moveSplit(deltaY);

  // 挂件端改动的统一出口：更新本地 state + 行级 diff 上报主窗口
  const applyAndSync = (fn: (prev: Task[]) => Task[]) => {
    const prev = tasksRef.current;
    const next = fn(prev);
    tasksRef.current = next;
    const { upserts, deletes } = diffTaskRows(prev, next, Date.now());
    setTasks(next);
    if (upserts.length || deletes.length) {
      emit("tasks-updated", { upserts, deletes }).catch((e) =>
        handleCommandError(e, "tasks-updated")
      );
    }
  };

  const toggleDone = (t: Task) =>
    applyAndSync((prev) =>
      prev.map((x): Task => {
        if (x.id !== t.id) return x;
        return x.column === "done"
          ? { ...x, column: isDueToday(x.due) ? "doing" : "todo", completedAt: undefined, archived: undefined }
          : { ...x, column: "done", completedAt: Date.now(), archived: false, botAssigned: undefined };
      })
    );

  const toggleCollapsed = (t: Task) =>
    applyAndSync((prev) =>
      prev.map((x) => (x.id === t.id ? { ...x, collapsed: !x.collapsed } : x))
    );

  // 新建任务：今日视图直接进 doing（今日列），否则进 todo；创建后立即进入标题编辑态
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

  const commitTitle = (t: Task, title: string) => {
    const next = title.trim() || t.title;
    applyAndSync((prev) =>
      prev.map((x) => (x.id === t.id ? { ...x, title: next } : x))
    );
    setEditingId(null);
  };

  const setSchedule = (t: Task, schedule: string | undefined) =>
    applyAndSync((prev) =>
      prev.map((x) => (x.id === t.id ? { ...x, schedule } : x))
    );

  const cancelTitle = () => setEditingId(null);

  const openInMain = (t: Task) => {
    emit("edit-task", { id: t.id }).catch(() => {});
    invoke("focus_main_window").catch(() => {});
  };

  const toggleSelectTask = (t: Task) => {
    setSelectedTasks((prev) =>
      prev.some((x) => x.id === t.id)
        ? prev.filter((x) => x.id !== t.id)
        : [...prev, t]
    );
  };

  const finishSelection = () => {
    setSelectedTasks([]);
    setSelecting(false);
  };

  // 进入编辑态时滚回顶部，保证新建任务输入框可见
  useEffect(() => {
    if (editingId) {
      listRef.current?.scrollTo({ top: 0, behavior: "smooth" });
    }
  }, [editingId]);

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

  const openFilePath = (path: string) => {
    openTarget(path);
  };

  const copyFilePath = (t: Task, path: string) => {
    invoke("copy_file_with_title", { path, title: t.title }).catch((e) =>
      handleCommandError(e, "copy_file_with_title", { silent: true })
    );
  };

  const removeFile = (t: Task, path: string) =>
    applyAndSync((prev) =>
      prev.map((x) =>
        x.id !== t.id
          ? x
          : { ...x, ...filesPatch(taskFiles(x).filter((f) => f.path !== path)) }
      )
    );

  // 挂件可见列表排序结束：重建全局顺序
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

  // 挂件「待办 / 今日」显示未完成；「完成」显示已完成列
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
  const edgeClass =
    edge === "right" ? "rounded-l-2xl"
      : edge === "left" ? "rounded-r-2xl"
      : edge === "top" ? "rounded-b-2xl"
      : "rounded-2xl";

  return (
    <div className="w-screen h-screen bg-transparent overflow-hidden">
      {!expanded && (
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
              <img src={widgetLogo} className="widget-logo w-5 h-5" alt="" draggable={false} />
              <span className="text-sm font-semibold text-[var(--t2)]">WMessage</span>
            </div>
            <div
              className="flex items-center gap-1"
              onPointerDown={(e) => e.stopPropagation()}
            >
              {/* 待办 / 今日 / 完成 / 工作区 / 锁定 五个按钮（SVG 内容省略以保持简洁，原始一致） */}
              <button className={`w-8 h-8 flex items-center justify-center ${
                view === "all" ? "nm-inset text-[var(--t1)] font-medium" : "nm-outset text-[var(--t4)]"
              }`} title="待办" onClick={() => setView("all")}>
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
              <button className={`w-8 h-8 flex items-center justify-center ${
                view === "today" ? "nm-inset text-[var(--t1)] font-medium" : "nm-outset text-[var(--t4)]"
              }`} title="今日" onClick={() => setView("today")}>
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
              <button className={`w-8 h-8 flex items-center justify-center ${
                view === "done" ? "nm-inset text-[var(--t1)] font-medium" : "nm-outset text-[var(--t4)]"
              }`} title="完成" onClick={() => setView("done")}>
                <svg viewBox="0 0 64 64" width="20" height="20">
                  <circle cx="32" cy="30" r="14" fill="#ffffff" stroke="#2460b3" strokeWidth="2.4" />
                  <circle cx="32" cy="30" r="9" fill="#ffffff" stroke="#2460b3" strokeWidth="1.6" />
                  <polyline points="27,30 30,33 37,26" fill="none" stroke="#f28522" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" />
                  <path d="M21 42 L18 52 L24 48 L30 52 Z" fill="#ffffff" stroke="#2460b3" strokeWidth="2" />
                  <path d="M43 42 L40 52 L46 48 L52 52 Z" fill="#ffffff" stroke="#2460b3" strokeWidth="2" />
                </svg>
              </button>
              <button className={`w-8 h-8 flex items-center justify-center ${
                view === "workspace" ? "nm-inset text-[var(--t1)] font-medium" : "nm-outset text-[var(--t4)]"
              }`} title="工作区" onClick={() => setView("workspace")}>
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
                <p className="text-xs text-[var(--t5)] text-center mt-8">暂无工作区</p>
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
                                        {link.kind === "url" ? "🔗" : link.kind === "folder" ? "📁" : "📄"}
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
                              emit("execute-task", { id: t.id, title: t.title }).catch(() => {})
                          : undefined
                      }
                      onSetSchedule={(sched) => setSchedule(t, sched)}
                    />
                  ))}
                </SortableContext>
              </DndContext>
            )}
          </div>

          {botOn && <SplitBar onSplit={moveSplit} onArrow={arrowSplit} />}

          <div
            ref={chatAreaRef}
            className={
              botOn
                ? "flex-1 min-h-[120px] overflow-hidden"
                : "h-0 min-h-0 overflow-hidden"
            }
            aria-hidden={!botOn}
          >
            <ChatPanel
              enabled={botOn}
              selecting={selecting}
              onToggleSelecting={() => setSelecting((v) => !v)}
              selectedTasks={selectedTasks}
              onRemoveSelected={(id) =>
                setSelectedTasks((prev) => prev.filter((x) => x.id !== id))
              }
              onFinishSelection={finishSelection}
            />
          </div>

          <ArtifactBatchDialog />
        </div>
      }
    </div>
  );
}
