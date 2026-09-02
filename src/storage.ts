// 旧版 localStorage 键：方案B 起任务数据存 SQLite（wmessage.db），此键仅用于首次迁移
export const STORAGE_KEY = "***";

// —— 方案B（2026-08-14）：任务数据存 SQLite，行级增量读写 ——
import { invoke } from "@tauri-apps/api/core";
import { handleCommandError } from "./lib/errorHandler";
import type { Task } from "./types";

/** 结构相等（用于 diff 行级变更）。T1-1：expectedUpdatedAt 是写前比对基线（传输元数据，
 *  非内容），不参与比较——否则 state 残留的脏基线会击穿纯排序豁免 / 制造假变更 */
export const taskEq = (a: Task, b: Task) =>
  JSON.stringify({ ...a, expectedUpdatedAt: undefined }) ===
  JSON.stringify({ ...b, expectedUpdatedAt: undefined });

/** loadTasksFromDb 结果：区分「读失败」与「空库」，避免调用方把 error 当 empty 触发种子/迁移写入 */
export type LoadTasksResult =
  | { ok: true; tasks: Task[] }
  | { ok: false; error: unknown };

/** 全量读取（Rust 侧自动迁移旧 data.json）；失败返回错误载荷，由调用方决定如何展示 */
export async function loadTasksFromDb(): Promise<LoadTasksResult> {
  try {
    const tasks = await invoke<Task[]>("db_load");
    return { ok: true, tasks };
  } catch (e) {
    console.error("[db_load] read failed", e);
    return { ok: false, error: e };
  }
}

/**
 * 行级增量写入（INSERT OR REPLACE）。
 * 写失败不再静默吞错（E1 2026-08-19）：弹 alert 并把错误抛给调用方——
 * 否则 UI 已更新而磁盘没落，重启后 UI/DB 永久分叉。
 */
export async function upsertTasks(tasks: Task[]): Promise<void> {
  if (!tasks.length) return;
  try {
    await invoke("db_upsert", { tasks });
  } catch (e) {
    handleCommandError(e, "db_upsert");
    throw e;
  }
}

/** 行级删除（失败处理同 upsertTasks：alert + 传播，不静默） */
export async function deleteTaskRows(ids: string[]): Promise<void> {
  if (!ids.length) return;
  try {
    await invoke("db_delete", { ids });
  } catch (e) {
    handleCommandError(e, "db_delete");
    throw e;
  }
}

/** 导出任务卡数据为 JSON 文件（全量：含归档、回收站），返回条数 */
export async function exportTasksToFile(path: string): Promise<number> {
  try {
    return await invoke<number>("tasks_export", { path });
  } catch (e) {
    handleCommandError(e, "tasks_export");
    return 0;
  }
}

/** 从 JSON 文件导入任务卡数据：按 id 合并，同 id 保留最后修改更晚的。返回写入条数。 */
export async function importTasksFromFile(path: string): Promise<number> {
  try {
    return await invoke<number>("tasks_import", { path });
  } catch (e) {
    handleCommandError(e, "tasks_import");
    return 0;
  }
}

/** 按 order 稳定排序（旧数据无 order 时保持原相对顺序） */
export const sortByOrder = (tasks: Task[]) =>
  [...tasks].sort((a, b) => (a.order ?? 0) - (b.order ?? 0));

/**
 * 行级 diff + 打修改时间戳（主窗口 mutate 与挂件 applyAndSync 共用）。
 * upserts = 新增/变化行，deletes = 消失行 id；变化行打 updatedAt=now。
 * P2-20（2026-08-19）：纯排序变更（除 order 外无字段差异）保留原 updatedAt——
 * 否则拖拽排序把被重排行的 updatedAt 全刷成 now，多客户端按 updatedAt 合并时
 * 排序写互相覆盖，顺序来回乱跳。
 */
