import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import {
  isPermissionGranted,
  requestPermission,
} from "@tauri-apps/plugin-notification";
import {
  Activity as ActivityIcon,
  AlarmClock,
  Archive,
  Bell,
  FolderOpen,
  LayoutDashboard,
  MessageSquare,
  Package,
  Plus,
  Search,
  Settings as SettingsIcon,
  SquareKanban,
  Trash2,
  Waypoints,
} from "lucide-react";
import { TaskTimelinePage } from "./components/TaskTimeline/TaskTimelinePage";
import {
  TaskEditorOverlay,
  EDITOR_EXIT_MS,
} from "./components/TaskTimeline/TaskEditorOverlay";
import { sortPoolTasks } from "./components/TaskTimeline/week";
import { HomePage } from "./components/HomePage/HomePage";
import { CommandPalette } from "./components/CommandPalette";
import { ErrorBoundary } from "./ui/ErrorBoundary";
import { ErrorDialogHost } from "./ui/ErrorDialogHost";
import mainLogo from "./assets/main-logo.png";
import { ArchivePage } from "./components/ArchivePage";
import { AppsPage } from "./components/AppsPage/AppsPage";
import { TrashPage } from "./components/TrashPage";
import { WorkspacePage } from "./components/WorkspacePage";
import { SettingsPage } from "./components/SettingsPage";
import { SchedulePage } from "./components/SchedulePage/SchedulePage";
import { ActivityPage } from "./components/ActivityPage";
import GraphPage from "./components/GraphPage/GraphPage";
import { NotificationsPage } from "./components/NotificationsPage/NotificationsPage";
import { migrateGraphLooseness } from "./lib/graphPrefs";
import {
  NOTIFICATIONS_CHANGED_EVENT,
  pendingNotificationCount,
} from "./lib/notifications";
import ConfirmMap from "./components/ConfirmMap";
import { ChatPanel } from "./components/ChatPanel";
import {
  deleteTaskRows,
  diffTaskRows,
  loadTasksFromDb,
  taskEq,
  upsertTasks,
  exportTasksToFile,
  importTasksFromFile,
  exportWorkspaceToFile,
  importWorkspaceFromFile,
  sortByOrder,
} from "./storage";
import { handleCommandError } from "./lib/errorHandler";
import { NAV_W } from "./lib/navWidth";
import { applyFontSizeSync } from "./lib/botConfig";
import { dayKey } from "./components/HomePage/homeData";
import { useConfirm } from "./lib/useConfirm";
import { unlistenSafe } from "./lib/useTauriListen";
import { isBackendPersisted, KNOWN_SOURCES } from "./lib/mutationOrigin";
import { revertPatch } from "./lib/taskRevert";
import {
  applyArchiveRule,
  loadArchiveDaysFromConfig,
  setArchiveAfterDays,
} from "./lib/archiveRule";

import {
  applySetting,
  getSetting,
  subscribeSystem,
  toggleTheme,
} from "./theme";
import {
  applyBubbleStyle,
  getBubbleStyle,
} from "./bubbleStyle";
import { isDueToday } from "./format";
import type { ThemeSetting } from "./theme";
import type { Task, ColumnId } from "./types";

// 修：mutate 串行化队列必须在**模块作用域**（App() 内会被每
//   次 render 重建 → 跨 render 的并发 mutate 落到不同链 → 串行化失效）。
//   抬到此处后全 App 实例共享同一链，跨 render 与 unmount/remount 都能衔接。
const mutating: { chain: Promise<void> } = { chain: Promise.resolve() };

// 左侧导航栏条目：视图切换职能自顶部工具条迁入；设置单独走底部入口。
// 分区结构：「任务卡」组（任务/图谱/归档/工作区/回收站）+ 「Agent能力」组（聊天/定时/通知）
type RailView =
  | "home"
  | "board"
  | "schedule"
  | "chat"
  | "activity"
  | "graph"
  | "archive"
  | "workspace"
  | "trash"
  | "apps"
  | "notifications";
const NAV_ITEMS: { key: RailView; label: string; icon: typeof SquareKanban }[] =
  [
    { key: "home", label: "首页", icon: LayoutDashboard },
    { key: "board", label: "任务", icon: SquareKanban },
    { key: "graph", label: "图谱", icon: Waypoints },
    { key: "archive", label: "归档", icon: Archive },
    { key: "workspace", label: "工作区", icon: FolderOpen },
    { key: "trash", label: "回收站", icon: Trash2 },
  ];
const AGENT_ITEMS: {
  key: RailView;
  label: string;
  icon: typeof SquareKanban;
  badge?: boolean;
}[] = [
  // 机器人聊天（挂件退役后收进主窗口）
  { key: "chat", label: "聊天", icon: MessageSquare },
  // 定时任务模块：任务卡到点自动执行的集中管理（列表=状态面板）
  { key: "schedule", label: "定时", icon: AlarmClock },
  // 程序模块（M1 起新增，apps 工具箱；详见 docs/APPS-DESIGN-2026-10-15.md）
  { key: "apps", label: "程序", icon: Package },
  //  执行活动聚合页：最近全部执行痕迹列表（Agent 透明化设计 §5.6）
  { key: "activity", label: "活动", icon: ActivityIcon },
  // 通知中心（Agent 通知模块）：badge=true 渲染待处理数角标
  { key: "notifications", label: "通知", icon: Bell, badge: true },
];

