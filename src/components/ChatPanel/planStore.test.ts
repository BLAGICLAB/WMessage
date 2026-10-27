import { describe, it, expect, beforeEach } from "vitest";
import * as planStore from "./planStore";

beforeEach(() => {
  // 模块级 store 跨用例存活（面板重挂载要接得住），不复位会串味
  planStore.resetAll();
});

describe("计划状态 store", () => {
  it("事件按信封里的会话号归位：别的会话不受影响", () => {
    planStore.applyEvent({
      sessionId: "s1",
      steps: [{ content: "s1 的步骤", status: "in_progress" }],
      updatedAt: 100,
    });
    planStore.applyEvent({
      sessionId: "s2",
      steps: [{ content: "s2 的步骤", status: "pending" }],
      updatedAt: 200,
    });
    expect(planStore.planOf("s1")?.steps[0].content).toBe("s1 的步骤");
    expect(planStore.planOf("s2")?.steps[0].content).toBe("s2 的步骤");
  });

  it("信封缺会话号 → 丢弃：不知道归谁就不写", () => {
    const ok = planStore.applyEvent({
      steps: [{ content: "没有归属的步骤", status: "pending" }],
      updatedAt: 100,
    });
    expect(ok).toBe(false);
    expect(planStore.planOf("s1")).toBeUndefined();
  });

  it("整表替换：后端给什么就是什么，不做增量合并", () => {
    planStore.applyEvent({
      sessionId: "s1",
      steps: [
        { content: "第一步", status: "completed" },
        { content: "第二步", status: "in_progress" },
      ],
      updatedAt: 100,
    });
    planStore.applyEvent({
      sessionId: "s1",
      steps: [{ content: "第二步", status: "completed" }],
      updatedAt: 200,
    });
    expect(planStore.planOf("s1")?.steps).toHaveLength(1);
    expect(planStore.planOf("s1")?.steps[0].status).toBe("completed");
  });

  it("旧的不许盖新的：updatedAt 更小的快照不覆盖事件状态", () => {
    planStore.applyEvent({
      sessionId: "s1",
      steps: [{ content: "新状态", status: "in_progress" }],
      updatedAt: 300,
    });
    const ok = planStore.applySnapshot("s1", {
      steps: [{ content: "旧快照", status: "pending" }],
      updatedAt: 100,
      turnStartedAt: 50,
    });
    expect(ok).toBe(false);
    expect(planStore.planOf("s1")?.steps[0].content).toBe("新状态");
  });

  it("快照为 null 不清已有条目：回包在途中时模型又更新过，回包是旧的", () => {
    planStore.applyEvent({
      sessionId: "s1",
      steps: [{ content: "事件带来的计划", status: "in_progress" }],
      updatedAt: 300,
    });
    expect(planStore.applySnapshot("s1", null)).toBe(false);
    expect(planStore.planOf("s1")?.steps[0].content).toBe("事件带来的计划");
  });

  it("快照比事件新时覆盖它（命令是权威状态源）", () => {
    planStore.applyEvent({
      sessionId: "s1",
      steps: [{ content: "事件带来的计划", status: "in_progress" }],
      updatedAt: 100,
    });
    planStore.applySnapshot("s1", {
      steps: [{ content: "快照里的计划", status: "completed" }],
      updatedAt: 300,
      turnStartedAt: 50,
    });
    expect(planStore.planOf("s1")?.steps[0].content).toBe("快照里的计划");
  });

  it("订阅者收到变化通知（面板据此重渲染）", () => {
    const seen: string[] = [];
    const off = planStore.subscribePlans((sid) => seen.push(sid));
    planStore.applyEvent({
      sessionId: "s1",
      steps: [{ content: "第一步", status: "pending" }],
      updatedAt: 100,
    });
    off();
    planStore.applyEvent({
      sessionId: "s1",
      steps: [{ content: "第二步", status: "pending" }],
      updatedAt: 200,
    });
    expect(seen).toEqual(["s1"]);
  });

  it("删会话一并清掉计划（防同 id 重建后继承旧计划）", () => {
    planStore.applyEvent({
      sessionId: "s1",
      steps: [{ content: "旧计划", status: "pending" }],
      updatedAt: 100,
    });
    planStore.dropSession("s1");
    expect(planStore.planOf("s1")).toBeUndefined();
  });

  it("turnStartedAt 缺失时不判过期（没有本轮起点就没有过期可言）", () => {
    planStore.applyEvent({
      sessionId: "s1",
      steps: [{ content: "步骤", status: "in_progress" }],
      updatedAt: 100,
    });
    expect(planStore.planOf("s1")?.turnStartedAt ?? null).toBeNull();
  });
});
