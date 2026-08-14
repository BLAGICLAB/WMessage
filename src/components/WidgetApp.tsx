import { useEffect, useRef, useState } from "react";
import {
  LogicalPosition,
  LogicalSize,
  currentMonitor,
  getCurrentWindow,
} from "@tauri-apps/api/window";
import { listen, emit } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { openPath } from "@tauri-apps/plugin-opener";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { loadTasksFromDb, taskEq } from "../storage";
import type { Task } from "../types";
import { TaskCardContent } from "./TaskCardContent";

// 收起为触发条 / 展开为侧边面板
const STRIP_W = 44;
const STRIP_H = 220;
const PANEL_W = 320;
const PANEL_H = 560;
const TOP_Y = 140; // 默认贴右缘的初始 Y

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

export default function WidgetApp() {
  const [tasks, setTasks] = useState<Task[]>([]);
  const tasksRef = useRef<Task[]>([]);
  const [expanded, setExpanded] = useState(false);
  const [locked, setLocked] = useState(false);
  const [view, setView] = useState<"all" | "today">("all");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [edge, setEdge] = useState<Edge>("right");
  const anchorRef = useRef<Anchor>({ x: 0, y: TOP_Y, edge: "right" });
  const listRef = useRef<HTMLDivElement>(null);

  // 任务数据同步：主窗口统一落盘 SQLite，挂件只读。三层：初始读取 + tasks-changed + 5s 兜底轮询
  useEffect(() => {
    const load = async () => {
      try {
        const list = await loadTasksFromDb();
        if (!list.length) return;
        setTasks((prev) => {
          const same = JSON.stringify(list) === JSON.stringify(prev);
          if (!same) tasksRef.current = list;
          return same ? prev : list;
        });
      } catch {
        /* ignore */
      }
    };
    load();
    const unlisten = listen("tasks-changed", () => {
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

  // 初始：透明背景 + 置顶 + 恢复到上次位置（默认贴右缘）
  useEffect(() => {
    document.body.style.background = "transparent";
    const win = getCurrentWindow();
    win.setAlwaysOnTop(true).catch(() => {});
    (async () => {
      const { w: sw } = await screenSize();
      const saved = loadAnchor();
      const anchor: Anchor = saved ?? { x: sw - STRIP_W, y: TOP_Y, edge: "right" };
      anchorRef.current = anchor;
      setEdge(anchor.edge);
      await win.setSize(new LogicalSize(STRIP_W, STRIP_H));
      await win.setPosition(new LogicalPosition(anchor.x, anchor.y));
    })();
  }, []);

  // 拖动结束（停止移动 500ms）后：贴边吸附 + 记录锚点，圆角跟随所在边缘
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
    const px = e === "right" ? x - (PANEL_W - STRIP_W) : x;
    if (e === "right") await win.setPosition(new LogicalPosition(px, y));
    await win.setSize(new LogicalSize(PANEL_W, PANEL_H));
    await win.setPosition(new LogicalPosition(px, y));
    setExpanded(true);
  };

  const collapse = async () => {
    if (locked) return;
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
    await win.setSize(new LogicalSize(STRIP_W, STRIP_H));
    setExpanded(false);
  };

  // 面板头部为拖动手柄：按住拖动挂件到任意位置
  const startDrag = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    getCurrentWindow().startDragging().catch(() => {});
  };

  // 挂件端改动的统一出口：更新本地 state + 行级 diff 上报主窗口（主窗口负责落盘 SQLite）
  const applyAndSync = (fn: (prev: Task[]) => Task[]) => {
    const prev = tasksRef.current;
    const next = fn(prev);
    tasksRef.current = next;
    const prevMap = new Map(prev.map((t) => [t.id, t]));
    const upserts = next.filter((t) => {
      const p = prevMap.get(t.id);
      return !p || !taskEq(p, t);
    });
    const nextIds = new Set(next.map((t) => t.id));
    const deletes = prev.filter((t) => !nextIds.has(t.id)).map((t) => t.id);
    setTasks(next);
    if (upserts.length || deletes.length) {
      emit("tasks-updated", { upserts, deletes }).catch(() => {});
    }
  };

  // 点圆圈完成/取消完成：与主窗口行为一致（完成 → 记时间；取消 → 退回待办）
  const toggleDone = (t: Task) =>
    applyAndSync((prev) =>
      prev.map((x): Task => {
        if (x.id !== t.id) return x;
        return x.column === "done"
          ? { ...x, column: "todo", completedAt: undefined, archived: undefined }
          : { ...x, column: "done", completedAt: Date.now(), archived: false };
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
    applyAndSync((prev) => [
      { id, title: "新任务", column: view === "today" ? "doing" : "todo" },
      ...prev,
    ]);
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

  const cancelTitle = () => setEditingId(null);

  // 点标题 → 通知主窗口打开该任务编辑态 + 聚焦主窗口（挂件内不再内联编辑）
  const openInMain = (t: Task) => {
    emit("edit-task", { id: t.id }).catch(() => {});
    focusMain();
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

  // 打开绑定文件/文件夹（与主窗口一致）
  const openFile = (t: Task) => {
    if (t.filePath)
      openPath(t.filePath).catch((e) => console.error("open failed", e));
  };

  // 复制文件+标题（与主窗口一致）
  const copyFile = (t: Task) => {
    if (t.filePath)
      invoke("copy_file_with_title", { path: t.filePath, title: t.title }).catch((e) =>
        console.error("copy failed", e)
      );
  };

  // 点击任务 → 聚焦主窗口（主窗口关闭时 Rust 侧已改为隐藏，因此始终能唤起）
  const focusMain = async () => {
    try {
      const main = await WebviewWindow.getByLabel("main");
      if (main) {
        await main.show();
        await main.setFocus();
      } else {
        console.warn("focusMain: 主窗口不存在");
      }
    } catch (e) {
      console.error("focusMain failed", e);
    }
  };

  // 挂件只显示未完成：todo + doing（打勾后进完成列 → 从挂件消失）
  const visible = tasks.filter(
    (t) => !t.archived && !t.deletedAt && t.column !== "done"
  );
  const list = view === "today" ? visible.filter((t) => t.column === "doing") : visible;

  // 圆角跟随所在边缘：贴右 → 左圆角；贴左 → 右圆角；贴顶 → 下圆角；悬浮 → 全圆角
  const ROUNDED: Record<Edge, string> = {
    right: "rounded-l-2xl",
    left: "rounded-r-2xl",
    top: "rounded-b-2xl",
    float: "rounded-2xl",
  };
  const edgeClass = ROUNDED[edge];

  return (
    <div className="w-screen h-screen bg-transparent overflow-hidden">
      {!expanded ? (
        /* 触发条：贴边隐藏状态 */
        <div
          className={`nm-sidebar-panel ${edgeClass} !p-2 w-full h-full flex flex-col items-center justify-center gap-2 cursor-pointer select-none`}
          onMouseEnter={expand}
        >
          <span className="text-sm leading-none">🗂</span>
          <span
            className="text-gray-500 text-xs tracking-widest"
            style={{ writingMode: "vertical-rl" }}
          >
            WMessage
          </span>
        </div>
      ) : (
        /* 展开面板 */
        <div
          className={`nm-sidebar-panel ${edgeClass} w-full h-full flex flex-col`}
          onMouseLeave={collapse}
        >
          {/* 头部：按住拖动挂件（按钮区不触发拖动） */}
          <div
            className="flex items-center justify-between mb-3 cursor-grab active:cursor-grabbing"
            title="按住拖动挂件"
            onPointerDown={startDrag}
          >
            <span className="text-sm font-semibold text-gray-700">WMessage</span>
            <div
              className="flex items-center gap-1"
              onPointerDown={(e) => e.stopPropagation()}
            >
              <button
                className={`nm-inset px-2 py-0.5 text-xs ${
                  view === "all" ? "text-gray-800 font-medium" : "text-gray-500"
                }`}
                onClick={() => setView("all")}
              >
                全部
              </button>
              <button
                className={`nm-inset px-2 py-0.5 text-xs ${
                  view === "today" ? "text-gray-800 font-medium" : "text-gray-500"
                }`}
                onClick={() => setView("today")}
              >
                今日
              </button>
              <button
                className={`nm-inset px-2 py-0.5 text-xs ${
                  locked ? "text-gray-800" : "text-gray-500"
                }`}
                title={locked ? "取消常驻" : "常驻锁定"}
                onClick={() => setLocked((v) => !v)}
              >
                {locked ? "🔒" : "📌"}
              </button>
            </div>
          </div>

          {/* 新建任务大长条：位于「全部/今日」下方，新建的任务从顶部出现 */}
          <button
            className="nm-inset w-full mb-2 py-2 text-sm text-gray-600 shrink-0"
            onClick={addTask}
          >
            + 新建任务
          </button>

          <div
            ref={listRef}
            className="flex-1 overflow-y-auto flex flex-col gap-2 pr-0.5"
          >
            {list.length === 0 ? (
              <p className="text-xs text-gray-400 text-center mt-8">
                {view === "today" ? "今日暂无任务" : "暂无任务"}
              </p>
            ) : (
              list.map((t) => (
                <div
                  key={t.id}
                  className="nm-card px-3 py-2 text-left shrink-0 cursor-pointer"
                  title="点击聚焦主窗口"
                  onClick={focusMain}
                >
                  {/* 与主窗口 TodoCard 展示一致：标题（折叠 + 打勾圆圈） → 备注 → 标签 → 子任务 → 文件 → 截止 */}
                  <TaskCardContent
                    task={t}
                    editingTitle={editingId === t.id}
                    onTitleClick={() => openInMain(t)}
                    onCommitTitle={(title) => commitTitle(t, title)}
                    onCancelTitle={cancelTitle}
                    onToggleDone={() => toggleDone(t)}
                    onToggleCollapsed={() => toggleCollapsed(t)}
                    onToggleSubtask={(sid) => toggleSubtask(t, sid)}
                    onOpenFile={() => openFile(t)}
                    onCopyFile={() => copyFile(t)}
                  />
                </div>
              ))
            )}
          </div>
        </div>
      )}
    </div>
  );
}
