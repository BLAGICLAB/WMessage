// 流式在途态的模块级 store（状态外置）。
//
// 为什么搬到组件外：草稿那次的同构先例（chatDrafts.ts）——组件 state 里的东西
// 随组件一起没了。流式在途态是「后端还在跑」这个事实的**一个视图**，客户端
// 只是观察者：搬出组件后面板重挂载能立刻重建视图，不必等一次往返去问后端。
//
// 更要紧的是**游标必须与状态同生共死**：游标记的是「本会话已应用到哪一号
// 事件」，它跑在状态前面，补齐就会跳过本该重建的那一段（接出来的是半截）。
// 两者分居两处（一个在组件、一个在模块）时，重挂载后就会出现「游标说已经
// 到 120、内容却只有 30」的错位——所以它们放在同一处，一起生一起死。
//
// 三份状态同源：
// - meta：某会话这一轮已收到的内容（正文 / 思考 / 工具行 / 回合状态）
// - inflight：哪些会话正在跑（驱动 UI 与事件过滤）
// - cursor：每会话已应用到的最大事件序号（去重闸：补齐与实时可能重叠）

import { invoke } from "@tauri-apps/api/core";
import type {
  FileChangeLite,
  Msg,
  SkillFailure,
  ToolCall,
  TurnStatus,
} from "./types";

/** 某会话这一轮的流式内容（与气泡上的字段同构）。 */
export type StreamMeta = {
  /** 本轮已流出的正文 */
  text?: string;
  /** 本轮已流出的思考过程 */
  thinking?: string;
  /** 工具行 */
  tools?: ToolCall[];
  /** 技能失败行（半成品 + 回滚状态） */
  skillFailure?: SkillFailure;
  /** 文件变更 */
  fileChanges?: FileChangeLite[];
  /** 回合状态机 */
  turnStatus?: TurnStatus;
  turnDetail?: string;
  /** 本轮起点（毫秒时间戳） */
  turnStartedAt?: number;
  /** 本轮的用户消息 */
  userMsg?: Msg;
};

/** 回放里的一条事件（与实时广播同构，另带序号）。 */
type ReplayEvent = {
  seq: number;
  event: string;
  payload: Record<string, unknown>;
};

/** 回放结果（后端 `bot_stream_replay`）。 */
export type StreamReplay = {
  events: ReplayEvent[];
  lastSeq: number;
  /** 游标老于缓冲窗口：中间有洞，得从权威快照（历史）重建 */
  truncated: boolean;
  /** 该会话此刻是否仍在跑 */
  inflight: boolean;
};

/** 在途会话摘要（后端 `bot_inflight_list`）。 */
export type InflightRun = {
  sessionId: string;
  startedAt: number;
  lastSeq: number;
  eventCount: number;
};

const metas = new Map<string, StreamMeta>();
const inflight = new Set<string>();
const cursors = new Map<string, number>();
/** 回灌过的会话：这轮不是本窗口发起的，结束时没有 invoke 结果可等 */
const resumed = new Set<string>();
/** 正在回灌的会话：同一会话的两次回灌不能并发（会重复灌一遍） */
const resuming = new Set<string>();
/** 本窗口 invoke 发起的轮：终态清场权在 runChat（收尾要从权威副本并
 *  thinking/tools），终态事件不得代 drop——后端发起轮（围观执行会话 /
 *  回灌轮）没有 invoke 可等，才归终态事件清场 */
const invoked = new Set<string>();
/** 每会话最近一次流式活动时刻（dead-man 看门狗读它） */
const activity = new Map<string, number>();
const inflightListeners = new Set<(sids: ReadonlySet<string>) => void>();

/** 某会话的流式内容（没有则 undefined）。 */
export function metaOf(sid: string): StreamMeta | undefined {
  return metas.get(sid);
}

/** 某会话的流式内容（没有就建一个空的）。 */
export function ensureMeta(sid: string): StreamMeta {
  let m = metas.get(sid);
  if (!m) {
    m = {};
    metas.set(sid, m);
  }
  return m;
}

/** 覆盖某会话的流式内容（新一轮开始时清空用）。 */
export function setMeta(sid: string, meta: StreamMeta): void {
  metas.set(sid, meta);
}

/** 删会话时一并清干净（meta / 游标 / 在途标记 / 回灌标记）。 */
export function dropSession(sid: string): void {
  metas.delete(sid);
  cursors.delete(sid);
  inflight.delete(sid);
  resumed.delete(sid);
  invoked.delete(sid);
  activity.delete(sid);
  notifyInflight();
}

/** 该会话是否正在跑。 */
export function isInflight(sid: string): boolean {
  return inflight.has(sid);
}

/** 标记某会话在途（发消息时置位 / 回灌时补上）。 */
export function markInflight(sid: string): void {
  if (!activity.has(sid)) activity.set(sid, Date.now());
  if (inflight.has(sid)) return;
  inflight.add(sid);
  notifyInflight();
}

