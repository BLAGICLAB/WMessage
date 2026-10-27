// 执行计划状态的模块级 store（状态外置）。
//
// 为什么搬到组件外：计划浮层是**条件挂载**的（关掉即卸载），状态放组件里
// 有两个坏处：
// - 关着的时候发生的更新没人收，开面板得重新问后端一遍才闪一下；
// - 实时订阅与「回放补齐」两条来路各写各的 state，晚到的那份能把新的盖掉。
// 搬出组件后两条来路写同一份，按 updatedAt 比新旧（与 streamState 同款做法）。
//
// 口径与后端一致：**整表替换**（后端给什么就是什么，不做增量合并）。
//
// 单一写入口：
// - applyEvent：`bot-plan` 事件载荷（实时与回放补齐共用同一批处理器）
// - applySnapshot：`bot_plan_state` 命令回包（面板挂载 / 切会话时拉当前状态）
//
// 单调闸：命令回包可能比先到的更新更旧（拉取途中模型又改了一次），
// 旧的不许盖新的。

export type PlanStepStatus = "pending" | "in_progress" | "completed";

/** 计划条目（后端整表替换，前端不做增量） */
type PlanStep = {
  content: string;
  status: PlanStepStatus;
};

/** 本会话当前计划 */
export type PlanView = {
  steps: PlanStep[];
  explanation?: string | null;
  updatedAt: number;
  turnStartedAt?: number | null;
};

/** `bot-plan` 事件载荷（信封字段 sessionId 由后端统一盖章） */
export type PlanEventPayload = {
  sessionId?: string | null;
  steps?: PlanStep[];
  explanation?: string | null;
  updatedAt?: number;
  turnStartedAt?: number | null;
};

const plans = new Map<string, PlanView>();
const listeners = new Set<(sid: string) => void>();

/** 某会话当前计划（没有则 undefined）。 */
export function planOf(sid: string): PlanView | undefined {
  return plans.get(sid);
}

/**
 * 应用一条 `bot-plan` 事件：按信封里的会话号归位，整表替换。
 *
 * 信封缺会话号 → 不知道该归谁，丢弃（后端统一出口盖章，缺了说明链路有 bug）。
 * 返回是否真的写进去了（比现有的旧 → 不写）。
 */
export function applyEvent(payload: PlanEventPayload): boolean {
  const sid = payload?.sessionId ?? null;
  if (!sid) return false;
  return write(sid, {
    steps: Array.isArray(payload.steps) ? payload.steps : [],
    explanation: payload.explanation ?? null,
    updatedAt: payload.updatedAt ?? 0,
    turnStartedAt: payload.turnStartedAt ?? null,
  });
}

/**
 * 应用命令回包（当前快照）。
 *
 * 「没有计划」不回写：回包在途中时模型又更新过一次，这时回包是旧的——
 * 拿它清空就把刚到的更新抹掉了。后端计划表本身只增不清（会话删除才清），
 * 所以快照为 null 时 store 里若有条目，那条目来自事件，比回包新。
 */
export function applySnapshot(sid: string, view: PlanView | null): boolean {
  if (!view) return false;
  return write(sid, view);
}

/** 写一份计划：比现有的旧就不许盖新的。 */
function write(sid: string, view: PlanView): boolean {
  const cur = plans.get(sid);
  if (cur && view.updatedAt < cur.updatedAt) return false;
  plans.set(sid, view);
  for (const cb of listeners) cb(sid);
  return true;
}

/** 订阅计划变化（UI 侧据此重渲染）。 */
export function subscribePlans(cb: (sid: string) => void): () => void {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
}

/** 删会话时一并清干净（计划挂在会话上）。 */
export function dropSession(sid: string): void {
  if (!plans.delete(sid)) return;
  for (const cb of listeners) cb(sid);
}

/**
 * 清空全部状态（**测试用**）。
 *
 * 模块级 store 跨用例存活是它的设计（面板重挂载要接得住），但测试里每个用例
 * 都重新渲染面板：不复位的话，上一例留下的计划会改写下一例的渲染。
 */
export function resetAll(): void {
  if (plans.size === 0) return;
  const sids = [...plans.keys()];
  plans.clear();
  for (const cb of listeners) for (const sid of sids) cb(sid);
}
