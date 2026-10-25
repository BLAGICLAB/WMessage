// ChatPanel 子模块：计划面板的状态源。
//
// `bot-plan` 事件是变化流，`bot_plan_state` 命令是当前快照——两条口径缺一不可：
// - 面板晚挂载（开面板之前计划就更新过）会错过事件那一条，所以挂载/切会话时
//   用命令拉一次当前状态；
// - 命令回包可能比先到的更新更旧（拉取途中模型又改了一次），两边都比
//   updatedAt，旧的不许盖新的。
//
// 「状态可能已过期」由 turnStartedAt 判：本轮开始之后模型没再更新过计划，
// 面板上的 in_progress 就不该继续冒充实时状态——进度是模型自报，模型不报
// 就不等于没在做。

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTauriListen } from "../../lib/useTauriListen";

export type PlanStepStatus = "pending" | "in_progress" | "completed";

/** 计划条目（随 PlanView 一起用，不单独导出） */
type PlanStep = {
  content: string;
  status: PlanStepStatus;
};

export type PlanView = {
  steps: PlanStep[];
  explanation?: string | null;
  updatedAt: number;
  turnStartedAt?: number | null;
};

/** 状态带上会话 id：切会话时靠它派生「不属于当前会话 → 不显示」，
 *  不用在 effect 里同步清 state（清 state 会多一次渲染）。 */
type PlanState = { sid: string | null; plan: PlanView | null };

/** 本会话当前计划 + 「可能已过期」标记 */
export function usePlanState(sessionId: string | null) {
  const [state, setState] = useState<PlanState>({ sid: null, plan: null });
  /** 最新一份计划：命令回包与事件都要跟它比新旧 */
  const latestRef = useRef<PlanView | null>(null);
  /** 当前会话 id 镜像：事件回调只捕获一次（useTauriListen 的约定），读 ref 拿当前值 */
  const sessionIdRef = useRef<string | null>(sessionId);
  useEffect(() => {
    sessionIdRef.current = sessionId;
  }, [sessionId]);

  const apply = useCallback((next: PlanView | null) => {
    const cur = latestRef.current;
    if (cur && next && next.updatedAt < cur.updatedAt) return;
    latestRef.current = next;
    setState({ sid: sessionIdRef.current, plan: next });
  }, []);

  // 切会话：丢掉上一会话的残留（ref 侧），再拉一次当前快照
  useEffect(() => {
    latestRef.current = null;
    if (!sessionId) return;
    let alive = true;
    invoke<PlanView | null>("bot_plan_state", { sessionId })
      .then((v) => {
        if (!alive) return;
        apply(v ?? null);
      })
      .catch(() => {
        // 取不到就当没有计划：面板不编一份空清单出来
        if (alive) apply(null);
      });
    return () => {
      alive = false;
    };
  }, [sessionId, apply]);

  // 实时事件：整表替换（后端口径），晚到的旧包不许盖新的
  useTauriListen<PlanView & { sessionId?: string | null }>("bot-plan", (p) => {
    const sid = p?.sessionId ?? null;
    if (!sid || sid !== sessionIdRef.current) return;
    apply({
      steps: Array.isArray(p.steps) ? p.steps : [],
      explanation: p.explanation ?? null,
      updatedAt: p.updatedAt ?? 0,
      turnStartedAt: p.turnStartedAt ?? null,
    });
  });

  // 别的会话的计划不显示（切会话瞬间旧数据不能闪一下）
  const plan = state.sid === sessionId ? state.plan : null;
  const stale =
    !!plan &&
    plan.turnStartedAt != null &&
    plan.updatedAt < plan.turnStartedAt;
  return { plan, stale };
}