/** 快捷键提示文案：mac ⌘ / 其他平台 Ctrl（纯前端 keydown，两平台同实现）。
 *  userAgent 而非已废弃的 navigator.platform（后者在现代浏览器可能返回空串） */
const IS_MAC =
  typeof navigator !== "undefined" &&
  /Mac|iP(hone|ad|od)/.test(navigator.userAgent);
const KBD_NEW = IS_MAC ? "⌘N" : "Ctrl+N";
const KBD_SEARCH = IS_MAC ? "⌘K" : "Ctrl+K";

/** 导航栏按钮：action=带边框动作钮（新建/搜索，可挂快捷键提示）；
 *  item=导航项（hover 淡底，选中走 nm-inset 凹陷语义） */
function RailButton({
  icon: Icon,
  label,
  kbd,
  active = false,
  onClick,
  variant = "item",
  badge = 0,
}: {
  icon: typeof SquareKanban;
  label: string;
  kbd?: string;
  active?: boolean;
  onClick: () => void;
  variant?: "action" | "item";
  /** 待处理数角标（0 不显示；通知中心入口用） */
  badge?: number;
}) {
  const base =
    "flex h-8 w-full items-center gap-2 px-2.5 text-sm transition-colors duration-[var(--dur-chip)] ease-[var(--ease-out)]";
  let skin: string;
  if (variant === "action") {
    skin = "nm-btn text-[var(--t3)]";
  } else if (active) {
    skin = "nm-inset rounded-[var(--r-sm)] text-[var(--t1)]";
  } else {
    skin = "rounded-[var(--r-sm)] text-[var(--t3)] hover:bg-[var(--hover-bg)]";
  }
  return (
    <button
      aria-current={active || undefined}
      onClick={onClick}
      className={`${base} ${skin}`}
    >
      <Icon size={15} aria-hidden />
      {/* truncate（含 nowrap）：系统字号放大时标题宁可省略也不换行——换行会把
          整条导航撑成两行高度，⌘K 提示条与角标也被挤到下一行。min-w-0 是前提，
          没有它 flex 项的 auto 最小尺寸会顶着内容宽度不肯收缩 */}
      <span className="min-w-0 flex-1 truncate text-left">{label}</span>
      {badge > 0 && (
        <span
          aria-label={`${badge} 条待处理`}
          className="shrink-0 rounded-full bg-[var(--t1)] px-1.5 py-0.5 text-[10px] font-medium leading-none text-[var(--bg)]"
        >
          {badge > 99 ? "99+" : badge}
        </span>
      )}
      {kbd && (
        <kbd className="shrink-0 rounded border border-[var(--edge)] px-1 py-0.5 text-[10px] font-medium text-[var(--t5)]">
          {kbd}
        </kbd>
      )}
    </button>
  );
}

/** 导航栏分区标题：小标题在上、横线在下，分隔栏目组（任务卡 / Agent能力） */
function RailSection({ label }: { label: string }) {
  return (
    <div className="mt-3 mb-1 px-2.5">
      <span className="block truncate text-[11px] font-medium text-[var(--t5)]">
        {label}
      </span>
      <div role="separator" className="mt-1 border-t border-[var(--edge)]" />
    </div>
  );
}

// 今日规则：截止日期为当天的任务自动进「今日」列（「完成」列不受影响）
function applyTodayRule(tasks: Task[]): Task[] {
  return tasks.map((t) =>
    isDueToday(t.due) && t.column === "todo" && !t.deletedAt
      ? { ...t, column: "doing" }
      : t,
  );
}

// 归档规则（applyArchiveRule + 归档天数配置）在 lib/archiveRule.ts：
// 天数存 bot-config.json，后端 migration 兜底归档同源读取（设置页数据管理可改）

/** 任务编辑浮窗的打开态：id + 焦点模式 + 退场标记（单原子，防退场定时器
 *  与「切换任务重开」竞态——dismiss 只清 closing 态）+ 是否新建卡。
 *  null = 关闭。 */
type EditorState = {
  id: string;
  autoFocus: boolean;
  closing: boolean;
  /** 新建卡：不保存关闭 = 取消新建（这张卡不该留下），与编辑已有卡的回滚不同路 */
  isNew: boolean;
};

