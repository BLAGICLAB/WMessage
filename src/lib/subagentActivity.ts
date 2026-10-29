// 子 agent 动作流（右侧栏实时可见性）：dispatch 在 call / return 两相位旁路
// `subagent-activity` 事件，这里把它们合并成「一行一条动作」的视图模型。
// 事件即焚不落库——历史回看走详情面板的结果与 digest。

/** dispatch 旁路的单条事件（call 或 return 相位） */
export type SubagentActivityEvent = {
  subagentId: string;
  turn?: number | null;
  toolCallId?: string | null;
  tool: string;
  phase: "call" | "return";
  ok?: boolean | null;
  ms?: number | null;
  preview?: string | null;
  usedToolCalls?: number | null;
};

/** 合并后的动作行：return 到达即有结局，call 未收口 = 进行中 */
export type SubagentActivityItem = {
  toolCallId: string | null;
  tool: string;
  /** null = call 已上屏但还没有结局（进行中） */
  ok: boolean | null;
  ms: number | null;
  preview: string;
  at: number;
};

/** 每个子 agent 保留的最近动作条数：详情页的「执行详情」窗口按条滚动回看，
 *  默认工具调用预算就是 100 次，留满 100 条整趟每一步都在窗里；仍是事件即焚
 *  不落库——历史会话回看走详情面板的结果与 digest。 */
export const ACTIVITY_MAX_ITEMS = 100;

function eventToItem(e: SubagentActivityEvent, now: number): SubagentActivityItem {
  const done = e.phase === "return";
  return {
    toolCallId: e.toolCallId ?? null,
    tool: e.tool,
    ok: done ? (e.ok ?? false) : null,
    ms: done ? (e.ms ?? null) : null,
    preview: e.preview ?? "",
    at: now,
  };
}

/** 把一条事件并入某子 agent 的动作列表（纯函数）：
 *  - call：新行入列头（进行中）
 *  - return：按 toolCallId 收口同一条 call 行；没有对应 call 的（如乱序/重启）
 *    直接入列头当一条已完成动作
 *  - 列表恒持最近 ACTIVITY_MAX_ITEMS 条，新在头 */
export function applyActivityEvent(
  list: SubagentActivityItem[],
  event: SubagentActivityEvent,
  now = Date.now(),
): SubagentActivityItem[] {
  const item = eventToItem(event, now);
  if (event.phase === "return" && item.toolCallId) {
    const idx = list.findIndex((a) => a.toolCallId === item.toolCallId);
    if (idx >= 0) {
      const next = list.slice();
      // 保留 call 行的位置（列表里动作发生的先后），只补结局
      next[idx] = { ...item, at: list[idx].at };
      return next;
    }
  }
  return [item, ...list].slice(0, ACTIVITY_MAX_ITEMS);
}
