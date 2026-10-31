// 提问卡状态的模块级 store（状态外置，与 planStore 同款做法）。
//
// 两条来路写同一份：`bot-ask`（提问）与 `bot-ask-resolved`（终态），实时订阅
// 与「回放补齐」共用同一批处理器——所以写入必须**整体替换**（原地改引用里的
// 对象会让 useSyncExternalStore 拿到同一个引用而不重渲染）。
//
// 提问以 questionId 幂等（回放与实时可能重叠投递同一条）；终态按 questionId
// 就地并入，已有终态的卡不再改写（晚到的重复终态无效）。
//
// 会话内同时至多一张活卡（提问阻塞模型轮，天然串行），但一轮预算内可能先后
// 多问——存为数组：答完的卡保留终态展示，回合收尾（bot-turn-status 终态）
// 由 clearResolved 统一清走。

type AskResolvedAction = "answer" | "assume" | "dismiss" | "expired";

/** 提问终态：用户作答（answer 带文本）/ 按假设继续 / 超时或停止未答 */
type AskResolved = {
  action: AskResolvedAction;
  answer?: string;
};

export type AskState = {
  questionId: string;
  question: string;
  why?: string;
  options: string[];
  assumption?: string;
  resolved?: AskResolved;
};

/** `bot-ask` 事件载荷（信封 sessionId 由后端统一盖章） */
export type AskEventPayload = {
  sessionId?: string | null;
  questionId?: string;
  question?: string;
  why?: string | null;
  options?: unknown;
  assumption?: string | null;
};

/** `bot-ask-resolved` 事件载荷 */
export type AskResolvedPayload = {
  sessionId?: string | null;
  questionId?: string;
  action?: string;
  answer?: string | null;
};

const asks = new Map<string, AskState[]>();
const listeners = new Set<(sid: string) => void>();

/** 空态共享同一实例：getSnapshot 返回新数组会让 useSyncExternalStore 无限重渲染 */
const EMPTY: AskState[] = [];

function notify(sid: string): void {
  for (const cb of listeners) cb(sid);
}

/** 某会话的提问卡列表（问完即留终态，回合收尾清走）。 */
export function asksOf(sid: string): AskState[] {
  return asks.get(sid) ?? EMPTY;
}

/** 该会话是否存在未答的提问卡。
 *
 * dead-man 看门狗据此豁免判死：提问以小时计（后端 24h 上限），等待应答
 * 期间零流式事件是**合法静默**，不能按「后端挂起」回收。豁免随终态解除
 * （后端保证 resolved 必发：应答、超时、停止都走终态事件）。 */
export function hasUnresolvedAsk(sid: string): boolean {
  return (asks.get(sid) ?? EMPTY).some((a) => !a.resolved);
}

/** 应用一条 `bot-ask`：按信封会话号归位，同 qid 幂等替换。缺关键字段丢弃。 */
export function applyAsk(payload: AskEventPayload): boolean {
  const sid = payload?.sessionId;
  const qid = payload?.questionId;
  const question = payload?.question;
  if (!sid || !qid || !question) return false;
  const list = asks.get(sid) ?? [];
  const card: AskState = {
    questionId: qid,
    question,
    why: payload.why ?? undefined,
    options: Array.isArray(payload.options)
      ? payload.options.filter((o): o is string => typeof o === "string")
      : [],
    assumption: payload.assumption ?? undefined,
  };
  const idx = list.findIndex((a) => a.questionId === qid);
  const next =
    idx >= 0
      ? list.map((a, i) => (i === idx ? card : a))
      : [...list, card];
  asks.set(sid, next);
  notify(sid);
  return true;
}

/** 应用一条 `bot-ask-resolved`：按 qid 并入终态（未知卡/已有终态不写）。 */
export function applyResolved(payload: AskResolvedPayload): boolean {
  const sid = payload?.sessionId;
  const qid = payload?.questionId;
  const action = payload?.action;
  if (!sid || !qid || !action) return false;
  const list = asks.get(sid);
  if (!list) return false;
  const card = list.find((a) => a.questionId === qid);
  if (!card || card.resolved) return false;
  const resolved: AskResolved = {
    action: action as AskResolvedAction,
    answer: payload.answer ?? undefined,
  };
  asks.set(
    sid,
    list.map((a) => (a.questionId === qid ? { ...a, resolved } : a)),
  );
  notify(sid);
  return true;
}

/** 回合收尾：答完（已有终态）的卡清走；未答的卡不动（正常不会有，防御保留）。 */
export function clearResolved(sid: string): boolean {
  const list = asks.get(sid);
  if (!list) return false;
  const next = list.filter((a) => !a.resolved);
  if (next.length === list.length) return false;
  if (next.length === 0) asks.delete(sid);
  else asks.set(sid, next);
  notify(sid);
  return true;
}

/** 订阅提问变化（UI 侧据此重渲染）。 */
export function subscribeAsks(cb: (sid: string) => void): () => void {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
}

/** 删会话时一并清干净（提问卡挂在会话上）。 */
export function dropSession(sid: string): void {
  if (!asks.delete(sid)) return;
  notify(sid);
}

/**
 * 清空全部状态（**测试用**）。
 *
 * 模块级 store 跨用例存活是它的设计（面板重挂载要接得住），但测试里每个用例
 * 都重新渲染组件：不复位的话，上一例留下的提问会改写下一例的渲染。
 */
export function resetAll(): void {
  if (asks.size === 0) return;
  const sids = [...asks.keys()];
  asks.clear();
  for (const cb of listeners) for (const sid of sids) cb(sid);
}
