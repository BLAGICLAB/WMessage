// 「不保存关闭」的回滚口径：把任务卡写回打开浮窗那一刻的值。
//
// 只回写与打开时不同的字段——整行回写会连带抹掉这段时间里机器人的写入
// （那不是用户改的，回滚它等于丢数据）。
//
// 代价：浮窗开着时页面仍可操作，字段级归因做不到——期间机器人/规则改过的
// 同一个字段也会被写回（机器人执行结果另有 result 字段兜底，不靠备注）。
//
// botAssigned 不在集合里：交给机器人是不可逆的外发动作（机器人已经在跑），
// 回滚这个标记只会让界面谎称「没派出去」。

import type { Task } from "../types";

/** 值相等判定：undefined 与 null 同义（后端两者都是「空」） */
const same = (a: unknown, b: unknown): boolean =>
  JSON.stringify(a ?? null) === JSON.stringify(b ?? null);

/**
 * 浮窗可改字段的差异补丁：before = 打开浮窗时的行，after = 当前行。
 * 空补丁 = 期间没改过任何字段（不需要回滚）。
 *
 * 字段口径与详情面板能改的字段一一对应；completedAt/archived 跟着 column 走
 * （标记完成的落库语义会连带清这两个）。
 */
export function revertPatch(before: Task, after: Task): Partial<Task> {
  const patch: Partial<Task> = {};
  if (before.title !== after.title) patch.title = before.title;
  if (!same(before.note, after.note)) patch.note = before.note;
  if (!same(before.tags, after.tags)) patch.tags = before.tags;
  if (!same(before.subtasks, after.subtasks)) patch.subtasks = before.subtasks;
  if (!same(before.files, after.files)) patch.files = before.files;
  if (!same(before.planStart, after.planStart))
    patch.planStart = before.planStart;
  if (!same(before.planEnd, after.planEnd)) patch.planEnd = before.planEnd;
  if (!same(before.due, after.due)) patch.due = before.due;
  if (before.column !== after.column) patch.column = before.column;
  if (!same(before.completedAt, after.completedAt))
    patch.completedAt = before.completedAt;
  if (!same(before.archived, after.archived)) patch.archived = before.archived;
  return patch;
}
