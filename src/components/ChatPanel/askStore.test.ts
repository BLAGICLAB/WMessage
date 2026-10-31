import { describe, it, expect, beforeEach } from "vitest";
import * as askStore from "./askStore";

const ASK = {
  sessionId: "s1",
  questionId: "q1",
  question: "投放哪个平台？",
  why: "规范不同",
  options: ["公众号", "知乎"],
  assumption: "公众号",
};

beforeEach(() => {
  askStore.resetAll();
});

describe("askStore：提问落位", () => {
  it("applyAsk 按会话归位，asksOf 取得到", () => {
    expect(askStore.applyAsk(ASK)).toBe(true);
    const list = askStore.asksOf("s1");
    expect(list).toHaveLength(1);
    expect(list[0].question).toBe("投放哪个平台？");
    expect(list[0].options).toEqual(["公众号", "知乎"]);
    expect(list[0].why).toBe("规范不同");
    expect(list[0].assumption).toBe("公众号");
    expect(list[0].resolved).toBeUndefined();
  });

  it("缺会话号/问题 id/问题文本的载荷丢弃（信封盖章缺失 = 链路 bug）", () => {
    expect(askStore.applyAsk({ ...ASK, sessionId: null })).toBe(false);
    expect(askStore.applyAsk({ ...ASK, questionId: undefined })).toBe(false);
    expect(askStore.applyAsk({ ...ASK, question: undefined })).toBe(false);
    expect(askStore.asksOf("s1")).toHaveLength(0);
  });

  it("同 qid 幂等替换（回放与实时重叠投递同一条）", () => {
    askStore.applyAsk(ASK);
    askStore.applyAsk({ ...ASK, question: "改口径：发哪个平台？" });
    const list = askStore.asksOf("s1");
    expect(list).toHaveLength(1);
    expect(list[0].question).toBe("改口径：发哪个平台？");
  });

  it("非字符串选项剔除，不给卡片渲染脏数据", () => {
    askStore.applyAsk({ ...ASK, options: ["公众号", 42, null, "知乎"] });
    expect(askStore.asksOf("s1")[0].options).toEqual(["公众号", "知乎"]);
  });
});

describe("askStore：未答豁免（dead-man 看门狗判死豁免的依据）", () => {
  it("有未答卡 = true；终态并入后 = false；无卡会话 = false", () => {
    expect(askStore.hasUnresolvedAsk("s1")).toBe(false);
    askStore.applyAsk(ASK);
    expect(askStore.hasUnresolvedAsk("s1")).toBe(true);
    expect(
      askStore.applyResolved({ sessionId: "s1", questionId: "q1", action: "answer" }),
    ).toBe(true);
    expect(askStore.hasUnresolvedAsk("s1")).toBe(false);
    expect(askStore.hasUnresolvedAsk("s2")).toBe(false);
  });

  it("多问串行：前一张已答、后一张未答仍豁免；全部终态才解除", () => {
    askStore.applyAsk(ASK);
    askStore.applyAsk({ ...ASK, questionId: "q2" });
    expect(
      askStore.applyResolved({ sessionId: "s1", questionId: "q1", action: "assume" }),
    ).toBe(true);
    expect(askStore.hasUnresolvedAsk("s1")).toBe(true);
    expect(
      askStore.applyResolved({ sessionId: "s1", questionId: "q2", action: "answer" }),
    ).toBe(true);
    expect(askStore.hasUnresolvedAsk("s1")).toBe(false);
  });
});

describe("askStore：终态并入", () => {
  beforeEach(() => {
    askStore.applyAsk(ASK);
  });

  it("answer 终态带回答文本；assume/dismiss/expired 不带", () => {
    expect(
      askStore.applyResolved({
        sessionId: "s1",
        questionId: "q1",
        action: "answer",
        answer: "知乎，重点发长文",
      }),
    ).toBe(true);
    expect(askStore.asksOf("s1")[0].resolved).toEqual({
      action: "answer",
      answer: "知乎，重点发长文",
    });

    // 新问题走 assume：answer 缺省为 undefined
    askStore.applyAsk({ ...ASK, questionId: "q2" });
    expect(
      askStore.applyResolved({ sessionId: "s1", questionId: "q2", action: "assume" }),
    ).toBe(true);
    expect(askStore.asksOf("s1")[1].resolved).toEqual({
      action: "assume",
      answer: undefined,
    });
  });

  it("未知会话/未知问题不写；已有终态的卡不再改写（晚到重复终态无效）", () => {
    expect(
      askStore.applyResolved({ sessionId: "s-gone", questionId: "q1", action: "answer" }),
    ).toBe(false);
    expect(
      askStore.applyResolved({ sessionId: "s1", questionId: "q-gone", action: "answer" }),
    ).toBe(false);
    expect(
      askStore.applyResolved({ sessionId: "s1", questionId: "q1", action: "answer", answer: "第一次" }),
    ).toBe(true);
    expect(
      askStore.applyResolved({ sessionId: "s1", questionId: "q1", action: "answer", answer: "第二次" }),
    ).toBe(false);
    expect(askStore.asksOf("s1")[0].resolved?.answer).toBe("第一次");
  });
});

describe("askStore：清理与订阅", () => {
  it("空态引用稳定：getSnapshot 每次拿同一个空数组（返回新数组 = useSyncExternalStore 无限重渲染）", () => {
    expect(askStore.asksOf("s-none")).toBe(askStore.asksOf("s-none"));
  });

  it("clearResolved 只清答完的卡，未答的留着", () => {
    askStore.applyAsk(ASK);
    askStore.applyAsk({ ...ASK, questionId: "q2" });
    askStore.applyResolved({ sessionId: "s1", questionId: "q1", action: "answer", answer: "知乎" });
    expect(askStore.clearResolved("s1")).toBe(true);
    const list = askStore.asksOf("s1");
    expect(list).toHaveLength(1);
    expect(list[0].questionId).toBe("q2");
  });

  it("没有可清的卡时是 no-op", () => {
    askStore.applyAsk(ASK);
    expect(askStore.clearResolved("s1")).toBe(false);
    expect(askStore.clearResolved("s-gone")).toBe(false);
    expect(askStore.asksOf("s1")).toHaveLength(1);
  });

  it("dropSession 清掉该会话的卡并通知订阅者", () => {
    askStore.applyAsk(ASK);
    const seen: string[] = [];
    const unsub = askStore.subscribeAsks((sid) => seen.push(sid));
    askStore.dropSession("s1");
    unsub();
    expect(askStore.asksOf("s1")).toHaveLength(0);
    expect(seen).toContain("s1");
    expect(askStore.dropSession("s1")).toBeUndefined();
  });

  it("resetAll 清空全部（测试隔离用）", () => {
    askStore.applyAsk(ASK);
    askStore.applyAsk({ ...ASK, sessionId: "s2" });
    askStore.resetAll();
    expect(askStore.asksOf("s1")).toHaveLength(0);
    expect(askStore.asksOf("s2")).toHaveLength(0);
  });
});
