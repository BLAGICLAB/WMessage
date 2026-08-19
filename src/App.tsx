import { useEffect, useRef, useState } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import { KanbanBoard } from "./components/KanbanBoard";
import mainLogo from "./assets/main-logo.png";
import { ArchivePage } from "./components/ArchivePage";
import { TrashPage } from "./components/TrashPage";
import { WorkspacePage } from "./components/WorkspacePage";
import { SettingsPage } from "./components/SettingsPage";
import { deleteTaskRows, loadTasksFromDb, taskEq, upsertTasks, exportTasksToFile, importTasksFromFile, STORAGE_KEY, sortByOrder, assignInsertOrder, upsertWorkspaceItems } from "./storage";
import { handleCommandError } from "./lib/errorHandler";
import { applySetting, getSetting, subscribeSystem, subscribeTheme, toggleTheme } from "./theme";
import { isDueToday } from "./format";
import type { ThemeSetting } from "./theme";
import type { Task, WorkspaceItem } from "./types";

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

export default function App() {
  const [tasks, setTasks] = useState<Task[]>([]);
  const tasksRef = useRef<Task[]>([]);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [view, setView] = useState<"board" | "archive" | "workspace" | "trash" | "settings">("board");
  const [theme, setTheme] = useState<ThemeSetting>(getSetting);

  // 主题：启动时应用 + 监听其他窗口（挂件）切换 + 跟随系统模式监听系统外观变化
  useEffect(() => {
    applySetting(theme);
  }, [theme]);
  useEffect(() => subscribeTheme(setTheme), []);
  useEffect(() => subscribeSystem(setTheme), []);
  // 全局快捷键 Cmd/Ctrl+Alt+T：Rust 侧发 toggle-theme 给主窗口
  useEffect(() => {
    const unlisten = listen("toggle-theme", () => setTheme(toggleTheme()));
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // 初始加载：SQLite → 空库迁移（旧 data.json 在 Rust 侧处理；更早的 localStorage 在此处理）→ 种子
  useEffect(() => {
    (async () => {
      try {
        const res = await loadTasksFromDb();
        if (!res.ok) {
          // 读失败 ≠ 空库：禁止走迁移/种子分支（避免覆盖真实数据），保持内存空数组并明确告警
          // recoverable=true 时 handleCommandError 会弹 confirm 提供「重试」（重载重新加载）
          handleCommandError(res.error, "db_load", {
            onRetry: () => window.location.reload(),
          });
          return;
        }
        let list = res.tasks;        if (list.length === 0) {
          let migrated = false;
          try {
            const legacy = localStorage.getItem(STORAGE_KEY);
            if (legacy) {
              const parsed = JSON.parse(legacy) as Task[];
              if (parsed.length) {
                await upsertTasks(parsed);
                list = parsed;
                migrated = true;
              }
              localStorage.removeItem(STORAGE_KEY);
            }
          } catch {
            /* ignore */
          }
          if (!migrated) {
            await upsertTasks(SEED);
            list = SEED;
          }
        }
        let next = applyArchiveRule(applyTodayRule(list));
        // 老数据补 order / updatedAt（updatedAt 缺失视为最旧 0），一次性落盘
        if (
          next.some((t) => t.order === undefined || t.updatedAt === undefined)
        ) {
          next = next.map((t, i) => ({
            ...t,
            order: t.order ?? i,
            updatedAt: t.updatedAt ?? 0,
          }));
          await upsertTasks(next);
        }
        next = sortByOrder(next);
        tasksRef.current = next;
        setTasks(next);
      } catch (e) {
        handleCommandError(e, "init load", { silent: true });
      }
    })();
  }, []);

  // 统一变更出口：计算新数组 → diff → 行级增量落盘（await 落盘完成）→ 更新 state → 广播挂件
  const mutate = async (fn: (prev: Task[]) => Task[]) => {
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
    // 打最后修改时间戳（合并导入按此比较同 id 取舍）
    const now = Date.now();
    upserts.forEach((t) => {
      t.updatedAt = now;
    });
    // 先落盘再广播：挂件收到 tasks-changed 后立刻 db_load，必须读到已提交的快照
    await upsertTasks(upserts);
    await deleteTaskRows(deletes);
    setTasks(next);
    if (upserts.length || deletes.length) {
      try {
        await emit("tasks-changed");
      } catch (e) {
        console.error("emit tasks-changed failed", e);
      }
    }
  };

  // 挂件上报工作区变更（workspace-updated：{upserts}），主窗口统一落盘后广播（单写者架构）
  useEffect(() => {
    const unlisten = listen<{ upserts?: WorkspaceItem[] }>(
      "workspace-updated",
      (e) => {
        const upserts = e.payload?.upserts ?? [];
        if (!upserts.length) return;
        upsertWorkspaceItems(upserts)
          .then(() => emit("workspace-changed").catch(() => {}))
          // P2-34：写失败不再空 catch 吞掉——storage 层已 alert，这里 console 留痕不重复打扰
          .catch((e) =>
            handleCommandError(e, "workspace-updated upsert", { silent: true })
          );
      }
    );
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // 挂件上报行级变更（tasks-updated：{upserts, deletes}），主窗口统一落盘后广播
  useEffect(() => {
    const unlisten = listen<{ upserts?: Task[]; deletes?: string[]; source?: string }>(
      "tasks-updated",
      async (e) => {
        const upserts = e.payload?.upserts ?? [];
        const deletes = e.payload?.deletes ?? [];
        if (!upserts.length && !deletes.length) return;
        // source:"api" / "migration" / "bot"：已由后端线程落盘，这里只合并 UI 状态，不回写，
        // 否则主窗口的异步回写会用旧事件快照覆盖后端的新写入（归档/软删被回滚）
        if (
          e.payload?.source !== "api" &&
          e.payload?.source !== "migration" &&
          e.payload?.source !== "bot"
        ) {
          // await 落盘完成后再合并/广播，避免挂件 db_load 读到未提交快照
          await upsertTasks(upserts);
          await deleteTaskRows(deletes);
        }
        // 合并 + 套规则（在 setTasks 之外基于 tasksRef 计算，保证 updater 纯净）
        const map = new Map(tasksRef.current.map((t) => [t.id, t]));
        upserts.forEach((t) => map.set(t.id, t));
        deletes.forEach((id) => map.delete(id));
        const merged = sortByOrder([...map.values()]);
        const next = applyArchiveRule(applyTodayRule(merged));
        // E2（2026-08-19）：规则改动（今日归位/超时归档）也要落盘——
        // 否则只改内存，重启/挂件读 db 又回到原始数据，三端长期不一致
        const mergedMap = new Map(merged.map((t) => [t.id, t]));
        const ruleChanged = next.filter((t) => !taskEq(mergedMap.get(t.id)!, t));
        if (ruleChanged.length) {
          const now = Date.now();
          ruleChanged.forEach((t) => {
            t.updatedAt = now;
          });
          await upsertTasks(ruleChanged);
          // 观测行（py_audit 是 Rust 内部函数、未暴露为前端命令，前端用 console 同格式记录）
          console.info(`[tasks-updated] merge_tasks | ${ruleChanged.length} changed`);
        }
        tasksRef.current = next;
        setTasks(next);
        try {
          await emit("tasks-changed");
        } catch (err) {
          console.error("emit tasks-changed failed", err);
        }
      }
    );
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // 挂件点标题 → 打开该任务编辑态（切回看板视图 + 标题自动进入编辑）；
  // 机器人搜出的归档任务点 📌 → 跳归档页（归档任务不在看板）
  useEffect(() => {
    const unlisten = listen<{ id?: string }>("edit-task", (e) => {
      const id = e.payload?.id;
      if (id) {
        const t = tasksRef.current.find((x) => x.id === id);
        // 按任务实际所在位置跳转：回收站 → trash；归档 → archive；其余 → 看板
        if (t?.deletedAt) setView("trash");
        else if (t?.archived) setView("archive");
        else setView("board");
        setEditingId(id);
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // 全局快捷键快速新建（Rust 侧 Cmd+Ctrl+N / Ctrl+Alt+N）：切回看板 + 新建任务进入编辑态
  useEffect(() => {
    const unlisten = listen("quick-add", () => {
      setView("board");
      addTask();
    });
    return () => {
      unlisten.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 每分钟重套今日规则 + 归档规则：覆盖跨零点归位、完成超时归档（diff 后行级落盘）
  useEffect(() => {
    const id = setInterval(
      () => mutate((prev) => applyArchiveRule(applyTodayRule(prev))),
      60_000
    );
    return () => clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const addTask = () => {
    const id = crypto.randomUUID();
    mutate((prev) => {
      const max = prev.reduce((m, t) => Math.max(m, t.order ?? 0), 0);
      return [...prev, { id, title: "新任务", column: "todo", order: max + 1 }];
    });
    setEditingId(id); // 新建后自动进入编辑态
  };

  // 导出任务数据：全量任务卡（含归档、回收站）写 JSON 文件
  const exportTasks = async () => {
    try {
      const path = await save({
        defaultPath: `wmessage-tasks-${localDateStr()}.json`,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (!path) return; // 用户取消
      const count = await exportTasksToFile(path);
      alert(`导出完成：共 ${count} 条任务卡`);
    } catch (e) {
      handleCommandError(e, "tasks_export", {
        onRetry: () => void exportTasks(),
      });
    }
  };

  // 导入任务数据：JSON 文件按 id 合并，同 id 保留更晚修改；导入后重读全量 + 套规则 + 广播挂件
  const importTasks = async () => {
    try {
      const selected = await open({
        multiple: false,
        directory: false,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (typeof selected !== "string") return; // 用户取消
      const merged = await importTasksFromFile(selected);
      // 重读全量数据（含合并结果）→ 套规则 → 排序 → 更新状态并广播挂件
      const res = await loadTasksFromDb();
      if (!res.ok) throw res.error;
      const next = applyArchiveRule(applyTodayRule(sortByOrder(res.tasks)));
      tasksRef.current = next;
      setTasks(next);
      emit("tasks-changed").catch(() => {});
      alert(`导入完成：本次写入 ${merged} 条任务卡`);
    } catch (e) {
      handleCommandError(e, "tasks_import", {
        onRetry: () => void importTasks(),
      });
    }
  };

  // 看板拖拽排序提交：数组顺序已由 KanbanBoard 排好（含跨列变更），
  // 这里补列变更完成语义（进完成列记时间、出完成列清除），再给被拖任务分配 order
  const commitBoardOrder = (activeId: string, next: Task[]) => {
    mutate((prev) => {
      const prevMap = new Map(prev.map((t) => [t.id, t]));
      const arr = next.map((t) => {
        const p = prevMap.get(t.id);
        if (!p || p.column === t.column) return t;
        if (t.column === "done")
          // 进入完成列：记完成时间，取消归档
          return { ...t, completedAt: Date.now(), archived: false };
        // 拖出完成列：清除完成时间与归档标记
        return { ...t, completedAt: undefined, archived: undefined };
      });
      return assignInsertOrder(arr, activeId);
    });
  };

  const updateTask = (taskId: string, patch: Partial<Task>) => {
    mutate((prev) =>
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
    mutate((prev) =>
      prev.map((t) => (t.id === taskId ? { ...t, deletedAt: Date.now() } : t))
    );
    setEditingId((cur) => (cur === taskId ? null : cur));
  };

  // 彻底删除（回收站）
  const hardDeleteTask = (taskId: string) => {
    mutate((prev) => prev.filter((t) => t.id !== taskId));
  };

  const clearTrash = () => {
    mutate((prev) => prev.filter((t) => !t.deletedAt));
  };

  return (
    <div className="min-h-screen bg-[var(--bg)] p-6">
      <header className="mb-4 flex items-center justify-between">
        <div className="flex items-center gap-3">
          <img
              src={mainLogo}
              alt="WMessage"
              className="w-7 h-7 shrink-0"
              draggable={false}
            />
            <h1 className="text-xl font-semibold text-[var(--t2)]">WMessage</h1>
          <div className="flex gap-1">
            <button
              className={`min-w-[94px] px-3 py-1.5 text-sm text-[var(--t3)] ${
                view === "board" ? "nm-inset" : "nm-outset"
              }`}
              onClick={() => setView("board")}
            >
              首页
            </button>
            <button
              className={`min-w-[94px] px-3 py-1.5 text-sm text-[var(--t3)] ${
                view === "archive" ? "nm-inset" : "nm-outset"
              }`}
              onClick={() => setView("archive")}
            >
              归档
            </button>
            <button
              className={`min-w-[94px] px-3 py-1.5 text-sm text-[var(--t3)] ${
                view === "workspace" ? "nm-inset" : "nm-outset"
              }`}
              onClick={() => setView("workspace")}
            >
              工作区
            </button>
            <button
              className={`min-w-[94px] px-3 py-1.5 text-sm text-[var(--t3)] ${
                view === "trash" ? "nm-inset" : "nm-outset"
              }`}
              onClick={() => setView("trash")}
            >
              回收站
            </button>
          </div>
        </div>
        <div className="flex items-center gap-2">
          {view === "board" && (
            <button
              className="nm-btn px-3 py-1.5 text-sm text-[var(--t3)]"
              onClick={addTask}
            >
              + 新建任务
            </button>
          )}
          <button
            className={`px-3 py-1.5 text-sm text-[var(--t3)] ${
              view === "settings" ? "nm-inset" : "nm-outset"
            }`}
            title="设置"
            onClick={() => setView("settings")}
          >
            ⚙️
          </button>
        </div>
      </header>
      {view === "board" ? (
        <KanbanBoard
          tasks={tasks}
          editingId={editingId}
          onReorder={commitBoardOrder}
          onUpdate={updateTask}
          onDelete={deleteTask}
          onOpenArchive={() => setView("archive")}
        />
      ) : view === "archive" ? (
        <ArchivePage
          tasks={tasks}
          editingId={editingId}
          onUpdate={updateTask}
          onDelete={deleteTask}
        />
      ) : view === "workspace" ? (
        <WorkspacePage />
      ) : view === "trash" ? (
        <TrashPage
          tasks={tasks}
          editingId={editingId}
          onUpdate={updateTask}
          onDelete={hardDeleteTask}
          onClearAll={clearTrash}
        />
      ) : (
        <SettingsPage
          theme={theme}
          onThemeChange={setTheme}
          onExportTasks={exportTasks}
          onImportTasks={importTasks}
        />
      )}
    </div>
  );
}
