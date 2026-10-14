// 任务可见域谓词单源：归档/回收站/进行区/完成列的组合判断原先散在
// 时间线/挂件/首页/归档页/回收站/图谱各写一份，口径漂移无从发现。
// 细颗粒拆分是刻意的：各页面的业务口径本就不同（到期看 completedAt、
// 时间线看 column），组合逻辑留在调用方，这里只供原子谓词。
import type { Task } from "../types";

/** 回收站口径：软删未清（回收站页挂载 / 各处「未删」前提） */
export function isTrashed(t: Task): boolean {
  return !!t.deletedAt;
}

/** 进行区口径：未删且未归档（时间网格 / 挂件 / 首页活卡的前提） */
export function isLive(t: Task): boolean {
  return !t.archived && !isTrashed(t);
}

/** 归档区口径：已归档且未删（归档页挂载 / 归档脉动统计） */
export function isArchived(t: Task): boolean {
  return !!t.archived && !isTrashed(t);
}

/** 待办卡口径：进行区且未进完成列（时间线待办池 / 挂件待办列表） */
export function isActiveTask(t: Task): boolean {
  return isLive(t) && t.column !== "done";
}

/** 完成卡口径：进行区且已进完成列（时间线完成池 / 挂件完成列表，原位保留） */
export function isDoneTask(t: Task): boolean {
  return isLive(t) && t.column === "done";
}

/** 可催办口径：进行区且无完成时间戳（首页到期轮播）——按 completedAt 判活，
 *  与完成列口径刻意不同：登记了完成时间的卡即使还在待办列也不再催 */
export function isPending(t: Task): boolean {
  return isLive(t) && !t.completedAt;
}