export function diffTaskRows(
  prev: Task[],
  next: Task[],
  now: number
): { upserts: Task[]; deletes: string[] } {
  const prevMap = new Map(prev.map((t) => [t.id, t]));
  const upserts = next.filter((t) => {
    const p = prevMap.get(t.id);
    return !p || !taskEq(p, t);
  });
  const nextIds = new Set(next.map((t) => t.id));
  const deletes = prev.filter((t) => !nextIds.has(t.id)).map((t) => t.id);
  upserts.forEach((t) => {
    const p = prevMap.get(t.id);
    // 纯 order 变更：不刷新 updatedAt（新任务 p 为 undefined，照常打戳）
    if (p && taskEq({ ...p, order: t.order }, t)) {
      // T1-1：纯排序也带基线（在 taskEq 之后设置，不参与内容比较）——
      // 排序写若撞上他端内容修改同样拒写，不用旧行整行压过去
      t.expectedUpdatedAt = p.updatedAt;
      return;
    }
    t.updatedAt = now;
    // T1-1：RMW 写回基线 = 快照行 updatedAt；新任务（无 prev）不带基线。
    // 注意必须在 taskEq 判定之后赋值：基线字段不参与「是否变化」比较，
    // 否则纯排序豁免会被脏基线击穿。
    t.expectedUpdatedAt = p?.updatedAt;
  });
  return { upserts, deletes };
}

// ───────────── 工作区（静态链接） ─────────────
import type { WorkspaceItem } from "./types";

/** 全量读取工作区条目 */
export async function loadWorkspaceFromDb(): Promise<WorkspaceItem[]> {
  try {
    return await invoke<WorkspaceItem[]>("workspace_load");
  } catch (e) {
    handleCommandError(e, "workspace_load", { silent: true });
    return [];
  }
}

/** 行级增量写入工作区条目（失败处理同 upsertTasks：alert + 传播，不静默） */
export async function upsertWorkspaceItems(items: WorkspaceItem[]): Promise<void> {
  if (!items.length) return;
  try {
    await invoke("workspace_upsert", { items });
  } catch (e) {
    handleCommandError(e, "workspace_upsert");
    throw e;
  }
}

/** 行级删除工作区条目（失败处理同 upsertTasks：alert + 传播，不静默） */
export async function deleteWorkspaceRows(ids: string[]): Promise<void> {
  if (!ids.length) return;
  try {
    await invoke("workspace_delete", { ids });
  } catch (e) {
    handleCommandError(e, "workspace_delete");
    throw e;
  }
}

/** 导出工作区链接数据为 JSON 文件（全量 WorkspaceItem），返回条数 */
export async function exportWorkspaceToFile(path: string): Promise<number> {
  try {
    return await invoke<number>("workspace_export", { path });
  } catch (e) {
    handleCommandError(e, "workspace_export");
    return 0;
  }
}

/** 从 JSON 文件导入工作区链接数据：按 id 合并，同 id 保留 updatedAt 更晚的。返回写入条数。 */
export async function importWorkspaceFromFile(path: string): Promise<number> {
  try {
    return await invoke<number>("workspace_import", { path });
  } catch (e) {
    handleCommandError(e, "workspace_import");
    return 0;
  }
}

/**
 * 给指定条目分配插入位 order：取新位置左右邻居的中点；
 * 边界取邻居 ±1；间隙耗尽（浮点精度）时全量整数重排。
 * 只有被拖条目生成新对象，配合 taskEq 只落盘变化行。
 * 任务卡与工作区条目共用（只需 id + order 字段）。
 */
export function assignInsertOrder<T extends { id: string; order?: number }>(
  arr: T[],
  activeId: string
): T[] {
  const idx = arr.findIndex((t) => t.id === activeId);
  if (idx === -1) return arr;
  const lo = idx > 0 ? arr[idx - 1].order : undefined;
  const hi = idx < arr.length - 1 ? arr[idx + 1].order : undefined;
  let order: number | null = null;
  if (lo !== undefined && hi !== undefined) {
    if (hi - lo > 1e-9) order = (lo + hi) / 2;
    else return arr.map((t, i) => ({ ...t, order: i }));
  } else if (lo !== undefined) {
    order = lo + 1;
  } else if (hi !== undefined) {
    order = hi - 1;
  } else {
    order = 0;
  }
  return arr.map((t, i) => (i === idx ? { ...t, order } : t));
}
