import { Component, useEffect, useRef, useState, type ErrorInfo, type ReactNode } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { isPermissionGranted, requestPermission } from "@tauri-apps/plugin-notification";
import { KanbanBoard } from "./components/KanbanBoard";
import mainLogo from "./assets/main-logo.png";
import { ArchivePage } from "./components/ArchivePage";
import { TrashPage } from "./components/TrashPage";
import { WorkspacePage } from "./components/WorkspacePage";
import { SettingsPage } from "./components/SettingsPage";
import ConfirmMap from "./components/ConfirmMap";
import { deleteTaskRows, diffTaskRows, loadTasksFromDb, taskEq, upsertTasks, exportTasksToFile, importTasksFromFile, exportWorkspaceToFile, importWorkspaceFromFile, STORAGE_KEY, sortByOrder, assignInsertOrder, upsertWorkspaceItems } from "./storage";
import { handleCommandError } from "./lib/errorHandler";
import { isBackendPersisted, KNOWN_SOURCES } from "./lib/mutationOrigin";

import { applySetting, getSetting, subscribeSystem, subscribeTheme, toggleTheme } from "./theme";
import { isDueToday } from "./format";
import type { ThemeSetting } from "./theme";
import type { Task, ColumnId, WorkspaceItem } from "./types";

// OCR C4-r1 验证 + 修：mutate 串行化队列必须在**模块作用域**（App() 内会被每
//   次 render 重建 → 跨 render 的并发 mutate 落到不同链 → 串行化失效）。
//   抬到此处后全 App 实例共享同一链，跨 render 与 unmount/remount 都能衔接。
const mutating: { chain: Promise<void> } = { chain: Promise.resolve() };

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

/** ErrorBoundary：主窗口任何子组件抛错时不再 unmount 变白，
 *  捕到错误显示堆栈 + 「重试」按钮重置 state。class component 必需（hooks
 *  写法目前 React 还没稳定 API）。 */
class ErrorBoundary extends Component<
  { children: ReactNode },
  { error: Error | null }