function App() {
  const [tasks, setTasks] = useState<Task[]>([]);
  const tasksRef = useRef<Task[]>([]);
  const [editor, setEditor] = useState<EditorState | null>(null);
  // 打开浮窗那一刻的任务行快照：不保存关闭时按它回滚本次改动
  const editorSnapRef = useRef<{ id: string; task: Task } | null>(null);
  const [view, setView] = useState<RailView | "settings">("home");
  // 归档定位目标（时间泳道双击归档卡 → 归档区滚动定位 + 紫边框）。
  // 切出归档视图即清（定位态只活在当下这次跳转）。
  const [archiveFocus, setArchiveFocus] = useState<string | null>(null);
  useEffect(() => {
    if (view !== "archive") setArchiveFocus(null);
  }, [view]);

  /** 打开浮窗统一入口：留快照（不保存关闭时按它回滚）+ 标记是否新建卡 */
  const openEditor = (id: string, autoFocus: boolean, isNew = false) => {
    const t = tasksRef.current.find((x) => x.id === id);
    editorSnapRef.current = t ? { id, task: { ...t } } : null;
    setEditor({ id, autoFocus, closing: false, isNew });
  };
  // 事件监听器只跑一次，闭包会拿到旧的 openEditor → ref 兜住最新一份
  const openEditorRef = useRef(openEditor);
  openEditorRef.current = openEditor;

  // main 滚动容器 ref：首页吸顶头部的滚动进度源（useScroll container）
  const mainRef = useRef<HTMLElement>(null);
  // Agent 通知中心待处理数（导航角标；notifications-changed 广播驱动刷新）
  const [notifCount, setNotifCount] = useState(0);
  // 机器人总开关（设置页 bot_set_enabled 后广播 bot-changed）。
  // ChatPanel 未开启时跳过全部对外 invoke，聊天页不给用户一个必然报错的界面。
  const [botOn, setBotOn] = useState(false);
  useEffect(() => {
    let cancelled = false;
    invoke<boolean>("bot_get_enabled")
      .then((v) => {
        if (!cancelled) setBotOn(v);
      })
      .catch((e) => console.error("bot_get_enabled 失败:", e));
    const un = listen<boolean>("bot-changed", (e) => setBotOn(e.payload));
    return () => {
      cancelled = true;
      unlistenSafe(un);
    };
  }, []);
  // 切到聊天页：托盘「打开聊天」、任务 🤖 按钮、命令行板 ⌘K 都走这条。
  // chat-focus-session 带 sessionId 时由 ChatPanel 自己切会话，这里只管视图。
  const openChat = useCallback(() => setView("chat"), []);
  useEffect(() => {
    const un = listen("chat-focus-session", () => setView("chat"));
    return () => {
      unlistenSafe(un);
    };
  }, []);
  useEffect(() => {
    const refresh = () =>
      pendingNotificationCount()
        .then(setNotifCount)
        .catch((e) => console.error("[notifications] 角标刷新失败:", e));
    void refresh();
    const un = listen(NOTIFICATIONS_CHANGED_EVENT, refresh);
    return () => {
      unlistenSafe(un);
    };
  }, []);
  const [theme, setTheme] = useState<ThemeSetting>(getSetting);
  const [paletteOpen, setPaletteOpen] = useState(false);
  // 可见任务选择器。
  // tasks 已套今日/归档规则，这里只做归属过滤。
  // 任务图谱（设计 §1.5）：ownerId 非空 = 导入的外来任务，默认同样不混入（只看自己的）；
  // 图谱页拿全量 tasks 自行过滤
  const visibleTasks = useMemo(() => tasks.filter((t) => !t.ownerId), [tasks]);
  // 聊天右侧栏「任务池」：未完成的自己的任务（已排期/未排期都列，排序直接复用
  // 任务页任务池的 sortPoolTasks，避免同一批任务在两处顺序不同）
  const openTasks = useMemo(
    () => sortPoolTasks(visibleTasks.filter((t) => t.column !== "done" && !t.archived)),
    [visibleTasks],
  );

  // 归档自动打标（任务图谱设计 §2）：观测「已加载任务从未归档 → 归档且无标签」
  // 转变，fire-and-forget 调后端一次性 LLM 打标；失败仅留 console 痕迹。
  // 首屏（prev 为 null）不触发——防启动时对历史归档卡批量调用；「prev 有该行」
  // 同时挡住导入场景（导入的归档卡带着别人的标签体系进来，不代打）
  const prevTasksForTagRef = useRef<Task[] | null>(null);
  useEffect(() => {
    const prev = prevTasksForTagRef.current;
    prevTasksForTagRef.current = tasks;
    if (!prev || prev.length === 0) return;
    const prevMap = new Map(prev.map((t) => [t.id, t]));
    for (const t of tasks) {
      const p = prevMap.get(t.id);
      if (!p) continue;
      if (!t.archived || p.archived) continue;
      if (t.column !== "done" || t.deletedAt || t.ownerId) continue;
      if (t.tags && t.tags.length > 0) continue;
      invoke("task_autotag", { id: t.id }).catch((e) =>
        console.error("[task_autotag] failed", t.id, e),
      );
    }
  }, [tasks]);

  // 主题：启动时应用 + 跟随系统模式监听系统外观变化
  useEffect(() => {
    applySetting(theme);
  }, [theme]);
  useEffect(() => subscribeSystem(setTheme), []);

  // 聊天气泡外观：启动应用（单窗口，无跨页同步面；设置页改动即时落 DOM）
  useEffect(() => {
    applyBubbleStyle(getBubbleStyle());
  }, []);

  // 图谱偏好搬迁：旧档位键写进新键（写成了才删旧键）。
  // 放在启动时做，而不是塞进读函数——偏好是在渲染期读的，
  // 读函数里写 localStorage 是渲染副作用
  useEffect(() => {
    migrateGraphLooseness();
  }, []);

  // 字体大小：启动从 bot config 拉值套用 + 监听广播重拉（共享体见 lib/botConfig）；
  // 主窗口额外同步归档天数——阈值变小 → 立即重套规则把已到期任务归档落盘，
  // 不等下一分钟定时器。规则幂等 + mutate 链串行，启动期并发多套一次无害
  // （任务未加载时为空 no-op）。
  useEffect(
    () =>
      applyFontSizeSync((c) => {
        setArchiveAfterDays(c.archiveAfterDays);
        mutateFire((prev) => applyArchiveRule(applyTodayRule(prev)));
      }),
    [],
  );

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
      unlistenSafe(unlisten);
    };
  }, []);

  // 初始加载：SQLite（旧 data.json 的迁移在 Rust 侧）
  useEffect(() => {
    (async () => {
      try {
        // 归档天数先于首套规则到位：首屏就按配置阈值归档，而不是先按默认 7 打错标
        await loadArchiveDaysFromConfig();
        const res = await loadTasksFromDb();
        if (!res.ok) {
          // 读失败 ≠ 空库：保持内存空数组并明确告警，不往任何库里写
          // recoverable=true 时 handleCommandError 会弹 confirm 提供「重试」（重载重新加载）
          handleCommandError(res.error, "db_load", {
            onRetry: () => window.location.reload(),
          });
          return;
        }
        const list = res.tasks;
        // 空库就是空库：不注入任何数据。空库也可能是**读到了错误的库**（数据目录
        // 解析漂移 / 便携目录劫持），往里写的东西用户无从分辨「我的任务卡呢」
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
        // 初始化失败不静默：setTasks 没跑到 → 界面停在空列表，用户会误判成
        // 「任务卡全没了」而去做不可逆操作（导入/覆盖）。报错 + 重载重试。
        handleCommandError(e, "init load", {
          onRetry: () => window.location.reload(),
        });
      }
    })();
  }, []);

  // 验证修复：mutate 串行化（并发保护）。
  // 修法：mutating 链已抬到模块作用域；此处只保留 mutate 闭包，
  //   闭包每 render 新建但读到的 mutating 引用恒为模块同一对象 → 链跨 render 衔接。
  // 验证限于：机制与顺序（并发场景无确定性回归测试）。
  const mutate = (fn: (prev: Task[]) => Task[]) => {
    const p = mutating.chain.then(async () => {
      const prev = tasksRef.current;
      const next = fn(prev);
      // 打最后修改时间戳（合并导入按此比较同 id 取舍）；
      // 纯排序变更保留原 updatedAt（diffTaskRows 内部判定）
      const { upserts, deletes } = diffTaskRows(prev, next, Date.now());
      // 先落盘再广播：收到 tasks-changed 的读取方必须读到已提交的快照
      await upsertTasks(upserts);
      try {
        await deleteTaskRows(deletes);
      } catch (e) {
        // delete 失败不阻断 UI 更新与广播（与 tasks-updated #18=B 同 rationale）：
        // 失败行仍留在库中，下次 db_load 自愈回来
        console.error("[mutate] deleteTaskRows failed", e);
      }
      // 快照必须存「已落库」的行：diffTaskRows 给变更行打了新 updatedAt（= DB 真值），
      // next 里还是旧值——不换的话同一行第二次整行写的基线必落后于库，
      // RMW 守卫拒写（生成子任务卡 → × 实测复现的写冲突根因）
      const stamped = new Map(upserts.map((u) => [u.id, u]));
      const persisted = next.map((t) => stamped.get(t.id) ?? t);
      tasksRef.current = persisted;
      setTasks(persisted);
      if (upserts.length || deletes.length) {
        try {
          await emit("tasks-changed");
        } catch (e) {
          console.error("emit tasks-changed failed", e);
        }
      }
    });
    // 推进链；then 空 catch 避免 unhandled rejection 阻塞后续
    mutating.chain = p.then(
      () => undefined,
      () => undefined,
    );
    return p;
  };

  // mutate 的 fire-and-forget 入口：失败时 mutate 抛错（见上），这里终止 promise 链——
  // 错误已经由 storage 层 alert 提示用户，call site 只留 console 痕迹，
  // 避免 unhandled rejection 噪音
  const mutateFire = (fn: (prev: Task[]) => Task[]) => {
    void mutate(fn).catch((e) => console.error("[mutate] persist failed", e));
  };

  // 后端（bot / api / migration）行级变更（tasks-updated：{upserts, deletes}），
  // 主窗口统一落盘后广播（单写者架构）
  useEffect(() => {
    // 事件处理串行化——原先 async 监听器内有多个 await 让出点，
    // 连续两个事件（bot 一轮多工具调用）都从同一个旧 tasksRef 出发合并、后完成者
    // 整体覆盖，先处理的事件在主窗口 state 里丢失（DB 不受影响）。Promise 链排队，逐个处理。
    // 单个事件失败 catch 住不阻断后续队列。
    const handle = async (payload: {
      upserts?: Task[];
      deletes?: string[];
      source?: string;
    }) => {
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
        console.warn(
          `[tasks-updated] unknown source: ${String(source)}，按未落盘处理`,
        );
      }
      if (!persisted) {
        // await 落盘完成后再合并/广播，避免其他读取方读到未提交快照
        await upsertTasks(upserts);
        try {
          await deleteTaskRows(deletes);
        } catch (e) {
          // delete 失败不阻断合并广播：行已从内存 map 移除（UI 消失）
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
      // 否则只改内存，重启后读 db 又回到原始数据，三端长期不一致
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
        console.info(
          `[tasks-updated] merge_tasks | ${ruleChanged.length} changed`,
        );
      }
      tasksRef.current = next;
      setTasks(next);
      try {
        await emit("tasks-changed");
      } catch (err) {
        console.error("emit tasks-changed failed", err);
      }
    };
    const unlisten = listen<{
      upserts?: Task[];
      deletes?: string[];
      source?: string;
    }>("tasks-updated", (e) => {
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
    });
    return () => {
      unlistenSafe(unlisten);
    };
  }, []);

  // 聊天区点任务引用 → 打开该任务的全局编辑浮窗（覆盖当前页滑出，不切视图）
  useEffect(() => {
    const unlisten = listen<{ id?: string }>("edit-task", (e) => {
      const id = e.payload?.id;
      if (id) openEditorRef.current(id, false);
    });
    return () => {
      unlistenSafe(unlisten);
    };
  }, []);

  // 全局快捷键快速新建（Rust 侧 Cmd+Ctrl+N / Ctrl+Alt+N）：当前页上层滑出新建浮窗
  useEffect(() => {
    const unlisten = listen("quick-add", () => {
      startNewTaskRef.current();
    });
    return () => {
      unlistenSafe(unlisten);
    };
  }, []);

  // 每分钟重套今日规则 + 归档规则：覆盖跨零点归位、完成超时归档（diff 后行级落盘）
  useEffect(() => {
    const id = setInterval(
      () => mutateFire((prev) => applyArchiveRule(applyTodayRule(prev))),
      60_000,
    );
    return () => clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const addTask = () => {
    const id = crypto.randomUUID();
    // 落盘完成后才开浮窗：浮窗按 tasks 快照解析任务，先开窗会撞「任务未入列」空窗
    void mutate((prev) => {
      const max = prev.reduce((m, t) => Math.max(m, t.order ?? 0), 0);
      return [...prev, { id, title: "新任务", column: "todo", order: max + 1 }];
    }).then(
      () => openEditorRef.current(id, true, true),
      (e) => console.error("[mutate] persist failed", e),
    );
  };

  // 快捷新建（⌘/Ctrl+N 与 quick-add 事件共用）：关面板 + 当前页上层滑出新建浮窗
  const startNewTaskRef = useRef<() => void>(() => {});
  useEffect(() => {
    startNewTaskRef.current = () => {
      setPaletteOpen(false);
      addTaskRef.current();
    };
  });

  // 全局快捷键纯前端 keydown 实现，不碰 Rust 全局注册）：
  // ⌘/Ctrl+N 新建任务、⌘/Ctrl+K 命令面板开合。
  // isComposing 跳过：中文输入法组合期不得触发（防选词按到修饰键误新建）
  const addTaskRef = useRef(addTask);
  addTaskRef.current = addTask;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.isComposing) return;
      if (!(e.metaKey || e.ctrlKey) || e.altKey || e.shiftKey) return;
      const k = e.key.toLowerCase();
      if (k === "n") {
        e.preventDefault();
        startNewTaskRef.current();
      } else if (k === "k") {
        e.preventDefault();
        setPaletteOpen((v) => !v);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // 子任务行 ×：删行；若该行已生成子任务卡，两边解除关联（parentId 清空）但
  // 保留子卡（退化为普通卡）。同一 mutate 原子提交，删行/解关联不会半截落库
  const deleteSubtaskRow = (motherId: string, subId: string) => {
    mutateFire((prev) => {
      const childId = prev
        .find((t) => t.id === motherId)
        ?.subtasks?.find((s) => s.id === subId)?.taskId;
      return prev.map((t) => {
        if (t.id === motherId) {
          return {
            ...t,
            subtasks: (t.subtasks ?? []).filter((s) => s.id !== subId),
          };
        }
        if (childId && t.id === childId) {
          return { ...t, parentId: null };
        }
        return t;
      });
    });
  };

  // ⌘K 跳转 / 图谱与首页点卡：当前页上层滑出该任务的编辑浮窗（不切视图）；
  // 找不到（已被彻底删除）静默不动
  const jumpToTask = (t: Task) => {
    openEditor(t.id, false);
  };
  // 子任务行 ↗ 跳转：按 id 定位
  const jumpToTaskId = (id: string) => {
    const t = tasksRef.current.find((x) => x.id === id);
    if (t) jumpToTask(t);
  };
  // 时间泳道双击已归档卡：切到归档区并定位该卡（ArchivePage 滚动 + 紫边框）
  const jumpToArchive = (id: string) => {
    setArchiveFocus(id);
    setView("archive");
  };

  // 子任务行一键生成子任务卡：新卡进待办（todo）带 parentId 指回母卡；
  // 同一 mutate 内原子回写子任务行的 taskId（行上 ↗ 标记），写失败整批不落
  const createSubtaskCard = (motherId: string, subId: string, title: string) => {
    const childId = crypto.randomUUID();
    mutateFire((prev) => {
      const max = prev.reduce((m, t) => Math.max(m, t.order ?? 0), 0);
      const child: Task = {
        id: childId,
        title,
        column: "todo",
        order: max + 1,
        parentId: motherId,
      };
      return [
        ...prev.map((t) =>
          t.id === motherId
            ? {
                ...t,
                subtasks: (t.subtasks ?? []).map((s) =>
                  s.id === subId ? { ...s, taskId: childId } : s,
                ),
              }
            : t,
        ),
        child,
      ];
    });
  };
  const jumpToSession = (sessionId: string) => {
    setView("chat");
    emit("chat-focus-session", { sessionId }).catch((e) => {
      console.error("emit chat-focus-session failed", e);
    });
  };

  // 导入导出四胞胎（任务/工作区 × 导出/导入）同形：save/open JSON 对话框 →
  // 传输（返回条数）→ 成功提示 → 失败可重试。差异全部参数化；导入侧的
  // 落库/刷新动作并进 transfer（任务=重读全量套规则广播；工作区=广播
  // workspace-changed 让子视图自刷）。
  const jsonTransfer = (opts: {
    kind: "export" | "import";
    /** 默认文件名词干（导出对话框，自动拼本地日期后缀） */
    stem: string;
    /** 路径 → 条数 */
    transfer: (path: string) => Promise<number>;
    done: (count: number) => void;
    /** handleCommandError 命令名（错误弹窗标签 + 重试） */
    label: string;
  }) => {
    const run = async () => {
      try {
        if (opts.kind === "export") {
          const path = await save({
            defaultPath: `${opts.stem}-${dayKey(new Date())}.json`,
            filters: [{ name: "JSON", extensions: ["json"] }],
          });
          if (!path) return; // 用户取消
          opts.done(await opts.transfer(path));
        } else {
          const selected = await open({
            multiple: false,
            directory: false,
            filters: [{ name: "JSON", extensions: ["json"] }],
          });
          if (typeof selected !== "string") return; // 用户取消
          opts.done(await opts.transfer(selected));
        }
      } catch (e) {
        handleCommandError(e, opts.label, { onRetry: () => void run() });
      }
    };
    return run();
  };

  // 导出任务数据：全量任务卡（含归档、回收站）写 JSON 文件
  const exportTasks = () =>
    jsonTransfer({
      kind: "export",
      stem: "wmessage-tasks",
      transfer: exportTasksToFile,
      done: (count) => alert(`导出完成：共 ${count} 条任务卡`),
      label: "tasks_export",
    });

  // 导入任务数据：JSON 文件按 id 合并，同 id 保留更晚修改；导入后重读全量 + 套规则 + 广播
  const importTasks = () =>
    jsonTransfer({
      kind: "import",
      stem: "wmessage-tasks",
      transfer: async (path) => {
        const merged = await importTasksFromFile(path);
        await reloadTasks();
        return merged;
      },
      done: (count) => alert(`导入完成：本次写入 ${count} 条任务卡`),
      label: "tasks_import",
    });

  // 重读全量任务：套规则 → 排序 → 更新状态 → 广播
  const reloadTasks = async () => {
    const res = await loadTasksFromDb();
    if (!res.ok) throw res.error;
    const next = applyArchiveRule(applyTodayRule(sortByOrder(res.tasks)));
    tasksRef.current = next;
    setTasks(next);
    emit("tasks-changed").catch(() => {});
  };

  // 导出工作区链接：全量 WorkspaceItem 写 JSON 文件（与任务数据管理风格一致；workspace 数据独立存于 workspace_items 表）
  const exportWorkspace = () =>
    jsonTransfer({
      kind: "export",
      stem: "wmessage-workspace",
      transfer: exportWorkspaceToFile,
      done: (count) => alert(`导出完成：共 ${count} 个工作区条目`),
      label: "workspace_export",
    });

  // 导入工作区链接：JSON 文件按 id 合并，同 id 保留更晚修改；导入后广播（App.tsx 不持 workspace 顶层 state，子组件 WorkspaceView 自管）
  const importWorkspace = () =>
    jsonTransfer({
      kind: "import",
      stem: "wmessage-workspace",
      transfer: async (path) => {
        const merged = await importWorkspaceFromFile(path);
        emit("workspace-changed").catch(() => {});
        return merged;
      },
      done: (count) => alert(`导入完成：本次写入 ${count} 个工作区条目`),
      label: "workspace_import",
    });

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
    const next = applyArchiveRule(
      applyTodayRule(sortByOrder([...map.values()])),
    );
    tasksRef.current = next;
    setTasks(next);
  };

  // TP-2：单任务定向补丁——undefined 归一为 null（JSON 序列化丢 undefined 键，
  // 不归一则「清空」语义静默丢失）；服务端同锁内读现值打基线，前端快照不参与
  const patchTask = async (taskId: string, patch: Partial<Task>) => {
    const normalized = Object.fromEntries(
      Object.entries(patch).map(([k, v]) => [k, v === undefined ? null : v]),
    );
    try {
      const row = await invoke<Task>("task_patch", {
        id: taskId,
        patch: normalized,
      });
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
  };

  // 软删除：进回收站
  // 软删只写 deletedAt：排期（planStart/planEnd）是周网格里的「位置」，不是到点触发器
  // ——旧的 tasks.schedule / sched_last 列早已退役，定时作业走独立的 scheduled_jobs 表，
  // 回收站里的卡不会被任何定时器读到。位置留着：软删是可逆的，恢复时原样回到网格。
  // 有子任务卡时先询问：一并软删 or 仅删母卡（子卡保留，parentId 悬挂不清理——
  // 母卡从回收站恢复后关联仍成立）
  const confirm = useConfirm();
  const deleteTask = async (taskId: string) => {
    const children = tasksRef.current.filter(
      (t) => t.parentId === taskId && !t.deletedAt,
    );
    let deleteIds = [taskId];
    if (children.length > 0) {
      const withChildren = await confirm({
        title: "删除任务卡",
        message: `任务卡「${
          tasksRef.current.find((t) => t.id === taskId)?.title ?? ""
        }」有 ${children.length} 张子任务卡，是否一并移入回收站？`,
        okLabel: "一并删除子卡",
        cancelLabel: "仅删母卡",
      });
      if (withChildren) deleteIds = deleteIds.concat(children.map((c) => c.id));
    }
    const now = Date.now();
    deleteIds.forEach((id) => {
      void patchTask(id, {
        deletedAt: now,
      });
    });
    // 删的是正在编辑的任务 → 浮窗跟着关（走退场动画）
    setEditor((cur) =>
      cur && deleteIds.includes(cur.id) ? { ...cur, closing: true } : cur,
    );
  };

  // 彻底删除（回收站，逐卡删除）
  const hardDeleteTask = (taskId: string) => {
    mutateFire((prev) => prev.filter((t) => t.id !== taskId));
  };

  // 编辑中的任务被彻底删除（如回收站硬删）→ 浮窗无内容可显，直接收起；
  // 软删走 deleteTask 的 closing 路径（带退场动画），这里只兜硬删的「任务消失」
  useEffect(() => {
    if (
      editor &&
      !editor.closing &&
      !tasks.some((t) => t.id === editor.id)
    ) {
      setEditor(null);
    }
  }, [editor, tasks]);
  const editorTask = editor
    ? (tasks.find((t) => t.id === editor.id) ?? null)
    : null;
  // 关浮窗 = 进退场动画（结束后 overlay 回调 dismiss 才真正卸载）
  const closeEditor = () =>
    setEditor((cur) => (cur && !cur.closing ? { ...cur, closing: true } : cur));
  // 只清退场态：退场途中若已切换/重开任务（closing=false），本次 dismiss 是陈旧定时器，不吞
  const dismissEditor = () => setEditor((cur) => (cur?.closing ? null : cur));

  /* 「不保存关闭」（浮窗里的拨杆关掉时）两条路：
   *  编辑已有卡 = 回滚到打开浮窗时的状态——按快照与当前行的差异回写，
   *    只有与打开时不同的字段才回写（整行回写会抹掉期间机器人的写入）；
   *  新建卡 = 取消新建——这张卡从没用过，直接抹掉，不进回收站。 */
  const discardEditor = () => {
    const cur = editor;
    if (!cur) return;
    if (cur.isNew) {
      closeEditor();
      // 等退场动画走完再删任务行：行先没 → 浮窗无内容可渲染，会瞬间消失
      setTimeout(() => {
        mutateFire((prev) => prev.filter((t) => t.id !== cur.id));
      }, EDITOR_EXIT_MS);
      return;
    }
    const snap = editorSnapRef.current;
    const now = tasksRef.current.find((t) => t.id === cur.id);
    if (snap && snap.id === cur.id && now) {
      const patch = revertPatch(snap.task, now);
      if (Object.keys(patch).length > 0) void patchTask(cur.id, patch);
    }
    closeEditor();
  };

  // 记录进入设置前的视图（ 设置壳）：「返回」跳回进入前的界面
  const lastViewRef = useRef<RailView>("home");
  useEffect(() => {
    if (view !== "settings") lastViewRef.current = view;
  }, [view]);

  // 设置视图全屏接管：替换左导航+主内容，壳内自带返回与分类导航
  if (view === "settings") {
    return (
      <ErrorBoundary>
        <div className="h-screen overflow-hidden bg-[var(--bg)]">
          <SettingsPage
            theme={theme}
            onThemeChange={setTheme}
            onExportTasks={exportTasks}
            onImportTasks={importTasks}
            onExportWorkspace={exportWorkspace}
            onImportWorkspace={importWorkspace}
            onBack={() => setView(lastViewRef.current)}
          />
          {paletteOpen && (
            <CommandPalette
              onClose={() => setPaletteOpen(false)}
              tasks={visibleTasks}
              onJumpTask={jumpToTask}
              onJumpSession={jumpToSession}
            />
          )}
        </div>
      </ErrorBoundary>
    );
  }

  return (
    <ErrorBoundary>
      <div className="flex h-screen overflow-hidden bg-[var(--bg)]">
        {/* 左侧导航栏：新建(⌘N)/搜索(⌘K)/视图切换/底部设置——职能自顶部工具条迁入。
            宽度 import 自 navWidth（浮窗左缘要越过它，口径必须单源），
            省下的宽度原样转给聊天会话栏 */}
        <nav
          aria-label="主导航"
          style={{ width: NAV_W }}
          className="flex shrink-0 flex-col gap-1 border-r border-[var(--edge)] p-3"
        >
          <div className="mb-2 flex items-center gap-2 px-1 pt-1">
            <img
              src={mainLogo}
              alt="WMessage"
              className="h-6 w-6 shrink-0"
              draggable={false}
            />
            <h1 className="text-sm font-semibold text-[var(--t2)]">WMessage</h1>
          </div>
          <RailButton
            variant="action"
            icon={Plus}
            label="新建任务"
            kbd={KBD_NEW}
            onClick={addTask}
          />
          <RailButton
            variant="action"
            icon={Search}
            label="搜索"
            kbd={KBD_SEARCH}
            onClick={() => setPaletteOpen(true)}
          />
          <RailSection label="任务卡" />
          {NAV_ITEMS.map((item) => (
            <RailButton
              key={item.key}
              icon={item.icon}
              label={item.label}
              active={view === item.key}
              onClick={() => setView(item.key)}
            />
          ))}
          <RailSection label="Agent能力" />
          {AGENT_ITEMS.map((item) => (
            <RailButton
              key={item.key}
              icon={item.icon}
              label={item.label}
              active={view === item.key}
              onClick={() => setView(item.key)}
              badge={item.badge ? notifCount : 0}
            />
          ))}
          <div className="flex-1" />
          <RailButton
            icon={SettingsIcon}
            label="设置"
            onClick={() => setView("settings")}
          />
        </nav>
        <main ref={mainRef} className="app-scroll min-w-0 flex-1 overflow-y-auto p-6">
          {view === "home" ? (
            <HomePage
              tasks={visibleTasks}
              scrollRef={mainRef}
              onNavigate={setView}
              onOpenTask={jumpToTask}
            />
          ) : view === "board" ? (
            <TaskTimelinePage
              tasks={visibleTasks}
              onNewTask={addTask}
              onUpdate={updateTask}
              onSetColumn={setTaskColumn}
              onOpenTask={jumpToTaskId}
              onOpenArchived={jumpToArchive}
            />
          ) : view === "schedule" ? (
            <SchedulePage onOpenChat={openChat} />
          ) : view === "activity" ? (
            <ActivityPage onOpenChat={openChat} />
          ) : view === "graph" ? (
            <GraphPage
              tasks={tasks}
              onOpenTask={jumpToTask}
              onPatchTask={updateTask}
            />
          ) : view === "archive" ? (
            <ArchivePage
              tasks={visibleTasks}
              onUpdate={updateTask}
              onDelete={deleteTask}
              onOpenSubtaskCard={jumpToTaskId}
              highlightTaskId={archiveFocus}
              onClearHighlight={() => setArchiveFocus(null)}
            />
          ) : view === "workspace" ? (
            <WorkspacePage />
          ) : view === "apps" ? (
            <AppsPage />
          ) : view === "trash" ? (
            <TrashPage
              tasks={visibleTasks}
              onUpdate={updateTask}
              onDelete={hardDeleteTask}
              onOpenSubtaskCard={jumpToTaskId}
            />
          ) : view === "notifications" ? (
            <NotificationsPage />
          ) : null}
          {/* 聊天常挂载：execute-task / chat-open-session / chat-focus-session /
              subagent-finished 全靠组件内监听器驱动，条件挂载会让切走页面期间的
              事件无处可去（挂件时代的「折叠不丢聊天」诉求，同解）。
              切视图只切可见性，messages 与监听器全程存活。 */}
          <div className={view === "chat" ? "h-full" : "hidden"}>
            <ChatPanel
              enabled={botOn}
              tasks={openTasks}
              onUpdateTask={updateTask}
            />
          </div>
          {/* 全局确认弹窗 */}
          <ConfirmMap />
          {/* 应用内错误弹窗宿主（批 4：接管 errorHandler，替代原生 alert/confirm） */}
          <ErrorDialogHost />
        </main>
        {/* 全局任务编辑浮窗：任何页面上层滑出（新建/编辑统一入口） */}
        {editor && editorTask && (
          <TaskEditorOverlay
            task={editorTask}
            autoFocusTitle={editor.autoFocus}
            closing={editor.closing}
            onClosed={dismissEditor}
            onClose={closeEditor}
            onDiscard={discardEditor}
            onUpdate={(patch) => updateTask(editorTask.id, patch)}
            onSetColumn={(col) => setTaskColumn(editorTask.id, col)}
            onDelete={() => {
              closeEditor();
              deleteTask(editorTask.id);
            }}
            onCreateSubtaskCard={(subId, title) =>
              createSubtaskCard(editorTask.id, subId, title)
            }
            onOpenTask={jumpToTaskId}
            onDeleteSubtask={(subId) => deleteSubtaskRow(editorTask.id, subId)}
          />
        )}
        {paletteOpen && (
          <CommandPalette
            onClose={() => setPaletteOpen(false)}
            tasks={visibleTasks}
            onJumpTask={jumpToTask}
            onJumpSession={jumpToSession}
          />
        )}
      </div>
    </ErrorBoundary>
  );
}

export default App;