/** 标记某会话收尾（invoke 落地 / 终态事件到达时清）。 */
export function markFinished(sid: string): void {
  if (!inflight.delete(sid)) return;
  notifyInflight();
}

/** 当前在途的会话集合。 */
export function inflightSessions(): ReadonlySet<string> {
  return new Set(inflight);
}

/** 订阅在途集合变化（UI 侧据此渲染停止键 / 占位文案）。 */
export function subscribeInflight(
  cb: (sids: ReadonlySet<string>) => void,
): () => void {
  inflightListeners.add(cb);
  return () => {
    inflightListeners.delete(cb);
  };
}

function notifyInflight(): void {
  const sids = new Set(inflight);
  for (const cb of inflightListeners) cb(sids);
}

/**
 * 序号去重闸：返回 false = 这条已经应用过，调用方应当跳过。
 *
 * 补齐与实时会重叠——客户端拉回放的往返里，实时事件还在到达，同一条事件
 * 两边各来一次。按序号判重是唯一可靠的办法（内容判重做不到：正文分片可能
 * 内容相同但属于不同位置）。
 *
 * 无序号的（老后端 / 非会话事件）一律放行。
 */
export function noteSeq(sid: string, seq: number | undefined | null): boolean {
  // 活动时刻先于去重闸记：被闸挡掉的重复事件同样证明「还活着」
  activity.set(sid, Date.now());
  if (seq === undefined || seq === null) return true;
  const cur = cursors.get(sid) ?? 0;
  if (seq <= cur) return false;
  cursors.set(sid, seq);
  return true;
}

/** 某会话最近一次流式活动时刻（从未活动过 = 0）。dead-man 看门狗读它。 */
export function lastActivityOf(sid: string): number {
  return activity.get(sid) ?? 0;
}

/** 测试用：把某会话的最近活动时刻拨回去（看门狗用例要造「沉默超窗」）。 */
export function rewindActivityForTest(sid: string, ms: number): void {
  const cur = activity.get(sid) ?? Date.now();
  activity.set(sid, cur - ms);
}

/** 标记某会话这一轮由本窗口 invoke 发起（终态清场权归 runChat）。 */
export function markInvoked(sid: string): void {
  invoked.add(sid);
}

/** 该会话这一轮是否本窗口 invoke 发起的。 */
export function isInvoked(sid: string): boolean {
  return invoked.has(sid);
}

/** 该会话已应用到的最大序号（拉回放时的游标）。 */
export function seqOf(sid: string): number {
  return cursors.get(sid) ?? 0;
}

/** 游标归零（回灌整轮前用：那一轮的内容要整段重建）。 */
export function resetCursor(sid: string): void {
  cursors.delete(sid);
}

/** 标记某会话是「回灌接回」的——这轮不是本窗口发起的，结束时没有 invoke 结果可等。 */
export function markResumed(sid: string): void {
  resumed.add(sid);
}

/** 该会话是不是回灌接回的。 */
export function isResumed(sid: string): boolean {
  return resumed.has(sid);
}

/** 回灌标记用完即清（终态处理过一次就不该再按回灌路径走）。 */
export function clearResumed(sid: string): void {
  resumed.delete(sid);
}

/** 该会话是否正在回灌（并发回灌会把这一轮灌两遍）。 */
export function isResuming(sid: string): boolean {
  return resuming.has(sid);
}

/** 标记回灌开始（与 isResuming 配对，判定与置位必须连着做，中间不能有 await）。 */
export function markResuming(sid: string): void {
  resuming.add(sid);
}

/** 回灌结束。 */
export function clearResuming(sid: string): void {
  resuming.delete(sid);
}

/** 拉某会话的流式事件回放（afterSeq=0 = 要本轮全部）。 */
export async function fetchReplay(
  sessionId: string,
  afterSeq = 0,
): Promise<StreamReplay> {
  return invoke<StreamReplay>("bot_stream_replay", { sessionId, afterSeq });
}

/** 在途会话列表（谁正在跑）。 */
export async function fetchInflightRuns(): Promise<InflightRun[]> {
  const rows = await invoke<InflightRun[] | null>("bot_inflight_list");
  return Array.isArray(rows) ? rows : [];
}

/**
 * 清空全部状态（**测试用**）。
 *
 * 模块级 store 跨用例存活是它的设计（面板重挂载要接得住），但测试里每个用例
 * 都重新渲染面板：不复位的话，上一例留下的「在途」标记会改写下一例的渲染
 * （占位文案从「和机器人说点什么」变成「回复中…」，停止键也多出来）。
 */
export function resetAll(): void {
  metas.clear();
  inflight.clear();
  cursors.clear();
  resumed.clear();
  resuming.clear();
  invoked.clear();
  activity.clear();
  notifyInflight();
}