> {
  state = { error: null as Error | null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("[App ErrorBoundary]", error, info.componentStack);
  }
  reset = () => this.setState({ error: null });
  render() {
    if (this.state.error) {
      return (
        <div className="min-h-screen bg-[var(--bg)] p-6 flex items-center justify-center">
          <div className="nm-card p-6 max-w-2xl">
            <p className="text-base font-semibold text-[var(--danger)] mb-2">
              ⚠️ 主窗口发生错误
            </p>
            <p className="text-sm text-[var(--t2)] mb-2">
              {this.state.error.message}
            </p>
            <pre className="text-[10px] text-[var(--t4)] whitespace-pre-wrap overflow-auto max-h-64 bg-[var(--inset)] p-3 rounded-xl mb-3">
              {this.state.error.stack}
            </pre>
            <button
              className="nm-btn px-4 py-1.5 text-sm text-[var(--t2)]"
              onClick={this.reset}
            >
              🔄 重试
            </button>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
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

function App() {
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

  // 字体大小：启动从 bot config 拉值，套到
  // documentElement[data-font-size]；设置页保存后会发 bot-config-changed，
  // 这里重新拉一次同步点选未保存也会被广播（预防设置页直接改 state 预览）。
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
          // 拉取失败也套上默认值，避免界面还没应用就被卡
          document.documentElement.dataset.fontSize = "small";
        }
      };
      await applyFromConfig();
      const u = await listen("bot-config-changed", applyFromConfig);
      unlistenCfg = u;
    })();
    return () => unlistenCfg?.();
  }, []);

  // 系统通知权限（任务卡截止提醒由 Rust 侧 due_notify 经 tauri-plugin-notification 发送）。
  // 平台差异：macOS 必须显式授权（UNUserNotificationCenter），未授权时通知静默失败，
  // 且未签名/开发构建可能不弹横幅（系统限制）；Windows 的 toast 无需此授权流程
  // （requestPermission 直接 granted），但会被「专注助手/勿扰」抑制、数秒后自动收入
  // 通知中心——两平台系统行为差异，代码层面统一走插件 API。
  useEffect(() => {
    (async () => {
      try {
        if (!(await isPermissionGranted())) {
          await requestPermission();
        }
      } catch (e) {
        // 授权失败只留 console 痕迹：通知是增强功能，不打扰主流程
        console.error("[notify] requestPermission failed", e);
      }
    })();
  }, []);
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
          let migrateFailed = false;
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
          } catch (e) {
            // 迁移失败（拍板 #19=A）：保留 legacy 待下次启动重试，并跳过种子落库——
            // 若照旧落 SEED，下次启动库非空不再进迁移分支，legacy 数据永久 orphan
            migrateFailed = true;
            console.error("[init] legacy 迁移失败，保留 localStorage 待下次重试", e);
          }
          if (!migrated && !migrateFailed) {
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

  // OCR C4-6 + C4-r1-验证修复：mutate 串行化（并发保护）。
  // 修法（r1 验证）：mutating 链已抬到模块作用域；此处只保留 mutate 闭包，
  //   闭包每 render 新建但读到的 mutating 引用恒为模块同一对象 → 链跨 render 衔接。
  // 验证限于：机制与顺序（并发场景无确定性回归测试）。
  const mutate = (fn: (prev: Task[]) => Task[]) => {
    const p = mutating.chain.then(async () => {
      const prev = tasksRef.current;
      const next = fn(prev);
      // 打最后修改时间戳（合并导入按此比较同 id 取舍）；
      // 纯排序变更保留原 updatedAt（diffTaskRows 内部判定）
      const { upserts, deletes } = diffTaskRows(prev, next, Date.now());
      // 先落盘再广播：挂件收到 tasks-changed 后立刻 db_load，必须读到已提交的快照
      await upsertTasks(upserts);
      try {
        await deleteTaskRows(deletes);
      } catch (e) {
        // delete 失败不阻断 UI 更新与广播（与 tasks-updated #18=B 同 rationale）：
        // 失败行仍留在库中，下次 db_load 自愈回来
        console.error("[mutate] deleteTaskRows failed", e);
      }
      tasksRef.current = next;
      setTasks(next);
      if (upserts.length || deletes.length) {
        try {
          await emit("tasks-changed");
        } catch (e) {
          console.error("emit tasks-changed failed", e);
        }
      }
    });
    // 推进链；then 空 catch 避免 unhandled rejection 阻塞后续
    mutating.chain = p.then(() => undefined, () => undefined);
    return p;
  };

  // mutate 的 fire-and-forget 入口：失败时 mutate 抛错（见上），这里终止 promise 链——
  // 错误已经由 storage 层 alert 提示用户，call site 只留 console 痕迹，
  // 避免 unhandled rejection 噪音
  const mutateFire = (fn: (prev: Task[]) => Task[]) => {
    void mutate(fn).catch((e) => console.error("[mutate] persist failed", e));
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
          // 写失败不再空 catch 吞掉——storage 层已 alert，这里 console 留痕不重复打扰
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
    // 事件处理串行化——原先 async 监听器内有多个 await 让出点，
    // 连续两个事件（bot 一轮多工具调用）都从同一个旧 tasksRef 出发合并、后完成者
    // 整体覆盖，先处理的事件在主窗口 state 里丢失（DB 不受影响）。Promise 链排队，逐个处理。
    // 单个事件失败 catch 住不阻断后续队列。
    const handle = async (payload: { upserts?: Task[]; deletes?: string[]; source?: string }) => {
        const upserts = payload?.upserts ?? [];
        const deletes = payload?.deletes ?? [];
        if (!upserts.length && !deletes.length) return;
        // source = Bot/Api/Migration：已由后端线程落盘（mutation.rs MutationOrigin 协议，
        // bot/tools.rs + api_handlers/commands.rs 实发），这里只合并 UI 状态，不回写，
        // 否则主窗口的异步回写会用旧事件快照覆盖后端的新写入（归档/软删被回滚）。
        // 非法 source（协议外字符串）：WARN 观测 + 按未落盘处理（宁可多写不丢数据）
        const source = payload?.source;
        const persisted = isBackendPersisted(source);
        if (source !== undefined && !KNOWN_SOURCES.has(source)) {
          console.warn(`[tasks-updated] unknown source: ${String(source)}，按未落盘处理`);
        }
        if (!persisted) {
          // await 落盘完成后再合并/广播，避免挂件 db_load 读到未提交快照
          await upsertTasks(upserts);
          try {
            await deleteTaskRows(deletes);
          } catch (e) {
            // delete 失败不阻断合并广播（拍板 #18=B）：行已从内存 map 移除（UI 消失）
            // 但库中仍在——下次 db_load 会重新出现，后续事件自愈；storage 层已 alert
            console.error("[tasks-updated] deleteTaskRows failed", e);
          }
        }
        // 合并 + 套规则（在 setTasks 之外基于 tasksRef 计算，保证 updater 纯净）
        const map = new Map(tasksRef.current.map((t) => [t.id, t]));
        upserts.forEach((t) => map.set(t.id, t));
        deletes.forEach((id) => map.delete(id));
        const merged = sortByOrder([...map.values()]);
        const next = applyArchiveRule(applyTodayRule(merged));
        // 规则改动（今日归位/超时归档）也要落盘——
        // 否则只改内存，重启/挂件读 db 又回到原始数据，三端长期不一致
        const mergedMap = new Map(merged.map((t) => [t.id, t]));
        const ruleChanged = next.filter((t) => !taskEq(mergedMap.get(t.id)!, t));
        if (ruleChanged.length) {
          const now = Date.now();
          ruleChanged.forEach((t) => {
            // RMW 基线 = 规则改动前的合并快照 updatedAt
            t.expectedUpdatedAt = mergedMap.get(t.id)?.updatedAt;
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
    };
    const unlisten = listen<{ upserts?: Task[]; deletes?: string[]; source?: string }>(
      "tasks-updated",
      (e) => {
        // SYNC-1：并入全局 mutating.chain（与 UI mutate/每分钟规则定时器同一条串行链）。
        // 原先独立局部 queue 与 mutate 链互不感知：handle 的 await 让出窗口内，
        // UI 写从另一条链读到旧 tasksRef，merge 落盘后该写基线过期 →
        // db_upsert RMW 守卫拒写 →「写冲突」弹窗（实测：主窗完成列取消✅）。
        // 合并后「读快照→落盘→更新 tasksRef」全窗口严格串行。单事件失败
        // catch 住不阻断后续（落盘冲突靠下次事件/db_load 自愈）。
        mutating.chain = mutating.chain
          .then(() => handle(e.payload ?? {}))
          .catch((err) => {
            console.error("[tasks-updated] handler failed", err);
          });
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
      () => mutateFire((prev) => applyArchiveRule(applyTodayRule(prev))),
      60_000
    );
    return () => clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const addTask = () => {
    const id = crypto.randomUUID();
    mutateFire((prev) => {
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

  // 导出工作区链接：全量 WorkspaceItem 写 JSON 文件（与任务数据管理风格一致；workspace 数据独立存于 workspace_items 表）
  const exportWorkspace = async () => {
    try {
      const path = await save({
        defaultPath: `wmessage-workspace-${localDateStr()}.json`,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (!path) return; // 用户取消
      const count = await exportWorkspaceToFile(path);
      alert(`导出完成：共 ${count} 个工作区条目`);
    } catch (e) {
      handleCommandError(e, "workspace_export", {
        onRetry: () => void exportWorkspace(),
      });
    }
  };

  // 导入工作区链接：JSON 文件按 id 合并，同 id 保留更晚修改；导入后重读全量并广播挂件
  const importWorkspace = async () => {
    try {
      const selected = await open({
        multiple: false,
        directory: false,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (typeof selected !== "string") return; // 用户取消
      const merged = await importWorkspaceFromFile(selected);
      // App.tsx 不持 workspace 顶层 state（子组件 WorkspaceView 自管），广播事件让挂件+子视图刷新
      emit("workspace-changed").catch(() => {});
      alert(`导入完成：本次写入 ${merged} 个工作区条目`);
    } catch (e) {
      handleCommandError(e, "workspace_import", {
        onRetry: () => void importWorkspace(),
      });
    }
  };

  // 看板拖拽排序提交：数组顺序已由 KanbanBoard 排好（含跨列变更），
  // 这里补列变更完成语义（进完成列记时间、出完成列清除），再给被拖任务分配 order
  const commitBoardOrder = (activeId: string, next: Task[]) => {
    mutateFire((prev) => {
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

  // TP-1：✅ 状态切换走服务端定向命令——服务端同一把 DB 写锁内读现值打基线写库，
  // 前端快照不参与，规则定时器/迁移/实例重启并发不再触发写冲突弹窗
  const setTaskColumn = async (taskId: string, col: ColumnId) => {
    try {
      const row = await invoke<Task>("task_set_column", { id: taskId, col });
      applyRemoteRows([row]);
    } catch (e) {
      handleCommandError(e, "切换任务状态");
    }
  };

  // 远端行合并进本地快照（与 tasks-updated 合并同规则；本地不回写）
  const applyRemoteRows = (rows: Task[]) => {
    const map = new Map(tasksRef.current.map((t) => [t.id, t]));
    rows.forEach((t) => map.set(t.id, t));
    const next = applyArchiveRule(applyTodayRule(sortByOrder([...map.values()])));
    tasksRef.current = next;
    setTasks(next);
  };

  // TP-2：单任务定向补丁——undefined 归一为 null（JSON 序列化丢 undefined 键，
  // 不归一则「清空」语义静默丢失）；服务端同锁内读现值打基线，前端快照不参与
  const patchTask = async (taskId: string, patch: Partial<Task>) => {
    const normalized = Object.fromEntries(
      Object.entries(patch).map(([k, v]) => [k, v === undefined ? null : v])
    );
    try {
      const row = await invoke<Task>("task_patch", { id: taskId, patch: normalized });
      if (row) applyRemoteRows([row]);
    } catch (e) {
      handleCommandError(e, "更新任务");
    }
  };

  const updateTask = (taskId: string, patch: Partial<Task>) => {
    // due 设为今天且当前在待办 → 同一补丁内立即套用今日规则
    const p: Partial<Task> = { ...patch };
    if ("due" in patch && patch.due && isDueToday(patch.due)) {
      const cur = tasksRef.current.find((t) => t.id === taskId);
      if (cur && cur.column === "todo") p.column = "doing";
    }
    void patchTask(taskId, p);
    setEditingId(null);
  };

  // 软删除：进回收站
  // 软删必须清调度字段——否则任务躺在回收站里 schedule 仍到点触发
  const deleteTask = (taskId: string) => {
    void patchTask(taskId, {
      deletedAt: Date.now(),
      schedule: null,
      schedLast: null,
    });
    setEditingId((cur) => (cur === taskId ? null : cur));
  };

  // 彻底删除（回收站，逐卡删除）
  const hardDeleteTask = (taskId: string) => {
    mutateFire((prev) => prev.filter((t) => t.id !== taskId));
  };

  return (
    <ErrorBoundary>
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
          onSetColumn={setTaskColumn}
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
        />
      ) : (
        <SettingsPage
          theme={theme}
          onThemeChange={setTheme}
          onExportTasks={exportTasks}
          onImportTasks={importTasks}
          onExportWorkspace={exportWorkspace}
          onImportWorkspace={importWorkspace}
        />
      )}
      {/* 全局确认弹窗（老板 14:45 拍板：confirm 走主窗口，不走 widget 挂件） */}
      <ConfirmMap />
    </div>
    </ErrorBoundary>
  );
}

export default App;
