import { useEffect, useState } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { KanbanBoard } from "./components/KanbanBoard";
import { ArchivePage } from "./components/ArchivePage";
import { TrashPage } from "./components/TrashPage";
import { STORAGE_KEY } from "./storage";
import type { ColumnId, Task } from "./types";

const SEED: Task[] = [
  {
    id: "t1",
    title: "梳理 WMessage 需求清单",
    due: "2026-08-14",
    column: "todo",
    subtasks: [
      { id: "s1", text: "确认拖拽交互", done: true },
      { id: "s2", text: "定里程碑计划", done: false },
    ],
  },
  { id: "t2", title: "过一遍新拟态样式细节", due: "2026-08-14T18:00", column: "doing" },
  { id: "t3", title: "完成看板拖拽原型", column: "done" },
];

function localDateStr(): string {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(
    d.getDate()
  ).padStart(2, "0")}`;
}

function isDueToday(due?: string): boolean {
  return !!due && due.slice(0, 10) === localDateStr();
}

// 今日规则：截止日期为当天的任务自动进「今日」列（「完成」列不受影响）
function applyTodayRule(tasks: Task[]): Task[] {
  return tasks.map((t) =>
    isDueToday(t.due) && t.column === "todo" && !t.deletedAt
      ? { ...t, column: "doing" }
      : t
  );
}

// 归档规则：完成超过 7 天的任务自动归档，从「完成」列隐藏
const ARCHIVE_AFTER_MS = 7 * 24 * 60 * 60 * 1000;

function applyArchiveRule(tasks: Task[]): Task[] {
  const now = Date.now();
  return tasks.map((t) => {
    if (t.column !== "done" || t.archived || t.deletedAt) return t;
    const completedAt = t.completedAt ?? now; // 老数据补完成时间
    if (now - completedAt >= ARCHIVE_AFTER_MS)
      return { ...t, completedAt, archived: true };
    return t.completedAt === completedAt ? t : { ...t, completedAt };
  });
}

function loadTasks(): Task[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    const parsed = raw ? (JSON.parse(raw) as Task[]) : SEED;
    return applyArchiveRule(applyTodayRule(parsed));
  } catch {
    return applyArchiveRule(applyTodayRule(SEED));
  }
}

export default function App() {
  const [tasks, setTasks] = useState<Task[]>(loadTasks);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [view, setView] = useState<"board" | "archive" | "trash">("board");

  // 本地持久化 + 通知挂件窗口同步
  useEffect(() => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(tasks));
    emit("tasks-changed").catch(() => {});
  }, [tasks]);

  // 挂件端改动（如点圆圈完成）写 localStorage 后 emit tasks-changed，这里回读同步。
  // 相等守卫：自己 emit 的回声不触发 setState，避免循环。
  useEffect(() => {
    const unlisten = listen("tasks-changed", () => {
      setTasks((prev) => {
        try {
          const raw = localStorage.getItem(STORAGE_KEY);
          const parsed = raw ? (JSON.parse(raw) as Task[]) : prev;
          const next = applyArchiveRule(applyTodayRule(parsed));
          return JSON.stringify(next) === JSON.stringify(prev) ? prev : next;
        } catch {
          return prev;
        }
      });
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // 挂件点标题 → 打开该任务编辑态（切回看板视图 + 标题自动进入编辑）
  useEffect(() => {
    const unlisten = listen<{ id?: string }>("edit-task", (e) => {
      const id = e.payload?.id;
      if (id) {
        setView("board");
        setEditingId(id);
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // 每分钟重套今日规则 + 归档规则：覆盖跨零点归位、完成超时归档
  useEffect(() => {
    const id = setInterval(
      () => setTasks((prev) => applyArchiveRule(applyTodayRule(prev))),
      60_000
    );
    return () => clearInterval(id);
  }, []);

  const addTask = () => {
    const id = crypto.randomUUID();
    setTasks((prev) => [...prev, { id, title: "新任务", column: "todo" }]);
    setEditingId(id); // 新建后自动进入编辑态
  };

  const moveTask = (taskId: string, column: ColumnId) => {
    setTasks((prev) =>
      prev.map((t) => {
        if (t.id !== taskId) return t;
        if (column === "done")
          // 进入完成列：记完成时间，取消归档
          return { ...t, column, completedAt: Date.now(), archived: false };
        // 拖出完成列：清除完成时间与归档标记
        return { ...t, column, completedAt: undefined, archived: undefined };
      })
    );
  };

  const updateTask = (taskId: string, patch: Partial<Task>) => {
    setTasks((prev) =>
      prev.map((t) => {
        if (t.id !== taskId) return t;
        const next = { ...t, ...patch };
        // 设了截止日期后立即套用今日规则
        if (patch.due !== undefined && isDueToday(next.due) && next.column === "todo") {
          next.column = "doing";
        }
        return next;
      })
    );
    setEditingId(null);
  };

  // 软删除：进回收站
  const deleteTask = (taskId: string) => {
    setTasks((prev) =>
      prev.map((t) => (t.id === taskId ? { ...t, deletedAt: Date.now() } : t))
    );
    setEditingId((cur) => (cur === taskId ? null : cur));
  };

  // 彻底删除（回收站）
  const hardDeleteTask = (taskId: string) => {
    setTasks((prev) => prev.filter((t) => t.id !== taskId));
  };

  const clearTrash = () => {
    setTasks((prev) => prev.filter((t) => !t.deletedAt));
  };

  return (
    <div className="min-h-screen bg-[#f4f7fa] p-6">
      <header className="mb-4 flex items-center justify-between">
        <div className="flex items-center gap-3">
          <h1 className="text-xl font-semibold text-gray-700">WMessage</h1>
          <div className="flex gap-1">
            <button
              className={`min-w-[94px] px-3 py-1.5 text-sm text-gray-600 ${
                view === "board" ? "nm-inset" : "nm-outset"
              }`}
              onClick={() => setView("board")}
            >
              看板
            </button>
            <button
              className={`min-w-[94px] px-3 py-1.5 text-sm text-gray-600 ${
                view === "archive" ? "nm-inset" : "nm-outset"
              }`}
              onClick={() => setView("archive")}
            >
              归档
            </button>
            <button
              className={`min-w-[94px] px-3 py-1.5 text-sm text-gray-600 ${
                view === "trash" ? "nm-inset" : "nm-outset"
              }`}
              onClick={() => setView("trash")}
            >
              回收站
            </button>
          </div>
        </div>
        {view === "board" && (
          <button
            className="nm-inset px-3 py-1.5 text-sm text-gray-600"
            onClick={addTask}
          >
            + 新建任务
          </button>
        )}
      </header>
      {view === "board" ? (
        <KanbanBoard
          tasks={tasks}
          editingId={editingId}
          onMove={moveTask}
          onUpdate={updateTask}
          onDelete={deleteTask}
          onOpenArchive={() => setView("archive")}
        />
      ) : view === "archive" ? (
        <ArchivePage tasks={tasks} onUpdate={updateTask} onDelete={deleteTask} />
      ) : (
        <TrashPage
          tasks={tasks}
          onUpdate={updateTask}
          onDelete={hardDeleteTask}
          onClearAll={clearTrash}
        />
      )}
    </div>
  );
}
