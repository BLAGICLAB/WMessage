// ChatPanel 子模块：计划面板的状态源。
//
// 两条口径缺一不可：
// - `bot-plan` 事件是变化流（实时订阅与回放补齐共用同一批处理器，写进
//   planStore）；
// - `bot_plan_state` 命令是当前快照——面板晚挂载（开面板之前计划就更新过）
//   会错过事件那一条，所以挂载 / 切会话时用命令拉一次；
// 两条都比 updatedAt，旧的不许盖新的（判据单源，在 planStore 里）。
//
// 状态放模块级 store 而不放组件 state：浮层是条件挂载的，状态放组件里，
// 关着时发生的更新没人收，且实时与回放两条来路会各写各的。
//
// 「状态可能已过期」由 turnStartedAt 判：本轮开始之后模型没再更新过计划，
// 面板上的 in_progress 就不该继续冒充实时状态——进度是模型自报，模型不报
// 就不等于没在做。

import { useEffect, useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import * as planStore from "./planStore";
import type { PlanView } from "./planStore";

/** 订阅回调（模块级，引用稳定）：store 变了就通知 React，不反复重订阅 */
function subscribeStore(cb: () => void): () => void {
  return planStore.subscribePlans(() => cb());
}

/** 本会话当前计划 + 「可能已过期」标记 */
export function usePlanState(sessionId: string | null) {
  // 计划状态在模块级 store 里（外部系统）：用 useSyncExternalStore 读它，
  // 不在 effect 里同步 setState 派生渲染值。按会话归位，别的会话的计划
  // 天然不会显示（切会话不闪旧数据）。
  const plan = useSyncExternalStore(
    subscribeStore,
    () => (sessionId ? (planStore.planOf(sessionId) ?? null) : null),
  );

  // 当前快照：命令是状态源，事件是变化流——面板晚挂载会错过事件那一条
  useEffect(() => {
    if (!sessionId) return;
    let alive = true;
    invoke<PlanView | null>("bot_plan_state", { sessionId })
      .then((v) => {
        if (!alive) return;
        planStore.applySnapshot(sessionId, v ?? null);
      })
      .catch(() => {
        // 取不到就当没有计划：面板不编一份空清单出来
        if (alive) planStore.applySnapshot(sessionId, null);
      });
    return () => {
      alive = false;
    };
  }, [sessionId]);

  const stale =
    !!plan &&
    plan.turnStartedAt != null &&
    plan.updatedAt < plan.turnStartedAt;
  return { plan, stale };
}
